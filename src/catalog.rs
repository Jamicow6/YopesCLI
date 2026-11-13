use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ProviderKind;

pub const CATALOG_URL: &str = "https://models.dev/api.json";
const OPENAI_COMPATIBLE: &str = "@ai-sdk/openai-compatible";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";
pub const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

pub type FetchResult = std::result::Result<Catalog, String>;

/// Where the provider catalog is cached: one file next to the config, so a
/// second /connect does not hit models.dev again.
pub fn cache_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("models.dev.json")
}

/// The catalog as last fetched, when it is already on disk.
pub fn load_cached(config_path: &Path) -> Option<Catalog> {
    let text = fs::read_to_string(cache_path(config_path)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Keep the fetched catalog for the next run.
pub fn save_cached(config_path: &Path, catalog: &Catalog) -> Result<()> {
    let path = cache_path(config_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string(catalog).context("encoding the model catalog")?;
    fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Catalog {
    #[serde(flatten)]
    pub providers: BTreeMap<String, Provider>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Provider {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub api: Option<String>,
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub env: Vec<String>,
    #[serde(default)]
    pub models: BTreeMap<String, Model>,
}

impl Provider {
    pub fn kind(&self, id: &str) -> ProviderKind {
        if id == "anthropic" || self.npm.as_deref() == Some("@ai-sdk/anthropic") {
            ProviderKind::Anthropic
        } else if id == "google" || self.npm.as_deref() == Some("@ai-sdk/google") {
            ProviderKind::Gemini
        } else {
            ProviderKind::OpenAi
        }
    }

    pub fn base_url(&self, id: &str) -> String {
        self.api
            .as_deref()
            .map(str::trim)
            .filter(|api| !api.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| match self.kind(id) {
                ProviderKind::Anthropic => Some(ANTHROPIC_BASE_URL.to_owned()),
                ProviderKind::Gemini => Some(GEMINI_BASE_URL.to_owned()),
                _ => (id == "openai").then(|| OPENAI_BASE_URL.to_owned()),
            })
            .unwrap_or_default()
    }

    fn is_connectable(&self, id: &str) -> bool {
        if id == "openai" {
            return true;
        }
        let kind = self.kind(id);
        if matches!(kind, ProviderKind::Anthropic | ProviderKind::Gemini) {
            return !self.base_url(id).is_empty();
        }
        self.npm.as_deref() == Some(OPENAI_COMPATIBLE)
            && self
                .api
                .as_deref()
                .is_some_and(|api| !api.trim().is_empty())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Model {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub limit: Option<Limit>,
    #[serde(default)]
    pub cost: Option<Cost>,
}

/// Models.dev prices, in dollars per million tokens. Zero is a real price:
/// a free model must not be billed at someone else's rate.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Cost {
    #[serde(default)]
    pub input: f64,
    #[serde(default)]
    pub output: f64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Limit {
    #[serde(default)]
    pub context: Option<u64>,
}

pub async fn fetch() -> Result<Catalog> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating the catalog client")?;
    let response = client
        .get(CATALOG_URL)
        .send()
        .await
        .context("contacting models.dev")?
        .error_for_status()
        .context("fetching models.dev")?;
    response
        .json()
        .await
        .context("reading the models.dev catalog")
}

/// Fetch an OpenAI-compatible model list from a custom provider's endpoint.
///
/// The endpoint is whatever the user stored, so providers whose model list
/// lives somewhere other than `<base>/models` still work.
pub async fn fetch_models(
    url: &str,
    auth: crate::config::AuthKind,
    key: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating the models client")?;
    let mut request = client.get(url.trim());
    if let Some(key) = key.map(str::trim).filter(|key| !key.is_empty()) {
        request = match auth {
            crate::config::AuthKind::Bearer => request.bearer_auth(key),
            crate::config::AuthKind::XApiKey => request.header("x-api-key", key),
            crate::config::AuthKind::None => request,
        };
    }
    let parsed: OpenAiModels = request
        .send()
        .await
        .context("contacting the models endpoint")?
        .error_for_status()
        .context("fetching the model list")?
        .json()
        .await
        .context("reading the model list")?;
    Ok(parsed
        .data
        .into_iter()
        .map(|model| (model.id, model.name.trim().to_owned()))
        .collect())
}

#[derive(Deserialize)]
struct OpenAiModels {
    #[serde(default)]
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
    #[serde(default)]
    name: String,
}

pub fn connectable_providers(catalog: &Catalog) -> Vec<(&str, &Provider)> {
    let mut providers = catalog
        .providers
        .iter()
        .filter(|(id, provider)| provider.is_connectable(id))
        .map(|(id, provider)| (id.as_str(), provider))
        .collect::<Vec<_>>();
    providers.sort_by_key(|(_, provider)| provider.name.to_ascii_lowercase());
    providers
}

pub fn provider_models(provider: &Provider) -> Vec<(&str, &Model)> {
    let mut models = provider
        .models
        .iter()
        .map(|(id, model)| (id.as_str(), model))
        .collect::<Vec<_>>();
    models.sort_by_key(|(_, model)| model.name.to_ascii_lowercase());
    models
}

#[cfg(test)]
mod tests {
    use super::{Catalog, OpenAiModels, connectable_providers, provider_models};

    const SAMPLE: &str = r#"{
        "openai": {
            "name": "OpenAI",
            "npm": "@ai-sdk/openai",
            "env": ["OPENAI_API_KEY"],
            "models": {"gpt-4o": {"name": "GPT-4o", "limit": {"context": 128000}}}
        },
        "openrouter": {
            "name": "OpenRouter",
            "npm": "@ai-sdk/openai-compatible",
            "api": "https://openrouter.ai/api/v1",
            "env": ["OPENROUTER_API_KEY"],
            "models": {
                "z-model": {"name": "Zed"},
                "a-model": {"name": "Alpha", "limit": {"context": 32000}}
            }
        },
        "anthropic": {
            "name": "Anthropic",
            "npm": "@ai-sdk/anthropic",
            "api": "https://api.anthropic.com/v1",
            "models": {"claude": {"name": "Claude"}}
        },
        "google": {
            "name": "Google",
            "npm": "@ai-sdk/google",
            "models": {"gemini-2.0-flash": {"name": "Gemini 2.0 Flash"}}
        }
    }"#;

    #[test]
    fn openai_models_response_parses() {
        let parsed: OpenAiModels = serde_json::from_str(
            r#"{"object":"list","data":[{"id":"glm-5.3"},{"id":"o4","name":"O4 Mini"}]}"#,
        )
        .unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(parsed.data[0].id, "glm-5.3");
        assert_eq!(parsed.data[1].id, "o4");
        assert_eq!(parsed.data[1].name, "O4 Mini");
    }

    #[test]
    fn supported_providers_are_listed() {
        let catalog: Catalog = serde_json::from_str(SAMPLE).unwrap();
        let ids = connectable_providers(&catalog)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["anthropic", "google", "openai", "openrouter"]);
    }

    #[test]
    fn openai_gets_a_default_base_url() {
        let catalog: Catalog = serde_json::from_str(SAMPLE).unwrap();
        let (_, provider) = connectable_providers(&catalog)
            .into_iter()
            .find(|(id, _)| *id == "openai")
            .unwrap();
        assert_eq!(provider.base_url("openai"), "https://api.openai.com/v1");
    }

    #[test]
    fn models_are_sorted_for_display() {
        let catalog: Catalog = serde_json::from_str(SAMPLE).unwrap();
        let provider = &catalog.providers["openrouter"];
        let ids = provider_models(provider)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["a-model", "z-model"]);
    }
}
