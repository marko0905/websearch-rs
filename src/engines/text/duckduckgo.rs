//! DuckDuckGo text engine — scrapes `html.duckduckgo.com/html/`.
//!
//! First engine ported onto the [`crate::http`] (wreq) layer. DuckDuckGo's HTML
//! endpoint works without TLS impersonation, so this is the safe first slice;
//! engines that require impersonation (Bing, etc.) follow the same shape.

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest, WebSearchError};

const NAME: &str = "duckduckgo";
const HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";
const HOME: &str = "https://duckduckgo.com/";

/// Specific block-page markers — never generic words like "captcha", so
/// legitimate results mentioning them don't false-positive.
const BLOCK_PAGE_MARKERS: &[&str] = &[
    "If this error persists, please let us know: ops@duckduckgo.com",
    "Unfortunately, bots use DuckDuckGo too",
    "anomaly-modal",
    "detected unusual activity from your",
];

/// DuckDuckGo HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct DuckDuckGo;

impl DuckDuckGo {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for DuckDuckGo {
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
        if looks_blocked(&body) {
            return Err(WebSearchError::EngineBlocked(format!(
                "{NAME} returned a block page"
            )));
        }
        parse_text(&body, request.max_results)
    }
}

fn build_url(request: &TextSearchRequest) -> Result<Url> {
    let mut url = Url::parse(HTML_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", request.query.trim());
        query.append_pair("kl", request.region.as_str());
        query.append_pair("kp", request.safe_search.as_duckduckgo_kp());
        if let Some(time_limit) = request.time_limit {
            query.append_pair("df", time_limit.as_duckduckgo_df());
        }
    }
    Ok(url)
}

fn looks_blocked(body: &str) -> bool {
    BLOCK_PAGE_MARKERS
        .iter()
        .any(|marker| body.contains(marker))
}

fn parse_text(body: &str, max_results: usize) -> Result<Vec<TextResult>> {
    let document = Html::parse_document(body);
    let result_selector = selector(".result")?;
    let title_selectors = selectors(&[".result__a", "a.result-link", "a"])?;
    let snippet_selectors = selectors(&[".result__snippet", ".result-snippet", ".result__body"])?;

    let mut results: Vec<TextResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for node in document.select(&result_selector) {
        if let Some(result) = parse_result_node(node, &title_selectors, &snippet_selectors)?
            && seen.insert(result.href.clone())
        {
            results.push(result);
            if results.len() >= max_results {
                return Ok(results);
            }
        }
    }

    // Fallback: some markup variants drop the `.result` wrapper.
    if results.is_empty() {
        let anchor_selector = selector("a.result__a, a.result-link")?;
        for anchor in document.select(&anchor_selector) {
            let title = element_text(anchor);
            let Some(raw_href) = anchor.value().attr("href") else {
                continue;
            };
            let href = clean_result_url(raw_href)?;
            if !title.is_empty() && !href.is_empty() && seen.insert(href.clone()) {
                results.push(TextResult::new(title, href, "", NAME));
                if results.len() >= max_results {
                    break;
                }
            }
        }
    }

    Ok(results)
}

fn parse_result_node(
    node: ElementRef<'_>,
    title_selectors: &[Selector],
    snippet_selectors: &[Selector],
) -> Result<Option<TextResult>> {
    let Some(anchor) = first_match(node, title_selectors) else {
        return Ok(None);
    };
    let title = element_text(anchor);
    let Some(raw_href) = anchor.value().attr("href") else {
        return Ok(None);
    };
    let href = clean_result_url(raw_href)?;
    if title.is_empty() || href.is_empty() {
        return Ok(None);
    }
    let body = first_match(node, snippet_selectors)
        .map(element_text)
        .unwrap_or_default();
    Ok(Some(TextResult::new(title, href, body, NAME)))
}

/// Resolve a DuckDuckGo result href, unwrapping the `/l/?uddg=` redirect.
fn clean_result_url(raw_href: &str) -> Result<String> {
    let href = raw_href.trim();
    if href.is_empty() {
        return Ok(String::new());
    }

    let parsed = if href.starts_with("//") {
        Url::parse(&format!("https:{href}"))?
    } else if href.starts_with('/') {
        Url::parse(HOME)?.join(href)?
    } else {
        Url::parse(href)?
    };

    if parsed.domain() == Some("duckduckgo.com")
        && parsed.path().starts_with("/l/")
        && let Some((_, target)) = parsed.query_pairs().find(|(key, _)| key == "uddg")
    {
        return Ok(target.into_owned());
    }

    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_fixture() {
        let body = include_str!("../../../fixtures/duckduckgo_text_page.html");
        let results = parse_text(body, 10).expect("parse");
        assert!(!results.is_empty(), "fixture should yield results");
        let first = &results[0];
        assert!(!first.title.is_empty());
        assert!(first.href.starts_with("http"));
        assert_eq!(first.source, NAME);
    }

    #[test]
    fn unwraps_redirect_url() {
        let href = "/l/?kh=-1&uddg=https%3A%2F%2Fexample.com%2Fpage";
        assert_eq!(clean_result_url(href).unwrap(), "https://example.com/page");
    }

    #[test]
    fn detects_block_page() {
        assert!(looks_blocked("... anomaly-modal ..."));
        assert!(!looks_blocked(
            "a normal page mentioning captcha in passing"
        ));
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/duckduckgo_text_page.html");
        let results = parse_text(body, 1).expect("parse");
        assert_eq!(results.len(), 1);
    }

    /// Verifies the selectors still match DuckDuckGo's live HTML markup.
    #[tokio::test]
    #[ignore = "hits live DuckDuckGo"]
    async fn live_duckduckgo_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = DuckDuckGo::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "duckduckgo returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
