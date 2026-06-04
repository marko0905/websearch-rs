//! Mojeek text engine — scrapes `www.mojeek.com/search`.
//!
//! Mojeek is an independent index that serves clean, server-rendered HTML with
//! stable class names and direct result URLs (no redirect wrapper), so it needs
//! no impersonation tricks beyond the shared transport.

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::Html;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "mojeek";
const SEARCH_ENDPOINT: &str = "https://www.mojeek.com/search";
const HOME: &str = "https://www.mojeek.com/";
const PAGE_SIZE: usize = 10;

/// Mojeek HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Mojeek;

impl Mojeek {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Mojeek {
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
        // Mojeek paginates with `s`, a 1-based result start offset.
        if request.page > 1 {
            let start = (request.page - 1) * PAGE_SIZE + 1;
            query.append_pair("s", &start.to_string());
        }
        if request.safe_search != crate::SafeSearch::Off {
            query.append_pair("safe", "1");
        }
    }
    Ok(url)
}

fn parse_text(body: &str, max_results: usize) -> Result<Vec<TextResult>> {
    let document = Html::parse_document(body);
    let result_selector = selector("ul.results-standard li")?;
    let title_selectors = selectors(&["a.title", "h2 a"])?;
    let snippet_selectors = selectors(&["p.s", "p.result-content"])?;

    let mut results: Vec<TextResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for node in document.select(&result_selector) {
        let Some(anchor) = first_match(node, &title_selectors) else {
            continue;
        };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let href = href.trim();
        let title = element_text(anchor);
        if title.is_empty() || !href.starts_with("http") || !seen.insert(href.to_string()) {
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
    fn parses_mojeek_fixture() {
        let body = include_str!("../../../fixtures/mojeek_text_page.html");
        let results = parse_text(body, 10).expect("parse");
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://rust-lang.org/");
        assert_eq!(results[0].source, NAME);
        assert!(results[0].body.contains("improve the programming"));

        assert_eq!(
            results[1].href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/mojeek_text_page.html");
        assert_eq!(parse_text(body, 2).unwrap().len(), 2);
    }

    /// Verifies the selectors still match Mojeek's live markup.
    #[tokio::test]
    #[ignore = "hits live Mojeek"]
    async fn live_mojeek_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Mojeek::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "mojeek returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
