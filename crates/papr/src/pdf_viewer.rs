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
//! unique (page, `scroll_px`, `pixel_w`, `pixel_h`) crop, store the resulting escape string,
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
//! The old loop: draw → compute timeout → read keys → `force_redraw`.  A keypress
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
//! different PDFs never share cached images or temporary raster files.
//!
//! ## Continuous scrolling (full-screen viewer on Kitty-protocol terminals)
//! Re-encoding a viewport-sized crop for every animation frame cannot keep up
//! with smooth scrolling on high-DPI displays (~25 MB per frame on Retina).
//! The full-screen viewer therefore uploads every rendered page to the
//! terminal **once** (as its own Kitty image, preferably via a temporary file
//! so no base64 crosses the PTY) and scrolls by re-issuing tiny placement
//! commands that select a pixel-exact source rectangle of each visible page.
//! Pages are laid out as one continuous strip separated by a small gap, just
//! like a browser PDF viewer.  Placements use a z-index below
//! non-default-background cells, so popups and the status bar naturally
//! cover the document.  The placement commands are written right after each
//! Ratatui draw (see [`flush_terminal_graphics`]) inside a synchronized
//! update, so text and images change atomically.
//!
//! Environment overrides:
//! * `PAPR_PDF_CONTINUOUS=0` disables continuous mode (legacy crop renderer).
//! * `PAPR_KITTY_TRANSFER=direct|file` forces the image transfer medium.
//!   The default is `file`, except in SSH sessions where the terminal cannot
//!   read local files.

use crate::state::{App, Page};

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as FmtWrite;
use std::hash::{Hash, Hasher};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread;

use anyhow::{Context, Result};
use flate2::{Compression, write::ZlibEncoder};
use image::{DynamicImage, Rgba, RgbaImage};
use num_traits::ToPrimitive;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget},
};
use ratatui_image::{
    Resize, StatefulImage,
    picker::{Picker, ProtocolType},
};

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
    /// The embedded project preview displays a fitted full page instead of a
    /// scroll crop, so it must not reuse an encoded scroll-mode frame.
    page_fit: bool,
    transparent_padding: bool,
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
    /// Image data that can be deleted after this frame's placeholders have
    /// reached the terminal.  Keeping the old image alive until then avoids
    /// the blank interval caused by re-transmitting an in-use Kitty image ID.
    retire_image_id: Option<u32>,
}

#[derive(Clone)]
struct RenderJob {
    key: PageKey,
    pdf_path: PathBuf,
    generation: u64,
    page: usize,
    dpi: u32,
    pixel_w: u32,
    priority: bool,
    cancelled: Arc<AtomicBool>,
}

/// Everything that lives inside the global singleton lock.
struct PageCache {
    picker: Option<Picker>,
    pages: HashMap<PageKey, RenderedPage>,
    in_flight: std::collections::HashSet<PageKey>,
    document_generations: HashMap<PathBuf, u64>,
    page_counts: HashMap<(PathBuf, u64), usize>,
    page_count_in_flight: HashSet<(PathBuf, u64)>,
    document_widths: HashMap<PathBuf, u32>,
    /// Raster files are owned by their source PDF so closing one document can
    /// remove only its files without disturbing another open document.
    temp_files: Vec<(PathBuf, PathBuf)>,
    /// Pending raster work. Keeping jobs here instead of spawning one thread
    /// per request bounds both peak memory and the number of Poppler children.
    render_queue: VecDeque<RenderJob>,
    active_renders: usize,
    render_cancellations: HashMap<PageKey, Arc<AtomicBool>>,

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
    /// Next Kitty image ID.  A new ID is used for each crop so the old image
    /// stays visible until the new virtual placement is on screen.
    next_kitty_id: u32,
    /// Kitty image IDs that must be retired during the next Ratatui draw.
    /// Keeping this in the draw pipeline avoids writing graphics escapes
    /// directly to stdout while Ratatui owns terminal output.
    pending_kitty_deletes: Vec<u32>,
    /// Image data currently owned by the terminal. This is deliberately
    /// independent of `last_encoded`: invalidating a PDF must keep the last
    /// good preview visible until its replacement has been uploaded.
    resident_kitty_image: Option<(u64, u32)>,

    // ── Continuous scrolling ─────────────────────────────────────────────
    /// Vertical gap (pixels) between consecutive pages. Zero in legacy mode
    /// so the crop/stitch renderer keeps its exact geometry.
    page_gap: f64,
    /// Height estimate for pages that have not been rasterised yet.
    default_page_h: f64,
    /// Whole-page Kitty images, one per rendered page.
    uploads: HashMap<PageKey, KittyUpload>,
    /// Kitty image IDs that currently have an on-screen placement.
    placed: HashSet<u32>,
    /// Graphics commands to write after the next Ratatui draw.
    graphics_output: String,
    /// Set by the continuous renderer during a draw; consumed by
    /// [`flush_terminal_graphics`] to remove placements once the viewer
    /// is no longer drawn.
    continuous_drawn: bool,
    /// A visible page is still rendering or uploading.
    continuous_pending: bool,
    /// Last page intersecting the viewport during the previous draw.
    visible_last_page: usize,
}

/// Lifecycle of a whole-page Kitty image used by continuous scrolling.
enum KittyUpload {
    /// Pixel data is being converted / written on a helper thread.
    Preparing { id: u32 },
    /// The transmit command is ready to be written to the terminal.
    Ready {
        id: u32,
        command: String,
        file: Option<PathBuf>,
    },
    /// The terminal owns the image; only placements are sent from now on.
    Uploaded { id: u32, file: Option<PathBuf> },
}

impl KittyUpload {
    fn id(&self) -> u32 {
        match self {
            Self::Preparing { id } | Self::Ready { id, .. } | Self::Uploaded { id, .. } => *id,
        }
    }

    fn file(&self) -> Option<&PathBuf> {
        match self {
            Self::Preparing { .. } => None,
            Self::Ready { file, .. } | Self::Uploaded { file, .. } => file.as_ref(),
        }
    }
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
                page_counts: HashMap::new(),
                page_count_in_flight: HashSet::new(),
                document_widths: HashMap::new(),
                temp_files: Vec::new(),
                render_queue: VecDeque::new(),
                active_renders: 0,
                render_cancellations: HashMap::new(),
                last_viewport_h: 600,
                target_page: 1,
                target_scroll_px: 0.0,
                current_page: 1,
                current_scroll_px: 0.0,
                last_update: None,
                last_crop_key: None,
                last_encoded: None,
                next_kitty_id: 1,
                pending_kitty_deletes: Vec::new(),
                resident_kitty_image: None,
                page_gap: 0.0,
                default_page_h: 1000.0,
                uploads: HashMap::new(),
                placed: HashSet::new(),
                graphics_output: String::new(),
                continuous_drawn: false,
                continuous_pending: false,
                visible_last_page: 0,
            }))
        })
        .clone()
}

fn cancel_render_jobs(g: &mut PageCache, mut should_cancel: impl FnMut(&PageKey) -> bool) {
    for job in &g.render_queue {
        if should_cancel(&job.key) {
            job.cancelled.store(true, Ordering::Release);
        }
    }
    for (key, cancelled) in &g.render_cancellations {
        if should_cancel(key) {
            cancelled.store(true, Ordering::Release);
        }
    }

    let mut removed = Vec::new();
    g.render_queue.retain(|job| {
        let keep = !job.cancelled.load(Ordering::Acquire);
        if !keep {
            removed.push(job.key.clone());
        }
        keep
    });
    for key in removed {
        g.in_flight.remove(&key);
        g.render_cancellations.remove(&key);
    }
}

/// Free the terminal-side image data (and any transfer file) of every
/// whole-page upload matching `should_drop`.
fn drop_uploads(g: &mut PageCache, mut should_drop: impl FnMut(&PageKey) -> bool) {
    let keys = g
        .uploads
        .keys()
        .filter(|key| should_drop(key))
        .cloned()
        .collect::<Vec<_>>();
    for key in keys {
        let Some(upload) = g.uploads.remove(&key) else {
            continue;
        };
        let id = upload.id();
        if !matches!(upload, KittyUpload::Preparing { .. }) {
            g.graphics_output.push_str(&kitty_delete_image(id));
        }
        g.placed.remove(&id);
        if let Some(file) = upload.file() {
            let _ = std::fs::remove_file(file);
            g.temp_files.retain(|(_, owned)| owned != file);
        }
    }
}

// ---------------------------------------------------------------------------
// Public lifecycle API
// ---------------------------------------------------------------------------

/// Flush all state that belongs to a different document than `new_path`.
/// Always call this before entering `AppMode::PdfView` for a new file.
pub fn reset_for_new_document(new_path: &Path) {
    let cache = get_cache();
    if let Ok(mut g) = cache.lock() {
        cancel_render_jobs(&mut g, |(path, _, _, _, _)| path.as_path() != new_path);
        g.pages.retain(|(p, _, _, _, _), _| p.as_path() == new_path);
        g.in_flight
            .retain(|(p, _, _, _, _)| p.as_path() == new_path);
        g.render_queue.retain(|job| job.pdf_path == new_path);
        g.document_widths.retain(|path, _| path == new_path);
        let active_documents = g
            .render_cancellations
            .keys()
            .map(|(path, _, _, _, _)| path.clone())
            .collect::<std::collections::HashSet<_>>();
        g.document_generations
            .retain(|path, _| path == new_path || active_documents.contains(path));
        g.document_generations
            .entry(new_path.to_path_buf())
            .or_default();
        g.page_counts.retain(|(path, _), _| path == new_path);
        g.page_count_in_flight.retain(|(path, _)| path == new_path);
        drop_uploads(&mut g, |(path, _, _, _, _)| path.as_path() != new_path);
        g.visible_last_page = 0;
        let new_fp = path_fingerprint(new_path);
        if let Some((resident_fp, image_id)) = g.resident_kitty_image
            && resident_fp != new_fp
        {
            g.pending_kitty_deletes.push(image_id);
            g.resident_kitty_image = None;
        }
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
        let generation = g
            .document_generations
            .entry(path.to_path_buf())
            .or_default();
        *generation = generation.wrapping_add(1);
        cancel_render_jobs(&mut g, |(cached, _, _, _, _)| cached.as_path() == path);
        g.pages
            .retain(|(cached, _, _, _, _), _| cached.as_path() != path);
        g.in_flight
            .retain(|(cached, _, _, _, _)| cached.as_path() != path);
        g.render_queue.retain(|job| job.pdf_path != path);
        g.document_widths.remove(path);
        g.page_counts.retain(|(cached, _), _| cached != path);
        g.page_count_in_flight.retain(|(cached, _)| cached != path);
        drop_uploads(&mut g, |(cached, _, _, _, _)| cached.as_path() == path);
        g.last_crop_key = None;
        g.last_encoded = None;
    }
}

/// Release raster and encoded state for a document that is no longer open.
/// Advancing the generation first prevents an already-running raster job from
/// inserting its result after the cache entries have been removed.
pub fn release_document(path: &Path) {
    let cache = get_cache();
    if let Ok(mut g) = cache.lock() {
        let generation = g
            .document_generations
            .entry(path.to_path_buf())
            .or_default();
        *generation = generation.wrapping_add(1);
        cancel_render_jobs(&mut g, |(cached, _, _, _, _)| cached.as_path() == path);
        g.pages
            .retain(|(cached, _, _, _, _), _| cached.as_path() != path);
        g.in_flight
            .retain(|(cached, _, _, _, _)| cached.as_path() != path);
        g.render_queue.retain(|job| job.pdf_path != path);
        g.document_widths.remove(path);
        g.page_counts.retain(|(cached, _), _| cached != path);
        g.page_count_in_flight.retain(|(cached, _)| cached != path);
        drop_uploads(&mut g, |(cached, _, _, _, _)| cached.as_path() == path);
        let mut retained_temp_files = Vec::with_capacity(g.temp_files.len());
        for (document, file) in g.temp_files.drain(..) {
            if document == path {
                let _ = std::fs::remove_file(file);
            } else {
                retained_temp_files.push((document, file));
            }
        }
        g.temp_files = retained_temp_files;
        let path_fp = path_fingerprint(path);
        if let Some((resident_fp, image_id)) = g.resident_kitty_image
            && resident_fp == path_fp
        {
            g.pending_kitty_deletes.push(image_id);
            g.resident_kitty_image = None;
        }
        if g.last_crop_key
            .as_ref()
            .is_some_and(|key| key.path_fp == path_fp)
        {
            g.last_crop_key = None;
            g.last_encoded = None;
        }
        if !g
            .render_cancellations
            .keys()
            .any(|(cached, _, _, _, _)| cached.as_path() == path)
        {
            g.document_generations.remove(path);
        }
    }
}

/// Return the known page count, starting a document-generation-scoped
/// `pdfinfo` query in the background when needed. A result from a closed or
/// replaced PDF is discarded instead of repopulating the cache.
pub fn page_count(path: &Path) -> Option<usize> {
    let cache = get_cache();
    let path = path.to_path_buf();
    let key = {
        let mut g = cache.lock().ok()?;
        let generation = *g.document_generations.entry(path.clone()).or_default();
        let key = (path.clone(), generation);
        if let Some(count) = g.page_counts.get(&key) {
            return Some(*count);
        }
        if !g.page_count_in_flight.insert(key.clone()) {
            return None;
        }
        key
    };

    thread::spawn(move || {
        let count = papr_core::get_pdf_page_count(&path).max(1);
        if let Ok(mut g) = cache.lock() {
            g.page_count_in_flight.remove(&key);
            if g.document_generations.get(&path).copied() == Some(key.1) {
                g.page_counts.insert(key, count);
            }
        }
    });
    None
}

/// Emit pending Kitty image deletions as part of Ratatui's next draw.  The
/// escape sequence is prepended to an existing cell, preserving its visible
/// text and keeping graphics output synchronized with terminal updates.
pub fn render_pending_kitty_cleanup(frame: &mut Frame<'_>) {
    let deletes = {
        let cache = get_cache();
        let Ok(mut g) = cache.lock() else {
            return;
        };
        std::mem::take(&mut g.pending_kitty_deletes)
    };
    if deletes.is_empty() {
        return;
    }
    let sequence = deletes
        .into_iter()
        .map(kitty_delete_image)
        .collect::<String>();
    frame.render_widget(
        KittyCleanup {
            sequence: &sequence,
        },
        Rect::new(frame.area().x, frame.area().y, 1, 1),
    );
}

/// Write the graphics commands produced by the continuous PDF renderer.
///
/// Call this right after every Ratatui draw (ideally inside the same
/// synchronized update).  Placement commands position the cursor
/// absolutely, so they are wrapped in DECSC/DECRC to leave Ratatui's cursor
/// bookkeeping untouched.  When the continuous viewer was not drawn in the
/// frame that just finished, its placements are removed so the document
/// never lingers beneath other screens.  Image data stays resident in the
/// terminal for an instant return to the viewer.
pub fn flush_terminal_graphics(out: &mut impl IoWrite) -> std::io::Result<()> {
    let Some(arc) = CACHE.get() else {
        return Ok(());
    };
    let output = {
        let Ok(mut g) = arc.lock() else {
            return Ok(());
        };
        if !g.continuous_drawn {
            g.continuous_pending = false;
            let placed = std::mem::take(&mut g.placed);
            for id in placed {
                let _ = write!(g.graphics_output, "{}", kitty_delete_placements(id));
            }
        }
        g.continuous_drawn = false;
        std::mem::take(&mut g.graphics_output)
    };
    if output.is_empty() {
        return Ok(());
    }
    out.write_all(b"\x1b7")?;
    out.write_all(output.as_bytes())?;
    out.write_all(b"\x1b8")?;
    out.flush()
}

/// True while a page visible in the continuous viewer is still rendering or
/// uploading, so the event loop keeps redrawing until it appears.
pub fn graphics_pending() -> bool {
    CACHE
        .get()
        .and_then(|arc| arc.lock().ok().map(|g| g.continuous_pending))
        .unwrap_or(false)
}

/// Delete all temporary raster files. Call once at application exit.
pub fn cleanup_temp_files() {
    if let Some(arc) = CACHE.get()
        && let Ok(mut g) = arc.lock()
    {
        for (_, file) in g.temp_files.drain(..) {
            let _ = std::fs::remove_file(file);
        }
    }
}

// ---------------------------------------------------------------------------
// Background rendering
// ---------------------------------------------------------------------------

const DPI: u32 = 150;
/// Poppler processes run in parallel. One slot is always kept free for a
/// newly visible (priority) page; the rest serve speculative prefetch, which
/// continuous scrolling relies on to have pages ready before they appear.
const MAX_CONCURRENT_RENDERS: usize = 3;

/// Submit a background render job if the result isn't already cached or
/// in-flight.  Uses a single lock acquisition to check-and-mark atomically.
fn request_page(pdf_path: &Path, page: usize, dpi: u32, pixel_w: u32, priority: bool) {
    let cache = get_cache();
    let generation;
    let key;

    // Single lock: check presence and mark in-flight atomically.
    {
        let Ok(mut g) = cache.lock() else { return };
        generation = *g.document_generations.get(pdf_path).unwrap_or(&0);
        key = (pdf_path.to_path_buf(), generation, page, dpi, pixel_w);
        if g.pages.contains_key(&key) {
            return;
        }
        if g.in_flight.contains(&key) {
            let promoted = if priority
                && let Some(index) = g.render_queue.iter().position(|job| job.key == key)
                && let Some(mut job) = g.render_queue.remove(index)
            {
                job.priority = true;
                g.render_queue.push_front(job);
                true
            } else {
                false
            };
            drop(g);
            if promoted {
                pump_render_queue();
            }
            return;
        }
        g.in_flight.insert(key.clone());
        let cancelled = Arc::new(AtomicBool::new(false));
        g.render_cancellations
            .insert(key.clone(), cancelled.clone());
        let job = RenderJob {
            key,
            pdf_path: pdf_path.to_path_buf(),
            generation,
            page,
            dpi,
            pixel_w,
            priority,
            cancelled,
        };
        if priority {
            g.render_queue.push_front(job);
        } else {
            g.render_queue.push_back(job);
        }
    }

    pump_render_queue();
}

fn pump_render_queue() {
    loop {
        let (cache, job) = {
            let cache = get_cache();
            let Ok(mut g) = cache.lock() else { return };
            if g.active_renders >= MAX_CONCURRENT_RENDERS {
                return;
            }
            if g.active_renders + 1 >= MAX_CONCURRENT_RENDERS
                && g.render_queue.front().is_some_and(|job| !job.priority)
            {
                // Keep one renderer slot free for a newly visible page. This
                // also lets the first page use Poppler without excessive
                // prefetch CPU contention, while still allowing a priority
                // render to join already-running speculative neighbours.
                return;
            }
            let Some(job) = g.render_queue.pop_front() else {
                return;
            };
            g.active_renders += 1;
            (cache.clone(), job)
        };

        thread::spawn(move || {
            let RenderJob {
                key,
                pdf_path,
                generation,
                page,
                dpi,
                pixel_w,
                priority: _,
                cancelled,
            } = job;
            let fp = path_fingerprint(&pdf_path);
            let temp_dir = std::env::temp_dir();
            let prefix = temp_dir.join(format!(
                "papr_pdf_{}_fp{:x}_g{}_p{}_d{}_w{}",
                std::process::id(),
                fp,
                generation,
                page,
                dpi,
                pixel_w,
            ));
            let raster = prefix.with_extension("ppm");

            let result =
                render_page_blocking(&pdf_path, page, pixel_w, &prefix, &raster, &cancelled);

            if let Ok(mut g) = cache.lock() {
                g.active_renders = g.active_renders.saturating_sub(1);
                g.in_flight.remove(&key);
                g.render_cancellations.remove(&key);
                let current_generation = *g.document_generations.get(&pdf_path).unwrap_or(&0);
                let current_width = g.document_widths.get(&pdf_path).copied();
                match result {
                    _ if current_generation != generation || current_width != Some(pixel_w) => {
                        // The document was closed/replaced while Poppler ran.
                        // Do not let its disk raster outlive the document either.
                        let _ = std::fs::remove_file(&raster);
                    }
                    Ok(page_data) => {
                        g.pages.insert(key, page_data);
                        if raster.exists() && !g.temp_files.iter().any(|(_, file)| file == &raster)
                        {
                            g.temp_files.push((pdf_path.clone(), raster));
                        }
                    }
                    Err(_) => {
                        // A failed Poppler/decode attempt may still have created
                        // a partial raster. It has no cache owner, so remove it.
                        let _ = std::fs::remove_file(&raster);
                    }
                }
                if !g.document_widths.contains_key(&pdf_path)
                    && !g.pages.keys().any(|(path, _, _, _, _)| path == &pdf_path)
                    && !g
                        .in_flight
                        .iter()
                        .any(|(path, _, _, _, _)| path == &pdf_path)
                    && !g.render_queue.iter().any(|job| job.pdf_path == pdf_path)
                    && !g
                        .render_cancellations
                        .keys()
                        .any(|(path, _, _, _, _)| path == &pdf_path)
                {
                    g.document_generations.remove(&pdf_path);
                }
            }
            pump_render_queue();
        });
    }
}

fn render_page_blocking(
    pdf_path: &Path,
    page: usize,
    pixel_w: u32,
    prefix: &Path,
    raster_path: &Path,
    cancelled: &AtomicBool,
) -> Result<RenderedPage> {
    if cancelled.load(Ordering::Acquire) {
        anyhow::bail!("PDF render cancelled");
    }
    if !raster_path.exists() {
        let mut child = std::process::Command::new("pdftoppm")
            .arg("-singlefile")
            // Rasterize at the size Papr will display. Rendering a larger
            // 150-DPI page and immediately downscaling it needlessly holds
            // both full-size buffers at once.
            .arg("-scale-to-x")
            .arg(pixel_w.to_string())
            .arg("-scale-to-y")
            .arg("-1")
            .arg("-f")
            .arg(page.to_string())
            .arg(pdf_path)
            .arg(prefix)
            // Poppler writes recoverable parser diagnostics to stderr.  The
            // internal viewer runs while the TUI owns that terminal, so an
            // inherited stderr would briefly overwrite the first viewer frame.
            // Rendering success is determined by the exit status and output
            // image below; keep parser warnings out of the user interface.
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("failed to spawn pdftoppm")?;
        let status = loop {
            if cancelled.load(Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(raster_path);
                anyhow::bail!("PDF render cancelled");
            }
            if let Some(status) = child.try_wait().context("failed to wait for pdftoppm")? {
                break status;
            }
            thread::sleep(std::time::Duration::from_millis(10));
        };
        if !status.success() {
            anyhow::bail!("pdftoppm exited with {status:?}");
        }
    }

    if cancelled.load(Ordering::Acquire) {
        let _ = std::fs::remove_file(raster_path);
        anyhow::bail!("PDF render cancelled");
    }

    let img = image::open(raster_path).context("failed to decode Poppler raster")?;
    // The decoded page is the cache owner. Keeping the much larger PPM file
    // until document close would provide no reuse benefit and wastes disk.
    let _ = std::fs::remove_file(raster_path);
    let img_w = img.width();
    let img_h = img.height();
    if img_w == 0 || pixel_w == 0 {
        anyhow::bail!("zero-width image");
    }

    let scaled_h = u32::try_from(
        u64::from(img_h)
            .saturating_mul(u64::from(pixel_w))
            .div_ceil(u64::from(img_w)),
    )
    .unwrap_or(u32::MAX)
    .max(1);
    let scaled = if img_w == pixel_w {
        img
    } else {
        img.resize_exact(pixel_w, scaled_h, image::imageops::FilterType::Triangle)
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
    let h = src.height();
    if w == 0 || h == 0 || crop_h == 0 {
        return DynamicImage::ImageRgba8(RgbaImage::new(w.max(1), crop_h.max(1)));
    }
    let bytes_per_row = w as usize * 4;
    let start = (crop_y as usize * bytes_per_row).min(src.as_raw().len());
    let end = (start + crop_h as usize * bytes_per_row).min(src.as_raw().len());
    let slice = &src.as_raw()[start..end];

    let expected_len = (w * crop_h * 4) as usize;
    let mut data = vec![0u8; expected_len];
    let copy_len = slice.len().min(expected_len);
    data[..copy_len].copy_from_slice(&slice[..copy_len]);

    let buf = RgbaImage::from_raw(w, crop_h, data).unwrap_or_else(|| RgbaImage::new(w, crop_h));
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

/// Streams compressed bytes into Kitty-sized base64 chunks. Only one encoded
/// chunk is staged so compression never creates a second full-image buffer.
struct KittyPayloadWriter {
    output: String,
    pending: Vec<u8>,
    staged: Option<String>,
    /// Control keys of the first chunk, e.g. `i=1,a=T,f=32,...,o=z`.
    control: String,
    first: bool,
}

impl KittyPayloadWriter {
    const BINARY_CHUNK_SIZE: usize = (4096 / 4) * 3;

    fn new(control: String) -> Self {
        Self {
            output: String::new(),
            pending: Vec::with_capacity(Self::BINARY_CHUNK_SIZE * 2),
            staged: None,
            control,
            first: true,
        }
    }

    fn stage(&mut self, bytes: &[u8]) {
        let mut encoded = String::with_capacity(4096);
        base64_encode_into(bytes, &mut encoded);
        if let Some(previous) = self.staged.replace(encoded) {
            self.emit(&previous, true);
        }
    }

    fn emit(&mut self, encoded: &str, more: bool) {
        let more = u8::from(more);
        if self.first {
            let _ = write!(self.output, "\x1b_Gq=2,{},m={more};", self.control);
            self.first = false;
        } else {
            let _ = write!(self.output, "\x1b_Gq=2,m={more};");
        }
        self.output.push_str(encoded);
        self.output.push_str("\x1b\\");
    }

    fn finish(mut self) -> String {
        if !self.pending.is_empty() {
            let final_bytes = std::mem::take(&mut self.pending);
            self.stage(&final_bytes);
        }
        if let Some(last) = self.staged.take() {
            self.emit(&last, false);
        }
        self.output
    }
}

impl IoWrite for KittyPayloadWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let available = Self::BINARY_CHUNK_SIZE - self.pending.len();
            let take = available.min(remaining.len());
            self.pending.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if self.pending.len() == Self::BINARY_CHUNK_SIZE {
                let mut chunk = std::mem::take(&mut self.pending);
                self.stage(&chunk);
                chunk.clear();
                self.pending = chunk;
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Build a Kitty APC sequence that uploads `img` as a virtual placement with
/// the given `id`. Zlib is lossless and is streamed directly into protocol
/// chunks, reducing terminal I/O without retaining another image-sized buffer.
fn kitty_transmit(img: &DynamicImage, id: u32) -> String {
    let (width, height) = (img.width(), img.height());
    // Every Papr crop is already RGBA. Avoid cloning the entire viewport just
    // to obtain its byte slice; retain the conversion fallback for callers
    // that provide another DynamicImage variant.
    let converted;
    let bytes = if let Some(rgba) = img.as_rgba8() {
        rgba.as_raw()
    } else {
        converted = img.to_rgba8();
        converted.as_raw()
    };

    let writer = KittyPayloadWriter::new(format!(
        "i={id},a=T,U=1,f=32,t=d,s={width},v={height},o=z"
    ));
    let mut encoder = ZlibEncoder::new(writer, Compression::fast());
    if encoder.write_all(bytes).is_err() {
        return kitty_transmit_raw(bytes, id, width, height);
    }
    encoder.finish().map_or_else(
        |_| kitty_transmit_raw(bytes, id, width, height),
        KittyPayloadWriter::finish,
    )
}

/// Infallible protocol fallback used only if the zlib encoder fails.
fn kitty_transmit_raw(bytes: &[u8], id: u32, width: u32, height: u32) -> String {
    kitty_chunks_raw(
        bytes,
        &format!("i={id},a=T,U=1,f=32,t=d,s={width},v={height}"),
    )
}

/// Chunk uncompressed `bytes` into Kitty APC sequences with `control` keys on
/// the first chunk.
fn kitty_chunks_raw(bytes: &[u8], control: &str) -> String {
    const CHUNK_SIZE: usize = (4096 / 4) * 3;
    let chunk_count = bytes.len().div_ceil(CHUNK_SIZE);
    let mut output = String::with_capacity(bytes.len() * 4 / 3 + chunk_count * 16 + 64);
    for (index, chunk) in bytes.chunks(CHUNK_SIZE).enumerate() {
        let more = u8::from(index + 1 < chunk_count);
        if index == 0 {
            let _ = write!(output, "\x1b_Gq=2,{control},m={more};");
        } else {
            let _ = write!(output, "\x1b_Gq=2,m={more};");
        }
        base64_encode_into(chunk, &mut output);
        output.push_str("\x1b\\");
    }
    output
}

/// Transmit (without placing) a whole RGB page over the PTY. Used when the
/// terminal cannot read local files (SSH) or the transfer file failed.
fn kitty_upload_direct(rgb: &[u8], id: u32, width: u32, height: u32) -> String {
    let writer = KittyPayloadWriter::new(format!(
        "i={id},a=t,f=24,t=d,s={width},v={height},o=z"
    ));
    let mut encoder = ZlibEncoder::new(writer, Compression::fast());
    let fallback = || kitty_chunks_raw(rgb, &format!("i={id},a=t,f=24,t=d,s={width},v={height}"));
    if encoder.write_all(rgb).is_err() {
        return fallback();
    }
    encoder
        .finish()
        .map_or_else(|_| fallback(), KittyPayloadWriter::finish)
}

/// Transmit (without placing) a whole RGB page by asking the terminal to read
/// it from a temporary file. Only the path crosses the PTY; Kitty deletes
/// the file after reading because its name contains `tty-graphics-protocol`.
fn kitty_upload_file(path: &Path, id: u32, width: u32, height: u32) -> String {
    let mut encoded_path = String::new();
    base64_encode_into(path.to_string_lossy().as_bytes(), &mut encoded_path);
    format!("\x1b_Gq=2,a=t,i={id},f=24,t=t,s={width},v={height};{encoded_path}\x1b\\")
}

/// Remove an image's placements but keep its data resident in the terminal.
fn kitty_delete_placements(id: u32) -> String {
    format!("\x1b_Gq=2,a=d,d=i,i={id}\x1b\\")
}

/// Z-index below cells with a non-default background colour: Ratatui popups
/// and the status bar cover the page, while default-background cells (the
/// PDF pane itself) let it show through.
const CONTINUOUS_Z_INDEX: i64 = -1_073_741_825;

/// A pixel-exact window of one page image, positioned on the screen.
#[derive(Debug, PartialEq, Eq)]
struct PagePlacement {
    /// Screen cell row (0-based, relative to the viewport) of the top edge.
    row: u32,
    /// Pixel offset of the top edge inside that cell.
    y_offset: u32,
    /// First source row of the page image.
    src_y: u32,
    /// Number of source rows shown.
    src_h: u32,
}

/// Clip a page whose top edge is at `top` (viewport pixels, may be negative)
/// to the viewport and express the result as a Kitty placement.
fn page_placement(top: i64, height: i64, viewport_h: i64, font_h: u32) -> Option<PagePlacement> {
    let visible_top = top.max(0);
    let visible_bottom = top.saturating_add(height).min(viewport_h);
    if visible_bottom <= visible_top || font_h == 0 {
        return None;
    }
    let font_h = i64::from(font_h);
    Some(PagePlacement {
        row: u32::try_from(visible_top / font_h).ok()?,
        y_offset: u32::try_from(visible_top % font_h).ok()?,
        src_y: u32::try_from(visible_top - top).ok()?,
        src_h: u32::try_from(visible_bottom - visible_top).ok()?,
    })
}

fn kitty_place_command(area: Rect, id: u32, pixel_w: u32, placement: &PagePlacement) -> String {
    let PagePlacement {
        row,
        y_offset,
        src_y,
        src_h,
    } = placement;
    format!(
        "\x1b[{};{}H\x1b_Gq=2,a=p,i={id},p=1,x=0,y={src_y},w={pixel_w},h={src_h},Y={y_offset},C=1,z={CONTINUOUS_Z_INDEX}\x1b\\",
        u32::from(area.y) + row + 1,
        u32::from(area.x) + 1,
    )
}

/// A page slot of the continuous strip.
#[derive(Debug, PartialEq, Eq)]
struct PageSlot {
    page: usize,
    /// Top edge in viewport pixels (negative when scrolled past).
    top: i64,
    height: i64,
    rendered: bool,
}

/// Lay out the pages intersecting a viewport of `viewport_h` pixels when
/// `scroll_px` pixels of `current_page` are scrolled out at the top.
fn layout_continuous_pages(
    current_page: usize,
    scroll_px: f64,
    last_page: usize,
    viewport_h: i64,
    gap: i64,
    default_h: i64,
    height_of: impl Fn(usize) -> Option<i64>,
) -> Vec<PageSlot> {
    let mut slots = Vec::new();
    let mut top = -scroll_px.round().to_i64().unwrap_or(0);
    let mut page = current_page.max(1);
    while page <= last_page && top < viewport_h {
        let measured = height_of(page);
        let height = measured.unwrap_or(default_h).max(1);
        slots.push(PageSlot {
            page,
            top,
            height,
            rendered: measured.is_some(),
        });
        top = top.saturating_add(height).saturating_add(gap);
        page += 1;
    }
    slots
}

/// Whether the full-screen viewer should use continuous scrolling.
fn continuous_scrolling_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("PAPR_PDF_CONTINUOUS")
            .map_or(true, |value| !matches!(value.trim(), "0" | "false" | "off" | "no"))
    })
}

/// Whether the terminal can read page images from local temporary files.
fn kitty_file_transfer_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| match std::env::var("PAPR_KITTY_TRANSFER").as_deref() {
        Ok("direct") => false,
        Ok("file") => true,
        _ => ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
            .iter()
            .all(|name| std::env::var_os(name).is_none()),
    })
}

fn rgba_to_rgb(image: &RgbaImage) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(image.as_raw().len() / 4 * 3);
    for pixel in image.as_raw().as_chunks::<4>().0 {
        rgb.extend_from_slice(&pixel[..3]);
    }
    rgb
}

/// Convert a cached page to RGB and build its upload command off the draw
/// thread. The result is discarded if the upload was dropped meanwhile.
fn spawn_upload_preparation(
    cache: Arc<Mutex<PageCache>>,
    key: PageKey,
    id: u32,
    image: Arc<RgbaImage>,
    use_file: bool,
) {
    thread::spawn(move || {
        let (width, height) = image.dimensions();
        let rgb = rgba_to_rgb(&image);
        drop(image);
        let mut file = None;
        let command = if use_file {
            let path = std::env::temp_dir().join(format!(
                "papr-tty-graphics-protocol-{}-{id}.rgb",
                std::process::id()
            ));
            if std::fs::write(&path, &rgb).is_ok() {
                let command = kitty_upload_file(&path, id, width, height);
                file = Some(path);
                command
            } else {
                let _ = std::fs::remove_file(&path);
                kitty_upload_direct(&rgb, id, width, height)
            }
        } else {
            kitty_upload_direct(&rgb, id, width, height)
        };
        drop(rgb);

        let Ok(mut g) = cache.lock() else {
            if let Some(path) = file {
                let _ = std::fs::remove_file(path);
            }
            return;
        };
        let still_wanted = matches!(
            g.uploads.get(&key),
            Some(KittyUpload::Preparing { id: pending }) if *pending == id
        );
        if still_wanted {
            if let Some(path) = &file {
                g.temp_files.push((key.0.clone(), path.clone()));
            }
            g.uploads
                .insert(key, KittyUpload::Ready { id, command, file });
        } else if let Some(path) = file {
            let _ = std::fs::remove_file(path);
        }
    });
}

/// Remove image data only after its replacement has been written.  `q=2`
/// suppresses the acknowledgement so it cannot enter the application's input
/// stream.
fn kitty_delete_image(id: u32) -> String {
    format!("\x1b_Gq=2,a=d,d=I,i={id}\x1b\\")
}

/// Minimal base64 encoder (standard alphabet, no padding needed by Kitty).
fn base64_encode_into(input: &[u8], out: &mut String) {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    out.reserve((input.len() * 4).div_ceil(3));
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
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
}

/// Row/column diacritics for Kitty unicode placeholder rendering.
/// Source: <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>
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
    '\u{135D}', '\u{135E}', '\u{135F}', '\u{17DD}', '\u{193A}', '\u{1A17}', '\u{1A75}', '\u{1A76}',
    '\u{1A77}', '\u{1A78}', '\u{1A79}', '\u{1A7A}', '\u{1A7B}', '\u{1A7C}', '\u{1B6B}', '\u{1B6D}',
    '\u{1B6E}', '\u{1B6F}', '\u{1B70}', '\u{1B71}', '\u{1B72}', '\u{1B73}', '\u{1CD0}', '\u{1CD1}',
    '\u{1CD2}', '\u{1CDA}', '\u{1CDB}', '\u{1CDC}', '\u{1CDD}', '\u{1CDE}', '\u{1CDF}', '\u{1CE0}',
    '\u{1CE2}', '\u{1CE3}', '\u{1CE4}', '\u{1CE5}', '\u{1CE6}', '\u{1CE7}', '\u{1CE8}', '\u{1CED}',
    '\u{1CF4}', '\u{1CF8}', '\u{1CF9}', '\u{1DC0}', '\u{1DC1}', '\u{1DC3}', '\u{1DC4}', '\u{1DC5}',
    '\u{1DC6}', '\u{1DC7}', '\u{1DC8}', '\u{1DC9}', '\u{1DCB}', '\u{1DCC}', '\u{1DD1}', '\u{1DD2}',
    '\u{1DD3}', '\u{1DD4}', '\u{1DD5}', '\u{1DD6}', '\u{1DD7}', '\u{1DD8}', '\u{1DD9}', '\u{1DDA}',
    '\u{1DDB}', '\u{1DDC}', '\u{1DDD}', '\u{1DDE}', '\u{1DDF}', '\u{1DE0}', '\u{1DE1}', '\u{1DE2}',
    '\u{1DE3}', '\u{1DE4}', '\u{1DE5}', '\u{1DE6}', '\u{1DFE}', '\u{20D0}', '\u{20D1}', '\u{20D4}',
    '\u{20D5}', '\u{20D6}', '\u{20D7}', '\u{20DB}', '\u{20DC}', '\u{20E1}', '\u{20E7}', '\u{20E9}',
    '\u{20F0}', '\u{2CEF}', '\u{2CF0}', '\u{2CF1}', '\u{2DE0}', '\u{2DE1}', '\u{2DE2}', '\u{2DE3}',
    '\u{2DE4}', '\u{2DE5}', '\u{2DE6}', '\u{2DE7}', '\u{2DE8}', '\u{2DE9}', '\u{2DEA}', '\u{2DEB}',
    '\u{2DEC}', '\u{2DED}', '\u{2DEE}', '\u{2DEF}', '\u{2DF0}', '\u{2DF1}', '\u{2DF2}', '\u{2DF3}',
    '\u{2DF4}', '\u{2DF5}', '\u{2DF6}', '\u{2DF7}', '\u{2DF8}', '\u{2DF9}', '\u{2DFA}', '\u{2DFB}',
    '\u{2DFC}', '\u{2DFD}', '\u{2DFE}', '\u{2DFF}', '\u{A66F}', '\u{A674}', '\u{A675}', '\u{A676}',
    '\u{A677}', '\u{A678}', '\u{A679}', '\u{A67A}', '\u{A67B}', '\u{A67C}', '\u{A67D}', '\u{A69E}',
    '\u{A69F}', '\u{A6F0}', '\u{A6F1}', '\u{A8E0}', '\u{A8E1}', '\u{A8E2}', '\u{A8E3}', '\u{A8E4}',
    '\u{A8E5}', '\u{A8E6}', '\u{A8E7}', '\u{A8E8}', '\u{A8E9}', '\u{A8EA}', '\u{A8EB}', '\u{A8EC}',
    '\u{A8ED}', '\u{A8EE}', '\u{A8EF}', '\u{A8F0}', '\u{A8F1}',
];

fn diacritic(idx: u16) -> char {
    DIACRITICS
        .get(idx as usize)
        .copied()
        .unwrap_or(DIACRITICS[0])
}

/// Render a cached Kitty image using unicode placeholders into the ratatui
/// buffer. `transmit_seq` is placed into the first visible segment; on
/// subsequent frames it will be an empty string (no re-upload).
fn render_kitty_placeholders(
    area: Rect,
    buf: &mut Buffer,
    encoded: &EncodedFrame,
    occlusion: Option<Rect>,
) {
    let rows = area.height.min(encoded.rows);
    let cols = area.width.min(encoded.cols);

    let [id_extra, id_r, id_g, id_b] = encoded.image_id.to_be_bytes();
    let id_color = format!("\x1b[38;2;{id_r};{id_g};{id_b}m");
    let mut upload = Some(encoded.transmit_seq.as_str());
    let mut last_anchor = None;
    for y in 0..rows {
        let row_y = area.top() + y;
        let row_left = area.left();
        let row_right = row_left.saturating_add(cols);
        let segments = if let Some(covered) = occlusion.filter(|covered| {
            row_y >= covered.top()
                && row_y < covered.bottom()
                && covered.left() < row_right
                && covered.right() > row_left
        }) {
            let covered_left = covered.left().clamp(row_left, row_right);
            let covered_right = covered.right().clamp(row_left, row_right);
            [
                (row_left, covered_left.saturating_sub(row_left)),
                (covered_right, row_right.saturating_sub(covered_right)),
            ]
        } else {
            [(row_left, cols), (row_right, 0)]
        };

        for (segment_left, segment_width) in segments {
            if segment_width == 0 {
                continue;
            }
            // A packed segment keeps combining diacritics together while
            // allowing an overlay to occlude the middle of a PDF row. The
            // first visible cell after the overlay becomes a new emitter,
            // rather than remaining a skipped continuation of a covered cell.
            let mut symbol = upload.take().unwrap_or_default().to_owned();
            symbol.reserve(3 + id_color.len() + (segment_width as usize * 4) + 19);
            let source_col = segment_left.saturating_sub(area.left());
            let _ = write!(
                symbol,
                "\x1b[s{id_color}\u{10EEEE}{}{}{}",
                diacritic(y),
                diacritic(source_col),
                diacritic(u16::from(id_extra))
            );
            symbol.extend(std::iter::repeat_n(
                '\u{10EEEE}',
                segment_width.saturating_sub(1) as usize,
            ));

            for x in 1..segment_width {
                if let Some(cell) = buf.cell_mut((segment_left + x, row_y)) {
                    cell.set_skip(true);
                }
            }

            // Restore the cursor and finish at the PDF pane's bottom-right,
            // matching ratatui-image's packed-row cursor contract.
            let right = area.right().saturating_sub(segment_left).saturating_sub(1);
            let down = area.height.saturating_sub(1);
            let _ = write!(symbol, "\x1b[u\x1b[{right}C\x1b[{down}B");

            if let Some(cell) = buf.cell_mut((segment_left, row_y)) {
                cell.set_symbol(&symbol);
            }
            last_anchor = Some((segment_left, row_y));
        }
    }

    // Retire the old image only after the final visible replacement segment.
    if let Some(id) = encoded.retire_image_id
        && let Some(anchor) = last_anchor
        && let Some(cell) = buf.cell_mut(anchor)
    {
        let mut symbol = cell.symbol().to_owned();
        symbol.push_str(&kitty_delete_image(id));
        cell.set_symbol(&symbol);
    }
}

fn fully_covered(area: Rect, occlusion: Option<Rect>) -> bool {
    if let Some(covered) = occlusion {
        let intersection = area.intersection(covered);
        intersection == area
    } else {
        false
    }
}

/// A ratatui widget that blits the pre-encoded Kitty frame.
struct KittyBlit<'a> {
    encoded: &'a EncodedFrame,
    occlusion: Option<Rect>,
}

impl Widget for KittyBlit<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_kitty_placeholders(area, buf, self.encoded, self.occlusion);
    }
}

struct KittyCleanup<'a> {
    sequence: &'a str,
}

impl Widget for KittyCleanup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if let Some(cell) = buf.cell_mut((area.x, area.y)) {
            let visible_symbol = cell.symbol().to_owned();
            let symbol = format!("{}{}", self.sequence, visible_symbol);
            cell.set_symbol(&symbol);
        }
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
    draw_pdf_viewer_in_with_occlusion(frame, app, area, None);
}

struct ViewerFrameData {
    font_w: u16,
    font_h: u16,
    page_data: Option<RenderedPage>,
    next_page_data: Option<RenderedPage>,
    current_page: usize,
    current_scroll_px: f64,
    last_crop_key: Option<CropKey>,
    has_encoded_frame: bool,
    is_kitty: bool,
    /// Draw with whole-page images and pixel-exact placements.
    continuous: bool,
}

struct ViewerLayout {
    pdf_path: PathBuf,
    pdf_area: Rect,
    status_area: Rect,
    page_fit: bool,
}

fn prepare_viewer_layout(frame: &mut Frame<'_>, app: &mut App, area: Rect) -> Option<ViewerLayout> {
    let bg = if app.settings_modal.transparent_background {
        Color::Reset
    } else {
        Color::Rgb(20, 20, 20)
    };
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(Style::default().bg(bg)), area);
    let Some(pdf_path) = app.pdf_viewer_path.clone() else {
        frame.render_widget(
            Paragraph::new("No PDF file open").style(Style::default().fg(Color::Red)),
            area,
        );
        return None;
    };
    if let Some(total_pages) = page_count(&pdf_path) {
        app.pdf_viewer_total_pages = total_pages;
        app.pdf_viewer_page = app.pdf_viewer_page.min(total_pages.max(1));
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);
    Some(ViewerLayout {
        pdf_path,
        pdf_area: chunks[0],
        status_area: chunks[1],
        page_fit: app.page == Page::Projects
            && app.active_project.is_some()
            && app.pdf_viewer == "internal",
    })
}

fn prepare_viewer_frame(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    pdf_area: Rect,
    pdf_path: &Path,
    page_fit: bool,
    continuous_allowed: bool,
) -> Option<ViewerFrameData> {
    let cache_arc = get_cache();
    let Ok(mut cache) = cache_arc.lock() else {
        return None;
    };
    let Some(picker) = cache.picker.as_ref() else {
        drop(cache);
        frame.render_widget(
            Paragraph::new(
                "Terminal graphics not detected (Kitty / Sixel / iTerm2 / halfblocks). \
                 Press Esc to exit.",
            )
            .style(Style::default().fg(Color::Yellow))
            .alignment(ratatui::layout::Alignment::Center),
            area,
        );
        return None;
    };
    let (font_w, font_h) = picker.font_size();
    let is_kitty = matches!(picker.protocol_type(), ProtocolType::Kitty);
    let continuous = continuous_allowed && is_kitty;
    let pixel_w = u32::from(pdf_area.width) * u32::from(font_w);
    let pixel_h = u32::from(pdf_area.height) * u32::from(font_h);
    if pixel_w == 0 || pixel_h == 0 {
        return None;
    }
    let generation = *cache.document_generations.get(pdf_path).unwrap_or(&0);
    cache
        .document_widths
        .insert(pdf_path.to_path_buf(), pixel_w);
    let selected_page = app.pdf_viewer_page;
    // Continuous mode may show several pages at once; keep two pages of
    // slack beyond the last visible one (bounded after large jumps).
    let lowest_kept = selected_page.saturating_sub(2);
    let highest_kept = if continuous {
        selected_page + 2 + cache.visible_last_page.saturating_sub(selected_page).min(4)
    } else {
        selected_page + 2
    };
    cache
        .pages
        .retain(|(path, cached_generation, page, _, cached_width), _| {
            path != pdf_path
                || (*cached_generation == generation
                    && *cached_width == pixel_w
                    && (lowest_kept..=highest_kept).contains(page))
        });
    cancel_render_jobs(&mut cache, |(path, _, _, _, cached_width)| {
        path == pdf_path && *cached_width != pixel_w
    });
    cache.last_viewport_h = pixel_h;
    cache.page_gap = if continuous {
        f64::from((font_h / 2).max(4))
    } else {
        0.0
    };
    // Pages of one document almost always share a size, so a rendered page
    // is the best estimate for those still rasterising.
    cache.default_page_h = cache
        .pages
        .iter()
        .find(|((path, cached_generation, _, _, cached_width), _)| {
            path == pdf_path && *cached_generation == generation && *cached_width == pixel_w
        })
        .map_or(f64::from(pixel_w) * 1.3, |(_, page)| f64::from(page.pixel_h));
    update_viewer_scroll(&mut cache, app.pdf_viewer_total_pages, page_fit);
    app.pdf_viewer_page = cache.current_page;
    app.pdf_viewer_scroll_y = (cache.current_scroll_px / f64::from(font_h))
        .to_u32()
        .unwrap_or(u32::MAX);
    let key = (
        pdf_path.to_path_buf(),
        generation,
        cache.current_page,
        DPI,
        pixel_w,
    );
    let next_key = (
        pdf_path.to_path_buf(),
        generation,
        cache.current_page + 1,
        DPI,
        pixel_w,
    );
    Some(ViewerFrameData {
        font_w,
        font_h,
        page_data: cache.pages.get(&key).cloned(),
        next_page_data: cache.pages.get(&next_key).cloned(),
        current_page: cache.current_page,
        current_scroll_px: cache.current_scroll_px,
        last_crop_key: cache.last_crop_key.clone(),
        has_encoded_frame: cache.last_encoded.is_some(),
        is_kitty,
        continuous,
    })
}

fn update_viewer_scroll(cache: &mut PageCache, total_pages: usize, page_fit: bool) {
    let now = std::time::Instant::now();
    let elapsed = cache
        .last_update
        .map_or(0.0, |previous| (now - previous).as_secs_f64());
    cache.last_update = Some(now);
    let default_height = cache.default_page_h;
    let gap = cache.page_gap;
    if page_fit {
        cache.target_scroll_px = 0.0;
        cache.current_scroll_px = 0.0;
        return;
    }
    if elapsed <= 0.0 {
        return;
    }
    let target = to_relative_px(
        cache.target_page,
        cache.target_scroll_px,
        cache.current_page,
        &cache.pages,
        default_height,
        gap,
    );
    let difference = target - cache.current_scroll_px;
    cache.current_scroll_px = if difference.abs() > 0.5 {
        cache.current_scroll_px + difference * (1.0 - (-SCROLL_SMOOTHING * elapsed).exp())
    } else {
        target
    };
    normalize_coords(
        &mut cache.current_page,
        &mut cache.current_scroll_px,
        total_pages,
        &cache.pages,
        default_height,
        gap,
    );
}

/// Exponential ease-out rate (1/s) used to chase the scroll target. A wheel
/// tick settles within ~150 ms, close to browser smooth scrolling.
const SCROLL_SMOOTHING: f64 = 20.0;

/// Draw the PDF while leaving an overlapping terminal-cell region available
/// to a later overlay widget. Kitty rows are split around that exact region.
pub fn draw_pdf_viewer_in_with_occlusion(
    frame: &mut Frame<'_>,
    app: &mut App,
    area: Rect,
    occlusion: Option<Rect>,
) {
    let Some(layout) = prepare_viewer_layout(frame, app, area) else {
        return;
    };

    let Some(view) = prepare_viewer_frame(
        frame,
        app,
        area,
        layout.pdf_area,
        &layout.pdf_path,
        layout.page_fit,
        !layout.page_fit && occlusion.is_none() && continuous_scrolling_enabled(),
    ) else {
        return;
    };

    let pixel_w = u32::from(layout.pdf_area.width) * u32::from(view.font_w);
    let pixel_h = u32::from(layout.pdf_area.height) * u32::from(view.font_h);
    if pixel_w == 0 || pixel_h == 0 {
        return;
    }

    if view.continuous {
        draw_continuous(
            frame,
            app,
            &layout.pdf_path,
            layout.pdf_area,
            layout.status_area,
            &view,
        );
    } else {
        draw_cropped(frame, app, &layout, occlusion, &view);
    }
}

fn draw_cropped(
    frame: &mut Frame<'_>,
    app: &mut App,
    layout: &ViewerLayout,
    occlusion: Option<Rect>,
    view: &ViewerFrameData,
) {
    let ViewerLayout {
        ref pdf_path,
        pdf_area,
        status_area,
        page_fit,
    } = *layout;
    let pixel_w = u32::from(pdf_area.width) * u32::from(view.font_w);
    let pixel_h = u32::from(pdf_area.height) * u32::from(view.font_h);
    request_visible_pages(
        pdf_path,
        view.current_page,
        app.pdf_viewer_total_pages,
        pixel_w,
    );

    let Some(page_data) = view.page_data.as_ref() else {
        render_pending_page(frame, pdf_area, view.current_page);
        render_status_bar(frame, status_area, pdf_path, view.current_page, app, 0, 0);
        return;
    };

    let page_h = page_data.pixel_h;
    app.pdf_viewer_page_pixel_h = page_h;
    let max_scroll_px = if page_fit {
        0
    } else {
        page_h.saturating_sub(pixel_h)
    };
    let scroll_px = if page_fit {
        0
    } else {
        view.current_scroll_px
            .to_u32()
            .unwrap_or(u32::MAX)
            .min(page_h)
    };
    let max_scroll_cells = max_scroll_px / u32::from(view.font_h);
    app.pdf_viewer_max_scroll_y = max_scroll_cells;

    // Build the crop key for this frame
    let transparent_padding = app.settings_modal.transparent_background && view.is_kitty;
    let crop_key = CropKey {
        path_fp: path_fingerprint(pdf_path),
        page: view.current_page,
        dpi: DPI,
        pixel_w,
        crop_y: scroll_px,
        crop_h: pixel_h,
        page_fit,
        transparent_padding,
    };

    let needs_encode = view.last_crop_key.as_ref() != Some(&crop_key);
    if (needs_encode || !view.has_encoded_frame)
        && encode_pdf_crop(
            frame,
            pdf_area,
            pdf_path,
            CropRenderContext {
                page_data,
                next_page_data: view.next_page_data.as_ref(),
                crop_key,
                pixel_w,
                pixel_h,
                scroll_px,
                page_fit,
                current_page: view.current_page,
                total_pages: app.pdf_viewer_total_pages,
                is_kitty: view.is_kitty,
            },
        )
    {
        render_status_bar(
            frame,
            status_area,
            pdf_path,
            view.current_page,
            app,
            app.pdf_viewer_scroll_y,
            max_scroll_cells,
        );
        return;
    }

    if view.is_kitty {
        blit_kitty_frame(frame, pdf_area, occlusion);
    }

    render_status_bar(
        frame,
        status_area,
        pdf_path,
        view.current_page,
        app,
        app.pdf_viewer_scroll_y,
        max_scroll_cells,
    );
}

fn request_visible_pages(pdf_path: &Path, current: usize, total: usize, pixel_w: u32) {
    for (candidate, priority) in [
        (Some(current), true),
        (current.checked_add(1), false),
        (current.checked_sub(1), false),
        (current.checked_add(2), false),
        (current.checked_sub(2), false),
    ] {
        if let Some(page) = candidate.filter(|page| *page <= total) {
            request_page(pdf_path, page, DPI, pixel_w, priority);
        }
    }
}

/// Draw the document as a continuous, pixel-scrolled strip of pages.
///
/// Each rendered page is uploaded to the terminal once; every frame only
/// emits placement commands (a few hundred bytes) selecting the visible
/// window of each page. The commands are queued in `graphics_output` and
/// written by [`flush_terminal_graphics`] after Ratatui's draw.
fn draw_continuous(
    frame: &mut Frame<'_>,
    app: &mut App,
    pdf_path: &Path,
    pdf_area: Rect,
    status_area: Rect,
    view: &ViewerFrameData,
) {
    let pixel_w = u32::from(pdf_area.width) * u32::from(view.font_w);
    let pixel_h = u32::from(pdf_area.height) * u32::from(view.font_h);
    let font_h = u32::from(view.font_h);
    let viewport_h = i64::from(pixel_h);
    // Default-background cells let Kitty draw the page images beneath them.
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Reset)),
        pdf_area,
    );

    let cache_arc = get_cache();
    let Ok(mut g) = cache_arc.lock() else {
        return;
    };
    let generation = *g.document_generations.get(pdf_path).unwrap_or(&0);
    let key_for = |page: usize| (pdf_path.to_path_buf(), generation, page, DPI, pixel_w);
    let last_page = app.pdf_viewer_total_pages.max(view.current_page).max(1);
    let slots = layout_continuous_pages(
        view.current_page,
        view.current_scroll_px,
        last_page,
        viewport_h,
        g.page_gap.round().to_i64().unwrap_or(0),
        g.default_page_h.round().to_i64().unwrap_or(1000),
        |page| {
            g.pages
                .get(&key_for(page))
                .map(|rendered| i64::from(rendered.pixel_h))
        },
    );
    let last_visible = slots.last().map_or(view.current_page, |slot| slot.page);
    g.visible_last_page = last_visible;
    sync_continuous_uploads(
        &mut g,
        &cache_arc,
        pdf_path,
        generation,
        pixel_w,
        view.current_page,
        last_visible,
    );

    // Position the visible window of each resident page. Placements are
    // re-issued every frame: they are tiny, and this also restores them
    // after anything (e.g. a terminal clear) removed them.
    let mut output = String::new();
    let mut placed = HashSet::new();
    let mut loading = Vec::new();
    for slot in &slots {
        let upload_id = match g.uploads.get(&key_for(slot.page)) {
            Some(KittyUpload::Uploaded { id, .. }) if slot.rendered => Some(*id),
            _ => None,
        };
        let Some(id) = upload_id else {
            loading.push(slot);
            continue;
        };
        if let Some(placement) = page_placement(slot.top, slot.height, viewport_h, font_h) {
            output.push_str(&kitty_place_command(pdf_area, id, pixel_w, &placement));
            placed.insert(id);
        }
    }
    for id in g.placed.difference(&placed) {
        output.push_str(&kitty_delete_placements(*id));
    }
    g.placed = placed;
    g.graphics_output.push_str(&output);
    g.continuous_drawn = true;
    g.continuous_pending = !loading.is_empty();
    let current_page_h = g
        .pages
        .get(&key_for(view.current_page))
        .map(|rendered| rendered.pixel_h);
    drop(g);

    for slot in loading {
        render_loading_slot(frame, pdf_area, slot, viewport_h, font_h);
    }

    request_continuous_pages(
        pdf_path,
        view.current_page,
        last_visible,
        app.pdf_viewer_total_pages,
        pixel_w,
    );

    let page_h = current_page_h.unwrap_or(0);
    app.pdf_viewer_page_pixel_h = page_h;
    let max_scroll_cells = page_h.saturating_sub(pixel_h) / font_h.max(1);
    app.pdf_viewer_max_scroll_y = max_scroll_cells;
    render_status_bar(
        frame,
        status_area,
        pdf_path,
        view.current_page,
        app,
        app.pdf_viewer_scroll_y,
        max_scroll_cells,
    );
}

fn sync_continuous_uploads(
    g: &mut PageCache,
    cache_arc: &Arc<Mutex<PageCache>>,
    pdf_path: &Path,
    generation: u64,
    pixel_w: u32,
    first_visible: usize,
    last_visible: usize,
) {
    let key_for = |page: usize| (pdf_path.to_path_buf(), generation, page, DPI, pixel_w);
    // Free terminal images whose page left the raster cache.
    let evicted = g
        .uploads
        .keys()
        .filter(|key| !g.pages.contains_key(*key))
        .cloned()
        .collect::<HashSet<_>>();
    drop_uploads(g, |key| evicted.contains(key));

    // Prepare uploads for visible pages and their immediate neighbours, so a
    // page is normally resident before it scrolls into view.
    let use_file = kitty_file_transfer_enabled();
    let mut wanted = (first_visible..=last_visible).collect::<Vec<_>>();
    wanted.push(last_visible + 1);
    wanted.extend(first_visible.checked_sub(1).filter(|page| *page >= 1));
    for page in wanted {
        let key = key_for(page);
        if g.uploads.contains_key(&key) {
            continue;
        }
        let Some(image) = g.pages.get(&key).map(|rendered| Arc::clone(&rendered.image)) else {
            continue;
        };
        let id = g.next_kitty_id;
        g.next_kitty_id = g.next_kitty_id.wrapping_add(1).max(1);
        g.uploads
            .insert(key.clone(), KittyUpload::Preparing { id });
        spawn_upload_preparation(Arc::clone(cache_arc), key, id, image, use_file);
    }

    // Transmit every prepared page (once).
    let ready = g
        .uploads
        .iter()
        .filter(|(_, upload)| matches!(upload, KittyUpload::Ready { .. }))
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in ready {
        if let Some(KittyUpload::Ready { id, command, file }) = g.uploads.remove(&key) {
            g.graphics_output.push_str(&command);
            g.uploads.insert(key, KittyUpload::Uploaded { id, file });
        }
    }
}

/// Request visible pages first (highest priority), then prefetch in both
/// directions with a bias towards reading forward.
fn request_continuous_pages(
    pdf_path: &Path,
    first_visible: usize,
    last_visible: usize,
    total_pages: usize,
    pixel_w: u32,
) {
    for page in (first_visible..=last_visible).rev() {
        request_page(pdf_path, page, DPI, pixel_w, true);
    }
    for page in [
        Some(last_visible + 1),
        first_visible.checked_sub(1),
        Some(last_visible + 2),
        first_visible.checked_sub(2),
    ]
    .into_iter()
    .flatten()
    .filter(|page| (1..=total_pages).contains(page))
    {
        request_page(pdf_path, page, DPI, pixel_w, false);
    }
}

/// Show a "Rendering page N…" label centred in a page slot that has no
/// resident image yet.
fn render_loading_slot(frame: &mut Frame<'_>, area: Rect, slot: &PageSlot, viewport_h: i64, font_h: u32) {
    let font_h = i64::from(font_h.max(1));
    let top = slot.top.max(0);
    let bottom = slot.top.saturating_add(slot.height).min(viewport_h);
    if bottom <= top {
        return;
    }
    let first_row = top / font_h;
    let last_row = (bottom - 1) / font_h;
    let middle = u16::try_from(i64::midpoint(first_row, last_row)).unwrap_or(0);
    if middle >= area.height {
        return;
    }
    let label_area = Rect::new(area.x, area.y + middle, area.width, 1);
    frame.render_widget(
        Paragraph::new(format!("Rendering page {}…", slot.page))
            .style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .alignment(ratatui::layout::Alignment::Center),
        label_area,
    );
}

fn render_pending_page(frame: &mut Frame<'_>, area: Rect, page: usize) {
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
            "Rendering page {page}{dots}  (press Esc to cancel)"
        ))
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .alignment(ratatui::layout::Alignment::Center),
        area,
    );
}

struct CropRenderContext<'a> {
    page_data: &'a RenderedPage,
    next_page_data: Option<&'a RenderedPage>,
    crop_key: CropKey,
    pixel_w: u32,
    pixel_h: u32,
    scroll_px: u32,
    page_fit: bool,
    current_page: usize,
    total_pages: usize,
    is_kitty: bool,
}

/// Returns true when a non-Kitty frame was rendered immediately.
fn encode_pdf_crop(
    frame: &mut Frame<'_>,
    area: Rect,
    pdf_path: &Path,
    context: CropRenderContext<'_>,
) -> bool {
    let page_height = context.page_data.pixel_h;
    let transparent_padding = context.crop_key.transparent_padding;
    let cropped = if context.page_fit {
        fit_page_to_viewport(&context.page_data.image, context.pixel_w, context.pixel_h)
    } else if context.scroll_px + context.pixel_h > page_height
        && context.current_page < context.total_pages
    {
        context.next_page_data.map_or_else(
            || {
                crop_single_with_padding(
                    &context.page_data.image,
                    context.scroll_px,
                    context.pixel_h,
                    transparent_padding,
                )
            },
            |next| {
                crop_and_stitch(
                    &context.page_data.image,
                    &next.image,
                    context.scroll_px,
                    context.pixel_h,
                )
            },
        )
    } else if context
        .pixel_h
        .min(page_height.saturating_sub(context.scroll_px))
        < context.pixel_h
    {
        crop_single_with_padding(
            &context.page_data.image,
            context.scroll_px,
            context.pixel_h,
            transparent_padding,
        )
    } else {
        crop_view(&context.page_data.image, context.scroll_px, context.pixel_h)
    };
    let cache_arc = get_cache();
    let Ok(mut cache) = cache_arc.lock() else {
        return false;
    };
    cache.last_crop_key = Some(context.crop_key);
    if context.is_kitty {
        store_kitty_frame(&mut cache, &cropped, pdf_path, area);
        return false;
    }
    let Some(picker) = cache.picker.as_ref() else {
        return false;
    };
    let mut protocol = picker.new_resize_protocol(cropped);
    drop(cache);
    frame.render_stateful_widget(
        StatefulImage::new().resize(Resize::Fit(None)),
        area,
        &mut protocol,
    );
    true
}

fn store_kitty_frame(cache: &mut PageCache, image: &DynamicImage, pdf_path: &Path, area: Rect) {
    let id = cache.next_kitty_id;
    cache.next_kitty_id = cache.next_kitty_id.wrapping_add(1).max(1);
    let path_fp = path_fingerprint(pdf_path);
    let retire_image_id = cache
        .resident_kitty_image
        .filter(|(resident_fp, _)| *resident_fp == path_fp)
        .map(|(_, resident_id)| resident_id);
    let [_id_extra, id_r, id_g, id_b] = id.to_be_bytes();
    cache.last_encoded = Some(EncodedFrame {
        transmit_seq: kitty_transmit(image, id),
        image_id: id,
        cols: area.width,
        rows: area.height,
        id_color: format!("\x1b[38;2;{id_r};{id_g};{id_b}m"),
        retire_image_id,
    });
    cache.resident_kitty_image = Some((path_fp, id));
}

fn blit_kitty_frame(frame: &mut Frame<'_>, area: Rect, occlusion: Option<Rect>) {
    let cache_arc = get_cache();
    let Ok(mut cache) = cache_arc.lock() else {
        return;
    };
    let Some(encoded) = cache.last_encoded.as_mut() else {
        return;
    };
    let should_transmit = !fully_covered(area, occlusion) && !encoded.transmit_seq.is_empty();
    let blit_frame = EncodedFrame {
        transmit_seq: if should_transmit {
            std::mem::take(&mut encoded.transmit_seq)
        } else {
            String::new()
        },
        image_id: encoded.image_id,
        cols: encoded.cols,
        rows: encoded.rows,
        id_color: encoded.id_color.clone(),
        retire_image_id: should_transmit
            .then(|| encoded.retire_image_id.take())
            .flatten(),
    };
    drop(cache);
    frame.render_widget(
        KittyBlit {
            encoded: &blit_frame,
            occlusion,
        },
        area,
    );
}

/// Letterbox a page into the available pane pixels without cropping it.
/// Project Preview uses this to make each navigation step correspond to one
/// complete PDF page regardless of the page's aspect ratio.
fn fit_page_to_viewport(page: &RgbaImage, viewport_w: u32, viewport_h: u32) -> DynamicImage {
    let scaled = DynamicImage::ImageRgba8(page.clone())
        .resize(
            viewport_w,
            viewport_h,
            image::imageops::FilterType::Triangle,
        )
        .into_rgba8();
    let mut canvas = RgbaImage::from_pixel(viewport_w, viewport_h, Rgba([255, 255, 255, 255]));
    let x = i64::from(viewport_w.saturating_sub(scaled.width()) / 2);
    let y = i64::from(viewport_h.saturating_sub(scaled.height()) / 2);
    image::imageops::overlay(&mut canvas, &scaled, x, y);
    DynamicImage::ImageRgba8(canvas)
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
            format!(" {filename} "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" │ {}/{} ", page, app.pdf_viewer_total_pages),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!(" │ {scroll}/{max_scroll} rows "),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            " │ Esc:exit  ↑↓/jk:scroll  PgUp/Dn:page ",
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let status_bg = if app.settings_modal.transparent_background {
        Color::Reset
    } else {
        Color::Rgb(28, 28, 36)
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().bg(status_bg)),
        area,
    );
}

// ---------------------------------------------------------------------------
// Cache check helpers
// ---------------------------------------------------------------------------

pub fn is_current_page_cached(app: &App) -> bool {
    let Some(pdf_path) = &app.pdf_viewer_path else {
        return true;
    };
    let page = app.pdf_viewer_page;
    let dpi = DPI;
    if let Some(arc) = CACHE.get()
        && let Ok(g) = arc.lock()
    {
        let generation = *g.document_generations.get(pdf_path).unwrap_or(&0);
        return g.pages.keys().any(|(p, doc_generation, pg, d, _)| {
            p == pdf_path && *doc_generation == generation && *pg == page && *d == dpi
        });
    }
    false
}

pub fn is_animating() -> bool {
    if let Some(arc) = CACHE.get()
        && let Ok(g) = arc.lock()
    {
        let target_rel = to_relative_px(
            g.target_page,
            g.target_scroll_px,
            g.current_page,
            &g.pages,
            g.default_page_h,
            g.page_gap,
        );
        return (target_rel - g.current_scroll_px).abs() > 0.5;
    }
    false
}

// ---------------------------------------------------------------------------
// Smooth scroll physics & stitching helpers
// ---------------------------------------------------------------------------

fn get_page_height_helper(
    page: usize,
    pages: &HashMap<PageKey, RenderedPage>,
    default_h: f64,
) -> f64 {
    for (key, val) in pages {
        if key.2 == page {
            return f64::from(val.pixel_h);
        }
    }
    default_h
}

/// Height of a page plus the gap that follows it in the continuous strip.
fn slot_height(page: usize, pages: &HashMap<PageKey, RenderedPage>, default_h: f64, gap: f64) -> f64 {
    get_page_height_helper(page, pages, default_h) + gap
}

fn normalize_coords(
    page: &mut usize,
    offset: &mut f64,
    total_pages: usize,
    pages: &HashMap<PageKey, RenderedPage>,
    default_h: f64,
    gap: f64,
) {
    loop {
        let h = slot_height(*page, pages, default_h, gap);
        if *offset >= h && *page < total_pages {
            *offset -= h;
            *page += 1;
        } else if *offset < 0.0 && *page > 1 {
            *page -= 1;
            let prev_h = slot_height(*page, pages, default_h, gap);
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
    gap: f64,
) -> f64 {
    match page.cmp(&current_page) {
        std::cmp::Ordering::Equal => offset,
        std::cmp::Ordering::Greater => {
            let mut sum = 0.0;
            for p in current_page..page {
                sum += slot_height(p, pages, default_h, gap);
            }
            sum + offset
        }
        std::cmp::Ordering::Less => {
            let mut sum = 0.0;
            for p in page..current_page {
                sum += slot_height(p, pages, default_h, gap);
            }
            offset - sum
        }
    }
}

/// Move the scroll target by `delta_px` and clamp it to the document: the
/// top of page 1 at the top, and the bottom of the last page at the bottom
/// of the viewport.
fn move_scroll_target(g: &mut PageCache, delta_px: f64, total_pages: usize) {
    let default_h = g.default_page_h;
    let gap = g.page_gap;
    let viewport_h = f64::from(g.last_viewport_h);
    let mut page = g.target_page;
    let mut offset = g.target_scroll_px + delta_px;
    normalize_coords(&mut page, &mut offset, total_pages, &g.pages, default_h, gap);

    if total_pages > 0 && page <= total_pages {
        // Content remaining below the viewport top, stopping early once it
        // exceeds a viewport (the common case away from the end).
        let mut remaining = -offset;
        for p in page..=total_pages {
            remaining += get_page_height_helper(p, &g.pages, default_h);
            if p < total_pages {
                remaining += gap;
            }
            if remaining >= viewport_h {
                break;
            }
        }
        if remaining < viewport_h {
            offset -= viewport_h - remaining;
            normalize_coords(&mut page, &mut offset, total_pages, &g.pages, default_h, gap);
        }
    }
    if page <= 1 && offset < 0.0 {
        page = 1;
        offset = 0.0;
    }
    g.target_page = page;
    g.target_scroll_px = offset;
}

pub fn scroll_by_rows(app: &App, delta_rows: i64) {
    let cache_arc = get_cache();
    let Ok(mut g) = cache_arc.lock() else { return };
    let font_h = f64::from(g.picker.as_ref().map_or(16, |p| p.font_size().1));
    let bounded_rows = i32::try_from(delta_rows).unwrap_or(if delta_rows.is_negative() {
        i32::MIN
    } else {
        i32::MAX
    });
    let delta_px = f64::from(bounded_rows) * font_h;
    move_scroll_target(&mut g, delta_px, app.pdf_viewer_total_pages);
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
        let scroll_amt = f64::from(g.last_viewport_h) * 0.9;
        move_scroll_target(&mut g, -scroll_amt, app.pdf_viewer_total_pages);
    }
}

pub fn page_down(app: &App) {
    let cache_arc = get_cache();
    if let Ok(mut g) = cache_arc.lock() {
        let scroll_amt = f64::from(g.last_viewport_h) * 0.9;
        move_scroll_target(&mut g, scroll_amt, app.pdf_viewer_total_pages);
    }
}

fn crop_and_stitch(
    curr_img: &RgbaImage,
    next_img: &RgbaImage,
    scroll_y: u32,
    viewport_h: u32,
) -> DynamicImage {
    let w = curr_img.width();
    if w == 0 || viewport_h == 0 {
        return DynamicImage::ImageRgba8(RgbaImage::new(w.max(1), viewport_h.max(1)));
    }
    let curr_h = curr_img.height();
    let curr_visible = curr_h.saturating_sub(scroll_y);
    let next_visible = viewport_h.saturating_sub(curr_visible);

    let mut data = vec![0u8; (w * viewport_h * 4) as usize];
    let bytes_per_row = w as usize * 4;

    if curr_visible > 0 {
        let start = (scroll_y as usize * bytes_per_row).min(curr_img.as_raw().len());
        let end = (start + curr_visible as usize * bytes_per_row).min(curr_img.as_raw().len());
        let slice = &curr_img.as_raw()[start..end];
        let copy_len = slice.len().min(data.len());
        data[0..copy_len].copy_from_slice(&slice[0..copy_len]);
    }

    if next_visible > 0 && next_img.width() == w {
        let end = (next_visible as usize * bytes_per_row).min(next_img.as_raw().len());
        let slice = &next_img.as_raw()[0..end];
        let dest_start = (curr_visible as usize * bytes_per_row).min(data.len());
        let dest_end = (dest_start + slice.len()).min(data.len());
        let copy_len = dest_end.saturating_sub(dest_start);
        if copy_len > 0 {
            data[dest_start..dest_end].copy_from_slice(&slice[0..copy_len]);
        }
    }

    let stitched =
        RgbaImage::from_raw(w, viewport_h, data).unwrap_or_else(|| RgbaImage::new(w, viewport_h));
    DynamicImage::ImageRgba8(stitched)
}

fn crop_single_with_padding(
    curr_img: &RgbaImage,
    scroll_y: u32,
    viewport_h: u32,
    transparent_padding: bool,
) -> DynamicImage {
    let w = curr_img.width();
    if w == 0 || viewport_h == 0 {
        return DynamicImage::ImageRgba8(RgbaImage::new(w.max(1), viewport_h.max(1)));
    }
    let curr_h = curr_img.height();
    let curr_visible = curr_h.saturating_sub(scroll_y);

    let mut data = vec![0u8; (w * viewport_h * 4) as usize];
    let bytes_per_row = w as usize * 4;

    if curr_visible > 0 {
        let start = (scroll_y as usize * bytes_per_row).min(curr_img.as_raw().len());
        let end = (start + curr_visible as usize * bytes_per_row).min(curr_img.as_raw().len());
        let slice = &curr_img.as_raw()[start..end];
        let copy_len = slice.len().min(data.len());
        data[0..copy_len].copy_from_slice(&slice[0..copy_len]);
    }

    if !transparent_padding {
        let curr_len = (curr_visible as usize * bytes_per_row).min(data.len());
        let padding_slice = &mut data[curr_len..];
        padding_slice
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|pixel| {
                pixel[0] = 20;
                pixel[1] = 20;
                pixel[2] = 20;
                pixel[3] = 255;
            });
    }

    let stitched =
        RgbaImage::from_raw(w, viewport_h, data).unwrap_or_else(|| RgbaImage::new(w, viewport_h));
    DynamicImage::ImageRgba8(stitched)
}

#[cfg(test)]
mod kitty_placeholder_tests {
    use super::*;

    #[test]
    fn emits_each_kitty_placeholder_row_from_one_buffer_cell() {
        let area = Rect::new(0, 0, 3, 2);
        let mut buffer = Buffer::empty(area);
        let encoded = EncodedFrame {
            transmit_seq: "upload".to_owned(),
            image_id: 1,
            cols: 3,
            rows: 2,
            id_color: String::new(),
            retire_image_id: None,
        };

        render_kitty_placeholders(area, &mut buffer, &encoded, None);

        for y in 0..2 {
            let row = buffer[(0, y)].symbol();
            assert_eq!(row.matches('\u{10EEEE}').count(), 3);
            assert!(row.contains("\x1b[s"));
            assert!(row.contains("\x1b[u"));
            assert_eq!(buffer[(1, y)].symbol(), " ");
            assert_eq!(buffer[(2, y)].symbol(), " ");
        }
        assert!(buffer[(0, 0)].symbol().starts_with("upload"));
        assert!(!buffer[(0, 1)].symbol().contains("upload"));
    }

    #[test]
    fn retires_the_previous_image_after_the_last_new_placeholder_row() {
        let area = Rect::new(0, 0, 2, 2);
        let mut buffer = Buffer::empty(area);
        let encoded = EncodedFrame {
            transmit_seq: String::new(),
            image_id: 2,
            cols: 2,
            rows: 2,
            id_color: String::new(),
            retire_image_id: Some(1),
        };

        render_kitty_placeholders(area, &mut buffer, &encoded, None);

        assert!(!buffer[(0, 0)].symbol().contains("a=d,d=I,i=1"));
        let final_row = buffer[(0, 1)].symbol();
        let positions = (final_row.rfind('\u{10EEEE}'), final_row.find("a=d,d=I,i=1"));
        assert!(matches!(positions, (Some(placeholder), Some(delete)) if delete > placeholder));
    }

    #[test]
    fn base64_encoder_appends_without_an_intermediate_payload() {
        let mut output = "prefix:".to_owned();
        base64_encode_into(b"Papr PDF", &mut output);
        assert_eq!(output, "prefix:UGFwciBQREY=");
    }

    #[test]
    fn kitty_rows_split_at_an_overlay_and_restore_after_it_closes() {
        let area = Rect::new(0, 0, 6, 2);
        let overlay = Rect::new(0, 0, 3, 2);
        let encoded = EncodedFrame {
            transmit_seq: "upload".to_owned(),
            image_id: 7,
            cols: 6,
            rows: 2,
            id_color: String::new(),
            retire_image_id: None,
        };
        let mut covered = Buffer::empty(area);

        render_kitty_placeholders(area, &mut covered, &encoded, Some(overlay));

        for y in 0..2 {
            for x in 0..3 {
                assert!(!covered[(x, y)].skip);
                assert_eq!(covered[(x, y)].symbol(), " ");
            }
            assert!(!covered[(3, y)].skip);
            assert!(covered[(3, y)].symbol().contains('\u{10EEEE}'));
            assert!(covered[(4, y)].skip);
            assert!(covered[(5, y)].skip);
        }
        assert!(covered[(3, 0)].symbol().starts_with("upload"));

        let right_segment = covered[(3, 0)].symbol().to_owned();
        Clear.render(overlay, &mut covered);
        Paragraph::new("popup").render(overlay, &mut covered);
        assert_eq!(covered[(0, 0)].symbol(), "p");
        assert_eq!(covered[(3, 0)].symbol(), right_segment);

        Clear.render(overlay, &mut covered);
        Paragraph::new("update").render(overlay, &mut covered);
        assert_eq!(covered[(0, 0)].symbol(), "u");
        assert_eq!(covered[(3, 0)].symbol(), right_segment);

        let mut restored = Buffer::empty(area);
        render_kitty_placeholders(area, &mut restored, &encoded, None);
        let updates = covered.diff(&restored);
        assert!(updates.iter().any(|(x, y, _)| (*x, *y) == (0, 0)));
        assert!(updates.iter().any(|(x, y, _)| (*x, *y) == (0, 1)));
    }

    #[test]
    fn kitty_upload_uses_lossless_streaming_compression() {
        let image =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(320, 200, Rgba([245, 245, 245, 255])));
        let upload = kitty_transmit(&image, 42);
        let raw = kitty_transmit_raw(image.as_bytes(), 42, 320, 200);

        assert!(upload.contains("f=32,t=d,s=320,v=200,o=z,"));
        assert!(upload.ends_with("\x1b\\"));
        assert!(upload.len() < raw.len());
    }

    #[test]
    fn crop_single_with_padding_respects_transparent_flag() {
        let src = RgbaImage::from_pixel(2, 1, Rgba([255, 255, 255, 255]));
        let opaque = crop_single_with_padding(&src, 0, 2, false).to_rgba8();
        assert_eq!(*opaque.get_pixel(0, 0), Rgba([255, 255, 255, 255]));
        assert_eq!(*opaque.get_pixel(0, 1), Rgba([20, 20, 20, 255]));

        let transparent = crop_single_with_padding(&src, 0, 2, true).to_rgba8();
        assert_eq!(*transparent.get_pixel(0, 0), Rgba([255, 255, 255, 255]));
        assert_eq!(*transparent.get_pixel(0, 1), Rgba([0, 0, 0, 0]));
    }

    struct ReleaseFixture {
        cache: Arc<Mutex<PageCache>>,
        closed: PathBuf,
        other: PathBuf,
        closed_temp: PathBuf,
        other_temp: PathBuf,
        active_key: PageKey,
        active_cancelled: Arc<AtomicBool>,
    }

    fn release_fixture() -> anyhow::Result<ReleaseFixture> {
        let closed = PathBuf::from("/tmp/papr-closed-document.pdf");
        let other = PathBuf::from("/tmp/papr-other-document.pdf");
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        );
        let closed_temp = std::env::temp_dir().join(format!("papr-closed-raster-{suffix}.png"));
        let other_temp = std::env::temp_dir().join(format!("papr-other-raster-{suffix}.png"));
        std::fs::write(&closed_temp, [])?;
        std::fs::write(&other_temp, [])?;
        let page = RenderedPage {
            pixel_h: 1,
            image: Arc::new(RgbaImage::new(1, 1)),
        };
        let active_key = (closed.clone(), 0, 2, DPI, 1);
        let active_cancelled = Arc::new(AtomicBool::new(false));
        let cache = get_cache();
        let mut state = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("PDF cache lock poisoned"))?;
        state
            .pages
            .insert((closed.clone(), 0, 1, DPI, 1), page.clone());
        state.pages.insert((other.clone(), 0, 1, DPI, 1), page);
        state.page_counts.insert((closed.clone(), 0), 3);
        state.page_counts.insert((other.clone(), 0), 4);
        state.page_count_in_flight.insert((closed.clone(), 0));
        state.page_count_in_flight.insert((other.clone(), 0));
        state.in_flight.insert(active_key.clone());
        state
            .render_cancellations
            .insert(active_key.clone(), Arc::clone(&active_cancelled));
        state.render_queue.push_back(RenderJob {
            key: (closed.clone(), 0, 3, DPI, 1),
            pdf_path: closed.clone(),
            generation: 0,
            page: 3,
            dpi: DPI,
            pixel_w: 1,
            priority: false,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        state.temp_files.push((closed.clone(), closed_temp.clone()));
        state.temp_files.push((other.clone(), other_temp.clone()));
        state.last_crop_key = Some(CropKey {
            path_fp: path_fingerprint(&closed),
            page: 1,
            dpi: DPI,
            pixel_w: 1,
            crop_y: 0,
            crop_h: 1,
            page_fit: false,
            transparent_padding: false,
        });
        state.last_encoded = Some(EncodedFrame {
            transmit_seq: String::new(),
            image_id: 77,
            cols: 1,
            rows: 1,
            id_color: String::new(),
            retire_image_id: None,
        });
        state.resident_kitty_image = Some((path_fingerprint(&closed), 77));
        drop(state);
        Ok(ReleaseFixture {
            cache,
            closed,
            other,
            closed_temp,
            other_temp,
            active_key,
            active_cancelled,
        })
    }

    fn assert_document_released(fixture: &ReleaseFixture) -> anyhow::Result<()> {
        let mut state = fixture
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("PDF cache lock poisoned"))?;
        assert!(
            !state
                .pages
                .keys()
                .any(|(path, _, _, _, _)| path == &fixture.closed)
        );
        assert!(
            !state
                .in_flight
                .iter()
                .any(|(path, _, _, _, _)| path == &fixture.closed)
        );
        assert!(
            !state
                .render_queue
                .iter()
                .any(|job| job.pdf_path == fixture.closed)
        );
        assert!(fixture.active_cancelled.load(Ordering::Acquire));
        assert!(
            state
                .pages
                .keys()
                .any(|(path, _, _, _, _)| path == &fixture.other)
        );
        assert!(!state.page_counts.contains_key(&(fixture.closed.clone(), 0)));
        assert!(state.page_counts.contains_key(&(fixture.other.clone(), 0)));
        assert!(
            !state
                .page_count_in_flight
                .contains(&(fixture.closed.clone(), 0))
        );
        assert!(
            state
                .page_count_in_flight
                .contains(&(fixture.other.clone(), 0))
        );
        assert_eq!(state.document_generations.get(&fixture.closed), Some(&1));
        assert!(state.last_crop_key.is_none());
        assert!(state.last_encoded.is_none());
        assert!(state.pending_kitty_deletes.contains(&77));
        assert!(state.resident_kitty_image.is_none());
        assert!(!fixture.closed_temp.exists());
        assert!(fixture.other_temp.exists());
        state
            .pages
            .retain(|(path, _, _, _, _), _| path != &fixture.other);
        state
            .page_counts
            .retain(|(path, _), _| path != &fixture.other);
        state
            .page_count_in_flight
            .retain(|(path, _)| path != &fixture.other);
        state.pending_kitty_deletes.retain(|id| *id != 77);
        state.render_cancellations.remove(&fixture.active_key);
        state.document_generations.remove(&fixture.closed);
        state
            .temp_files
            .retain(|(_, file)| file != &fixture.other_temp);
        let _ = std::fs::remove_file(&fixture.other_temp);
        Ok(())
    }

    fn assert_rebuild_lifecycle(cache: &Arc<Mutex<PageCache>>) -> anyhow::Result<()> {
        let rebuilt = PathBuf::from("/tmp/papr-rebuilt-document.pdf");
        {
            let mut state = cache
                .lock()
                .map_err(|_| anyhow::anyhow!("PDF cache lock poisoned"))?;
            state.resident_kitty_image = Some((path_fingerprint(&rebuilt), 88));
            state.last_crop_key = Some(CropKey {
                path_fp: path_fingerprint(&rebuilt),
                page: 1,
                dpi: DPI,
                pixel_w: 1,
                crop_y: 0,
                crop_h: 1,
                page_fit: true,
                transparent_padding: false,
            });
            state.last_encoded = Some(EncodedFrame {
                transmit_seq: "old upload".into(),
                image_id: 88,
                cols: 1,
                rows: 1,
                id_color: String::new(),
                retire_image_id: None,
            });
        }
        invalidate_document(&rebuilt);
        {
            let state = cache
                .lock()
                .map_err(|_| anyhow::anyhow!("PDF cache lock poisoned"))?;
            assert_eq!(
                state.resident_kitty_image,
                Some((path_fingerprint(&rebuilt), 88))
            );
            assert!(!state.pending_kitty_deletes.contains(&88));
            assert!(state.last_encoded.is_none());
        }
        release_document(&rebuilt);
        let mut state = cache
            .lock()
            .map_err(|_| anyhow::anyhow!("PDF cache lock poisoned"))?;
        assert!(state.pending_kitty_deletes.contains(&88));
        state.pending_kitty_deletes.retain(|id| *id != 88);
        Ok(())
    }

    #[test]
    fn releasing_a_document_removes_only_its_cache_and_invalidates_jobs() -> anyhow::Result<()> {
        let fixture = release_fixture()?;
        release_document(&fixture.closed);
        assert_document_released(&fixture)?;
        assert_rebuild_lifecycle(&fixture.cache)
    }

    #[test]
    fn page_placement_clips_partially_visible_pages_to_subcell_pixels() {
        // Page top scrolled 25 px above the viewport; font cell height is 20 px.
        assert_eq!(
            page_placement(-25, 300, 200, 20),
            Some(PagePlacement {
                row: 0,
                y_offset: 0,
                src_y: 25,
                src_h: 200,
            })
        );
        // Following page begins at y = 283 px (row 14, 3 px inside cell 14).
        assert_eq!(
            page_placement(283, 300, 400, 20),
            Some(PagePlacement {
                row: 14,
                y_offset: 3,
                src_y: 0,
                src_h: 117,
            })
        );
        assert_eq!(page_placement(-300, 300, 200, 20), None);
        assert_eq!(page_placement(200, 300, 200, 20), None);
    }

    #[test]
    fn continuous_layout_stacks_pages_with_gap_until_viewport_ends() {
        let slots = layout_continuous_pages(1, 50.0, 3, 250, 10, 120, |page| match page {
            1 => Some(100),
            2 => Some(120),
            _ => None,
        });
        assert_eq!(
            slots,
            vec![
                PageSlot {
                    page: 1,
                    top: -50,
                    height: 100,
                    rendered: true,
                },
                PageSlot {
                    page: 2,
                    top: 60,
                    height: 120,
                    rendered: true,
                },
                PageSlot {
                    page: 3,
                    top: 190,
                    height: 120,
                    rendered: false,
                },
            ]
        );
    }
}
