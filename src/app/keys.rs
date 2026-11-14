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
                ConnectStage::Endpoint => connect.custom_endpoint.push_str(&clean),
                ConnectStage::ModelsEndpoint => connect.custom_models_url.push_str(&clean),
                ConnectStage::CustomToken => connect.custom_token.push_str(&clean),
                ConnectStage::ModelId => connect.custom_model_id.push_str(&clean),
                ConnectStage::ModelName => connect.custom_model_name.push_str(&clean),
                ConnectStage::ModelContextWindow => {
                    connect.custom_model_context_window.push_str(&clean);
                    connect.select_custom_context();
                }
                ConnectStage::Providers | ConnectStage::Models => {
                    connect.filter.push_str(&clean);
                    connect.selected = 0;
                }
                ConnectStage::RenameProvider => connect.custom_name.push_str(&clean),
                ConnectStage::AddProvider => {
                    if let Some(target) = connect.active_add_provider_field_mut() {
                        target.push_str(&clean);
                    }
                }
                ConnectStage::CustomModels => {}
            }
            return;
        }
        if !self.show_help {
            let trimmed = value.trim().trim_matches('"').trim_matches('\'');
            if (trimmed.ends_with(".png")
                || trimmed.ends_with(".jpg")
                || trimmed.ends_with(".jpeg")
                || trimmed.ends_with(".webp")
                || trimmed.ends_with(".gif")
                || trimmed.ends_with(".bmp"))
                && std::path::Path::new(trimmed).is_file()
            {
                if let Ok(bytes) = std::fs::read(trimmed) {
                    if let Ok(img) = image::load_from_memory(&bytes) {
                        let mut png_bytes = std::io::Cursor::new(Vec::new());
                        if img
                            .write_to(&mut png_bytes, image::ImageFormat::Png)
                            .is_ok()
                        {
                            use base64::Engine;
                            let b64 = base64::engine::general_purpose::STANDARD
                                .encode(png_bytes.into_inner());
                            let name = format!("paste-{}.png", self.next_image);
                            self.next_image += 1;
                            self.attachments.push(ImageAttachment { name, data: b64 });
                            self.info(format!(
                                "Image {} attached — send it with the next message.",
                                self.attachments.len()
                            ));
                            return;
                        }
                    }
                }
            }
            self.input.insert_str(value);
            self.reset_palette();
        }
    }

    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> Action {
        if let Some(approval) = self.approval.as_mut() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    approval.scroll = approval.scroll.saturating_sub(3);
                }
                MouseEventKind::ScrollDown => {
                    approval.scroll = approval.scroll.saturating_add(3);
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let popup = approval.popup_area.get();
                    if popup.width > 0 && popup.height > 0 {
                        if mouse.row == popup.y.saturating_add(popup.height).saturating_sub(1) {
                            let rel_x = mouse.column.saturating_sub(popup.x);
                            if rel_x < 13 {
                                return self.approve_pending_tool(false);
                            } else if rel_x < 25 {
                                return self.approve_pending_tool(true);
                            } else if rel_x < 43 {
                                self.tools_mode = ToolsMode::Auto;
                                self.info(
                                    "Permission: yolo — all commands and edits run automatically",
                                );
                                return self.approve_pending_tool(true);
                            } else {
                                return self.deny_pending_tool();
                            }
                        } else if mouse.column >= popup.x
                            && mouse.column < popup.x + popup.width
                            && mouse.row >= popup.y
                            && mouse.row < popup.y + popup.height
                        {
                            return self.approve_pending_tool(false);
                        } else {
                            return self.deny_pending_tool();
                        }
                    }
                    return self.approve_pending_tool(false);
                }
                _ => {}
            }
            return Action::None;
        }
        if let Some(connect) = self.connect.as_mut() {
            if connect.stage == ConnectStage::AddProvider {
                if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
                    if let Some(field) = connect.field_at_mouse(mouse.column, mouse.row) {
                        connect.add_provider_field = field;
                        if field == AddProviderField::Done {
                            return self.submit_add_provider();
                        }
                    }
                }
                return Action::None;
            }
            match mouse.kind {
                MouseEventKind::ScrollUp => self.connect_move(false),
                MouseEventKind::ScrollDown => self.connect_move(true),
                MouseEventKind::Down(MouseButton::Left) => {
                    if let Some(index) = connect.item_at_mouse(mouse.column, mouse.row) {
                        if connect.stage == ConnectStage::ModelContextWindow {
                            connect.context_row = index;
                        } else {
                            connect.selected = index;
                        }
                        return self.select_connect_item();
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        if self.model_picker.is_some() {
            let count = self.model_picker_count();
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(picker) = self.model_picker.as_mut() {
                        picker.selected = picker.selected.saturating_sub(1);
                    }
                    return Action::None;
                }
                MouseEventKind::ScrollDown => {
                    if let Some(picker) = self.model_picker.as_mut() {
                        picker.selected = (picker.selected + 1).min(count.saturating_sub(1));
                    }
                    return Action::None;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let selected = self
                        .model_picker
                        .as_ref()
                        .map(|p| p.selected)
                        .unwrap_or_default();
                    let matches = self.model_matches();
                    let picked = matches
                        .get(selected.min(count.saturating_sub(1)))
                        .map(|model| (*model).clone());
                    self.model_picker = None;
                    if let Some(model) = picked {
                        self.pick_model(model);
                    }
                    return Action::None;
                }
                _ => {}
            }
            return Action::None;
        }
        if self.session_picker.is_some() {
            let count = self.session_picker_count();
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    if let Some(picker) = self.session_picker.as_mut() {
                        picker.selected = picker.selected.saturating_sub(1);
                    }
                    return Action::None;
                }
                MouseEventKind::ScrollDown => {
                    if let Some(picker) = self.session_picker.as_mut() {
                        picker.selected = (picker.selected + 1).min(count.saturating_sub(1));
                    }
                    return Action::None;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    let selected = self
                        .session_picker
                        .as_ref()
                        .map(|p| p.selected)
                        .unwrap_or_default();
                    let sessions = self.session_matches();
                    let picked = sessions
                        .get(selected.min(count.saturating_sub(1)))
                        .map(|session| session.path.clone());
                    self.session_picker = None;
                    if let Some(path) = picked {
                        self.switch_session(path);
                    }
                    return Action::None;
                }
                _ => {}
            }
            return Action::None;
        }
        if self.palette_open() {
            match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.palette_move(false);
                    return Action::None;
                }
                MouseEventKind::ScrollDown => {
                    self.palette_move(true);
                    return Action::None;
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    return self.palette_accept();
                }
                _ => {}
            }
            return Action::None;
        }
        if self.show_help {
            match mouse.kind {
                MouseEventKind::ScrollUp => self.help_scroll = self.help_scroll.saturating_sub(3),
                MouseEventKind::ScrollDown => self.help_scroll = self.help_scroll.saturating_add(3),
                _ => {}
            }
            return Action::None;
        }
        if let Some(target) = self.composer_area {
            let inside = mouse.column >= target.x
                && mouse.column < target.x + target.width
                && mouse.row >= target.y
                && mouse.row < target.y + target.height;

            if inside && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
                let rel_x = (mouse.column - target.x) as usize;
                let rel_y = (mouse.row - target.y) as usize + self.input_scroll;
                let pos = self
                    .input
                    .pos_at_coords(rel_x, rel_y, target.width as usize);
                self.input.selection_anchor = Some(pos);
                self.input.cursor = pos;
                return Action::None;
            }

            if matches!(mouse.kind, MouseEventKind::Drag(MouseButton::Left))
                && (inside || self.input.selection_anchor.is_some())
            {
                let clamped_col = mouse
                    .column
                    .clamp(target.x, target.x + target.width.saturating_sub(1));
                let rel_x = (clamped_col - target.x) as usize;
                let rel_y = if mouse.row < target.y {
                    self.input_scroll
                } else {
                    let r = (mouse.row - target.y).min(target.height.saturating_sub(1));
                    r as usize + self.input_scroll
                };
                let pos = self
                    .input
                    .pos_at_coords(rel_x, rel_y, target.width as usize);
                if self.input.selection_anchor.is_none() {
                    self.input.selection_anchor = Some(self.input.cursor);
                }
                self.input.cursor = pos;
                return Action::None;
            }
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
        Action::None
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
            && matches!(key.code, KeyCode::Char('q' | 'Q'))
        {
            self.should_quit = true;
            return Action::Quit;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C') | KeyCode::Null)
        {
            if let Some(selected) = self.input.selected_text() {
                if !selected.is_empty() {
                    let text = selected.to_owned();
                    self.info("Copied selection to clipboard");
                    return Action::CopyClipboard(text);
                }
            }
            if !self.input.value.is_empty() {
                let text = self.input.value.clone();
                self.info("Copied input to clipboard");
                return Action::CopyClipboard(text);
            }
            if self.is_streaming() {
                return self.stop_streaming();
            }
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
                KeyCode::Char('a' | 'A') => {
                    self.input.select_all();
                    self.reset_palette();
                }
                KeyCode::Char('x' | 'X') => {
                    if let Some(selected) = self.input.selected_text() {
                        let text = selected.to_owned();
                        self.input.delete_selection();
                        self.reset_palette();
                        self.info("Cut selection to clipboard");
                        return Action::CopyClipboard(text);
                    }
                }
                KeyCode::Char('e') => self.input.cursor = self.input.value.len(),
                KeyCode::Char('v' | 'V') => return Action::PasteImage,
                KeyCode::Char('p' | 'P') => {
                    if self.palette_open() {
                        self.palette_move(true);
                    } else {
                        self.input.set("/");
                        self.reset_palette();
                    }
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
            KeyCode::Esc if self.input.selection().is_some() => {
                self.input.clear_selection();
                Action::None
            }
            KeyCode::Esc if self.palette_open() => {
                self.palette_hidden = true;
                Action::None
            }
            KeyCode::Esc if self.is_streaming() => self.stop_streaming(),
            KeyCode::Esc => Action::None,
            KeyCode::BackTab if self.palette_open() => {
                self.palette_move(false);
                Action::None
            }
            KeyCode::Tab if self.palette_open() && key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.palette_move(false);
                Action::None
            }
            KeyCode::BackTab => {
                self.cycle_tools_mode();
                Action::None
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.cycle_tools_mode();
                Action::None
            }
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
                self.input.left(key.modifiers.contains(KeyModifiers::SHIFT));
                Action::None
            }
            KeyCode::Right => {
                self.input
                    .right(key.modifiers.contains(KeyModifiers::SHIFT));
                Action::None
            }
            KeyCode::Home => {
                self.input.home(key.modifiers.contains(KeyModifiers::SHIFT));
                Action::None
            }
            KeyCode::End => {
                self.input.end(key.modifiers.contains(KeyModifiers::SHIFT));
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
                self.input
                    .move_vertical(false, key.modifiers.contains(KeyModifiers::SHIFT));
                Action::None
            }
            KeyCode::Down if self.input.value.contains('\n') => {
                self.input
                    .move_vertical(true, key.modifiers.contains(KeyModifiers::SHIFT));
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
            KeyCode::BackTab => {
                self.tools_mode = ToolsMode::Auto;
                self.info("Permission: yolo — all commands and edits run automatically");
                self.approve_pending_tool(true)
            }
            KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.tools_mode = ToolsMode::Auto;
                self.info("Permission: yolo — all commands and edits run automatically");
                self.approve_pending_tool(true)
            }
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

    pub(crate) fn cycle_tools_mode(&mut self) {
        self.tools_mode = self.tools_mode.next_permission();
        self.info(format!(
            "Permission: {} — Shift+Tab again to cycle on · yolo · ask",
            self.tools_mode.label()
        ));
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
                popup_area: std::cell::Cell::new(ratatui::layout::Rect::default()),
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
        self.begin_turn()
    }
}
