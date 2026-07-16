//! Streaming PDF download manager.

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
