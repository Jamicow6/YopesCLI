use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Result};
use serde::Deserialize;

pub const CATALOG_URL: &str = "https://models.dev/api.json";
/// Preferred provider: Fasfas AI, listed in /connect above models.dev.
pub const FASFAST_ID: &str = "fasfas";
pub const FASFAST_LABEL: &str = "Fasfas AI";
pub const FASFAST_BASE_URL: &str = "https://ai.coffeevps.com/v1";
const FASFAST_MODELS_URL: &str = "https://ai.coffeevps.com/v1/models";
pub const FASFAST_DISCORD: &str = "https://discord.gg/vvndVbBZRe";
const OPENAI_COMPATIBLE: &str = "@ai-sdk/openai-compatible";
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

pub type FetchResult = std::result::Result<Catalog, String>;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Catalog {
    #[serde(flatten)]
    pub providers: BTreeMap<String, Provider>,
}

#[derive(Clone, Debug, Deserialize)]
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
    pub fn base_url(&self, id: &str) -> String {
        self.api
            .as_deref()
            .map(str::trim)
            .filter(|api| !api.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| (id == "openai").then(|| OPENAI_BASE_URL.to_owned()))
            .unwrap_or_default()
    }

    fn is_connectable(&self, id: &str) -> bool {
        if id == "openai" {
            return true;
        }
        self.npm.as_deref() == Some(OPENAI_COMPATIBLE)
            && self
                .api
                .as_deref()
                .is_some_and(|api| !api.trim().is_empty())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub limit: Option<Limit>,
}

#[derive(Clone, Debug, Default, Deserialize)]
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

/// OpenAI-compatible fetch of the Fasfas model list: GET /v1/models with the
/// Discord-issued key, rebuilt as a one-provider synthetic catalog so the
/// stock model picker can render it.
pub async fn fetch_fasfas(key: &str) -> Result<Catalog> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating the Fasfas client")?;
    let response = client
        .get(FASFAST_MODELS_URL)
        .bearer_auth(key.trim())
        .send()
        .await
        .context("contacting Fasfas AI")?
        .error_for_status()
        .context("fetching models from Fasfas AI")?;
    let parsed: FasfasModels = response.json().await.context("reading Fasfas AI models")?;
    Ok(fasfas_catalog(parsed))
}

/// Shape a parsed `/v1/models` response as a one-provider synthetic catalog.
fn fasfas_catalog(parsed: FasfasModels) -> Catalog {
    let mut models = BTreeMap::new();
    for model in parsed.data {
        models.insert(
            model.id.clone(),
            Model {
                name: model.name.trim().to_owned(),
                limit: None,
            },
        );
    }
    let mut providers = BTreeMap::new();
    providers.insert(
        FASFAST_ID.to_owned(),
        Provider {
            name: FASFAST_LABEL.to_owned(),
            api: Some(FASFAST_BASE_URL.to_owned()),
            npm: Some(OPENAI_COMPATIBLE.to_owned()),
            env: Vec::new(),
            models,
        },
    );
    Catalog { providers }
}

#[derive(Deserialize)]
struct FasfasModels {
    #[serde(default)]
    data: Vec<FasfasModel>,
}

#[derive(Deserialize)]
struct FasfasModel {
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
#[cfg(test)]
mod tests {
    use super::{FasfasModels, FASFAST_BASE_URL, Catalog, connectable_providers, fasfas_catalog, provider_models};

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
        }
    }"#;

    #[test]
    fn fasfas_models_become_a_synthetic_catalog() {
        let parsed: FasfasModels = serde_json::from_str(
            r#"{"object":"list","data":[{"id":"glm-5.3"},{"id":"o4","name":"O4 Mini"}]}"#,
        )
        .unwrap();
        let catalog = fasfas_catalog(parsed);
        assert!(catalog.providers.contains_key("fasfas"));
        let provider = &catalog.providers["fasfas"];
        assert_eq!(provider.base_url("fasfas"), FASFAST_BASE_URL);
        assert_eq!(provider.models["o4"].name, "O4 Mini");
        assert!(provider.models["glm-5.3"].name.is_empty());
        assert!(provider.is_connectable("fasfas"));
    }

    #[test]
    fn only_openai_compatible_providers_are_listed() {
        let catalog: Catalog = serde_json::from_str(SAMPLE).unwrap();
        let ids = connectable_providers(&catalog)
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["openai", "openrouter"]);
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
