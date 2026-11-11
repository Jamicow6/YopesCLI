use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    time::Instant,
};

use anyhow::Result;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use crate::{
    backend::{Backend, StreamEvent},
    catalog::{self, Catalog},
    config::{self, AuthKind, Connection, ProviderKind, SavedModel},
    diff::FileDiff,
    session::{self, Message, Role, Session, ToolCall},
    tools,
};

const MAX_TOOL_ROUNDS: usize = 24;

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
    pub argument: Option<&'static str>,
}

impl CommandSpec {
    pub fn display_name(self) -> String {
        self.argument.map_or_else(
            || self.name.to_owned(),
            |argument| format!("{} [{argument}]", self.name),
        )
    }

    fn completion(self) -> String {
        if self.argument.is_some() {
            format!("{} ", self.name)
        } else {
            self.name.to_owned()
        }
    }
}

pub const COMMANDS: [CommandSpec; 6] = [
    CommandSpec {
        name: "/help",
        description: "Show keys and commands",
        argument: None,
    },
    CommandSpec {
        name: "/new",
        description: "Start a fresh session",
        argument: None,
    },
    CommandSpec {
        name: "/connect",
        description: "Connect a provider and model",
        argument: None,
    },
    CommandSpec {
        name: "/model",
        description: "Show or change the active model",
        argument: Some("name"),
    },
    CommandSpec {
        name: "/tools",
        description: "Set tool mode: on, off, auto, ask",
        argument: Some("mode"),
    },
    CommandSpec {
        name: "/quit",
        description: "Save and exit",
        argument: None,
    },
];

struct ActiveRequest {
    id: u64,
    started: Instant,
    prompt_tokens: u64,
    usage: Option<(u64, u64)>,
    tool_progress: Option<(String, String, u64)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToolsMode {
    #[default]
    On,
    Off,
    Auto,
    Ask,
}

impl ToolsMode {
    pub const fn label(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
            Self::Auto => "auto",
            Self::Ask => "ask",
        }
    }

    pub const fn detail(self) -> &'static str {
        match self {
            Self::On => "edits run automatically; commands the model flags as risky ask",
            Self::Off => "tools are not offered to the model",
            Self::Auto => "all tool calls run automatically",
            Self::Ask => "every command the model flags as risky asks",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            "auto" => Some(Self::Auto),
            "ask" => Some(Self::Ask),
            _ => None,
        }
    }

    pub const fn advertised(self) -> bool {
        !matches!(self, Self::Off)
    }

    fn needs_approval(self, call: &ToolCall, approved: &HashSet<String>) -> bool {
        match self {
            Self::Off | Self::Auto => false,
            Self::Ask => tools::requires_approval(call),
            Self::On => tools::requires_approval(call) && !approved.contains(&call.function.name),
        }
    }
}

pub struct ToolApproval {
    pub call: ToolCall,
    pub note: Option<String>,
    pub scroll: usize,
    pub page_height: u16,
}

pub struct RunningTool {
    pub call: ToolCall,
    pub started: Instant,
    /// File contents from before the call, kept for the outcome diff.
    pub before: Option<String>,
    /// Whether the write or edit target existed before the call ran.
    pub existed: bool,
}

pub struct ToolEvent {
    pub call: ToolCall,
    pub result: std::result::Result<String, String>,
}

/// A diff shown under a tool result in the conversation.
pub struct ChangeReport {
    pub diff: FileDiff,
    pub side_by_side: bool,
}

pub const CUSTOM_PROVIDER_ID: &str = "custom";
const ADD_MODEL_ID: &str = "add-model";
/// Rows that edit an existing saved endpoint from the model list.
const EDIT_ENDPOINT_ID: &str = "edit-endpoint";
const EDIT_TOKEN_ID: &str = "edit-token";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectStage {
    Providers,
    Models,
    ApiKey,
    ProviderName,
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
            Self::ProviderName => "name",
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
    pub logo: &'static str,
    /// Set when the item is a saved custom endpoint, holding its provider id.
    pub custom_id: Option<String>,
    /// True for the provider and model the session is currently using.
    pub active: bool,
}

/// A single-glyph logo for a models.dev provider or the custom flow.
pub fn provider_logo(id: &str) -> &'static str {
    match id {
        "openai" => "●",
        "openrouter" => "⇄",
        "anthropic" => "◇",
        "google" => "▴",
        "cohere" => "∞",
        "mistral" => "↯",
        "deepseek" => "∿",
        "groq" => "»",
        "fireworks" => "✦",
        "xai" => "✕",
        "perplexity" => "?",
        "together" => "◈",
        "azure" => "▣",
        "aws" => "☁",
        "cerebras" => "□",
        "friendliai" => "♥",
        "custom" => "⚙",
        _ => "▸",
    }
}

pub struct ConnectState {
    pub stage: ConnectStage,
    pub loading: bool,
    pub error: Option<String>,
    pub filter: String,
    pub selected: usize,
    catalog: Option<Catalog>,
    saved_custom: Vec<config::CustomProvider>,
    provider_id: Option<String>,
    model_id: Option<String>,
    api_key: String,
    password: String,
    custom_flow: bool,
    custom_provider_id: Option<String>,
    custom_name: String,
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
            saved_custom: Vec::new(),
            provider_id: None,
            model_id: None,
            api_key: String::new(),
            password: String::new(),
            custom_flow: false,
            custom_provider_id: None,
            custom_name: String::new(),
            custom_endpoint: String::new(),
            custom_auth: AuthKind::Bearer,
            custom_token: String::new(),
            custom_models: Vec::new(),
            custom_model_id: String::new(),
            custom_model_name: String::new(),
        }
    }

    pub fn items(&self) -> Vec<ConnectItem> {
        self.items_using("", "")
    }

    /// Build the list, marking whatever the session is already using.
    ///
    /// The active endpoint is the backend's base URL, which uniquely identifies
    /// a provider (two saved custom endpoints can share a name but not a URL).
    pub fn items_using(&self, active_base_url: &str, active_model: &str) -> Vec<ConnectItem> {
        let filter = self.filter.to_ascii_lowercase();
        let active_base_url = active_base_url.trim_end_matches('/');
        let is_active = |endpoint: &str| {
            !active_base_url.is_empty()
                && endpoint.trim_end_matches('/') == active_base_url
        };
        match self.stage {
            ConnectStage::Providers => {
                let mut items = Vec::new();
                if filter.is_empty() || "custom".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: CUSTOM_PROVIDER_ID.to_owned(),
                        title: "Add custom provider".to_owned(),
                        detail: "new OpenAI-compatible endpoint".to_owned(),
                        logo: provider_logo(CUSTOM_PROVIDER_ID),
                        custom_id: None,
                        active: false,
                    });
                }
                for saved in self.saved_custom.iter().filter(|saved| {
                    filter.is_empty() || saved.name.to_ascii_lowercase().contains(filter.as_str())
                }) {
                    items.push(ConnectItem {
                        id: saved.id.clone(),
                        title: saved.name.clone(),
                        detail: format!("saved · {} models", saved.models.len()),
                        logo: "★",
                        custom_id: Some(saved.id.clone()),
                        active: is_active(&saved.base_url),
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
                                logo: provider_logo(id),
                                custom_id: None,
                                active: is_active(provider.api.as_deref().unwrap_or("")),
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
                        let provider_id = self.provider_id.as_deref().unwrap_or_default();
                        ConnectItem {
                            id: id.to_owned(),
                            title,
                            detail: context,
                            logo: provider_logo(provider_id),
                            custom_id: None,
                            active: !active_model.is_empty() && id == active_model,
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
                    logo: "•",
                    custom_id: None,
                    active: false,
                })
                .collect(),
            ConnectStage::CustomModels => {
                let mut items: Vec<ConnectItem> = self
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
                        logo: "▸",
                        custom_id: None,
                        active: !active_model.is_empty() && model.id == active_model,
                    })
                    .collect();
                if filter.is_empty() || "add".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: ADD_MODEL_ID.to_owned(),
                        title: "Add new model".to_owned(),
                        detail: "enter a model id".to_owned(),
                        logo: "+",
                        custom_id: None,
                        active: false,
                    });
                }
                // Editing only makes sense for an endpoint that is already saved.
                if self.custom_provider_id.is_some() {
                    // Match the same way every other row filters: against the title.
                    let matches = |title: &str| {
                        filter.is_empty() || title.to_ascii_lowercase().contains(filter.as_str())
                    };
                    if matches("Change endpoint") {
                        items.push(ConnectItem {
                            id: EDIT_ENDPOINT_ID.to_owned(),
                            title: "Change endpoint".to_owned(),
                            detail: self.custom_endpoint.clone(),
                            logo: "✎",
                            custom_id: None,
                            active: false,
                        });
                    }
                    if matches("Change API key") {
                        items.push(ConnectItem {
                            id: EDIT_TOKEN_ID.to_owned(),
                            title: "Change API key".to_owned(),
                            detail: if self.custom_token.is_empty() {
                                "not set".to_owned()
                            } else {
                                format!("set · {} chars", self.custom_token.chars().count())
                            },
                            logo: "✎",
                            custom_id: None,
                            active: false,
                        });
                    }
                }
                items
            }
            ConnectStage::ApiKey
            | ConnectStage::ProviderName
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
            ConnectStage::ProviderName => &self.custom_name,
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
            ConnectStage::ProviderName => "provider name",
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
        ConnectStage::ProviderName => Some(&mut connect.custom_name),
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
        tools_enabled: bool,
    },
    Cancel(u64),
    RunTool {
        call: ToolCall,
    },
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
    pub tools_mode: ToolsMode,
    pub approval: Option<ToolApproval>,
    pub running_tool: Option<RunningTool>,
    /// Diffs for write and edit calls, shown under their results.
    pub changes: HashMap<String, ChangeReport>,
    /// Tool results the user expanded to see the full output.
    pub expanded: HashSet<String>,
    /// Screen row of each collapsible tool output, for click detection.
    pub output_rows: Vec<(u16, String)>,
    /// Unified or side-by-side diff view, kept across approvals and results.
    pub diff_sides: bool,
    active: Option<ActiveRequest>,
    next_request_id: u64,
    pending_tools: VecDeque<ToolCall>,
    approved_tools: HashSet<String>,
    tool_rounds: usize,
    session_path: Option<PathBuf>,
    config_path: PathBuf,
    custom_providers: Vec<config::CustomProvider>,
    palette_index: usize,
    palette_hidden: bool,
    connect: Option<ConnectState>,
    /// Prompt tokens billed across this session.
    spent_prompt_tokens: u64,
    /// Completion tokens billed across this session.
    spent_completion_tokens: u64,
}

impl App {
    pub fn new(
        session: Session,
        backend: Backend,
        session_path: Option<PathBuf>,
        config_path: PathBuf,
        custom_providers: Vec<config::CustomProvider>,
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
            tools_mode: ToolsMode::default(),
            approval: None,
            running_tool: None,
            changes: HashMap::new(),
            expanded: HashSet::new(),
            output_rows: Vec::new(),
            diff_sides: true,
            active: None,
            next_request_id: 0,
            pending_tools: VecDeque::new(),
            approved_tools: HashSet::new(),
            tool_rounds: 0,
            session_path,
            config_path,
            custom_providers,
            palette_index: 0,
            palette_hidden: false,
            connect: None,
            spent_prompt_tokens: 0,
            spent_completion_tokens: 0,
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

    pub fn tools_busy(&self) -> bool {
        self.approval.is_some() || self.running_tool.is_some() || !self.pending_tools.is_empty()
    }

    pub fn is_busy(&self) -> bool {
        self.is_streaming() || self.tools_busy()
    }

    pub fn running_tool_elapsed(&self) -> Option<f64> {
        self.running_tool
            .as_ref()
            .map(|running| running.started.elapsed().as_secs_f64())
    }

    pub fn active_request_id(&self) -> Option<u64> {
        self.active.as_ref().map(|active| active.id)
    }

    pub fn elapsed_seconds(&self) -> Option<f64> {
        self.active
            .as_ref()
            .map(|active| active.started.elapsed().as_secs_f64())
    }

    pub fn stream_tool_progress(&self) -> Option<(String, String, u64)> {
        self.active
            .as_ref()
            .and_then(|active| active.tool_progress.clone())
    }

    /// (in, out) tokens for the active request: server-reported usage, else chars/4 estimates.
    pub fn stream_tokens(&self) -> Option<(u64, u64)> {
        let active = self.active.as_ref()?;
        match active.usage {
            Some((prompt, completion)) => Some((prompt.max(active.prompt_tokens), completion)),
            None => Some((active.prompt_tokens, estimated_output_tokens(&self.session))),
        }
    }

    /// Session spend so far, priced from the active model.
    pub fn spent_cost(&self) -> String {
        format_cost(
            self.spent_prompt_tokens,
            self.spent_completion_tokens,
            self.backend.model(),
        )
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
        if self.approval.is_some() {
            return;
        }
        if let Some(connect) = self.connect.as_mut() {
            let clean = value
                .chars()
                .filter(|character| !character.is_control())
                .collect::<String>();
            match connect.stage {
                ConnectStage::ApiKey => connect.api_key.push_str(&clean),
                ConnectStage::Password => connect.password.push_str(&clean),
                ConnectStage::ProviderName => connect.custom_name.push_str(&clean),
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
        if let Some(approval) = self.approval.as_mut() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    approval.scroll = approval.scroll.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    approval.scroll = approval.scroll.saturating_add(3);
                }
                _ => {}
            }
            return;
        }
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
            MouseEventKind::Down(MouseButton::Left) => self.toggle_output_at(mouse.row),
            _ => {}
        }
    }

    /// Expand or collapse the tool output drawn on a screen row.
    fn toggle_output_at(&mut self, row: u16) {
        let Some((_, call_id)) = self
            .output_rows
            .iter()
            .find(|(line, _)| *line == row)
            .cloned()
        else {
            return;
        };
        if !self.expanded.remove(&call_id) {
            self.expanded.insert(call_id);
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                key.code,
                KeyCode::Char('c' | 'C' | 'q' | 'Q') | KeyCode::Null
            )
        {
            self.should_quit = true;
            return Action::Quit;
        }
        if self.approval.is_some() {
            return self.handle_approval_key(key);
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
            KeyCode::Char('v' | 'V')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.input.value.trim().is_empty() =>
            {
                self.toggle_diff_view();
                Action::None
            }
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

    fn handle_approval_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => self.deny_pending_tool(),
            KeyCode::Enter => self.approve_pending_tool(false),
            KeyCode::Char('a' | 'A')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.approve_pending_tool(true)
            }
            KeyCode::Char('v' | 'V')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.toggle_diff_view();
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = 0;
                }
                Action::None
            }
            KeyCode::Up => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = approval.scroll.saturating_sub(1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = approval.scroll.saturating_add(1);
                }
                Action::None
            }
            KeyCode::PageUp => {
                if let Some(approval) = self.approval.as_mut() {
                    let page = usize::from(approval.page_height.max(1));
                    approval.scroll = approval.scroll.saturating_sub(page);
                }
                Action::None
            }
            KeyCode::PageDown => {
                if let Some(approval) = self.approval.as_mut() {
                    let page = usize::from(approval.page_height.max(1));
                    approval.scroll = approval.scroll.saturating_add(page);
                }
                Action::None
            }
            KeyCode::Home => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = 0;
                }
                Action::None
            }
            KeyCode::End => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = usize::MAX;
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Switch every visible diff between unified and side-by-side.
    fn toggle_diff_view(&mut self) {
        self.diff_sides = !self.diff_sides;
        for report in self.changes.values_mut() {
            report.side_by_side = self.diff_sides;
        }
    }

    fn approve_pending_tool(&mut self, always: bool) -> Action {
        self.approval = None;
        let Some(call) = self.pending_tools.pop_front() else {
            return Action::None;
        };
        if always {
            self.approved_tools.insert(call.function.name.clone());
        }
        self.start_tool(call)
    }

    fn deny_pending_tool(&mut self) -> Action {
        self.approval = None;
        let Some(call) = self.pending_tools.pop_front() else {
            return Action::None;
        };
        self.record_tool_result(
            call.id,
            "The user denied this tool call. Do not retry it without asking.",
        );
        self.follow_tail = true;
        self.save_session();
        self.advance_tools()
    }

    fn start_tool(&mut self, call: ToolCall) -> Action {
        let preview = tools::change_preview(&call);
        self.running_tool = Some(RunningTool {
            existed: tools::target_existed(&call),
            before: preview.before,
            call: call.clone(),
            started: Instant::now(),
        });
        self.follow_tail = true;
        Action::RunTool { call }
    }

    fn advance_tools(&mut self) -> Action {
        if self.is_streaming() || self.approval.is_some() || self.running_tool.is_some() {
            return Action::None;
        }
        let Some(call) = self.pending_tools.front().cloned() else {
            return self.continue_after_tools();
        };
        if self.tools_mode.needs_approval(&call, &self.approved_tools) {
            let change = tools::change_preview(&call);
            self.approval = Some(ToolApproval {
                call,
                note: change.note,
                scroll: 0,
                page_height: 0,
            });
            self.follow_tail = true;
            return Action::None;
        }
        self.pending_tools.pop_front();
        self.start_tool(call)
    }

    fn continue_after_tools(&mut self) -> Action {
        if !self
            .session
            .messages
            .last()
            .is_some_and(|message| message.role == Role::Tool)
        {
            return Action::None;
        }
        if self.tool_rounds >= MAX_TOOL_ROUNDS {
            self.error(format!(
                "Stopped after {MAX_TOOL_ROUNDS} tool rounds. Send another message to continue."
            ));
            self.save_session();
            return Action::None;
        }
        let messages = self.session.messages.clone();
        self.begin_request(messages)
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
            ConnectStage::ProviderName
            | ConnectStage::Endpoint
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
                    // A saved endpoint's manager backs out to the provider list;
                    // the create flow backs out to the endpoint field.
                    connect.stage = if connect.custom_provider_id.is_some() {
                        ConnectStage::Providers
                    } else {
                        ConnectStage::Endpoint
                    };
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
        if stage == ConnectStage::ProviderName {
            self.connect = None;
            return;
        }
        let Some(connect) = self.connect.as_mut() else {
            return;
        };
        connect.error = None;
        // While editing a saved endpoint, back returns to its model list.
        if connect.custom_provider_id.is_some()
            && matches!(stage, ConnectStage::Endpoint | ConnectStage::CustomToken)
        {
            connect.stage = ConnectStage::CustomModels;
            connect.selected = 0;
            return;
        }
        match stage {
            ConnectStage::Endpoint => {
                connect.stage = ConnectStage::ProviderName;
                connect.selected = 0;
            }
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
            ConnectStage::ProviderName => {
                let name = self
                    .connect
                    .as_ref()
                    .map(|connect| connect.custom_name.trim().to_owned())
                    .unwrap_or_default();
                if name.is_empty() {
                    self.connect_error("Enter a name for this provider.");
                    return Action::None;
                }
                if let Some(connect) = self.connect.as_mut() {
                    connect.custom_name = name;
                    connect.stage = ConnectStage::Endpoint;
                    connect.selected = 0;
                    connect.error = None;
                }
            }
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
                    // Editing a saved endpoint returns to its model list; the
                    // create flow continues on to pick an authorization kind.
                    if connect.custom_provider_id.is_some() {
                        connect.stage = ConnectStage::CustomModels;
                        connect.selected = 0;
                        connect.error = None;
                        return Action::None;
                    }
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
                    if connect.custom_provider_id.is_some() {
                        connect.stage = ConnectStage::CustomModels;
                        connect.selected = 0;
                        connect.error = None;
                        return Action::None;
                    }
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
                // A saved endpoint is identified by `custom_id`; its row id can
                // legitimately equal the reserved `custom` id used above.
                if let Some(saved_id) = &item.custom_id {
                    let Some(saved) = connect
                        .saved_custom
                        .iter()
                        .find(|provider| &provider.id == saved_id)
                    else {
                        return Action::None;
                    };
                    connect.provider_id = None;
                    connect.custom_flow = true;
                    connect.custom_provider_id = Some(saved.id.clone());
                    connect.custom_name = saved.name.clone();
                    connect.custom_endpoint = saved.base_url.clone();
                    connect.custom_auth = saved.auth;
                    connect.custom_models = saved.models.clone();
                    connect.custom_token = saved.token.clone().unwrap_or_default();
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    // Land on the manager so the endpoint, key and models can be
                    // edited; the token is one of its rows.
                    connect.stage = ConnectStage::CustomModels;
                } else if item.id == CUSTOM_PROVIDER_ID {
                    connect.provider_id = None;
                    connect.custom_flow = true;
                    connect.custom_provider_id = None;
                    connect.custom_name.clear();
                    connect.custom_endpoint.clear();
                    connect.custom_token.clear();
                    connect.custom_models.clear();
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    connect.stage = ConnectStage::ProviderName;
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
                if item.id == ADD_MODEL_ID {
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    connect.stage = ConnectStage::ModelId;
                    connect.selected = 0;
                    connect.error = None;
                    return Action::None;
                }
                if item.id == EDIT_ENDPOINT_ID {
                    // Keep the current value in the field so it can be amended.
                    connect.stage = ConnectStage::Endpoint;
                    connect.selected = 0;
                    connect.error = None;
                    return Action::None;
                }
                if item.id == EDIT_TOKEN_ID {
                    connect.stage = ConnectStage::CustomToken;
                    connect.selected = 0;
                    connect.error = None;
                    return Action::None;
                }
                connect.model_id = Some(item.id);
                connect.password.clear();
                connect.stage = ConnectStage::Password;
                connect.selected = 0;
                connect.error = None;
            }
            ConnectStage::ApiKey
            | ConnectStage::ProviderName
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
            custom_provider_id: None,
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
                if connect.custom_name.trim().is_empty() {
                    "Custom".to_owned()
                } else {
                    connect.custom_name.trim().to_owned()
                },
                connect.custom_provider_id.clone(),
            )
        };
        let (endpoint, model_id, auth, token, models, name, custom_provider_id) = plan;
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
            provider_name: name.clone(),
            model: model_id.clone(),
            base_url: endpoint.clone(),
            api_key: api_key.clone(),
            api_key_env: None,
            auth,
            models: models.clone(),
            custom_provider_id,
        };
        let saved = config::save_connection(&self.config_path, &connection, password.as_deref());
        self.backend.configure(
            ProviderKind::Custom,
            name.clone(),
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
            self.custom_providers = config::load_custom_providers(&self.config_path);
            self.info(format!("Connected to {name} · {model_id}"));
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
        if self.is_busy() {
            self.info("Wait for the current response or tool to finish.");
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
        self.tool_rounds = 0;
        self.session.update_title(prompt);
        self.session.messages.push(Message::new(Role::User, value));
        let messages = self.session.messages.clone();
        self.begin_request(messages)
    }

    fn begin_request(&mut self, messages: Vec<Message>) -> Action {
        self.session
            .messages
            .push(Message::new(Role::Assistant, ""));
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let request_id = self.next_request_id;
        let tools_enabled = self.tools_mode.advertised();
        let prompt_chars = tools::system_prompt(tools_enabled).chars().count()
            + messages
                .iter()
                .map(|message| {
                    message.content.chars().count()
                        + message.tool_calls.as_ref().map_or(0, |calls| {
                            calls
                                .iter()
                                .map(|call| {
                                    call.function.name.chars().count()
                                        + call.function.arguments.chars().count()
                                })
                                .sum::<usize>()
                        })
                })
                .sum::<usize>();
        self.active = Some(ActiveRequest {
            id: request_id,
            started: Instant::now(),
            prompt_tokens: prompt_chars as u64 / 4,
            usage: None,
            tool_progress: None,
        });
        self.follow_tail = true;
        self.save_session();
        Action::Start {
            request_id,
            messages,
            tools_enabled,
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
            "/tools" => {
                let value = parts.next().unwrap_or_default().to_ascii_lowercase();
                if value.is_empty() {
                    self.info(format!(
                        "Tools: {} — {} · /tools on|off|auto|ask",
                        self.tools_mode.label(),
                        self.tools_mode.detail()
                    ));
                } else if let Some(mode) = ToolsMode::parse(&value) {
                    self.tools_mode = mode;
                    let suffix = if mode.advertised()
                        && !matches!(
                            self.backend.provider_kind(),
                            ProviderKind::OpenAi | ProviderKind::Custom
                        ) {
                        " · tools require an OpenAI-compatible provider"
                    } else {
                        ""
                    };
                    self.info(format!(
                        "Tools: {} — {}{suffix}",
                        mode.label(),
                        mode.detail()
                    ));
                } else {
                    self.error("Usage: /tools on|off|auto|ask");
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
        if self.is_busy() {
            self.info("Finish the current turn before connecting a provider.");
            return Action::None;
        }
        self.palette_hidden = true;
        let mut connect = ConnectState::loading();
        connect.saved_custom = self.custom_providers.clone();
        if self.backend.provider_kind() == ProviderKind::Custom {
            connect.custom_endpoint = self.backend.base_url().to_owned();
            connect.custom_auth = self.backend.auth();
            connect.custom_models = self.backend.saved_models().to_vec();
            if let Some(matched) = self
                .custom_providers
                .iter()
                .find(|provider| provider.base_url == connect.custom_endpoint)
            {
                connect.custom_provider_id = Some(matched.id.clone());
                connect.custom_name = matched.name.clone();
                connect.custom_token = matched.token.clone().unwrap_or_default();
            }
        }
        self.connect = Some(connect);
        Action::LoadCatalog
    }

    fn new_session(&mut self) {
        if self.is_busy() {
            self.info("Finish the current turn before starting a new session.");
            return;
        }
        self.session = Session::new();
        self.scroll = 0;
        self.follow_tail = true;
        self.notice = None;
        self.tool_rounds = 0;
        self.pending_tools.clear();
        self.approval = None;
        self.approved_tools.clear();
        self.save_session();
    }

    fn stop_streaming(&mut self) -> Action {
        let Some(active) = self.active.take() else {
            return Action::None;
        };
        self.discard_pending_tools("The user stopped the response before this tool could run.");
        self.tool_rounds = 0;
        self.remove_empty_response();
        self.info("Response stopped. Partial output was kept.");
        self.save_session();
        Action::Cancel(active.id)
    }

    pub fn on_stream_event(&mut self, event: StreamEvent) -> Action {
        match event {
            StreamEvent::Delta { request_id, text }
                if self.active_request_id() == Some(request_id) =>
            {
                if let Some(message) = self.session.messages.last_mut() {
                    message.content.push_str(&text);
                }
                Action::None
            }
            StreamEvent::Reasoning { request_id, text }
                if self.active_request_id() == Some(request_id) =>
            {
                if let Some(message) = self.session.messages.last_mut() {
                    message.reasoning.push_str(&text);
                }
                Action::None
            }
            StreamEvent::ToolProgress {
                request_id,
                name,
                detail,
                bytes,
            } if self.active_request_id() == Some(request_id) => {
                if let Some(active) = self.active.as_mut() {
                    active.tool_progress = Some((name, detail, bytes));
                }
                Action::None
            }
            StreamEvent::Usage {
                request_id,
                prompt_tokens,
                completion_tokens,
            } if self.active_request_id() == Some(request_id) => {
                if let Some(active) = self.active.as_mut() {
                    let usage = active.usage.get_or_insert((0, 0));
                    usage.0 = usage.0.max(prompt_tokens);
                    usage.1 = usage.1.max(completion_tokens);
                }
                Action::None
            }
            StreamEvent::ToolCalls { request_id, calls }
                if self.active_request_id() == Some(request_id) =>
            {
                if let Some(message) = self.session.messages.last_mut() {
                    if message.role == Role::Assistant {
                        message.tool_calls = Some(calls.clone());
                    }
                }
                self.tool_rounds = self.tool_rounds.saturating_add(1);
                self.pending_tools.extend(calls);
                self.follow_tail = true;
                self.save_session();
                Action::None
            }
            StreamEvent::Done { request_id } if self.active_request_id() == Some(request_id) => {
                // Fold this request's billed tokens into the session total.
                if let Some((prompt, completion)) = self.active.as_ref().and_then(|a| a.usage) {
                    self.spent_prompt_tokens = self.spent_prompt_tokens.saturating_add(prompt);
                    self.spent_completion_tokens =
                        self.spent_completion_tokens.saturating_add(completion);
                }
                // Record what the reply cost and how long it took, so the
                // transcript can show it under the finished message.
                let finished = self.active.as_ref().map(|active| {
                    let elapsed_ms = u64::try_from(active.started.elapsed().as_millis()).unwrap_or(0);
                    let (prompt_tokens, completion_tokens) = active
                        .usage
                        .unwrap_or((active.prompt_tokens, estimated_output_tokens(&self.session)));
                    (prompt_tokens, completion_tokens, elapsed_ms)
                });
                if let Some((prompt_tokens, completion_tokens, elapsed_ms)) = finished {
                    if let Some(message) = self.session.messages.last_mut() {
                        if message.role == Role::Assistant {
                            message.usage = Some(session::Usage {
                                prompt_tokens,
                                completion_tokens,
                                elapsed_ms,
                            });
                        }
                    }
                }
                self.active = None;
                if self.pending_tools.is_empty() {
                    self.tool_rounds = 0;
                    self.remove_empty_response();
                    self.save_session();
                    Action::None
                } else {
                    self.save_session();
                    self.advance_tools()
                }
            }
            StreamEvent::Error {
                request_id,
                message,
            } if self.active_request_id() == Some(request_id) => {
                self.active = None;
                self.discard_pending_tools("The response failed before this tool call could run.");
                self.tool_rounds = 0;
                self.remove_empty_response();
                self.error(message);
                self.save_session();
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn on_tool_result(&mut self, event: ToolEvent) -> Action {
        let Some(running) = self
            .running_tool
            .take()
            .filter(|running| running.call.id == event.call.id)
        else {
            return Action::None;
        };
        let before = running.before;
        let existed = running.existed;
        let content = match event.result {
            Ok(output) => {
                if let Some(diff) = tools::applied_change(&event.call, before, existed) {
                    self.changes.insert(
                        event.call.id.clone(),
                        ChangeReport {
                            diff,
                            side_by_side: self.diff_sides,
                        },
                    );
                }
                output
            }
            Err(error) => format!("Error: {error}"),
        };
        self.record_tool_result(event.call.id, content);
        self.save_session();
        self.advance_tools()
    }

    fn record_tool_result(&mut self, call_id: impl Into<String>, content: impl Into<String>) {
        self.session
            .messages
            .push(Message::tool_result(call_id, content));
    }

    fn discard_pending_tools(&mut self, reason: &str) {
        while let Some(call) = self.pending_tools.pop_front() {
            self.record_tool_result(call.id, reason);
        }
        self.approval = None;
    }

    fn remove_empty_response(&mut self) {
        // A reply that only produced reasoning is kept: long thinking is real
        // output the user asked for, so dropping it loses the whole answer.
        if self.session.messages.last().is_some_and(|message| {
            message.role == Role::Assistant
                && message.content.is_empty()
                && message.reasoning.trim().is_empty()
                && message.tool_calls.is_none()
        }) {
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

fn estimated_output_tokens(session: &Session) -> u64 {
    match session.messages.last() {
        Some(message) if message.role == Role::Assistant => {
            (message.content.chars().count() + message.reasoning.chars().count()) as u64 / 4
        }
        _ => 0,
    }
}

pub fn format_tokens(tokens: u64) -> String {
    if tokens < 1000 {
        tokens.to_string()
    } else {
        format!("{:.1}k", tokens as f64 / 1000.0)
    }
}

/// Dollars per million tokens, `(input, output)`.
///
/// These are list prices and only cover the common models; an unknown model
/// falls back to the default so the counter still moves instead of lying flat.
fn model_price(model: &str) -> (f64, f64) {
    let model = model.to_ascii_lowercase();
    let table: [(&str, f64, f64); 8] = [
        ("gpt-4o-mini", 0.15, 0.60),
        ("gpt-4o", 2.50, 10.0),
        ("gpt-4.1", 2.00, 8.00),
        ("o3", 2.00, 8.00),
        ("o4-mini", 1.10, 4.40),
        ("claude-3-5-sonnet", 3.00, 15.0),
        ("claude-3-5-haiku", 0.80, 4.00),
        ("deepseek", 0.27, 1.10),
    ];
    // Longest match wins so `gpt-4o-mini` does not bill as `gpt-4o`.
    let mut best: Option<(&str, f64, f64)> = None;
    for (name, input, output) in table {
        if model.contains(name) && best.is_none_or(|(seen, _, _)| name.len() > seen.len()) {
            best = Some((name, input, output));
        }
    }
    best.map_or((2.50, 10.0), |(_, input, output)| (input, output))
}

/// Session spend as `$0.0000`, priced from the active model.
pub fn format_cost(prompt_tokens: u64, completion_tokens: u64, model: &str) -> String {
    let (input, output) = model_price(model);
    let dollars = (prompt_tokens as f64 * input + completion_tokens as f64 * output) / 1_000_000.0;
    // Small sessions cost fractions of a cent, so keep four decimals until it
    // is worth showing cents.
    if dollars < 1.0 {
        format!("${dollars:.4}")
    } else {
        format!("${dollars:.2}")
    }
}

pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{COMMANDS, Input, format_bytes, format_tokens, matching_commands};

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
            vec!["/help", "/new", "/connect", "/model", "/tools", "/quit"]
        );
        assert_eq!(command_names("/c"), vec!["/connect"]);
        assert_eq!(command_names("/m"), vec!["/model"]);
        assert_eq!(command_names("/t"), vec!["/tools"]);
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

    #[test]
    fn saved_provider_row_is_identified_by_custom_id() {
        use super::{CUSTOM_PROVIDER_ID, ConnectStage, ConnectState};
        use crate::config::{AuthKind, CustomProvider};

        // A legacy config with no custom_providers list is folded in with the
        // slug of its name, which can equal the reserved new-provider id.
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.saved_custom.push(CustomProvider {
            id: CUSTOM_PROVIDER_ID.to_owned(),
            name: "Custom".to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            auth: AuthKind::Bearer,
            models: Vec::new(),
            token: None,
            encrypted: false,
        });
        let items = connect.items_using("https://example.test/v1", "model");
        assert_eq!(items[0].custom_id, None, "first row adds a provider");
        assert_eq!(items[1].custom_id.as_deref(), Some(CUSTOM_PROVIDER_ID));
        assert!(items[1].active);
    }

    #[test]
    fn formats_keep_tokens_and_bytes_compact() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1200), "1.2k");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2150), "2.1 KB");
    }
}
