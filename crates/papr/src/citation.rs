use papr_core::models::CitationMetadata;
use std::fmt::Write as _;
use std::path::PathBuf;

static CLIPBOARD: std::sync::OnceLock<std::sync::Mutex<Option<arboard::Clipboard>>> =
    std::sync::OnceLock::new();

/// Fetch the canonical BibTeX for `metadata`.
///
/// Priority:
/// 1. DOI → `doi.org` content-negotiation (returns the publisher's own record)
/// 2. arXiv ID → `arxiv.org/bibtex/` endpoint
/// 3. Best-effort local generation (no network required)
///
/// Returns `(citation_key, bibtex_string)`.
pub async fn fetch_bibtex(metadata: &CitationMetadata) -> (String, String) {
    // 1. Try DOI
    if let Some(doi) = metadata.doi.as_deref().and_then(extract_doi)
        && let Ok(response) = reqwest::Client::new()
            .get(format!("https://doi.org/{doi}"))
            .header("Accept", "application/x-bibtex")
            .send()
            .await
        && response.status().is_success()
        && let Ok(bibtex) = response.text().await
        && !bibtex.trim().is_empty()
    {
        // Extract key from the BibTeX string (first token after `{`)
        let key = bibtex
            .trim()
            .trim_start_matches('@')
            .find('{')
            .and_then(|i| {
                bibtex.trim().trim_start_matches('@')[i + 1..]
                    .split(',')
                    .next()
                    .map(|k| k.trim().to_string())
            })
            .unwrap_or_else(|| best_effort_key(metadata));
        return (key, bibtex);
    }

    // 2. Try arXiv
    if let Some(arxiv_id) = metadata.arxiv_id.as_deref().and_then(extract_arxiv)
        && let Ok(response) = reqwest::Client::new()
            .get(format!("https://arxiv.org/bibtex/{arxiv_id}"))
            .send()
            .await
        && response.status().is_success()
        && let Ok(bibtex) = response.text().await
        && !bibtex.trim().is_empty()
    {
        let key = bibtex
            .trim()
            .trim_start_matches('@')
            .find('{')
            .and_then(|i| {
                bibtex.trim().trim_start_matches('@')[i + 1..]
                    .split(',')
                    .next()
                    .map(|k| k.trim().to_string())
            })
            .unwrap_or_else(|| best_effort_key(metadata));
        return (key, bibtex);
    }

    // 3. Best-effort local generation
    generate_bibtex(metadata)
}

pub async fn fetch_and_copy_citation(
    metadata: CitationMetadata,
    sender: tokio::sync::mpsc::UnboundedSender<crate::AppEvent>,
) {
    let (_, bibtex) = fetch_bibtex(&metadata).await;

    let source = if bibtex.trim().contains("doi.org") || bibtex.trim().contains("doi =") {
        "(DOI)"
    } else if bibtex.trim().contains("arXiv") || bibtex.trim().contains("arxiv") {
        "(arXiv)"
    } else {
        "(Generated)"
    };

    if copy_to_clipboard(&bibtex) {
        let _ = sender.send(crate::AppEvent::Toast(format!(
            "Citation copied to clipboard {source}"
        )));
    } else {
        let _ = sender.send(crate::AppEvent::Toast(
            "Failed to write to clipboard".into(),
        ));
    }
}

pub async fn fetch_and_insert_project_citation(
    metadata: CitationMetadata,
    bib_path: PathBuf,
    sender: tokio::sync::mpsc::UnboundedSender<crate::AppEvent>,
) {
    let (key, bibtex) = fetch_bibtex(&metadata).await;
    let title = metadata.title.clone();
    let _ = sender.send(crate::AppEvent::ProjectCitationReady {
        key,
        bibtex,
        title,
        bib_path,
    });
}

pub fn generate_bibtex(metadata: &CitationMetadata) -> (String, String) {
    let mut bibtex = String::new();
    let key = best_effort_key(metadata);
    let _ = writeln!(bibtex, "@article{{{key},");
    let _ = writeln!(bibtex, "  title={{{}}},", metadata.title);
    if !metadata.authors.is_empty() {
        let _ = writeln!(bibtex, "  author={{{}}},", metadata.authors);
    }
    if let Some(year) = metadata.year.as_deref().filter(|y| !y.is_empty()) {
        let _ = writeln!(bibtex, "  year={{{year}}},");
    }
    if let Some(journal) = metadata.journal_ref.as_deref().filter(|j| !j.is_empty()) {
        let _ = writeln!(bibtex, "  journal={{{journal}}},");
    } else if let Some(arxiv_id) = metadata.arxiv_id.as_deref().and_then(extract_arxiv) {
        let _ = writeln!(bibtex, "  journal={{arXiv preprint arXiv:{arxiv_id}}},");
    }
    if let Some(doi) = metadata.doi.as_deref().and_then(extract_doi) {
        let _ = writeln!(bibtex, "  doi={{{doi}}},");
    }
    bibtex.push_str("}\n");
    (key, bibtex)
}

fn extract_arxiv(s: &str) -> Option<String> {
    let mut s = s.trim();
    if s.starts_with("http://arxiv.org/abs/") || s.starts_with("https://arxiv.org/abs/") {
        s = s.split("/abs/").last().unwrap_or(s);
    } else if s.starts_with("arxiv:") {
        s = &s[6..];
    }
    let s = s.split('v').next().unwrap_or(s);
    if s.chars().all(|c| c.is_ascii_digit() || c == '.') {
        Some(s.to_string())
    } else {
        None
    }
}

fn extract_doi(s: &str) -> Option<String> {
    let s = s.trim();
    let s = if let Some(idx) = s.find("doi.org/") {
        &s[idx + 8..]
    } else if let Some(idx) = s.find("doi:") {
        &s[idx + 4..]
    } else {
        s
    };
    if s.starts_with("10.") {
        Some(s.to_string())
    } else {
        None
    }
}

fn best_effort_key(metadata: &CitationMetadata) -> String {
    let mut key = String::new();
    if let Some(first_author) = metadata.authors.split(" and ").next() {
        let last_name = first_author
            .split_whitespace()
            .last()
            .unwrap_or(first_author);
        key.push_str(
            &last_name
                .to_lowercase()
                .replace(|c: char| !c.is_alphanumeric(), ""),
        );
    } else {
        key.push_str("paper");
    }
    if let Some(year) = &metadata.year {
        key.push_str(year);
    }
    let first_title_word = metadata
        .title
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase()
        .replace(|c: char| !c.is_alphanumeric(), "");
    if !first_title_word.is_empty() {
        key.push_str(&first_title_word);
    }
    if key.is_empty() {
        "citation".to_string()
    } else {
        key
    }
}

fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Try wl-copy first (Wayland native, persists perfectly)
    if let Ok(mut child) = Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }

    // Try xclip (X11)
    if let Ok(mut child) = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }

    // Try pbcopy (macOS)
    if let Ok(mut child) = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }

    // Fallback to arboard
    let clip_mutex =
        CLIPBOARD.get_or_init(|| std::sync::Mutex::new(arboard::Clipboard::new().ok()));

    if let Ok(mut lock) = clip_mutex.lock()
        && let Some(clipboard) = lock.as_mut()
    {
        return clipboard.set_text(text).is_ok();
    }
    false
}
