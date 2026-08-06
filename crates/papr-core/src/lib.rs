//! Core domain and infrastructure for the `papr` research workspace.

pub mod api;
pub mod config;
pub mod completions;
pub mod database;
pub mod downloads;
pub mod library;
pub mod models;
pub mod plugins;
pub mod paths;
pub mod projects;
pub mod dashboard;
pub mod terminal;
pub mod editor;

pub use api::arxiv::ArxivClient;
pub use downloads::{DownloadEvent, DownloadManager, DownloadStatus, DownloadTask};
pub use projects::{ProjectBuildDiagnostic, ProjectDiagnosticSeverity};
pub use config::{Config, Paths};
pub use completions::{CitationEntry, CitationSource, CompletionItem, CompletionSource};
pub use database::Database;

pub use library::{CollectionDirectory, ImportedPdf, LibraryIndexer, LibraryWatcher};
pub use models::{
    ActivityItem, BookmarkSummary, CollectionSummary, LibraryPaper, PaperNote, ReadingDay,
    ReadingStatistics, RemotePaper, ResearchDashboard,
};
pub use plugins::{
    PluginAction, PluginCapability, PluginDiagnostic, PluginHost, PluginInfo, PluginManifest,
    PluginRequest, PluginResponse,
};
pub use paths::{canonicalize_path, move_pdf_file, sanitize_download_filename_component, validate_collection_name, InvalidCollectionName, get_pdf_page_count};
pub use projects::{Project, ProjectError, ProjectManager, parse_latex_diagnostics, parse_typst_diagnostics, parse_project_diagnostics};
pub use editor::{
    cursor_visual_position, config_editor_wrap_rows, config_editor_line_start,
    config_editor_line_end, prev_word_boundary, next_word_boundary,
    expand_tabs_for_editor_view, project_editor_line_at, prev_char_boundary, next_char_boundary, byte_index_for_char_column, cursor_from_visual_position,
};
pub use dashboard::*;
pub use terminal::*;
