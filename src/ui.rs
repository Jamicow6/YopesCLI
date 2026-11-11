use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    app::{
        App, COMMANDS, CUSTOM_PROVIDER_ID, ChangeReport, CommandSpec, ConnectStage, ConnectState,
        NoticeKind, ToolApproval, format_bytes, format_tokens,
    },
    diff::{ChangeKind, DiffLine, FileDiff},
    session::{Message, Role, Usage},
    tools,
};

const MUTED: Color = Color::DarkGray;
const ACCENT: Color = Color::Cyan;
const ERROR: Color = Color::Red;
const ADDED: Color = Color::Green;
const REMOVED: Color = Color::Red;
const TEXT: Color = Color::Reset;

/// Rows of command text shown in the approval prompt before it stops.
const MAX_APPROVAL_LINES: usize = 4;
/// The startup mascot: a small cat, drawn only on the welcome screen.
const CAT_ART: [&str; 6] = [
    "  /\\_/\\  ",
    "+ ( o.o ) +",
    "  #> ^ <#  ",
    "  /| . |\\  ",
    " (_|   |_) ",
    "   \"   \"   ",
];
/// Tool output rows shown before the rest is collapsed.
const TOOL_OUTPUT_PREVIEW_LINES: usize = 7;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const PALETTE_WIDTH: u16 = 60;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 28 || area.height < 8 {
        app.output_rows.clear();
        frame.render_widget(
            Paragraph::new("Terminal too small")
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED)),
            area,
        );
        return;
    }

    let palette = if app.show_help || app.connect_open() || app.approval.is_some() {
        Vec::new()
    } else {
        app.palette_matches()
    };
    let palette_height = if palette.is_empty() {
        0
    } else {
        (palette.len() as u16).saturating_add(2)
    };
    let composer_height = composer_height(app, area.width);

    let mut constraints = vec![Constraint::Length(1), Constraint::Min(2)];
    if palette_height > 0 {
        constraints.push(Constraint::Length(palette_height));
    }
    constraints.push(Constraint::Length(1));
    constraints.push(Constraint::Length(composer_height));
    constraints.push(Constraint::Length(1));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    draw_header(frame, app, chunks[0]);
    draw_transcript(frame, app, chunks[1]);

    let mut index = 2;
    if palette_height > 0 {
        draw_palette(frame, app, &palette, chunks[index]);
        index += 1;
    } else {
        draw_separator(frame, chunks[index]);
    }
    draw_composer(frame, app, chunks[index + 1]);
    draw_footer(frame, app, chunks[index + 2]);

    let spinner = app.spinner;
    if let Some(approval) = app.approval.as_mut() {
        draw_approval(frame, approval, area);
    } else if app.show_help {
        draw_help(frame, area);
    } else if let Some(connect) = app.connect_state() {
        draw_connect(frame, connect, spinner, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let inner = padded(area, 2);
    let right = if app.backend.model().is_empty() {
        format!("{} · {}", app.backend.provider_label(), app.spent_cost())
    } else {
        format!(
            "{} · {} · {}",
            app.backend.provider_label(),
            app.backend.model(),
            app.spent_cost()
        )
    };
    let right_width = (right.chars().count() as u16).min(inner.width / 2);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(4), Constraint::Length(right_width)])
        .split(inner);

    let title = Line::from(vec![
        Span::styled(
            "yopes",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(app.session.title.trim(), Style::default().fg(MUTED)),
    ]);
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

fn draw_transcript(frame: &mut Frame, app: &mut App, area: Rect) {
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
        for art in CAT_ART {
            welcome.push(Line::styled(art, Style::default().fg(ACCENT)));
        }
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
        frame.render_widget(
            Paragraph::new(welcome).alignment(Alignment::Center),
            inner,
        );
        return;
    }

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
            Role::Assistant => ("yopes", ACCENT),
            Role::Tool => ("tool", MUTED),
        };
        if message.role == Role::Tool {
            let start = lines.len();
            let framed = tool_result_lines(app, message, inner.width, &mut rows, start);
            lines.extend(framed);
            if streaming {
                if let Some(line) = lines.last_mut() {
                    line.spans
                        .push(Span::styled(" ▌", Style::default().fg(ACCENT)));
                }
            }
            continue;
        }
        lines.push(Line::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        if let Some(reasoning) = reasoning_line(message, streaming) {
            lines.push(reasoning);
        }
        if message.content.is_empty() && streaming && !has_calls && message.reasoning.is_empty() {
            lines.push(Line::styled("▌", Style::default().fg(ACCENT)));
            continue;
        }
        if !message.content.is_empty() {
            for content_line in message.content.split('\n') {
                lines.push(message_line(content_line));
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
                line.spans
                    .push(Span::styled(" ▌", Style::default().fg(ACCENT)));
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
        app.output_rows.push((inner.y.saturating_add(offset), call_id));
    }
}

fn draw_palette(frame: &mut Frame, app: &App, palette: &[&'static CommandSpec], area: Rect) {
    let width = area.width.saturating_sub(4).min(PALETTE_WIDTH);
    if width < 20 {
        return;
    }
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
        width,
        area.height,
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title_top(Line::styled(" commands ", Style::default().fg(MUTED)))
        .title_bottom(Line::styled(
            " ↑↓ move · tab complete · enter run ",
            Style::default().fg(MUTED),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let selected = app.palette_selected();
    for (index, command) in palette.iter().enumerate() {
        if index as u16 >= inner.height {
            break;
        }
        let row = Rect::new(inner.x, inner.y + index as u16, inner.width, 1);
        let chosen = index == selected;
        let marker = if chosen { "› " } else { "  " };
        let name = command.display_name();
        let name_style = if chosen {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        let description_style = if chosen {
            Style::default().fg(TEXT)
        } else {
            Style::default().fg(MUTED)
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(format!("{name:<14}"), name_style),
            Span::styled(command.description, description_style),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

fn draw_separator(frame: &mut Frame, area: Rect) {
    let inner = padded(area, 2);
    if inner.width == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new("─".repeat(usize::from(inner.width))).style(Style::default().fg(MUTED)),
        inner,
    );
}

fn composer_height(app: &App, width: u16) -> u16 {
    let content_width = width.saturating_sub(6).max(1);
    let lines = wrapped_line_count(&app.input.with_cursor(), content_width);
    (lines as u16).clamp(1, 6)
}

fn draw_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = padded(area, 2);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);
    let prompt_style = if app.is_busy() {
        Style::default().fg(MUTED)
    } else {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    };
    frame.render_widget(Paragraph::new("›").style(prompt_style), columns[0]);

    let target = columns[1];
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
        Paragraph::new(app.input.with_cursor())
            .style(Style::default().fg(TEXT))
            .wrap(Wrap { trim: false })
            .scroll((app.input_scroll.min(u16::MAX as usize) as u16, 0)),
        target,
    );
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
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
            "approval needed · enter run · a always allow · esc deny · v diff".to_owned(),
            ACCENT,
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
    } else if let Some(notice) = &app.notice {
        (
            notice.message.clone(),
            match notice.kind {
                NoticeKind::Info => MUTED,
                NoticeKind::Error => ERROR,
            },
        )
    } else {
        ("enter send · / commands · F1 help".to_owned(), MUTED)
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

/// A small prompt for a risky command. It only names the command and asks.
fn draw_approval(frame: &mut Frame, approval: &mut ToolApproval, area: Rect) {
    let width = area.width.saturating_sub(4).min(72);
    if width < 30 || area.height < 8 {
        return;
    }
    let content_width = usize::from(width.saturating_sub(6));
    let lines = approval_lines(approval, content_width);
    let height = (lines.len() as u16 + 4).clamp(7, area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) * 2 / 3,
        width,
        height,
    );
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title_top(Line::from(vec![Span::styled(
            " run this command? ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )]))
        .title_bottom(Line::styled(
            " enter run · a always · esc deny ",
            Style::default().fg(MUTED),
        ))
        .padding(Padding::new(1, 1, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    approval.page_height = inner.height;
    let maximum = lines.len().saturating_sub(usize::from(inner.height));
    approval.scroll = approval.scroll.min(maximum);
    frame.render_widget(
        Paragraph::new(lines).scroll((approval.scroll.min(u16::MAX as usize) as u16, 0)),
        inner,
    );
}

/// A tool result, framed so its output does not blend into the conversation.
///
/// The transcript is one paragraph, so the frame is drawn with box glyphs
/// rather than a widget, and every inner line is clipped to the frame width.
fn tool_result_lines(
    app: &App,
    message: &Message,
    width: u16,
    rows: &mut Vec<(u16, String)>,
    start_row: usize,
) -> Vec<Line<'static>> {
    let outer = usize::from(width).max(1);
    let inner_width = outer.saturating_sub(4);
    let failed = message.content.starts_with("Error: ");
    let border = if failed { ERROR } else { MUTED };
    let rendered = message.content.split('\n').collect::<Vec<_>>();
    // On a very narrow terminal a frame would not fit; draw the raw lines.
    if inner_width < 6 {
        return rendered
            .iter()
            .take(TOOL_OUTPUT_PREVIEW_LINES)
            .map(|line| {
                Line::styled(
                    truncate(line, outer),
                    Style::default().fg(if failed { ERROR } else { TEXT }),
                )
            })
            .collect();
    }

    let color = if failed { ERROR } else { TEXT };
    let call_id = message.tool_call_id.clone().unwrap_or_default();
    let report = message
        .tool_call_id
        .as_deref()
        .and_then(|call_id| app.changes.get(call_id));
    let mut body: Vec<Line<'static>> = Vec::new();
    for content_line in &rendered {
        body.push(Line::styled(
            truncate(content_line, inner_width),
            Style::default().fg(color),
        ));
    }
    if let Some(report) = report {
        body.extend(change_lines(report, inner_width));
    }
    if body.is_empty() {
        body.push(Line::styled(" ", Style::default().fg(MUTED)));
    }
    // Long output, diff included, is folded; a click on the frame reveals the rest.
    let total = body.len();
    let clickable = total > TOOL_OUTPUT_PREVIEW_LINES;
    let expanded = clickable && app.expanded.contains(&call_id);
    if clickable && !expanded {
        let hidden = total - TOOL_OUTPUT_PREVIEW_LINES;
        let hint = if inner_width >= 30 {
            format!(
                "… {hidden} more line{} · click to expand",
                if hidden == 1 { "" } else { "s" }
            )
        } else {
            format!("… {hidden} more")
        };
        let mut shown: Vec<Line<'static>> = body.drain(..TOOL_OUTPUT_PREVIEW_LINES).collect();
        shown.push(Line::styled(
            truncate(&hint, inner_width),
            Style::default().fg(MUTED),
        ));
        body = shown;
    }
    if expanded {
        body.push(Line::styled(
            truncate("· click to collapse", inner_width),
            Style::default().fg(MUTED),
        ));
    }

    let label = truncate(
        report.map_or("tool", |report| report.diff.path.as_str()),
        inner_width,
    );
    let framed = frame_lines(label, body, inner_width, border);
    if clickable {
        // Rows are traced so a click anywhere on the frame toggles it. The
        // leading blank line is row 0 of `framed`, so skip it.
        for offset in 1..framed.len() {
            rows.push((start_row.saturating_add(offset) as u16, call_id.clone()));
        }
    }
    framed
}

/// Wrap body lines in a light frame of exactly `inner_width + 4` columns.
fn frame_lines(
    label: String,
    body: Vec<Line<'static>>,
    inner_width: usize,
    border: Color,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(body.len() + 3);
    lines.push(Line::raw(""));
    // Every row is exactly inner_width + 4 columns, so the frame stays aligned
    // even when the label is clipped. The label gets what the corners, the left
    // cap, and one separating space leave over.
    let total = inner_width + 4;
    let label = truncate(&label, total.saturating_sub(6));
    let head = "╭─ ".len() + label.chars().count() + 1;
    lines.push(Line::from(vec![
        Span::styled(format!("╭─ {label} "), Style::default().fg(border)),
        Span::styled(
            "─".repeat(total.saturating_sub(head + 2)),
            Style::default().fg(border),
        ),
        Span::styled("─╮", Style::default().fg(border)),
    ]));
    for line in body {
        let content = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        let clipped = truncate(&content, inner_width);
        let used = clipped.chars().count();
        let style = line
            .spans
            .first()
            .map_or_else(Style::default, |span| span.style);
        lines.push(Line::from(vec![
            Span::styled("│ ", Style::default().fg(border)),
            Span::styled(clipped, style),
            Span::raw(" ".repeat(inner_width.saturating_sub(used) + 1)),
            Span::styled("│", Style::default().fg(border)),
        ]));
    }
    lines.push(Line::styled(
        format!("╰{}╯", "─".repeat(total.saturating_sub(2))),
        Style::default().fg(border),
    ));
    lines
}

/// The diff under a tool result: a file header, then the change itself.
fn change_lines(report: &ChangeReport, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let diff = &report.diff;
    let mut lines = Vec::new();
    let mut spans = vec![Span::styled(
        truncate(&diff.path, width.saturating_sub(22)),
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
    )];
    if diff.created {
        spans.push(Span::styled("  new file", Style::default().fg(MUTED)));
    }
    spans.push(Span::styled(
        format!("  +{}", diff.additions),
        Style::default().fg(ADDED),
    ));
    spans.push(Span::styled(
        format!("  −{}", diff.deletions),
        Style::default().fg(REMOVED),
    ));
    lines.push(Line::from(spans));
    if diff.is_empty() {
        let note = diff.note.as_deref().unwrap_or("No line changes.");
        lines.push(Line::styled(
            truncate(note, width),
            Style::default().fg(MUTED),
        ));
        return lines;
    }
    if report.side_by_side {
        lines.extend(side_by_side_lines(diff, width));
    } else {
        lines.extend(unified_lines(diff, width));
    }
    lines
}

fn approval_lines(approval: &ToolApproval, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let command = tools::command_text(&approval.call).unwrap_or_default();
    // The command is the whole point: show it first, wrapped, capped at a few
    // rows so the prompt stays small.
    let mut shown = 0;
    for (index, raw) in command.lines().enumerate() {
        let line = if index == 0 {
            format!("$ {raw}")
        } else {
            format!("  {raw}")
        };
        for wrapped in textwrap::wrap(&line, textwrap::Options::new(width)) {
            if shown == MAX_APPROVAL_LINES {
                lines.push(Line::styled("…", Style::default().fg(MUTED)));
                break;
            }
            lines.push(Line::styled(
                wrapped.into_owned(),
                Style::default().fg(TEXT),
            ));
            shown += 1;
        }
        if shown == MAX_APPROVAL_LINES {
            break;
        }
    }
    if shown == 0 {
        lines.push(Line::styled("$", Style::default().fg(TEXT)));
    }
    if let Some(note) = &approval.note {
        lines.push(Line::styled(
            truncate(note, width),
            Style::default().fg(ERROR),
        ));
    }
    lines
}

fn unified_lines(diff: &FileDiff, width: usize) -> Vec<Line<'static>> {
    let gutter = line_number_width(diff);
    let text_width = width.saturating_sub(gutter * 2 + 4).max(1);
    let mut lines = Vec::new();
    for (index, hunk) in diff.hunks.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            diff.header(hunk),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        for line in &hunk.lines {
            let (sign, color) = diff_sign(line.kind);
            let old = line
                .old_line
                .map(|number| number.to_string())
                .unwrap_or_default();
            let new = line
                .new_line
                .map(|number| number.to_string())
                .unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{old:>gutter$} {new:>gutter$} "),
                    Style::default().fg(MUTED),
                ),
                Span::styled(format!("{sign} "), Style::default().fg(color)),
                Span::styled(truncate(&line.text, text_width), Style::default().fg(color)),
            ]));
        }
    }
    lines
}

fn side_by_side_lines(diff: &FileDiff, width: usize) -> Vec<Line<'static>> {
    let gutter = line_number_width(diff);
    let text_width = width.saturating_sub(gutter * 2 + 5).max(1) / 2;
    let mut lines = Vec::new();
    for (index, hunk) in diff.hunks.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        lines.push(Line::styled(
            diff.header(hunk),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        for row in hunk.side_rows() {
            let left_number = row
                .left
                .as_ref()
                .and_then(|line| line.old_line)
                .map(|number| number.to_string())
                .unwrap_or_default();
            let right_number = row
                .right
                .as_ref()
                .and_then(|line| line.new_line)
                .map(|number| number.to_string())
                .unwrap_or_default();
            let left_text = row
                .left
                .as_ref()
                .map_or_else(String::new, |line| truncate(&line.text, text_width));
            let right_text = row
                .right
                .as_ref()
                .map_or_else(String::new, |line| truncate(&line.text, text_width));
            let left_style = row
                .left
                .as_ref()
                .map_or_else(|| Style::default().fg(TEXT), |line| diff_style(line));
            let right_style = row
                .right
                .as_ref()
                .map_or_else(|| Style::default().fg(TEXT), |line| diff_style(line));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{left_number:>gutter$} "),
                    Style::default().fg(MUTED),
                ),
                Span::styled(format!("{left_text:<text_width$}"), left_style),
                Span::styled(" │ ", Style::default().fg(MUTED)),
                Span::styled(
                    format!("{right_number:>gutter$} "),
                    Style::default().fg(MUTED),
                ),
                Span::styled(right_text, right_style),
            ]));
        }
    }
    lines
}

fn line_number_width(diff: &FileDiff) -> usize {
    diff.hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter_map(|line| line.old_line.or(line.new_line))
        .max()
        .unwrap_or(1)
        .to_string()
        .len()
        .max(3)
}

fn diff_sign(kind: ChangeKind) -> (&'static str, Color) {
    match kind {
        ChangeKind::Context => (" ", TEXT),
        ChangeKind::Insert => ("+", ADDED),
        ChangeKind::Delete => ("-", REMOVED),
    }
}

fn diff_style(line: &DiffLine) -> Style {
    let (_, color) = diff_sign(line.kind);
    Style::default().fg(color)
}

fn draw_connect(frame: &mut Frame, connect: &ConnectState, spinner: usize, area: Rect) {
    let width = area.width.saturating_sub(6).min(78);
    let height = area.height.saturating_sub(4).min(24);
    if width < 34 || height < 10 {
        return;
    }
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title_top(Line::from(vec![
            Span::styled(
                " connect ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("· {} ", connect.stage.label()),
                Style::default().fg(MUTED),
            ),
        ]))
        .title_bottom(Line::styled(
            " providers + custom · stored locally ",
            Style::default().fg(MUTED),
        ))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height < 4 {
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(inner);

    let mut spans = vec![
        Span::styled(
            format!("{}  ", connect.input_label()),
            Style::default().fg(MUTED),
        ),
        Span::styled(connect.input(), Style::default().fg(TEXT)),
    ];
    if connect.uses_cursor() {
        spans.push(Span::styled("▌", Style::default().fg(ACCENT)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[0]);

    draw_connect_list(frame, connect, spinner, rows[2]);
    draw_connect_hint(frame, connect, rows[3]);
}

fn draw_connect_list(frame: &mut Frame, connect: &ConnectState, spinner: usize, area: Rect) {
    if connect.loading {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    SPINNER[spinner % SPINNER.len()],
                    Style::default().fg(ACCENT),
                ),
                Span::styled(
                    "  loading providers from models.dev",
                    Style::default().fg(MUTED),
                ),
            ])),
            area,
        );
        return;
    }
    let mut area = area;
    if let Some(error) = &connect.error {
        if area.height == 0 {
            return;
        }
        let line = Rect::new(area.x, area.y, area.width, 1);
        frame.render_widget(
            Paragraph::new(truncate(error, usize::from(area.width)))
                .style(Style::default().fg(ERROR)),
            line,
        );
        area = Rect {
            y: area.y.saturating_add(1),
            height: area.height.saturating_sub(1),
            ..area
        };
    }
    if !matches!(
        connect.stage,
        ConnectStage::Providers
            | ConnectStage::Models
            | ConnectStage::Auth
            | ConnectStage::CustomModels
    ) {
        return;
    }

    let items = connect.items_using(app.backend.base_url(), app.backend.model());
    if items.is_empty() {
        if connect.error.is_none() {
            frame.render_widget(
                Paragraph::new("No matches.").style(Style::default().fg(MUTED)),
                area,
            );
        }
        return;
    }
    let height = usize::from(area.height);
    if height == 0 {
        return;
    }
    let selected = connect.selected.min(items.len() - 1);
    let custom_count = items
        .iter()
        .take_while(|item| item.id == CUSTOM_PROVIDER_ID || item.custom_id.is_some())
        .count();
    let divider =
        connect.stage == ConnectStage::Providers && custom_count > 0 && custom_count < items.len();
    let divider_rows = if divider { 1 } else { 0 };
    let mut rows = Vec::with_capacity(items.len() + divider_rows);
    for index in 0..items.len() {
        if divider && index == custom_count {
            rows.push(None);
        }
        rows.push(Some(index));
    }
    let selected_row = selected
        + if divider && selected >= custom_count {
            1
        } else {
            0
        };
    let start_row = selected_row.saturating_sub(height.saturating_sub(1));
    let start = start_row.min(rows.len().saturating_sub(height));
    let title_width = usize::from(area.width).saturating_sub(30).max(10);

    for (offset, row) in rows.iter().skip(start).take(height).enumerate() {
        let rect = Rect::new(area.x, area.y + offset as u16, area.width, 1);
        let Some(index) = *row else {
            let label = " models.dev ";
            let remaining = usize::from(area.width)
                .saturating_sub(label.chars().count())
                .saturating_sub(3);
            frame.render_widget(
                Paragraph::new(Line::styled(
                    format!("  ─{label}{}", "─".repeat(remaining)),
                    Style::default().fg(MUTED),
                )),
                rect,
            );
            continue;
        };
        let item = &items[index];
        let chosen = index == selected;
        let marker = if chosen { "› " } else { "  " };
        let logo_style = if chosen {
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ACCENT)
        };
        let title_style = if chosen {
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        let detail_style = if chosen {
            Style::default().fg(TEXT)
        } else {
            Style::default().fg(MUTED)
        };
        let mut spans = vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(item.logo.to_string(), logo_style),
            Span::styled("  ", Style::default()),
            Span::styled(
                format!("{:<title_width$}", truncate(&item.title, title_width)),
                title_style,
            ),
        ];
        // A tick marks the provider and model this session is already using.
        spans.push(Span::styled(
            if item.active { "✓ " } else { "  " },
            Style::default().fg(ADDED).add_modifier(Modifier::BOLD),
        ));
        if connect.stage == ConnectStage::Providers {
            spans.push(Span::styled("→", Style::default().fg(MUTED)));
            spans.push(Span::styled("  ", Style::default()));
        } else {
            spans.push(Span::styled("  ", Style::default()));
        }
        spans.push(Span::styled(item.detail.clone(), detail_style));
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
    }
}

fn draw_connect_hint(frame: &mut Frame, connect: &ConnectState, area: Rect) {
    let text = match connect.stage {
        ConnectStage::Providers => "↑↓ move · type to filter · enter choose · esc close".to_owned(),
        ConnectStage::Models => "↑↓ move · type to filter · enter choose · esc back".to_owned(),
        ConnectStage::ApiKey => connect.required_env().map_or_else(
            || "enter continue · esc back · empty skips the key".to_owned(),
            |name| format!("enter continue · esc back · empty uses ${name}"),
        ),
        ConnectStage::Endpoint => "enter continue · esc back".to_owned(),
        ConnectStage::ProviderName => "enter continue · esc close".to_owned(),
        ConnectStage::Auth => "↑↓ choose · enter continue · esc back".to_owned(),
        ConnectStage::CustomToken => "enter continue · esc back".to_owned(),
        ConnectStage::ModelId => {
            if connect.has_custom_models() {
                "enter continue · empty keeps saved models · esc back".to_owned()
            } else {
                "enter continue · esc back".to_owned()
            }
        }
        ConnectStage::ModelName => {
            "enter add model · empty uses the model id · esc back".to_owned()
        }
        ConnectStage::CustomModels => {
            "↑↓ choose · enter use · esc back · rows below edit the endpoint".to_owned()
        }
        ConnectStage::Password => {
            "enter save · esc back · empty stores without encryption".to_owned()
        }
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), area);
}

fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let mut characters = text.chars();
    let mut result = characters.by_ref().take(width).collect::<String>();
    if characters.next().is_some() {
        result.pop();
        result.push('…');
    }
    result
}

/// Reasoning shows live while streaming and collapsed afterwards, always dim.
/// One line of assistant text, with light inline markdown: `**bold**`,
/// `` `code` ``, and `-`/`*`/`1.` bullets rendered with a coloured marker.
fn message_line(raw: &str) -> Line<'static> {
    let trimmed = raw.trim_start();
    let (indent, body) = raw.split_at(raw.len() - trimmed.len());
    let bullet = bullet_marker(body);
    let mut spans = vec![Span::styled(
        if bullet {
            format!("{indent}• ")
        } else {
            indent.to_owned()
        },
        Style::default().fg(if bullet { ACCENT } else { TEXT }),
    )];
    spans.extend(inline_spans(if bullet {
        strip_bullet(body)
    } else {
        body
    }));
    Line::from(spans)
}

/// Whether a line starts a markdown list item.
fn bullet_marker(line: &str) -> bool {
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return true;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && line
            .chars()
            .nth(digits)
            .is_some_and(|next| next == '.' || next == ')')
        && line.chars().nth(digits + 1).is_some_and(|next| next == ' ')
}

fn strip_bullet(line: &str) -> &str {
    if let Some(rest) = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("+ "))
    {
        return rest;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if !bullet_marker(line) {
        return line;
    }
    // `1. item` and `2) item`: skip the digits, the separator, and one space.
    line.char_indices()
        .nth(digits + 2)
        .map_or(line, |(offset, _)| &line[offset..])
}

fn inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let bold = rest.match_indices("**").next().map(|(at, _)| (at, "**"));
        let code = rest.match_indices('`').next().map(|(at, _)| (at, "`"));
        let next = match (bold, code) {
            (None, None) => None,
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        };
        let Some((at, marker)) = next else {
            spans.push(Span::styled(rest.to_owned(), Style::default().fg(TEXT)));
            break;
        };
        if at > 0 {
            spans.push(Span::styled(
                rest[..at].to_owned(),
                Style::default().fg(TEXT),
            ));
        }
        let after = &rest[at + marker.len()..];
        let Some(end) = after.find(marker) else {
            spans.push(Span::styled(rest[at..].to_owned(), Style::default().fg(TEXT)));
            break;
        };
        let inner = &after[..end];
        if inner.is_empty() {
            spans.push(Span::styled(rest[at..].to_owned(), Style::default().fg(TEXT)));
            break;
        }
        spans.push(if marker == "**" {
            Span::styled(
                inner.to_owned(),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            )
        } else {
            Span::styled(inner.to_owned(), Style::default().fg(ACCENT))
        });
        rest = &after[end + marker.len()..];
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), Style::default().fg(TEXT)));
    }
    spans
}

/// The muted stats line under a finished reply: tokens in and out, then how long.
fn usage_line(usage: Usage) -> Line<'static> {
    let mut text = format!(
        "in {} · out {}",
        format_tokens(usage.prompt_tokens),
        format_tokens(usage.completion_tokens)
    );
    let seconds = usage.elapsed_ms as f64 / 1000.0;
    if usage.elapsed_ms > 0 {
        text.push_str(&format!(" · {seconds:.1}s"));
    }
    if let Some(rate) = usage.tokens_per_second() {
        text.push_str(&format!(" · {rate:.0} t/s"));
    }
    Line::styled(text, Style::default().fg(MUTED))
}

fn reasoning_line(message: &Message, streaming: bool) -> Option<Line<'static>> {
    let lines: Vec<&str> = message
        .reasoning
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return None;
    }
    let text = if streaming {
        lines.last().copied().unwrap_or_default().to_owned()
    } else if lines.len() > 2 {
        format!("{} … {} lines of thinking", lines[0], lines.len())
    } else {
        lines.join(" ")
    };
    Some(Line::from(vec![
        Span::styled(
            if streaming { "│ " } else { "▸ " },
            Style::default().fg(MUTED),
        ),
        Span::styled(text, Style::default().fg(MUTED).add_modifier(Modifier::DIM)),
    ]))
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let width = area.width.min(68);
    let height = area.height.min(32);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title_top(Line::styled(
            " help ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Line::styled(" esc close ", Style::default().fg(MUTED)))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines = vec![
        help_section("keys"),
        help_row("enter", "send message"),
        help_row("alt+enter", "new line"),
        help_row("/", "open command palette"),
        help_row("tab", "complete selected command"),
        help_row("↑ ↓", "palette, conversation, or diff"),
        help_row("esc", "stop streaming / deny tool / close"),
        help_row("enter", "run the pending tool"),
        help_row("a", "always allow that tool this session"),
        help_row("v", "toggle unified / side-by-side diffs"),
        help_row("ctrl+n", "new session"),
        help_row("ctrl+u", "clear input"),
        help_row("ctrl+w", "delete previous word"),
        help_row("ctrl+c", "save and quit"),
        Line::raw(""),
        help_section("commands"),
    ];
    for command in COMMANDS {
        lines.push(help_row(command.display_name(), command.description));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn help_section(title: &'static str) -> Line<'static> {
    Line::styled(
        title,
        Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
    )
}

fn help_row(key: impl Into<String>, description: &'static str) -> Line<'static> {
    let key = key.into();
    Line::from(vec![
        Span::styled(format!("{key:<14}"), Style::default().fg(TEXT)),
        Span::styled(description, Style::default().fg(MUTED)),
    ])
}

fn padded(area: Rect, horizontal: u16) -> Rect {
    Rect {
        x: area.x.saturating_add(horizontal),
        width: area.width.saturating_sub(horizontal.saturating_mul(2)),
        ..area
    }
}

fn wrapped_line_count(text: &str, width: u16) -> usize {
    let options = textwrap::Options::new(usize::from(width).max(1));
    text.split('\n')
        .map(|line| textwrap::wrap(line, &options).len().max(1))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{frame_lines, wrapped_line_count};
    use ratatui::{style::Color, text::Line};

    fn text_of(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn every_framed_line_has_the_same_width() {
        let body = vec![
            Line::raw("short"),
            Line::raw("a much longer body line that has to be clipped"),
            Line::raw(""),
        ];
        let lines = frame_lines("main.py".to_owned(), body, 20, Color::Reset);
        assert_eq!(lines.len(), 6);
        for line in &lines[1..] {
            assert_eq!(text_of(line).chars().count(), 24, "{:?}", text_of(line));
        }
        assert!(text_of(&lines[1]).starts_with("╭─ main.py "));
        assert!(text_of(&lines[3]).starts_with("│ a much longer body"));
        assert!(text_of(&lines[5]).starts_with('╰'));
    }

    #[test]
    fn a_label_wider_than_the_frame_does_not_break_it() {
        let lines = frame_lines("x".repeat(60), vec![Line::raw("body")], 10, Color::Reset);
        assert_eq!(text_of(&lines[1]).chars().count(), 14);
        assert_eq!(text_of(&lines[2]).chars().count(), 14);
    }

    #[test]
    fn the_frame_fits_every_usable_width() {
        for inner in 6..90 {
            for label in ["tool", "main.py", "yopes", "a-very-long-path/some/file.rs"] {
                let body = vec![
                    Line::raw("short"),
                    Line::raw("a much longer body line that has to be clipped"),
                    Line::raw(""),
                ];
                let lines = frame_lines(label.to_owned(), body, inner, Color::Reset);
                for line in &lines[1..] {
                    assert_eq!(
                        text_of(line).chars().count(),
                        inner + 4,
                        "inner={inner} label={label}",
                    );
                }
            }
        }
    }

    #[test]
    fn wrapped_counts_include_words_and_explicit_newlines() {
        assert_eq!(wrapped_line_count("", 10), 1);
        assert_eq!(wrapped_line_count("one\ntwo", 10), 2);
        assert_eq!(wrapped_line_count("aaaaaa aaaaaa aaaaaa", 10), 3);
    }
}
