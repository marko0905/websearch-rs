//! Bing text engine — scrapes `www.bing.com/search`.
//!
//! Bing fingerprint-detects plain TLS stacks and serves a resultless page to
//! them (confirmed in the Phase-1 smoke test: rustls got 0 results, wreq got the
//! full SERP). It therefore only works over the impersonating [`crate::http`]
//! transport. Result links are wrapped in a `/ck/a?...&u=a1<base64url>` redirect
//! that we decode back to the real destination.

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::Html;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{decode_base64, element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "bing";
const SEARCH_ENDPOINT: &str = "https://www.bing.com/search";
const HOME: &str = "https://www.bing.com/";
const PAGE_SIZE: usize = 10;

/// Bing HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Bing;

impl Bing {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Bing {
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
        // Bing pagination is a 1-based result offset.
        let first = (request.page.saturating_sub(1)) * PAGE_SIZE + 1;
        query.append_pair("first", &first.to_string());
        // Region is `xx-yy`; the country half maps to Bing's `cc`.
        if let Some((country, _)) = request.region.as_str().split_once('-')
            && country != "wt"
        {
            query.append_pair("cc", country);
        }
    }
    Ok(url)
}

fn parse_text(body: &str, max_results: usize) -> Result<Vec<TextResult>> {
    let document = Html::parse_document(body);
    let result_selector = selector("li.b_algo")?;
    let title_selectors = selectors(&["h2 a", "h2 > a", "a.tilk"])?;
    let snippet_selectors = selectors(&["div.b_caption p", "p.b_lineclamp2", "div.b_caption"])?;

    let mut results: Vec<TextResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for node in document.select(&result_selector) {
        let Some(anchor) = first_match(node, &title_selectors) else {
            continue;
        };
        let title = element_text(anchor);
        let Some(raw_href) = anchor.value().attr("href") else {
            continue;
        };
        let href = resolve_url(raw_href);
        if title.is_empty() || href.is_empty() || !seen.insert(href.clone()) {
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

/// Resolve a Bing result href: decode the `/ck/a?...&u=a1<base64url>` redirect
/// back to the real URL, or pass through a direct link unchanged.
fn resolve_url(raw_href: &str) -> String {
    let href = raw_href.trim();
    if href.is_empty() {
        return String::new();
    }

    if let Ok(parsed) = Url::parse(href)
        && parsed.path().contains("/ck/a")
        && let Some((_, encoded)) = parsed.query_pairs().find(|(key, _)| key == "u")
    {
        // The `u` value is `a1` + base64url of the destination URL.
        let payload = encoded.strip_prefix("a1").unwrap_or(&encoded);
        if let Some(bytes) = decode_base64(payload)
            && let Ok(decoded) = String::from_utf8(bytes)
            && decoded.starts_with("http")
        {
            return decoded;
        }
    }

    href.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bing_fixture_and_decodes_urls() {
        let body = include_str!("../../../fixtures/bing_text_page.html");
        let results = parse_text(body, 10).expect("parse");
        assert_eq!(results.len(), 3);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://www.rust-lang.org/");
        assert_eq!(results[0].source, NAME);
        assert!(results[0].body.contains("memory-efficient"));

        // ck/a redirect with parenthesized URL decodes intact.
        assert_eq!(
            results[1].href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        // Direct (non-redirect) href passes through.
        assert_eq!(results[2].href, "https://doc.rust-lang.org/book/");
    }

    #[test]
    fn resolve_url_passes_through_direct_links() {
        assert_eq!(
            resolve_url("https://example.com/page"),
            "https://example.com/page"
        );
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/bing_text_page.html");
        assert_eq!(parse_text(body, 2).unwrap().len(), 2);
    }

    /// Verifies the selectors and ck/a decoding work on Bing's live markup
    /// (and that wreq impersonation clears Bing's fingerprint soft-block).
    #[tokio::test]
    #[ignore = "hits live Bing"]
    async fn live_bing_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Bing::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "bing returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
