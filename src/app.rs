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

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
    }

    pub fn handle_paste(&mut self, value: &str) {
        if !self.show_help {
            self.input.insert_str(value);
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
                KeyCode::Char('u') => self.input.clear(),
                KeyCode::Char('w') => self.input.delete_word(),
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
                Action::None
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Esc if self.is_streaming() => self.stop_streaming(),
            KeyCode::Esc => Action::None,
            KeyCode::Backspace => {
                self.input.backspace();
                Action::None
            }
            KeyCode::Delete => {
                self.input.delete();
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
            unknown => self.error(format!("Unknown command `{unknown}`. Try /help.")),
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

#[cfg(test)]
mod tests {
    use super::Input;

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
}
