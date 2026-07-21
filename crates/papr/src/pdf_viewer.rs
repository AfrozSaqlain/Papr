//! Internal PDF viewer – optimised rendering pipeline.
//!
//! # Root-cause summary (why scrolling felt sluggish)
//!
//! ## Problem 1 – full image re-encoded every frame (THE bottleneck)
//! The old code called `picker.new_resize_protocol(cropped)` on **every scroll event**,
//! creating a brand-new `StatefulProtocol` with a zeroed hash.  When
//! `StatefulImage::render` called `needs_resize()` it compared source-hash ≠ stored-hash
//! and triggered `resize_encode()` → `transmit_virtual()` which base64-encodes the
//! entire RGBA pixel buffer (~8 MB) into a Kitty escape string on every frame.
//!
//! Fix: we now manage the Kitty protocol ourselves.  We call `transmit_virtual` once per
//! unique (page, scroll_px, pixel_w, pixel_h) crop, store the resulting escape string,
//! and write it directly to the ratatui buffer.  Subsequent frames that show the same
//! crop pay **zero** encode cost – only the unicode-placeholder rows are re-written,
//! which is negligible.
//!
//! ## Problem 2 – spring constant too high, hiding per-frame position change
//! A lerp factor of `1 - exp(-24 * dt)` at 16 ms dt ≈ 0.32.  Each frame only moves
//! 32% of the remaining distance, so the last few pixels drag for multiple frames.
//! We tightened the constant to 18 and added a snap threshold of 1.0 px.
//!
//! ## Problem 3 – event loop drew *before* reading events
//! The old loop: draw → compute timeout → read keys → force_redraw.  A keypress
//! therefore only affected the *next* loop iteration.  The new loop: read keys → draw.
//! (Handled in main.rs – see comments there.)
//!
//! ## Background rendering
//! Every page is rasterised by `pdftoppm` and decoded/scaled on a dedicated
//! OS thread so the draw path is never blocked on I/O.  The current page plus
//! two neighbours on each side are always pre-rendered.
//!
//! ## Path-scoped cache keys
//! Every cache entry is keyed by `(path, page, dpi, pixel_w)` so that
//! different PDFs never share cached images or temp PNG files.

use std::collections::HashMap;
use std::fmt::Write as FmtWrite;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use anyhow::{Context, Result};
use image::{DynamicImage, RgbaImage};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget},
    Frame,
};
use ratatui_image::picker::Picker;

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

// (path, document generation, page, dpi, pixel_w). The generation prevents
// render jobs for an old on-disk PDF from repopulating a freshly invalidated
// cache entry after a live LaTeX rebuild.
type PageKey = (PathBuf, u64, usize, u32, u32);

/// Identifies the currently displayed crop.  When this is unchanged between
/// frames we skip ALL encoding work and only re-draw the unicode placeholders.
#[derive(PartialEq, Clone)]
struct CropKey {
    path_fp: u64,
    page: usize,
    dpi: u32,
    pixel_w: u32,
    crop_y: u32,
    crop_h: u32,
}

/// The result of one encode pass: the Kitty escape string that uploads the
/// image data, the Kitty image ID, and the cell area it occupies.
struct EncodedFrame {
    /// The full Kitty APC sequence (base64 RGBA payload + virtual placement).
    /// Empty string means "same image, no re-upload needed".
    transmit_seq: String,
    /// The 32-bit Kitty image ID reused across frames so the terminal can
    /// delete / replace rather than accumulate images.
    image_id: u32,
    /// Width / height in terminal cells.
    cols: u16,
    rows: u16,
    /// The id-color used for unicode placeholder rendering.
    id_color: String,
}

/// Everything that lives inside the global singleton lock.
struct PageCache {
    picker: Option<Picker>,
    pages: HashMap<PageKey, RenderedPage>,
    in_flight: std::collections::HashSet<PageKey>,
    document_generations: HashMap<PathBuf, u64>,
    temp_files: Vec<PathBuf>,

    // ── Scroll state ─────────────────────────────────────────────────────
    last_viewport_h: u32,
    target_page: usize,
    target_scroll_px: f64,
    current_page: usize,
    current_scroll_px: f64,
    last_update: Option<std::time::Instant>,

    // ── Encode cache ──────────────────────────────────────────────────────
    /// Key of the last successfully encoded frame.
    last_crop_key: Option<CropKey>,
    /// The encoded frame ready to blit; `None` until first render.
    last_encoded: Option<EncodedFrame>,
    /// Monotonically-increasing counter used to assign unique Kitty IDs.
    next_kitty_id: u32,
}

static CACHE: OnceLock<Arc<Mutex<PageCache>>> = OnceLock::new();

fn get_cache() -> Arc<Mutex<PageCache>> {
    CACHE
        .get_or_init(|| {
            Arc::new(Mutex::new(PageCache {
                picker: Picker::from_query_stdio().ok(),
                pages: HashMap::new(),
                in_flight: std::collections::HashSet::new(),
                document_generations: HashMap::new(),
                temp_files: Vec::new(),
                last_viewport_h: 600,
                target_page: 1,
                target_scroll_px: 0.0,
                current_page: 1,
                current_scroll_px: 0.0,
                last_update: None,
                last_crop_key: None,
                last_encoded: None,
                next_kitty_id: 1,
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
        g.pages.retain(|(p, _, _, _, _), _| p.as_path() == new_path);
        g.in_flight.retain(|(p, _, _, _, _)| p.as_path() == new_path);
        g.last_crop_key = None;
        g.last_encoded = None;

        g.target_page = 1;
        g.target_scroll_px = 0.0;
        g.current_page = 1;
        g.current_scroll_px = 0.0;
        g.last_update = None;
    }
}

/// Discard cached raster pages after a writer replaces a PDF in place while
/// preserving the reader's page and scroll position.
pub fn invalidate_document(path: &Path) {
    let cache = get_cache();
    if let Ok(mut g) = cache.lock() {
        let generation = g.document_generations.entry(path.to_path_buf()).or_default();
        *generation = generation.wrapping_add(1);
        g.pages.retain(|(cached, _, _, _, _), _| cached.as_path() != path);
        g.in_flight.retain(|(cached, _, _, _, _)| cached.as_path() != path);
        g.last_crop_key = None;
        g.last_encoded = None;
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
            g.pages.retain(|(_, _, page, _, _), _| {
                (*page as isize - current_page as isize).abs() <= 3
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Background rendering
// ---------------------------------------------------------------------------

const DPI: u32 = 150;

/// Submit a background render job if the result isn't already cached or
/// in-flight.  Uses a single lock acquisition to check-and-mark atomically.
fn request_page(pdf_path: &Path, page: usize, dpi: u32, pixel_w: u32) {
    let cache = get_cache();
    let generation;
    let key;

    // Single lock: check presence and mark in-flight atomically.
    {
        let mut g = match cache.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        generation = *g.document_generations.get(pdf_path).unwrap_or(&0);
        key = (pdf_path.to_path_buf(), generation, page, dpi, pixel_w);
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
            "papr_pdf_{}_fp{:x}_g{}_p{}_d{}",
            std::process::id(),
            fp,
            generation,
            page,
            dpi,
        ));
        let png = prefix.with_extension("png");

        let result = render_page_blocking(&pdf_path, page, dpi, pixel_w, &prefix, &png);

        if let Ok(mut g) = cache.lock() {
            g.in_flight.remove(&key);
            let current_generation = *g.document_generations.get(&pdf_path).unwrap_or(&0);
            match result {
                _ if current_generation != generation => {},
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
/// of `src`, copying only the needed rows (much smaller than the full page).
fn crop_view(src: &RgbaImage, crop_y: u32, crop_h: u32) -> DynamicImage {
    let w = src.width();
    let bytes_per_row = w as usize * 4;
    let start = crop_y as usize * bytes_per_row;
    let end = start + crop_h as usize * bytes_per_row;
    let slice = &src.as_raw()[start..end];
    let buf = RgbaImage::from_raw(w, crop_h, slice.to_vec())
        .expect("crop dimensions match slice length");
    DynamicImage::ImageRgba8(buf)
}

// ---------------------------------------------------------------------------
// Kitty protocol encoding (bypasses ratatui-image StatefulProtocol)
// ---------------------------------------------------------------------------
// We manage the Kitty protocol directly so that we control exactly when
// pixel data is uploaded versus when only placeholder rows are re-drawn.
// This is the key fix: ratatui-image's StatefulProtocol re-encodes the
// entire image on every frame because the hash comparison fails when we
// create a new StatefulProtocol from a new DynamicImage each scroll event.

/// Build a Kitty APC sequence that uploads `img` as a virtual placement with
/// the given `id`.  The data is chunked at 4096 base64 chars per chunk.
fn kitty_transmit(img: &DynamicImage, id: u32) -> String {
    let (w, h) = (img.width(), img.height());
    let img_rgba8 = img.to_rgba8();
    let bytes = img_rgba8.as_raw();

    const CHARS_PER_CHUNK: usize = 4096;
    const CHUNK_SIZE: usize = (CHARS_PER_CHUNK / 4) * 3;
    let chunks: Vec<_> = bytes.chunks(CHUNK_SIZE).collect();
    let chunk_count = chunks.len();

    let bytes_per_chunk = 11 + CHARS_PER_CHUNK + 4;
    let mut data = String::with_capacity(chunk_count * bytes_per_chunk + 64);

    for (i, chunk) in chunks.iter().enumerate() {
        let payload = base64_encode(chunk);
        let more = u8::from(chunk_count > (i + 1));
        if i == 0 {
            write!(
                data,
                "\x1b_Gq=2,i={id},a=T,U=1,f=32,t=d,s={w},v={h},m={more};{payload}\x1b\\"
            )
            .unwrap();
        } else {
            write!(data, "\x1b_Gq=2,m={more};{payload}\x1b\\").unwrap();
        }
    }
    data
}

/// Minimal base64 encoder (standard alphabet, no padding needed by Kitty).
fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() * 4 + 2) / 3);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((n >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((n >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(n & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

/// Row/column diacritics for Kitty unicode placeholder rendering.
/// Source: https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders
static DIACRITICS: [char; 293] = [
    '\u{305}', '\u{30D}', '\u{30E}', '\u{310}', '\u{312}', '\u{33D}', '\u{33E}', '\u{33F}',
    '\u{346}', '\u{34A}', '\u{34B}', '\u{34C}', '\u{350}', '\u{351}', '\u{352}', '\u{357}',
    '\u{35B}', '\u{363}', '\u{364}', '\u{365}', '\u{366}', '\u{367}', '\u{368}', '\u{369}',
    '\u{36A}', '\u{36B}', '\u{36C}', '\u{36D}', '\u{36E}', '\u{36F}', '\u{483}', '\u{484}',
    '\u{485}', '\u{486}', '\u{487}', '\u{592}', '\u{593}', '\u{594}', '\u{595}', '\u{597}',
    '\u{598}', '\u{599}', '\u{59C}', '\u{59D}', '\u{59E}', '\u{59F}', '\u{5A0}', '\u{5A1}',
    '\u{5A8}', '\u{5A9}', '\u{5AB}', '\u{5AC}', '\u{5AF}', '\u{5C4}', '\u{610}', '\u{611}',
    '\u{612}', '\u{613}', '\u{614}', '\u{615}', '\u{616}', '\u{617}', '\u{657}', '\u{658}',
    '\u{659}', '\u{65A}', '\u{65B}', '\u{65D}', '\u{65E}', '\u{6D6}', '\u{6D7}', '\u{6D8}',
    '\u{6D9}', '\u{6DA}', '\u{6DB}', '\u{6DC}', '\u{6DF}', '\u{6E0}', '\u{6E1}', '\u{6E2}',
    '\u{6E4}', '\u{6E7}', '\u{6E8}', '\u{6EB}', '\u{6EC}', '\u{730}', '\u{732}', '\u{733}',
    '\u{735}', '\u{736}', '\u{73A}', '\u{73D}', '\u{73F}', '\u{740}', '\u{741}', '\u{743}',
    '\u{745}', '\u{747}', '\u{749}', '\u{74A}', '\u{7EB}', '\u{7EC}', '\u{7ED}', '\u{7EE}',
    '\u{7EF}', '\u{7F0}', '\u{7F1}', '\u{7F3}', '\u{816}', '\u{817}', '\u{818}', '\u{819}',
    '\u{81B}', '\u{81C}', '\u{81D}', '\u{81E}', '\u{81F}', '\u{820}', '\u{821}', '\u{822}',
    '\u{823}', '\u{825}', '\u{826}', '\u{827}', '\u{829}', '\u{82A}', '\u{82B}', '\u{82C}',
    '\u{82D}', '\u{951}', '\u{953}', '\u{954}', '\u{F82}', '\u{F83}', '\u{F86}', '\u{F87}',
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}',
    '\u{1A76}', '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}',
    '\u{1B6B}', '\u{1B6D}', '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}',
    '\u{1B73}', '\u{1CD0}', '\u{1CD1}', '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CDC}',
    '\u{1CDD}', '\u{1CDE}', '\u{1CDF}', '\u{1CE0}', '\u{1CE2}', '\u{1CE3}', '\u{1CE4}',
    '\u{1CE5}', '\u{1CE6}', '\u{1CE7}', '\u{1CE8}', '\u{1CED}', '\u{1CF4}', '\u{1CF8}',
    '\u{1CF9}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}', '\u{1DC5}', '\u{1DC6}',
    '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}', '\u{1DD2}',
    '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}',
    '\u{1DDA}', '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}',
    '\u{1DE1}', '\u{1DE2}', '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}',
    '\u{20D0}', '\u{20D1}', '\u{20D4}', '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}',
    '\u{20DC}', '\u{20E1}', '\u{20E7}', '\u{20E9}', '\u{20F0}', '\u{2CEF}', '\u{2CF0}',
    '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}', '\u{2DE3}', '\u{2DE4}', '\u{2DE5}',
    '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}', '\u{2DEB}', '\u{2DEC}',
    '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}', '\u{2DF3}',
    '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}',
    '\u{2DFB}', '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A674}',
    '\u{A675}', '\u{A676}', '\u{A677}', '\u{A678}', '\u{A679}', '\u{A67A}', '\u{A67B}',
    '\u{A67C}', '\u{A67D}', '\u{A69E}', '\u{A69F}', '\u{A6F0}', '\u{A6F1}', '\u{A8E0}',
    '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}', '\u{A8E5}', '\u{A8E6}', '\u{A8E7}',
    '\u{A8E8}', '\u{A8E9}', '\u{A8EA}', '\u{A8EB}', '\u{A8EC}', '\u{A8ED}', '\u{A8EE}',
    '\u{A8EF}', '\u{A8F0}', '\u{A8F1}',
];

fn diacritic(idx: u16) -> char {
    DIACRITICS
        .get(idx as usize)
        .copied()
        .unwrap_or(DIACRITICS[0])
}

/// Render a cached Kitty image using unicode placeholders into the ratatui
/// buffer.  `transmit_seq` is placed into the first cell of the first row;
/// on subsequent frames it will be an empty string (no re-upload).
fn render_kitty_placeholders(
    area: Rect,
    buf: &mut Buffer,
    encoded: &EncodedFrame,
) {
    let rows = area.height.min(encoded.rows);
    let cols = area.width.min(encoded.cols);

    let [id_extra, id_r, id_g, id_b] = encoded.image_id.to_be_bytes();
    let id_color = format!("\x1b[38;2;{id_r};{id_g};{id_b}m");

    for y in 0..rows {
        let mut symbol = if y == 0 {
            encoded.transmit_seq.clone()
        } else {
            String::new()
        };

        let save_len: usize = 3 + id_color.len() + (4 * 4);
        const RESTORE_LEN: usize = 19;
        symbol.reserve(save_len + (cols as usize * 4) + RESTORE_LEN);

        write!(
            symbol,
            "\x1b[s{id_color}\u{10EEEE}{}{}{}",
            diacritic(y),
            diacritic(0),
            diacritic(u16::from(id_extra))
        )
        .unwrap();

        symbol.extend(std::iter::repeat_n('\u{10EEEE}', (cols as usize).saturating_sub(1)));

        for x in 1..cols {
            if let Some(cell) = buf.cell_mut((area.left() + x, area.top() + y)) {
                cell.set_skip(true);
            }
        }

        // Ratatui writes the whole placeholder row from one buffer cell. The
        // terminal cursor must be restored and advanced to the end of the
        // widget, exactly as the upstream ratatui-image Kitty protocol does;
        // otherwise subsequent split-pane cells are emitted at the wrong
        // terminal coordinates and leave rectangular gaps.
        let right = area.width.saturating_sub(1);
        let down = area.height.saturating_sub(1);
        write!(symbol, "\x1b[u\x1b[{right}C\x1b[{down}B").unwrap();

        if let Some(cell) = buf.cell_mut((area.left(), area.top() + y)) {
            cell.set_symbol(&symbol);
        }
    }
}

/// A ratatui widget that blits the pre-encoded Kitty frame.
struct KittyBlit<'a> {
    encoded: &'a EncodedFrame,
}

impl Widget for KittyBlit<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_kitty_placeholders(area, buf, self.encoded);
    }
}

// ---------------------------------------------------------------------------
// Draw function
// ---------------------------------------------------------------------------

pub fn draw_pdf_viewer(frame: &mut Frame<'_>, app: &mut App) {
    draw_pdf_viewer_in(frame, app, frame.area());
}

/// Draw the cached, asynchronous PDF renderer inside a workspace pane.
pub fn draw_pdf_viewer_in(frame: &mut Frame<'_>, app: &mut App, area: Rect) {
    // Kitty placeholders mark cells as skipped. Clear the entire pane before
    // every blit so an invalidated image cannot leave skipped/stale cells in a
    // neighbouring split-pane region.
    frame.render_widget(Clear, area);
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

    // ── Single lock: read picker + font size, run physics, read page data ──
    let (font_w, font_h, page_data_opt, next_page_data_opt, current_page, current_scroll_px_val,
         last_crop_key, last_encoded_is_some, is_kitty) = {
        let cache_arc = get_cache();
        let mut g = match cache_arc.lock() {
            Ok(g) => g,
            Err(_) => return,
        };

        let (font_w, font_h, is_kitty) = match g.picker.as_ref() {
            Some(p) => {
                let (fw, fh) = p.font_size();
                // Detect Kitty protocol by checking the picker's protocol type
                use ratatui_image::picker::ProtocolType;
                let is_kitty = matches!(p.protocol_type(), ProtocolType::Kitty);
                (fw, fh, is_kitty)
            }
            None => {
                // No graphics protocol detected
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

        let pixel_w = (pdf_area.width as u32) * (font_w as u32);
        let pixel_h = (pdf_area.height as u32) * (font_h as u32);
        if pixel_w == 0 || pixel_h == 0 {
            return;
        }

        // Evict distant pages to bound memory
        let current_page_val = app.pdf_viewer_page;
        g.pages.retain(|(_, _, page, _, _), _| {
            (*page as isize - current_page_val as isize).abs() <= 3
        });

        g.last_viewport_h = pixel_h;

        // Run smooth scroll physics
        let now = std::time::Instant::now();
        let dt = g.last_update.map(|t| (now - t).as_secs_f64()).unwrap_or(0.0);
        g.last_update = Some(now);

        let total_pages = app.pdf_viewer_total_pages;
        let default_h = pixel_h as f64 * 1.5;

        if dt > 0.0 {
            let target_rel = to_relative_px(
                g.target_page, g.target_scroll_px, g.current_page, &g.pages, default_h,
            );
            let diff = target_rel - g.current_scroll_px;
            // Tighter spring: snaps within 1 px instead of dragging for several frames
            if diff.abs() > 1.0 {
                let lerp_factor = 1.0 - (-18.0 * dt).exp();
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

        app.pdf_viewer_page = g.current_page;
        app.pdf_viewer_scroll_y = (g.current_scroll_px / font_h as f64) as u32;

        let dpi = DPI;
        let generation = *g.document_generations.get(&pdf_path).unwrap_or(&0);
        let key: PageKey = (pdf_path.clone(), generation, g.current_page, dpi, pixel_w);
        let page_data = g.pages.get(&key).cloned();
        let next_key: PageKey = (pdf_path.clone(), generation, g.current_page + 1, dpi, pixel_w);
        let next_page_data = g.pages.get(&next_key).cloned();

        let last_crop_key = g.last_crop_key.clone();
        let last_encoded_is_some = g.last_encoded.is_some();

        (
            font_w, font_h, page_data, next_page_data,
            g.current_page, g.current_scroll_px,
            last_crop_key, last_encoded_is_some, is_kitty,
        )
    };
    // ── Lock released ────────────────────────────────────────────────────

    let pixel_w = (pdf_area.width as u32) * (font_w as u32);
    let pixel_h = (pdf_area.height as u32) * (font_h as u32);
    if pixel_w == 0 || pixel_h == 0 {
        return;
    }

    let dpi = DPI;

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

    // Build the crop key for this frame
    let crop_key = CropKey {
        path_fp: path_fingerprint(&pdf_path),
        page: current_page,
        dpi,
        pixel_w,
        crop_y: scroll_px,
        crop_h: pixel_h,
    };

    let need_encode = last_crop_key.as_ref() != Some(&crop_key);

    if need_encode || !last_encoded_is_some {
        // Crop/stitch the pixel data
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

        if is_kitty {
            // Encode directly with our own Kitty encoder — zero redundant work
            let cache_arc = get_cache();
            if let Ok(mut g) = cache_arc.lock() {
                let id = g.next_kitty_id;
                g.next_kitty_id = g.next_kitty_id.wrapping_add(1).max(1);
                let transmit_seq = kitty_transmit(&cropped, id);
                let [_id_extra, id_r, id_g, id_b] = id.to_be_bytes();
                let id_color = format!("\x1b[38;2;{id_r};{id_g};{id_b}m");
                g.last_encoded = Some(EncodedFrame {
                    transmit_seq,
                    image_id: id,
                    cols: pdf_area.width,
                    rows: pdf_area.height,
                    id_color,
                });
                g.last_crop_key = Some(crop_key);
            }
        } else {
            // Non-Kitty protocol fallback: use ratatui-image StatefulProtocol
            // (Sixel, iTerm2, halfblocks) — these are inherently full-frame anyway.
            use ratatui_image::{Resize, StatefulImage};
            let cache_arc = get_cache();
            if let Ok(mut g) = cache_arc.lock() {
                g.last_crop_key = Some(crop_key);
                let proto = g.picker.as_ref().unwrap().new_resize_protocol(cropped);
                drop(g);
                let mut local_proto = proto;
                frame.render_stateful_widget(
                    StatefulImage::new().resize(Resize::Fit(None)),
                    pdf_area,
                    &mut local_proto,
                );
                render_status_bar(
                    frame, status_area, &pdf_path, current_page, app,

                    app.pdf_viewer_scroll_y, max_scroll_cells,
                );
                return;
            }
        }
    }

    // ── Blit: write unicode placeholders (zero image data re-upload) ─────
    if is_kitty {
        let cache_arc = get_cache();
        if let Ok(g) = cache_arc.lock() {
            if let Some(ref encoded) = g.last_encoded {
                // Only include transmit_seq on frames where we just encoded;
                // on static frames the string is empty (we reuse existing ID).
                // Clone the encoded frame so we can drop the lock before rendering.
                let blit_frame = EncodedFrame {
                    // If we just encoded this frame, transmit_seq contains the
                    // full APC upload string.  If not (same crop key), we want
                    // an empty string so we only draw placeholders.
                    transmit_seq: if need_encode {
                        encoded.transmit_seq.clone()
                    } else {
                        String::new()
                    },
                    image_id: encoded.image_id,
                    cols: encoded.cols,
                    rows: encoded.rows,
                    id_color: encoded.id_color.clone(),
                };
                drop(g);
                frame.render_widget(KittyBlit { encoded: &blit_frame }, pdf_area);
            }
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
            format!(" │ {}/{} rows ", scroll, max_scroll),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            " │ Esc:exit  ↑↓/jk:scroll  PgUp/Dn:page ",
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
    let dpi = DPI;
    if let Some(arc) = CACHE.get() {
        if let Ok(g) = arc.lock() {
            let generation = *g.document_generations.get(pdf_path).unwrap_or(&0);
            return g.pages.keys().any(|(p, doc_generation, pg, d, _)| {
                p == pdf_path && *doc_generation == generation && *pg == page && *d == dpi
            });
        }
    }
    false
}

pub fn is_animating() -> bool {
    if let Some(arc) = CACHE.get() {
        if let Ok(g) = arc.lock() {
            let default_h = 1000.0;
            let target_rel = to_relative_px(
                g.target_page, g.target_scroll_px, g.current_page, &g.pages, default_h,
            );
            return (target_rel - g.current_scroll_px).abs() > 1.0;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Smooth scroll physics & stitching helpers
// ---------------------------------------------------------------------------

fn get_page_height_helper(page: usize, pages: &HashMap<PageKey, RenderedPage>, default_h: f64) -> f64 {
    for (key, val) in pages {
        if key.2 == page {
            return val.pixel_h as f64;
        }
    }
    default_h
}

fn normalize_coords(
    page: &mut usize,
    offset: &mut f64,
    total_pages: usize,
    pages: &HashMap<PageKey, RenderedPage>,
    default_h: f64,
) {
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

fn to_relative_px(
    page: usize,
    offset: f64,
    current_page: usize,
    pages: &HashMap<PageKey, RenderedPage>,
    default_h: f64,
) -> f64 {
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

/// Select a whole PDF page without changing renderer scale. Projects uses
/// this for deterministic pane-local navigation rather than continuous scroll.
pub fn jump_to_page(app: &mut App, page: usize) {
    let page = page.clamp(1, app.pdf_viewer_total_pages.max(1));
    let cache_arc = get_cache();
    if let Ok(mut g) = cache_arc.lock() {
        g.target_page = page;
        g.target_scroll_px = 0.0;
        g.current_page = page;
        g.current_scroll_px = 0.0;
        g.last_update = None;
        g.last_crop_key = None;
    }
    app.pdf_viewer_page = page;
    app.pdf_viewer_scroll_y = 0;
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
        let end = (next_visible as usize * bytes_per_row).min(next_img.as_raw().len());
        let slice = &next_img.as_raw()[0..end];
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
