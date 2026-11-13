//! Configuration: CLI arguments, the resolved `Config`, provider kinds, and
//! the paths where state lives.
//!
//! `mod.rs` holds the types every other module touches. `cli` owns the
//! argument parsing, and `saved` owns the saved-config JSON and everything
//! that reads or writes it.

mod cli;
mod saved;

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::credentials;

pub use cli::{Cli, Command};
use saved::{custom_providers, load_saved, resolve_api_key};
pub(crate) use saved::{
    delete_custom_provider, delete_saved_model, disconnect_provider, load_connected_providers,
    load_custom_providers, load_endpoint_models, load_endpoint_secret, rename_custom_provider,
    save_auto_compact, save_connection, save_model, save_reasoning_effort, save_system_prompt,
    update_model_context_window,
};

const CONFIG_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    #[value(skip)]
    Unconfigured,
    #[value(name = "openai")]
    OpenAi,
    Ollama,
    #[value(skip)]
    Custom,
}

impl ProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unconfigured => "unconfigured",
            Self::OpenAi => "openai",
            Self::Ollama => "ollama",
            Self::Custom => "custom",
        }
    }

    fn parse_env(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "none" | "unconfigured" => Ok(Self::Unconfigured),
            "openai" | "openai-compatible" => Ok(Self::OpenAi),
            "ollama" => Ok(Self::Ollama),
            _ => bail!("invalid YOPES_PROVIDER `{value}`; expected openai or ollama"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthKind {
    #[default]
    Bearer,
    XApiKey,
    None,
}

impl AuthKind {
    pub const fn choices() -> [Self; 3] {
        [Self::Bearer, Self::XApiKey, Self::None]
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::Bearer => "bearer",
            Self::XApiKey => "x-api-key",
            Self::None => "none",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer token",
            Self::XApiKey => "x-api-key",
            Self::None => "No auth",
        }
    }

    pub const fn detail(self) -> &'static str {
        match self {
            Self::Bearer => "Authorization: Bearer <token>",
            Self::XApiKey => "x-api-key: <token>",
            Self::None => "send no authorization header",
        }
    }

    pub const fn requires_token(self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::choices().into_iter().find(|auth| auth.id() == value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedModel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// Dollars per million input tokens, when models.dev lists a price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost: Option<f64>,
    /// Dollars per million output tokens, when models.dev lists a price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost: Option<f64>,
    /// Manually set context-window size in tokens (e.g. 128000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub auth: AuthKind,
    pub models: Vec<SavedModel>,
    /// Where to fetch the provider's model list; defaults to `<base>/models`.
    pub models_url: Option<String>,
    /// Plaintext token when stored without encryption; `None` when encrypted or absent.
    pub token: Option<String>,
    pub encrypted: bool,
    /// The stored secret, encrypted or not.
    pub secret: StoredSecret,
}

/// A provider the user has connected, remembered so its key survives a switch
/// to another provider.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SavedProvider {
    /// The models.dev provider id, when the key came from the catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default)]
    pub provider_name: String,
    #[serde(default)]
    pub secret: StoredSecret,
    /// The model list this provider last offered, so /models can show models
    /// from every connected provider, not only the active one.
    #[serde(default)]
    pub models: Vec<SavedModel>,
}

/// A connected provider, for the interface to show what is already set up.
#[derive(Clone, Debug)]
pub struct ConnectedProvider {
    pub base_url: String,
    pub provider_id: Option<String>,
    pub name: String,
    #[allow(dead_code)]
    pub has_key: bool,
    #[allow(dead_code)]
    pub encrypted: bool,
    pub models: Vec<SavedModel>,
}

pub struct Config {
    pub provider: ProviderKind,
    pub provider_label: String,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub auth: AuthKind,
    pub models: Vec<SavedModel>,
    /// Custom system prompt text, appended to the built-in one.
    pub system_prompt: Option<String>,
    pub custom_providers: Vec<CustomProvider>,
    pub session_path: Option<PathBuf>,
    /// Directory holding every saved session file, when saving is on.
    pub sessions_dir: Option<PathBuf>,
    pub config_path: PathBuf,
    /// Tavily key backing the web_search tool.
    pub tavily_api_key: Option<String>,
    /// Whether compaction runs automatically when the context grows large.
    pub auto_compact: bool,
    /// Reasoning effort to request from OpenAI-compatible backends.
    pub reasoning_effort: Option<String>,
}

pub struct Connection {
    pub provider: ProviderKind,
    pub provider_name: String,
    pub model: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub auth: AuthKind,
    pub models: Vec<SavedModel>,
    /// Endpoint serving the provider's model list, when it differs from the
    /// default `<base>/models`.
    pub models_url: Option<String>,
    /// Id of the saved custom provider to update; `None` creates a new entry.
    pub custom_provider_id: Option<String>,
    /// The models.dev provider id, when the connection came from the catalog.
    pub provider_id: Option<String>,
    /// The key for this endpoint is already stored: keep that secret as it is
    /// rather than writing a new one.
    pub reuse_secret: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StoredSecret {
    #[serde(default)]
    pub encrypted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphertext: Option<String>,
}

impl StoredSecret {
    /// Whether a key is stored here at all.
    pub fn has_key(&self) -> bool {
        self.api_key.is_some() || self.ciphertext.is_some()
    }

    /// The key as stored, when it was saved without a password.
    pub fn plaintext(&self) -> Option<&str> {
        if self.encrypted {
            return None;
        }
        self.api_key
            .as_deref()
            .filter(|api_key| !api_key.trim().is_empty())
    }

    /// Unlock the key with a password. A secret saved without one opens with
    /// any password, so callers can treat the two the same way.
    pub fn unlock(&self, password: &str) -> Result<String> {
        if !self.encrypted {
            return Ok(self.plaintext().unwrap_or_default().to_owned());
        }
        let (Some(salt), Some(nonce), Some(ciphertext)) =
            (&self.salt, &self.nonce, &self.ciphertext)
        else {
            bail!("the stored API key is missing its encryption data");
        };
        credentials::open(password, salt, nonce, ciphertext)
    }

    /// Prompt for the password and unlock the key.
    fn resolve(&self) -> Result<Option<String>> {
        if !self.encrypted {
            return Ok(self
                .api_key
                .clone()
                .filter(|api_key| !api_key.trim().is_empty()));
        }
        let password = credentials::prompt_password("API key password: ")?;
        if password.is_empty() {
            bail!("an encryption password is required to unlock the stored API key");
        }
        Ok(Some(self.unlock(&password)?))
    }
}

impl Config {
    pub fn from_cli(cli: Cli) -> Result<Self> {
        Self::resolve(cli)
    }

    fn resolve(cli: Cli) -> Result<Self> {
        let config_path = default_config_path();
        let saved = load_saved(&config_path)?;

        let env_provider = env::var("YOPES_PROVIDER")
            .ok()
            .map(|value| ProviderKind::parse_env(&value))
            .transpose()?;
        let env_key = env::var("OPENAI_API_KEY")
            .ok()
            .filter(|api_key| !api_key.trim().is_empty());
        let provider = cli
            .provider
            .or(env_provider)
            .or_else(|| saved.as_ref().map(|saved| saved.provider))
            .or_else(|| env_key.as_ref().map(|_| ProviderKind::OpenAi))
            .unwrap_or(ProviderKind::Unconfigured);
        let saved_for_provider = saved.as_ref().filter(|saved| saved.provider == provider);

        let session_path = if cli.no_save {
            None
        } else {
            let explicit = cli
                .session
                .or_else(|| env::var("YOPES_SESSION").ok().map(PathBuf::from));
            Some(explicit.unwrap_or_else(default_session_with_history))
        };
        let sessions_dir = session_path.as_ref().map(|path| {
            path.parent()
                .unwrap_or_else(|| Path::new("."))
                .join("sessions")
        });

        if provider == ProviderKind::Unconfigured {
            return Ok(Self {
                provider,
                provider_label: "not connected".to_owned(),
                model: String::new(),
                base_url: String::new(),
                api_key: None,
                auth: AuthKind::Bearer,
                models: Vec::new(),
                system_prompt: None,
                custom_providers: custom_providers(saved.as_ref()),
                session_path,
                sessions_dir,
                config_path,
                tavily_api_key: None,
                auto_compact: saved.as_ref().is_none_or(|saved| saved.auto_compact),
                reasoning_effort: saved
                    .as_ref()
                    .and_then(|saved| saved.reasoning_effort.clone()),
            });
        }

        let provider_label = saved_for_provider
            .map(|saved| saved.provider_name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or(provider.as_str())
            .to_owned();

        let model = cli
            .model
            .or_else(|| env::var("YOPES_MODEL").ok())
            .or_else(|| {
                saved_for_provider
                    .map(|saved| saved.model.clone())
                    .filter(|model| !model.trim().is_empty())
            })
            .unwrap_or_else(|| default_model(provider));
        if model.trim().is_empty() {
            bail!("model name cannot be empty; run /connect");
        }

        let provider_base_var = match provider {
            ProviderKind::OpenAi => Some("OPENAI_BASE_URL"),
            ProviderKind::Ollama => Some("OLLAMA_BASE_URL"),
            ProviderKind::Unconfigured | ProviderKind::Custom => None,
        };
        let base_url = cli
            .base_url
            .or_else(|| env::var("YOPES_BASE_URL").ok())
            .or_else(|| provider_base_var.and_then(|name| env::var(name).ok()))
            .or_else(|| {
                saved_for_provider
                    .map(|saved| saved.base_url.clone())
                    .filter(|base_url| !base_url.trim().is_empty())
            })
            .unwrap_or_else(|| default_base_url(provider).to_owned());

        let auth = saved_for_provider.map_or(AuthKind::Bearer, |saved| saved.auth);
        let models = saved_for_provider
            .map(|saved| saved.models.clone())
            .unwrap_or_default();
        let env_system_prompt = env::var("YOPES_SYSTEM_PROMPT")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let system_prompt = cli
            .system_prompt
            .filter(|value| !value.trim().is_empty())
            .or(env_system_prompt)
            .or_else(|| {
                saved
                    .as_ref()
                    .and_then(|saved| saved.system_prompt.clone())
                    .filter(|value| !value.trim().is_empty())
            })
            .map(|value| value.trim().to_owned());
        let api_key = resolve_api_key(saved_for_provider, env_key)?;
        let tavily_api_key = env::var("TAVILY_API_KEY")
            .ok()
            .filter(|key| !key.trim().is_empty())
            .or_else(|| {
                saved
                    .as_ref()
                    .and_then(|saved| saved.tavily_api_key.clone())
                    .filter(|key| !key.trim().is_empty())
            });
        if provider == ProviderKind::OpenAi && api_key.is_none() && saved_for_provider.is_none() {
            bail!(
                "no API key for the OpenAI-compatible provider; set OPENAI_API_KEY or run /connect"
            );
        }

        Ok(Self {
            provider,
            provider_label,
            model,
            base_url,
            api_key,
            auth,
            models,
            system_prompt,
            custom_providers: custom_providers(saved.as_ref()),
            session_path,
            sessions_dir,
            config_path,
            tavily_api_key,
            auto_compact: saved.as_ref().is_none_or(|saved| saved.auto_compact),
            reasoning_effort: saved
                .as_ref()
                .and_then(|saved| saved.reasoning_effort.clone()),
        })
    }
}

fn default_model(provider: ProviderKind) -> String {
    match provider {
        ProviderKind::Unconfigured | ProviderKind::Custom => "",
        ProviderKind::OpenAi => "gpt-4o-mini",
        ProviderKind::Ollama => "llama3.2",
    }
    .to_owned()
}

fn default_base_url(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Unconfigured | ProviderKind::Custom => "",
        ProviderKind::OpenAi => "https://api.openai.com/v1",
        ProviderKind::Ollama => "http://localhost:11434",
    }
}

fn default_config_path() -> PathBuf {
    ProjectDirs::from("dev", "Yopes", "Yopes").map_or_else(
        || PathBuf::from(".yopes-config.json"),
        |dirs| dirs.config_dir().join("config.json"),
    )
}

fn default_session_path() -> PathBuf {
    ProjectDirs::from("dev", "Yopes", "Yopes").map_or_else(
        || PathBuf::from(".yopes-session.json"),
        |dirs| dirs.data_local_dir().join("session.json"),
    )
}

/// Per-session files live in a `sessions` directory next to the old single
/// session path; opening resumes the newest one and folds the legacy file in.
fn default_session_with_history() -> PathBuf {
    let legacy = default_session_path();
    let Some(data_dir) = legacy.parent() else {
        return legacy;
    };
    let dir = data_dir.join("sessions");
    if let Ok(list) = fs::read_dir(&dir) {
        let mut sessions: Vec<(SystemTime, PathBuf)> = list
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|x| x == "json"))
            .filter_map(|path| {
                fs::metadata(&path)
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .map(|modified| (modified, path))
            })
            .collect();
        // Ascending sort then pop: the most recently active session is last.
        sessions.sort();
        if let Some((_, newest)) = sessions.pop() {
            return newest;
        }
    }
    let named = |stamp: u128| dir.join(format!("session-{stamp}.json"));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis());
    if legacy.exists()
        && fs::metadata(&legacy)
            .map(|metadata| metadata.len() > 0)
            .unwrap_or(false)
        && fs::rename(&legacy, named(stamp)).is_ok()
    {
        // The legacy session becomes the newest entry, so it is what opens.
        return named(stamp);
    }
    named(stamp)
}

pub fn load_session(path: Option<&Path>) -> Result<crate::session::Session> {
    path.map_or_else(
        || Ok(crate::session::Session::new()),
        |path| {
            crate::session::Session::load(path)
                .with_context(|| format!("loading {}", path.display()))
        },
    )
}
