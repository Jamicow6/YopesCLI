//! The saved-config JSON: reading, writing, and everything that updates it.
//!
//! `SavedConfig` is the on-disk shape; the public types in `mod.rs` are what
//! the rest of the crate sees. Its fields are `pub(super)` so the resolved
//! `Config` can read them directly.

use std::{collections::BTreeMap, env, fs, path::Path};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use super::{
    AuthKind, CONFIG_VERSION, ConnectedProvider, Connection, CustomProvider, ProviderKind,
    SavedModel, SavedProvider, StoredSecret, credentials,
};

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SavedConfig {
    pub(super) version: u32,
    #[serde(default)]
    pub(super) provider: ProviderKind,
    #[serde(default)]
    pub(super) provider_name: String,
    #[serde(default)]
    pub(super) model: String,
    #[serde(default)]
    pub(super) base_url: String,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    pub(super) auth: AuthKind,
    #[serde(default)]
    pub(super) models: Vec<SavedModel>,
    /// Custom system prompt text, appended to the built-in one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) system_prompt: Option<String>,
    #[serde(default)]
    custom_providers: Vec<SavedCustomProvider>,
    #[serde(default)]
    secret: StoredSecret,
    /// Keys for every provider the user has connected, keyed by base URL.
    /// Without this, connecting a second provider overwrites the first one's
    /// key and the user has to type it again.
    #[serde(default)]
    provider_keys: BTreeMap<String, SavedProvider>,
    /// Tavily key for the web_search tool; `TAVILY_API_KEY` overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) tavily_api_key: Option<String>,
    /// Whether compaction summarizes older turns automatically.
    #[serde(default = "auto_compact_default")]
    pub(super) auto_compact: bool,
    /// Reasoning effort sent with OpenAI-compatible requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) reasoning_effort: Option<String>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    models_url: Option<String>,
    #[serde(default)]
    secret: StoredSecret,
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
            provider_keys: BTreeMap::new(),
            tavily_api_key: None,
            auto_compact: true,
            reasoning_effort: None,
        }
    }
}

/// Saved custom endpoints, with a legacy active custom connection folded in once.
pub(super) fn custom_providers(saved: Option<&SavedConfig>) -> Vec<CustomProvider> {
    let Some(saved) = saved else {
        return Vec::new();
    };
    let mut providers: Vec<CustomProvider> = saved
        .custom_providers
        .iter()
        .map(custom_provider_from_saved)
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
            models_url: None,
            token: if saved.secret.encrypted {
                None
            } else {
                saved.secret.api_key.clone()
            },
            encrypted: saved.secret.encrypted,
            secret: saved.secret.clone(),
        });
    }
    providers
}

fn custom_provider_from_saved(saved: &SavedCustomProvider) -> CustomProvider {
    CustomProvider {
        id: saved.id.clone(),
        name: saved.name.clone(),
        base_url: saved.base_url.clone(),
        auth: saved.auth,
        models: saved.models.clone(),
        models_url: saved.models_url.clone(),
        token: if saved.secret.encrypted {
            None
        } else {
            saved.secret.api_key.clone()
        },
        encrypted: saved.secret.encrypted,
        secret: saved.secret.clone(),
    }
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

pub(super) fn resolve_api_key(
    saved: Option<&SavedConfig>,
    env_key: Option<String>,
) -> Result<Option<String>> {
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
        // The active secret is the one for the active endpoint. Only when it
        // is missing does the per-provider map answer, so a password is never
        // asked for twice.
        if saved.secret.has_key() {
            return saved.secret.resolve();
        }
        // The active endpoint may have a key of its own in the per-provider
        // map, kept when the user connected another provider afterwards.
        let endpoint = saved.base_url.trim_end_matches('/');
        if let Some(stored) = saved.provider_keys.get(endpoint) {
            return stored.secret.resolve();
        }
        return Ok(None);
    }
    Ok(env_key)
}

/// Saved custom endpoints from disk, for in-session list refreshes.
pub(crate) fn load_custom_providers(path: &Path) -> Vec<CustomProvider> {
    custom_providers(load_saved(path).ok().flatten().as_ref())
}

pub(crate) fn save_connection(
    path: &Path,
    connection: &Connection,
    password: Option<&str>,
) -> Result<()> {
    let api_key = if connection.auth.requires_token() {
        connection
            .api_key
            .as_deref()
            .filter(|api_key| !api_key.trim().is_empty())
    } else {
        None
    };
    let provided_secret = if connection.reuse_secret {
        // The key is already on disk for this endpoint: keep it as it is, so
        // an encrypted key stays encrypted and is not written again.
        StoredSecret::default()
    } else {
        match (api_key, password.filter(|password| !password.is_empty())) {
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
        }
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
            input_cost: model.input_cost,
            output_cost: model.output_cost,
            context_window: model.context_window,
        });
    }
    let mut saved = load_saved(path)?.unwrap_or_else(SavedConfig::empty);
    let endpoint = connection.base_url.trim_end_matches('/').to_owned();
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
                // A catalog provider keeps its key under its own endpoint, so
                // connecting another provider does not take this one away.
                saved
                    .provider_keys
                    .get(&endpoint)
                    .map(|stored| stored.secret.clone())
                    .or_else(|| {
                        (saved.base_url.trim_end_matches('/') == endpoint)
                            .then(|| saved.secret.clone())
                    })
                    .unwrap_or_default()
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
    // Every connected provider keeps its own key, so the next /connect run
    // finds it instead of asking again.
    let entry = saved.provider_keys.entry(endpoint.clone()).or_default();
    entry.provider_id = connection.provider_id.clone();
    entry.provider_name = connection.provider_name.clone();
    entry.secret = secret.clone();
    entry.models = models.clone();
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
            models_url: connection
                .models_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(|url| url.trim_end_matches('/').to_owned()),
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
pub(crate) fn delete_saved_model(
    path: &Path,
    custom_provider_id: &str,
    model_id: &str,
) -> Result<()> {
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

/// Remove a saved custom provider, in place.
///
/// Also unconfigures the active connection if it was using this custom provider.
// ponytail: silently ignores non-existent id; upgrade to error if caller needs existence guarantee.
pub(crate) fn delete_custom_provider(path: &Path, id: &str) -> Result<()> {
    update_saved(path, |saved| {
        let legacy_id =
            if saved.provider == ProviderKind::Custom && saved.custom_providers.is_empty() {
                slug_provider_name(&saved.provider_name, &[]).unwrap_or_else(|| "custom".to_owned())
            } else {
                String::new()
            };
        let is_legacy = !legacy_id.is_empty() && (legacy_id == id || id == "custom");
        let endpoint = saved
            .custom_providers
            .iter()
            .find(|stored| stored.id == id)
            .map(|stored| stored.base_url.trim_end_matches('/').to_owned())
            .or_else(|| {
                if is_legacy {
                    Some(saved.base_url.trim_end_matches('/').to_owned())
                } else {
                    None
                }
            });
        saved.custom_providers.retain(|stored| stored.id != id);
        if let Some(endpoint) = endpoint {
            let still_in_use = saved
                .custom_providers
                .iter()
                .any(|stored| stored.base_url.trim_end_matches('/') == endpoint);
            if !still_in_use {
                saved.provider_keys.remove(&endpoint);
            }
            if saved.provider == ProviderKind::Custom
                && saved.base_url.trim_end_matches('/') == endpoint
                && (is_legacy || !still_in_use)
            {
                saved.provider = ProviderKind::Unconfigured;
                saved.provider_name = String::new();
                saved.model = String::new();
                saved.base_url = String::new();
                saved.secret = StoredSecret::default();
                saved.models = Vec::new();
            }
        }
    })
}

/// Rename a saved custom provider, in place.
pub(crate) fn rename_custom_provider(path: &Path, id: &str, new_name: &str) -> Result<()> {
    let new_name = new_name.trim();
    if new_name.is_empty() {
        bail!("provider name cannot be empty");
    }
    update_saved(path, |saved| {
        let endpoint = saved
            .custom_providers
            .iter_mut()
            .find(|stored| stored.id == id)
            .map(|stored| {
                stored.name = new_name.to_owned();
                stored.base_url.trim_end_matches('/').to_owned()
            });
        if let Some(endpoint) = endpoint {
            if saved.provider == ProviderKind::Custom
                && saved.base_url.trim_end_matches('/') == endpoint
            {
                saved.provider_name = new_name.to_owned();
            }
        }
    })
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

/// The secret stored for an endpoint, if the user ever connected it.
///
/// The lookup is by base URL, which is what identifies an endpoint across
/// providers: two providers can share a name but not a URL.
pub(crate) fn load_endpoint_secret(path: &Path, base_url: &str) -> Option<StoredSecret> {
    let endpoint = base_url.trim_end_matches('/');
    let saved = load_saved(path).ok().flatten()?;
    let stored = saved.provider_keys.get(endpoint)?;
    stored.secret.has_key().then(|| stored.secret.clone())
}

/// Models saved for an endpoint, from provider keys or the active connection.
pub(crate) fn load_endpoint_models(path: &Path, base_url: &str) -> Vec<SavedModel> {
    let endpoint = base_url.trim_end_matches('/');
    let Some(saved) = load_saved(path).ok().flatten() else {
        return Vec::new();
    };
    if let Some(stored) = saved.provider_keys.get(endpoint) {
        if !stored.models.is_empty() {
            return stored.models.clone();
        }
    }
    if saved.base_url.trim_end_matches('/') == endpoint {
        return saved.models.clone();
    }
    Vec::new()
}

/// Update the context window limit for a model across active, saved, and custom lists.
pub(crate) fn update_model_context_window(
    path: &Path,
    endpoint: &str,
    model_id: &str,
    context_window: Option<u64>,
) -> Result<()> {
    let endpoint = endpoint.trim_end_matches('/');
    update_saved(path, |saved| {
        if saved.base_url.trim_end_matches('/') == endpoint {
            for model in &mut saved.models {
                if model.id.eq_ignore_ascii_case(model_id) {
                    model.context_window = context_window;
                }
            }
        }
        if let Some(provider) = saved.provider_keys.get_mut(endpoint) {
            for model in &mut provider.models {
                if model.id.eq_ignore_ascii_case(model_id) {
                    model.context_window = context_window;
                }
            }
        }
        for custom in &mut saved.custom_providers {
            if custom.base_url.trim_end_matches('/') == endpoint {
                for model in &mut custom.models {
                    if model.id.eq_ignore_ascii_case(model_id) {
                        model.context_window = context_window;
                    }
                }
            }
        }
    })
}

/// Every provider with a stored key, for the interface to show what is set up.
pub(crate) fn load_connected_providers(path: &Path) -> Vec<ConnectedProvider> {
    let Some(saved) = load_saved(path).ok().flatten() else {
        return Vec::new();
    };
    let mut providers: Vec<ConnectedProvider> = saved
        .provider_keys
        .iter()
        .map(|(base_url, stored)| ConnectedProvider {
            base_url: base_url.clone(),
            provider_id: stored.provider_id.clone(),
            name: stored.provider_name.clone(),
            has_key: stored.secret.has_key(),
            encrypted: stored.secret.encrypted,
            // Older configs only kept the active provider's models.
            models: if stored.models.is_empty()
                && base_url.trim_end_matches('/') == saved.base_url.trim_end_matches('/')
            {
                saved.models.clone()
            } else {
                stored.models.clone()
            },
        })
        .collect();
    providers.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
    });
    providers
}

/// Forget a provider's stored key and every model it offered.
pub(crate) fn disconnect_provider(path: &Path, base_url: &str) -> Result<()> {
    let endpoint = base_url.trim_end_matches('/');
    update_saved(path, |saved| {
        saved.provider_keys.remove(endpoint);
        saved
            .custom_providers
            .retain(|stored| stored.base_url.trim_end_matches('/') != endpoint);
        if saved.base_url.trim_end_matches('/') == endpoint {
            saved.provider = ProviderKind::Unconfigured;
            saved.provider_name = String::new();
            saved.model = String::new();
            saved.base_url = String::new();
            saved.secret = StoredSecret::default();
            saved.models = Vec::new();
        }
    })
}

/// Store the active model of the current connection, so the choice survives a
/// restart.
pub(crate) fn save_model(path: &Path, model: &str) -> Result<()> {
    let model = model.trim().to_owned();
    if model.is_empty() {
        return Ok(());
    }
    update_saved(path, |saved| {
        saved.model = model.clone();
    })
}

/// Store or clear the custom system prompt, in place.
pub(crate) fn save_system_prompt(path: &Path, value: Option<String>) -> Result<()> {
    update_saved(path, |saved| {
        saved.system_prompt = value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
    })
}

pub(crate) fn save_auto_compact(path: &Path, value: bool) -> Result<()> {
    update_saved(path, |saved| saved.auto_compact = value)
}

/// Store or clear the reasoning effort, in place; an empty value clears.
pub(crate) fn save_reasoning_effort(path: &Path, value: Option<String>) -> Result<()> {
    update_saved(path, |saved| {
        saved.reasoning_effort = value
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
    })
}

pub(super) fn load_saved(path: &Path) -> Result<Option<SavedConfig>> {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        AuthKind, CONFIG_VERSION, Connection, ProviderKind, SavedModel, delete_custom_provider,
        delete_saved_model, disconnect_provider, load_custom_providers, load_endpoint_models,
        load_endpoint_secret, load_saved, rename_custom_provider, save_connection,
        save_reasoning_effort, save_system_prompt, slug_provider_name, update_model_context_window,
    };

    #[test]
    fn auto_compact_defaults_to_on_when_absent() {
        let path =
            std::env::temp_dir().join(format!("yopes-compact-test-{}.json", std::process::id()));
        std::fs::write(&path, format!("{{\"version\": {}}}", CONFIG_VERSION)).unwrap();
        let saved = load_saved(&path).unwrap().unwrap();
        assert!(saved.auto_compact);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn switching_a_model_keeps_the_stored_key() {
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
                ..SavedModel::default()
            }],
            models_url: None,
            custom_provider_id: None,
            provider_id: None,
            reuse_secret: false,
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
            ..SavedModel::default()
        }];
        save_connection(&path, &connection, None).unwrap();

        let providers = load_custom_providers(&path);
        assert_eq!(providers[0].token.as_deref(), Some("sk-test"));
        assert_eq!(providers[0].models[0].id, "two");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn provider_slugs_dedupe_against_existing_ids() {
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
    fn every_provider_keeps_its_own_key() {
        use std::fs;

        let path =
            std::env::temp_dir().join(format!("yopes-keys-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let connection = |name: &str, base_url: &str, key: &str| Connection {
            provider: ProviderKind::OpenAi,
            provider_name: name.to_owned(),
            model: "one".to_owned(),
            base_url: base_url.to_owned(),
            api_key: Some(key.to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![SavedModel {
                id: "one".to_owned(),
                name: String::new(),
                ..SavedModel::default()
            }],
            models_url: None,
            custom_provider_id: None,
            provider_id: Some(name.to_owned()),
            reuse_secret: false,
        };

        // Connecting a second provider must not take the first one's key away.
        save_connection(
            &path,
            &connection("one", "https://one.test/v1", "sk-one"),
            None,
        )
        .unwrap();
        save_connection(
            &path,
            &connection("two", "https://two.test/v1", "sk-two"),
            None,
        )
        .unwrap();

        let one = load_endpoint_secret(&path, "https://one.test/v1").unwrap();
        let two = load_endpoint_secret(&path, "https://two.test/v1").unwrap();
        assert_eq!(one.plaintext(), Some("sk-one"));
        assert_eq!(two.plaintext(), Some("sk-two"));

        // Switching models on one of them keeps its key too.
        let mut switched = connection("one", "https://one.test/v1", "sk-one");
        switched.model = "two-models".to_owned();
        switched.api_key = None;
        switched.reuse_secret = true;
        save_connection(&path, &switched, None).unwrap();
        let one = load_endpoint_secret(&path, "https://one.test/v1").unwrap();
        assert_eq!(one.plaintext(), Some("sk-one"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn disconnecting_forgets_the_key() {
        use std::fs;

        let path =
            std::env::temp_dir().join(format!("yopes-disconnect-test-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        let connection = Connection {
            provider: ProviderKind::OpenAi,
            provider_name: "One".to_owned(),
            model: "m".to_owned(),
            base_url: "https://one.test/v1".to_owned(),
            api_key: Some("sk-one".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: Vec::new(),
            models_url: None,
            custom_provider_id: None,
            provider_id: Some("one".to_owned()),
            reuse_secret: false,
        };
        save_connection(&path, &connection, None).unwrap();
        assert!(load_endpoint_secret(&path, "https://one.test/v1").is_some());

        disconnect_provider(&path, "https://one.test/v1").unwrap();
        assert!(load_endpoint_secret(&path, "https://one.test/v1").is_none());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn the_system_prompt_saves_and_clears() {
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
                    ..SavedModel::default()
                },
                SavedModel {
                    id: "two".to_owned(),
                    name: String::new(),
                    ..SavedModel::default()
                },
            ],
            models_url: None,
            custom_provider_id: None,
            provider_id: None,
            reuse_secret: false,
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
        assert!(
            !saved["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["id"] == "one")
        );
        assert_eq!(saved["model"], "one");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn updating_model_context_window_persists() {
        use std::{fs, process};

        let path = std::env::temp_dir().join(format!("yopes-context-test-{}.json", process::id()));
        let _ = fs::remove_file(&path);
        let connection = Connection {
            provider: ProviderKind::OpenAi,
            provider_name: "OpenAI".to_owned(),
            model: "gpt-4o".to_owned(),
            base_url: "https://api.openai.com/v1".to_owned(),
            api_key: Some("sk-test".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![SavedModel {
                id: "gpt-4o".to_owned(),
                name: "GPT-4o".to_owned(),
                context_window: None,
                ..SavedModel::default()
            }],
            models_url: None,
            custom_provider_id: None,
            provider_id: Some("openai".to_owned()),
            reuse_secret: false,
        };
        save_connection(&path, &connection, None).unwrap();
        update_model_context_window(&path, "https://api.openai.com/v1", "gpt-4o", Some(64_000))
            .unwrap();

        let models = load_endpoint_models(&path, "https://api.openai.com/v1");
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].context_window, Some(64_000));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn deleting_custom_provider_removes_from_saved_and_unconfigures() {
        use std::{fs, process};

        let path =
            std::env::temp_dir().join(format!("yopes-delete-provider-test-{}.json", process::id()));
        let _ = fs::remove_file(&path);
        let connection = Connection {
            provider: ProviderKind::Custom,
            provider_name: "TestCustom".to_owned(),
            model: "m1".to_owned(),
            base_url: "https://custom.test/v1".to_owned(),
            api_key: Some("sk-custom".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![SavedModel {
                id: "m1".to_owned(),
                name: String::new(),
                ..SavedModel::default()
            }],
            models_url: None,
            custom_provider_id: None,
            provider_id: None,
            reuse_secret: false,
        };
        save_connection(&path, &connection, None).unwrap();
        let id = load_custom_providers(&path)[0].id.clone();

        delete_custom_provider(&path, &id).unwrap();

        let providers = load_custom_providers(&path);
        assert!(providers.is_empty());
        let saved = load_saved(&path).unwrap().unwrap();
        assert_eq!(saved.provider, ProviderKind::Unconfigured);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn renaming_custom_provider_updates_saved_and_active_name() {
        use std::process;
        let path =
            std::env::temp_dir().join(format!("yopes-rename-provider-test-{}.json", process::id()));
        let _ = fs::remove_file(&path);
        let connection = Connection {
            provider: ProviderKind::Custom,
            provider_name: "OriginalName".to_owned(),
            model: "m1".to_owned(),
            base_url: "https://custom.test/v1".to_owned(),
            api_key: Some("sk-custom".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![SavedModel {
                id: "m1".to_owned(),
                name: String::new(),
                ..SavedModel::default()
            }],
            models_url: None,
            custom_provider_id: None,
            provider_id: None,
            reuse_secret: false,
        };
        save_connection(&path, &connection, None).unwrap();
        let id = load_custom_providers(&path)[0].id.clone();

        rename_custom_provider(&path, &id, "RenamedProvider").unwrap();

        let providers = load_custom_providers(&path);
        assert_eq!(providers[0].name, "RenamedProvider");
        let saved = load_saved(&path).unwrap().unwrap();
        assert_eq!(saved.provider_name, "RenamedProvider");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn shared_endpoint_deletion_keeps_surviving_provider() {
        use std::process;
        let path =
            std::env::temp_dir().join(format!("yopes-shared-endpoint-test-{}.json", process::id()));
        let _ = fs::remove_file(&path);
        let conn1 = Connection {
            provider: ProviderKind::Custom,
            provider_name: "CustomOne".to_owned(),
            model: "m1".to_owned(),
            base_url: "https://shared.test/v1".to_owned(),
            api_key: Some("sk-key".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![SavedModel {
                id: "m1".to_owned(),
                name: String::new(),
                ..SavedModel::default()
            }],
            models_url: None,
            custom_provider_id: Some("id1".to_owned()),
            provider_id: None,
            reuse_secret: false,
        };
        let conn2 = Connection {
            provider: ProviderKind::Custom,
            provider_name: "CustomTwo".to_owned(),
            model: "m2".to_owned(),
            base_url: "https://shared.test/v1".to_owned(),
            api_key: Some("sk-key".to_owned()),
            api_key_env: None,
            auth: AuthKind::Bearer,
            models: vec![SavedModel {
                id: "m2".to_owned(),
                name: String::new(),
                ..SavedModel::default()
            }],
            models_url: None,
            custom_provider_id: Some("id2".to_owned()),
            provider_id: None,
            reuse_secret: true,
        };
        save_connection(&path, &conn1, None).unwrap();
        save_connection(&path, &conn2, None).unwrap();

        delete_custom_provider(&path, "id1").unwrap();

        let providers = load_custom_providers(&path);
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id, "id2");
        let saved = load_saved(&path).unwrap().unwrap();
        assert_eq!(saved.provider, ProviderKind::Custom);
        assert_eq!(saved.provider_name, "CustomTwo");
        assert!(saved.provider_keys.contains_key("https://shared.test/v1"));
        let _ = fs::remove_file(&path);
    }
}
