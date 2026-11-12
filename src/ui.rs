use std::{collections::VecDeque, path::PathBuf, time::SystemTime};

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{
        App, COMMANDS, CUSTOM_PROVIDER_ID, ChangeReport, CommandSpec, ConnectStage, ConnectState,
        Mode, ListPicker, NoticeKind, ToolApproval, format_bytes, format_tokens,
    },
    catalog::FASFAST_ID,
    diff::{ChangeKind, DiffLine, FileDiff},
    session::{Message, Role, Usage},
    tools,
};

/// How a `read_file` result surfaces in the transcript: quiet. The header and
/// range lines render instead of the contents; a click still expands it.
fn read_result_summary(result: &str) -> Option<(String, Vec<String>)> {
    let first = result.lines().next()?;
    let open = first.rfind(" (")?;
    let path = first[..open].trim();
    if path.is_empty() {
        return None;
    }
    let count = first.get(open + 2..)?.strip_suffix(" lines)")?;
    count.trim().parse::<usize>().ok()?;
    let mut summary = vec![first.trim().to_owned()];
    if let Some(range) = result.lines().nth(1).filter(|line| line.starts_with("lines ")) {
        summary.push(range.trim().to_owned());
    }
    Some((path.to_owned(), summary))
}

/// Name of the tool that produced the result at `index`, found by walking
/// back to the assistant message that requested the call.
fn tool_call_name(site: &App, index: usize) -> Option<&str> {
    let id = site.session.messages[index].tool_call_id.as_deref()?;
    for message in site.session.messages[..index].iter().rev() {
        if let Some(calls) = &message.tool_calls {
            if let Some(call) = calls.iter().find(|call| call.id.as_str() == id) {
                return Some(call.function.name.as_str());
            }
        }
    }
    None
}

const MUTED: Color = Color::DarkGray;
const ACCENT: Color = Color::Cyan;
const ERROR: Color = Color::Red;
const ADDED: Color = Color::Green;
const REMOVED: Color = Color::Red;
const TEXT: Color = Color::Reset;

/// Rows of command text shown in the approval prompt before it stops.
const MAX_APPROVAL_LINES: usize = 4;
/// The startup mascot: a small cat, drawn only on the welcome screen, with a
/// yarn ball that rolls beneath it on the idle ticker, one step every ~200ms.
const CAT_BASE: [&str; 6] = [
    "  /\\_/\\  ",
    "+ ( o.o ) +",
    "  #> ^ <#  ",
    "  /| . |\\  ",
    " (_|   |_) ",
    "   \"   \"   ",
];
/// One row below the cat: the ball drifts right and gives back one step.
const YARN_ROLL: [&str; 6] = [
    " o         ",
    "    o      ",
    "      o    ",
    "        o  ",
    "         o ",
    "        o  ",
];
/// Tool output rows shown before the rest is collapsed.
const TOOL_OUTPUT_PREVIEW_LINES: usize = 7;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const PALETTE_WIDTH: u16 = 60;

/// The streaming caret alternates on a ~300ms cadence driven by the 100ms
/// ticker, so a quiet stretch of tokens still reads as alive.
fn caret(spinner: usize) -> &'static str {
    if (spinner / 3) % 2 == 0 {
        "▌"
    } else {
        ""
    }
}

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

    let picker = app.model_picker.clone();
    let session_picker = app.session_picker.clone();
    // The Progress panel steps aside once every task is done: a finished
    // checklist means finished work, not a panel parked over the composer.
    let tasks = app.session.tasks.iter().any(|task| !task.done);
    let palette = if app.show_help
        || app.connect_open()
        || app.approval.is_some()
        || picker.is_some()
        || session_picker.is_some()
    {
        Vec::new()
    } else {
        app.palette_matches()
    };
    let palette_height = if palette.is_empty() {
        0
    } else {
        (palette.len() as u16).saturating_add(2)
    };
    let picker_height = if picker.is_some() {
        // A long list scrolls inside the bordered popup instead of growing.
        (app.model_picker_count().min(12).max(1) as u16).saturating_add(2)
    } else {
        0
    };
    let session_height = if session_picker.is_some() {
        (app.session_picker_count().min(12).max(1) as u16).saturating_add(2)
    } else {
        0
    };
    let popup_height = palette_height + picker_height + session_height;
    let composer_height = composer_height(app, area.width);
    let progress_height = if tasks { (app.session.tasks.len().min(6) as u16) + 2 } else { 0 };

    let mut constraints = vec![Constraint::Length(1), Constraint::Min(2)];
    if popup_height > 0 {
        constraints.push(Constraint::Length(popup_height));
    }
    if progress_height > 0 {
        constraints.push(Constraint::Length(progress_height));
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
    if popup_height > 0 {
        if let Some(session_picker) = &session_picker {
            draw_session_picker(frame, app, session_picker, chunks[index]);
        } else if let Some(picker) = &picker {
            draw_model_picker(frame, app, picker, chunks[index]);
        } else {
            draw_palette(frame, app, &palette, chunks[index]);
        }
        index += 1;
    }
    if progress_height > 0 {
        draw_progress(frame, app, chunks[index]);
        index += 1;
    } else if popup_height == 0 {
        draw_separator(frame, chunks[index]);
    }
    // The separator slot is reserved even when an overlay or the panel takes
    // that row instead.
    index += 1;
    draw_composer(frame, app, chunks[index]);
    draw_footer(frame, app, chunks[index + 1]);

    let spinner = app.spinner;
    let sides = app.diff_sides;
    if let Some(approval) = app.approval.as_mut() {
        draw_approval(frame, approval, sides, spinner, area);
    } else if app.show_help {
        draw_help(frame, area);
    } else if let Some(connect) = app.connect_state() {
        draw_connect(frame, app, connect, spinner, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
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
        frame.render_widget(
            Paragraph::new(welcome).alignment(Alignment::Center),
            inner,
        );
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
            let framed = tool_result_lines(app, message, inner.width, &mut rows, start, name.as_deref());
            lines.extend(framed);
            if streaming {
                if let Some(line) = lines.last_mut() {
                    line.spans
                        .push(Span::styled(format!(" {}", caret(app.spinner)), Style::default().fg(ACCENT)));
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
            lines.push(Line::styled(caret(app.spinner), Style::default().fg(ACCENT)));
            continue;
        }
        if !message.content.is_empty() {
            for content_line in message.content.split('\n') {
                lines.push(message_line(content_line));
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
                    .push(Span::styled(format!(" {}", caret(app.spinner)), Style::default().fg(ACCENT)));
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

fn draw_model_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
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
    let filter_field = if picker.filter.is_empty() {
        String::new()
    } else {
        format!(" · {}", picker.filter)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title_top(Line::styled(
            format!(" model{filter_field} "),
            Style::default().fg(MUTED),
        ))
        .title_bottom(Line::styled(
            " ↑↓ move · enter switch · esc cancel ",
            Style::default().fg(MUTED),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let models = app.model_matches();
    if models.is_empty() {
        frame.render_widget(
            Paragraph::new("No matches.").style(Style::default().fg(MUTED)),
            inner,
        );
        return;
    }
    let selected = picker.selected.min(models.len() - 1);
    // The popup shows a window of the list, moving with the selection.
    let visible = inner.height as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1));
    for (offset, model) in models
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        let index = start + offset;
        let row = Rect::new(inner.x, inner.y + offset as u16, inner.width, 1);
        let chosen = index == selected;
        let marker = if chosen { "› " } else { "  " };
        let title = if model.name.trim().is_empty() {
            model.id.clone()
        } else {
            model.name.clone()
        };
        let title_style = if chosen {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(
                format!(
                    "{title:<width$}  ",
                    width = inner.width.saturating_sub(30) as usize
                ),
                title_style,
            ),
            Span::styled(model.id.clone(), Style::default().fg(MUTED)),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

fn draw_session_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
    let width = area.width.saturating_sub(4).min(PALETTE_WIDTH);
    if width < 24 || area.height < 3 {
        return;
    }
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y,
        width,
        area.height,
    );
    let filter_field = if picker.filter.is_empty() {
        String::new()
    } else {
        format!(" · {}", picker.filter)
    };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(MUTED))
        .title_top(Line::styled(
            format!(" sessions{filter_field} "),
            Style::default().fg(MUTED),
        ))
        .title_bottom(Line::styled(
            " ↑↓ move · enter load · esc cancel ",
            Style::default().fg(MUTED),
        ));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let sessions = app.session_matches();
    if sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("No matching sessions.").style(Style::default().fg(MUTED)),
            inner,
        );
        return;
    }
    let selected = picker.selected.min(sessions.len() - 1);
    let active: Option<PathBuf> = app.active_session_path().cloned();
    // Rows show a window of the whole list, moving with the selection.
    let visible = inner.height as usize;
    let start = selected.saturating_sub(visible.saturating_sub(1));
    let right_width = 4;
    for (offset, entry) in sessions
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        let index = start + offset;
        let row = Rect::new(inner.x, inner.y + offset as u16, inner.width, 1);
        let chosen = index == selected;
        let marker = if chosen { "› " } else { "  " };
        let current = active.as_ref() == Some(&entry.path);
        let right = if current {
            format!("{} · now", age_label(entry.modified))
        } else {
            age_label(entry.modified)
        };
        let left_width = inner.width.saturating_sub(right_width + 4) as usize;
        let title = entry.title.clone();
        let title_style = if chosen {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(format!("{title:<left_width$} "), title_style),
            Span::styled(right, Style::default().fg(MUTED)),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

/// Rough age of a saved session file: now / minutes / hours / days.
fn age_label(modified: Option<SystemTime>) -> String {
    let Some(modified) = modified else {
        return String::new();
    };
    let seconds = modified.elapsed().map_or(0, |age| age.as_secs());
    if seconds < 60 {
        "now".to_owned()
    } else if seconds < 3_600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h", seconds / 3_600)
    } else {
        format!("{}d", seconds / 86_400)
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
    // The prompt flashes briefly after a send: input registered, turn running.
    let pulse = app
        .send_pulse
        .is_some_and(|mark| app.spinner.saturating_sub(mark) < 5);
    let prompt_style = if pulse {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else if app.is_busy() {
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

/// The native progress panel: the session's task checklist above the composer.
fn draw_progress(frame: &mut Frame, app: &App, area: Rect) {
    if area.height < 3 || area.width < 20 {
        return;
    }
    let inner = padded(area, 2);
    let done = app.session.tasks.iter().filter(|task| task.done).count();
    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!("Progress {done}/{}", app.session.tasks.len()),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · todo_write", Style::default().fg(MUTED)),
    ])];
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
    frame.render_widget(Paragraph::new(lines), inner);
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
        // The mode is the idle status's first word: Tab cycles build · plan · chat.
        let text = format!(
            "mode {} · enter send · / commands · F1 help",
            app.mode.label()
        );
        let color = match app.mode {
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

/// A small prompt for a risky command. It shows the command and, for a write
/// or edit, the change itself. The border pulses on the idle ticker so the
/// dialog pulls the eye.
fn draw_approval(
    frame: &mut Frame,
    approval: &mut ToolApproval,
    side_by_side: bool,
    spinner: usize,
    area: Rect,
) {
    let width = area.width.saturating_sub(4).min(72);
    if width < 30 || area.height < 8 {
        return;
    }
    let content_width = usize::from(width.saturating_sub(6));
    let lines = approval_lines(approval, content_width, side_by_side);
    let height = (lines.len() as u16 + 4).clamp(7, area.height.saturating_sub(2));
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) * 2 / 3,
        width,
        height,
    );
    frame.render_widget(Clear, popup);

    let blink = (spinner / 5) % 2 == 0;
    let border = Style::default()
        .fg(ACCENT)
        .add_modifier(if blink {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let has_command = tools::command_text(&approval.call).is_some();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(border)
        .title_top(Line::from(vec![Span::styled(
            if has_command {
                " run this command? "
            } else {
                " apply this change? "
            },
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
    call_name: Option<&str>,
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
    let mut label: Option<String> = None;
    let mut quiet = false;
    // A read stays quiet: its header and range replace the contents, and
    // clicking the frame still reveals everything it returned.
    if !failed && call_name == Some("read_file") {
        if let Some((path, summary)) = read_result_summary(&message.content) {
            quiet = true;
            label = Some(path);
            for line in summary {
                body.push(Line::styled(
                    truncate(&line, inner_width),
                    Style::default().fg(TEXT),
                ));
            }
        }
    }
    if !quiet {
        for content_line in &rendered {
            body.push(Line::styled(
                truncate(content_line, inner_width),
                Style::default().fg(color),
            ));
        }
    }
    if let Some(report) = report {
        body.extend(change_lines(report, inner_width));
    }
    if body.is_empty() {
        body.push(Line::styled(" ", Style::default().fg(MUTED)));
    }
    // Long output, diff included, is folded; a click on the frame reveals the rest.
    let expanded = app.expanded.contains(&call_id);
    let clickable = if quiet {
        true
    } else {
        body.len() > TOOL_OUTPUT_PREVIEW_LINES
    };
    if !expanded {
        if quiet {
            body.push(Line::styled(
                truncate(
                    &format!(
                        "… {} more line{} · click to expand",
                        rendered.len(),
                        if rendered.len() == 1 { "" } else { "s" }
                    ),
                    inner_width,
                ),
                Style::default().fg(MUTED),
            ));
        } else if body.len() > TOOL_OUTPUT_PREVIEW_LINES {
            let total = body.len();
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
    } else if quiet {
        for content_line in &rendered {
            body.push(Line::styled(
                truncate(content_line, inner_width),
                Style::default().fg(TEXT),
            ));
        }
        body.push(Line::styled(
            truncate("· click to collapse", inner_width),
            Style::default().fg(MUTED),
        ));
    } else {
        body.push(Line::styled(
            truncate("· click to collapse", inner_width),
            Style::default().fg(MUTED),
        ));
    }

    let label = truncate(
        report
            .and_then(|report| report.diffs.first())
            .map(|diff| diff.path.clone())
            .or(label)
            .unwrap_or_else(|| "tool".to_owned())
            .as_str(),
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
    let head = "╭─ ".chars().count() + label.chars().count() + 1;
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

/// The diffs under a tool result: per file, a header then the change, with a
/// blank line between files.
fn change_lines(report: &ChangeReport, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for (index, diff) in report.diffs.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        lines.extend(change_file_lines(diff, width, report.side_by_side));
    }
    lines
}

fn change_file_lines(diff: &FileDiff, width: usize, side_by_side: bool) -> Vec<Line<'static>> {
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
    if side_by_side {
        lines.extend(side_by_side_lines(diff, width));
    } else {
        lines.extend(unified_lines(diff, width));
    }
    lines
}

fn approval_lines(approval: &ToolApproval, width: usize, side_by_side: bool) -> Vec<Line<'static>> {
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
    if shown == 0 && approval.diffs.is_empty() {
        lines.push(Line::styled("$", Style::default().fg(TEXT)));
    }
    for (index, diff) in approval.diffs.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let report = ChangeReport {
            diffs: vec![diff.clone()],
            side_by_side,
        };
        lines.append(&mut change_lines(&report, width));
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

fn draw_connect(frame: &mut Frame, app: &App, connect: &ConnectState, spinner: usize, area: Rect) {
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

    draw_connect_list(frame, app, connect, spinner, rows[2]);
    draw_connect_hint(frame, connect, rows[3]);
}

fn draw_connect_list(
    frame: &mut Frame,
    app: &App,
    connect: &ConnectState,
    spinner: usize,
    area: Rect,
) {
    if connect.loading {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    SPINNER[spinner % SPINNER.len()],
                    Style::default().fg(ACCENT),
                ),
                Span::styled(
                    connect.loading_detail().to_owned(),
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
            | ConnectStage::FasfasNotice
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
    // Preferred provider first, then the custom block, then models.dev.
    // Sections need a divider row each, skipped by the selection.
    let has_preferred = connect.stage == ConnectStage::Providers
        && items.first().is_some_and(|item| item.id == FASFAST_ID);
    let base = usize::from(has_preferred);
    let tail = &items[base..];
    let custom_count = tail
        .iter()
        .take_while(|item| item.id == CUSTOM_PROVIDER_ID || item.custom_id.is_some())
        .count();
    let preferred_divider = has_preferred && !tail.is_empty();
    let models_divider = custom_count > 0 && custom_count < tail.len();
    // Rows: Ok(index) renders item index, Err(label) renders a divider.
    let mut rows = Vec::with_capacity(items.len() + usize::from(preferred_divider) + usize::from(models_divider));
    for index in 0..items.len() {
        if preferred_divider && index == base {
            rows.push(Err(" preferred "));
        }
        if models_divider && index == base + custom_count {
            rows.push(Err(" models.dev "));
        }
        rows.push(Ok(index));
    }
    let mut extra = 0;
    if preferred_divider && selected >= base && selected > 0 {
        extra += 1;
    }
    if models_divider && selected >= base + custom_count {
        extra += 1;
    }
    let selected_row = selected + extra;
    let start_row = selected_row.saturating_sub(height.saturating_sub(1));
    let start = start_row.min(rows.len().saturating_sub(height));
    let title_width = usize::from(area.width).saturating_sub(30).max(10);

    for (offset, row) in rows.iter().skip(start).take(height).enumerate() {
        let rect = Rect::new(area.x, area.y + offset as u16, area.width, 1);
        let Ok(index) = *row else {
            let Err(label) = *row else { unreachable!() };
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
        ConnectStage::FasfasNotice => {
            "enter then paste your key · esc close".to_owned()
        }
        ConnectStage::FasfasKey => "enter load models · esc back".to_owned(),
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
        ConnectStage::CustomModels => "↑↓ choose · enter use · d delete · esc back".to_owned(),
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

/// Thinking above a reply: every dim line while streaming or expanded, and
/// after the reply a single summary that the expand set can open back up.
/// Lines of thinking shown while streaming; older lines collapse into a
/// summary so long reasoning does not re-wrap the whole transcript per frame.
const STREAMING_REASONING_LINES: usize = 24;

fn reasoning_lines(message: &Message, streaming: bool, expanded: bool) -> Vec<Line<'static>> {
    let lines: Vec<&str> = message
        .reasoning
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return Vec::new();
    }
    if streaming || expanded {
        let style = Style::default().fg(MUTED);
        let hidden = lines.len().saturating_sub(STREAMING_REASONING_LINES);
        let show_hidden = streaming && hidden > 0 && !expanded;
        let start = if show_hidden { hidden } else { 0 };
        let mut output: Vec<Line<'static>> = Vec::new();
        if show_hidden {
            output.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(MUTED)),
                Span::styled(
                    format!("… {hidden} earlier lines of thinking"),
                    style,
                ),
            ]));
        }
        for line in &lines[start..] {
            output.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(MUTED)),
                Span::styled((*line).to_owned(), style),
            ]));
        }
        return output;
    }
    let text = if lines.len() > 2 {
        format!("{} … {} lines of thinking", lines[0], lines.len())
    } else {
        lines.join(" ")
    };
    vec![Line::from(vec![
        Span::styled("▸ ", Style::default().fg(MUTED)),
        Span::styled(text, Style::default().fg(MUTED)),
    ])]
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
    let width = usize::from(width).max(1);
    text.split('\n').map(|line| wrapped_rows(line, width)).sum()
}

/// Rows ratatui's `WordWrapper { trim: false }` renders for one logical line.
///
/// A port of the width bookkeeping in ratatui 0.29 `process_input`: greedy word
/// wrap, a word longer than the width splits across rows. textwrap disagrees on
/// exactly those cases, and the click map must match what is drawn.
fn wrapped_rows(line: &str, width: usize) -> usize {
    let mut rows = 0usize;
    let mut line_width = 0usize;
    let mut word_width = 0usize;
    let mut whitespace_width = 0usize;
    let mut pending_whitespace: VecDeque<usize> = VecDeque::new();
    let mut non_whitespace_previous = false;
    let mut line_has_graphemes = false;
    let mut word_has_graphemes = false;

    for grapheme in line.graphemes(true) {
        let is_whitespace = grapheme.chars().all(char::is_whitespace);
        let symbol_width = grapheme.width();
        if symbol_width > width {
            continue;
        }

        let word_found = non_whitespace_previous && is_whitespace;
        let untrimmed_overflow =
            !line_has_graphemes && word_width + whitespace_width + symbol_width > width;
        if word_found || untrimmed_overflow {
            line_width += whitespace_width + word_width;
            line_has_graphemes = true;
            whitespace_width = 0;
            word_width = 0;
            word_has_graphemes = false;
            pending_whitespace.clear();
        }

        let line_full = line_width >= width;
        let pending_word_overflow =
            symbol_width > 0 && line_width + whitespace_width + word_width >= width;
        if line_full || pending_word_overflow {
            rows += 1;
            let mut remaining = width.saturating_sub(line_width);
            line_width = 0;
            line_has_graphemes = false;
            while let Some(&first) = pending_whitespace.front() {
                if first > remaining {
                    break;
                }
                whitespace_width -= first;
                remaining -= first;
                pending_whitespace.pop_front();
            }
            if is_whitespace && pending_whitespace.is_empty() {
                continue;
            }
        }

        if is_whitespace {
            whitespace_width += symbol_width;
            pending_whitespace.push_back(symbol_width);
        } else {
            word_width += symbol_width;
            word_has_graphemes = true;
        }
        non_whitespace_previous = !is_whitespace;
    }

    if !line_has_graphemes && !word_has_graphemes && !pending_whitespace.is_empty() {
        rows += 1;
    }
    line_has_graphemes = line_has_graphemes || !pending_whitespace.is_empty() || word_has_graphemes;
    if line_has_graphemes {
        rows += 1;
    }
    rows.max(1)
}

#[cfg(test)]
mod tests {
    use super::{frame_lines, reasoning_lines, wrapped_line_count};
    use ratatui::{style::Color, text::Line};
    use crate::session::{Message, Role};

    fn text_of(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn approvals_render_the_diff_of_a_change() {
        use super::approval_lines;
        use crate::app::ToolApproval;
        use crate::diff::FileDiff;
        use crate::session::ToolCall;

        let call = ToolCall::new(
            "call_1",
            "write_file",
            r#"{"path":"hello.txt","content":"hi"}"#,
        );
        let approval = ToolApproval {
            call,
            note: Some("old_text was not found; the edit will fail.".to_owned()),
            diffs: vec![FileDiff::new("hello.txt", "", "hi\n", true)],
            scroll: 0,
            page_height: 0,
        };
        let lines = approval_lines(&approval, 60, false);
        let text = lines
            .iter()
            .map(text_of)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("hello.txt"), "{text}");
        assert!(text.contains("+1"), "{text}");
        assert!(!text.contains("$"), "{text}");
        assert!(text.contains("old_text was not found"), "{text}");
    }

    #[test]
    fn quiet_read_summaries_skip_the_contents() {
        use super::read_result_summary;
        let (path, summary) = read_result_summary(
            "src/main.rs (1014 lines)\nlines 1-400 of 1014\nfn main() {\n    ...",
        )
        .unwrap();
        assert_eq!(path, "src/main.rs");
        assert_eq!(summary[0], "src/main.rs (1014 lines)");
        assert_eq!(summary[1], "lines 1-400 of 1014");
        assert_eq!(summary.len(), 2);

        // A full read keeps only the header line.
        let (path, summary) =
            read_result_summary("todo.md (12 lines)\n- [x] ready\n- [ ] go").unwrap();
        assert_eq!(path, "todo.md");
        assert_eq!(summary.len(), 1);

        // Directory listings and unplanned shapes stay visible in full.
        assert!(read_result_summary("./\nnote.txt  2 B\nnested/\n").is_none());
        assert!(read_result_summary("src/missing.txt (10 lines) nope").is_none());
    }

    #[test]
    fn streaming_reasoning_shows_recent_lines_with_a_summary() {
        let mut message = Message::new(Role::Assistant, "");
        message.reasoning = (0..40).map(|i| format!("thought {i}")).collect::<Vec<_>>().join("\n");
        let lines = reasoning_lines(&message, true, false);
        assert_eq!(lines.len(), 25);
        assert_eq!(text_of(&lines[0]), "│ … 16 earlier lines of thinking");
        assert_eq!(text_of(&lines[1]), "│ thought 16");
        assert_eq!(text_of(&lines[24]), "│ thought 39");
        // Expanding keeps every line.
        assert_eq!(reasoning_lines(&message, true, true).len(), 40);
        assert_eq!(reasoning_lines(&message, false, false).len(), 1);
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
