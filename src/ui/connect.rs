use super::*;

pub(super) fn draw_connect(
    frame: &mut Frame,
    app: &App,
    connect: &ConnectState,
    spinner: usize,
    area: Rect,
) {
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

    // The logo of the provider being picked is drawn above the list.
    let logo = selected_logo_id(connect).and_then(|id| app.logos.get(id.as_str()));
    let logo_height = logo.map_or(0, |logo| logo.len() as u16);
    let mut constraints = vec![Constraint::Length(1)];
    if logo_height > 0 {
        constraints.push(Constraint::Length(logo_height));
    }
    constraints.push(Constraint::Min(2));
    constraints.push(Constraint::Length(1));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    let mut index = 0;

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
    frame.render_widget(Paragraph::new(Line::from(spans)), rows[index]);
    index += 1;

    if let Some(logo) = logo {
        let area = rows[index];
        // The logo sits in the middle of the width it needs.
        let width = logo
            .first()
            .map_or(0, |line| line.spans.len() as u16)
            .min(area.width);
        let left = area.x + area.width.saturating_sub(width) / 2;
        for (offset, line) in logo.iter().enumerate() {
            let row = Rect::new(left, area.y + offset as u16, width, 1);
            frame.render_widget(Paragraph::new(line.clone()), row);
        }
        index += 1;
    }

    draw_connect_list(frame, app, connect, spinner, rows[index]);
    draw_connect_hint(frame, connect, rows[index + 1]);
}

/// The provider whose logo is shown above the list.
fn selected_logo_id(connect: &ConnectState) -> Option<String> {
    match connect.stage {
        ConnectStage::Models => connect.provider_id.clone(),
        ConnectStage::Providers => {
            let items = connect.items();
            let item = items.get(connect.selected.min(items.len().saturating_sub(1)))?;
            // A saved endpoint and the "add provider" row have no logo.
            if item.custom_id.is_some() || item.id == CUSTOM_PROVIDER_ID {
                return None;
            }
            Some(item.id.clone())
        }
        _ => None,
    }
}

pub(super) fn draw_connect_list(
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
    let mut rows = Vec::with_capacity(
        items.len() + usize::from(preferred_divider) + usize::from(models_divider),
    );
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

pub(super) fn draw_connect_hint(frame: &mut Frame, connect: &ConnectState, area: Rect) {
    let text = match connect.stage {
        ConnectStage::Providers => "↑↓ move · type to filter · enter choose · esc close".to_owned(),
        ConnectStage::FasfasNotice => "enter then paste your key · esc close".to_owned(),
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
        ConnectStage::Unlock => "enter unlock · esc back".to_owned(),
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), area);
}
