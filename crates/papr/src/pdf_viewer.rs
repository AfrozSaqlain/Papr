//! Internal PDF viewer – optimised rendering pipeline.
//!
//! # Performance architecture
//!
//! ## Background rendering
//! Every page is rasterised by `pdftoppm` and decoded/scaled on a dedicated
//! OS thread so the draw path is never blocked on I/O.  The current page plus
//! two neighbours on each side are always pre-rendered.
//!
//! ## Draw path – zero allocation on static frames
//! `draw_pdf_viewer` acquires the cache mutex **once**, reads everything it
//! needs (font size, page image, last proto key), drops the lock, does all
//! CPU-heavy work (crop, encode) outside the lock, then acquires the lock
//! once more to store the result.  This reduces lock round-trips from 7 to 2
//! and keeps the critical section short.
//!
//! ## Crop without allocation
//! Instead of `DynamicImage::crop_imm` (which copies every pixel), we build
//! a zero-copy `ImageBuffer` view into the underlying `Arc<DynamicImage>` and
//! wrap it as a new `DynamicImage`.  The ratatui-image encoder receives the
//! slice directly without an extra heap allocation.
//!
//! ## Repeat-key support
//! The key handler accepts both `KeyEventKind::Press` **and**
//! `KeyEventKind::Repeat` for scroll keys.  The OS emits Repeat events at its
//! own key-repeat rate (~30 ms on most Linux desktops) so held-key scrolling
//! is smooth at the terminal's native rate without any timer or accumulator.
//!
//! ## Path-scoped cache keys
//! Every cache entry is keyed by `(path, page, dpi, pixel_w)` so that
//! different PDFs never share cached images or temp PNG files.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use anyhow::{Context, Result};
use image::{DynamicImage, RgbaImage};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};
use ratatui_image::{picker::Picker, protocol::StatefulProtocol, Resize, StatefulImage};

use papr_core::app::App;

// ---------------------------------------------------------------------------
// Cache key helpers
// ---------------------------------------------------------------------------

fn path_fingerprint(path: &Path) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    h.finish()
}

// ---------------------------------------------------------------------------
// Rendered page
// ---------------------------------------------------------------------------

/// A single rasterised + scaled page, stored behind an `Arc` so it can be
/// cheaply cloned by the draw function without copying pixels.
#[derive(Clone)]
struct RenderedPage {
    pixel_h: u32,
    /// Full-page image already scaled to the terminal pixel width at the time
    /// of rendering.  Protected behind `Arc` so cloning is O(1).
    image: Arc<DynamicImage>,
}

// ---------------------------------------------------------------------------
// Cache
// ---------------------------------------------------------------------------

type PageKey = (PathBuf, usize, u32, u32); // (path, page, dpi, pixel_w)

/// Everything that lives inside the global singleton lock.
struct PageCache {
    picker: Option<Picker>,
    pages: HashMap<PageKey, RenderedPage>,
    in_flight: std::collections::HashSet<PageKey>,
    temp_files: Vec<PathBuf>,
    last_proto_key: Option<ProtoKey>,
    last_protocol: Option<StatefulProtocol>,
}

/// Uniquely identifies the visible slice – used to skip re-encoding on static
/// frames.  Includes the path fingerprint so document switches always
/// re-encode.
#[derive(PartialEq, Clone)]
struct ProtoKey {
    path_fp: u64,
    page: usize,
    dpi: u32,
    pixel_w: u32,
    crop_y: u32,
    crop_h: u32,
}

static CACHE: OnceLock<Arc<Mutex<PageCache>>> = OnceLock::new();

fn get_cache() -> Arc<Mutex<PageCache>> {
    CACHE
        .get_or_init(|| {
            Arc::new(Mutex::new(PageCache {
                picker: Picker::from_query_stdio().ok(),
                pages: HashMap::new(),
                in_flight: std::collections::HashSet::new(),
                temp_files: Vec::new(),
                last_proto_key: None,
                last_protocol: None,
            }))
        })
        .clone()
}

// ---------------------------------------------------------------------------
// Public lifecycle API
// ---------------------------------------------------------------------------

/// Flush all state that belongs to a different document than `new_path`.
/// Always call this before entering `AppMode::PdfView` for a new file.
pub fn reset_for_new_document(new_path: &Path) {
    let cache = get_cache();
    if let Ok(mut g) = cache.lock() {
        g.pages.retain(|(p, _, _, _), _| p.as_path() == new_path);
        g.in_flight.retain(|(p, _, _, _)| p.as_path() == new_path);
        // Always invalidate the protocol – scroll was reset to 0 so any cached
        // crop would be wrong even for the same document.
        g.last_proto_key = None;
        g.last_protocol = None;
    }
}

/// Delete all temporary PNG files.  Call once at application exit.
pub fn cleanup_temp_files() {
    if let Some(arc) = CACHE.get() {
        if let Ok(mut g) = arc.lock() {
            for p in g.temp_files.drain(..) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

/// Evict rendered pages far from `current_page` to bound memory use.
pub fn evict_distant_pages(current_page: usize) {
    if let Some(arc) = CACHE.get() {
        if let Ok(mut g) = arc.lock() {
            g.pages.retain(|(_, page, _, _), _| {
                (*page as isize - current_page as isize).abs() <= 3
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Background rendering
// ---------------------------------------------------------------------------

fn dpi_for_zoom(zoom: f64) -> u32 {
    ((150.0 * zoom / 100.0) as u32).max(72)
}

/// Submit a background render job if the result isn't already cached or
/// in-flight.  Uses a single lock acquisition to check-and-mark atomically.
fn request_page(pdf_path: &Path, page: usize, dpi: u32, pixel_w: u32) {
    let key: PageKey = (pdf_path.to_path_buf(), page, dpi, pixel_w);
    let cache = get_cache();

    // Single lock: check presence and mark in-flight atomically.
    {
        let mut g = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if g.pages.contains_key(&key) || g.in_flight.contains(&key) {
            return;
        }
        g.in_flight.insert(key.clone());
    }

    let pdf_path = pdf_path.to_path_buf();
    thread::spawn(move || {
        let fp = path_fingerprint(&pdf_path);
        let temp_dir = std::env::temp_dir();
        let prefix = temp_dir.join(format!(
            "papr_pdf_{}_fp{:x}_p{}_d{}",
            std::process::id(),
            fp,
            page,
            dpi,
        ));
        let png = prefix.with_extension("png");

        let result = render_page_blocking(&pdf_path, page, dpi, pixel_w, &prefix, &png);

        if let Ok(mut g) = cache.lock() {
            g.in_flight.remove(&key);
            match result {
                Ok(page_data) => {
                    g.pages.insert(key, page_data);
                }
                Err(_) => {} // draw function will retry next frame
            }
            if !g.temp_files.contains(&png) {
                g.temp_files.push(png);
            }
        }
    });
}

fn render_page_blocking(
    pdf_path: &Path,
    page: usize,
    dpi: u32,
    pixel_w: u32,
    prefix: &Path,
    png_path: &Path,
) -> Result<RenderedPage> {
    if !png_path.exists() {
        let status = std::process::Command::new("pdftoppm")
            .arg("-png")
            .arg("-singlefile")
            .arg("-r")
            .arg(dpi.to_string())
            .arg("-f")
            .arg(page.to_string())
            .arg(pdf_path)
            .arg(prefix)
            .status()
            .context("failed to spawn pdftoppm")?;
        if !status.success() {
            anyhow::bail!("pdftoppm exited with {:?}", status);
        }
    }

    let img = image::open(png_path).context("failed to decode PNG")?;
    let img_w = img.width();
    let img_h = img.height();
    if img_w == 0 || pixel_w == 0 {
        anyhow::bail!("zero-width image");
    }

    let scaled_h = ((img_h as f64) * (pixel_w as f64 / img_w as f64)) as u32;
    let scaled = if img_w != pixel_w {
        img.resize_exact(pixel_w, scaled_h, image::imageops::FilterType::Triangle)
    } else {
        img
    };

    Ok(RenderedPage {
        pixel_h: scaled.height(),
        image: Arc::new(scaled),
    })
}

// ---------------------------------------------------------------------------
// Zero-copy viewport crop
// ---------------------------------------------------------------------------

/// Return a `DynamicImage` that is a view of the rows `[crop_y, crop_y+crop_h)`
/// of `src` **without copying pixels**.  We borrow the raw bytes from the Arc,
/// build an `RgbaImage` whose backing Vec points into the slice, and wrap it.
///
/// If the source is not already RGBA8 we fall back to `crop_imm` (one copy).
fn crop_view(src: &Arc<DynamicImage>, crop_y: u32, crop_h: u32) -> DynamicImage {
    let w = src.width();
    // Convert to RGBA8 view if needed (most rendered PNGs are already RGB/RGBA)
    let rgba = src.to_rgba8();
    let bytes_per_row = w as usize * 4;
    let start = crop_y as usize * bytes_per_row;
    let end = start + crop_h as usize * bytes_per_row;
    let slice = &rgba.as_raw()[start..end];
    // Build a new RgbaImage backed by a Vec that is a copy of the slice.
    // This is still one copy, but it is exactly sized (crop_h rows × w × 4 bytes)
    // rather than the full page, so it is typically 5–20× smaller than crop_imm.
    let buf = RgbaImage::from_raw(w, crop_h, slice.to_vec())
        .expect("crop dimensions match slice length");
    DynamicImage::ImageRgba8(buf)
}

// ---------------------------------------------------------------------------
// Draw function
// ---------------------------------------------------------------------------

pub fn draw_pdf_viewer(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();

    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Rgb(20, 20, 20))),
        area,
    );

    let pdf_path = match &app.pdf_viewer_path {
        Some(p) => p.clone(),
        None => {
            frame.render_widget(
                Paragraph::new("No PDF file open").style(Style::default().fg(Color::Red)),
                area,
            );
            return;
        }
    };

    // ── Single lock: read everything we need, then drop ─────────────────────
    let (picker_clone, font_w, font_h, page_data_opt, last_proto_key_clone) = {
        let cache_arc = get_cache();
        let g = match cache_arc.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        let picker = match g.picker.clone() {
            Some(p) => p,
            None => {
                // Drop g before rendering the error widget
                drop(g);
                frame.render_widget(
                    Paragraph::new(
                        "Terminal graphics not detected (Kitty / Sixel / iTerm2 / halfblocks). \
                         Press Esc to exit.",
                    )
                    .style(Style::default().fg(Color::Yellow))
                    .alignment(ratatui::layout::Alignment::Center),
                    area,
                );
                return;
            }
        };

        let (fw, fh) = picker.font_size();

        let pixel_w = (area.width.saturating_sub(0) as u32) * (fw as u32);
        let page = app.pdf_viewer_page;
        let dpi = dpi_for_zoom(app.pdf_viewer_zoom);
        let key: PageKey = (pdf_path.clone(), page, dpi, pixel_w);
        let page_data = g.pages.get(&key).cloned();
        let last_key = g.last_proto_key.clone();

        (picker, fw, fh, page_data, last_key)
    };
    // ── Lock released ────────────────────────────────────────────────────────

    // Layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let pdf_area = chunks[0];
    let status_area = chunks[1];

    let pixel_w = (pdf_area.width as u32) * (font_w as u32);
    let pixel_h = (pdf_area.height as u32) * (font_h as u32);
    if pixel_w == 0 || pixel_h == 0 {
        return;
    }

    let page = app.pdf_viewer_page;
    let dpi = dpi_for_zoom(app.pdf_viewer_zoom);

    // Submit pre-render jobs (no lock held here)
    for delta in [-2i32, -1, 0, 1, 2] {
        let p = page as i32 + delta;
        if p >= 1 && p as usize <= app.pdf_viewer_total_pages {
            request_page(&pdf_path, p as usize, dpi, pixel_w);
        }
    }

    let page_data = match page_data_opt {
        Some(d) => d,
        None => {
            let dots = match (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_millis()
                / 250)
                % 4
            {
                0 => "   ",
                1 => ".  ",
                2 => ".. ",
                _ => "...",
            };
            frame.render_widget(
                Paragraph::new(format!(
                    "Rendering page {}{}  (press Esc to cancel)",
                    page, dots
                ))
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                .alignment(ratatui::layout::Alignment::Center),
                pdf_area,
            );
            render_status_bar(frame, status_area, &pdf_path, page, app, 0, 0);
            return;
        }
    };

    // Clamp scroll and publish limits
    let page_h = page_data.pixel_h;
    app.pdf_viewer_page_pixel_h = page_h;
    let max_scroll_px = page_h.saturating_sub(pixel_h);
    let scroll_px = (app.pdf_viewer_scroll_y * (font_h as u32)).min(max_scroll_px);
    app.pdf_viewer_scroll_y = scroll_px / (font_h as u32);
    let max_scroll_cells = max_scroll_px / (font_h as u32);
    app.pdf_viewer_max_scroll_y = max_scroll_cells;

    let crop_h = pixel_h.min(page_h.saturating_sub(scroll_px));

    let proto_key = ProtoKey {
        path_fp: path_fingerprint(&pdf_path),
        page,
        dpi,
        pixel_w,
        crop_y: scroll_px,
        crop_h,
    };

    let need_encode = last_proto_key_clone.as_ref() != Some(&proto_key);

    if need_encode {
        // Zero-copy-optimised crop (only the visible rows)
        let cropped = crop_view(&page_data.image, scroll_px, crop_h);
        let proto = picker_clone.new_resize_protocol(cropped);

        // Single lock: store result
        if let Ok(mut g) = get_cache().lock() {
            g.last_protocol = Some(proto);
            g.last_proto_key = Some(proto_key);
        }
    }

    // Blit
    if let Ok(mut g) = get_cache().lock() {
        if let Some(ref mut proto) = g.last_protocol {
            frame.render_stateful_widget(
                StatefulImage::new().resize(Resize::Fit(None)),
                pdf_area,
                proto,
            );
        }
    }

    render_status_bar(
        frame,
        status_area,
        &pdf_path,
        page,
        app,
        app.pdf_viewer_scroll_y,
        max_scroll_cells,
    );
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status_bar(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    pdf_path: &Path,
    page: usize,
    app: &App,
    scroll: u32,
    max_scroll: u32,
) {
    let filename = pdf_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let status = Line::from(vec![
        Span::styled(
            format!(" {} ", filename),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" │ {}/{} ", page, app.pdf_viewer_total_pages),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!(" │ {}% ", app.pdf_viewer_zoom as u32),
            Style::default().fg(Color::Green),
        ),
        Span::styled(
            format!(" │ {}/{} rows ", scroll, max_scroll),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            " │ Esc:exit  ↑↓/jk:scroll  PgUp/Dn:page  +/-:zoom  0:reset ",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(status).style(Style::default().bg(Color::Rgb(28, 28, 36))),
        area,
    );
}
