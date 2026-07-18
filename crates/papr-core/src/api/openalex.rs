//! OpenAlex API client for DOI-based journal discovery.

use reqwest::{Client, Url};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
/// Errors returned by the OpenAlex API client.
pub enum OpenAlexError {
    #[error("HTTP request failed: {0}")]
    /// The request failed before a response was received.
    Request(#[from] reqwest::Error),
    #[error("failed to parse response: {0}")]
    /// The response was not valid OpenAlex JSON.
    Parse(#[from] serde_json::Error),
    #[error("work not found")]
    /// No work matched the DOI.
    NotFound,
}

/// Client for DOI lookups against OpenAlex.
#[derive(Clone)]
pub struct OpenAlexClient {
    client: Client,
    endpoint: Url,
}

impl OpenAlexClient {
    /// Build a client with the default OpenAlex endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent(concat!("papr/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
            endpoint: Url::parse("https://api.openalex.org/works/").expect("valid URL"),
        }
    }

    /// Fetch the canonical journal name for a DOI, if OpenAlex has one.
    pub async fn journal_by_doi(&self, doi: &str) -> Result<Option<String>, OpenAlexError> {
        let url = self
            .endpoint
            .join(&format!("https://doi.org/{doi}"))
            .map_err(|_| OpenAlexError::NotFound)?;
        let response = self.client.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response.error_for_status()?;
        #[derive(Deserialize)]
        struct Work {
            primary_location: Option<Location>,
            host_venue: Option<Venue>,
        }
        #[derive(Deserialize)]
        struct Location {
            source: Option<Venue>,
        }
        #[derive(Deserialize)]
        struct Venue {
            display_name: Option<String>,
        }
        let work: Work = serde_json::from_slice(&response.bytes().await?)?;
        Ok(work
            .primary_location
            .and_then(|location| location.source)
            .and_then(|source| source.display_name)
            .or_else(|| work.host_venue.and_then(|venue| venue.display_name))
            .filter(|name| !name.trim().is_empty()))
    }
}

impl Default for OpenAlexClient {
    fn default() -> Self {
        Self::new()
    }
}
