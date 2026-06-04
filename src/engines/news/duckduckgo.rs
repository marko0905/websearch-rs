//! DuckDuckGo news engine — queries the `news.js` JSON endpoint.
//!
//! Like the DuckDuckGo image/video endpoints, `news.js` requires a `vqd` token
//! first scraped from the home page, and XHR-style headers, or it returns 403.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{DDG_JSON_HEADERS, clean_html, fetch_ddg_vqd};
use crate::http::HttpClient;
use crate::{NewsResult, NewsSearchRequest, Result, WebSearchError};

const NAME: &str = "duckduckgo_news";
const NEWS_ENDPOINT: &str = "https://duckduckgo.com/news.js";

/// DuckDuckGo news engine.
#[derive(Clone, Debug, Default)]
pub struct DuckDuckGoNews;

impl DuckDuckGoNews {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<NewsSearchRequest, NewsResult> for DuckDuckGoNews {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &NewsSearchRequest,
    ) -> Result<Vec<NewsResult>> {
        let vqd = fetch_ddg_vqd(http, request.query.trim()).await?;
        let url = build_url(request, &vqd)?;
        let body = http.get_text(url.as_str(), DDG_JSON_HEADERS).await?;
        parse_news(&body, request.max_results)
    }
}

fn build_url(request: &NewsSearchRequest, vqd: &str) -> Result<Url> {
    let mut url = Url::parse(NEWS_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", request.query.trim());
        query.append_pair("l", request.region.as_str());
        query.append_pair("o", "json");
        query.append_pair("noamp", "1");
        query.append_pair("vqd", vqd);
        query.append_pair("p", request.safe_search.as_duckduckgo_kp());
        if let Some(time_limit) = request.time_limit {
            query.append_pair("df", time_limit.as_duckduckgo_df());
        }
    }
    Ok(url)
}

#[derive(Deserialize)]
struct NewsResponse {
    #[serde(default)]
    results: Vec<NewsItem>,
}

#[derive(Deserialize)]
struct NewsItem {
    date: Option<u64>,
    excerpt: Option<String>,
    source: Option<String>,
    title: String,
    url: String,
}

fn parse_news(body: &str, max_results: usize) -> Result<Vec<NewsResult>> {
    let response: NewsResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("duckduckgo news json: {err}")))?;

    let mut results: Vec<NewsResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for item in response.results {
        let title = clean_html(&item.title);
        let href = item.url.trim().to_owned();
        if title.is_empty() || href.is_empty() || !seen.insert(href.clone()) {
            continue;
        }
        let body_text = item.excerpt.as_deref().map(clean_html).unwrap_or_default();
        let source = item.source.as_deref().map(clean_html).unwrap_or_default();
        results.push(NewsResult::new(
            title, href, body_text, item.date, source, NAME,
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
    fn parses_news_fixture() {
        let body = include_str!("../../../fixtures/duckduckgo_news.json");
        let results = parse_news(body, 10).expect("parse");
        assert_eq!(results.len(), 2);

        assert_eq!(
            results[0].title,
            "Rust language release improves developer experience"
        );
        assert_eq!(results[0].href, "https://example.com/rust-release");
        assert_eq!(results[0].source, "Example News");
        assert_eq!(results[0].provider, NAME);
        assert_eq!(results[0].date, Some(1710000000));
        // HTML tags in excerpt are stripped.
        assert!(!results[0].body.contains('<'));
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/duckduckgo_news.json");
        assert_eq!(parse_news(body, 1).unwrap().len(), 1);
    }

    /// Verifies the vqd flow and live `news.js` response still parse.
    #[tokio::test]
    #[ignore = "hits live DuckDuckGo"]
    async fn live_duckduckgo_news_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = DuckDuckGoNews::new()
            .search(&http, &NewsSearchRequest::new("rust programming"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "duckduckgo news returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
