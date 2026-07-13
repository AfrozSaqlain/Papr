//! `papr` executable entry point.

mod terminal;
mod ui;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use anyhow::{Context, Result};
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
        let mut roots = config.library_folders.clone();
        roots.push(config.download_path.clone().unwrap_or(paths.downloads_dir));
        let pdfs = LibraryIndexer::scan(&roots);
        let mut imported = 0_usize;
        for pdf in &pdfs {
            imported += usize::from(database.import_pdf(pdf)?);
        }
        println!("indexed: {}, imported: {}", pdfs.len(), imported);
        return Ok(());
    }
    let download_dir = config.download_path.clone().unwrap_or(paths.downloads_dir);
    std::fs::create_dir_all(&download_dir).context("failed to create download directory")?;

    let collection_roots = config.library_folders.clone();
    let mut library_roots = collection_roots.clone();
    if !library_roots.contains(&download_dir) {
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

    discover_local_downloads(&mut app, &download_dir);

    app.library.papers = database
        .library_papers_in_roots(&library_roots)
        .context("failed to load library")?;
    refresh_organization(&database, &mut app)?;
    let (watch_sender, watch_receiver) = mpsc::unbounded_channel();
    let _watcher = LibraryWatcher::start(&library_roots, move || {
        let _ = watch_sender.send(());
    })
    .context("failed to watch library folders")?;

    let arxiv = ArxivClient::new().context("failed to initialize arXiv client")?;
    let downloads = DownloadManager::new().context("failed to initialize download manager")?;
    let mut session = TerminalSession::start(config.mouse)?;
    let primary_library_root = library_roots[0].clone();
    let dashboard_keywords = config.dashboard_keyword_list();
    let dashboard_keyword_signature = dashboard_keywords.join(",");
    let runtime = Runtime {
        arxiv,
        downloads,
        database,
        database_file: paths.database_file.clone(),
        download_dir,
        pdf_viewer: config.pdf_viewer.clone().unwrap_or_else(default_pdf_viewer),
        primary_library_root,
        library_roots,
        collection_roots,
        dashboard_keywords,
        dashboard_keyword_signature,
        dashboard_feed_date: local_feed_date(),
        watch_receiver,
    };
    run(&mut session, &mut app, &theme, runtime).await
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
    OpenDownload(String),
    RenamePdf(i64),
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

struct Runtime {
    arxiv: ArxivClient,
    downloads: DownloadManager,
    database: Database,
    database_file: PathBuf,
    download_dir: PathBuf,
    pdf_viewer: String,
    primary_library_root: PathBuf,
    library_roots: Vec<PathBuf>,
    collection_roots: Vec<PathBuf>,
    dashboard_keywords: Vec<String>,
    dashboard_keyword_signature: String,
    dashboard_feed_date: String,
    watch_receiver: mpsc::UnboundedReceiver<()>,
}

struct ActionSenders {
    search: mpsc::UnboundedSender<SearchResponse>,
    index: mpsc::UnboundedSender<IndexResponse>,
    download: mpsc::UnboundedSender<DownloadEvent>,
    today: mpsc::UnboundedSender<TodayResponse>,
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
    theme: &Theme,
    mut runtime: Runtime,
) -> Result<()> {
    let (sender, mut receiver) = mpsc::unbounded_channel::<SearchResponse>();
    let (index_sender, mut index_receiver) = mpsc::unbounded_channel::<IndexResponse>();
    let (download_sender, mut download_receiver) = mpsc::unbounded_channel::<DownloadEvent>();
    let (today_sender, mut today_receiver) = mpsc::unbounded_channel::<TodayResponse>();
    let senders = ActionSenders {
        search: sender,
        index: index_sender,
        download: download_sender,
        today: today_sender,
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
            apply_index_response(response, &runtime, app)?;
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
        session
            .terminal_mut()
            .draw(|frame| ui::render(frame, app, theme))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if let Some(action) = handle_key(app, key) {
                        apply_ui_action(
                            action,
                            &mut runtime,
                            &senders,
                            &mut pending_downloads,
                            app,
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
            runtime.database.record_open(paper_id, true)?;
            open_pdf(&runtime.pdf_viewer, &path, app)?;
            refresh_dashboard(runtime, app)?;
        }
        UiAction::OpenNote(target) => {
            let paper_id = resolve_target(target, &runtime.database)?;
            runtime
                .database
                .record_activity("note_opened", Some(paper_id), None)?;
            app.note_editor = Some(runtime.database.paper_note(paper_id)?);
            app.note_preview = false;
            app.mode = AppMode::NoteEdit;
        }
        UiAction::SaveNote(note) => runtime.database.save_note(&note)?,
        UiAction::Prompt(target) => {
            let paper_id = resolve_target(target, &runtime.database)?;
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
            refresh_organization(&runtime.database, app)?;
            refresh_dashboard(runtime, app)?;
            app.toast = Some(format!("Saved {}", prompt.value));
        }
        UiAction::Bookmark(target) => {
            let paper_id = resolve_target(target, &runtime.database)?;
            let active = runtime.database.toggle_bookmark(paper_id)?;
            runtime.database.record_activity(
                "bookmarked",
                Some(paper_id),
                Some(if active { "added" } else { "removed" }),
            )?;
            refresh_organization(&runtime.database, app)?;
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
        UiAction::OpenDownload(id) => {
            let path = runtime.download_dir.join(format!("{id}.pdf"));
            open_pdf(&runtime.pdf_viewer, &path, app)?;
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
            refresh_organization(&runtime.database, app)?;
            refresh_dashboard(runtime, app)?;
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
        refresh_renamed_collection(&runtime.database, app, collection_id)?;
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
    Ok(())
}

fn refresh_renamed_collection(
    database: &Database,
    app: &mut App,
    collection_id: i64,
) -> Result<()> {
    app.collections = database.collections()?;
    app.collection_selected = app
        .collections
        .iter()
        .position(|collection| collection.id == collection_id)
        .unwrap_or_else(|| app.collections.len().saturating_sub(1));
    if app.last_opened_collection_id == Some(collection_id) {
        app.collection_papers = database.papers_for_collection(collection_id)?;
        app.collection_paper_selected = app
            .collection_paper_selected
            .min(app.collection_papers.len().saturating_sub(1));
    }
    if app.active_collection.as_ref().map(|item| item.id) == Some(collection_id) {
        app.active_collection = app
            .collections
            .iter()
            .find(|collection| collection.id == collection_id)
            .cloned();
    }
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

fn resolve_target(target: PaperTarget, database: &Database) -> Result<i64> {
    match target {
        PaperTarget::Local(id) => Ok(id),
        PaperTarget::Remote(paper) => database.ensure_remote_paper(&paper).map_err(Into::into),
    }
}

fn refresh_organization(database: &Database, app: &mut App) -> Result<()> {
    app.collections = database.collections()?;
    app.collection_selected = app
        .collection_selected
        .min(app.collections.len().saturating_sub(1));
    if let Some(active) = &app.active_collection {
        app.active_collection = app
            .collections
            .iter()
            .find(|collection| collection.id == active.id)
            .cloned();
    }
    app.bookmarks = database.bookmarks()?;
    app.bookmark_selected = app
        .bookmark_selected
        .min(app.bookmarks.len().saturating_sub(1));
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

fn default_pdf_viewer() -> String {
    if cfg!(target_os = "macos") {
        "open".into()
    } else if cfg!(target_os = "windows") {
        "cmd /C start".into()
    } else {
        "xdg-open".into()
    }
}

fn open_pdf(viewer: &str, path: &Path, app: &mut App) -> Result<()> {
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
    let mut command = ProcessCommand::new(&program);
    command.args(argv);
    if !has_placeholder {
        command.arg(path);
    }

    command.stdout(Stdio::null());
    command.stderr(Stdio::null());

    match command.spawn() {
        Ok(_) => app.toast = Some(format!("Opened {}", path.display())),
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

fn start_runtime_scan(runtime: &Runtime, senders: &ActionSenders, app: &mut App) {
    start_scan(
        &runtime.library_roots,
        &runtime.collection_roots,
        &senders.index,
        app,
    );
}

fn apply_index_response(response: IndexResponse, runtime: &Runtime, app: &mut App) -> Result<()> {
    let database = &runtime.database;
    match response {
        IndexResponse::Scan { pdfs, directories } => {
            let found = pdfs.len();
            let mut imported = 0_usize;
            for directory in &directories {
                database.sync_collection_directory(directory)?;
            }
            for pdf in &pdfs {
                imported += usize::from(database.import_pdf(pdf)?);
                if let Some(paper_id) = database.paper_id_for_pdf(pdf)? {
                    sync_pdf_collection_membership(
                        database,
                        paper_id,
                        pdf,
                        &runtime.collection_roots,
                    )?;
                }
            }
            database.reconcile_collections(&runtime.collection_roots, &directories)?;
            app.library.indexing = false;
            app.library.message = Some(format!("Indexed {found} PDFs, imported {imported} new"));
        }
        IndexResponse::File(Ok(pdf)) => {
            let imported = database.import_pdf(&pdf)?;
            if let Some(paper_id) = database.paper_id_for_pdf(&pdf)? {
                sync_pdf_collection_membership(
                    database,
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
        }
        IndexResponse::File(Err(error)) => app.library.message = Some(error),
    }
    refresh_library(runtime, app)?;
    refresh_organization(database, app)?;
    refresh_dashboard(runtime, app)?;
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

fn discover_local_downloads(app: &mut App, download_dir: &std::path::Path) {
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
            app.downloads.push(DownloadTask {
                id,
                title,
                downloaded: size,
                total: Some(size),
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
    let filename = paper
        .id
        .rsplit('/')
        .next()
        .unwrap_or("paper")
        .replace(['/', '\\'], "_");
    let destination = directory.join(format!("{filename}.pdf"));
    let id = paper.id.clone();
    pending.insert(id.clone(), paper.clone());
    app.downloads.push(DownloadTask {
        id: id.clone(),
        title: paper.title,
        downloaded: 0,
        total: None,
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
            }
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
            _ => {
                edit_text(&mut app.palette_query, &mut app.palette_cursor, key);
            }
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
    if app.mode == AppMode::Search {
        return handle_search_key(app, key);
    }
    let key = normalize_panel_navigation(key);
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
    if let Some(action) = bookmark_action(app, key) {
        return Some(action);
    }
    if app.page == papr_core::Page::Discover && key.code == KeyCode::Char('o') {
        return app
            .discovery
            .results
            .get(app.discovery.selected)
            .map(|paper| UiAction::OpenBrowser(paper.id.clone()));
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

fn bookmark_action(app: &App, key: KeyEvent) -> Option<UiAction> {
    if app.page != papr_core::Page::Bookmarks {
        return None;
    }
    let bookmark = app.bookmarks.get(app.bookmark_selected)?;
    match key.code {
        KeyCode::Char('B') => Some(UiAction::Bookmark(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Enter | KeyCode::Char('p') => Some(UiAction::OpenPdf {
            paper_id: bookmark.paper_id,
            path: PathBuf::from(&bookmark.pdf_path),
        }),
        KeyCode::Char('s') => Some(UiAction::Prompt(PaperTarget::Local(bookmark.paper_id))),
        KeyCode::Char('R') => Some(UiAction::RenamePdf(bookmark.paper_id)),
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
        if let Some(task) = app.downloads.get(app.download_selected) {
            if matches!(task.status, DownloadStatus::Completed) {
                return Some(UiAction::OpenDownload(task.id.clone()));
            }
        }
    }
    if matches!(key.code, KeyCode::Char('R') | KeyCode::Char('s')) {
        if let Some(task) = app.downloads.get(app.download_selected) {
            if matches!(task.status, DownloadStatus::Completed) {
                let suffix = format!("{}.pdf", task.id);
                if let Some(paper) = app.library.papers.iter().find(|p| {
                    p.pdf_path
                        .as_ref()
                        .map_or(false, |path| path.ends_with(&suffix))
                }) {
                    return match key.code {
                        KeyCode::Char('R') => Some(UiAction::RenamePdf(paper.id)),
                        KeyCode::Char('s') => Some(UiAction::Prompt(PaperTarget::Local(paper.id))),
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
                let Some(paper) = app.collection_papers.get(app.collection_paper_selected) else {
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
                    app.collection_papers
                        .get(app.collection_paper_selected)
                        .map(|paper| UiAction::Bookmark(PaperTarget::Local(paper.id))),
                );
            }
            KeyCode::Char('R') => {
                return (
                    true,
                    app.collection_papers
                        .get(app.collection_paper_selected)
                        .map(|paper| UiAction::RenamePdf(paper.id)),
                );
            }
            KeyCode::Char('s') => {
                return (
                    true,
                    app.collection_papers
                        .get(app.collection_paper_selected)
                        .map(|paper| UiAction::Prompt(PaperTarget::Local(paper.id))),
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
            .collections
            .get(app.collection_selected)
            .map(|collection| UiAction::OpenCollection(collection.id));
        return (true, action);
    }
    if key.code == KeyCode::Char('R') {
        return (
            true,
            app.collections
                .get(app.collection_selected)
                .map(|collection| UiAction::RenameCollection(collection.id)),
        );
    }
    if matches!(key.code, KeyCode::Char('c' | 'n')) {
        return (true, Some(UiAction::CreateCollection));
    }
    (false, None)
}

fn navigation_command(key: KeyEvent) -> Option<Command> {
    match (key.code, key.modifiers) {
        (KeyCode::Char('p'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(Command::TogglePalette)
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
        .library
        .papers
        .get(app.library.selected)
        .map(|paper| PaperTarget::Local(paper.id))?;
    app.modal_return = AppMode::Normal;
    match key.code {
        KeyCode::Char('n') => Some(UiAction::OpenNote(target)),
        KeyCode::Char('s') => Some(UiAction::Prompt(target)),
        KeyCode::Char('B') => Some(UiAction::Bookmark(target)),
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
    let paper = app.library.papers.get(app.library.selected)?;
    paper
        .pdf_path
        .as_ref()
        .map(|path| (paper.id, PathBuf::from(path)))
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use papr_core::{
        App, AppMode, BookmarkSummary, CollectionSummary, LibraryPaper, Page, PaperNote,
        RemotePaper,
    };

    use super::{UiAction, diverse_latest_papers, handle_key, parse_command};

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
    fn palette_captures_and_erases_query() {
        let mut app = App {
            mode: AppMode::CommandPalette,
            ..App::default()
        };
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE),
        );
        let _ = handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(app.palette_query.is_empty());
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
