use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    app::{App, NoticeKind},
    session::Role,
};

const BACKGROUND: Color = Color::Rgb(13, 16, 24);
const SURFACE: Color = Color::Rgb(20, 24, 35);
const BORDER: Color = Color::Rgb(48, 56, 75);
const TEXT: Color = Color::Rgb(218, 224, 235);
const MUTED: Color = Color::Rgb(125, 137, 158);
const ACCENT: Color = Color::Rgb(103, 232, 210);
const USER: Color = Color::Rgb(139, 170, 255);
const ERROR: Color = Color::Rgb(255, 118, 132);
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BACKGROUND)),
        area,
    );
    if area.width < 38 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("Terminal too small\n\nResize to at least 38 × 10")
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED).bg(BACKGROUND)),
            area,
        );
        return;
    }

    let composer_height = composer_height(app, area.width);
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(composer_height),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(frame, app, layout[0]);
    draw_transcript(frame, app, layout[1]);
    draw_status(frame, app, layout[2]);
    draw_composer(frame, app, layout[3]);
    draw_footer(frame, layout[4], area.width);

    if app.show_help {
        draw_help(frame, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(inner);
    let title = Line::from(vec![
        Span::styled(
            " YOPES ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled("/ ", Style::default().fg(MUTED)),
        Span::styled(&app.session.title, Style::default().fg(TEXT)),
    ]);
    frame.render_widget(Paragraph::new(title), columns[0]);

    let status = if app.is_streaming() {
        "STREAMING"
    } else {
        "READY"
    };
    let right = Line::from(vec![
        Span::styled(
            "● ",
            Style::default().fg(if app.is_streaming() { USER } else { ACCENT }),
        ),
        Span::styled(
            format!(
                "{} · {} · {} ",
                app.backend.provider().as_str(),
                app.backend.model(),
                status
            ),
            Style::default().fg(MUTED),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(right).alignment(Alignment::Right),
        columns[1],
    );
}

fn draw_transcript(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .style(Style::default().bg(BACKGROUND))
        .padding(Padding::new(2, 2, 1, 0));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    app.transcript_height = inner.height;
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.session.messages.is_empty() {
        let welcome = vec![
            Line::styled(
                "Y O P E S",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled("A session-first AI harness", Style::default().fg(TEXT)),
            Line::raw(""),
            Line::styled("Type a message below to begin.", Style::default().fg(MUTED)),
            Line::styled(
                format!(
                    "Backend: {} · Model: {}",
                    app.backend.provider().as_str(),
                    app.backend.model()
                ),
                Style::default().fg(MUTED),
            ),
            Line::raw(""),
            Line::styled(
                "F1 help  ·  /model <name>  ·  Ctrl+N new session",
                Style::default().fg(MUTED),
            ),
        ];
        frame.render_widget(Paragraph::new(welcome).alignment(Alignment::Center), inner);
        return;
    }

    let mut lines = Vec::new();
    for (index, message) in app.session.messages.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let (label, color) = match message.role {
            Role::User => ("YOU", USER),
            Role::Assistant => ("YOPES", ACCENT),
        };
        lines.push(Line::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        let streaming = index + 1 == app.session.messages.len() && app.is_streaming();
        if message.content.is_empty() && streaming {
            lines.push(Line::styled("▌", Style::default().fg(ACCENT)));
            continue;
        }
        for content_line in message.content.split('\n') {
            lines.push(Line::styled(content_line, Style::default().fg(TEXT)));
        }
        if streaming {
            if let Some(line) = lines.last_mut() {
                line.spans
                    .push(Span::styled(" ▌", Style::default().fg(ACCENT)));
            }
        }
    }

    let text = Text::from(lines);
    let total_lines = text
        .lines
        .iter()
        .map(|line| {
            let content = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            wrapped_line_count(&content, inner.width)
        })
        .sum::<usize>();
    let paragraph = Paragraph::new(text)
        .style(Style::default().bg(BACKGROUND))
        .wrap(Wrap { trim: false });
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
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (text, color) = if let Some(elapsed) = app.elapsed_seconds() {
        (
            format!(
                " {} Thinking · {elapsed:.1}s · Esc to stop",
                SPINNER[app.spinner % SPINNER.len()]
            ),
            ACCENT,
        )
    } else if let Some(notice) = &app.notice {
        (
            format!(" {}", notice.message),
            match notice.kind {
                NoticeKind::Info => MUTED,
                NoticeKind::Error => ERROR,
            },
        )
    } else {
        (" Ready".to_owned(), MUTED)
    };
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(18)])
        .split(area);
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(color)),
        columns[0],
    );
    frame.render_widget(
        Paragraph::new(format!("{} messages ", app.session.messages.len()))
            .alignment(Alignment::Right)
            .style(Style::default().fg(MUTED)),
        columns[1],
    );
}

fn wrapped_line_count(text: &str, width: u16) -> usize {
    let options = textwrap::Options::new(usize::from(width).max(1));
    text.split('\n')
        .map(|line| textwrap::wrap(line, &options).len().max(1))
        .sum()
}

fn composer_height(app: &App, width: u16) -> u16 {
    let inner_width = width.saturating_sub(4).max(1);
    let lines = wrapped_line_count(&app.input.with_cursor(), inner_width);
    (lines as u16).saturating_add(2).clamp(5, 10)
}

fn draw_composer(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(if app.is_streaming() { BORDER } else { ACCENT }))
        .style(Style::default().bg(SURFACE))
        .title(Span::styled(
            " MESSAGE ",
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let before =
        wrapped_line_count(app.input.before_cursor(), inner.width.max(1)).saturating_sub(1);
    let height = usize::from(inner.height);
    if before < app.input_scroll {
        app.input_scroll = before;
    } else if before >= app.input_scroll.saturating_add(height) {
        app.input_scroll = before.saturating_add(1).saturating_sub(height);
    }
    let input = Paragraph::new(app.input.with_cursor())
        .style(Style::default().fg(TEXT).bg(SURFACE))
        .wrap(Wrap { trim: false })
        .scroll((app.input_scroll.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(input, inner);
}

fn draw_footer(frame: &mut Frame, area: Rect, width: u16) {
    let text = if width < 80 {
        " enter send  ·  esc stop  ·  F1 help"
    } else {
        " enter send   alt/shift+enter newline   ↑↓ scroll   ctrl+n new   esc stop   F1 help"
    };
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(MUTED).bg(BACKGROUND)),
        area,
    );
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let width = area.width.min(68);
    let height = area.height.min(24);
    let popup = Rect::new(
        area.x + (area.width.saturating_sub(width)) / 2,
        area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(SURFACE))
        .title(Span::styled(
            " HELP ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);
    let lines = vec![
        help_section("SESSION"),
        help_row("Enter", "Send message"),
        help_row("Alt/Shift + Enter", "Insert newline"),
        help_row("Esc", "Stop streaming"),
        help_row("Ctrl + N", "New session"),
        help_row("Ctrl + C / Q", "Save and quit"),
        Line::raw(""),
        help_section("EDIT"),
        help_row("Ctrl + U", "Clear composer"),
        help_row("Ctrl + W", "Delete previous word"),
        help_row("Ctrl + A / E", "Move to start / end"),
        help_row("Left / Right", "Move cursor"),
        Line::raw(""),
        help_section("NAVIGATE"),
        help_row("Up / Down / Mouse", "Scroll conversation"),
        help_row("PageUp / PageDown", "Scroll one page"),
        help_row("End", "Jump to latest"),
        Line::raw(""),
        help_section("COMMANDS"),
        Line::styled(
            "/help  /new  /model [name]  /quit",
            Style::default().fg(TEXT),
        ),
        Line::raw(""),
        Line::styled("F1 or Esc to close", Style::default().fg(MUTED)),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn help_section(title: &'static str) -> Line<'static> {
    Line::styled(
        title,
        Style::default().fg(USER).add_modifier(Modifier::BOLD),
    )
}

fn help_row(key: &'static str, description: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<20}"), Style::default().fg(ACCENT)),
        Span::styled(description, Style::default().fg(TEXT)),
    ])
}

#[cfg(test)]
mod tests {
    use super::wrapped_line_count;

    #[test]
    fn wrapped_counts_include_words_and_explicit_newlines() {
        assert_eq!(wrapped_line_count("", 10), 1);
        assert_eq!(wrapped_line_count("one\ntwo", 10), 2);
        assert_eq!(wrapped_line_count("aaaaaa aaaaaa aaaaaa", 10), 3);
    }
}
