use std::{path::PathBuf, time::Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};

use crate::{
    backend::{Backend, StreamEvent},
    session::{Message, Role, Session},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    Info,
    Error,
}

pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub takes_argument: bool,
}

impl CommandSpec {
    pub fn display_name(self) -> String {
        if self.takes_argument {
            format!("{} [name]", self.name)
        } else {
            self.name.to_owned()
        }
    }

    fn completion(self) -> String {
        if self.takes_argument {
            format!("{} ", self.name)
        } else {
            self.name.to_owned()
        }
    }
}

pub const COMMANDS: [CommandSpec; 4] = [
    CommandSpec {
        name: "/help",
        description: "Show keys and commands",
        takes_argument: false,
    },
    CommandSpec {
        name: "/new",
        description: "Start a fresh session",
        takes_argument: false,
    },
    CommandSpec {
        name: "/model",
        description: "Show or change the active model",
        takes_argument: true,
    },
    CommandSpec {
        name: "/quit",
        description: "Save and exit",
        takes_argument: false,
    },
];

struct ActiveRequest {
    id: u64,
    started: Instant,
}

pub enum Action {
    None,
    Start {
        request_id: u64,
        messages: Vec<Message>,
    },
    Cancel(u64),
    Quit,
}

#[derive(Default)]
pub struct Input {
    value: String,
    cursor: usize,
}

impl Input {
    pub fn before_cursor(&self) -> &str {
        &self.value[..self.cursor]
    }

    pub fn with_cursor(&self) -> String {
        let mut value = self.value.clone();
        value.insert(self.cursor, '▌');
        value
    }

    fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    fn set(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.len();
    }

    fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.value)
    }

    fn insert(&mut self, character: char) {
        self.value.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn insert_str(&mut self, value: &str) {
        let value = value.replace("\r\n", "\n").replace('\r', "\n");
        let clean = value
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .collect::<String>();
        self.value.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    fn backspace(&mut self) {
        if let Some((index, _)) = self.value[..self.cursor].char_indices().next_back() {
            self.value.remove(index);
            self.cursor = index;
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    fn left(&mut self) {
        self.cursor = self.value[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
    }

    fn right(&mut self) {
        self.cursor = self.value[self.cursor..]
            .chars()
            .next()
            .map_or(self.value.len(), |character| {
                self.cursor + character.len_utf8()
            });
    }

    fn delete_word(&mut self) {
        let before = &self.value[..self.cursor];
        let without_space = before.trim_end();
        let start = without_space
            .char_indices()
            .rev()
            .find(|(_, character)| character.is_whitespace())
            .map(|(index, character)| index + character.len_utf8())
            .unwrap_or(0);
        self.value.replace_range(start..self.cursor, "");
        self.cursor = start;
    }
}

pub struct App {
    pub session: Session,
    pub backend: Backend,
    pub input: Input,
    pub scroll: usize,
    pub input_scroll: usize,
    pub transcript_height: u16,
    pub follow_tail: bool,
    pub show_help: bool,
    pub should_quit: bool,
    pub notice: Option<Notice>,
    pub spinner: usize,
    active: Option<ActiveRequest>,
    next_request_id: u64,
    session_path: Option<PathBuf>,
    palette_index: usize,
    palette_hidden: bool,
}

impl App {
    pub fn new(session: Session, backend: Backend, session_path: Option<PathBuf>) -> Self {
        Self {
            session,
            backend,
            input: Input::default(),
            scroll: 0,
            input_scroll: 0,
            transcript_height: 10,
            follow_tail: true,
            show_help: false,
            should_quit: false,
            notice: None,
            spinner: 0,
            active: None,
            next_request_id: 0,
            session_path,
            palette_index: 0,
            palette_hidden: false,
        }
    }

    pub fn is_streaming(&self) -> bool {
        self.active.is_some()
    }

    pub fn active_request_id(&self) -> Option<u64> {
        self.active.as_ref().map(|active| active.id)
    }

    pub fn elapsed_seconds(&self) -> Option<f64> {
        self.active
            .as_ref()
            .map(|active| active.started.elapsed().as_secs_f64())
    }

    pub fn palette_matches(&self) -> Vec<&'static CommandSpec> {
        if self.palette_hidden {
            return Vec::new();
        }
        matching_commands(&self.input.value)
    }

    pub fn palette_selected(&self) -> usize {
        let count = self.palette_matches().len();
        if count == 0 {
            0
        } else {
            self.palette_index.min(count - 1)
        }
    }

    fn palette_open(&self) -> bool {
        !self.palette_matches().is_empty()
    }

    fn reset_palette(&mut self) {
        self.palette_index = 0;
        self.palette_hidden = false;
    }

    fn palette_move(&mut self, forward: bool) {
        let count = self.palette_matches().len();
        if count == 0 {
            return;
        }
        let current = self.palette_index.min(count - 1);
        self.palette_index = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
    }

    fn palette_complete(&mut self) -> Option<CommandSpec> {
        let matches = self.palette_matches();
        let index = self.palette_index.min(matches.len().saturating_sub(1));
        let command = *matches.get(index).copied()?;
        self.input.set(command.completion());
        self.palette_index = 0;
        self.palette_hidden = true;
        Some(command)
    }

    fn palette_accept(&mut self) -> Action {
        if self.palette_complete().is_some() {
            self.submit()
        } else {
            Action::None
        }
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
    }

    pub fn handle_paste(&mut self, value: &str) {
        if !self.show_help {
            self.input.insert_str(value);
            self.reset_palette();
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(3),
            MouseEventKind::ScrollDown => self.scroll_down(3),
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
        {
            self.should_quit = true;
            return Action::Quit;
        }
        if key.code == KeyCode::F(1) {
            self.show_help = !self.show_help;
            return Action::None;
        }
        if self.show_help {
            if key.code == KeyCode::Esc {
                self.show_help = false;
            }
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('n') => self.new_session(),
                KeyCode::Char('u') => {
                    self.input.clear();
                    self.reset_palette();
                }
                KeyCode::Char('w') => {
                    self.input.delete_word();
                    self.reset_palette();
                }
                KeyCode::Char('a') => self.input.cursor = 0,
                KeyCode::Char('e') => self.input.cursor = self.input.value.len(),
                _ => {}
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.insert('\n');
                self.reset_palette();
                Action::None
            }
            KeyCode::Enter if self.palette_open() => self.palette_accept(),
            KeyCode::Enter => self.submit(),
            KeyCode::Esc if self.palette_open() => {
                self.palette_hidden = true;
                Action::None
            }
            KeyCode::Esc if self.is_streaming() => self.stop_streaming(),
            KeyCode::Esc => Action::None,
            KeyCode::Tab if self.palette_open() => {
                self.palette_complete();
                Action::None
            }
            KeyCode::Backspace => {
                self.input.backspace();
                self.reset_palette();
                Action::None
            }
            KeyCode::Delete => {
                self.input.delete();
                self.reset_palette();
                Action::None
            }
            KeyCode::Left => {
                self.input.left();
                Action::None
            }
            KeyCode::Right => {
                self.input.right();
                Action::None
            }
            KeyCode::Home => {
                self.input.cursor = 0;
                Action::None
            }
            KeyCode::End => {
                self.follow_tail = true;
                Action::None
            }
            KeyCode::Up if self.palette_open() => {
                self.palette_move(false);
                Action::None
            }
            KeyCode::Down if self.palette_open() => {
                self.palette_move(true);
                Action::None
            }
            KeyCode::Up => {
                self.scroll_up(1);
                Action::None
            }
            KeyCode::Down => {
                self.scroll_down(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.scroll_up(usize::from(self.transcript_height));
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll_down(usize::from(self.transcript_height));
                Action::None
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.insert(character);
                self.reset_palette();
                Action::None
            }
            _ => Action::None,
        }
    }

    fn scroll_up(&mut self, amount: usize) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    fn submit(&mut self) -> Action {
        if self.is_streaming() {
            self.info("Stop the current response before sending another message.");
            return Action::None;
        }
        let value = self.input.take();
        self.reset_palette();
        let prompt = value.trim();
        if prompt.is_empty() {
            return Action::None;
        }
        if prompt.starts_with('/') {
            return self.command(prompt);
        }

        self.notice = None;
        self.session.update_title(prompt);
        self.session.messages.push(Message::new(Role::User, value));
        let messages = self.session.messages.clone();
        self.session
            .messages
            .push(Message::new(Role::Assistant, ""));
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let request_id = self.next_request_id;
        self.active = Some(ActiveRequest {
            id: request_id,
            started: Instant::now(),
        });
        self.follow_tail = true;
        self.save_session();
        Action::Start {
            request_id,
            messages,
        }
    }

    fn command(&mut self, command: &str) -> Action {
        let mut parts = command.split_whitespace();
        match parts.next().unwrap_or_default() {
            "/help" => self.show_help = true,
            "/new" => self.new_session(),
            "/model" => {
                let model = parts.collect::<Vec<_>>().join(" ");
                if model.is_empty() {
                    self.info(format!("Active model: {}", self.backend.model()));
                } else {
                    self.backend.set_model(model.clone());
                    self.info(format!("Model changed to {model}"));
                }
            }
            "/quit" => {
                self.should_quit = true;
                return Action::Quit;
            }
            unknown => self.error(format!(
                "Unknown command `{unknown}`. Type / to see commands."
            )),
        }
        Action::None
    }

    fn new_session(&mut self) {
        if self.is_streaming() {
            self.info("Stop the current response before starting a new session.");
            return;
        }
        self.session = Session::new();
        self.scroll = 0;
        self.follow_tail = true;
        self.notice = None;
        self.save_session();
    }

    fn stop_streaming(&mut self) -> Action {
        let Some(active) = self.active.take() else {
            return Action::None;
        };
        self.remove_empty_response();
        self.info("Response stopped. Partial output was kept.");
        self.save_session();
        Action::Cancel(active.id)
    }

    pub fn on_stream_event(&mut self, event: StreamEvent) {
        match event {
            StreamEvent::Delta { request_id, text }
                if self.active_request_id() == Some(request_id) =>
            {
                if let Some(message) = self.session.messages.last_mut() {
                    message.content.push_str(&text);
                }
            }
            StreamEvent::Done { request_id } if self.active_request_id() == Some(request_id) => {
                self.active = None;
                self.remove_empty_response();
                self.save_session();
            }
            StreamEvent::Error {
                request_id,
                message,
            } if self.active_request_id() == Some(request_id) => {
                self.active = None;
                self.remove_empty_response();
                self.error(message);
                self.save_session();
            }
            _ => {}
        }
    }

    fn remove_empty_response(&mut self) {
        if self
            .session
            .messages
            .last()
            .is_some_and(|message| message.role == Role::Assistant && message.content.is_empty())
        {
            self.session.messages.pop();
        }
    }

    fn info(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Info,
            message: message.into(),
        });
    }

    fn error(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Error,
            message: message.into(),
        });
    }

    fn save_session(&mut self) {
        let result = self
            .session_path
            .as_ref()
            .map(|path| self.session.save(path));
        if let Some(Err(error)) = result {
            self.error(format!("Could not save session: {error:#}"));
        }
    }

    pub fn persist(&self) -> Result<()> {
        self.session_path
            .as_ref()
            .map_or(Ok(()), |path| self.session.save(path))
    }
}

fn matching_commands(value: &str) -> Vec<&'static CommandSpec> {
    if !value.starts_with('/') || value.chars().any(char::is_whitespace) {
        return Vec::new();
    }
    let query = value[1..].to_ascii_lowercase();
    COMMANDS
        .iter()
        .filter(|command| command.name[1..].starts_with(query.as_str()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, Input, matching_commands};

    fn command_names(value: &str) -> Vec<&'static str> {
        matching_commands(value)
            .iter()
            .map(|command| command.name)
            .collect()
    }

    #[test]
    fn editing_respects_utf8_boundaries() {
        let mut input = Input::default();
        input.insert('é');
        input.insert('🦀');
        input.left();
        input.backspace();
        assert_eq!(input.value, "🦀");
        input.right();
        input.insert_str("\r\nhello");
        assert_eq!(input.value, "🦀\nhello");
    }

    #[test]
    fn deletes_the_previous_word_and_space() {
        let mut input = Input::default();
        input.insert_str("hello world  ");
        input.delete_word();
        assert_eq!(input.value, "hello ");
        input.delete_word();
        assert_eq!(input.value, "");
    }

    #[test]
    fn palette_matches_command_prefixes() {
        assert_eq!(command_names("/"), vec!["/help", "/new", "/model", "/quit"]);
        assert_eq!(command_names("/m"), vec!["/model"]);
        assert_eq!(command_names("/MO"), vec!["/model"]);
        assert!(command_names("/model gpt").is_empty());
        assert!(command_names("hello").is_empty());
    }

    #[test]
    fn completion_adds_a_space_only_when_arguments_are_expected() {
        let model = COMMANDS
            .iter()
            .find(|command| command.name == "/model")
            .unwrap();
        assert_eq!(model.completion(), "/model ");
        let help = COMMANDS
            .iter()
            .find(|command| command.name == "/help")
            .unwrap();
        assert_eq!(help.completion(), "/help");
    }
}
