//! Crossref API client for fetching paper metadata by DOI.

use crate::models::RemotePaper;
use chrono::{NaiveDate, TimeZone, Utc};
use reqwest::{Client, Url};
use serde::Deserialize;

/// Represents an error returned by the Crossref API.
#[derive(Debug, thiserror::Error)]
pub enum CrossrefError {
    /// A network or HTTP request error occurred.
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),
    /// Failed to parse the JSON response from Crossref.
    #[error("Failed to parse response: {0}")]
    Parse(#[from] serde_json::Error),
    /// The requested DOI was not found in Crossref.
    #[error("Work not found")]
    NotFound,
}

/// A client for fetching scholarly metadata from the Crossref API.
#[derive(Clone)]
pub struct CrossrefClient {
    client: Client,
    endpoint: Url,
}

impl CrossrefClient {
    /// Creates a new `CrossrefClient` with default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent("papr/0.1.0")
                .build()
                .unwrap_or_default(),
            endpoint: Url::parse("https://api.crossref.org/works/").expect("valid url"),
        }
    }

    /// Fetch metadata for a specific DOI.
    ///
    /// # Errors
    /// Returns an error when the request fails or JSON content is malformed.
    pub async fn get_by_doi(&self, doi: &str) -> Result<Option<RemotePaper>, CrossrefError> {
        let url = self
            .endpoint
            .join(doi)
            .map_err(|_| CrossrefError::NotFound)?;
        let response = self.client.get(url).send().await?;
        if response.status() == 404 {
            return Ok(None);
        }
        let response = response.error_for_status()?;

        #[derive(Deserialize)]
        struct CrossrefResponse {
            message: CrossrefWork,
        }

        #[derive(Deserialize)]
        struct CrossrefWork {
            #[serde(rename = "DOI")]
            doi: Option<String>,
            title: Option<Vec<String>>,
            author: Option<Vec<CrossrefAuthor>>,
            #[serde(rename = "abstract")]
            abstract_text: Option<String>,
            published: Option<CrossrefDate>,
            created: Option<CrossrefDate>,
            deposited: Option<CrossrefDate>,
        }

        #[derive(Deserialize)]
        struct CrossrefAuthor {
            given: Option<String>,
            family: Option<String>,
        }

        #[derive(Deserialize)]
        struct CrossrefDate {
            #[serde(rename = "date-parts")]
            date_parts: Option<Vec<Vec<u32>>>,
        }

        let body = response.bytes().await?;
        let res: CrossrefResponse = serde_json::from_slice(&body)?;
        let work = res.message;

        let title = work
            .title
            .and_then(|mut t| t.pop())
            .unwrap_or_else(|| "Unknown Title".to_string());

        let authors = work
            .author
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| match (a.given, a.family) {
                (Some(g), Some(f)) => Some(format!("{} {}", g, f)),
                (None, Some(f)) => Some(f),
                (Some(g), None) => Some(g),
                _ => None,
            })
            .collect();

        // Abstract from crossref often has JATS XML tags like <jats:p>. We should clean them.
        let raw_abs = work.abstract_text.unwrap_or_default();
        let abstract_text = raw_abs
            .replace("<jats:p>", "")
            .replace("</jats:p>", "\n")
            .replace("<jats:sec>", "")
            .replace("</jats:sec>", "")
            .trim()
            .to_string();

        let mut date = Utc::now();
        if let Some(cd) = work.published.or(work.created).or(work.deposited) {
            if let Some(parts) = cd.date_parts {
                if let Some(p) = parts.first() {
                    let year = p.first().copied().unwrap_or(0);
                    let month = p.get(1).copied().unwrap_or(1);
                    let day = p.get(2).copied().unwrap_or(1);
                    if let Some(nd) = NaiveDate::from_ymd_opt(year as i32, month, day) {
                        if let Some(ndt) = nd.and_hms_opt(0, 0, 0) {
                            date = Utc.from_utc_datetime(&ndt);
                        }
                    }
                }
            }
        }

        Ok(Some(RemotePaper {
            id: work.doi.clone().unwrap_or_else(|| doi.to_string()),
            title,
            authors,
            abstract_text,
            published: date,
            updated: date,
            categories: vec![],
            pdf_url: None,
            doi: work.doi,
            journal_ref: None,
        }))
    }
}

impl Default for CrossrefClient {
    fn default() -> Self {
        Self::new()
    }
}
