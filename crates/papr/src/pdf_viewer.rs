use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use anyhow::{Result, Context};
use image::DynamicImage;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
    Frame,
};
use ratatui_image::{
    picker::Picker,
    protocol::StatefulProtocol,
    StatefulImage,
    Resize,
};
use papr_core::app::App;

struct ScaledPageCache {
    path: PathBuf,
    page: usize,
    zoom: f64,
    pixel_w: u32,
    image: DynamicImage,
    scaled_h: u32,
}

struct PdfRenderCache {
    picker: Option<Picker>,
    scaled_page: Option<ScaledPageCache>,
    last_rendered: Option<(PathBuf, usize, f64, u32, u32, u32)>,
    last_protocol: Option<StatefulProtocol>,
    temp_files: Vec<PathBuf>,
}

static PDF_CACHE: OnceLock<Mutex<PdfRenderCache>> = OnceLock::new();

fn get_cache() -> &'static Mutex<PdfRenderCache> {
    PDF_CACHE.get_or_init(|| {
        Mutex::new(PdfRenderCache {
            picker: Picker::from_query_stdio().ok(),
            scaled_page: None,
            last_rendered: None,
            last_protocol: None,
            temp_files: Vec::new(),
        })
    })
}

/// Clean up any temporary PNG files created during PDF rendering.
pub fn cleanup_temp_files() {
    if let Some(cache_mutex) = PDF_CACHE.get() {
        if let Ok(mut cache) = cache_mutex.lock() {
            for path in cache.temp_files.drain(..) {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

fn render_pdf_page(pdf_path: &Path, page: usize, dpi: usize) -> Result<PathBuf> {
    let temp_dir = std::env::temp_dir();
    let output_prefix = temp_dir.join(format!("papr_pdf_page_{}_{}_{}", std::process::id(), page, dpi));
    let output_path = output_prefix.with_extension("png");

    if !output_path.exists() {
        let status = std::process::Command::new("pdftoppm")
            .arg("-png")
            .arg("-singlefile")
            .arg("-r")
            .arg(dpi.to_string())
            .arg("-f")
            .arg(page.to_string())
            .arg(pdf_path)
            .arg(&output_prefix)
            .status()
            .context("failed to execute pdftoppm")?;
        if !status.success() {
            anyhow::bail!("pdftoppm failed with status: {:?}", status);
        }
    }
    Ok(output_path)
}

pub fn draw_pdf_viewer(frame: &mut Frame<'_>, app: &mut App) {
    let area = frame.area();
    
    // Clear screen with black/dark background for the viewer
    frame.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );

    let pdf_path = match &app.pdf_viewer_path {
        Some(p) => p,
        None => {
            let msg = Paragraph::new("No PDF file open")
                .style(Style::default().fg(Color::Red));
            frame.render_widget(msg, area);
            return;
        }
    };

    let cache_mutex = get_cache();
    let mut cache = match cache_mutex.lock() {
        Ok(c) => c,
        Err(_) => return,
    };

    let picker = match cache.picker.clone() {
        Some(p) => p,
        None => {
            let msg = Paragraph::new("Terminal graphics protocol not detected. Please verify your terminal supports graphics (Kitty, Sixel, ITerm2, or halfblocks). Press Esc to exit.")
                .style(Style::default().fg(Color::Yellow))
                .alignment(ratatui::layout::Alignment::Center);
            frame.render_widget(msg, area);
            return;
        }
    };

    // Split layout: main PDF area and a status bar at the bottom
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    let pdf_area = chunks[0];
    let status_area = chunks[1];

    let (font_w, font_h) = picker.font_size();
    let pixel_w = (pdf_area.width as u32) * (font_w as u32);
    let pixel_h = (pdf_area.height as u32) * (font_h as u32);

    if pixel_w == 0 || pixel_h == 0 {
        return;
    }

    let page = app.pdf_viewer_page;
    let zoom = app.pdf_viewer_zoom;

    // Retrieve or generate scaled page
    let mut need_scale = true;
    if let Some(ref cached) = cache.scaled_page {
        if cached.path == *pdf_path && cached.page == page && cached.zoom == zoom && cached.pixel_w == pixel_w {
            need_scale = false;
        }
    }

    if need_scale {
        let dpi = (150.0 * zoom / 100.0) as usize;
        match render_pdf_page(pdf_path, page, dpi) {
            Ok(png_path) => {
                // Record for cleanup
                if !cache.temp_files.contains(&png_path) {
                    cache.temp_files.push(png_path.clone());
                }
                if let Ok(img) = image::open(&png_path) {
                    let img_w = img.width();
                    let img_h = img.height();
                    if img_w > 0 && img_h > 0 {
                        let scaled_h = (img_h as f64 * (pixel_w as f64 / img_w as f64)) as u32;
                        let resized_img = img.resize(pixel_w, scaled_h, image::imageops::FilterType::Triangle);
                        cache.scaled_page = Some(ScaledPageCache {
                            path: pdf_path.clone(),
                            page,
                            zoom,
                            pixel_w,
                            image: resized_img,
                            scaled_h,
                        });
                    }
                }
            }
            Err(e) => {
                let msg = Paragraph::new(format!("Failed to render page: {}", e))
                    .style(Style::default().fg(Color::Red));
                frame.render_widget(msg, pdf_area);
                return;
            }
        }
    }

    let scaled = match &cache.scaled_page {
        Some(s) => s,
        None => {
            let msg = Paragraph::new("Failed to process page image")
                .style(Style::default().fg(Color::Red));
            frame.render_widget(msg, pdf_area);
            return;
        }
    };

    let max_scroll_y_pixels = scaled.scaled_h.saturating_sub(pixel_h);
    let max_scroll_y_cells = max_scroll_y_pixels / (font_h as u32);
    if app.pdf_viewer_scroll_y > max_scroll_y_cells {
        app.pdf_viewer_scroll_y = max_scroll_y_cells;
    }
    let scroll_y_pixels = app.pdf_viewer_scroll_y * (font_h as u32);

    let cache_key = (pdf_path.clone(), page, zoom, scroll_y_pixels, pixel_w, pixel_h);
    let mut render_new_proto = true;
    if let Some(ref last_key) = cache.last_rendered {
        if *last_key == cache_key {
            render_new_proto = false;
        }
    }

    if render_new_proto {
        let cropped_img = scaled.image.crop_imm(
            0,
            scroll_y_pixels,
            pixel_w,
            pixel_h.min(scaled.scaled_h.saturating_sub(scroll_y_pixels)),
        );
        let proto = picker.new_resize_protocol(cropped_img);
        cache.last_protocol = Some(proto);
        cache.last_rendered = Some(cache_key);
    }

    if let Some(ref mut proto) = cache.last_protocol {
        let image_widget = StatefulImage::new().resize(Resize::Fit(None));
        frame.render_stateful_widget(image_widget, pdf_area, proto);
    }

    // Status bar content
    let filename = pdf_path.file_name().unwrap_or_default().to_string_lossy();
    let status_line = Line::from(vec![
        Span::styled(format!(" PDF: {} ", filename), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" | Page: {}/{} ", page, app.pdf_viewer_total_pages), Style::default().fg(Color::White)),
        Span::styled(format!(" | Zoom: {}% ", zoom as u32), Style::default().fg(Color::Green)),
        Span::styled(format!(" | Scroll: {}/{} ", app.pdf_viewer_scroll_y, max_scroll_y_cells), Style::default().fg(Color::White)),
        Span::styled(" | Esc: Exit | Up/Down: Scroll | PgUp/PgDn: Page | +/-: Zoom ", Style::default().fg(Color::DarkGray)),
    ]);
    let status_widget = Paragraph::new(status_line)
        .style(Style::default().bg(Color::Rgb(30, 30, 30)));
    frame.render_widget(status_widget, status_area);
}
