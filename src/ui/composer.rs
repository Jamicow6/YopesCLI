use super::*;

pub(super) fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let inner = padded(area, 2);
    let mut right = if app.backend.model().is_empty() {
        format!("{} · {}", app.backend.provider_label(), app.spent_cost())
    } else {
        format!(
            "{} · {} · {}",
            app.backend.provider_label(),
            app.backend.model(),
            app.spent_cost()
        )
    };
    if app.context_tokens() > 0 {
        right.push_str(&format!(" · ctx {}", format_tokens(app.context_tokens())));
    }
    let right_width = (UnicodeWidthStr::width(right.as_str()) as u16).min(inner.width / 2);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(4), Constraint::Length(right_width)])
        .split(inner);

    // No product branding in the working view: the mode dot and the session
    // title are the only things that need the space.
    let mut title_spans = vec![
        Span::styled(
            "●",
            Style::default()
                .fg(app.mode.color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}", app.mode.label()),
            Style::default()
                .fg(app.mode.color())
                .add_modifier(Modifier::BOLD),
        ),
    ];
    match app.tools_mode {
        ToolsMode::Auto => {
            title_spans.push(Span::styled(
                " [YOLO]",
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
            ));
        }
        ToolsMode::Ask => {
            title_spans.push(Span::styled(" [ASK]", Style::default().fg(MUTED)));
        }
        ToolsMode::Off => {
            title_spans.push(Span::styled(" [OFF]", Style::default().fg(MUTED)));
        }
        ToolsMode::On => {}
    }
    title_spans.push(Span::styled("  ", Style::default()));
    title_spans.push(Span::styled(
        app.session.title.trim(),
        Style::default().fg(MUTED),
    ));
    let title = Line::from(title_spans);
    frame.render_widget(Paragraph::new(title), columns[0]);

    if right_width > 0 {
        frame.render_widget(
            Paragraph::new(right)
                .alignment(Alignment::Right)
                .style(Style::default().fg(MUTED)),
            columns[1],
        );
    }
}

pub(super) fn draw_separator(frame: &mut Frame, area: Rect) {
    let inner = padded(area, 2);
    if inner.width == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new("─".repeat(usize::from(inner.width))).style(Style::default().fg(MUTED)),
        inner,
    );
}

pub(super) fn composer_height(app: &App, width: u16) -> u16 {
    let content_width = width.saturating_sub(6).max(1);
    let lines = wrapped_line_count(&app.input.with_cursor(), content_width);
    (lines as u16).clamp(1, 6)
}

pub(super) fn draw_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);
    // The prompt flashes briefly after a send: input registered, turn running.
    let pulse = app
        .send_pulse
        .is_some_and(|mark| app.spinner.saturating_sub(mark) < 5);
    let (prompt_marker, prompt_style) = if app.tools_mode == ToolsMode::Auto {
        ("⚡", Style::default().fg(YELLOW).add_modifier(Modifier::BOLD))
    } else if pulse {
        ("›", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
    } else if app.is_busy() {
        ("›", Style::default().fg(MUTED))
    } else {
        ("›", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD))
    };
    frame.render_widget(Paragraph::new(prompt_marker).style(prompt_style), columns[0]);

    let target = columns[1];
    app.composer_area = Some(target);
    if target.width == 0 || target.height == 0 {
        return;
    }
    let before = wrapped_line_count(app.input.before_cursor(), target.width).saturating_sub(1);
    let height = usize::from(target.height);
    if before < app.input_scroll {
        app.input_scroll = before;
    } else if before >= app.input_scroll.saturating_add(height) {
        app.input_scroll = before.saturating_add(1).saturating_sub(height);
    }
    frame.render_widget(
        Paragraph::new(render_composer_text(&app.input))
            .wrap(Wrap { trim: false })
            .scroll((app.input_scroll.min(u16::MAX as usize) as u16, 0)),
        target,
    );
}

fn render_composer_text(input: &Input) -> Text<'static> {
    let sel = input.selection();
    let sel_style = Style::default().add_modifier(Modifier::REVERSED);
    let norm_style = Style::default().fg(TEXT);
    let cursor_style = Style::default().fg(ACCENT);

    let mut lines = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::new();
    let mut current_chunk = String::new();
    let mut current_style = norm_style;

    let flush_chunk = |spans: &mut Vec<Span<'static>>, chunk: &mut String, style: Style| {
        if !chunk.is_empty() {
            spans.push(Span::styled(std::mem::take(chunk), style));
        }
    };

    let total_len = input.value.len();
    let mut idx = 0;
    let mut chars = input.value.char_indices().peekable();

    while idx <= total_len {
        if idx == input.cursor {
            flush_chunk(&mut current_spans, &mut current_chunk, current_style);
            current_spans.push(Span::styled("▌", cursor_style));
        }

        if idx == total_len {
            break;
        }

        let (_, ch) = chars.next().unwrap();
        let next_idx = idx + ch.len_utf8();

        let is_selected = if let Some(ref r) = sel {
            idx >= r.start && idx < r.end
        } else {
            false
        };

        let target_style = if is_selected { sel_style } else { norm_style };
        if target_style != current_style {
            flush_chunk(&mut current_spans, &mut current_chunk, current_style);
            current_style = target_style;
        }

        if ch == '\n' {
            flush_chunk(&mut current_spans, &mut current_chunk, current_style);
            lines.push(Line::from(std::mem::take(&mut current_spans)));
        } else {
            current_chunk.push(ch);
        }

        idx = next_idx;
    }

    flush_chunk(&mut current_spans, &mut current_chunk, current_style);
    lines.push(Line::from(current_spans));

    Text::from(lines)
}

/// The native progress panel: the session's task checklist above the composer.
pub(super) fn draw_progress(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width < 20 {
        return;
    }
    app.progress_row = Some(area.y);
    let inner = padded(area, 2);
    let done = app.session.tasks.iter().filter(|task| task.done).count();
    let marker = if app.progress_collapsed { "▸" } else { "▾" };
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{marker} "), Style::default().fg(MUTED)),
        Span::styled(
            format!("Progress {done}/{}", app.session.tasks.len()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · todo_write · click to fold", Style::default().fg(MUTED)),
    ])];
    if !app.progress_collapsed {
        if area.height < 3 {
            return;
        }
        for (index, task) in app.session.tasks.iter().enumerate() {
            if index == 6 {
                lines.push(Line::styled(
                    format!("… {} more", app.session.tasks.len() - 6),
                    Style::default().fg(MUTED),
                ));
                break;
            }
            let mark = if task.done { "[x]" } else { "[ ]" };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{mark} "),
                    Style::default().fg(if task.done { MUTED } else { ACCENT }),
                ),
                Span::styled(task.text.clone(), Style::default().fg(TEXT)),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

pub(super) fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let inner = padded(area, 2);
    let count = app.session.messages.len();
    let right = if count == 1 {
        "1 message".to_owned()
    } else {
        format!("{count} messages")
    };
    let right_width =
        if inner.width >= 60 && count > 0 && app.notice.is_none() && !app.is_streaming() {
            (right.chars().count() as u16).min(inner.width / 3)
        } else {
            0
        };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(8), Constraint::Length(right_width)])
        .split(inner);

    let (text, color) = if app.approval.is_some() {
        (
            "approval needed · enter run · a always allow · shift+tab yolo · esc deny · v diff"
                .to_owned(),
            ACCENT,
        )
    } else if let Some(retry) = &app.retry {
        // A retry in flight: the wait is shown so a slow provider does not
        // look like a hang.
        (
            format!(
                "retrying in {}s · attempt {} · {}",
                retry.delay_secs, retry.attempt, retry.reason
            ),
            ERROR,
        )
    } else if let Some(running) = &app.running_tool {
        let elapsed = app.running_tool_elapsed().unwrap_or_default();
        (
            format!(
                "{} running {} · {elapsed:.1}s",
                SPINNER[app.spinner % SPINNER.len()],
                running.call.function.name
            ),
            ACCENT,
        )
    } else if let Some(elapsed) = app.elapsed_seconds() {
        // Compaction is a request of its own: say so instead of "thinking".
        if app.compaction_active() {
            (
                format!(
                    "{} compacting context · {elapsed:.1}s · esc stop",
                    SPINNER[app.spinner % SPINNER.len()]
                ),
                ACCENT,
            )
        } else {
            let progress = match app.stream_tool_progress() {
                Some((name, detail, bytes)) => {
                    let size = format_bytes(bytes);
                    if detail.is_empty() {
                        format!("{name} · {size}")
                    } else {
                        format!("{name} {detail} · {size}")
                    }
                }
                None => "thinking".to_owned(),
            };
            let mut text = format!(
                "{} {progress} · {elapsed:.1}s",
                SPINNER[app.spinner % SPINNER.len()]
            );
            if let Some((in_tokens, out_tokens)) = app.stream_tokens() {
                text.push_str(&format!(" · in {}", format_tokens(in_tokens)));
                if out_tokens > 0 {
                    let rate = out_tokens as f64 / elapsed.max(0.2);
                    text.push_str(&format!(
                        " · out {} · {rate:.0} t/s",
                        format_tokens(out_tokens)
                    ));
                }
            }
            text.push_str(" · esc stop");
            (text, ACCENT)
        }
    } else if let Some(notice) = &app.notice {
        (
            notice.message.clone(),
            match notice.kind {
                NoticeKind::Info => MUTED,
                NoticeKind::Error => ERROR,
            },
        )
    } else {
        // The mode is the idle status's first word: Tab cycles build · plan · chat.
        let text = if app.tools_mode == ToolsMode::Auto {
            format!(
                "mode {} · [yolo] · shift+tab permissions · enter send · / commands",
                app.mode.label()
            )
        } else {
            format!(
                "mode {} · shift+tab yolo · enter send · / commands · F1 help",
                app.mode.label()
            )
        };
        let color = match app.mode {
            Mode::Build if app.tools_mode == ToolsMode::Auto => YELLOW,
            Mode::Build => MUTED,
            _ => ACCENT,
        };
        (text, color)
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(color)),
        columns[0],
    );

    if right_width > 0 {
        frame.render_widget(
            Paragraph::new(right)
                .alignment(Alignment::Right)
                .style(Style::default().fg(MUTED)),
            columns[1],
        );
    }
}
