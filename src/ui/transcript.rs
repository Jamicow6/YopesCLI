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
        // The header already says "yopes", so the welcome names the model
        // instead of repeating the product name.
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
        let ball = YARN_ROLL[(app.spinner / 2) % YARN_ROLL.len()];
        for art in CAT_BASE {
            welcome.push(Line::styled(art, Style::default().fg(ACCENT)));
        }
        welcome.push(Line::styled(ball, Style::default().fg(MUTED)));
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
    app.warm_image_cache(inner.width.min(48).max(4));

    let mut lines = Vec::new();
    let mut rows: Vec<(u16, String)> = Vec::new();
    for (index, message) in app.session.messages.iter().enumerate() {
        let has_calls = message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty());
        let streaming = index + 1 == app.session.messages.len() && app.is_streaming();
        // A reply that only produced reasoning is still shown: dropping it
        // would hide long thinking that ended without a final answer.
        if message.role == Role::Assistant
            && message.content.is_empty()
            && message.reasoning.trim().is_empty()
            && !has_calls
            && !streaming
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
                (if model.is_empty() { "yopes" } else { model }, ACCENT)
            }
        };
        if message.role == Role::Tool {
            let start = lines.len();
            let name = tool_call_name(app, index).map(str::to_owned);
            let framed =
                tool_result_lines(app, message, inner.width, &mut rows, start, name.as_deref());
            lines.extend(framed);
            if streaming {
                if let Some(line) = lines.last_mut() {
                    line.spans.push(Span::styled(
                        format!(" {}", caret(app.spinner)),
                        Style::default().fg(ACCENT),
                    ));
                }
            }
            continue;
        }
        lines.push(Line::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
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
            lines.extend(markdown_lines(&message.content));
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
            if !message.content.is_empty() && !has_calls {
                lines.push(usage_line(usage));
            }
        }
        if let Some(calls) = &message.tool_calls {
            for call in calls {
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
                line.spans.push(Span::styled(
                    format!(" {}", caret(app.spinner)),
                    Style::default().fg(ACCENT),
                ));
            }
        }
    }

    // Screen row of every transcript line, accounting for wrapped text above.
    let mut starts: Vec<usize> = Vec::with_capacity(lines.len());
    let mut next_row = 0;
    for line in &lines {
        starts.push(next_row);
        let content = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        next_row += wrapped_line_count(&content, inner.width);
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
