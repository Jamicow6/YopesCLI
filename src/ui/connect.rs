use crate::app::AddProviderField;

use super::*;

pub(super) fn draw_connect(
    frame: &mut Frame,
    app: &App,
    connect: &ConnectState,
    spinner: usize,
    area: Rect,
) {
    let width = (area.width.saturating_sub(4) * 92 / 100)
        .clamp(60, 120)
        .min(area.width.saturating_sub(2));
    let height = (area.height.saturating_sub(2) * 90 / 100)
        .clamp(16, 40)
        .min(area.height.saturating_sub(2));
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

    if connect.stage == ConnectStage::AddProvider {
        draw_add_provider_form(frame, connect, inner);
        return;
    }

    let mut constraints = vec![Constraint::Length(1)];
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

    draw_connect_list(frame, app, connect, spinner, rows[index]);
    draw_connect_hint(frame, connect, rows[index + 1]);
}

fn draw_add_provider_form(frame: &mut Frame, connect: &ConnectState, inner: Rect) {
    connect.popup_inner.set(inner);
    if inner.width < 30 || inner.height < 12 {
        return;
    }
    let active = connect.add_provider_field;

    let field_line =
        |label: &str, value: &str, is_active: bool, width: u16| -> (Line<'static>, Line<'static>) {
            let label_style = if is_active {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            };
            let label_line = Line::from(vec![Span::styled(label.to_owned(), label_style)]);
            let bracket_style = if is_active {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(MUTED)
            };
            let text_style = if is_active {
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            };
            let mut spans = vec![
                Span::styled("[ ", bracket_style),
                Span::styled(
                    truncate(value, usize::from(width).saturating_sub(6)),
                    text_style,
                ),
            ];
            if is_active {
                spans.push(Span::styled("▌", Style::default().fg(ACCENT)));
            }
            spans.push(Span::styled(" ]", bracket_style));
            (label_line, Line::from(spans))
        };

    let mut lines = Vec::new();

    // 1. Provider name:
    let (lbl, val) = field_line(
        "Provider name:",
        &connect.custom_name,
        active == AddProviderField::Name,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    // 2. Completion URL:
    let (lbl, val) = field_line(
        "Completion URL:",
        &connect.custom_endpoint,
        active == AddProviderField::CompletionUrl,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    // 3. (optional) Model fetching URL:
    let (lbl, val) = field_line(
        "(optional) Model fetching URL:",
        &connect.custom_models_url,
        active == AddProviderField::ModelsUrl,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    // 4. Models: name / modelID (two box split)
    let models_active =
        active == AddProviderField::ModelName || active == AddProviderField::ModelId;
    lines.push(Line::from(vec![Span::styled(
        "Models:",
        if models_active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        },
    )]));

    let half = inner.width.saturating_sub(4) / 2;
    let name_active = active == AddProviderField::ModelName;
    let id_active = active == AddProviderField::ModelId;

    let mut models_spans = Vec::new();
    models_spans.push(Span::styled(
        "name: [ ",
        if name_active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        },
    ));
    models_spans.push(Span::styled(
        truncate(
            &connect.custom_model_name,
            usize::from(half).saturating_sub(12),
        ),
        Style::default().fg(TEXT),
    ));
    if name_active {
        models_spans.push(Span::styled("▌", Style::default().fg(ACCENT)));
    }
    models_spans.push(Span::styled(
        " ]  ",
        if name_active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        },
    ));

    models_spans.push(Span::styled(
        "modelID: [ ",
        if id_active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        },
    ));
    models_spans.push(Span::styled(
        truncate(
            &connect.custom_model_id,
            usize::from(half).saturating_sub(14),
        ),
        Style::default().fg(TEXT),
    ));
    if id_active {
        models_spans.push(Span::styled("▌", Style::default().fg(ACCENT)));
    }
    models_spans.push(Span::styled(
        " ]",
        if id_active {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        },
    ));
    lines.push(Line::from(models_spans));

    // 5. Context Window:
    let cw_display = if connect.custom_model_context_window.is_empty() {
        "128000"
    } else {
        &connect.custom_model_context_window
    };
    let (lbl, val) = field_line(
        "Context Window:",
        cw_display,
        active == AddProviderField::ContextWindow,
        inner.width,
    );
    lines.push(lbl);
    lines.push(val);

    lines.push(Line::raw(""));

    // 6. Done button
    let done_active = active == AddProviderField::Done;
    let done_style = if done_active {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
    };
    lines.push(Line::from(vec![Span::styled(
        if done_active {
            " › [ Done ] ‹ "
        } else {
            "   [ Done ]   "
        },
        done_style,
    )]));

    // 7. Hint or error
    if let Some(error) = &connect.error {
        lines.push(Line::styled(
            format!("✗ {error}"),
            Style::default().fg(ERROR),
        ));
    } else {
        lines.push(Line::styled(
            "Tab switch box · Enter submit · Esc cancel",
            Style::default().fg(MUTED),
        ));
    }

    frame.render_widget(Paragraph::new(lines), inner);
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
        ConnectStage::Providers | ConnectStage::Models | ConnectStage::CustomModels
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
    connect.list_area.set(area);
    let height = usize::from(area.height);
    if height == 0 {
        return;
    }
    let selected = connect.selected.min(items.len() - 1);
    // Custom block first, then models.dev.
    // Sections need a divider row each, skipped by the selection.
    let is_providers = connect.stage == ConnectStage::Providers;
    let custom_count = if is_providers {
        items
            .iter()
            .take_while(|item| item.id == CUSTOM_PROVIDER_ID || item.custom_id.is_some())
            .count()
    } else {
        0
    };
    let models_divider = is_providers && custom_count > 0 && custom_count < items.len();
    // Rows: Ok(index) renders item index, Err(label) renders a divider.
    let mut rows = Vec::with_capacity(items.len() + usize::from(models_divider));
    for index in 0..items.len() {
        if models_divider && index == custom_count {
            rows.push(Err(" models.dev "));
        }
        rows.push(Ok(index));
    }
    let extra = usize::from(models_divider && selected >= custom_count);
    let selected_row = selected + extra;
    let start_row = selected_row.saturating_sub(height.saturating_sub(1));
    let start = start_row.min(rows.len().saturating_sub(height));
    let list = area;
    let title_width = (usize::from(list.width) * 55 / 100).max(16);

    for (offset, row) in rows.iter().skip(start).take(height).enumerate() {
        let rect = Rect::new(list.x, list.y + offset as u16, list.width, 1);
        let Ok(index) = *row else {
            let Err(label) = *row else { unreachable!() };
            let remaining = usize::from(list.width)
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
                fit_to(&item.title, title_width),
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
        ConnectStage::Providers => {
            "↑↓ move · type to filter · enter choose · f2 rename · delete / ctrl+d remove · esc close"
                .to_owned()
        }
        ConnectStage::AddProvider => {
            "tab switch box · enter submit · esc close".to_owned()
        }
        ConnectStage::RenameProvider => {
            "enter rename · esc back".to_owned()
        }
        ConnectStage::Models => {
            "↑↓ move · type to filter · enter choose · tab context · esc back".to_owned()
        }
        ConnectStage::ApiKey => connect.required_env().map_or_else(
            || "enter continue · esc back · empty skips the key".to_owned(),
            |name| format!("enter continue · esc back · empty uses ${name}"),
        ),
        ConnectStage::Endpoint => "enter continue · esc back".to_owned(),
        ConnectStage::ModelsEndpoint => "enter continue · blank uses <endpoint>/models".to_owned(),
        ConnectStage::CustomToken => "enter continue · esc back".to_owned(),
        ConnectStage::ModelId => {
            if connect.has_custom_models() {
                "enter continue · empty keeps saved models · esc back".to_owned()
            } else {
                "enter continue · esc back".to_owned()
            }
        }
        ConnectStage::ModelName => "enter continue · empty uses the model id · esc back".to_owned(),
        ConnectStage::ModelContextWindow => {
            if connect.custom_flow {
                "enter add model · blank skips · esc back".to_owned()
            } else {
                "enter save · blank resets · esc back".to_owned()
            }
        }
        ConnectStage::CustomModels => {
            "↑↓ choose · enter use · tab context · d delete · esc back".to_owned()
        }
        ConnectStage::Password => {
            "enter save · esc back · empty stores without encryption".to_owned()
        }
        ConnectStage::Unlock => "enter unlock · esc back".to_owned(),
    };
    frame.render_widget(Paragraph::new(text).style(Style::default().fg(MUTED)), area);
}
