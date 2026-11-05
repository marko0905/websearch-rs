//! DuckDuckGo image engine — queries the `i.js` JSON endpoint.
//!
//! Shares the `vqd`-token + XHR-headers flow with the DuckDuckGo news engine;
//! without them `i.js` returns 403.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{DDG_IMAGES_HEADERS, fetch_ddg_vqd, json_field, json_raw};
use crate::http::HttpClient;
use crate::{ImageResult, ImageSearchRequest, Result, SafeSearch, TimeLimit, WebSearchError};

/// Upstream `name` (the key in `ENGINES[category]`).
const NAME: &str = "duckduckgo";
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

    /// Upstream `provider = "bing"`.
    fn provider(&self) -> &'static str {
        "bing"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &ImageSearchRequest,
    ) -> Result<Vec<ImageResult>> {
        let vqd = fetch_ddg_vqd(http, &request.query, DDG_IMAGES_HEADERS).await?;
        let url = build_url(request, &vqd)?;
        let Some(body) = http.get_text(url.as_str(), DDG_IMAGES_HEADERS).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        parse_images(&body)
    }
}

/// Upstream `build_payload`, in its dict order: `o, q, l, vqd, p, ct`, then
/// `f` only when a filter is set, then `s` for `page > 1`. The query and
/// region are sent verbatim.
fn build_url(request: &ImageSearchRequest, vqd: &str) -> Result<Url> {
    // Upstream `safesearch_base = {"on": "1", "moderate": "1", "off": "-1"}`.
    let p = match request.safe_search {
        SafeSearch::On | SafeSearch::Moderate => "1",
        SafeSearch::Off => "-1",
    };
    // Each filter is `"<prefix>:<value>"` when the value is truthy, else "".
    let filter = |prefix: &str, value: Option<&str>| match value {
        Some(value) if !value.is_empty() => format!("{prefix}:{value}"),
        _ => String::new(),
    };
    // `timelimit_base[timelimit]`: any other value raises `KeyError`.
    let time_limit = match &request.time_limit {
        None => None,
        Some(TimeLimit::Day) => Some("Day"),
        Some(TimeLimit::Week) => Some("Week"),
        Some(TimeLimit::Month) => Some("Month"),
        Some(TimeLimit::Year) => Some("Year"),
        Some(custom @ TimeLimit::Custom(_)) => return Err(custom.key_error()),
    };
    let time_limit = filter("time", time_limit);
    let parts = [
        time_limit,
        filter("size", request.size.as_deref()),
        filter("color", request.color.as_deref()),
        filter("type", request.type_image.as_deref()),
        filter("layout", request.layout.as_deref()),
        filter("license", request.license_image.as_deref()),
    ];

    let mut url = Url::parse(IMAGE_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("o", "json");
        query.append_pair("q", &request.query);
        query.append_pair("l", request.region.as_str());
        query.append_pair("vqd", vqd);
        query.append_pair("p", p);
        query.append_pair("ct", "AT");
        if parts.iter().any(|part| !part.is_empty()) {
            query.append_pair("f", &parts.join(","));
        }
        if request.page > 1 {
            query.append_pair("s", &((request.page - 1) * 100).to_string());
        }
    }
    Ok(url)
}

#[derive(Deserialize)]
struct ImageResponse {
    #[serde(default)]
    results: Vec<serde_json::Map<String, serde_json::Value>>,
}

/// Field mapping from upstream `duckduckgo_images.py` `elements_replace`: each
/// of `title`, `image`, `thumbnail`, `url`, `height`, `width`, `source` is
/// copied from the same JSON key with `item.get(key)` (missing → `null`).
/// `height`/`width` have no normalizer upstream, so they keep their JSON type
/// (integers). No filtering or dedup, as upstream.
fn parse_images(body: &str) -> Result<Vec<ImageResult>> {
    let response: ImageResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("duckduckgo images json: {err}")))?;

    let results: Vec<ImageResult> = response
        .results
        .iter()
        .map(|item| {
            ImageResult {
                title: json_field(item, "title"),
                image: json_field(item, "image"),
                thumbnail: json_field(item, "thumbnail"),
                url: json_field(item, "url"),
                height: json_raw(item, "height"),
                width: json_raw(item, "width"),
                source: json_field(item, "source"),
            }
            .normalized()
        })
        .collect();
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_matches_upstream_order_and_values() {
        let plain = build_url(&ImageSearchRequest::new(" rust  "), "V").unwrap();
        // No filter → no `f`; moderate → `p=1`; query verbatim.
        assert_eq!(
            plain.query(),
            Some("o=json&q=+rust++&l=us-en&vqd=V&p=1&ct=AT")
        );
        let request = ImageSearchRequest::new("rust")
            .safe_search(SafeSearch::Off)
            .time_limit(Some(TimeLimit::Week))
            .color("Red")
            .license_image("Public")
            .page(3);
        let url = build_url(&request, "V").unwrap();
        assert_eq!(
            url.query(),
            Some(
                "o=json&q=rust&l=us-en&vqd=V&p=-1&ct=AT\
                 &f=time%3AWeek%2C%2Ccolor%3ARed%2C%2C%2Clicense%3APublic&s=200"
            )
        );
        // An empty filter string is falsy upstream: no `f`.
        let empty = build_url(&ImageSearchRequest::new("q").size(""), "V").unwrap();
        assert!(!empty.as_str().contains("&f="));
        let on = ImageSearchRequest::new("q").safe_search(SafeSearch::On);
        assert!(build_url(&on, "V").unwrap().as_str().contains("&p=1&"));
    }

    #[test]
    fn parses_images_fixture() {
        let body = include_str!("../../../fixtures/duckduckgo_images.json");
        let results = parse_images(body).expect("parse");
        assert_eq!(results.len(), 2);

        let first = &results[0];
        assert_eq!(first.title.as_deref(), Some("Rust Programming Language"));
        assert_eq!(
            first.image.as_deref(),
            Some("https://example.com/rust-image.png")
        );
        assert_eq!(
            first.url.as_deref(),
            Some("https://example.com/rust-article")
        );
        assert_eq!(
            first.thumbnail.as_deref(),
            Some("https://example.com/rust-thumb.png")
        );
        // Integers stay integers, as upstream's raw copy.
        assert_eq!(first.width, serde_json::Value::from(700));
        assert_eq!(first.height, serde_json::Value::from(444));
        assert_eq!(first.source.as_deref(), Some("Bing"));
        let json = serde_json::to_string(first).unwrap();
        assert!(json.contains(r#""height":444,"width":700"#), "{json}");
    }

    #[test]
    fn missing_keys_are_null_and_nothing_is_filtered() {
        let body =
            r#"{"results":[{"image":"https://a.com/i.png"},{"image":"https://a.com/i.png"}]}"#;
        let results = parse_images(body).expect("parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].url, None);
        assert_eq!(results[0].height, serde_json::Value::Null);
    }

    #[test]
    fn returns_every_result_without_a_cap() {
        // Upstream engines never see `max_results`; the orchestrator applies it.
        let body = include_str!("../../../fixtures/duckduckgo_images.json");
        assert!(parse_images(body).unwrap().len() > 1);
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
        assert!(
            results[0]
                .image
                .as_deref()
                .unwrap_or("")
                .starts_with("http")
        );
    }
}
