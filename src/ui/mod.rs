use std::{collections::VecDeque, path::PathBuf, time::SystemTime};

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app::{
        App, COMMANDS, CUSTOM_PROVIDER_ID, ChangeReport, CommandSpec, ConnectStage, ConnectState,
        Input, ListPicker, Mode, ModelRow, NoticeKind, ToolApproval, ToolsMode, format_bytes,
        format_tokens,
    },
    diff::{ChangeKind, DiffLine, FileDiff},
    session::{Message, Role, Usage},
    tools,
};

mod approval;
mod composer;
mod connect;
mod help;
mod markdown;
mod pickers;
mod transcript;

use self::{approval::*, composer::*, connect::*, help::*, pickers::*, transcript::*};

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
    if let Some(range) = result
        .lines()
        .nth(1)
        .filter(|line| line.starts_with("lines "))
    {
        summary.push(range.trim().to_owned());
    }
    Some((path.to_owned(), summary))
}

/// Tools whose call and result stay out of the transcript: their effect is
/// already visible in the Progress panel.
fn is_hidden_tool(name: &str) -> bool {
    matches!(name, "todo_write")
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
const YELLOW: Color = Color::Yellow;
const TEXT: Color = Color::Reset;

pub(super) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Rows of command text shown in the approval prompt before it stops.
const MAX_APPROVAL_LINES: usize = 4;
/// The startup mascot: a small cat, drawn only on the welcome screen, with a
/// ball of yarn that rolls beneath it.
const CAT_ROWS: [&str; 6] = [
    "  /\\_/\\  ",
    "+ ( o.o ) +",
    "  #> ^ <#  ",
    "  /| . |\\  ",
    " (_|   |_)",
    "   \"   \"  ",
];
/// The tail flicks through these positions.
const TAILS: [&str; 4] = ["", "\\", "\\\\", "\\~"];
/// The yarn ball spins while rolling.
const BALL_FRAMES: [&str; 4] = ["(o)", "(@)", "(O)", "(@)"];
/// How wide the whole mascot is, so every row lines up.
const MASCOT_WIDTH: usize = 15;
/// The ball of yarn rolls this many steps out and back.
const YARN_TRAVEL: usize = 6;
/// Tool output rows shown before the rest is collapsed.
const TOOL_OUTPUT_PREVIEW_LINES: usize = 7;
// Custom loading circle instead of the generic braille spinner.
const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

pub(super) fn responsive_palette_width(area_width: u16) -> u16 {
    (area_width.saturating_sub(4) * 85 / 100)
        .clamp(60, 110)
        .min(area_width.saturating_sub(2))
}

/// One frame of the welcome mascot.
///
/// The cat tracks the rolling yarn, blinks, twitches its ears, paws the ball,
/// and snoozes during longer idle stretches.
fn mascot_frame(step: usize) -> Vec<Line<'static>> {
    // ponytail: procedural tick animation; add stateful sprite machine if interactive pet actions are needed.
    let mut rows: Vec<String> = CAT_ROWS.iter().map(|row| (*row).to_owned()).collect();
    let sleeping = step % 160 >= 135;

    if sleeping {
        let z = match (step / 4) % 3 {
            0 => "  /\\_/\\  z",
            1 => "  /\\_/\\  Z",
            _ => "  /\\_/\\ zZ",
        };
        rows[0] = z.to_owned();
        rows[1] = rows[1].replace("o.o", "-.-");
        rows[2] = rows[2].replace('^', "v");
        let mut yarn = String::from("(o)");
        yarn.push_str(&"~".repeat(MASCOT_WIDTH - 3));
        rows.push(yarn);
    } else {
        // Ear twitches and occasional mew.
        if step % 70 == 20 {
            rows[0] = "  --_/\\  ".to_owned();
        } else if step % 70 == 50 {
            rows[0] = "  /\\_--  ".to_owned();
        } else if step % 160 == 70 {
            rows[0] = "  /\\_/\\  *mew".to_owned();
        }

        // Yarn ball position and rolling direction.
        let travel = (step / 2) % (YARN_TRAVEL * 2);
        let (position, trailing_left) = if travel < YARN_TRAVEL {
            (travel, true)
        } else {
            (YARN_TRAVEL * 2 - travel, false)
        };

        // Eye tracking: follow the yarn, blink, or smile when batting.
        let eyes = if step % 40 >= 38 {
            "-.-"
        } else if position >= 5 && !trailing_left {
            "^.^"
        } else {
            match position {
                0..=1 => "<.<",
                2..=3 => "o.o",
                _ => ">.>",
            }
        };
        rows[1] = rows[1].replace("o.o", eyes);

        // Paw bats the yarn at the right turn.
        if position >= 5 && trailing_left {
            rows[4] = rows[4].replace("|_)", "/_)");
            rows[5] = "   \"    / ".to_owned();
        }

        // Tail flick.
        rows[4].push_str(TAILS[(step / 4) % TAILS.len()]);

        // Rolling yarn ball.
        let ball = BALL_FRAMES[(step / 2) % BALL_FRAMES.len()];
        let mut yarn = String::new();
        if trailing_left {
            yarn.push_str(&"~".repeat(position));
            yarn.push_str(ball);
            yarn.push_str(&" ".repeat(MASCOT_WIDTH - position - 3));
        } else {
            yarn.push_str(&" ".repeat(position));
            yarn.push_str(ball);
            yarn.push_str(&"~".repeat(MASCOT_WIDTH - position - 3));
        }
        rows.push(yarn);
    }

    rows.into_iter()
        .map(|row| {
            let mut row = row;
            row.push_str(&" ".repeat(MASCOT_WIDTH.saturating_sub(row.chars().count())));
            Line::styled(row, Style::default().fg(ACCENT))
        })
        .collect()
}

/// The streaming caret alternates on a ~300ms cadence driven by the 100ms
/// ticker, so a quiet stretch of tokens still reads as alive.
fn caret(spinner: usize) -> &'static str {
    if (spinner / 3) % 2 == 0 { "▌" } else { "" }
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
        (app.model_picker_count().clamp(1, 12) as u16).saturating_add(2)
    } else {
        0
    };
    let session_height = if session_picker.is_some() {
        (app.session_picker_count().clamp(1, 12) as u16).saturating_add(2)
    } else {
        0
    };
    let popup_height = palette_height + picker_height + session_height;
    let composer_height = composer_height(app, area.width);
    let progress_height = if tasks {
        if app.progress_collapsed {
            1
        } else {
            (app.session.tasks.len().min(6) as u16) + 2
        }
    } else {
        app.progress_row = None;
        0
    };

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
        draw_help(frame, app, area);
    } else if let Some(connect) = app.connect_state() {
        draw_connect(frame, app, connect, spinner, area);
    }
}

fn truncate(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= width {
        return text.to_owned();
    }
    let mut result = String::new();
    let mut used = 0usize;
    for grapheme in text.graphemes(true) {
        let cells = UnicodeWidthStr::width(grapheme);
        if used + cells + 1 > width {
            break;
        }
        result.push_str(grapheme);
        used += cells;
    }
    result.push('…');
    result
}

// ponytail: 4-space tab stops; add configurable tab size if user settings support it.
/// Tabs become spaces and carriage returns are stripped: ratatui has no tab stops
/// and raw `\r` desyncs cursor position in terminal cells.
pub(super) fn normalize_line(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut column = 0;
    for ch in line.chars() {
        if ch == '\r' {
            continue;
        }
        if ch == '\t' {
            let spaces = 4 - column % 4;
            output.push_str(&" ".repeat(spaces));
            column += spaces;
        } else {
            output.push(ch);
            column += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    output
}

/// Pad `text` with spaces to `width` display columns, so CJK and emoji
/// align like ASCII does.
#[cfg(test)]
pub(super) fn pad_to(text: &str, width: usize) -> String {
    let display = UnicodeWidthStr::width(text);
    if display >= width {
        text.to_owned()
    } else {
        format!("{text}{}", " ".repeat(width - display))
    }
}

/// Fit `text` to exactly `width` columns: truncates with ellipsis if longer,
/// pads with spaces if shorter.
pub(super) fn fit_to(text: &str, width: usize) -> String {
    let truncated = truncate(text, width);
    let display = UnicodeWidthStr::width(truncated.as_str());
    if display >= width {
        truncated
    } else {
        format!("{truncated}{}", " ".repeat(width - display))
    }
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
                Span::styled(format!("… {hidden} earlier lines of thinking"), style),
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
    use super::{frame_lines, pad_to, reasoning_lines, wrapped_line_count};
    use crate::session::{Message, Role};
    use ratatui::{style::Color, text::Line};
    use unicode_width::UnicodeWidthStr;

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
            popup_area: std::cell::Cell::new(ratatui::layout::Rect::default()),
        };
        let lines = approval_lines(&approval, 60, false);
        let text = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");
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
        message.reasoning = (0..40)
            .map(|i| format!("thought {i}"))
            .collect::<Vec<_>>()
            .join("\n");
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
    fn frame_lines_sanitizes_tabs_and_carriage_returns() {
        let body = vec![Line::raw("col\tnext\r"), Line::raw("\t\tindented")];
        let lines = frame_lines("tool".to_owned(), body, 20, Color::Reset);
        for line in &lines[1..] {
            let text = text_of(line);
            assert_eq!(UnicodeWidthStr::width(text.as_str()), 24, "{text:?}");
            assert!(!text.contains('\t'));
            assert!(!text.contains('\r'));
            assert!(text.ends_with('│') || text.ends_with('╮') || text.ends_with('╯'));
        }
    }

    #[test]
    fn frame_lines_preserves_span_styles_for_diff_colors() {
        use ratatui::style::Style;
        use ratatui::text::Span;
        let line = Line::from(vec![
            Span::styled("1 ", Style::default().fg(Color::DarkGray)),
            Span::styled("+ ", Style::default().fg(Color::Green)),
            Span::styled("new line", Style::default().fg(Color::Green)),
        ]);
        let lines = frame_lines("diff".to_owned(), vec![line], 30, Color::Reset);
        assert_eq!(lines.len(), 4);
        let body_row = &lines[2];
        assert_eq!(body_row.spans[1].style.fg, Some(Color::DarkGray));
        assert_eq!(body_row.spans[2].style.fg, Some(Color::Green));
        assert_eq!(body_row.spans[3].style.fg, Some(Color::Green));
    }

    #[test]
    fn a_label_wider_than_the_frame_does_not_break_it() {
        let lines = frame_lines("x".repeat(60), vec![Line::raw("body")], 10, Color::Reset);
        assert_eq!(text_of(&lines[1]).chars().count(), 14);
        assert_eq!(text_of(&lines[2]).chars().count(), 14);
    }

    #[test]
    fn padding_counts_display_columns_not_chars() {
        // A CJK char is two columns; char-count padding would under-pad it.
        assert_eq!(pad_to("a", 4), "a   ");
        assert_eq!(pad_to("汉", 4), "汉  ");
        assert_eq!(pad_to("汉汉", 4), "汉汉");
        assert_eq!(pad_to("too long here", 4), "too long here");
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
    fn the_mascot_animates_and_keeps_its_width() {
        use super::mascot_frame;

        let frames: Vec<Vec<String>> = (0..200)
            .map(|step| {
                mascot_frame(step)
                    .into_iter()
                    .map(|line| {
                        line.spans
                            .iter()
                            .map(|span| span.content.to_string())
                            .collect()
                    })
                    .collect()
            })
            .collect();
        for frame in &frames {
            // Every row is the same width, so the art stays aligned.
            let widths: Vec<usize> = frame.iter().map(|row| row.chars().count()).collect();
            assert!(
                widths.windows(2).all(|pair| pair[0] == pair[1]),
                "{widths:?}"
            );
            assert_eq!(frame.len(), 7, "the cat and the ball of yarn");
        }
        // The mascot animates across ticks.
        let first = &frames[0];
        let later = &frames[8];
        assert_ne!(first, later, "the mascot should move");
    }

    #[test]
    fn wrapped_counts_include_words_and_explicit_newlines() {
        assert_eq!(wrapped_line_count("", 10), 1);
        assert_eq!(wrapped_line_count("one\ntwo", 10), 2);
        assert_eq!(wrapped_line_count("aaaaaa aaaaaa aaaaaa", 10), 3);
    }
}
