//! GitHub paper synchronization client and reconciliation logic.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::models::LibraryPaper;

/// Errors produced during GitHub repository synchronization.
#[derive(Debug, Error)]
pub enum SyncError {
    /// HTTP transfer failure.
    #[error("sync network request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Manifest serialization or deserialization failure.
    #[error("sync manifest JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Base64 decoding error from GitHub API content.
    #[error("sync base64 decoding failed: {0}")]
    Base64(#[from] base64::DecodeError),
    /// GitHub API returned a non-success response.
    #[error("GitHub API error ({status}): {message}")]
    Api {
        /// HTTP status code.
        status: u16,
        /// Error message reported by the API.
        message: String,
    },
    /// The configured repository string is invalid.
    #[error("invalid repository name: '{0}'")]
    InvalidRepo(String),
}

/// A lightweight representation of a paper in the sync manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncedPaper {
    /// arXiv identifier if known (e.g. `2301.12345`).
    pub arxiv_id: Option<String>,
    /// Digital Object Identifier if known.
    pub doi: Option<String>,
    /// Canonical paper title.
    pub title: String,
    /// Direct URL to download the PDF if known.
    pub pdf_url: Option<String>,
}

/// Manifest stored in the user's private GitHub repository (`papers.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncManifest {
    /// Schema format version.
    pub version: u32,
    /// ISO 8601 timestamp when this manifest was last updated.
    pub updated_at: String,
    /// Collection of papers tracked by the user.
    pub papers: Vec<SyncedPaper>,
}

impl Default for SyncManifest {
    fn default() -> Self {
        Self {
            version: 1,
            updated_at: Utc::now().to_rfc3339(),
            papers: Vec::new(),
        }
    }
}

/// The outcome of reconciling local library papers against a remote sync manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReconciliation {
    /// Papers present in the remote repository that need to be downloaded locally.
    pub missing_locally: Vec<SyncedPaper>,
    /// Papers present in the local library that need to be added to the remote repository.
    pub new_locally: Vec<SyncedPaper>,
    /// The unified manifest incorporating both local and remote papers.
    pub merged_manifest: SyncManifest,
}

/// Response payload for GitHub repository contents API.
#[derive(Debug, Deserialize)]
struct GitHubContentResponse {
    sha: String,
    content: Option<String>,
}

/// Request payload for GitHub repository creation API.
#[derive(Debug, Serialize)]
struct CreateRepoRequest<'a> {
    name: &'a str,
    private: bool,
    description: &'a str,
    auto_init: bool,
}

/// Request payload for updating or creating a file in a GitHub repository.
#[derive(Debug, Serialize)]
struct PutFileRequest<'a> {
    message: &'a str,
    content: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha: Option<&'a str>,
    branch: &'a str,
}

/// Authenticated user response from `GET /user`.
#[derive(Debug, Deserialize)]
struct GitHubUserResponse {
    login: String,
}

/// Service that coordinates synchronization between Papr and a private GitHub repository.
#[derive(Debug, Clone)]
pub struct GitHubSyncService {
    client: Client,
    repo_name: String,
    token: String,
    branch: String,
}

impl GitHubSyncService {
    /// Create a new sync service.
    ///
    /// # Errors
    /// Returns an error if the HTTP client fails to initialize.
    pub fn new(
        repo_name: String,
        token: String,
        branch: String,
    ) -> Result<Self, SyncError> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            client,
            repo_name,
            token,
            branch,
        })
    }

    /// Resolve the repository owner and repository name.
    ///
    /// If the configured repo already contains `owner/name`, it is split directly.
    /// Otherwise, `GET /user` is called to discover the authenticated user's login.
    async fn resolve_owner_and_repo(&self) -> Result<(String, String), SyncError> {
        let trimmed = self.repo_name.trim();
        if trimmed.is_empty() {
            return Err(SyncError::InvalidRepo(self.repo_name.clone()));
        }

        if let Some((owner, repo)) = trimmed.split_once('/') {
            if owner.is_empty() || repo.is_empty() {
                return Err(SyncError::InvalidRepo(self.repo_name.clone()));
            }
            return Ok((owner.to_string(), repo.to_string()));
        }

        // Fetch username from GitHub API
        let response = self
            .client
            .get("https://api.github.com/user")
            .header("User-Agent", "papr")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response.text().await.unwrap_or_default();
            return Err(SyncError::Api { status, message });
        }

        let user: GitHubUserResponse = serde_json::from_slice(&response.bytes().await?)?;
        Ok((user.login, trimmed.to_string()))
    }

    /// Ensure the repository exists, creating it privately if missing.
    ///
    /// # Errors
    /// Returns an error if the API request fails or permission is denied.
    pub async fn ensure_repository_exists(&self) -> Result<(String, String), SyncError> {
        let (owner, repo) = self.resolve_owner_and_repo().await?;

        let check_url = format!("https://api.github.com/repos/{owner}/{repo}");
        let check_res = self
            .client
            .get(&check_url)
            .header("User-Agent", "papr")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        if check_res.status().is_success() {
            return Ok((owner, repo));
        }

        if check_res.status() == StatusCode::NOT_FOUND {
            // Create the repository as private
            let create_payload = serde_json::to_vec(&CreateRepoRequest {
                name: &repo,
                private: true,
                description: "Papr research library sync",
                auto_init: true,
            })?;

            let create_res = self
                .client
                .post("https://api.github.com/user/repos")
                .header("User-Agent", "papr")
                .header("Authorization", format!("Bearer {}", self.token))
                .header("Accept", "application/vnd.github+json")
                .header("Content-Type", "application/json")
                .body(create_payload)
                .send()
                .await?;

            if !create_res.status().is_success() {
                let status = create_res.status().as_u16();
                let message = create_res.text().await.unwrap_or_default();
                return Err(SyncError::Api { status, message });
            }

            return Ok((owner, repo));
        }

        let status = check_res.status().as_u16();
        let message = check_res.text().await.unwrap_or_default();
        Err(SyncError::Api { status, message })
    }

    /// Fetch the remote manifest from `papers.json`, returning the parsed manifest and the file's Git SHA.
    ///
    /// Returns `Ok(None)` if `papers.json` does not exist yet.
    ///
    /// # Errors
    /// Returns an error if the network transfer or JSON parsing fails.
    pub async fn fetch_manifest(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Option<(SyncManifest, String)>, SyncError> {
        let url = format!(
            "https://api.github.com/repos/{owner}/{repo}/contents/papers.json?ref={}",
            self.branch
        );
        let res = self
            .client
            .get(&url)
            .header("User-Agent", "papr")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        if res.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(SyncError::Api { status, message });
        }

        let content_resp: GitHubContentResponse = serde_json::from_slice(&res.bytes().await?)?;
        let raw_b64 = content_resp.content.unwrap_or_default().replace(['\n', '\r'], "");
        if raw_b64.is_empty() {
            return Ok(Some((SyncManifest::default(), content_resp.sha)));
        }

        let decoded_bytes = BASE64_STANDARD.decode(&raw_b64)?;
        let manifest: SyncManifest = serde_json::from_slice(&decoded_bytes)?;
        Ok(Some((manifest, content_resp.sha)))
    }

    /// Push the updated manifest to `papers.json` in the user's repository.
    ///
    /// # Errors
    /// Returns an error if the update request is rejected.
    pub async fn push_manifest(
        &self,
        owner: &str,
        repo: &str,
        manifest: &SyncManifest,
        previous_sha: Option<&str>,
    ) -> Result<String, SyncError> {
        let url = format!("https://api.github.com/repos/{owner}/{repo}/contents/papers.json");
        let json_bytes = serde_json::to_vec_pretty(manifest)?;
        let b64_content = BASE64_STANDARD.encode(json_bytes);

        let put_req = PutFileRequest {
            message: "papr: sync papers",
            content: &b64_content,
            sha: previous_sha,
            branch: &self.branch,
        };
        let put_payload = serde_json::to_vec(&put_req)?;

        let res = self
            .client
            .put(&url)
            .header("User-Agent", "papr")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("Content-Type", "application/json")
            .body(put_payload)
            .send()
            .await?;

        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(SyncError::Api { status, message });
        }

        let content_resp: GitHubContentResponse = serde_json::from_slice(&res.bytes().await?)?;
        Ok(content_resp.sha)
    }

    /// Reconcile local library papers with a remote manifest.
    ///
    /// Identifies papers missing locally, papers missing remotely, and produces a merged manifest.
    #[must_use]
    pub fn reconcile(
        local_papers: &[LibraryPaper],
        remote_manifest: Option<SyncManifest>,
    ) -> SyncReconciliation {
        let remote_manifest = remote_manifest.unwrap_or_default();
        let mut merged_papers = remote_manifest.papers.clone();
        let mut new_locally = Vec::new();

        // 1. Find local papers not present in remote manifest
        for local in local_papers {
            let already_in_remote = remote_manifest.papers.iter().any(|remote| {
                matches_paper(
                    local.arxiv_id.as_deref(),
                    local.doi.as_deref(),
                    &local.title,
                    remote.arxiv_id.as_deref(),
                    remote.doi.as_deref(),
                    &remote.title,
                )
            });

            if !already_in_remote {
                let synced = SyncedPaper {
                    arxiv_id: local.arxiv_id.clone(),
                    doi: local.doi.clone(),
                    title: local.title.clone(),
                    pdf_url: local.arxiv_id.as_ref().map(|id| format!("https://arxiv.org/pdf/{id}.pdf")),
                };
                new_locally.push(synced.clone());
                merged_papers.push(synced);
            }
        }

        // 2. Find remote papers not present in local library
        let mut missing_locally = Vec::new();
        for remote in &remote_manifest.papers {
            let already_local = local_papers.iter().any(|local| {
                matches_paper(
                    local.arxiv_id.as_deref(),
                    local.doi.as_deref(),
                    &local.title,
                    remote.arxiv_id.as_deref(),
                    remote.doi.as_deref(),
                    &remote.title,
                )
            });

            if !already_local {
                missing_locally.push(remote.clone());
            }
        }

        let merged_manifest = SyncManifest {
            version: 1,
            updated_at: Utc::now().to_rfc3339(),
            papers: merged_papers,
        };

        SyncReconciliation {
            missing_locally,
            new_locally,
            merged_manifest,
        }
    }
}

/// Helper function to match whether two papers represent the same publication.
fn matches_paper(
    a_arxiv: Option<&str>,
    a_doi: Option<&str>,
    a_title: &str,
    b_arxiv: Option<&str>,
    b_doi: Option<&str>,
    b_title: &str,
) -> bool {
    if let (Some(a), Some(b)) = (a_arxiv, b_arxiv)
        && a.trim().eq_ignore_ascii_case(b.trim())
    {
        return true;
    }
    if let (Some(a), Some(b)) = (a_doi, b_doi)
        && a.trim().eq_ignore_ascii_case(b.trim())
    {
        return true;
    }

    let t_a = a_title.trim();
    let t_b = b_title.trim();
    if !t_a.is_empty() && !t_b.is_empty() && t_a.eq_ignore_ascii_case(t_b) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reconciliation_identifies_missing_and_new() {
        let local_paper = LibraryPaper {
            id: 1,
            title: "Attention Is All You Need".to_string(),
            authors: "Vaswani et al.".to_string(),
            doi: None,
            arxiv_id: Some("1706.03762".to_string()),
            pdf_path: Some("/tmp/attention.pdf".to_string()),
            file_size: Some(1024),
            reading_status: "unread".to_string(),
            is_favorite: false,
            abstract_text: None,
            published_at: None,
            downloaded_at: None,
        };

        let remote_paper = SyncedPaper {
            arxiv_id: Some("2301.12345".to_string()),
            doi: None,
            title: "Some Other Deep Learning Paper".to_string(),
            pdf_url: Some("https://arxiv.org/pdf/2301.12345.pdf".to_string()),
        };

        let remote_manifest = SyncManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            papers: vec![remote_paper.clone()],
        };

        let reconciliation = GitHubSyncService::reconcile(&[local_paper], Some(remote_manifest));

        assert_eq!(reconciliation.missing_locally.len(), 1);
        assert_eq!(reconciliation.missing_locally[0], remote_paper);

        assert_eq!(reconciliation.new_locally.len(), 1);
        assert_eq!(reconciliation.new_locally[0].arxiv_id.as_deref(), Some("1706.03762"));

        assert_eq!(reconciliation.merged_manifest.papers.len(), 2);
    }

    #[test]
    fn test_reconciliation_deduplicates_by_arxiv_id() {
        let local_paper = LibraryPaper {
            id: 1,
            title: "Local Title (Slightly modified)".to_string(),
            authors: "".to_string(),
            doi: None,
            arxiv_id: Some("1706.03762".to_string()),
            pdf_path: Some("/tmp/paper.pdf".to_string()),
            file_size: Some(1024),
            reading_status: "unread".to_string(),
            is_favorite: false,
            abstract_text: None,
            published_at: None,
            downloaded_at: None,
        };

        let remote_paper = SyncedPaper {
            arxiv_id: Some("1706.03762".to_string()),
            doi: None,
            title: "Attention Is All You Need".to_string(),
            pdf_url: None,
        };

        let remote_manifest = SyncManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            papers: vec![remote_paper],
        };

        let reconciliation = GitHubSyncService::reconcile(&[local_paper], Some(remote_manifest));

        assert!(reconciliation.missing_locally.is_empty());
        assert!(reconciliation.new_locally.is_empty());
        assert_eq!(reconciliation.merged_manifest.papers.len(), 1);
    }

    #[test]
    fn test_manifest_roundtrip_serialization() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = SyncManifest {
            version: 1,
            updated_at: "2026-11-05T12:00:00Z".to_string(),
            papers: vec![SyncedPaper {
                arxiv_id: Some("2301.12345".to_string()),
                doi: Some("10.1145/12345".to_string()),
                title: "Test Paper".to_string(),
                pdf_url: Some("https://arxiv.org/pdf/2301.12345.pdf".to_string()),
            }],
        };

        let json = serde_json::to_string(&manifest)?;
        let parsed: SyncManifest = serde_json::from_str(&json)?;
        assert_eq!(parsed, manifest);
        Ok(())
    }
}
