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
