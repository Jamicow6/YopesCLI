use super::markdown::markdown_lines;
use super::*;

pub(super) fn draw_transcript(frame: &mut Frame, app: &mut App, area: Rect) {
    app.output_rows.clear();
    let inner = Block::default()
        .padding(Padding::new(2, 2, 1, 1))
        .inner(area);
    app.transcript_height = inner.height;
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.session.messages.is_empty() {
        // The welcome names the model; the working view carries no branding.
        let model = app.backend.model();
        let heading = if model.is_empty() {
            app.backend.provider_label().to_owned()
        } else {
            model.to_owned()
        };
        let mut welcome = vec![
            Line::styled(
                heading,
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
        ];
        // The mascot belongs to the startup screen only, never to a reply.
        welcome.extend(mascot_frame(app.spinner));
        welcome.push(Line::raw(""));
        welcome.push(Line::styled(
            "A quiet place to think out loud.",
            Style::default().fg(TEXT),
        ));
        welcome.push(Line::raw(""));
        if app.backend.is_configured() {
            welcome.push(Line::styled(
                "Type a message to begin.",
                Style::default().fg(MUTED),
            ));
        } else {
            welcome.push(Line::styled(
                "No provider connected.",
                Style::default().fg(MUTED),
            ));
            welcome.push(Line::styled(
                "Run /connect to add a provider or a custom endpoint.",
                Style::default().fg(MUTED),
            ));
        }
        welcome.push(Line::styled(
            "Type / to see commands.",
            Style::default().fg(MUTED),
        ));
        frame.render_widget(Paragraph::new(welcome).alignment(Alignment::Center), inner);
        return;
    }

    // Rows for newly seen images are rendered up front: the message loop
    // borrows the session immutably, so it can only read the finished cache.
    app.warm_image_cache(inner.width.clamp(4, 48));

    let mut lines = Vec::new();
    let mut rows: Vec<(u16, String)> = Vec::new();
    // Copied out: the summary is owned so the session can stay borrowed.
    let compaction = app.compaction_text().map(str::to_owned);
    for (index, message) in app.session.messages.iter().enumerate() {
        let call_name = (message.role == Role::Tool)
            .then(|| tool_call_name(app, index))
            .flatten();
        // A hidden tool leaves no trace in the transcript; its effect shows in
        // the Progress panel instead.
        if message.role == Role::Tool && call_name.is_some_and(is_hidden_tool) {
            continue;
        }
        let has_calls = message.tool_calls.as_ref().is_some_and(|calls| {
            calls
                .iter()
                .any(|call| !is_hidden_tool(&call.function.name))
        });
        let streaming = index + 1 == app.session.messages.len() && app.is_streaming();
        // A reply that only produced reasoning is still shown: dropping it
        // would hide long thinking that ended without a final answer.
        if message.role == Role::Assistant
            && message.content.is_empty()
            && message.reasoning.trim().is_empty()
            && !has_calls
            && !streaming
            && message.error.is_none()
        {
            continue;
        }
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        let (label, color) = match message.role {
            Role::User => ("you", MUTED),
            Role::Tool => ("tool", MUTED),
            // The label names the responding model, not the product.
            Role::Assistant => {
                let model = app.backend.model();
                (if model.is_empty() { "assistant" } else { model }, ACCENT)
            }
        };
        if message.role == Role::Tool {
            let start = lines.len();
            let name = call_name.map(str::to_owned);
            let framed =
                tool_result_lines(app, message, inner.width, &mut rows, start, name.as_deref());
            lines.extend(framed);
            continue;
        }
        let show_label = if message.role == Role::Assistant {
            let is_first_response = index == 0
                || app.session.messages[..index]
                    .iter()
                    .rev()
                    .find(|m| m.role != Role::Tool)
                    .is_some_and(|m| m.role == Role::User);
            let is_end_stream = index + 1 == app.session.messages.len();
            is_first_response || is_end_stream
        } else {
            true
        };
        if show_label {
            lines.push(Line::styled(
                label,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ));
        }
        // A failed request shows its provider message here, where the answer
        // would have been, instead of as a footer notice.
        if let Some(error) = &message.error {
            lines.push(Line::styled(
                format!("✗ {error}"),
                Style::default().fg(ERROR),
            ));
            continue;
        }
        // Thinking: shown in full while streaming or expanded, otherwise a
        // one-line summary that a click expands like a tool result.
        let reason_key = format!("reason-{index}");
        let think_count = message
            .reasoning
            .split('\n')
            .filter(|line| !line.trim().is_empty())
            .count();
        let reasoning = reasoning_lines(message, streaming, app.expanded.contains(&reason_key));
        if !reasoning.is_empty() {
            let start = lines.len();
            if think_count > 2 && !streaming {
                rows.push((start as u16, reason_key));
            }
            lines.extend(reasoning);
        }
        if message.content.is_empty() && streaming && !has_calls && message.reasoning.is_empty() {
            lines.push(Line::styled(
                caret(app.spinner),
                Style::default().fg(ACCENT),
            ));
            continue;
        }
        if !message.content.is_empty() {
            if !streaming {
                if let Some((cached_width, cached_len, cached_lines)) =
                    app.markdown_cache.get(&index)
                {
                    if *cached_width == inner.width && *cached_len == message.content.len() {
                        lines.extend(cached_lines.clone());
                    } else {
                        let parsed = markdown_lines(&message.content, inner.width);
                        app.markdown_cache
                            .insert(index, (inner.width, message.content.len(), parsed.clone()));
                        lines.extend(parsed);
                    }
                } else {
                    let parsed = markdown_lines(&message.content, inner.width);
                    app.markdown_cache
                        .insert(index, (inner.width, message.content.len(), parsed.clone()));
                    lines.extend(parsed);
                }
            } else {
                lines.extend(markdown_lines(&message.content, inner.width));
            }
        }
        // Pasted screenshots show up as pixel art directly under the text.
        for image in &message.images {
            match app.cached_image(&image.name) {
                Some(rows) => lines.extend(rows.iter().cloned()),
                None => lines.push(Line::styled(
                    format!(" {}", image.name),
                    Style::default().fg(MUTED),
                )),
            }
        }
        // Stats belong under a finished answer, not under a tool-call turn.
        if let Some(usage) = message.usage {
            if (!message.content.is_empty() || !message.reasoning.is_empty()) && !has_calls {
                lines.push(usage_line(usage));
            }
        }
        if let Some(calls) = &message.tool_calls {
            for call in calls
                .iter()
                .filter(|call| !is_hidden_tool(&call.function.name))
            {
                lines.push(Line::from(vec![
                    Span::styled("→ ", Style::default().fg(ACCENT)),
                    Span::styled(
                        call.function.name.clone(),
                        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {}", tools::preview(call)),
                        Style::default().fg(MUTED),
                    ),
                ]));
            }
        }
        if streaming {
            if let Some(line) = lines.last_mut() {
                let line_len = spans_width(&line.spans);
                if line_len + 2 < usize::from(inner.width) {
                    line.spans.push(Span::styled(
                        format!(" {}", caret(app.spinner)),
                        Style::default().fg(ACCENT),
                    ));
                } else {
                    lines.push(Line::styled(
                        caret(app.spinner),
                        Style::default().fg(ACCENT),
                    ));
                }
            }
        }
    }

    // Compaction streams into its own buffer: show it live so a long summary
    // does not look like a frozen screen.
    if let Some(summary) = compaction {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            "compacting context",
            Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
        ));
        if summary.trim().is_empty() {
            lines.push(Line::styled(
                caret(app.spinner),
                Style::default().fg(ACCENT),
            ));
        } else {
            lines.extend(markdown_lines(&summary, inner.width));
        }
    }

    // Screen row of every transcript line, accounting for wrapped text above.
    let mut starts: Vec<usize> = Vec::with_capacity(lines.len());
    let mut next_row = 0;
    let mut scratch = String::new();
    for line in &lines {
        starts.push(next_row);
        scratch.clear();
        for span in &line.spans {
            scratch.push_str(&span.content);
        }
        next_row += wrapped_line_count(&scratch, inner.width);
    }
    let total_lines = next_row;
    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    let maximum = total_lines.saturating_sub(usize::from(inner.height));
    if app.follow_tail {
        app.scroll = maximum;
    } else {
        app.scroll = app.scroll.min(maximum);
        if app.scroll == maximum {
            app.follow_tail = true;
        }
    }
    frame.render_widget(
        paragraph.scroll((app.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );

    // Map tool frames to screen rows so a click can hit one.
    let scroll = app.scroll.min(u16::MAX as usize);
    for (line_index, call_id) in rows {
        let Some(&screen_row) = starts.get(usize::from(line_index)) else {
            continue;
        };
        if screen_row < scroll {
            continue;
        }
        let offset = (screen_row - scroll) as u16;
        if offset >= inner.height {
            continue;
        }
        app.output_rows
            .push((inner.y.saturating_add(offset), call_id));
    }
}
