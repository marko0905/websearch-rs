//! Startpage text engine — scrapes `www.startpage.com/sp/search`.
//!
//! Startpage proxies Google results and renders with emotion CSS-in-JS, so
//! class names carry volatile `css-xxxxx` hashes. Selectors key on stable parts
//! instead — the `result` container token, the `data-testid="gl-title-link"`
//! anchor, and `wgl-title`/`description` tokens. Result hrefs are direct.

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::Html;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "startpage";
const SEARCH_ENDPOINT: &str = "https://www.startpage.com/sp/search";
const HOME: &str = "https://www.startpage.com/";

/// Startpage HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Startpage;

impl Startpage {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Startpage {
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
        query.append_pair("query", request.query.trim());
        query.append_pair("cat", "web");
        if request.page > 1 {
            query.append_pair("page", &request.page.to_string());
        }
    }
    Ok(url)
}

fn parse_text(body: &str, max_results: usize) -> Result<Vec<TextResult>> {
    let document = Html::parse_document(body);
    let result_selector = selector("div.result")?;
    let anchor_selectors = selectors(&[
        r#"a[data-testid="gl-title-link"]"#,
        "a.result-link",
        "a.result-title",
    ])?;
    let title_selectors = selectors(&["h2.wgl-title", "h2"])?;
    let snippet_selectors = selectors(&["p.description", ".wgl-description", "p"])?;

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
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| element_text(anchor));
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
    fn parses_startpage_fixture() {
        let body = include_str!("../../../fixtures/startpage_text_page.html");
        let results = parse_text(body, 10).expect("parse");
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://rust-lang.org/");
        assert_eq!(results[0].source, NAME);
        assert!(results[0].body.contains("blazingly fast"));

        assert_eq!(
            results[1].href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/startpage_text_page.html");
        assert_eq!(parse_text(body, 2).unwrap().len(), 2);
    }

    /// Verifies the selectors still match Startpage's live markup.
    #[tokio::test]
    #[ignore = "hits live Startpage"]
    async fn live_startpage_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Startpage::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "startpage returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
