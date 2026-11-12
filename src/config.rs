use std::{
    env, fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::credentials;

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

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SavedModel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Parser)]
#[command(name = "yopes", version = crate::update::current_version(), about = "A fast, session-first AI harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Model backend: openai or ollama.
    #[arg(long, value_enum)]
    provider: Option<ProviderKind>,

    /// Model name sent to the provider.
    #[arg(long)]
    model: Option<String>,

    /// Provider base URL, without /chat/completions or /api/chat.
    #[arg(long)]
    base_url: Option<String>,

    /// Session JSON path. Defaults to the platform data directory.
    #[arg(long)]
    session: Option<PathBuf>,

    /// Do not load or save the session.
    #[arg(long)]
    no_save: bool,

    /// Custom system prompt text, appended to the built-in one.
    #[arg(long)]
    system_prompt: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check for updates, install the latest release, and restart.
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub auth: AuthKind,
    pub models: Vec<SavedModel>,
    /// Plaintext token when stored without encryption; `None` when encrypted or absent.
    pub token: Option<String>,
    pub encrypted: bool,
}

impl CustomProvider {
    fn from_saved(saved: &SavedCustomProvider) -> Self {
        Self {
            id: saved.id.clone(),
            name: saved.name.clone(),
            base_url: saved.base_url.clone(),
            auth: saved.auth,
            models: saved.models.clone(),
            token: if saved.secret.encrypted {
                None
            } else {
                saved.secret.api_key.clone()
            },
            encrypted: saved.secret.encrypted,
        }
    }
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
    /// Id of the saved custom provider to update; `None` creates a new entry.
    pub custom_provider_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedConfig {
    version: u32,
    #[serde(default)]
    provider: ProviderKind,
    #[serde(default)]
    provider_name: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    base_url: String,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    auth: AuthKind,
    #[serde(default)]
    models: Vec<SavedModel>,
    /// Custom system prompt text, appended to the built-in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    system_prompt: Option<String>,
    #[serde(default)]
    custom_providers: Vec<SavedCustomProvider>,
    #[serde(default)]
    secret: StoredSecret,
    /// Tavily key for the web_search tool; `TAVILY_API_KEY` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tavily_api_key: Option<String>,
    /// Whether compaction summarizes older turns automatically.
    #[serde(default = "auto_compact_default")]
    auto_compact: bool,
    /// Reasoning effort sent with OpenAI-compatible requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
}

fn auto_compact_default() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
struct SavedCustomProvider {
    id: String,
    name: String,
    base_url: String,
    auth: AuthKind,
    #[serde(default)]
    models: Vec<SavedModel>,
    #[serde(default)]
    secret: StoredSecret,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoredSecret {
    #[serde(default)]
    encrypted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    salt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ciphertext: Option<String>,
}

impl StoredSecret {
    fn resolve(&self) -> Result<Option<String>> {
        if !self.encrypted {
            return Ok(self
                .api_key
                .clone()
                .filter(|api_key| !api_key.trim().is_empty()));
        }
        let (Some(salt), Some(nonce), Some(ciphertext)) =
            (&self.salt, &self.nonce, &self.ciphertext)
        else {
            bail!("the stored API key is missing its encryption data");
        };
        let password = credentials::prompt_password("API key password: ")?;
        if password.is_empty() {
            bail!("an encryption password is required to unlock the stored API key");
        }
        let api_key = credentials::open(&password, salt, nonce, ciphertext)?;
        Ok(Some(api_key))
    }
}

impl SavedConfig {
    /// A blank config for a first write.
    fn empty() -> Self {
        Self {
            version: CONFIG_VERSION,
            provider: ProviderKind::Unconfigured,
            provider_name: String::new(),
            model: String::new(),
            base_url: String::new(),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: Vec::new(),
            system_prompt: None,
            custom_providers: Vec::new(),
            secret: StoredSecret::default(),
            tavily_api_key: None,
            auto_compact: true,
            reasoning_effort: None,
        }
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

/// Saved custom endpoints, with a legacy active custom connection folded in once.
fn custom_providers(saved: Option<&SavedConfig>) -> Vec<CustomProvider> {
    let Some(saved) = saved else {
        return Vec::new();
    };
    let mut providers: Vec<CustomProvider> = saved
        .custom_providers
        .iter()
        .map(CustomProvider::from_saved)
        .collect();
    let legacy = saved.provider == ProviderKind::Custom
        && !saved.base_url.trim().is_empty()
        && providers.is_empty();
    if legacy {
        let id =
            slug_provider_name(&saved.provider_name, &[]).unwrap_or_else(|| "custom".to_owned());
        providers.push(CustomProvider {
            id,
            name: if saved.provider_name.trim().is_empty() {
                "Custom".to_owned()
            } else {
                saved.provider_name.clone()
            },
            base_url: saved.base_url.trim_end_matches('/').to_owned(),
            auth: saved.auth,
            models: saved.models.clone(),
            token: if saved.secret.encrypted {
                None
            } else {
                saved.secret.api_key.clone()
            },
            encrypted: saved.secret.encrypted,
        });
    }
    providers
}

fn slug_provider_name(name: &str, existing_ids: &[String]) -> Option<String> {
    let base: String = name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect();
    let base = base.trim_matches('-');
    if base.is_empty() {
        return None;
    }
    let mut candidate = base.to_owned();
    let mut suffix = 2;
    while existing_ids.contains(&candidate) {
        candidate = format!("{base}-{suffix}");
        suffix += 1;
    }
    Some(candidate)
}

fn resolve_api_key(saved: Option<&SavedConfig>, env_key: Option<String>) -> Result<Option<String>> {
    if let Some(saved) = saved {
        if saved.auth == AuthKind::None {
            return Ok(None);
        }
        if let Some(name) = saved.api_key_env.as_deref() {
            if let Some(api_key) = env::var(name)
                .ok()
                .filter(|api_key| !api_key.trim().is_empty())
            {
                return Ok(Some(api_key));
            }
        }
        if let Some(api_key) = saved.secret.resolve()? {
            return Ok(Some(api_key));
        }
        return Ok(None);
    }
    Ok(env_key)
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

/// Saved custom endpoints from disk, for in-session list refreshes.
pub fn load_custom_providers(path: &Path) -> Vec<CustomProvider> {
    custom_providers(load_saved(path).ok().flatten().as_ref())
}

pub fn save_connection(path: &Path, connection: &Connection, password: Option<&str>) -> Result<()> {
    let api_key = if connection.auth.requires_token() {
        connection
            .api_key
            .as_deref()
            .filter(|api_key| !api_key.trim().is_empty())
    } else {
        None
    };
    let provided_secret = match (api_key, password.filter(|password| !password.is_empty())) {
        (Some(api_key), Some(password)) => {
            let sealed = credentials::seal(password, api_key)?;
            StoredSecret {
                encrypted: true,
                salt: Some(sealed.salt),
                nonce: Some(sealed.nonce),
                ciphertext: Some(sealed.ciphertext),
                ..StoredSecret::default()
            }
        }
        (Some(api_key), None) => StoredSecret {
            api_key: Some(api_key.to_owned()),
            ..StoredSecret::default()
        },
        _ => StoredSecret::default(),
    };
    let mut models = Vec::new();
    for model in &connection.models {
        let id = model.id.trim();
        if id.is_empty() || models.iter().any(|model: &SavedModel| model.id == id) {
            continue;
        }
        models.push(SavedModel {
            id: id.to_owned(),
            name: model.name.trim().to_owned(),
        });
    }
    let mut saved = load_saved(path)?.unwrap_or_else(SavedConfig::empty);
    // Switching a model must not wipe or downgrade an existing key: when no new
    // key was supplied, keep whatever is already stored for this endpoint.
    let secret = if provided_secret.api_key.is_none() && provided_secret.ciphertext.is_none() {
        match (connection.provider, &connection.custom_provider_id) {
            (ProviderKind::Custom, Some(id)) => saved
                .custom_providers
                .iter()
                .find(|stored| stored.id == id.trim())
                .map(|stored| stored.secret.clone())
                .or_else(|| {
                    // Legacy configs keep the active custom secret at the top
                    // level, with no custom_providers entry to look up.
                    (saved.provider == ProviderKind::Custom
                        && saved.base_url.trim_end_matches('/')
                            == connection.base_url.trim_end_matches('/'))
                    .then(|| saved.secret.clone())
                })
                .unwrap_or_default(),
            (ProviderKind::Custom, None) => StoredSecret::default(),
            _ => {
                if saved.base_url.trim_end_matches('/') == connection.base_url.trim_end_matches('/')
                {
                    saved.secret.clone()
                } else {
                    Default::default()
                }
            }
        }
    } else {
        provided_secret
    };
    saved.provider = connection.provider;
    saved.provider_name = connection.provider_name.clone();
    saved.model = connection.model.clone();
    saved.base_url = connection.base_url.trim_end_matches('/').to_owned();
    saved.api_key_env = connection.api_key_env.clone();
    saved.auth = connection.auth;
    saved.models = models.clone();
    saved.secret = secret.clone();
    if connection.provider == ProviderKind::Custom {
        let display_name = if connection.provider_name.trim().is_empty() {
            "Custom".to_owned()
        } else {
            connection.provider_name.clone()
        };
        let existing_ids: Vec<String> = saved
            .custom_providers
            .iter()
            .map(|stored| stored.id.clone())
            .collect();
        let id = match &connection.custom_provider_id {
            Some(id) if !id.trim().is_empty() => id.trim().to_owned(),
            _ => slug_provider_name(&display_name, &existing_ids)
                .unwrap_or_else(|| "custom".to_owned()),
        };
        let entry = SavedCustomProvider {
            id: id.clone(),
            name: display_name,
            base_url: saved.base_url.clone(),
            auth: connection.auth,
            models: models.clone(),
            secret: saved.secret.clone(),
        };
        if let Some(existing) = saved
            .custom_providers
            .iter_mut()
            .find(|stored| stored.id == id)
        {
            *existing = entry;
        } else {
            saved.custom_providers.push(entry);
        }
    }
    save_saved(path, &saved)
}

/// Remove a model from a saved custom provider, in place.
///
/// The entry is matched by id; when that provider is the active connection,
/// its top-level model list is dropped too so a restart shows the same list.
pub fn delete_saved_model(path: &Path, custom_provider_id: &str, model_id: &str) -> Result<()> {
    let mut saved = load_saved(path)?.context("no configuration file")?;
    let entry = saved
        .custom_providers
        .iter_mut()
        .find(|stored| stored.id == custom_provider_id)
        .context("the endpoint is no longer saved")?;
    entry.models.retain(|model| model.id != model_id);
    if saved.provider == ProviderKind::Custom
        && saved.base_url.trim_end_matches('/') == entry.base_url.trim_end_matches('/')
    {
        saved.models.retain(|model| model.id != model_id);
    }
    save_saved(path, &saved)
}

/// Read the saved config, apply `change`, and write it back.
///
/// Works before the first connection too: a config holding just the change is
/// created when none exists yet.
fn update_saved(path: &Path, change: impl FnOnce(&mut SavedConfig)) -> Result<()> {
    let mut saved = load_saved(path)?.unwrap_or_else(SavedConfig::empty);
    change(&mut saved);
    save_saved(path, &saved)
}

/// Store or clear the custom system prompt, in place.
pub fn save_system_prompt(path: &Path, value: Option<String>) -> Result<()> {
    update_saved(path, |saved| {
        saved.system_prompt = value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
    })
}

pub fn save_auto_compact(path: &Path, value: bool) -> Result<()> {
    update_saved(path, |saved| saved.auto_compact = value)
}

/// Store or clear the reasoning effort, in place; an empty value clears.
pub fn save_reasoning_effort(path: &Path, value: Option<String>) -> Result<()> {
    update_saved(path, |saved| {
        saved.reasoning_effort = value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
    })
}

fn load_saved(path: &Path) -> Result<Option<SavedConfig>> {
    if !path.exists() {
        return Ok(None);
    }
    let source = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let saved: SavedConfig =
        serde_json::from_str(&source).with_context(|| format!("parsing {}", path.display()))?;
    if saved.version != CONFIG_VERSION {
        bail!(
            "unsupported config version {}; expected {}",
            saved.version,
            CONFIG_VERSION
        );
    }
    Ok(Some(saved))
}

fn save_saved(path: &Path, saved: &SavedConfig) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(saved)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, json).with_context(|| format!("writing {}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(windows))]
    fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    #[cfg(windows)]
    {
        if path.exists() {
            fs::remove_file(path).with_context(|| format!("removing old {}", path.display()))?;
        }
        fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{AuthKind, Cli, ProviderKind};
    use clap::Parser;

    #[test]
    fn provider_values_are_stable_cli_names() {
        let cli = Cli::try_parse_from(["yopes", "--provider", "openai"]).unwrap();
        assert_eq!(cli.provider, Some(ProviderKind::OpenAi));
        assert!(Cli::try_parse_from(["yopes", "--provider", "invalid"]).is_err());
        assert!(Cli::try_parse_from(["yopes", "--provider", "custom"]).is_err());
        assert!(Cli::try_parse_from(["yopes", "--provider", "demo"]).is_err());
    }

    #[test]
    fn custom_auth_names_are_stable() {
        assert_eq!(
            serde_json::to_string(&AuthKind::XApiKey).unwrap(),
            "\"x-api-key\""
        );
        assert_eq!(
            serde_json::from_str::<AuthKind>("\"none\"").unwrap(),
            AuthKind::None
        );
    }

    #[test]
    fn the_update_subcommand_parses() {
        let cli = Cli::try_parse_from(["yopes", "update"]).unwrap();
        assert!(matches!(cli.command, Some(super::Command::Update)));
    }

    #[test]
    fn auto_compact_defaults_to_on_when_absent() {
        use super::load_saved;
        let path =
            std::env::temp_dir().join(format!("yopes-compact-test-{}.json", std::process::id()));
        std::fs::write(&path, format!("{{\"version\": {}}}", super::CONFIG_VERSION)).unwrap();
        let saved = load_saved(&path).unwrap().unwrap();
        assert!(saved.auto_compact);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn switching_a_model_keeps_the_stored_key() {
        use super::{Connection, SavedModel, load_custom_providers, save_connection};
        use std::fs;

        let path =
            std::env::temp_dir().join(format!("yopes-config-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let mut connection = Connection {
            provider: ProviderKind::Custom,
            provider_name: "Test".to_owned(),
            model: "one".to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            api_key: Some("sk-test".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![SavedModel {
                id: "one".to_owned(),
                name: String::new(),
            }],
            custom_provider_id: None,
        };
        save_connection(&path, &connection, None).unwrap();

        // The manager saves a model switch without going through the key again.
        let id = load_custom_providers(&path)[0].id.clone();
        connection.model = "two".to_owned();
        connection.api_key = None;
        connection.custom_provider_id = Some(id);
        connection.models = vec![SavedModel {
            id: "two".to_owned(),
            name: String::new(),
        }];
        save_connection(&path, &connection, None).unwrap();

        let providers = load_custom_providers(&path);
        assert_eq!(providers[0].token.as_deref(), Some("sk-test"));
        assert_eq!(providers[0].models[0].id, "two");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn provider_slugs_dedupe_against_existing_ids() {
        use super::slug_provider_name;
        assert_eq!(
            slug_provider_name("My Provider", &[]).as_deref(),
            Some("my-provider")
        );
        assert_eq!(
            slug_provider_name("My Provider", &["my-provider".to_owned()]).as_deref(),
            Some("my-provider-2")
        );
        assert_eq!(
            slug_provider_name(
                "My Provider",
                &["my-provider".to_owned(), "my-provider-2".to_owned()]
            )
            .as_deref(),
            Some("my-provider-3")
        );
        assert_eq!(slug_provider_name("  !!  ", &[]), None);
    }

    #[test]
    fn the_system_prompt_saves_and_clears() {
        use super::save_system_prompt;
        use std::fs;

        let path =
            std::env::temp_dir().join(format!("yopes-sysprompt-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        save_system_prompt(&path, Some(" Answer in rhyme. ".to_owned())).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["system_prompt"], "Answer in rhyme.");
        save_system_prompt(&path, Some("  ".to_owned())).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.get("system_prompt"), None);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_reasoning_effort_saves_and_clears() {
        use super::save_reasoning_effort;
        use std::fs;

        let path =
            std::env::temp_dir().join(format!("yopes-effort-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        save_reasoning_effort(&path, Some(" high ".to_owned())).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["reasoning_effort"], "high");
        save_reasoning_effort(&path, None).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved.get("reasoning_effort"), None);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn deleting_a_model_updates_both_lists_of_the_active_provider() {
        use super::{
            Connection, SavedModel, delete_saved_model, load_custom_providers, save_connection,
        };
        use std::{fs, process};

        let path = std::env::temp_dir().join(format!("yopes-delete-test-{}.json", process::id()));
        let _ = fs::remove_file(&path);
        let connection = Connection {
            provider: ProviderKind::Custom,
            provider_name: "Test".to_owned(),
            model: "one".to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            api_key: Some("sk-test".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![
                SavedModel {
                    id: "one".to_owned(),
                    name: String::new(),
                },
                SavedModel {
                    id: "two".to_owned(),
                    name: String::new(),
                },
            ],
            custom_provider_id: None,
        };
        save_connection(&path, &connection, None).unwrap();
        let id = load_custom_providers(&path)[0].id.clone();

        delete_saved_model(&path, &id, "one").unwrap();

        let providers = load_custom_providers(&path);
        assert_eq!(providers[0].models.len(), 1);
        assert_eq!(providers[0].models[0].id, "two");
        assert_eq!(providers[0].token.as_deref(), Some("sk-test"));
        // The active connection's list drops the model too; its selected model
        // stays so the running session is untouched.
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["id"] == "one"),
            false
        );
        assert_eq!(saved["model"], "one");
        let _ = fs::remove_file(&path);
    }
}
