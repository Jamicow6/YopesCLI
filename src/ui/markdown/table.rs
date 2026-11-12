//! Table rendering for Markdown: alignment, widths, and the box border.

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use super::super::truncate;
use super::Fence;
use super::inline::inline_spans;
use super::{MUTED, TEXT};

/// Tables are capped so one long cell cannot crowd out every other column.
const MAX_TABLE_CELL: usize = 40;
#[derive(Clone, Copy)]
enum CellAlign {
    Left,
    Center,
    Right,
}

pub(super) fn is_table_start(source: &[String], index: usize) -> bool {
    index + 1 < source.len()
        && source[index].contains('|')
        && delimiter_row(&source[index + 1]).is_some()
}

fn delimiter_row(line: &str) -> Option<Vec<CellAlign>> {
    if !line.contains('|') {
        return None;
    }
    let cells = split_cells(line);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        let trimmed = cell.trim();
        if trimmed.is_empty() {
            return None;
        }
        let left = trimmed.starts_with(':');
        let right = trimmed.ends_with(':');
        let inner = trimmed.trim_matches(':');
        if inner.is_empty() || !inner.chars().all(|ch| ch == '-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => CellAlign::Center,
            (false, true) => CellAlign::Right,
            _ => CellAlign::Left,
        });
    }
    Some(aligns)
}

fn split_cells(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in line.trim().chars() {
        if escaped {
            // A backslash only escapes a table delimiter here. Preserve other
            // Markdown escapes for the inline renderer to interpret later.
            if ch != '|' {
                current.push('\\');
            }
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            cells.push(current.trim().to_owned());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    if escaped {
        current.push('\\');
    }
    cells.push(current.trim().to_owned());
    if cells.first().is_some_and(|cell| cell.is_empty()) {
        cells.remove(0);
    }
    if cells.last().is_some_and(|cell| cell.is_empty()) {
        cells.pop();
    }
    cells
}

pub(super) fn render_table(
    source: &[String],
    index: usize,
    width: u16,
    output: &mut Vec<Line<'static>>,
) -> usize {
    let Some(mut aligns) = delimiter_row(&source[index + 1]) else {
        return index + 1;
    };
    let columns = aligns.len().max(split_cells(&source[index]).len()).max(1);
    aligns.resize(columns, CellAlign::Left);

    let mut header = split_cells(&source[index]);
    header.resize(columns, String::new());
    let mut rows = vec![header];
    let mut cursor = index + 2;
    while cursor < source.len() {
        let line = &source[cursor];
        if line.trim().is_empty() || !line.contains('|') || Fence::parse(line).is_some() {
            break;
        }
        let mut cells = split_cells(line);
        cells.truncate(columns);
        cells.resize(columns, String::new());
        rows.push(cells);
        cursor += 1;
    }

    let widths = table_widths(&rows, width);
    output.push(table_border('┌', '┬', '┐', &widths));
    output.push(table_row(&rows[0], &widths, &aligns, true));
    output.push(table_border('├', '┼', '┤', &widths));
    for row in &rows[1..] {
        output.push(table_row(row, &widths, &aligns, false));
    }
    output.push(table_border('└', '┴', '┘', &widths));
    cursor
}

fn table_widths(rows: &[Vec<String>], width: u16) -> Vec<usize> {
    let columns = rows.first().map_or(1, |row| row.len());
    let mut widths = vec![3; columns];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            let measured = UnicodeWidthStr::width(cell.as_str()).min(MAX_TABLE_CELL);
            widths[column] = widths[column].max(measured);
        }
    }
    let budget = usize::from(width).max(8);
    let mut total = widths.iter().sum::<usize>() + 3 * columns + 1;
    while total > budget {
        let mut widest = 0;
        for (column, column_width) in widths.iter().enumerate() {
            if *column_width > widths[widest] {
                widest = column;
            }
        }
        if widths[widest] <= 1 {
            break;
        }
        widths[widest] -= 1;
        total -= 1;
    }
    widths
}

fn table_border(left: char, middle: char, right: char, widths: &[usize]) -> Line<'static> {
    let mut text = String::new();
    text.push(left);
    for (index, width) in widths.iter().enumerate() {
        text.push_str(&"─".repeat(width + 2));
        text.push(if index + 1 == widths.len() {
            right
        } else {
            middle
        });
    }
    Line::styled(text, Style::default().fg(MUTED))
}

fn table_row(
    cells: &[String],
    widths: &[usize],
    aligns: &[CellAlign],
    header: bool,
) -> Line<'static> {
    let base = if header {
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT)
    };
    let mut spans = vec![Span::styled("│", Style::default().fg(MUTED))];
    for (index, width) in widths.iter().enumerate() {
        let cell = cells.get(index).map_or("", |cell| cell.as_str());
        let clipped = truncate(cell, *width);
        let cell_spans = inline_spans(&clipped, base);
        let used = spans_width(&cell_spans);
        let padding = width.saturating_sub(used);
        let (before, after) = match aligns.get(index).copied().unwrap_or(CellAlign::Left) {
            CellAlign::Left => (0, padding),
            CellAlign::Center => (padding / 2, padding - padding / 2),
            CellAlign::Right => (padding, 0),
        };
        spans.push(Span::styled(
            " ".repeat(before + 1),
            Style::default().fg(MUTED),
        ));
        spans.extend(cell_spans);
        spans.push(Span::styled(
            format!("{}│", " ".repeat(after + 1)),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum()
}
