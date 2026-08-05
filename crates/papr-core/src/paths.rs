//! Cross-platform filesystem path normalization.

use std::{io, path::{Path, PathBuf}};

/// Resolve a path through the filesystem while preserving the native path form
/// expected by applications and users on every supported platform.
///
/// Windows' standard canonicalization API returns extended-length (`\\?\\`)
/// paths. They identify the same filesystem object but are unsuitable as the
/// application-wide representation because they leak into persisted metadata
/// and normal UI rendering. `dunce` removes that transport prefix whenever a
/// regular Windows path can represent the same location.
pub fn canonicalize_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Sanitizes a string for use as a download filename component.
pub fn sanitize_download_filename_component(title: &str) -> String {
    let sanitized: String = title
        .chars()
        .map(|c| match c {
            '/' => '_',
            '\n' | '\r' | '\t' => ' ',
            #[cfg(windows)]
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            #[cfg(windows)]
            c if c.is_control() => ' ',
            #[cfg(not(windows))]
            c if c.is_control() => c,
            c => c,
        })
        .collect();
    let sanitized = sanitized.trim();
    #[cfg(windows)]
    {
        sanitized.trim_end_matches(['.', ' ']).to_owned()
    }
    #[cfg(not(windows))]
    {
        sanitized.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(not(windows))]
    fn download_filename_preserves_colon_on_supported_platforms() {
        let sanitized = sanitize_download_filename_component("AntiGlitch: Better / Faster");
        assert_eq!(sanitized, "AntiGlitch: Better _ Faster");
    }

    #[test]
    #[cfg(windows)]
    fn download_filename_sanitizes_colon_on_windows() {
        let sanitized = sanitize_download_filename_component("AntiGlitch: Better / Faster");
        assert_eq!(sanitized, "AntiGlitch_ Better _ Faster");
    }
}
