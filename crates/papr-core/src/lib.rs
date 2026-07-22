//! Core domain and infrastructure for the `papr` research workspace.

pub mod api;
pub mod app;
pub mod config;
pub mod completions;
pub mod database;
pub mod downloads;
pub mod library;
pub mod models;
pub mod plugins;
pub mod projects;
pub mod theme;

pub use api::arxiv::ArxivClient;
pub use app::{
    App, AppMode, CollectionSearchItem, Command, DeletionTarget, DiscoveryState, DiscoveryStatus, DownloadStatus, DownloadTask,
    LibraryState, MetadataPrompt, Page, ProjectPane,
};
pub use config::{Config, Paths};
pub use completions::{CitationEntry, CitationSource, CompletionItem, CompletionSource};
pub use database::Database;
pub use downloads::{DownloadEvent, DownloadManager};
pub use library::{CollectionDirectory, ImportedPdf, LibraryIndexer, LibraryWatcher};
pub use models::{
    ActivityItem, BookmarkSummary, CollectionSummary, LibraryPaper, PaperNote, ReadingDay,
    ReadingStatistics, RemotePaper, ResearchDashboard,
};
pub use plugins::{
    PluginAction, PluginCapability, PluginDiagnostic, PluginHost, PluginInfo, PluginManifest,
    PluginRequest, PluginResponse,
};
pub use projects::{Project, ProjectError, ProjectManager};
pub use theme::Theme;
