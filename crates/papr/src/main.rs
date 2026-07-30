//! `papr` executable entry point.

mod terminal;
mod ui;
mod citation;
mod pdf_viewer;
mod settings_modal;

use std::{
    collections::HashMap,
    io::{BufRead, BufReader},
    path::{Component, Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
    sync::mpsc::{self as std_mpsc, TryRecvError},
    sync::Arc,
};

use anyhow::{Context, Result};
use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use toml;
use chrono::Local;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};
use papr_core::{
    App, AppMode, ArxivClient, CollectionDirectory, Command, Config, Database, DiscoveryStatus,
    DownloadEvent, DownloadManager, DownloadStatus, DownloadTask, ImportedPdf, LibraryIndexer,
    LibraryWatcher, MetadataPrompt, Page, PaperNote, Paths, PluginHost, RemotePaper, Theme, Project,
    CitationSource, CompletionSource, ProjectManager, ProjectPane,
};
use sha2::{Digest, Sha256};
use tokio::{sync::{mpsc, Semaphore}, task::JoinSet};

use terminal::TerminalSession;

const DASHBOARD_CANDIDATE_LIMIT: u16 = 30;
const DASHBOARD_DISPLAY_LIMIT: usize = 10;
const DASHBOARD_FEED_ALGORITHM_VERSION: &str = "balanced-v3";
const METADATA_ENRICHMENT_CONCURRENCY: usize = 1;

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
        println!("projects: {}", paths.projects_dir.display());
        return Ok(());
    }

    let config = Config::load_or_create(&paths).context("failed to load configuration")?;
    let project_manager = ProjectManager::new(config.projects_directory(&paths))
        .context("failed to initialize projects directory")?;
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
    let dashboard_keywords = config.dashboard_keyword_list();
    let dashboard_keyword_signature = dashboard_keyword_signature(&dashboard_keywords);
    let dashboard_feed_date = local_feed_date();
    let arxiv = ArxivClient::new().context("failed to initialize arXiv client")?;
    let (today_sender, today_receiver) = mpsc::unbounded_channel();
    let initial_cached_papers = database.dashboard_feed_cache(
        &dashboard_feed_date,
        &dashboard_keyword_signature,
    )?;
    let initial_dashboard_fetch = if initial_cached_papers.is_none() {
        let key = DashboardFeedKey {
            feed_date: dashboard_feed_date.clone(),
            keyword_signature: dashboard_keyword_signature.clone(),
        };
        start_dashboard_fetch(
            arxiv.clone(),
            dashboard_keywords.clone(),
            key.clone(),
            today_sender.clone(),
        );
        Some(key)
    } else {
        None
    };
    let download_dir = config.download_path.clone().unwrap_or_else(|| paths.downloads_dir.clone());
    std::fs::create_dir_all(&download_dir).context("failed to create download directory")?;
    let download_dir = std::fs::canonicalize(&download_dir).unwrap_or(download_dir);

    let mut collection_roots = Vec::new();
    for root in &config.library_folders {
        collection_roots.push(std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()));
    }
    let mut library_roots = collection_roots.clone();
    let download_inside = collection_roots.iter().any(|root| {
        download_dir.starts_with(root)
    });
    if !download_inside {
        library_roots.push(download_dir.clone());
    }

    let mut dashboard = database
        .research_dashboard()
        .context("failed to load research dashboard")?;
    let (collection_pdf_count, collection_pdf_size) =
        LibraryIndexer::pdf_storage_stats(&collection_roots);
    let (download_pdf_count, download_pdf_size) =
        LibraryIndexer::pdf_storage_stats(&[download_dir.clone()]);
    let library_papers = database.library_papers_in_roots(&library_roots)?;
    dashboard.counts.papers = collection_pdf_count;
    dashboard.counts.downloaded = download_pdf_count;
    dashboard.read = library_papers
        .iter()
        .filter(|p| p.reading_status == "read")
        .count() as u64;
    dashboard.disk_usage = collection_pdf_size;
    dashboard.downloads_size = download_pdf_size;
    dashboard.database_size = std::fs::metadata(&paths.database_file)
        .map(|m| m.len())
        .unwrap_or(0);

    let initial_page = Page::from_config_str(&config.startup_page).unwrap_or(Page::Dashboard);
    let initial_sidebar_index = Page::ALL
        .iter()
        .position(|&p| p == initial_page)
        .unwrap_or(0);

    let mut app = App {
        page: initial_page,
        sidebar_index: initial_sidebar_index,
        stats: dashboard.counts,
        dashboard,
        pdf_viewer: config.pdf_viewer.clone().unwrap_or_else(default_pdf_viewer),
        ..App::default()
    };
    app.plugins = plugin_host.plugins();
    app.plugin_diagnostics = plugin_host.diagnostics().len();
    app.config_editor_text = std::fs::read_to_string(&paths.config_file).unwrap_or_default();
    app.projects = project_manager.list().unwrap_or_default();
    if let Some(papers) = initial_cached_papers {
        app.today_papers = papers;
        app.today_status = DiscoveryStatus::Ready;
    } else {
        app.today_status = DiscoveryStatus::Loading;
    }

    discover_local_downloads(&mut app, &download_dir, &database);

    app.library.papers = library_papers;
    refresh_organization(&database, &library_roots, &mut app)?;
    let (watch_sender, watch_receiver) = mpsc::unbounded_channel();
    let watcher = start_library_watcher(&library_roots, watch_sender.clone())?;

    let downloads = DownloadManager::new().context("failed to initialize download manager")?;
    let mut session = TerminalSession::start()?;
    let primary_library_root = library_roots[0].clone();
    let runtime = Runtime {
        arxiv,
        crossref: papr_core::api::crossref::CrossrefClient::new(),
        openalex: papr_core::api::openalex::OpenAlexClient::new(),
        downloads,
        database,
        database_file: paths.database_file.clone(),
        config_file: paths.config_file.clone(),
        plugins_dir: paths.plugins_dir.clone(),
        plugin_host,
        project_manager,
        project_compiler: None,
        default_downloads_dir: paths.downloads_dir.clone(),
        download_dir,
        pdf_viewer: config.pdf_viewer.clone().unwrap_or_else(default_pdf_viewer),
        primary_library_root,
        library_roots,
        collection_roots,
        dashboard_keywords,
        dashboard_keyword_signature,
        dashboard_feed_date,
        active_dashboard_fetch: initial_dashboard_fetch,
        watch_sender,
        watch_receiver,
        _watcher: watcher,
        active_enrichments: std::collections::HashSet::new(),
        citation_index: None,
        citation_source: CitationSource::default(),
    };
    run(
        &mut session,
        &mut app,
        theme,
        runtime,
        today_sender,
        today_receiver,
    )
    .await
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
    RetryDiscoverMore,
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
    MarkUnread(i64),
    CopyCitation(PaperTarget),
    ConfirmDeletePaper { paper_id: i64, title: String, path: Option<PathBuf> },
    ConfirmDeleteCollection { collection_id: i64, name: String, path: Option<PathBuf> },
    DeletePaper { paper_id: i64, path: Option<PathBuf> },
    DeleteCollection { collection_id: i64, path: Option<PathBuf> },
    AddToQueue(i64),
    RemoveFromQueue(i64),
    MoveQueueItemUp(i64),
    MoveQueueItemDown(i64),
    ClosePdf,
    RetryDownload { id: String, paper: RemotePaper },
    RefreshProjects,
    CreateProject(String),
    CreateProjectFile(String),
    OpenProject(Project),
    OpenProjectFile(PathBuf),
    ConfirmDeleteProjectEntry(PathBuf),
    DeleteProjectEntry(PathBuf),
    RenameProject { project: Project, name: String },
    ConfirmDeleteProject(Project),
    DeleteProject(Project),
    RenameProjectEntry { path: PathBuf, name: String },
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
    request_id: u64,
    update: SearchUpdate,
}

#[derive(Debug)]
enum SearchUpdate {
    Partial { papers: Vec<RemotePaper>, next_start: Option<u16> },
    Retrying { attempt: u8, max_attempts: u8 },
    Complete(Vec<RemotePaper>),
    InitialFailure(String),
    PartialFailure { next_start: u16 },
}

const DISCOVERY_CANDIDATE_LIMIT: u16 = 250;
const DISCOVERY_FETCH_BATCH_SIZE: u16 = 100;
const DISCOVERY_PAGE_RETRY_ATTEMPTS: u8 = 3;
const DISCOVERY_PAGE_RETRY_BASE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

#[derive(Debug)]
struct TodayResponse {
    key: DashboardFeedKey,
    result: Result<Vec<RemotePaper>, String>,
}

/// Identifies one daily feed request.  Responses are accepted only when both
/// the local date and the configured feed algorithm/keywords still match.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DashboardFeedKey {
    feed_date: String,
    keyword_signature: String,
}

pub(crate) struct ConfigEditorView {
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
}

struct Runtime {
    arxiv: ArxivClient,
    crossref: papr_core::api::crossref::CrossrefClient,
    openalex: papr_core::api::openalex::OpenAlexClient,
    downloads: DownloadManager,
    database: Database,
    database_file: PathBuf,
    config_file: PathBuf,
    plugins_dir: PathBuf,
    plugin_host: PluginHost,
    project_manager: ProjectManager,
    project_compiler: Option<ProjectCompiler>,
    default_downloads_dir: PathBuf,
    download_dir: PathBuf,
    pdf_viewer: String,
    primary_library_root: PathBuf,
    library_roots: Vec<PathBuf>,
    collection_roots: Vec<PathBuf>,
    dashboard_keywords: Vec<String>,
    dashboard_keyword_signature: String,
    dashboard_feed_date: String,
    active_dashboard_fetch: Option<DashboardFeedKey>,
    watch_sender: mpsc::UnboundedSender<()>,
    watch_receiver: mpsc::UnboundedReceiver<()>,
    _watcher: LibraryWatcher,
    active_enrichments: std::collections::HashSet<i64>,
    citation_index: Option<CitationIndexer>,
    citation_source: CitationSource,
}

/// Background BibTeX indexer. It watches only the active project and sends a
/// fresh immutable source to the UI thread, keeping typing non-blocking.
struct CitationIndexer {
    events: std_mpsc::Receiver<CitationSource>,
    _watcher: RecommendedWatcher,
}

impl CitationIndexer {
    fn start(project: &Project) -> Result<Self> {
        let (sender, events) = std_mpsc::channel();
        refresh_citation_index(project.path.clone(), sender.clone());
        let root = project.path.clone();
        let mut watcher = RecommendedWatcher::new(move |event: notify::Result<notify::Event>| {
            if let Ok(event) = event
                && event.paths.iter().any(|path| path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("bib")))
            {
                refresh_citation_index(root.clone(), sender.clone());
            }
        }, NotifyConfig::default())?;
        watcher.watch(&project.path, RecursiveMode::Recursive)?;
        Ok(Self { events, _watcher: watcher })
    }

    fn drain(&self) -> Option<CitationSource> {
        let mut newest = None;
        while let Ok(source) = self.events.try_recv() { newest = Some(source); }
        newest
    }
}

fn refresh_citation_index(root: PathBuf, sender: std_mpsc::Sender<CitationSource>) {
    std::thread::spawn(move || {
        let entries = walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext.eq_ignore_ascii_case("bib")))
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .flat_map(|contents| CitationSource::parse_bibtex(&contents)).collect();
        let _ = sender.send(CitationSource::new(entries));
    });
}

fn update_project_completions(app: &mut App, source: Option<&CitationSource>) {
    let items = source.map_or_else(Vec::new, |source| source.complete(&app.project_editor_text, app.project_editor_cursor));
    app.project_completions = items;
    app.project_completion_selected = app.project_completion_selected.min(app.project_completions.len().saturating_sub(1));
}

fn accept_project_completion(app: &mut App) -> bool {
    let Some(item) = app.project_completions.get(app.project_completion_selected).cloned() else { return false; };
    let Some(query) = papr_core::completions::citation_query(&app.project_editor_text, app.project_editor_cursor) else { return false; };
    let start = app.project_editor_cursor.saturating_sub(query.len());
    app.project_editor_text.replace_range(start..app.project_editor_cursor, &item.insert_text);
    app.project_editor_cursor = start + item.insert_text.len();
    app.project_editor_dirty = true;
    app.project_completions.clear();
    true
}

/// Cross-platform lifecycle wrapper for the persistent latexmk watcher.
struct ProjectCompiler {
    project: Project,
    child: std::process::Child,
    events: std_mpsc::Receiver<ProjectBuildEvent>,
    _watcher: RecommendedWatcher,
    pdf_changed: bool,
    build_succeeded: bool,
    stopped: bool,
}

#[derive(Debug)]
enum ProjectBuildEvent {
    PdfChanged,
    Succeeded,
    Failed(String),
}

impl ProjectCompiler {
    fn start(project: Project) -> Result<Self> {
        let (sender, events) = std_mpsc::channel();
        let watch_sender = sender.clone();
        let mut watcher = RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| {
                if let Ok(event) = event
                    // The watcher is non-recursive and scoped to this project;
                    // matching the basename tolerates canonical-path differences
                    // reported by platform backends.
                    && event.paths.iter().any(|path| path.file_name().is_some_and(|name| name == "main.pdf"))
                {
                    let _ = watch_sender.send(ProjectBuildEvent::PdfChanged);
                }
            },
            NotifyConfig::default(),
        )?;
        watcher.watch(&project.path, RecursiveMode::NonRecursive)?;

        let mut command = ProcessCommand::new("latexmk");
        command
            .args(["-pdf", "-pvc", "-view=none", "main.tex"])
            .current_dir(&project.path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            // A dedicated group lets teardown include latexmk's active TeX
            // subprocesses, which otherwise retain the captured output pipes.
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .context("latexmk is not available; install a TeX distribution and latexmk")?;
        if let Some(stdout) = child.stdout.take() {
            spawn_latexmk_reader(stdout, sender.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_latexmk_reader(stderr, sender.clone());
        }
        Ok(Self {
            project,
            child,
            events,
            _watcher: watcher,
            pdf_changed: false,
            build_succeeded: false,
            stopped: false,
        })
    }

    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        #[cfg(unix)]
        {
            let process_group = format!("-{}", self.child.id());
            let _ = ProcessCommand::new("kill")
                .args(["-KILL", "--", &process_group])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Consume build and filesystem events. No filesystem metadata is polled.
    fn drain_events(&mut self, app: &mut App) -> bool {
        let mut changed = false;
        loop {
            match self.events.try_recv() {
                Ok(ProjectBuildEvent::PdfChanged) => self.pdf_changed = true,
                Ok(ProjectBuildEvent::Succeeded) => self.build_succeeded = true,
                Ok(ProjectBuildEvent::Failed(error)) => {
                    self.build_succeeded = false;
                    self.pdf_changed = false;
                    app.project_build_status = "Build failed (editing continues)".into();
                    app.project_build_errors = vec![error];
                    changed = true;
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if self.pdf_changed && self.build_succeeded {
            self.pdf_changed = false;
            self.build_succeeded = false;
            let pdf = self.project.path.join("main.pdf");
            if pdf.exists() {
                app.project_build_status = "Built successfully".into();
                app.project_build_errors.clear();
                let page = app.pdf_viewer_page;
                app.pdf_viewer_total_pages = get_pdf_page_count(&pdf);
                app.pdf_viewer_page = page.min(app.pdf_viewer_total_pages.max(1));
                if app.pdf_viewer_path.as_deref() == Some(pdf.as_path()) {
                    pdf_viewer::invalidate_document(&pdf);
                } else {
                    pdf_viewer::reset_for_new_document(&pdf);
                    app.pdf_viewer_path = Some(pdf.clone());
                    if app.pdf_viewer != "internal" {
                        let viewer = app.pdf_viewer.clone();
                        let _ = open_pdf(&viewer, &pdf, app, None, None);
                    }
                }
                changed = true;
            }
        }
        changed
    }
}

impl Drop for ProjectCompiler {
    fn drop(&mut self) {
        self.stop();
    }
}

fn spawn_latexmk_reader<R: std::io::Read + Send + 'static>(reader: R, sender: std_mpsc::Sender<ProjectBuildEvent>) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let normalized = line.to_ascii_lowercase();
            if normalized.contains("all targets") && normalized.contains("up-to-date") {
                let _ = sender.send(ProjectBuildEvent::Succeeded);
            } else if normalized.contains("errors, so i did not complete") {
                let _ = sender.send(ProjectBuildEvent::Failed(line));
            }
        }
    });
}

fn config_editor_wrap_rows(char_len: usize, wrap_width: usize) -> usize {
    if char_len == 0 {
        1
    } else {
        char_len.div_ceil(wrap_width.max(1))
    }
}

fn open_project_workspace(app: &mut App, project: Project) {
    app.project_tree_dir = Some(project.path.clone());
    app.project_files = project_tree_entries(&project.path);
    app.project_file_selected = 0;
    app.project_editor_path = None;
    app.project_editor_text.clear();
    app.project_editor_dirty = false;
    app.project_editor_cursor = 0;
    app.project_editor_insert_mode = false;
    app.project_editor_scroll = 0;
    app.project_build_status = "Starting latexmk…".into();
    app.project_build_errors.clear();
    app.project_build_scroll = 0;
    app.project_pane = ProjectPane::FileTree;
    app.active_project = Some(project);
    if let Some(project) = &app.active_project {
        let pdf = project.path.join("main.pdf");
        if pdf.exists() {
            pdf_viewer::reset_for_new_document(&pdf);
            app.pdf_viewer_path = Some(pdf.clone());
            app.pdf_viewer_total_pages = get_pdf_page_count(&pdf);
            app.pdf_viewer_page = 1;
            app.pdf_viewer_scroll_y = 0;
            if app.pdf_viewer != "internal" {
                let viewer = app.pdf_viewer.clone();
                let _ = open_pdf(&viewer, &pdf, app, None, None);
            }
        }
    }
    if let Some(main) = app.project_files.iter().find(|p| p.file_name().is_some_and(|n| n == "main.tex")).cloned() {
        if let Ok(text) = std::fs::read_to_string(&main) {
            app.project_editor_text = text;
            app.project_editor_path = Some(main);
            app.project_editor_cursor = 0;
        }
    }
}

fn start_project_compiler(runtime: &mut Runtime, app: &mut App) {
    if let Some(mut compiler) = runtime.project_compiler.take() { compiler.stop(); }
    let Some(project) = app.active_project.clone() else { return; };
    match ProjectCompiler::start(project) {
        Ok(compiler) => runtime.project_compiler = Some(compiler),
        Err(error) => {
            app.project_build_status = "Compiler unavailable".into();
            app.project_build_errors = vec![error.to_string()];
        }
    }
}

fn start_citation_indexer(runtime: &mut Runtime, app: &mut App) {
    runtime.citation_index = app.active_project.as_ref().and_then(|project| CitationIndexer::start(project).ok());
    runtime.citation_source = CitationSource::default();
    app.project_completions.clear();
    app.project_completion_selected = 0;
}

fn project_tree_entries(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else { return Vec::new(); };
    let mut entries = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name() != ".git")
        .filter_map(|entry| {
            let path = entry.path();
            (path.is_dir() || is_project_tree_file(&path)).then_some(path)
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right.is_dir().cmp(&left.is_dir()).then_with(|| left.file_name().cmp(&right.file_name()))
    });
    entries
}

fn is_project_text_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| matches!(ext.to_str(), Some("tex" | "bib" | "sty" | "cls" | "md" | "txt")))
}

fn is_project_tree_file(path: &Path) -> bool {
    is_project_text_file(path) || image::ImageFormat::from_path(path).is_ok()
}

fn create_project_file(project_root: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    let create_directory = name.ends_with('/');
    let relative = Path::new(name.trim_end_matches('/'));
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.file_name().is_none()
        || !relative.components().all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("enter a relative file path inside the project".into());
    }

    let path = project_root.join(relative);
    let parent = path.parent().ok_or_else(|| "enter a relative file path inside the project".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;

    let canonical_root = std::fs::canonicalize(project_root).map_err(|error| error.to_string())?;
    let canonical_parent = std::fs::canonicalize(parent).map_err(|error| error.to_string())?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err("file path must stay inside the project".into());
    }

    if create_directory {
        return std::fs::create_dir(&path)
            .map(|()| path)
            .map_err(|error| if error.kind() == std::io::ErrorKind::AlreadyExists {
                "already exists".into()
            } else { error.to_string() });
    }
    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
        Ok(_) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err("a file with that name already exists".into())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn open_project_file(app: &mut App, path: PathBuf) {
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            app.project_editor_text = text;
            app.project_editor_path = Some(path);
            app.project_editor_dirty = false;
            app.project_editor_cursor = 0;
            app.project_editor_insert_mode = false;
            app.project_editor_scroll = 0;
            app.project_pane = ProjectPane::Editor;
        }
        Err(error) => app.toast = Some(format!("Could not open file: {error}")),
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

fn prev_word_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let mut pos = cursor.min(text.len());
    
    // First, skip any whitespace/newlines to the left
    while pos > 0 {
        let prev = prev_char_boundary(text, pos);
        let ch = text[prev..pos].chars().next().unwrap_or(' ');
        if !ch.is_whitespace() {
            break;
        }
        pos = prev;
    }
    
    if pos == 0 {
        return 0;
    }
    
    // Now determine the type of character we are on
    let prev = prev_char_boundary(text, pos);
    let first_ch = text[prev..pos].chars().next().unwrap_or(' ');
    let is_word_char = first_ch.is_alphanumeric() || first_ch == '_';
    
    // Skip characters of the same type
    while pos > 0 {
        let prev = prev_char_boundary(text, pos);
        let ch = text[prev..pos].chars().next().unwrap_or(' ');
        if ch.is_whitespace() {
            break;
        }
        let ch_is_word = ch.is_alphanumeric() || ch == '_';
        if ch_is_word != is_word_char {
            break;
        }
        pos = prev;
    }
    
    pos
}

fn next_word_boundary(text: &str, cursor: usize) -> usize {
    let mut pos = cursor.min(text.len());
    if pos >= text.len() {
        return text.len();
    }
    
    // Determine the type of character at the cursor
    let first_ch = text[pos..].chars().next().unwrap_or(' ');
    
    if first_ch.is_whitespace() {
        // Skip whitespace/newlines
        while pos < text.len() {
            let next = next_char_boundary(text, pos);
            let ch = text[pos..next].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = next;
        }
    } else {
        let is_word_char = first_ch.is_alphanumeric() || first_ch == '_';
        // Skip characters of the same type
        while pos < text.len() {
            let next = next_char_boundary(text, pos);
            let ch = text[pos..next].chars().next().unwrap_or(' ');
            if ch.is_whitespace() {
                break;
            }
            let ch_is_word = ch.is_alphanumeric() || ch == '_';
            if ch_is_word != is_word_char {
                break;
            }
            pos = next;
        }
        // Then, skip trailing whitespace
        while pos < text.len() {
            let next = next_char_boundary(text, pos);
            let ch = text[pos..next].chars().next().unwrap_or(' ');
            if !ch.is_whitespace() {
                break;
            }
            pos = next;
        }
    }
    
    pos
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

#[allow(dead_code)]
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

fn move_config_editor_page(app: &mut App, direction: isize) {
    move_config_editor_vertical(
        app,
        direction * app.config_editor_viewport_height.max(1) as isize,
    );
}

fn reset_config_editor_goal_column(app: &mut App) {
    app.config_editor_goal_column = None;
}

/// Replaces every mutable editor state with the supplied on-disk buffer.
fn reset_config_editor_buffer(app: &mut App, text: String) {
    app.config_editor_text = text;
    app.config_editor_cursor = 0;
    app.config_editor_scroll = 0;
    app.config_editor_history = vec![app.config_editor_text.clone()];
    app.config_editor_history_idx = 0;
    app.config_editor_command = None;
    app.config_editor_insert_mode = false;
    app.config_editor_error = None;
    reset_config_editor_goal_column(app);
}

/// Reloads the configuration editor from its authoritative source on disk.
fn reload_config_editor_buffer(app: &mut App, config_file: &std::path::Path) {
    match std::fs::read_to_string(config_file) {
        Ok(text) => reset_config_editor_buffer(app, text),
        Err(error) => {
            reset_config_editor_buffer(app, String::new());
            app.config_editor_error = Some(format!("Could not reload configuration: {error}"));
        }
    }
}

pub(crate) fn build_config_editor_view(
    text: &str,
    cursor: usize,
    wrap_width: usize,
    viewport_height: usize,
    scroll: &mut usize,
) -> ConfigEditorView {
    let wrap_width = wrap_width.max(1);
    let (display_text, display_cursor) = expand_tabs_for_editor_view(text, cursor, 4);
    let (cursor_row, cursor_col) = cursor_visual_position(&display_text, display_cursor, wrap_width);

    if cursor_row < *scroll {
        *scroll = cursor_row;
    } else if viewport_height > 0 && cursor_row >= *scroll + viewport_height {
        *scroll = cursor_row - viewport_height + 1;
    }

    let mut lines = Vec::new();
    for (line_idx, line) in display_text.split('\n').enumerate() {
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

/// Expand stored tab characters only for display. The buffer remains byte-for-
/// byte unchanged while cursor geometry and wrapping use terminal cell widths.
fn expand_tabs_for_editor_view(text: &str, cursor: usize, tab_width: usize) -> (String, usize) {
    let tab_width = tab_width.max(1);
    let cursor = cursor.min(text.len());
    let mut display = String::with_capacity(text.len());
    let mut display_cursor = 0;
    let mut column = 0_usize;
    for (byte_index, character) in text.char_indices() {
        if byte_index == cursor {
            display_cursor = display.len();
        }
        match character {
            '\t' => {
                let spaces = tab_width - (column % tab_width);
                display.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            '\n' => {
                display.push(character);
                column = 0;
            }
            _ => {
                display.push(character);
                column += 1;
            }
        }
    }
    if cursor == text.len() {
        display_cursor = display.len();
    }
    (display, display_cursor)
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
enum EnrichmentOutcome {
    Success(RemotePaper),
    Journal(String),
    Failed,
    Unavailable,
}

#[derive(Debug)]
struct MetadataEnrichment {
    paper_id: i64,
    outcome: EnrichmentOutcome,
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
    #[allow(dead_code)]
    File(Result<ImportedPdf, String>),
}

async fn run(
    session: &mut TerminalSession,
    app: &mut App,
    mut theme: Theme,
    mut runtime: Runtime,
    today_sender: mpsc::UnboundedSender<TodayResponse>,
    mut today_receiver: mpsc::UnboundedReceiver<TodayResponse>,
) -> Result<()> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<SearchResponse>();
    let (index_sender, mut index_receiver) = mpsc::unbounded_channel::<IndexResponse>();
    let (download_sender, mut download_receiver) = mpsc::unbounded_channel::<DownloadEvent>();
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
    start_runtime_scan(&runtime, &senders, app);
    let mut last_date_check = std::time::Instant::now();
    let mut last_enrichment_check = std::time::Instant::now();
    let mut last_page: Option<Page> = None;
    let mut last_toast = None;
    let mut last_pdf_page_cached = false;
    let mut force_redraw = true;

    while !app.should_quit {
        // state_changed drives non-PDF redraws; force_redraw (for PDF/animation)
        // is consumed and reset at the bottom of the loop after drawing.
        let mut state_changed = force_redraw;

        if let Some(compiler) = runtime.project_compiler.as_mut() {
            let previous = (app.project_build_status.clone(), app.project_build_errors.clone());
            let preview_changed = compiler.drain_events(app);
            if preview_changed || previous.0 != app.project_build_status || previous.1 != app.project_build_errors { state_changed = true; }
        }
        if let Some(indexer) = runtime.citation_index.as_ref()
            && let Some(source) = indexer.drain()
        {
            runtime.citation_source = source;
            update_project_completions(app, Some(&runtime.citation_source));
            state_changed = true;
        }

        let project_preview_active = app.page == papr_core::Page::Projects
            && app.active_project.is_some()
            && app.pdf_viewer_path.is_some();
        if app.mode == AppMode::PdfView || project_preview_active {
            let pdf_page_cached = pdf_viewer::is_current_page_cached(app);
            if pdf_page_cached != last_pdf_page_cached {
                state_changed = true;
                last_pdf_page_cached = pdf_page_cached;
            }
        }

        while let Ok(TodayResponse { key, result }) = today_receiver.try_recv() {
            state_changed = true;
            if key.feed_date != runtime.dashboard_feed_date
                || key.keyword_signature != runtime.dashboard_keyword_signature
            {
                continue;
            }
            runtime.active_dashboard_fetch = None;
            match result {
                Ok(papers) => {
                    runtime.database.save_dashboard_feed_cache(
                        &key.feed_date,
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
                refresh_dashboard_papers(&mut runtime, &senders, app)?;
                state_changed = true;
            }
        }
        if last_enrichment_check.elapsed() >= std::time::Duration::from_secs(300) {
            last_enrichment_check = std::time::Instant::now();
            spawn_enrichment_if_needed(&mut runtime, &senders, app)?;
        }
        while let Ok(response) = receiver.try_recv() {
            state_changed = true;
            if response.query == app.discovery.query && response.request_id == app.discovery.request_id {
                match response.update {
                    SearchUpdate::Partial { papers, next_start } => {
                        app.discovery.update_results(papers);
                        app.discovery.next_batch_start = next_start;
                        app.discovery.progress_message = next_start
                            .map(|_| "Loading more results...".to_owned());
                        app.discovery.status = DiscoveryStatus::Loading;
                    }
                    SearchUpdate::Retrying { attempt, max_attempts } => {
                        app.discovery.progress_message = Some(format!(
                            "Retrying more results ({attempt}/{max_attempts})..."
                        ));
                        app.discovery.status = DiscoveryStatus::Loading;
                    }
                    SearchUpdate::Complete(papers) => {
                        app.discovery.update_results(papers);
                        app.discovery.next_batch_start = None;
                        app.discovery.progress_message = None;
                        app.discovery.status = DiscoveryStatus::Ready;
                    }
                    SearchUpdate::InitialFailure(error) => {
                        app.discovery.status = DiscoveryStatus::Error(error);
                    }
                    SearchUpdate::PartialFailure { next_start } => {
                        app.discovery.next_batch_start = Some(next_start);
                        app.discovery.progress_message = Some(
                            "More results could not be loaded. Press r to retry.".to_owned(),
                        );
                        app.discovery.status = DiscoveryStatus::Ready;
                    }
                }
            }
        }
        let has_active_downloads = app.downloads.iter().any(|task| {
            !matches!(task.status, DownloadStatus::Completed | DownloadStatus::Failed(_))
        });
        while runtime.watch_receiver.try_recv().is_ok() {
            state_changed = true;
            if !has_active_downloads {
                start_silent_runtime_scan(&runtime, &senders, app);
            }
        }
        while let Ok(response) = index_receiver.try_recv() {
            state_changed = true;
            apply_index_response(response, &mut runtime, &senders, app).await?;
        }
        let mut any_enrichment_processed = false;
        while let Ok(MetadataEnrichment { paper_id, outcome }) = enrichment_receiver.try_recv() {
            match outcome {
                EnrichmentOutcome::Success(ref p) => {
                    runtime.database.apply_arxiv_metadata(paper_id, p)?;
                    // Update in-memory today_papers
                    for paper in &mut app.today_papers {
                        if paper.id == p.id || (p.doi.is_some() && paper.doi == p.doi) {
                            *paper = merge_enriched_remote_paper(paper, p);
                        }
                    }
                    // Update in-memory discovery results
                    for paper in &mut app.discovery.results {
                        if paper.id == p.id || (p.doi.is_some() && paper.doi == p.doi) {
                            *paper = merge_enriched_remote_paper(paper, p);
                        }
                    }
                    // Save updated today_papers to SQLite dashboard feed cache
                    let _ = runtime.database.save_dashboard_feed_cache(
                        &runtime.dashboard_feed_date,
                        &runtime.dashboard_keyword_signature,
                        &app.today_papers,
                    );
                }
                EnrichmentOutcome::Journal(ref journal) => {
                    runtime.database.apply_journal_metadata(paper_id, journal)?;
                }
                EnrichmentOutcome::Failed => {
                    runtime.database.update_enrichment_status(paper_id, "failed")?;
                }
                EnrichmentOutcome::Unavailable => {
                    runtime.database.update_enrichment_status(paper_id, "unavailable")?;
                }
            }
            runtime.active_enrichments.remove(&paper_id);
            if let Some(task) = app.downloads.iter_mut().find(|t| t.paper_id == Some(paper_id)) {
                task.status = DownloadStatus::Completed;
                finalize_download_task(task);
            }
            any_enrichment_processed = true;
            state_changed = true;
        }
        if any_enrichment_processed {
            refresh_library(&runtime, app)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            refresh_dashboard(&mut runtime, app)?;
            refresh_downloads(&runtime, app);
        }
        if enrichment_receiver.is_empty() && app.enrichment_pending {
            app.enrichment_pending = false;
            state_changed = true;
        }
        while let Ok(event) = download_receiver.try_recv() {
            state_changed = true;
            apply_download_event(
                event,
                &mut pending_downloads,
                &mut runtime,
                app,
                &senders,
            ).await?;
        }
        while let Ok(event) = app_events_receiver.try_recv() {
            state_changed = true;
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
        if Some(app.page) != last_page {
            app.workspace_query.clear();
            app.workspace_query_cursor = 0;
            if last_page == Some(papr_core::Page::Settings) {
                let original = app.settings_modal.original_theme.clone();
                if !original.is_empty() && theme.name != original {
                    if let Ok(reverted) = Theme::load(&original) {
                        theme = reverted;
                    }
                }
            }
            if matches!(
                app.page,
                papr_core::Page::Dashboard | papr_core::Page::History | papr_core::Page::Statistics
            ) {
                refresh_dashboard(&runtime, app)?;
            }
            // Auto-open the settings workspace whenever the user navigates to the
            // Settings page. Read the config fresh from disk so the workspace
            // always reflects the persisted state.
            if app.page == papr_core::Page::Settings {
                if let Ok(config) = Config::load_or_create(&Paths::discover()?) {
                    settings_modal::open_settings_modal(app, &config, &theme.name);
                }
            }
            last_page = Some(app.page);
            state_changed = true;
        }
        if app.toast.is_some() {
            if app.toast != last_toast {
                app.toast_timestamp = Some(std::time::Instant::now());
                last_toast = app.toast.clone();
                state_changed = true;
            }
            if let Some(ts) = app.toast_timestamp {
                if ts.elapsed() >= std::time::Duration::from_secs(7) {
                    app.toast = None;
                    app.toast_timestamp = None;
                    last_toast = None;
                    state_changed = true;
                }
            }
        } else {
            if last_toast.is_some() {
                state_changed = true;
            }
            app.toast_timestamp = None;
            last_toast = None;
        }

        // Auto-cleanup failed downloads after 2 minutes (120 seconds)
        let before_len = app.downloads.len();
        app.downloads.retain(|task| {
            if let DownloadStatus::Failed(_) = task.status {
                if let Some(failed_at) = task.failed_at {
                    if failed_at.elapsed() >= std::time::Duration::from_secs(120) {
                        return false;
                    }
                }
            }
            true
        });
        if app.downloads.len() != before_len {
            state_changed = true;
            app.download_selected = app.download_selected.min(app.downloads.len().saturating_sub(1));
        }

        // ── FIXED EVENT ORDERING ─────────────────────────────────────────────
        // Read all pending events FIRST, THEN draw.  Previously the loop drew
        // state before reading keys, adding one full iteration of input latency.
        // Now: drain pending events → block-wait → draw updated state.

        // Use a dynamic timeout based on cache and animation status.
        // Only call is_animating() once per iteration (it acquires a mutex).
        let preview_active = app.mode == AppMode::PdfView
            || (app.page == papr_core::Page::Projects
                && app.active_project.is_some()
                && app.pdf_viewer_path.is_some());
        let animating = preview_active && pdf_viewer::is_animating();
        let poll_timeout = if preview_active {
            if animating {
                std::time::Duration::from_millis(8)  // ~120fps cap while animating
            } else if pdf_viewer::is_current_page_cached(app) {
                std::time::Duration::from_millis(250)
            } else {
                std::time::Duration::from_millis(50)
            }
        } else {
            std::time::Duration::from_millis(100)
        };

        // Step 1: drain all immediately-available events so we fold multiple
        // key-repeat events into a single physics update before drawing.
        let mut got_event = false;
        while event::poll(std::time::Duration::ZERO)? {
            got_event = true;
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press || (app.mode == AppMode::PdfView && key.kind == KeyEventKind::Repeat) =>
                {
                    if app.page == papr_core::Page::Settings && app.content_focused && app.mode == AppMode::Normal {
                        if let Some(action) = handle_settings_modal_key(app, key, &mut runtime, &mut theme, &senders)? {
                            apply_ui_action(
                                action,
                                &mut runtime,
                                &senders,
                                &mut pending_downloads,
                                app,
                                &mut theme,
                            ).await?;
                        }
                    } else if let Some(action) = handle_key(app, key) {
                        apply_ui_action(
                            action,
                            &mut runtime,
                            &senders,
                            &mut pending_downloads,
                            app,
                            &mut theme,
                        ).await?;
                    }
                }
                Event::Mouse(mouse) => {
                    if let Some(action) = handle_mouse(app, mouse) {
                        apply_ui_action(
                            action,
                            &mut runtime,
                            &senders,
                            &mut pending_downloads,
                            app,
                            &mut theme,
                        ).await?;
                    }
                }
                Event::Paste(text) => {
                    handle_project_bibtex_paste(app, &text);
                }
                _ => {}
            }
        }

        // Step 2: block-wait for the next event (up to poll_timeout).
        if event::poll(poll_timeout)? {
            got_event = true;
            match event::read()? {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press || (app.mode == AppMode::PdfView && key.kind == KeyEventKind::Repeat) =>
                {
                    if app.page == papr_core::Page::Settings && app.content_focused && app.mode == AppMode::Normal {
                        if let Some(action) = handle_settings_modal_key(app, key, &mut runtime, &mut theme, &senders)? {
                            apply_ui_action(
                                action,
                                &mut runtime,
                                &senders,
                                &mut pending_downloads,
                                app,
                                &mut theme,
                            ).await?;
                        }
                    } else if let Some(action) = handle_key(app, key) {
                        apply_ui_action(
                            action,
                            &mut runtime,
                            &senders,
                            &mut pending_downloads,
                            app,
                            &mut theme,
                        ).await?;
                    }
                }
                Event::Mouse(mouse) => {
                    if let Some(action) = handle_mouse(app, mouse) {
                        apply_ui_action(
                            action,
                            &mut runtime,
                            &senders,
                            &mut pending_downloads,
                            app,
                            &mut theme,
                        ).await?;
                    }
                }
                Event::Paste(text) => {
                    handle_project_bibtex_paste(app, &text);
                }
                _ => {}
            }
        }

        if got_event || animating {
            force_redraw = true;
        }
        if got_event && app.page == papr_core::Page::Projects {
            let previous = app.project_completions.clone();
            update_project_completions(app, Some(&runtime.citation_source));
            state_changed |= previous != app.project_completions;
        }

        // Step 3: draw with the fully-updated state (key effects are visible
        // in THIS iteration, not the next one).
        if state_changed || force_redraw {
            force_redraw = false;  // consumed here
            let draw_start = std::time::Instant::now();
            session
                .terminal_mut()
                .draw(|frame| ui::render(frame, app, &theme))?;
            if draw_start.elapsed() > std::time::Duration::from_millis(16) {
                log_message(&runtime.database_file, &format!("Slow draw: {:?}", draw_start.elapsed()));
            }
        }

        tokio::task::yield_now().await;
    }
    if let Some(mut compiler) = runtime.project_compiler.take() { compiler.stop(); }
    pdf_viewer::cleanup_temp_files();
    Ok(())
}

async fn fetch_discovery_pages(
    client: ArxivClient,
    query: String,
    request_id: u64,
    mut papers: Vec<RemotePaper>,
    mut start: u16,
    response_sender: mpsc::UnboundedSender<SearchResponse>,
) {
    loop {
        let mut page = None;
        for retry in 0..=DISCOVERY_PAGE_RETRY_ATTEMPTS {
            match client
                .search_ranked_candidate_page(
                    &query,
                    &papers,
                    start,
                    DISCOVERY_CANDIDATE_LIMIT,
                    DISCOVERY_FETCH_BATCH_SIZE,
                )
                .await
            {
                Ok(result) => {
                    page = Some(result);
                    break;
                }
                Err(error) if retry == DISCOVERY_PAGE_RETRY_ATTEMPTS => {
                    let update = if papers.is_empty() {
                        SearchUpdate::InitialFailure(error.to_string())
                    } else {
                        SearchUpdate::PartialFailure { next_start: start }
                    };
                    let _ = response_sender.send(SearchResponse {
                        query,
                        request_id,
                        update,
                    });
                    return;
                }
                Err(_) => {
                    let attempt = retry + 1;
                    let _ = response_sender.send(SearchResponse {
                        query: query.clone(),
                        request_id,
                        update: SearchUpdate::Retrying {
                            attempt,
                            max_attempts: DISCOVERY_PAGE_RETRY_ATTEMPTS,
                        },
                    });
                    let multiplier = 1_u32 << u32::from(retry);
                    tokio::time::sleep(DISCOVERY_PAGE_RETRY_BASE_DELAY * multiplier).await;
                }
            }
        }

        let Some(page) = page else {
            return;
        };
        papers = page.papers;
        match page.next_start {
            Some(next_start) => {
                let _ = response_sender.send(SearchResponse {
                    query: query.clone(),
                    request_id,
                    update: SearchUpdate::Partial {
                        papers: papers.clone(),
                        next_start: Some(next_start),
                    },
                });
                start = next_start;
            }
            None => {
                let _ = response_sender.send(SearchResponse {
                    query,
                    request_id,
                    update: SearchUpdate::Complete(papers),
                });
                return;
            }
        }
    }
}

async fn apply_ui_action(
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
            refresh_dashboard(runtime, app)?;
            let request_id = app.discovery.begin_search();
            let client = runtime.arxiv.clone();
            let response_sender = senders.search.clone();
            tokio::spawn(async move {
                fetch_discovery_pages(client, query, request_id, Vec::new(), 0, response_sender).await;
            });
        }
        UiAction::RetryDiscoverMore => {
            let Some(start) = app.discovery.next_batch_start else {
                return Ok(());
            };
            app.discovery.status = DiscoveryStatus::Loading;
            app.discovery.progress_message = Some("Loading more results...".to_owned());
            let client = runtime.arxiv.clone();
            let response_sender = senders.search.clone();
            let query = app.discovery.query.clone();
            let request_id = app.discovery.request_id;
            let papers = app.discovery.results.clone();
            tokio::spawn(async move {
                fetch_discovery_pages(client, query, request_id, papers, start, response_sender).await;
            });
        }
        UiAction::OpenPaper(paper) => {
            let paper_id = runtime.database.ensure_remote_paper(&paper)?;
            runtime.database.record_activity("paper_browsed", Some(paper_id), None)?;
            dispatch_plugin_events(runtime, app, &["paper_opened"], paper_id).await?;
            refresh_dashboard(runtime, app)?;
            app.mode = AppMode::PaperDetail;
            app.paper_detail_scroll = 0;
            refresh_paper_views(runtime, app)?;
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
        UiAction::RetryDownload { id, paper } => {
            if pending_downloads.contains_key(&id) {
                return Ok(());
            }
            if let Some(task) = app.downloads.iter_mut().find(|t| t.id == id) {
                if let Some(ref path_str) = task.pdf_path {
                    let path = std::path::PathBuf::from(path_str);
                    let part_path = path.with_extension("pdf.part");
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(&part_path);
                }
                task.downloaded = 0;
                task.total = None;
                task.status = DownloadStatus::Starting;
                task.failed_at = None;
                let destination = task.pdf_path.as_ref()
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|| runtime.download_dir.join(format!("{}.pdf", id)));
                pending_downloads.insert(id.clone(), paper.clone());
                app.toast = Some("Retrying download...".to_owned());
                let manager = runtime.downloads.clone();
                let events = senders.download.clone();
                tokio::spawn(async move {
                    if let Err(error) = manager.download(&id, &paper.pdf_url.clone().unwrap_or_default(), &destination, &events).await {
                        let _ = events.send(DownloadEvent::Failed {
                            id,
                            error: error.to_string(),
                        });
                    }
                });
            }
        }
        UiAction::Reindex => start_runtime_scan(runtime, senders, app),
        UiAction::RefreshProjects => {
            app.projects = runtime.project_manager.list().map_err(|e| anyhow::anyhow!(e))?;
            app.projects_selected = app.projects_selected.min(app.projects.len().saturating_sub(1));
        }
        UiAction::CreateProject(name) => {
            let name = name.trim();
            let project = match runtime.project_manager.create(name) {
                Ok(project) => project,
                Err(error) => {
                    app.toast = Some(format!("Could not create project: {error}"));
                    return Ok(());
                }
            };
            runtime.database.record_project_activity("project_created", &project.name)?;
            runtime.database.record_project_activity("project_opened", &project.name)?;
            refresh_dashboard(runtime, app)?;
            open_project_workspace(app, project.clone());
            start_project_compiler(runtime, app);
            start_citation_indexer(runtime, app);
            app.projects = runtime.project_manager.list().unwrap_or_default();
            app.projects_selected = app
                .projects
                .iter()
                .position(|candidate| candidate.path == project.path)
                .unwrap_or(0);
            app.toast = Some(format!("Created project {}", project.name));
        }
        UiAction::CreateProjectFile(name) => {
            let Some(project) = app.active_project.as_ref() else {
                app.toast = Some("Could not create file: no project is open.".into());
                return Ok(());
            };
            let tree_dir = app.project_tree_dir.clone().unwrap_or_else(|| project.path.clone());
            let path = match create_project_file(&tree_dir, &name) {
                Ok(path) => path,
                Err(error) => {
                    app.toast = Some(format!("Could not create file: {error}"));
                    return Ok(());
                }
            };
            app.project_files = project_tree_entries(&tree_dir);
            app.project_file_selected = app.project_files.iter().position(|candidate| candidate == &path).unwrap_or(0);
            app.project_pane = ProjectPane::FileTree;
            if path.is_file() && is_project_text_file(&path) {
                open_project_file(app, path);
            }
            app.toast = Some(if name.trim().ends_with('/') { "Created folder" } else { "Created file" }.into());
        }
        UiAction::OpenProject(project) => {
            let project = runtime.project_manager.open(project.path).map_err(|e| anyhow::anyhow!(e))?;
            runtime.database.record_project_activity("project_opened", &project.name)?;
            refresh_dashboard(runtime, app)?;
            open_project_workspace(app, project);
            start_project_compiler(runtime, app);
            start_citation_indexer(runtime, app);
        }
        UiAction::OpenProjectFile(path) => {
            if app.project_editor_dirty && !save_project_editor(app) {
                return Ok(());
            }
            open_project_file(app, path);
        }
        UiAction::RenameProjectEntry { path, name } => {
            let Some(project) = app.active_project.as_ref() else {
                app.toast = Some("Could not rename entry: no project is open.".into());
                return Ok(());
            };
            let project_root = project.path.clone();
            let name = name.trim();
            if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
                app.toast = Some("Could not rename entry: enter a single file or folder name.".into());
                return Ok(());
            }
            let Some(parent) = path.parent() else {
                app.toast = Some("Could not rename entry: invalid path.".into());
                return Ok(());
            };
            if !path.starts_with(&project_root) {
                app.toast = Some("Could not rename entry outside the project.".into());
                return Ok(());
            }
            let renamed = parent.join(name);
            if let Err(error) = std::fs::rename(&path, &renamed) {
                app.toast = Some(format!("Could not rename entry: {error}"));
                return Ok(());
            }
            if let Some(editor_path) = &app.project_editor_path
                && let Ok(relative) = editor_path.strip_prefix(&path)
            {
                app.project_editor_path = Some(renamed.join(relative));
            } else if app.project_editor_path.as_ref() == Some(&path) {
                app.project_editor_path = Some(renamed.clone());
            }
            let tree_dir = app.project_tree_dir.clone().unwrap_or(project_root);
            app.project_files = project_tree_entries(&tree_dir);
            app.project_file_selected = app.project_files.iter().position(|entry| entry == &renamed).unwrap_or(0);
            app.toast = Some("Renamed".into());
        }
        UiAction::ConfirmDeleteProjectEntry(path) => {
            let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("entry").to_owned();
            app.delete_confirmation = Some(papr_core::DeletionTarget::ProjectEntry {
                is_directory: path.is_dir(),
                path,
                name,
            });
            app.mode = AppMode::ConfirmDelete;
        }
        UiAction::DeleteProjectEntry(path) => {
            let Some(project) = app.active_project.as_ref() else {
                app.toast = Some("Could not delete entry: no project is open.".into());
                return Ok(());
            };
            if path == project.path || !path.starts_with(&project.path) {
                app.toast = Some("Could not delete entry outside the project.".into());
                return Ok(());
            }
            let result = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
            if let Err(error) = result {
                app.toast = Some(format!("Could not delete entry: {error}"));
                return Ok(());
            }
            let tree_dir = app.project_tree_dir.clone().unwrap_or_else(|| project.path.clone());
            app.project_files = project_tree_entries(&tree_dir);
            app.project_file_selected = app.project_file_selected.min(app.project_files.len().saturating_sub(1));
            if app.project_editor_path.as_ref() == Some(&path) {
                app.project_editor_path = None;
                app.project_editor_text.clear();
                app.project_editor_dirty = false;
                app.project_editor_cursor = 0;
                app.project_pane = ProjectPane::FileTree;
            }
            app.toast = Some("Deleted".into());
        }
        UiAction::RenameProject { project, name } => {
            match runtime.project_manager.rename(&project, &name) {
                Ok(renamed) => {
                    runtime.database.record_project_activity("project_renamed", &format!("{} -> {}", project.name, renamed.name))?;
                    refresh_dashboard(runtime, app)?;
                    if app.active_project.as_ref().is_some_and(|active| active.path == project.path) {
                        let old_root = project.path;
                        let old_editor = app.project_editor_path.clone();
                        open_project_workspace(app, renamed.clone());
                        if let Some(old_editor) = old_editor {
                            if let Ok(relative) = old_editor.strip_prefix(&old_root) {
                                let new_editor = renamed.path.join(relative);
                                if let Ok(text) = std::fs::read_to_string(&new_editor) {
                                    app.project_editor_path = Some(new_editor);
                                    app.project_editor_text = text;
                                    app.project_pane = ProjectPane::Editor;
                                }
                            }
                        }
                        start_project_compiler(runtime, app);
                        start_citation_indexer(runtime, app);
                    }
                    app.projects = runtime.project_manager.list().unwrap_or_default();
                    app.projects_selected = app.projects.iter().position(|p| p.path == renamed.path).unwrap_or(0);
                    app.toast = Some(format!("Renamed project to {}", renamed.name));
                }
                Err(error) => app.toast = Some(format!("Could not rename project: {error}")),
            }
        }
        UiAction::ConfirmDeleteProject(project) => {
            app.delete_confirmation = Some(papr_core::DeletionTarget::Project { project });
            app.mode = AppMode::ConfirmDelete;
        }
        UiAction::DeleteProject(project) => {
            let was_active = app
                .active_project
                .as_ref()
                .is_some_and(|active| active.path == project.path);
            if let Err(error) = runtime.project_manager.delete(&project) {
                app.toast = Some(format!("Could not delete project: {error}"));
                return Ok(());
            }
            runtime.database.record_project_activity("project_deleted", &project.name)?;
            refresh_dashboard(runtime, app)?;
            if was_active {
                if let Some(mut compiler) = runtime.project_compiler.take() {
                    compiler.stop();
                }
                app.active_project = None;
                app.project_files.clear();
                app.project_tree_dir = None;
                app.project_file_selected = 0;
                app.project_editor_path = None;
                app.project_editor_text.clear();
                app.project_editor_dirty = false;
                app.project_editor_cursor = 0;
                app.project_editor_insert_mode = false;
                app.project_editor_scroll = 0;
                app.project_completions.clear();
                runtime.citation_index = None;
                runtime.citation_source = CitationSource::default();
                app.project_build_status = "Idle".into();
                app.project_build_errors.clear();
                app.project_build_scroll = 0;
                app.pdf_viewer_path = None;
                app.pdf_viewer_page = 1;
                app.pdf_viewer_total_pages = 1;
                app.pdf_viewer_scroll_y = 0;
            }
            app.project_pane = ProjectPane::ProjectList;
            app.projects = runtime.project_manager.list().unwrap_or_default();
            app.projects_selected = app.projects_selected.min(app.projects.len().saturating_sub(1));
            app.toast = Some(format!("Deleted project {}", project.name));
        }
        UiAction::OpenPdf { paper_id, path } => {
            let session_id = runtime.database.record_open(paper_id, true)?;
            open_pdf(
                &runtime.pdf_viewer,
                &path,
                app,
                Some(session_id),
                Some(senders.app_events.clone()),
            )?;
            dispatch_plugin_events(runtime, app, &["paper_opened"], paper_id).await?;
            refresh_paper_views(runtime, app)?;
        }
        UiAction::OpenNote(target) => {
            let paper_id = resolve_target(target, &mut runtime.database)?;
            runtime
                .database
                .record_activity("note_opened", Some(paper_id), None)?;
            dispatch_plugin_events(runtime, app, &["paper_opened"], paper_id).await?;
            refresh_dashboard(runtime, app)?;
            app.note_editor = Some(runtime.database.paper_note(paper_id)?);
            app.note_preview = false;
            app.note_scroll = 0;
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
        UiAction::AddToQueue(paper_id) => {
            runtime.database.add_to_queue(paper_id)?;
            refresh_paper_views(runtime, app)?;
            app.toast = Some("Added to Reading Queue".into());
        }
        UiAction::RemoveFromQueue(paper_id) => {
            runtime.database.remove_from_queue(paper_id)?;
            refresh_paper_views(runtime, app)?;
            app.toast = Some("Removed from Reading Queue".into());
        }
        UiAction::MoveQueueItemUp(paper_id) => {
            runtime.database.move_queue_item(paper_id, true)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            app.reading_queue_selected = app.reading_queue_selected.saturating_sub(1);
        }
        UiAction::MoveQueueItemDown(paper_id) => {
            runtime.database.move_queue_item(paper_id, false)?;
            refresh_organization(&runtime.database, &runtime.library_roots, app)?;
            app.reading_queue_selected = (app.reading_queue_selected + 1)
                .min(app.reading_queue_papers.len().saturating_sub(1));
        }
        UiAction::ClosePdf => {
            if let (Some(session_id), Some(start)) = (app.active_pdf_session_id, app.active_pdf_session_start) {
                let duration_s = start.elapsed().as_secs();
                runtime.database.record_reading_duration(session_id, duration_s)?;
                refresh_dashboard(runtime, app)?;
            }
            app.active_pdf_session_id = None;
            app.active_pdf_session_start = None;
            app.mode = AppMode::Normal;
            app.pdf_viewer_path = None;
            pdf_viewer::evict_distant_pages(0);
        }
        UiAction::OpenCollection(collection_id) => {
            open_collection(&runtime.database, app, collection_id)?;
        }
        UiAction::OpenAuthor(author_id) => {
            open_author(&runtime.database, &runtime.library_roots, app, author_id)?;
        }
        UiAction::OpenDownload(id) => {
            let task = app.downloads.iter().find(|t| t.id == id);
            let mut paper_id = None;
            let mut path = None;
            if let Some(task) = task {
                if let Some(task_paper_id) = task.paper_id {
                    paper_id = Some(task_paper_id);
                    if let Some(paper) = app.library.papers.iter().find(|p| p.id == task_paper_id) {
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
            let session_id = paper_id.map(|paper_id| runtime.database.record_open(paper_id, true)).transpose()?;
            open_pdf(
                &runtime.pdf_viewer,
                &path,
                app,
                session_id,
                Some(senders.app_events.clone()),
            )?;
            if paper_id.is_some() {
                refresh_paper_views(runtime, app)?;
            }
        }
        UiAction::MarkUnread(paper_id) => {
            runtime.database.mark_unread(paper_id)?;
            refresh_paper_views(runtime, app)?;
            app.toast = Some("Marked unread".into());
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
            app.toast = Some("Group permanently deleted".into());
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
            if let Some(task) = app.downloads.iter_mut().find(|t| t.paper_id == Some(paper_id)) {
                task.status = DownloadStatus::Renaming;
            }
            move_pdf_file(&source, &destination)?;
            runtime.database.rename_pdf(paper_id, &destination)?;
            if let Some(task) = app.downloads.iter_mut().find(|t| t.paper_id == Some(paper_id)) {
                task.pdf_path = Some(destination.to_string_lossy().into_owned());
                task.status = DownloadStatus::Completed;
            }
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
            .context("group no longer exists")?;
        let old = collection.folder_path.as_ref().map_or_else(
            || runtime.primary_library_root.join(&collection.name),
            PathBuf::from,
        );
        let new = old
            .parent()
            .unwrap_or(&runtime.primary_library_root)
            .join(name);
        std::fs::rename(&old, &new).context("failed to rename group directory")?;
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
            anyhow::bail!("a group with this name already exists");
        }
        let folder = runtime.primary_library_root.join(name);
        std::fs::create_dir(&folder).context("failed to create group directory")?;
        if let Err(error) = runtime.database.create_collection(name, &folder) {
            let _ = std::fs::remove_dir(&folder);
            return Err(error.into());
        }
        return Ok(());
    }
    let paper_id = prompt
        .paper_id
        .context("group assignment has no paper")?;
    let paper = app
        .library
        .papers
        .iter()
        .find(|paper| paper.id == paper_id)
        .context("paper must have a local PDF before group assignment")?;
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
            anyhow::bail!("a PDF with this filename already exists in the group");
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
        anyhow::bail!("group name must be one safe directory name");
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

fn add_paper_to_collection_with_disk(
    runtime: &Runtime,
    _app: &mut App,
    paper_id: i64,
    name: &str,
) -> Result<bool> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(false);
    }
    validate_collection_name(name)?;

    let collections = runtime.database.collections()?;
    let existing = collections
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

    let Some(paper) = runtime.database.library_paper_by_id(paper_id)? else {
        return Ok(false);
    };

    let current_collection_name = runtime.database.paper_collection_name(paper_id)?;
    let already_in_collection = current_collection_name.as_deref().map_or(false, |c| c.eq_ignore_ascii_case(name));

    if let Some(pdf_path_str) = &paper.pdf_path {
        let source = PathBuf::from(pdf_path_str);
        if source.exists() {
            let destination = folder.join(
                source
                    .file_name()
                    .context("PDF path has no filename")?,
            );
            let mut moved = false;
            if source != destination {
                if !destination.exists() {
                    move_pdf_file(&source, &destination)?;
                    moved = true;
                }
            }
            runtime
                .database
                .assign_moved_pdf(paper_id, collection_id, &destination)?;
            let directories = LibraryIndexer::collection_directories(&runtime.collection_roots);
            for directory in &directories {
                let _ = runtime.database.sync_collection_directory(directory);
            }
            let _ = runtime.database.reconcile_collections(&runtime.collection_roots, &directories);
            return Ok(!already_in_collection || moved);
        }
    }

    runtime.database.add_to_collection(paper_id, name)?;
    let directories = LibraryIndexer::collection_directories(&runtime.collection_roots);
    for directory in &directories {
        let _ = runtime.database.sync_collection_directory(directory);
    }
    let _ = runtime.database.reconcile_collections(&runtime.collection_roots, &directories);
    Ok(!already_in_collection)
}

async fn dispatch_plugin_events(
    runtime: &Runtime,
    app: &mut App,
    events: &[&str],
    paper_id: i64,
) -> Result<()> {
    let Some(paper) = runtime.database.library_paper_by_id(paper_id)? else {
        return Ok(());
    };

    let paper_json = serde_json::json!({
        "id": paper.id,
        "title": paper.title,
        "authors": paper.authors,
        "doi": paper.doi,
        "arxiv_id": paper.arxiv_id,
        "pdf_path": paper.pdf_path,
        "reading_status": paper.reading_status,
        "is_favorite": paper.is_favorite,
    });

    let enabled_plugins = runtime.plugin_host.plugins();
    let mut organization_dirty = false;
    let mut last_notify = None;

    for plugin in enabled_plugins {
        if !plugin.enabled {
            continue;
        }

        for &event_name in events {
            let request = papr_core::PluginRequest::new(
                event_name,
                serde_json::json!({
                    "paper_id": paper.id,
                    "paper": paper_json.clone(),
                }),
            );

            match runtime
                .plugin_host
                .invoke(&plugin.id, &request, std::time::Duration::from_secs(5))
                .await
            {
                Ok(response) => {
                    for action in response.actions {
                        match action {
                            papr_core::PluginAction::Notify { message } => {
                                last_notify = Some(message);
                            }
                            papr_core::PluginAction::AddToCollection { name } => {
                                match add_paper_to_collection_with_disk(runtime, app, paper.id, &name) {
                                    Ok(changed) => {
                                        if changed {
                                            organization_dirty = true;
                                        }
                                    }
                                    Err(err) => {
                                        eprintln!("Failed to add paper to collection '{name}': {err}");
                                    }
                                }
                            }
                        }
                    }
                }
                Err(err) => {
                    eprintln!("Plugin '{}' invocation failed: {err}", plugin.id);
                }
            }
        }
    }

    if organization_dirty {
        refresh_organization(&runtime.database, &runtime.library_roots, app)?;
        refresh_library(runtime, app)?;
        if let Some(msg) = last_notify {
            app.toast = Some(msg);
        }
    }

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
    app.reading_queue_papers = database.reading_queue_papers_in_roots(library_roots)?;
    app.reading_queue_selected = app
        .reading_queue_selected
        .min(app.reading_queue_papers.len().saturating_sub(1));
    Ok(())
}

fn refresh_dashboard_papers(
    runtime: &mut Runtime,
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
        runtime.active_dashboard_fetch = None;
        return Ok(());
    }
    let key = DashboardFeedKey {
        feed_date: runtime.dashboard_feed_date.clone(),
        keyword_signature: runtime.dashboard_keyword_signature.clone(),
    };
    app.today_status = DiscoveryStatus::Loading;
    if runtime.active_dashboard_fetch.as_ref() == Some(&key) {
        return Ok(());
    }
    start_dashboard_fetch(
        runtime.arxiv.clone(),
        runtime.dashboard_keywords.clone(),
        key.clone(),
        senders.today.clone(),
    );
    runtime.active_dashboard_fetch = Some(key);
    Ok(())
}

fn start_dashboard_fetch(
    client: ArxivClient,
    keywords: Vec<String>,
    key: DashboardFeedKey,
    sender: mpsc::UnboundedSender<TodayResponse>,
) {
    tokio::spawn(async move {
        let result = dashboard_papers(client, keywords, &key.feed_date)
            .await
            .map_err(|error| error.to_string());
        let _ = sender.send(TodayResponse { key, result });
    });
}

fn local_feed_date() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

fn dashboard_keyword_signature(keywords: &[String]) -> String {
    format!("{DASHBOARD_FEED_ALGORITHM_VERSION}:{}", keywords.join(","))
}

async fn dashboard_papers(
    client: ArxivClient,
    keywords: Vec<String>,
    feed_date: &str,
) -> Result<Vec<RemotePaper>> {
    if keywords.is_empty() {
        let mut papers = client.latest(DASHBOARD_CANDIDATE_LIMIT).await?;
        shuffle_daily_bucket(&mut papers, feed_date, "");
        papers.truncate(DASHBOARD_DISPLAY_LIMIT);
        return Ok(papers);
    }

    let mut buckets = (0..keywords.len())
        .map(|_| None)
        .collect::<Vec<Option<(String, Vec<RemotePaper>)>>>();
    let mut last_error: Option<anyhow::Error> = None;
    let mut requests = JoinSet::new();
    for (index, keyword) in keywords.into_iter().enumerate() {
        let client = client.clone();
        requests.spawn(async move {
            let result = client
                .search_latest(&keyword, DASHBOARD_CANDIDATE_LIMIT)
                .await;
            (index, keyword, result)
        });
    }
    while let Some(result) = requests.join_next().await {
        match result {
            Ok((index, keyword, Ok(mut papers))) => {
                shuffle_daily_bucket(&mut papers, feed_date, &keyword);
                buckets[index] = Some((keyword, papers));
            }
            Ok((_, _, Err(error))) => last_error = Some(error.into()),
            Err(error) => last_error = Some(anyhow::anyhow!(
                "dashboard keyword fetch task failed: {error}"
            )),
        }
    }
    let buckets = buckets.into_iter().flatten().collect::<Vec<_>>();
    if buckets.is_empty() {
        if let Some(error) = last_error {
            return Err(error);
        }
        return Ok(Vec::new());
    }
    Ok(select_dashboard_papers(
        buckets,
        DASHBOARD_DISPLAY_LIMIT,
        feed_date,
    ))
}

/// Deterministically order one keyword's eligible papers for a local day.
///
/// A SHA-256 rank gives every paper an equal chance of every position and
/// includes the date, so the selection changes each day. This avoids any
/// dependence on arXiv response order, database iteration, or process-local
/// RNG state. The paper ID only resolves the cryptographically negligible case
/// of two equal ranks.
fn shuffle_daily_bucket(papers: &mut [RemotePaper], feed_date: &str, keyword: &str) {
    papers.sort_by_cached_key(|paper| (daily_paper_rank(feed_date, keyword, &paper.id), paper.id.clone()));
}

fn keyword_terms(keyword: &str) -> Vec<String> {
    keyword
        .split(|character: char| !character.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn title_match_strength(title: &str, terms: &[String]) -> usize {
    let title = title.to_lowercase();
    terms.iter().filter(|term| title.contains(term.as_str())).count()
}

#[derive(Debug)]
struct DashboardCandidate {
    paper: RemotePaper,
    matches: Vec<KeywordMatch>,
    daily_rank: [u8; 32],
}

#[derive(Debug)]
struct KeywordMatch {
    keyword_index: usize,
    title_term_matches: usize,
    full_title_match: bool,
}

/// Select a balanced, relevance-ranked daily dashboard feed.
///
/// Each keyword receives a gentle, position-weighted representation target.
/// Greedy selection maximizes coverage of still-unmet targets, then uses a
/// deterministic daily rank to rotate papers. A selected paper counts toward
/// every keyword it matches, so cross-keyword papers are boosted rather than
/// attributed to whichever bucket happened to be visited first. Title matches
/// remain a tie-breaker after daily rotation.
fn select_dashboard_papers(
    buckets: Vec<(String, Vec<RemotePaper>)>,
    limit: usize,
    feed_date: &str,
) -> Vec<RemotePaper> {
    let keywords: Vec<_> = buckets
        .iter()
        .map(|(keyword, _)| keyword.clone())
        .collect();
    let terms: Vec<_> = keywords.iter().map(|keyword| keyword_terms(keyword)).collect();
    let mut candidates = Vec::<DashboardCandidate>::new();
    let mut candidate_indexes = HashMap::<String, usize>::new();
    let mut available_by_keyword = vec![0_usize; buckets.len()];

    for (keyword_index, (_, papers)) in buckets.into_iter().enumerate() {
        for paper in papers {
            let title_term_matches = title_match_strength(&paper.title, &terms[keyword_index]);
            let full_title_match = !terms[keyword_index].is_empty()
                && title_term_matches == terms[keyword_index].len();
            let keyword_match = KeywordMatch {
                keyword_index,
                title_term_matches,
                full_title_match,
            };
            if let Some(&candidate_index) = candidate_indexes.get(&paper.id) {
                if !candidates[candidate_index]
                    .matches
                    .iter()
                    .any(|matched| matched.keyword_index == keyword_index)
                {
                    available_by_keyword[keyword_index] += 1;
                    candidates[candidate_index].matches.push(keyword_match);
                }
            } else {
                let candidate_index = candidates.len();
                candidate_indexes.insert(paper.id.clone(), candidate_index);
                available_by_keyword[keyword_index] += 1;
                candidates.push(DashboardCandidate {
                    daily_rank: daily_paper_rank(feed_date, "dashboard", &paper.id),
                    paper,
                    matches: vec![keyword_match],
                });
            }
        }
    }

    let targets = keyword_representation_targets(&available_by_keyword, &keywords, limit, feed_date);
    let keyword_weights = keyword_priority_weights(&available_by_keyword);
    let mut represented = vec![0_usize; targets.len()];
    let mut selected = Vec::with_capacity(limit.min(candidates.len()));

    while selected.len() < limit && !candidates.is_empty() {
        let best_index = candidates
            .iter()
            .enumerate()
            .max_by_key(|(_, candidate)| {
                let coverage_keywords = candidate
                    .matches
                    .iter()
                    .filter(|matched| represented[matched.keyword_index] < targets[matched.keyword_index])
                    .count();
                let weighted_coverage = candidate
                    .matches
                    .iter()
                    .filter_map(|matched| {
                        let deficit = targets[matched.keyword_index]
                            .saturating_sub(represented[matched.keyword_index]);
                        (deficit > 0).then_some(deficit * keyword_weights[matched.keyword_index])
                    })
                    .sum::<usize>();
                let full_title_matches = candidate
                    .matches
                    .iter()
                    .filter(|matched| matched.full_title_match)
                    .count();
                let title_term_matches = candidate
                    .matches
                    .iter()
                    .map(|matched| matched.title_term_matches)
                    .sum::<usize>();
                (
                    coverage_keywords,
                    candidate.matches.len(),
                    weighted_coverage,
                    candidate.daily_rank,
                    full_title_matches,
                    title_term_matches,
                    candidate.paper.id.as_str(),
                )
            })
            .map(|(index, _)| index);
        let Some(best_index) = best_index else {
            break;
        };
        let candidate = candidates.swap_remove(best_index);
        for matched in &candidate.matches {
            represented[matched.keyword_index] += 1;
        }
        selected.push(candidate.paper);
    }
    selected
}

fn keyword_priority_weights(available_by_keyword: &[usize]) -> Vec<usize> {
    let active = available_by_keyword.iter().filter(|&&count| count > 0).count();
    let mut active_rank = 0_usize;
    available_by_keyword
        .iter()
        .map(|&available| {
            if available == 0 {
                0
            } else {
                // The range is intentionally narrow (at most ~10%) so keyword
                // order is a preference, not a monopoly.
                let weight = active * 10 + active.saturating_sub(active_rank + 1);
                active_rank += 1;
                weight
            }
        })
        .collect()
}

fn keyword_representation_targets(
    available_by_keyword: &[usize],
    keywords: &[String],
    limit: usize,
    feed_date: &str,
) -> Vec<usize> {
    let active: Vec<_> = available_by_keyword
        .iter()
        .enumerate()
        .filter_map(|(index, &available)| (available > 0).then_some(index))
        .collect();
    let mut targets = vec![0_usize; available_by_keyword.len()];
    if active.len() > limit {
        let weights = keyword_priority_weights(available_by_keyword);
        let mut weighted_window: Vec<_> = active
            .iter()
            .map(|&index| {
                let rank = daily_paper_rank(feed_date, "keyword-window", &keywords[index]);
                let mut bytes = [0_u8; 8];
                bytes.copy_from_slice(&rank[..8]);
                let uniform = (u64::from_be_bytes(bytes) as f64 / u64::MAX as f64)
                    .max(f64::MIN_POSITIVE);
                // Efraimidis-Spirakis weighted sampling: higher-priority
                // keywords have a slightly better daily chance of appearing.
                (uniform.powf(1.0 / weights[index] as f64), index)
            })
            .collect();
        weighted_window.sort_by(|left, right| right.0.total_cmp(&left.0));
        for (_, index) in weighted_window.into_iter().take(limit) {
            targets[index] = 1;
        }
        return targets;
    }
    for &index in active.iter().take(limit) {
        targets[index] = 1;
    }
    let remaining = limit.saturating_sub(active.len().min(limit));
    if remaining == 0 || active.is_empty() {
        return targets;
    }

    let weights = keyword_priority_weights(available_by_keyword);
    let weight_total: usize = active.iter().map(|&index| weights[index]).sum();
    let mut remainders = Vec::with_capacity(active.len());
    let mut assigned_extra = 0_usize;
    for &index in &active {
        let numerator = remaining * weights[index];
        let allocation = numerator / weight_total;
        targets[index] += allocation;
        assigned_extra += allocation;
        remainders.push((numerator % weight_total, index));
    }
    remainders.sort_by(|(left_remainder, left_index), (right_remainder, right_index)| {
        right_remainder
            .cmp(left_remainder)
            .then_with(|| left_index.cmp(right_index))
    });
    for (_, index) in remainders
        .into_iter()
        .take(remaining.saturating_sub(assigned_extra))
    {
        targets[index] += 1;
    }
    targets
}

fn daily_paper_rank(feed_date: &str, keyword: &str, paper_id: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"papr-dashboard-feed-v1\0");
    for value in [feed_date, keyword, paper_id] {
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    hasher.finalize().into()
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

fn refresh_paper_views(runtime: &Runtime, app: &mut App) -> Result<()> {
    refresh_library(runtime, app)?;
    refresh_organization(&runtime.database, &runtime.library_roots, app)?;
    refresh_dashboard(runtime, app)?;
    refresh_downloads(runtime, app);
    Ok(())
}

fn default_pdf_viewer() -> String {
    if cfg!(target_os = "macos") {
        "open".into()
    } else if cfg!(target_os = "windows") {
        "cmd /C start msedge \"\"".into()
    } else {
        "xdg-open".into()
    }
}

fn get_pdf_page_count(path: &Path) -> usize {
    if let Ok(output) = std::process::Command::new("pdfinfo")
        .arg(path)
        .output()
    {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.starts_with("Pages:") {
                    if let Some(pages_str) = line.split_whitespace().nth(1) {
                        if let Ok(pages) = pages_str.parse::<usize>() {
                            return pages;
                        }
                    }
                }
            }
        }
    }
    1
}

/// Scroll the PDF viewer by `delta` rows.
fn pdf_scroll(app: &mut App, delta: i64) {
    pdf_viewer::scroll_by_rows(app, delta);
}

fn handle_mouse(app: &mut App, mouse: MouseEvent) -> Option<UiAction> {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if app.mode == AppMode::PdfView {
                pdf_scroll(app, -3);
            }
        }
        MouseEventKind::ScrollDown => {
            if app.mode == AppMode::PdfView {
                pdf_scroll(app, 3);
            }
        }
        _ => {}
    }
    None
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

    let absolute_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    #[cfg(target_os = "windows")]
    let absolute_path = {
        let path_str = absolute_path.to_string_lossy();
        if let Some(stripped) = path_str.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            absolute_path
        }
    };
    let path = &absolute_path;

    if viewer == "internal" {
        // Flush any cached images / protocol state that belong to a different
        // document.  This is the single place that guarantees the cache is
        // always consistent with whatever path is about to be stored in
        // `app.pdf_viewer_path`.
        pdf_viewer::reset_for_new_document(path);
        app.mode = AppMode::PdfView;
        app.pdf_viewer_path = Some(path.to_path_buf());
        app.pdf_viewer_page = 1;
        app.pdf_viewer_scroll_y = 0;
        app.pdf_viewer_page_pixel_h = 0;
        app.pdf_viewer_max_scroll_y = 0;
        app.pdf_viewer_total_pages = get_pdf_page_count(path);
        app.active_pdf_session_id = session_id;
        app.active_pdf_session_start = Some(std::time::Instant::now());
        app.toast = Some(format!(
            "Viewing PDF: {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
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

    #[cfg(target_os = "windows")]
    let program = if program.eq_ignore_ascii_case("start") {
        argv.insert(0, program);
        if argv.len() == 1 {
            // Only "start" was provided, add empty title to prevent path being treated as title
            argv.push("".to_string());
        }
        argv.insert(0, "/C".to_string());
        "cmd".to_string()
    } else {
        program
    };

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
    silent: bool,
) {
    if app.library.indexing {
        return;
    }
    app.library.indexing = !silent;
    if !silent {
        app.library.message = Some("Indexing library folders...".into());
    }
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

fn log_message(database_file: &Path, message: &str) {
    if let Some(parent) = database_file.parent() {
        let log_file = parent.join("papr.log");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file)
        {
            use std::io::Write;
            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
            let _ = writeln!(file, "[{}] {}", timestamp, message);
        }
    }
}

/// Project enriched metadata onto a paper already shown by the UI.
///
/// Database enrichment deliberately retains the stored abstract when a provider
/// has none. The UI must apply that same merge before it persists the dashboard
/// cache; replacing the object wholesale would otherwise make a valid abstract
/// disappear from the UI even though it remains in SQLite.
fn merge_enriched_remote_paper(current: &RemotePaper, enriched: &RemotePaper) -> RemotePaper {
    let mut merged = enriched.clone();
    if merged.abstract_text.trim().is_empty() {
        merged.abstract_text = current.abstract_text.clone();
    }
    merged
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
    let all_papers = runtime.database.papers_needing_enrichment_with_doi()?;
    let papers: Vec<_> = all_papers
        .into_iter()
        .filter(|(pid, _, _, _)| !runtime.active_enrichments.contains(pid))
        .collect();
    for task in &mut app.downloads {
        if task.status == DownloadStatus::ExtractingMetadata {
            let needs_enrichment = papers.iter().any(|(pid, _, _, _)| Some(*pid) == task.paper_id);
            if !needs_enrichment {
                task.status = DownloadStatus::Completed;
                finalize_download_task(task);
            }
        }
    }
    if !papers.is_empty() {
        for (paper_id, _, _, _) in &papers {
            runtime.active_enrichments.insert(*paper_id);
            if let Some(task) = app.downloads.iter_mut().find(|t| t.paper_id == Some(*paper_id)) {
                task.status = DownloadStatus::Enriching;
            }
        }
        let arxiv_client = runtime.arxiv.clone();
        let crossref_client = runtime.crossref.clone();
        let openalex_client = runtime.openalex.clone();
        let enrichment_tx = senders.enrichment.clone();
        app.enrichment_pending = true;
        let db_file_log = runtime.database_file.clone();
        // arXiv asks API clients to pause between requests; serial enrichment
        // prevents background metadata work from monopolizing the shared client.
        let concurrency = Arc::new(Semaphore::new(METADATA_ENRICHMENT_CONCURRENCY));
        tokio::spawn(async move {
            let mut jobs = JoinSet::new();
            for (paper_id, mut candidate_arxiv, mut candidate_doi, pdf_path) in papers {
                let arxiv_client = arxiv_client.clone();
                let crossref_client = crossref_client.clone();
                let openalex_client = openalex_client.clone();
                let enrichment_tx = enrichment_tx.clone();
                let db_file_log = db_file_log.clone();
                let permit = concurrency.clone();
                jobs.spawn(async move {
                let _permit = permit.acquire_owned().await.expect("enrichment semaphore closed");
                if let Some(path) = pdf_path {
                    if let Ok(output) = tokio::process::Command::new("pdftotext")
                        .args(["-l", "2", &path, "-"])
                        .output()
                        .await
                    {
                        if output.status.success() {
                            let text = String::from_utf8_lossy(&output.stdout);
                            let lower_text = text.to_lowercase();

                            if candidate_arxiv.is_none() {
                                if let Some(idx) = lower_text.find("arxiv:") {
                                    let substr = &text[idx + 6..];
                                    let end = substr
                                        .find(|c: char| !c.is_ascii_digit() && c != '.')
                                        .unwrap_or(substr.len());
                                    let id = &substr[..end];
                                    if id.len() >= 7 {
                                        candidate_arxiv = Some(id.to_string());
                                    }
                                }
                            }
                            if candidate_doi.is_none() {
                                if let Some(idx) = lower_text.find("10.") {
                                    let substr = &text[idx..];
                                    let end = substr
                                        .find(|c: char| c.is_whitespace() || c == '\n')
                                        .unwrap_or(substr.len());
                                    let id = substr[..end].trim_end_matches(['.', ',', ';', ')']);
                                    if id.len() >= 5 {
                                        candidate_doi = Some(id.to_string());
                                    }
                                }
                            }
                        }
                    }
                }

                let outcome;
                if let Some(doi) = candidate_doi {
                    match crossref_client.get_by_doi(&doi).await {
                        Ok(Some(mut paper)) => {
                            if paper.journal_ref.is_none() {
                                if let Ok(Some(journal)) = openalex_client.journal_by_doi(&doi).await {
                                    paper.journal_ref = Some(journal);
                                }
                            }
                            outcome = EnrichmentOutcome::Success(paper);
                        }
                        Ok(None) => {
                            if let Ok(Some(journal)) = openalex_client.journal_by_doi(&doi).await {
                                outcome = EnrichmentOutcome::Journal(journal);
                            } else if let Some(arxiv_id) = candidate_arxiv {
                                match arxiv_client.get_by_id(&arxiv_id).await {
                                    Ok(Some(paper)) => {
                                        outcome = EnrichmentOutcome::Success(paper);
                                    }
                                    Ok(None) => outcome = EnrichmentOutcome::Unavailable,
                                    Err(e) => {
                                        log_message(
                                            &db_file_log,
                                            &format!("arXiv enrichment failed for {arxiv_id}: {e}"),
                                        );
                                        outcome = EnrichmentOutcome::Failed;
                                    }
                                }
                            } else {
                                outcome = EnrichmentOutcome::Unavailable;
                            }
                        }
                        Err(e) => {
                            let err_msg = format!("Crossref enrichment failed for {doi}: {e}");
                            log_message(&db_file_log, &err_msg);
                            outcome = match openalex_client.journal_by_doi(&doi).await {
                                Ok(Some(journal)) => EnrichmentOutcome::Journal(journal),
                                _ => EnrichmentOutcome::Failed,
                            };
                        }
                    }
                } else if let Some(arxiv_id) = candidate_arxiv {
                    match arxiv_client.get_by_id(&arxiv_id).await {
                        Ok(Some(paper)) => {
                            outcome = EnrichmentOutcome::Success(paper);
                        }
                        Ok(None) => {
                            outcome = EnrichmentOutcome::Unavailable;
                        }
                        Err(e) => {
                            let err_msg = format!("arXiv enrichment failed for {arxiv_id}: {e}");
                            log_message(&db_file_log, &err_msg);
                            outcome = EnrichmentOutcome::Failed;
                        }
                    }
                } else {
                    outcome = EnrichmentOutcome::Unavailable;
                }
                let _ = enrichment_tx.send(MetadataEnrichment {
                    paper_id,
                    outcome,
                });
                });
            }
            while let Some(result) = jobs.join_next().await {
                if let Err(error) = result {
                    log_message(&db_file_log, &format!("Metadata enrichment task failed: {error}"));
                }
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
        false,
    );
}

fn start_silent_runtime_scan(runtime: &Runtime, senders: &ActionSenders, app: &mut App) {
    start_scan(
        &runtime.library_roots,
        &runtime.collection_roots,
        &senders.index,
        app,
        true,
    );
}

async fn apply_index_response(
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
                let was_newly_imported = runtime.database.import_pdf(pdf)?;
                imported += usize::from(was_newly_imported);
                if let Some(paper_id) = runtime.database.paper_id_for_pdf(pdf)? {
                    sync_pdf_collection_membership(
                        &runtime.database,
                        paper_id,
                        pdf,
                        &runtime.collection_roots,
                    )?;
                    if was_newly_imported {
                        dispatch_plugin_events(runtime, app, &["paper_imported"], paper_id).await?;
                    }
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
                if imported {
                    dispatch_plugin_events(runtime, app, &["paper_imported"], paper_id).await?;
                }
            }
            app.library.message = Some(if imported {
                format!("Imported {}", pdf.title)
            } else {
                "Ignored duplicate PDF".into()
            });
            spawn_enrichment_if_needed(runtime, senders, app)?;
        }
        IndexResponse::File(Err(error)) => {
            log_message(&runtime.database_file, &format!("Library indexing error: {error}"));
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
            
            if app.downloads.iter().any(|task| {
                task.pdf_path.as_deref() == Some(&pdf_path) || task.id == id
            }) {
                continue;
            }

            let canonical_path = std::fs::canonicalize(&pdf_path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| pdf_path.clone());

            let content_hash = {
                let mut hash_ok = None;
                if let Ok(mut file) = std::fs::File::open(&pdf_path) {
                    use sha2::{Digest, Sha256};
                    let mut hasher = Sha256::new();
                    if std::io::copy(&mut file, &mut hasher).is_ok() {
                        hash_ok = Some(format!("{:x}", hasher.finalize()));
                    }
                }
                hash_ok
            };

            let mut paper_id = database
                .paper_id_for_path(&pdf_path)
                .ok()
                .flatten()
                .or_else(|| database.paper_id_for_path(&canonical_path).ok().flatten());

            let mut db_pdf_path = None;
            if let Some(ref hash) = content_hash {
                if let Ok(Some((id, path))) = database.paper_by_hash(hash) {
                    paper_id = Some(id);
                    db_pdf_path = Some(path);
                }
            }

            if let Some(ref path) = db_pdf_path {
                let path_buf = std::path::PathBuf::from(path);
                if !path_buf.starts_with(download_dir) {
                    // Stale download file (already moved to collection)! Remove it.
                    let _ = std::fs::remove_file(&pdf_path);
                    continue;
                }
            }

            app.downloads.push(DownloadTask {
                id,
                title,
                downloaded: size,
                total: Some(size),
                paper_id,
                pdf_path: Some(pdf_path),
                status: DownloadStatus::Completed,
                remote_paper: None,
                failed_at: None,
            });
        }
    }
}

fn sanitize_download_filename_component(title: &str) -> String {
    let sanitized: String = title
        .chars()
        .map(|c| match c {
            '/' => '_',
            '\n' | '\r' | '\t' => ' ',
            #[cfg(windows)]
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            #[cfg(windows)]
            c if c.is_control() => ' ',
            #[cfg(not(windows))]
            c if c.is_control() => c,
            c => c,
        })
        .collect();
    let sanitized = sanitized.trim();
    #[cfg(windows)]
    {
        sanitized.trim_end_matches(['.', ' ']).to_owned()
    }
    #[cfg(not(windows))]
    {
        sanitized.to_owned()
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
    if app.downloaded_remote_paper(&paper).is_some() {
        app.toast = Some("Paper already available. Skipping download...".to_owned());
        return;
    }
    if pending.contains_key(&paper.id) {
        return;
    }
    let sanitized_title = sanitize_download_filename_component(&paper.title);
    let filename = if sanitized_title.is_empty() {
        paper
            .id
            .rsplit('/')
            .next()
            .unwrap_or("paper")
            .chars()
            .map(|c| if c == '/' { '_' } else { c })
            .collect()
    } else {
        sanitized_title
    };
    let destination = directory.join(format!("{filename}.pdf"));
    let id = paper.id.clone();
    pending.insert(id.clone(), paper.clone());
    app.toast = Some("Downloading paper...".to_owned());
    app.downloads.push(DownloadTask {
        id: id.clone(),
        title: paper.title.clone(),
        downloaded: 0,
        total: None,
        paper_id: None,
        pdf_path: Some(destination.to_string_lossy().into_owned()),
        status: DownloadStatus::Starting,
        remote_paper: Some(paper),
        failed_at: None,
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

async fn apply_download_event(
    event: DownloadEvent,
    pending: &mut HashMap<String, RemotePaper>,
    runtime: &mut Runtime,
    app: &mut App,
    senders: &ActionSenders,
) -> Result<()> {
    let is_completed = matches!(event, DownloadEvent::Completed { .. });
    let id = match &event {
        DownloadEvent::Started { id, .. }
        | DownloadEvent::Progress { id, .. }
        | DownloadEvent::Completed { id, .. }
        | DownloadEvent::Failed { id, .. } => id.clone(),
    };
    let task = app
        .downloads
        .iter_mut()
        .find(|t| t.id == id)
        .context("received download event for unknown task")?;
    match event {
        DownloadEvent::Started { total, .. } => {
            task.status = DownloadStatus::Running;
            task.total = total;
        }
        DownloadEvent::Progress { downloaded, .. } => {
            task.status = DownloadStatus::Running;
            task.downloaded = downloaded;
        }
        DownloadEvent::Completed { id, path } => {
            task.status = DownloadStatus::ExtractingMetadata;
            let final_path = path.with_extension("");
            if path.exists() {
                std::fs::rename(&path, &final_path).context("failed to promote temporary download path")?;
            }
            let pdf = LibraryIndexer::inspect(&final_path).context("failed to index downloaded PDF")?;
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

            if let Some(paper_id) = task.paper_id {
                sync_pdf_collection_membership(
                    &runtime.database,
                    paper_id,
                    &pdf,
                    &runtime.collection_roots,
                )?;
                dispatch_plugin_events(runtime, app, &["paper_downloaded", "paper_opened"], paper_id).await?;
            }

            // Project the completed download into every workspace before the next
            // render, so remote views immediately expose their local-PDF actions.
            spawn_enrichment_if_needed(runtime, senders, app)?;
            refresh_paper_views(runtime, app)?;
            refresh_dashboard(runtime, app)?;
            if app.toast.is_none() {
                app.toast = Some("Download complete. Press Enter to open the PDF.".to_owned());
            }
        }
        DownloadEvent::Failed { id, error } => {
            pending.remove(&id);
            task.status = DownloadStatus::Failed(error);
            task.failed_at = Some(std::time::Instant::now());
        }
    }
    if is_completed {
        app.downloads.retain(|t| t.id != id || !matches!(t.status, DownloadStatus::Failed(_)));
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.mode == AppMode::PdfView {
        // Accept both Press and Repeat so held-key scrolling is driven by
        // the OS key-repeat rate rather than by the 16 ms poll timeout.
        let is_scroll_event = matches!(
            key.kind,
            KeyEventKind::Press | KeyEventKind::Repeat
        );
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') if key.kind == KeyEventKind::Press => {
                return Some(UiAction::ClosePdf);
            }
            KeyCode::Up | KeyCode::Char('k') if is_scroll_event => {
                pdf_scroll(app, -1);
            }
            KeyCode::Down | KeyCode::Char('j') if is_scroll_event => {
                pdf_scroll(app, 1);
            }
            KeyCode::PageUp if key.kind == KeyEventKind::Press => {
                pdf_viewer::page_up(app);
            }
            KeyCode::PageDown if key.kind == KeyEventKind::Press => {
                pdf_viewer::page_down(app);
            }
            _ => {}
        }
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
        app.mode = AppMode::TerminalCommand;
        app.terminal_command.clear();
        app.terminal_command_cursor = 0;
        app.terminal_command_output.clear();
        app.terminal_command_directory = app.active_project.as_ref().map(|project| project.path.clone());
        return None;
    }
    // Help is a global command. Resolve it before any view-specific handler
    // (notably PaperDetail) can interpret the key as navigation. Text-entry
    // contexts retain the character as normal input.
    if key.code == KeyCode::Char('?') && !has_active_text_input(app) {
        app.dispatch(Command::ToggleHelp);
        return None;
    }
    if matches!(
        app.mode,
        AppMode::ProjectRename
            | AppMode::ProjectCreate
            | AppMode::ProjectFileCreate
            | AppMode::ProjectEntryRename
    ) {
        match key.code {
            KeyCode::Esc => {
                app.project_rename_input.clear();
                app.project_rename_cursor = 0;
                app.project_entry_rename_path = None;
                app.mode = AppMode::Normal;
            }
            KeyCode::Enter => {
                let creating_project = app.mode == AppMode::ProjectCreate;
                let creating_file = app.mode == AppMode::ProjectFileCreate;
                let renaming_entry = app.mode == AppMode::ProjectEntryRename;
                app.mode = AppMode::Normal;
                let name = app.project_rename_input.trim().to_owned();
                app.project_rename_input.clear();
                app.project_rename_cursor = 0;
                if creating_project {
                    return Some(UiAction::CreateProject(name));
                }
                if creating_file {
                    return Some(UiAction::CreateProjectFile(name));
                }
                if renaming_entry {
                    return app
                        .project_entry_rename_path
                        .take()
                        .map(|path| UiAction::RenameProjectEntry { path, name });
                }
                let project = app.active_project.clone().or_else(|| app.projects.get(app.projects_selected).cloned());
                return project.map(|project| UiAction::RenameProject { project, name });
            }
            _ => {
                let _ = edit_text(
                    &mut app.project_rename_input,
                    &mut app.project_rename_cursor,
                    key,
                );
            }
        }
        return None;
    }
    if app.mode == AppMode::CommandPalette {
        match key.code {
            KeyCode::Esc => app.dispatch(Command::TogglePalette),
            KeyCode::Up => {
                app.palette_selected = app.palette_selected.saturating_sub(1);
            }
            KeyCode::Down => {
                app.palette_selected = (app.palette_selected + 1)
                    .min(app.filtered_palette_items().len().saturating_sub(1));
            }
            KeyCode::Enter => {
                let items = app.filtered_palette_items();
                if let Some(&page) = items.get(app.palette_selected) {
                    app.dispatch(Command::TogglePalette);
                    if let Some(index) = papr_core::Page::ALL.iter().position(|&p| p == page) {
                        app.sidebar_index = index;
                    }
                    app.page = page;
                    app.content_focused = true;
                    if app.active_search_workspaces.contains(&app.page) {
                        app.mode = AppMode::WorkspaceSearch;
                    } else {
                        app.mode = AppMode::Normal;
                    }
                }
            }
            _ => {
                let old_query = app.palette_query.clone();
                if edit_text(
                    &mut app.palette_query,
                    &mut app.palette_query_cursor,
                    key,
                ) {
                    if app.palette_query != old_query {
                        app.palette_selected = 0;
                    }
                }
            }
        }
        return None;
    }
    if app.mode == AppMode::TerminalCommand {
        match key.code {
            KeyCode::Esc => app.mode = AppMode::Normal,
            KeyCode::Enter => run_terminal_command(app),
            _ => {
                let _ = edit_text(
                    &mut app.terminal_command,
                    &mut app.terminal_command_cursor,
                    key,
                );
            }
        }
        return None;
    }
    if app.mode == AppMode::Help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('?' | 'q') => app.dispatch(Command::ToggleHelp),
            KeyCode::Up | KeyCode::Char('k') => {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.help_scroll = app.help_scroll.saturating_add(1);
            }
            KeyCode::PageUp => app.help_scroll = app.help_scroll.saturating_sub(10),
            KeyCode::PageDown => app.help_scroll = app.help_scroll.saturating_add(10),
            KeyCode::Home => app.help_scroll = 0,
            KeyCode::End => app.help_scroll = usize::MAX,
            _ => {}
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
    if app.mode == AppMode::DiscoverFilter {
        return handle_discover_filter_key(app, key);
    }
    if app.mode == AppMode::WorkspaceSearch {
        return handle_workspace_search_key(app, key);
    }
    if app.page == papr_core::Page::Discover
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Right
    {
        app.discovery.next_page();
        return None;
    }
    if app.page == papr_core::Page::Discover
        && key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Left
    {
        app.discovery.previous_page();
        return None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('b') {
        app.dispatch(Command::TogglePalette);
        return None;
    }
    // Project panes normally own their input, so route search before handing
    // them the key. Insert mode remains the sole editor exception above.
    if key.code == KeyCode::Char('/')
        && !(app.content_focused
            && app.page == papr_core::Page::Projects
            && app.project_pane == ProjectPane::Editor
            && app.project_editor_insert_mode)
    {
        app.page = papr_core::Page::Discover;
        app.sidebar_index = 1;
        app.content_focused = true;
        app.mode = AppMode::Search;
        app.discovery.query_cursor = app.discovery.query.len();
        return None;
    }
    // Projects owns its raw key events. In particular, do not normalize arrow
    // keys into h/j/k/l before the currently focused pane sees them.
    if app.page == papr_core::Page::Projects
        && app.content_focused
    {
        return handle_projects_key(app, key);
    }
    if app.page == papr_core::Page::Discover
        && app.content_focused
        && app.mode == AppMode::Normal
        && !app.discovery.results.is_empty()
        && key.code == KeyCode::Left
    {
        app.content_focused = false;
        return None;
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

    if !app.content_focused {
        if app.page == papr_core::Page::Settings && matches!(key.code, KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter) {
            // Opening the settings modal is handled by the sidebar navigation
            // path; just focus content so UI is consistent.
            app.content_focused = true;
            return None;
        }
        if let Some(command) = navigation_command(key) {
            app.dispatch(command);
        }
        return None;
    }
    // When Settings page gains content_focused, immediately open the modal.
    if app.content_focused && app.page == papr_core::Page::Settings && app.mode == AppMode::Normal {
        // The modal will be opened by the page-change handler; just return.
    }
    if key.code == KeyCode::Char('r')
        && app.page == papr_core::Page::Discover
        && !app.discovery.query.trim().is_empty()
    {
        if app.discovery.next_batch_start.is_some()
            && app.discovery.progress_message.as_deref()
                == Some("More results could not be loaded. Press r to retry.")
        {
            return Some(UiAction::RetryDiscoverMore);
        }
        let query = app.discovery.query.trim().to_owned();
        app.discovery.query.clone_from(&query);
        return Some(UiAction::Search(query));
    }
    if key.code == KeyCode::Char('r') && app.page == papr_core::Page::Library {
        return Some(UiAction::Reindex);
    }
    if key.code == KeyCode::Char('o') {
        if let Some(arxiv_reference) = selected_paper_arxiv_reference(app) {
            return open_arxiv_page(app, arxiv_reference);
        }
    }
    if matches!(app.page, papr_core::Page::Dashboard | papr_core::Page::Discover) {
        match key.code {
            KeyCode::Char('c') => return selected_remote_target(app).map(UiAction::CopyCitation),
            KeyCode::Char('d') => return selected_remote_paper(app).cloned().map(UiAction::Download),
            _ => {}
        }
    }
    if key.code == KeyCode::Char('u') {
        if let Some(paper_id) = selected_local_paper_id(app) {
            return Some(UiAction::MarkUnread(paper_id));
        }
    }
    if key.code == KeyCode::Char('a') {
        if let Some(paper_id) = selected_local_paper_id(app) {
            let is_queued = app.reading_queue_papers.iter().any(|p| p.id == paper_id);
            if is_queued {
                return Some(UiAction::RemoveFromQueue(paper_id));
            } else {
                return Some(UiAction::AddToQueue(paper_id));
            }
        }
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
    if let Some(action) = handle_reading_queue_key(app, key) {
        return Some(action);
    }
    if let Some(action) = handle_credits_key(app, key) {
        return Some(action);
    }
    if app.page == papr_core::Page::Discover
        && key.code == KeyCode::Char('>')
        && !app.discovery.results.is_empty()
    {
        app.mode = AppMode::DiscoverFilter;
        app.discovery.filter_cursor = app.discovery.filter.len();
        return None;
    }
    if app.page == papr_core::Page::Discover
        && matches!(
            key.code,
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')
        )
    {
        return app
            .discovery
            .selected_paper()
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
        if app.page == papr_core::Page::Discover && command == Command::MoveUp && app.discovery.selected == 0 {
            app.mode = if app.discovery.results.is_empty() {
                AppMode::Search
            } else {
                AppMode::DiscoverFilter
            };
            if app.mode == AppMode::DiscoverFilter {
                app.discovery.filter_cursor = app.discovery.filter.len();
            }
            return None;
        }
        app.dispatch(command);
    }
    None
}

fn has_active_text_input(app: &App) -> bool {
    matches!(
        app.mode,
        AppMode::ProjectRename
            | AppMode::ProjectCreate
            | AppMode::ProjectFileCreate
            | AppMode::ProjectEntryRename
            | AppMode::CommandPalette
            | AppMode::TerminalCommand
            | AppMode::NoteEdit
            | AppMode::Prompt
            | AppMode::Search
            | AppMode::DiscoverFilter
            | AppMode::WorkspaceSearch
    ) || (app.page == papr_core::Page::Projects
        && app.content_focused
        && app.project_pane == ProjectPane::Editor
        && app.project_editor_insert_mode)
}

fn run_terminal_command(app: &mut App) {
    let command_text = app.terminal_command.trim().to_owned();
    if command_text.is_empty() {
        return;
    }
    if command_text == "clear" {
        app.terminal_command_output.clear();
        app.terminal_command.clear();
        app.terminal_command_cursor = 0;
        return;
    }
    if command_text == "cd" || command_text.starts_with("cd ") {
        let path = command_text[2..].trim();
        app.terminal_command.clear();
        app.terminal_command_cursor = 0;
        change_terminal_directory(app, path);
        return;
    }

    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = ProcessCommand::new("cmd");
        command.args(["/C", &command_text]);
        command
    };
    #[cfg(not(target_os = "windows"))]
    let mut command = {
        let mut command = ProcessCommand::new("sh");
        command.args(["-c", &command_text]);
        command
    };
    if let Some(directory) = &app.terminal_command_directory {
        command.current_dir(directory);
    }

    let output = match command.output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if !stderr.is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&stderr);
            }
            if text.is_empty() {
                text = "Command completed with no output.".into();
            }
            format!(
                "[exit {}]\n{text}",
                output.status.code().map_or_else(|| "signal".to_owned(), |code| code.to_string())
            )
        }
        Err(error) => format!("Could not run command: {error}"),
    };
    append_terminal_output(app, &command_text, &output);
    app.terminal_command.clear();
    app.terminal_command_cursor = 0;
}

fn change_terminal_directory(app: &mut App, path: &str) {
    let base = app
        .terminal_command_directory
        .clone()
        .or_else(|| std::env::current_dir().ok());
    let candidate = match base {
        Some(base) if Path::new(path).is_relative() => base.join(path),
        _ => PathBuf::from(path),
    };
    match std::fs::canonicalize(&candidate) {
        Ok(directory) if directory.is_dir() => {
            app.terminal_command_directory = Some(directory.clone());
            append_terminal_output(app, &format!("cd {path}"), &directory.display().to_string());
        }
        Ok(_) => append_terminal_output(app, &format!("cd {path}"), "Not a directory."),
        Err(error) => append_terminal_output(app, &format!("cd {path}"), &format!("cd: {error}")),
    }
}

fn append_terminal_output(app: &mut App, command: &str, output: &str) {
    if !app.terminal_command_output.is_empty() {
        app.terminal_command_output.push('\n');
    }
    app.terminal_command_output.push_str("$ ");
    app.terminal_command_output.push_str(command);
    app.terminal_command_output.push('\n');
    app.terminal_command_output.push_str(&sanitize_terminal_output(output));
    const MAX_SCROLLBACK_BYTES: usize = 64 * 1024;
    if app.terminal_command_output.len() > MAX_SCROLLBACK_BYTES {
        let start = app.terminal_command_output.len() - MAX_SCROLLBACK_BYTES;
        let start = next_char_boundary(&app.terminal_command_output, start);
        app.terminal_command_output.drain(..start);
    }
}

fn sanitize_terminal_output(output: &str) -> String {
    output
        .chars()
        .filter(|character| matches!(character, '\n' | '\t') || !character.is_control())
        .collect()
}

fn handle_projects_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    // Pane shortcuts deliberately do nothing in Insert mode. They are still
    // consumed so an Alt sequence can never become editor text.
    if app.project_pane == ProjectPane::Editor
        && app.project_editor_insert_mode
        && is_project_pane_shortcut(key)
    {
        return None;
    }
    if handle_project_pane_shortcut(app, key) {
        return None;
    }
    if app.active_project.is_none() || app.project_pane == ProjectPane::ProjectList {
        match key.code {
            KeyCode::Char('q') => app.dispatch(Command::Quit),
            KeyCode::Left => app.content_focused = false,
            KeyCode::Char('n') => {
                app.project_rename_input.clear();
                app.project_rename_cursor = 0;
                app.mode = AppMode::ProjectCreate;
            }
            KeyCode::Char('r') => return Some(UiAction::RefreshProjects),
            KeyCode::Char('R') => {
                if let Some(project) = app.projects.get(app.projects_selected) {
                    app.project_rename_input = project.name.clone();
                    app.project_rename_cursor = app.project_rename_input.len();
                    app.mode = AppMode::ProjectRename;
                }
            }
            KeyCode::Char('x') => {
                return app
                    .projects
                    .get(app.projects_selected)
                    .cloned()
                    .map(UiAction::ConfirmDeleteProject);
            }
            KeyCode::Up | KeyCode::Char('k') => app.projects_selected = app.projects_selected.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => app.projects_selected = (app.projects_selected + 1).min(app.projects.len().saturating_sub(1)),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => return app.projects.get(app.projects_selected).cloned().map(UiAction::OpenProject),
            _ => {}
        }
        return None;
    }
    if key.code == KeyCode::Esc {
        if app.project_pane == ProjectPane::FileTree {
            exit_project_view(app);
        } else {
            return_to_project_file_tree(app);
        }
        return None;
    }
    if app.project_pane == ProjectPane::FileTree && key.code == KeyCode::Left {
        if !move_project_tree_to_parent(app) {
            exit_project_view(app);
        }
        return None;
    }
    // Save is mode-independent: handle it before Insert-mode text dispatch.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
        save_project_editor(app);
        return None;
    }
    if is_project_bibtex_paste_shortcut(key) && is_project_bibtex_editor(app) {
        match read_clipboard_text() {
            Some(text) if !text.is_empty() => {
                handle_project_bibtex_paste(app, &text);
            }
            _ => app.toast = Some("Clipboard does not contain text to paste.".into()),
        }
        return None;
    }
    if app.project_pane == ProjectPane::Editor && app.project_editor_insert_mode {
        if !app.project_completions.is_empty() {
            match key.code {
                KeyCode::Up => {
                    app.project_completion_selected = app.project_completion_selected.saturating_sub(1);
                    return None;
                }
                KeyCode::Down => {
                    app.project_completion_selected = (app.project_completion_selected + 1).min(app.project_completions.len().saturating_sub(1));
                    return None;
                }
                KeyCode::Tab | KeyCode::Enter => {
                    if accept_project_completion(app) { return None; }
                }
                KeyCode::Esc => {
                    app.project_completions.clear();
                    return None;
                }
                _ => {}
            }
        }
        if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
            move_project_editor_page(app, if key.code == KeyCode::PageUp { -1 } else { 1 });
            return None;
        }
        match apply_editor_insert_key(
            &mut app.project_editor_text,
            &mut app.project_editor_cursor,
            key,
        ) {
            EditorInsertResult::ExitInsert => app.project_editor_insert_mode = false,
            EditorInsertResult::Changed => app.project_editor_dirty = true,
            EditorInsertResult::Ignored | EditorInsertResult::Moved => {}
        }
        return None;
    }
    // Match the shared workspace command map in every non-text-input Projects
    // context. Insert mode returned above, so its `q` remains ordinary text.
    if key.code == KeyCode::Char('q') {
        app.dispatch(Command::Quit);
        return None;
    }
    if app.project_pane == ProjectPane::Editor {
        handle_project_editor_normal_key(app, key);
        return None;
    }
    match app.project_pane {
        ProjectPane::FileTree => match key.code {
            KeyCode::Char('n') => {
                app.project_rename_input.clear();
                app.project_rename_cursor = 0;
                app.mode = AppMode::ProjectFileCreate;
            }
            KeyCode::Char('R') => {
                if let Some(path) = app.project_files.get(app.project_file_selected).cloned() {
                    app.project_rename_input = path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or_default()
                        .to_owned();
                    app.project_rename_cursor = app.project_rename_input.len();
                    app.project_entry_rename_path = Some(path);
                    app.mode = AppMode::ProjectEntryRename;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.project_file_selected = app.project_file_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.project_file_selected = (app.project_file_selected + 1)
                    .min(app.project_files.len().saturating_sub(1));
            }
            KeyCode::Enter | KeyCode::Right => {
                if let Some(path) = app.project_files.get(app.project_file_selected).cloned() {
                    if path.is_dir() {
                        app.project_tree_dir = Some(path.clone());
                        app.project_files = project_tree_entries(&path);
                        app.project_file_selected = 0;
                    } else {
                        return Some(UiAction::OpenProjectFile(path));
                    }
                }
            }
            KeyCode::Char('x') => return app
                .project_files
                .get(app.project_file_selected)
                .cloned()
                .map(UiAction::ConfirmDeleteProjectEntry),
            _ => {}
        },
        ProjectPane::Build => handle_project_build_key(app, key),
        ProjectPane::Preview => handle_project_preview_key(app, key),
        ProjectPane::ProjectList | ProjectPane::Editor => {}
    }
    None
}

/// Direct focus selection is resolved before any pane consumes its own keys.
/// Alt combinations never reach the editor buffer, including in Insert mode.
fn handle_project_pane_shortcut(app: &mut App, key: KeyEvent) -> bool {
    if !is_project_pane_shortcut(key) {
        return false;
    }
    let pane = match key.code {
        KeyCode::Char('1') => ProjectPane::FileTree,
        KeyCode::Char('2') => ProjectPane::Editor,
        KeyCode::Char('3') => ProjectPane::Preview,
        KeyCode::Char('4') => ProjectPane::Build,
        _ => return false,
    };
    if app.project_pane == ProjectPane::Editor
        && pane != ProjectPane::Editor
        && app.project_editor_dirty
        && !save_project_editor(app)
    {
        return true;
    }
    let available = match pane {
        ProjectPane::ProjectList => true,
        ProjectPane::FileTree | ProjectPane::Build => app.active_project.is_some(),
        ProjectPane::Editor => app.active_project.is_some() && app.project_editor_path.is_some(),
        ProjectPane::Preview => {
            app.pdf_viewer == "internal"
                && app.active_project.is_some()
                && app.pdf_viewer_path.as_ref().is_some_and(|path| path.exists())
        }
    };
    if available {
        app.project_pane = pane;
    } else {
        app.toast = Some(match pane {
            ProjectPane::Editor => "Open a source file before focusing the editor.",
            ProjectPane::Preview => {
                if app.pdf_viewer != "internal" {
                    "PDF preview is disabled when using an external viewer."
                } else {
                    "PDF preview is unavailable until the first successful build."
                }
            }
            _ => "Open a project before focusing this pane.",
        }
        .into());
    }
    true
}

fn is_project_pane_shortcut(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::ALT)
        && matches!(key.code, KeyCode::Char('1' | '2' | '3' | '4'))
}

fn handle_project_build_key(app: &mut App, key: KeyEvent) {
    let line_count = app.project_build_errors.len().max(1);
    let max_scroll = line_count.saturating_sub(app.project_build_viewport_height.max(1));
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            app.project_build_scroll = app.project_build_scroll.saturating_sub(1);
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.project_build_scroll = (app.project_build_scroll + 1).min(max_scroll);
        }
        KeyCode::PageUp => {
            app.project_build_scroll = app
                .project_build_scroll
                .saturating_sub(app.project_build_viewport_height.max(1));
        }
        KeyCode::PageDown => {
            app.project_build_scroll = (app.project_build_scroll
                + app.project_build_viewport_height.max(1))
                .min(max_scroll);
        }
        KeyCode::Home => app.project_build_scroll = 0,
        KeyCode::End => app.project_build_scroll = max_scroll,
        _ => {}
    }
}

fn handle_project_preview_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Up => pdf_viewer::jump_to_page(app, app.pdf_viewer_page.saturating_sub(1)),
        KeyCode::Down => pdf_viewer::jump_to_page(
            app,
            (app.pdf_viewer_page + 1).min(app.pdf_viewer_total_pages.max(1)),
        ),
        KeyCode::Home => pdf_viewer::jump_to_page(app, 1),
        KeyCode::End => pdf_viewer::jump_to_page(app, app.pdf_viewer_total_pages.max(1)),
        KeyCode::Left | KeyCode::Right => {}
        _ => {}
    }
}

fn move_project_editor_page(app: &mut App, direction: isize) {
    let wrap_width = app.project_editor_wrap_width.max(1);
    let (row, column) = cursor_visual_position(
        &app.project_editor_text,
        app.project_editor_cursor,
        wrap_width,
    );
    let total_rows = app
        .project_editor_text
        .split('\n')
        .map(|line| config_editor_wrap_rows(line.chars().count(), wrap_width))
        .sum::<usize>()
        .max(1);
    let target = row
        .saturating_add_signed(direction * app.project_editor_viewport_height.max(1) as isize)
        .min(total_rows.saturating_sub(1));
    app.project_editor_cursor = cursor_from_visual_position(
        &app.project_editor_text,
        target,
        column,
        wrap_width,
    );
}

fn save_project_editor(app: &mut App) -> bool {
    let Some(path) = app.project_editor_path.as_ref() else { return true; };
    match std::fs::write(path, &app.project_editor_text) {
        Ok(()) => {
            app.project_editor_dirty = false;
            app.project_build_status = "Saved; latexmk watching…".into();
            app.toast = Some("Saved".into());
            true
        }
        Err(error) => {
            app.toast = Some(format!("Could not save file: {error}"));
            false
        }
    }
}

fn exit_project_view(app: &mut App) {
    if app.project_editor_dirty && !save_project_editor(app) {
        return;
    }
    if let Some(active) = &app.active_project
        && let Some(selected) = app
            .projects
            .iter()
            .position(|project| project.path == active.path)
    {
        app.projects_selected = selected;
    }
    app.project_editor_insert_mode = false;
    app.project_completions.clear();
    app.project_pane = ProjectPane::ProjectList;
}

fn return_to_project_file_tree(app: &mut App) {
    if app.project_pane == ProjectPane::Editor
        && app.project_editor_dirty
        && !save_project_editor(app)
    {
        return;
    }
    app.project_editor_insert_mode = false;
    app.project_completions.clear();
    app.project_pane = ProjectPane::FileTree;
}

fn move_project_tree_to_parent(app: &mut App) -> bool {
    let Some(project) = app.active_project.as_ref() else { return false; };
    let root = &project.path;
    let Some(current) = app.project_tree_dir.as_ref() else { return false; };
    if current == root {
        return false;
    }
    let Some(parent) = current.parent() else { return false; };
    if !parent.starts_with(root) {
        return false;
    }
    let previous = current.clone();
    let parent = parent.to_path_buf();
    app.project_tree_dir = Some(parent.clone());
    app.project_files = project_tree_entries(&parent);
    app.project_file_selected = app
        .project_files
        .iter()
        .position(|entry| entry == &previous)
        .unwrap_or(0);
    true
}

fn is_project_bibtex_paste_shortcut(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && key.modifiers.contains(KeyModifiers::SHIFT)
        && matches!(key.code, KeyCode::Char('v' | 'V'))
}

fn is_project_bibtex_editor(app: &App) -> bool {
    app.page == Page::Projects
        && app.content_focused
        && app.project_pane == ProjectPane::Editor
        && app.project_editor_path.as_ref().is_some_and(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("bib"))
        })
}

/// Inserts terminal-paste text without normalizing its whitespace or entries.
/// Returns whether the paste was accepted for the active editor.
fn handle_project_bibtex_paste(app: &mut App, text: &str) -> bool {
    if !is_project_bibtex_editor(app) || text.is_empty() {
        return false;
    }

    insert_project_bibtex_text(app, text);
    save_project_editor(app)
}

fn insert_project_bibtex_text(app: &mut App, text: &str) {
    app.project_editor_text
        .insert_str(app.project_editor_cursor, text);
    app.project_editor_cursor += text.len();
    app.project_editor_dirty = true;
    app.project_completions.clear();
}

fn read_clipboard_text() -> Option<String> {
    if let Ok(mut clipboard) = arboard::Clipboard::new()
        && let Ok(text) = clipboard.get_text()
    {
        return Some(text);
    }

    for (program, args) in [
        ("wl-paste", &["--no-newline"][..]),
        ("xclip", &["-selection", "clipboard", "-o"][..]),
        ("pbpaste", &[][..]),
    ] {
        if let Ok(output) = ProcessCommand::new(program).args(args).output()
            && output.status.success()
            && let Ok(text) = String::from_utf8(output.stdout)
        {
            return Some(text);
        }
    }
    None
}

/// Projects intentionally uses the same movement primitives as Settings.  The
/// only project-specific concern is persistence, handled by Ctrl+S above.
fn handle_project_editor_normal_key(app: &mut App, key: KeyEvent) {
    let movement = match key.code {
        KeyCode::Left | KeyCode::Char('h') => Some(KeyCode::Left),
        KeyCode::Right | KeyCode::Char('l') => Some(KeyCode::Right),
        KeyCode::Up | KeyCode::Char('k') => Some(KeyCode::Up),
        KeyCode::Down | KeyCode::Char('j') => Some(KeyCode::Down),
        _ => None,
    };
    if let Some(code) = movement {
        let mut movement_key = KeyEvent::new(code, key.modifiers);
        movement_key.kind = key.kind;
        let _ = edit_text(&mut app.project_editor_text, &mut app.project_editor_cursor, movement_key);
        return;
    }
    match key.code {
        KeyCode::Char('i') => app.project_editor_insert_mode = true,
        KeyCode::Char('w') => app.project_editor_cursor = next_word_boundary(&app.project_editor_text, app.project_editor_cursor),
        KeyCode::Char('b') => app.project_editor_cursor = prev_word_boundary(&app.project_editor_text, app.project_editor_cursor),
        KeyCode::Char('0') | KeyCode::Home => app.project_editor_cursor = config_editor_line_start(&app.project_editor_text, app.project_editor_cursor),
        KeyCode::Char('$') | KeyCode::End => app.project_editor_cursor = config_editor_line_end(&app.project_editor_text, app.project_editor_cursor),
        KeyCode::Backspace => {
            app.project_editor_cursor = prev_char_boundary(
                &app.project_editor_text,
                app.project_editor_cursor,
            );
        }
        KeyCode::Delete | KeyCode::Char('x') => {
            if app.project_editor_cursor < app.project_editor_text.len() {
                let next = next_char_boundary(
                    &app.project_editor_text,
                    app.project_editor_cursor,
                );
                app.project_editor_text
                    .drain(app.project_editor_cursor..next);
                app.project_editor_dirty = true;
            }
        }
        KeyCode::PageUp | KeyCode::PageDown => {
            move_project_editor_page(app, if key.code == KeyCode::PageUp { -1 } else { 1 });
        }
        _ => {}
    }
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
        KeyCode::Down => {
            if !app.discovery.results.is_empty() {
                app.mode = AppMode::Normal;
                app.discovery.selected = 0;
            }
        }
        KeyCode::Right if !key.modifiers.contains(KeyModifiers::CONTROL)
            && app.discovery.query_cursor == app.discovery.query.len()
            && !app.discovery.results.is_empty() => {
            app.mode = AppMode::DiscoverFilter;
            app.discovery.filter_cursor = app.discovery.filter.len();
        }
        KeyCode::Left if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.discovery.query_cursor == 0 {
                app.content_focused = false;
                app.mode = AppMode::Normal;
            } else {
                edit_text(
                    &mut app.discovery.query,
                    &mut app.discovery.query_cursor,
                    key,
                );
            }
        }
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

fn handle_discover_filter_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.discovery.results.is_empty() {
        app.mode = AppMode::Normal;
        return None;
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('>') => app.mode = AppMode::Normal,
        KeyCode::Up => {
            app.mode = AppMode::Search;
            app.discovery.query_cursor = app.discovery.query.len();
        }
        KeyCode::Down | KeyCode::Enter => {
            app.mode = AppMode::Normal;
            app.discovery.selected = app.discovery.selected.min(app.discovery.visible_page_len().saturating_sub(1));
        }
        KeyCode::Left if !key.modifiers.contains(KeyModifiers::CONTROL) && app.discovery.filter_cursor == 0 => {
            app.mode = AppMode::Search;
            app.discovery.query_cursor = app.discovery.query.len();
        }
        KeyCode::Right if !key.modifiers.contains(KeyModifiers::CONTROL)
            && app.discovery.filter_cursor == app.discovery.filter.len() => {
            app.mode = AppMode::Normal;
        }
        _ => {
            if edit_text(&mut app.discovery.filter, &mut app.discovery.filter_cursor, key) {
                app.discovery.rebuild_filter();
            }
        }
    }
    None
}

fn handle_workspace_search_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if key.code == KeyCode::Char('>') {
        app.mode = AppMode::Normal;
        app.content_focused = true;
        app.active_search_workspaces.remove(&app.page);
        return None;
    }

    match key.code {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
            app.active_search_workspaces.remove(&app.page);
        }
        KeyCode::Down | KeyCode::Enter => {
            app.mode = AppMode::Normal;
            app.content_focused = true;
            app.active_search_workspaces.remove(&app.page);
            if key.code == KeyCode::Down {
                match app.page {
                    papr_core::Page::Library => app.library.selected = 0,
                    papr_core::Page::Downloads => app.download_selected = 0,
                    papr_core::Page::Collections => {
                        if app.active_collection.is_some() {
                            app.collection_paper_selected = 0;
                        } else {
                            app.collection_selected = 0;
                        }
                    }
                    papr_core::Page::Authors => {
                        if app.active_author.is_some() {
                            app.author_paper_selected = 0;
                        } else {
                            app.author_selected = 0;
                        }
                    }
                    papr_core::Page::Bookmarks => app.bookmark_selected = 0,
                    papr_core::Page::Notes => app.notes_selected = 0,
                    papr_core::Page::ReadingQueue => app.reading_queue_selected = 0,
                    _ => {}
                }
            }
        }
        KeyCode::Left if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.workspace_query_cursor == 0 {
                app.mode = AppMode::Normal;
                app.content_focused = false;
            } else {
                edit_text(
                    &mut app.workspace_query,
                    &mut app.workspace_query_cursor,
                    key,
                );
            }
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
                DeletionTarget::Project { project } => Some(UiAction::DeleteProject(project)),
                DeletionTarget::Paper { id, path, .. } => {
                    Some(UiAction::DeletePaper { paper_id: id, path })
                }
                DeletionTarget::Collection { id, path, .. } => {
                    Some(UiAction::DeleteCollection { collection_id: id, path })
                }
                DeletionTarget::ProjectEntry { path, .. } => {
                    Some(UiAction::DeleteProjectEntry(path))
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
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => Some(UiAction::OpenPdf {
            paper_id: bookmark.paper_id,
            path: PathBuf::from(&bookmark.pdf_path),
        }),
        KeyCode::Char('n') => Some(UiAction::OpenNote(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Char('g') => Some(UiAction::Prompt(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Char('R') => Some(UiAction::RenamePdf(bookmark.paper_id)),
        KeyCode::Char('x') => Some(UiAction::ConfirmDeletePaper {
            paper_id: bookmark.paper_id,
            title: bookmark.paper_title.clone(),
            path: Some(PathBuf::from(&bookmark.pdf_path)),
        }),
        _ => None,
    }
}

fn handle_credits_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Credits {
        return None;
    }
    match key.code {
        KeyCode::Enter => {
            let items = app.credits_items();
            let selected = items.get(app.credits_selected)?;
            Some(UiAction::OpenBrowser(selected.url.clone()))
        }
        _ => None,
    }
}

fn handle_notes_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Notes {
        return None;
    }
    let paper = *app.filtered_notes_papers().get(app.notes_selected)?;
    match key.code {
        KeyCode::Char('n') => Some(UiAction::OpenNote(PaperTarget::Local(paper.id))),
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
            let path = paper.pdf_path.clone().map(PathBuf::from)?;
            Some(UiAction::OpenPdf {
                paper_id: paper.id,
                path,
            })
        }
        KeyCode::Char('B') => Some(UiAction::Bookmark(PaperTarget::Local(paper.id))),
        KeyCode::Char('c') => Some(UiAction::CopyCitation(PaperTarget::Local(paper.id))),
        KeyCode::Char('g') => Some(UiAction::Prompt(PaperTarget::Local(paper.id))),
        KeyCode::Char('R') => Some(UiAction::RenamePdf(paper.id)),
        KeyCode::Char('x') => Some(UiAction::ConfirmDeletePaper {
            paper_id: paper.id,
            title: paper.title.clone(),
            path: paper.pdf_path.as_ref().map(PathBuf::from),
        }),
        _ => None,
    }
}

fn handle_reading_queue_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::ReadingQueue {
        return None;
    }
    
    // Check for moving items up/down in the queue
    if key.code == KeyCode::Up
        && (key.modifiers.contains(KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::CONTROL))
    {
        let paper = *app.filtered_reading_queue_papers().get(app.reading_queue_selected)?;
        return Some(UiAction::MoveQueueItemUp(paper.id));
    }
    if key.code == KeyCode::Down
        && (key.modifiers.contains(KeyModifiers::SHIFT) || key.modifiers.contains(KeyModifiers::CONTROL))
    {
        let paper = *app.filtered_reading_queue_papers().get(app.reading_queue_selected)?;
        return Some(UiAction::MoveQueueItemDown(paper.id));
    }

    let paper = *app.filtered_reading_queue_papers().get(app.reading_queue_selected)?;
    match key.code {
        KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
            let path = paper.pdf_path.clone().map(PathBuf::from)?;
            Some(UiAction::OpenPdf {
                paper_id: paper.id,
                path,
            })
        }
        KeyCode::Char('B') => Some(UiAction::Bookmark(PaperTarget::Local(paper.id))),
        KeyCode::Char('c') => Some(UiAction::CopyCitation(PaperTarget::Local(paper.id))),
        KeyCode::Char('n') => Some(UiAction::OpenNote(PaperTarget::Local(paper.id))),
        KeyCode::Char('g') => Some(UiAction::Prompt(PaperTarget::Local(paper.id))),
        KeyCode::Char('R') => Some(UiAction::RenamePdf(paper.id)),
        KeyCode::Char('x') => Some(UiAction::ConfirmDeletePaper {
            paper_id: paper.id,
            title: paper.title.clone(),
            path: paper.pdf_path.as_ref().map(PathBuf::from),
        }),
        _ => None,
    }
}

fn handle_downloads_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Downloads {
        return None;
    }
    if key.code == KeyCode::Char('r') {
        if let Some(&task) = app.filtered_downloads().get(app.download_selected) {
            if matches!(task.status, DownloadStatus::Failed(_)) {
                if let Some(ref remote_paper) = task.remote_paper {
                    return Some(UiAction::RetryDownload {
                        id: task.id.clone(),
                        paper: remote_paper.clone(),
                    });
                }
            }
        }
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
        KeyCode::Char('R') | KeyCode::Char('g') | KeyCode::Char('B') | KeyCode::Char('n') | KeyCode::Char('c') | KeyCode::Char('x')
    ) {
        if let Some(task) = app.filtered_downloads().get(app.download_selected).cloned().cloned() {
            if matches!(task.status, DownloadStatus::Completed) {
                let paper_id = task.paper_id.or_else(|| {
                    task.pdf_path.as_ref().and_then(|pdf_path| {
                        app.library
                            .papers
                            .iter()
                            .find(|paper| {
                                paper.pdf_path.as_deref() == Some(pdf_path.as_str())
                                    || (|| {
                                        let paper_path = PathBuf::from(paper.pdf_path.as_deref()?);
                                        let task_path = PathBuf::from(pdf_path);
                                        let c_paper = std::fs::canonicalize(&paper_path).ok()?;
                                        let c_task = std::fs::canonicalize(&task_path).ok()?;
                                        Some(c_paper == c_task)
                                    })().unwrap_or(false)
                            })
                            .map(|paper| paper.id)
                    })
                });
                if let Some(paper_id) = paper_id {
                    app.modal_return = AppMode::Normal;
                    return match key.code {
                        KeyCode::Char('R') => Some(UiAction::RenamePdf(paper_id)),
                        KeyCode::Char('g') => Some(UiAction::Prompt(PaperTarget::Local(paper_id))),
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
    if matches!(key.code, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')) {
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
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
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
            KeyCode::Char('g') => {
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
                if matches!(key.code, KeyCode::Enter | KeyCode::Right | KeyCode::Char('l')) {
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
                if key.code == KeyCode::Char('g') {
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
    if key.code == KeyCode::Char('g') {
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
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
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
            KeyCode::Char('g') => {
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
        (KeyCode::Char('b'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::TogglePalette)
        }
        (KeyCode::Char('>'), _) => {
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
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                *cursor = prev_word_boundary(text, *cursor);
            } else if *cursor > 0 {
                let mut prev = *cursor - 1;
                while prev > 0 && !text.is_char_boundary(prev) {
                    prev -= 1;
                }
                *cursor = prev;
            }
        }
        KeyCode::Right => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                *cursor = next_word_boundary(text, *cursor);
            } else if *cursor < text.len() {
                let mut next = *cursor + 1;
                while next < text.len() && !text.is_char_boundary(next) {
                    next += 1;
                }
                *cursor = next;
            }
        }
        KeyCode::Home => {
            let s = &text[..*cursor];
            let line_start = s.rfind('\n').map(|idx| idx + 1).unwrap_or(0);
            *cursor = line_start;
        }
        KeyCode::End => {
            let line_end = text[*cursor..]
                .find('\n')
                .map(|idx| *cursor + idx)
                .unwrap_or(text.len());
            *cursor = line_end;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorInsertResult {
    Ignored,
    Moved,
    Changed,
    ExitInsert,
}

/// Shared Insert-mode input engine for every embedded Papr editor.
fn apply_editor_insert_key(text: &mut String, cursor: &mut usize, key: KeyEvent) -> EditorInsertResult {
    match key.code {
        KeyCode::Esc => EditorInsertResult::ExitInsert,
        KeyCode::Enter => {
            text.insert(*cursor, '\n');
            *cursor += 1;
            EditorInsertResult::Changed
        }
        KeyCode::Tab => {
            text.insert(*cursor, '\t');
            *cursor += 1;
            EditorInsertResult::Changed
        }
        KeyCode::BackTab => EditorInsertResult::Ignored,
        KeyCode::Backspace => {
            if *cursor == 0 {
                return EditorInsertResult::Ignored;
            }
            let previous = prev_char_boundary(text, *cursor);
            text.drain(previous..*cursor);
            *cursor = previous;
            EditorInsertResult::Changed
        }
        KeyCode::Delete => {
            if *cursor >= text.len() {
                return EditorInsertResult::Ignored;
            }
            let next = next_char_boundary(text, *cursor);
            text.drain(*cursor..next);
            EditorInsertResult::Changed
        }
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down | KeyCode::Home | KeyCode::End => {
            let _ = edit_text(text, cursor, key);
            EditorInsertResult::Moved
        }
        KeyCode::Char(character)
            if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            text.insert(*cursor, character);
            *cursor += character.len_utf8();
            EditorInsertResult::Changed
        }
        _ => EditorInsertResult::Ignored,
    }
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
        app.note_scroll = 0;
        return None;
    }
    if app.note_preview {
        match key.code {
            KeyCode::Esc => {
                app.mode = app.modal_return;
                return app.note_editor.clone().map(UiAction::SaveNote);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.note_scroll = app.note_scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.note_scroll = app.note_scroll.saturating_sub(1);
            }
            _ => {}
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
    // Keep the detail view self-contained even if its handler is invoked from
    // a future input path: help must never fall through to detail navigation.
    if key.code == KeyCode::Char('?') {
        app.dispatch(Command::ToggleHelp);
        return None;
    }
    if matches!(app.page, papr_core::Page::Dashboard | papr_core::Page::Discover)
        && matches!(
            key.code,
            KeyCode::Char('B' | 'n' | 't' | 'g')
        )
    {
        return None;
    }

    match key.code {
        KeyCode::Esc | KeyCode::Left | KeyCode::Char('h' | 'q') => app.dispatch(Command::Back),
        KeyCode::Char('j') | KeyCode::Down => {
            app.paper_detail_scroll = app.paper_detail_scroll.saturating_add(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.paper_detail_scroll = app.paper_detail_scroll.saturating_sub(1);
        }
        KeyCode::Char('d') => {
            return selected_remote_paper(app).cloned().map(UiAction::Download);
        }
        KeyCode::Char('c') => {
            return selected_remote_target(app).map(UiAction::CopyCitation);
        }
        KeyCode::Char('o') => {
            return open_arxiv_page(app, selected_remote_paper(app).map(|paper| paper.id.clone()));
        }
        KeyCode::Enter => {
            let remote = selected_remote_paper(app)?;
            let local = app.downloaded_remote_paper(remote)?;
            let path = local.pdf_path.as_ref()?;
            return Some(UiAction::OpenPdf {
                paper_id: local.id,
                path: PathBuf::from(path),
            });
        }
        KeyCode::Char('n' | 'g') => {
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
        KeyCode::Char('g') => Some(UiAction::Prompt(target)),
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
    selected_remote_paper(app)
        .cloned()
        .map(Box::new)
        .map(PaperTarget::Remote)
}

fn selected_remote_paper(app: &App) -> Option<&RemotePaper> {
    match app.page {
        papr_core::Page::Dashboard => app.today_papers.get(app.today_selected),
        _ => app.discovery.selected_paper(),
    }
}

fn selected_library_pdf(app: &App) -> Option<(i64, PathBuf)> {
    let paper = *app.filtered_library_papers().get(app.library.selected)?;
    paper
        .pdf_path
        .as_ref()
        .map(|path| (paper.id, PathBuf::from(path)))
}

/// Return the online arXiv reference for the currently selected paper row.
/// The outer option distinguishes a selected paper row from other workspace
/// rows (for example, a group or author heading).
fn selected_paper_arxiv_reference(app: &App) -> Option<Option<String>> {
    match app.page {
        papr_core::Page::Dashboard => app
            .today_papers
            .get(app.today_selected)
            .map(|paper| Some(paper.id.clone())),
        papr_core::Page::Discover => app
            .discovery
            .selected_paper()
            .map(|paper| Some(paper.id.clone())),
        papr_core::Page::Downloads => app
            .filtered_downloads()
            .get(app.download_selected)
            .map(|task| {
                task.remote_paper
                    .as_ref()
                    .map(|paper| paper.id.clone())
                    .or_else(|| {
                        selected_local_paper_id(app).and_then(|id| {
                            app.library
                                .papers
                                .iter()
                                .find(|paper| paper.id == id)
                                .and_then(|paper| paper.arxiv_id.clone())
                        })
                    })
            }),
        papr_core::Page::Library
        | papr_core::Page::ReadingQueue
        | papr_core::Page::Collections
        | papr_core::Page::Bookmarks
        | papr_core::Page::Authors
        | papr_core::Page::Notes => selected_local_paper_id(app).map(|id| {
            app.library
                .papers
                .iter()
                .find(|paper| paper.id == id)
                .and_then(|paper| paper.arxiv_id.clone())
        }),
        papr_core::Page::Projects
        | papr_core::Page::History
        | papr_core::Page::Statistics
        | papr_core::Page::Settings
        | papr_core::Page::Credits => None,
    }
}

fn open_arxiv_page(app: &mut App, arxiv_reference: Option<String>) -> Option<UiAction> {
    let Some(url) = arxiv_reference.as_deref().and_then(arxiv_page_url) else {
        app.toast = Some("No valid arXiv page is available for this paper".into());
        return None;
    };
    Some(UiAction::OpenBrowser(url))
}

fn arxiv_page_url(reference: &str) -> Option<String> {
    let reference = reference.trim();
    if reference.starts_with("https://") || reference.starts_with("http://") {
        return Some(reference.to_owned());
    }
    let id = reference
        .strip_prefix("arXiv:")
        .unwrap_or(reference)
        .trim_end_matches(".pdf");
    let (base, version) = match id.rsplit_once('v') {
        Some((base, version)) if !version.is_empty() && version.chars().all(|character| character.is_ascii_digit()) => (base, Some(version)),
        _ => (id, None),
    };
    let modern = base.split_once('.').is_some_and(|(year_month, number)| {
        year_month.len() == 4
            && year_month.chars().all(|character| character.is_ascii_digit())
            && matches!(number.len(), 4 | 5)
            && number.chars().all(|character| character.is_ascii_digit())
    });
    let legacy = base.split_once('/').is_some_and(|(category, number)| {
        !category.is_empty()
            && category.chars().all(|character| character.is_ascii_alphabetic() || matches!(character, '-' | '.'))
            && number.len() == 7
            && number.chars().all(|character| character.is_ascii_digit())
    });
    (modern || legacy).then(|| match version {
        Some(version) => format!("https://arxiv.org/abs/{base}v{version}"),
        None => format!("https://arxiv.org/abs/{base}"),
    })
}

fn selected_local_paper_id(app: &App) -> Option<i64> {
    match app.page {
        papr_core::Page::Library => app
            .filtered_library_papers()
            .get(app.library.selected)
            .map(|paper| paper.id),
        papr_core::Page::Downloads => app
            .filtered_downloads()
            .get(app.download_selected)
            .and_then(|task| {
                task.paper_id.or_else(|| {
                    task.pdf_path.as_ref().and_then(|pdf_path| {
                        app.library
                            .papers
                            .iter()
                            .find(|paper| {
                                paper.pdf_path.as_deref() == Some(pdf_path.as_str())
                                    || (|| {
                                        let paper_path = PathBuf::from(paper.pdf_path.as_deref()?);
                                        let task_path = PathBuf::from(pdf_path);
                                        let c_paper = std::fs::canonicalize(&paper_path).ok()?;
                                        let c_task = std::fs::canonicalize(&task_path).ok()?;
                                        Some(c_paper == c_task)
                                    })().unwrap_or(false)
                            })
                            .map(|paper| paper.id)
                    })
                })
            }),
        papr_core::Page::Collections => {
            if app.active_collection.is_some() {
                app.filtered_collection_papers()
                    .get(app.collection_paper_selected)
                    .map(|paper| paper.id)
            } else {
                app.filtered_collections()
                    .get(app.collection_selected)
                    .and_then(|item| match item {
                        papr_core::CollectionSearchItem::Paper(paper, _) => Some(paper.id),
                        papr_core::CollectionSearchItem::Collection(_) => None,
                    })
            }
        }
        papr_core::Page::Authors => {
            if app.active_author.is_some() {
                app.filtered_author_papers()
                    .get(app.author_paper_selected)
                    .map(|paper| paper.id)
            } else {
                None
            }
        }
        papr_core::Page::Bookmarks => app
            .filtered_bookmarks()
            .get(app.bookmark_selected)
            .map(|bookmark| bookmark.paper_id),
        papr_core::Page::Notes => app
            .filtered_notes_papers()
            .get(app.notes_selected)
            .map(|paper| paper.id),
        papr_core::Page::ReadingQueue => app
            .filtered_reading_queue_papers()
            .get(app.reading_queue_selected)
            .map(|paper| paper.id),
        papr_core::Page::Dashboard
        | papr_core::Page::Projects
        | papr_core::Page::Discover
        | papr_core::Page::History
        | papr_core::Page::Statistics
        | papr_core::Page::Settings
        | papr_core::Page::Credits => None,
    }
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

fn handle_settings_modal_key(
    app: &mut App,
    key: KeyEvent,
    runtime: &mut Runtime,
    theme: &mut Theme,
    senders: &ActionSenders,
) -> Result<Option<UiAction>> {
    use settings_modal::{handle_settings_key, SettingsKeyResult, staged_config};

    match handle_settings_key(app, key) {
        SettingsKeyResult::Handled => {}

        SettingsKeyResult::Apply => {
            let base_config = Config::load_or_create(&Paths::discover()?).unwrap_or_default();
            let new_config = staged_config(&app.settings_modal, &app.startup_page_options, &base_config);
            let toml_str = match toml::to_string_pretty(&new_config) {
                Ok(s) => s,
                Err(e) => {
                    app.toast = Some(format!("Serialization failed: {e}"));
                    return Ok(None);
                }
            };
            if let Err(e) = std::fs::write(&runtime.config_file, &toml_str) {
                app.toast = Some(format!("Write failed: {e}"));
                return Ok(None);
            }
            // Also refresh the config editor buffer.
            app.config_editor_text = toml_str;
            app.config_editor_history = vec![app.config_editor_text.clone()];
            app.config_editor_history_idx = 0;
            app.config_editor_error = None;

            if let Err(e) = apply_config_update(runtime, app, &new_config, theme, senders) {
                app.toast = Some(format!("Apply failed: {e}"));
            } else {
                app.settings_modal.original_theme = new_config.theme.clone();
                settings_modal::sync_theme_selection_to_applied(app);
                app.toast = Some("Settings saved and applied.".to_owned());
                return Ok(Some(UiAction::Reindex));
            }
        }

        SettingsKeyResult::ReturnToSidebar => {
            app.content_focused = false;
            let original = app.settings_modal.original_theme.clone();
            if !original.is_empty() && theme.name != original {
                if let Ok(reverted) = Theme::load(&original) {
                    *theme = reverted;
                }
            }
            settings_modal::sync_theme_selection_to_applied(app);
        }

        SettingsKeyResult::Quit => {
            let original = app.settings_modal.original_theme.clone();
            if !original.is_empty() && theme.name != original {
                if let Ok(reverted) = Theme::load(&original) {
                    *theme = reverted;
                }
            }
            if let Ok(config) = Config::load_or_create(&Paths::discover()?) {
                settings_modal::open_settings_modal(app, &config, &original);
            }
            app.dispatch(papr_core::Command::Quit);
        }

        SettingsKeyResult::PreviewTheme(name) => {
            if let Ok(preview) = Theme::load(&name) {
                *theme = preview;
            }
        }
    }
    Ok(None)
}

fn apply_config_update(
    runtime: &mut Runtime,
    app: &mut App,
    config: &Config,
    theme: &mut Theme,
    senders: &ActionSenders,
) -> Result<()> {
    let new_theme = Theme::load(&config.theme).map_err(|e| anyhow::anyhow!("Theme load failed: {e}"))?;
    *theme = new_theme;

    runtime.pdf_viewer = config.pdf_viewer.clone().unwrap_or_else(default_pdf_viewer);
    app.pdf_viewer = runtime.pdf_viewer.clone();
    if app.pdf_viewer != "internal" && app.project_pane == ProjectPane::Preview {
        app.project_pane = ProjectPane::FileTree;
    }
    if let Some(projects_directory) = config.projects_directory.clone() {
        runtime.project_manager = ProjectManager::new(projects_directory)
            .map_err(|e| anyhow::anyhow!("projects directory: {e}"))?;
        app.projects = runtime.project_manager.list().unwrap_or_default();
        app.projects_selected = app.projects_selected.min(app.projects.len().saturating_sub(1));
    }

    let download_dir = config.download_path.clone().unwrap_or_else(|| runtime.default_downloads_dir.clone());
    let _ = std::fs::create_dir_all(&download_dir);
    let download_dir = std::fs::canonicalize(&download_dir).unwrap_or(download_dir);
    runtime.download_dir = download_dir.clone();

    let mut collection_roots = Vec::new();
    for root in &config.library_folders {
        collection_roots.push(std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()));
    }
    runtime.collection_roots = collection_roots.clone();

    let mut library_roots = collection_roots.clone();
    let download_inside = collection_roots.iter().any(|root| {
        download_dir.starts_with(root)
    });
    if !download_inside {
        library_roots.push(download_dir.clone());
    }
    if !library_roots.is_empty() {
        runtime.primary_library_root = library_roots[0].clone();
    }
    runtime.library_roots = library_roots;

    let old_sig = runtime.dashboard_keyword_signature.clone();
    runtime.dashboard_keywords = config.dashboard_keyword_list();
    runtime.dashboard_keyword_signature = dashboard_keyword_signature(&runtime.dashboard_keywords);
    let keywords_changed = old_sig != runtime.dashboard_keyword_signature;

    restart_runtime_watcher(runtime)?;
    refresh_library(runtime, app)?;
    refresh_organization(&runtime.database, &runtime.library_roots, app)?;
    refresh_dashboard(runtime, app)?;
    refresh_downloads(runtime, app);

    if let Ok(plugin_host) = PluginHost::discover(&runtime.plugins_dir, &config.enabled_plugins) {
        app.plugins = plugin_host.plugins();
        app.plugin_diagnostics = plugin_host.diagnostics().len();
        runtime.plugin_host = plugin_host;
    }

    if keywords_changed {
        refresh_dashboard_papers(runtime, senders, app)?;
    }

    Ok(())
}

#[allow(dead_code)]
fn handle_config_editor_key(
    app: &mut App,
    key: KeyEvent,
    runtime: &mut Runtime,
    theme: &mut Theme,
    senders: &ActionSenders,
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
                let mut action_to_return = None;
                if trimmed == "w" || trimmed == "wq" {
                    let toml_str = &app.config_editor_text;
                    match toml::from_str::<Config>(toml_str) {
                        Ok(new_config) => {
                            let canonical_toml = match toml::to_string_pretty(&new_config) {
                                Ok(toml) => toml,
                                Err(e) => {
                                    app.config_editor_error = Some(format!("Serialization failed: {e}"));
                                    return None;
                                }
                            };
                            if let Err(e) = std::fs::write(&runtime.config_file, &canonical_toml) {
                                app.config_editor_error = Some(format!("Write failed: {e}"));
                            } else {
                                reset_config_editor_buffer(app, canonical_toml);
                                app.config_editor_error = None;
                                app.toast = Some("Configuration saved and applied.".to_owned());
                                if let Err(e) = apply_config_update(runtime, app, &new_config, theme, senders) {
                                    app.config_editor_error = Some(format!("Apply failed: {e}"));
                                } else {
                                    action_to_return = Some(UiAction::Reindex);
                                }
                            }
                        }
                        Err(e) => {
                            app.config_editor_error = Some(format!("Invalid TOML: {e}"));
                        }
                    }
                }
                if trimmed == "q" || (trimmed == "wq" && app.config_editor_error.is_none()) {
                    if trimmed == "q" {
                        reload_config_editor_buffer(app, &runtime.config_file);
                    }
                    app.config_editor_focused = false;
                    app.content_focused = false;
                }
                if action_to_return.is_some() {
                    return action_to_return;
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

    if key.code == KeyCode::Char('?') {
        app.dispatch(Command::ToggleHelp);
        return None;
    }

    match key.code {
        KeyCode::Esc => {
            app.config_editor_focused = false;
        }
        KeyCode::Char('i') => {
            app.config_editor_insert_mode = true;
            reset_config_editor_goal_column(app);
        }
        KeyCode::Char(':') => {
            app.config_editor_command = Some(String::new());
        }
        KeyCode::Left | KeyCode::Char('h') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                app.config_editor_cursor = prev_word_boundary(&app.config_editor_text, app.config_editor_cursor);
            } else if app.config_editor_cursor > 0 {
                let mut prev = app.config_editor_cursor - 1;
                while prev > 0 && !app.config_editor_text.is_char_boundary(prev) {
                    prev -= 1;
                }
                if app.config_editor_text.as_bytes().get(prev) != Some(&b'\n') {
                    app.config_editor_cursor = prev;
                }
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Right | KeyCode::Char('l') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                app.config_editor_cursor = next_word_boundary(&app.config_editor_text, app.config_editor_cursor);
            } else if app.config_editor_cursor < app.config_editor_text.len() {
                let next = next_char_boundary(&app.config_editor_text, app.config_editor_cursor);
                if app.config_editor_text.as_bytes().get(app.config_editor_cursor) != Some(&b'\n') {
                    app.config_editor_cursor = next.min(app.config_editor_text.len());
                }
            }
            reset_config_editor_goal_column(app);
        }
        KeyCode::Home => {
            app.config_editor_cursor = config_editor_line_start(&app.config_editor_text, app.config_editor_cursor);
            reset_config_editor_goal_column(app);
        }
        KeyCode::End => {
            app.config_editor_cursor = config_editor_line_end(&app.config_editor_text, app.config_editor_cursor);
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
        KeyCode::PageUp => move_config_editor_page(app, -1),
        KeyCode::PageDown => move_config_editor_page(app, 1),
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
        KeyCode::Up => {
            move_config_editor_vertical(app, -1);
            return;
        }
        KeyCode::Down => {
            move_config_editor_vertical(app, 1);
            return;
        }
        KeyCode::PageUp => {
            move_config_editor_page(app, -1);
            return;
        }
        KeyCode::PageDown => {
            move_config_editor_page(app, 1);
            return;
        }
        _ => {}
    }
    if matches!(key.code, KeyCode::Backspace | KeyCode::Delete | KeyCode::Enter | KeyCode::Tab)
        || matches!(key.code, KeyCode::Char(_))
    {
        record_config_history(app);
    }
    match apply_editor_insert_key(
        &mut app.config_editor_text,
        &mut app.config_editor_cursor,
        key,
    ) {
        EditorInsertResult::ExitInsert => app.config_editor_insert_mode = false,
        EditorInsertResult::Ignored | EditorInsertResult::Moved | EditorInsertResult::Changed => {}
    }
    reset_config_editor_goal_column(app);
}

fn finalize_download_task(task: &mut DownloadTask) {
    if let Some(ref pdf_path) = task.pdf_path {
        let final_path = std::path::PathBuf::from(pdf_path);
        let temp_path = final_path.with_extension("pdf.part");
        if temp_path.exists() {
            let _ = std::fs::rename(&temp_path, &final_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::{TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use papr_core::{
        App, AppMode, BookmarkSummary, CollectionSummary, Database, DiscoveryState, DownloadStatus,
        CitationEntry, CitationSource, DownloadTask, LibraryPaper, Page, PaperNote, Project, ProjectPane, RemotePaper,
    };
    use papr_core::models::AuthorSummary;

    use super::{
        UiAction, build_config_editor_view, cursor_visual_position,
        handle_config_editor_insert_key, handle_downloads_key, handle_key, handle_paper_detail_key, parse_command,
        keyword_representation_targets, move_config_editor_page, refresh_downloads_from_dir,
        accept_project_completion, create_project_file, is_project_text_file, project_tree_entries,
        insert_project_bibtex_text,
        reload_config_editor_buffer, select_dashboard_papers, shuffle_daily_bucket,
        update_project_completions, merge_enriched_remote_paper, run_terminal_command,
        sanitize_terminal_output, PaperTarget,
    };

    #[test]
    fn test_word_wise_and_line_editing_navigation() {
        use super::{edit_text, prev_word_boundary, next_word_boundary};
        
        let text = "hello world  rust_programming  123";
        // Test word boundaries
        assert_eq!(next_word_boundary(text, 0), 6); // start of "world"
        assert_eq!(next_word_boundary(text, 6), 13); // start of "rust_programming"
        assert_eq!(next_word_boundary(text, 13), 31); // start of "123"
        
        assert_eq!(prev_word_boundary(text, 34), 31); // start of "123"
        assert_eq!(prev_word_boundary(text, 31), 13); // start of "rust_programming"
        assert_eq!(prev_word_boundary(text, 13), 6); // start of "world"
        
        let mut buffer = "first second\nthird fourth".to_owned();
        let mut cursor = 6;
        
        // Test Home
        edit_text(&mut buffer, &mut cursor, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(cursor, 0);
        
        // Test End
        edit_text(&mut buffer, &mut cursor, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(cursor, 12); // end of "first second"
        
        // Test Ctrl + Left
        edit_text(&mut buffer, &mut cursor, KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL));
        assert_eq!(cursor, 6); // start of "second"
        
        // Test Ctrl + Right
        edit_text(&mut buffer, &mut cursor, KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL));
        assert_eq!(cursor, 13);
    }

    #[test]
    fn enrichment_projection_keeps_the_displayed_abstract_when_provider_has_none() {
        let mut displayed = remote_paper("https://arxiv.org/abs/2602.00004", "Original title");
        displayed.abstract_text = "Abstract fetched from arXiv.".into();
        let mut provider = displayed.clone();
        provider.title = "Enriched title".into();
        provider.abstract_text = "  ".into();

        let merged = merge_enriched_remote_paper(&displayed, &provider);

        assert_eq!(merged.title, "Enriched title");
        assert_eq!(merged.abstract_text, "Abstract fetched from arXiv.");
    }

    #[test]
    fn control_b_opens_browse_papr() {
        let mut app = App::default();
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.mode, AppMode::CommandPalette);
    }

    #[test]
    fn test_settings_workspace_ctrl_b_transfers_focus_to_command_palette() {
        let mut app = App {
            page: papr_core::Page::Settings,
            content_focused: true,
            mode: AppMode::Normal,
            ..App::default()
        };

        // Press Ctrl+B while in settings workspace
        let res = crate::settings_modal::handle_settings_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
        );
        assert!(matches!(res, crate::settings_modal::SettingsKeyResult::Handled));
        assert_eq!(app.mode, AppMode::CommandPalette);

        // When app.mode == AppMode::CommandPalette, event loop condition
        // (app.page == Page::Settings && app.content_focused && app.mode == AppMode::Normal) is false.
        assert!(!(app.page == papr_core::Page::Settings && app.content_focused && app.mode == AppMode::Normal));

        // Subsequent keys go to handle_key for CommandPalette
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        );
        assert_eq!(app.palette_query, "d");

        // Esc closes CommandPalette and restores normal focus in Settings workspace
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        );
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.page, papr_core::Page::Settings);
        assert!(app.content_focused);
    }

    #[test]
    fn test_settings_workspace_question_mark_transfers_focus_to_help() {
        let mut app = App {
            page: papr_core::Page::Settings,
            content_focused: true,
            mode: AppMode::Normal,
            ..App::default()
        };

        // Press '?' while in settings workspace
        let res = crate::settings_modal::handle_settings_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert!(matches!(res, crate::settings_modal::SettingsKeyResult::Handled));
        assert_eq!(app.mode, AppMode::Help);

        // While app.mode == AppMode::Help, key routing condition for settings workspace is false
        assert!(!(app.page == papr_core::Page::Settings && app.content_focused && app.mode == AppMode::Normal));

        // Subsequent keys go to handle_key for Help mode (scrolling)
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(app.help_scroll, 1);

        // Pressing '?' or Esc closes Help mode and restores normal focus in Settings workspace
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.page, papr_core::Page::Settings);
        assert!(app.content_focused);
    }

    #[test]
    fn test_startup_page_config_initializes_app_page_and_sidebar_index() {
        let mut config = papr_core::Config::default();
        config.startup_page = "reading_queue".into();

        let initial_page = Page::from_config_str(&config.startup_page).unwrap_or(Page::Dashboard);
        let initial_sidebar_index = Page::ALL
            .iter()
            .position(|&p| p == initial_page)
            .unwrap_or(0);

        let app = App {
            page: initial_page,
            sidebar_index: initial_sidebar_index,
            ..App::default()
        };

        assert_eq!(app.page, Page::ReadingQueue);
        assert_eq!(app.sidebar_index, 3);
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
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        assert_eq!(app.palette_selected, 2);
    }

    #[test]
    fn palette_filtering_and_typing() {
        let mut app = App {
            mode: AppMode::CommandPalette,
            ..App::default()
        };

        // Type 'l' to filter (should match Library, etc.)
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        assert_eq!(app.palette_query, "l");
        let items = app.filtered_palette_items();
        assert!(items.contains(&papr_core::Page::Library));

        // Down arrow should move selection
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        );
        // Press Enter to activate
        let action = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        );
        assert!(action.is_none());
        assert_eq!(app.mode, AppMode::Normal);
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
    fn remote_workspace_detail_ignores_bookmark_note_tag_and_group_keys() {
        for page in [Page::Dashboard, Page::Discover] {
            let mut app = App {
                page,
                content_focused: true,
                mode: AppMode::PaperDetail,
                today_papers: vec![remote_paper("https://arxiv.org/abs/dashboard", "Dashboard paper")],
                discovery: DiscoveryState {
                    results: vec![remote_paper("https://arxiv.org/abs/discover", "Discover paper")],
                    ..DiscoveryState::default()
                },
                ..App::default()
            };

            for key in ['B', 'n', 't', 'g'] {
                let action = handle_key(
                    &mut app,
                    KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE),
                );
                assert!(action.is_none(), "{key} should be ignored in {page:?}");
                assert_eq!(app.mode, AppMode::PaperDetail);
            }
        }
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
        for (index, page) in Page::ALL
            .into_iter()
            .enumerate()
            .filter(|(_, page)| *page != Page::Projects)
        {
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
    fn test_workspace_search_navigation_and_restore() {
        use std::collections::HashSet;
        for page in [
            Page::Library,
            Page::Downloads,
            Page::Collections,
            Page::Authors,
            Page::Bookmarks,
            Page::Notes,
            Page::ReadingQueue,
        ] {
            let mut app = App {
                page,
                content_focused: true,
                mode: AppMode::WorkspaceSearch,
                workspace_query: "test query".to_string(),
                workspace_query_cursor: 10,
                active_search_workspaces: {
                    let mut s = HashSet::new();
                    s.insert(page);
                    s
                },
                ..App::default()
            };

            // Pressing Left Arrow in WorkspaceSearch mode when cursor > 0 should move cursor left
            let action = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
            assert!(action.is_none());
            assert!(app.content_focused);
            assert_eq!(app.mode, AppMode::WorkspaceSearch);
            assert_eq!(app.workspace_query_cursor, 9);

            // Pressing Left Arrow when cursor == 0 should transfer focus to navigation pane
            app.workspace_query_cursor = 0;
            let action = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
            assert!(action.is_none());
            assert!(!app.content_focused);
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.workspace_query, "test query");
            assert!(app.active_search_workspaces.contains(&page));

            // Move to another page
            app.sidebar_index = 0; // Dashboard
            app.page = Page::Dashboard;

            // Enter Dashboard - should not restore search mode since it's not active
            app.dispatch(papr_core::Command::Open);
            assert!(app.content_focused);
            assert_eq!(app.mode, AppMode::Normal);

            // Go back to sidebar, select original page, and enter it
            app.content_focused = false;
            if let Some(index) = Page::ALL.iter().position(|&p| p == page) {
                app.sidebar_index = index;
            }
            app.dispatch(papr_core::Command::Open);

            // It should enter the workspace and restore search mode and its state!
            assert!(app.content_focused);
            assert_eq!(app.mode, AppMode::WorkspaceSearch);
            assert_eq!(app.workspace_query, "test query");
            assert_eq!(app.workspace_query_cursor, 0);
        }
    }

    #[test]
    fn test_discover_search_navigation() {
        let mut app = App {
            page: Page::Discover,
            content_focused: true,
            mode: AppMode::Search,
            discovery: DiscoveryState {
                query: "test query".to_string(),
                query_cursor: 10,
                ..DiscoveryState::default()
            },
            ..App::default()
        };

        // Pressing Left Arrow when cursor > 0 should move cursor left
        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(action.is_none());
        assert!(app.content_focused);
        assert_eq!(app.mode, AppMode::Search);
        assert_eq!(app.discovery.query_cursor, 9);

        // Pressing Left Arrow when cursor == 0 should transfer focus to navigation pane
        app.discovery.query_cursor = 0;
        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(action.is_none());
        assert!(!app.content_focused);
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn discover_results_left_returns_to_navigation_and_preserves_state() {
        for from_filter in [false, true] {
            let mut app = App {
                page: Page::Discover,
                sidebar_index: Page::ALL.iter().position(|&page| page == Page::Discover).unwrap(),
                content_focused: true,
                mode: if from_filter { AppMode::DiscoverFilter } else { AppMode::Search },
                discovery: DiscoveryState {
                    query: "quantum search".into(),
                    query_cursor: 14,
                    filter: "paper".into(),
                    filter_cursor: 5,
                    ..DiscoveryState::default()
                },
                ..App::default()
            };
            app.discovery.set_results(vec![
                remote_paper("first", "First paper"),
                remote_paper("second", "Second paper"),
            ]);
            app.discovery.filter = "paper".into();
            app.discovery.filter_cursor = app.discovery.filter.len();
            app.discovery.rebuild_filter();

            // Both inputs move into the same results pane.
            assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)).is_none());
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.discovery.selected, 0);
            app.discovery.scroll = 1;

            // Left leaves the results pane for navigation without entering either input.
            assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE)).is_none());
            assert!(!app.content_focused);
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.discovery.query, "quantum search");
            assert_eq!(app.discovery.filter, "paper");
            assert_eq!(app.discovery.selected, 0);
            assert_eq!(app.discovery.scroll, 1);

            // Leaving Discover and returning restores the existing results-pane state.
            app.dispatch(papr_core::Command::MoveDown);
            app.dispatch(papr_core::Command::MoveUp);
            app.dispatch(papr_core::Command::Open);
            assert!(app.content_focused);
            assert_eq!(app.page, Page::Discover);
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.discovery.query, "quantum search");
            assert_eq!(app.discovery.filter, "paper");
            assert_eq!(app.discovery.selected, 0);
            assert_eq!(app.discovery.scroll, 1);
        }
    }

    #[test]
    fn discover_control_arrows_switch_cached_result_pages() {
        let mut app = App {
            page: Page::Discover,
            content_focused: true,
            ..App::default()
        };
        app.discovery.set_results(
            (0..51)
                .map(|index| {
                    remote_paper(
                        &format!("https://arxiv.org/abs/{index}"),
                        &format!("Paper {index}"),
                    )
                })
                .collect(),
        );

        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::CONTROL)).is_none());
        assert_eq!(app.discovery.page, 1);
        assert_eq!(app.discovery.current_page_results()[0].title, "Paper 50");
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::CONTROL)).is_none());
        assert_eq!(app.discovery.page, 0);
        assert_eq!(app.discovery.current_page_results()[0].title, "Paper 0");
    }

    #[test]
    fn test_workspace_search_greater_than_and_arrow_keys() {
        for page in [
            Page::Library,
            Page::Downloads,
            Page::Collections,
            Page::Authors,
            Page::Bookmarks,
            Page::Notes,
            Page::ReadingQueue,
        ] {
            let mut app = App {
                page,
                content_focused: true,
                mode: AppMode::Normal,
                workspace_query: "some query".to_string(),
                workspace_query_cursor: 10,
                library: papr_core::LibraryState {
                    selected: 5,
                    ..papr_core::LibraryState::default()
                },
                download_selected: 5,
                collection_selected: 5,
                collection_paper_selected: 5,
                author_selected: 5,
                author_paper_selected: 5,
                bookmark_selected: 5,
                notes_selected: 5,
                reading_queue_selected: 5,
                ..App::default()
            };
            app.active_collection = Some(papr_core::models::CollectionSummary {
                id: 1,
                name: "Collection".into(),
                paper_count: 5,
                folder_path: None,
            });
            app.active_author = Some(papr_core::models::AuthorSummary {
                id: 2,
                name: "Author".into(),
                paper_count: 5,
            });

            // 1. When workspace has focus, pressing '>' should return focus to search bar
            let action = handle_key(&mut app, KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
            assert!(action.is_none());
            assert!(app.content_focused);
            assert_eq!(app.mode, AppMode::WorkspaceSearch);
            assert!(app.active_search_workspaces.contains(&page));

            // 2. Pressing '>' again in search mode should return focus to workspace
            let action = handle_key(&mut app, KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
            assert!(action.is_none());
            assert!(app.content_focused);
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.workspace_query, "some query");
            assert!(!app.active_search_workspaces.contains(&page));

            // 3. Enter search mode again and test Down Arrow
            handle_key(&mut app, KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
            assert_eq!(app.mode, AppMode::WorkspaceSearch);

            // Pressing Down Arrow should move focus to first visible paper in filtered list (index 0)
            let action = handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            assert!(action.is_none());
            assert!(app.content_focused);
            assert_eq!(app.mode, AppMode::Normal);
            assert!(!app.active_search_workspaces.contains(&page));

            match page {
                Page::Library => assert_eq!(app.library.selected, 0),
                Page::Downloads => assert_eq!(app.download_selected, 0),
                Page::Collections => assert_eq!(app.collection_paper_selected, 0),
                Page::Authors => assert_eq!(app.author_paper_selected, 0),
                Page::Bookmarks => assert_eq!(app.bookmark_selected, 0),
                Page::Notes => assert_eq!(app.notes_selected, 0),
                Page::ReadingQueue => assert_eq!(app.reading_queue_selected, 0),
                _ => {}
            }

            // 4. Enter search mode and test Esc
            handle_key(&mut app, KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
            assert_eq!(app.mode, AppMode::WorkspaceSearch);

            let action = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert!(action.is_none());
            assert!(app.content_focused);
            assert_eq!(app.mode, AppMode::Normal);
            assert_eq!(app.workspace_query, "some query");
            assert!(!app.active_search_workspaces.contains(&page));
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

    fn project_editor_app(text: &str, cursor: usize) -> App {
        App {
            page: Page::Projects,
            content_focused: true,
            active_project: Some(Project {
                name: "keyboard-test".into(),
                path: std::path::PathBuf::from("keyboard-test"),
                opened_at: 0,
            }),
            project_pane: ProjectPane::Editor,
            project_editor_text: text.into(),
            project_editor_cursor: cursor,
            project_editor_insert_mode: true,
            project_editor_wrap_width: 80,
            project_editor_viewport_height: 20,
            ..App::default()
        }
    }

    #[test]
    fn citation_completion_replaces_only_the_current_key() {
        let mut app = project_editor_app("\\cite{newton1687, eins}", 22);
        let source = CitationSource::new(vec![CitationEntry {
            key: "einstein1905".into(), author: "Albert Einstein".into(),
            title: "Moving Bodies".into(), year: "1905".into(),
        }]);
        update_project_completions(&mut app, Some(&source));
        assert!(accept_project_completion(&mut app));
        assert_eq!(app.project_editor_text, "\\cite{newton1687, einstein1905}");
    }

    #[test]
    fn project_editor_insert_mode_handles_editing_keys_without_workspace_interception() {
        let mut app = project_editor_app("ab", 1);

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.project_editor_text, "a\tb");
        assert_eq!(app.project_editor_cursor, 2);
        assert!(app.project_editor_dirty);

        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(app.project_editor_text, "ab");
        assert_eq!(app.project_editor_cursor, 1);

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
        );
        assert_eq!(app.project_editor_text, "a\nqb");
        assert_eq!(app.project_editor_cursor, 3);
        assert!(app.project_editor_insert_mode);
        assert!(!app.should_quit);
    }

    #[test]
    fn bibtex_paste_preserves_the_clipboard_text_exactly() {
        let citation = "@article{doe2026,\n  title = {Exact {BibTeX} Formatting},\n  author = {Doe, Jane},\n}\n";
        let mut app = project_editor_app("before\nafter", "before\n".len());

        insert_project_bibtex_text(&mut app, citation);

        assert_eq!(app.project_editor_text, format!("before\n{citation}after"));
        assert_eq!(app.project_editor_cursor, "before\n".len() + citation.len());
        assert!(app.project_editor_dirty);
    }

    #[test]
    fn ctrl_t_opens_the_terminal_command_palette() {
        let mut app = App::default();

        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.mode, AppMode::TerminalCommand);
        assert!(app.terminal_command.is_empty());
        assert!(app.terminal_command_output.is_empty());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn terminal_command_uses_the_open_project_directory() {
        let root = std::env::temp_dir().join(format!("papr-terminal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut app = project_editor_app("", 0);
        app.active_project = Some(Project { name: "terminal".into(), path: root.clone(), opened_at: 0 });
        app.terminal_command_directory = Some(root.clone());
        app.terminal_command = "pwd".into();
        app.terminal_command_cursor = app.terminal_command.len();

        run_terminal_command(&mut app);

        assert!(app.terminal_command_output.contains(root.to_string_lossy().as_ref()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn terminal_clear_and_control_sequences_stay_inside_the_palette() {
        let mut app = App::default();
        app.terminal_command_output = "previous output".into();
        app.terminal_command = "clear".into();

        run_terminal_command(&mut app);

        assert!(app.terminal_command_output.is_empty());
        assert!(app.terminal_command.is_empty());
        assert_eq!(sanitize_terminal_output("safe\u{1b}[2Jtext"), "safe[2Jtext");
    }

    #[test]
    fn help_shortcut_is_global_and_editor_insert_mode_keeps_question_mark() {
        let mut library = App {
            page: Page::Library,
            content_focused: true,
            ..App::default()
        };
        let _ = handle_key(
            &mut library,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert_eq!(library.mode, AppMode::Help);

        let mut projects = project_editor_app("", 0);
        projects.project_editor_insert_mode = false;
        let _ = handle_key(
            &mut projects,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert_eq!(projects.mode, AppMode::Help);

        let mut insert = project_editor_app("", 0);
        let _ = handle_key(
            &mut insert,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert_eq!(insert.mode, AppMode::Normal);
        assert_eq!(insert.project_editor_text, "?");
    }

    #[test]
    fn help_opens_without_navigating_from_dashboard_or_discover() {
        let mut dashboard = App {
            page: Page::Dashboard,
            content_focused: false,
            sidebar_index: 0,
            ..App::default()
        };
        let _ = handle_key(
            &mut dashboard,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert_eq!(dashboard.mode, AppMode::Help);
        assert_eq!(dashboard.page, Page::Dashboard);
        assert!(!dashboard.content_focused);
        assert_eq!(dashboard.sidebar_index, 0);

        let mut discover = App {
            page: Page::Discover,
            content_focused: true,
            sidebar_index: 1,
            ..App::default()
        };
        discover.discovery.selected = 3;
        let _ = handle_key(
            &mut discover,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        );
        assert_eq!(discover.mode, AppMode::Help);
        assert_eq!(discover.page, Page::Discover);
        assert_eq!(discover.discovery.selected, 3);
        assert_eq!(discover.sidebar_index, 1);
    }

    #[test]
    fn help_preserves_dashboard_and_discover_paper_detail_views() {
        for page in [Page::Dashboard, Page::Discover] {
            let mut app = App {
                page,
                content_focused: true,
                mode: AppMode::PaperDetail,
                paper_detail_scroll: 9,
                ..App::default()
            };

            let _ = handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
            );
            assert_eq!(app.mode, AppMode::Help);
            assert_eq!(app.page, page);
            assert_eq!(app.paper_detail_scroll, 9);

            let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert_eq!(app.mode, AppMode::PaperDetail);
            assert_eq!(app.page, page);
            assert_eq!(app.paper_detail_scroll, 9);
        }
    }

    #[test]
    fn paper_detail_handler_never_routes_help_to_back_navigation() {
        let mut app = App {
            page: Page::Discover,
            content_focused: true,
            mode: AppMode::PaperDetail,
            paper_detail_scroll: 4,
            discovery: DiscoveryState {
                selected: 2,
                ..DiscoveryState::default()
            },
            ..App::default()
        };

        let action = handle_paper_detail_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::SHIFT),
        );
        assert!(action.is_none());
        assert_eq!(app.mode, AppMode::Help);
        assert_eq!(app.help_return_mode, AppMode::PaperDetail);
        assert_eq!(app.paper_detail_scroll, 4);
        assert_eq!(app.discovery.selected, 2);
    }

    #[test]
    fn slash_opens_discover_search_from_projects_but_inserts_in_editor_insert_mode() {
        let mut projects = project_editor_app("", 0);
        projects.project_editor_insert_mode = false;
        let _ = handle_key(
            &mut projects,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        assert_eq!(projects.page, Page::Discover);
        assert_eq!(projects.mode, AppMode::Search);

        let mut insert = project_editor_app("", 0);
        let _ = handle_key(
            &mut insert,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        );
        assert_eq!(insert.page, Page::Projects);
        assert_eq!(insert.mode, AppMode::Normal);
        assert_eq!(insert.project_editor_text, "/");
    }

    #[test]
    fn project_editor_backspace_and_delete_remove_complete_unicode_characters() {
        let mut backspace = project_editor_app("aéb", 3);
        let _ = handle_key(
            &mut backspace,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert_eq!(backspace.project_editor_text, "ab");
        assert_eq!(backspace.project_editor_cursor, 1);

        let mut delete = project_editor_app("aéb", 1);
        let _ = handle_key(
            &mut delete,
            KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE),
        );
        assert_eq!(delete.project_editor_text, "ab");
        assert_eq!(delete.project_editor_cursor, 1);
    }

    #[test]
    fn project_editor_navigation_keys_move_without_mutating_the_buffer() {
        let mut app = project_editor_app("abc\ndef", 2);

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.project_editor_cursor, 1);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(app.project_editor_cursor, 3);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.project_editor_cursor, 7);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.project_editor_cursor, 4);
        assert_eq!(app.project_editor_text, "abc\ndef");
        assert!(!app.project_editor_dirty);
    }

    #[test]
    fn editor_view_expands_tabs_and_maps_the_cursor_to_the_visible_tab_stop() {
        let mut scroll = 0;
        let view = build_config_editor_view("\tX", 1, 20, 4, &mut scroll);

        assert_eq!(view.cursor_row, 0);
        assert_eq!(view.cursor_col, 4);
        assert_eq!(view.lines, vec!["  1     X"]);
    }

    #[test]
    fn project_alt_number_shortcuts_select_available_panes_directly() {
        let mut app = project_editor_app("unchanged", 4);
        app.project_editor_path = Some(std::path::PathBuf::from("keyboard-test/main.tex"));
        app.project_editor_insert_mode = false;
        app.pdf_viewer_path = Some(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));

        for (number, expected) in [
            ('1', ProjectPane::FileTree),
            ('2', ProjectPane::Editor),
            ('3', ProjectPane::Preview),
            ('4', ProjectPane::Build),
        ] {
            let _ = handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(number), KeyModifiers::ALT),
            );
            assert_eq!(app.project_pane, expected);
        }
    }

    #[test]
    fn project_alt_shortcuts_are_inactive_in_insert_mode_and_unavailable_panes_are_safe() {
        let mut app = project_editor_app("abc", 1);
        app.project_editor_path = Some(std::path::PathBuf::from("keyboard-test/main.tex"));

        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('4'), KeyModifiers::ALT),
        );
        assert_eq!(app.project_pane, ProjectPane::Editor);
        assert_eq!(app.project_editor_text, "abc");
        assert_eq!(app.project_editor_cursor, 1);
        assert!(app.project_editor_insert_mode);

        app.active_project = None;
        app.project_pane = ProjectPane::ProjectList;
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT),
        );
        assert_eq!(app.project_pane, ProjectPane::ProjectList);
        assert!(app.toast.as_deref().is_some_and(|message| message.contains("Open a project")));
    }

    #[test]
    fn project_list_right_opens_and_x_requests_confirmed_deletion() {
        let mut app = project_editor_app("", 0);
        app.project_pane = ProjectPane::ProjectList;
        let project = app.active_project.clone().unwrap();
        app.projects = vec![project.clone()];

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert!(matches!(action, Some(UiAction::OpenProject(opened)) if opened == project));

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert!(matches!(action, Some(UiAction::ConfirmDeleteProject(selected)) if selected == project));
    }

    #[test]
    fn project_creation_uses_a_named_modal_with_standard_text_editing() {
        let mut app = App {
            page: Page::Projects,
            content_focused: true,
            project_pane: ProjectPane::ProjectList,
            ..App::default()
        };

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(app.mode, AppMode::ProjectCreate);
        for character in ['p', 'a', 'p', 'r'] {
            let _ = handle_key(
                &mut app,
                KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            );
        }
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(app.project_rename_input, "pap");

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, Some(UiAction::CreateProject(name)) if name == "pap"));
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn discover_filter_refines_results_without_starting_a_search() {
        let mut title_match = remote_paper("title", "Genome assembly");
        title_match.authors = vec!["Ada Author".into()];
        let mut author_match = remote_paper("author", "Other paper");
        author_match.authors = vec!["Genome Researcher".into()];
        let mut abstract_match = remote_paper("abstract", "Third genome paper");
        abstract_match.abstract_text = "A genome-scale analysis.".into();
        let mut app = App {
            page: Page::Discover,
            content_focused: true,
            ..App::default()
        };
        app.discovery.set_results(vec![author_match, abstract_match, title_match]);

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
        assert!(action.is_none());
        assert_eq!(app.mode, AppMode::DiscoverFilter);
        for character in "genome".chars() {
            assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)).is_none());
        }
        assert_eq!(app.discovery.filtered_result_count(), 3);
        assert_eq!(app.discovery.selected_paper().map(|paper| paper.id.as_str()), Some("author"));

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, AppMode::Normal);
        assert_eq!(app.discovery.filter, "genome");
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Char('>'), KeyModifiers::NONE));
        assert_eq!(app.mode, AppMode::DiscoverFilter);
        for _ in 0.."genome".len() {
            let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        }
        assert!(app.discovery.filter.is_empty());
        assert_eq!(app.discovery.filtered_result_count(), 3);
    }

    #[test]
    fn file_tree_new_file_modal_uses_standard_text_editing() {
        let mut app = project_editor_app("", 0);
        app.project_pane = ProjectPane::FileTree;

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        assert_eq!(app.mode, AppMode::ProjectFileCreate);
        for character in ['n', 'o', 't', 'e', 's', '.', 'm', 'd'] {
            let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
        assert_eq!(app.project_rename_input, "notes.m");

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, Some(UiAction::CreateProjectFile(name)) if name == "notes.m"));
        assert_eq!(app.mode, AppMode::Normal);
    }

    #[test]
    fn creates_nested_project_files_without_overwriting() {
        let root = std::env::temp_dir().join(format!("papr-file-create-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let created = create_project_file(&root, "chapters/introduction.tex").unwrap();
        assert_eq!(created, root.join("chapters/introduction.tex"));
        assert!(created.exists());
        assert!(is_project_text_file(&created));
        assert!(project_tree_entries(&root).contains(&root.join("chapters")));
        let folder = create_project_file(&root, "assets/").unwrap();
        assert_eq!(folder, root.join("assets"));
        assert!(folder.is_dir());
        assert!(project_tree_entries(&root).contains(&folder));
        let image = root.join("figure.webp");
        std::fs::write(&image, []).unwrap();
        assert!(project_tree_entries(&root).contains(&image));
        assert!(create_project_file(&root, "chapters/introduction.tex").unwrap_err().contains("already exists"));
        assert!(create_project_file(&root, "../outside.tex").is_err());
        assert!(create_project_file(&root, "/tmp/outside.tex").is_err());

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_tree_enters_folders_and_left_returns_to_the_parent() {
        let root = std::env::temp_dir().join(format!("papr-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let folder = root.join("assets");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("logo.tex"), "logo").unwrap();

        let mut app = project_editor_app("", 0);
        app.active_project = Some(Project { name: "tree".into(), path: root.clone(), opened_at: 0 });
        app.project_pane = ProjectPane::FileTree;
        app.project_tree_dir = Some(root.clone());
        app.project_files = project_tree_entries(&root);

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.project_tree_dir.as_deref(), Some(folder.as_path()));
        assert_eq!(app.project_files, vec![folder.join("logo.tex")]);

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.project_tree_dir.as_deref(), Some(root.as_path()));
        assert_eq!(app.project_files, vec![folder.clone()]);
        assert_eq!(app.project_file_selected, 0);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn file_tree_x_requests_confirmation_for_the_selected_entry() {
        let mut app = project_editor_app("", 0);
        let folder = std::path::PathBuf::from("keyboard-test/assets");
        app.project_pane = ProjectPane::FileTree;
        app.project_files = vec![folder.clone()];

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));

        assert!(matches!(action, Some(UiAction::ConfirmDeleteProjectEntry(path)) if path == folder));
    }

    #[test]
    fn file_tree_r_opens_the_entry_rename_prompt() {
        let mut app = project_editor_app("", 0);
        let folder = std::path::PathBuf::from("keyboard-test/assets");
        app.project_pane = ProjectPane::FileTree;
        app.project_files = vec![folder.clone()];

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Char('R'), KeyModifiers::NONE));

        assert_eq!(app.mode, AppMode::ProjectEntryRename);
        assert_eq!(app.project_rename_input, "assets");
        assert_eq!(app.project_entry_rename_path, Some(folder));
    }

    #[test]
    fn project_left_navigation_follows_file_tree_then_project_list_hierarchy() {
        let mut app = project_editor_app("", 0);
        let active = app.active_project.clone().unwrap();
        app.projects = vec![
            Project { name: "other".into(), path: "other".into(), opened_at: 0 },
            active,
        ];
        app.projects_selected = 0;
        app.project_pane = ProjectPane::FileTree;

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(app.content_focused);
        assert_eq!(app.project_pane, ProjectPane::ProjectList);
        assert_eq!(app.projects_selected, 1);
        assert!(app.active_project.is_some());

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(!app.content_focused);
        assert_eq!(app.project_pane, ProjectPane::ProjectList);
        assert_eq!(app.projects_selected, 1);

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert!(!app.content_focused);
        assert_eq!(app.projects_selected, 1);
    }

    #[test]
    fn escape_returns_to_file_tree_before_exiting_the_project() {
        let mut app = project_editor_app("abc", 1);
        let active = app.active_project.clone().unwrap();
        app.projects = vec![
            Project { name: "other".into(), path: "other".into(), opened_at: 0 },
            active,
        ];
        app.projects_selected = 0;

        for pane in [ProjectPane::Editor, ProjectPane::Build, ProjectPane::Preview] {
            app.project_pane = pane;
            app.project_editor_insert_mode = pane == ProjectPane::Editor;

            let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

            assert_eq!(app.project_pane, ProjectPane::FileTree);
            assert!(app.content_focused);
            assert!(!app.project_editor_insert_mode);
        }

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.project_pane, ProjectPane::ProjectList);
        assert!(app.content_focused);
        assert_eq!(app.projects_selected, 1);
    }

    #[test]
    fn file_tree_right_arrow_opens_the_selected_file() {
        let mut app = project_editor_app("", 0);
        let file = std::path::PathBuf::from("keyboard-test/references.bib");
        app.project_pane = ProjectPane::FileTree;
        app.project_files = vec![file.clone()];

        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        assert!(matches!(action, Some(UiAction::OpenProjectFile(path)) if path == file));
    }

    #[test]
    fn project_build_and_preview_arrows_only_navigate_their_content() {
        let mut app = project_editor_app("abc", 1);
        app.project_editor_insert_mode = false;
        app.project_build_errors = (0..8).map(|line| format!("error {line}")).collect();
        app.project_build_viewport_height = 2;
        app.project_pane = ProjectPane::Build;

        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.project_build_scroll, 1);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        assert_eq!(app.project_build_scroll, 3);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(app.project_build_scroll, 0);

        app.project_pane = ProjectPane::Preview;
        app.pdf_viewer_total_pages = 5;
        app.pdf_viewer_page = 3;
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.pdf_viewer_page, 2);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.pdf_viewer_page, 3);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert_eq!(app.pdf_viewer_page, 5);
        let _ = handle_key(&mut app, KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.pdf_viewer_page, 5);
    }

    #[test]
    fn downloads_workspace_retries_failed_download() {
        let paper = RemotePaper {
            id: "retry_id".into(),
            title: "Retry Paper".into(),
            authors: vec!["Researcher".into()],
            abstract_text: "Abstract".into(),
            published: Utc::now(),
            updated: Utc::now(),
            categories: vec!["cs.DL".into()],
            pdf_url: Some("http://example.com/retry.pdf".into()),
            doi: None,
            journal_ref: None,
        };
        let mut app = App {
            page: Page::Downloads,
            content_focused: true,
            downloads: vec![DownloadTask {
                id: "retry_id".into(),
                title: "Retry Paper".into(),
                downloaded: 0,
                total: None,
                paper_id: None,
                pdf_path: Some("/tmp/retry_path.pdf".into()),
                status: DownloadStatus::Failed("Network Error".into()),
                remote_paper: Some(paper.clone()),
                failed_at: Some(std::time::Instant::now()),
            }],
            ..App::default()
        };
        let action = handle_downloads_key(&mut app, KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(action.is_some());
        if let Some(UiAction::RetryDownload { id, paper: retry_paper }) = action {
            assert_eq!(id, "retry_id");
            assert_eq!(retry_paper.title, "Retry Paper");
        } else {
            panic!("Expected UiAction::RetryDownload");
        }
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
    fn editor_page_navigation_moves_by_the_visible_row_count() {
        let mut app = App {
            config_editor_text: "abcdefghijklmnop".into(),
            config_editor_wrap_width: 4,
            config_editor_viewport_height: 3,
            ..App::default()
        };

        move_config_editor_page(&mut app, 1);
        assert_eq!(app.config_editor_cursor, 12);

        move_config_editor_page(&mut app, -1);
        assert_eq!(app.config_editor_cursor, 0);
    }

    #[test]
    fn reloading_config_editor_discards_the_entire_unsaved_buffer_state() {
        let config_file = std::env::temp_dir().join(format!(
            "papr-config-editor-reload-{}-{}-{}.toml",
            std::process::id(),
            Utc::now().timestamp_micros(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&config_file, "theme = \"paper\"\n").unwrap();

        let mut app = App {
            config_editor_text: "unsaved = true".into(),
            config_editor_cursor: 7,
            config_editor_insert_mode: true,
            config_editor_error: Some("Invalid TOML".into()),
            config_editor_scroll: 4,
            config_editor_history: vec!["original = true".into(), "unsaved = true".into()],
            config_editor_history_idx: 1,
            config_editor_command: Some("q".into()),
            config_editor_goal_column: Some(3),
            ..App::default()
        };

        reload_config_editor_buffer(&mut app, &config_file);

        assert_eq!(app.config_editor_text, "theme = \"paper\"\n");
        assert_eq!(app.config_editor_cursor, 0);
        assert_eq!(app.config_editor_scroll, 0);
        assert_eq!(app.config_editor_history, vec!["theme = \"paper\"\n"]);
        assert_eq!(app.config_editor_history_idx, 0);
        assert!(!app.config_editor_insert_mode);
        assert!(app.config_editor_command.is_none());
        assert!(app.config_editor_error.is_none());
        assert!(app.config_editor_goal_column.is_none());

        fs::remove_file(config_file).unwrap();
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
                    remote_paper: None,
                    failed_at: None,
                },
                DownloadTask {
                    id: "running".into(),
                    title: "Running".into(),
                    downloaded: 5,
                    total: Some(10),
                    paper_id: None,
                    pdf_path: None,
                    status: DownloadStatus::Running,
                    remote_paper: None,
                    failed_at: None,
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
    fn unread_keybind_targets_selected_paper_across_workspaces() {
        let library_paper = LibraryPaper {
            id: 11,
            title: "Library Paper".into(),
            authors: "Researcher".into(),
            doi: None,
            arxiv_id: None,
            pdf_path: Some("/tmp/library.pdf".into()),
            file_size: Some(1),
            reading_status: "read".into(),
            is_favorite: false,
        };

        let mut library_app = App {
            page: Page::Library,
            content_focused: true,
            library: papr_core::LibraryState {
                papers: vec![library_paper.clone()],
                selected: 0,
                ..papr_core::LibraryState::default()
            },
            ..App::default()
        };
        assert!(matches!(
            handle_key(&mut library_app, KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)),
            Some(UiAction::MarkUnread(11))
        ));

        let mut downloads_app = App {
            page: Page::Downloads,
            content_focused: true,
            library: papr_core::LibraryState {
                papers: vec![library_paper.clone()],
                ..papr_core::LibraryState::default()
            },
            downloads: vec![DownloadTask {
                id: "paper".into(),
                title: "Paper".into(),
                downloaded: 10,
                total: Some(10),
                paper_id: Some(11),
                pdf_path: Some("/tmp/library.pdf".into()),
                status: DownloadStatus::Completed,
                remote_paper: None,
                failed_at: None,
            }],
            ..App::default()
        };
        assert!(matches!(
            handle_key(
                &mut downloads_app,
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
            ),
            Some(UiAction::MarkUnread(11))
        ));

        let mut collections_app = App {
            page: Page::Collections,
            content_focused: true,
            active_collection: Some(CollectionSummary {
                id: 1,
                name: "Collection".into(),
                paper_count: 1,
                folder_path: None,
            }),
            collection_papers: vec![library_paper.clone()],
            ..App::default()
        };
        assert!(matches!(
            handle_key(
                &mut collections_app,
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
            ),
            Some(UiAction::MarkUnread(11))
        ));

        let mut bookmarks_app = App {
            page: Page::Bookmarks,
            content_focused: true,
            bookmarks: vec![BookmarkSummary {
                id: 1,
                paper_id: 11,
                paper_title: "Bookmarked".into(),
                authors: "Researcher".into(),
                year: None,
                journal: None,
                doi: None,
                pdf_path: "/tmp/library.pdf".into(),
                page: None,
                label: None,
            }],
            ..App::default()
        };
        assert!(matches!(
            handle_key(
                &mut bookmarks_app,
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
            ),
            Some(UiAction::MarkUnread(11))
        ));

        let mut authors_app = App {
            page: Page::Authors,
            content_focused: true,
            active_author: Some(AuthorSummary {
                id: 3,
                name: "Researcher".into(),
                paper_count: 1,
            }),
            author_papers: vec![library_paper.clone()],
            ..App::default()
        };
        assert!(matches!(
            handle_key(
                &mut authors_app,
                KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)
            ),
            Some(UiAction::MarkUnread(11))
        ));

        let mut notes_app = App {
            page: Page::Notes,
            content_focused: true,
            notes_papers: vec![library_paper],
            ..App::default()
        };
        assert!(matches!(
            handle_key(&mut notes_app, KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)),
            Some(UiAction::MarkUnread(11))
        ));
    }

    #[test]
    fn downloads_keybinding_g_assigns_to_group() {
        let library_paper = LibraryPaper {
            id: 11,
            title: "Library Paper".into(),
            authors: "Researcher".into(),
            doi: None,
            arxiv_id: None,
            pdf_path: Some("/tmp/library.pdf".into()),
            file_size: Some(1),
            reading_status: "read".into(),
            is_favorite: false,
        };

        let mut downloads_app = App {
            page: Page::Downloads,
            content_focused: true,
            library: papr_core::LibraryState {
                papers: vec![library_paper],
                ..papr_core::LibraryState::default()
            },
            downloads: vec![DownloadTask {
                id: "paper".into(),
                title: "Paper".into(),
                downloaded: 10,
                total: Some(10),
                paper_id: Some(11),
                pdf_path: Some("/tmp/library.pdf".into()),
                status: DownloadStatus::Completed,
                remote_paper: None,
                failed_at: None,
            }],
            ..App::default()
        };
        assert!(matches!(
            handle_key(
                &mut downloads_app,
                KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE)
            ),
            Some(UiAction::Prompt(PaperTarget::Local(11)))
        ));
        assert_eq!(downloads_app.modal_return, AppMode::Normal);
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
    fn dashboard_and_discover_paper_rows_support_citation_and_download_shortcuts() {
        for page in [Page::Dashboard, Page::Discover] {
            let paper = remote_paper("https://arxiv.org/abs/2607.12345", "Paper");
            let mut app = App {
                page,
                content_focused: true,
                today_papers: (page == Page::Dashboard)
                    .then_some(paper.clone())
                    .into_iter()
                    .collect(),
                discovery: DiscoveryState {
                    results: (page == Page::Discover).then_some(paper).into_iter().collect(),
                    ..DiscoveryState::default()
                },
                ..App::default()
            };
            assert!(matches!(
                handle_key(&mut app, KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)),
                Some(UiAction::CopyCitation(PaperTarget::Remote(_)))
            ));
            assert!(matches!(
                handle_key(&mut app, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)),
                Some(UiAction::Download(_))
            ));
        }
    }

    #[test]
    fn browser_shortcut_opens_local_papers_and_reports_missing_arxiv_metadata() {
        let paper = LibraryPaper {
            id: 42,
            title: "Local paper".into(),
            authors: String::new(),
            doi: None,
            arxiv_id: Some("2607.12345v2".into()),
            pdf_path: Some("/tmp/local.pdf".into()),
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
        assert!(matches!(
            handle_key(&mut app, KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)),
            Some(UiAction::OpenBrowser(url)) if url == "https://arxiv.org/abs/2607.12345v2"
        ));

        app.library.papers[0].arxiv_id = None;
        assert!(handle_key(&mut app, KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)).is_none());
        assert_eq!(
            app.toast.as_deref(),
            Some("No valid arXiv page is available for this paper")
        );
    }

    #[test]
    fn downloaded_remote_paper_opens_from_metadata_without_changing_browser_shortcut() {
        for page in [Page::Dashboard, Page::Discover] {
            let remote = remote_paper("https://arxiv.org/abs/downloaded", "Downloaded paper");
            let mut app = App {
                page,
                content_focused: true,
                mode: AppMode::PaperDetail,
                today_papers: (page == Page::Dashboard).then_some(remote.clone()).into_iter().collect(),
                discovery: DiscoveryState {
                    results: (page == Page::Discover).then_some(remote).into_iter().collect(),
                    ..DiscoveryState::default()
                },
                library: papr_core::LibraryState {
                    papers: vec![LibraryPaper {
                        id: 42,
                        title: "Downloaded paper".into(),
                        authors: String::new(),
                        doi: None,
                        arxiv_id: Some("https://arxiv.org/abs/downloaded".into()),
                        pdf_path: Some("/tmp/downloaded.pdf".into()),
                        file_size: None,
                        reading_status: "unread".into(),
                        is_favorite: false,
                    }],
                    ..papr_core::LibraryState::default()
                },
                ..App::default()
            };

            let open = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert!(matches!(
                open,
                Some(UiAction::OpenPdf { paper_id: 42, path })
                    if path == std::path::Path::new("/tmp/downloaded.pdf")
            ));

            let browser = handle_key(&mut app, KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
            assert!(matches!(
                browser,
                Some(UiAction::OpenBrowser(url)) if url.ends_with("/downloaded")
            ));
        }
    }

    #[test]
    fn dashboard_open_keeps_discover_results_independent() {
        let dashboard_paper = remote_paper("https://arxiv.org/abs/dashboard", "Dashboard Paper");
        let discover_paper = remote_paper("https://arxiv.org/abs/discover", "Discover Paper");
        let mut app = App {
            page: Page::Dashboard,
            content_focused: true,
            today_papers: vec![dashboard_paper],
            discovery: papr_core::DiscoveryState {
                query: "search".into(),
                query_cursor: 6,
                results: vec![discover_paper],
                selected: 0,
                scroll: 3,
                status: papr_core::DiscoveryStatus::Ready,
                detail_scroll: 11,
                ..papr_core::DiscoveryState::default()
            },
            ..App::default()
        };

        app.dispatch(papr_core::Command::Open);

        assert_eq!(app.mode, AppMode::PaperDetail);
        assert_eq!(app.paper_detail_scroll, 0);
        assert_eq!(app.discovery.query, "search");
        assert_eq!(app.discovery.query_cursor, 6);
        assert_eq!(app.discovery.scroll, 3);
        assert_eq!(app.discovery.selected, 0);
        assert_eq!(app.discovery.detail_scroll, 11);
        assert_eq!(app.discovery.results.len(), 1);
        assert_eq!(app.discovery.results[0].title, "Discover Paper");
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
                    arxiv_id: None,
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
            arxiv_id: None,
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
            arxiv_id: None,
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
    fn daily_dashboard_permutation_is_input_order_independent_and_changes_by_date() {
        let papers: Vec<_> = (0..30)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/{index}"), "Paper"))
            .collect();
        let mut reordered = papers.clone();
        reordered.reverse();

        shuffle_daily_bucket(&mut reordered, "2026-07-19", "quantum gravity");
        let mut same_day = papers.clone();
        shuffle_daily_bucket(&mut same_day, "2026-07-19", "quantum gravity");
        assert_eq!(reordered, same_day);

        let mut next_day = papers;
        shuffle_daily_bucket(&mut next_day, "2026-07-20", "quantum gravity");
        assert_ne!(
            same_day.iter().take(10).map(|paper| &paper.id).collect::<Vec<_>>(),
            next_day.iter().take(10).map(|paper| &paper.id).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn keyword_dashboard_feed_rotates_selected_papers_each_day() {
        let papers = (0..30)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/{index}"), "Quantum result"))
            .collect::<Vec<_>>();

        let first_day = select_dashboard_papers(
            vec![("quantum".into(), papers.clone())],
            10,
            "2026-07-19",
        );
        let next_day = select_dashboard_papers(
            vec![("quantum".into(), papers)],
            10,
            "2026-07-20",
        );

        assert_ne!(
            first_day.iter().map(|paper| &paper.id).collect::<Vec<_>>(),
            next_day.iter().map(|paper| &paper.id).collect::<Vec<_>>(),
        );
    }

    #[test]
    fn dashboard_prioritizes_papers_with_all_keyword_terms_in_the_title() {
        let papers = vec![
            remote_paper("https://arxiv.org/abs/abstract", "A related result"),
            remote_paper("https://arxiv.org/abs/title-one", "Quantum gravity constraints"),
            remote_paper("https://arxiv.org/abs/title-two", "Gravity in quantum systems"),
        ];

        let selected = select_dashboard_papers(
            vec![("quantum gravity".into(), papers)],
            10,
            "2026-07-19",
        );

        assert!(selected[0].title.to_lowercase().contains("quantum"));
        assert!(selected[0].title.to_lowercase().contains("gravity"));
        assert!(selected[1].title.to_lowercase().contains("quantum"));
        assert!(selected[1].title.to_lowercase().contains("gravity"));
        assert_eq!(selected[2].id, "https://arxiv.org/abs/abstract");
    }

    #[test]
    fn dashboard_represents_keywords_even_when_only_one_has_a_title_match() {
        let selected = select_dashboard_papers(
            vec![
                (
                    "neural networks".into(),
                    vec![remote_paper("https://arxiv.org/abs/abstract", "A related result")],
                ),
                (
                    "quantum gravity".into(),
                    vec![remote_paper(
                        "https://arxiv.org/abs/title",
                        "Quantum gravity constraints",
                    )],
                ),
            ],
            10,
            "2026-07-19",
        );

        assert_eq!(selected.len(), 2);
        assert!(selected.iter().any(|paper| paper.id == "https://arxiv.org/abs/title"));
        assert!(selected
            .iter()
            .any(|paper| paper.id == "https://arxiv.org/abs/abstract"));
    }

    #[test]
    fn dashboard_balances_keyword_targets_before_title_quality() {
        let first_keyword: Vec<_> = (0..10)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/alpha-{index}"), "Alpha"))
            .collect();
        let second_keyword: Vec<_> = (0..10)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/beta-{index}"), "Related work"))
            .collect();
        let selected = select_dashboard_papers(
            vec![("alpha".into(), first_keyword), ("beta".into(), second_keyword)],
            10,
            "2026-07-19",
        );

        let first_count = selected
            .iter()
            .filter(|paper| paper.id.contains("alpha-"))
            .count();
        assert_eq!(first_count, 5);
        assert_eq!(selected.len() - first_count, 5);
    }

    #[test]
    fn dashboard_keyword_targets_are_balanced_with_a_gentle_earlier_preference() {
        let keywords = |count| (0..count).map(|index| format!("keyword {index}")).collect::<Vec<_>>();
        assert_eq!(
            keyword_representation_targets(&[50, 50], &keywords(2), 10, "2026-07-19"),
            vec![5, 5]
        );
        assert_eq!(
            keyword_representation_targets(&[50, 50, 50], &keywords(3), 10, "2026-07-19"),
            vec![4, 3, 3]
        );
        assert_eq!(
            keyword_representation_targets(&[50, 50, 50, 50], &keywords(4), 10, "2026-07-19"),
            vec![3, 3, 2, 2]
        );
        assert_eq!(
            keyword_representation_targets(&[50, 0, 50], &keywords(3), 10, "2026-07-19"),
            vec![5, 0, 5]
        );
    }

    #[test]
    fn dashboard_many_keywords_uses_a_daily_weighted_window() {
        let keywords = (0..15)
            .map(|index| format!("keyword {index}"))
            .collect::<Vec<_>>();
        let targets = keyword_representation_targets(&[50; 15], &keywords, 10, "2026-07-19");

        assert_eq!(targets.iter().sum::<usize>(), 10);
        assert_eq!(targets.iter().filter(|&&target| target == 1).count(), 10);
    }

    #[test]
    fn dashboard_reallocates_when_a_keyword_runs_out_of_candidates() {
        let second_keyword: Vec<_> = (0..20)
            .map(|index| remote_paper(&format!("https://arxiv.org/abs/beta-{index}"), "Beta"))
            .collect();
        let selected = select_dashboard_papers(
            vec![
                (
                    "alpha".into(),
                    vec![remote_paper("https://arxiv.org/abs/alpha-only", "Alpha")],
                ),
                ("beta".into(), second_keyword),
            ],
            10,
            "2026-07-19",
        );

        assert_eq!(selected.len(), 10);
        assert_eq!(
            selected
                .iter()
                .filter(|paper| paper.id == "https://arxiv.org/abs/alpha-only")
                .count(),
            1
        );
    }

    #[test]
    fn a_multi_keyword_paper_counts_once_toward_each_target() {
        let shared = remote_paper("https://arxiv.org/abs/shared", "Alpha beta gamma");
        let bucket = |keyword: &str, count: usize| {
            (0..count)
                .map(|index| {
                    remote_paper(
                        &format!("https://arxiv.org/abs/{keyword}-{index}"),
                        keyword,
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut alpha = bucket("alpha", 4);
        let mut beta = bucket("beta", 3);
        let mut gamma = bucket("gamma", 3);
        alpha.push(shared.clone());
        beta.push(shared.clone());
        gamma.push(shared);

        let selected = select_dashboard_papers(
            vec![
                ("alpha".into(), alpha),
                ("beta".into(), beta),
                ("gamma".into(), gamma),
            ],
            10,
            "2026-07-19",
        );

        assert_eq!(selected[0].id, "https://arxiv.org/abs/shared");
        assert!(selected.iter().filter(|paper| paper.id.contains("beta-")).count() >= 2);
        assert!(selected.iter().filter(|paper| paper.id.contains("gamma-")).count() >= 2);
    }

    #[test]
    fn dashboard_boosts_and_deduplicates_multi_keyword_matches() {
        let shared = remote_paper("https://arxiv.org/abs/shared", "Alpha beta methods");
        let selected = select_dashboard_papers(
            vec![
                (
                    "alpha".into(),
                    vec![remote_paper("https://arxiv.org/abs/alpha", "Alpha result"), shared.clone()],
                ),
                (
                    "beta".into(),
                    vec![remote_paper("https://arxiv.org/abs/beta", "Beta result"), shared],
                ),
            ],
            2,
            "2026-07-19",
        );

        assert_eq!(selected[0].id, "https://arxiv.org/abs/shared");
        assert_eq!(
            selected
                .iter()
                .filter(|paper| paper.id == "https://arxiv.org/abs/shared")
                .count(),
            1
        );
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

    #[test]
    fn reading_queue_workspace_keybinds_and_actions() {
        let paper = LibraryPaper {
            id: 42,
            title: "Queue Paper".into(),
            authors: "Researcher".into(),
            doi: None,
            arxiv_id: None,
            pdf_path: Some("/tmp/queue.pdf".into()),
            file_size: None,
            reading_status: "unread".into(),
            is_favorite: false,
        };

        // Case 1: Toggle Add to queue from Library
        let mut app = App {
            page: Page::Library,
            content_focused: true,
            library: papr_core::LibraryState {
                papers: vec![paper.clone()],
                ..papr_core::LibraryState::default()
            },
            ..App::default()
        };
        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(action, Some(UiAction::AddToQueue(42))));

        // Case 2: Toggle Remove from queue from ReadingQueue page
        app.page = Page::ReadingQueue;
        app.reading_queue_papers = vec![paper];
        app.reading_queue_selected = 0;
        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        assert!(matches!(action, Some(UiAction::RemoveFromQueue(42))));

        // Case 3: Move Up and Move Down in queue
        let action_up = handle_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT));
        assert!(matches!(action_up, Some(UiAction::MoveQueueItemUp(42))));
        let action_down = handle_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
        assert!(matches!(action_down, Some(UiAction::MoveQueueItemDown(42))));
    }

    #[test]
    fn credits_workspace_keybinds_and_actions() {
        let mut app = App {
            page: Page::Credits,
            content_focused: true,
            credits_selected: 0,
            ..App::default()
        };

        // MoveDown command should navigate down
        app.dispatch(papr_core::Command::MoveDown);
        assert_eq!(app.credits_selected, 1);

        app.dispatch(papr_core::Command::MoveUp);
        assert_eq!(app.credits_selected, 0);

        // Enter key should trigger UiAction::OpenBrowser(url)
        let action = handle_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, Some(UiAction::OpenBrowser(ref url)) if url == "https://github.com/AfrozSaqlain/Papr"));
    }

    #[test]
    #[cfg(not(windows))]
    fn download_filename_preserves_colon_on_supported_platforms() {
        let sanitized = super::sanitize_download_filename_component("AntiGlitch: Better / Faster");
        assert_eq!(sanitized, "AntiGlitch: Better _ Faster");
    }

    #[test]
    #[cfg(windows)]
    fn download_filename_sanitizes_colon_on_windows() {
        let sanitized = super::sanitize_download_filename_component("AntiGlitch: Better / Faster");
        assert_eq!(sanitized, "AntiGlitch_ Better _ Faster");
    }

    #[tokio::test]
    async fn test_pdf_rename_flow_full() -> Result<(), Box<dyn std::error::Error>> {
        use std::collections::HashSet;
        use tokio::sync::mpsc;
        use super::{Runtime, apply_collection_prompt, ActionSenders};

        let temp_dir = std::env::temp_dir().join(format!(
            "papr-rename-flow-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir)?;
        let database_file = temp_dir.join("papr.db");
        let database = Database::open(&database_file)?;
        let library_roots = vec![temp_dir.clone()];
        let collection_roots = vec![temp_dir.clone()];
        let download_dir = temp_dir.clone();

        let (watch_sender, watch_receiver) = mpsc::unbounded_channel();
        let watcher = papr_core::library::LibraryWatcher::start(&[], || {}).unwrap();

        let mut runtime = Runtime {
            arxiv: papr_core::api::arxiv::ArxivClient::new().unwrap(),
            crossref: papr_core::api::crossref::CrossrefClient::new(),
            openalex: papr_core::api::openalex::OpenAlexClient::new(),
            downloads: papr_core::downloads::DownloadManager::new().unwrap(),
            database,
            database_file,
            config_file: temp_dir.join("papr.toml"),
            plugins_dir: temp_dir.join("plugins"),
            plugin_host: papr_core::PluginHost::discover(&temp_dir.join("plugins"), &[]).unwrap(),
            project_manager: papr_core::ProjectManager::new(temp_dir.join("projects"))?,
            project_compiler: None,
            default_downloads_dir: temp_dir.clone(),
            download_dir,
            pdf_viewer: "xdg-open".into(),
            primary_library_root: temp_dir.clone(),
            library_roots,
            collection_roots,
            dashboard_keywords: vec![],
            dashboard_keyword_signature: "".into(),
            dashboard_feed_date: "".into(),
            active_dashboard_fetch: None,
            watch_sender,
            watch_receiver,
            _watcher: watcher,
            active_enrichments: HashSet::new(),
            citation_index: None,
            citation_source: papr_core::CitationSource::default(),
        };

        let old_pdf_path = temp_dir.join("my_old_paper.pdf");
        std::fs::write(&old_pdf_path, "%PDF-1.4 old")?;

        let pdf = papr_core::library::LibraryIndexer::inspect_in_roots(
            &old_pdf_path,
            std::slice::from_ref(&temp_dir),
        )?;
        runtime.database.import_pdf(&pdf)?;

        let papers = runtime.database.library_papers()?;
        let paper_id = papers[0].id;

        let mut app = App::default();
        app.library.papers = vec![LibraryPaper {
            id: paper_id,
            title: "my_old_paper".into(),
            authors: "".into(),
            doi: None,
            arxiv_id: None,
            pdf_path: Some(old_pdf_path.to_string_lossy().into_owned()),
            file_size: Some(12),
            reading_status: "unread".into(),
            is_favorite: false,
        }];

        let prompt = papr_core::MetadataPrompt {
            paper_id: Some(paper_id),
            rename_collection_id: None,
            rename_paper_id: Some(paper_id),
            value: "my_new_paper".into(),
            cursor: 0,
            selected: 0,
            current_collection: None,
        };

        apply_collection_prompt(&mut runtime, &mut app, &prompt)?;

        assert_eq!(app.library.papers[0].title, "my old paper");
        assert_eq!(app.library.papers[0].pdf_path, Some(temp_dir.join("my_new_paper.pdf").to_string_lossy().into_owned()));

        let index_res = papr_core::library::LibraryIndexer::scan(&[temp_dir.clone()]);
        let response = crate::IndexResponse::Scan {
            pdfs: index_res,
            directories: vec![],
        };

        let (search_tx, _) = mpsc::unbounded_channel();
        let (index_tx, _) = mpsc::unbounded_channel();
        let (enrich_tx, _) = mpsc::unbounded_channel();
        let (download_tx, _) = mpsc::unbounded_channel();
        let (today_tx, _) = mpsc::unbounded_channel();
        let (app_events_tx, _) = mpsc::unbounded_channel();
        let senders = ActionSenders {
            search: search_tx,
            index: index_tx,
            enrichment: enrich_tx,
            download: download_tx,
            today: today_tx,
            app_events: app_events_tx,
        };

        super::apply_index_response(response, &mut runtime, &senders, &mut app).await?;
        assert_eq!(app.library.papers[0].title, "my old paper");

        std::fs::remove_dir_all(&temp_dir)?;
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_plugin_event_dispatch_and_auto_tagger_action() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;
        use tokio::sync::mpsc;
        use super::{Runtime, dispatch_plugin_events};

        let temp_dir = std::env::temp_dir().join(format!(
            "papr-plugin-dispatch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let plugins_dir = temp_dir.join("plugins");
        let auto_tagger_dir = plugins_dir.join("auto-tagger");
        std::fs::create_dir_all(&auto_tagger_dir)?;

        let manifest = r#"
id = "auto-tagger"
name = "Auto Tagger"
version = "1.0.0"
api_version = 1
description = "Auto tagger test"
executable = "tagger.py"
capabilities = ["activity-events", "read-paper-metadata"]
"#;
        std::fs::write(auto_tagger_dir.join("plugin.toml"), manifest)?;

        let script = r#"#!/usr/bin/env python3
import json
import sys

req = json.load(sys.stdin)
paper = req.get("context", {}).get("paper", {})
title = paper.get("title", "").lower()

actions = []
if "neural" in title or "deep learning" in title:
    actions.append({"type": "add_to_collection", "name": "Machine Learning"})
    actions.append({"type": "notify", "message": "Tagged paper!"})

print(json.dumps({"actions": actions}))
"#;
        let script_path = auto_tagger_dir.join("tagger.py");
        std::fs::write(&script_path, script)?;
        let mut perms = std::fs::metadata(&script_path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms)?;

        let plugin_host = papr_core::PluginHost::discover(&plugins_dir, &["auto-tagger".to_string()])?;
        let database_file = temp_dir.join("papr.db");
        let database = Database::open(&database_file)?;

        let (watch_sender, watch_receiver) = mpsc::unbounded_channel();
        let watcher = papr_core::library::LibraryWatcher::start(&[], || {}).unwrap();

        let runtime = Runtime {
            arxiv: papr_core::api::arxiv::ArxivClient::new().unwrap(),
            crossref: papr_core::api::crossref::CrossrefClient::new(),
            openalex: papr_core::api::openalex::OpenAlexClient::new(),
            downloads: papr_core::downloads::DownloadManager::new().unwrap(),
            database,
            database_file,
            config_file: temp_dir.join("papr.toml"),
            plugins_dir: plugins_dir.clone(),
            plugin_host,
            project_manager: papr_core::ProjectManager::new(temp_dir.join("projects"))?,
            project_compiler: None,
            default_downloads_dir: temp_dir.clone(),
            download_dir: temp_dir.clone(),
            pdf_viewer: "xdg-open".into(),
            primary_library_root: temp_dir.clone(),
            library_roots: vec![temp_dir.clone()],
            collection_roots: vec![temp_dir.clone()],
            dashboard_keywords: vec![],
            dashboard_keyword_signature: "".into(),
            dashboard_feed_date: "".into(),
            active_dashboard_fetch: None,
            watch_sender,
            watch_receiver,
            _watcher: watcher,
            active_enrichments: std::collections::HashSet::new(),
            citation_index: None,
            citation_source: papr_core::CitationSource::default(),
        };

        let pdf_path = temp_dir.join("neural_networks.pdf");
        std::fs::write(&pdf_path, "%PDF-1.4 test")?;
        let pdf = papr_core::library::LibraryIndexer::inspect_in_roots(&pdf_path, &[temp_dir.clone()])?;
        runtime.database.import_pdf(&pdf)?;

        let papers = runtime.database.library_papers()?;
        assert!(!papers.is_empty());
        let paper_id = papers[0].id;

        let mut app = App::default();
        dispatch_plugin_events(&runtime, &mut app, &["paper_imported"], paper_id).await?;

        let collections = runtime.database.collections()?;
        assert!(collections.iter().any(|c| c.name == "Machine Learning"));
        assert_eq!(app.toast, Some("Tagged paper!".to_string()));

        let moved_pdf = temp_dir.join("Machine Learning").join("neural_networks.pdf");
        assert!(moved_pdf.exists());

        let directories = papr_core::LibraryIndexer::collection_directories(&runtime.collection_roots);
        runtime.database.reconcile_collections(&runtime.collection_roots, &directories)?;
        let collections_after = runtime.database.collections()?;
        assert!(collections_after.iter().any(|c| c.name == "Machine Learning"));

        std::fs::remove_dir_all(&temp_dir)?;
        Ok(())
    }
}
