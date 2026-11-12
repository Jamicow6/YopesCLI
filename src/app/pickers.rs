use super::*;

impl App {
    pub(crate) fn model_matches(&self) -> Vec<&ModelChoice> {
        let filter = self
            .model_picker
            .as_ref()
            .map_or("", |picker| picker.filter.as_str());
        model_matching(&self.model_choices, filter)
    }

    /// Rows shown in the `/models` popup for its current filter.
    pub(crate) fn model_picker_count(&self) -> usize {
        self.model_matches().len()
    }

    /// Every model offered by a connected provider, active provider first.
    fn build_model_choices(&self) -> Vec<ModelChoice> {
        let mut choices: Vec<ModelChoice> = Vec::new();
        let active = self.backend.base_url().trim_end_matches('/').to_owned();
        let mut seen = std::collections::HashSet::new();
        let push = |choices: &mut Vec<ModelChoice>,
                    seen: &mut std::collections::HashSet<(String, String)>,
                    provider: &str,
                    base_url: &str,
                    model: &SavedModel,
                    active_provider: bool| {
            if model.id.trim().is_empty() {
                return;
            }
            let key = (base_url.to_owned(), model.id.clone());
            if !seen.insert(key) {
                return;
            }
            let name = if model.name.trim().is_empty() {
                model.id.clone()
            } else {
                model.name.clone()
            };
            choices.push(ModelChoice {
                provider: provider.to_owned(),
                base_url: base_url.to_owned(),
                id: model.id.clone(),
                name,
                active_provider,
            });
        };
        for model in self.backend.saved_models() {
            push(
                &mut choices,
                &mut seen,
                self.backend.provider_label(),
                &active,
                model,
                true,
            );
        }
        for provider in config::load_connected_providers(&self.config_path) {
            for model in &provider.models {
                push(
                    &mut choices,
                    &mut seen,
                    &provider.name,
                    provider.base_url.trim_end_matches('/'),
                    model,
                    false,
                );
            }
        }
        for provider in &self.custom_providers {
            for model in &provider.models {
                push(
                    &mut choices,
                    &mut seen,
                    &provider.name,
                    provider.base_url.trim_end_matches('/'),
                    model,
                    false,
                );
            }
        }
        choices
    }

    /// `/models` with no argument: a popup over every connected provider's
    /// models; empty lists fall back to the info notice.
    pub(crate) fn open_model_picker(&mut self) {
        self.model_choices = self.build_model_choices();
        if self.model_choices.is_empty() {
            self.info("No saved model list — run /connect to browse models.");
            return;
        }
        self.model_picker = Some(ListPicker::default());
    }

    /// Make one popup choice active, switching provider when it belongs to
    /// another connected endpoint.
    fn pick_model(&mut self, choice: ModelChoice) {
        if choice.base_url.trim_end_matches('/') == self.backend.base_url().trim_end_matches('/') {
            self.backend.set_model(choice.id.clone());
            self.persist_model(&choice.id);
            self.info(format!("Model changed to {}", choice.id));
            return;
        }
        let base = choice.base_url.trim_end_matches('/').to_owned();
        let custom = self
            .custom_providers
            .iter()
            .find(|provider| provider.base_url.trim_end_matches('/') == base)
            .cloned();
        let connected = config::load_connected_providers(&self.config_path)
            .into_iter()
            .find(|provider| provider.base_url.trim_end_matches('/') == base);
        let secret = custom
            .as_ref()
            .map(|provider| provider.secret.clone())
            .or_else(|| config::load_endpoint_secret(&self.config_path, &base));
        let api_key = secret
            .as_ref()
            .and_then(|secret| secret.plaintext().map(str::to_owned))
            .or_else(|| custom.as_ref().and_then(|provider| provider.token.clone()));
        let auth = custom
            .as_ref()
            .map_or(AuthKind::Bearer, |provider| provider.auth);
        if auth.requires_token() && api_key.is_none() {
            self.error(format!(
                "{} has an encrypted key — run /connect to unlock it.",
                choice.provider
            ));
            return;
        }
        let models = custom
            .as_ref()
            .map(|provider| provider.models.clone())
            .or_else(|| connected.as_ref().map(|provider| provider.models.clone()))
            .unwrap_or_default();
        let kind = if custom.is_some() {
            ProviderKind::Custom
        } else {
            ProviderKind::OpenAi
        };
        self.backend.configure(
            kind,
            choice.provider.clone(),
            choice.id.clone(),
            base.clone(),
            api_key,
            auth,
            models.clone(),
        );
        let connection = Connection {
            provider: kind,
            provider_name: choice.provider.clone(),
            model: choice.id.clone(),
            base_url: base,
            api_key: None,
            api_key_env: None,
            auth,
            models,
            models_url: custom
                .as_ref()
                .and_then(|provider| provider.models_url.clone()),
            custom_provider_id: custom.as_ref().map(|provider| provider.id.clone()),
            provider_id: connected.and_then(|provider| provider.provider_id),
            reuse_secret: true,
        };
        if let Err(error) = config::save_connection(&self.config_path, &connection, None) {
            self.error(format!("Could not save the model: {error:#}"));
        }
        self.info(format!(
            "Model changed to {} · {}",
            choice.id, choice.provider
        ));
    }

    pub(crate) fn handle_model_picker_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            self.model_picker = None;
            return Action::None;
        }
        let Some(picker) = self.model_picker.as_mut() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                let count = model_matching(&self.model_choices, &picker.filter).len();
                picker.selected = (picker.selected + 1).min(count.saturating_sub(1));
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.selected = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.filter.push(character);
                picker.selected = 0;
            }
            KeyCode::Enter => {
                let picked = {
                    let matches = model_matching(&self.model_choices, &picker.filter);
                    matches
                        .get(picker.selected.min(matches.len().saturating_sub(1)))
                        .map(|model| (*model).clone())
                };
                self.model_picker = None;
                if let Some(model) = picked {
                    self.pick_model(model);
                }
            }
            _ => {}
        }
        Action::None
    }

    pub(crate) fn open_session_picker(&mut self) {
        if self.is_busy() || self.connect.is_some() {
            self.info("Finish the current turn before switching sessions.");
            return;
        }
        let current = self.session_path.clone().unwrap_or_default();
        let entries = self.collect_sessions();
        if entries.len() == 1 && entries[0].path == current {
            self.info("Only the current session exists yet — /new starts another.");
            return;
        }
        self.session_picker = Some(ListPicker::default());
    }

    /// `/sessions` rows: every file in the sessions directory, newest first,
    /// plus the active file when it lives somewhere else.
    pub(crate) fn collect_sessions(&self) -> Vec<SessionEntry> {
        let mut paths: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &self.sessions_dir {
            if let Ok(list) = fs::read_dir(dir) {
                paths.extend(
                    list.flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().is_some_and(|x| x == "json")),
                );
            }
            if let Some(current) = &self.session_path {
                if !current.starts_with(dir) {
                    paths.push(current.clone());
                }
            }
        } else if let Some(current) = &self.session_path {
            paths.push(current.clone());
        }
        let mut entries: Vec<SessionEntry> = paths.into_iter().map(session_entry).collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.modified));
        entries.truncate(80);
        entries
    }

    pub(crate) fn session_matches(&self) -> Vec<SessionEntry> {
        let entries = self.collect_sessions();
        let filter = self
            .session_picker
            .as_ref()
            .map_or("", |picker| picker.filter.as_str());
        session_matching(&entries, filter)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Rows shown in the `/sessions` popup for its current filter.
    pub(crate) fn session_picker_count(&self) -> usize {
        self.session_matches().len()
    }

    pub(crate) fn handle_session_picker_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            self.session_picker = None;
            return Action::None;
        }
        // Editing a clone and writing it back keeps the borrow rules simple:
        // helper methods borrow the whole App, not single fields.
        let Some(mut picker) = self.session_picker.clone() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                let entries = self.collect_sessions();
                let count = session_matching(&entries, &picker.filter).len();
                picker.selected = (picker.selected + 1).min(count.saturating_sub(1));
            }
            KeyCode::Backspace => {
                picker.filter.pop();
                picker.selected = 0;
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                picker.filter.push(character);
                picker.selected = 0;
            }
            KeyCode::Enter => {
                let entries = self.collect_sessions();
                let matches = session_matching(&entries, &picker.filter);
                let target = matches
                    .get(picker.selected.min(matches.len().saturating_sub(1)))
                    .map(|entry| entry.path.clone());
                self.session_picker = None;
                if let Some(path) = target {
                    self.switch_session(path);
                }
                return Action::None;
            }
            _ => return Action::None,
        }
        self.session_picker = Some(picker);
        Action::None
    }

    /// Swap in the saved session at `path`; future saves go there too.
    pub(crate) fn switch_session(&mut self, path: PathBuf) {
        if Some(&path) == self.session_path.as_ref() {
            return;
        }
        match Session::load(&path) {
            Ok(session) => {
                self.session = session;
                self.session_path = Some(path);
                self.reset_session_view();
                self.save_session();
                self.info(format!("Loaded: {}", self.session.title));
            }
            Err(error) => self.error(format!("Could not load session: {error:#}")),
        }
    }

    /// State that belongs to the view of one session, reset on /new or switch.
    pub(crate) fn reset_session_view(&mut self) {
        self.scroll = 0;
        self.follow_tail = true;
        self.notice = None;
        self.tool_rounds = 0;
        self.pending_tools.clear();
        self.approval = None;
        self.approved_tools.clear();
    }

    /// After the first completed reply of an unnamed session, ask the model
    /// for a short title; it lands back through on_session_title.
    pub(crate) fn maybe_name_session(&mut self) -> Action {
        if self.session.title != "New session" || !self.backend.is_configured() {
            return Action::None;
        }
        let excerpt = |text: &str| -> String {
            text.split('\n')
                .find(|line| !line.trim().is_empty())
                .unwrap_or("")
                .trim()
                .chars()
                .take(400)
                .collect()
        };
        let Some(user) = self
            .session
            .messages
            .iter()
            .find(|message| message.role == Role::User && !message.content.trim().is_empty())
        else {
            return Action::None;
        };
        let Some(reply) =
            self.session.messages.iter().find(|message| {
                message.role == Role::Assistant && !message.content.trim().is_empty()
            })
        else {
            return Action::None;
        };
        Action::NameSession {
            task: format!(
                "Name this conversation with a title of at most five words, capitalized, \
                 no quotes and no final period.\n\nUser message:\n{}\n\nAssistant reply:\n{}\n\n\
                 Answer with only the title.",
                excerpt(&user.content),
                excerpt(&reply.content),
            ),
        }
    }

    /// Result of the background naming request, handled in the main loop.
    /// The id keeps a fast `/sessions` switch from stealing another session's name.
    pub(crate) fn on_session_title(&mut self, session_id: u64, raw: String) {
        if self.session.id != session_id || self.session.title != "New session" {
            return;
        }
        let title: String = raw
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or_default()
            .trim_matches(|character: char| matches!(character, '"' | '\'' | '`' | '*' | '#'))
            .chars()
            .take(48)
            .collect();
        let title = title.trim();
        if title.is_empty() || title.eq_ignore_ascii_case("new session") {
            return;
        }
        self.session.title = title.to_owned();
        self.save_session();
    }
}

/// Filter state of the `/model` popup.
#[derive(Debug, Clone, Default)]
pub struct ListPicker {
    pub filter: String,
    pub selected: usize,
}

/// One row of the `/models` popup: a model on any connected provider.
#[derive(Clone, Debug)]
pub struct ModelChoice {
    pub provider: String,
    pub base_url: String,
    pub id: String,
    pub name: String,
    /// True when the row belongs to the provider the session is using now.
    pub active_provider: bool,
}

/// Saved models matching the picker filter, in stored order.
pub(crate) fn model_matching<'a>(models: &'a [ModelChoice], filter: &str) -> Vec<&'a ModelChoice> {
    let filter = filter.to_ascii_lowercase();
    models
        .iter()
        .filter(|model| {
            filter.is_empty()
                || model.name.to_ascii_lowercase().contains(&filter)
                || model.id.to_ascii_lowercase().contains(&filter)
                || model.provider.to_ascii_lowercase().contains(&filter)
        })
        .collect()
}

/// One row of the `/sessions` popup: a saved session file with its title.
#[derive(Clone)]
pub struct SessionEntry {
    pub path: PathBuf,
    pub title: String,
    pub modified: Option<SystemTime>,
}

/// Title probe: parse only the `title` field out of a session file.
#[derive(Default, serde::Deserialize)]
struct SessionProbe {
    #[serde(default)]
    title: String,
}

pub(crate) fn session_entry(path: PathBuf) -> SessionEntry {
    let title = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<SessionProbe>(&text).ok())
        .map(|probe| probe.title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| "Untitled".to_owned());
    let modified = fs::metadata(&path)
        .ok()
        .and_then(|metadata| metadata.modified().ok());
    SessionEntry {
        path,
        title,
        modified,
    }
}

/// Saved sessions matching the picker filter, newest first.
pub(crate) fn session_matching<'a>(
    sessions: &'a [SessionEntry],
    filter: &str,
) -> Vec<&'a SessionEntry> {
    let filter = filter.to_ascii_lowercase();
    sessions
        .iter()
        .filter(|entry| filter.is_empty() || entry.title.to_ascii_lowercase().contains(&filter))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{ModelChoice, model_matching};

    #[test]
    fn model_matching_filters_on_name_id_and_provider() {
        let choice = |provider: &str, id: &str, name: &str| ModelChoice {
            provider: provider.to_owned(),
            base_url: "https://example.test/v1".to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            active_provider: provider == "active",
        };
        let models = vec![
            choice("active", "glm-4.7", "GLM 4.7"),
            choice("active", "gpt-5.2", ""),
            choice("other", "claude-haiku", "Claude Haiku"),
        ];

        assert_eq!(
            model_matching(&models, "").len(),
            3,
            "empty filter keeps everything"
        );
        let found = model_matching(&models, "GLM");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "glm-4.7");
        let found = model_matching(&models, "gpt");
        assert_eq!(found.len(), 1, "matches the id when the name is empty");
        assert_eq!(found[0].id, "gpt-5.2");
        let found = model_matching(&models, "other");
        assert_eq!(found.len(), 1, "filters by provider too");
        assert_eq!(found[0].id, "claude-haiku");
        assert!(model_matching(&models, "llama").is_empty());
    }
}
