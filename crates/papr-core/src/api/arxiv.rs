//! Asynchronous client for the public arXiv Atom API.

use chrono::{DateTime, Utc};
use quick_xml::de::from_str;
use reqwest::{Client, RequestBuilder, Response, StatusCode, Url, header::RETRY_AFTER};
use serde::Deserialize;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::sync::Mutex;

use crate::models::RemotePaper;

const API_URL: &str = "https://export.arxiv.org/api/query";
/// arXiv asks API clients to pause for three seconds between consecutive calls.
const REQUEST_INTERVAL: Duration = Duration::from_secs(3);
/// A rate-limit response without a `Retry-After` header needs a meaningful cooldown.
const RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
const LATEST_QUERY: &str = "cat:cs.* OR cat:physics.* OR cat:math.* OR cat:stat.* OR \
    cat:q-bio.* OR cat:astro-ph.* OR cat:cond-mat.* OR cat:gr-qc OR cat:quant-ph";

/// Errors returned while querying or decoding arXiv.
#[derive(Debug, Error)]
pub enum ArxivError {
    /// The API URL could not be constructed.
    #[error("invalid arXiv API URL: {0}")]
    Url(#[from] url::ParseError),
    /// The network request failed.
    #[error("arXiv request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// The Atom response was malformed.
    #[error("invalid arXiv response: {0}")]
    Decode(#[from] quick_xml::DeError),
    /// A response timestamp was invalid.
    #[error("invalid arXiv timestamp: {0}")]
    Timestamp(#[from] chrono::ParseError),
    /// A CPU-bound response processing task did not complete.
    #[error("arXiv response processing task failed: {0}")]
    Background(String),
}

/// Configured, reusable arXiv API client.
#[derive(Debug, Clone)]
pub struct ArxivClient {
    client: Client,
    endpoint: Url,
    next_request_at: Arc<Mutex<Instant>>,
}

/// One ranked page in an incremental arXiv candidate search.
#[derive(Debug, Clone)]
pub struct RankedCandidatePage {
    /// All successfully loaded candidates, ranked globally so far.
    pub papers: Vec<RemotePaper>,
    /// Offset of the next batch, or `None` when the result set is complete.
    pub next_start: Option<u16>,
}

impl ArxivClient {
    /// Build a client with a descriptive user agent and request timeout.
    ///
    /// # Errors
    ///
    /// Returns an error if the endpoint or HTTP client cannot be constructed.
    pub fn new() -> Result<Self, ArxivError> {
        Ok(Self {
            client: Client::builder()
                .user_agent(concat!("papr/", env!("CARGO_PKG_VERSION")))
                .timeout(std::time::Duration::from_secs(20))
                .build()?,
            endpoint: Url::parse(API_URL)?,
            next_request_at: Arc::new(Mutex::new(Instant::now())),
        })
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: &str) -> Result<Self, ArxivError> {
        let mut client = Self::new()?;
        client.endpoint = Url::parse(endpoint)?;
        Ok(client)
    }

    /// Search arXiv using word-based matching.
    ///
    /// The optional prefixes `author:`, `title:`, `abstract:`, and `category:`
    /// restrict a query to one arXiv field.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or Atom content is malformed.
    pub async fn search(&self, query: &str, limit: u16) -> Result<Vec<RemotePaper>, ArxivError> {
        if let Some(arxiv_id) = parse_arxiv_id(query) {
            if let Some(paper) = self.get_by_id(&arxiv_id).await? {
                return Ok(vec![paper]);
            }
        }
        let papers = self
            .query(&build_search_query(query), 0, limit, "relevance")
            .await?;
        rank_papers(query.to_owned(), papers).await
    }

    /// Search a larger arXiv candidate set in batches, then rank it once globally.
    ///
    /// The returned vector is suitable for client-side pagination: its order is final
    /// until the caller starts a new search.
    pub async fn search_ranked_candidates(
        &self,
        query: &str,
        candidate_limit: u16,
        batch_size: u16,
    ) -> Result<Vec<RemotePaper>, ArxivError> {
        if let Some(arxiv_id) = parse_arxiv_id(query) {
            if let Some(paper) = self.get_by_id(&arxiv_id).await? {
                return Ok(vec![paper]);
            }
        }

        let search_query = build_search_query(query);
        let total = candidate_limit.clamp(1, 1_000);
        let batch_size = batch_size.clamp(1, 100);
        let mut papers = Vec::with_capacity(usize::from(total));
        let mut start = 0_u16;

        while start < total {
            let requested = batch_size.min(total - start);
            let mut batch = self
                .query(&search_query, start, requested, "relevance")
                .await?;
            let received = u16::try_from(batch.len()).unwrap_or(u16::MAX);
            papers.append(&mut batch);
            if received < requested {
                break;
            }
            start = start.saturating_add(requested);
        }

        rank_papers(query.to_owned(), papers).await
    }

    /// Search candidate batches and publish ranked snapshots before the final batch.
    ///
    /// Each snapshot is ranked against every candidate received so far. The final
    /// batch is ranked once, for the returned authoritative result suitable for
    /// caching and pagination.
    pub async fn search_ranked_candidates_incremental<F>(
        &self,
        query: &str,
        candidate_limit: u16,
        batch_size: u16,
        mut on_batch: F,
    ) -> Result<Vec<RemotePaper>, ArxivError>
    where
        F: FnMut(Vec<RemotePaper>),
    {
        if let Some(arxiv_id) = parse_arxiv_id(query) {
            if let Some(paper) = self.get_by_id(&arxiv_id).await? {
                return Ok(vec![paper]);
            }
        }

        let search_query = build_search_query(query);
        let total = candidate_limit.clamp(1, 1_000);
        let batch_size = batch_size.clamp(1, 100);
        let mut papers = Vec::with_capacity(usize::from(total));
        let mut start = 0_u16;

        while start < total {
            let requested = batch_size.min(total - start);
            let mut batch = self
                .query(&search_query, start, requested, "relevance")
                .await?;
            let received = u16::try_from(batch.len()).unwrap_or(u16::MAX);
            papers.append(&mut batch);
            let final_batch = received < requested || start.saturating_add(requested) >= total;
            if !final_batch {
                on_batch(rank_papers(query.to_owned(), papers.clone()).await?);
            }
            if final_batch {
                break;
            }
            start = start.saturating_add(requested);
        }

        rank_papers(query.to_owned(), papers).await
    }

    /// Fetch one candidate page and rank it with all previously loaded pages.
    ///
    /// Callers retain the returned page state and can retry the same `start` offset
    /// without losing already loaded papers.
    pub async fn search_ranked_candidate_page(
        &self,
        query: &str,
        existing: &[RemotePaper],
        start: u16,
        candidate_limit: u16,
        batch_size: u16,
    ) -> Result<RankedCandidatePage, ArxivError> {
        if start == 0 {
            if let Some(arxiv_id) = parse_arxiv_id(query) {
                if let Some(paper) = self.get_by_id(&arxiv_id).await? {
                    return Ok(RankedCandidatePage {
                        papers: vec![paper],
                        next_start: None,
                    });
                }
            }
        }

        let total = candidate_limit.clamp(1, 1_000);
        if start >= total {
            return Ok(RankedCandidatePage {
                papers: rank_papers(query.to_owned(), existing.to_vec()).await?,
                next_start: None,
            });
        }
        let requested = batch_size.clamp(1, 100).min(total - start);
        let mut papers = existing.to_vec();
        let mut batch = self
            .query(&build_search_query(query), start, requested, "relevance")
            .await?;
        let received = u16::try_from(batch.len()).unwrap_or(u16::MAX);
        papers.append(&mut batch);
        let next_start = (received == requested && start.saturating_add(requested) < total)
            .then_some(start.saturating_add(requested));

        Ok(RankedCandidatePage {
            papers: rank_papers(query.to_owned(), papers).await?,
            next_start,
        })
    }

    /// Load the newest submissions across arXiv for the dashboard.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or Atom content is malformed.
    pub async fn latest(&self, limit: u16) -> Result<Vec<RemotePaper>, ArxivError> {
        self.query(LATEST_QUERY, 0, limit, "submittedDate").await
    }

    /// Search arXiv and return the newest matching submissions first.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or Atom content is malformed.
    pub async fn search_latest(
        &self,
        query: &str,
        limit: u16,
    ) -> Result<Vec<RemotePaper>, ArxivError> {
        if let Some(arxiv_id) = parse_arxiv_id(query) {
            if let Some(paper) = self.get_by_id(&arxiv_id).await? {
                return Ok(vec![paper]);
            }
        }
        self.query(&build_search_query(query), 0, limit, "submittedDate")
            .await
    }

    /// Fetch metadata for a specific arXiv ID.
    ///
    /// # Errors
    /// Returns an error when the request fails or Atom content is malformed.
    pub async fn get_by_id(&self, id: &str) -> Result<Option<RemotePaper>, ArxivError> {
        let Some(id) = parse_arxiv_id(id) else {
            return Ok(None);
        };
        let response = self
            .send(
                self.client
                    .get(self.endpoint.clone())
                    .query(&[("id_list", id)]),
            )
            .await?
            .error_for_status()?
            .text()
            .await?;
        let mut papers = parse_response(response).await?;
        Ok(papers.pop())
    }

    async fn query(
        &self,
        search_query: &str,
        start: u16,
        limit: u16,
        sort_by: &str,
    ) -> Result<Vec<RemotePaper>, ArxivError> {
        let response = self
            .send(self.client.get(self.endpoint.clone()).query(&[
                ("search_query", search_query.to_owned()),
                ("start", start.to_string()),
                ("max_results", limit.clamp(1, 100).to_string()),
                ("sortBy", sort_by.to_owned()),
                ("sortOrder", "descending".to_owned()),
            ]))
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_response(response).await
    }

    async fn send(&self, request: RequestBuilder) -> Result<Response, ArxivError> {
        self.wait_for_request_slot().await;
        let response = request.send().await?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            self.defer_after_rate_limit(retry_after(&response)).await;
        }
        Ok(response)
    }

    async fn wait_for_request_slot(&self) {
        let scheduled_at = {
            let mut next_request_at = self.next_request_at.lock().await;
            let now = Instant::now();
            let scheduled_at = (*next_request_at).max(now);
            *next_request_at = scheduled_at + REQUEST_INTERVAL;
            scheduled_at
        };
        tokio::time::sleep_until(tokio::time::Instant::from_std(scheduled_at)).await;
    }

    async fn defer_after_rate_limit(&self, cooldown: Duration) {
        let mut next_request_at = self.next_request_at.lock().await;
        *next_request_at = (*next_request_at).max(Instant::now() + cooldown);
    }
}

fn retry_after(response: &Response) -> Duration {
    response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(RATE_LIMIT_COOLDOWN)
}

async fn parse_response(response: String) -> Result<Vec<RemotePaper>, ArxivError> {
    tokio::task::spawn_blocking(move || parse_feed(&response))
        .await
        .map_err(|error| ArxivError::Background(error.to_string()))?
}

async fn rank_papers(
    input: String,
    mut papers: Vec<RemotePaper>,
) -> Result<Vec<RemotePaper>, ArxivError> {
    tokio::task::spawn_blocking(move || {
        rank_by_query_relevance(&input, &mut papers);
        papers
    })
    .await
    .map_err(|error| ArxivError::Background(error.to_string()))
}

#[derive(Debug, Deserialize)]
struct Feed {
    #[serde(rename = "entry", default)]
    entries: Vec<Entry>,
    /// Absorb feed-level `<link>` elements so `quick_xml` does not report
    /// "duplicate field `link`" when the real Atom feed contains them.
    #[serde(rename = "link", default)]
    #[allow(dead_code)]
    _links: Vec<Link>,
    /// Feed title returned by arXiv (e.g. "`ArXiv` Query: ...").
    #[serde(default)]
    #[allow(dead_code)]
    title: Option<String>,
    /// Feed identifier.
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    /// Feed-level update timestamp.
    #[serde(default)]
    #[allow(dead_code)]
    updated: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    id: String,
    title: String,
    summary: String,
    published: String,
    updated: String,
    #[serde(rename = "author", default)]
    authors: Vec<Author>,
    #[serde(rename = "category", default)]
    categories: Vec<Category>,
    #[serde(rename = "link", default)]
    links: Vec<Link>,
    /// arXiv may emit more than one DOI for a paper and its erratum. Keep all
    /// values while decoding so one such record cannot invalidate the feed.
    #[serde(rename = "doi", default)]
    dois: Vec<String>,
    #[serde(rename = "journal_ref")]
    journal_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Author {
    name: String,
}

#[derive(Debug, Deserialize)]
struct Category {
    #[serde(rename = "@term")]
    term: String,
}

#[derive(Debug, Deserialize)]
struct Link {
    #[serde(rename = "@href")]
    href: String,
    #[serde(rename = "@title")]
    title: Option<String>,
    #[serde(rename = "@type")]
    media_type: Option<String>,
}

fn parse_feed(xml: &str) -> Result<Vec<RemotePaper>, ArxivError> {
    let feed: Feed = from_str(xml)?;
    feed.entries
        .into_iter()
        .map(|entry| {
            let pdf_url = entry
                .links
                .iter()
                .find(|link| {
                    link.title.as_deref() == Some("pdf")
                        || link.media_type.as_deref() == Some("application/pdf")
                })
                .map(|link| link.href.clone());
            Ok(RemotePaper {
                id: entry.id,
                title: normalize(&entry.title),
                authors: entry
                    .authors
                    .into_iter()
                    .map(|author| author.name)
                    .collect(),
                abstract_text: normalize(&entry.summary),
                published: DateTime::parse_from_rfc3339(&entry.published)?.with_timezone(&Utc),
                updated: DateTime::parse_from_rfc3339(&entry.updated)?.with_timezone(&Utc),
                categories: entry
                    .categories
                    .into_iter()
                    .map(|category| category.term)
                    .collect(),
                pdf_url,
                // `RemotePaper` has one DOI field. Prefer arXiv's first
                // non-empty value, which is the paper's primary DOI when
                // additional values describe corrections or related work.
                doi: entry.dois.into_iter().find(|doi| !doi.trim().is_empty()),
                journal_ref: entry.journal_ref,
            })
        })
        .collect()
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn build_search_query(input: &str) -> String {
    let input = input.trim();
    if let Some((prefix, value)) = input.split_once(':') {
        let value = value.trim();
        let field = match prefix.trim().to_ascii_lowercase().as_str() {
            "author" | "au" => Some("au"),
            "title" | "ti" => Some("ti"),
            "abstract" | "abs" => Some("abs"),
            "category" | "cat" => Some("cat"),
            _ => None,
        };
        if let Some(field) = field {
            return field_query(field, value);
        }
    }
    if looks_like_category(input) {
        return format!("cat:{input}");
    }
    if input.starts_with("10.") {
        return format!("all:\"{}\"", escape_phrase(input));
    }
    word_query("all", input)
}

fn rank_by_query_relevance(input: &str, papers: &mut [RemotePaper]) {
    let search = QueryTerms::from_input(input);
    if search.terms.is_empty() || search.is_category || search.is_doi {
        return;
    }
    papers.sort_by(|left, right| {
        score_paper(&search, right)
            .cmp(&score_paper(&search, left))
            .then_with(|| right.published.cmp(&left.published))
    });
}

fn score_paper(search: &QueryTerms, paper: &RemotePaper) -> usize {
    let title = paper.title.to_lowercase();
    let authors = paper.authors.join(" ").to_lowercase();
    let abstract_text = paper.abstract_text.to_lowercase();
    let categories = paper.categories.join(" ").to_lowercase();
    let all_text = format!("{title} {authors} {abstract_text} {categories}");

    let title_matches = search
        .terms
        .iter()
        .filter(|term| title.contains(term.as_str()))
        .count();
    let author_matches = search
        .terms
        .iter()
        .filter(|term| authors.contains(term.as_str()))
        .count();
    let abstract_matches = search
        .terms
        .iter()
        .filter(|term| abstract_text.contains(term.as_str()))
        .count();
    let category_matches = search
        .terms
        .iter()
        .filter(|term| categories.contains(term.as_str()))
        .count();

    let all_terms_in_title = title_matches == search.terms.len();
    let all_terms_in_authors = author_matches == search.terms.len();
    let all_terms_anywhere = search
        .terms
        .iter()
        .all(|term| all_text.contains(term.as_str()));
    let phrase_in_title = search
        .phrase
        .as_deref()
        .is_some_and(|phrase| title.contains(phrase));
    let phrase_in_authors = search
        .phrase
        .as_deref()
        .is_some_and(|phrase| authors.contains(phrase));
    let phrase_in_abstract = search
        .phrase
        .as_deref()
        .is_some_and(|phrase| abstract_text.contains(phrase));

    usize::from(phrase_in_authors) * 1_000
        + usize::from(phrase_in_title) * 900
        + usize::from(all_terms_in_authors) * 650
        + usize::from(all_terms_in_title) * 600
        + usize::from(all_terms_anywhere) * 250
        + usize::from(phrase_in_abstract) * 75
        + author_matches * 40
        + title_matches * 35
        + abstract_matches * 12
        + category_matches * 8
}

fn field_query(field: &str, value: &str) -> String {
    if field == "cat" && looks_like_category(value) {
        format!("cat:{value}")
    } else {
        word_query(field, value)
    }
}

fn word_query(field: &str, value: &str) -> String {
    let terms = tokenize_search_terms(value);
    if terms.is_empty() {
        return format!("{field}:\"{}\"", escape_phrase(value.trim()));
    }
    terms
        .into_iter()
        .map(|term| format!("{field}:\"{}\"", escape_phrase(&term)))
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn escape_phrase(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[derive(Debug)]
struct QueryTerms {
    terms: Vec<String>,
    phrase: Option<String>,
    is_category: bool,
    is_doi: bool,
}

impl QueryTerms {
    fn from_input(input: &str) -> Self {
        let input = input.trim();
        let query_text = input
            .split_once(':')
            .and_then(|(prefix, value)| {
                matches!(
                    prefix.trim().to_ascii_lowercase().as_str(),
                    "author" | "au" | "title" | "ti" | "abstract" | "abs"
                )
                .then_some(value.trim())
            })
            .unwrap_or(input);
        let terms = tokenize_search_terms(query_text)
            .into_iter()
            .map(|term| term.to_lowercase())
            .collect::<Vec<_>>();
        Self {
            phrase: (terms.len() > 1).then(|| terms.join(" ")),
            terms,
            is_category: looks_like_category(input)
                || input.split_once(':').is_some_and(|(prefix, value)| {
                    matches!(
                        prefix.trim().to_ascii_lowercase().as_str(),
                        "category" | "cat"
                    ) && looks_like_category(value.trim())
                }),
            is_doi: input.starts_with("10."),
        }
    }
}

fn tokenize_search_terms(value: &str) -> Vec<String> {
    value
        .split(|character: char| {
            !character.is_alphanumeric() && character != '-' && character != '.'
        })
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect()
}

fn looks_like_category(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(char::is_whitespace)
        && value.contains(['.', '-'])
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
}

fn clean_arxiv_id(s: &str) -> String {
    let s = s.trim().to_ascii_lowercase();
    let clean = if let Some(idx) = s.find("/abs/") {
        &s[idx + 5..]
    } else if let Some(idx) = s.find("/pdf/") {
        let after = &s[idx + 5..];
        after.strip_suffix(".pdf").unwrap_or(after)
    } else {
        &s
    };
    clean.trim().to_string()
}

fn parse_arxiv_id(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    let clean = clean_arxiv_id(&s);
    let (base, version) = if let Some(v_idx) = clean.rfind('v') {
        let (b, v) = clean.split_at(v_idx);
        let v_suffix = &v[1..];
        if !v_suffix.is_empty() && v_suffix.chars().all(|c| c.is_ascii_digit()) {
            (b, Some(v.to_string()))
        } else {
            (clean.as_str(), None)
        }
    } else {
        (clean.as_str(), None)
    };

    // Modern format: "YYMM.NNNN" or "YYMM.NNNNN"
    if base.len() >= 9 && base.len() <= 10 {
        let parts: Vec<&str> = base.split('.').collect();
        if parts.len() == 2 {
            let yymm = parts[0];
            let nnnn = parts[1];
            if yymm.len() == 4
                && yymm.chars().all(|c| c.is_ascii_digit())
                && (nnnn.len() == 4 || nnnn.len() == 5)
                && nnnn.chars().all(|c| c.is_ascii_digit())
            {
                let mut normalized = base.to_string();
                if let Some(v) = version {
                    normalized.push_str(&v);
                }
                return Some(normalized);
            }
        }
    }

    // Legacy format: "archive/YYMMNNN" or "subject-class/YYMMNNN"
    if let Some(slash_idx) = base.find('/') {
        let (cat, num) = base.split_at(slash_idx);
        let num = &num[1..];
        if num.len() == 7 && num.chars().all(|c| c.is_ascii_digit()) {
            if !cat.is_empty()
                && cat
                    .chars()
                    .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '.')
            {
                let mut normalized = base.to_string();
                if let Some(v) = version {
                    normalized.push_str(&v);
                }
                return Some(normalized);
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        ArxivClient, build_search_query, parse_arxiv_id, parse_feed, rank_by_query_relevance,
    };

    const FEED: &str = r#"<?xml version="1.0" encoding="utf-8"?>
    <feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
      <link href="http://arxiv.org/api/query?search_query=all:test" rel="self" type="application/atom+xml"/>
      <title>ArXiv Query: search_query=all:test</title>
      <id>http://arxiv.org/api/query?search_query=all:test</id>
      <updated>2025-01-02T00:00:00-05:00</updated>
      <entry><id>http://arxiv.org/abs/2501.00001v1</id><updated>2025-01-02T00:00:00Z</updated>
      <published>2025-01-01T00:00:00Z</published><title>A  Useful
      Paper</title><summary>An abstract.</summary>
      <author><name>Ada Lovelace</name></author><category term="cs.LG"/>
      <link href="http://arxiv.org/abs/2501.00001v1" rel="alternate" type="text/html"/>
      <link title="pdf" href="https://arxiv.org/pdf/2501.00001" type="application/pdf"/>
      <arxiv:doi>10.1000/example</arxiv:doi><arxiv:doi>10.1000/example-erratum</arxiv:doi>
      <arxiv:journal_ref>Example Journal</arxiv:journal_ref></entry>
    </feed>"#;

    #[test]
    fn parses_atom_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let papers = parse_feed(FEED)?;
        assert_eq!(papers.len(), 1);
        assert_eq!(papers[0].title, "A Useful Paper");
        assert_eq!(papers[0].authors, ["Ada Lovelace"]);
        assert_eq!(papers[0].categories, ["cs.LG"]);
        assert_eq!(papers[0].doi.as_deref(), Some("10.1000/example"));
        assert!(papers[0].pdf_url.is_some());
        Ok(())
    }

    #[test]
    fn parses_entry_links_split_by_other_metadata() -> Result<(), Box<dyn std::error::Error>> {
        let feed = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom" xmlns:arxiv="http://arxiv.org/schemas/atom">
          <entry>
            <id>http://arxiv.org/abs/2501.00001v1</id>
            <updated>2025-01-02T00:00:00Z</updated>
            <published>2025-01-01T00:00:00Z</published>
            <title>A Useful Paper</title>
            <summary>An abstract.</summary>
            <author><name>Ada Lovelace</name></author>
            <link href="http://arxiv.org/abs/2501.00001v1" rel="alternate" type="text/html"/>
            <arxiv:primary_category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
            <category term="cs.LG" scheme="http://arxiv.org/schemas/atom"/>
            <link title="pdf" href="https://arxiv.org/pdf/2501.00001" type="application/pdf"/>
          </entry>
        </feed>"#;

        let papers = parse_feed(feed)?;

        assert_eq!(
            papers[0].pdf_url.as_deref(),
            Some("https://arxiv.org/pdf/2501.00001")
        );
        Ok(())
    }

    #[test]
    fn test_endpoint_constructor_rejects_bad_url() {
        assert!(ArxivClient::with_endpoint("not a url").is_err());
    }

    #[test]
    fn rejects_legacy_ids_without_an_archive_prefix() {
        assert_eq!(parse_arxiv_id("0211069"), None);
        assert_eq!(
            parse_arxiv_id("hep-th/0211069v2").as_deref(),
            Some("hep-th/0211069v2")
        );
    }

    #[test]
    fn multiword_search_matches_all_words_across_arxiv_fields() {
        assert_eq!(
            build_search_query("Prayush Kumar"),
            "all:\"Prayush\" AND all:\"Kumar\""
        );
        assert_eq!(
            build_search_query("machine learning in gravitational waves"),
            "all:\"machine\" AND all:\"learning\" AND all:\"in\" AND all:\"gravitational\" AND all:\"waves\""
        );
    }

    #[test]
    fn explicit_fields_and_categories_are_preserved() {
        assert_eq!(
            build_search_query("author: Prayush Kumar"),
            "au:\"Prayush\" AND au:\"Kumar\""
        );
        assert_eq!(
            build_search_query("title: gravitational waves"),
            "ti:\"gravitational\" AND ti:\"waves\""
        );
        assert_eq!(build_search_query("category: gr-qc"), "cat:gr-qc");
        assert_eq!(build_search_query("cs.LG"), "cat:cs.LG");
    }

    #[test]
    fn query_syntax_is_tokenized_before_building_arxiv_query() {
        assert_eq!(
            build_search_query("title: waves\" OR all:*"),
            "ti:\"waves\" AND ti:\"OR\" AND ti:\"all\""
        );
    }

    #[test]
    fn relevance_ranking_boosts_title_and_author_matches_without_filtering()
    -> Result<(), Box<dyn std::error::Error>> {
        let template = parse_feed(FEED)?
            .into_iter()
            .next()
            .ok_or("fixture did not contain a paper")?;
        let mut author_match = template.clone();
        author_match.title = "Gravitational-wave inference".into();
        author_match.authors = vec!["Prayush Kumar".into()];
        let mut title_match = template.clone();
        title_match.title = "Machine learning for gravitational waves".into();
        title_match.authors = vec!["Another Researcher".into()];
        let mut abstract_only = template;
        abstract_only.title = "Cyber bullying detection".into();
        abstract_only.authors = vec!["Another Researcher".into()];
        abstract_only.abstract_text =
            "We compare with work by Prayush Kumar on gravitational waves.".into();

        let mut papers = vec![abstract_only, title_match, author_match];
        rank_by_query_relevance("Prayush Kumar", &mut papers);

        assert_eq!(papers.len(), 3);
        assert_eq!(papers[0].authors, ["Prayush Kumar"]);
        assert_eq!(papers[1].title, "Cyber bullying detection");
        Ok(())
    }
}
