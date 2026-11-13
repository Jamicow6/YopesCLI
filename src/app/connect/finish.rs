use super::state::*;
use super::*;

impl App {
    pub(crate) fn select_connect_item(&mut self) -> Action {
        let Some(item) = self.connect.as_ref().and_then(ConnectState::selected_item) else {
            return Action::None;
        };
        let backend_base = self.backend.base_url().trim_end_matches('/').to_owned();
        let backend_has_key = self.backend.api_key().is_some();
        // The config path is cloned so the connect state can be borrowed
        // mutably while a stored key is looked up on disk.
        let config_path = self.config_path.clone();
        let mut finish_now = false;
        // Fetching needs the whole App (the live key may be reused), so it is
        // done after the connect state is released.
        if item.id == FETCH_MODELS_ID
            && self
                .connect
                .as_ref()
                .is_some_and(|connect| connect.stage == ConnectStage::CustomModels)
        {
            return self.fetch_custom_models();
        }
        if item.id == REMOVE_PROVIDER_ID
            && self
                .connect
                .as_ref()
                .is_some_and(|connect| connect.stage == ConnectStage::CustomModels)
        {
            let custom_id = self
                .connect
                .as_ref()
                .and_then(|c| c.custom_provider_id.clone());
            if let Some(id) = custom_id {
                self.delete_custom_provider(&id);
                if let Some(connect) = self.connect.as_mut() {
                    connect.stage = ConnectStage::Providers;
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            return Action::None;
        }
        let Some(connect) = self.connect.as_mut() else {
            return Action::None;
        };
        match connect.stage {
            ConnectStage::Providers => {
                if item.id == REFRESH_CATALOG_ID {
                    return Action::RefreshCatalog;
                }
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
                    connect.custom_models_url = saved.models_url.clone().unwrap_or_default();
                    connect.custom_token = saved.token.clone().unwrap_or_default();
                    connect.custom_token_dirty = false;
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    // The endpoint keeps its own key, so coming back to it
                    // does not ask for the key again.
                    connect.stored_secret = Some(saved.secret.clone());
                    connect.unlock_endpoint = saved.base_url.clone();
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
                    connect.custom_token_dirty = false;
                    connect.custom_models.clear();
                    connect.custom_models_url.clear();
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    connect.custom_model_context_window = "128000".to_owned();
                    connect.add_provider_field = AddProviderField::Name;
                    connect.stage = ConnectStage::AddProvider;
                    connect.error = None;
                } else {
                    connect.provider_id = Some(item.id.clone());
                    connect.custom_flow = false;
                    // A catalog provider keeps its key under its own endpoint,
                    // so coming back to it does not ask for the key again.
                    let endpoint = connect.endpoint();
                    connect.stored_secret = config::load_endpoint_secret(&config_path, &endpoint);
                    if let Some(catalog) = connect.catalog.as_mut() {
                        if let Some(provider) = catalog.providers.get_mut(&item.id) {
                            for model in config::load_endpoint_models(&config_path, &endpoint) {
                                if let Some(context) = model.context_window {
                                    if let Some(m) = provider.models.get_mut(&model.id) {
                                        m.limit = Some(catalog::Limit {
                                            context: Some(context),
                                        });
                                    }
                                }
                            }
                        }
                    }
                    connect.stage = ConnectStage::Models;
                }
            }
            ConnectStage::Models => {
                if item.id == EDIT_KEY_ID {
                    // Replace the stored key: the key field opens empty.
                    connect.stored_secret = None;
                    connect.api_key.clear();
                    connect.stage = ConnectStage::ApiKey;
                    connect.selected = 0;
                    connect.error = None;
                    return Action::None;
                }
                connect.model_id = Some(item.id);
                connect.password.clear();
                connect.selected = 0;
                connect.error = None;
                // A key already stored for this endpoint is used as it is. The
                // user is only asked when there is nothing to reuse.
                let stored = connect.stored_key();
                let locked = connect
                    .stored_secret
                    .as_ref()
                    .is_some_and(|secret| secret.encrypted && secret.has_key());
                if let Some(key) = stored {
                    return self.finish_connect_with(KeySource::Stored(key), None);
                }
                // An encrypted key needs its password before it can be used,
                // so ask for that instead of the key.
                if locked {
                    connect.unlock_endpoint = connect.endpoint();
                    connect.password.clear();
                    connect.stage = ConnectStage::Unlock;
                    return Action::None;
                }
                connect.api_key.clear();
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
                if item.id == EDIT_NAME_ID {
                    connect.stage = ConnectStage::RenameProvider;
                    connect.selected = 0;
                    connect.error = None;
                    return Action::None;
                }
                if item.id == EDIT_TOKEN_ID {
                    connect.stage = ConnectStage::CustomToken;
                    connect.selected = 0;
                    connect.error = None;
                    connect.custom_token_dirty = true;
                    return Action::None;
                }
                if item.id == EDIT_MODELS_URL_ID {
                    connect.stage = ConnectStage::ModelsEndpoint;
                    connect.selected = 0;
                    connect.error = None;
                    return Action::None;
                }
                connect.model_id = Some(item.id);
                connect.password.clear();
                connect.selected = 0;
                connect.error = None;
                // Switching a model on a saved endpoint reuses the stored key.
                // Only ask for the encryption password when the key changed.
                let reuse_secret = !connect.custom_token_dirty
                    && connect.custom_provider_id.as_ref().is_some_and(|id| {
                        connect.saved_custom.iter().any(|saved| {
                            &saved.id == id
                                && (!saved.auth.requires_token()
                                    || saved.token.is_some()
                                    || (backend_has_key
                                        && saved.base_url.trim_end_matches('/') == backend_base))
                        })
                    });
                if reuse_secret {
                    finish_now = true;
                } else if connect.custom_token_dirty {
                    connect.stage = ConnectStage::Password;
                } else {
                    // The stored token is encrypted: only its password is
                    // asked for, never the token itself.
                    connect.unlock_endpoint = connect.custom_endpoint.clone();
                    connect.password.clear();
                    connect.stage = ConnectStage::Unlock;
                }
            }
            ConnectStage::ApiKey
            | ConnectStage::ProviderName
            | ConnectStage::Endpoint
            | ConnectStage::ModelsEndpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName
            | ConnectStage::ModelContextWindow
            | ConnectStage::Password
            | ConnectStage::Unlock
            | ConnectStage::AddProvider
            | ConnectStage::RenameProvider => {}
        }
        if finish_now {
            return self.finish_custom_connect(None);
        }
        Action::None
    }

    pub(crate) fn submit_add_provider(&mut self) -> Action {
        let (name, endpoint, models_url, model_name, model_id, context_window_str) = {
            let Some(connect) = self.connect.as_ref() else {
                return Action::None;
            };
            (
                connect.custom_name.trim().to_owned(),
                connect.custom_endpoint.trim().to_owned(),
                connect.custom_models_url.trim().to_owned(),
                connect.custom_model_name.trim().to_owned(),
                connect.custom_model_id.trim().to_owned(),
                connect.custom_model_context_window.trim().to_owned(),
            )
        };
        if name.is_empty() {
            self.connect_error("Provider name cannot be empty.");
            return Action::None;
        }
        if endpoint.is_empty() {
            self.connect_error("Completion URL cannot be empty.");
            return Action::None;
        }
        if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
            self.connect_error("Completion URL must start with http:// or https://.");
            return Action::None;
        }
        if !models_url.is_empty()
            && !models_url.starts_with("http://")
            && !models_url.starts_with("https://")
        {
            self.connect_error("Model fetching URL must start with http:// or https://.");
            return Action::None;
        }
        if model_id.is_empty() && model_name.is_empty() {
            self.connect_error("Enter a model name or model ID.");
            return Action::None;
        }
        let final_id = if model_id.is_empty() {
            model_name.clone()
        } else {
            model_id.clone()
        };
        let final_name = if model_name.is_empty() {
            final_id.clone()
        } else {
            model_name
        };
        let context_window = if context_window_str.is_empty() {
            Some(128_000)
        } else {
            context_window_str.parse::<u64>().ok().or(Some(128_000))
        };

        if let Some(connect) = self.connect.as_mut() {
            connect.custom_name = name;
            connect.custom_endpoint = endpoint;
            connect.custom_models_url = models_url;
            connect.model_id = Some(final_id.clone());
            connect.custom_models = vec![config::SavedModel {
                id: final_id,
                name: final_name,
                context_window,
                input_cost: None,
                output_cost: None,
            }];
            connect.custom_auth = config::AuthKind::Bearer;
            connect.error = None;
        }
        self.finish_custom_connect(None)
    }

    pub(crate) fn finish_connect(
        &mut self,
        api_key: Option<String>,
        password: Option<String>,
    ) -> Action {
        let source = match api_key.filter(|api_key| !api_key.trim().is_empty()) {
            Some(api_key) => KeySource::Entered(api_key),
            None => KeySource::None,
        };
        self.finish_connect_with(source, password)
    }

    /// Connect with a key that was typed now or is already stored.
    ///
    /// A stored key is not written again: whatever is on disk for the endpoint
    /// stays, so an encrypted key stays encrypted.
    pub(crate) fn finish_connect_with(
        &mut self,
        source: KeySource,
        password: Option<String>,
    ) -> Action {
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
            let models = provider
                .models
                .iter()
                .map(|(id, model)| config::SavedModel {
                    id: id.to_owned(),
                    name: model.name.clone(),
                    input_cost: model.cost.as_ref().map(|cost| cost.input),
                    output_cost: model.cost.as_ref().map(|cost| cost.output),
                    context_window: model.limit.as_ref().and_then(|l| l.context),
                })
                .collect::<Vec<_>>();
            (
                provider_name,
                provider.base_url(&provider_id),
                model_id,
                provider.env.first().cloned(),
                models,
                provider_id.clone(),
                provider.kind(&provider_id),
            )
        };
        let (provider_name, base_url, model_id, api_key_env, models, provider_id, provider_kind) =
            plan;
        if base_url.trim().is_empty() {
            if let Some(connect) = self.connect.as_mut() {
                connect.error = Some(format!("{provider_name} has no API endpoint."));
            }
            return Action::None;
        }
        let entered_key = match &source {
            KeySource::Entered(api_key) => Some(api_key.clone()),
            KeySource::Stored(api_key) => Some(api_key.clone()),
            KeySource::None => None,
        };
        let effective_key = entered_key
            .clone()
            .or_else(|| {
                api_key_env
                    .as_deref()
                    .and_then(|name| std::env::var(name).ok())
                    .or_else(|| {
                        (provider_kind == ProviderKind::Gemini)
                            .then(|| std::env::var("GEMINI_API_KEY").ok())
                            .flatten()
                    })
                    .filter(|api_key| !api_key.trim().is_empty())
            })
            .or_else(|| {
                // Switching models on the active endpoint keeps the live key.
                (self.backend.base_url().trim_end_matches('/') == base_url.trim_end_matches('/'))
                    .then(|| self.backend.api_key().map(str::to_owned))
                    .flatten()
            });
        // A stored key is already on disk, so it is not written a second time.
        let reuse_secret = matches!(source, KeySource::Stored(_));
        let auth = match provider_kind {
            ProviderKind::Anthropic => AuthKind::XApiKey,
            _ => AuthKind::Bearer,
        };
        let connection = Connection {
            provider: provider_kind,
            provider_name: provider_name.clone(),
            model: model_id.clone(),
            base_url: base_url.clone(),
            api_key: entered_key,
            api_key_env,
            auth,
            models: models.clone(),
            models_url: None,
            custom_provider_id: None,
            provider_id: Some(provider_id),
            reuse_secret,
        };
        let saved = config::save_connection(&self.config_path, &connection, password.as_deref());
        self.backend.configure(
            provider_kind,
            provider_name.clone(),
            model_id.clone(),
            base_url,
            effective_key,
            auth,
            models,
        );
        self.connect = None;
        if let Err(error) = saved {
            self.error(format!("Connected, but could not save: {error:#}"));
        } else {
            self.info(format!("Connected to {provider_name} · {model_id}"));
        }
        Action::None
    }

    /// The secret already stored for an endpoint, if the user connected it.
    pub(crate) fn stored_secret_for(&self, base_url: &str) -> Option<config::StoredSecret> {
        config::load_endpoint_secret(&self.config_path, base_url)
    }

    /// Fetch the model list from the custom provider's models endpoint.
    pub(crate) fn fetch_custom_models(&mut self) -> Action {
        let (endpoint, configured, auth, token) = {
            let Some(connect) = self.connect.as_ref() else {
                return Action::None;
            };
            let endpoint = connect.custom_endpoint.trim_end_matches('/').to_owned();
            let configured = connect.custom_models_url.trim().to_owned();
            let typed = connect.custom_token.trim().to_owned();
            let same_endpoint = self.backend.base_url().trim_end_matches('/') == endpoint;
            let token = if typed.is_empty() {
                connect
                    .unlocked_key
                    .clone()
                    .or_else(|| {
                        connect
                            .stored_secret
                            .as_ref()
                            .and_then(|secret| secret.plaintext().map(str::to_owned))
                    })
                    .or_else(|| {
                        same_endpoint
                            .then(|| self.backend.api_key().map(str::to_owned))
                            .flatten()
                    })
            } else {
                Some(typed)
            };
            (endpoint, configured, connect.custom_auth, token)
        };
        if endpoint.is_empty() {
            self.connect_error("Enter the completion endpoint first.");
            return Action::None;
        }
        let url = if configured.is_empty() {
            format!("{endpoint}/models")
        } else {
            configured
        };
        if let Some(connect) = self.connect.as_mut() {
            connect.loading = true;
            connect.error = None;
        }
        Action::FetchCustomModels { url, token, auth }
    }

    /// The custom endpoint's model list arrived: merge it into the manager.
    pub(crate) fn on_custom_models(&mut self, result: Result<Vec<(String, String)>, String>) {
        let Some(connect) = self.connect.as_mut() else {
            return;
        };
        connect.loading = false;
        match result {
            Err(error) => connect.error = Some(format!("Could not fetch models: {error}")),
            Ok(models) => {
                for (id, name) in models {
                    let id = id.trim().to_owned();
                    if id.is_empty() {
                        continue;
                    }
                    match connect
                        .custom_models
                        .iter_mut()
                        .find(|model| model.id == id)
                    {
                        Some(existing) => {
                            if !name.is_empty() {
                                existing.name = name;
                            }
                        }
                        None => connect.custom_models.push(SavedModel {
                            id,
                            name,
                            ..SavedModel::default()
                        }),
                    }
                }
                connect.stage = ConnectStage::CustomModels;
                connect.selected = 0;
                connect.error = if connect.custom_models.is_empty() {
                    Some("The models endpoint returned no models; add one manually.".to_owned())
                } else {
                    None
                };
            }
        }
    }

    pub(crate) fn finish_custom_connect(&mut self, password: Option<String>) -> Action {
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
                connect.unlocked_key.clone(),
                {
                    let url = connect.custom_models_url.trim();
                    (!url.is_empty()).then(|| url.to_owned())
                },
            )
        };
        let (
            endpoint,
            model_id,
            auth,
            token,
            models,
            name,
            custom_provider_id,
            unlocked,
            models_url,
        ) = plan;
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
        let stored_key = if auth.requires_token() && !token.is_empty() {
            Some(token)
        } else {
            None
        };
        // A saved endpoint keeps its key on disk. When it is the active
        // endpoint, reuse the live key so switching models never asks for the
        // encryption password again.
        let runtime_key = if auth.requires_token() {
            stored_key.clone().or_else(|| unlocked.clone()).or_else(|| {
                (self.backend.base_url().trim_end_matches('/') == endpoint.trim_end_matches('/'))
                    .then(|| self.backend.api_key().map(str::to_owned))
                    .flatten()
            })
        } else {
            None
        };
        if auth.requires_token() && runtime_key.is_none() {
            if let Some(connect) = self.connect.as_mut() {
                connect.stage = ConnectStage::CustomToken;
                connect.error = Some(format!("Enter a token for {}.", auth.label()));
            }
            return Action::None;
        }
        // An unlocked key is already on disk: it is not written again, so an
        // encrypted token stays encrypted.
        let reuse_secret = stored_key.is_none() && unlocked.is_some();
        let connection = Connection {
            provider: ProviderKind::Custom,
            provider_name: name.clone(),
            model: model_id.clone(),
            base_url: endpoint.clone(),
            api_key: stored_key.clone(),
            api_key_env: None,
            auth,
            models: models.clone(),
            models_url,
            custom_provider_id,
            provider_id: None,
            reuse_secret,
        };
        let saved = config::save_connection(&self.config_path, &connection, password.as_deref());
        self.backend.configure(
            ProviderKind::Custom,
            name.clone(),
            model_id.clone(),
            endpoint,
            runtime_key,
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

    pub(crate) fn connect_move(&mut self, forward: bool) {
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
}
