//! GitHub paper synchronization client and reconciliation logic.

use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64_STANDARD};
use chrono::Utc;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    collab::{
        CollabError, ProjectKeyEnvelope, SshEd25519KeyPair, SshEd25519PublicKey,
        decrypt_project_bundle, encrypt_project_directory, ensure_local_project_collab,
        load_local_envelope, save_local_envelope, unpack_project_archive,
        validate_github_username,
    },
    models::LibraryPaper,
};

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
    /// Project encryption or collaboration error.
    #[error("{0}")]
    Collab(#[from] CollabError),
}

/// Summary of a collaborative project synchronization pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSyncSummary {
    /// Names of encrypted projects pulled and decrypted locally.
    pub downloaded: Vec<String>,
    /// Names of local projects encrypted and pushed to the repository.
    pub uploaded: Vec<String>,
    /// Names of remote projects skipped because the current user is not an authorized collaborator.
    pub skipped_unauthorized: Vec<String>,
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

/// Response payload for GitHub create or update file contents API (PUT).
#[derive(Debug, Deserialize)]
struct GitHubPutFileResponse {
    content: Option<GitHubPutFileContent>,
    commit: Option<GitHubPutFileCommit>,
}

#[derive(Debug, Deserialize)]
struct GitHubPutFileContent {
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GitHubPutFileCommit {
    sha: String,
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

#[derive(Debug, Deserialize)]
struct GitHubSshKeyObject {
    key: String,
}

#[derive(Debug, Deserialize)]
struct GitHubDirEntry {
    name: String,
    #[serde(rename = "type")]
    entry_type: String,
}

#[derive(Debug, Serialize)]
struct InviteCollaboratorRequest<'a> {
    permission: &'a str,
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

        let put_resp: GitHubPutFileResponse = serde_json::from_slice(&res.bytes().await?)?;
        let sha = put_resp
            .content
            .map(|c| c.sha)
            .or_else(|| put_resp.commit.map(|c| c.sha))
            .unwrap_or_default();
        Ok(sha)
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
        for paper in &mut merged_papers {
            sanitize_synced_paper(paper);
        }
        let mut new_locally = Vec::new();

        // 1. Find local papers not present in remote manifest
        for local in local_papers {
            let already_in_remote = merged_papers.iter().any(|remote| {
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
                let clean_arxiv = local
                    .arxiv_id
                    .as_deref()
                    .and_then(crate::api::arxiv::parse_arxiv_id);
                let doi = local.doi.clone().or_else(|| {
                    local.arxiv_id.as_ref().and_then(|id| {
                        if (id.starts_with("10.") || id.contains('/')) && clean_arxiv.is_none() {
                            Some(id.clone())
                        } else {
                            None
                        }
                    })
                });
                let pdf_url = clean_arxiv
                    .as_ref()
                    .map(|id| format!("https://arxiv.org/pdf/{id}.pdf"));
                let synced = SyncedPaper {
                    arxiv_id: clean_arxiv,
                    doi,
                    title: local.title.clone(),
                    pdf_url,
                };
                new_locally.push(synced.clone());
                merged_papers.push(synced);
            }
        }

        // 2. Find remote papers not present in local library
        let mut missing_locally = Vec::new();
        for remote in &merged_papers {
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

    /// Discover the authenticated GitHub user's login (`GET /user`), normalized to lowercase.
    ///
    /// # Errors
    /// Returns [`SyncError`] if the network request or API authentication fails.
    pub async fn fetch_authenticated_user(&self) -> Result<String, SyncError> {
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
        Ok(user.login.trim().to_ascii_lowercase())
    }

    /// Fetch raw file bytes and Git blob SHA at `path` inside `{owner}/{repo}`.
    ///
    /// Returns `Ok(None)` if the file does not exist (`404 Not Found`).
    ///
    /// # Errors
    /// Returns [`SyncError`] if the request or base64 decoding fails.
    pub async fn get_file_bytes(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
    ) -> Result<Option<(Vec<u8>, String)>, SyncError> {
        let url = format!(
            "https://api.github.com/repos/{owner}/{repo}/contents/{path}?ref={}",
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
        let raw_b64 = content_resp
            .content
            .unwrap_or_default()
            .replace(['\n', '\r'], "");
        let decoded = if raw_b64.is_empty() {
            Vec::new()
        } else {
            BASE64_STANDARD.decode(&raw_b64)?
        };
        Ok(Some((decoded, content_resp.sha)))
    }

    /// Create or update a file at `path` inside `{owner}/{repo}`.
    ///
    /// Automatically resolves the existing file's SHA when `previous_sha` is `None`.
    ///
    /// # Errors
    /// Returns [`SyncError`] if the GitHub Contents API rejects the update.
    pub async fn put_file_bytes(
        &self,
        owner: &str,
        repo: &str,
        path: &str,
        raw_bytes: &[u8],
        message: &str,
        previous_sha: Option<&str>,
    ) -> Result<String, SyncError> {
        let resolved_sha = if let Some(sha) = previous_sha {
            Some(sha.to_string())
        } else {
            self.get_file_bytes(owner, repo, path)
                .await?
                .map(|(_, sha)| sha)
        };

        let url = format!("https://api.github.com/repos/{owner}/{repo}/contents/{path}");
        let b64_content = BASE64_STANDARD.encode(raw_bytes);
        let put_req = PutFileRequest {
            message,
            content: &b64_content,
            sha: resolved_sha.as_deref(),
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
            let msg = res.text().await.unwrap_or_default();
            return Err(SyncError::Api {
                status,
                message: msg,
            });
        }

        let put_resp: GitHubPutFileResponse = serde_json::from_slice(&res.bytes().await?)?;
        let sha = put_resp
            .content
            .map(|c| c.sha)
            .or_else(|| put_resp.commit.map(|c| c.sha))
            .unwrap_or_default();
        Ok(sha)
    }

    /// Fetch a GitHub user's `ssh-ed25519` public keys from:
    /// 1. `GET https://api.github.com/users/{username}/keys`
    /// 2. `GET https://github.com/{username}.keys`
    /// 3. `projects/.keys/{username}.pub` inside `{owner}/{repo}` (Papr fallback key directory)
    ///
    /// # Errors
    /// Returns [`SyncError`] if `username` is syntactically invalid.
    pub async fn fetch_user_ssh_keys(
        &self,
        owner: &str,
        repo: &str,
        username: &str,
    ) -> Result<Vec<SshEd25519PublicKey>, SyncError> {
        let clean_user = validate_github_username(username)?;
        let mut keys = Vec::new();

        // 1. Authenticated GitHub API endpoint
        let api_url = format!("https://api.github.com/users/{clean_user}/keys");
        if let Ok(res) = self
            .client
            .get(&api_url)
            .header("User-Agent", "papr")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            && res.status().is_success()
            && let Ok(bytes) = res.bytes().await
            && let Ok(list) = serde_json::from_slice::<Vec<GitHubSshKeyObject>>(&bytes)
        {
            for item in list {
                if let Ok(parsed) = SshEd25519PublicKey::parse_openssh_line(&item.key)
                    && !keys
                        .iter()
                        .any(|k: &SshEd25519PublicKey| k.ed25519_bytes() == parsed.ed25519_bytes())
                {
                    keys.push(parsed);
                }
            }
        }

        // 2. Public https://github.com/{username}.keys endpoint
        if keys.is_empty() {
            let web_url = format!("https://github.com/{clean_user}.keys");
            if let Ok(res) = self
                .client
                .get(&web_url)
                .header("User-Agent", "papr")
                .send()
                .await
                && res.status().is_success()
                && let Ok(text) = res.text().await
            {
                for parsed in SshEd25519PublicKey::parse_many_lines(&text) {
                    if !keys
                        .iter()
                        .any(|k: &SshEd25519PublicKey| k.ed25519_bytes() == parsed.ed25519_bytes())
                    {
                        keys.push(parsed);
                    }
                }
            }
        }

        // 3. Repository fallback directory `projects/.keys/{username}.pub`
        let repo_key_path = format!("projects/.keys/{clean_user}.pub");
        if let Ok(Some((bytes, _))) = self.get_file_bytes(owner, repo, &repo_key_path).await
            && let Ok(text) = String::from_utf8(bytes)
        {
            for parsed in SshEd25519PublicKey::parse_many_lines(&text) {
                if !keys
                    .iter()
                    .any(|k: &SshEd25519PublicKey| k.ed25519_bytes() == parsed.ed25519_bytes())
                {
                    keys.push(parsed);
                }
            }
        }

        Ok(keys)
    }

    /// Publish the current user's local `ssh-ed25519` public keys to `projects/.keys/{username}.pub`
    /// inside `{owner}/{repo}` so collaborators can wrap keys even if the user hasn't uploaded
    /// an SSH key to their GitHub profile settings yet.
    ///
    /// # Errors
    /// Returns [`SyncError`] if the upload fails.
    pub async fn publish_user_public_key(
        &self,
        owner: &str,
        repo: &str,
        username: &str,
        local_keys: &[SshEd25519KeyPair],
    ) -> Result<(), SyncError> {
        let clean_user = validate_github_username(username)?;
        if local_keys.is_empty() {
            return Ok(());
        }
        let mut lines = Vec::new();
        for pair in local_keys {
            let line = pair.public_key().to_openssh_line();
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        let payload = format!("{}\n", lines.join("\n"));
        let path = format!("projects/.keys/{clean_user}.pub");

        let existing = self.get_file_bytes(owner, repo, &path).await?;
        if let Some((ref current_bytes, _)) = existing
            && current_bytes == payload.as_bytes()
        {
            return Ok(());
        }
        let prev_sha = existing.as_ref().map(|(_, sha)| sha.as_str());
        let msg = format!("papr: publish public key for @{clean_user}");
        let _ = self
            .put_file_bytes(owner, repo, &path, payload.as_bytes(), &msg, prev_sha)
            .await?;
        Ok(())
    }

    /// Invite `username` as a repository collaborator on `{owner}/{repo}` so they can push/pull
    /// encrypted project bundles stored in the shared repository.
    ///
    /// Note: Project-level read/write confidentiality is enforced cryptographically by each
    /// project's [`ProjectKeyEnvelope`] (`keys.json`) and `ChaCha20-Poly1305` `bundle.enc`.
    ///
    /// # Errors
    /// Returns [`SyncError`] only if `username` is invalid or a network failure occurs.
    pub async fn invite_repo_collaborator(
        &self,
        owner: &str,
        repo: &str,
        username: &str,
    ) -> Result<(), SyncError> {
        let clean_user = validate_github_username(username)?;
        if clean_user.eq_ignore_ascii_case(owner) {
            return Ok(());
        }
        let url =
            format!("https://api.github.com/repos/{owner}/{repo}/collaborators/{clean_user}");
        let body = serde_json::to_vec(&InviteCollaboratorRequest { permission: "push" })?;
        let _ = self
            .client
            .put(&url)
            .header("User-Agent", "papr")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Accept", "application/vnd.github+json")
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await;
        Ok(())
    }

    /// List all project directories under `projects/` in `{owner}/{repo}`.
    ///
    /// # Errors
    /// Returns [`SyncError`] if the GitHub API call fails.
    pub async fn list_remote_projects(
        &self,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<String>, SyncError> {
        let url = format!(
            "https://api.github.com/repos/{owner}/{repo}/contents/projects?ref={}",
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
            return Ok(Vec::new());
        }
        if !res.status().is_success() {
            let status = res.status().as_u16();
            let message = res.text().await.unwrap_or_default();
            return Err(SyncError::Api { status, message });
        }

        let entries: Vec<GitHubDirEntry> = serde_json::from_slice(&res.bytes().await?)?;
        let mut names: Vec<String> = entries
            .into_iter()
            .filter(|e| e.entry_type == "dir" && !e.name.starts_with('.'))
            .map(|e| e.name)
            .collect();
        names.sort();
        Ok(names)
    }

    /// Encrypt and push a single local project (`project_root`) to `projects/{project_name}/`
    /// in `{owner}/{repo}`.
    ///
    /// Resolves and wraps the project key for all collaborators in [`ProjectKeyEnvelope`]
    /// using their GitHub `ssh-ed25519` public keys before uploading `keys.json` and `bundle.enc`.
    ///
    /// # Errors
    /// Returns [`SyncError`] if encryption or GitHub upload fails.
    pub async fn push_encrypted_project(
        &self,
        owner: &str,
        repo: &str,
        project_root: &Path,
        project_name: &str,
        current_user: &str,
        local_keys: &[SshEd25519KeyPair],
    ) -> Result<ProjectKeyEnvelope, SyncError> {
        let (mut envelope, project_key) = ensure_local_project_collab(
            project_root,
            project_name,
            "typst",
            current_user,
            local_keys,
        )?;

        // Refresh SSH public keys and wrap ProjectKey for owner + all collaborators
        for member in envelope.all_members() {
            if let Ok(remote_keys) = self.fetch_user_ssh_keys(owner, repo, &member).await
                && !remote_keys.is_empty()
            {
                envelope.wrap_keys_for_user(&member, &remote_keys, &project_key)?;
            }
        }
        for collaborator in &envelope.collaborators {
            let _ = self
                .invite_repo_collaborator(owner, repo, collaborator)
                .await;
        }

        envelope.updated_at = Utc::now().to_rfc3339();
        save_local_envelope(project_root, &envelope, Some(&project_key))?;

        let encrypted_bundle = encrypt_project_directory(
            project_root,
            project_name,
            &envelope.compiler,
            envelope.key_generation,
            &project_key,
        )?;

        let keys_path = format!("projects/{project_name}/keys.json");
        let bundle_path = format!("projects/{project_name}/bundle.enc");

        let keys_json = serde_json::to_vec_pretty(&envelope)?;
        let keys_msg = format!(
            "papr: update key envelope for project '{project_name}' (gen {})",
            envelope.key_generation
        );
        self.put_file_bytes(owner, repo, &keys_path, &keys_json, &keys_msg, None)
            .await?;

        let bundle_msg = format!(
            "papr: sync encrypted project '{project_name}' (gen {})",
            envelope.key_generation
        );
        self.put_file_bytes(
            owner,
            repo,
            &bundle_path,
            &encrypted_bundle,
            &bundle_msg,
            None,
        )
        .await?;

        Ok(envelope)
    }

    /// Synchronize all collaborative encrypted projects between `projects_root` and `{owner}/{repo}`.
    ///
    /// 1. Publishes the current user's `ssh-ed25519` public key to `projects/.keys/{current_user}.pub`.
    /// 2. Pulls remote projects under `projects/`:
    ///    - If the current user's local keys unlock `keys.json`, decrypts `bundle.enc` and unpacks
    ///      new or updated projects locally.
    ///    - If the current user is not an authorized collaborator on a remote project, skips it
    ///      and records it in [`ProjectSyncSummary::skipped_unauthorized`].
    /// 3. Pushes local collaborative projects (those with `.papr-collab.json`) that are new
    ///    remotely or modified locally.
    ///
    /// # Errors
    /// Returns [`SyncError`] if listing or transferring repository contents fails.
    pub async fn sync_all_projects(
        &self,
        owner: &str,
        repo: &str,
        projects_root: &Path,
        current_user: &str,
        local_keys: &[SshEd25519KeyPair],
    ) -> Result<ProjectSyncSummary, SyncError> {
        let mut summary = ProjectSyncSummary::default();
        let _ = self
            .publish_user_public_key(owner, repo, current_user, local_keys)
            .await;

        let remote_projects = self.list_remote_projects(owner, repo).await?;

        for proj_name in &remote_projects {
            let keys_path = format!("projects/{proj_name}/keys.json");
            let Some((keys_bytes, _)) = self.get_file_bytes(owner, repo, &keys_path).await? else {
                continue;
            };
            let Ok(remote_envelope) = serde_json::from_slice::<ProjectKeyEnvelope>(&keys_bytes)
            else {
                continue;
            };

            let Ok(project_key) = remote_envelope.unwrap_project_key(local_keys) else {
                summary.skipped_unauthorized.push(proj_name.clone());
                continue;
            };

            let local_dir = projects_root.join(proj_name);
            let local_env = load_local_envelope(&local_dir).ok().flatten();
            let should_pull = match &local_env {
                None => true,
                Some(local) => {
                    remote_envelope.key_generation > local.key_generation
                        || remote_envelope.updated_at > local.updated_at
                        || remote_envelope.collaborators != local.collaborators
                }
            };

            if should_pull {
                let bundle_path = format!("projects/{proj_name}/bundle.enc");
                if let Some((bundle_bytes, _)) =
                    self.get_file_bytes(owner, repo, &bundle_path).await?
                    && let Ok(archive) =
                        decrypt_project_bundle(&bundle_bytes, proj_name, &project_key)
                {
                    unpack_project_archive(&archive, &local_dir)?;
                    save_local_envelope(&local_dir, &remote_envelope, Some(&project_key))?;
                    summary.downloaded.push(proj_name.clone());
                }
            }
        }

        // Push local collaborative projects (those with .papr-collab.json)
        if let Ok(entries) = std::fs::read_dir(projects_root) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(proj_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if proj_name.starts_with('.') || summary.downloaded.iter().any(|d| d == proj_name)
                {
                    continue;
                }
                let Ok(Some(_local_env)) = load_local_envelope(&path) else {
                    continue;
                };
                if self
                    .push_encrypted_project(owner, repo, &path, proj_name, current_user, local_keys)
                    .await
                    .is_ok()
                {
                    summary.uploaded.push(proj_name.to_string());
                }
            }
        }

        Ok(summary)
    }
}

/// Helper function to clean and fix an arXiv ID and PDF URL in a synced paper.
fn sanitize_synced_paper(paper: &mut SyncedPaper) {
    if let Some(ref aid) = paper.arxiv_id
        && (aid.starts_with("10.")
            || (aid.contains('/') && crate::api::arxiv::parse_arxiv_id(aid).is_none()))
    {
        if paper.doi.is_none() {
            paper.doi = Some(aid.clone());
        }
        paper.arxiv_id = None;
    }

    let clean_arxiv = paper
        .arxiv_id
        .as_deref()
        .and_then(crate::api::arxiv::parse_arxiv_id)
        .or_else(|| {
            paper
                .pdf_url
                .as_deref()
                .and_then(crate::api::arxiv::parse_arxiv_id)
        });
    if let Some(clean_id) = clean_arxiv {
        paper.pdf_url = Some(format!("https://arxiv.org/pdf/{clean_id}.pdf"));
        paper.arxiv_id = Some(clean_id);
    } else if let Some(ref url) = paper.pdf_url
        && (url.contains("arxiv.org/pdf/10.")
            || (url.contains("arxiv.org/pdf/") && crate::api::arxiv::parse_arxiv_id(url).is_none()))
    {
        paper.pdf_url = None;
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
    fn test_reconciliation_sanitizes_doi_in_arxiv_id() {
        let remote_paper = SyncedPaper {
            arxiv_id: Some("10.1103/physrevd.65.084011".to_string()),
            doi: Some("10.1103/physrevd.65.084011".to_string()),
            title: "Second post-Newtonian gravitational wave polarizations".to_string(),
            pdf_url: Some("https://arxiv.org/pdf/10.1103/physrevd.65.084011.pdf".to_string()),
        };

        let remote_manifest = SyncManifest {
            version: 1,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            papers: vec![remote_paper],
        };

        let reconciliation = GitHubSyncService::reconcile(&[], Some(remote_manifest));

        assert_eq!(reconciliation.missing_locally.len(), 1);
        let sanitized = &reconciliation.missing_locally[0];
        assert_eq!(sanitized.arxiv_id, None);
        assert_eq!(sanitized.doi.as_deref(), Some("10.1103/physrevd.65.084011"));
        assert_eq!(sanitized.pdf_url, None);
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

    #[test]
    fn test_github_put_file_response_deserialization() -> Result<(), Box<dyn std::error::Error>> {
        let payload = r#"{
            "content": {
                "name": "papers.json",
                "path": "papers.json",
                "sha": "95b9668d1d50300c7a4e00511ed8f1732442b0c2",
                "size": 1234
            },
            "commit": {
                "sha": "7638417db6d59f3c431d3e1f261cc637155684cd",
                "message": "papr: sync papers"
            }
        }"#;

        let resp: GitHubPutFileResponse = serde_json::from_str(payload)?;
        assert_eq!(
            resp.content.map(|c| c.sha).as_deref(),
            Some("95b9668d1d50300c7a4e00511ed8f1732442b0c2")
        );
        assert_eq!(
            resp.commit.map(|c| c.sha).as_deref(),
            Some("7638417db6d59f3c431d3e1f261cc637155684cd")
        );
        Ok(())
    }
}
