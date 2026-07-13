//! Local PDF discovery, hashing, and filesystem watching.

use std::{
    fs::File,
    io::{self},
    path::{Path, PathBuf},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use thiserror::Error;
use walkdir::WalkDir;

/// Metadata collected from a local PDF before database ingestion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedPdf {
    /// Absolute or configured source path.
    pub path: PathBuf,
    /// Human-readable title inferred from the filename.
    pub title: String,
    /// SHA-256 content digest used for duplicate detection.
    
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
        roots
            .iter()
            .flat_map(|root| WalkDir::new(root).follow_links(false).into_iter())
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_pdf(entry.path()))
            .filter_map(|entry| Self::inspect_in_roots(entry.path(), roots).ok())
            .collect()
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
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();
        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .map_or_else(|| "Untitled PDF".to_owned(), humanize_filename);
        Ok(ImportedPdf {
            path: path.to_path_buf(),
            title,
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
        if let Some(root) = roots
            .iter()
            .filter(|root| path.starts_with(root))
            .max_by_key(|root| root.components().count())
        {
            pdf.library_root = Some(root.clone());
            pdf.relative_directory = path
                .parent()
                .and_then(|parent| parent.strip_prefix(root).ok())
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
                if result.is_ok() {
                    on_event();
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
}
