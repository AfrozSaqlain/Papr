//! Asynchronous client for the public arXiv Atom API.

use chrono::{DateTime, Utc};
use quick_xml::de::from_str;
use reqwest::{Client, Url};
use serde::Deserialize;
use thiserror::Error;

use crate::models::RemotePaper;

const API_URL: &str = "https://export.arxiv.org/api/query";

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
}

/// Configured, reusable arXiv API client.
#[derive(Debug, Clone)]
pub struct ArxivClient {
    client: Client,
    endpoint: Url,
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
        })
    }

    #[cfg(test)]
    fn with_endpoint(endpoint: &str) -> Result<Self, ArxivError> {
        let mut client = Self::new()?;
        client.endpoint = Url::parse(endpoint)?;
        Ok(client)
    }

    /// Search all indexed arXiv fields, ordered by relevance.
    ///
    /// # Errors
    ///
    /// Returns an error when the request fails or Atom content is malformed.
    pub async fn search(&self, query: &str, limit: u16) -> Result<Vec<RemotePaper>, ArxivError> {
        let response = self
            .client
            .get(self.endpoint.clone())
            .query(&[
                ("search_query", format!("all:{query}")),
                ("start", "0".to_owned()),
                ("max_results", limit.clamp(1, 100).to_string()),
                ("sortBy", "relevance".to_owned()),
                ("sortOrder", "descending".to_owned()),
            ])
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_feed(&response)
    }
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
    #[serde(rename = "doi")]
    doi: Option<String>,
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
                doi: entry.doi,
                journal_ref: entry.journal_ref,
            })
        })
        .collect()
}

fn normalize(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::{ArxivClient, parse_feed};

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
      <arxiv:doi>10.1000/example</arxiv:doi><arxiv:journal_ref>Example Journal</arxiv:journal_ref></entry>
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
}
