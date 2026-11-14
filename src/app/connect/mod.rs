use super::*;
mod finish;
mod keys;
pub(crate) mod state;
mod text;
pub(crate) use self::state::*;

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
            ConnectStage::ApiKey => self.handle_connect_api_key(key),
            ConnectStage::AddProvider => self.handle_connect_add_provider(key),
            ConnectStage::RenameProvider => self.handle_connect_rename_provider(key),
            ConnectStage::Endpoint
            | ConnectStage::ModelsEndpoint
            | ConnectStage::CustomToken
            | ConnectStage::ModelId
            | ConnectStage::ModelName
            | ConnectStage::ModelContextWindow => self.handle_connect_custom_text(key),
            ConnectStage::CustomModels => self.handle_connect_choice_key(key),
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
                let (password, endpoint) = {
                    let Some(connect) = self.connect.as_ref() else {
                        return Action::None;
                    };
                    (connect.password.clone(), connect.unlock_endpoint.clone())
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
    pub(crate) fn on_catalog(&mut self, result: catalog::FetchResult) {
        self.fetching_models = false;
        let Some(connect) = self.connect.as_mut() else {
            match result {
                Err(error) => self.error(format!("Could not refresh catalog: {error}")),
                Ok(cat) => {
                    for cp in config::load_connected_providers(&self.config_path) {
                        if let Some(pid) = &cp.provider_id {
                            if let Some(prov) = cat.providers.get(pid) {
                                let models: Vec<SavedModel> = prov
                                    .models
                                    .iter()
                                    .map(|(id, m)| SavedModel {
                                        id: id.clone(),
                                        name: m.name.clone(),
                                        context_window: m.limit.as_ref().and_then(|l| l.context),
                                        input_cost: m.cost.as_ref().map(|c| c.input),
                                        output_cost: m.cost.as_ref().map(|c| c.output),
                                    })
                                    .collect();
                                let _ = config::update_endpoint_models(
                                    &self.config_path,
                                    &cp.base_url,
                                    &models,
                                );
                                if self.backend.base_url().trim_end_matches('/')
                                    == cp.base_url.trim_end_matches('/')
                                {
                                    self.backend.set_saved_models(models);
                                }
                            }
                        }
                    }
                    self.model_choices = self.build_model_choices();
                    self.info("Refreshed models catalog.");
                }
            }
            return;
        };
        connect.loading = false;
        match result {
            Ok(catalog) => {
                connect.catalog = Some(catalog);
                connect.error = None;
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

    /// Ask for the logos of the providers currently on screen (deprecated).
    pub(crate) fn request_visible_logos(&mut self) {
        // ponytail: provider logo fetching is deprecated to remove network overhead and TUI lag.
        self.pending_logos.clear();
    }
}
