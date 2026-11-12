//! The provider logos, drawn from the SVG files models.dev serves.
//!
//! A logo is fetched once, rasterised to a small bitmap and kept as the
//! half-block rows the interface draws. The rows are cached on disk so the
//! download happens once per provider.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

/// Where models.dev serves the provider logos.
const LOGO_BASE: &str = "https://models.dev/logos";
/// How wide a logo is drawn, in terminal cells.
const LOGO_COLS: u16 = 6;

/// A logo drawn as half-block rows, ready to be put in the interface.
pub type Logo = Vec<Line<'static>>;

/// The logos fetched so far, keyed by provider id.
#[derive(Default)]
pub struct Logos {
    rows: HashMap<String, Logo>,
    /// The directory the rendered logos are cached in.
    cache_dir: Option<PathBuf>,
}

impl Logos {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            rows: HashMap::new(),
            cache_dir,
        }
    }

    /// The rows of a logo, if it has been drawn.
    pub fn get(&self, provider_id: &str) -> Option<&Logo> {
        self.rows.get(provider_id)
    }

    /// Whether a provider has a logo to draw.
    pub fn has(&self, provider_id: &str) -> bool {
        self.rows.contains_key(provider_id)
    }

    /// Remember the rows of a logo.
    pub fn insert(&mut self, provider_id: &str, logo: Logo) {
        self.rows.insert(provider_id.to_owned(), logo);
    }

    /// The file a provider's logo is cached in.
    fn cache_path(&self, provider_id: &str) -> Option<PathBuf> {
        let dir = self.cache_dir.as_ref()?;
        // A provider id is a slug, but it is not trusted: only safe
        // characters end up in the file name.
        let safe: String = provider_id
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
            .collect();
        if safe.is_empty() {
            return None;
        }
        Some(dir.join(format!("{safe}.logo")))
    }

    /// Read a cached logo from disk.
    pub fn load_cached(&mut self, provider_id: &str) -> bool {
        let Some(path) = self.cache_path(provider_id) else {
            return false;
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return false;
        };
        let Some(rows) = decode(&text) else {
            return false;
        };
        self.insert(provider_id, rows);
        true
    }

    /// Write a logo to the cache, so it is only downloaded once.
    pub fn cache(&self, provider_id: &str, logo: &Logo) {
        let Some(path) = self.cache_path(provider_id) else {
            return;
        };
        let Some(text) = encode(logo) else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, text);
    }
}

/// Fetch a provider's logo and draw it.
pub async fn fetch(provider_id: &str) -> Result<Logo> {
    let url = format!("{LOGO_BASE}/{provider_id}.svg");
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("yopes/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("creating the logo client")?;
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("fetching the logo for {provider_id}"))?
        .error_for_status()
        .with_context(|| format!("fetching the logo for {provider_id}"))?;
    let svg = response.text().await.context("reading the provider logo")?;
    render_svg(&svg)
}

/// Draw an SVG as half-block rows.
///
/// The logo is rasterised to a bitmap twice as wide as it is tall, so the
/// cells stay square, and each cell carries two pixel rows through `▄`.
pub fn render_svg(svg: &str) -> Result<Logo> {
    let options = usvg::Options::default();
    let tree =
        usvg::Tree::from_data(svg.as_bytes(), &options).context("reading the provider logo")?;
    let size = tree.size().to_int_size();
    if size.width() == 0 || size.height() == 0 {
        bail!("the provider logo has no size");
    }
    // The drawing is two pixels wide per cell, so the bitmap is wider than it
    // is tall and the cells come out square.
    let width = u32::from(LOGO_COLS) * 2;
    let height = (f64::from(u32::from(LOGO_COLS)) * f64::from(size.height())
        / f64::from(size.width()))
    .round()
    .max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(width, height).context("creating the logo bitmap")?;
    let scale = width as f32 / size.width() as f32;
    let transform = tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    Ok(pixmap_to_rows(&pixmap))
}

/// Turn a rendered bitmap into half-block rows.
fn pixmap_to_rows(pixmap: &tiny_skia::Pixmap) -> Logo {
    let width = pixmap.width();
    let height = pixmap.height();
    let mut rows = Vec::new();
    let mut top = 0;
    while top < height {
        let bottom = (top + 1).min(height - 1);
        let mut spans = Vec::with_capacity(width as usize);
        for x in 0..width {
            let upper = pixmap.pixel(x, top);
            let lower = pixmap.pixel(x, bottom);
            spans.push(Span::styled(
                "▄",
                Style::default()
                    .fg(pixel_color(lower))
                    .bg(pixel_color(upper)),
            ));
        }
        rows.push(Line::from(spans));
        top += 2;
    }
    rows
}

/// The colour of a pixel, or the background colour when it is not drawn.
fn pixel_color(pixel: Option<tiny_skia::PremultipliedColorU8>) -> Color {
    let Some(pixel) = pixel else {
        return Color::Reset;
    };
    let color = pixel.demultiply();
    if color.alpha() == 0 {
        return Color::Reset;
    }
    Color::Rgb(color.red(), color.green(), color.blue())
}

/// Encode the rows so they can be cached on disk.
///
/// Each row is one line of hex colours: two bytes per cell, the lower pixel
/// first, then the upper one.
fn encode(logo: &Logo) -> Option<String> {
    let mut text = String::new();
    for line in logo {
        let mut row = String::new();
        for span in &line.spans {
            let Color::Rgb(red, green, blue) = span.style.fg.unwrap_or(Color::Reset) else {
                continue;
            };
            let Color::Rgb(bred, bgreen, bblue) = span.style.bg.unwrap_or(Color::Reset) else {
                continue;
            };
            row.push_str(&format!("{blue:02x}{green:02x}{red:02x}"));
            row.push_str(&format!("{bblue:02x}{bgreen:02x}{bred:02x}"));
        }
        text.push_str(&row);
        text.push('\n');
    }
    Some(text)
}

/// Read the rows back from the cache.
fn decode(text: &str) -> Option<Logo> {
    let mut rows = Vec::new();
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let bytes = hex_decode(line)?;
        let mut spans = Vec::with_capacity(bytes.len() / 6);
        for cell in bytes.chunks_exact(6) {
            let lower = Color::Rgb(cell[2], cell[1], cell[0]);
            let upper = Color::Rgb(cell[5], cell[4], cell[3]);
            spans.push(Span::styled("▄", Style::default().fg(lower).bg(upper)));
        }
        rows.push(Line::from(spans));
    }
    if rows.is_empty() {
        return None;
    }
    Some(rows)
}

/// Read a hex string into bytes.
fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    let mut chars = text.chars();
    while let (Some(high), Some(low)) = (chars.next(), chars.next()) {
        let high = high.to_digit(16)? as u8;
        let low = low.to_digit(16)? as u8;
        bytes.push((high << 4) | low);
    }
    Some(bytes)
}

/// The directory the rendered logos are cached in.
pub fn cache_dir(config_path: &Path) -> Option<PathBuf> {
    let dir = config_path.parent()?;
    Some(dir.join("logos"))
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, render_svg};

    const SAMPLE: &str = r##"<svg width="24" height="24" viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg">
    <path d="M12 2 L22 22 L2 22 Z" fill="#ff8800"/>
    </svg>"##;

    #[test]
    fn an_svg_becomes_half_block_rows() {
        let logo = render_svg(SAMPLE).unwrap();
        assert!(!logo.is_empty());
        assert!(logo.iter().all(|line| !line.spans.is_empty()));
        assert!(logo[0].spans[0].content.as_ref() == "▄");
    }

    #[test]
    fn the_cache_round_trips() {
        let logo = render_svg(SAMPLE).unwrap();
        let text = encode(&logo).unwrap();
        let read = decode(&text).unwrap();
        assert_eq!(read.len(), logo.len());
        assert_eq!(read[0].spans.len(), logo[0].spans.len());
    }

    #[test]
    fn a_broken_cache_is_rejected() {
        assert!(decode("not hex").is_none());
        assert!(decode("").is_none());
    }
}
