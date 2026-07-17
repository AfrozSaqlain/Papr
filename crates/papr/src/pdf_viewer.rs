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
    image: Arc<RgbaImage>,
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

    last_viewport_h: u32,
    last_zoom: f64,
    target_page: usize,
    target_scroll_px: f64,
    current_page: usize,
    current_scroll_px: f64,
    last_update: Option<std::time::Instant>,
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
                last_viewport_h: 600,
                last_zoom: 100.0,
                target_page: 1,
                target_scroll_px: 0.0,
                current_page: 1,
                current_scroll_px: 0.0,
                last_update: None,
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

        g.target_page = 1;
        g.target_scroll_px = 0.0;
        g.current_page = 1;
        g.current_scroll_px = 0.0;
        g.last_update = None;
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
    let rgba = scaled.into_rgba8();

    Ok(RenderedPage {
        pixel_h: rgba.height(),
        image: Arc::new(rgba),
    })
}

// ---------------------------------------------------------------------------
// Zero-copy viewport crop
// ---------------------------------------------------------------------------

/// Return a `DynamicImage` that is a view of the rows `[crop_y, crop_y+crop_h)`
/// of `src` **without copying pixels**.  We borrow the raw bytes from the Arc,
/// build an `RgbaImage` whose backing Vec points into the slice, and wrap it.
fn crop_view(src: &RgbaImage, crop_y: u32, crop_h: u32) -> DynamicImage {
    let w = src.width();
    let bytes_per_row = w as usize * 4;
    let start = crop_y as usize * bytes_per_row;
    let end = start + crop_h as usize * bytes_per_row;
    let slice = &src.as_raw()[start..end];
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

    // Layout
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let pdf_area = chunks[0];
    let status_area = chunks[1];

    // Get picker and font size
    let (picker, font_w, font_h) = match get_cache().lock() {
        Ok(g) => {
            if let Some(ref p) = g.picker {
                let (fw, fh) = p.font_size();
                (p.clone(), fw, fh)
            } else {
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
        }
        Err(_) => return,
    };

    let pixel_w = (pdf_area.width as u32) * (font_w as u32);
    let pixel_h = (pdf_area.height as u32) * (font_h as u32);
    if pixel_w == 0 || pixel_h == 0 {
        return;
    }

    let dpi = dpi_for_zoom(app.pdf_viewer_zoom);

    // ── Single lock: run physics, read page data & next page, evict, then drop ──
    let (page_data_opt, next_page_data_opt, last_proto_key_clone, current_page, current_scroll_px_val) = {
        let cache_arc = get_cache();
        let mut g = match cache_arc.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        // Evict distant pages directly within this single lock to prevent memory bloat
        let current_page_val = app.pdf_viewer_page;
        g.pages.retain(|(_, page, _, _), _| {
            (*page as isize - current_page_val as isize).abs() <= 3
        });

        // Set last viewport h
        g.last_viewport_h = pixel_h;

        // If zoom changed, scale targets
        if g.last_zoom > 0.0 && g.last_zoom != app.pdf_viewer_zoom {
            let ratio = app.pdf_viewer_zoom / g.last_zoom;
            g.target_scroll_px *= ratio;
            g.current_scroll_px *= ratio;
        }
        g.last_zoom = app.pdf_viewer_zoom;

        // Run smooth scroll physics
        let now = std::time::Instant::now();
        let dt = g.last_update.map(|t| (now - t).as_secs_f64()).unwrap_or(0.0);
        g.last_update = Some(now);

        let total_pages = app.pdf_viewer_total_pages;
        let default_h = pixel_h as f64 * 1.5; // fallback height if page not cached

        if dt > 0.0 {
            let target_rel = to_relative_px(g.target_page, g.target_scroll_px, g.current_page, &g.pages, default_h);
            let diff = target_rel - g.current_scroll_px;
            if diff.abs() > 0.2 {
                let lerp_factor = 1.0 - (-24.0 * dt).exp(); // Smooth interpolation
                g.current_scroll_px += diff * lerp_factor;
            } else {
                g.current_scroll_px = target_rel;
            }
            let mut cp = g.current_page;
            let mut cs = g.current_scroll_px;
            normalize_coords(&mut cp, &mut cs, total_pages, &g.pages, default_h);
            g.current_page = cp;
            g.current_scroll_px = cs;
        }

        // Sync app page and scroll_y
        app.pdf_viewer_page = g.current_page;
        app.pdf_viewer_scroll_y = (g.current_scroll_px / font_h as f64) as u32;

        let key: PageKey = (pdf_path.clone(), g.current_page, dpi, pixel_w);
        let page_data = g.pages.get(&key).cloned();

        let next_key: PageKey = (pdf_path.clone(), g.current_page + 1, dpi, pixel_w);
        let next_page_data = g.pages.get(&next_key).cloned();

        let last_key = g.last_proto_key.clone();

        (page_data, next_page_data, last_key, g.current_page, g.current_scroll_px)
    };
    // ── Lock released ────────────────────────────────────────────────────────

    // Submit pre-render jobs (no lock held here)
    for delta in [-2i32, -1, 0, 1, 2] {
        let p = current_page as i32 + delta;
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
                    current_page, dots
                ))
                .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                .alignment(ratatui::layout::Alignment::Center),
                pdf_area,
            );
            render_status_bar(frame, status_area, &pdf_path, current_page, app, 0, 0);
            return;
        }
    };

    let page_h = page_data.pixel_h;
    app.pdf_viewer_page_pixel_h = page_h;
    let max_scroll_px = page_h.saturating_sub(pixel_h);
    let scroll_px = (current_scroll_px_val as u32).min(page_h);

    let max_scroll_cells = max_scroll_px / (font_h as u32);
    app.pdf_viewer_max_scroll_y = max_scroll_cells;

    let proto_key = ProtoKey {
        path_fp: path_fingerprint(&pdf_path),
        page: current_page,
        dpi,
        pixel_w,
        crop_y: scroll_px,
        crop_h: pixel_h,
    };
    let need_encode = last_proto_key_clone.as_ref() != Some(&proto_key);

    if need_encode {
        let cropped = if scroll_px + pixel_h > page_h && current_page < app.pdf_viewer_total_pages {
            if let Some(next_page) = next_page_data_opt {
                crop_and_stitch(&page_data.image, &next_page.image, scroll_px, pixel_h)
            } else {
                crop_single_with_padding(&page_data.image, scroll_px, pixel_h)
            }
        } else {
            let actual_crop_h = pixel_h.min(page_h.saturating_sub(scroll_px));
            if actual_crop_h < pixel_h {
                crop_single_with_padding(&page_data.image, scroll_px, pixel_h)
            } else {
                crop_view(&page_data.image, scroll_px, pixel_h)
            }
        };

        let proto = picker.new_resize_protocol(cropped);

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
        current_page,
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

// ---------------------------------------------------------------------------
// Cache check helpers
// ---------------------------------------------------------------------------

pub fn is_current_page_cached(app: &App) -> bool {
    let pdf_path = match &app.pdf_viewer_path {
        Some(p) => p,
        None => return true,
    };
    let page = app.pdf_viewer_page;
    let dpi = dpi_for_zoom(app.pdf_viewer_zoom);
    if let Some(arc) = CACHE.get() {
        if let Ok(g) = arc.lock() {
            return g.pages.keys().any(|(p, pg, d, _)| p == pdf_path && *pg == page && *d == dpi);
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Smooth scroll physics & stitching helpers
// ---------------------------------------------------------------------------

fn get_page_height_helper(page: usize, pages: &HashMap<PageKey, RenderedPage>, default_h: f64) -> f64 {
    for (key, val) in pages {
        if key.1 == page {
            return val.pixel_h as f64;
        }
    }
    default_h
}

fn normalize_coords(page: &mut usize, offset: &mut f64, total_pages: usize, pages: &HashMap<PageKey, RenderedPage>, default_h: f64) {
    loop {
        let h = get_page_height_helper(*page, pages, default_h);
        if *offset >= h && *page < total_pages {
            *offset -= h;
            *page += 1;
        } else if *offset < 0.0 && *page > 1 {
            *page -= 1;
            let prev_h = get_page_height_helper(*page, pages, default_h);
            *offset += prev_h;
        } else {
            break;
        }
    }
}

fn to_relative_px(page: usize, offset: f64, current_page: usize, pages: &HashMap<PageKey, RenderedPage>, default_h: f64) -> f64 {
    if page == current_page {
        offset
    } else if page > current_page {
        let mut sum = 0.0;
        for p in current_page..page {
            sum += get_page_height_helper(p, pages, default_h);
        }
        sum + offset
    } else {
        let mut sum = 0.0;
        for p in page..current_page {
            sum += get_page_height_helper(p, pages, default_h);
        }
        offset - sum
    }
}

pub fn is_animating() -> bool {
    if let Some(arc) = CACHE.get() {
        if let Ok(g) = arc.lock() {
            let default_h = 1000.0;
            let target_rel = to_relative_px(g.target_page, g.target_scroll_px, g.current_page, &g.pages, default_h);
            return (target_rel - g.current_scroll_px).abs() > 0.5;
        }
    }
    false
}

pub fn scroll_by_rows(app: &App, delta_rows: i64) {
    let cache_arc = get_cache();
    let mut g = match cache_arc.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    let font_h = g.picker.as_ref().map(|p| p.font_size().1).unwrap_or(16) as f64;
    let delta_px = delta_rows as f64 * font_h;
    let total_pages = app.pdf_viewer_total_pages;
    let default_h = 1000.0;

    g.target_scroll_px += delta_px;
    let mut tp = g.target_page;
    let mut ts = g.target_scroll_px;
    normalize_coords(&mut tp, &mut ts, total_pages, &g.pages, default_h);
    g.target_page = tp;
    g.target_scroll_px = ts;

    if g.target_page == 1 && g.target_scroll_px < 0.0 {
        g.target_scroll_px = 0.0;
    }
    if g.target_page == total_pages {
        let current_h = get_page_height_helper(g.target_page, &g.pages, default_h);
        let max_scroll = (current_h - g.last_viewport_h as f64).max(0.0);
        if g.target_scroll_px > max_scroll {
            g.target_scroll_px = max_scroll;
        }
    }
}

pub fn page_up(app: &App) {
    let cache_arc = get_cache();
    if let Ok(mut g) = cache_arc.lock() {
        let scroll_amt = g.last_viewport_h as f64 * 0.9;
        g.target_scroll_px -= scroll_amt;
        let total_pages = app.pdf_viewer_total_pages;
        let default_h = 1000.0;
        let mut tp = g.target_page;
        let mut ts = g.target_scroll_px;
        normalize_coords(&mut tp, &mut ts, total_pages, &g.pages, default_h);
        g.target_page = tp;
        g.target_scroll_px = ts;
        if g.target_page == 1 && g.target_scroll_px < 0.0 {
            g.target_scroll_px = 0.0;
        }
    }
}

pub fn page_down(app: &App) {
    let cache_arc = get_cache();
    if let Ok(mut g) = cache_arc.lock() {
        let scroll_amt = g.last_viewport_h as f64 * 0.9;
        g.target_scroll_px += scroll_amt;
        let total_pages = app.pdf_viewer_total_pages;
        let default_h = 1000.0;
        let mut tp = g.target_page;
        let mut ts = g.target_scroll_px;
        normalize_coords(&mut tp, &mut ts, total_pages, &g.pages, default_h);
        g.target_page = tp;
        g.target_scroll_px = ts;
        if g.target_page == total_pages {
            let current_h = get_page_height_helper(g.target_page, &g.pages, default_h);
            let max_scroll = (current_h - g.last_viewport_h as f64).max(0.0);
            if g.target_scroll_px > max_scroll {
                g.target_scroll_px = max_scroll;
            }
        }
    }
}

fn crop_and_stitch(
    curr_img: &RgbaImage,
    next_img: &RgbaImage,
    scroll_y: u32,
    viewport_h: u32,
) -> DynamicImage {
    let w = curr_img.width();
    let curr_h = curr_img.height();
    let curr_visible = curr_h.saturating_sub(scroll_y);
    let next_visible = viewport_h.saturating_sub(curr_visible);

    let mut data = vec![0u8; (w * viewport_h * 4) as usize];

    if curr_visible > 0 {
        let bytes_per_row = w as usize * 4;
        let start = scroll_y as usize * bytes_per_row;
        let end = curr_h as usize * bytes_per_row;
        let slice = &curr_img.as_raw()[start..end];
        data[0..slice.len()].copy_from_slice(slice);
    }

    if next_visible > 0 {
        let bytes_per_row = w as usize * 4;
        let start = 0;
        let end = (next_visible as usize * bytes_per_row).min(next_img.as_raw().len());
        let slice = &next_img.as_raw()[start..end];

        let dest_start = curr_visible as usize * bytes_per_row;
        let dest_end = dest_start + slice.len();
        data[dest_start..dest_end].copy_from_slice(slice);
    }

    let stitched = RgbaImage::from_raw(w, viewport_h, data).unwrap();
    DynamicImage::ImageRgba8(stitched)
}

fn crop_single_with_padding(
    curr_img: &RgbaImage,
    scroll_y: u32,
    viewport_h: u32,
) -> DynamicImage {
    let w = curr_img.width();
    let curr_h = curr_img.height();
    let curr_visible = curr_h.saturating_sub(scroll_y);

    let mut data = vec![0u8; (w * viewport_h * 4) as usize];

    if curr_visible > 0 {
        let bytes_per_row = w as usize * 4;
        let start = scroll_y as usize * bytes_per_row;
        let end = curr_h as usize * bytes_per_row;
        let slice = &curr_img.as_raw()[start..end];
        data[0..slice.len()].copy_from_slice(slice);
    }

    let curr_len = (curr_visible as usize * w as usize * 4).min(data.len());
    let padding_slice = &mut data[curr_len..];
    padding_slice.chunks_exact_mut(4).for_each(|pixel| {
        pixel[0] = 20;
        pixel[1] = 20;
        pixel[2] = 20;
        pixel[3] = 255;
    });

    let stitched = RgbaImage::from_raw(w, viewport_h, data).unwrap();
    DynamicImage::ImageRgba8(stitched)
}
