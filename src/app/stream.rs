use super::*;

impl App {
    pub(crate) fn on_stream_event(&mut self, event: StreamEvent) -> Action {
        match event {
            StreamEvent::Delta { request_id, text }
                if self.active_request_id() == Some(request_id) =>
            {
                if let Some(compaction) = self
                    .active
                    .as_mut()
                    .and_then(|active| active.compacting.as_mut())
                {
                    compaction.text.push_str(&text);
                } else if let Some(message) = self.session.messages.last_mut() {
                    message.content.push_str(&text);
                }
                Action::None
            }
            StreamEvent::Reasoning { request_id, text }
                if self.active_request_id() == Some(request_id) =>
            {
                // A compaction summary is content only; reasoning is ignored.
                if self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.compacting.is_none())
                {
                    if let Some(message) = self.session.messages.last_mut() {
                        message.reasoning.push_str(&text);
                    }
                }
                Action::None
            }
            StreamEvent::ToolProgress {
                request_id,
                name,
                detail,
                bytes,
            } if self.active_request_id() == Some(request_id) => {
                if let Some(active) = self.active.as_mut() {
                    active.tool_progress = Some((name, detail, bytes));
                }
                Action::None
            }
            StreamEvent::Usage {
                request_id,
                prompt_tokens,
                completion_tokens,
            } if self.active_request_id() == Some(request_id) => {
                if let Some(active) = self.active.as_mut() {
                    let usage = active.usage.get_or_insert((0, 0));
                    usage.0 = usage.0.max(prompt_tokens);
                    usage.1 = usage.1.max(completion_tokens);
                }
                Action::None
            }
            StreamEvent::ToolCalls { request_id, calls }
                if self.active_request_id() == Some(request_id)
                    && self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.compacting.is_none()) =>
            {
                if let Some(message) = self.session.messages.last_mut() {
                    if message.role == Role::Assistant {
                        message.tool_calls = Some(calls.clone());
                    }
                }
                self.tool_rounds = self.tool_rounds.saturating_add(1);
                if self.tool_rounds == MAX_TOOL_ROUNDS - TOOL_ROUND_HEADS_UP {
                    self.info(format!(
                        "{TOOL_ROUND_HEADS_UP} tool rounds left this turn; the turn stops \
                         at {MAX_TOOL_ROUNDS} and one more message restarts the count."
                    ));
                }
                self.pending_tools.extend(calls);
                self.follow_tail = true;
                self.save_session();
                Action::None
            }
            StreamEvent::Retrying {
                request_id,
                attempt,
                delay_secs,
                reason,
            } if self.active_request_id() == Some(request_id) => {
                // The wait is shown in the footer, so a slow provider does not
                // look like a hang.
                self.retry = Some(RetryNotice {
                    attempt,
                    delay_secs,
                    reason,
                });
                Action::None
            }
            StreamEvent::Done { request_id } if self.active_request_id() == Some(request_id) => {
                self.retry = None;
                // Fold this request's billed tokens into the session total.
                if let Some((prompt, completion)) = self.active.as_ref().and_then(|a| a.usage) {
                    self.spent_prompt_tokens = self.spent_prompt_tokens.saturating_add(prompt);
                    self.spent_completion_tokens =
                        self.spent_completion_tokens.saturating_add(completion);
                }
                // A finished compaction folds the summary in, then the queued
                // turn starts against the now-small history.
                let compacted = self
                    .active
                    .as_ref()
                    .and_then(|active| active.compacting.as_ref())
                    .map(|compaction| (compaction.split, compaction.text.clone()));
                if let Some((split, summary)) = compacted {
                    self.active = None;
                    if summary.trim().is_empty() {
                        self.info("Compaction returned nothing; sending the full history.");
                    } else {
                        self.session.compact(split, summary);
                    }
                    self.save_session();
                    return self.begin_turn();
                }
                // Record what the reply cost and how long it took, so the
                // transcript can show it under the finished message.
                let finished = self.active.as_ref().map(|active| {
                    let elapsed_ms =
                        u64::try_from(active.started.elapsed().as_millis()).unwrap_or(0);
                    let (prompt_tokens, completion_tokens) = active
                        .usage
                        .unwrap_or((active.prompt_tokens, estimated_output_tokens(&self.session)));
                    (prompt_tokens, completion_tokens, elapsed_ms)
                });
                if let Some((prompt_tokens, completion_tokens, elapsed_ms)) = finished {
                    if let Some(message) = self.session.messages.last_mut() {
                        if message.role == Role::Assistant {
                            message.usage = Some(session::Usage {
                                prompt_tokens,
                                completion_tokens,
                                elapsed_ms,
                            });
                        }
                    }
                }
                self.active = None;
                if self.pending_tools.is_empty() {
                    self.tool_rounds = 0;
                    // A provider can end the stream without sending anything;
                    // the placeholder is dropped, but the user is told why no
                    // reply appeared instead of seeing nothing.
                    if self.remove_empty_response() {
                        self.info(
                            "The model returned no content for this turn; try sending again.",
                        );
                    }
                    self.save_session();
                    return self.maybe_name_session();
                }
                self.save_session();
                self.advance_tools()
            }
            StreamEvent::Error {
                request_id,
                message,
            } if self.active_request_id() == Some(request_id) => {
                let compacting = self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.compacting.is_some());
                self.active = None;
                self.retry = None;
                self.active = None;
                if compacting {
                    // The queued turn never started, so continue without the
                    // summary instead of failing it too.
                    self.info("Compaction failed; sending the full history.");
                    self.save_session();
                    let messages = self.session.messages.clone();
                    return self.begin_request(messages, None);
                }
                self.discard_pending_tools("The response failed before this tool call could run.");
                self.tool_rounds = 0;
                self.remove_empty_response();
                // Providers fail in the reply block, not the footer: the error
                // belongs where the answer would have been.
                self.session.messages.push(Message::provider_error(message));
                self.follow_tail = true;
                self.save_session();
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(crate) fn on_tool_result(&mut self, event: ToolEvent) -> Action {
        let Some(running) = self
            .running_tool
            .take()
            .filter(|running| running.call.id == event.call.id)
        else {
            return Action::None;
        };
        let before = running.before;
        let existed = running.existed;
        let content = match event.result {
            Ok(output) => {
                if event.call.function.name == "todo_write" {
                    if let Ok(tasks) = serde_json::from_str::<Vec<TaskState>>(&output) {
                        let done = tasks.iter().filter(|task| task.done).count();
                        let count = tasks.len();
                        self.session.tasks = tasks;
                        // An empty write deliberately clears the checklist.
                        let message = if count == 0 {
                            "Cleared the task list; the Progress panel is closed.".to_owned()
                        } else {
                            format!(
                                "Stored {count} task{} ({done} done); the Progress panel shows them.",
                                if count == 1 { "" } else { "s" }
                            )
                        };
                        self.record_tool_result(event.call.id, message);
                        self.follow_tail = true;
                        self.save_session();
                        return self.advance_tools();
                    }
                }
                let diffs =
                    tools::applied_change_all(&event.call, before, existed, &running.targets);
                if !diffs.is_empty() {
                    self.changes.insert(
                        event.call.id.clone(),
                        ChangeReport {
                            diffs,
                            side_by_side: self.diff_sides,
                        },
                    );
                }
                output
            }
            Err(error) => format!("Error: {error}"),
        };
        let images = if event.call.function.name == "read_image" {
            images::tool_image_attachment(&event.call)
                .into_iter()
                .collect()
        } else {
            Vec::new()
        };
        self.record_tool_result_with_images(event.call.id, content, images);
        self.save_session();
        self.advance_tools()
    }

    pub(crate) fn record_tool_result(
        &mut self,
        call_id: impl Into<String>,
        content: impl Into<String>,
    ) {
        self.record_tool_result_with_images(call_id, content, Vec::new());
    }

    /// Store a tool result, optionally with images the model should see.
    pub(crate) fn record_tool_result_with_images(
        &mut self,
        call_id: impl Into<String>,
        content: impl Into<String>,
        images: Vec<ImageAttachment>,
    ) {
        let mut message = Message::tool_result(call_id, content);
        message.images = images;
        self.session.messages.push(message);
    }

    pub(crate) fn discard_pending_tools(&mut self, reason: &str) {
        while let Some(call) = self.pending_tools.pop_front() {
            self.record_tool_result(call.id, reason);
        }
        self.approval = None;
    }

    /// Drop an empty assistant placeholder; `true` when one was dropped.
    ///
    /// A reply with only reasoning is kept: long thinking is real output the
    /// user asked for, so dropping it loses the whole answer.
    pub(crate) fn remove_empty_response(&mut self) -> bool {
        if self.session.messages.last().is_some_and(|message| {
            message.role == Role::Assistant
                && message.content.is_empty()
                && message.reasoning.trim().is_empty()
                && message.tool_calls.is_none()
                && message.error.is_none()
        }) {
            self.session.messages.pop();
            return true;
        }
        false
    }

    pub(crate) fn connect_error(&mut self, message: impl Into<String>) {
        if let Some(connect) = self.connect.as_mut() {
            connect.error = Some(message.into());
        }
    }

    pub(crate) fn info(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Info,
            message: message.into(),
        });
    }

    pub(crate) fn error(&mut self, message: impl Into<String>) {
        self.notice = Some(Notice {
            kind: NoticeKind::Error,
            message: message.into(),
        });
    }

    pub(crate) fn save_session(&mut self) {
        if let Some(error) = self.session_saver.take_error() {
            self.error(format!("Could not save session: {error}"));
        }
        let Some(path) = self.session_path.clone() else {
            return;
        };
        self.session_saver.save(path, self.session.clone());
    }

    /// Store the custom system prompt in the saved config.
    pub(crate) fn persist_system_prompt(&mut self, value: Option<String>) {
        if let Err(error) = config::save_system_prompt(&self.config_path, value) {
            self.error(format!("Could not save the system prompt: {error:#}"));
        }
    }

    /// Store the automatic compaction switch in the saved config.
    pub(crate) fn persist_auto_compact(&mut self, value: bool) {
        if let Err(error) = config::save_auto_compact(&self.config_path, value) {
            self.error(format!("Could not save the compact setting: {error:#}"));
        }
    }

    /// Store the reasoning effort in the saved config.
    pub(crate) fn persist_reasoning_effort(&mut self, value: Option<String>) {
        if let Err(error) = config::save_reasoning_effort(&self.config_path, value) {
            self.error(format!("Could not save the reasoning effort: {error:#}"));
        }
    }

    pub(crate) fn persist(&self) -> Result<()> {
        let Some(path) = self.session_path.clone() else {
            return Ok(());
        };
        if self
            .session_saver
            .save_and_wait(path.clone(), self.session.clone())
        {
            return Ok(());
        }
        self.session.save(&path)
    }
}
