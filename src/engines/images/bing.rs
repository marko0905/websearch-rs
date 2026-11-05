//! Bing Images engine — scrapes `www.bing.com/images/async`.
//!
//! The async endpoint returns HTML fragments. Each image tile stores metadata in
//! the `m` JSON attribute on `a.iusc` (`murl`, `turl`, `purl`, title, etc.).

use async_trait::async_trait;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::json_field;
use crate::html;
use crate::http::HttpClient;
use crate::{ImageResult, ImageSearchRequest, Result, WebSearchError};

/// Upstream `name` (the key in `ENGINES[category]`).
const NAME: &str = "bing";
/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[./div[@class='imgpt']/a[@m] and ./div[@class='infopt']]";
const SEARCH_ENDPOINT: &str = "https://www.bing.com/images/async";
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

    /// Upstream `provider = "bing"`.
    fn provider(&self) -> &'static str {
        "bing"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &ImageSearchRequest,
    ) -> Result<Vec<ImageResult>> {
        let url = build_url(request)?;
        // Upstream has no `headers_update` for this engine: no extra headers.
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        parse_images(&body)
    }
}

fn build_url(request: &ImageSearchRequest) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    // Upstream: `count = max(int(kwargs.get("max_results", 10)), 35)`. But
    // `_search_sync` passes `max_results` as its own argument, never in
    // `kwargs`, so this is always `max(10, 35)` = 35 (upstream quirk,
    // reproduced: the request's `max_results` is ignored).
    let count = 10.max(MIN_COUNT);
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", &request.query);
        query.append_pair("async", "1");
        query.append_pair("first", &((request.page - 1) * count + 1).to_string());
        query.append_pair("count", &count.to_string());
    }
    // Upstream quirk: `{'day': 1440, 'week': 10080, 'month': 44640, 'year':
    // 525600}[timelimit]` is keyed by full words, but `timelimit` is
    // `d`/`w`/`m`/`y`, so any time limit raises `KeyError` while the payload is
    // built (before any request) and the engine fails. Reproduced: the
    // engine error carries `str(KeyError('d'))`, i.e. `'d'`.
    if let Some(time_limit) = &request.time_limit {
        return Err(time_limit.key_error());
    }
    Ok(url)
}

/// Field mapping from upstream `bing_images.py` `extract_results`.
///
/// - Items: `//div[./div[@class='imgpt']/a[@m] and ./div[@class='infopt']]`.
/// - Only items with `.//a[@class='iusc']/@m` produce a result (upstream's
///   `if metadata`); `title`, `image`, `thumbnail`, `url` are `m.get("t")`,
///   `"murl"`, `"turl"`, `"purl"` (missing → `null`).
/// - `width`/`height`: first text node of
///   `.//div[contains(@class, 'img_info')][./span]/span[@class='nowrap']/text()`,
///   split on `x` (after `×` → `x`).
/// - `source`: first text node of `.//div[@class='lnkw']//a/text()`.
///
/// No other filtering or dedup, as upstream.
fn parse_images(body: &str) -> Result<Vec<ImageResult>> {
    let document = html::Document::fromstring(body)?;
    let mut results = Vec::new();
    for item in document.root().xpath_nodes(ITEMS_XPATH)? {
        // `if metadata := item.xpath(".//a[@class='iusc']/@m")`
        let metadata = item.xpath_strings(".//a[@class='iusc']/@m")?;
        let Some(raw_metadata) = metadata.first() else {
            continue;
        };
        // `json.loads(metadata[0])`; a bad value raises and fails the engine.
        let metadata: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(raw_metadata).map_err(|err| {
                WebSearchError::Parse(format!("bing images metadata json: {err}"))
            })?;
        let mut result = ImageResult {
            title: json_field(&metadata, "t"),
            image: json_field(&metadata, "murl"),
            thumbnail: json_field(&metadata, "turl"),
            url: json_field(&metadata, "purl"),
            ..ImageResult::default()
        };
        let dimension = item.xpath_strings(
            ".//div[contains(@class, 'img_info')][./span]/span[@class='nowrap']/text()",
        )?;
        if let Some(dimension) = dimension.first() {
            let (width, height) = parse_dimensions(dimension)?;
            result.width = serde_json::Value::String(width);
            result.height = serde_json::Value::String(height);
        }
        let source = item.xpath_strings(".//div[@class='lnkw']//a/text()")?;
        if let Some(source) = source.into_iter().next() {
            result.source = Some(source);
        }
        results.push(result.normalized());
    }
    Ok(results)
}

/// Upstream: `width, height = text.replace("×", "x").split("x")`, then
/// `width.strip()` and `height.split()[0].strip()`. Returns `(width, height)`.
/// Where upstream raises (`ValueError` unless exactly two parts, `IndexError`
/// for an empty height), the whole engine fails, so this is an error.
fn parse_dimensions(text: &str) -> Result<(String, String)> {
    let malformed = || WebSearchError::Parse(format!("bing images: bad dimensions {text:?}"));
    let replaced = text.replace('×', "x");
    let parts: Vec<&str> = replaced.split('x').collect();
    let [width, height] = parts.as_slice() else {
        return Err(malformed());
    };
    let height = height
        .split(crate::normalize::is_py_whitespace)
        .find(|part| !part.is_empty())
        .ok_or_else(malformed)?;
    Ok((
        width
            .trim_matches(crate::normalize::is_py_whitespace)
            .to_owned(),
        height.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_is_fixed_count_and_time_limit_raises_key_error() {
        let url = build_url(&ImageSearchRequest::new(" rust").max_results(100)).unwrap();
        assert_eq!(url.query(), Some("q=+rust&async=1&first=1&count=35"));
        let request = ImageSearchRequest::new("rust").time_limit(Some(crate::TimeLimit::Day));
        let error = build_url(&request).unwrap_err();
        assert_eq!(error.to_string(), "'d'");
    }

    #[test]
    fn matches_upstream_output() {
        // ddgs 9.16.1's own BingImages `extract_results` +
        // `post_extract_results` on the same fixture.
        let expected: Vec<ImageResult> =
            serde_json::from_str(include_str!("../../../fixtures/bing_images_expected.json"))
                .expect("expected json");
        let results =
            parse_images(include_str!("../../../fixtures/bing_images_page.html")).unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn parses_bing_images_fixture() {
        let body = include_str!("../../../fixtures/bing_images_page.html");
        let results = parse_images(body).expect("parse");
        assert_eq!(results.len(), 2);
        let first = &results[0];
        assert_eq!(first.title.as_deref(), Some("Rust Programming Language"));
        assert_eq!(first.image.as_deref(), Some("https://example.com/rust.png"));
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://example.com/rust-thumb.jpg")
        );
        assert_eq!(first.url.as_deref(), Some("https://example.com/rust-page"));
        assert_eq!(first.width, serde_json::Value::from("1200"));
        assert_eq!(first.height, serde_json::Value::from("800"));
        // First direct text node of the `lnkw` anchor only.
        assert_eq!(first.source.as_deref(), Some("example.com"));
        assert_eq!(results[1].width, serde_json::Value::from("640"));
        assert_eq!(results[1].height, serde_json::Value::from("480"));
        // `m` without `t` → null title; no dimensions → default "".
        assert_eq!(results[1].title, None);
    }

    #[test]
    fn item_needs_imgpt_and_infopt_children() {
        let body = r#"<div><div class="imgpt"><a class="iusc" m='{"murl":"u"}'></a></div></div>"#;
        assert!(parse_images(body).expect("parse").is_empty());
    }

    #[test]
    fn malformed_dimensions_fail_the_engine() {
        assert!(parse_dimensions("1200 x 800 x 3").is_err());
        assert!(parse_dimensions("1200").is_err());
        assert!(parse_dimensions("1200 x ").is_err());
        assert_eq!(
            parse_dimensions("1200 × 800 · jpeg").unwrap(),
            ("1200".to_owned(), "800".to_owned())
        );
        let body = r#"<div><div class="imgpt"><a class="iusc" m='{"murl":"u"}'></a></div><div class="infopt"><div class="img_info"><span class="nowrap">big</span></div></div></div>"#;
        assert!(parse_images(body).is_err());
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
        assert!(
            results[0]
                .image
                .as_deref()
                .unwrap_or("")
                .starts_with("http")
        );
    }
}
