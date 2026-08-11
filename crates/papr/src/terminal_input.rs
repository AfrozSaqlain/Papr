//! Terminal command parsing and completion for the interactive TUI.
use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn parse_command(command: &str) -> Result<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                // Preserve Windows path separators. A backslash has escape
                // meaning only when it precedes syntax we explicitly support.
                match chars.peek().copied() {
                    Some(next)
                        if next.is_whitespace() || next == '\\' || next == '\'' || next == '\"' =>
                    {
                        current.push(next);
                        let _ = chars.next();
                    }
                    _ => current.push(character),
                }
            }
            '\'' | '"' if quote == Some(character) => quote = None,
            '\'' | '"' if quote.is_none() => quote = Some(character),
            character if character.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if let Some(character) = quote {
        anyhow::bail!("unterminated {character} quote in pdf_viewer");
    }
    if !current.is_empty() {
        args.push(current);
    }
    Ok(args)
}

pub fn terminal_path_candidates(prefix: &str, directory: Option<&Path>) -> Vec<String> {
    let working_directory = directory
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    let Some(working_directory) = working_directory else {
        return Vec::new();
    };
    let (search_directory, display_parent, name_prefix) =
        if let Some(path) = prefix.strip_prefix("~/") {
            let Some(home) = terminal_home_directory() else {
                return Vec::new();
            };
            let typed_path = Path::new(path);
            let (parent, name_prefix) = terminal_path_parent_and_prefix(typed_path, path);
            let search_directory = parent
                .as_ref()
                .map_or_else(|| home.clone(), |parent| home.join(parent));
            let display_parent = Some(parent.as_ref().map_or_else(
                || "~".to_owned(),
                |parent| format!("~{}{}", std::path::MAIN_SEPARATOR, parent.display()),
            ));
            (search_directory, display_parent, name_prefix)
        } else {
            let typed_path = Path::new(prefix);
            let (parent, name_prefix) = terminal_path_parent_and_prefix(typed_path, prefix);
            let search_directory = parent.as_ref().map_or_else(
                || working_directory.clone(),
                |parent| {
                    if parent.is_absolute() {
                        parent.clone()
                    } else {
                        working_directory.join(parent)
                    }
                },
            );
            let display_parent = parent;
            (
                search_directory,
                display_parent.map(|parent| parent.display().to_string()),
                name_prefix,
            )
        };
    let Ok(entries) = std::fs::read_dir(search_directory) else {
        return Vec::new();
    };
    let mut candidates = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.to_lowercase().starts_with(&name_prefix.to_lowercase()) {
                return None;
            }
            let mut candidate = display_parent.as_ref().map_or_else(
                || name.clone(),
                |parent| format!("{parent}{}{}", std::path::MAIN_SEPARATOR, name),
            );
            if entry.path().is_dir() {
                candidate.push(std::path::MAIN_SEPARATOR);
            }
            Some(candidate)
        })
        .collect::<Vec<_>>();
    sort_terminal_candidates(&mut candidates);
    candidates
}

pub fn terminal_path_parent_and_prefix(path: &Path, raw: &str) -> (Option<PathBuf>, String) {
    let has_trailing_separator = raw.ends_with('/') || raw.ends_with(std::path::MAIN_SEPARATOR);
    if has_trailing_separator {
        let parent = raw.trim_end_matches(|character| {
            character == '/' || character == std::path::MAIN_SEPARATOR
        });
        let parent = if parent.is_empty() {
            PathBuf::from(std::path::MAIN_SEPARATOR.to_string())
        } else {
            PathBuf::from(parent)
        };
        (Some(parent), String::new())
    } else {
        (
            path.parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf),
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_owned(),
        )
    }
}

pub fn sort_terminal_candidates(candidates: &mut [String]) {
    candidates.sort_by(|left, right| {
        left.to_lowercase()
            .cmp(&right.to_lowercase())
            .then_with(|| left.cmp(right))
    });
}

pub fn terminal_home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

pub fn sanitize_terminal_output(output: &str) -> String {
    output
        .chars()
        .filter(|character| matches!(character, '\n' | '\t') || !character.is_control())
        .collect()
}
