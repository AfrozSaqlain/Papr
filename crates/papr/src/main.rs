//! `papr` executable entry point.

mod terminal;
mod ui;
mod citation;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use anyhow::{Context, Result};
use toml;
use chrono::Local;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use papr_core::{
    App, AppMode, ArxivClient, CollectionDirectory, Command, Config, Database, DiscoveryStatus,
    DownloadEvent, DownloadManager, DownloadStatus, DownloadTask, ImportedPdf, LibraryIndexer,
    LibraryWatcher, MetadataPrompt, PaperNote, Paths, PluginHost, RemotePaper, Theme,
};
use tokio::sync::mpsc;

use terminal::TerminalSession;

#[derive(Debug, Parser)]
#[command(name = "papr", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Debug, Subcommand)]
enum CliCommand {
    /// Print resolved configuration and data paths.
    Paths,
    /// Scan configured library folders and update the catalog.
    Index,
    /// Generate completion definitions for a supported shell.
    Completions {
        /// Shell syntax to generate.
        shell: Shell,
    },
    /// List discovered plugins and validation diagnostics.
    Plugins,
    /// Invoke an enabled plugin event using the JSON protocol.
    Plugin {
        /// Enabled plugin identifier.
        id: String,
        /// Event or command name.
        event: String,
        /// Execution deadline in seconds.
        #[arg(long, default_value_t = 10)]
        timeout: u64,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(CliCommand::Completions { shell }) = &cli.command {
        generate(*shell, &mut Cli::command(), "papr", &mut std::io::stdout());
        return Ok(());
    }
    let paths = Paths::discover().context("failed to resolve papr directories")?;
    if matches!(&cli.command, Some(CliCommand::Paths)) {
        println!("config: {}", paths.config_file.display());
        println!("database: {}", paths.database_file.display());
        println!("downloads: {}", paths.downloads_dir.display());
        println!("plugins: {}", paths.plugins_dir.display());
        return Ok(());
    }

    let config = Config::load_or_create(&paths).context("failed to load configuration")?;
    let plugin_host = PluginHost::discover(&paths.plugins_dir, &config.enabled_plugins)
        .context("failed to discover plugins")?;
    if handle_plugin_cli(cli.command.as_ref(), &plugin_host).await? {
        return Ok(());
    }
    let theme = Theme::load(&config.theme).context("failed to load theme")?;
    let database = Database::open(&paths.database_file).context("failed to open database")?;
    if matches!(&cli.command, Some(CliCommand::Index)) {
        let download_dir = config.download_path.clone().unwrap_or(paths.downloads_dir);
        let mut roots = config.library_folders.clone();
        let download_inside = roots.iter().any(|root| {
            let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
            let dl_canon = std::fs::canonicalize(&download_dir).unwrap_or_else(|_| download_dir.clone());
            dl_canon.starts_with(&root_canon)
        });
        if !download_inside {
            roots.push(download_dir);
        }
        let pdfs = LibraryIndexer::scan(&roots);
        let mut imported = 0_usize;
        for pdf in &pdfs {
            imported += usize::from(database.import_pdf(pdf)?);
        }
        println!("indexed: {}, imported: {}", pdfs.len(), imported);
        return Ok(());
    }
    let download_dir = config.download_path.clone().unwrap_or_else(|| paths.downloads_dir.clone());
    std::fs::create_dir_all(&download_dir).context("failed to create download directory")?;

    let collection_roots = config.library_folders.clone();
    let mut library_roots = collection_roots.clone();
    let download_inside = collection_roots.iter().any(|root| {
        let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let dl_canon = std::fs::canonicalize(&download_dir).unwrap_or_else(|_| download_dir.clone());
        dl_canon.starts_with(&root_canon)
    });
    if !download_inside {
        library_roots.push(download_dir.clone());
    }

    let mut dashboard = database
        .research_dashboard()
        .context("failed to load research dashboard")?;
    dashboard.counts.papers = LibraryIndexer::count_pdfs(&collection_roots);
    dashboard.counts.downloaded = LibraryIndexer::count_pdfs(&[download_dir.clone()]);
    dashboard.read = database
        .library_papers_in_roots(&library_roots)?
        .into_iter()
        .filter(|p| p.reading_status == "read")
        .count() as u64;
    dashboard.disk_usage = LibraryIndexer::pdf_storage_size(&collection_roots);
    dashboard.downloads_size = LibraryIndexer::pdf_storage_size(&[download_dir.clone()]);
    dashboard.database_size = std::fs::metadata(&paths.database_file)
        .map(|m| m.len())
        .unwrap_or(0);

    let mut app = App {
        stats: dashboard.counts,
        dashboard,
        ..App::default()
    };
    app.plugins = plugin_host.plugins();
    app.plugin_diagnostics = plugin_host.diagnostics().len();
    app.config_editor_text = std::fs::read_to_string(&paths.config_file).unwrap_or_default();

    discover_local_downloads(&mut app, &download_dir, &database);

    app.library.papers = database
        .library_papers_in_roots(&library_roots)
        .context("failed to load library")?;
    refresh_organization(&database, &library_roots, &mut app)?;
    let (watch_sender, watch_receiver) = mpsc::unbounded_channel();
    let watcher = start_library_watcher(&library_roots, watch_sender.clone())?;

    let arxiv = ArxivClient::new().context("failed to initialize arXiv client")?;
    let downloads = DownloadManager::new().context("failed to initialize download manager")?;
    let mut session = TerminalSession::start(config.mouse)?;
    let primary_library_root = library_roots[0].clone();
    let dashboard_keywords = config.dashboard_keyword_list();
    let dashboard_keyword_signature = dashboard_keywords.join(",");
    let runtime = Runtime {
        arxiv,
        crossref: papr_core::api::crossref::CrossrefClient::new(),
        downloads,
        database,
        database_file: paths.database_file.clone(),
        config_file: paths.config_file.clone(),
        default_downloads_dir: paths.downloads_dir.clone(),
        download_dir,
        pdf_viewer: config.pdf_viewer.clone().unwrap_or_else(default_pdf_viewer),
        primary_library_root,
        library_roots,
        collection_roots,
        dashboard_keywords,
        dashboard_keyword_signature,
        dashboard_feed_date: local_feed_date(),
        watch_sender,
        watch_receiver,
        _watcher: watcher,
    };
    run(&mut session, &mut app, theme, runtime).await
}

async fn handle_plugin_cli(command: Option<&CliCommand>, plugin_host: &PluginHost) -> Result<bool> {
    if matches!(command, Some(CliCommand::Plugins)) {
        for plugin in plugin_host.plugins() {
            println!(
                "{}\t{}\t{}\t{}",
                plugin.id,
                plugin.version,
                if plugin.enabled {
                    "enabled"
                } else {
                    "disabled"
                },
                plugin.name
            );
        }
        for diagnostic in plugin_host.diagnostics() {
            eprintln!(
                "invalid\t{}\t{}",
                diagnostic.path.display(),
                diagnostic.message
            );
        }
        return Ok(true);
    }
    if let Some(CliCommand::Plugin { id, event, timeout }) = command {
        let response = plugin_host
            .invoke(
                id,
                &papr_core::PluginRequest::new(event, serde_json::json!({})),
                std::time::Duration::from_secs(*timeout),
            )
            .await
            .context("plugin invocation failed")?;
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(true);
    }
    Ok(false)
}

#[derive(Debug)]
enum UiAction {
    Search(String),
    OpenPaper(RemotePaper),
    OpenBrowser(String),
    Download(RemotePaper),
    Reindex,
    OpenPdf { paper_id: i64, path: PathBuf },
    OpenNote(PaperTarget),
    SaveNote(PaperNote),
    Prompt(PaperTarget),
    RenameCollection(i64),
    CreateCollection,
    SubmitPrompt(MetadataPrompt),
    Bookmark(PaperTarget),
    OpenCollection(i64),
    OpenAuthor(i64),
    OpenDownload(String),
    RenamePdf(i64),
    CopyCitation(PaperTarget),
    ConfirmDeletePaper { paper_id: i64, title: String, path: Option<PathBuf> },
    ConfirmDeleteCollection { collection_id: i64, name: String, path: Option<PathBuf> },
    DeletePaper { paper_id: i64, path: Option<PathBuf> },
    DeleteCollection { collection_id: i64, path: Option<PathBuf> },
}

enum KeyHandling {
    Ignored,
    Handled(Option<Box<UiAction>>),
}

#[derive(Debug)]
enum PaperTarget {
    Local(i64),
    Remote(Box<RemotePaper>),
}

#[derive(Debug)]
struct SearchResponse {
    query: String,
    result: Result<Vec<RemotePaper>, String>,
}

#[derive(Debug)]
struct TodayResponse {
    feed_date: String,
    result: Result<Vec<RemotePaper>, String>,
}

pub(crate) struct ConfigEditorView {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

struct Runtime {
    arxiv: ArxivClient,
    crossref: papr_core::api::crossref::CrossrefClient,
    downloads: DownloadManager,
    database: Database,
    database_file: PathBuf,
    config_file: PathBuf,
    default_downloads_dir: PathBuf,
    download_dir: PathBuf,
    pdf_viewer: String,
    primary_library_root: PathBuf,
    library_roots: Vec<PathBuf>,
    collection_roots: Vec<PathBuf>,
    dashboard_keywords: Vec<String>,
    dashboard_keyword_signature: String,
    dashboard_feed_date: String,
    watch_sender: mpsc::UnboundedSender<()>,
    watch_receiver: mpsc::UnboundedReceiver<()>,
    _watcher: LibraryWatcher,
}

fn config_editor_wrap_rows(char_len: usize, wrap_width: usize) -> usize {
    if char_len == 0 {
        1
    } else {
        char_len.div_ceil(wrap_width.max(1))
    }
}

fn config_editor_line_start(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[..cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0)
}

fn config_editor_line_end(text: &str, cursor: usize) -> usize {
    let cursor = cursor.min(text.len());
    text[cursor..]
        .find('\n')
        .map(|idx| cursor + idx)
        .unwrap_or(text.len())
}

fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut prev = cursor - 1;
    while prev > 0 && !text.is_char_boundary(prev) {
        prev -= 1;
    }
    prev
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    let mut next = cursor + 1;
    while next < text.len() && !text.is_char_boundary(next) {
        next += 1;
    }
    next.min(text.len())
}

fn byte_index_for_char_column(text: &str, char_col: usize) -> usize {
    if char_col == 0 {
        return 0;
    }
    text.char_indices()
        .nth(char_col)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn cursor_visual_position(text: &str, cursor: usize, wrap_width: usize) -> (usize, usize) {
    let wrap_width = wrap_width.max(1);
    let cursor = cursor.min(text.len());
    let before = &text[..cursor];
    let line_idx = before.chars().filter(|&c| c == '\n').count();
    let line_start = config_editor_line_start(text, cursor);
    let line_end = config_editor_line_end(text, cursor);
    let line = &text[line_start..line_end];
    let line_col = text[line_start..cursor].chars().count();
    let line_len = line.chars().count();

    let rows_before = text
        .split('\n')
        .take(line_idx)
        .map(|segment| config_editor_wrap_rows(segment.chars().count(), wrap_width))
        .sum::<usize>();

    if line_col == line_len && line_len > 0 && line_len % wrap_width == 0 {
        (rows_before + (line_col / wrap_width).saturating_sub(1), wrap_width - 1)
    } else {
        (rows_before + (line_col / wrap_width), line_col % wrap_width)
    }
}

fn cursor_from_visual_position(
    text: &str,
    target_row: usize,
    target_col: usize,
    wrap_width: usize,
) -> usize {
    let wrap_width = wrap_width.max(1);
    let mut row_base = 0_usize;
    let mut line_start = 0_usize;

    for line in text.split('\n') {
        let line_len = line.chars().count();
        let row_count = config_editor_wrap_rows(line_len, wrap_width);
        if target_row < row_base + row_count {
            let local_row = target_row - row_base;
            let target_char_col = (local_row * wrap_width + target_col).min(line_len);
            return line_start + byte_index_for_char_column(line, target_char_col);
        }
        row_base += row_count;
        line_start += line.len() + 1;
    }

    text.len()
}

fn move_config_editor_vertical(app: &mut App, row_delta: isize) {
    let wrap_width = app.config_editor_wrap_width.max(1);
    let (row, col) = cursor_visual_position(&app.config_editor_text, app.config_editor_cursor, wrap_width);
    let goal_col = app.config_editor_goal_column.unwrap_or(col);
    let total_rows = app
        .config_editor_text
        .split('\n')
        .map(|line| config_editor_wrap_rows(line.chars().count(), wrap_width))
        .sum::<usize>()
        .max(1);
    let target_row = row
        .saturating_add_signed(row_delta)
        .min(total_rows.saturating_sub(1));
    app.config_editor_cursor = cursor_from_visual_position(
        &app.config_editor_text,
        target_row,
        goal_col,
        wrap_width,
    );
    app.config_editor_goal_column = Some(goal_col);
}

fn reset_config_editor_goal_column(app: &mut App) {
    app.config_editor_goal_column = None;
}

pub(crate) fn build_config_editor_view(
    text: &str,
    cursor: usize,
    wrap_width: usize,
    viewport_height: usize,
    scroll: &mut usize,
) -> ConfigEditorView {
    let wrap_width = wrap_width.max(1);
    let (cursor_row, cursor_col) = cursor_visual_position(text, cursor, wrap_width);

    if cursor_row < *scroll {
        *scroll = cursor_row;
    } else if viewport_height > 0 && cursor_row >= *scroll + viewport_height {
        *scroll = cursor_row - viewport_height + 1;
    }

    let mut lines = Vec::new();
    for (line_idx, line) in text.split('\n').enumerate() {
        let chars: Vec<char> = line.chars().collect();
        let row_count = config_editor_wrap_rows(chars.len(), wrap_width);
        for row in 0..row_count {
            let prefix = if row == 0 {
                format!("{:3} ", line_idx + 1)
            } else {
                "    ".to_owned()
            };
            let start = row * wrap_width;
            let end = (start + wrap_width).min(chars.len());
            let segment = chars[start..end].iter().collect::<String>();
            lines.push(format!("{prefix}{segment}"));
        }
    }

    ConfigEditorView {
        lines,
        cursor_row,
        cursor_col,
    }
}

struct ActionSenders {
    search: mpsc::UnboundedSender<SearchResponse>,
    index: mpsc::UnboundedSender<IndexResponse>,
    download: mpsc::UnboundedSender<DownloadEvent>,
    today: mpsc::UnboundedSender<TodayResponse>,
    app_events: mpsc::UnboundedSender<AppEvent>,
    enrichment: mpsc::UnboundedSender<MetadataEnrichment>,
}

#[derive(Debug)]
struct MetadataEnrichment {
    paper_id: i64,
    paper: RemotePaper,
}

#[derive(Debug)]
enum AppEvent {
    ReadingSessionCompleted { session_id: i64, duration_s: u64 },
    Toast(String),
}

#[derive(Debug)]
enum IndexResponse {
    Scan {
        pdfs: Vec<ImportedPdf>,
        directories: Vec<CollectionDirectory>,
    },
    File(Result<ImportedPdf, String>),
}

async fn run(
    session: &mut TerminalSession,
    app: &mut App,
    mut theme: Theme,
    mut runtime: Runtime,
) -> Result<()> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<SearchResponse>();
    let (index_sender, mut index_receiver) = mpsc::unbounded_channel::<IndexResponse>();
    let (download_sender, mut download_receiver) = mpsc::unbounded_channel::<DownloadEvent>();
    let (today_sender, mut today_receiver) = mpsc::unbounded_channel::<TodayResponse>();
    let (app_events_sender, mut app_events_receiver) = mpsc::unbounded_channel::<AppEvent>();
    let (enrichment_sender, mut enrichment_receiver) =
        mpsc::unbounded_channel::<MetadataEnrichment>();
    let senders = ActionSenders {
        search: sender,
        index: index_sender,
        download: download_sender,
        today: today_sender,
        app_events: app_events_sender,
        enrichment: enrichment_sender,
    };
    let mut pending_downloads = HashMap::<String, RemotePaper>::new();
    refresh_dashboard_papers(&runtime, &senders, app)?;
    start_runtime_scan(&runtime, &senders, app);
    let mut last_date_check = std::time::Instant::now();
    while !app.should_quit {
        while let Ok(TodayResponse { feed_date, result }) = today_receiver.try_recv() {
            if feed_date != runtime.dashboard_feed_date {
                continue;
            }
            match result {
                Ok(papers) => {
                    runtime.database.save_dashboard_feed_cache(
                        &feed_date,
                        &runtime.dashboard_keyword_signature,
                        &papers,
                    )?;
                    app.today_papers = papers;
                    app.today_selected = app
                        .today_selected
                        .min(app.today_papers.len().saturating_sub(1));
                    app.today_status = DiscoveryStatus::Ready;
                }
                Err(error) => app.today_status = DiscoveryStatus::Error(error),
            }
        }
        if last_date_check.elapsed() >= std::time::Duration::from_secs(1) {
            last_date_check = std::time::Instant::now();
            let current_date = local_feed_date();
            if current_date != runtime.dashboard_feed_date {
                runtime.dashboard_feed_date = current_date;
                refresh_dashboard_papers(&runtime, &senders, app)?;
            }
        }
        while let Ok(response) = receiver.try_recv() {
            if response.query == app.discovery.query {
                match response.result {
                    Ok(papers) => {
                        app.discovery.results = papers;
                        app.discovery.selected = 0;
                        app.discovery.status = DiscoveryStatus::Ready;
                    }
                    Err(error) => app.discovery.status = DiscoveryStatus::Error(error),
                }
            }
        }
        while runtime.watch_receiver.try_recv().is_ok() {
            start_runtime_scan(&runtime, &senders, app);
        }
        while let Ok(response) = index_receiver.try_recv() {
            apply_index_response(response, &mut runtime, &senders, app)?;
        }
        let mut enriched_any = false;
        while let Ok(MetadataEnrichment { paper_id, paper }) = enrichment_receiver.try_recv() {
            runtime.database.apply_arxiv_metadata(paper_id, &paper)?;
            enriched_any = true;
        }
        if enriched_any {
            refresh_library(&runtime, app)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(&mut runtime, app)?;
        }
        if enrichment_receiver.is_empty() && app.enrichment_pending {
            app.enrichment_pending = false;
        }
        while let Ok(event) = download_receiver.try_recv() {
            apply_download_event(
                event,
                &mut pending_downloads,
                &mut runtime,
                app,
                &senders.index,
            )?;
        }
        while let Ok(event) = app_events_receiver.try_recv() {
            match event {
                AppEvent::ReadingSessionCompleted {
                    session_id,
                    duration_s,
                } => {
                    runtime
                        .database
                        .record_reading_duration(session_id, duration_s)?;
                    refresh_dashboard(&mut runtime, app)?;
                }
                AppEvent::Toast(message) => {
                    app.toast = Some(message);
                }
            }
        }
        session
            .terminal_mut()
            .draw(|frame| ui::render(frame, app, &theme))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.page == papr_core::Page::Settings && app.config_editor_focused {
                        handle_config_editor_key(app, key, &mut runtime, &mut theme);
                    } else if let Some(action) = handle_key(app, key) {
                        apply_ui_action(
                            action,
                            &mut runtime,
                            &senders,
                            &mut pending_downloads,
                            app,
                            &mut theme,
                        )?;
                    }
                }
                _ => {}
            }
        }
        tokio::task::yield_now().await;
    }
    Ok(())
}

fn apply_ui_action(
    action: UiAction,
    runtime: &mut Runtime,
    senders: &ActionSenders,
    pending_downloads: &mut HashMap<String, RemotePaper>,
    app: &mut App,
    _theme: &mut Theme,
) -> Result<()> {
    match action {
        UiAction::Search(query) => {
            runtime
                .database
                .record_activity("search", None, Some(&query))?;
            app.discovery.status = DiscoveryStatus::Loading;
            let client = runtime.arxiv.clone();
            let response_sender = senders.search.clone();
            tokio::spawn(async move {
                let result = client
                    .search(&query, 50)
                    .await
                    .map_err(|error| error.to_string());
                let _ = response_sender.send(SearchResponse { query, result });
            });
        }
        UiAction::OpenPaper(paper) => {
            let paper_id = runtime.database.ensure_remote_paper(&paper)?;
            runtime.database.record_open(paper_id, false)?;
            if app.page == papr_core::Page::Dashboard {
                app.discovery.results.clone_from(&app.today_papers);
                app.discovery.selected = app
                    .today_selected
                    .min(app.discovery.results.len().saturating_sub(1));
            }
            app.mode = AppMode::PaperDetail;
            app.discovery.detail_scroll = 0;
            refresh_library(runtime, app)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(runtime, app)?;
        }
        UiAction::OpenBrowser(url) => open_browser(&url, app),
        UiAction::Download(paper) => start_download(
            paper,
            &runtime.download_dir,
            &runtime.downloads,
            &senders.download,
            pending_downloads,
            app,
        ),
        UiAction::Reindex => start_runtime_scan(runtime, senders, app),
        UiAction::OpenPdf { paper_id, path } => {
            let session_id = runtime.database.record_open(paper_id, true)?;
            open_pdf(
                &runtime.pdf_viewer,
                &path,
                app,
                Some(session_id),
                Some(senders.app_events.clone()),
            )?;
            refresh_library(runtime, app)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(runtime, app)?;
        }
        UiAction::OpenNote(target) => {
            let paper_id = resolve_target(target, &mut runtime.database)?;
            runtime
                .database
                .record_activity("note_opened", Some(paper_id), None)?;
            app.note_editor = Some(runtime.database.paper_note(paper_id)?);
            app.note_preview = false;
            app.mode = AppMode::NoteEdit;
        }
        UiAction::SaveNote(note) => {
            runtime.database.save_note(&note)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
        }
        UiAction::Prompt(target) => {
            let paper_id = resolve_target(target, &mut runtime.database)?;
            let current = runtime.database.paper_collection_name(paper_id)?;
            show_collection_prompt(app, Some(paper_id), None, current);
        }
        UiAction::RenameCollection(id) => {
            app.metadata_prompt = Some(MetadataPrompt {
                paper_id: None,
                rename_collection_id: Some(id),
                rename_paper_id: None,
                value: String::new(),
                cursor: 0,
                selected: 0,
                current_collection: None,
            });
            app.mode = AppMode::Prompt;
        }
        UiAction::RenamePdf(id) => {
            app.metadata_prompt = Some(MetadataPrompt {
                paper_id: None,
                rename_collection_id: None,
                rename_paper_id: Some(id),
                value: String::new(),
                cursor: 0,
                selected: 0,
                current_collection: None,
            });
            app.mode = AppMode::Prompt;
        }
        UiAction::CreateCollection => {
            show_collection_prompt(app, None, None, None);
        }
        UiAction::SubmitPrompt(prompt) => {
            apply_collection_prompt(runtime, app, &prompt)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(runtime, app)?;
            app.toast = Some(format!("Saved {}", prompt.value));
        }
        UiAction::Bookmark(target) => {
            let paper_id = resolve_target(target, &mut runtime.database)?;
            let active = runtime.database.toggle_bookmark(paper_id)?;
            runtime.database.record_activity(
                "bookmarked",
                Some(paper_id),
                Some(if active { "added" } else { "removed" }),
            )?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(runtime, app)?;
            app.toast = Some(if active {
                "Paper bookmarked".into()
            } else {
                "Bookmark removed".into()
            });
        }
        UiAction::OpenCollection(collection_id) => {
            open_collection(&runtime.database, app, collection_id)?;
        }
        UiAction::OpenAuthor(author_id) => {
            open_author(&runtime.database, &runtime.library_roots, app, author_id)?;
        }
        UiAction::OpenDownload(id) => {
            let task = app.downloads.iter().find(|t| t.id == id);
            let mut path = None;
            if let Some(task) = task {
                if let Some(paper_id) = task.paper_id {
                    if let Some(paper) = app.library.papers.iter().find(|p| p.id == paper_id) {
                        if let Some(pdf_path) = &paper.pdf_path {
                            path = Some(PathBuf::from(pdf_path));
                        }
                    }
                }
                if path.is_none() {
                    if let Some(pdf_path) = &task.pdf_path {
                        path = Some(PathBuf::from(pdf_path));
                    }
                }
            }
            let path = path.unwrap_or_else(|| runtime.download_dir.join(format!("{id}.pdf")));
            open_pdf(&runtime.pdf_viewer, &path, app, None, None)?;
        }
        UiAction::CopyCitation(target) => {
            let metadata = match target {
                PaperTarget::Local(id) => runtime.database.paper_citation_metadata(id)?,
                PaperTarget::Remote(paper) => Some(papr_core::models::CitationMetadata {
                    title: paper.title.clone(),
                    authors: paper.authors.join(" and "),
                    doi: paper.doi.clone(),
                    arxiv_id: Some(paper.id.clone()),
                    year: Some(paper.published.format("%Y").to_string()),
                    journal_ref: paper.journal_ref.clone(),
                }),
            };
            if let Some(metadata) = metadata {
                app.toast = Some("Fetching citation...".into());
                tokio::spawn(citation::fetch_and_copy_citation(
                    metadata,
                    senders.app_events.clone(),
                ));
            } else {
                app.toast = Some("Citation metadata not available".into());
            }
        }
        UiAction::ConfirmDeletePaper { paper_id, title, path } => {
            app.delete_confirmation = Some(papr_core::DeletionTarget::Paper {
                id: paper_id,
                title,
                path,
            });
            app.mode = AppMode::ConfirmDelete;
        }
        UiAction::ConfirmDeleteCollection { collection_id, name, path } => {
            app.delete_confirmation = Some(papr_core::DeletionTarget::Collection {
                id: collection_id,
                name,
                path,
            });
            app.mode = AppMode::ConfirmDelete;
        }
        UiAction::DeletePaper { paper_id, path } => {
            if let Some(p) = &path {
                if p.exists() {
                    let _ = std::fs::remove_file(p);
                }
            }
            runtime.database.delete_paper(paper_id)?;
            refresh_library(runtime, app)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(runtime, app)?;
            refresh_downloads(runtime, app);
            app.toast = Some("PDF permanently deleted".into());
        }
        UiAction::DeleteCollection { collection_id, path } => {
            let papers = runtime.database.papers_for_collection(collection_id)?;
            for paper in papers {
                if let Some(ref path_str) = paper.pdf_path {
                    let p = PathBuf::from(path_str);
                    if p.exists() {
                        let _ = std::fs::remove_file(p);
                    }
                }
                runtime.database.delete_paper(paper.id)?;
            }
            if let Some(p) = &path {
                if p.exists() {
                    let _ = std::fs::remove_dir_all(p);
                }
            }
            runtime.database.delete_collection(collection_id)?;
            if app.active_collection.as_ref().map(|c| c.id) == Some(collection_id) {
                app.active_collection = None;
                app.collection_papers.clear();
            }
            refresh_library(runtime, app)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(runtime, app)?;
            refresh_downloads(runtime, app);
            app.toast = Some("Collection permanently deleted".into());
        }
    }
    Ok(())
}

fn show_collection_prompt(
    app: &mut App,
    paper_id: Option<i64>,
    rename_id: Option<i64>,
    current_collection: Option<String>,
) {
    app.metadata_prompt = Some(MetadataPrompt {
        paper_id,
        rename_collection_id: rename_id,
        rename_paper_id: None,
        value: String::new(),
        cursor: 0,
        selected: 0,
        current_collection,
    });
    app.mode = AppMode::Prompt;
}

fn apply_collection_prompt(
    runtime: &mut Runtime,
    app: &mut App,
    prompt: &MetadataPrompt,
) -> Result<()> {
    let name = prompt.value.trim();
    if name.is_empty() {
        return Ok(());
    }

    if let Some(paper_id) = prompt.rename_paper_id {
        if name.contains(['/', '\\']) {
            anyhow::bail!("filename must not contain path separators");
        }
        let mut new_name = name.to_string();
        if !new_name.to_lowercase().ends_with(".pdf") {
            new_name.push_str(".pdf");
        }

        let paper = app
            .library
            .papers
            .iter()
            .find(|p| p.id == paper_id)
            .context("paper not found")?;
        let source = PathBuf::from(paper.pdf_path.as_ref().context("paper has no local PDF")?);
        let destination = source.with_file_name(&new_name);

        if source != destination {
            if destination.exists() {
                anyhow::bail!("a file with this name already exists");
            }
            move_pdf_file(&source, &destination)?;
            runtime.database.rename_pdf(paper_id, &destination)?;
            refresh_library(runtime, app)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(runtime, app)?;
            refresh_downloads(runtime, app);
        }
        return Ok(());
    }

    validate_collection_name(name)?;
    if let Some(collection_id) = prompt.rename_collection_id {
        let collection = app
            .collections
            .iter()
            .find(|item| item.id == collection_id)
            .context("collection no longer exists")?;
        let old = collection.folder_path.as_ref().map_or_else(
            || runtime.primary_library_root.join(&collection.name),
            PathBuf::from,
        );
        let new = old
            .parent()
            .unwrap_or(&runtime.primary_library_root)
            .join(name);
        std::fs::rename(&old, &new).context("failed to rename collection directory")?;
        if let Err(error) = runtime
            .database
            .rename_collection(collection_id, name, &old, &new)
        {
            let _ = std::fs::rename(&new, &old);
            return Err(error.into());
        }
        let directories = LibraryIndexer::collection_directories(&runtime.collection_roots);
        for directory in &directories {
            runtime.database.sync_collection_directory(directory)?;
        }
        runtime
            .database
            .reconcile_collections(&runtime.collection_roots, &directories)?;
        refresh_renamed_collection(&runtime.database, &runtime.library_roots, app, collection_id)?;
        return Ok(());
    }
    if prompt.paper_id.is_none() {
        if app
            .collections
            .iter()
            .any(|collection| collection.name.eq_ignore_ascii_case(name))
        {
            anyhow::bail!("a collection with this name already exists");
        }
        let folder = runtime.primary_library_root.join(name);
        std::fs::create_dir(&folder).context("failed to create collection directory")?;
        if let Err(error) = runtime.database.create_collection(name, &folder) {
            let _ = std::fs::remove_dir(&folder);
            return Err(error.into());
        }
        return Ok(());
    }
    let paper_id = prompt
        .paper_id
        .context("collection assignment has no paper")?;
    let paper = app
        .library
        .papers
        .iter()
        .find(|paper| paper.id == paper_id)
        .context("paper must have a local PDF before collection assignment")?;
    let source = PathBuf::from(paper.pdf_path.as_ref().context("paper has no local PDF")?);
    let existing = app
        .collections
        .iter()
        .find(|item| item.name.eq_ignore_ascii_case(name));
    let (collection_id, folder) = if let Some(collection) = existing {
        let folder = collection.folder_path.as_ref().map_or_else(
            || runtime.primary_library_root.join(&collection.name),
            PathBuf::from,
        );
        std::fs::create_dir_all(&folder)?;
        runtime
            .database
            .set_collection_folder(collection.id, &folder)?;
        (collection.id, folder)
    } else {
        let folder = runtime.primary_library_root.join(name);
        std::fs::create_dir_all(&folder)?;
        (runtime.database.create_collection(name, &folder)?, folder)
    };
    let destination = folder.join(source.file_name().context("PDF path has no filename")?);
    if source != destination {
        if destination.exists() {
            anyhow::bail!("a PDF with this filename already exists in the collection");
        }
        move_pdf_file(&source, &destination)?;
    }
    if let Err(error) = runtime
        .database
        .assign_moved_pdf(paper_id, collection_id, &destination)
    {
        if source != destination {
            let _ = move_pdf_file(&destination, &source);
        }
        return Err(error.into());
    }
    refresh_library(runtime, app)?;
    refresh_organization(&runtime.database, &runtime.library_roots, app)?;
    refresh_dashboard(runtime, app)?;
    refresh_downloads(runtime, app);
    Ok(())
}

fn refresh_renamed_collection(
    database: &Database,
    library_roots: &[PathBuf],
    app: &mut App,
    collection_id: i64,
) -> Result<()> {
    refresh_organization(database, library_roots, app)?;
    app.collection_selected = app
        .collections
        .iter()
        .position(|collection| collection.id == collection_id)
        .unwrap_or_else(|| app.collections.len().saturating_sub(1));
    Ok(())
}

fn move_pdf_file(source: &Path, destination: &Path) -> Result<()> {
    if let Err(rename_error) = std::fs::rename(source, destination) {
        std::fs::copy(source, destination).with_context(|| {
            format!(
                "failed to move PDF from {} to {}: {rename_error}",
                source.display(),
                destination.display()
            )
        })?;
        if let Err(remove_error) = std::fs::remove_file(source) {
            let _ = std::fs::remove_file(destination);
            return Err(remove_error.into());
        }
    }
    Ok(())
}

fn validate_collection_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        anyhow::bail!("collection name must be one safe directory name");
    }
    Ok(())
}

fn open_collection(database: &Database, app: &mut App, collection_id: i64) -> Result<()> {
    let restore_selection = app.last_opened_collection_id == Some(collection_id);
    app.active_collection = app
        .collections
        .iter()
        .find(|collection| collection.id == collection_id)
        .cloned();
    app.collection_papers = database.papers_for_collection(collection_id)?;
    app.collection_paper_selected = if restore_selection {
        app.collection_paper_selected
            .min(app.collection_papers.len().saturating_sub(1))
    } else {
        0
    };
    app.last_opened_collection_id = Some(collection_id);
    Ok(())
}

fn open_author(database: &Database, library_roots: &[PathBuf], app: &mut App, author_id: i64) -> Result<()> {
    let restore_selection = app.last_opened_author_id == Some(author_id);
    app.active_author = app
        .authors
        .iter()
        .find(|author| author.id == author_id)
        .cloned();
    app.author_papers = database.author_papers(author_id, library_roots)?;
    app.author_paper_selected = if restore_selection {
        app.author_paper_selected
            .min(app.author_papers.len().saturating_sub(1))
    } else {
        0
    };
    app.last_opened_author_id = Some(author_id);
    Ok(())
}

fn resolve_target(target: PaperTarget, database: &mut Database) -> Result<i64> {
    match target {
        PaperTarget::Local(id) => Ok(id),
        PaperTarget::Remote(paper) => database.ensure_remote_paper(&paper).map_err(Into::into),
    }
}

fn refresh_organization(database: &Database, library_roots: &[PathBuf], app: &mut App) -> Result<()> {
    app.collections = database.collections()?;
    app.collection_papers_map = database.collection_papers_map().unwrap_or_default();
    app.collection_selected = app
        .collection_selected
        .min(app.collections.len().saturating_sub(1));
    if let Some(active_id) = app.active_collection.as_ref().map(|c| c.id) {
        app.active_collection = app
            .collections
            .iter()
            .find(|collection| collection.id == active_id)
            .cloned();
        app.collection_papers = database.papers_for_collection(active_id)?;
        app.collection_paper_selected = app
            .collection_paper_selected
            .min(app.collection_papers.len().saturating_sub(1));
    }
    app.bookmarks = database.bookmarks(library_roots)?;
    app.bookmark_selected = app
        .bookmark_selected
        .min(app.bookmarks.len().saturating_sub(1));
    app.authors = database.authors(library_roots)?;
    app.author_selected = app.author_selected.min(app.authors.len().saturating_sub(1));
    if let Some(active_id) = app.active_author.as_ref().map(|a| a.id) {
        app.active_author = app.authors.iter().find(|a| a.id == active_id).cloned();
        if app.active_author.is_some() {
            app.author_papers = database.author_papers(active_id, library_roots)?;
            app.author_paper_selected = app
                .author_paper_selected
                .min(app.author_papers.len().saturating_sub(1));
        } else {
            app.author_papers.clear();
            app.author_paper_selected = 0;
        }
    }
    app.notes_papers = database.papers_with_notes(library_roots)?;
    app.notes_selected = app
        .notes_selected
        .min(app.notes_papers.len().saturating_sub(1));
    Ok(())
}

fn refresh_dashboard_papers(
    runtime: &Runtime,
    senders: &ActionSenders,
    app: &mut App,
) -> Result<()> {
    if let Some(papers) = runtime.database.dashboard_feed_cache(
        &runtime.dashboard_feed_date,
        &runtime.dashboard_keyword_signature,
    )? {
        app.today_papers = papers;
        app.today_selected = app
            .today_selected
            .min(app.today_papers.len().saturating_sub(1));
        app.today_status = DiscoveryStatus::Ready;
        return Ok(());
    }
    app.today_status = DiscoveryStatus::Loading;
    let client = runtime.arxiv.clone();
    let keywords = runtime.dashboard_keywords.clone();
    let feed_date = runtime.dashboard_feed_date.clone();
    let sender = senders.today.clone();
    tokio::spawn(async move {
        let result = dashboard_papers(client, keywords)
            .await
            .map_err(|error| error.to_string());
        let _ = sender.send(TodayResponse { feed_date, result });
    });
    Ok(())
}

fn local_feed_date() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

async fn dashboard_papers(client: ArxivClient, keywords: Vec<String>) -> Result<Vec<RemotePaper>> {
    if keywords.is_empty() {
        return client.latest(10).await.map_err(Into::into);
    }

    let mut buckets = Vec::new();
    let mut last_error = None;
    for keyword in keywords {
        match client.search_latest(&keyword, 20).await {
            Ok(papers) => buckets.push(papers),
            Err(error) => last_error = Some(error),
        }
    }
    if buckets.is_empty() {
        if let Some(error) = last_error {
            return Err(error.into());
        }
        return Ok(Vec::new());
    }
    Ok(diverse_latest_papers(buckets, 10))
}

fn diverse_latest_papers(mut buckets: Vec<Vec<RemotePaper>>, limit: usize) -> Vec<RemotePaper> {
    let mut selected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut index = 0_usize;
    while selected.len() < limit {
        let mut added_this_round = false;
        for bucket in &mut buckets {
            while let Some(paper) = bucket.get(index) {
                if seen.insert(paper.id.clone()) {
                    selected.push(paper.clone());
                    added_this_round = true;
                    break;
                }
                bucket.remove(index);
            }
            if selected.len() == limit {
                break;
            }
        }
        if !added_this_round {
            break;
        }
        index = index.saturating_add(1);
    }
    selected
}

fn refresh_library(runtime: &Runtime, app: &mut App) -> Result<()> {
    app.library.papers = runtime
        .database
        .library_papers_in_roots(&runtime.library_roots)?;
    if app.library.selected >= app.library.papers.len() {
        app.library.selected = app.library.papers.len().saturating_sub(1);
    }
    Ok(())
}

fn refresh_dashboard(runtime: &Runtime, app: &mut App) -> Result<()> {
    app.dashboard = runtime.database.research_dashboard()?;
    app.dashboard.counts.papers = LibraryIndexer::count_pdfs(&runtime.collection_roots);
    app.dashboard.counts.downloaded = LibraryIndexer::count_pdfs(&[runtime.download_dir.clone()]);
    app.dashboard.read = runtime
        .database
        .library_papers_in_roots(&runtime.library_roots)?
        .into_iter()
        .filter(|p| p.reading_status == "read")
        .count() as u64;
    app.dashboard.disk_usage = LibraryIndexer::pdf_storage_size(&runtime.collection_roots);
    app.dashboard.downloads_size =
        LibraryIndexer::pdf_storage_size(&[runtime.download_dir.clone()]);
    app.dashboard.database_size = std::fs::metadata(&runtime.database_file)
        .map(|m| m.len())
        .unwrap_or(0);
    app.stats = app.dashboard.counts;
    Ok(())
}

fn refresh_downloads_from_dir(
    app: &mut App,
    download_dir: &Path,
    database: &Database,
) {
    let previous_selected_path = app
        .filtered_downloads()
        .get(app.download_selected)
        .and_then(|task| task.pdf_path.clone());
    let previous_selected = app.download_selected;

    let mut transient_downloads = app
        .downloads
        .iter()
        .filter(|task| !matches!(task.status, DownloadStatus::Completed))
        .cloned()
        .collect::<Vec<_>>();

    app.downloads.clear();
    app.downloads.append(&mut transient_downloads);
    discover_local_downloads(app, download_dir, database);

    app.download_selected = previous_selected_path
        .as_ref()
        .and_then(|path| {
            app.filtered_downloads()
                .iter()
                .position(|task| task.pdf_path.as_deref() == Some(path.as_str()))
        })
        .unwrap_or_else(|| previous_selected.min(app.filtered_downloads().len().saturating_sub(1)));
}

fn refresh_downloads(runtime: &Runtime, app: &mut App) {
    refresh_downloads_from_dir(app, &runtime.download_dir, &runtime.database);
}

fn default_pdf_viewer() -> String {
    if cfg!(target_os = "macos") {
        "open".into()
    } else if cfg!(target_os = "windows") {
        "cmd /C start".into()
    } else {
        "xdg-open".into()
    }
}

fn open_pdf(
    viewer: &str,
    path: &Path,
    app: &mut App,
    session_id: Option<i64>,
    event_sender: Option<mpsc::UnboundedSender<AppEvent>>,
) -> Result<()> {
    if !path.exists() {
        app.toast = Some(format!("PDF not found: {}", path.display()));
        return Ok(());
    }

    let mut argv = parse_command(viewer)?;
    if argv.is_empty() {
        argv.push(default_pdf_viewer());
    }
    let has_placeholder = argv.iter().any(|arg| arg.contains("{path}"));
    if has_placeholder {
        let path_text = path.to_string_lossy();
        for arg in &mut argv {
            *arg = arg.replace("{path}", &path_text);
        }
    }

    let program = argv.remove(0);
    let mut command = tokio::process::Command::new(&program);
    command.args(argv);
    if !has_placeholder {
        command.arg(path);
    }

    command.stdout(Stdio::null());
    command.stderr(Stdio::null());

    match command.spawn() {
        Ok(mut child) => {
            app.toast = Some(format!("Opened {}", path.display()));
            if let (Some(session_id), Some(sender)) = (session_id, event_sender) {
                let start = std::time::Instant::now();
                tokio::spawn(async move {
                    let _ = child.wait().await;
                    let duration_s = start.elapsed().as_secs();
                    let _ = sender.send(AppEvent::ReadingSessionCompleted {
                        session_id,
                        duration_s,
                    });
                });
            }
        }
        Err(error) => app.toast = Some(format!("Could not open PDF with {program}: {error}")),
    }
    Ok(())
}

fn open_browser(url: &str, app: &mut App) {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = ProcessCommand::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = ProcessCommand::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = ProcessCommand::new("xdg-open");
        command.arg(url);
        command
    };
    command.stdout(Stdio::null()).stderr(Stdio::null());
    app.toast = Some(match command.spawn() {
        Ok(_) => "Opened paper in browser".into(),
        Err(error) => format!("Could not open browser: {error}"),
    });
}

fn parse_command(command: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars();
    let mut quote = None;
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    current.push(character);
                }
            }
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            character if character.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if let Some(character) = quote {
        anyhow::bail!("unterminated {character} quote in pdf_viewer");
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

fn start_scan(
    pdf_roots: &[PathBuf],
    collection_roots: &[PathBuf],
    sender: &mpsc::UnboundedSender<IndexResponse>,
    app: &mut App,
) {
    if app.library.indexing {
        return;
    }
    app.library.indexing = true;
    app.library.message = Some("Indexing library folders...".into());
    let pdf_roots = pdf_roots.to_vec();
    let collection_roots = collection_roots.to_vec();
    let sender = sender.clone();
    tokio::task::spawn_blocking(move || {
        let _ = sender.send(IndexResponse::Scan {
            pdfs: LibraryIndexer::scan(&pdf_roots),
            directories: LibraryIndexer::collection_directories(&collection_roots),
        });
    });
}

fn start_library_watcher(
    roots: &[PathBuf],
    watch_sender: mpsc::UnboundedSender<()>,
) -> Result<LibraryWatcher> {
    LibraryWatcher::start(roots, move || {
        let _ = watch_sender.send(());
    })
    .context("failed to watch library folders")
}

fn restart_runtime_watcher(runtime: &mut Runtime) -> Result<()> {
    runtime._watcher = start_library_watcher(&runtime.library_roots, runtime.watch_sender.clone())?;
    Ok(())
}

fn spawn_enrichment_if_needed(
    runtime: &mut Runtime,
    senders: &ActionSenders,
    app: &mut App,
) -> Result<()> {
    let papers = runtime.database.papers_needing_enrichment()?;
    if !papers.is_empty() {
        let arxiv_client = runtime.arxiv.clone();
        let crossref_client = runtime.crossref.clone();
        let enrichment_tx = senders.enrichment.clone();
        let count = papers.len();
        app.enrichment_pending = true;
        tokio::spawn(async move {
            let mut enriched = 0usize;
            for (i, (paper_id, mut candidate_arxiv, pdf_path)) in papers.into_iter().enumerate() {
                let mut candidate_doi = None;

                if candidate_arxiv.is_none() {
                    if let Some(path) = pdf_path {
                        if let Ok(output) = tokio::process::Command::new("pdftotext")
                            .args(["-l", "2", &path, "-"])
                            .output()
                            .await
                        {
                            if output.status.success() {
                                let text = String::from_utf8_lossy(&output.stdout);
                                let lower_text = text.to_lowercase();

                                if let Some(idx) = lower_text.find("arxiv:") {
                                    let substr = &text[idx + 6..];
                                    let end = substr
                                        .find(|c: char| !c.is_ascii_digit() && c != '.')
                                        .unwrap_or(substr.len());
                                    let id = &substr[..end];
                                    if id.len() >= 7 {
                                        candidate_arxiv = Some(id.to_string());
                                    }
                                } else if let Some(idx) = lower_text.find("10.") {
                                    let substr = &text[idx..];
                                    let end = substr
                                        .find(|c: char| c.is_whitespace() || c == '\n')
                                        .unwrap_or(substr.len());
                                    let id = &substr[..end];
                                    if id.len() >= 5 {
                                        candidate_doi = Some(id.to_string());
                                    }
                                }
                            }
                        }
                    }
                }

                if let Some(arxiv_id) = candidate_arxiv {
                    if i > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    }
                    match arxiv_client.get_by_id(&arxiv_id).await {
                        Ok(Some(paper)) => {
                            enriched += 1;
                            let _ = enrichment_tx.send(MetadataEnrichment { paper_id, paper });
                        }
                        Ok(None) => {}
                        Err(e) => {
                            eprintln!("arXiv enrichment failed for {arxiv_id}: {e}");
                        }
                    }
                } else if let Some(doi) = candidate_doi {
                    if i > 0 {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Crossref rate limit is friendlier
                    }
                    match crossref_client.get_by_doi(&doi).await {
                        Ok(Some(paper)) => {
                            enriched += 1;
                            let _ = enrichment_tx.send(MetadataEnrichment { paper_id, paper });
                        }
                        Ok(None) => {}
                        Err(e) => {
                            eprintln!("Crossref enrichment failed for {doi}: {e}");
                        }
                    }
                }
            }
            if enriched > 0 {
                eprintln!("Enriched {enriched}/{count} papers with metadata");
            }
        });
    }
    Ok(())
}

fn start_runtime_scan(runtime: &Runtime, senders: &ActionSenders, app: &mut App) {
    start_scan(
        &runtime.library_roots,
        &runtime.collection_roots,
        &senders.index,
        app,
    );
}

fn apply_index_response(
    response: IndexResponse,
    runtime: &mut Runtime,
    senders: &ActionSenders,
    app: &mut App,
) -> Result<()> {
    match response {
        IndexResponse::Scan { pdfs, directories } => {
            let found = pdfs.len();
            let mut imported = 0_usize;
            for directory in &directories {
                runtime.database.sync_collection_directory(directory)?;
            }
            for pdf in &pdfs {
                imported += usize::from(runtime.database.import_pdf(pdf)?);
                if let Some(paper_id) = runtime.database.paper_id_for_pdf(pdf)? {
                    sync_pdf_collection_membership(
                        &runtime.database,
                        paper_id,
                        pdf,
                        &runtime.collection_roots,
                    )?;
                }
            }
            runtime
                .database
                .reconcile_collections(&runtime.collection_roots, &directories)?;
            app.library.indexing = false;
            app.library.message = Some(format!("Indexed {found} PDFs, imported {imported} new"));

            spawn_enrichment_if_needed(runtime, senders, app)?;
        }
        IndexResponse::File(Ok(pdf)) => {
            let imported = runtime.database.import_pdf(&pdf)?;
            if let Some(paper_id) = runtime.database.paper_id_for_pdf(&pdf)? {
                sync_pdf_collection_membership(
                    &runtime.database,
                    paper_id,
                    &pdf,
                    &runtime.collection_roots,
                )?;
            }
            app.library.message = Some(if imported {
                format!("Imported {}", pdf.title)
            } else {
                "Ignored duplicate PDF".into()
            });
            spawn_enrichment_if_needed(runtime, senders, app)?;
        }
        IndexResponse::File(Err(error)) => {
            eprintln!("Library indexing error: {error}");
        }
    }
    refresh_library(runtime, app)?;
    refresh_organization(&runtime.database, &runtime.library_roots, app)?;
    refresh_dashboard(runtime, app)?;
    refresh_downloads(runtime, app);
    Ok(())
}

fn sync_pdf_collection_membership(
    database: &Database,
    paper_id: i64,
    pdf: &ImportedPdf,
    roots: &[PathBuf],
) -> Result<()> {
    let Some(root) = roots
        .iter()
        .filter(|root| pdf.path.starts_with(root))
        .max_by_key(|root| root.components().count())
    else {
        database.clear_collection_membership(paper_id)?;
        return Ok(());
    };
    let mut classified = pdf.clone();
    classified.library_root = Some(root.clone());
    classified.relative_directory = pdf
        .path
        .parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(Path::to_path_buf);
    database.sync_pdf_collection(paper_id, &classified)?;
    Ok(())
}

fn discover_local_downloads(
    app: &mut App,
    download_dir: &std::path::Path,
    database: &papr_core::database::Database,
) {
    if let Ok(entries) = std::fs::read_dir(download_dir) {
        let mut existing_files: Vec<_> = entries
            .filter_map(std::result::Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "pdf"))
            .collect();
        existing_files.sort_by_key(|e| {
            std::cmp::Reverse(
                e.metadata()
                    .and_then(|m| m.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
            )
        });
        for entry in existing_files {
            let size = entry.metadata().ok().map_or(0, |m| m.len());
            let title = entry.file_name().to_string_lossy().into_owned();
            let id = title.strip_suffix(".pdf").unwrap_or(&title).to_owned();
            let pdf_path = entry.path().to_string_lossy().into_owned();
            let paper_id = database.paper_id_for_path(&pdf_path).ok().flatten();
            app.downloads.push(DownloadTask {
                id,
                title,
                downloaded: size,
                total: Some(size),
                paper_id,
                pdf_path: Some(pdf_path),
                status: DownloadStatus::Completed,
            });
        }
    }
}

fn start_download(
    paper: RemotePaper,
    directory: &std::path::Path,
    manager: &DownloadManager,
    events: &mpsc::UnboundedSender<DownloadEvent>,
    pending: &mut HashMap<String, RemotePaper>,
    app: &mut App,
) {
    let Some(url) = paper.pdf_url.clone() else {
        return;
    };
    if pending.contains_key(&paper.id) {
        return;
    }
    let sanitized_title: String = paper
        .title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            '\n' | '\r' | '\t' => ' ',
            c => c,
        })
        .collect();
    let sanitized_title = sanitized_title.trim();
    let filename = if sanitized_title.is_empty() {
        paper
            .id
            .rsplit('/')
            .next()
            .unwrap_or("paper")
            .replace(['/', '\\'], "_")
    } else {
        sanitized_title.to_string()
    };
    let destination = directory.join(format!("{filename}.pdf"));
    let id = paper.id.clone();
    pending.insert(id.clone(), paper.clone());
    app.downloads.push(DownloadTask {
        id: id.clone(),
        title: paper.title,
        downloaded: 0,
        total: None,
        paper_id: None,
        pdf_path: None,
        status: DownloadStatus::Starting,
    });
    let manager = manager.clone();
    let events = events.clone();
    tokio::spawn(async move {
        if let Err(error) = manager.download(&id, &url, &destination, &events).await {
            let _ = events.send(DownloadEvent::Failed {
                id,
                error: error.to_string(),
            });
        }
    });
}

fn apply_download_event(
    event: DownloadEvent,
    pending: &mut HashMap<String, RemotePaper>,
    runtime: &mut Runtime,
    app: &mut App,
    index_sender: &mpsc::UnboundedSender<IndexResponse>,
) -> Result<()> {
    let id = match &event {
        DownloadEvent::Started { id, .. }
        | DownloadEvent::Progress { id, .. }
        | DownloadEvent::Completed { id, .. }
        | DownloadEvent::Failed { id, .. } => id,
    };
    let Some(task) = app.downloads.iter_mut().find(|task| &task.id == id) else {
        return Ok(());
    };
    match event {
        DownloadEvent::Started { total, .. } => {
            task.total = total;
            task.status = DownloadStatus::Running;
        }
        DownloadEvent::Progress {
            downloaded, total, ..
        } => {
            task.downloaded = downloaded;
            task.total = total;
        }
        DownloadEvent::Completed { id, path } => {
            let pdf = LibraryIndexer::inspect(&path).context("failed to index downloaded PDF")?;
            if let Some(paper) = pending.remove(&id) {
                let paper_id = runtime.database.attach_download(&paper, &pdf)?;
                runtime
                    .database
                    .record_activity("downloaded", Some(paper_id), None)?;
                task.paper_id = Some(paper_id);
            }
            task.pdf_path = Some(pdf.path.to_string_lossy().to_string());
            task.downloaded = pdf.file_size;
            task.total = Some(pdf.file_size);
            task.status = DownloadStatus::Completed;
            refresh_library(runtime, app)?;
            refresh_dashboard(runtime, app)?;
            let _ = index_sender.send(IndexResponse::File(Ok(pdf)));
        }
        DownloadEvent::Failed { id, error } => {
            pending.remove(&id);
            task.status = DownloadStatus::Failed(error);
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.mode == AppMode::CommandPalette {
        match key.code {
            KeyCode::Esc => app.dispatch(Command::TogglePalette),
            KeyCode::Up | KeyCode::Char('k') => {
                app.palette_selected = app.palette_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.palette_selected = (app.palette_selected + 1)
                    .min(papr_core::Page::ALL.len().saturating_sub(1));
            }
            KeyCode::Enter => {
                let selected = app.palette_selected;
                app.dispatch(Command::TogglePalette);
                app.sidebar_index = selected;
                app.page = papr_core::Page::ALL[selected];
                app.content_focused = true;
            }
            _ => {}
        }
        return None;
    }
    if app.mode == AppMode::Help {
        if matches!(key.code, KeyCode::Esc | KeyCode::Char('?' | 'q')) {
            app.dispatch(Command::ToggleHelp);
        }
        return None;
    }
    if matches!(app.mode, AppMode::NoteEdit | AppMode::Prompt) {
        return handle_modal_key(app, key);
    }
    if app.mode == AppMode::ConfirmDelete {
        return handle_confirm_delete_key(app, key);
    }
    if app.mode == AppMode::Search {
        return handle_search_key(app, key);
    }
    if app.mode == AppMode::WorkspaceSearch {
        return handle_workspace_search_key(app, key);
    }
    let key = normalize_panel_navigation(key);
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let Some(command) = navigation_command(key) {
            app.dispatch(command);
            return None;
        }
    }
    if app.mode == AppMode::PaperDetail {
        return handle_paper_detail_key(app, key);
    }

    if key.code == KeyCode::Char('/') {
        app.page = papr_core::Page::Discover;
        app.sidebar_index = 1;
        app.content_focused = true;
        app.mode = AppMode::Search;
        app.discovery.query_cursor = app.discovery.query.len();
        return None;
    }
    if !app.content_focused {
        if app.page == papr_core::Page::Settings && matches!(key.code, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')) {
            app.content_focused = true;
            app.config_editor_focused = true;
            return None;
        }
        if let Some(command) = navigation_command(key) {
            app.dispatch(command);
        }
        return None;
    }
    if key.code == KeyCode::Char('r')
        && app.page == papr_core::Page::Discover
        && !app.discovery.query.trim().is_empty()
    {
        let query = app.discovery.query.trim().to_owned();
        app.discovery.query.clone_from(&query);
        return Some(UiAction::Search(query));
    }
    if key.code == KeyCode::Char('r') && app.page == papr_core::Page::Library {
        return Some(UiAction::Reindex);
    }
    if let KeyHandling::Handled(action) = handle_dashboard_key(app, key) {
        return action.map(|action| *action);
    }
    if app.page == papr_core::Page::Collections {
        let (handled, action) = handle_collection_key(app, key);
        if handled {
            return action;
        }
    }
    if app.page == papr_core::Page::Authors {
        let (handled, action) = handle_author_key(app, key);
        if handled {
            return action;
        }
    }
    if let Some(action) = bookmark_action(app, key) {
        return Some(action);
    }
    if let Some(action) = handle_notes_key(app, key) {
        return Some(action);
    }
    if app.page == papr_core::Page::Discover && key.code == KeyCode::Char('o') {
        return app
            .discovery
            .results
            .get(app.discovery.selected)
            .map(|paper| UiAction::OpenBrowser(paper.id.clone()));
    }
    if app.page == papr_core::Page::Discover && key.code == KeyCode::Char('c') {
        return app
            .discovery
            .results
            .get(app.discovery.selected)
            .cloned()
            .map(|paper| UiAction::CopyCitation(PaperTarget::Remote(Box::new(paper))));
    }
    if app.page == papr_core::Page::Discover
        && matches!(
            key.code,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
        )
    {
        return app
            .discovery
            .results
            .get(app.discovery.selected)
            .cloned()
            .map(UiAction::OpenPaper);
    }
    if let Some(action) = handle_downloads_key(app, key) {
        return Some(action);
    }

    if let Some(action) = library_action(app, key) {
        return Some(action);
    }

    if let Some(command) = navigation_command(key) {
        app.dispatch(command);
    }
    None
}

fn normalize_panel_navigation(mut key: KeyEvent) -> KeyEvent {
    key.code = match key.code {
        KeyCode::Left => KeyCode::Char('h'),
        KeyCode::Right => KeyCode::Char('l'),
        code => code,
    };
    key
}

fn handle_search_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    match key.code {
        KeyCode::Esc => app.mode = AppMode::Normal,
        KeyCode::Enter if !app.discovery.query.trim().is_empty() => {
            let query = app.discovery.query.trim().to_owned();
            app.discovery.query.clone_from(&query);
            app.mode = AppMode::Normal;
            return Some(UiAction::Search(query));
        }
        _ => {
            edit_text(
                &mut app.discovery.query,
                &mut app.discovery.query_cursor,
                key,
            );
        }
    }
    None
}

fn handle_workspace_search_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    match key.code {
        KeyCode::Esc => app.mode = AppMode::Normal,
        KeyCode::Down | KeyCode::Enter => {
            app.mode = AppMode::Normal;
            app.content_focused = true;
        }
        _ => {
            edit_text(
                &mut app.workspace_query,
                &mut app.workspace_query_cursor,
                key,
            );
        }
    }
    None
}

fn handle_confirm_delete_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    use papr_core::DeletionTarget;
    match key.code {
        KeyCode::Char('y' | 'Y') | KeyCode::Enter => {
            let target = app.delete_confirmation.take()?;
            app.mode = AppMode::Normal;
            match target {
                DeletionTarget::Paper { id, path, .. } => {
                    Some(UiAction::DeletePaper { paper_id: id, path })
                }
                DeletionTarget::Collection { id, path, .. } => {
                    Some(UiAction::DeleteCollection { collection_id: id, path })
                }
            }
        }
        KeyCode::Char('n' | 'N') | KeyCode::Esc | KeyCode::Char('q') => {
            app.delete_confirmation = None;
            app.mode = AppMode::Normal;
            None
        }
        _ => None,
    }
}

fn bookmark_action(app: &App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Bookmarks {
        return None;
    }
    let bookmark = *app.filtered_bookmarks().get(app.bookmark_selected)?;
    match key.code {
        KeyCode::Char('B') => Some(UiAction::Bookmark(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Char('c') => Some(UiAction::CopyCitation(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Enter | KeyCode::Char('p') => Some(UiAction::OpenPdf {
            paper_id: bookmark.paper_id,
            path: PathBuf::from(&bookmark.pdf_path),
        }),
        KeyCode::Char('n') => Some(UiAction::OpenNote(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Char('s') => Some(UiAction::Prompt(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Char('R') => Some(UiAction::RenamePdf(bookmark.paper_id)),
        KeyCode::Char('x') => Some(UiAction::ConfirmDeletePaper {
            paper_id: bookmark.paper_id,
            title: bookmark.paper_title.clone(),
            path: Some(PathBuf::from(&bookmark.pdf_path)),
        }),
        _ => None,
    }
}

fn handle_notes_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Notes {
        return None;
    }
    let paper = *app.filtered_notes_papers().get(app.notes_selected)?;
    match key.code {
        KeyCode::Char('n') | KeyCode::Enter => Some(UiAction::OpenNote(PaperTarget::Local(paper.id))),
        KeyCode::Char('p') => {
            let path = paper.pdf_path.clone().map(PathBuf::from)?;
            Some(UiAction::OpenPdf {
                paper_id: paper.id,
                path,
            })
        }
        KeyCode::Char('B') => Some(UiAction::Bookmark(PaperTarget::Local(paper.id))),
        KeyCode::Char('c') => Some(UiAction::CopyCitation(PaperTarget::Local(paper.id))),
        KeyCode::Char('s') => Some(UiAction::Prompt(PaperTarget::Local(paper.id))),
        KeyCode::Char('R') => Some(UiAction::RenamePdf(paper.id)),
        KeyCode::Char('x') => Some(UiAction::ConfirmDeletePaper {
            paper_id: paper.id,
            title: paper.title.clone(),
            path: paper.pdf_path.as_ref().map(PathBuf::from),
        }),
        _ => None,
    }
}

fn handle_downloads_key(app: &App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Downloads {
        return None;
    }
    if matches!(
        key.code,
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
    ) {
        if let Some(&task) = app.filtered_downloads().get(app.download_selected) {
            if matches!(task.status, DownloadStatus::Completed) {
                return Some(UiAction::OpenDownload(task.id.clone()));
            }
        }
    }
    if matches!(
        key.code,
        KeyCode::Char('R') | KeyCode::Char('s') | KeyCode::Char('B') | KeyCode::Char('n') | KeyCode::Char('c') | KeyCode::Char('x')
    ) {
        if let Some(&task) = app.filtered_downloads().get(app.download_selected) {
            if matches!(task.status, DownloadStatus::Completed) {
                if let Some(paper_id) = task.paper_id {
                    return match key.code {
                        KeyCode::Char('R') => Some(UiAction::RenamePdf(paper_id)),
                        KeyCode::Char('s') => Some(UiAction::Prompt(PaperTarget::Local(paper_id))),
                        KeyCode::Char('B') => Some(UiAction::Bookmark(PaperTarget::Local(paper_id))),
                        KeyCode::Char('c') => Some(UiAction::CopyCitation(PaperTarget::Local(paper_id))),
                        KeyCode::Char('n') => Some(UiAction::OpenNote(PaperTarget::Local(paper_id))),
                        KeyCode::Char('x') => Some(UiAction::ConfirmDeletePaper {
                            paper_id,
                            title: task.title.clone(),
                            path: task.pdf_path.as_ref().map(PathBuf::from),
                        }),
                        _ => None,
                    };
                }
            }
        }
    }
    None
}

fn library_action(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Library {
        return None;
    }
    if matches!(key.code, KeyCode::Char('p') | KeyCode::Enter) {
        return selected_library_pdf(app)
            .map(|(paper_id, path)| UiAction::OpenPdf { paper_id, path });
    }
    if key.code == KeyCode::Char('x') {
        let paper = *app.filtered_library_papers().get(app.library.selected)?;
        return Some(UiAction::ConfirmDeletePaper {
            paper_id: paper.id,
            title: paper.title.clone(),
            path: paper.pdf_path.as_ref().map(PathBuf::from),
        });
    }
    handle_library_metadata_key(app, key)
}

fn handle_dashboard_key(app: &mut App, key: KeyEvent) -> KeyHandling {
    if app.page != papr_core::Page::Dashboard {
        return KeyHandling::Ignored;
    }
    if key.code == KeyCode::Char('o') {
        return KeyHandling::Handled(
            app.today_papers
                .get(app.today_selected)
                .map(|paper| UiAction::OpenBrowser(paper.id.clone()))
                .map(Box::new),
        );
    }
    if key.code == KeyCode::Char('c') {
        return KeyHandling::Handled(
            app.today_papers
                .get(app.today_selected)
                .cloned()
                .map(|paper| Box::new(UiAction::CopyCitation(PaperTarget::Remote(Box::new(paper))))),
        );
    }
    if matches!(
        key.code,
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
    ) {
        return KeyHandling::Handled(
            app.today_papers
                .get(app.today_selected)
                .cloned()
                .map(UiAction::OpenPaper)
                .map(Box::new),
        );
    }
    KeyHandling::Ignored
}

fn handle_collection_key(app: &mut App, key: KeyEvent) -> (bool, Option<UiAction>) {
    if app.active_collection.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') => {
                app.active_collection = None;
                app.collection_papers.clear();
                return (true, None);
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l' | 'p') => {
                let Some(&paper) = app.filtered_collection_papers().get(app.collection_paper_selected) else {
                    return (true, None);
                };
                let Some(path) = &paper.pdf_path else {
                    app.toast = Some("This paper has no local PDF to open".into());
                    return (true, None);
                };
                return (
                    true,
                    Some(UiAction::OpenPdf {
                        paper_id: paper.id,
                        path: PathBuf::from(path),
                    }),
                );
            }
            KeyCode::Char('B') => {
                return (
                    true,
                    app.filtered_collection_papers()
                        .get(app.collection_paper_selected)
                        .map(|&paper| UiAction::Bookmark(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('c') => {
                return (
                    true,
                    app.filtered_collection_papers()
                        .get(app.collection_paper_selected)
                        .map(|&paper| UiAction::CopyCitation(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('R') => {
                return (
                    true,
                    app.filtered_collection_papers()
                        .get(app.collection_paper_selected)
                        .map(|&paper| UiAction::RenamePdf(paper.id)),
                );
            }
            KeyCode::Char('s') => {
                return (
                    true,
                    app.filtered_collection_papers()
                        .get(app.collection_paper_selected)
                        .map(|&paper| UiAction::Prompt(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('n') => {
                return (
                    true,
                    app.filtered_collection_papers()
                        .get(app.collection_paper_selected)
                        .map(|&paper| UiAction::OpenNote(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('x') => {
                return (
                    true,
                    app.filtered_collection_papers()
                        .get(app.collection_paper_selected)
                        .map(|&paper| UiAction::ConfirmDeletePaper {
                            paper_id: paper.id,
                            title: paper.title.clone(),
                            path: paper.pdf_path.as_ref().map(PathBuf::from),
                        }),
                );
            }
            _ => return (false, None),
        }
    }
    if let Some(item) = app.filtered_collections().get(app.collection_selected) {
        use papr_core::CollectionSearchItem;
        match item {
            CollectionSearchItem::Collection(collection) => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')) {
                    return (true, Some(UiAction::OpenCollection(collection.id)));
                }
                if key.code == KeyCode::Char('R') {
                    return (true, Some(UiAction::RenameCollection(collection.id)));
                }
                if key.code == KeyCode::Char('x') {
                    return (
                        true,
                        Some(UiAction::ConfirmDeleteCollection {
                            collection_id: collection.id,
                            name: collection.name.clone(),
                            path: collection.folder_path.as_ref().map(PathBuf::from),
                        }),
                    );
                }
            }
            CollectionSearchItem::Paper(paper, _collection) => {
                if matches!(key.code, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l' | 'p')) {
                    if let Some(path) = &paper.pdf_path {
                        return (
                            true,
                            Some(UiAction::OpenPdf {
                                paper_id: paper.id,
                                path: PathBuf::from(path),
                            }),
                        );
                    } else {
                        app.toast = Some("This paper has no local PDF to open".into());
                        return (true, None);
                    }
                }
                if key.code == KeyCode::Char('B') {
                    return (true, Some(UiAction::Bookmark(PaperTarget::Local(paper.id))));
                }
                if key.code == KeyCode::Char('c') {
                    return (true, Some(UiAction::CopyCitation(PaperTarget::Local(paper.id))));
                }
                if key.code == KeyCode::Char('R') {
                    return (true, Some(UiAction::RenamePdf(paper.id)));
                }
                if key.code == KeyCode::Char('s') {
                    return (true, Some(UiAction::Prompt(PaperTarget::Local(paper.id))));
                }
                if key.code == KeyCode::Char('n') {
                    return (true, Some(UiAction::OpenNote(PaperTarget::Local(paper.id))));
                }
                if key.code == KeyCode::Char('x') {
                    return (
                        true,
                        Some(UiAction::ConfirmDeletePaper {
                            paper_id: paper.id,
                            title: paper.title.clone(),
                            path: paper.pdf_path.as_ref().map(PathBuf::from),
                        }),
                    );
                }
            }
        }
    }
    if matches!(key.code, KeyCode::Char('c' | 'n')) {
        return (true, Some(UiAction::CreateCollection));
    }
    (false, None)
}

fn handle_author_key(app: &mut App, key: KeyEvent) -> (bool, Option<UiAction>) {
    if app.active_author.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') => {
                app.active_author = None;
                app.author_papers.clear();
                return (true, None);
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l' | 'p') => {
                let Some(&paper) = app.filtered_author_papers().get(app.author_paper_selected) else {
                    return (true, None);
                };
                let Some(path) = &paper.pdf_path else {
                    app.toast = Some("This paper has no local PDF to open".into());
                    return (true, None);
                };
                return (
                    true,
                    Some(UiAction::OpenPdf {
                        paper_id: paper.id,
                        path: PathBuf::from(path),
                    }),
                );
            }
            KeyCode::Char('B') => {
                return (
                    true,
                    app.filtered_author_papers()
                        .get(app.author_paper_selected)
                        .map(|&paper| UiAction::Bookmark(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('c') => {
                return (
                    true,
                    app.filtered_author_papers()
                        .get(app.author_paper_selected)
                        .map(|&paper| UiAction::CopyCitation(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('R') => {
                return (
                    true,
                    app.filtered_author_papers()
                        .get(app.author_paper_selected)
                        .map(|&paper| UiAction::RenamePdf(paper.id)),
                );
            }
            KeyCode::Char('s') => {
                return (
                    true,
                    app.filtered_author_papers()
                        .get(app.author_paper_selected)
                        .map(|&paper| UiAction::Prompt(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('n') => {
                return (
                    true,
                    app.filtered_author_papers()
                        .get(app.author_paper_selected)
                        .map(|&paper| UiAction::OpenNote(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('x') => {
                return (
                    true,
                    app.filtered_author_papers()
                        .get(app.author_paper_selected)
                        .map(|&paper| UiAction::ConfirmDeletePaper {
                            paper_id: paper.id,
                            title: paper.title.clone(),
                            path: paper.pdf_path.as_ref().map(PathBuf::from),
                        }),
                );
            }
            _ => return (false, None),
        }
    }
    if matches!(
        key.code,
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
    ) {
        let action = app
            .filtered_authors()
            .get(app.author_selected)
            .map(|&author| UiAction::OpenAuthor(author.id));
        return (true, action);
    }
    (false, None)
}

fn navigation_command(key: KeyEvent) -> Option<Command> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('p'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::TogglePalette)
        }
        (KeyCode::Char('/'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::ToggleWorkspaceSearch)
        }
        (KeyCode::Char('j') | KeyCode::Down, _) => Some(Command::MoveDown),
        (KeyCode::Char('k') | KeyCode::Up, _) => Some(Command::MoveUp),
        (KeyCode::Enter | KeyCode::Right | KeyCode::Char('l'), _) => Some(Command::Open),
        (KeyCode::Left | KeyCode::Char('h'), _) => Some(Command::Back),
        (KeyCode::Char('?'), _) => Some(Command::ToggleHelp),
        (KeyCode::Char('q'), _) => Some(Command::Quit),
        _ => None,
    }
}

fn edit_text(text: &mut String, cursor: &mut usize, key: KeyEvent) -> bool {
    let mut changed = false;
    match key.code {
        KeyCode::Left => {
            if *cursor > 0 {
                let mut prev = *cursor - 1;
                while prev > 0 && !text.is_char_boundary(prev) {
                    prev -= 1;
                }
                *cursor = prev;
            }
        }
        KeyCode::Right => {
            if *cursor < text.len() {
                let mut next = *cursor + 1;
                while next < text.len() && !text.is_char_boundary(next) {
                    next += 1;
                }
                *cursor = next;
            }
        }
        KeyCode::Backspace => {
            if *cursor > 0 {
                let mut prev = *cursor - 1;
                while prev > 0 && !text.is_char_boundary(prev) {
                    prev -= 1;
                }
                text.remove(prev);
                *cursor = prev;
                changed = true;
            }
        }
        KeyCode::Delete => {
            if *cursor < text.len() {
                text.remove(*cursor);
                changed = true;
            }
        }
        KeyCode::Up => {
            if *cursor > 0 {
                let current_line_start = text[..*cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
                let col = *cursor - current_line_start;
                if current_line_start > 0 {
                    let prev_line_search = &text[..current_line_start - 1];
                    let prev_line_start = prev_line_search.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
                    let prev_line_len = (current_line_start - 1) - prev_line_start;
                    let mut target = prev_line_start + col.min(prev_line_len);
                    while target > prev_line_start && !text.is_char_boundary(target) {
                        target -= 1;
                    }
                    *cursor = target;
                }
            }
        }
        KeyCode::Down => {
            if *cursor < text.len() {
                let current_line_start = text[..*cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
                let col = *cursor - current_line_start;
                if let Some(current_line_end) = text[*cursor..].find('\n').map(|idx| *cursor + idx) {
                    let next_line_start = current_line_end + 1;
                    if next_line_start <= text.len() {
                        let next_line_end = text[next_line_start..].find('\n').map(|idx| next_line_start + idx).unwrap_or(text.len());
                        let next_line_len = next_line_end - next_line_start;
                        let mut target = next_line_start + col.min(next_line_len);
                        while target > next_line_start && !text.is_char_boundary(target) {
                            target -= 1;
                        }
                        *cursor = target;
                    }
                }
            }
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            text.insert(*cursor, character);
            *cursor += character.len_utf8();
            changed = true;
        }
        _ => {}
    }
    changed
}

fn handle_modal_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.mode == AppMode::Prompt {
        match key.code {
            KeyCode::Esc => {
                app.metadata_prompt = None;
                app.mode = app.modal_return;
            }
            KeyCode::Enter => {
                if let Some(prompt) = &mut app.metadata_prompt
                    && prompt.value.trim().is_empty()
                    && prompt.rename_collection_id.is_none()
                    && prompt.paper_id.is_some()
                    && let Some(collection) = app.collections.get(prompt.selected)
                {
                    prompt.value.clone_from(&collection.name);
                }
                let prompt = app.metadata_prompt.take();
                app.mode = app.modal_return;
                return prompt.map(UiAction::SubmitPrompt);
            }
            KeyCode::Down => {
                if let Some(prompt) = &mut app.metadata_prompt {
                    prompt.selected =
                        (prompt.selected + 1).min(app.collections.len().saturating_sub(1));
                }
            }
            KeyCode::Up => {
                if let Some(prompt) = &mut app.metadata_prompt {
                    prompt.selected = prompt.selected.saturating_sub(1);
                }
            }
            _ => {
                if let Some(prompt) = &mut app.metadata_prompt {
                    edit_text(&mut prompt.value, &mut prompt.cursor, key);
                }
            }
        }
        return None;
    }
    if key.code == KeyCode::Tab {
        app.note_preview = !app.note_preview;
        return None;
    }
    if app.note_preview {
        if key.code == KeyCode::Esc {
            app.mode = app.modal_return;
            return app.note_editor.clone().map(UiAction::SaveNote);
        }
        return None;
    }
    let mut changed = false;
    match key.code {
        KeyCode::Esc => {
            app.mode = app.modal_return;
            return app.note_editor.clone().map(UiAction::SaveNote);
        }
        KeyCode::Enter => {
            if let Some(note) = &mut app.note_editor {
                note.body.insert(note.cursor, '\n');
                note.cursor += 1;
                changed = true;
            }
        }
        _ => {
            if let Some(note) = &mut app.note_editor {
                changed = edit_text(&mut note.body, &mut note.cursor, key);
            }
        }
    }
    changed
        .then(|| app.note_editor.clone())
        .flatten()
        .map(UiAction::SaveNote)
}

fn handle_paper_detail_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    match key.code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h' | 'q') => app.dispatch(Command::Back),
        KeyCode::Char('j') | KeyCode::Down => {
            app.discovery.detail_scroll = app.discovery.detail_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.discovery.detail_scroll = app.discovery.detail_scroll.saturating_sub(1);
        }
        KeyCode::Char('d') => {
            return app
                .discovery
                .results
                .get(app.discovery.selected)
                .cloned()
                .map(UiAction::Download);
        }
        KeyCode::Char('c') => {
            return selected_remote_target(app).map(UiAction::CopyCitation);
        }
        KeyCode::Char('o') => {
            return app
                .discovery
                .results
                .get(app.discovery.selected)
                .map(|paper| UiAction::OpenBrowser(paper.id.clone()));
        }
        KeyCode::Char('n' | 's') => {
            app.modal_return = AppMode::PaperDetail;
            let target = selected_remote_target(app)?;
            return Some(match key.code {
                KeyCode::Char('n') => UiAction::OpenNote(target),
                _ => UiAction::Prompt(target),
            });
        }
        KeyCode::Char('B') => return selected_remote_target(app).map(UiAction::Bookmark),
        _ => {}
    }
    None
}

fn handle_library_metadata_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    let target = app
        .filtered_library_papers()
        .get(app.library.selected)
        .map(|&paper| PaperTarget::Local(paper.id))?;
    app.modal_return = AppMode::Normal;
    match key.code {
        KeyCode::Char('n') => Some(UiAction::OpenNote(target)),
        KeyCode::Char('s') => Some(UiAction::Prompt(target)),
        KeyCode::Char('B') => Some(UiAction::Bookmark(target)),
        KeyCode::Char('c') => Some(UiAction::CopyCitation(target)),
        KeyCode::Char('R') => {
            if let PaperTarget::Local(id) = target {
                Some(UiAction::RenamePdf(id))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn selected_remote_target(app: &App) -> Option<PaperTarget> {
    app.discovery
        .results
        .get(app.discovery.selected)
        .cloned()
        .map(Box::new)
        .map(PaperTarget::Remote)
}

fn selected_library_pdf(app: &App) -> Option<(i64, PathBuf)> {
    let paper = *app.filtered_library_papers().get(app.library.selected)?;
    paper
        .pdf_path
        .as_ref()
        .map(|path| (paper.id, PathBuf::from(path)))
}

fn record_config_history(app: &mut App) {
    if app.config_editor_history.is_empty() || app.config_editor_history[app.config_editor_history_idx] != app.config_editor_text {
        app.config_editor_history.truncate(app.config_editor_history_idx + 1);
        app.config_editor_history.push(app.config_editor_text.clone());
        if app.config_editor_history.len() > 50 {
            app.config_editor_history.remove(0);
        }
        app.config_editor_history_idx = app.config_editor_history.len() - 1;
    }
}

fn apply_config_update(
    runtime: &mut Runtime,
    app: &mut App,
    config: &Config,
    theme: &mut Theme,
) -> Result<()> {
    let new_theme = Theme::load(&config.theme).map_err(|e| anyhow::anyhow!("Theme load failed: {e}"))?;
    *theme = new_theme;

    runtime.pdf_viewer = config.pdf_viewer.clone().unwrap_or_else(default_pdf_viewer);

    let download_dir = config.download_path.clone().unwrap_or_else(|| runtime.default_downloads_dir.clone());
    runtime.download_dir = download_dir.clone();

    let collection_roots = config.library_folders.clone();
    runtime.collection_roots = collection_roots.clone();

    let mut library_roots = collection_roots.clone();
    let download_inside = collection_roots.iter().any(|root| {
        let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        let dl_canon = std::fs::canonicalize(&download_dir).unwrap_or_else(|_| download_dir.clone());
        dl_canon.starts_with(&root_canon)
    });
    if !download_inside {
        library_roots.push(download_dir.clone());
    }
    if !library_roots.is_empty() {
        runtime.primary_library_root = library_roots[0].clone();
    }
    runtime.library_roots = library_roots;

    runtime.dashboard_keywords = config.dashboard_keyword_list();
    runtime.dashboard_keyword_signature.clear();

    restart_runtime_watcher(runtime)?;
    refresh_library(runtime, app)?;
    refresh_organization(&runtime.database, &runtime.library_roots, app)?;
    refresh_dashboard(runtime, app)?;
    refresh_downloads(runtime, app);

    Ok(())
}

fn handle_config_editor_key(
    app: &mut App,
    key: KeyEvent,
    runtime: &mut Runtime,
    theme: &mut Theme,
) -> Option<UiAction> {
    if let Some(mut cmd) = app.config_editor_command.clone() {
        match key.code {
            KeyCode::Esc => {
                app.config_editor_command = None;
            }
            KeyCode::Char(c) => {
                cmd.push(c);
                app.config_editor_command = Some(cmd);
            }
            KeyCode::Backspace => {
                cmd.pop();
                app.config_editor_command = Some(cmd);
            }
            KeyCode::Enter => {
                app.config_editor_command = None;
                let trimmed = cmd.trim();
                if trimmed == "w" || trimmed == "wq" {
                    let toml_str = &app.config_editor_text;
                    match toml::from_str::<Config>(toml_str) {
                        Ok(new_config) => {
                            if let Err(e) = std::fs::write(&runtime.config_file, toml_str) {
                                app.config_editor_error = Some(format!("Write failed: {e}"));
                            } else {
                                app.config_editor_error = None;
                                app.toast = Some("Configuration saved and applied.".to_owned());
                                if let Err(e) = apply_config_update(runtime, app, &new_config, theme) {
                                    app.config_editor_error = Some(format!("Apply failed: {e}"));
                                }
                            }
                        }
                        Err(e) => {
                            app.config_editor_error = Some(format!("Invalid TOML: {e}"));
                        }
                    }
                }
                if trimmed == "q" || (trimmed == "wq" && app.config_editor_error.is_none()) {
                    app.config_editor_focused = false;
                    app.content_focused = false;
                }
            }
            _ => {}
        }
        return None;
    }

    if app.config_editor_insert_mode {
        handle_config_editor_insert_key(app, key);
        return None;
    }

    match key.code {
        KeyCode::Char('i') => {
            app.config_editor_insert_mode = true;
            reset_config_editor_goal_column(app);
        }
        KeyCode::Char(':') => {
            app.config_editor_command = Some(String::new());
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if key.code == KeyCode::Left {
                app.config_editor_focused = false;
                app.content_focused = false;
            } else if app.config_editor_cursor > 0 {
                let mut prev = app.config_editor_cursor - 1;
                while prev > 0 && !app.config_editor_text.is_char_boundary(prev) {
                    prev -= 1;
                }
                if app.config_editor_text.as_bytes().get(prev) != Some(&b'\n') {
                    app.config_editor_cursor = prev;
                }
                reset_config_editor_goal_column(app);
            }
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if app.config_editor_cursor < app.config_editor_text.len() {
                let next = next_char_boundary(&app.config_editor_text, app.config_editor_cursor);
                if app.config_editor_text.as_bytes().get(app.config_editor_cursor) != Some(&b'\n') {
                    app.config_editor_cursor = next.min(app.config_editor_text.len());
                }
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let text = &app.config_editor_text;
            let cursor = &mut app.config_editor_cursor;
            if *cursor > 0 {
                let current_line_start = text[..*cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
                let col = *cursor - current_line_start;
                if current_line_start > 0 {
                    let prev_line_search = &text[..current_line_start - 1];
                    let prev_line_start = prev_line_search.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
                    let prev_line_len = (current_line_start - 1) - prev_line_start;
                    let mut target = prev_line_start + col.min(prev_line_len);
                    while target > prev_line_start && !text.is_char_boundary(target) {
                        target -= 1;
                    }
                    *cursor = target;
                }
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let text = &app.config_editor_text;
            let cursor = &mut app.config_editor_cursor;
            if *cursor < text.len() {
                let current_line_start = text[..*cursor].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
                let col = *cursor - current_line_start;
                if let Some(current_line_end) = text[*cursor..].find('\n').map(|idx| *cursor + idx) {
                    let next_line_start = current_line_end + 1;
                    if next_line_start <= text.len() {
                        let next_line_end = text[next_line_start..].find('\n').map(|idx| next_line_start + idx).unwrap_or(text.len());
                        let next_line_len = next_line_end - next_line_start;
                        let mut target = next_line_start + col.min(next_line_len);
                        while target > next_line_start && !text.is_char_boundary(target) {
                            target -= 1;
                        }
                        *cursor = target;
                    }
                }
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Char('x') => {
            if app.config_editor_cursor < app.config_editor_text.len() {
                record_config_history(app);
                app.config_editor_text.remove(app.config_editor_cursor);
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Char('u') => {
            if app.config_editor_history_idx > 0 {
                app.config_editor_history_idx -= 1;
                app.config_editor_text = app.config_editor_history[app.config_editor_history_idx].clone();
                app.config_editor_cursor = app.config_editor_cursor.min(app.config_editor_text.len());
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.config_editor_history_idx + 1 < app.config_editor_history.len() {
                app.config_editor_history_idx += 1;
                app.config_editor_text = app.config_editor_history[app.config_editor_history_idx].clone();
                app.config_editor_cursor = app.config_editor_cursor.min(app.config_editor_text.len());
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Char('q') => {
            app.config_editor_focused = false;
            app.content_focused = false;
            reset_config_editor_goal_column(app);
        }
        _ => {}
    }
    None
}

fn handle_config_editor_insert_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.config_editor_insert_mode = false;
            reset_config_editor_goal_column(app);
        }
        KeyCode::Left => {
            app.config_editor_cursor = prev_char_boundary(&app.config_editor_text, app.config_editor_cursor);
            reset_config_editor_goal_column(app);
        }
        KeyCode::Right => {
            app.config_editor_cursor = next_char_boundary(&app.config_editor_text, app.config_editor_cursor);
            reset_config_editor_goal_column(app);
        }
        KeyCode::Up => move_config_editor_vertical(app, -1),
        KeyCode::Down => move_config_editor_vertical(app, 1),
        KeyCode::Home => {
            app.config_editor_cursor = config_editor_line_start(&app.config_editor_text, app.config_editor_cursor);
            reset_config_editor_goal_column(app);
        }
        KeyCode::End => {
            app.config_editor_cursor = config_editor_line_end(&app.config_editor_text, app.config_editor_cursor);
            reset_config_editor_goal_column(app);
        }
        KeyCode::PageUp => move_config_editor_vertical(
            app,
            -(app.config_editor_viewport_height.max(1) as isize),
        ),
        KeyCode::PageDown => move_config_editor_vertical(
            app,
            app.config_editor_viewport_height.max(1) as isize,
        ),
        KeyCode::Backspace => {
            if app.config_editor_cursor > 0 {
                record_config_history(app);
                let prev = prev_char_boundary(&app.config_editor_text, app.config_editor_cursor);
                app.config_editor_text.remove(prev);
                app.config_editor_cursor = prev;
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Delete => {
            if app.config_editor_cursor < app.config_editor_text.len() {
                record_config_history(app);
                app.config_editor_text.remove(app.config_editor_cursor);
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Enter => {
            record_config_history(app);
            app.config_editor_text.insert(app.config_editor_cursor, '\n');
            app.config_editor_cursor += 1;
            reset_config_editor_goal_column(app);
        }
        KeyCode::Tab => {
            record_config_history(app);
            app.config_editor_text.insert(app.config_editor_cursor, '\t');
            app.config_editor_cursor += 1;
            reset_config_editor_goal_column(app);
        }
        KeyCode::Char(c) => {
            record_config_history(app);
            app.config_editor_text.insert(app.config_editor_cursor, c);
            app.config_editor_cursor += c.len_utf8();
            reset_config_editor_goal_column(app);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use papr_core::{
        App, AppMode, BookmarkSummary, CollectionSummary, Database, DownloadStatus, DownloadTask,
        LibraryPaper, Page, PaperNote, RemotePaper,
    };

    use super::{
        UiAction, build_config_editor_view, cursor_visual_position, diverse_latest_papers,
        handle_config_editor_insert_key, handle_key, parse_command, refresh_downloads_from_dir,
    };

    #[test]
    fn control_p_opens_palette() {
        let mut app = App::default();
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.mode, AppMode::CommandPalette);
    }

    #[test]
    fn palette_navigates_options() {
        let mut app = App {
            mode: AppMode::CommandPalette,
            ..App::default()
        };
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        );
        assert_eq!(app.palette_selected, 2);
    }

    #[test]
    fn slash_opens_discovery_search() {
        let mut app = App::default();
        let action = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        assert!(action.is_none());
        assert_eq!(app.mode, AppMode::Search);
        assert_eq!(app.page, papr_core::Page::Discover);
    }

    #[test]
    fn dashboard_navigation_opens_the_selected_paper() {
        let first = remote_paper("https://arxiv.org/abs/1", "First paper");
        let second = remote_paper("https://arxiv.org/abs/2", "Selected paper");
        let mut app = App {
            page: Page::Dashboard,
            content_focused: true,
            today_papers: vec![first, second],
            today_selected: 1,
            ..App::default()
        };

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            action,
            Some(UiAction::OpenPaper(paper)) if paper.title == "Selected paper"
        ));
    }

    #[test]
    fn enter_and_right_open_every_navigation_section() {
        for (index, page) in Page::ALL.into_iter().enumerate() {
            for key in [KeyCode::Enter, KeyCode::Right] {
                let mut app = App {
                    sidebar_index: index,
                    ..App::default()
                };
                let action = handle_key(&mut app, KeyEvent::new(key, KeyModifiers::NONE));
                assert!(action.is_none());
                assert_eq!(app.page, page);
                assert!(app.content_focused);
            }
        }
    }

    #[test]
    fn left_returns_to_navigation_without_changing_selection() {
        for (index, page) in Page::ALL.into_iter().enumerate() {
            let mut app = App {
                page,
                sidebar_index: index,
                content_focused: true,
                ..App::default()
            };
            let action = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
            assert!(action.is_none());
            assert_eq!(app.page, page);
            assert_eq!(app.sidebar_index, index);
            assert!(!app.content_focused);
        }
    }

    #[test]
    fn arrow_and_vim_panel_navigation_preserve_the_same_selection() {
        for (back, open) in [
            (KeyCode::Char('h'), KeyCode::Char('l')),
            (KeyCode::Left, KeyCode::Right),
        ] {
            let mut app = App {
                page: Page::Library,
                sidebar_index: 2,
                content_focused: true,
                library: papr_core::LibraryState {
                    selected: 7,
                    ..papr_core::LibraryState::default()
                },
                ..App::default()
            };
            let _ = handle_key(&mut app, KeyEvent::new(back, KeyModifiers::NONE));
            assert!(!app.content_focused);
            assert_eq!(app.sidebar_index, 2);
            assert_eq!(app.library.selected, 7);

            let _ = handle_key(&mut app, KeyEvent::new(open, KeyModifiers::NONE));
            assert!(app.content_focused);
            assert_eq!(app.page, Page::Library);
            assert_eq!(app.sidebar_index, 2);
            assert_eq!(app.library.selected, 7);
        }
    }

    #[test]
    fn left_and_h_preserve_the_nested_collection_cursor() {
        for back in [KeyCode::Char('h'), KeyCode::Left] {
            let mut app = App {
                page: Page::Collections,
                sidebar_index: 4,
                content_focused: true,
                collection_selected: 3,
                active_collection: Some(CollectionSummary {
                    id: 9,
                    name: "Selected collection".into(),
                    paper_count: 2,
                    folder_path: Some("/tmp/Selected collection".into()),
                }),
                collection_paper_selected: 1,
                last_opened_collection_id: Some(9),
                ..App::default()
            };
            let action = handle_key(&mut app, KeyEvent::new(back, KeyModifiers::NONE));
            assert!(action.is_none());
            assert!(app.active_collection.is_none());
            assert_eq!(app.collection_selected, 3);
            assert_eq!(app.collection_paper_selected, 1);
            assert_eq!(app.last_opened_collection_id, Some(9));
            assert!(app.content_focused);
        }
    }

    #[test]
    fn insert_mode_arrow_keys_move_without_exiting_insert_mode() {
        let mut app = App {
            config_editor_text: "abc\ndef".into(),
            config_editor_cursor: 1,
            config_editor_insert_mode: true,
            config_editor_wrap_width: 8,
            config_editor_viewport_height: 4,
            ..App::default()
        };

        handle_config_editor_insert_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(app.config_editor_insert_mode);
        assert_eq!(app.config_editor_cursor, 2);

        handle_config_editor_insert_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(app.config_editor_insert_mode);
        assert_eq!(app.config_editor_cursor, 1);

        handle_config_editor_insert_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.config_editor_insert_mode);
        assert_eq!(app.config_editor_cursor, 5);

        handle_config_editor_insert_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(app.config_editor_insert_mode);
        assert_eq!(app.config_editor_cursor, 1);
    }

    #[test]
    fn insert_mode_vertical_movement_respects_wrapped_rows() {
        let mut app = App {
            config_editor_text: "abcdefghij".into(),
            config_editor_cursor: 3,
            config_editor_insert_mode: true,
            config_editor_wrap_width: 4,
            config_editor_viewport_height: 3,
            ..App::default()
        };

        handle_config_editor_insert_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.config_editor_cursor, 7);

        handle_config_editor_insert_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.config_editor_cursor, 10);

        handle_config_editor_insert_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.config_editor_cursor, 7);
    }

    #[test]
    fn wrapped_editor_view_tracks_visual_cursor_and_scroll() {
        let text = "abcd\nefghijkl";
        let mut scroll = 0;
        let view = build_config_editor_view(text, 11, 4, 2, &mut scroll);

        assert_eq!(view.lines, vec!["  1 abcd", "  2 efgh", "    ijkl"]);
        assert_eq!((view.cursor_row, view.cursor_col), (2, 2));
        assert_eq!(scroll, 1);
    }

    #[test]
    fn cursor_visual_position_handles_wrap_boundary_at_line_end() {
        let (row, col) = cursor_visual_position("abcd", 4, 4);
        assert_eq!((row, col), (0, 3));
    }

    #[test]
    fn downloads_workspace_syncs_completed_entries_to_download_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "papr-download-sync-{}-{}",
            std::process::id(),
            Utc::now().timestamp_micros()
        ));
        fs::create_dir_all(&root)?;
        let keep_path = root.join("keep.pdf");
        fs::write(&keep_path, b"%PDF keep")?;

        let database = Database::in_memory()?;
        let mut app = App {
            downloads: vec![
                DownloadTask {
                    id: "stale".into(),
                    title: "Stale".into(),
                    downloaded: 10,
                    total: Some(10),
                    paper_id: None,
                    pdf_path: Some(root.join("stale.pdf").to_string_lossy().into_owned()),
                    status: DownloadStatus::Completed,
                },
                DownloadTask {
                    id: "running".into(),
                    title: "Running".into(),
                    downloaded: 5,
                    total: Some(10),
                    paper_id: None,
                    pdf_path: None,
                    status: DownloadStatus::Running,
                },
            ],
            ..App::default()
        };

        refresh_downloads_from_dir(&mut app, &root, &database);
        assert_eq!(app.downloads.len(), 2);
        assert!(app.downloads.iter().any(|task| task.id == "running"));
        assert!(
            app.downloads
                .iter()
                .any(|task| task.pdf_path.as_deref() == Some(keep_path.to_string_lossy().as_ref()))
        );
        assert!(!app.downloads.iter().any(|task| task.id == "stale"));

        let incoming_path = root.join("incoming.pdf");
        fs::write(&incoming_path, b"%PDF incoming")?;
        fs::remove_file(&keep_path)?;
        refresh_downloads_from_dir(&mut app, &root, &database);

        assert!(
            app.downloads
                .iter()
                .any(|task| task.pdf_path.as_deref() == Some(incoming_path.to_string_lossy().as_ref()))
        );
        assert!(
            !app.downloads
                .iter()
                .any(|task| task.pdf_path.as_deref() == Some(keep_path.to_string_lossy().as_ref()))
        );

        let _ = fs::remove_dir_all(root);
        Ok(())
    }

    #[test]
    fn browser_shortcut_uses_selected_dashboard_and_search_urls() {
        let dashboard_paper = remote_paper("https://arxiv.org/abs/dashboard", "Dashboard");
        let search_paper = remote_paper("https://arxiv.org/abs/search", "Search");
        let mut app = App {
            page: Page::Dashboard,
            content_focused: true,
            today_papers: vec![dashboard_paper],
            ..App::default()
        };
        let dashboard = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        );
        assert!(matches!(
            dashboard,
            Some(UiAction::OpenBrowser(url)) if url.ends_with("/dashboard")
        ));

        app.page = Page::Discover;
        app.discovery.results = vec![search_paper];
        let search = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE),
        );
        assert!(matches!(
            search,
            Some(UiAction::OpenBrowser(url)) if url.ends_with("/search")
        ));
    }

    #[test]
    fn note_editor_emits_autosave_action() {
        let mut app = App {
            mode: AppMode::NoteEdit,
            note_editor: Some(PaperNote {
                paper_id: 7,
                title: String::new(),
                body: String::new(),
                cursor: 0,
            }),
            ..App::default()
        };
        let action = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('#'), KeyModifiers::NONE),
        );
        assert!(matches!(action, Some(super::UiAction::SaveNote(_))));
        assert_eq!(
            app.note_editor.as_ref().map(|note| note.body.as_str()),
            Some("#")
        );
    }

    #[test]
    fn library_enter_opens_selected_pdf() {
        let mut app = App {
            page: Page::Library,
            content_focused: true,
            library: papr_core::LibraryState {
                papers: vec![LibraryPaper {
                    id: 7,
                    title: "Paper".into(),
                    authors: String::new(),
                    doi: None,
                    pdf_path: Some("/tmp/paper.pdf".into()),
                    file_size: None,
                    reading_status: "unread".into(),
                    is_favorite: false,
                }],
                ..papr_core::LibraryState::default()
            },
            ..App::default()
        };
        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            action,
            Some(UiAction::OpenPdf { paper_id: 7, path })
                if path == std::path::Path::new("/tmp/paper.pdf")
        ));
    }

    #[test]
    fn parses_pdf_viewer_command_with_arguments() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(parse_command("xdg-open")?, vec!["xdg-open"]);
        assert_eq!(
            parse_command("'tdf viewer' --flag {path}")?,
            vec!["tdf viewer", "--flag", "{path}"]
        );
        Ok(())
    }

    #[test]
    fn collections_open_then_open_the_selected_paper_pdf() {
        let paper = LibraryPaper {
            id: 9,
            title: "Collected paper".into(),
            authors: String::new(),
            doi: None,
            pdf_path: Some("/tmp/collected.pdf".into()),
            file_size: None,
            reading_status: "unread".into(),
            is_favorite: false,
        };
        let mut app = App {
            page: Page::Collections,
            content_focused: true,
            collections: vec![CollectionSummary {
                id: 3,
                name: "Review".into(),
                paper_count: 1,
                folder_path: Some("/tmp/Review".into()),
            }],
            ..App::default()
        };
        let open_collection =
            handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(open_collection, Some(UiAction::OpenCollection(3))));

        app.active_collection = app.collections.first().cloned();
        app.collection_papers.push(paper);
        let open_pdf = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            open_pdf,
            Some(UiAction::OpenPdf { paper_id: 9, path })
                if path == std::path::Path::new("/tmp/collected.pdf")
        ));
    }

    #[test]
    fn library_and_collection_papers_toggle_bookmarks() {
        let paper = LibraryPaper {
            id: 19,
            title: "Bookmark me".into(),
            authors: "Researcher".into(),
            doi: None,
            pdf_path: Some("/tmp/bookmark.pdf".into()),
            file_size: None,
            reading_status: "unread".into(),
            is_favorite: false,
        };
        let mut app = App {
            page: Page::Library,
            content_focused: true,
            library: papr_core::LibraryState {
                papers: vec![paper.clone()],
                ..papr_core::LibraryState::default()
            },
            ..App::default()
        };
        let library_action = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE),
        );
        assert!(matches!(
            library_action,
            Some(UiAction::Bookmark(super::PaperTarget::Local(19)))
        ));

        app.page = Page::Collections;
        app.active_collection = Some(CollectionSummary {
            id: 2,
            name: "Reading".into(),
            paper_count: 1,
            folder_path: Some("/tmp/Reading".into()),
        });
        app.collection_papers.push(paper);
        let collection_action = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE),
        );
        assert!(matches!(
            collection_action,
            Some(UiAction::Bookmark(super::PaperTarget::Local(19)))
        ));
    }

    #[test]
    fn bookmarks_can_be_opened_and_removed() {
        let bookmark = BookmarkSummary {
            id: 4,
            paper_id: 23,
            paper_title: "Saved PDF".into(),
            authors: "Researcher".into(),
            year: Some("2026".into()),
            journal: Some("Journal".into()),
            doi: None,
            pdf_path: "/tmp/saved.pdf".into(),
            page: None,
            label: None,
        };
        let mut app = App {
            page: Page::Bookmarks,
            content_focused: true,
            bookmarks: vec![bookmark],
            ..App::default()
        };
        let open = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            open,
            Some(UiAction::OpenPdf { paper_id: 23, path })
                if path == std::path::Path::new("/tmp/saved.pdf")
        ));
        let remove = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('B'), KeyModifiers::NONE),
        );
        assert!(matches!(
            remove,
            Some(UiAction::Bookmark(super::PaperTarget::Local(23)))
        ));
    }

    #[test]
    fn dashboard_paper_selection_interleaves_keyword_results_and_deduplicates() {
        let buckets = vec![
            vec![
                remote_paper("https://arxiv.org/abs/1", "First keyword latest"),
                remote_paper("https://arxiv.org/abs/shared", "Shared paper"),
            ],
            vec![
                remote_paper("https://arxiv.org/abs/2", "Second keyword latest"),
                remote_paper("https://arxiv.org/abs/shared", "Shared paper"),
            ],
        ];

        let selected = diverse_latest_papers(buckets, 10);

        assert_eq!(selected.len(), 3);
        assert_eq!(selected[0].id, "https://arxiv.org/abs/1");
        assert_eq!(selected[1].id, "https://arxiv.org/abs/2");
        assert_eq!(selected[2].id, "https://arxiv.org/abs/shared");
    }

    fn remote_paper(id: &str, title: &str) -> RemotePaper {
        let timestamp = Utc
            .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
            .single()
            .unwrap_or_else(Utc::now);
        RemotePaper {
            id: id.into(),
            title: title.into(),
            authors: vec!["Researcher".into()],
            abstract_text: String::new(),
            published: timestamp,
            updated: timestamp,
            categories: vec!["cs.DL".into()],
            pdf_url: None,
            doi: None,
            journal_ref: None,
        }
    }
}
