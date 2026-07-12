//! Local PDF discovery, hashing, and filesystem watching.

use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use sha2::{Digest, Sha256};
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
    pub content_hash: String,
    /// File size in bytes.
    pub file_size: u64,
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
            .filter_map(|entry| Self::inspect(entry.path()).ok())
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
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .map_or_else(|| "Untitled PDF".to_owned(), humanize_filename);
        Ok(ImportedPdf {
            path: path.to_path_buf(),
            title,
            content_hash: format!("{:x}", hasher.finalize()),
            file_size,
        })
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
    pub fn start<F>(roots: &[PathBuf], mut on_pdf: F) -> Result<Self, LibraryError>
    where
        F: FnMut(PathBuf) + Send + 'static,
    {
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result {
                    for path in event.paths.into_iter().filter(|path| is_pdf(path)) {
                        on_pdf(path);
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
    use std::{fs, path::PathBuf};

    use super::LibraryIndexer;

    #[test]
    fn inspect_hashes_and_humanizes_pdf() -> Result<(), Box<dyn std::error::Error>> {
        let path = std::env::temp_dir().join(format!("papr-index-{}.pdf", std::process::id()));
        fs::write(&path, b"%PDF-1.7\ntest")?;
        let imported = LibraryIndexer::inspect(&path)?;
        assert_eq!(imported.file_size, 13);
        assert!(imported.title.starts_with("papr index"));
        assert_eq!(imported.content_hash.len(), 64);
        fs::remove_file(path)?;
        Ok(())
    }

    #[test]
    fn missing_roots_produce_empty_scan() {
        let roots = [PathBuf::from("/path/that/does/not/exist/papr")];
        assert!(LibraryIndexer::scan(&roots).is_empty());
    }
}
