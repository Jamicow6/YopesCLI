//! Block and inline Markdown rendering for the transcript.
//!
//! Messages are rendered whole so fenced code, lists, and tables can span
//! lines. The transcript paragraph still owns wrapping at the terminal width;
//! this module only decides structure and style.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::{ACCENT, MUTED, TEXT, normalize_line};

/// Code stays distinct from inline accents without competing with diff colours.
pub(super) const CODE: Color = Color::LightCyan;
mod inline;
mod table;

use self::inline::inline_spans;
use self::table::{is_table_start, render_table};
pub(super) fn markdown_lines(markdown: &str, width: u16) -> Vec<Line<'static>> {
    let source = markdown.split('\n').map(normalize_line).collect::<Vec<_>>();
    let mut output = Vec::new();
    let mut index = 0;
    while index < source.len() {
        if let Some((fence, info)) = Fence::parse(&source[index]) {
            index = render_code(&source, index, fence, info, width, &mut output);
            continue;
        }
        if is_table_start(&source, index) {
            index = render_table(&source, index, width, &mut output);
            continue;
        }

        let line = &source[index];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            output.push(Line::raw(""));
            index += 1;
            continue;
        }
        if is_rule(trimmed) {
            output.push(rule_line(width));
            index += 1;
            continue;
        }
        if let Some((level, title)) = heading(trimmed) {
            output.push(heading_line(level, title));
            index += 1;
            continue;
        }
        if let Some((depth, text)) = quote(line) {
            output.push(quote_line(depth, text));
            index += 1;
            continue;
        }
        if let Some(item) = list_line(line) {
            output.push(item);
            index += 1;
            continue;
        }
        // A line followed by `===` or `---` is a setext heading, unless the
        // line already opened another block (list, quote, heading, fence).
        if let Some(level) = source
            .get(index + 1)
            .and_then(|next| setext_underline(next))
        {
            if !looks_like_block(trimmed) {
                output.push(heading_line(level, trimmed));
                index += 2;
                continue;
            }
        }

        output.push(Line::from(inline_spans(line, Style::default().fg(TEXT))));
        index += 1;
    }
    output
}

#[derive(Clone, Copy)]
pub(super) struct Fence {
    mark: char,
    length: usize,
}

impl Fence {
    pub(super) fn parse(line: &str) -> Option<(Self, &str)> {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            return None;
        }
        let mark = trimmed.chars().next()?;
        if !matches!(mark, '`' | '~') {
            return None;
        }
        let length = trimmed.chars().take_while(|ch| *ch == mark).count();
        if length < 3 {
            return None;
        }
        let info = trimmed[length..].trim();
        if mark == '`' && info.contains('`') {
            return None;
        }
        Some((Self { mark, length }, info))
    }

    pub(super) fn closes(self, line: &str) -> bool {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() > 3 {
            return false;
        }
        let length = trimmed.chars().take_while(|ch| *ch == self.mark).count();
        length >= self.length && trimmed[length..].trim().is_empty()
    }
}

fn render_code(
    source: &[String],
    index: usize,
    fence: Fence,
    info: &str,
    width: u16,
    output: &mut Vec<Line<'static>>,
) -> usize {
    output.push(code_open(info, width));
    let mut cursor = index + 1;
    while cursor < source.len() {
        if fence.closes(&source[cursor]) {
            output.push(code_close(width));
            return cursor + 1;
        }
        output.push(Line::from(vec![
            Span::styled("│ ", Style::default().fg(MUTED)),
            Span::styled(source[cursor].clone(), Style::default().fg(CODE)),
        ]));
        cursor += 1;
    }
    // An open fence stays open while streaming; closing it keeps the
    // transcript readable without a dangling backtick row.
    output.push(code_close(width));
    cursor
}

fn code_open(info: &str, width: u16) -> Line<'static> {
    let label = info.split_whitespace().next().unwrap_or("code");
    let room = usize::from(width).saturating_sub(5);
    let label = super::truncate(label, room);
    border_line(&format!("╭─ {label} "), width)
}

fn code_close(width: u16) -> Line<'static> {
    border_line("╰─", width)
}

fn border_line(prefix: &str, width: u16) -> Line<'static> {
    let total = usize::from(width).max(1);
    let mut text = super::truncate(prefix, total);
    let used = UnicodeWidthStr::width(text.as_str());
    text.push_str(&"─".repeat(total.saturating_sub(used)));
    Line::styled(text, Style::default().fg(MUTED))
}

fn rule_line(width: u16) -> Line<'static> {
    Line::styled(
        "─".repeat(usize::from(width).max(1)),
        Style::default().fg(MUTED),
    )
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let title = line.get(level..)?.strip_prefix(' ')?.trim();
    let trailing_hashes = title.chars().rev().take_while(|ch| *ch == '#').count();
    let title = if trailing_hashes > 0 {
        let before_hashes = &title[..title.len() - trailing_hashes];
        if before_hashes
            .chars()
            .next_back()
            .is_some_and(char::is_whitespace)
        {
            before_hashes.trim_end()
        } else {
            title
        }
    } else {
        title
    };
    if title.is_empty() {
        return None;
    }
    Some((level, title))
}

fn heading_line(level: usize, title: &str) -> Line<'static> {
    let style = if level <= 2 {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![Span::styled(
        format!("{} ", "▰".repeat(level.min(3))),
        Style::default().fg(ACCENT),
    )];
    spans.extend(inline_spans(title, style));
    Line::from(spans)
}

fn setext_underline(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    let mark = trimmed.chars().next()?;
    if !matches!(mark, '=' | '-') {
        return None;
    }
    if !trimmed.chars().all(|ch| ch == mark) {
        return None;
    }
    Some(if mark == '=' { 1 } else { 2 })
}

fn looks_like_block(line: &str) -> bool {
    heading(line).is_some()
        || is_rule(line)
        || quote(line).is_some()
        || list_marker(line.trim_start()).is_some()
        || Fence::parse(line).is_some()
}

fn is_rule(line: &str) -> bool {
    let mut chars = line.chars();
    let Some(mark) = chars.next() else {
        return false;
    };
    if !matches!(mark, '-' | '*' | '_') {
        return false;
    }
    let mut marks = 1;
    for ch in chars {
        if ch == mark {
            marks += 1;
        } else if ch != ' ' {
            return false;
        }
    }
    marks >= 3
}

fn quote(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let mut rest = trimmed;
    let mut depth = 0;
    while depth < 6 {
        let Some(after) = rest.strip_prefix('>') else {
            break;
        };
        depth += 1;
        rest = after.strip_prefix(' ').unwrap_or(after);
    }
    (depth > 0).then_some((depth, rest))
}

fn quote_line(depth: usize, text: &str) -> Line<'static> {
    let mut spans = vec![Span::styled("│ ".repeat(depth), Style::default().fg(MUTED))];
    spans.extend(inline_spans(
        text,
        Style::default().fg(MUTED).add_modifier(Modifier::ITALIC),
    ));
    Line::from(spans)
}

enum ListKind {
    Bullet,
    Ordered(String),
}

fn list_marker(line: &str) -> Option<(ListKind, &str)> {
    for mark in ['-', '*', '+'] {
        if let Some(rest) = line
            .strip_prefix(mark)
            .and_then(|rest| rest.strip_prefix(' '))
        {
            return Some((ListKind::Bullet, rest));
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = &line[digits..];
    let mut chars = rest.chars();
    let separator = chars.next()?;
    if !matches!(separator, '.' | ')') {
        return None;
    }
    if chars.next() != Some(' ') {
        return None;
    }
    let number = &line[..digits];
    Some((
        ListKind::Ordered(format!("{number}{separator} ")),
        &rest[2..],
    ))
}

fn task_box(rest: &str) -> (Option<bool>, &str) {
    if let Some(content) = rest
        .strip_prefix("[x] ")
        .or_else(|| rest.strip_prefix("[X] "))
    {
        return (Some(true), content);
    }
    if let Some(content) = rest.strip_prefix("[ ] ") {
        return (Some(false), content);
    }
    (None, rest)
}

fn list_line(line: &str) -> Option<Line<'static>> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    let body = &line[indent..];
    let (kind, rest) = list_marker(body)?;
    let (task, content) = task_box(rest);

    let (marker, marker_style, content_style) = match (task, kind) {
        (Some(checked), ListKind::Bullet) => {
            let style = if checked {
                Style::default().fg(MUTED)
            } else {
                Style::default().fg(ACCENT)
            };
            let text = if checked { "[x] " } else { "[ ] " };
            let content = if checked {
                Style::default().fg(MUTED)
            } else {
                Style::default().fg(TEXT)
            };
            (text.to_owned(), style, content)
        }
        (Some(checked), ListKind::Ordered(number)) => {
            let style = if checked {
                Style::default().fg(MUTED)
            } else {
                Style::default().fg(ACCENT)
            };
            let checkbox = if checked { "[x] " } else { "[ ] " };
            let content = if checked {
                Style::default().fg(MUTED)
            } else {
                Style::default().fg(TEXT)
            };
            (format!("{number}{checkbox}"), style, content)
        }
        (None, ListKind::Bullet) => {
            let symbol = match indent / 2 {
                0 => "• ",
                1 => "◦ ",
                _ => "▪ ",
            };
            (
                symbol.to_owned(),
                Style::default().fg(ACCENT),
                Style::default().fg(TEXT),
            )
        }
        (None, ListKind::Ordered(number)) => (
            number,
            Style::default().fg(ACCENT),
            Style::default().fg(TEXT),
        ),
    };

    let mut spans = vec![Span::styled(
        format!("{}{marker}", " ".repeat(indent)),
        marker_style,
    )];
    spans.extend(inline_spans(content, content_style));
    Some(Line::from(spans))
}
#[cfg(test)]
mod tests {
    use super::{inline_spans, markdown_lines};
    use ratatui::{style::Modifier, text::Line};

    fn text_of(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn render(markdown: &str) -> Vec<String> {
        markdown_lines(markdown, 40).iter().map(text_of).collect()
    }

    #[test]
    fn inline_markup_loses_its_markers_but_keeps_its_style() {
        let lines = markdown_lines("Use **bold**, *italic*, and `code`.", 60);
        let line = &lines[0];
        let text = text_of(line);
        assert_eq!(text, "Use bold, italic, and code.");
        assert!(line.spans.iter().any(|span| {
            span.content.as_ref() == "bold" && span.style.add_modifier.contains(Modifier::BOLD)
        }));
        assert!(line.spans.iter().any(|span| {
            span.content.as_ref() == "italic" && span.style.add_modifier.contains(Modifier::ITALIC)
        }));
        assert!(
            line.spans
                .iter()
                .any(|span| span.content.as_ref() == "code")
        );
    }

    #[test]
    fn underscores_inside_identifiers_are_left_alone() {
        assert_eq!(render("snake_case_name")[0], "snake_case_name");
        assert_eq!(render("_italic_ text")[0], "italic text");
    }

    #[test]
    fn links_show_the_label_and_the_destination() {
        let text = render("See [the docs](https://example.com/docs).")[0].clone();
        assert_eq!(text, "See the docs (https://example.com/docs).");
    }

    #[test]
    fn escapes_and_strikethrough_are_handled() {
        assert_eq!(render("a \\* b ~~old~~")[0], "a * b old");
        let lines = markdown_lines("~~old~~", 20);
        let line = &lines[0];
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.add_modifier.contains(Modifier::CROSSED_OUT))
        );
    }

    #[test]
    fn headings_lists_and_quotes_keep_their_structure() {
        let lines = render("# Title\n\n- one\n- two\n\n1. first\n2. second\n\n> quoted");
        assert!(lines.iter().any(|line| line.contains("Title")));
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("• one"))
        );
        assert!(lines.iter().any(|line| line.contains("1. first")));
        assert!(lines.iter().any(|line| line.contains("2. second")));
        assert!(
            lines
                .iter()
                .any(|line| line.trim_start().starts_with("│ quoted"))
        );
    }

    #[test]
    fn task_lists_render_checkboxes() {
        let lines = render("- [x] done\n- [ ] open");
        assert!(lines.iter().any(|line| line.contains("[x] done")));
        assert!(lines.iter().any(|line| line.contains("[ ] open")));
    }

    #[test]
    fn fenced_code_keeps_its_body_and_names_the_language() {
        let lines = render("```rust\nfn main() {}\n```");
        assert!(lines[0].starts_with("╭─ rust "), "{:?}", lines[0]);
        assert_eq!(lines[1], "│ fn main() {}");
        assert!(lines[2].starts_with("╰─"), "{:?}", lines[2]);
    }

    #[test]
    fn an_open_fence_still_closes_cleanly() {
        let lines = render("```\nstreaming...");
        assert_eq!(lines[1], "│ streaming...");
        assert!(lines[2].starts_with("╰─"));
    }

    #[test]
    fn tables_draw_a_box_and_keep_their_cells() {
        let lines = render("| Name | Age |\n|:-----|----:|\n| Ada  |  36 |");
        assert!(lines[0].starts_with('┌'), "{:?}", lines[0]);
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Name") && line.contains("Age"))
        );
        assert!(
            lines
                .iter()
                .any(|line| line.contains("Ada") && line.contains("36"))
        );
        assert!(lines.last().unwrap().starts_with('└'));
    }

    #[test]
    fn table_inline_markup_stays_aligned() {
        let lines = render("| **Name** | Age |\n|---|---|\n| Ada | 36 |");
        let widths = lines
            .iter()
            .map(|line| unicode_width::UnicodeWidthStr::width(line.as_str()))
            .collect::<Vec<_>>();
        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "{lines:?}"
        );
    }

    #[test]
    fn tables_fit_the_requested_width() {
        let lines = markdown_lines("| a | b |\n|---|---|\n| one | two |", 20);
        for line in &lines {
            assert!(
                unicode_width::UnicodeWidthStr::width(text_of(line).as_str()) <= 20,
                "{:?}",
                text_of(line)
            );
        }
    }

    #[test]
    fn plain_text_keeps_its_spacing() {
        assert_eq!(render("a  b\n  c")[0], "a  b");
        assert_eq!(render("a  b\n  c")[1], "  c");
    }

    #[test]
    fn inline_parser_never_panics_on_unclosed_markers() {
        for text in ["**", "`", "_", "~~", "***", "[x](", "![a](", "\\"] {
            let spans = inline_spans(text, ratatui::style::Style::default());
            assert!(!spans.is_empty(), "{text:?}");
        }
    }
    #[test]
    fn escaped_pipes_inside_code_spans_stay_in_one_table_cell() {
        let lines = render("| expression | meaning |\n|---|---|\n| `a\\|b` | literal pipe |");
        assert!(lines.iter().any(|line| line.contains("a|b")));
        assert!(!lines.iter().any(|line| line.contains("\\|")));
    }

    #[test]
    fn headings_preserve_language_names_ending_in_hashes() {
        let lines = render("# C#\n\n# Closing marker ##");
        assert!(lines.iter().any(|line| line.contains("C#")));
        assert!(lines.iter().any(|line| line.contains("Closing marker")));
        assert!(!lines.iter().any(|line| line.contains("Closing marker ##")));
    }

    #[test]
    fn ordered_task_lists_keep_their_numbers() {
        let lines = render("3. [ ] first\\n4. [x] second");
        assert!(lines.iter().any(|line| line.contains("3. [ ] first")));
        assert!(lines.iter().any(|line| line.contains("4. [x] second")));
    }

    #[test]
    fn emphasis_does_not_consume_whitespace_separated_asterisks() {
        assert_eq!(render("a * b * c")[0], "a * b * c");
    }

    #[test]
    fn emphasis_closers_inside_code_spans_are_ignored() {
        let lines = markdown_lines("*Use `*` literally*", 40);
        assert_eq!(text_of(&lines[0]), "Use * literally");
        assert!(lines[0].spans.iter().any(|span| {
            span.content.as_ref() == "*" && span.style.add_modifier.contains(Modifier::ITALIC)
        }));
    }
}
