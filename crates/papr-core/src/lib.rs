//! Core domain and infrastructure for the `papr` research workspace.

pub mod api;
pub mod collab;
pub mod completions;
pub mod config;
pub mod dashboard;
pub mod database;
pub mod downloads;
pub mod enrichment;
pub mod library;
pub mod models;
pub mod paths;
pub mod plugins;
pub mod projects;
pub mod sync;
pub mod typst;

pub use api::arxiv::{ArxivClient, ArxivRetryPolicy, RankedCandidatePageRequest, parse_arxiv_id};
pub use collab::{
    BundledProjectFile, CollabError, LOCAL_COLLAB_FILE, LOCAL_PROJECT_KEY_FILE, ProjectArchive,
    ProjectKey, ProjectKeyEnvelope, SshEd25519KeyPair, SshEd25519PublicKey, WrappedProjectKey,
    decrypt_project_bundle, discover_or_create_local_keys, encrypt_project_directory,
    ensure_local_project_collab, load_local_envelope, save_local_envelope, unpack_project_archive,
    validate_github_username,
};
pub use completions::{CitationEntry, CitationSource, CompletionItem, CompletionSource};
pub use config::{Config, Paths};
pub use dashboard::{
    DashboardFeedError, DashboardService, select_dashboard_papers, shuffle_daily_bucket,
};
pub use database::Database;
pub use downloads::{DownloadEvent, DownloadManager};
pub use enrichment::{MetadataCandidate, MetadataEnrichmentOutcome, MetadataEnrichmentService};
pub use library::{
    CollectionDirectory, ImportedPdf, IngestedPdf, LibraryIndexer, LibraryIngestionResult,
    LibraryIngestionService, LibraryWatcher,
};
pub use models::{
    ActivityItem, BookmarkSummary, CollectionSummary, LibraryPaper, PaperNote, ReadingDay,
    ReadingStatistics, RemotePaper, ResearchDashboard,
};
pub use paths::{
    InvalidCollectionName, canonicalize_path, get_pdf_page_count, move_pdf_file,
    sanitize_download_filename_component, validate_collection_name,
};
pub use sync::{
    GitHubSyncService, ProjectSyncSummary, SyncError, SyncManifest, SyncReconciliation, SyncedPaper,
};
pub use plugins::{
    PluginAction, PluginCapability, PluginDiagnostic, PluginHost, PluginInfo, PluginManifest,
    PluginRequest, PluginResponse,
};
pub use projects::{
    LatexBuildEvent, LatexBuildProcess, LatexBuildSignal, Project, ProjectBuildDiagnostic,
    ProjectDiagnosticSeverity, ProjectError, ProjectManager, classify_latexmk_line,
    parse_latex_diagnostics,
};
pub use typst::{TypstCompileResult, TypstCompiler};
