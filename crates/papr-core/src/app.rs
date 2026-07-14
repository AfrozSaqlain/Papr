//! Application state machine and navigation commands.

use crate::models::{
    AuthorSummary, BookmarkSummary, CollectionSummary, DashboardStats, LibraryPaper, PaperNote,
    RemotePaper, ResearchDashboard,
};
use crate::plugins::PluginInfo;

/// Top-level application pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Research overview.
    Dashboard,
    /// Remote discovery providers.
    Discover,
    /// Local catalog.
    Library,
    /// Prioritized reading lists.
    ReadingQueue,
    /// User-defined paper collections.
    Collections,
    /// Saved papers and locations.
    Bookmarks,
    /// Followed and cataloged authors.
    Authors,
    /// Markdown research notes.
    Notes,
    /// Background transfers.
    Downloads,
    /// Reading activity timeline.
    History,
    /// Reading analytics.
    Statistics,
    /// User preferences.
    Settings,
    /// Shortcut reference.
    Help,
}

impl Page {
    /// All pages in sidebar order.
    pub const ALL: [Self; 13] = [
        Self::Dashboard,
        Self::Discover,
        Self::Library,
        Self::ReadingQueue,
        Self::Collections,
        Self::Bookmarks,
        Self::Authors,
        Self::Notes,
        Self::Downloads,
        Self::History,
        Self::Statistics,
        Self::Settings,
        Self::Help,
    ];

    /// Human-readable navigation label.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Discover => "Discover",
            Self::Library => "Library",
            Self::ReadingQueue => "Reading Queue",
            Self::Collections => "Collections",
            Self::Bookmarks => "Bookmarks",
            Self::Authors => "Authors",
            Self::Notes => "Notes",
            Self::Downloads => "Downloads",
            Self::History => "History",
            Self::Statistics => "Statistics",
            Self::Settings => "Settings",
            Self::Help => "Help",
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
}

/// Target to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeletionTarget {
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
    /// Papers returned by the most recent request.
    pub results: Vec<RemotePaper>,
    /// Selected result row.
    pub selected: usize,
    /// Vertical list scroll offset.
    pub scroll: usize,
    /// Network request state.
    pub status: DiscoveryStatus,
    /// Vertical detail-page scroll offset.
    pub detail_scroll: u16,
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
}

/// Commands available to keybindings and the command palette.
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
    /// Command palette selected row.
    pub palette_selected: usize,
    /// Vertical list scroll offset for the command palette.
    pub palette_scroll: usize,
    /// Local workspace search query.
    pub workspace_query: String,
    /// Cursor position in workspace query.
    pub workspace_query_cursor: usize,
    /// Remote paper discovery state.
    pub discovery: DiscoveryState,
    /// Local paper catalog state.
    pub library: LibraryState,
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
}

impl Default for App {
    fn default() -> Self {
        Self {
            page: Page::Dashboard,
            sidebar_index: 0,
            sidebar_scroll: 0,
            content_focused: false,
            mode: AppMode::Normal,
            should_quit: false,
            stats: DashboardStats::default(),
            dashboard: ResearchDashboard::default(),
            today_papers: Vec::new(),
            today_selected: 0,
            today_scroll: 0,
            today_status: DiscoveryStatus::Idle,
            palette_selected: 0,
            palette_scroll: 0,
            workspace_query: String::new(),
            workspace_query_cursor: 0,
            discovery: DiscoveryState::default(),
            library: LibraryState::default(),
            downloads: Vec::new(),
            download_selected: 0,
            download_scroll: 0,
            note_editor: None,
            note_preview: false,
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
            modal_return: AppMode::Normal,
            plugins: Vec::new(),
            plugin_diagnostics: 0,
            enrichment_pending: false,
            notes_papers: Vec::new(),
            notes_selected: 0,
            notes_scroll: 0,
        }
    }
}

impl App {
    /// Check if a query matches the title or authors case-insensitively.
    pub fn matches_query(query: &str, title: &str, authors: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let q = query.to_lowercase();
        title.to_lowercase().contains(&q) || authors.to_lowercase().contains(&q)
    }

    /// Get the library papers filtered by the workspace search query.
    pub fn filtered_library_papers(&self) -> Vec<&LibraryPaper> {
        self.library
            .papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors))
            .collect()
    }

    /// Get the download tasks filtered by the workspace search query.
    pub fn filtered_downloads(&self) -> Vec<&DownloadTask> {
        self.downloads
            .iter()
            .filter(|d| Self::matches_query(&self.workspace_query, &d.title, ""))
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
                        if Self::matches_query(&self.workspace_query, &p.title, &p.authors) {
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
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors))
            .collect()
    }

    /// Get the authors filtered by the workspace search query.
    pub fn filtered_authors(&self) -> Vec<&AuthorSummary> {
        self.authors
            .iter()
            .filter(|a| Self::matches_query(&self.workspace_query, &a.name, ""))
            .collect()
    }

    /// Get the active author's papers filtered by the workspace search query.
    pub fn filtered_author_papers(&self) -> Vec<&LibraryPaper> {
        self.author_papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors))
            .collect()
    }

    /// Get the bookmarks filtered by the workspace search query.
    pub fn filtered_bookmarks(&self) -> Vec<&BookmarkSummary> {
        self.bookmarks
            .iter()
            .filter(|b| Self::matches_query(&self.workspace_query, &b.paper_title, &b.authors))
            .collect()
    }

    /// Get the notes papers filtered by the workspace search query.
    pub fn filtered_notes_papers(&self) -> Vec<&LibraryPaper> {
        self.notes_papers
            .iter()
            .filter(|p| Self::matches_query(&self.workspace_query, &p.title, &p.authors))
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
                        .min(self.discovery.results.len().saturating_sub(1));
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
                }
            }
            Command::Open => {
                if !self.content_focused {
                    self.page = Page::ALL[self.sidebar_index];
                    self.content_focused = true;
                } else if self.page == Page::Dashboard && !self.today_papers.is_empty() {
                    self.discovery.results.clone_from(&self.today_papers);
                    self.discovery.selected = self
                        .today_selected
                        .min(self.discovery.results.len().saturating_sub(1));
                    self.mode = AppMode::PaperDetail;
                    self.discovery.detail_scroll = 0;
                } else if self.page == Page::Discover && !self.discovery.results.is_empty() {
                    self.mode = AppMode::PaperDetail;
                    self.discovery.detail_scroll = 0;
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
            }
            Command::ToggleHelp => {
                if self.mode == AppMode::Help {
                    self.mode = AppMode::Normal;
                } else {
                    self.mode = AppMode::Help;
                }
            }
            Command::ToggleWorkspaceSearch => {
                if matches!(
                    self.page,
                    Page::Library | Page::Downloads | Page::Collections | Page::Authors | Page::Bookmarks
                ) {
                    if self.mode == AppMode::WorkspaceSearch {
                        self.mode = AppMode::Normal;
                        self.content_focused = true;
                    } else {
                        self.mode = AppMode::WorkspaceSearch;
                        self.content_focused = true;
                    }
                }
            }
            Command::Quit => self.should_quit = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, Command, Page};

    #[test]
    fn navigation_is_bounded_and_opens_selection() {
        let mut app = App::default();
        app.dispatch(Command::MoveUp);
        assert_eq!(app.sidebar_index, 0);
        app.dispatch(Command::MoveDown);
        app.dispatch(Command::Open);
        assert_eq!(app.page, Page::Discover);
    }
}
