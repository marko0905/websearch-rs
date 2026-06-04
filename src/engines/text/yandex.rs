//! Yandex text engine — scrapes the web SERP at `yandex.com/search/`.
//!
//! Organic results are `li.serp-item` blocks whose title link carries the
//! `OrganicTitle-Link` class and the real destination URL (no redirect), with
//! the snippet in an `OrganicText` block. Ad `serp-item`s have no
//! `OrganicTitle-Link` anchor and are skipped. Yandex aggressively anti-bots
//! datacenter traffic, so live results depend on the requesting IP.

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::Html;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "yandex";
const SEARCH_ENDPOINT: &str = "https://yandex.com/search/";
const HOME: &str = "https://yandex.com/";

/// Yandex HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Yandex;

impl Yandex {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Yandex {
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
        query.append_pair("text", request.query.trim());
        // Yandex paginates with `p`, a 0-based page index.
        if request.page > 1 {
            query.append_pair("p", &request.page.saturating_sub(1).to_string());
        }
    }
    Ok(url)
}

fn parse_text(body: &str, max_results: usize) -> Result<Vec<TextResult>> {
    let document = Html::parse_document(body);
    let result_selector = selector("li.serp-item")?;
    // Organic titles only — ad blocks lack `.OrganicTitle-Link`.
    let anchor_selectors = selectors(&["a.OrganicTitle-Link[href]", "h2 a[href]"])?;
    let snippet_selectors = selectors(&[".OrganicText", ".TextContainer", ".organic__text"])?;

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
        let title = element_text(anchor);
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
    fn parses_yandex_fixture() {
        let body = include_str!("../../../fixtures/yandex_text_page.html");
        let results = parse_text(body, 10).expect("parse");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://rust-lang.org/");
        assert!(results[0].body.contains("empowering everyone"));
        assert_eq!(results[0].source, NAME);
    }

    #[test]
    fn skips_ad_items_without_organic_title() {
        // An ad serp-item (no OrganicTitle-Link) must not yield a result.
        let html = r#"<ul><li class="serp-item"><a class="Link OrganicLabel-Link"
            href="https://ad.example/promo">Sponsored</a></li></ul>"#;
        assert!(parse_text(html, 10).unwrap().is_empty());
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/yandex_text_page.html");
        assert_eq!(parse_text(body, 2).unwrap().len(), 2);
    }

    /// Verifies the selectors match Yandex's live markup (when the IP isn't
    /// being anti-botted).
    #[tokio::test]
    #[ignore = "hits live Yandex (anti-bot sensitive)"]
    async fn live_yandex_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Yandex::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "yandex returned no results");
        assert!(results[0].href.starts_with("http"));
    }
}
