//! Extensible completion primitives for source editors.

/// A completion item supplied by any editor completion source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompletionItem {
    /// Text inserted when the item is accepted.
    pub insert_text: String,
    /// Primary text displayed in the completion menu.
    pub label: String,
    /// Additional context displayed beside the label.
    pub detail: String,
}

/// A source of completions. Future label, glossary, and snippet sources can
/// implement this trait without changing the editor UI.
pub trait CompletionSource: Send + Sync {
    /// Return items relevant at `cursor` in `text`.
    fn complete(&self, text: &str, cursor: usize) -> Vec<CompletionItem>;
}

/// A compact bibliographic record suitable for completion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CitationEntry {
    /// BibTeX citation key.
    pub key: String,
    /// Author field as stored in BibTeX.
    pub author: String,
    /// Publication title.
    pub title: String,
    /// Publication year.
    pub year: String,
}

/// Citation completion source populated from a project's BibTeX files.
#[derive(Clone, Debug, Default)]
pub struct CitationSource {
    entries: Vec<CitationEntry>,
}

impl CitationSource {
    /// Construct a source from already-indexed bibliography records.
    #[must_use]
    pub fn new(entries: Vec<CitationEntry>) -> Self {
        Self { entries }
    }

    /// Return all indexed bibliography records (e.g. to build an "already added" set).
    #[must_use]
    pub fn all_entries(&self) -> &[CitationEntry] {
        &self.entries
    }

    /// Parse the useful fields from BibTeX. This deliberately accepts common
    /// hand-written BibTeX formatting rather than requiring a strict parser.
    #[must_use]
    pub fn parse_bibtex(input: &str) -> Vec<CitationEntry> {
        let mut entries = Vec::new();
        for chunk in input.split('@').skip(1) {
            let Some(open) = chunk.find(['{', '(']) else {
                continue;
            };
            let body = &chunk[open + 1..];
            let Some(comma) = body.find(',') else {
                continue;
            };
            let key = body[..comma].trim();
            if key.is_empty() || key.starts_with("comment") || key.starts_with("string") {
                continue;
            }
            let fields = &body[comma + 1..];
            let field = |name: &str| bib_field(fields, name);
            entries.push(CitationEntry {
                key: key.to_owned(),
                author: field("author").unwrap_or_default(),
                title: field("title").unwrap_or_default(),
                year: field("year").unwrap_or_default(),
            });
        }
        entries
    }
}

impl CompletionSource for CitationSource {
    fn complete(&self, text: &str, cursor: usize) -> Vec<CompletionItem> {
        let Some(query) = citation_query(text, cursor) else {
            return Vec::new();
        };
        let query = query.to_ascii_lowercase();
        let mut matches = self
            .entries
            .iter()
            .filter_map(|entry| {
                let haystack = format!(
                    "{} {} {} {}",
                    entry.key, entry.author, entry.title, entry.year
                )
                .to_ascii_lowercase();
                fuzzy_score(&query, &haystack).map(|score| (score, entry))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left, left_entry), (right, right_entry)| {
            right
                .cmp(left)
                .then_with(|| left_entry.key.cmp(&right_entry.key))
        });
        matches
            .into_iter()
            .take(8)
            .map(|(_, entry)| {
                let label = if entry.title.trim().is_empty() {
                    entry.key.clone()
                } else {
                    entry.title.trim().to_string()
                };
                CompletionItem {
                    insert_text: entry.key.clone(),
                    label,
                    detail: format!(
                        "{} — {} ({})",
                        first_author(&entry.author),
                        entry.title,
                        entry.year
                    ),
                }
            })
            .collect()
    }
}

/// Return the incomplete citation key immediately before `cursor`. Recognises
/// all commands whose name contains `cite`, including natbib and biblatex.
#[must_use]
pub fn citation_query(text: &str, cursor: usize) -> Option<&str> {
    let cursor = cursor.min(text.len());
    if !text.is_char_boundary(cursor) {
        return None;
    }
    let before = &text[..cursor];
    let brace = before.rfind('{')?;
    let command = before[..brace]
        .rsplit_once('\\')?
        .1
        .chars()
        .take_while(|c| c.is_ascii_alphabetic() || *c == '*')
        .collect::<String>();
    if command.is_empty() || !command.to_ascii_lowercase().contains("cite") {
        return None;
    }
    let current = &before[brace + 1..];
    if current.contains('}') {
        return None;
    }
    Some(current.rsplit(',').next().unwrap_or_default().trim_start())
}

fn bib_field(fields: &str, name: &str) -> Option<String> {
    let lower = fields.to_ascii_lowercase();
    let mut start = 0;
    loop {
        let found = lower[start..].find(name)? + start;
        let mut value_start = found + name.len();
        while lower
            .as_bytes()
            .get(value_start)
            .is_some_and(u8::is_ascii_whitespace)
        {
            value_start += 1;
        }
        if lower.as_bytes().get(value_start) == Some(&b'=') {
            start = value_start + 1;
            break;
        }
        start = found + name.len();
    }
    let value = fields[start..].trim_start();
    let trimmed_start = value.trim_start_matches(['{', '"']);
    let end = trimmed_start
        .find(['}', '"', ','])
        .unwrap_or(trimmed_start.len());
    Some(
        trimmed_start
            .get(..end)
            .unwrap_or(trimmed_start)
            .trim()
            .replace('\n', " "),
    )
}

fn first_author(author: &str) -> String {
    author
        .split(" and ")
        .next()
        .unwrap_or(author)
        .trim()
        .to_owned()
}

fn fuzzy_score(needle: &str, haystack: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let mut at = 0usize;
    let mut score = 0;
    for wanted in needle.chars() {
        let found = haystack[at..].find(wanted)?;
        score += if found == 0 { 4 } else { 1 };
        at += found + wanted.len_utf8();
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completes_current_key_in_multi_citation_command() {
        let source = CitationSource::new(vec![CitationEntry {
            key: "einstein1905".into(),
            author: "Albert Einstein".into(),
            title: "On a heuristic viewpoint".into(),
            year: "1905".into(),
        }]);
        assert_eq!(citation_query("\\cite{newton1687, eins", 22), Some("eins"));
        assert_eq!(
            source.complete("\\cite{newton1687, eins", 22)[0].insert_text,
            "einstein1905"
        );
    }
    #[test]
    fn parses_and_matches_metadata() {
        let source = CitationSource::new(CitationSource::parse_bibtex(
            "@article{einstein1905, author = {Albert Einstein}, title = {On the Electrodynamics of Moving Bodies}, year = {1905}}",
        ));
        assert_eq!(
            source.complete("\\autocite{moving", 16)[0].label,
            "On the Electrodynamics of Moving Bodies"
        );
        assert_eq!(
            source.complete("\\autocite{moving", 16)[0].insert_text,
            "einstein1905"
        );
        assert!(citation_query("plain text", 10).is_none());
    }
    #[test]
    fn falls_back_to_citation_key_when_title_is_missing() {
        let source = CitationSource::new(CitationSource::parse_bibtex(
            "@article{no_title_key, author = {Jane Doe}, year = {2020}}",
        ));
        let items = source.complete("\\cite{no_title", 14);
        assert_eq!(items[0].label, "no_title_key");
        assert_eq!(items[0].insert_text, "no_title_key");
    }
}
