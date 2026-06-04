//! Grokipedia text engine — queries the typeahead JSON endpoint.
//!
//! Python `ddgs` treats Grokipedia as a high-priority, single-result text
//! engine. The endpoint returns a small JSON result list with title, slug, and
//! snippet fields; we expose the first matching page as a text result.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::clean_html;
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest, WebSearchError};

const NAME: &str = "grokipedia";
const SEARCH_ENDPOINT: &str = "https://grokipedia.com/api/typeahead";
const PAGE_BASE: &str = "https://grokipedia.com/page/";
const HOME: &str = "https://grokipedia.com/";

/// Grokipedia typeahead engine.
#[derive(Clone, Debug, Default)]
pub struct Grokipedia;

impl Grokipedia {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Grokipedia {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        // Runs next to Wikipedia at the front of the text engine fan-out.
        10
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        let url = build_url(request)?;
        let body = http.get_text(url.as_str(), &[("Referer", HOME)]).await?;
        parse_results(&body, request.max_results)
    }
}

fn build_url(request: &TextSearchRequest) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("query", request.query.trim())
        .append_pair("limit", "1");
    Ok(url)
}

#[derive(Deserialize)]
struct ApiResponse {
    #[serde(default)]
    results: Vec<Item>,
}

#[derive(Deserialize)]
struct Item {
    title: String,
    slug: String,
    #[serde(default)]
    snippet: String,
}

fn parse_results(body: &str, max_results: usize) -> Result<Vec<TextResult>> {
    let response: ApiResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("grokipedia json: {err}")))?;

    let mut results = Vec::new();
    for item in response.results {
        let title = clean_html(item.title.trim_matches('_'));
        let slug = item.slug.trim();
        if title.is_empty() || slug.is_empty() {
            continue;
        }
        let snippet = item
            .snippet
            .split_once("\n\n")
            .map(|(_, body)| body)
            .unwrap_or(item.snippet.as_str());
        results.push(TextResult::new(
            title,
            format!("{PAGE_BASE}{slug}"),
            clean_html(snippet),
            NAME,
        ));
        if results.len() >= max_results {
            break;
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_grokipedia_fixture() {
        let body = include_str!("../../../fixtures/grokipedia_typeahead.json");
        let results = parse_results(body, 10).expect("parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust (programming language)");
        assert_eq!(
            results[0].href,
            "https://grokipedia.com/page/Rust_(programming_language)"
        );
        assert!(results[0].body.contains("systems programming language"));
        assert_eq!(results[0].source, NAME);
    }

    #[tokio::test]
    #[ignore = "hits live Grokipedia"]
    async fn live_grokipedia_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Grokipedia::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "grokipedia returned no results");
        assert!(results[0].href.starts_with(PAGE_BASE));
    }
}
