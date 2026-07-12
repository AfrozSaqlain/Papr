//! `papr` executable entry point.

mod terminal;
mod ui;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, Stdio},
};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use papr_core::{
    App, AppMode, ArxivClient, Command, Config, Database, DiscoveryStatus, DownloadEvent,
    DownloadManager, DownloadStatus, DownloadTask, ImportedPdf, LibraryIndexer, LibraryWatcher,
    MetadataPrompt, PaperNote, Paths, PromptKind, RemotePaper, Theme,
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover().context("failed to resolve papr directories")?;
    if matches!(&cli.command, Some(CliCommand::Paths)) {
        println!("config: {}", paths.config_file.display());
        println!("database: {}", paths.database_file.display());
        println!("downloads: {}", paths.downloads_dir.display());
        return Ok(());
    }

    let config = Config::load_or_create(&paths).context("failed to load configuration")?;
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
    let mut app = App {
        stats: database
            .dashboard_stats()
            .context("failed to load dashboard")?,
        ..App::default()
    };
    app.library.papers = database
        .library_papers()
        .context("failed to load library")?;
    refresh_organization(&database, &mut app)?;

    let download_dir = config.download_path.clone().unwrap_or(paths.downloads_dir);
    std::fs::create_dir_all(&download_dir).context("failed to create download directory")?;
    let mut library_roots = config.library_folders.clone();
    if !library_roots.contains(&download_dir) {
        library_roots.push(download_dir.clone());
    }
    let (watch_sender, watch_receiver) = mpsc::unbounded_channel();
    let _watcher = LibraryWatcher::start(&library_roots, move |path| {
        let _ = watch_sender.send(path);
    })
    .context("failed to watch library folders")?;

    let arxiv = ArxivClient::new().context("failed to initialize arXiv client")?;
    let downloads = DownloadManager::new().context("failed to initialize download manager")?;
    let mut session = TerminalSession::start(config.mouse)?;
    let runtime = Runtime {
        arxiv,
        downloads,
        database,
        download_dir,
        pdf_viewer: config.pdf_viewer.clone().unwrap_or_else(default_pdf_viewer),
        library_roots,
        watch_receiver,
    };
    run(&mut session, &mut app, &theme, runtime).await
}

#[derive(Debug)]
enum UiAction {
    Search(String),
    Download(RemotePaper),
    Reindex,
    OpenPdf(PathBuf),
    OpenNote(PaperTarget),
    SaveNote(PaperNote),
    Prompt(PaperTarget, PromptKind),
    SubmitPrompt(MetadataPrompt),
    Bookmark(PaperTarget),
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

struct Runtime {
    arxiv: ArxivClient,
    downloads: DownloadManager,
    database: Database,
    download_dir: PathBuf,
    pdf_viewer: String,
    library_roots: Vec<PathBuf>,
    watch_receiver: mpsc::UnboundedReceiver<PathBuf>,
}

struct ActionSenders {
    search: mpsc::UnboundedSender<SearchResponse>,
    index: mpsc::UnboundedSender<IndexResponse>,
    download: mpsc::UnboundedSender<DownloadEvent>,
}

#[derive(Debug)]
enum IndexResponse {
    Scan(Vec<ImportedPdf>),
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
    let senders = ActionSenders {
        search: sender,
        index: index_sender,
        download: download_sender,
    };
    let mut pending_downloads = HashMap::<String, RemotePaper>::new();
    start_scan(&runtime.library_roots, &senders.index, app);
    while !app.should_quit {
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
        while let Ok(path) = runtime.watch_receiver.try_recv() {
            let response_sender = senders.index.clone();
            tokio::task::spawn_blocking(move || {
                let result = LibraryIndexer::inspect(&path).map_err(|error| error.to_string());
                let _ = response_sender.send(IndexResponse::File(result));
            });
        }
        while let Ok(response) = index_receiver.try_recv() {
            apply_index_response(response, &runtime.database, app)?;
        }
        while let Ok(event) = download_receiver.try_recv() {
            apply_download_event(
                event,
                &mut pending_downloads,
                &mut runtime.database,
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
        UiAction::Download(paper) => start_download(
            paper,
            &runtime.download_dir,
            &runtime.downloads,
            &senders.download,
            pending_downloads,
            app,
        ),
        UiAction::Reindex => start_scan(&runtime.library_roots, &senders.index, app),
        UiAction::OpenPdf(path) => open_pdf(&runtime.pdf_viewer, &path, app)?,
        UiAction::OpenNote(target) => {
            let paper_id = resolve_target(target, &runtime.database)?;
            app.note_editor = Some(runtime.database.paper_note(paper_id)?);
            app.note_preview = false;
            app.mode = AppMode::NoteEdit;
        }
        UiAction::SaveNote(note) => runtime.database.save_note(&note)?,
        UiAction::Prompt(target, kind) => {
            let paper_id = resolve_target(target, &runtime.database)?;
            app.metadata_prompt = Some(MetadataPrompt {
                paper_id,
                kind,
                value: String::new(),
            });
            app.mode = AppMode::Prompt;
        }
        UiAction::SubmitPrompt(prompt) => {
            match prompt.kind {
                PromptKind::Tag => {
                    runtime.database.add_tag(prompt.paper_id, &prompt.value)?;
                }
                PromptKind::Collection => {
                    runtime
                        .database
                        .add_to_collection(prompt.paper_id, &prompt.value)?;
                }
            }
            refresh_organization(&runtime.database, app)?;
            app.toast = Some(format!("Saved {}", prompt.value));
        }
        UiAction::Bookmark(target) => {
            let paper_id = resolve_target(target, &runtime.database)?;
            let active = runtime.database.toggle_bookmark(paper_id)?;
            refresh_organization(&runtime.database, app)?;
            app.toast = Some(if active {
                "Paper bookmarked".into()
            } else {
                "Bookmark removed".into()
            });
        }
    }
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
    app.tags = database.tags()?;
    app.bookmarks = database.bookmarks()?;
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

fn start_scan(roots: &[PathBuf], sender: &mpsc::UnboundedSender<IndexResponse>, app: &mut App) {
    if app.library.indexing {
        return;
    }
    app.library.indexing = true;
    app.library.message = Some("Indexing library folders...".into());
    let roots = roots.to_vec();
    let sender = sender.clone();
    tokio::task::spawn_blocking(move || {
        let _ = sender.send(IndexResponse::Scan(LibraryIndexer::scan(&roots)));
    });
}

fn apply_index_response(response: IndexResponse, database: &Database, app: &mut App) -> Result<()> {
    match response {
        IndexResponse::Scan(pdfs) => {
            let found = pdfs.len();
            let mut imported = 0_usize;
            for pdf in &pdfs {
                imported += usize::from(database.import_pdf(pdf)?);
            }
            app.library.indexing = false;
            app.library.message = Some(format!("Indexed {found} PDFs, imported {imported} new"));
        }
        IndexResponse::File(Ok(pdf)) => {
            let imported = database.import_pdf(&pdf)?;
            app.library.message = Some(if imported {
                format!("Imported {}", pdf.title)
            } else {
                "Ignored duplicate PDF".into()
            });
        }
        IndexResponse::File(Err(error)) => app.library.message = Some(error),
    }
    app.library.papers = database.library_papers()?;
    app.stats = database.dashboard_stats()?;
    Ok(())
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
    database: &mut Database,
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
                database.attach_download(&paper, &pdf)?;
            }
            task.downloaded = pdf.file_size;
            task.total = Some(pdf.file_size);
            task.status = DownloadStatus::Completed;
            app.library.papers = database.library_papers()?;
            app.stats = database.dashboard_stats()?;
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
            KeyCode::Backspace => {
                app.palette_query.pop();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.palette_query.push(character);
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
    if app.mode == AppMode::Search {
        match key.code {
            KeyCode::Esc => app.mode = AppMode::Normal,
            KeyCode::Enter if !app.discovery.query.trim().is_empty() => {
                let query = app.discovery.query.trim().to_owned();
                app.discovery.query.clone_from(&query);
                app.mode = AppMode::Normal;
                return Some(UiAction::Search(query));
            }
            KeyCode::Backspace => {
                app.discovery.query.pop();
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.discovery.query.push(character);
            }
            _ => {}
        }
        return None;
    }
    if app.mode == AppMode::PaperDetail {
        return handle_paper_detail_key(app, key);
    }

    if key.code == KeyCode::Char('/') {
        app.page = papr_core::Page::Discover;
        app.sidebar_index = 1;
        app.mode = AppMode::Search;
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
    if app.page == papr_core::Page::Library
        && matches!(key.code, KeyCode::Char('p') | KeyCode::Enter)
    {
        return selected_library_pdf(app).map(UiAction::OpenPdf);
    }
    if app.page == papr_core::Page::Library
        && let Some(action) = handle_library_metadata_key(app, key)
    {
        return Some(action);
    }

    let command = match (key.code, key.modifiers) {
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
    };
    if let Some(command) = command {
        app.dispatch(command);
    }
    None
}

fn handle_modal_key(app: &mut App, key: KeyEvent) -> Option<UiAction> {
    if app.mode == AppMode::Prompt {
        match key.code {
            KeyCode::Esc => {
                app.metadata_prompt = None;
                app.mode = app.modal_return;
            }
            KeyCode::Enter => {
                let prompt = app.metadata_prompt.take();
                app.mode = app.modal_return;
                return prompt.map(UiAction::SubmitPrompt);
            }
            KeyCode::Backspace => {
                if let Some(prompt) = &mut app.metadata_prompt {
                    prompt.value.pop();
                }
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(prompt) = &mut app.metadata_prompt {
                    prompt.value.push(character);
                }
            }
            _ => {}
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
        KeyCode::Backspace => {
            if let Some(note) = &mut app.note_editor {
                note.body.pop();
                changed = true;
            }
        }
        KeyCode::Enter => {
            if let Some(note) = &mut app.note_editor {
                note.body.push('\n');
                changed = true;
            }
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(note) = &mut app.note_editor {
                note.body.push(character);
                changed = true;
            }
        }
        _ => {}
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
        KeyCode::Char('n' | 't' | 's') => {
            app.modal_return = AppMode::PaperDetail;
            let target = selected_remote_target(app)?;
            return Some(match key.code {
                KeyCode::Char('n') => UiAction::OpenNote(target),
                KeyCode::Char('t') => UiAction::Prompt(target, PromptKind::Tag),
                _ => UiAction::Prompt(target, PromptKind::Collection),
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
        KeyCode::Char('t') => Some(UiAction::Prompt(target, PromptKind::Tag)),
        KeyCode::Char('s') => Some(UiAction::Prompt(target, PromptKind::Collection)),
        KeyCode::Char('B') => Some(UiAction::Bookmark(target)),
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

fn selected_library_pdf(app: &App) -> Option<PathBuf> {
    app.library
        .papers
        .get(app.library.selected)
        .and_then(|paper| paper.pdf_path.as_ref())
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use papr_core::{App, AppMode, LibraryPaper, Page, PaperNote};

    use super::{UiAction, handle_key, parse_command};

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
    fn note_editor_emits_autosave_action() {
        let mut app = App {
            mode: AppMode::NoteEdit,
            note_editor: Some(PaperNote {
                paper_id: 7,
                title: String::new(),
                body: String::new(),
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
        assert!(
            matches!(action, Some(UiAction::OpenPdf(path)) if path == std::path::Path::new("/tmp/paper.pdf"))
        );
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
}
