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
}

/// Tag with its current paper count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSummary {
    /// Stable tag identifier.
    pub id: i64,
    /// Unique display name.
    pub name: String,
    /// Number of tagged papers.
    pub paper_count: u64,
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
