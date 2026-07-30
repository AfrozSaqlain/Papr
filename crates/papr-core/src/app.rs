//! Application state machine and navigation commands.

use crate::models::{
    AuthorSummary, BookmarkSummary, CollectionSummary, DashboardStats, LibraryPaper, PaperNote,
    RemotePaper, ResearchDashboard,
};
use crate::plugins::PluginInfo;
use crate::projects::Project;

/// Top-level application pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    /// Research overview.
    Dashboard,
    /// Remote discovery providers.
    Discover,
    /// Local catalog.
    Library,
    /// Prioritized reading lists.
    ReadingQueue,
    /// User-defined paper groups.
    Collections,
    /// Saved papers and locations.
    Bookmarks,
    /// Followed and cataloged authors.
    Authors,
    /// Markdown research notes.
    Notes,
    /// Background transfers.
    Downloads,
    /// Integrated LaTeX writing projects.
    Projects,
    /// Reading activity timeline.
    History,
    /// Reading analytics.
    Statistics,
    /// User preferences.
    Settings,
    /// Credits and about information.
    Credits,
}

impl Page {
    /// Whether this page supports local/workspace search.
    #[must_use]
    pub const fn supports_workspace_search(self) -> bool {
        matches!(
            self,
            Self::Library
                | Self::Downloads
                | Self::Collections
                | Self::Authors
                | Self::Bookmarks
                | Self::Notes
                | Self::ReadingQueue
        )
    }

    /// All pages in sidebar order.
    pub const ALL: [Self; 14] = [
        Self::Dashboard,
        Self::Discover,
        Self::Library,
        Self::ReadingQueue,
        Self::Collections,
        Self::Bookmarks,
        Self::Authors,
        Self::Notes,
        Self::Downloads,
        Self::Projects,
        Self::History,
        Self::Statistics,
        Self::Settings,
        Self::Credits,
    ];

    /// Human-readable navigation label.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Projects => "Projects",
            Self::Discover => "Discover",
            Self::Library => "Library",
            Self::ReadingQueue => "Reading Queue",
            Self::Collections => "Groups",
            Self::Bookmarks => "Bookmarks",
            Self::Authors => "Authors",
            Self::Notes => "Notes",
            Self::Downloads => "Downloads",
            Self::History => "History",
            Self::Statistics => "Statistics",
            Self::Settings => "Settings",
            Self::Credits => "Credits",
        }
    }

    /// Parse a configuration string into a `Page`.
    #[must_use]
    pub fn from_config_str(s: &str) -> Option<Self> {
        let normalized = s.trim().to_lowercase().replace('-', "_").replace(' ', "_");
        match normalized.as_str() {
            "dashboard" => Some(Self::Dashboard),
            "discover" => Some(Self::Discover),
            "library" => Some(Self::Library),
            "reading_queue" | "readingqueue" => Some(Self::ReadingQueue),
            "collections" | "groups" | "group" => Some(Self::Collections),
            "bookmarks" | "bookmark" => Some(Self::Bookmarks),
            "authors" | "author" => Some(Self::Authors),
            "notes" | "note" => Some(Self::Notes),
            "downloads" | "download" => Some(Self::Downloads),
            "projects" | "project" => Some(Self::Projects),
            "history" => Some(Self::History),
            "statistics" | "stats" => Some(Self::Statistics),
            "settings" => Some(Self::Settings),
            "credits" => Some(Self::Credits),
            _ => None,
        }
    }

    /// Canonical configuration string representation for this page.
    #[must_use]
    pub const fn config_str(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Discover => "discover",
            Self::Library => "library",
            Self::ReadingQueue => "reading_queue",
            Self::Collections => "collections",
            Self::Bookmarks => "bookmarks",
            Self::Authors => "authors",
            Self::Notes => "notes",
            Self::Downloads => "downloads",
            Self::Projects => "projects",
            Self::History => "history",
            Self::Statistics => "statistics",
            Self::Settings => "settings",
            Self::Credits => "credits",
        }
    }
}

/// Current input focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Normal page navigation.
    #[default]
    Normal,
    /// Fuzzy command lookup.
    CommandPalette,
    /// Shortcut overlay.
    Help,
    /// Text entry for a discovery search.
    Search,
    /// Text entry for filtering the currently displayed discovery results.
    DiscoverFilter,
    /// Full paper metadata view.
    PaperDetail,
    /// Direct Markdown note editing.
    NoteEdit,
    /// Single-line collection input.
    Prompt,
    /// Workspace-local search.
    WorkspaceSearch,
    /// Confirm deletion of a paper or collection.
    ConfirmDelete,
    /// Internal PDF viewer.
    PdfView,
    /// One-line project rename input.
    ProjectRename,
    /// One-line project creation input.
    ProjectCreate,
    /// One-line file creation input within the open project.
    ProjectFileCreate,
    /// Interactive settings modal.
    SettingsModal,
}

/// Tabs shown in the settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    /// Theme selection with live preview.
    #[default]
    Theme,
    /// General application preferences.
    General,
    /// Filesystem paths.
    Paths,
    /// Plugin management.
    Plugins,
}

impl SettingsTab {
    /// All tabs in display order.
    pub const ALL: [Self; 4] = [Self::Theme, Self::General, Self::Paths, Self::Plugins];

    /// Human-readable tab label.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Theme => "Theme",
            Self::General => "General",
            Self::Paths => "Paths",
            Self::Plugins => "Plugins",
        }
    }

    /// Tab index in `ALL`.
    #[must_use]
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    /// Navigate right with wrapping.
    #[must_use]
    pub fn next(self) -> Self {
        let idx = (self.index() + 1) % Self::ALL.len();
        Self::ALL[idx]
    }

    /// Navigate left with wrapping.
    #[must_use]
    pub fn prev(self) -> Self {
        let idx = self.index().wrapping_sub(1).min(Self::ALL.len() - 1);
        Self::ALL[idx]
    }
}

/// State for a single editable path entry in the library_folders list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathEntryState {
    /// Current text content of this entry.
    pub text: String,
    /// Byte cursor position within `text`.
    pub cursor: usize,
    /// Validation error message for this entry, if any.
    pub error: Option<String>,
}

impl PathEntryState {
    /// Create a new entry from an existing path string.
    #[must_use]
    pub fn new(text: String) -> Self {
        Self { text, cursor: 0, error: None }
    }
}

/// Focus target within the Paths tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PathsTabFocus {
    /// The library_folders multi-entry list.
    #[default]
    LibraryFolders,
    /// The download_path single-line field.
    DownloadPath,
    /// The projects_directory single-line field.
    ProjectsDirectory,
}

/// Focus target within the General tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GeneralTabFocus {
    /// startup_page selection list.
    #[default]
    StartupPage,
    /// pdf_viewer text field.
    PdfViewer,
    /// dashboard_keywords list.
    DashboardKeywords,
    /// enabled_plugins toggle list.
    EnabledPlugins,
}

/// Staged (in-memory) settings being edited in the modal.
#[derive(Debug, Clone)]
pub struct SettingsModalState {
    /// Whether focus is currently on the tab bar header.
    pub tab_bar_focused: bool,
    /// Active tab.
    pub tab: SettingsTab,
    /// Selected theme index in `Theme::BUILTIN_THEMES`.
    pub theme_selected: usize,
    /// Scroll offset for the theme list.
    pub theme_scroll: usize,
    // --- General tab ---
    /// Staged startup_page value.
    pub startup_page: String,
    /// Selected startup_page option index.
    pub startup_page_selected: usize,
    /// Staged pdf_viewer value (raw command string).
    pub pdf_viewer: String,
    /// Byte cursor in pdf_viewer text field.
    pub pdf_viewer_cursor: usize,
    /// Whether the pdf_viewer field is being edited.
    pub pdf_viewer_editing: bool,
    /// Staged dashboard_keywords as editable entries.
    pub keyword_entries: Vec<PathEntryState>,
    /// Selected entry index in keyword_entries.
    pub keyword_selected: usize,
    /// Whether the selected keyword entry is actively being edited.
    pub keyword_editing: bool,
    /// Staged enabled_plugins list.
    pub enabled_plugins: Vec<String>,
    /// Focus within the General tab.
    pub general_focus: GeneralTabFocus,
    // --- Paths tab ---
    /// Staged library_folders as editable entries.
    pub library_entries: Vec<PathEntryState>,
    /// Selected entry index in library_entries.
    pub library_selected: usize,
    /// Whether the selected library entry is actively being edited.
    pub library_editing: bool,
    /// Staged download_path value.
    pub download_path: String,
    /// Byte cursor in download_path text field.
    pub download_path_cursor: usize,
    /// Whether download_path is actively being edited.
    pub download_path_editing: bool,
    /// Validation error for download_path.
    pub download_path_error: Option<String>,
    /// Staged projects_directory value.
    pub projects_directory: String,
    /// Byte cursor in projects_directory text field.
    pub projects_directory_cursor: usize,
    /// Whether projects_directory is actively being edited.
    pub projects_directory_editing: bool,
    /// Validation error for projects_directory.
    pub projects_directory_error: Option<String>,
    /// Focus within the Paths tab.
    pub paths_focus: PathsTabFocus,
    // --- Plugins tab ---
    /// Selected plugin index.
    pub plugins_selected: usize,
    /// Scroll offset for the plugins list.
    pub plugins_scroll: usize,
    /// Original theme before live preview started.
    pub original_theme: String,
}

impl Default for SettingsModalState {
    fn default() -> Self {
        Self {
            tab_bar_focused: true,
            tab: SettingsTab::default(),
            theme_selected: 0,
            theme_scroll: 0,
            startup_page: String::new(),
            startup_page_selected: 0,
            pdf_viewer: String::new(),
            pdf_viewer_cursor: 0,
            pdf_viewer_editing: false,
            keyword_entries: Vec::new(),
            keyword_selected: 0,
            keyword_editing: false,
            enabled_plugins: Vec::new(),
            general_focus: GeneralTabFocus::default(),
            library_entries: Vec::new(),
            library_selected: 0,
            library_editing: false,
            download_path: String::new(),
            download_path_cursor: 0,
            download_path_editing: false,
            download_path_error: None,
            projects_directory: String::new(),
            projects_directory_cursor: 0,
            projects_directory_editing: false,
            projects_directory_error: None,
            paths_focus: PathsTabFocus::default(),
            plugins_selected: 0,
            plugins_scroll: 0,
            original_theme: String::new(),
        }
    }
}

/// Logical focus target within the Projects workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectPane {
    /// Project browser before a project is opened.
    #[default]
    ProjectList,
    /// Project file tree.
    FileTree,
    /// Source editor.
    Editor,
    /// Compiler diagnostics.
    Build,
    /// Live PDF preview.
    Preview,
}

/// Target to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionTarget {
    /// A LaTex project directory.
    Project {
        /// Project metadata and directory to remove.
        project: Project,
    },
    /// A paper to delete.
    Paper {
        /// ID of the paper in DB.
        id: i64,
        /// Title of the paper.
        title: String,
        /// Path to the PDF file.
        path: Option<std::path::PathBuf>,
    },
    /// A collection to delete.
    Collection {
        /// ID of the collection in DB.
        id: i64,
        /// Name of the collection.
        name: String,
        /// Path to the collection directory.
        path: Option<std::path::PathBuf>,
    },
    /// A file or folder inside an open project.
    ProjectEntry {
        /// Entry path.
        path: std::path::PathBuf,
        /// Display name.
        name: String,
        /// Whether the entry is a folder and will be deleted recursively.
        is_directory: bool,
    },
}

/// Active metadata input prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataPrompt {
    /// Target paper identifier.
    pub paper_id: Option<i64>,
    /// Collection identifier when renaming.
    pub rename_collection_id: Option<i64>,
    /// Paper identifier when renaming a PDF.
    pub rename_paper_id: Option<i64>,
    /// Editable input value.
    pub value: String,
    /// Cursor position in the input value.
    pub cursor: usize,
    /// Selected existing collection.
    pub selected: usize,
    /// Current collection name, if assigning a paper already in a collection.
    pub current_collection: Option<String>,
}

/// Current state of a remote discovery request.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DiscoveryStatus {
    /// No search has been submitted.
    #[default]
    Idle,
    /// A request is running in the background.
    Loading,
    /// Results are ready for browsing.
    Ready,
    /// The provider returned an error.
    Error(String),
}

/// Search query, results, and selection state.
#[derive(Debug, Default)]
pub struct DiscoveryState {
    /// Query currently shown in the search field.
    pub query: String,
    /// Cursor position in the query field.
    pub query_cursor: usize,
    /// Text used to refine the current result cache without starting a search.
    pub filter: String,
    /// Cursor position in the result filter.
    pub filter_cursor: usize,
    /// Papers returned by the most recent request.
    pub results: Vec<RemotePaper>,
    /// Ranked indexes into `results` that are visible after applying `filter`.
    #[doc(hidden)]
    pub filtered_indices: Vec<usize>,
    /// Zero-based page in the globally ranked result cache.
    pub page: usize,
    /// Selected result row.
    pub selected: usize,
    /// Vertical list scroll offset.
    pub scroll: usize,
    /// Remembered selection for each cached page.
    pub page_selections: Vec<usize>,
    /// Remembered scroll offset for each cached page.
    pub page_scrolls: Vec<usize>,
    /// Network request state.
    pub status: DiscoveryStatus,
    /// Monotonically increasing identifier for ignoring superseded searches.
    pub request_id: u64,
    /// Offset of the next remote candidate batch, when more results remain.
    pub next_batch_start: Option<u16>,
    /// Non-blocking progress or partial-failure message shown above the results.
    pub progress_message: Option<String>,
    /// Vertical detail-page scroll offset.
    pub detail_scroll: u16,
}

impl DiscoveryState {
    /// Number of results displayed on one Discover page.
    pub const PAGE_SIZE: usize = 50;

    /// Number of available pages in the globally ranked result cache.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.visible_result_count().div_ceil(Self::PAGE_SIZE)
    }

    /// Results belonging to the currently visible page.
    #[must_use]
    pub fn current_page_results(&self) -> &[RemotePaper] {
        let start = self.page.saturating_mul(Self::PAGE_SIZE).min(self.results.len());
        let end = (start + Self::PAGE_SIZE).min(self.results.len());
        &self.results[start..end]
    }

    /// Results on the visible page after applying the local filter.
    pub fn visible_page_results(&self) -> Box<dyn Iterator<Item = &RemotePaper> + '_> {
        if self.uses_unfiltered_fallback() {
            Box::new(self.current_page_results().iter())
        } else {
            Box::new(self.current_page_indices().iter().filter_map(|&index| self.results.get(index)))
        }
    }

    /// Number of filtered results on the visible page.
    #[must_use]
    pub fn visible_page_len(&self) -> usize {
        if self.uses_unfiltered_fallback() {
            self.current_page_results().len()
        } else {
            self.current_page_indices().len()
        }
    }

    /// Total count after applying the local filter.
    #[must_use]
    pub fn filtered_result_count(&self) -> usize {
        self.visible_result_count()
    }

    /// Selected paper on the currently visible page.
    #[must_use]
    pub fn selected_paper(&self) -> Option<&RemotePaper> {
        if self.uses_unfiltered_fallback() {
            self.current_page_results().get(self.selected)
        } else {
            self.current_page_indices()
                .get(self.selected)
                .and_then(|&index| self.results.get(index))
        }
    }

    /// Replace the cached result set after one globally ranked search.
    pub fn set_results(&mut self, results: Vec<RemotePaper>) {
        self.results = results;
        self.rebuild_filter();
        self.page = 0;
        self.selected = 0;
        self.scroll = 0;
        self.page_selections = vec![0; self.page_count()];
        self.page_scrolls = vec![0; self.page_count()];
    }

    /// Start a new search and discard results belonging to the previous query.
    pub fn begin_search(&mut self) -> u64 {
        self.request_id = self.request_id.wrapping_add(1);
        self.filter.clear();
        self.filter_cursor = 0;
        self.set_results(Vec::new());
        self.status = DiscoveryStatus::Loading;
        self.next_batch_start = None;
        self.progress_message = None;
        self.request_id
    }

    /// Replace an in-progress result snapshot without discarding the paper the user selected.
    pub fn update_results(&mut self, results: Vec<RemotePaper>) {
        let selected_id = self.selected_paper().map(|paper| paper.id.clone());
        self.results = results;
        self.rebuild_filter_with_selected(selected_id.as_deref());
        let page_count = self.page_count();
        self.page_selections.resize(page_count, 0);
        self.page_scrolls.resize(page_count, 0);

        self.page = self.page.min(page_count.saturating_sub(1));
        self.selected = self
            .selected
            .min(self.visible_page_len().saturating_sub(1));
    }

    /// Move to the next cached page, preserving the current page view state.
    pub fn next_page(&mut self) -> bool {
        if self.page + 1 >= self.page_count() {
            return false;
        }
        self.store_page_view();
        self.page += 1;
        self.restore_page_view();
        true
    }

    /// Move to the previous cached page, preserving the current page view state.
    pub fn previous_page(&mut self) -> bool {
        if self.page == 0 {
            return false;
        }
        self.store_page_view();
        self.page -= 1;
        self.restore_page_view();
        true
    }

    /// Save the current page's selection and scroll offset after rendering.
    pub fn store_page_view(&mut self) {
        if let Some(selection) = self.page_selections.get_mut(self.page) {
            *selection = self.selected;
        }
        if let Some(scroll) = self.page_scrolls.get_mut(self.page) {
            *scroll = self.scroll;
        }
    }

    fn restore_page_view(&mut self) {
        self.selected = self.page_selections.get(self.page).copied().unwrap_or(0);
        self.scroll = self.page_scrolls.get(self.page).copied().unwrap_or(0);
        self.selected = self.selected.min(self.visible_page_len().saturating_sub(1));
    }

    /// Re-rank the visible result indexes after a local filter edit.
    pub fn rebuild_filter(&mut self) {
        // A changed filter defines a new ranked result set.  Always start at
        // its best match rather than retaining an index from the old set.
        self.rebuild_filter_with_selected(None);
    }

    fn rebuild_filter_with_selected(&mut self, selected_id: Option<&str>) {
        self.filtered_indices = self
            .results
            .iter()
            .enumerate()
            .filter(|(_, paper)| {
                App::matches_query(
                    &self.filter,
                    &paper.title,
                    &paper.author_line(),
                    paper.doi.as_deref(),
                    Some(paper.id.as_str()),
                )
            })
            .map(|(index, _)| index)
            .collect();
        self.page_selections = vec![0; self.page_count()];
        self.page_scrolls = vec![0; self.page_count()];
        self.page = 0;
        self.selected = 0;
        self.scroll = 0;
        if let Some(selected_id) = selected_id
            && let Some(index) = self.filtered_indices.iter().position(|&index| self.results[index].id == selected_id)
        {
            self.page = index / Self::PAGE_SIZE;
            self.selected = index % Self::PAGE_SIZE;
        }
    }

    fn current_page_indices(&self) -> &[usize] {
        let start = self.page.saturating_mul(Self::PAGE_SIZE).min(self.filtered_indices.len());
        let end = (start + Self::PAGE_SIZE).min(self.filtered_indices.len());
        &self.filtered_indices[start..end]
    }

    fn visible_result_count(&self) -> usize {
        if self.uses_unfiltered_fallback() { self.results.len() } else { self.filtered_indices.len() }
    }

    fn uses_unfiltered_fallback(&self) -> bool {
        self.filter.is_empty() && self.filtered_indices.is_empty() && !self.results.is_empty()
    }
}

/// An item in the hierarchical collection search view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollectionSearchItem<'a> {
    /// A collection.
    Collection(&'a CollectionSummary),
    /// A paper inside a collection.
    Paper(&'a LibraryPaper, &'a CollectionSummary),
}

/// Local catalog rows and selection.
#[derive(Debug, Default)]
pub struct LibraryState {
    /// Papers currently loaded from `SQLite`.
    pub papers: Vec<LibraryPaper>,
    /// Selected library row.
    pub selected: usize,
    /// Vertical list scroll offset.
    pub scroll: usize,
    /// Whether a background filesystem scan is active.
    pub indexing: bool,
    /// Last indexing summary or failure.
    pub message: Option<String>,
}

/// State of one visible background transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStatus {
    /// Waiting to receive response bytes.
    Starting,
    /// Bytes are actively streaming.
    Running,
    /// Extracting metadata from PDF (inspection, etc.)
    ExtractingMetadata,
    /// Fetching paper metadata from online API (arXiv/Crossref)
    Enriching,
    /// Renaming target PDF file based on title/metadata
    Renaming,
    /// PDF has been finalized and indexed.
    Completed,
    /// Transfer or indexing failed.
    Failed(String),
}


/// Download progress shown in the Downloads page and status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadTask {
    /// arXiv identifier.
    pub id: String,
    /// Paper title.
    pub title: String,
    /// Persisted bytes.
    pub downloaded: u64,
    /// Expected response size when supplied by the server.
    pub total: Option<u64>,
    /// Associated database paper ID, if attached.
    pub paper_id: Option<i64>,
    /// Final or current PDF path on disk.
    pub pdf_path: Option<String>,
    /// Current transfer state.
    pub status: DownloadStatus,
    /// Remote paper metadata preserved for retries.
    pub remote_paper: Option<RemotePaper>,
    /// When the task failed (used to auto-cleanup older failures).
    pub failed_at: Option<std::time::Instant>,
}

impl DownloadTask {
    /// Display filename without the PDF extension, falling back to the remote paper title.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.pdf_path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| self.title.strip_suffix(".pdf").unwrap_or(&self.title))
    }
}

/// Commands available to keybindings and Browse Papr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Move selection up.
    MoveUp,
    /// Move selection down.
    MoveDown,
    /// Activate selected sidebar page.
    Open,
    /// Return focus to the navigation pane.
    Back,
    /// Show command lookup.
    TogglePalette,
    /// Show shortcut help.
    ToggleHelp,
    /// Toggle workspace-local search focus.
    ToggleWorkspaceSearch,
    /// Terminate the application.
    Quit,
}

/// Complete UI state independent of terminal rendering.
#[derive(Debug)]
pub struct App {
    /// Active content page.
    pub page: Page,
    /// Selected sidebar row.
    pub sidebar_index: usize,
    /// Vertical list scroll offset for the sidebar.
    pub sidebar_scroll: usize,
    /// Whether keyboard input targets section content instead of the navigation pane.
    pub content_focused: bool,
    /// Current modal input mode.
    pub mode: AppMode,
    /// First visible row in the complete keyboard reference.
    pub help_scroll: usize,
    /// Mode to restore after dismissing the keyboard reference.
    pub help_return_mode: AppMode,
    /// Whether the event loop should stop.
    pub should_quit: bool,
    /// Summary metrics loaded from persistence.
    pub stats: DashboardStats,
    /// Complete local dashboard snapshot.
    pub dashboard: ResearchDashboard,
    /// Latest papers loaded for Today in Research.
    pub today_papers: Vec<RemotePaper>,
    /// Selected dashboard paper row.
    pub today_selected: usize,
    /// Vertical list scroll offset for the dashboard feed.
    pub today_scroll: usize,
    /// Loading state for the latest-paper feed.
    pub today_status: DiscoveryStatus,
    /// Browse Papr selected row.
    pub palette_selected: usize,
    /// Vertical list scroll offset for Browse Papr.
    pub palette_scroll: usize,
    /// Browse Papr search query.
    pub palette_query: String,
    /// Cursor position in Browse Papr query.
    pub palette_query_cursor: usize,
    /// Selected credits item row.
    pub credits_selected: usize,
    /// Vertical list scroll offset for credits list.
    pub credits_scroll: usize,
    /// Local workspace search query.
    pub workspace_query: String,
    /// Cursor position in workspace query.
    pub workspace_query_cursor: usize,
    /// Active search mode for each supported workspace page.
    pub active_search_workspaces: std::collections::HashSet<Page>,
    /// Projects discovered in the configured projects directory and registry.
    pub projects: Vec<Project>,
    /// Selected project in the browser.
    pub projects_selected: usize,
    /// Open project, if the split writing workspace is active.
    pub active_project: Option<Project>,
    /// Files and folders shown in the active project's tree.
    pub project_files: Vec<std::path::PathBuf>,
    /// Directory currently displayed by the active project's file tree.
    pub project_tree_dir: Option<std::path::PathBuf>,
    /// Selected file in the project tree.
    pub project_file_selected: usize,
    /// Text buffer for the selected source file.
    pub project_editor_text: String,
    /// Source file backing the current editor buffer.
    pub project_editor_path: Option<std::path::PathBuf>,
    /// Whether the project editor has unsaved changes.
    pub project_editor_dirty: bool,
    /// Byte cursor in the project editor buffer.
    pub project_editor_cursor: usize,
    /// Vim insert-mode state for the project editor.
    pub project_editor_insert_mode: bool,
    /// Vertical visual-row offset for the project editor.
    pub project_editor_scroll: usize,
    /// Cached wrapped width of the project editor viewport.
    pub project_editor_wrap_width: usize,
    /// Cached height of the project editor viewport.
    pub project_editor_viewport_height: usize,
    /// Items currently offered by the editor completion engine.
    pub project_completions: Vec<crate::completions::CompletionItem>,
    /// Selected item in the completion popup.
    pub project_completion_selected: usize,
    /// Last compiler status shown in the writing workspace.
    pub project_build_status: String,
    /// Latest compiler diagnostics, preserving the last good PDF on failure.
    pub project_build_errors: Vec<String>,
    /// First visible compiler-output line in the Projects build pane.
    pub project_build_scroll: usize,
    /// Cached content height of the Projects build pane.
    pub project_build_viewport_height: usize,
    /// Current logical Projects workspace focus.
    pub project_pane: ProjectPane,
    /// Pending project name while the rename prompt is open.
    pub project_rename_input: String,
    /// Byte cursor within the project name modal input.
    pub project_rename_cursor: usize,
    /// Remote paper discovery state.
    pub discovery: DiscoveryState,
    /// Scroll offset within the currently open paper detail view.
    pub paper_detail_scroll: u16,
    /// Local paper catalog state.
    pub library: LibraryState,
    /// Papers in the reading queue.
    pub reading_queue_papers: Vec<LibraryPaper>,
    /// Selected reading queue paper row.
    pub reading_queue_selected: usize,
    /// Vertical list scroll offset for the reading queue.
    pub reading_queue_scroll: usize,
    /// Background PDF transfers.
    pub downloads: Vec<DownloadTask>,
    /// Selected transfer row.
    pub download_selected: usize,
    /// Vertical list scroll offset for active downloads.
    pub download_scroll: usize,
    /// Currently edited note.
    pub note_editor: Option<PaperNote>,
    /// Whether the note overlay shows rendered Markdown instead of source.
    pub note_preview: bool,
    /// Vertical scroll offset shared by the note source and preview panes.
    pub note_scroll: u16,
    /// Active collection prompt.
    pub metadata_prompt: Option<MetadataPrompt>,
    /// Collection summaries.
    pub collections: Vec<CollectionSummary>,
    /// Selected collection row.
    pub collection_selected: usize,
    /// Vertical list scroll offset for the collections list.
    pub collection_scroll: usize,
    /// Collection currently opened for paper browsing.
    pub active_collection: Option<CollectionSummary>,
    /// Papers assigned to the active collection.
    pub collection_papers: Vec<LibraryPaper>,
    /// Selected paper within the active collection.
    pub collection_paper_selected: usize,
    /// Vertical list scroll offset for papers within a collection.
    pub collection_paper_scroll: usize,
    /// Cached mapping from collection ID to paper IDs.
    pub collection_papers_map: std::collections::HashMap<i64, Vec<i64>>,
    /// Most recently opened collection, used to restore its paper cursor.
    pub last_opened_collection_id: Option<i64>,
    /// Active delete confirmation state.
    pub delete_confirmation: Option<DeletionTarget>,
    /// Bookmark summaries.
    pub bookmarks: Vec<BookmarkSummary>,
    /// Selected bookmarked PDF row.
    pub bookmark_selected: usize,
    /// Vertical list scroll offset for bookmarks.
    pub bookmark_scroll: usize,
    /// Authors.
    pub authors: Vec<AuthorSummary>,
    /// Selected author row.
    pub author_selected: usize,
    /// Vertical list scroll offset for the authors list.
    pub author_scroll: usize,
    /// Author currently opened for paper browsing.
    pub active_author: Option<AuthorSummary>,
    /// Papers assigned to the active author.
    pub author_papers: Vec<LibraryPaper>,
    /// Selected paper within the active author.
    pub author_paper_selected: usize,
    /// Vertical list scroll offset for papers within an author.
    pub author_paper_scroll: usize,
    /// Most recently opened author, used to restore its paper cursor.
    pub last_opened_author_id: Option<i64>,
    /// Short user-facing operation result.
    pub toast: Option<String>,
    /// Timestamp when toast was last set.
    pub toast_timestamp: Option<std::time::Instant>,
    /// Mode restored when an editor or prompt closes.
    pub modal_return: AppMode,
    /// Discovered plugin summaries.
    pub plugins: Vec<PluginInfo>,
    /// Number of invalid plugin bundles found at startup.
    pub plugin_diagnostics: usize,
    /// Whether background arXiv metadata enrichment is in progress.
    pub enrichment_pending: bool,
    /// Papers with associated notes.
    pub notes_papers: Vec<LibraryPaper>,
    /// Selected paper with notes.
    pub notes_selected: usize,
    /// Vertical scroll offset for notes papers.
    pub notes_scroll: usize,
    /// Text of the config.toml file.
    pub config_editor_text: String,
    /// Byte cursor position in config_editor_text.
    pub config_editor_cursor: usize,
    /// Whether the configuration editor currently has keyboard focus.
    pub config_editor_focused: bool,
    /// Whether the editor is in insert mode (otherwise normal mode).
    pub config_editor_insert_mode: bool,
    /// Validation error message if the saved configuration is invalid.
    pub config_editor_error: Option<String>,
    /// Vertical scroll offset of the editor.
    pub config_editor_scroll: usize,
    /// Undo history log of configuration states.
    pub config_editor_history: Vec<String>,
    /// Index pointing to active state in config_editor_history.
    pub config_editor_history_idx: usize,
    /// Current Vim command string being entered.
    pub config_editor_command: Option<String>,
    /// Cached wrapped content width of the editor viewport.
    pub config_editor_wrap_width: usize,
    /// Cached visible height of the editor viewport in visual rows.
    pub config_editor_viewport_height: usize,
    /// Preferred visual column for vertical cursor movement.
    pub config_editor_goal_column: Option<usize>,
    /// Internal PDF viewer path.
    pub pdf_viewer_path: Option<std::path::PathBuf>,
    /// Internal PDF viewer current page (1-indexed).
    pub pdf_viewer_page: usize,
    /// Scroll offset within the current page in terminal cell rows.
    pub pdf_viewer_scroll_y: u32,
    /// Internal PDF viewer total page count.
    pub pdf_viewer_total_pages: usize,
    /// Full pixel height of the last rendered page (used for upward page transitions).
    pub pdf_viewer_page_pixel_h: u32,
    /// Maximum scroll_y (in cell rows) for the current page and viewport.
    /// Written by draw_pdf_viewer every frame; read by pdf_scroll to detect
    /// when the user has reached the bottom of the page.
    pub pdf_viewer_max_scroll_y: u32,
    /// Active internal reading session ID.
    pub active_pdf_session_id: Option<i64>,
    /// Active internal reading session start time.
    pub active_pdf_session_start: Option<std::time::Instant>,
    /// Configured PDF viewer name.
    pub pdf_viewer: String,
    /// State of the interactive settings modal.
    pub settings_modal: SettingsModalState,
    /// Available startup page choices.
    pub startup_page_options: Vec<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            page: Page::Dashboard,
            sidebar_index: 0,
            sidebar_scroll: 0,
            content_focused: false,
            mode: AppMode::Normal,
            help_scroll: 0,
            help_return_mode: AppMode::Normal,
            should_quit: false,
            stats: DashboardStats::default(),
            dashboard: ResearchDashboard::default(),
            today_papers: Vec::new(),
            today_selected: 0,
            today_scroll: 0,
            today_status: DiscoveryStatus::Idle,
            palette_selected: 0,
            palette_scroll: 0,
            palette_query: String::new(),
            palette_query_cursor: 0,
            credits_selected: 0,
            credits_scroll: 0,
            workspace_query: String::new(),
            workspace_query_cursor: 0,
            active_search_workspaces: std::collections::HashSet::new(),
            projects: Vec::new(),
            projects_selected: 0,
            active_project: None,
            project_files: Vec::new(),
            project_tree_dir: None,
            project_file_selected: 0,
            project_editor_text: String::new(),
            project_editor_path: None,
            project_editor_dirty: false,
            project_editor_cursor: 0,
            project_editor_insert_mode: false,
            project_editor_scroll: 0,
            project_editor_wrap_width: 1,
            project_editor_viewport_height: 0,
            project_completions: Vec::new(),
            project_completion_selected: 0,
            project_build_status: "Idle".into(),
            project_build_errors: Vec::new(),
            project_build_scroll: 0,
            project_build_viewport_height: 0,
            project_pane: ProjectPane::ProjectList,
            project_rename_input: String::new(),
            project_rename_cursor: 0,
            discovery: DiscoveryState::default(),
            paper_detail_scroll: 0,
            library: LibraryState::default(),
            reading_queue_papers: Vec::new(),
            reading_queue_selected: 0,
            reading_queue_scroll: 0,
            downloads: Vec::new(),
            download_selected: 0,
            download_scroll: 0,
            note_editor: None,
            note_preview: false,
            note_scroll: 0,
            metadata_prompt: None,
            collections: Vec::new(),
            collection_selected: 0,
            collection_scroll: 0,
            active_collection: None,
            collection_papers: Vec::new(),
            collection_paper_selected: 0,
            collection_paper_scroll: 0,
            collection_papers_map: std::collections::HashMap::new(),
            last_opened_collection_id: None,
            delete_confirmation: None,
            bookmarks: Vec::new(),
            bookmark_selected: 0,
            bookmark_scroll: 0,
            authors: Vec::new(),
            author_selected: 0,
            author_scroll: 0,
            active_author: None,
            author_papers: Vec::new(),
            author_paper_selected: 0,
            author_paper_scroll: 0,
            last_opened_author_id: None,
            toast: None,
            toast_timestamp: None,
            modal_return: AppMode::Normal,
            plugins: Vec::new(),
            plugin_diagnostics: 0,
            enrichment_pending: false,
            notes_papers: Vec::new(),
            notes_selected: 0,
            notes_scroll: 0,
            config_editor_text: String::new(),
            config_editor_cursor: 0,
            config_editor_focused: false,
            config_editor_insert_mode: false,
            config_editor_error: None,
            config_editor_scroll: 0,
            config_editor_history: Vec::new(),
            config_editor_history_idx: 0,
            config_editor_command: None,
            config_editor_wrap_width: 1,
            config_editor_viewport_height: 0,
            config_editor_goal_column: None,
            pdf_viewer_path: None,
            pdf_viewer_page: 1,
            pdf_viewer_scroll_y: 0,
            pdf_viewer_total_pages: 0,
            pdf_viewer_page_pixel_h: 0,
            pdf_viewer_max_scroll_y: 0,
            active_pdf_session_id: None,
            active_pdf_session_start: None,
            pdf_viewer: "internal".to_string(),
            settings_modal: SettingsModalState::default(),
            startup_page_options: vec![
                "dashboard".into(),
                "discover".into(),
                "library".into(),
                "reading_queue".into(),
                "projects".into(),
            ],
        }
    }
}

/// Represents a clickable credit/dependency item on the Credits/About page.
#[derive(Debug, Clone)]
pub struct InteractiveCreditItem {
    /// The display label.
    pub label: String,
    /// The external URL.
    pub url: String,
}

impl App {
    /// Set the active page and synchronize `sidebar_index`.
    pub fn set_page(&mut self, page: Page) {
        self.page = page;
        if let Some(index) = Page::ALL.iter().position(|&p| p == page) {
            self.sidebar_index = index;
        }
    }

    /// Returns the local PDF record for a remote paper when it has been downloaded.
    #[must_use]
    pub fn downloaded_remote_paper(&self, remote: &RemotePaper) -> Option<&LibraryPaper> {
        self.library.papers.iter().find(|local| {
            if local.pdf_path.is_none() {
                return false;
            }
            if local.arxiv_id.as_deref() == Some(remote.id.as_str()) {
                return true;
            }
            if let (Some(local_doi), Some(remote_doi)) = (&local.doi, &remote.doi) {
                if local_doi.eq_ignore_ascii_case(remote_doi) {
                    return true;
                }
            }
            local.title.eq_ignore_ascii_case(&remote.title)
        })
    }

    /// Get dependencies parsed from Cargo.toml.
    pub fn get_dependencies(&self) -> Vec<(String, String)> {
        let mut deps = Vec::new();
        if let Ok(value) = toml::from_str::<toml::Value>(include_str!("../../../Cargo.toml")) {
            if let Some(workspace) = value.get("workspace") {
                if let Some(dependencies) = workspace.get("dependencies") {
                    if let Some(table) = dependencies.as_table() {
                        for (k, v) in table {
                            let version = match v {
                                toml::Value::String(s) => s.clone(),
                                toml::Value::Table(t) => {
                                    t.get("version")
                                        .and_then(|ver| ver.as_str())
                                        .unwrap_or("")
                                        .to_string()
                                }
                                _ => "".to_string(),
                            };
                            deps.push((k.clone(), version));
                        }
                    }
                }
            }
        }
        deps.sort_by(|a, b| a.0.cmp(&b.0));
        deps
    }

    /// Get list of all interactive credits items (docs, libraries, dependencies).
    pub fn credits_items(&self) -> Vec<InteractiveCreditItem> {
        let mut items = vec![
            InteractiveCreditItem {
                label: "GitHub Repository (AfrozSaqlain/Papr)".to_string(),
                url: "https://github.com/AfrozSaqlain/Papr".to_string(),
            },
            InteractiveCreditItem {
                label: "arXiv API Documentation".to_string(),
                url: "https://arxiv.org/help/api/index".to_string(),
            },
            InteractiveCreditItem {
                label: "Crossref REST API".to_string(),
                url: "https://www.crossref.org/documentation/retrieve-metadata/rest-api/".to_string(),
            },
            InteractiveCreditItem {
                label: "Ratatui TUI Framework".to_string(),
                url: "https://ratatui.rs/".to_string(),
            },
            InteractiveCreditItem {
                label: "Tokio Async Runtime".to_string(),
                url: "https://tokio.rs/".to_string(),
            },
            InteractiveCreditItem {
                label: "SQLite Database Engine".to_string(),
                url: "https://www.sqlite.org/".to_string(),
            },
        ];

        for (name, version) in self.get_dependencies() {
            items.push(InteractiveCreditItem {
                label: format!("{} ({})", name, version),
                url: format!("https://crates.io/crates/{}", name),
            });
        }

        items
    }

    /// Check if a query matches the title, authors, DOI, or arXiv ID case-insensitively.
    pub fn matches_query(
        query: &str,
        title: &str,
        authors: &str,
        doi: Option<&str>,
        arxiv_id: Option<&str>,
    ) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.trim().to_ascii_lowercase();

        // 1. Check if the query matches an arXiv ID
        if let Some(q_arxiv) = parse_arxiv_id(&q) {
            if let Some(p_arxiv_raw) = arxiv_id {
                if let Some(p_arxiv) = parse_arxiv_id(&clean_arxiv_id(p_arxiv_raw)) {
                    if strip_arxiv_version(&q_arxiv) == strip_arxiv_version(&p_arxiv) {
                        return true;
                    }
                }
            }
        }

        // 2. Check if the query matches DOI
        if let Some(p_doi) = doi {
            if p_doi.trim().to_ascii_lowercase() == q {
                return true;
            }
        }

        // 3. Fall back to title and authors matching
        title.to_lowercase().contains(&q) || authors.to_lowercase().contains(&q)
    }

    /// Get the library papers filtered by the workspace search query.
    pub fn filtered_library_papers(&self) -> Vec<&LibraryPaper> {
        self.library
            .papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors, p.doi.as_deref(), p.arxiv_id.as_deref()))
            .collect()
    }

    /// Get the reading queue papers filtered by the workspace search query.
    pub fn filtered_reading_queue_papers(&self) -> Vec<&LibraryPaper> {
        self.reading_queue_papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors, p.doi.as_deref(), p.arxiv_id.as_deref()))
            .collect()
    }

    /// Get the download tasks filtered by the workspace search query.
    pub fn filtered_downloads(&self) -> Vec<&DownloadTask> {
        self.downloads
            .iter()
            .filter(|d| {
                let paper = if let Some(paper_id) = d.paper_id {
                    self.library.papers.iter().find(|p| p.id == paper_id)
                } else if let Some(pdf_path) = &d.pdf_path {
                    self.library.papers.iter().find(|p| p.pdf_path.as_ref() == Some(pdf_path))
                } else {
                    None
                };
                if let Some(p) = paper {
                    Self::matches_query(&self.workspace_query, &p.title, &p.authors, p.doi.as_deref(), p.arxiv_id.as_deref())
                } else {
                    let raw_arxiv = d.id.strip_prefix("arxiv:").unwrap_or(&d.id);
                    Self::matches_query(&self.workspace_query, &d.title, "", None, Some(raw_arxiv))
                }
            })
            .collect()
    }

    /// Get the collections filtered by the workspace search query.
    pub fn filtered_collections(&self) -> Vec<CollectionSearchItem<'_>> {
        let mut results = Vec::new();
        if self.workspace_query.is_empty() {
            for c in &self.collections {
                results.push(CollectionSearchItem::Collection(c));
            }
            return results;
        }

        for c in &self.collections {
            if let Some(paper_ids) = self.collection_papers_map.get(&c.id) {
                let mut matching_papers = Vec::new();
                for &pid in paper_ids {
                    if let Some(p) = self.library.papers.iter().find(|p| p.id == pid) {
                        if Self::matches_query(&self.workspace_query, &p.title, &p.authors, p.doi.as_deref(), p.arxiv_id.as_deref()) {
                            matching_papers.push(p);
                        }
                    }
                }
                if !matching_papers.is_empty() {
                    results.push(CollectionSearchItem::Collection(c));
                    for p in matching_papers {
                        results.push(CollectionSearchItem::Paper(p, c));
                    }
                }
            }
        }
        results
    }

    /// Get the active collection's papers filtered by the workspace search query.
    pub fn filtered_collection_papers(&self) -> Vec<&LibraryPaper> {
        self.collection_papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors, p.doi.as_deref(), p.arxiv_id.as_deref()))
            .collect()
    }

    /// Get the authors filtered by the workspace search query.
    pub fn filtered_authors(&self) -> Vec<&AuthorSummary> {
        self.authors
            .iter()
            .filter(|a| Self::matches_query(&self.workspace_query, &a.name, "", None, None))
            .collect()
    }

    /// Get the active author's papers filtered by the workspace search query.
    pub fn filtered_author_papers(&self) -> Vec<&LibraryPaper> {
        self.author_papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors, p.doi.as_deref(), p.arxiv_id.as_deref()))
            .collect()
    }

    /// Get the bookmarks filtered by the workspace search query.
    pub fn filtered_bookmarks(&self) -> Vec<&BookmarkSummary> {
        self.bookmarks
            .iter()
            .filter(|b| {
                let lib_paper = self.library.papers.iter().find(|p| p.id == b.paper_id);
                let paper_doi = lib_paper.and_then(|p| p.doi.as_deref());
                let paper_arxiv = lib_paper.and_then(|p| p.arxiv_id.as_deref());
                Self::matches_query(&self.workspace_query, &b.paper_title, &b.authors, paper_doi, paper_arxiv)
            })
            .collect()
    }

    /// Get the notes papers filtered by the workspace search query.
    pub fn filtered_notes_papers(&self) -> Vec<&LibraryPaper> {
        self.notes_papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors, p.doi.as_deref(), p.arxiv_id.as_deref()))
            .collect()
    }
    /// Get Browse Papr destinations filtered by the query.
    pub fn filtered_palette_items(&self) -> Vec<Page> {
        let q = self.palette_query.to_lowercase();
        Page::ALL
            .iter()
            .copied()
            .filter(|page| page.title().to_lowercase().contains(&q))
            .collect()
    }

    /// Apply a semantic command to the state machine.
    pub fn dispatch(&mut self, command: Command) {
        match command {
            Command::MoveUp => {
                if !self.content_focused {
                    self.sidebar_index = self.sidebar_index.saturating_sub(1);
                    self.page = Page::ALL[self.sidebar_index];
                } else if self.page == Page::Dashboard && !self.today_papers.is_empty() {
                    self.today_selected = self.today_selected.saturating_sub(1);
                } else if self.page == Page::Discover {
                    self.discovery.selected = self.discovery.selected.saturating_sub(1);
                    self.discovery.store_page_view();
                } else if self.page == Page::Library {
                    if self.library.selected == 0 {
                        self.mode = AppMode::WorkspaceSearch;
                    } else {
                        self.library.selected -= 1;
                    }
                } else if self.page == Page::Downloads {
                    if self.download_selected == 0 {
                        self.mode = AppMode::WorkspaceSearch;
                    } else {
                        self.download_selected -= 1;
                    }
                } else if self.page == Page::Collections {
                    if self.active_collection.is_some() {
                        if self.collection_paper_selected == 0 {
                            self.mode = AppMode::WorkspaceSearch;
                        } else {
                            self.collection_paper_selected -= 1;
                        }
                    } else {
                        if self.collection_selected == 0 {
                            self.mode = AppMode::WorkspaceSearch;
                        } else {
                            self.collection_selected -= 1;
                        }
                    }
                } else if self.page == Page::Authors {
                    if self.active_author.is_some() {
                        if self.author_paper_selected == 0 {
                            self.mode = AppMode::WorkspaceSearch;
                        } else {
                            self.author_paper_selected -= 1;
                        }
                    } else {
                        if self.author_selected == 0 {
                            self.mode = AppMode::WorkspaceSearch;
                        } else {
                            self.author_selected -= 1;
                        }
                    }
                } else if self.page == Page::Bookmarks {
                    if self.bookmark_selected == 0 {
                        self.mode = AppMode::WorkspaceSearch;
                    } else {
                        self.bookmark_selected -= 1;
                    }
                } else if self.page == Page::Notes {
                    if self.notes_selected == 0 {
                        self.mode = AppMode::WorkspaceSearch;
                    } else {
                        self.notes_selected -= 1;
                    }
                } else if self.page == Page::ReadingQueue {
                    if self.reading_queue_selected == 0 {
                        self.mode = AppMode::WorkspaceSearch;
                    } else {
                        self.reading_queue_selected -= 1;
                    }
                } else if self.page == Page::Credits {
                    self.credits_selected = self.credits_selected.saturating_sub(1);
                }
                if self.mode == AppMode::WorkspaceSearch {
                    self.active_search_workspaces.insert(self.page);
                }
            }
            Command::MoveDown => {
                if !self.content_focused {
                     self.sidebar_index =
                        (self.sidebar_index + 1).min(Page::ALL.len().saturating_sub(1));
                     self.page = Page::ALL[self.sidebar_index];
                } else if self.page == Page::Dashboard && !self.today_papers.is_empty() {
                    self.today_selected =
                        (self.today_selected + 1).min(self.today_papers.len().saturating_sub(1));
                } else if self.page == Page::Discover {
                    self.discovery.selected = (self.discovery.selected + 1)
                        .min(self.discovery.visible_page_len().saturating_sub(1));
                    self.discovery.store_page_view();
                } else if self.page == Page::Library {
                    self.library.selected = (self.library.selected + 1)
                        .min(self.filtered_library_papers().len().saturating_sub(1));
                } else if self.page == Page::Downloads {
                    self.download_selected =
                        (self.download_selected + 1).min(self.filtered_downloads().len().saturating_sub(1));
                } else if self.page == Page::Collections {
                    if self.active_collection.is_some() {
                        self.collection_paper_selected = (self.collection_paper_selected + 1)
                            .min(self.filtered_collection_papers().len().saturating_sub(1));
                    } else {
                        self.collection_selected = (self.collection_selected + 1)
                            .min(self.filtered_collections().len().saturating_sub(1));
                    }
                } else if self.page == Page::Authors {
                    if self.active_author.is_some() {
                        self.author_paper_selected = (self.author_paper_selected + 1)
                            .min(self.filtered_author_papers().len().saturating_sub(1));
                    } else {
                        self.author_selected =
                            (self.author_selected + 1).min(self.filtered_authors().len().saturating_sub(1));
                    }
                } else if self.page == Page::Bookmarks {
                    self.bookmark_selected =
                        (self.bookmark_selected + 1).min(self.filtered_bookmarks().len().saturating_sub(1));
                } else if self.page == Page::Notes {
                    self.notes_selected =
                        (self.notes_selected + 1).min(self.filtered_notes_papers().len().saturating_sub(1));
                } else if self.page == Page::ReadingQueue {
                    self.reading_queue_selected = (self.reading_queue_selected + 1)
                        .min(self.filtered_reading_queue_papers().len().saturating_sub(1));
                } else if self.page == Page::Credits {
                    self.credits_selected = (self.credits_selected + 1)
                        .min(self.credits_items().len().saturating_sub(1));
                }
            }
            Command::Open => {
                if !self.content_focused {
                    self.page = Page::ALL[self.sidebar_index];
                    self.content_focused = true;
                    if self.active_search_workspaces.contains(&self.page) {
                        self.mode = AppMode::WorkspaceSearch;
                    } else {
                        self.mode = AppMode::Normal;
                    }
                } else if self.page == Page::Dashboard && !self.today_papers.is_empty() {
                    self.mode = AppMode::PaperDetail;
                    self.paper_detail_scroll = 0;
                } else if self.page == Page::Discover && !self.discovery.results.is_empty() {
                    self.mode = AppMode::PaperDetail;
                    self.paper_detail_scroll = 0;
                }
            }
            Command::Back => {
                if self.mode == AppMode::PaperDetail {
                    self.mode = AppMode::Normal;
                } else {
                    self.content_focused = false;
                }
            }
            Command::TogglePalette => {
                self.mode = if self.mode == AppMode::CommandPalette {
                    AppMode::Normal
                } else {
                    AppMode::CommandPalette
                };
                self.palette_selected = 0;
                self.palette_scroll = 0;
                self.palette_query.clear();
                self.palette_query_cursor = 0;
            }
            Command::ToggleHelp => {
                if self.mode == AppMode::Help {
                    self.mode = self.help_return_mode;
                } else {
                    self.help_return_mode = self.mode;
                    self.mode = AppMode::Help;
                    self.help_scroll = 0;
                }
            }
            Command::ToggleWorkspaceSearch => {
                if self.page.supports_workspace_search() {
                    if self.mode == AppMode::WorkspaceSearch {
                        self.mode = AppMode::Normal;
                        self.content_focused = true;
                        self.active_search_workspaces.remove(&self.page);
                    } else {
                        self.mode = AppMode::WorkspaceSearch;
                        self.content_focused = true;
                        self.active_search_workspaces.insert(self.page);
                    }
                }
            }
            Command::Quit => self.should_quit = true,
        }
    }
}

fn strip_arxiv_version(id: &str) -> &str {
    if let Some(pos) = id.rfind('v') {
        if id[pos + 1..].chars().all(|c| c.is_ascii_digit()) && pos + 1 < id.len() {
            return &id[..pos];
        }
    }
    id
}

fn clean_arxiv_id(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    let clean = if let Some(idx) = s.find("/abs/") {
        &s[idx + 5..]
    } else if let Some(idx) = s.find("/pdf/") {
        let after = &s[idx + 5..];
        after.strip_suffix(".pdf").unwrap_or(after)
    } else {
        &s
    };
    clean.trim().to_string()
}

fn parse_arxiv_id(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let clean = clean_arxiv_id(&s);
    let (base, version) = if let Some(v_idx) = clean.rfind('v') {
        let (b, v) = clean.split_at(v_idx);
        let v_suffix = &v[1..];
        if !v_suffix.is_empty() && v_suffix.chars().all(|c| c.is_ascii_digit()) {
            (b, Some(v.to_string()))
        } else {
            (clean.as_str(), None)
        }
    } else {
        (clean.as_str(), None)
    };

    // Modern format: "YYMM.NNNN" or "YYMM.NNNNN"
    if base.len() >= 9 && base.len() <= 10 {
        let parts: Vec<&str> = base.split('.').collect();
        if parts.len() == 2 {
            let yymm = parts[0];
            let nnnn = parts[1];
            if yymm.len() == 4 && yymm.chars().all(|c| c.is_ascii_digit())
                && (nnnn.len() == 4 || nnnn.len() == 5) && nnnn.chars().all(|c| c.is_ascii_digit())
            {
                let mut normalized = base.to_string();
                if let Some(v) = version {
                    normalized.push_str(&v);
                }
                return Some(normalized);
            }
        }
    }

    // Legacy format: "archive/YYMMNNN" or "subject-class/YYMMNNN"
    if let Some(slash_idx) = base.find('/') {
        let (cat, num) = base.split_at(slash_idx);
        let num = &num[1..];
        if num.len() == 7 && num.chars().all(|c| c.is_ascii_digit()) {
            if !cat.is_empty() && cat.chars().all(|c| c.is_ascii_alphabetic() || c == '-' || c == '.') {
                let mut normalized = base.to_string();
                if let Some(v) = version {
                    normalized.push_str(&v);
                }
                return Some(normalized);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::models::RemotePaper;

    use super::{App, Command, DiscoveryState, Page};

    fn remote_paper(index: usize) -> RemotePaper {
        RemotePaper {
            id: format!("https://arxiv.org/abs/2501.{index:05}"),
            title: format!("Paper {index}"),
            authors: Vec::new(),
            abstract_text: String::new(),
            published: Utc::now(),
            updated: Utc::now(),
            categories: Vec::new(),
            pdf_url: None,
            doi: None,
            journal_ref: None,
        }
    }

    #[test]
    fn navigation_is_bounded_and_opens_selection() {
        let mut app = App::default();
        app.dispatch(Command::MoveUp);
        assert_eq!(app.sidebar_index, 0);
        app.dispatch(Command::MoveDown);
        app.dispatch(Command::Open);
        assert_eq!(app.page, Page::Discover);
    }

    #[test]
    fn discovery_pages_cache_results_and_restore_each_page_view() {
        let mut discovery = DiscoveryState::default();
        discovery.set_results((0..101).map(remote_paper).collect());

        assert_eq!(discovery.page_count(), 3);
        assert_eq!(discovery.current_page_results().len(), 50);
        assert_eq!(discovery.current_page_results()[0].title, "Paper 0");

        discovery.selected = 17;
        discovery.scroll = 9;
        assert!(discovery.next_page());
        assert_eq!(discovery.page, 1);
        assert_eq!(discovery.selected, 0);
        assert_eq!(discovery.scroll, 0);
        assert_eq!(discovery.current_page_results()[0].title, "Paper 50");

        discovery.selected = 4;
        discovery.scroll = 2;
        assert!(discovery.previous_page());
        assert_eq!((discovery.page, discovery.selected, discovery.scroll), (0, 17, 9));
        assert!(discovery.next_page());
        assert_eq!((discovery.page, discovery.selected, discovery.scroll), (1, 4, 2));
        assert_eq!(discovery.results.len(), 101);
    }

    #[test]
    fn incremental_discovery_updates_keep_the_selected_paper() {
        let mut discovery = DiscoveryState::default();
        discovery.set_results((0..100).map(remote_paper).collect());
        discovery.selected = 23;

        let mut updated = (100..150).map(remote_paper).collect::<Vec<_>>();
        updated.push(remote_paper(23));
        discovery.update_results(updated);

        assert_eq!(
            discovery.selected_paper().map(|paper| paper.title.as_str()),
            Some("Paper 23")
        );
        assert_eq!(discovery.begin_search(), 1);
        assert!(discovery.results.is_empty());
    }

    #[test]
    fn test_arxiv_id_search_matching() {
        // Modern ID match (with and without version)
        assert!(App::matches_query("1402.4146v2", "Title", "Authors", None, Some("http://arxiv.org/abs/1402.4146v2")));
        assert!(App::matches_query("1402.4146", "Title", "Authors", None, Some("http://arxiv.org/abs/1402.4146v2")));
        assert!(App::matches_query("1402.4146v2", "Title", "Authors", None, Some("1402.4146")));
        assert!(App::matches_query("1402.4146", "Title", "Authors", None, Some("1402.4146")));
        assert!(App::matches_query("1402.4146v1", "Title", "Authors", None, Some("1402.4146v3")));

        // Legacy ID match (with and without version)
        assert!(App::matches_query("hep-th/0309012v1", "Title", "Authors", None, Some("https://arxiv.org/abs/hep-th/0309012")));
        assert!(App::matches_query("hep-th/0309012", "Title", "Authors", None, Some("https://arxiv.org/abs/hep-th/0309012v3")));
        assert!(App::matches_query("math.GT/0309012", "Title", "Authors", None, Some("math.gt/0309012")));

        // Case insensitivity
        assert!(App::matches_query("HeP-Th/0309012", "Title", "Authors", None, Some("hep-th/0309012")));

        // Non-matching
        assert!(!App::matches_query("1402.4146", "Title", "Authors", None, Some("1502.4146")));
        assert!(!App::matches_query("hep-th/0309012", "Title", "Authors", None, Some("hep-th/0309013")));
    }

    #[test]
    fn test_discover_filter_matches_query() {
        // Exact title match
        assert!(App::matches_query("rust", "Rust", "Authors", None, None));
        // Substring title match
        assert!(App::matches_query("rust", "The Rust compiler", "Authors", None, None));
        // Author match
        assert!(App::matches_query("smith", "Title", "John Smith, Jane Doe", None, None));
        // Case insensitivity
        assert!(App::matches_query("RuSt", "rust compiler", "Authors", None, None));
        // Non-matching
        assert!(!App::matches_query("rust", "C++ compiler", "John Smith", None, None));
    }

    #[test]
    fn test_page_from_config_str_and_set_page() {
        assert_eq!(Page::from_config_str("dashboard"), Some(Page::Dashboard));
        assert_eq!(Page::from_config_str("discover"), Some(Page::Discover));
        assert_eq!(Page::from_config_str("library"), Some(Page::Library));
        assert_eq!(Page::from_config_str("reading_queue"), Some(Page::ReadingQueue));
        assert_eq!(Page::from_config_str("reading-queue"), Some(Page::ReadingQueue));
        assert_eq!(Page::from_config_str("projects"), Some(Page::Projects));
        assert_eq!(Page::from_config_str("collections"), Some(Page::Collections));
        assert_eq!(Page::from_config_str("groups"), Some(Page::Collections));
        assert_eq!(Page::from_config_str("bookmarks"), Some(Page::Bookmarks));
        assert_eq!(Page::from_config_str("authors"), Some(Page::Authors));
        assert_eq!(Page::from_config_str("notes"), Some(Page::Notes));
        assert_eq!(Page::from_config_str("downloads"), Some(Page::Downloads));
        assert_eq!(Page::from_config_str("history"), Some(Page::History));
        assert_eq!(Page::from_config_str("statistics"), Some(Page::Statistics));
        assert_eq!(Page::from_config_str("settings"), Some(Page::Settings));
        assert_eq!(Page::from_config_str("credits"), Some(Page::Credits));
        assert_eq!(Page::from_config_str("unknown_page"), None);

        let mut app = App::default();
        assert_eq!(app.page, Page::Dashboard);
        assert_eq!(app.sidebar_index, 0);

        app.set_page(Page::Projects);
        assert_eq!(app.page, Page::Projects);
        assert_eq!(app.sidebar_index, Page::ALL.iter().position(|&p| p == Page::Projects).unwrap());

        app.set_page(Page::Discover);
        assert_eq!(app.page, Page::Discover);
        assert_eq!(app.sidebar_index, Page::ALL.iter().position(|&p| p == Page::Discover).unwrap());
    }
}
