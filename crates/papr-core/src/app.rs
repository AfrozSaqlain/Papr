//! Application state machine and navigation commands.

use crate::models::{
    BookmarkSummary, CollectionSummary, DashboardStats, LibraryPaper, PaperNote, RemotePaper,
    ResearchDashboard, TagSummary,
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
    /// Paper taxonomy.
    Tags,
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
    pub const ALL: [Self; 14] = [
        Self::Dashboard,
        Self::Discover,
        Self::Library,
        Self::ReadingQueue,
        Self::Collections,
        Self::Bookmarks,
        Self::Authors,
        Self::Tags,
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
            Self::Tags => "Tags",
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
    /// Single-line tag or collection input.
    Prompt,
}

/// Purpose of the active single-line metadata prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    /// Assign a tag.
    Tag,
    /// Add to a collection.
    Collection,
}

/// Active metadata input prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataPrompt {
    /// Target paper identifier.
    pub paper_id: i64,
    /// Input purpose.
    pub kind: PromptKind,
    /// Editable input value.
    pub value: String,
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
    /// Return to the dashboard.
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
    /// Currently edited note.
    pub note_editor: Option<PaperNote>,
    /// Whether the note overlay shows rendered Markdown instead of source.
    pub note_preview: bool,
    /// Active tag or collection prompt.
    pub metadata_prompt: Option<MetadataPrompt>,
    /// Collection summaries.
    pub collections: Vec<CollectionSummary>,
    /// Tag summaries.
    pub tags: Vec<TagSummary>,
    /// Bookmark summaries.
    pub bookmarks: Vec<BookmarkSummary>,
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
            mode: AppMode::Normal,
            should_quit: false,
            stats: DashboardStats::default(),
            dashboard: ResearchDashboard::default(),
            today_papers: Vec::new(),
            today_status: DiscoveryStatus::Idle,
            palette_query: String::new(),
            discovery: DiscoveryState::default(),
            library: LibraryState::default(),
            downloads: Vec::new(),
            download_selected: 0,
            note_editor: None,
            note_preview: false,
            metadata_prompt: None,
            collections: Vec::new(),
            tags: Vec::new(),
            bookmarks: Vec::new(),
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
                if self.page == Page::Discover {
                    self.discovery.selected = self.discovery.selected.saturating_sub(1);
                } else if self.page == Page::Library {
                    self.library.selected = self.library.selected.saturating_sub(1);
                } else if self.page == Page::Downloads {
                    self.download_selected = self.download_selected.saturating_sub(1);
                } else {
                    self.sidebar_index = self.sidebar_index.saturating_sub(1);
                }
            }
            Command::MoveDown => {
                if self.page == Page::Discover {
                    self.discovery.selected = (self.discovery.selected + 1)
                        .min(self.discovery.results.len().saturating_sub(1));
                } else if self.page == Page::Library {
                    self.library.selected = (self.library.selected + 1)
                        .min(self.library.papers.len().saturating_sub(1));
                } else if self.page == Page::Downloads {
                    self.download_selected =
                        (self.download_selected + 1).min(self.downloads.len().saturating_sub(1));
                } else {
                    self.sidebar_index = (self.sidebar_index + 1).min(Page::ALL.len() - 1);
                }
            }
            Command::Open => {
                if self.page == Page::Discover && !self.discovery.results.is_empty() {
                    self.mode = AppMode::PaperDetail;
                    self.discovery.detail_scroll = 0;
                } else {
                    self.page = Page::ALL[self.sidebar_index];
                }
            }
            Command::Back => {
                if self.mode == AppMode::PaperDetail {
                    self.mode = AppMode::Normal;
                } else if self.page != Page::Dashboard {
                    self.page = Page::Dashboard;
                    self.sidebar_index = 0;
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
