//! Streaming PDF download manager.
use crate::RemotePaper;

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use reqwest::Client;
use thiserror::Error;
use tokio::{fs, io::AsyncWriteExt, sync::mpsc};

/// Progress events emitted by a PDF download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Transfer began and may have a known content length.
    Started {
        /// Provider paper identifier.
        id: String,
        /// Expected response bytes.
        total: Option<u64>,
    },
    /// More response bytes were persisted.
    Progress {
        /// Provider paper identifier.
        id: String,
        /// Bytes persisted so far.
        downloaded: u64,
        /// Expected response bytes.
        total: Option<u64>,
    },
    /// The temporary file was atomically promoted.
    Completed {
        /// Provider paper identifier.
        id: String,
        /// Final PDF path.
        path: PathBuf,
    },
    /// The transfer could not complete.
    Failed {
        /// Provider paper identifier.
        id: String,
        /// Human-readable failure.
        error: String,
    },
}

/// PDF transfer errors.
#[derive(Debug, Error)]
pub enum DownloadError {
    /// Network transfer failed.
    #[error("download request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// Destination file operation failed.
    #[error("download file operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Reusable asynchronous PDF downloader.
#[derive(Debug, Clone)]
pub struct DownloadManager {
    client: Client,
}

impl DownloadManager {
    /// Create a downloader with a bounded transfer timeout.
    ///
    /// # Errors
    ///
    /// Returns an error when the HTTP client cannot be configured.
    pub fn new() -> Result<Self, DownloadError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(concat!("papr/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(300))
                .build()?,
        })
    }

    /// Stream a URL to a partial file and atomically finalize it.
    ///
    /// # Errors
    ///
    /// Returns an error for HTTP failures or destination file errors.
    pub async fn download(
        &self,
        id: &str,
        url: &str,
        destination: &Path,
        events: &mpsc::UnboundedSender<DownloadEvent>,
    ) -> Result<(), DownloadError> {
        let temp_destination = destination.with_extension("pdf.part");
        if let Some(parent) = temp_destination.parent() {
            fs::create_dir_all(parent).await?;
        }
        let response = self.client.get(url).send().await?.error_for_status()?;
        let total = response.content_length();
        let _ = events.send(DownloadEvent::Started {
            id: id.to_owned(),
            total,
        });
        let mut file = fs::File::create(&temp_destination).await?;
        let mut downloaded = 0_u64;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded = downloaded.saturating_add(chunk.len() as u64);
            let _ = events.send(DownloadEvent::Progress {
                id: id.to_owned(),
                downloaded,
                total,
            });
        }
        file.flush().await?;
        drop(file);
        let _ = events.send(DownloadEvent::Completed {
            id: id.to_owned(),
            path: temp_destination,
        });
        Ok(())
    }
}

/// State of one visible background transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStatus {
    /// Waiting to receive response bytes.
    Starting,
    /// Bytes are actively streaming.
    Running,
    /// Extracting metadata from PDF (inspection, etc.)
    ExtractingMetadata,
    /// Fetching paper metadata from online API (arXiv/Crossref)
    Enriching,
    /// Renaming target PDF file based on title/metadata
    Renaming,
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
    /// Associated database paper ID, if attached.
    pub paper_id: Option<i64>,
    /// Final or current PDF path on disk.
    pub pdf_path: Option<String>,
    /// Current transfer state.
    pub status: DownloadStatus,
    /// Remote paper metadata preserved for retries.
    pub remote_paper: Option<RemotePaper>,
    /// When the task failed (used to auto-cleanup older failures).
    pub failed_at: Option<std::time::Instant>,
}

impl DownloadTask {
    /// Display filename without the PDF extension, falling back to the remote paper title.
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.pdf_path
            .as_deref()
            .and_then(|path| std::path::Path::new(path).file_stem())
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| self.title.strip_suffix(".pdf").unwrap_or(&self.title))
    }
}
