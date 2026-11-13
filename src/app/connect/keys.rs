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
}
