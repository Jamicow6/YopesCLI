use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::PathBuf,
    time::{Instant, SystemTime},
};

use anyhow::Result;
use base64::Engine as _;
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

use crate::{
    backend::{Backend, StreamEvent},
    catalog::{self, Catalog},
    config::{self, AuthKind, Connection, ProviderKind, SavedModel},
    diff::FileDiff,
    session::{self, ImageAttachment, Message, Role, Session, TaskState, ToolCall},
    skills, tools,
};

mod connect;
mod format;
mod images;
mod keys;
mod mode;
mod palette;
mod pickers;
mod stream;
mod turn;

pub use format::{format_bytes, format_cost, format_tokens};
pub use mode::{Mode, ToolsMode};

/// Warn this many rounds before the stop, so a long turn is not a surprise.
const TOOL_ROUND_HEADS_UP: usize = 10;

/// The tool-round brake: real agentic turns regularly need far more than what a
/// belt-tightening 24 allowed, but the brake still exists so a looping model
/// cannot bill forever without a human once per hundred rounds.
const MAX_TOOL_ROUNDS: usize = 100;
/// The choices the /effort command accepts, printed in its messages.
const EFFORT_CHOICES: &str = "Usage: /effort off|none|low|medium|high|xhigh";

/// Estimated prompt tokens that trigger compaction of older turns.
const COMPACTION_THRESHOLD_TOKENS: u64 = 32_000;
/// Tokens of recent history kept intact when compaction runs.
const COMPACTION_KEEP_TOKENS: u64 = 12_000;

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

pub const COMMANDS: [CommandSpec; 11] = [
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
        name: "/sessions",
        description: "Switch between saved sessions",
        argument: None,
    },
    CommandSpec {
        name: "/tools",
        description: "Set tool mode: on, off, auto, ask",
        argument: Some("mode"),
    },
    CommandSpec {
        name: "/system",
        description: "Show or set the custom system prompt",
        argument: Some("text"),
    },
    CommandSpec {
        name: "/compact",
        description: "Compact history now; auto on|off",
        argument: Some("on|off|now"),
    },
    CommandSpec {
        name: "/effort",
        description: "Show or set reasoning effort",
        argument: Some("level"),
    },
    CommandSpec {
        name: "/skills",
        description: "List installed skills",
        argument: None,
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
    /// Set while this request summarizes older turns instead of replying.
    compacting: Option<Compaction>,
}

/// A compaction in flight: which history prefix it summarizes and the text
/// streamed back so far.
struct Compaction {
    split: usize,
    text: String,
}

pub struct ToolApproval {
    pub call: ToolCall,
    pub note: Option<String>,
    /// Change previews of a write or edit, drawn in the dialog itself.
    pub diffs: Vec<FileDiff>,
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
    /// Every file the call may change and whether each existed before.
    pub targets: Vec<(String, bool)>,
}

/// Filter state of the `/model` popup.
#[derive(Debug, Clone, Default)]
pub struct ListPicker {
    pub filter: String,
    pub selected: usize,
}

/// Saved models matching the picker filter, in stored order.
fn model_matching<'a>(models: &'a [SavedModel], filter: &str) -> Vec<&'a SavedModel> {
    let filter = filter.to_ascii_lowercase();
    models
        .iter()
        .filter(|model| {
            filter.is_empty()
                || model.name.to_ascii_lowercase().contains(&filter)
                || model.id.to_ascii_lowercase().contains(&filter)
        })
        .collect()
}

/// One row of the `/sessions` popup: a saved session file with its title.
#[derive(Clone)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub title: String,
    pub modified: Option<SystemTime>,
}

/// Title probe: parse only the `title` field out of a session file.
#[derive(Default, serde::Deserialize)]
struct SessionProbe {
    #[serde(default)]
    title: String,
}

fn session_entry(path: PathBuf) -> SessionEntry {
    let title = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<SessionProbe>(&text).ok())
        .map(|probe| probe.title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Untitled".to_owned());
    let modified = fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    SessionEntry {
        path,
        title,
        modified,
    }
}

/// Saved sessions matching the picker filter, newest first.
fn session_matching<'a>(sessions: &'a [SessionEntry], filter: &str) -> Vec<&'a SessionEntry> {
    let filter = filter.to_ascii_lowercase();
    sessions
        .iter()
        .filter(|entry| filter.is_empty() || entry.title.to_ascii_lowercase().contains(&filter))
        .collect()
}

pub struct ToolEvent {
    pub call: ToolCall,
    pub result: std::result::Result<String, String>,
}

/// A diff shown under a tool result in the conversation.
pub struct ChangeReport {
    pub diffs: Vec<FileDiff>,
    pub side_by_side: bool,
}

pub const CUSTOM_PROVIDER_ID: &str = "custom";
/// Preferred provider row in /connect: Fasfas AI above models.dev.
pub const FASFAST_ID: &str = catalog::FASFAST_ID;
const ADD_MODEL_ID: &str = "add-model";
/// Rows that edit an existing saved endpoint from the model list.
const EDIT_ENDPOINT_ID: &str = "edit-endpoint";
const EDIT_TOKEN_ID: &str = "edit-token";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectStage {
    Providers,
    /// Preferred Fasfas flow: the Discord key notice, then the key input.
    FasfasNotice,
    FasfasKey,
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
            Self::FasfasNotice => "fasfas",
            Self::FasfasKey => "fasfas key",
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
        "fasfas" => "✦",
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
    /// The token was edited during this visit, so it needs saving again.
    custom_token_dirty: bool,
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
            custom_token_dirty: false,
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
            !active_base_url.is_empty() && endpoint.trim_end_matches('/') == active_base_url
        };
        match self.stage {
            ConnectStage::Providers => {
                let mut items = Vec::new();
                // The preferred provider always sits above everything else.
                if filter.is_empty()
                    || catalog::FASFAST_LABEL
                        .to_ascii_lowercase()
                        .contains(filter.as_str())
                    || catalog::FASFAST_ID.contains(filter.as_str())
                {
                    items.push(ConnectItem {
                        id: FASFAST_ID.to_owned(),
                        title: catalog::FASFAST_LABEL.to_owned(),
                        detail: "preferred · key from Discord".to_owned(),
                        logo: provider_logo(FASFAST_ID),
                        custom_id: None,
                        active: is_active(catalog::FASFAST_BASE_URL),
                    });
                }
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
            ConnectStage::FasfasNotice => vec![ConnectItem {
                id: catalog::FASFAST_DISCORD.to_owned(),
                title: "Get a key in Discord".to_owned(),
                // Scheme dropped so the invite fits the list column.
                detail: catalog::FASFAST_DISCORD
                    .trim_start_matches("https://")
                    .to_owned(),
                logo: provider_logo(FASFAST_ID),
                custom_id: None,
                active: false,
            }],
            ConnectStage::FasfasKey => Vec::new(),
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

    /// Drop the model row the user is on in the saved endpoint's model list.
    ///
    /// Returns the model id so the caller can persist the deletion; special
    /// rows (add model, change endpoint, change key) are never removable.
    fn remove_selected_model(&mut self) -> Option<String> {
        if self.stage != ConnectStage::CustomModels {
            return None;
        }
        let items = self.items();
        let item = items.get(self.selected.min(items.len().saturating_sub(1)))?;
        if matches!(
            item.id.as_str(),
            ADD_MODEL_ID | EDIT_ENDPOINT_ID | EDIT_TOKEN_ID
        ) {
            return None;
        }
        let id = item.id.clone();
        self.custom_models.retain(|model| model.id != id);
        self.selected = self.selected.min(self.items().len().saturating_sub(1));
        self.error = None;
        Some(id)
    }

    pub fn input(&self) -> String {
        if self.stage == ConnectStage::Auth {
            return self.auth_at_selection().label().to_owned();
        }
        if self.stage == ConnectStage::CustomModels || self.stage == ConnectStage::FasfasNotice {
            let items = self.items();
            return items
                .get(self.selected.min(items.len().saturating_sub(1)))
                .map_or_else(|| "none".to_owned(), |item| item.title.clone());
        }
        let value = match self.stage {
            ConnectStage::ApiKey | ConnectStage::FasfasKey => &self.api_key,
            ConnectStage::Password => &self.password,
            ConnectStage::ProviderName => &self.custom_name,
            ConnectStage::Endpoint => &self.custom_endpoint,
            ConnectStage::CustomToken => &self.custom_token,
            ConnectStage::ModelId => &self.custom_model_id,
            ConnectStage::ModelName => &self.custom_model_name,
            ConnectStage::Providers | ConnectStage::Models => &self.filter,
            ConnectStage::FasfasNotice => unreachable!("handled above"),
            ConnectStage::Auth | ConnectStage::CustomModels => unreachable!(),
        };
        if matches!(
            self.stage,
            ConnectStage::ApiKey
                | ConnectStage::Password
                | ConnectStage::CustomToken
                | ConnectStage::FasfasKey
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
            ConnectStage::FasfasNotice => "fasfas",
            ConnectStage::FasfasKey => "fasfas key",
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
        !matches!(
            self.stage,
            ConnectStage::Auth | ConnectStage::CustomModels | ConnectStage::FasfasNotice
        )
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

    /// What the loading row of /connect names as its source: the initial
    /// fetch fills the provider list, the Fasfas key fetch lists models.
    pub const fn loading_detail(&self) -> &'static str {
        match self.stage {
            ConnectStage::FasfasKey => "  loading models from Fasfas AI",
            _ => "  loading providers from models.dev",
        }
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
        /// Mode instruction appended to the system prompt for this request.
        mode_note: Option<&'static str>,
    },
    Cancel(u64),
    RunTool {
        call: ToolCall,
    },
    /// Read the clipboard on Ctrl+V and attach an image when it holds one.
    PasteImage,
    Quit,
    LoadCatalog,
    /// Fetch the Fasfas `/v1/models` list with the key entered in /connect.
    FetchFasfas(String),
    /// Ask the provider for a short title after the first reply of a session.
    NameSession {
        task: String,
    },
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

    /// Move vertically while preserving the cursor's character column.
    fn move_vertical(&mut self, down: bool) {
        let before = &self.value[..self.cursor];
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let column = before[line_start..].chars().count();
        let line_end = self.value[line_start..]
            .find('\n')
            .map_or(self.value.len(), |n| line_start + n);
        let target_start = if down {
            if line_end == self.value.len() {
                return;
            }
            line_end + 1
        } else {
            if line_start == 0 {
                return;
            }
            self.value[..line_start - 1]
                .rfind('\n')
                .map_or(0, |n| n + 1)
        };
        let target_end = self.value[target_start..]
            .find('\n')
            .map_or(self.value.len(), |n| target_start + n);
        self.cursor = self.value[target_start..target_end]
            .char_indices()
            .nth(column)
            .map_or(target_end, |(offset, _)| target_start + offset);
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
    /// Submitted turn body: the composer prompt flashes for a few ticks.
    pub send_pulse: Option<usize>,
    pub tools_mode: ToolsMode,
    /// What the current turn is for: applying changes, planning, or chat.
    pub mode: Mode,
    /// Whether history is summarized automatically when the context fills.
    pub auto_compact: bool,
    /// Images pasted with Ctrl+V, sent with the next submitted message.
    pub attachments: Vec<ImageAttachment>,
    /// Counter giving each pasted image a unique display name.
    next_image: usize,
    /// Pre-rendered half-block rows of pasted images, keyed `name:width`.
    image_rows: HashMap<String, Vec<Line<'static>>>,
    /// Terminal width the cached image rows were rendered for.
    image_width: u16,
    pub approval: Option<ToolApproval>,
    /// Popup picker opened by `/model` with no argument.
    pub model_picker: Option<ListPicker>,
    /// Popup picker opened by `/sessions`.
    pub session_picker: Option<ListPicker>,
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
    /// Directory holding every saved session file, when saving.
    sessions_dir: Option<PathBuf>,
    config_path: PathBuf,
    custom_providers: Vec<config::CustomProvider>,
    /// Tavily key for the web_search tool.
    tavily_api_key: Option<String>,
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
        sessions_dir: Option<PathBuf>,
        config_path: PathBuf,
        custom_providers: Vec<config::CustomProvider>,
        tavily_api_key: Option<String>,
        auto_compact: bool,
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
            send_pulse: None,
            tools_mode: ToolsMode::default(),
            mode: Mode::default(),
            auto_compact,
            attachments: Vec::new(),
            next_image: 0,
            image_rows: HashMap::new(),
            image_width: 0,
            approval: None,
            model_picker: None,
            session_picker: None,
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
            sessions_dir,
            config_path,
            custom_providers,
            tavily_api_key,
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

    pub fn tavily_key(&self) -> Option<&str> {
        self.tavily_api_key.as_deref()
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

    /// Path of the session currently open, for the `/sessions` popup marker.
    pub fn active_session_path(&self) -> Option<&PathBuf> {
        self.session_path.as_ref()
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

    /// Estimated tokens the next request would send, the context window load.
    pub fn context_tokens(&self) -> u64 {
        Session::estimate_tokens(&self.session.messages)
    }

    pub fn tick(&mut self) {
        self.spinner = self.spinner.wrapping_add(1);
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
    use super::{
        COMMANDS, Input, format_bytes, format_tokens, images::build_image_rows, matching_commands,
    };

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
            vec![
                "/help",
                "/new",
                "/connect",
                "/model",
                "/sessions",
                "/tools",
                "/system",
                "/compact",
                "/effort",
                "/quit",
            ]
        );
        assert_eq!(command_names("/c"), vec!["/connect", "/compact"]);
        assert_eq!(command_names("/m"), vec!["/model"]);
        assert_eq!(command_names("/t"), vec!["/tools"]);
        assert_eq!(command_names("/e"), vec!["/effort"]);
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
        assert_eq!(items[0].id, "fasfas", "preferred provider comes first");
        assert_eq!(items[1].custom_id, None, "next row adds a provider");
        assert_eq!(items[2].custom_id.as_deref(), Some(CUSTOM_PROVIDER_ID));
        assert!(items[2].active);
    }

    #[test]
    fn fasfas_filter_hides_other_sections() {
        use super::{ConnectStage, ConnectState};

        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::Providers;
        connect.filter = "fasfas".to_owned();
        let items = connect.items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "fasfas");

        connect.filter = "custom".to_owned();
        let items = connect.items();
        assert!(items.iter().all(|item| item.id != "fasfas"));
    }

    #[test]
    fn formats_keep_tokens_and_bytes_compact() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1200), "1.2k");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2150), "2.1 KB");
    }

    #[test]
    fn delete_removes_only_a_model_row() {
        use super::{ADD_MODEL_ID, ConnectStage, ConnectState};
        use crate::config::SavedModel;

        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::CustomModels;
        connect.custom_models = vec![
            SavedModel {
                id: "one".to_owned(),
                name: "One".to_owned(),
            },
            SavedModel {
                id: "two".to_owned(),
                name: "Two".to_owned(),
            },
        ];
        assert_eq!(connect.remove_selected_model().as_deref(), Some("one"));
        assert_eq!(connect.custom_models.len(), 1);

        // The action rows (add model, change endpoint) are never removable.
        let last = connect.items().len().saturating_sub(1);
        connect.selected = last;
        assert!(connect.remove_selected_model().is_none());
        assert_eq!(connect.custom_models.len(), 1);
        assert_eq!(connect.items()[last].id, ADD_MODEL_ID);

        connect.custom_provider_id = Some("endpoint".to_owned());
        let last = connect.items().len().saturating_sub(1);
        connect.selected = last;
        assert!(connect.remove_selected_model().is_none());
        assert_eq!(connect.custom_models.len(), 1);
        assert_eq!(connect.custom_models[0].id, "two");
    }

    #[test]
    fn the_mode_cycle_and_denials_match_the_labels() {
        use super::Mode;
        assert_eq!(Mode::default(), Mode::Build);
        assert_eq!(Mode::Build.next(), Mode::Plan);
        assert_eq!(Mode::Plan.next(), Mode::Chat);
        assert_eq!(Mode::Chat.next(), Mode::Build);
        assert!(!Mode::Build.denied("write_file"));
        assert!(!Mode::Plan.denied("read_file"));
        assert!(!Mode::Plan.denied("web_search"));
        assert!(Mode::Plan.denied("write_file"));
        assert!(Mode::Plan.denied("edit_file"));
        assert!(Mode::Plan.denied("multi_edit"));
        assert!(Mode::Plan.denied("run_command"));
        assert!(Mode::Plan.denied("bash_background"));
        assert!(Mode::Chat.denied("read_file"));
        assert!(
            Mode::Plan
                .instruction()
                .is_some_and(|note| note.contains("PLAN"))
        );
        assert!(
            Mode::Build
                .instruction()
                .is_some_and(|note| note.contains("BUILD"))
        );
    }

    #[test]
    fn an_image_renders_as_half_block_pixel_rows() {
        use base64::Engine as _;
        use ratatui::style::Color;

        let png = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 2, |_, _| {
            image::Rgb([255u8, 0, 0])
        }));
        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        png.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let rows = build_image_rows(&data, 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spans.len(), 4);
        assert_eq!(rows[0].spans[0].content, "▄");
        assert_eq!(rows[0].spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(rows[0].spans[3].style.fg, Some(Color::Rgb(255, 0, 0)));
    }

    #[test]
    fn model_matching_filters_on_name_and_id() {
        use super::model_matching;
        use crate::config::SavedModel;

        let models = vec![
            SavedModel {
                id: "glm-4.7".into(),
                name: "GLM 4.7".into(),
            },
            SavedModel {
                id: "gpt-5.2".into(),
                name: String::new(),
            },
            SavedModel {
                id: "claude-haiku".into(),
                name: "Claude Haiku".into(),
            },
        ];

        assert_eq!(
            model_matching(&models, "").len(),
            3,
            "empty filter keeps everything"
        );
        let found = model_matching(&models, "GLM");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "glm-4.7");
        let found = model_matching(&models, "gpt");
        assert_eq!(found.len(), 1, "matches the id when the name is empty");
        assert_eq!(found[0].id, "gpt-5.2");
        assert!(model_matching(&models, "llama").is_empty());
    }
}
