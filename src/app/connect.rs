use super::*;

impl App {
    pub(crate) fn handle_connect_key(&mut self, key: KeyEvent) -> Action {
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
            ConnectStage::FasfasNotice => self.handle_connect_choice_key(key),
            ConnectStage::FasfasKey => self.handle_connect_fasfas_key(key),
            ConnectStage::ApiKey => self.handle_connect_api_key(key),
            ConnectStage::ProviderName
            | ConnectStage::Endpoint
            | ConnectStage::ModelsEndpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName
            | ConnectStage::ModelContextWindow => self.handle_connect_custom_text(key),
            ConnectStage::Auth | ConnectStage::CustomModels => self.handle_connect_choice_key(key),
            ConnectStage::Password => self.handle_connect_password(key),
            ConnectStage::Unlock => self.handle_connect_unlock(key),
        }
    }

    /// Unlock a key that is already stored for the endpoint.
    ///
    /// Only the password is asked for: the key itself never has to be typed
    /// again.
    pub(crate) fn handle_connect_unlock(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Some(connect) = self.connect.as_mut() {
                    // Back to where the key was asked for.
                    connect.stage = if connect.custom_flow {
                        ConnectStage::CustomModels
                    } else {
                        ConnectStage::Models
                    };
                    connect.password.clear();
                    connect.selected = 0;
                    connect.error = None;
                }
            }
            KeyCode::Enter => {
                let (password, endpoint, target) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (
                        connect.password.clone(),
                        connect.unlock_endpoint.clone(),
                        connect.unlock_target,
                    )
                };
                let Some(secret) = self.stored_secret_for(&endpoint) else {
                    self.connect_error("That key is no longer stored.");
                    return Action::None;
                };
                let api_key = match secret.unlock(&password) {
                    Ok(api_key) => api_key,
                    Err(error) => {
                        self.connect_error(format!("Could not unlock the key: {error:#}"));
                        return Action::None;
                    }
                };
                match target {
                    UnlockTarget::FetchModels => {
                        if let Some(connect) = self.connect.as_mut() {
                            connect.api_key = api_key.clone();
                            connect.unlocked_key = Some(api_key.clone());
                            connect.password.clear();
                        }
                        return self.fetch_fasfas_models(api_key);
                    }
                    UnlockTarget::Finish => {
                        // A custom endpoint keeps the unlocked key aside, so it
                        // can be used without being written again.
                        let custom_flow = self
                            .connect
                            .as_ref()
                            .is_some_and(|connect| connect.custom_flow);
                        if custom_flow {
                            if let Some(connect) = self.connect.as_mut() {
                                connect.unlocked_key = Some(api_key);
                                connect.password.clear();
                            }
                            return self.finish_custom_connect(None);
                        }
                        return self.finish_connect_with(KeySource::Stored(api_key), None);
                    }
                }
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

    pub(crate) fn handle_connect_list_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('d' | 'D'))
        {
            self.disconnect_selected_provider();
            return Action::None;
        }
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

    /// Ctrl+D on a provider row: forget the stored key of the ticked provider.
    fn disconnect_selected_provider(&mut self) {
        let Some(connect) = self.connect.as_ref() else {
            return;
        };
        let Some(item) = connect.selected_item() else {
            return;
        };
        if !item.active {
            self.info("Only the connected provider is marked ✓; select it and press ctrl+d.");
            return;
        }
        let Some(endpoint) = connect.selected_endpoint() else {
            self.info("This row has no saved endpoint to disconnect.");
            return;
        };
        self.disconnect_endpoint(&endpoint);
        // Reset the selection so the now-unconfigured backend does not keep a
        // stale active row under the cursor.
        if let Some(connect) = self.connect.as_mut() {
            connect.selected = connect
                .selected
                .min(connect.items().len().saturating_sub(1));
        }
    }

    pub(crate) fn handle_connect_choice_key(&mut self, key: KeyEvent) -> Action {
        let Some(stage) = self.connect.as_ref().map(|connect| connect.stage) else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => {
                if let Some(connect) = self.connect.as_mut() {
                    // A saved endpoint's manager backs out to the provider list;
                    // the create flow backs out to the endpoint field; Fasfas
                    // step one backs out to the provider list.
                    connect.stage = if connect.custom_provider_id.is_some()
                        || stage == ConnectStage::FasfasNotice
                    {
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
            KeyCode::Delete | KeyCode::Char('d' | 'D')
                if stage == ConnectStage::CustomModels
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                return self.delete_selected_model();
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

    /// Delete key in the model list: drop the row and persist it on disk.
    pub(crate) fn delete_selected_model(&mut self) -> Action {
        let Some(id) = self
            .connect
            .as_mut()
            .and_then(ConnectState::remove_selected_model)
        else {
            return Action::None;
        };
        // The provider id decides whether the deletion is persisted; in the
        // create flow the endpoint is not saved yet, so nothing to rewrite.
        let Some(provider_id) = self
            .connect
            .as_ref()
            .and_then(|connect| connect.custom_provider_id.clone())
        else {
            return Action::None;
        };
        match config::delete_saved_model(&self.config_path, &provider_id, &id) {
            Ok(()) => {
                self.custom_providers = config::load_custom_providers(&self.config_path);
                self.info(format!("Removed model {id}"));
            }
            Err(error) => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.error = Some(format!("Could not remove {id}: {error:#}"));
                }
            }
        }
        Action::None
    }

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
                            self.connect_error("Context window must be a number (e.g. 128000), or leave blank.");
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
                        connect.custom_models[index].context_window = context_window;
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

    /// Fasfas key entry: first step after the Discord notice. Enter fetches
    /// the model list from ai.coffeevps.com with the key as a bearer token.
    pub(crate) fn handle_connect_fasfas_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.stage = ConnectStage::Providers;
                    connect.filter.clear();
                    connect.selected = 0;
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
                    // A key already stored for this endpoint is reused, so the
                    // preferred provider does not ask for its key twice.
                    if let Some(secret) = self
                        .connect
                        .as_ref()
                        .and_then(|connect| connect.stored_secret.clone())
                    {
                        if secret.has_key() {
                            if secret.encrypted {
                                // Only the password is needed to unlock it.
                                if let Some(connect) = self.connect.as_mut() {
                                    connect.unlock_endpoint = catalog::FASFAST_BASE_URL.to_owned();
                                    connect.unlock_target = UnlockTarget::FetchModels;
                                    connect.password.clear();
                                    connect.stage = ConnectStage::Unlock;
                                    connect.error = None;
                                }
                                return Action::None;
                            }
                            if let Some(key) = secret.plaintext() {
                                let key = key.to_owned();
                                if let Some(connect) = self.connect.as_mut() {
                                    connect.api_key = key.clone();
                                }
                                return self.fetch_fasfas_models(key);
                            }
                        }
                    }
                    self.connect_error("Enter a Fasfas key from Discord.");
                    return Action::None;
                }
                if let Some(connect) = self.connect.as_mut() {
                    connect.loading = true;
                    connect.error = None;
                }
                return Action::FetchFasfas(api_key);
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
                } else if item.id == FASFAST_ID {
                    connect.custom_flow = false;
                    connect.provider_id = Some(FASFAST_ID.to_owned());
                    connect.model_id = None;
                    connect.api_key.clear();
                    connect.filter.clear();
                    connect.selected = 0;
                    connect.error = None;
                    // The Discord key stays with this endpoint, so a second
                    // visit reuses it instead of asking again.
                    connect.stored_secret =
                        config::load_endpoint_secret(&config_path, catalog::FASFAST_BASE_URL);
                    // Show where the key comes from; Enter on the notice row
                    // or on this list both move to the key input.
                    connect.stage = ConnectStage::FasfasNotice;
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
                    connect.stage = ConnectStage::ProviderName;
                } else {
                    connect.provider_id = Some(item.id);
                    connect.custom_flow = false;
                    // A catalog provider keeps its key under its own endpoint,
                    // so coming back to it does not ask for the key again.
                    let endpoint = connect.endpoint();
                    connect.stored_secret = config::load_endpoint_secret(&config_path, &endpoint);
                    connect.stage = ConnectStage::Models;
                }
            }
            ConnectStage::FasfasNotice => {
                connect.filter.clear();
                connect.selected = 0;
                connect.error = None;
                connect.stage = ConnectStage::FasfasKey;
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
                if connect.provider_id.as_deref() == Some(FASFAST_ID) {
                    // The Fasfas key was entered earlier: connect directly.
                    let key = connect.api_key.trim().to_owned();
                    if key.is_empty() {
                        connect.stage = ConnectStage::FasfasKey;
                        connect.error = Some("Enter a Fasfas key from Discord.".to_owned());
                        return Action::None;
                    }
                    return self.finish_connect(Some(key), None);
                }
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
                    connect.unlock_target = UnlockTarget::Finish;
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
                    connect.unlock_target = UnlockTarget::Finish;
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
            | ConnectStage::FasfasKey => {}
        }
        if finish_now {
            return self.finish_custom_connect(None);
        }
        Action::None
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
                provider_id,
            )
        };
        let (provider_name, base_url, model_id, api_key_env, models, provider_id) = plan;
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
        let connection = Connection {
            provider: ProviderKind::OpenAi,
            provider_name: provider_name.clone(),
            model: model_id.clone(),
            base_url: base_url.clone(),
            api_key: entered_key,
            api_key_env,
            auth: AuthKind::Bearer,
            models: models.clone(),
            models_url: None,
            custom_provider_id: None,
            provider_id: Some(provider_id),
            reuse_secret,
        };
        let saved = config::save_connection(&self.config_path, &connection, password.as_deref());
        self.backend.configure(
            ProviderKind::OpenAi,
            provider_name.clone(),
            model_id.clone(),
            base_url,
            effective_key,
            AuthKind::Bearer,
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
    fn stored_secret_for(&self, base_url: &str) -> Option<config::StoredSecret> {
        config::load_endpoint_secret(&self.config_path, base_url)
    }

    /// Fetch the Fasfas model list with a key.
    fn fetch_fasfas_models(&mut self, key: String) -> Action {
        if let Some(connect) = self.connect.as_mut() {
            connect.loading = true;
            connect.error = None;
        }
        Action::FetchFasfas(key)
    }

    /// Fetch the model list from the custom provider's models endpoint.
    fn fetch_custom_models(&mut self) -> Action {
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

    pub(crate) fn on_catalog(&mut self, result: catalog::FetchResult) {
        let Some(connect) = self.connect.as_mut() else {
            return;
        };
        connect.loading = false;
        match result {
            Ok(catalog) => {
                connect.catalog = Some(catalog);
                connect.error = None;
                // The Fasfas fetch hands back a one-provider synthetic catalog:
                // jump from the key field straight to its model list. The
                // initial models.dev fetch leaves the provider list alone.
                if connect.stage == ConnectStage::FasfasKey
                    && connect.provider_id.as_deref() == Some(FASFAST_ID)
                {
                    connect.stage = ConnectStage::Models;
                    connect.filter.clear();
                }
                connect.selected = 0;
            }
            Err(error) => {
                connect.error = Some(error);
            }
        }
        // The provider list is on screen, so its logos are wanted.
        self.request_visible_logos();
    }

    /// A logo arrived: keep it for the next draw, and cache it on disk.
    pub(crate) fn on_logo(
        &mut self,
        provider_id: String,
        result: Result<crate::logos::Logo, String>,
    ) {
        if let Ok(logo) = result {
            self.logos.cache(&provider_id, &logo);
            self.logos.insert(&provider_id, logo);
        }
    }

    /// The logos that still have to be fetched, taken out of the queue.
    pub(crate) fn take_pending_logos(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_logos)
    }

    /// Ask for the logos of the providers currently on screen.
    ///
    /// Only the ones that are not drawn yet are fetched, and the ones already
    /// on disk are read instead of downloaded again.
    pub(crate) fn request_visible_logos(&mut self) {
        let Some(connect) = self.connect.as_ref() else {
            return;
        };
        // Only the provider rows carry model artwork; the model list would
        // otherwise request /logos/<model-id>.svg for every model on screen.
        if connect.stage != ConnectStage::Providers {
            return;
        }
        let ids: Vec<String> = connect
            .items()
            .into_iter()
            .filter(|item| item.custom_id.is_none())
            .map(|item| item.id)
            .filter(|id| id != CUSTOM_PROVIDER_ID && id != REFRESH_CATALOG_ID)
            .collect();
        for id in ids {
            if self.logos.has(&id) || self.pending_logos.contains(&id) {
                continue;
            }
            if self.logos.load_cached(&id) {
                continue;
            }
            // The catalog id names the logo file, so a provider without one
            // simply never appears.
            self.pending_logos.push(id);
        }
    }
}
