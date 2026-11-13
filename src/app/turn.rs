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
        "/help"
            | "/models"
            | "/tools"
            | "/permission"
            | "/permissions"
            | "/yolo"
            | "/system"
            | "/effort"
            | "/skills"
            | "/version"
            | "/quit"
    )
}

fn normalize_endpoint_url(url: &str) -> (String, &str) {
    let url = url.trim().trim_end_matches('/');
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((host, path)) = rest.split_once('/') {
            (
                format!(
                    "{}://{}",
                    scheme.to_ascii_lowercase(),
                    host.to_ascii_lowercase()
                ),
                path,
            )
        } else {
            (
                format!(
                    "{}://{}",
                    scheme.to_ascii_lowercase(),
                    rest.to_ascii_lowercase()
                ),
                "",
            )
        }
    } else {
        (url.to_ascii_lowercase(), "")
    }
}

fn endpoints_match(a: &str, b: &str) -> bool {
    normalize_endpoint_url(a) == normalize_endpoint_url(b)
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
        // A pending approval still owns the keyboard; a running tool does not.
        if self.approval.is_some() {
            self.info("Approve or deny the pending tool first.");
            return Action::None;
        }
        let tool_busy = self.running_tool.is_some() || !self.pending_tools.is_empty();
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
        if prompt.starts_with('!') {
            let cmd = prompt.trim_start_matches('!').trim();
            if cmd.is_empty() {
                self.info("Usage: !<command>");
                return Action::None;
            }
            if tool_busy {
                self.input.set(prompt);
                self.info("Finish or stop the tool before running that command.");
                return Action::None;
            }
            if steering {
                self.stop_streaming();
            }
            return self.run_user_command(cmd);
        }
        if prompt.starts_with('/') {
            // Builtins never lose their name to a skill of the same spelling.
            if COMMANDS
                .iter()
                .any(|command| prompt.split_whitespace().next() == Some(command.name))
            {
                // A command that only reads or changes a setting is safe to
                // run while the model is answering or a tool is running; the
                // ones that would disturb the turn are not.
                if tool_busy {
                    self.input.set(prompt);
                    self.info("Finish or stop the tool before running that command.");
                    return Action::None;
                }
                if steering && !command_is_safe_while_streaming(&prompt) {
                    self.stop_streaming();
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
                None => {
                    if tool_busy {
                        self.input.set(prompt);
                        self.info("Finish or stop the tool before running that command.");
                        return Action::None;
                    }
                    if steering && !command_is_safe_while_streaming(&prompt) {
                        self.stop_streaming();
                    }
                    return self.command(&prompt);
                }
            }
        }
        if !self.backend.is_configured() {
            self.error("No provider connected. Run /connect to get started.");
            return Action::None;
        }

        self.notice = None;
        self.send_pulse = Some(self.spinner);
        self.session.update_title(&prompt);
        self.prompt_started = Some(Instant::now());
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
        // Chat during a tool run queues the message: the post-tool
        // continuation sends it together with the tool result.
        if tool_busy {
            self.follow_tail = true;
            self.save_session();
            return Action::None;
        }
        self.begin_turn()
    }

    /// Start the next turn: when the history outgrows the context budget,
    /// summarize the older turns first, then send the real request.
    pub(crate) fn begin_turn(&mut self) -> Action {
        let total = Session::estimate_tokens(&self.session.messages);
        let window = self.model_context_window().unwrap_or(128_000);
        let (threshold, keep) = (
            window / COMPACTION_THRESHOLD_FRAC,
            window / COMPACTION_KEEP_FRAC,
        );
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
        let mode_note = self.mode.instruction(self.tools_mode);
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
            "/disconnect" => {
                let target = parts.collect::<Vec<_>>().join(" ");
                if target.is_empty() {
                    let active_endpoint = self.backend.base_url().to_owned();
                    if active_endpoint.is_empty()
                        || self.backend.provider_kind() == ProviderKind::Unconfigured
                    {
                        self.info("No provider currently connected.");
                    } else {
                        self.disconnect_endpoint(&active_endpoint);
                    }
                } else {
                    self.disconnect_named_provider(&target);
                }
            }
            "/run" => {
                let cmd = parts.collect::<Vec<_>>().join(" ");
                if cmd.is_empty() {
                    self.info("Usage: /run <command>");
                } else {
                    return self.run_user_command(&cmd);
                }
            }
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
            "/tools" | "/permission" | "/permissions" => {
                let value = parts.next().unwrap_or_default().to_ascii_lowercase();
                if value.is_empty() {
                    self.info(format!(
                        "Tools: {} — {} · /tools on|yolo|ask|off",
                        self.tools_mode.label(),
                        self.tools_mode.detail()
                    ));
                } else if let Some(mode) = ToolsMode::parse(&value) {
                    self.tools_mode = mode;
                    let suffix = if mode.advertised()
                        && !matches!(
                            self.backend.provider_kind(),
                            ProviderKind::OpenAi
                                | ProviderKind::Custom
                                | ProviderKind::Anthropic
                                | ProviderKind::Gemini
                        ) {
                        " · tools require an OpenAI, Anthropic, or Gemini provider"
                    } else {
                        ""
                    };
                    self.info(format!(
                        "Tools: {} — {}{suffix}",
                        mode.label(),
                        mode.detail()
                    ));
                } else {
                    self.error("Usage: /tools on|yolo|ask|off");
                }
            }
            "/yolo" => {
                if self.tools_mode == ToolsMode::Auto {
                    self.tools_mode = ToolsMode::On;
                    self.info("Permission: on — edits run automatically, risky commands ask");
                } else {
                    self.tools_mode = ToolsMode::Auto;
                    self.info("Permission: yolo — all commands and edits run automatically without asking");
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
                                .unwrap_or(DEFAULT_CONTEXT_WINDOW / COMPACTION_KEEP_FRAC),
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

    /// Run a shell command in the workspace directly for the user.
    pub(crate) fn run_user_command(&mut self, command: &str) -> Action {
        let command = command.trim();
        if command.is_empty() {
            self.info("Usage: /run <command> or !<command>");
            return Action::None;
        }
        self.notice = None;
        let user_msg = Message::new(Role::User, format!("$ {command}"));
        self.session.messages.push(user_msg);
        let output = match tools::run_user_command(command) {
            Ok(output) => output,
            Err(error) => format!("Command failed: {error:#}"),
        };
        let mut tool_msg = Message::new(Role::Tool, output);
        tool_msg.tool_call_id = Some(format!("user-cmd-{}", self.session.messages.len()));
        self.session.messages.push(tool_msg);
        self.follow_tail = true;
        self.save_session();
        Action::None
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

    /// Remove a saved custom provider, resetting the session when it is the
    /// provider in use.
    pub(crate) fn delete_custom_provider(&mut self, id: &str) {
        if self.is_busy() {
            self.info("Finish the current turn before removing a provider.");
            return;
        }
        let target_endpoint = self
            .custom_providers
            .iter()
            .find(|provider| provider.id == id)
            .map(|provider| provider.base_url.trim_end_matches('/').to_owned());
        match config::delete_custom_provider(&self.config_path, id) {
            Ok(()) => {
                if let Some(target) = target_endpoint {
                    if self.backend.base_url().trim_end_matches('/') == target {
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
                }
                self.custom_providers = config::load_custom_providers(&self.config_path);
                if let Some(connect) = self.connect.as_mut() {
                    connect.saved_custom = self.custom_providers.clone();
                    connect.error = None;
                }
                self.info("Provider removed.");
            }
            Err(error) => self.error(format!("Could not remove provider: {error:#}")),
        }
    }

    /// Rename a saved custom provider, updating active backend if it matches.
    pub(crate) fn rename_custom_provider(&mut self, id: &str, new_name: &str) {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            self.connect_error("Provider name cannot be empty.");
            return;
        }
        let target_endpoint = self
            .custom_providers
            .iter()
            .find(|provider| provider.id == id)
            .map(|provider| provider.base_url.trim_end_matches('/').to_owned());
        match config::rename_custom_provider(&self.config_path, id, new_name) {
            Ok(()) => {
                if let Some(target) = target_endpoint {
                    if self.backend.base_url().trim_end_matches('/') == target {
                        self.backend.set_provider_label(new_name.to_owned());
                    }
                }
                self.custom_providers = config::load_custom_providers(&self.config_path);
                if let Some(connect) = self.connect.as_mut() {
                    connect.saved_custom = self.custom_providers.clone();
                    connect.custom_name = new_name.to_owned();
                    connect.error = None;
                    connect.stage = ConnectStage::CustomModels;
                }
                self.info(format!("Provider renamed to {new_name}."));
            }
            Err(error) => self.connect_error(format!("Could not rename provider: {error:#}")),
        }
    }

    /// Disconnect a provider by name, provider id, or endpoint.
    // ponytail: linear scan over providers; fast enough for typical <20 saved endpoints.
    pub(crate) fn disconnect_named_provider(&mut self, target: &str) {
        let needle = target.trim().to_ascii_lowercase();
        if let Some(custom) = self
            .custom_providers
            .iter()
            .find(|p| p.id.to_ascii_lowercase() == needle || p.name.to_ascii_lowercase() == needle)
        {
            let id = custom.id.clone();
            self.delete_custom_provider(&id);
            return;
        }
        let connected = config::load_connected_providers(&self.config_path);
        if let Some(provider) = connected.iter().find(|p| {
            p.provider_id
                .as_ref()
                .is_some_and(|id| id.to_ascii_lowercase() == needle)
                || p.name.to_ascii_lowercase() == needle
                || endpoints_match(&p.base_url, target)
        }) {
            let endpoint = provider.base_url.clone();
            self.disconnect_endpoint(&endpoint);
            return;
        }
        if self.backend.provider_label().to_ascii_lowercase() == needle
            || endpoints_match(self.backend.base_url(), target)
        {
            let endpoint = self.backend.base_url().to_owned();
            self.disconnect_endpoint(&endpoint);
            return;
        }
        self.info(format!("No saved provider matches '{target}'."));
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
        self.markdown_cache.clear();
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
        self.prompt_started = None;
        if active.compacting.is_some() {
            // The queued turn never started, so there is nothing to finish.
            self.info("Compaction stopped. Send another message to continue.");
            self.save_session();
            return Action::Cancel(active.id);
        }
        self.discard_pending_tools("The user stopped the response before this tool could run.");
        self.remove_empty_response();
        self.info("Response stopped. Partial output was kept.");
        self.save_session();
        Action::Cancel(active.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthKind, Config, ProviderKind};
    use crate::session::ToolCall;
    use std::time::Instant;

    fn test_app() -> App {
        let config = Config {
            provider: ProviderKind::OpenAi,
            provider_label: "OpenAI".to_owned(),
            model: "gpt-4o".to_owned(),
            base_url: "https://api.openai.com/v1".to_owned(),
            api_key: Some("test-key".to_owned()),
            auth: AuthKind::Bearer,
            models: Vec::new(),
            system_prompt: None,
            custom_providers: Vec::new(),
            session_path: None,
            sessions_dir: None,
            config_path: PathBuf::new(),
            tavily_api_key: None,
            auto_compact: false,
            reasoning_effort: None,
        };
        let backend = Backend::new(&config).expect("backend");
        App::new(
            Session::new(),
            backend,
            None,
            None,
            PathBuf::new(),
            Vec::new(),
            None,
            false,
        )
    }

    #[test]
    fn submit_during_tool_run_queues_message_and_blocks_unsafe_commands() {
        let mut app = test_app();
        app.running_tool = Some(RunningTool {
            call: ToolCall::new("call-1", "test_tool", "{}"),
            started: Instant::now(),
            before: None,
            existed: false,
            targets: Vec::new(),
        });

        // Unsafe command while tool runs is blocked and restored to input
        app.input.set("/new");
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert_eq!(app.input.value, "/new");
        assert!(app.notice.is_some());

        // Chat message while tool runs is queued into session messages
        app.input.set("queued turn");
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert_eq!(app.session.messages.len(), 1);
        assert_eq!(app.session.messages[0].content, "queued turn");
        assert_eq!(app.session.messages[0].role, Role::User);
        assert!(app.follow_tail);

        // Approval gate blocks submit
        let mut app_approval = test_app();
        app_approval.approval = Some(ToolApproval {
            call: ToolCall::new("call-2", "test_tool", "{}"),
            note: None,
            diffs: Vec::new(),
            scroll: 0,
            page_height: 0,
        });
        app_approval.input.set("blocked while waiting approval");
        let action = app_approval.submit();
        assert!(matches!(action, Action::None));
        assert!(app_approval.session.messages.is_empty());
    }

    #[test]
    fn submit_command_while_model_streaming_stops_stream_and_runs_command() {
        let mut app = test_app();
        app.active = Some(ActiveRequest {
            id: 1,
            started: Instant::now(),
            prompt_tokens: 10,
            usage: None,
            tool_progress: None,
            compacting: None,
        });
        assert!(app.is_streaming());

        app.input.set("!echo hello_active");
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert!(!app.is_streaming());
        assert_eq!(app.session.messages.len(), 2);
        assert_eq!(app.session.messages[0].content, "$ echo hello_active");
        assert!(app.session.messages[1].content.contains("hello_active"));
    }

    #[test]
    fn user_can_run_commands_via_slash_run_and_exclamation() {
        let mut app = test_app();
        app.input.set("!echo hello_bang");
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert_eq!(app.session.messages.len(), 2);
        assert_eq!(app.session.messages[0].role, Role::User);
        assert_eq!(app.session.messages[0].content, "$ echo hello_bang");
        assert_eq!(app.session.messages[1].role, Role::Tool);
        assert!(app.session.messages[1].content.contains("hello_bang"));

        app.input.set("/run echo hello_slash");
        let action = app.submit();
        assert!(matches!(action, Action::None));
        assert_eq!(app.session.messages.len(), 4);
        assert_eq!(app.session.messages[2].role, Role::User);
        assert_eq!(app.session.messages[2].content, "$ echo hello_slash");
        assert_eq!(app.session.messages[3].role, Role::Tool);
        assert!(app.session.messages[3].content.contains("hello_slash"));
    }

    #[test]
    fn connect_custom_provider_without_models() {
        let temp = crate::temp::unique_dir("config-test");
        let config_path = temp.join("config.json");
        let mut app = test_app();
        app.config_path = config_path;
        app.begin_connect();
        if let Some(connect) = app.connect.as_mut() {
            connect.stage = ConnectStage::AddProvider;
            connect.custom_name = "Ollama".to_owned();
            connect.custom_endpoint = "http://localhost:11434/v1".to_owned();
            connect.custom_model_id = String::new();
            connect.custom_model_name = String::new();
        }
        let action = app.submit_add_provider();
        assert!(matches!(action, Action::None));
        assert!(app.connect.is_none());
        assert_eq!(app.backend.provider_label(), "Ollama");
        assert_eq!(app.backend.base_url(), "http://localhost:11434/v1");
        assert_eq!(app.backend.model(), "");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|n| n.message.contains("Connected to Ollama"))
        );
    }

    #[test]
    fn endpoint_url_matching_preserves_path_case() {
        assert!(endpoints_match(
            "https://Example.COM/v1/chat",
            "https://example.com/v1/chat/"
        ));
        assert!(!endpoints_match(
            "https://example.com/v1/Chat",
            "https://example.com/v1/chat"
        ));
    }

    #[test]
    fn shift_tab_cycles_permission_modes() {
        let mut app = test_app();
        assert_eq!(app.tools_mode, ToolsMode::On);

        // Press Shift+Tab (BackTab) -> Auto (yolo)
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::BackTab,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert_eq!(app.tools_mode, ToolsMode::Auto);
        assert_eq!(app.tools_mode.label(), "yolo");
        assert!(
            app.notice
                .as_ref()
                .is_some_and(|n| n.message.contains("yolo"))
        );

        // Press Shift+Tab (Tab with SHIFT) -> Ask
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Tab,
            crossterm::event::KeyModifiers::SHIFT,
        ));
        assert_eq!(app.tools_mode, ToolsMode::Ask);
        assert_eq!(app.tools_mode.label(), "ask");

        // Press Shift+Tab again -> On
        app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::BackTab,
            crossterm::event::KeyModifiers::SHIFT,
        ));
        assert_eq!(app.tools_mode, ToolsMode::On);
        assert_eq!(app.tools_mode.label(), "on");
    }

    #[test]
    fn slash_yolo_toggles_mode_and_prompt_includes_instruction() {
        let mut app = test_app();
        assert_eq!(app.tools_mode, ToolsMode::On);

        // Run /yolo -> enables YOLO
        app.command("/yolo");
        assert_eq!(app.tools_mode, ToolsMode::Auto);
        assert!(
            app.mode
                .instruction(app.tools_mode)
                .is_some_and(|inst| inst.contains("BUILD MODE (YOLO)"))
        );

        // Run /yolo again -> disables YOLO (back to On)
        app.command("/yolo");
        assert_eq!(app.tools_mode, ToolsMode::On);
        assert!(
            !app.mode
                .instruction(app.tools_mode)
                .is_some_and(|inst| inst.contains("YOLO"))
        );
    }

    #[test]
    fn shift_tab_during_approval_enables_yolo_and_approves() {
        let mut app = test_app();
        app.approval = Some(ToolApproval {
            call: ToolCall::new(
                "call-1",
                "run_command",
                r#"{"command":"cargo test","risk":true}"#,
            ),
            note: None,
            diffs: Vec::new(),
            scroll: 0,
            page_height: 0,
        });
        app.pending_tools.push_back(ToolCall::new(
            "call-1",
            "run_command",
            r#"{"command":"cargo test","risk":true}"#,
        ));

        // Press Shift+Tab (BackTab) during approval
        let action = app.handle_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::BackTab,
            crossterm::event::KeyModifiers::empty(),
        ));
        assert!(matches!(action, Action::RunTool { .. }));
        assert_eq!(app.tools_mode, ToolsMode::Auto);
        assert!(app.approval.is_none());
        assert!(app.running_tool.is_some());
    }

    #[test]
    fn model_end_sse_shows_elapsed_time_since_prompt() {
        let mut app = test_app();
        app.input.set("hello assistant");
        let action = app.submit();
        assert!(matches!(action, Action::Start { .. }));
        assert!(app.prompt_started.is_some());

        // Backdate prompt_started to simulate 500ms elapsed since prompt
        let prompt_time = Instant::now() - std::time::Duration::from_millis(500);
        app.prompt_started = Some(prompt_time);

        let req_id = app.active_request_id().unwrap();
        app.on_stream_event(StreamEvent::Delta {
            request_id: req_id,
            text: "Hello there!".to_owned(),
        });
        app.on_stream_event(StreamEvent::Done { request_id: req_id });

        assert!(app.prompt_started.is_none());
        let last_message = app.session.messages.last().expect("assistant message");
        assert_eq!(last_message.role, Role::Assistant);
        let usage = last_message.usage.expect("usage recorded");
        assert!(usage.elapsed_ms >= 500);
    }

    #[test]
    fn model_end_sse_with_intermediate_tools_measures_full_turn_elapsed() {
        let mut app = test_app();
        app.input.set("check files");
        app.submit();

        let prompt_time = Instant::now() - std::time::Duration::from_millis(1000);
        app.prompt_started = Some(prompt_time);

        let req_id = app.active_request_id().unwrap();
        // Step 1: Model calls tool
        app.on_stream_event(StreamEvent::ToolCalls {
            request_id: req_id,
            calls: vec![ToolCall::new("call-1", "read_file", r#"{"path":"foo"}"#)],
        });
        app.on_stream_event(StreamEvent::Done { request_id: req_id });

        // Intermediate Done does NOT consume prompt_started because tools are pending
        assert_eq!(app.prompt_started, Some(prompt_time));

        // Advance tools to pop and start the tool
        app.advance_tools();
        assert!(app.running_tool.is_some());

        // Tool completes
        app.on_tool_result(ToolEvent {
            call: ToolCall::new("call-1", "read_file", r#"{"path":"foo"}"#),
            result: Ok("file content".to_owned()),
        });

        // Continue after tools starts new request
        let req2_id = app.active_request_id().unwrap();
        app.on_stream_event(StreamEvent::Delta {
            request_id: req2_id,
            text: "done reading".to_owned(),
        });
        app.on_stream_event(StreamEvent::Done {
            request_id: req2_id,
        });

        // Final response end SSE consumes prompt_started and reflects total elapsed
        assert!(app.prompt_started.is_none());
        let last = app.session.messages.last().unwrap();
        let usage = last.usage.expect("usage on final message");
        assert!(usage.elapsed_ms >= 1000);
    }
}
