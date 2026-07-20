//! TOML-backed application configuration.

use std::{collections::HashSet, fs, path::PathBuf};

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
    /// Plugin configuration file.
    pub plugins_config_file: PathBuf,
    /// Default directory for managed LaTeX projects.
    pub projects_dir: PathBuf,
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
            plugins_config_file: dirs.config_dir().join("plugins.toml"),
            projects_dir: dirs.data_dir().join("projects"),
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
    /// Plugin identifiers explicitly allowed to execute.
    pub enabled_plugins: Vec<String>,
    /// Default directory used to create and discover writing projects.
    pub projects_directory: Option<PathBuf>,
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
            enabled_plugins: Vec::new(),
            projects_directory: None,
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
            let mut config: Self = toml::from_str(&fs::read_to_string(&paths.config_file)?)?;
            // Upgrade older configurations in place so the setting remains
            // visible and editable through Papr's normal settings workflow.
            if config.projects_directory.is_none() {
                config.projects_directory = Some(paths.projects_dir.clone());
                fs::write(&paths.config_file, toml::to_string_pretty(&config)?)?;
            }
            fs::create_dir_all(config.projects_directory(paths))?;
            return Ok(config);
        }
        if let Some(parent) = paths.config_file.parent() {
            fs::create_dir_all(parent)?;
        }

        let default_pdf_viewer = if cfg!(target_os = "macos") {
            "open"
        } else if cfg!(target_os = "windows") {
            "cmd /C start msedge \"\""
        } else {
            "xdg-open"
        };

        #[derive(Serialize)]
        struct PathValue {
            val: PathBuf,
        }
        let serialized_path = toml::to_string(&PathValue { val: paths.downloads_dir.clone() })?;
        let downloads_dir_str = serialized_path
            .strip_prefix("val = ")
            .unwrap_or(&serialized_path)
            .trim_end();
        let serialized_projects_path = toml::to_string(&PathValue { val: paths.projects_dir.clone() })?;
        let projects_dir_str = serialized_projects_path
            .strip_prefix("val = ")
            .unwrap_or(&serialized_projects_path)
            .trim_end();

        #[derive(Serialize)]
        struct StringValue {
            val: String,
        }
        let serialized_viewer = toml::to_string(&StringValue { val: default_pdf_viewer.to_string() })?;
        let pdf_viewer_str = serialized_viewer
            .strip_prefix("val = ")
            .unwrap_or(&serialized_viewer)
            .trim_end();

        let toml_content = format!(
            r#"# Active built-in theme name or path to a custom TOML theme.
theme = "catppuccin-mocha"

# Page selected at startup.
startup_page = "dashboard"

# Preferred external PDF viewer command.
pdf_viewer = {pdf_viewer_str}

# Directories scanned for existing papers.
library_folders = [
  {downloads_dir_str}
]

# Destination for downloaded papers.
download_path = {downloads_dir_str}

# Comma-separated search terms used for dashboard recommendations.
dashboard_keywords = ""

# Plugin identifiers explicitly allowed to execute.
enabled_plugins = []

# Default directory for LaTeX writing projects.
projects_directory = {projects_dir_str}
"#
        );

        fs::write(&paths.config_file, toml_content)?;

        let mut config = Self::default();
        config.library_folders = vec![paths.downloads_dir.clone()];
        config.download_path = Some(paths.downloads_dir.clone());
        config.projects_directory = Some(paths.projects_dir.clone());
        config.pdf_viewer = Some(default_pdf_viewer.to_string());
        fs::create_dir_all(&paths.projects_dir)?;
        Ok(config)
    }

    /// Resolve the projects directory, retaining a platform-default for older
    /// configuration files that predate `projects_directory`.
    #[must_use]
    pub fn projects_directory(&self, paths: &Paths) -> PathBuf {
        self.projects_directory
            .clone()
            .unwrap_or_else(|| paths.projects_dir.clone())
    }

    /// Return configured dashboard recommendation terms.
    #[must_use]
    pub fn dashboard_keyword_list(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.dashboard_keywords
            .split(',')
            .map(|keyword| keyword.split_whitespace().collect::<Vec<_>>().join(" "))
            .map(|keyword| keyword.to_lowercase())
            .filter(|keyword| !keyword.is_empty() && seen.insert(keyword.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use serde::{Deserialize, Serialize};

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

    #[test]
    fn dashboard_keywords_normalize_spacing_and_remove_duplicates() -> Result<(), toml::de::Error> {
        let config: Config = toml::from_str(
            "dashboard_keywords = 'Machine   Learning, machine learning, GRAVITATIONAL WAVES'",
        )?;
        assert_eq!(
            config.dashboard_keyword_list(),
            ["machine learning", "gravitational waves"]
        );
        Ok(())
    }

    #[test]
    fn test_load_or_create_generates_all_keys() -> Result<(), Box<dyn std::error::Error>> {
        let unique_id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("papr_test_{}", unique_id));
        std::fs::create_dir_all(&temp_dir)?;

        let paths = super::Paths {
            config_file: temp_dir.join("config.toml"),
            database_file: temp_dir.join("papr.db"),
            downloads_dir: temp_dir.join("papers"),
            plugins_dir: temp_dir.join("plugins"),
            plugins_config_file: temp_dir.join("plugins.toml"),
            projects_dir: temp_dir.join("projects"),
        };

        let config = Config::load_or_create(&paths)?;
        
        let content = std::fs::read_to_string(&paths.config_file)?;
        
        assert!(content.contains("theme ="));
        assert!(content.contains("startup_page ="));
        assert!(content.contains("pdf_viewer ="));
        assert!(content.contains("library_folders ="));
        assert!(content.contains("download_path ="));
        assert!(content.contains("dashboard_keywords ="));
        assert!(content.contains("enabled_plugins ="));
        assert!(content.contains("projects_directory ="));
        
        let parsed: Config = toml::from_str(&content)?;
        assert_eq!(parsed.theme, config.theme);
        assert_eq!(parsed.startup_page, config.startup_page);
        
        let expected_viewer = if cfg!(target_os = "macos") {
            "open".to_string()
        } else if cfg!(target_os = "windows") {
            "cmd /C start msedge \"\"".to_string()
        } else {
            "xdg-open".to_string()
        };
        assert_eq!(parsed.pdf_viewer, Some(expected_viewer));

        std::fs::remove_dir_all(&temp_dir)?;
        Ok(())
    }

    #[test]
    fn test_pdf_viewer_serialization_formats_correctly() -> Result<(), Box<dyn std::error::Error>> {
        let test_cases = vec![
            "open",
            "cmd /C start \"\"",
            "cmd /C start msedge \"\"",
            "xdg-open",
            "C:\\Program Files\\SumatraPDF\\SumatraPDF.exe",
        ];

        for viewer in test_cases {
            #[derive(Serialize)]
            struct StringValue {
                val: String,
            }
            let serialized = toml::to_string(&StringValue { val: viewer.to_string() })?;
            let formatted_viewer = serialized
                .strip_prefix("val = ")
                .unwrap_or(&serialized)
                .trim_end();
            
            let toml_content = format!("pdf_viewer = {}\n", formatted_viewer);
            
            #[derive(Deserialize)]
            struct MockConfig {
                pdf_viewer: String,
            }
            let parsed: MockConfig = toml::from_str(&toml_content)?;
            assert_eq!(parsed.pdf_viewer, viewer);
        }
        Ok(())
    }
}
