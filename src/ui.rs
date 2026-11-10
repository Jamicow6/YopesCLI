use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap},
};

use crate::{
    app::{App, COMMANDS, CommandSpec, ConnectStage, ConnectState, NoticeKind},
    session::Role,
};

const MUTED: Color = Color::DarkGray;
const ACCENT: Color = Color::Cyan;
const ERROR: Color = Color::Red;
const TEXT: Color = Color::Reset;
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const PALETTE_WIDTH: u16 = 60;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 28 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("Terminal too small")
                .alignment(Alignment::Center)
                .style(Style::default().fg(MUTED)),
            area,
        );
        return;
    }

    let palette = if app.show_help || app.connect_open() {
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

    if app.show_help {
        draw_help(frame, area);
    } else if let Some(connect) = app.connect_state() {
        draw_connect(frame, connect, app.spinner, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let inner = padded(area, 2);
    let right = if app.backend.model().is_empty() {
        app.backend.provider_label().to_owned()
    } else {
        format!("{} · {}", app.backend.provider_label(), app.backend.model())
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
    let inner = Block::default()
        .padding(Padding::new(2, 2, 1, 1))
        .inner(area);
    app.transcript_height = inner.height;
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.session.messages.is_empty() {
        let mut welcome = vec![
            Line::styled(
                "yopes",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                "A quiet place to think out loud.",
                Style::default().fg(TEXT),
            ),
            Line::raw(""),
        ];
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
        frame.render_widget(Paragraph::new(welcome), inner);
        return;
    }

    let mut lines = Vec::new();
    for (index, message) in app.session.messages.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(""));
        }
        let (label, color) = match message.role {
            Role::User => ("you", MUTED),
            Role::Assistant => ("yopes", ACCENT),
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

    let text = ratatui::text::Text::from(lines);
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
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
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
    let prompt_style = if app.is_streaming() {
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

    let (text, color) = if let Some(elapsed) = app.elapsed_seconds() {
        (
            format!(
                "{} thinking · {elapsed:.1}s · esc stop",
                SPINNER[app.spinner % SPINNER.len()]
            ),
            ACCENT,
        )
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

    let items = connect.items();
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
    let divider = connect.stage == ConnectStage::Providers && items.len() > 1;
    let divider_rows = if divider { 1 } else { 0 };
    let mut rows = Vec::with_capacity(items.len() + divider_rows);
    for index in 0..items.len() {
        if divider && index == 1 {
            rows.push(None);
        }
        rows.push(Some(index));
    }
    let selected_row = selected + if divider && selected > 0 { 1 } else { 0 };
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
        let badge_style = if chosen {
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(ACCENT).bg(Color::DarkGray)
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
            Span::styled(format!(" {:<2} ", monogram(&item.title)), badge_style),
            Span::styled(" ", Style::default()),
            Span::styled(
                format!("{:<title_width$}", truncate(&item.title, title_width)),
                title_style,
            ),
        ];
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
        ConnectStage::Endpoint => "enter continue · esc close".to_owned(),
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
            "↑↓ choose · enter use · a add model · esc endpoint".to_owned()
        }
        ConnectStage::Password => {
            "enter save · esc back · empty stores without encryption".to_owned()
        }
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), area);
}

fn monogram(name: &str) -> String {
    let letters = name
        .chars()
        .filter(|character| character.is_alphanumeric())
        .take(2)
        .collect::<String>();
    if letters.is_empty() {
        "·".to_owned()
    } else {
        letters.to_uppercase()
    }
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

fn draw_help(frame: &mut Frame, area: Rect) {
    let width = area.width.min(62);
    let height = area.height.min(24);
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
        help_row("↑ ↓", "palette or conversation"),
        help_row("esc", "stop streaming / close"),
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
    use super::wrapped_line_count;

    #[test]
    fn wrapped_counts_include_words_and_explicit_newlines() {
        assert_eq!(wrapped_line_count("", 10), 1);
        assert_eq!(wrapped_line_count("one\ntwo", 10), 2);
        assert_eq!(wrapped_line_count("aaaaaa aaaaaa aaaaaa", 10), 3);
    }
}
