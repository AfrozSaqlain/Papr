//! Cross-platform filesystem path normalization.

use std::{
    io,
    path::{Path, PathBuf},
};

/// Resolve a path through the filesystem while preserving the native path form
/// expected by applications and users on every supported platform.
///
/// Windows' standard canonicalization API returns extended-length (`\\?\\`)
/// paths. They identify the same filesystem object but are unsuitable as the
/// application-wide representation because they leak into persisted metadata
/// and normal UI rendering. `dunce` removes that transport prefix whenever a
/// regular Windows path can represent the same location.
///
/// # Errors
///
/// Returns an error when the filesystem cannot resolve the supplied path.
pub fn canonicalize_path(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Sanitizes a string for use as a download filename component.
#[must_use]
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

/// Move a PDF file from source to destination.
///
/// # Errors
///
/// Returns an error when neither an atomic rename nor the copy-and-remove
/// fallback can complete safely.
pub fn move_pdf_file(source: &Path, destination: &Path) -> io::Result<()> {
    if let Err(_rename_error) = std::fs::rename(source, destination) {
        std::fs::copy(source, destination)?;
        if let Err(remove_error) = std::fs::remove_file(source) {
            let _ = std::fs::remove_file(destination);
            return Err(remove_error);
        }
    }
    Ok(())
}

/// Error when an invalid collection name is provided.
#[derive(Debug, thiserror::Error)]
#[error("group name must be one safe directory name")]
pub struct InvalidCollectionName;

/// Validates that a collection name is a safe directory name.
///
/// # Errors
///
/// Returns [`InvalidCollectionName`] when the name is empty, reserved, or
/// contains a path separator.
pub fn validate_collection_name(name: &str) -> Result<(), InvalidCollectionName> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\']) {
        Err(InvalidCollectionName)
    } else {
        Ok(())
    }
}

/// Gets the page count of a PDF using `pdfinfo`.
/// Falls back to returning 1 if it fails.
#[must_use]
pub fn get_pdf_page_count(path: &Path) -> usize {
    if let Ok(output) = std::process::Command::new("pdfinfo").arg(path).output()
        && output.status.success()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            if line.starts_with("Pages:")
                && let Some(pages_str) = line.split_whitespace().nth(1)
                && let Ok(pages) = pages_str.parse::<usize>()
            {
                return pages;
            }
        }
    }
    1
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
