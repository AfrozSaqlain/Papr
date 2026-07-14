use papr_core::models::CitationMetadata;

pub async fn fetch_and_copy_citation(
    metadata: CitationMetadata,
    sender: tokio::sync::mpsc::UnboundedSender<crate::AppEvent>,
) {
    if let Some(doi) = metadata.doi.as_deref().and_then(extract_doi) {
        if let Ok(client) = reqwest::Client::new()
            .get(format!("https://doi.org/{}", doi))
            .header("Accept", "application/x-bibtex")
            .send()
            .await
        {
            if client.status().is_success() {
                if let Ok(bibtex) = client.text().await {
                    if copy_to_clipboard(&bibtex) {
                        let _ = sender.send(crate::AppEvent::Toast("Citation copied to clipboard (DOI)".into()));
                        return;
                    }
                }
            }
        }
    }

    let arxiv_id_opt = metadata.arxiv_id.as_deref().and_then(extract_arxiv);
    if let Some(arxiv_id) = &arxiv_id_opt {
        if let Ok(client) = reqwest::Client::new()
            .get(format!("https://arxiv.org/bibtex/{}", arxiv_id))
            .send()
            .await
        {
            if client.status().is_success() {
                if let Ok(bibtex) = client.text().await {
                    if copy_to_clipboard(&bibtex) {
                        let _ = sender.send(crate::AppEvent::Toast("Citation copied to clipboard (arXiv)".into()));
                        return;
                    }
                }
            }
        }
    }

    // Best-effort local generation
    let mut bibtex = String::new();
    let key = best_effort_key(&metadata);
    bibtex.push_str(&format!("@article{{{key},\n"));
    bibtex.push_str(&format!("  title={{{}}},\n", metadata.title));
    if !metadata.authors.is_empty() {
        bibtex.push_str(&format!("  author={{{}}},\n", metadata.authors));
    }
    if let Some(year) = metadata.year.as_deref().filter(|y| !y.is_empty()) {
        bibtex.push_str(&format!("  year={{{year}}},\n"));
    }
    if let Some(journal) = metadata.journal_ref.as_deref().filter(|j| !j.is_empty()) {
        bibtex.push_str(&format!("  journal={{{journal}}},\n"));
    } else if let Some(arxiv_id) = arxiv_id_opt.as_deref() {
        bibtex.push_str(&format!("  journal={{arXiv preprint arXiv:{arxiv_id}}},\n"));
    }
    if let Some(doi) = metadata.doi.as_deref().and_then(extract_doi) {
        bibtex.push_str(&format!("  doi={{{doi}}},\n"));
    }
    bibtex.push_str("}\n");

    if copy_to_clipboard(&bibtex) {
        let _ = sender.send(crate::AppEvent::Toast("Citation copied to clipboard (Generated)".into()));
    } else {
        let _ = sender.send(crate::AppEvent::Toast("Failed to write to clipboard".into()));
    }
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
        let last_name = first_author.split_whitespace().last().unwrap_or(first_author);
        key.push_str(&last_name.to_lowercase().replace(|c: char| !c.is_alphanumeric(), ""));
    } else {
        key.push_str("paper");
    }
    if let Some(year) = &metadata.year {
        key.push_str(year);
    }
    let first_title_word = metadata.title.split_whitespace().next().unwrap_or("").to_lowercase().replace(|c: char| !c.is_alphanumeric(), "");
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
    use std::process::{Command, Stdio};
    use std::io::Write;

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
        if child.wait().map(|s| s.success()).unwrap_or(false) {
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
        if child.wait().map(|s| s.success()).unwrap_or(false) {
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
        if child.wait().map(|s| s.success()).unwrap_or(false) {
            return true;
        }
    }

    // Fallback to arboard
    static CLIPBOARD: std::sync::OnceLock<std::sync::Mutex<Option<arboard::Clipboard>>> = std::sync::OnceLock::new();
    
    let clip_mutex = CLIPBOARD.get_or_init(|| {
        std::sync::Mutex::new(arboard::Clipboard::new().ok())
    });

    if let Ok(mut lock) = clip_mutex.lock() {
        if let Some(clipboard) = lock.as_mut() {
            return clipboard.set_text(text).is_ok();
        }
    }
    false
}
