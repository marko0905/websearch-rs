//! Brave text engine — scrapes `search.brave.com/search`.
//!
//! Brave renders results with Svelte, so class names carry volatile hash
//! suffixes (`svelte-xxxxx`); selectors key on the stable parts instead — the
//! `data-type="web"` container attribute and the `title`/`content` class tokens.
//! Result anchors carry the real destination URL directly (no redirect wrapper).

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::Html;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "brave";
const SEARCH_ENDPOINT: &str = "https://search.brave.com/search";
const HOME: &str = "https://search.brave.com/";

/// Brave HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Brave;

impl Brave {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Brave {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        let url = build_url(request)?;
        let body = http.get_text(url.as_str(), &[("Referer", HOME)]).await?;
        parse_text(&body, request.max_results)
    }
}

fn build_url(request: &TextSearchRequest) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", request.query.trim());
        // Brave paginates with a 0-based page offset.
        query.append_pair("offset", &request.page.saturating_sub(1).to_string());
        query.append_pair("source", "web");
        if let Some((country, _)) = request.region.as_str().split_once('-')
            && country != "wt"
        {
            query.append_pair("country", country);
        }
    }
    Ok(url)
}

fn parse_text(body: &str, max_results: usize) -> Result<Vec<TextResult>> {
    let document = Html::parse_document(body);
    let result_selector = selector(r#"div.snippet[data-type="web"]"#)?;
    let anchor_selectors = selectors(&["a[href]"])?;
    let title_selectors = selectors(&[".title", "[class*=title]"])?;
    let snippet_selectors =
        selectors(&[".content", ".snippet-description", "[class*=description]"])?;

    let mut results: Vec<TextResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for node in document.select(&result_selector) {
        let Some(anchor) = first_match(node, &anchor_selectors) else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let href = href.trim();
        if !href.starts_with("http") || !seen.insert(href.to_string()) {
            continue;
        }
        let title = first_match(node, &title_selectors)
            .map(element_text)
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        let body_text = first_match(node, &snippet_selectors)
            .map(element_text)
            .unwrap_or_default();
        results.push(TextResult::new(title, href, body_text, NAME));
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
    fn parses_brave_fixture() {
        let body = include_str!("../../../fixtures/brave_text_page.html");
        let results = parse_text(body, 10).expect("parse");
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://rust-lang.org/");
        assert_eq!(results[0].source, NAME);
        assert!(results[0].body.contains("reliable and efficient"));

        assert_eq!(
            results[1].href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/brave_text_page.html");
        assert_eq!(parse_text(body, 2).unwrap().len(), 2);
    }

    /// Verifies the selectors still match Brave's live (hashed-class) markup.
    #[tokio::test]
    #[ignore = "hits live Brave"]
    async fn live_brave_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Brave::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "brave returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
