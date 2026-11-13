use super::*;
mod finish;
mod keys;
pub(crate) mod state;
mod text;
use self::state::*;

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
