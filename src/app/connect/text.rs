use super::state::*;
use super::*;

impl App {
    pub(crate) fn handle_connect_custom_text(&mut self, key: KeyEvent) -> Action {
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

    pub(crate) fn back_from_custom_text(&mut self, stage: ConnectStage) {
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
            && matches!(
                stage,
                ConnectStage::Endpoint | ConnectStage::ModelsEndpoint | ConnectStage::CustomToken
            )
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
            ConnectStage::ModelsEndpoint => {
                connect.stage = ConnectStage::Endpoint;
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
            ConnectStage::ModelContextWindow => {
                connect.stage = ConnectStage::ModelName;
                connect.selected = 0;
            }
            _ => {}
        }
    }

    pub(crate) fn advance_custom_text(&mut self, stage: ConnectStage) -> Action {
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
                    // The model list almost always lives at <base>/models; the
                    // field can be changed before continuing.
                    if connect.custom_models_url.trim().is_empty() {
                        connect.custom_models_url =
                            format!("{}/models", connect.custom_endpoint.trim_end_matches('/'));
                    }
                    connect.stage = ConnectStage::ModelsEndpoint;
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            ConnectStage::ModelsEndpoint => {
                let url = self
                    .connect
                    .as_ref()
                    .map(|connect| connect.custom_models_url.trim().to_owned())
                    .unwrap_or_default();
                if !url.is_empty() && !url.starts_with("http://") && !url.starts_with("https://") {
                    self.connect_error("Models endpoint must start with http:// or https://.");
                    return Action::None;
                }
                let auth_index = self
                    .connect
                    .as_ref()
                    .map(ConnectState::auth_index)
                    .unwrap_or(0);
                if let Some(connect) = self.connect.as_mut() {
                    connect.custom_models_url = url;
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
                let mut fetch = false;
                if let Some(connect) = self.connect.as_mut() {
                    connect.custom_token = token;
                    connect.custom_token_dirty = true;
                    if connect.custom_provider_id.is_some() {
                        connect.stage = ConnectStage::CustomModels;
                        connect.selected = 0;
                        connect.error = None;
                        return Action::None;
                    }
                    if connect.custom_models_url.trim().is_empty() {
                        connect.stage = ConnectStage::ModelId;
                        connect.selected = 0;
                        connect.error = None;
                    } else {
                        fetch = true;
                    }
                }
                if fetch {
                    return self.fetch_custom_models();
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
                let id = self
                    .connect
                    .as_ref()
                    .map(|c| c.custom_model_id.trim().to_owned())
                    .unwrap_or_default();
                if id.is_empty() {
                    if let Some(connect) = self.connect.as_mut() {
                        connect.stage = ConnectStage::ModelId;
                        connect.error = Some("Enter a model ID first.".to_owned());
                    }
                    return Action::None;
                }
                if let Some(connect) = self.connect.as_mut() {
                    connect.stage = ConnectStage::ModelContextWindow;
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            ConnectStage::ModelContextWindow => {
                let (id, name, raw_window) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (
                        connect.custom_model_id.trim().to_owned(),
                        connect.custom_model_name.trim().to_owned(),
                        connect.custom_model_context_window.trim().to_owned(),
                    )
                };
                if id.is_empty() {
                    if let Some(connect) = self.connect.as_mut() {
                        connect.stage = ConnectStage::ModelId;
                        connect.error = Some("Enter a model ID first.".to_owned());
                    }
                    return Action::None;
                }
                let context_window: Option<u64> = if raw_window.is_empty() {
                    None
                } else {
                    match raw_window.parse::<u64>() {
                        Ok(n) => Some(n),
                        Err(_) => {
                            self.connect_error(
                                "Context window must be a number (e.g. 128000), or leave blank.",
                            );
                            return Action::None;
                        }
                    }
                };
                if let Some(connect) = self.connect.as_mut() {
                    let existing = connect
                        .custom_models
                        .iter()
                        .position(|model| model.id == id);
                    if let Some(index) = existing {
                        connect.custom_models[index].name = name;
                        // Blank keeps the limit already saved for this model.
                        if !raw_window.is_empty() {
                            connect.custom_models[index].context_window = context_window;
                        }
                    } else {
                        connect.custom_models.push(SavedModel {
                            id: id.clone(),
                            name,
                            context_window,
                            ..SavedModel::default()
                        });
                    }
                    connect.selected = existing.unwrap_or(connect.custom_models.len() - 1);
                    connect.custom_model_id.clear();
                    connect.custom_model_name.clear();
                    connect.custom_model_context_window.clear();
                    connect.stage = ConnectStage::CustomModels;
                    connect.error = None;
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(crate) fn handle_connect_api_key(&mut self, key: KeyEvent) -> Action {
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
                // A key already stored for this endpoint is used as it is, so
                // coming back to a provider does not ask for it again.
                if let Some(key) = self.connect.as_ref().and_then(ConnectState::stored_key) {
                    return self.finish_connect_with(KeySource::Stored(key), None);
                }
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

    pub(crate) fn handle_connect_password(&mut self, key: KeyEvent) -> Action {
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
}
