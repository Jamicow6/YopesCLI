use super::*;

pub(super) fn draw_model_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
    let width = responsive_palette_width(area.width);
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
    for (offset, model) in models.iter().skip(start).take(visible).enumerate() {
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
        let detail = if model.active_provider {
            model.id.clone()
        } else {
            format!("{} · {}", model.provider, model.id)
        };
        let detail_style = if model.active_provider {
            Style::default().fg(MUTED)
        } else {
            Style::default().fg(ADDED)
        };
        let title_width = (usize::from(inner.width).saturating_sub(6) * 55 / 100).clamp(16, 40);
        let detail_width = usize::from(inner.width).saturating_sub(title_width + 4);
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(format!("{} ", fit_to(&title, title_width)), title_style),
            Span::styled(fit_to(&detail, detail_width), detail_style),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

pub(super) fn draw_session_picker(frame: &mut Frame, app: &App, picker: &ListPicker, area: Rect) {
    let width = responsive_palette_width(area.width);
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
    for (offset, entry) in sessions.iter().skip(start).take(visible).enumerate() {
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
        let right_width = 8;
        let left_width = inner.width.saturating_sub(right_width as u16 + 4) as usize;
        let title = entry.title.clone();
        let title_style = if chosen {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT)
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(format!("{} ", fit_to(&title, left_width)), title_style),
            Span::styled(fit_to(&right, right_width), Style::default().fg(MUTED)),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}

/// Rough age of a saved session file: now / minutes / hours / days.
pub(super) fn age_label(modified: Option<SystemTime>) -> String {
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

pub(super) fn draw_palette(
    frame: &mut Frame,
    app: &App,
    palette: &[&'static CommandSpec],
    area: Rect,
) {
    let width = responsive_palette_width(area.width);
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
    let name_col_width = (usize::from(inner.width) * 35 / 100).clamp(18, 28);
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
        let desc_col_width = usize::from(inner.width).saturating_sub(name_col_width + 3);
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(ACCENT)),
            Span::styled(fit_to(&name, name_col_width), name_style),
            Span::raw(" "),
            Span::styled(
                truncate(command.description, desc_col_width),
                description_style,
            ),
        ]);
        frame.render_widget(Paragraph::new(line), row);
    }
}
