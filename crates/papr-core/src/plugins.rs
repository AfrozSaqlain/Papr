//! Versioned, process-isolated plugin discovery and invocation.

use std::{
    collections::HashSet,
    path::{Component, Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

/// Protocol version supported by this build.
pub const PLUGIN_API_VERSION: u16 = 1;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// Operations a plugin may request access to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCapability {
    /// Contribute scholarly metadata providers.
    MetadataProvider,
    /// Contribute commands to Browse Papr.
    Commands,
    /// Receive lifecycle activity events.
    ActivityEvents,
    /// Read public paper metadata supplied in invocation context.
    ReadPaperMetadata,
}

/// Parsed plugin manifest contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Stable lowercase identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plugin release version.
    pub version: String,
    /// Supported papr plugin API version.
    pub api_version: u16,
    /// Short purpose statement.
    #[serde(default)]
    pub description: String,
    /// Executable path relative to the manifest.
    pub executable: PathBuf,
    /// Arguments passed before protocol input.
    #[serde(default)]
    pub args: Vec<String>,
    /// Declared permissions and integration surfaces.
    #[serde(default)]
    pub capabilities: Vec<PluginCapability>,
}

/// Plugin information safe to expose in the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    /// Stable plugin identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plugin release version.
    pub version: String,
    /// Short purpose statement.
    pub description: String,
    /// Whether configuration permits execution.
    pub enabled: bool,
    /// Declared capabilities.
    pub capabilities: Vec<PluginCapability>,
}

/// Non-fatal manifest discovery problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDiagnostic {
    /// Manifest or bundle path.
    pub path: PathBuf,
    /// Actionable failure description.
    pub message: String,
}

/// JSON request sent to a plugin process on standard input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginRequest {
    /// Protocol version for compatibility checks.
    pub api_version: u16,
    /// Hook or command identifier.
    pub event: String,
    /// Event-specific structured data.
    #[serde(default)]
    pub context: Value,
}

impl PluginRequest {
    /// Construct a request using the current protocol version.
    #[must_use]
    pub fn new(event: impl Into<String>, context: Value) -> Self {
        Self {
            api_version: PLUGIN_API_VERSION,
            event: event.into(),
            context,
        }
    }
}

/// A constrained host action returned by a plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginAction {
    /// Show a non-blocking message.
    Notify {
        /// User-facing text.
        message: String,
    },
    /// Add the paper in context to a collection.
    AddToCollection {
        /// Collection name.
        name: String,
    },
}

/// JSON response read from plugin standard output.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginResponse {
    /// Requested host-side actions.
    #[serde(default)]
    pub actions: Vec<PluginAction>,
}

#[derive(Debug, Clone)]
struct LoadedPlugin {
    manifest: PluginManifest,
    executable: PathBuf,
    enabled: bool,
}

/// Plugin discovery and execution failures.
#[derive(Debug, Error)]
pub enum PluginError {
    /// Plugin directory access failed.
    #[error("plugin filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    /// Manifest TOML was invalid.
    #[error("invalid plugin manifest: {0}")]
    Manifest(#[from] toml::de::Error),
    /// Manifest validation failed.
    #[error("invalid plugin manifest: {0}")]
    Validation(String),
    /// JSON protocol encoding or decoding failed.
    #[error("invalid plugin protocol message: {0}")]
    Json(#[from] serde_json::Error),
    /// Plugin is absent or disabled.
    #[error("plugin '{0}' is not installed and enabled")]
    NotEnabled(String),
    /// Plugin process exceeded the execution deadline.
    #[error("plugin '{0}' timed out")]
    Timeout(String),
    /// Plugin exited unsuccessfully.
    #[error("plugin '{id}' failed: {message}")]
    Process {
        /// Plugin identifier.
        id: String,
        /// Captured standard error.
        message: String,
    },
    /// Plugin response exceeded the safety limit.
    #[error("plugin '{0}' response exceeded 1 MiB")]
    ResponseTooLarge(String),
}

/// Registry of validated external plugins.
#[derive(Debug, Default)]
pub struct PluginHost {
    plugins: Vec<LoadedPlugin>,
    diagnostics: Vec<PluginDiagnostic>,
}

fn ensure_builtin_plugins(root: &Path) -> Result<(), std::io::Error> {
    let has_dirs = if let Ok(entries) = std::fs::read_dir(root) {
        entries.filter_map(Result::ok).any(|entry| entry.path().is_dir())
    } else {
        false
    };

    if !has_dirs {
        let auto_tagger_dir = root.join("auto-tagger");
        std::fs::create_dir_all(&auto_tagger_dir)?;

        let manifest = r#"id = "auto-tagger"
name = "Auto Tagger"
version = "1.0.0"
api_version = 1
description = "Automatically categorizes papers based on keyword rules"
executable = "tagger.py"
capabilities = ["activity-events", "read-paper-metadata"]
"#;
        std::fs::write(auto_tagger_dir.join("plugin.toml"), manifest)?;

        let script = r#"#!/usr/bin/env python3
import json
import sys

def main():
    try:
        request = json.load(sys.stdin)
    except Exception:
        sys.exit(1)

    response = {"actions": []}

    event = request.get("event")
    if event in ("paper_imported", "paper_downloaded"):
        paper = request.get("context", {}).get("paper", {})
        title = paper.get("title", "").lower()

        if any(kw in title for kw in ["neural network", "deep learning", "machine learning", "learning", "transformer", "ai"]):
            response["actions"].append({
                "type": "add_to_collection",
                "name": "Machine Learning"
            })
            response["actions"].append({
                "type": "notify",
                "message": f"Added '{paper.get('title', '')[:30]}...' to Machine Learning"
            })

    print(json.dumps(response))

if __name__ == "__main__":
    main()
"#;
        let script_path = auto_tagger_dir.join("tagger.py");
        std::fs::write(&script_path, script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = std::fs::metadata(&script_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);
                let _ = std::fs::set_permissions(&script_path, perms);
            }
        }
    }
    Ok(())
}

impl PluginHost {
    /// Discover plugin bundles beneath a platform data directory.
    ///
    /// Each bundle is a directory containing plugin.toml. Invalid bundles are
    /// retained as diagnostics and do not prevent other plugins from loading.
    ///
    /// # Errors
    ///
    /// Returns an error only when the root directory cannot be created or read.
    pub fn discover(root: &Path, enabled_ids: &[String]) -> Result<Self, PluginError> {
        std::fs::create_dir_all(root)?;
        let _ = ensure_builtin_plugins(root);
        let enabled = enabled_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut host = Self::default();
        for entry in std::fs::read_dir(root)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    host.diagnostics.push(PluginDiagnostic {
                        path: root.to_path_buf(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if !entry.path().is_dir() {
                continue;
            }
            let manifest_path = entry.path().join("plugin.toml");
            match load_plugin(&manifest_path, &enabled) {
                Ok(plugin) => host.plugins.push(plugin),
                Err(error) => host.diagnostics.push(PluginDiagnostic {
                    path: manifest_path,
                    message: error.to_string(),
                }),
            }
        }
        host.plugins
            .sort_by(|left, right| left.manifest.name.cmp(&right.manifest.name));
        Ok(host)
    }

    /// Valid plugin summaries for settings and diagnostics.
    #[must_use]
    pub fn plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|plugin| PluginInfo {
                id: plugin.manifest.id.clone(),
                name: plugin.manifest.name.clone(),
                version: plugin.manifest.version.clone(),
                description: plugin.manifest.description.clone(),
                enabled: plugin.enabled,
                capabilities: plugin.manifest.capabilities.clone(),
            })
            .collect()
    }

    /// Non-fatal errors encountered during discovery.
    #[must_use]
    pub fn diagnostics(&self) -> &[PluginDiagnostic] {
        &self.diagnostics
    }

    /// Invoke an enabled plugin with a bounded JSON request/response exchange.
    ///
    /// # Errors
    ///
    /// Returns an error for disabled plugins, process failures, timeouts,
    /// oversized output, or malformed responses.
    pub async fn invoke(
        &self,
        id: &str,
        request: &PluginRequest,
        deadline: Duration,
    ) -> Result<PluginResponse, PluginError> {
        let plugin = self
            .plugins
            .iter()
            .find(|plugin| plugin.enabled && plugin.manifest.id == id)
            .ok_or_else(|| PluginError::NotEnabled(id.to_owned()))?;
        let ext = plugin.executable.extension().and_then(|e| e.to_str());
        let mut cmd = match ext {
            Some("py") => {
                let mut c = Command::new("python3");
                c.arg(&plugin.executable);
                c.args(&plugin.manifest.args);
                c
            }
            Some("js") => {
                let mut c = Command::new("node");
                c.arg(&plugin.executable);
                c.args(&plugin.manifest.args);
                c
            }
            Some("sh") => {
                let mut c = Command::new("sh");
                c.arg(&plugin.executable);
                c.args(&plugin.manifest.args);
                c
            }
            _ => {
                let mut c = Command::new(&plugin.executable);
                c.args(&plugin.manifest.args);
                c
            }
        };

        let mut child = cmd
            .env("PAPR_PLUGIN_ID", &plugin.manifest.id)
            .env("PAPR_PLUGIN_API_VERSION", PLUGIN_API_VERSION.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&serde_json::to_vec(request)?).await?;
            stdin.shutdown().await?;
        }
        let output = timeout(deadline, child.wait_with_output())
            .await
            .map_err(|_| PluginError::Timeout(id.to_owned()))??;
        if !output.status.success() {
            return Err(PluginError::Process {
                id: id.to_owned(),
                message: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            });
        }
        if output.stdout.len() > MAX_RESPONSE_BYTES {
            return Err(PluginError::ResponseTooLarge(id.to_owned()));
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    }
}

fn load_plugin(manifest_path: &Path, enabled: &HashSet<&str>) -> Result<LoadedPlugin, PluginError> {
    let manifest: PluginManifest = toml::from_str(&std::fs::read_to_string(manifest_path)?)?;
    validate_manifest(&manifest)?;
    let parent = manifest_path
        .parent()
        .ok_or_else(|| PluginError::Validation("manifest has no parent directory".into()))?;
    let bundle = parent.canonicalize()?;
    let unresolved_executable = parent.join(&manifest.executable);
    if !unresolved_executable.is_file() {
        return Err(PluginError::Validation(format!(
            "executable does not exist: {}",
            unresolved_executable.display()
        )));
    }
    let executable = unresolved_executable.canonicalize()?;
    if !executable.starts_with(&bundle) {
        return Err(PluginError::Validation(
            "executable resolves outside its plugin bundle".into(),
        ));
    }
    let is_enabled = enabled.contains(manifest.id.as_str());
    Ok(LoadedPlugin {
        manifest,
        executable,
        enabled: is_enabled,
    })
}

fn validate_manifest(manifest: &PluginManifest) -> Result<(), PluginError> {
    if manifest.api_version != PLUGIN_API_VERSION {
        return Err(PluginError::Validation(format!(
            "unsupported api_version {}; expected {PLUGIN_API_VERSION}",
            manifest.api_version
        )));
    }
    if manifest.id.is_empty()
        || !manifest.id.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(PluginError::Validation(
            "id must contain only lowercase ASCII letters, digits, and hyphens".into(),
        ));
    }
    if manifest.name.trim().is_empty() || manifest.version.trim().is_empty() {
        return Err(PluginError::Validation(
            "name and version must not be empty".into(),
        ));
    }
    if manifest.executable.is_absolute()
        || manifest
            .executable
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return Err(PluginError::Validation(
            "executable must be a bundle-relative path without parent traversal".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use super::{PLUGIN_API_VERSION, PluginHost, PluginRequest};

    #[test]
    fn discovers_enabled_valid_plugin_and_reports_invalid_bundle()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = temporary_root("discover");
        let valid = root.join("example");
        let invalid = root.join("invalid");
        fs::create_dir_all(&valid)?;
        fs::create_dir_all(&invalid)?;
        fs::write(valid.join("run"), b"executable")?;
        fs::write(
            valid.join("plugin.toml"),
            "id = 'example'\nname = 'Example'\nversion = '1.0.0'\napi_version = 1\nexecutable = 'run'\ncapabilities = ['commands']\n",
        )?;
        fs::write(invalid.join("plugin.toml"), "id = 'INVALID'")?;

        let host = PluginHost::discover(&root, &["example".into()])?;
        assert_eq!(host.plugins().len(), 1);
        assert!(host.plugins()[0].enabled);
        assert_eq!(host.diagnostics().len(), 1);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn request_uses_current_api_version() {
        let request = PluginRequest::new("startup", serde_json::json!({}));
        assert_eq!(request.api_version, PLUGIN_API_VERSION);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn invokes_enabled_process_plugin() -> Result<(), Box<dyn std::error::Error>> {
        use std::{os::unix::fs::PermissionsExt, time::Duration};

        let root = temporary_root("invoke");
        let bundle = root.join("echo");
        fs::create_dir_all(&bundle)?;
        let executable = bundle.join("run");
        fs::write(
            &executable,
            b"#!/bin/sh\ncat >/dev/null\nprintf '{\"actions\":[{\"type\":\"notify\",\"message\":\"ok\"}]}'\n",
        )?;
        let mut permissions = fs::metadata(&executable)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions)?;
        fs::write(
            bundle.join("plugin.toml"),
            "id = 'echo'\nname = 'Echo'\nversion = '1.0.0'\napi_version = 1\nexecutable = 'run'\n",
        )?;
        let host = PluginHost::discover(&root, &["echo".into()])?;
        let response = host
            .invoke(
                "echo",
                &PluginRequest::new("test", serde_json::json!({})),
                Duration::from_secs(2),
            )
            .await?;
        assert_eq!(response.actions.len(), 1);
        fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn creates_builtin_auto_tagger_when_plugins_dir_is_empty() -> Result<(), Box<dyn std::error::Error>> {
        let root = temporary_root("empty_builtin");
        fs::create_dir_all(&root)?;

        let host = PluginHost::discover(&root, &[])?;
        let plugins = host.plugins();
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].id, "auto-tagger");
        assert!(!plugins[0].enabled);
        assert!(root.join("auto-tagger").join("plugin.toml").exists());
        assert!(root.join("auto-tagger").join("tagger.py").exists());

        fs::remove_dir_all(root)?;
        Ok(())
    }

    fn temporary_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("papr-plugin-{label}-{}", std::process::id()))
    }
}
