//! TOML-backed application configuration.

use std::{fs, path::PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors produced while resolving or loading configuration.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// The operating system did not expose standard user directories.
    #[error("could not resolve platform application directories")]
    MissingPlatformDirectories,
    /// Configuration I/O failed.
    #[error("configuration I/O failed: {0}")]
    Io(#[from] std::io::Error),
    /// TOML parsing or serialization failed.
    #[error("invalid configuration: {0}")]
    Toml(#[from] toml::de::Error),
    /// TOML serialization failed.
    #[error("could not serialize configuration: {0}")]
    Serialize(#[from] toml::ser::Error),
}

/// Resolved platform-specific filesystem locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    /// Main TOML configuration file.
    pub config_file: PathBuf,
    /// `SQLite` database file.
    pub database_file: PathBuf,
    /// Default PDF download directory.
    pub downloads_dir: PathBuf,
    /// Directory containing external plugin bundles.
    pub plugins_dir: PathBuf,
}

impl Paths {
    /// Resolve paths using the operating system's standard directories.
    ///
    /// # Errors
    ///
    /// Returns an error when platform application directories are unavailable.
    pub fn discover() -> Result<Self, ConfigError> {
        let dirs = ProjectDirs::from("org", "papr", "papr")
            .ok_or(ConfigError::MissingPlatformDirectories)?;
        Ok(Self {
            config_file: dirs.config_dir().join("config.toml"),
            database_file: dirs.data_dir().join("papr.db"),
            downloads_dir: dirs.data_dir().join("papers"),
            plugins_dir: dirs.data_dir().join("plugins"),
        })
    }
}

/// User preferences loaded from TOML.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Config {
    /// Active built-in theme name or path to a custom TOML theme.
    pub theme: String,
    /// Page selected at startup.
    pub startup_page: String,
    /// Preferred external PDF viewer command.
    pub pdf_viewer: Option<String>,
    /// Directories scanned for existing papers.
    pub library_folders: Vec<PathBuf>,
    /// Destination for downloaded papers.
    pub download_path: Option<PathBuf>,
    /// Comma-separated search terms used for dashboard recommendations.
    pub dashboard_keywords: String,
    /// Whether mouse event capture is enabled.
    pub mouse: bool,
    /// Plugin identifiers explicitly allowed to execute.
    pub enabled_plugins: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "catppuccin-mocha".into(),
            startup_page: "dashboard".into(),
            pdf_viewer: None,
            library_folders: Vec::new(),
            download_path: None,
            dashboard_keywords: String::new(),
            mouse: false,
            enabled_plugins: Vec::new(),
        }
    }
}

impl Config {
    /// Load configuration, creating a documented default file when absent.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read, written, or parsed.
    pub fn load_or_create(paths: &Paths) -> Result<Self, ConfigError> {
        if paths.config_file.exists() {
            return Ok(toml::from_str(&fs::read_to_string(&paths.config_file)?)?);
        }
        if let Some(parent) = paths.config_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let config = Self::default();
        fs::write(&paths.config_file, toml::to_string_pretty(&config)?)?;
        Ok(config)
    }

    /// Return configured dashboard recommendation terms.
    #[must_use]
    pub fn dashboard_keyword_list(&self) -> Vec<String> {
        self.dashboard_keywords
            .split(',')
            .map(str::trim)
            .filter(|keyword| !keyword.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Config;

    #[test]
    fn defaults_apply_to_partial_toml() -> Result<(), toml::de::Error> {
        let config: Config = toml::from_str("theme = 'nord'")?;
        assert_eq!(config.theme, "nord");
        assert_eq!(config.startup_page, "dashboard");
        Ok(())
    }

    #[test]
    fn dashboard_keywords_parse_as_comma_separated_terms() -> Result<(), toml::de::Error> {
        let config: Config =
            toml::from_str("dashboard_keywords = 'machine learning, gravitational waves,  '")?;
        assert_eq!(
            config.dashboard_keyword_list(),
            ["machine learning", "gravitational waves"]
        );
        Ok(())
    }
}
