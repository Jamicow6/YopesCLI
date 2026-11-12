use super::*;

/// A small prompt for a risky command. It shows the command and, for a write
/// or edit, the change itself. The border pulses on the idle ticker so the
/// dialog pulls the eye.
pub(super) fn draw_approval(
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
    let border = Style::default().fg(ACCENT).add_modifier(if blink {
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
pub(super) fn tool_result_lines(
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
pub(super) fn frame_lines(
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
    let head = UnicodeWidthStr::width("╭─ ") + UnicodeWidthStr::width(label.as_str()) + 1;
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
        let used = UnicodeWidthStr::width(clipped.as_str());
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
pub(super) fn change_lines(report: &ChangeReport, width: usize) -> Vec<Line<'static>> {
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

pub(super) fn change_file_lines(
    diff: &FileDiff,
    width: usize,
    side_by_side: bool,
) -> Vec<Line<'static>> {
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

pub(super) fn approval_lines(
    approval: &ToolApproval,
    width: usize,
    side_by_side: bool,
) -> Vec<Line<'static>> {
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

pub(super) fn unified_lines(diff: &FileDiff, width: usize) -> Vec<Line<'static>> {
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

pub(super) fn side_by_side_lines(diff: &FileDiff, width: usize) -> Vec<Line<'static>> {
    let gutter = line_number_width(diff);
    // Each pane gets an explicit +/- marker as well as its line number,
    // so side-by-side edits remain obvious even when terminal colors are subtle.
    let text_width = width.saturating_sub(gutter * 2 + 7).max(1) / 2;
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
                .map_or_else(|| Style::default().fg(TEXT), diff_style);
            let right_style = row
                .right
                .as_ref()
                .map_or_else(|| Style::default().fg(TEXT), diff_style);
            let (left_sign, left_color) = row
                .left
                .as_ref()
                .map_or((" ", TEXT), |line| diff_sign(line.kind));
            let (right_sign, right_color) = row
                .right
                .as_ref()
                .map_or((" ", TEXT), |line| diff_sign(line.kind));
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{left_number:>gutter$} "),
                    Style::default().fg(MUTED),
                ),
                Span::styled(left_sign, Style::default().fg(left_color)),
                Span::styled(format!("{left_text:<text_width$}"), left_style),
                Span::styled(" │ ", Style::default().fg(MUTED)),
                Span::styled(
                    format!("{right_number:>gutter$} "),
                    Style::default().fg(MUTED),
                ),
                Span::styled(right_sign, Style::default().fg(right_color)),
                Span::styled(right_text, right_style),
            ]));
        }
    }
    lines
}

pub(super) fn line_number_width(diff: &FileDiff) -> usize {
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

pub(super) fn diff_sign(kind: ChangeKind) -> (&'static str, Color) {
    match kind {
        ChangeKind::Context => (" ", TEXT),
        ChangeKind::Insert => ("+", ADDED),
        ChangeKind::Delete => ("-", REMOVED),
    }
}

pub(super) fn diff_style(line: &DiffLine) -> Style {
    let (_, color) = diff_sign(line.kind);
    Style::default().fg(color)
}
