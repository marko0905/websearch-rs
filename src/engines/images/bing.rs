//! Bing Images engine — scrapes `www.bing.com/images/async`.
//!
//! The async endpoint returns HTML fragments. Each image tile stores metadata in
//! the `m` JSON attribute on `a.iusc` (`murl`, `turl`, `purl`, title, etc.).

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::{ElementRef, Html};
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{ImageResult, ImageSearchRequest, Result, WebSearchError};

const NAME: &str = "bing_images";
const SEARCH_ENDPOINT: &str = "https://www.bing.com/images/async";
const HOME: &str = "https://www.bing.com/images";
const MIN_COUNT: usize = 35;

/// Bing Images HTML-fragment engine.
#[derive(Clone, Debug, Default)]
pub struct BingImages;

impl BingImages {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<ImageSearchRequest, ImageResult> for BingImages {
    fn name(&self) -> &'static str {
        NAME
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &ImageSearchRequest,
    ) -> Result<Vec<ImageResult>> {
        let url = build_url(request)?;
        let body = http.get_text(url.as_str(), &[("Referer", HOME)]).await?;
        parse_images(&body, request.max_results)
    }
}

fn build_url(request: &ImageSearchRequest) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    let count = request.max_results.max(MIN_COUNT);
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", request.query.trim());
        query.append_pair("async", "1");
        query.append_pair("first", &((request.page - 1) * count + 1).to_string());
        query.append_pair("count", &count.to_string());
    }
    Ok(url)
}

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    t: String,
    #[serde(default)]
    murl: String,
    #[serde(default)]
    turl: String,
    #[serde(default)]
    purl: String,
}

fn parse_images(body: &str, max_results: usize) -> Result<Vec<ImageResult>> {
    let document = Html::parse_fragment(body);
    let item_selector = selector("div.iuscp, div.imgpt, div[class*=iusc]")?;
    let metadata_selectors = selectors(&["a.iusc[m]", "a[m]"])?;
    let source_selectors = selectors(&["div.lnkw a", ".lnkw a", "cite"])?;
    let dimension_selectors = selectors(&["div.img_info span.nowrap", ".img_info .nowrap"])?;

    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for item in document.select(&item_selector) {
        let Some(anchor) = first_match(item, &metadata_selectors) else {
            continue;
        };
        let Some(raw_metadata) = anchor.value().attr("m") else {
            continue;
        };
        let metadata: Metadata = serde_json::from_str(raw_metadata)
            .map_err(|err| WebSearchError::Parse(format!("bing images metadata json: {err}")))?;
        let image = metadata.murl.trim().to_owned();
        let page_url = metadata.purl.trim().to_owned();
        if image.is_empty() || page_url.is_empty() || !seen.insert(image.clone()) {
            continue;
        }
        let source = first_match(item, &source_selectors)
            .map(element_text)
            .unwrap_or_default();
        let (width, height) = first_match(item, &dimension_selectors)
            .and_then(parse_dimensions)
            .unwrap_or((None, None));
        results.push(ImageResult::new(
            metadata.t,
            image,
            metadata.turl,
            page_url,
            width,
            height,
            source,
            NAME,
        ));
        if results.len() >= max_results {
            break;
        }
    }

    Ok(results)
}

fn parse_dimensions(node: ElementRef<'_>) -> Option<(Option<u64>, Option<u64>)> {
    let text = element_text(node).replace('×', "x");
    let (width, rest) = text.split_once('x')?;
    let height = rest.split_whitespace().next().unwrap_or_default();
    Some((
        width.trim().parse::<u64>().ok(),
        height.trim().parse::<u64>().ok(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bing_images_fixture() {
        let body = include_str!("../../../fixtures/bing_images_page.html");
        let results = parse_images(body, 10).expect("parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].image, "https://example.com/rust.png");
        assert_eq!(results[0].thumbnail, "https://example.com/rust-thumb.jpg");
        assert_eq!(results[0].url, "https://example.com/rust-page");
        assert_eq!(results[0].width, Some(1200));
        assert_eq!(results[0].height, Some(800));
        assert_eq!(results[0].provider, NAME);
    }

    #[tokio::test]
    #[ignore = "hits live Bing Images"]
    async fn live_bing_images_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = BingImages::new()
            .search(&http, &ImageSearchRequest::new("rust programming"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "bing images returned no results");
        assert!(results[0].image.starts_with("http"));
    }
}
