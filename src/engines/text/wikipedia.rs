//! Wikipedia text engine — queries the MediaWiki search API.
//!
//! Unlike the scraping engines this is a clean JSON API (`action=query&
//! list=search`); results are built from the page title. Wikipedia leads text
//! results in Python `ddgs`, so it carries a high (low-numbered) priority and
//! the ranker also floats it into the top bucket.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::clean_html;
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest, WebSearchError};

const NAME: &str = "wikipedia";
const DEFAULT_LANG: &str = "en";

/// Wikipedia MediaWiki-search engine.
#[derive(Clone, Debug, Default)]
pub struct Wikipedia;

impl Wikipedia {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Wikipedia {
    fn name(&self) -> &'static str {
        NAME
    }

    fn priority(&self) -> u8 {
        // Leads text results, as in Python ddgs.
        10
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        let lang = language(request);
        let url = build_url(request, &lang)?;
        let body = http
            .get_text(url.as_str(), &[("Accept", "application/json")])
            .await?;
        parse_results(&body, &lang)
    }
}

/// Language subdomain for the wiki, derived from the region's language half.
fn language(request: &TextSearchRequest) -> String {
    match request.region.as_str().split_once('-') {
        Some((_, lang)) if lang != "wt" && !lang.is_empty() => lang.to_ascii_lowercase(),
        _ => DEFAULT_LANG.to_string(),
    }
}

fn build_url(request: &TextSearchRequest, lang: &str) -> Result<Url> {
    let mut url = Url::parse(&format!("https://{lang}.wikipedia.org/w/api.php"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("action", "query");
        query.append_pair("list", "search");
        query.append_pair("srsearch", request.query.trim());
        query.append_pair("srlimit", &request.max_results.to_string());
        query.append_pair("srprop", "snippet");
        query.append_pair("format", "json");
        let offset = request.page.saturating_sub(1) * request.max_results;
        if offset > 0 {
            query.append_pair("sroffset", &offset.to_string());
        }
    }
    Ok(url)
}

#[derive(Deserialize)]
struct ApiResponse {
    query: Option<QueryBlock>,
}

#[derive(Deserialize)]
struct QueryBlock {
    #[serde(default)]
    search: Vec<SearchItem>,
}

#[derive(Deserialize)]
struct SearchItem {
    title: String,
    #[serde(default)]
    snippet: String,
}

fn parse_results(body: &str, lang: &str) -> Result<Vec<TextResult>> {
    let response: ApiResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("wikipedia json: {err}")))?;

    let Some(query) = response.query else {
        return Ok(Vec::new());
    };

    let mut results = Vec::with_capacity(query.search.len());
    for item in query.search {
        let title = item.title.trim();
        if title.is_empty() {
            continue;
        }
        let href = page_url(lang, title);
        let body_text = clean_html(&item.snippet);
        results.push(TextResult::new(title, href, body_text, NAME));
    }
    Ok(results)
}

/// Build the canonical page URL from a title (spaces become underscores; the
/// `url` crate percent-encodes the rest).
fn page_url(lang: &str, title: &str) -> String {
    let base = format!("https://{lang}.wikipedia.org/wiki/");
    let underscored = title.replace(' ', "_");
    match Url::parse(&base).and_then(|base| base.join(&underscored)) {
        Ok(url) => url.into(),
        Err(_) => format!("{base}{underscored}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wikipedia_fixture() {
        let body = include_str!("../../../fixtures/wikipedia_search.json");
        let results = parse_results(body, "en").expect("parse");
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust (programming language)");
        assert_eq!(
            results[0].href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert_eq!(results[0].source, NAME);
        // HTML highlight tags stripped, entity decoded.
        assert!(!results[0].body.contains('<'));
        assert!(results[0].body.contains("performance & memory safety"));
    }

    #[test]
    fn empty_query_block_is_empty() {
        assert!(
            parse_results(r#"{"batchcomplete":""}"#, "en")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn language_defaults_to_en() {
        let mut req = TextSearchRequest::new("x");
        assert_eq!(language(&req), "en"); // default region wt-wt
        req.region = crate::Region::new("de-de").unwrap();
        assert_eq!(language(&req), "de");
    }

    /// Verifies the live MediaWiki API still parses.
    #[tokio::test]
    #[ignore = "hits live Wikipedia"]
    async fn live_wikipedia_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Wikipedia::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "wikipedia returned no results");
        assert!(results[0].href.contains("wikipedia.org/wiki/"));
        assert!(!results[0].title.is_empty());
    }
}
