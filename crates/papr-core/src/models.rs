//! Shared domain models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A paper returned by a remote discovery provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePaper {
    /// Provider-specific stable identifier.
    pub id: String,
    /// Canonical paper title.
    pub title: String,
    /// Author names in publication order.
    pub authors: Vec<String>,
    /// Full abstract when provided.
    pub abstract_text: String,
    /// Original publication timestamp.
    pub published: DateTime<Utc>,
    /// Most recent revision timestamp.
    pub updated: DateTime<Utc>,
    /// Subject categories assigned by the provider.
    pub categories: Vec<String>,
    /// Direct PDF location.
    pub pdf_url: Option<String>,
    /// DOI when linked by the provider.
    pub doi: Option<String>,
    /// Journal reference when available.
    pub journal_ref: Option<String>,
}

impl RemotePaper {
    /// Return a compact comma-separated author line.
    #[must_use]
    pub fn author_line(&self) -> String {
        self.authors.join(", ")
    }
}

/// A paper stored in the local catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Paper {
    /// Stable database identifier.
    pub id: i64,
    /// Canonical title.
    pub title: String,
    /// Optional digital object identifier.
    pub doi: Option<String>,
    /// When the item was added to the library.
    pub created_at: DateTime<Utc>,
}

/// Counts displayed on the dashboard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DashboardStats {
    /// Total catalog entries.
    pub papers: u64,
    /// Papers waiting in the reading queue.
    pub queued: u64,
    /// Locally available PDF files.
    pub downloaded: u64,
    /// Papers marked as favorites.
    pub favorites: u64,
}

/// A paper row shown in the local library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryPaper {
    /// Stable database identifier.
    pub id: i64,
    /// Display title.
    pub title: String,
    /// Comma-separated authors when known.
    pub authors: String,
    /// Digital object identifier when known.
    pub doi: Option<String>,
    /// Local PDF path when downloaded or imported.
    pub pdf_path: Option<String>,
    /// PDF size in bytes.
    pub file_size: Option<u64>,
    /// Current reading state.
    pub reading_status: String,
    /// Whether the paper is a favorite.
    pub is_favorite: bool,
}

/// A Markdown note associated with one paper.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PaperNote {
    /// Parent paper identifier.
    pub paper_id: i64,
    /// Note heading.
    pub title: String,
    /// Markdown source.
    pub body: String,
}

/// Named paper collection with its current item count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionSummary {
    /// Stable collection identifier.
    pub id: i64,
    /// Unique display name.
    pub name: String,
    /// Number of assigned papers.
    pub paper_count: u64,
    /// Backing directory for filesystem collections.
    pub folder_path: Option<String>,
}

/// A bookmarked paper or position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkSummary {
    /// Stable bookmark identifier.
    pub id: i64,
    /// Parent paper identifier.
    pub paper_id: i64,
    /// Paper title.
    pub paper_title: String,
    /// Optional bookmarked PDF page.
    pub page: Option<u32>,
    /// Optional user label.
    pub label: Option<String>,
}

/// One human-readable research activity event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityItem {
    /// Event type stored in the activity log.
    pub kind: String,
    /// Related paper title or event detail.
    pub label: String,
    /// UTC event timestamp.
    pub occurred_at: DateTime<Utc>,
}

/// One day in the reading heatmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadingDay {
    /// Calendar date formatted as `YYYY-MM-DD`.
    pub date: String,
    /// Papers opened on that date.
    pub count: u64,
}

/// Aggregated research and reading statistics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReadingStatistics {
    /// Consecutive reading days ending today or yesterday.
    pub current_streak: u64,
    /// Papers opened during the current calendar month.
    pub monthly_reading: u64,
    /// Papers opened during the current calendar year.
    pub yearly_reading: u64,
    /// Total recorded reading sessions.
    pub sessions: u64,
    /// Average recorded session duration in seconds.
    pub average_reading_seconds: u64,
    /// Weekday with the most reading sessions.
    pub most_active_day: Option<String>,
    /// Most frequently opened author.
    pub most_read_author: Option<String>,
    /// Most frequently opened journal.
    pub most_read_journal: Option<String>,
    /// Recent daily activity used by the heatmap.
    pub heatmap: Vec<ReadingDay>,
}

/// Data displayed on the research dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResearchDashboard {
    /// Standard library counters.
    pub counts: DashboardStats,
    /// Papers that have not been marked read.
    pub unread: u64,
    /// Number of user collections.
    pub collections: u64,
    /// Total known local PDF bytes.
    pub disk_usage: u64,
    /// Approximate live `SQLite` size in bytes.
    pub database_size: u64,
    /// Current reading analytics.
    pub reading: ReadingStatistics,
    /// Most recent user activity.
    pub recent_activity: Vec<ActivityItem>,
}
