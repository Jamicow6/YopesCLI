use super::state::*;
use super::*;

impl App {
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
            KeyCode::Delete => {
                if self
                    .connect
                    .as_ref()
                    .is_some_and(|c| c.stage == ConnectStage::Providers)
                {
                    self.disconnect_selected_provider();
                    return Action::None;
                }
            }
            KeyCode::F(2) => {
                if let Some(connect) = self.connect.as_mut() {
                    if connect.stage == ConnectStage::Providers {
                        if let Some(item) = connect.selected_item() {
                            if let Some(custom_id) = item.custom_id.clone() {
                                connect.custom_provider_id = Some(custom_id);
                                connect.custom_name = item.title.clone();
                                connect.stage = ConnectStage::RenameProvider;
                                connect.error = None;
                                return Action::None;
                            }
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                if let Some(connect) = self.connect.as_mut() {
                    connect.filter.pop();
                    connect.selected = 0;
                }
            }
            KeyCode::Tab => {
                if let Some(connect) = self.connect.as_mut() {
                    if connect.stage == ConnectStage::Models {
                        if let Some(item) = connect.selected_item() {
                            if item.id != EDIT_KEY_ID {
                                let current_context = connect
                                    .provider()
                                    .and_then(|p| p.models.get(&item.id))
                                    .and_then(|m| m.limit.as_ref())
                                    .and_then(|l| l.context);
                                connect.model_id = Some(item.id.clone());
                                connect.custom_model_id = item.id;
                                connect.custom_model_name = item.title;
                                connect.custom_model_context_window =
                                    current_context.map(|c| c.to_string()).unwrap_or_default();
                                connect.stage = ConnectStage::ModelContextWindow;
                                connect.error = None;
                            }
                        }
                    }
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

    /// Ctrl+D or Delete on a provider row: remove custom provider or forget stored key.
    fn disconnect_selected_provider(&mut self) {
        let Some(connect) = self.connect.as_ref() else {
            return;
        };
        let Some(item) = connect.selected_item() else {
            return;
        };
        if let Some(custom_id) = item.custom_id.clone() {
            let name = item.title.clone();
            self.delete_custom_provider(&custom_id);
            if let Some(connect) = self.connect.as_mut() {
                connect.selected = connect
                    .selected
                    .min(connect.items().len().saturating_sub(1));
            }
            self.info(format!("Removed provider {name}"));
            return;
        }
        let Some(endpoint) = connect.selected_endpoint() else {
            self.info("This row has no saved endpoint to disconnect.");
            return;
        };
        let has_key = config::load_endpoint_secret(&self.config_path, &endpoint).is_some();
        if !item.active && !has_key {
            self.info("Only saved or connected providers can be disconnected.");
            return;
        }
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
                    // the create flow backs out to the endpoint field.
                    connect.stage = if connect.custom_provider_id.is_some() {
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
            KeyCode::Tab if stage == ConnectStage::CustomModels => {
                if let Some(connect) = self.connect.as_mut() {
                    if let Some(item) = connect.selected_item() {
                        if item.id != ADD_MODEL_ID
                            && item.id != EDIT_ENDPOINT_ID
                            && item.id != EDIT_TOKEN_ID
                            && item.id != EDIT_MODELS_URL_ID
                            && item.id != FETCH_MODELS_ID
                            && item.id != REMOVE_PROVIDER_ID
                        {
                            let current_context = connect
                                .custom_models
                                .iter()
                                .find(|m| m.id == item.id)
                                .and_then(|m| m.context_window);
                            connect.custom_model_id = item.id;
                            connect.custom_model_name = item.title;
                            connect.custom_model_context_window =
                                current_context.map(|c| c.to_string()).unwrap_or_default();
                            connect.stage = ConnectStage::ModelContextWindow;
                            connect.error = None;
                        }
                    }
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

    pub(crate) fn handle_connect_add_provider(&mut self, key: KeyEvent) -> Action {
        let Some(connect) = self.connect.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => {
                connect.stage = ConnectStage::Providers;
                connect.selected = 0;
                connect.error = None;
                Action::None
            }
            KeyCode::Tab => {
                connect.add_provider_field = match connect.add_provider_field {
                    AddProviderField::Name => AddProviderField::CompletionUrl,
                    AddProviderField::CompletionUrl => AddProviderField::ModelsUrl,
                    AddProviderField::ModelsUrl => AddProviderField::ModelName,
                    AddProviderField::ModelName => AddProviderField::ModelId,
                    AddProviderField::ModelId => AddProviderField::ContextWindow,
                    AddProviderField::ContextWindow => AddProviderField::Done,
                    AddProviderField::Done => AddProviderField::Name,
                };
                Action::None
            }
            KeyCode::BackTab => {
                connect.add_provider_field = match connect.add_provider_field {
                    AddProviderField::Name => AddProviderField::Done,
                    AddProviderField::CompletionUrl => AddProviderField::Name,
                    AddProviderField::ModelsUrl => AddProviderField::CompletionUrl,
                    AddProviderField::ModelName => AddProviderField::ModelsUrl,
                    AddProviderField::ModelId => AddProviderField::ModelName,
                    AddProviderField::ContextWindow => AddProviderField::ModelId,
                    AddProviderField::Done => AddProviderField::ContextWindow,
                };
                Action::None
            }
            KeyCode::Up => {
                connect.add_provider_field = match connect.add_provider_field {
                    AddProviderField::Name => AddProviderField::Done,
                    AddProviderField::CompletionUrl => AddProviderField::Name,
                    AddProviderField::ModelsUrl => AddProviderField::CompletionUrl,
                    AddProviderField::ModelName | AddProviderField::ModelId => {
                        AddProviderField::ModelsUrl
                    }
                    AddProviderField::ContextWindow => AddProviderField::ModelName,
                    AddProviderField::Done => AddProviderField::ContextWindow,
                };
                Action::None
            }
            KeyCode::Down => {
                connect.add_provider_field = match connect.add_provider_field {
                    AddProviderField::Name => AddProviderField::CompletionUrl,
                    AddProviderField::CompletionUrl => AddProviderField::ModelsUrl,
                    AddProviderField::ModelsUrl => AddProviderField::ModelName,
                    AddProviderField::ModelName | AddProviderField::ModelId => {
                        AddProviderField::ContextWindow
                    }
                    AddProviderField::ContextWindow => AddProviderField::Done,
                    AddProviderField::Done => AddProviderField::Name,
                };
                Action::None
            }
            KeyCode::Left if connect.add_provider_field == AddProviderField::ModelId => {
                connect.add_provider_field = AddProviderField::ModelName;
                Action::None
            }
            KeyCode::Right if connect.add_provider_field == AddProviderField::ModelName => {
                connect.add_provider_field = AddProviderField::ModelId;
                Action::None
            }
            KeyCode::Enter => {
                if connect.add_provider_field == AddProviderField::Done {
                    self.submit_add_provider()
                } else {
                    connect.add_provider_field = match connect.add_provider_field {
                        AddProviderField::Name => AddProviderField::CompletionUrl,
                        AddProviderField::CompletionUrl => AddProviderField::ModelsUrl,
                        AddProviderField::ModelsUrl => AddProviderField::ModelName,
                        AddProviderField::ModelName => AddProviderField::ModelId,
                        AddProviderField::ModelId => AddProviderField::ContextWindow,
                        AddProviderField::ContextWindow => AddProviderField::Done,
                        AddProviderField::Done => AddProviderField::Done,
                    };
                    Action::None
                }
            }
            KeyCode::Backspace => {
                if let Some(target) = connect.active_add_provider_field_mut() {
                    target.pop();
                    connect.error = None;
                }
                Action::None
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                if let Some(target) = connect.active_add_provider_field_mut() {
                    target.push(c);
                    connect.error = None;
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(crate) fn handle_connect_rename_provider(&mut self, key: KeyEvent) -> Action {
        let Some(connect) = self.connect.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Esc => {
                connect.stage = if connect.custom_provider_id.is_some() {
                    ConnectStage::CustomModels
                } else {
                    ConnectStage::Providers
                };
                connect.error = None;
                Action::None
            }
            KeyCode::Enter => {
                let id = connect.custom_provider_id.clone();
                let new_name = connect.custom_name.clone();
                if let Some(id) = id {
                    self.rename_custom_provider(&id, &new_name);
                }
                Action::None
            }
            KeyCode::Backspace => {
                connect.custom_name.pop();
                connect.error = None;
                Action::None
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                connect.custom_name.push(c);
                connect.error = None;
                Action::None
            }
            _ => Action::None,
        }
    }
}
