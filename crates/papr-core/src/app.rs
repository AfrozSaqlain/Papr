//! Application state machine and navigation commands.

use crate::models::{
    BookmarkSummary, CollectionSummary, DashboardStats, LibraryPaper, PaperNote, RemotePaper,
    ResearchDashboard,
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
}

/// Active metadata input prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataPrompt {
    /// Target paper identifier.
    pub paper_id: Option<i64>,
    /// Collection identifier when renaming.
    pub rename_collection_id: Option<i64>,
    /// Editable input value.
    pub value: String,
    /// Selected existing collection.
    pub selected: usize,
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
    /// Command palette query.
    pub palette_query: String,
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
    /// Most recently opened collection, used to restore its paper cursor.
    pub last_opened_collection_id: Option<i64>,
    /// Bookmark summaries.
    pub bookmarks: Vec<BookmarkSummary>,
    /// Selected bookmarked PDF row.
    pub bookmark_selected: usize,
    /// Vertical list scroll offset for bookmarks.
    pub bookmark_scroll: usize,
    /// Short user-facing operation result.
    pub toast: Option<String>,
    /// Mode restored when an editor or prompt closes.
    pub modal_return: AppMode,
    /// Discovered plugin summaries.
    pub plugins: Vec<PluginInfo>,
    /// Number of invalid plugin bundles found at startup.
    pub plugin_diagnostics: usize,
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
            palette_query: String::new(),
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
            last_opened_collection_id: None,
            bookmarks: Vec::new(),
            bookmark_selected: 0,
            bookmark_scroll: 0,
            toast: None,
            modal_return: AppMode::Normal,
            plugins: Vec::new(),
            plugin_diagnostics: 0,
        }
    }
}

impl App {
    /// Apply a semantic command to the state machine.
    pub fn dispatch(&mut self, command: Command) {
        match command {
            Command::MoveUp => {
                if !self.content_focused {
                    self.sidebar_index = self.sidebar_index.saturating_sub(1);
                } else if self.page == Page::Dashboard && !self.today_papers.is_empty() {
                    self.today_selected = self.today_selected.saturating_sub(1);
                } else if self.page == Page::Discover {
                    self.discovery.selected = self.discovery.selected.saturating_sub(1);
                } else if self.page == Page::Library {
                    self.library.selected = self.library.selected.saturating_sub(1);
                } else if self.page == Page::Downloads {
                    self.download_selected = self.download_selected.saturating_sub(1);
                } else if self.page == Page::Collections {
                    if self.active_collection.is_some() {
                        self.collection_paper_selected =
                            self.collection_paper_selected.saturating_sub(1);
                    } else {
                        self.collection_selected = self.collection_selected.saturating_sub(1);
                    }
                } else if self.page == Page::Bookmarks {
                    self.bookmark_selected = self.bookmark_selected.saturating_sub(1);
                }
            }
            Command::MoveDown => {
                if !self.content_focused {
                    self.sidebar_index =
                        (self.sidebar_index + 1).min(Page::ALL.len().saturating_sub(1));
                } else if self.page == Page::Dashboard && !self.today_papers.is_empty() {
                    self.today_selected =
                        (self.today_selected + 1).min(self.today_papers.len().saturating_sub(1));
                } else if self.page == Page::Discover {
                    self.discovery.selected = (self.discovery.selected + 1)
                        .min(self.discovery.results.len().saturating_sub(1));
                } else if self.page == Page::Library {
                    self.library.selected = (self.library.selected + 1)
                        .min(self.library.papers.len().saturating_sub(1));
                } else if self.page == Page::Downloads {
                    self.download_selected =
                        (self.download_selected + 1).min(self.downloads.len().saturating_sub(1));
                } else if self.page == Page::Collections {
                    if self.active_collection.is_some() {
                        self.collection_paper_selected = (self.collection_paper_selected + 1)
                            .min(self.collection_papers.len().saturating_sub(1));
                    } else {
                        self.collection_selected = (self.collection_selected + 1)
                            .min(self.collections.len().saturating_sub(1));
                    }
                } else if self.page == Page::Bookmarks {
                    self.bookmark_selected =
                        (self.bookmark_selected + 1).min(self.bookmarks.len().saturating_sub(1));
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
                self.palette_query.clear();
            }
            Command::ToggleHelp => {
                self.mode = if self.mode == AppMode::Help {
                    AppMode::Normal
                } else {
                    AppMode::Help
                };
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
