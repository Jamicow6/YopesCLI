use super::*;

/// The longest edge sent to a vision model, in pixels: larger images are
/// downscaled before encoding, so one file cannot dominate the context.
const MAX_IMAGE_DIMENSION: u32 = 1568;

/// Read the pixels of a `read_image` result. The tool's text result cannot
/// carry binary data, so the app attaches the PNG to the tool message itself.
pub(super) fn tool_image_attachment(call: &ToolCall) -> Option<ImageAttachment> {
    let value: serde_json::Value = serde_json::from_str(&call.function.arguments).ok()?;
    let path = value.get("path")?.as_str()?.trim();
    if path.is_empty() {
        return None;
    }
    let path = std::path::Path::new(path);
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let mut image = image::open(&resolved).ok()?;
    if image.width() > MAX_IMAGE_DIMENSION || image.height() > MAX_IMAGE_DIMENSION {
        image = image.resize(
            MAX_IMAGE_DIMENSION,
            MAX_IMAGE_DIMENSION,
            image::imageops::FilterType::Triangle,
        );
    }
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .ok()?;
    let name = resolved.file_name().map_or_else(
        || "image.png".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    );
    Some(ImageAttachment {
        name,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Half-block pixel art of a pasted image: each TUI row shows two pixel
/// lines at once, through `▄` glyphs with per-span foreground and background.
pub(super) fn build_image_rows(encoded: &str, cols: u16) -> Vec<Line<'static>> {
    let unreadable = || {
        Line::styled(
            " (image could not be shown)",
            Style::default().fg(Color::DarkGray),
        )
    };
    let decoded = match base64::engine::general_purpose::STANDARD.decode(encoded.trim()) {
        Ok(bytes) => bytes,
        Err(_) => return vec![unreadable()],
    };
    let rendered = match image::load_from_memory(&decoded) {
        Ok(image) => image.into_rgba8(),
        Err(_) => return vec![unreadable()],
    };
    let cols = usize::from(cols).max(1);
    let (width, height) = (rendered.width().max(1), rendered.height().max(1));
    // Cells are twice as wide as tall, and one `▄` carries two pixel rows,
    // so two pixels per cell in both directions keep the aspect square.
    let row_pairs = ((u64::from(height) * cols as u64) / u64::from(width) / 2).clamp(1, 18) as u32;
    let scaled = image::imageops::thumbnail(&rendered, (cols * 2) as u32, row_pairs * 2);
    let row_pairs = scaled.height().div_ceil(2);
    let mut rows = Vec::with_capacity(row_pairs as usize);
    for pair in 0..row_pairs {
        let top_y = (pair * 2).min(scaled.height() - 1);
        let bottom_y = (top_y + 1).min(scaled.height() - 1);
        let mut spans = Vec::with_capacity(cols);
        for x in 0..scaled.width() {
            spans.push(Span::styled(
                "▄",
                Style::default()
                    .fg(pixel_color(scaled.get_pixel(x, bottom_y)))
                    .bg(pixel_color(scaled.get_pixel(x, top_y))),
            ));
        }
        rows.push(Line::from(spans));
    }
    rows
}

pub(super) fn pixel_color(pixel: &image::Rgba<u8>) -> Color {
    let [red, green, blue, _] = pixel.0;
    Color::Rgb(red, green, blue)
}

impl App {
    /// A clipboard read arrived: queue it as an image or insert its text.
    pub(crate) fn on_paste_image(&mut self, result: Result<(String, String), String>) -> Action {
        match result {
            Ok((kind, payload)) => {
                if kind == "text" {
                    // The same route as a bracketed paste keeps the caret,
                    // connect fields and help overlay behaving the same.
                    self.handle_paste(&payload);
                } else {
                    let name = format!("paste-{}.png", self.next_image);
                    self.next_image += 1;
                    self.attachments.push(ImageAttachment {
                        name,
                        data: payload,
                    });
                    self.info(format!(
                        "Image {} attached — send it with the next message.",
                        self.attachments.len()
                    ));
                }
            }
            Err(error) => self.error(error),
        }
        Action::None
    }

    /// Render any not-yet-cached pasted image for a terminal width.
    ///
    /// The transcript loop only reads the session, so rows are prepared here
    /// beforehand and the loop then extends its lines from this cache.
    pub(crate) fn warm_image_cache(&mut self, width: u16) {
        if self.image_width != width {
            self.image_rows.clear();
            self.image_width = width;
        }
        if self.image_rows.len() > 40 {
            self.image_rows.clear();
        }
        let mut rows = std::mem::take(&mut self.image_rows);
        for message in &self.session.messages {
            for image in &message.images {
                let key = format!("{}:{width}", image.name);
                rows.entry(key)
                    .or_insert_with(|| build_image_rows(&image.data, width));
            }
        }
        self.image_rows = rows;
    }

    /// The cached half-block rows of one pasted image, if rendered for the
    /// current terminal width.
    pub(crate) fn cached_image(&self, name: &str) -> Option<&[Line<'static>]> {
        self.image_rows
            .get(&format!("{name}:{}", self.image_width))
            .map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::build_image_rows;
    use base64::Engine as _;

    #[test]
    fn an_image_renders_as_half_block_pixel_rows() {
        use ratatui::style::Color;

        let png = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 2, |_, _| {
            image::Rgb([255u8, 0, 0])
        }));
        let mut bytes = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut bytes);
        png.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        let data = base64::engine::general_purpose::STANDARD.encode(&bytes);

        let rows = build_image_rows(&data, 2);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].spans.len(), 4);
        assert_eq!(rows[0].spans[0].content, "▄");
        assert_eq!(rows[0].spans[0].style.fg, Some(Color::Rgb(255, 0, 0)));
        assert_eq!(rows[0].spans[3].style.fg, Some(Color::Rgb(255, 0, 0)));
    }
}
