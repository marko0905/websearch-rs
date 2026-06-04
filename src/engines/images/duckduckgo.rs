//! DuckDuckGo image engine — queries the `i.js` JSON endpoint.
//!
//! Shares the `vqd`-token + XHR-headers flow with the DuckDuckGo news engine;
//! without them `i.js` returns 403.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{DDG_JSON_HEADERS, clean_html, fetch_ddg_vqd};
use crate::http::HttpClient;
use crate::{ImageResult, ImageSearchRequest, Result, WebSearchError};

const NAME: &str = "duckduckgo_images";
const IMAGE_ENDPOINT: &str = "https://duckduckgo.com/i.js";

/// DuckDuckGo image engine.
#[derive(Clone, Debug, Default)]
pub struct DuckDuckGoImages;

impl DuckDuckGoImages {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<ImageSearchRequest, ImageResult> for DuckDuckGoImages {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &ImageSearchRequest,
    ) -> Result<Vec<ImageResult>> {
        let vqd = fetch_ddg_vqd(http, request.query.trim()).await?;
        let url = build_url(request, &vqd)?;
        let body = http.get_text(url.as_str(), DDG_JSON_HEADERS).await?;
        parse_images(&body, request.max_results)
    }
}

fn build_url(request: &ImageSearchRequest, vqd: &str) -> Result<Url> {
    let mut url = Url::parse(IMAGE_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", request.query.trim());
        query.append_pair("l", request.region.as_str());
        query.append_pair("o", "json");
        query.append_pair("vqd", vqd);
        query.append_pair("p", request.safe_search.as_duckduckgo_kp());
        query.append_pair("f", "");
        // `ct=AT` is required alongside the fetch-metadata headers, matching
        // Python ddgs; without it `i.js` 403s.
        query.append_pair("ct", "AT");
    }
    Ok(url)
}

#[derive(Deserialize)]
struct ImageResponse {
    #[serde(default)]
    results: Vec<ImageItem>,
}

#[derive(Deserialize)]
struct ImageItem {
    height: Option<u64>,
    image: String,
    source: Option<String>,
    thumbnail: Option<String>,
    title: String,
    url: String,
    width: Option<u64>,
}

fn parse_images(body: &str, max_results: usize) -> Result<Vec<ImageResult>> {
    let response: ImageResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("duckduckgo images json: {err}")))?;

    let mut results: Vec<ImageResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for item in response.results {
        let image = item.image.trim().to_owned();
        let url = item.url.trim().to_owned();
        if image.is_empty() || url.is_empty() || !seen.insert(image.clone()) {
            continue;
        }
        let source = item.source.as_deref().map(clean_html).unwrap_or_default();
        results.push(ImageResult::new(
            clean_html(&item.title),
            image,
            item.thumbnail.unwrap_or_default(),
            url,
            item.width,
            item.height,
            source,
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
    fn parses_images_fixture() {
        let body = include_str!("../../../fixtures/duckduckgo_images.json");
        let results = parse_images(body, 10).expect("parse");
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].image, "https://example.com/rust-image.png");
        assert_eq!(results[0].url, "https://example.com/rust-article");
        assert_eq!(results[0].provider, NAME);
        assert_eq!(results[0].width, Some(700));
        assert_eq!(results[0].height, Some(444));
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/duckduckgo_images.json");
        assert_eq!(parse_images(body, 1).unwrap().len(), 1);
    }

    /// Verifies the vqd flow and live `i.js` response still parse.
    #[tokio::test]
    #[ignore = "hits live DuckDuckGo"]
    async fn live_duckduckgo_images_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = DuckDuckGoImages::new()
            .search(&http, &ImageSearchRequest::new("rust programming"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "duckduckgo images returned no results");
        assert!(results[0].image.starts_with("http"));
    }
}
