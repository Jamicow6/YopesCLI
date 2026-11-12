//! Inline Markdown parsing: emphasis, code spans, links, and escapes.

use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

use super::CODE;
use super::{ACCENT, MUTED};

pub(super) fn inline_spans(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        let Some(first) = rest.chars().next() else {
            break;
        };

        if first == '\\' {
            let mut chars = rest.chars();
            chars.next();
            if let Some(next) = chars.next().filter(|next| is_escapable(*next)) {
                plain.push(next);
                index += 1 + next.len_utf8();
                continue;
            }
            plain.push('\\');
            index += 1;
            continue;
        }

        if first == '`' {
            if let Some((content, consumed)) = code_span(rest) {
                flush_plain(&mut spans, &mut plain, base);
                spans.push(Span::styled(content, base.fg(CODE)));
                index += consumed;
                continue;
            }
        }

        if rest.starts_with("![") {
            if let Some((label, _, consumed)) = parse_link(&rest[1..]) {
                flush_plain(&mut spans, &mut plain, base);
                let label = if label.is_empty() {
                    "image"
                } else {
                    label.as_str()
                };
                spans.extend(inline_spans(label, Style::default().fg(MUTED)));
                index += consumed + 1;
                continue;
            }
        }

        if first == '[' {
            if let Some((label, url, consumed)) = parse_link(rest) {
                flush_plain(&mut spans, &mut plain, base);
                spans.extend(link_spans(&label, &url));
                index += consumed;
                continue;
            }
        }

        if first == '<' {
            if let Some(end) = rest.find('>') {
                let target = &rest[1..end];
                if is_url(target) {
                    flush_plain(&mut spans, &mut plain, base);
                    spans.push(Span::styled(target.to_owned(), link_style()));
                    index += end + 1;
                    continue;
                }
            }
        }

        let mut matched = false;
        for (marker, modifier) in [
            ("***", Modifier::BOLD | Modifier::ITALIC),
            ("___", Modifier::BOLD | Modifier::ITALIC),
            ("**", Modifier::BOLD),
            ("__", Modifier::BOLD),
            ("~~", Modifier::CROSSED_OUT),
            ("*", Modifier::ITALIC),
            ("_", Modifier::ITALIC),
        ] {
            if !rest.starts_with(marker) {
                continue;
            }
            if !emphasis_opens(text, index, marker.len()) {
                continue;
            }
            if marker.starts_with('_') && !underscore_opens(text, index, marker.len()) {
                continue;
            }
            let after = &rest[marker.len()..];
            let Some(end) = closing_marker(after, marker) else {
                continue;
            };
            if end == 0 || (marker.starts_with('_') && !underscore_closes(after, end, marker.len()))
            {
                continue;
            }
            flush_plain(&mut spans, &mut plain, base);
            spans.extend(inline_spans(&after[..end], base.add_modifier(modifier)));
            index += marker.len() + end + marker.len();
            matched = true;
            break;
        }
        if matched {
            continue;
        }

        plain.push(first);
        index += first.len_utf8();
    }
    flush_plain(&mut spans, &mut plain, base);
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn flush_plain(spans: &mut Vec<Span<'static>>, plain: &mut String, style: Style) {
    if !plain.is_empty() {
        spans.push(Span::styled(std::mem::take(plain), style));
    }
}

fn is_escapable(ch: char) -> bool {
    matches!(
        ch,
        '\\' | '`'
            | '*'
            | '_'
            | '{'
            | '}'
            | '['
            | ']'
            | '('
            | ')'
            | '#'
            | '+'
            | '-'
            | '.'
            | '!'
            | '|'
            | '>'
            | '~'
    )
}

fn code_span(rest: &str) -> Option<(String, usize)> {
    let ticks = rest.chars().take_while(|ch| *ch == '`').count();
    let after = &rest[ticks..];
    let closing = "`".repeat(ticks);
    let mut search = 0;
    while let Some(found) = after[search..].find(closing.as_str()) {
        let at = search + found;
        let run = after[at..].chars().take_while(|ch| *ch == '`').count();
        if run == ticks {
            let mut content = &after[..at];
            if content.len() >= 2
                && content.starts_with(' ')
                && content.ends_with(' ')
                && !content.trim().is_empty()
            {
                content = &content[1..content.len() - 1];
            }
            return Some((content.to_owned(), ticks + at + ticks));
        }
        search = at + run;
    }
    None
}

fn parse_link(text: &str) -> Option<(String, String, usize)> {
    let body = text.strip_prefix('[')?;
    let closing = body.find(']')?;
    let label = &body[..closing];
    let target = body[closing + 1..].strip_prefix('(')?;
    let end = target.find(')')?;
    let url = target[..end].split_whitespace().next().unwrap_or("");
    Some((label.to_owned(), url.to_owned(), closing + end + 4))
}

fn link_style() -> Style {
    Style::default()
        .fg(ACCENT)
        .add_modifier(Modifier::UNDERLINED)
}

fn link_spans(label: &str, url: &str) -> Vec<Span<'static>> {
    let display = if label.is_empty() { url } else { label };
    let mut spans = inline_spans(display, link_style());
    if !url.is_empty() && url != display {
        spans.push(Span::styled(
            format!(" ({url})"),
            Style::default().fg(MUTED),
        ));
    }
    spans
}

fn is_url(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://") || text.starts_with("mailto:")
}

fn emphasis_opens(text: &str, at: usize, length: usize) -> bool {
    text[at + length..]
        .chars()
        .next()
        .is_some_and(|ch| !ch.is_whitespace())
}

fn closing_marker(text: &str, marker: &str) -> Option<usize> {
    let marker_char = marker.chars().next()?;
    let mut search = 0;
    while search < text.len() {
        let rest = &text[search..];
        let ch = rest.chars().next()?;

        // Delimiters inside code spans are literal and cannot close emphasis.
        if ch == '`' {
            if let Some((_, consumed)) = code_span(rest) {
                search += consumed;
                continue;
            }
        }

        // Backslash-escaped delimiters are literal too.
        if ch == '\\' {
            search += ch.len_utf8();
            if let Some(next) = text[search..].chars().next() {
                search += next.len_utf8();
            }
            continue;
        }

        if rest.starts_with(marker) && !is_escaped(text, search) {
            let before = text[..search].chars().next_back();
            let after = text[search + marker.len()..].chars().next();
            let not_adjacent =
                marker.len() != 1 || (before != Some(marker_char) && after != Some(marker_char));
            if not_adjacent && before.is_some_and(|ch| !ch.is_whitespace()) {
                return Some(search);
            }
        }
        search += ch.len_utf8();
    }
    None
}

fn is_escaped(text: &str, at: usize) -> bool {
    let backslashes = text[..at]
        .chars()
        .rev()
        .take_while(|ch| *ch == '\\')
        .count();
    (backslashes & 1) == 1
}

fn underscore_opens(text: &str, at: usize, length: usize) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + length..].chars().next();
    !before.is_some_and(char::is_alphanumeric) && after.is_some_and(|ch| !ch.is_whitespace())
}

fn underscore_closes(text: &str, at: usize, length: usize) -> bool {
    let before = text[..at].chars().next_back();
    let after = text[at + length..].chars().next();
    before.is_some_and(|ch| !ch.is_whitespace()) && !after.is_some_and(char::is_alphanumeric)
}
