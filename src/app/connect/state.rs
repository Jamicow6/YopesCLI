//! The `/connect` wizard's state: the stage machine, its list items, and the
//! helpers every handler reaches for.
use crate::catalog::{self, Catalog};
use crate::config::{self, AuthKind, SavedModel};
pub const CUSTOM_PROVIDER_ID: &str = "custom";
/// Preferred provider row in /connect: Fasfas AI above models.dev.
pub const FASFAST_ID: &str = catalog::FASFAST_ID;
pub(super) const ADD_MODEL_ID: &str = "add-model";
/// Rows that edit an existing saved endpoint from the model list.
pub(super) const EDIT_ENDPOINT_ID: &str = "edit-endpoint";
pub(super) const EDIT_TOKEN_ID: &str = "edit-token";
/// Row that edits the endpoint serving a custom provider's model list.
pub(super) const EDIT_MODELS_URL_ID: &str = "edit-models-url";
/// Row that refetches the model list from that endpoint.
pub(super) const FETCH_MODELS_ID: &str = "fetch-models";
/// Row that replaces the key of a catalog provider from its model list.
pub(super) const EDIT_KEY_ID: &str = "edit-key";
/// Row that forces a models.dev refetch from the provider list.
pub(super) const REFRESH_CATALOG_ID: &str = "refresh-catalog";
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
    /// Optional URL serving the provider's model list; defaults to `<base>/models`.
    ModelsEndpoint,
    Auth,
    CustomToken,
    ModelId,
    ModelName,
    CustomModels,
    Password,
    /// Unlock a key that is already stored for the endpoint.
    Unlock,
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
            Self::ModelsEndpoint => "models endpoint",
            Self::Auth => "authorization",
            Self::CustomToken => "token",
            Self::ModelId => "model id",
            Self::ModelName => "model name",
            Self::CustomModels => "models",
            Self::Password => "password",
            Self::Unlock => "password",
        }
    }
}
/// What happens once a stored key is unlocked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnlockTarget {
    /// Fetch the provider's model list with the unlocked key.
    FetchModels,
    /// Connect with the model that is already selected.
    Finish,
}
/// How a connection gets its key.
#[derive(Clone, Debug)]
pub enum KeySource {
    /// The user typed the key now, so it is written to disk.
    Entered(String),
    /// The key is already stored for this endpoint, so it is kept as it is.
    Stored(String),
    /// No key at all: the environment variable or the live key applies.
    None,
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
    pub(crate) catalog: Option<Catalog>,
    pub(crate) saved_custom: Vec<config::CustomProvider>,
    /// The models.dev provider being connected, when it came from the catalog.
    pub provider_id: Option<String>,
    pub(crate) model_id: Option<String>,
    pub(crate) api_key: String,
    pub(crate) password: String,
    pub(crate) custom_flow: bool,
    pub(crate) custom_provider_id: Option<String>,
    pub(crate) custom_name: String,
    pub(crate) custom_endpoint: String,
    pub(crate) custom_models_url: String,
    pub(crate) custom_auth: AuthKind,
    pub(crate) custom_token: String,
    /// The token was edited during this visit, so it needs saving again.
    pub(crate) custom_token_dirty: bool,
    pub(crate) custom_models: Vec<SavedModel>,
    pub(crate) custom_model_id: String,
    pub(crate) custom_model_name: String,
    /// The key already stored for the endpoint being connected, if any.
    pub(crate) stored_secret: Option<config::StoredSecret>,
    /// What to do once a stored key is unlocked.
    pub(crate) unlock_target: UnlockTarget,
    /// The endpoint whose stored key is being unlocked.
    pub(crate) unlock_endpoint: String,
    /// A key that was unlocked during this visit.
    pub(crate) unlocked_key: Option<String>,
}
impl ConnectState {
    pub(crate) fn loading() -> Self {
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
            custom_models_url: String::new(),
            custom_auth: AuthKind::Bearer,
            custom_token: String::new(),
            custom_token_dirty: false,
            custom_models: Vec::new(),
            custom_model_id: String::new(),
            custom_model_name: String::new(),
            stored_secret: None,
            unlock_target: UnlockTarget::Finish,
            unlock_endpoint: String::new(),
            unlocked_key: None,
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
                if filter.is_empty() || "refresh".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: REFRESH_CATALOG_ID.to_owned(),
                        title: "Refresh provider list".to_owned(),
                        detail: "reload model IDs from models.dev".to_owned(),
                        logo: "↻",
                        custom_id: None,
                        active: false,
                    });
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
                let mut items: Vec<ConnectItem> = catalog::provider_models(provider)
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
                    .collect();
                // A stored key is reused when a model is picked, so the row
                // that replaces it has to be there too.
                if self
                    .stored_secret
                    .as_ref()
                    .is_some_and(|secret| secret.has_key())
                    && (filter.is_empty() || "change api key".contains(filter.as_str()))
                {
                    items.push(ConnectItem {
                        id: EDIT_KEY_ID.to_owned(),
                        title: "Change API key".to_owned(),
                        detail: if self
                            .stored_secret
                            .as_ref()
                            .is_some_and(|secret| secret.encrypted)
                        {
                            "set · encrypted".to_owned()
                        } else {
                            "set".to_owned()
                        },
                        logo: "✎",
                        custom_id: None,
                        active: false,
                    });
                }
                items
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
                // The model list can be read from any OpenAI-compatible
                // endpoint; the field defaults to `<base>/models`.
                if filter.is_empty() || "models endpoint".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: EDIT_MODELS_URL_ID.to_owned(),
                        title: "Models endpoint".to_owned(),
                        detail: if self.custom_models_url.trim().is_empty() {
                            "default <endpoint>/models".to_owned()
                        } else {
                            self.custom_models_url.clone()
                        },
                        logo: "✎",
                        custom_id: None,
                        active: false,
                    });
                }
                if filter.is_empty() || "fetch models".contains(filter.as_str()) {
                    items.push(ConnectItem {
                        id: FETCH_MODELS_ID.to_owned(),
                        title: "Fetch models".to_owned(),
                        detail: "read the list from the models endpoint".to_owned(),
                        logo: "↻",
                        custom_id: None,
                        active: false,
                    });
                }
                items
            }
            ConnectStage::ApiKey
            | ConnectStage::ProviderName
            | ConnectStage::Endpoint
            | ConnectStage::ModelsEndpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName
            | ConnectStage::Password
            | ConnectStage::Unlock => Vec::new(),
        }
    }
    /// Drop the model row the user is on in the saved endpoint's model list.
    ///
    /// Returns the model id so the caller can persist the deletion; special
    /// rows (add model, change endpoint, change key) are never removable.
    pub(super) fn remove_selected_model(&mut self) -> Option<String> {
        if self.stage != ConnectStage::CustomModels {
            return None;
        }
        let items = self.items();
        let item = items.get(self.selected.min(items.len().saturating_sub(1)))?;
        if matches!(
            item.id.as_str(),
            ADD_MODEL_ID | EDIT_ENDPOINT_ID | EDIT_TOKEN_ID | EDIT_MODELS_URL_ID | FETCH_MODELS_ID
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
            ConnectStage::Password | ConnectStage::Unlock => &self.password,
            ConnectStage::ProviderName => &self.custom_name,
            ConnectStage::Endpoint => &self.custom_endpoint,
            ConnectStage::ModelsEndpoint => &self.custom_models_url,
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
                | ConnectStage::Unlock
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
            ConnectStage::ModelsEndpoint => "models endpoint (blank: <endpoint>/models)",
            ConnectStage::Auth => "authorization",
            ConnectStage::CustomToken => "token",
            ConnectStage::ModelId => "model id",
            ConnectStage::ModelName => "model name",
            ConnectStage::CustomModels => "model",
            ConnectStage::Password => "encryption password (optional)",
            ConnectStage::Unlock => "key password",
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
    pub(super) fn auth_index(&self) -> usize {
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
    pub(super) fn provider(&self) -> Option<&catalog::Provider> {
        let id = self.provider_id.as_deref()?;
        self.catalog.as_ref()?.providers.get(id)
    }
    /// The endpoint of the provider being connected.
    pub(super) fn endpoint(&self) -> String {
        match self.provider_id.as_deref() {
            Some(id) => self
                .provider()
                .map(|provider| provider.base_url(id))
                .unwrap_or_default(),
            None => self.custom_endpoint.clone(),
        }
    }
    /// The stored key, ready to use without asking for anything.
    ///
    /// A secret saved without a password opens right away; an encrypted one
    /// needs its password first, so it is not returned here.
    pub(super) fn stored_key(&self) -> Option<String> {
        let secret = self.stored_secret.as_ref()?;
        if !secret.has_key() || secret.encrypted {
            return None;
        }
        secret.plaintext().map(ToOwned::to_owned)
    }
    pub(super) fn selected_item(&self) -> Option<ConnectItem> {
        let items = self.items();
        if items.is_empty() {
            return None;
        }
        items.get(self.selected.min(items.len() - 1)).cloned()
    }
    /// The endpoint of the provider row under the selection, when that row is
    /// a real provider rather than an action.
    pub(crate) fn selected_endpoint(&self) -> Option<String> {
        if self.stage != ConnectStage::Providers {
            return None;
        }
        let item = self.selected_item()?;
        if item.id == CUSTOM_PROVIDER_ID {
            return None;
        }
        if item.custom_id.is_some() {
            return self
                .saved_custom
                .iter()
                .find(|provider| provider.id == item.id)
                .map(|provider| provider.base_url.clone());
        }
        if item.id == FASFAST_ID {
            return Some(catalog::FASFAST_BASE_URL.to_owned());
        }
        self.catalog
            .as_ref()?
            .providers
            .get(&item.id)
            .map(|provider| provider.base_url(&item.id))
            .filter(|endpoint| !endpoint.trim().is_empty())
    }
    pub(super) fn move_selection(&mut self, forward: bool) {
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
    pub(super) fn move_selection_by(&mut self, delta: isize) {
        let len = self.items().len();
        if len == 0 {
            return;
        }
        let current = self.selected.min(len - 1) as isize;
        self.selected = (current + delta).clamp(0, len as isize - 1) as usize;
    }
}
pub(super) fn custom_field_mut(
    connect: &mut ConnectState,
    stage: ConnectStage,
) -> Option<&mut String> {
    match stage {
        ConnectStage::ProviderName => Some(&mut connect.custom_name),
        ConnectStage::Endpoint => Some(&mut connect.custom_endpoint),
        ConnectStage::ModelsEndpoint => Some(&mut connect.custom_models_url),
        ConnectStage::CustomToken => Some(&mut connect.custom_token),
        ConnectStage::ModelId => Some(&mut connect.custom_model_id),
        ConnectStage::ModelName => Some(&mut connect.custom_model_name),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
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
            models_url: None,
            token: None,
            encrypted: false,
            secret: crate::config::StoredSecret::default(),
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
    fn delete_removes_only_a_model_row() {
        use super::{ConnectStage, ConnectState, FETCH_MODELS_ID};
        use crate::config::SavedModel;
        let mut connect = ConnectState::loading();
        connect.stage = ConnectStage::CustomModels;
        connect.custom_models = vec![
            SavedModel {
                id: "one".to_owned(),
                name: "One".to_owned(),
                ..SavedModel::default()
            },
            SavedModel {
                id: "two".to_owned(),
                name: "Two".to_owned(),
                ..SavedModel::default()
            },
        ];
        assert_eq!(connect.remove_selected_model().as_deref(), Some("one"));
        assert_eq!(connect.custom_models.len(), 1);
        // The action rows (add model, change endpoint) are never removable.
        let last = connect.items().len().saturating_sub(1);
        connect.selected = last;
        assert!(connect.remove_selected_model().is_none());
        assert_eq!(connect.custom_models.len(), 1);
        assert_eq!(connect.items()[last].id, FETCH_MODELS_ID);
        connect.custom_provider_id = Some("endpoint".to_owned());
        let last = connect.items().len().saturating_sub(1);
        connect.selected = last;
        assert!(connect.remove_selected_model().is_none());
        assert_eq!(connect.custom_models.len(), 1);
        assert_eq!(connect.custom_models[0].id, "two");
    }
}
