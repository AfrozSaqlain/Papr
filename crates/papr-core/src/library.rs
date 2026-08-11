//! Local PDF discovery, hashing, and filesystem watching.

use std::{
    fs::File,
    io::{self},
    path::{Path, PathBuf},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use thiserror::Error;
use walkdir::WalkDir;

use crate::database::{Database, DatabaseError};

/// Metadata collected from a local PDF before database ingestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedPdf {
    /// Absolute or configured source path.
    pub path: PathBuf,
    /// Human-readable title inferred from the filename.
    pub title: String,
    /// SHA-256 content digest used for duplicate detection.
    pub content_hash: String,
    /// File size in bytes.
    pub file_size: u64,
    /// Configured library root containing this file.
    pub library_root: Option<PathBuf>,
    /// Directory relative to the library root, excluding the filename.
    pub relative_directory: Option<PathBuf>,
}

/// A subdirectory discovered beneath a configured library root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionDirectory {
    /// Configured library root containing the directory.
    pub library_root: PathBuf,
    /// Directory path relative to the library root.
    pub relative_path: PathBuf,
}

/// One paper processed by a library ingestion workflow.
#[derive(Debug, Clone)]
pub struct IngestedPdf {
    /// Inspected filesystem metadata.
    pub pdf: ImportedPdf,
    /// Resolved database identifier.
    pub paper_id: Option<i64>,
    /// Whether this ingestion created a new database paper.
    pub newly_imported: bool,
}

/// Result of reconciling one complete filesystem scan.
#[derive(Debug, Clone)]
pub struct LibraryIngestionResult {
    /// Number of PDFs found by the scan.
    pub found: usize,
    /// Per-paper persistence results.
    pub papers: Vec<IngestedPdf>,
}

/// Coordinates filesystem scan results with persistent library organization.
pub struct LibraryIngestionService<'a> {
    database: &'a Database,
    collection_roots: &'a [PathBuf],
}

impl<'a> LibraryIngestionService<'a> {
    /// Bind ingestion to one database and set of collection roots.
    #[must_use]
    pub const fn new(database: &'a Database, collection_roots: &'a [PathBuf]) -> Self {
        Self {
            database,
            collection_roots,
        }
    }

    /// Persist and reconcile one complete filesystem scan.
    ///
    /// # Errors
    ///
    /// Returns an error when a collection or paper cannot be persisted or
    /// reconciled with the database.
    pub fn ingest_scan(
        &self,
        pdfs: Vec<ImportedPdf>,
        directories: &[CollectionDirectory],
    ) -> Result<LibraryIngestionResult, DatabaseError> {
        for directory in directories {
            self.database.sync_collection_directory(directory)?;
        }
        let found = pdfs.len();
        let mut papers = Vec::with_capacity(found);
        for pdf in pdfs {
            papers.push(self.ingest_pdf(pdf)?);
        }
        self.database
            .reconcile_collections(self.collection_roots, directories)?;
        Ok(LibraryIngestionResult { found, papers })
    }

    /// Persist and classify one inspected PDF.
    ///
    /// # Errors
    ///
    /// Returns an error when the paper cannot be imported, resolved, or assigned
    /// to its filesystem collection.
    pub fn ingest_pdf(&self, pdf: ImportedPdf) -> Result<IngestedPdf, DatabaseError> {
        let newly_imported = self.database.import_pdf(&pdf)?;
        let paper_id = self.database.paper_id_for_pdf(&pdf)?;
        if let Some(paper_id) = paper_id {
            sync_pdf_collection_membership(self.database, paper_id, &pdf, self.collection_roots)?;
        }
        Ok(IngestedPdf {
            pdf,
            paper_id,
            newly_imported,
        })
    }

    /// Reconcile collection membership for a paper persisted by another workflow.
    ///
    /// # Errors
    ///
    /// Returns an error when collection membership cannot be updated.
    pub fn reconcile_paper(&self, paper_id: i64, pdf: &ImportedPdf) -> Result<(), DatabaseError> {
        sync_pdf_collection_membership(self.database, paper_id, pdf, self.collection_roots)
    }
}

fn sync_pdf_collection_membership(
    database: &Database,
    paper_id: i64,
    pdf: &ImportedPdf,
    roots: &[PathBuf],
) -> Result<(), DatabaseError> {
    let Some(root) = roots
        .iter()
        .filter(|root| pdf.path.starts_with(root))
        .max_by_key(|root| root.components().count())
    else {
        database.clear_collection_membership(paper_id)?;
        return Ok(());
    };
    let mut classified = pdf.clone();
    classified.library_root = Some(root.clone());
    classified.relative_directory = pdf
        .path
        .parent()
        .and_then(|parent| parent.strip_prefix(root).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(Path::to_path_buf);
    database.sync_pdf_collection(paper_id, &classified)?;
    Ok(())
}

/// Local library indexing errors.
#[derive(Debug, Error)]
pub enum LibraryError {
    /// Reading a PDF failed.
    #[error("could not read PDF: {0}")]
    Io(#[from] io::Error),
    /// Starting or configuring a filesystem watcher failed.
    #[error("filesystem watcher failed: {0}")]
    Notify(#[from] notify::Error),
}

/// Stateless recursive PDF indexer.
#[derive(Debug, Default, Clone, Copy)]
pub struct LibraryIndexer;

impl LibraryIndexer {
    /// Recursively scan configured roots and collect readable PDF metadata.
    #[must_use]
    pub fn scan(roots: &[PathBuf]) -> Vec<ImportedPdf> {
        let mut seen = std::collections::HashSet::new();
        roots
            .iter()
            .flat_map(|root| WalkDir::new(root).follow_links(false).into_iter())
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_pdf(entry.path()))
            .filter(|entry| {
                let p = std::fs::canonicalize(entry.path())
                    .unwrap_or_else(|_| entry.path().to_path_buf());
                seen.insert(p)
            })
            .filter_map(|entry| Self::inspect_in_roots(entry.path(), roots).ok())
            .collect()
    }

    /// Recursively count readable PDF files in configured roots.
    #[must_use]
    pub fn count_pdfs(roots: &[PathBuf]) -> u64 {
        let mut seen = std::collections::HashSet::new();
        roots
            .iter()
            .flat_map(|root| WalkDir::new(root).follow_links(false).into_iter())
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_pdf(entry.path()))
            .filter(|entry| {
                let p = std::fs::canonicalize(entry.path())
                    .unwrap_or_else(|_| entry.path().to_path_buf());
                seen.insert(p)
            })
            .count() as u64
    }

    /// Recursively calculate total size of readable PDF files in configured roots.
    #[must_use]
    pub fn pdf_storage_size(roots: &[PathBuf]) -> u64 {
        let mut seen = std::collections::HashSet::new();
        roots
            .iter()
            .flat_map(|root| WalkDir::new(root).follow_links(false).into_iter())
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_pdf(entry.path()))
            .filter(|entry| {
                let p = std::fs::canonicalize(entry.path())
                    .unwrap_or_else(|_| entry.path().to_path_buf());
                seen.insert(p)
            })
            .map(|entry| entry.metadata().map(|m| m.len()).unwrap_or(0))
            .sum()
    }

    /// Recursively count PDF files and calculate their total size in one pass.
    ///
    /// This preserves the count and deduplication semantics of [`Self::count_pdfs`]
    /// while avoiding a second traversal when both values are needed together.
    #[must_use]
    pub fn pdf_storage_stats(roots: &[PathBuf]) -> (u64, u64) {
        let mut seen = std::collections::HashSet::new();
        roots
            .iter()
            .flat_map(|root| WalkDir::new(root).follow_links(false).into_iter())
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_pdf(entry.path()))
            .filter(|entry| {
                let p = std::fs::canonicalize(entry.path())
                    .unwrap_or_else(|_| entry.path().to_path_buf());
                seen.insert(p)
            })
            .fold((0_u64, 0_u64), |(count, bytes), entry| {
                (
                    count + 1,
                    bytes + entry.metadata().map(|metadata| metadata.len()).unwrap_or(0),
                )
            })
    }

    /// Discover every subdirectory represented as a filesystem collection.
    #[must_use]
    pub fn collection_directories(roots: &[PathBuf]) -> Vec<CollectionDirectory> {
        roots
            .iter()
            .flat_map(|root| {
                WalkDir::new(root)
                    .min_depth(1)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(Result::ok)
                    .filter(|entry| entry.file_type().is_dir())
                    .filter_map(|entry| {
                        entry
                            .path()
                            .strip_prefix(root)
                            .ok()
                            .map(|relative| CollectionDirectory {
                                library_root: root.clone(),
                                relative_path: relative.to_path_buf(),
                            })
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Hash and describe one PDF.
    ///
    /// # Errors
    ///
    /// Returns an error when the file metadata or contents cannot be read.
    pub fn inspect(path: &Path) -> Result<ImportedPdf, LibraryError> {
        let mut file = File::open(path)?;
        let file_size = file.metadata()?.len();
        let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .map_or_else(|| "Untitled PDF".to_owned(), humanize_filename);

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        let content_hash = format!("{:x}", hasher.finalize());

        Ok(ImportedPdf {
            path: canonical_path,
            title,
            content_hash,
            file_size,
            library_root: None,
            relative_directory: None,
        })
    }

    /// Inspect a PDF and classify it relative to the most specific library root.
    ///
    /// # Errors
    /// Returns an error when the PDF cannot be inspected.
    pub fn inspect_in_roots(path: &Path, roots: &[PathBuf]) -> Result<ImportedPdf, LibraryError> {
        let mut pdf = Self::inspect(path)?;
        let canonical_roots: Vec<PathBuf> = roots
            .iter()
            .map(|r| std::fs::canonicalize(r).unwrap_or_else(|_| r.clone()))
            .collect();
        if let Some((root, canonical_root)) = roots
            .iter()
            .zip(canonical_roots.iter())
            .filter(|(_, canonical_root)| pdf.path.starts_with(canonical_root))
            .max_by_key(|(_, canonical_root)| canonical_root.components().count())
        {
            pdf.library_root = Some(root.clone());
            pdf.relative_directory = pdf
                .path
                .parent()
                .and_then(|parent| parent.strip_prefix(canonical_root).ok())
                .filter(|relative| !relative.as_os_str().is_empty())
                .map(Path::to_path_buf);
        }
        Ok(pdf)
    }
}

/// Keeps a native filesystem watcher alive for configured library roots.
pub struct LibraryWatcher {
    _watcher: RecommendedWatcher,
}

impl LibraryWatcher {
    /// Watch roots recursively and invoke a callback for changed PDF paths.
    ///
    /// # Errors
    ///
    /// Returns an error if the platform watcher cannot start or watch a root.
    pub fn start<F>(roots: &[PathBuf], mut on_event: F) -> Result<Self, LibraryError>
    where
        F: FnMut() + Send + 'static,
    {
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    if !matches!(event.kind, notify::EventKind::Access(_)) {
                        on_event();
                    }
                }
            })?;
        for root in roots.iter().filter(|root| root.exists()) {
            watcher.watch(root, RecursiveMode::Recursive)?;
        }
        Ok(Self { _watcher: watcher })
    }
}

fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn humanize_filename(name: &str) -> String {
    name.replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::LibraryIndexer;

    #[test]
    fn inspect_hashes_and_humanizes_pdf() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("papr-index-{}.pdf", std::process::id()));
        fs::write(&path, b"%PDF-1.7\ntest")?;
        let imported = LibraryIndexer::inspect(&path)?;
        assert_eq!(imported.file_size, 13);
        assert!(imported.title.starts_with("papr index"));
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn missing_roots_produce_empty_scan() {
        let roots = [PathBuf::from("/path/that/does/not/exist/papr")];
        assert!(LibraryIndexer::scan(&roots).is_empty());
    }

    #[test]
    fn classifies_root_and_nested_pdfs() -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "papr-library-layout-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let collection = root.join("Gravitational Waves");
        fs::create_dir_all(&collection)?;
        let root_pdf = root.join("unassigned.pdf");
        let nested_pdf = collection.join("assigned.pdf");
        fs::create_dir_all(root.join("Empty Collection"))?;
        fs::write(&root_pdf, b"%PDF root")?;
        fs::write(&nested_pdf, b"%PDF nested")?;

        let root_item = LibraryIndexer::inspect_in_roots(&root_pdf, std::slice::from_ref(&root))?;
        let nested_item =
            LibraryIndexer::inspect_in_roots(&nested_pdf, std::slice::from_ref(&root))?;
        assert_eq!(root_item.library_root.as_deref(), Some(root.as_path()));
        assert_eq!(root_item.relative_directory, None);
        assert_eq!(
            nested_item.relative_directory,
            Some(PathBuf::from("Gravitational Waves"))
        );
        let directories = LibraryIndexer::collection_directories(std::slice::from_ref(&root));
        assert!(
            directories
                .iter()
                .any(|directory| { directory.relative_path == Path::new("Empty Collection") })
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn nested_roots_do_not_double_count() -> Result<(), Box<dyn std::error::Error>> {
        let root =
            std::env::temp_dir().join(format!("papr-nested-roots-test-{}", std::process::id()));
        let sub = root.join("downloads");
        fs::create_dir_all(&sub)?;
        let pdf = sub.join("paper.pdf");
        fs::write(&pdf, b"%PDF-1.4 paper")?;

        let roots = vec![root.clone(), sub.clone()];
        let scanned = LibraryIndexer::scan(&roots);
        assert_eq!(scanned.len(), 1);

        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn storage_stats_match_individual_count_and_size_walks()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!(
            "papr-storage-stats-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let nested = root.join("nested");
        fs::create_dir_all(&nested)?;
        fs::write(root.join("first.pdf"), b"%PDF first")?;
        fs::write(nested.join("second.pdf"), b"%PDF second")?;

        let roots = vec![root.clone(), nested];
        assert_eq!(
            LibraryIndexer::pdf_storage_stats(&roots),
            (
                LibraryIndexer::count_pdfs(&roots),
                LibraryIndexer::pdf_storage_size(&roots),
            )
        );

        fs::remove_dir_all(root)?;
        Ok(())
    }
}
