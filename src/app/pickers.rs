use super::*;

impl App {
    pub(crate) fn model_matches(&self) -> Vec<&SavedModel> {
        let filter = self
            .model_picker
            .as_ref()
            .map_or("", |picker| picker.filter.as_str());
        model_matching(self.backend.saved_models(), filter)
    }

    /// Rows shown in the `/model` popup for its current filter.
    pub(crate) fn model_picker_count(&self) -> usize {
        self.model_matches().len()
    }

    /// `/model` with no argument: a popup over the endpoint's saved models;
    /// empty lists fall back to the info notice.
    pub(crate) fn open_model_picker(&mut self) {
        if self.backend.saved_models().is_empty() {
            self.info("No saved model list for this endpoint — run /connect to browse models.");
            return;
        }
        self.model_picker = Some(ListPicker::default());
    }

    pub(crate) fn handle_model_picker_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            self.model_picker = None;
            return Action::None;
        }
        let picker = match self.model_picker.as_mut() {
            Some(picker) => picker,
            None => return Action::None,
        };
        match key.code {
            KeyCode::Up => picker.selected = picker.selected.saturating_sub(1),
            KeyCode::Down => {
                let count = model_matching(self.backend.saved_models(), &picker.filter).len();
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
                    let matches = model_matching(self.backend.saved_models(), &picker.filter);
                    matches
                        .get(picker.selected.min(matches.len().saturating_sub(1)))
                        .map(|model| model.id.clone())
                };
                self.model_picker = None;
                if let Some(model) = picked {
                    self.backend.set_model(model.clone());
                    self.info(format!("Model changed to {model}"));
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
        entries.sort_by(|a, b| b.modified.cmp(&a.modified));
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
