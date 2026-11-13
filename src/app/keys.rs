use super::*;

impl App {
    pub(crate) fn handle_paste(&mut self, value: &str) {
        if self.approval.is_some() {
            return;
        }
        if let Some(connect) = self.connect.as_mut() {
            let clean = value
                .chars()
                .filter(|character| !character.is_control())
                .collect::<String>();
            match connect.stage {
                ConnectStage::ApiKey => connect.api_key.push_str(&clean),
                ConnectStage::Password | ConnectStage::Unlock => connect.password.push_str(&clean),
                ConnectStage::ProviderName => connect.custom_name.push_str(&clean),
                ConnectStage::Endpoint => connect.custom_endpoint.push_str(&clean),
                ConnectStage::ModelsEndpoint => connect.custom_models_url.push_str(&clean),
                ConnectStage::CustomToken => connect.custom_token.push_str(&clean),
                ConnectStage::ModelId => connect.custom_model_id.push_str(&clean),
                ConnectStage::ModelName => connect.custom_model_name.push_str(&clean),
                ConnectStage::ModelContextWindow => {
                    connect.custom_model_context_window.push_str(&clean)
                }
                ConnectStage::Providers | ConnectStage::Models => {
                    connect.filter.push_str(&clean);
                    connect.selected = 0;
                }
                ConnectStage::Auth | ConnectStage::CustomModels => {}
            }
            return;
        }
        if !self.show_help {
            self.input.insert_str(value);
            self.reset_palette();
        }
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        if let Some(approval) = self.approval.as_mut() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    approval.scroll = approval.scroll.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    approval.scroll = approval.scroll.saturating_add(3);
                }
                _ => {}
            }
            return;
        }
        if self.connect.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.connect_move(false),
                MouseEventKind::ScrollDown => self.connect_move(true),
                _ => {}
            }
            return;
        }
        if self.show_help {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.help_scroll = self.help_scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => self.help_scroll = self.help_scroll.saturating_add(3),
                _ => {}
            }
            return;
        }
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up(3),
            MouseEventKind::ScrollDown => self.scroll_down(3),
            MouseEventKind::Down(MouseButton::Left) => {
                if self.progress_row == Some(mouse.row) {
                    self.progress_collapsed = !self.progress_collapsed;
                } else {
                    self.toggle_output_at(mouse.row);
                }
            }
            _ => {}
        }
    }

    /// Expand or collapse the tool output drawn on a screen row.
    pub(crate) fn toggle_output_at(&mut self, row: u16) {
        let Some((_, call_id)) = self
            .output_rows
            .iter()
            .find(|(line, _)| *line == row)
            .cloned()
        else {
            return;
        };
        if !self.expanded.remove(&call_id) {
            self.expanded.insert(call_id);
        }
    }

    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.kind == KeyEventKind::Release {
            return Action::None;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                key.code,
                KeyCode::Char('c' | 'C' | 'q' | 'Q') | KeyCode::Null
            )
        {
            self.should_quit = true;
            return Action::Quit;
        }
        if self.approval.is_some() {
            return self.handle_approval_key(key);
        }
        if self.connect.is_some() {
            return self.handle_connect_key(key);
        }
        if key.code == KeyCode::F(1) {
            self.show_help = !self.show_help;
            self.help_scroll = 0;
            return Action::None;
        }
        if self.show_help {
            match key.code {
                KeyCode::Esc | KeyCode::F(1) => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Up | KeyCode::Left => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Right => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::Home => self.help_scroll = 0,
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(10),
                KeyCode::PageDown => self.help_scroll = self.help_scroll.saturating_add(10),
                _ => {}
            }
            return Action::None;
        }
        if self.model_picker.is_some() {
            return self.handle_model_picker_key(key);
        }
        if self.session_picker.is_some() {
            return self.handle_session_picker_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('n') => self.new_session(),
                KeyCode::Char('u') => {
                    self.input.clear();
                    self.reset_palette();
                }
                KeyCode::Char('w') => {
                    self.input.delete_word();
                    self.reset_palette();
                }
                KeyCode::Char('a') => {
                    self.input.clear();
                    self.reset_palette();
                }
                KeyCode::Char('e') => self.input.cursor = self.input.value.len(),
                KeyCode::Char('v') => return Action::PasteImage,
                KeyCode::Char('p') if !self.session.tasks.is_empty() => {
                    self.progress_collapsed = !self.progress_collapsed;
                }
                _ => {}
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Char('v' | 'V')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && self.input.value.trim().is_empty() =>
            {
                self.toggle_diff_view();
                Action::None
            }
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.insert('\n');
                self.reset_palette();
                Action::None
            }
            KeyCode::Enter if self.palette_open() => self.palette_accept(),
            KeyCode::Enter => self.submit(),
            KeyCode::Esc if self.palette_open() => {
                self.palette_hidden = true;
                Action::None
            }
            KeyCode::Esc if self.is_streaming() => self.stop_streaming(),
            KeyCode::Esc => Action::None,
            KeyCode::Tab if self.palette_open() => {
                self.palette_complete();
                Action::None
            }
            KeyCode::Tab => {
                self.mode = self.mode.next();
                self.info(format!(
                    "Mode: {} — Tab again to cycle build · plan · chat",
                    self.mode.label()
                ));
                Action::None
            }
            KeyCode::Backspace => {
                self.input.backspace();
                self.reset_palette();
                Action::None
            }
            KeyCode::Delete => {
                self.input.delete();
                self.reset_palette();
                Action::None
            }
            KeyCode::Left => {
                self.input.left();
                Action::None
            }
            KeyCode::Right => {
                self.input.right();
                Action::None
            }
            KeyCode::Home => {
                self.input.cursor = 0;
                Action::None
            }
            KeyCode::End => {
                self.input.cursor = self.input.value.len();
                Action::None
            }
            KeyCode::Up if self.palette_open() => {
                self.palette_move(false);
                Action::None
            }
            KeyCode::Down if self.palette_open() => {
                self.palette_move(true);
                Action::None
            }
            KeyCode::Up if self.input.value.contains('\n') => {
                self.input.move_vertical(false);
                Action::None
            }
            KeyCode::Down if self.input.value.contains('\n') => {
                self.input.move_vertical(true);
                Action::None
            }
            KeyCode::Up => {
                self.scroll_up(1);
                Action::None
            }
            KeyCode::Down => {
                self.scroll_down(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.scroll_up(usize::from(self.transcript_height));
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll_down(usize::from(self.transcript_height));
                Action::None
            }
            KeyCode::Char(character)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.input.insert(character);
                self.reset_palette();
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(crate) fn handle_approval_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc => self.deny_pending_tool(),
            KeyCode::Enter => self.approve_pending_tool(false),
            KeyCode::Char('a' | 'A')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.approve_pending_tool(true)
            }
            KeyCode::Char('v' | 'V')
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.toggle_diff_view();
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = 0;
                }
                Action::None
            }
            KeyCode::Up => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = approval.scroll.saturating_sub(1);
                }
                Action::None
            }
            KeyCode::Down => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = approval.scroll.saturating_add(1);
                }
                Action::None
            }
            KeyCode::PageUp => {
                if let Some(approval) = self.approval.as_mut() {
                    let page = usize::from(approval.page_height.max(1));
                    approval.scroll = approval.scroll.saturating_sub(page);
                }
                Action::None
            }
            KeyCode::PageDown => {
                if let Some(approval) = self.approval.as_mut() {
                    let page = usize::from(approval.page_height.max(1));
                    approval.scroll = approval.scroll.saturating_add(page);
                }
                Action::None
            }
            KeyCode::Home => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = 0;
                }
                Action::None
            }
            KeyCode::End => {
                if let Some(approval) = self.approval.as_mut() {
                    approval.scroll = usize::MAX;
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    /// Switch every visible diff between unified and side-by-side.
    pub(crate) fn toggle_diff_view(&mut self) {
        self.diff_sides = !self.diff_sides;
        for report in self.changes.values_mut() {
            report.side_by_side = self.diff_sides;
        }
    }

    pub(crate) fn approve_pending_tool(&mut self, always: bool) -> Action {
        self.approval = None;
        let Some(call) = self.pending_tools.pop_front() else {
            return Action::None;
        };
        if always {
            self.approved_tools.insert(call.function.name.clone());
        }
        self.start_tool(call)
    }

    pub(crate) fn deny_pending_tool(&mut self) -> Action {
        self.approval = None;
        let Some(call) = self.pending_tools.pop_front() else {
            return Action::None;
        };
        self.record_tool_result(
            call.id,
            "The user denied this tool call. Do not retry it without asking.",
        );
        self.follow_tail = true;
        self.save_session();
        self.advance_tools()
    }

    pub(crate) fn start_tool(&mut self, call: ToolCall) -> Action {
        let preview = tools::change_preview(&call);
        self.running_tool = Some(RunningTool {
            existed: tools::target_existed(&call),
            before: preview.before,
            targets: tools::edit_targets(&call),
            call: call.clone(),
            started: Instant::now(),
        });
        self.follow_tail = true;
        Action::RunTool { call }
    }

    pub(crate) fn advance_tools(&mut self) -> Action {
        if self.is_streaming() || self.approval.is_some() || self.running_tool.is_some() {
            return Action::None;
        }
        let Some(call) = self.pending_tools.front().cloned() else {
            return self.continue_after_tools();
        };
        if self.mode.denied(&call.function.name) {
            self.pending_tools.pop_front();
            self.record_tool_result(
                call.id,
                format!(
                    "This tool call was blocked: {} mode only proposes work. Switch to build \
                     mode (Tab) to let changes run.",
                    self.mode.label()
                ),
            );
            self.follow_tail = true;
            self.save_session();
            return self.advance_tools();
        }
        if self.tools_mode.needs_approval(&call, &self.approved_tools) {
            let change = tools::change_preview(&call);
            self.approval = Some(ToolApproval {
                call,
                note: change.note,
                diffs: change.diffs,
                scroll: 0,
                page_height: 0,
            });
            self.follow_tail = true;
            return Action::None;
        }
        self.pending_tools.pop_front();
        self.start_tool(call)
    }

    pub(crate) fn continue_after_tools(&mut self) -> Action {
        if !self
            .session
            .messages
            .last()
            .is_some_and(|message| message.role == Role::Tool)
        {
            return Action::None;
        }
        if self.tool_rounds >= MAX_TOOL_ROUNDS {
            self.error(format!(
                "Stopped after {MAX_TOOL_ROUNDS} tool rounds. Send another message to continue."
            ));
            self.save_session();
            return Action::None;
        }
        self.begin_turn()
    }
}
