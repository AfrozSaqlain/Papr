//! Internal PDF viewer with asynchronous page rendering and a multi-page cache.
//!
//! # Bug-fix: path-scoped cache keys
//!
//! Every cache lookup is keyed by **`(path, page, dpi, pixel_w)`** so that
//! opening a second PDF never serves images from a previously opened document.
//! The temp PNG file names on disk are similarly keyed by a hash of the path,
//! so two different PDFs at the same page/DPI never share a file.
//!
//! When a new document is opened, `reset_for_new_document` must be called
//! **before** entering `AppMode::PdfView`.  It flushes all per-document state
//! (rendered pages, in-flight requests, encoded terminal protocol) that belongs
//! to any other path, ensuring a clean slate without evicting the `Picker`
//! which is expensive to recreate.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use anyhow::{Context, Result};
use image::DynamicImage;
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

/// A short, collision-resistant fingerprint of a path used as part of temp
/// file names and as the discriminator in the page-cache map.
fn path_fingerprint(path: &Path) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Page cache
// ---------------------------------------------------------------------------

/// One rendered + scaled page for a specific document / DPI / width.
#[derive(Clone)]
struct RenderedPage {
    /// Pixel height of the scaled image.
    pixel_h: u32,
    /// The decoded full-page image, already scaled to `pixel_w`.
    image: Arc<DynamicImage>,
}

/// Complete page-cache key – includes the document path so pages from
/// different PDFs never collide.
type PageKey = (PathBuf, usize, u32, u32); // (path, page, dpi, pixel_w)

/// All state that lives in the global cache singleton.
struct PageCache {
    /// `Picker` is created once and reused across documents (it queries the
    /// terminal once and the result does not change between documents).
    picker: Option<Picker>,
    /// Rendered pages, keyed by (path, page, dpi, pixel_w).
    pages: HashMap<PageKey, RenderedPage>,
    /// Render jobs currently executing in the background.
    in_flight: std::collections::HashSet<PageKey>,
    /// Temp PNG files on disk that must be deleted on exit.
    temp_files: Vec<PathBuf>,
    /// The last viewport that was encoded into a terminal protocol.
    last_proto_key: Option<ProtoKey>,
    /// The encoded terminal-protocol image ready to blit.
    last_protocol: Option<StatefulProtocol>,
}

/// Identifies the exact visible slice so we can skip re-encoding unchanged frames.
/// **Includes the document path** so switching PDFs always re-encodes.
#[derive(PartialEq, Clone)]
struct ProtoKey {
    /// Short hash of the path (avoids cloning the whole PathBuf every frame).
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
// Public API – document lifecycle
// ---------------------------------------------------------------------------

/// **Call this every time a new PDF is opened**, before entering PdfView mode.
///
/// Flushes all rendered pages and in-flight requests that belong to any
/// *other* path, and invalidates the cached terminal protocol.  Pages for
/// `new_path` itself (if any are still valid) are kept so that reopening the
/// same PDF is instant.
pub fn reset_for_new_document(new_path: &Path) {
    let cache = get_cache();
    if let Ok(mut guard) = cache.lock() {
        // Evict pages / in-flight entries that belong to a different document.
        guard
            .pages
            .retain(|(path, _, _, _), _| path.as_path() == new_path);
        guard
            .in_flight
            .retain(|(path, _, _, _)| path.as_path() == new_path);
        // Always invalidate the encoded protocol – even if the same PDF is
        // reopened, the scroll position has been reset to 0 by open_pdf, so
        // the old protocol would show the wrong crop.
        guard.last_proto_key = None;
        guard.last_protocol = None;
    }
}

/// Clean up all temporary PNG files.  Call once on application exit.
pub fn cleanup_temp_files() {
    if let Some(cache_arc) = CACHE.get() {
        if let Ok(mut guard) = cache_arc.lock() {
            for path in guard.temp_files.drain(..) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Evict pages that are far from `current_page` for the given document to
/// keep memory reasonable while the viewer is open.
pub fn evict_distant_pages(current_page: usize) {
    if let Some(cache_arc) = CACHE.get() {
        if let Ok(mut guard) = cache_arc.lock() {
            guard
                .pages
                .retain(|(_, page, _, _), _| (*page as isize - current_page as isize).abs() <= 3);
        }
    }
}

// ---------------------------------------------------------------------------
// Background rendering
// ---------------------------------------------------------------------------

fn dpi_for_zoom(zoom: f64) -> u32 {
    // 150 dpi @ 100 % zoom
    ((150.0 * zoom / 100.0) as u32).max(72)
}

/// Submit a background render job for `(path, page, dpi, pixel_w)` unless
/// the result is already cached or a job is already running.
fn request_page(
    pdf_path: &Path,
    page: usize,
    dpi: u32,
    pixel_w: u32,
    cache: &Arc<Mutex<PageCache>>,
) {
    let key: PageKey = (pdf_path.to_path_buf(), page, dpi, pixel_w);

    {
        let guard = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if guard.pages.contains_key(&key) || guard.in_flight.contains(&key) {
            return;
        }
    }

    {
        let mut guard = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        guard.in_flight.insert(key.clone());
    }

    let cache_arc = cache.clone();
    let pdf_path = pdf_path.to_path_buf();

    thread::spawn(move || {
        // Build the temp-file path inside the worker so the path fingerprint
        // is computed off the heap without any locking.
        let fp = path_fingerprint(&pdf_path);
        let temp_dir = std::env::temp_dir();
        let prefix = temp_dir.join(format!(
            "papr_pdf_{}_fp{:x}_p{}_d{}",
            std::process::id(),
            fp,
            page,
            dpi
        ));
        let temp_file = prefix.with_extension("png");

        let result = render_page_blocking(&pdf_path, page, dpi, pixel_w, &prefix, &temp_file);

        if let Ok(mut guard) = cache_arc.lock() {
            guard.in_flight.remove(&key);
            match result {
                Ok(page_data) => {
                    guard.pages.insert(key, page_data);
                }
                Err(_) => {} // leave absent; draw function will retry next frame
            }
            if !guard.temp_files.contains(&temp_file) {
                guard.temp_files.push(temp_file);
            }
        }
    });
}

/// Blocking render: runs `pdftoppm`, reads the PNG, scales to `pixel_w`.
/// The temp-file `prefix` and `png_path` are passed in so the caller
/// controls the file-naming scheme (and can incorporate the path fingerprint).
fn render_page_blocking(
    pdf_path: &Path,
    page: usize,
    dpi: u32,
    pixel_w: u32,
    prefix: &Path,
    png_path: &Path,
) -> Result<RenderedPage> {
    // Only invoke pdftoppm when the PNG for *this specific document* does not
    // exist yet.  Because `prefix` includes the path fingerprint, two
    // different PDFs will always produce different file names.
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

    let img = image::open(png_path).context("failed to open rendered PNG")?;
    let img_w = img.width();
    let img_h = img.height();
    if img_w == 0 || pixel_w == 0 {
        anyhow::bail!("zero-width image or pixel_w");
    }

    // Scale width to exactly `pixel_w`, preserving aspect ratio.
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
// The draw function – called every frame from ui::render
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

    let cache = get_cache();

    // Check picker availability without holding the lock during rendering.
    let picker_available = match cache.lock() {
        Ok(g) => g.picker.is_some(),
        Err(_) => false,
    };
    if !picker_available {
        frame.render_widget(
            Paragraph::new(
                "Terminal graphics not detected (needs Kitty / Sixel / iTerm2 / halfblocks). \
                 Press Esc to exit.",
            )
            .style(Style::default().fg(Color::Yellow))
            .alignment(ratatui::layout::Alignment::Center),
            area,
        );
        return;
    }

    // Layout: PDF area + 1-line status bar
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    let pdf_area = chunks[0];
    let status_area = chunks[1];

    // Font size in pixels per terminal cell
    let (font_w, font_h) = {
        let g = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        g.picker.as_ref().map(|p| p.font_size()).unwrap_or((10, 20))
    };

    let pixel_w = (pdf_area.width as u32) * (font_w as u32);
    let pixel_h = (pdf_area.height as u32) * (font_h as u32);
    if pixel_w == 0 || pixel_h == 0 {
        return;
    }

    let page = app.pdf_viewer_page;
    let zoom = app.pdf_viewer_zoom;
    let dpi = dpi_for_zoom(zoom);

    // Submit current page + neighbours for background rendering.
    // All keys include `pdf_path` so they are scoped to the current document.
    for delta in [-2i32, -1, 0, 1, 2] {
        let p = page as i32 + delta;
        if p >= 1 && p as usize <= app.pdf_viewer_total_pages {
            request_page(&pdf_path, p as usize, dpi, pixel_w, &cache);
        }
    }

    // Resolve the current page from cache.
    let key: PageKey = (pdf_path.clone(), page, dpi, pixel_w);
    let page_data: Option<RenderedPage> = {
        let g = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        g.pages.get(&key).cloned()
    };

    let page_data = match page_data {
        Some(p) => p,
        None => {
            // Still rendering – show a non-blocking spinner.
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
                .style(
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )
                .alignment(ratatui::layout::Alignment::Center),
                pdf_area,
            );
            render_status_bar(frame, status_area, &pdf_path, page, app, 0, 0);
            return;
        }
    };

    // Clamp scroll offset to the rendered page height
    let page_h = page_data.pixel_h;
    // Expose height so key handler can compute page-boundary crossings.
    app.pdf_viewer_page_pixel_h = page_h;
    let max_scroll_px: u32 = page_h.saturating_sub(pixel_h);
    let scroll_px = (app.pdf_viewer_scroll_y * (font_h as u32)).min(max_scroll_px);
    // Keep the cell-level counter in sync with the clamped pixel offset.
    app.pdf_viewer_scroll_y = scroll_px / (font_h as u32);
    let max_scroll_cells = max_scroll_px / (font_h as u32);
    // Publish the maximum so pdf_scroll can compare in the same unit (cell rows).
    app.pdf_viewer_max_scroll_y = max_scroll_cells;


    // Visible crop rectangle
    let crop_h = pixel_h.min(page_h.saturating_sub(scroll_px));

    // Re-encode the terminal protocol only when the visible slice has changed.
    // The key includes `path_fingerprint` so switching documents always
    // triggers a re-encode.
    let proto_key = ProtoKey {
        path_fp: path_fingerprint(&pdf_path),
        page,
        dpi,
        pixel_w,
        crop_y: scroll_px,
        crop_h,
    };

    let need_encode = {
        let g = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        g.last_proto_key.as_ref() != Some(&proto_key)
    };

    if need_encode {
        let cropped = page_data.image.crop_imm(0, scroll_px, pixel_w, crop_h);

        let picker_clone = {
            let g = match cache.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            g.picker.clone()
        };

        if let Some(picker) = picker_clone {
            let proto = picker.new_resize_protocol(cropped);
            let mut g = match cache.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            g.last_protocol = Some(proto);
            g.last_proto_key = Some(proto_key);
        }
    }

    // Blit the (potentially cached) protocol to the frame.
    {
        let mut g = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if let Some(ref mut proto) = g.last_protocol {
            let widget = StatefulImage::new().resize(Resize::Fit(None));
            frame.render_stateful_widget(widget, pdf_area, proto);
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
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
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
            " │ Esc:exit  ↑↓:scroll  PgUp/Dn:jump  +/-:zoom  0:reset ",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(status).style(Style::default().bg(Color::Rgb(28, 28, 36))),
        area,
    );
}
