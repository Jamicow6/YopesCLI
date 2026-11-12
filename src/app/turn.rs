use super::*;

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Whether a command can run while the model is still answering.
///
/// Commands that only read state or change a setting are safe: they do not
/// touch the turn in flight. The ones that would swap the session, the
/// provider or the conversation are not.
fn command_is_safe_while_streaming(prompt: &str) -> bool {
    let name = prompt.split_whitespace().next().unwrap_or_default();
    matches!(
        name,
        "/help" | "/models" | "/tools" | "/system" | "/effort" | "/skills" | "/version" | "/quit"
    )
}

impl App {
    pub(crate) fn scroll_up(&mut self, amount: usize) {
        self.follow_tail = false;
        self.scroll = self.scroll.saturating_sub(amount);
    }

    pub(crate) fn scroll_down(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    pub(crate) fn submit(&mut self) -> Action {
        let steering = self.is_streaming();
        if self.is_busy() && !steering {
            self.info("Wait for the current response or tool to finish.");
            return Action::None;
        }
        if steering
            && self
                .active
                .as_ref()
                .is_some_and(|active| active.compacting.is_some())
        {
            self.info("Wait for compaction to finish.");
            return Action::None;
        }
        let mut value = self.input.take();
        self.reset_palette();
        let prompt = value.trim().to_owned();
        if prompt.is_empty() && self.attachments.is_empty() {
            return Action::None;
        }
        if prompt.starts_with('/') {
            // Builtins never lose their name to a skill of the same spelling.
            if COMMANDS
                .iter()
                .any(|command| prompt.split_whitespace().next() == Some(command.name))
            {
                // A command that only reads or changes a setting is safe to
                // run while the model is answering; the ones that would
                // disturb the turn are not.
                if steering && !command_is_safe_while_streaming(&prompt) {
                    // The command is put back, so it can be run after the
                    // response finishes.
                    self.input.set(prompt);
                    self.info("Finish or stop the response before running that command.");
                    return Action::None;
                }
                return self.command(&prompt);
            }
            let expanded = std::env::current_dir()
                .ok()
                .and_then(|cwd| skills::expand(&prompt, &cwd));
            match expanded {
                // A skill invocation becomes a normal turn that carries the
                // skill body, so it works with tools off too.
                Some(expanded) => value = expanded,
                None => return self.command(&prompt),
            }
        }
        if !self.backend.is_configured() {
            self.error("No provider connected. Run /connect to get started.");
            return Action::None;
        }

        self.notice = None;
        self.send_pulse = Some(self.spinner);
        self.tool_rounds = 0;
        self.session.update_title(&prompt);
        if steering {
            // Drop the blank reply the interrupted request left behind, then
            // fold the steer into the conversation and restart the turn.
            self.discard_pending_tools("The user steered the response before this tool could run.");
            self.remove_empty_response();
            // Several steers before the model answered read as one message
            // block, not one bubble per line.
            if let Some(last) = self
                .session
                .messages
                .last_mut()
                .filter(|message| message.role == Role::User)
            {
                if !last.content.trim().is_empty() && !value.trim().is_empty() {
                    last.content.push_str("\n\n");
                }
                last.content.push_str(&value);
                last.images.extend(std::mem::take(&mut self.attachments));
                return self.begin_turn();
            }
        }
        let mut message = Message::new(Role::User, value);
        message.images = std::mem::take(&mut self.attachments);
        self.session.messages.push(message);
        self.begin_turn()
    }

    /// Start the next turn: when the history outgrows the context budget,
    /// summarize the older turns first, then send the real request.
    pub(crate) fn begin_turn(&mut self) -> Action {
        let total = Session::estimate_tokens(&self.session.messages);
        let (threshold, keep) = match self.model_context_window() {
            Some(window) => (
                window / COMPACTION_THRESHOLD_FRAC,
                window / COMPACTION_KEEP_FRAC,
            ),
            None => (COMPACTION_THRESHOLD_DEFAULT, COMPACTION_KEEP_DEFAULT),
        };
        if self.auto_compact && total > threshold {
            if let Some(split) = self.session.compaction_split(keep) {
                if split > 0 {
                    return self.begin_compaction(split);
                }
            }
        }
        let messages = self.session.messages.clone();
        self.begin_request(messages, None)
    }

    /// Summarize everything before `split`, then continue with the turn.
    pub(crate) fn begin_compaction(&mut self, split: usize) -> Action {
        let mut messages = self.session.messages[..split].to_vec();
        messages.push(Message::new(
            Role::User,
            "Summarize the conversation above for an assistant taking over: what the user wants, decisions made, files changed, and anything still unfinished. Reply with the summary only.",
        ));
        self.begin_request(messages, Some(split))
    }

    pub(crate) fn begin_request(
        &mut self,
        messages: Vec<Message>,
        compacting: Option<usize>,
    ) -> Action {
        // A compaction request streams into its own buffer, not the transcript,
        // so no reply placeholder is pushed.
        if compacting.is_none() {
            self.session
                .messages
                .push(Message::new(Role::Assistant, ""));
        }
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let request_id = self.next_request_id;
        let tools_enabled = self.mode != Mode::Chat && self.tools_mode.advertised();
        let mode_note = self.mode.instruction();
        let prompt_chars = tools::system_prompt(tools_enabled).chars().count()
            + self
                .backend
                .system_prompt()
                .map_or(0, |prompt| prompt.chars().count() + 2)
            + mode_note.map_or(0, |note| note.chars().count() + 2)
            + messages
                .iter()
                .map(|message| {
                    message.content.chars().count()
                        + message.tool_calls.as_ref().map_or(0, |calls| {
                            calls
                                .iter()
                                .map(|call| {
                                    call.function.name.chars().count()
                                        + call.function.arguments.chars().count()
                                })
                                .sum::<usize>()
                        })
                })
                .sum::<usize>();
        self.active = Some(ActiveRequest {
            id: request_id,
            started: Instant::now(),
            prompt_tokens: prompt_chars as u64 / 4,
            usage: None,
            tool_progress: None,
            compacting: compacting.map(|split| Compaction {
                split,
                text: String::new(),
            }),
        });
        self.follow_tail = true;
        self.save_session();
        Action::Start {
            request_id,
            messages,
            tools_enabled,
            mode_note,
        }
    }

    pub(crate) fn command(&mut self, command: &str) -> Action {
        let mut parts = command.split_whitespace();
        match parts.next().unwrap_or_default() {
            "/help" => self.show_help = true,
            "/new" => self.new_session(),
            "/connect" => return self.begin_connect(),
            "/cd" => {
                let path = parts.collect::<Vec<_>>().join(" ");
                self.change_directory(&path);
            }
            "/models" => {
                let model = parts.collect::<Vec<_>>().join(" ");
                if model.is_empty() {
                    self.open_model_picker();
                } else {
                    self.backend.set_model(model.clone());
                    self.persist_model(&model);
                    self.info(format!("Model changed to {model}"));
                }
            }
            "/version" => {
                self.info(format!("yopes {}", crate::update::current_version()));
            }
            "/sessions" => self.open_session_picker(),
            "/system" => {
                let rest = parts.collect::<Vec<_>>().join(" ");
                let current = self.backend.system_prompt().map(str::to_owned);
                if rest.is_empty() {
                    if let Some(current) = current {
                        self.info(format!("Custom system prompt: {current}"));
                    } else {
                        self.info(
                            "No custom system prompt · /system <text> to set one, /system off to remove it",
                        );
                    }
                } else if matches!(rest.as_str(), "off" | "clear" | "none") {
                    self.backend.set_system_prompt(None);
                    self.persist_system_prompt(None);
                    self.info("Custom system prompt removed");
                } else {
                    self.backend.set_system_prompt(Some(rest.clone()));
                    self.persist_system_prompt(Some(rest));
                    self.info("Custom system prompt set");
                }
            }
            "/tools" => {
                let value = parts.next().unwrap_or_default().to_ascii_lowercase();
                if value.is_empty() {
                    self.info(format!(
                        "Tools: {} — {} · /tools on|off|auto|ask",
                        self.tools_mode.label(),
                        self.tools_mode.detail()
                    ));
                } else if let Some(mode) = ToolsMode::parse(&value) {
                    self.tools_mode = mode;
                    let suffix = if mode.advertised()
                        && !matches!(
                            self.backend.provider_kind(),
                            ProviderKind::OpenAi | ProviderKind::Custom
                        ) {
                        " · tools require an OpenAI-compatible provider"
                    } else {
                        ""
                    };
                    self.info(format!(
                        "Tools: {} — {}{suffix}",
                        mode.label(),
                        mode.detail()
                    ));
                } else {
                    self.error("Usage: /tools on|off|auto|ask");
                }
            }
            "/compact" => {
                let value = parts.next().unwrap_or_default().to_ascii_lowercase();
                if value.is_empty() || value == "now" {
                    if !self.backend.is_configured() {
                        self.error("No provider connected. Run /connect to get started.");
                    } else if let Some(split) = self
                        .session
                        .compaction_split(
                            self.model_context_window()
                                .map(|w| w / COMPACTION_KEEP_FRAC)
                                .unwrap_or(COMPACTION_KEEP_DEFAULT),
                        )
                        .filter(|split| *split > 0)
                    {
                        return self.begin_compaction(split);
                    } else {
                        self.info(format!(
                            "Nothing to compact · auto compact {} · /compact on|off|now",
                            if self.auto_compact { "on" } else { "off" }
                        ));
                    }
                } else if matches!(value.as_str(), "on" | "off") {
                    self.auto_compact = value == "on";
                    self.persist_auto_compact(self.auto_compact);
                    self.info(if self.auto_compact {
                        "Auto compact on — older turns summarize once the context fills".to_owned()
                    } else {
                        "Auto compact off — the full history is sent; /compact summarizes \
                         manually"
                            .to_owned()
                    });
                } else {
                    self.error("Usage: /compact on|off|now");
                }
            }
            "/effort" => {
                let value = parts.next().unwrap_or_default().to_ascii_lowercase();
                if value.is_empty() {
                    match self.backend.reasoning_effort() {
                        Some(effort) => self.info(format!(
                            "Reasoning effort: {effort} · /effort to toggle, {EFFORT_CHOICES}"
                        )),
                        None => self.info(format!(
                            "Reasoning effort: backend default · {EFFORT_CHOICES}"
                        )),
                    }
                } else if matches!(value.as_str(), "off" | "none") {
                    self.persist_reasoning_effort(None);
                    self.backend.set_reasoning_effort(None);
                    self.info("Reasoning effort off — the backend default applies");
                } else if matches!(value.as_str(), "low" | "medium" | "high" | "xhigh") {
                    self.persist_reasoning_effort(Some(value.clone()));
                    self.backend.set_reasoning_effort(Some(value.clone()));
                    self.info(format!("Reasoning effort set to {value}"));
                } else {
                    self.error(format!("Usage: {EFFORT_CHOICES}"));
                }
            }
            "/skills" => self.show_skills(),
            "/quit" => {
                self.should_quit = true;
                return Action::Quit;
            }
            unknown => self.error(format!(
                "Unknown command `{unknown}`. Type / to see commands."
            )),
        }
        Action::None
    }

    /// List what is installed, one line per skill, including where it came
    /// from. Nothing here reads files; discovery only checks names.
    pub(crate) fn show_skills(&mut self) {
        let cwd = std::env::current_dir().unwrap_or_default();
        let message = skills::summary(&cwd).unwrap_or_else(|| {
            "No skills installed · add Skill.md files under ~/.yopes/skills/<name>/ \
             or .yopes/skills/<name>/"
                .to_owned()
        });
        self.info(message);
    }

    /// Switch the process working directory, so every path-based tool and the
    /// next session resolve from there.
    pub(crate) fn change_directory(&mut self, value: &str) {
        let value = value.trim();
        if value.is_empty() {
            match std::env::current_dir() {
                Ok(cwd) => self.info(format!("Working directory: {}", cwd.display())),
                Err(error) => self.error(format!("Could not read the working directory: {error}")),
            }
            return;
        }
        let expanded = if let Some(rest) = value
            .strip_prefix("~/")
            .or_else(|| value.strip_prefix("~\\"))
        {
            home_dir().map(|home| home.join(rest))
        } else if value == "~" {
            home_dir()
        } else {
            Some(PathBuf::from(value))
        };
        let Some(path) = expanded else {
            self.error("Could not resolve the home directory.");
            return;
        };
        let path = if path.is_absolute() {
            path
        } else {
            match std::env::current_dir() {
                Ok(cwd) => cwd.join(path),
                Err(error) => {
                    self.error(format!("Could not read the working directory: {error}"));
                    return;
                }
            }
        };
        let path = match path.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                self.error(format!("Could not open {}: {error}", path.display()));
                return;
            }
        };
        if !path.is_dir() {
            self.error(format!("{} is not a directory.", path.display()));
            return;
        }
        match std::env::set_current_dir(&path) {
            Ok(()) => {
                self.scroll = 0;
                self.follow_tail = true;
                self.info(format!("Working directory: {}", path.display()));
            }
            Err(error) => self.error(format!("Could not change directory: {error}")),
        }
    }

    /// Forget the key stored for one endpoint, resetting the session when it is
    /// the provider in use.
    pub(crate) fn disconnect_endpoint(&mut self, target: &str) {
        if self.is_busy() {
            self.info("Finish the current turn before disconnecting.");
            return;
        }
        match config::disconnect_provider(&self.config_path, target) {
            Ok(()) => {
                // The active connection goes with it.
                if self.backend.base_url().trim_end_matches('/') == target.trim_end_matches('/') {
                    self.backend.configure(
                        ProviderKind::Unconfigured,
                        "not connected".to_owned(),
                        String::new(),
                        String::new(),
                        None,
                        AuthKind::Bearer,
                        Vec::new(),
                    );
                }
                self.custom_providers = config::load_custom_providers(&self.config_path);
                if let Some(connect) = self.connect.as_mut() {
                    // The row loses its tick immediately.
                    connect.saved_custom = self.custom_providers.clone();
                    connect.error = None;
                }
                self.info("Disconnected. The key is asked for again next time.");
            }
            Err(error) => self.error(format!("Could not disconnect: {error:#}")),
        }
    }

    /// Store the model of the active connection, so the choice survives a
    /// restart.
    pub(crate) fn persist_model(&mut self, model: &str) {
        if let Err(error) = config::save_model(&self.config_path, model) {
            self.error(format!("Could not save the model: {error:#}"));
        }
    }

    pub(crate) fn begin_connect(&mut self) -> Action {
        if self.is_busy() {
            self.info("Finish the current turn before connecting a provider.");
            return Action::None;
        }
        self.palette_hidden = true;
        let mut connect = ConnectState::loading();
        connect.saved_custom = self.custom_providers.clone();
        if self.backend.provider_kind() == ProviderKind::Custom {
            connect.custom_endpoint = self.backend.base_url().to_owned();
            connect.custom_auth = self.backend.auth();
            connect.custom_models = self.backend.saved_models().to_vec();
            if let Some(matched) = self
                .custom_providers
                .iter()
                .find(|provider| provider.base_url == connect.custom_endpoint)
            {
                connect.custom_provider_id = Some(matched.id.clone());
                connect.custom_name = matched.name.clone();
                connect.custom_token = matched.token.clone().unwrap_or_default();
            }
        }
        self.connect = Some(connect);
        Action::LoadCatalog
    }

    pub(crate) fn new_session(&mut self) {
        if self.is_busy() {
            self.info("Finish the current turn before starting a new session.");
            return;
        }
        self.session = Session::new();
        // Future saves land in a fresh file, so this one keeps its history.
        if let Some(dir) = &self.sessions_dir {
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |duration| duration.as_millis() as u64);
            self.session_path = Some(dir.join(format!("session-{stamp}.json")));
        }
        self.reset_session_view();
        self.save_session();
    }

    pub(crate) fn stop_streaming(&mut self) -> Action {
        let Some(active) = self.active.take() else {
            return Action::None;
        };
        if active.compacting.is_some() {
            // The queued turn never started, so there is nothing to finish.
            self.info("Compaction stopped. Send another message to continue.");
            self.save_session();
            return Action::Cancel(active.id);
        }
        self.discard_pending_tools("The user stopped the response before this tool could run.");
        self.tool_rounds = 0;
        self.remove_empty_response();
        self.info("Response stopped. Partial output was kept.");
        self.save_session();
        Action::Cancel(active.id)
    }
}
