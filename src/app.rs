use std::{path::PathBuf, time::Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};

use crate::{
    backend::{Backend, StreamEvent},
    catalog::{self, Catalog},
    config::{self, AuthKind, Connection, ProviderKind, SavedModel},
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

pub const COMMANDS: [CommandSpec; 5] = [
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
        name: "/connect",
        description: "Connect a provider and model",
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

const CUSTOM_PROVIDER_ID: &str = "custom";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectStage {
    Providers,
    Models,
    ApiKey,
    Endpoint,
    Auth,
    CustomToken,
    ModelId,
    ModelName,
    CustomModels,
    Password,
}

impl ConnectStage {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Providers => "provider",
            Self::Models => "model",
            Self::ApiKey => "api key",
            Self::Endpoint => "endpoint",
            Self::Auth => "authorization",
            Self::CustomToken => "token",
            Self::ModelId => "model id",
            Self::ModelName => "model name",
            Self::CustomModels => "models",
            Self::Password => "password",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConnectItem {
    pub id: String,
    pub title: String,
    pub detail: String,
}

pub struct ConnectState {
    pub stage: ConnectStage,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: String,
    pub selected: usize,
    catalog: Option<Catalog>,
    provider_id: Option<String>,
    model_id: Option<String>,
    api_key: String,
    password: String,
    custom_flow: bool,
    custom_endpoint: String,
    custom_auth: AuthKind,
    custom_token: String,
    custom_models: Vec<SavedModel>,
    custom_model_id: String,
    custom_model_name: String,
}

impl ConnectState {
    fn loading() -> Self {
        Self {
            stage: ConnectStage::Providers,
            loading: true,
            error: None,
            filter: String::new(),
            selected: 0,
            catalog: None,
            provider_id: None,
            model_id: None,
            api_key: String::new(),
            password: String::new(),
            custom_flow: false,
            custom_endpoint: String::new(),
            custom_auth: AuthKind::Bearer,
            custom_token: String::new(),
            custom_models: Vec::new(),
            custom_model_id: String::new(),
            custom_model_name: String::new(),
        }
    }

    pub fn items(&self) -> Vec<ConnectItem> {
        let filter = self.filter.to_ascii_lowercase();
        match self.stage {
            ConnectStage::Providers => {
                let mut items = Vec::new();
                if filter.is_empty() || "custom".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: CUSTOM_PROVIDER_ID.to_owned(),
                        title: "Custom".to_owned(),
                        detail: "OpenAI-compatible endpoint".to_owned(),
                    });
                }
                if let Some(catalog) = &self.catalog {
                    items.extend(
                        catalog::connectable_providers(catalog)
                            .into_iter()
                            .filter(|(id, _)| *id != CUSTOM_PROVIDER_ID)
                            .filter(|(id, provider)| {
                                filter.is_empty()
                                    || provider.name.to_ascii_lowercase().contains(filter.as_str())
                                    || id.to_ascii_lowercase().contains(filter.as_str())
                            })
                            .map(|(id, provider)| ConnectItem {
                                id: id.to_owned(),
                                title: provider.name.clone(),
                                detail: format!("{} models", provider.models.len()),
                            }),
                    );
                }
                items
            }
            ConnectStage::Models => {
                let Some(provider) = self.provider() else {
                    return Vec::new();
                };
                catalog::provider_models(provider)
                    .into_iter()
                    .filter(|(id, model)| {
                        filter.is_empty()
                            || model.name.to_ascii_lowercase().contains(filter.as_str())
                            || id.to_ascii_lowercase().contains(filter.as_str())
                    })
                    .map(|(id, model)| {
                        let title = if model.name.trim().is_empty() {
                            id.to_owned()
                        } else {
                            model.name.clone()
                        };
                        let context = model
                            .limit
                            .as_ref()
                            .and_then(|limit| limit.context)
                            .map_or_else(
                                || id.to_owned(),
                                |context| format!("{id} · {}k context", context / 1000),
                            );
                        ConnectItem {
                            id: id.to_owned(),
                            title,
                            detail: context,
                        }
                    })
                    .collect()
            }
            ConnectStage::Auth => AuthKind::choices()
                .into_iter()
                .map(|auth| ConnectItem {
                    id: auth.id().to_owned(),
                    title: auth.label().to_owned(),
                    detail: auth.detail().to_owned(),
                })
                .collect(),
            ConnectStage::CustomModels => self
                .custom_models
                .iter()
                .map(|model| ConnectItem {
                    id: model.id.clone(),
                    title: if model.name.trim().is_empty() {
                        model.id.clone()
                    } else {
                        model.name.clone()
                    },
                    detail: if model.name.trim().is_empty() {
                        "model id".to_owned()
                    } else {
                        model.id.clone()
                    },
                })
                .collect(),
            ConnectStage::ApiKey
            | ConnectStage::Endpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName
            | ConnectStage::Password => Vec::new(),
        }
    }

    pub fn input(&self) -> String {
        if self.stage == ConnectStage::Auth {
            return self.auth_at_selection().label().to_owned();
        }
        if self.stage == ConnectStage::CustomModels {
            let items = self.items();
            return items
                .get(self.selected.min(items.len().saturating_sub(1)))
                .map_or_else(|| "none".to_owned(), |item| item.title.clone());
        }
        let value = match self.stage {
            ConnectStage::ApiKey => &self.api_key,
            ConnectStage::Password => &self.password,
            ConnectStage::Endpoint => &self.custom_endpoint,
            ConnectStage::CustomToken => &self.custom_token,
            ConnectStage::ModelId => &self.custom_model_id,
            ConnectStage::ModelName => &self.custom_model_name,
            ConnectStage::Providers | ConnectStage::Models => &self.filter,
            ConnectStage::Auth | ConnectStage::CustomModels => unreachable!(),
        };
        if matches!(
            self.stage,
            ConnectStage::ApiKey | ConnectStage::Password | ConnectStage::CustomToken
        ) && !value.is_empty()
        {
            "•".repeat(value.chars().count().min(64))
        } else {
            value.clone()
        }
    }

    pub const fn input_label(&self) -> &'static str {
        match self.stage {
            ConnectStage::Providers | ConnectStage::Models => "search",
            ConnectStage::ApiKey => "api key",
            ConnectStage::Endpoint => "completion endpoint",
            ConnectStage::Auth => "authorization",
            ConnectStage::CustomToken => "token",
            ConnectStage::ModelId => "model id",
            ConnectStage::ModelName => "model name",
            ConnectStage::CustomModels => "model",
            ConnectStage::Password => "encryption password (optional)",
        }
    }

    pub const fn uses_cursor(&self) -> bool {
        !matches!(self.stage, ConnectStage::Auth | ConnectStage::CustomModels)
    }

    pub fn has_custom_models(&self) -> bool {
        !self.custom_models.is_empty()
    }

    fn auth_at_selection(&self) -> AuthKind {
        let choices = AuthKind::choices();
        choices[self.selected.min(choices.len() - 1)]
    }

    fn auth_index(&self) -> usize {
        AuthKind::choices()
            .iter()
            .position(|auth| *auth == self.custom_auth)
            .unwrap_or(0)
    }

    pub fn required_env(&self) -> Option<&str> {
        self.provider()
            .and_then(|provider| provider.env.first())
            .map(String::as_str)
    }

    fn provider(&self) -> Option<&catalog::Provider> {
        let id = self.provider_id.as_deref()?;
        self.catalog.as_ref()?.providers.get(id)
    }

    fn selected_item(&self) -> Option<ConnectItem> {
        let items = self.items();
        if items.is_empty() {
            return None;
        }
        items.get(self.selected.min(items.len() - 1)).cloned()
    }

    fn move_selection(&mut self, forward: bool) {
        let len = self.items().len();
        if len == 0 {
            return;
        }
        let current = self.selected.min(len - 1);
        self.selected = if forward {
            (current + 1) % len
        } else {
            (current + len - 1) % len
        };
    }

    fn move_selection_by(&mut self, delta: isize) {
        let len = self.items().len();
        if len == 0 {
            return;
        }
        let current = self.selected.min(len - 1) as isize;
        self.selected = (current + delta).clamp(0, len as isize - 1) as usize;
    }
}

fn custom_field_mut<'a>(
    connect: &'a mut ConnectState,
    stage: ConnectStage,
) -> Option<&'a mut String> {
    match stage {
        ConnectStage::Endpoint => Some(&mut connect.custom_endpoint),
        ConnectStage::CustomToken => Some(&mut connect.custom_token),
        ConnectStage::ModelId => Some(&mut connect.custom_model_id),
        ConnectStage::ModelName => Some(&mut connect.custom_model_name),
        _ => None,
    }
}

pub enum Action {
    None,
    Start {
        request_id: u64,
        messages: Vec<Message>,
    },
    Cancel(u64),
    Quit,
    LoadCatalog,
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
    config_path: PathBuf,
    palette_index: usize,
    palette_hidden: bool,
    connect: Option<ConnectState>,
}

impl App {
    pub fn new(
        session: Session,
        backend: Backend,
        session_path: Option<PathBuf>,
        config_path: PathBuf,
    ) -> Self {
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
            config_path,
            palette_index: 0,
            palette_hidden: false,
            connect: None,
        }
    }

    pub fn connect_state(&self) -> Option<&ConnectState> {
        self.connect.as_ref()
    }

    pub fn connect_open(&self) -> bool {
        self.connect.is_some()
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
        if let Some(connect) = self.connect.as_mut() {
            let clean = value
                .chars()
                .filter(|character| !character.is_control())
                .collect::<String>();
            match connect.stage {
                ConnectStage::ApiKey => connect.api_key.push_str(&clean),
                ConnectStage::Password => connect.password.push_str(&clean),
                ConnectStage::Endpoint => connect.custom_endpoint.push_str(&clean),
                ConnectStage::CustomToken => connect.custom_token.push_str(&clean),
                ConnectStage::ModelId => connect.custom_model_id.push_str(&clean),
                ConnectStage::ModelName => connect.custom_model_name.push_str(&clean),
                ConnectStage::Providers | ConnectStage::Models => {
                    connect.filter.push_str(&clean);
                    connect.selected = 0;
                }
                ConnectStage::Auth | ConnectStage::CustomModels => {}
            }
            return;
        }
        if !self.show_help {
            self.input.insert_str(value);
            self.reset_palette();
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        if self.connect.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.connect_move(false),
                MouseEventKind::ScrollDown => self.connect_move(true),
                _ => {}
            }
            return;
        }
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
        if self.connect.is_some() {
            return self.handle_connect_key(key);
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

    fn handle_connect_key(&mut self, key: KeyEvent) -> Action {
        let Some(stage) = self.connect.as_ref().map(|connect| connect.stage) else {
            return Action::None;
        };
        if self.connect.as_ref().is_some_and(|connect| connect.loading) {
            if key.code == KeyCode::Esc {
                self.connect = None;
            }
            return Action::None;
        }
        match stage {
            ConnectStage::Providers | ConnectStage::Models => self.handle_connect_list_key(key),
            ConnectStage::ApiKey => self.handle_connect_api_key(key),
            ConnectStage::Endpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName => self.handle_connect_custom_text(key),
            ConnectStage::Auth | ConnectStage::CustomModels => self.handle_connect_choice_key(key),
            ConnectStage::Password => self.handle_connect_password(key),
        }
    }

    fn handle_connect_list_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                let cleared_filter = if let Some(connect) = self.connect.as_mut() {
                    if connect.filter.is_empty() {
                        false
                    } else {
                        connect.filter.clear();
                        connect.selected = 0;
                        true
                    }
                } else {
                    return Action::None;
                };
                if cleared_filter {
                    return Action::None;
                }
                let went_back = if let Some(connect) = self.connect.as_mut() {
                    if connect.stage == ConnectStage::Models {
                        connect.stage = ConnectStage::Providers;
                        connect.selected = 0;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if !went_back {
                    self.connect = None;
                }
            }
            KeyCode::Enter => return self.select_connect_item(),
            KeyCode::Up => self.connect_move(false),
            KeyCode::Down => self.connect_move(true),
            KeyCode::PageUp => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.move_selection_by(-10);
                }
            }
            KeyCode::PageDown => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.move_selection_by(10);
                }
            }
            KeyCode::Home => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.selected = 0;
                }
            }
            KeyCode::End => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.selected = usize::MAX;
                }
            }
            KeyCode::Backspace => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.filter.pop();
                    connect.selected = 0;
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(connect) = self.connect.as_mut() {
                    connect.filter.push(character);
                    connect.selected = 0;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_connect_choice_key(&mut self, key: KeyEvent) -> Action {
        let Some(stage) = self.connect.as_ref().map(|connect| connect.stage) else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.stage = ConnectStage::Endpoint;
                    connect.selected = 0;
                    connect.filter.clear();
                    connect.error = None;
                }
            }
            KeyCode::Enter => return self.select_connect_item(),
            KeyCode::Up => self.connect_move(false),
            KeyCode::Down => self.connect_move(true),
            KeyCode::PageUp => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.move_selection_by(-10);
                }
            }
            KeyCode::PageDown => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.move_selection_by(10);
                }
            }
            KeyCode::Home => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.selected = 0;
                }
            }
            KeyCode::End => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.selected = usize::MAX;
                }
            }
            KeyCode::Char('a' | 'A')
                if stage == ConnectStage::CustomModels
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(connect) = self.connect.as_mut() {
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    connect.stage = ConnectStage::ModelId;
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_connect_custom_text(&mut self, key: KeyEvent) -> Action {
        let Some(stage) = self.connect.as_ref().map(|connect| connect.stage) else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => self.back_from_custom_text(stage),
            KeyCode::Enter => return self.advance_custom_text(stage),
            KeyCode::Backspace => {
                if let Some(field) = self
                    .connect
                    .as_mut()
                    .and_then(|connect| custom_field_mut(connect, stage))
                {
                    field.pop();
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(field) = self
                    .connect
                    .as_mut()
                    .and_then(|connect| custom_field_mut(connect, stage))
                {
                    field.push(character);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn back_from_custom_text(&mut self, stage: ConnectStage) {
        if stage == ConnectStage::Endpoint {
            self.connect = None;
            return;
        }
        let Some(connect) = self.connect.as_mut() else {
            return;
        };
        connect.error = None;
        match stage {
            ConnectStage::CustomToken => {
                let selected = connect.auth_index();
                connect.stage = ConnectStage::Auth;
                connect.selected = selected;
            }
            ConnectStage::ModelId => {
                let stage = if connect.custom_auth.requires_token() {
                    ConnectStage::CustomToken
                } else {
                    ConnectStage::Auth
                };
                let selected = if stage == ConnectStage::Auth {
                    connect.auth_index()
                } else {
                    0
                };
                connect.stage = stage;
                connect.selected = selected;
            }
            ConnectStage::ModelName => {
                connect.stage = ConnectStage::ModelId;
                connect.selected = 0;
            }
            _ => {}
        }
    }

    fn advance_custom_text(&mut self, stage: ConnectStage) -> Action {
        match stage {
            ConnectStage::Endpoint => {
                let endpoint = self
                    .connect
                    .as_ref()
                    .map(|connect| connect.custom_endpoint.trim().to_owned())
                    .unwrap_or_default();
                if endpoint.is_empty() {
                    self.connect_error("Enter the completion endpoint.");
                    return Action::None;
                }
                if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
                    self.connect_error("Endpoint must start with http:// or https://.");
                    return Action::None;
                }
                let auth_index = self
                    .connect
                    .as_ref()
                    .map(ConnectState::auth_index)
                    .unwrap_or(0);
                if let Some(connect) = self.connect.as_mut() {
                    connect.custom_endpoint = endpoint;
                    connect.stage = ConnectStage::Auth;
                    connect.selected = auth_index;
                    connect.error = None;
                }
            }
            ConnectStage::CustomToken => {
                let (token, auth) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (connect.custom_token.trim().to_owned(), connect.custom_auth)
                };
                if token.is_empty() && auth.requires_token() {
                    self.connect_error(format!("Enter a token for {}.", auth.label()));
                    return Action::None;
                }
                if let Some(connect) = self.connect.as_mut() {
                    connect.custom_token = token;
                    connect.stage = ConnectStage::ModelId;
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            ConnectStage::ModelId => {
                let (id, has_models) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (
                        connect.custom_model_id.trim().to_owned(),
                        !connect.custom_models.is_empty(),
                    )
                };
                if id.is_empty() {
                    if has_models {
                        if let Some(connect) = self.connect.as_mut() {
                            connect.stage = ConnectStage::CustomModels;
                            connect.selected = 0;
                            connect.error = None;
                        }
                    } else {
                        self.connect_error("Enter a model ID.");
                    }
                    return Action::None;
                }
                let existing_name = self.connect.as_ref().and_then(|connect| {
                    connect
                        .custom_models
                        .iter()
                        .find(|model| model.id == id)
                        .map(|model| model.name.clone())
                });
                if let Some(connect) = self.connect.as_mut() {
                    connect.custom_model_id = id;
                    connect.custom_model_name = existing_name.unwrap_or_default();
                    connect.stage = ConnectStage::ModelName;
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            ConnectStage::ModelName => {
                let (id, name) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (
                        connect.custom_model_id.trim().to_owned(),
                        connect.custom_model_name.trim().to_owned(),
                    )
                };
                if id.is_empty() {
                    if let Some(connect) = self.connect.as_mut() {
                        connect.stage = ConnectStage::ModelId;
                        connect.error = Some("Enter a model ID first.".to_owned());
                    }
                    return Action::None;
                }
                if let Some(connect) = self.connect.as_mut() {
                    let existing = connect
                        .custom_models
                        .iter()
                        .position(|model| model.id == id);
                    if let Some(index) = existing {
                        connect.custom_models[index].name = name;
                    } else {
                        connect.custom_models.push(SavedModel {
                            id: id.clone(),
                            name,
                        });
                    }
                    connect.selected = existing.unwrap_or(connect.custom_models.len() - 1);
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    connect.stage = ConnectStage::CustomModels;
                    connect.error = None;
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_connect_api_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.stage = ConnectStage::Models;
                    connect.selected = 0;
                    connect.api_key.clear();
                    connect.error = None;
                }
            }
            KeyCode::Enter => {
                let api_key = self
                    .connect
                    .as_ref()
                    .map(|connect| connect.api_key.trim().to_owned())
                    .unwrap_or_default();
                if api_key.is_empty() {
                    return self.finish_connect(None, None);
                }
                if let Some(connect) = self.connect.as_mut() {
                    connect.stage = ConnectStage::Password;
                    connect.password.clear();
                    connect.error = None;
                }
            }
            KeyCode::Backspace => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.api_key.pop();
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(connect) = self.connect.as_mut() {
                    connect.api_key.push(character);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_connect_password(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.stage = if connect.custom_flow {
                        ConnectStage::CustomModels
                    } else {
                        ConnectStage::ApiKey
                    };
                    connect.password.clear();
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            KeyCode::Enter => {
                let (password, custom_flow, api_key) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (
                        if connect.password.is_empty() {
                            None
                        } else {
                            Some(connect.password.clone())
                        },
                        connect.custom_flow,
                        connect.api_key.trim().to_owned(),
                    )
                };
                if custom_flow {
                    return self.finish_custom_connect(password);
                }
                if api_key.is_empty() {
                    return self.finish_connect(None, None);
                }
                return self.finish_connect(Some(api_key), password);
            }
            KeyCode::Backspace => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.password.pop();
                }
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(connect) = self.connect.as_mut() {
                    connect.password.push(character);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn select_connect_item(&mut self) -> Action {
        let Some(item) = self.connect.as_ref().and_then(ConnectState::selected_item) else {
            return Action::None;
        };
        let Some(connect) = self.connect.as_mut() else {
            return Action::None;
        };
        match connect.stage {
            ConnectStage::Providers => {
                connect.model_id = None;
                connect.filter.clear();
                connect.selected = 0;
                connect.error = None;
                if item.id == CUSTOM_PROVIDER_ID {
                    connect.provider_id = None;
                    connect.custom_flow = true;
                    connect.custom_token.clear();
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    connect.stage = ConnectStage::Endpoint;
                } else {
                    connect.provider_id = Some(item.id);
                    connect.custom_flow = false;
                    connect.stage = ConnectStage::Models;
                }
            }
            ConnectStage::Models => {
                connect.model_id = Some(item.id);
                connect.api_key.clear();
                connect.password.clear();
                connect.stage = ConnectStage::ApiKey;
                connect.filter.clear();
                connect.selected = 0;
                connect.error = None;
            }
            ConnectStage::Auth => {
                let Some(auth) = AuthKind::parse(&item.id) else {
                    return Action::None;
                };
                connect.custom_auth = auth;
                connect.model_id = None;
                connect.selected = 0;
                connect.error = None;
                if auth.requires_token() {
                    connect.stage = ConnectStage::CustomToken;
                } else {
                    connect.custom_token.clear();
                    connect.stage = ConnectStage::ModelId;
                }
            }
            ConnectStage::CustomModels => {
                connect.model_id = Some(item.id);
                connect.password.clear();
                connect.stage = ConnectStage::Password;
                connect.selected = 0;
                connect.error = None;
            }
            ConnectStage::ApiKey
            | ConnectStage::Endpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName
            | ConnectStage::Password => {}
        }
        Action::None
    }

    fn finish_connect(&mut self, api_key: Option<String>, password: Option<String>) -> Action {
        let plan = {
            let Some(connect) = self.connect.as_ref() else {
                return Action::None;
            };
            let Some(provider_id) = connect.provider_id.clone() else {
                return Action::None;
            };
            let Some(provider) = connect.provider() else {
                return Action::None;
            };
            let Some(model_id) = connect.model_id.clone() else {
                return Action::None;
            };
            let provider_name = if provider.name.trim().is_empty() {
                provider_id.clone()
            } else {
                provider.name.clone()
            };
            (
                provider_name,
                provider.base_url(&provider_id),
                model_id,
                provider.env.first().cloned(),
            )
        };
        let (provider_name, base_url, model_id, api_key_env) = plan;
        if base_url.trim().is_empty() {
            if let Some(connect) = self.connect.as_mut() {
                connect.error = Some(format!("{provider_name} has no API endpoint."));
            }
            return Action::None;
        }
        let entered_key = api_key.filter(|api_key| !api_key.trim().is_empty());
        let effective_key = entered_key.clone().or_else(|| {
            api_key_env
                .as_deref()
                .and_then(|name| std::env::var(name).ok())
                .filter(|api_key| !api_key.trim().is_empty())
        });
        let connection = Connection {
            provider: ProviderKind::OpenAi,
            provider_name: provider_name.clone(),
            model: model_id.clone(),
            base_url: base_url.clone(),
            api_key: entered_key,
            api_key_env,
            auth: AuthKind::Bearer,
            models: Vec::new(),
        };
        let saved = config::save_connection(&self.config_path, &connection, password.as_deref());
        self.backend.configure(
            ProviderKind::OpenAi,
            provider_name.clone(),
            model_id.clone(),
            base_url,
            effective_key,
            AuthKind::Bearer,
            Vec::new(),
        );
        self.connect = None;
        if let Err(error) = saved {
            self.error(format!("Connected, but could not save: {error:#}"));
        } else {
            self.info(format!("Connected to {provider_name} · {model_id}"));
        }
        Action::None
    }

    fn finish_custom_connect(&mut self, password: Option<String>) -> Action {
        let plan = {
            let Some(connect) = self.connect.as_ref() else {
                return Action::None;
            };
            if !connect.custom_flow {
                return Action::None;
            }
            (
                connect.custom_endpoint.trim().to_owned(),
                connect.model_id.clone(),
                connect.custom_auth,
                connect.custom_token.trim().to_owned(),
                connect.custom_models.clone(),
            )
        };
        let (endpoint, model_id, auth, token, models) = plan;
        let Some(model_id) = model_id else {
            self.connect_error("Choose a model.");
            return Action::None;
        };
        if endpoint.is_empty() {
            self.connect_error("Enter the completion endpoint.");
            return Action::None;
        }
        if !models.iter().any(|model| model.id == model_id) {
            self.connect_error("Choose a model.");
            return Action::None;
        }
        if auth.requires_token() && token.is_empty() {
            if let Some(connect) = self.connect.as_mut() {
                connect.stage = ConnectStage::CustomToken;
                connect.error = Some(format!("Enter a token for {}.", auth.label()));
            }
            return Action::None;
        }
        let api_key = if auth.requires_token() {
            Some(token)
        } else {
            None
        };
        let connection = Connection {
            provider: ProviderKind::Custom,
            provider_name: "Custom".to_owned(),
            model: model_id.clone(),
            base_url: endpoint.clone(),
            api_key: api_key.clone(),
            api_key_env: None,
            auth,
            models: models.clone(),
        };
        let saved = config::save_connection(&self.config_path, &connection, password.as_deref());
        self.backend.configure(
            ProviderKind::Custom,
            "Custom".to_owned(),
            model_id.clone(),
            endpoint,
            api_key,
            auth,
            models,
        );
        self.connect = None;
        if let Err(error) = saved {
            self.error(format!("Connected, but could not save: {error:#}"));
        } else {
            self.info(format!("Connected to Custom · {model_id}"));
        }
        Action::None
    }

    fn connect_move(&mut self, forward: bool) {
        if let Some(connect) = self.connect.as_mut() {
            if matches!(
                connect.stage,
                ConnectStage::Providers
                    | ConnectStage::Models
                    | ConnectStage::Auth
                    | ConnectStage::CustomModels
            ) {
                connect.move_selection(forward);
            }
        }
    }

    pub fn on_catalog(&mut self, result: catalog::FetchResult) {
        let Some(connect) = self.connect.as_mut() else {
            return;
        };
        connect.loading = false;
        match result {
            Ok(catalog) => {
                connect.catalog = Some(catalog);
                connect.error = None;
                connect.selected = 0;
            }
            Err(error) => {
                connect.error = Some(error);
            }
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
        if !self.backend.is_configured() {
            self.error("No provider connected. Run /connect to get started.");
            return Action::None;
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
            "/connect" => return self.begin_connect(),
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

    fn begin_connect(&mut self) -> Action {
        if self.is_streaming() {
            self.info("Stop the current response before connecting a provider.");
            return Action::None;
        }
        self.palette_hidden = true;
        let mut connect = ConnectState::loading();
        if self.backend.provider_kind() == ProviderKind::Custom {
            connect.custom_endpoint = self.backend.base_url().to_owned();
            connect.custom_auth = self.backend.auth();
            connect.custom_models = self.backend.saved_models().to_vec();
        }
        self.connect = Some(connect);
        Action::LoadCatalog
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

    fn connect_error(&mut self, message: impl Into<String>) {
        if let Some(connect) = self.connect.as_mut() {
            connect.error = Some(message.into());
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
        assert_eq!(
            command_names("/"),
            vec!["/help", "/new", "/connect", "/model", "/quit"]
        );
        assert_eq!(command_names("/c"), vec!["/connect"]);
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
