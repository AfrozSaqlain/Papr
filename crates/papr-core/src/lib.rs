//! Core domain and infrastructure for the `papr` research workspace.

pub mod api;
pub mod app;
pub mod config;
pub mod database;
pub mod downloads;
pub mod library;
pub mod models;
pub mod theme;

pub use api::arxiv::ArxivClient;
pub use app::{
    App, AppMode, Command, DiscoveryStatus, DownloadStatus, DownloadTask, LibraryState,
    MetadataPrompt, Page, PromptKind,
};
pub use config::{Config, Paths};
pub use database::Database;
pub use downloads::{DownloadEvent, DownloadManager};
pub use library::{ImportedPdf, LibraryIndexer, LibraryWatcher};
pub use models::{
    BookmarkSummary, CollectionSummary, LibraryPaper, PaperNote, RemotePaper, TagSummary,
};
pub use theme::Theme;
