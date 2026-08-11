//! OpenAlex API client for DOI-based journal discovery.

use reqwest::{Client, Url};
use serde::Deserialize;

const OPENALEX_ENDPOINT: &str = "https://api.openalex.org/works/";

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

#[derive(Debug, thiserror::Error)]
/// Errors returned by the `OpenAlex` API client.
pub enum OpenAlexError {
    #[error("HTTP request failed: {0}")]
    /// The request failed before a response was received.
    Request(#[from] reqwest::Error),
    #[error("failed to parse response: {0}")]
    /// The response was not valid `OpenAlex` JSON.
    Parse(#[from] serde_json::Error),
    #[error("work not found")]
    /// No work matched the DOI.
    NotFound,
}

/// Client for DOI lookups against `OpenAlex`.
#[derive(Clone)]
pub struct OpenAlexClient {
    client: Client,
}

impl OpenAlexClient {
    /// Build a client with the default `OpenAlex` endpoint.
    #[must_use]
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent(concat!("papr/", env!("CARGO_PKG_VERSION")))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Fetch the canonical journal name for a DOI, if `OpenAlex` has one.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or the response is malformed.
    pub async fn journal_by_doi(&self, doi: &str) -> Result<Option<String>, OpenAlexError> {
        let url = Url::parse(OPENALEX_ENDPOINT)
            .map_err(|_| OpenAlexError::NotFound)?
            .join(&format!("https://doi.org/{doi}"))
            .map_err(|_| OpenAlexError::NotFound)?;
        let response = self.client.get(url).send().await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let response = response.error_for_status()?;
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
