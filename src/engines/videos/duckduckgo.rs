//! DuckDuckGo video engine — queries the `v.js` JSON endpoint.
//!
//! Shares the `vqd`-token + XHR-headers flow with the DuckDuckGo news and image
//! engines; without them `v.js` returns 403.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{fetch_ddg_vqd, json_field, json_raw};
use crate::http::HttpClient;
use crate::{Result, TimeLimit, VideoResult, VideoSearchRequest, WebSearchError};

/// Upstream `name` (the key in `ENGINES[category]`).
const NAME: &str = "duckduckgo";
const VIDEO_ENDPOINT: &str = "https://duckduckgo.com/v.js";

/// DuckDuckGo video engine.
#[derive(Clone, Debug, Default)]
pub struct DuckDuckGoVideos;

impl DuckDuckGoVideos {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<VideoSearchRequest, VideoResult> for DuckDuckGoVideos {
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
        request: &VideoSearchRequest,
    ) -> Result<Vec<VideoResult>> {
        // Upstream has no `headers_update` for this engine: no extra headers.
        let vqd = fetch_ddg_vqd(http, &request.query, &[]).await?;
        let url = build_url(request, &vqd)?;
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        parse_videos(&body)
    }
}

/// Upstream `build_payload`, in its dict order: `l, o, q, vqd, f, p`, then
/// `s` for `page > 1`. `f` is always sent, as
/// `"{publishedAfter:t},{videoDefinition:r},{videoDuration:d},{videoLicense:l}"`
/// with empty parts for unset filters (so `",,,"` with none).
fn build_url(request: &VideoSearchRequest, vqd: &str) -> Result<Url> {
    let filter = |prefix: &str, value: Option<&str>| match value {
        Some(value) if !value.is_empty() => format!("{prefix}:{value}"),
        _ => String::new(),
    };
    let f = [
        filter(
            "publishedAfter",
            request.time_limit.as_ref().map(TimeLimit::as_duckduckgo_df),
        ),
        filter("videoDefinition", request.resolution.as_deref()),
        filter("videoDuration", request.duration.as_deref()),
        filter("videoLicense", request.license_videos.as_deref()),
    ]
    .join(",");

    let mut url = Url::parse(VIDEO_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("l", request.region.as_str());
        query.append_pair("o", "json");
        query.append_pair("q", &request.query);
        query.append_pair("vqd", vqd);
        query.append_pair("f", &f);
        // Upstream `{"on": "1", "moderate": "-1", "off": "-2"}`.
        query.append_pair("p", request.safe_search.as_duckduckgo_kp());
        if request.page > 1 {
            query.append_pair("s", &((request.page - 1) * 60).to_string());
        }
    }
    Ok(url)
}

#[derive(Deserialize)]
struct VideoResponse {
    #[serde(default)]
    results: Vec<serde_json::Map<String, serde_json::Value>>,
}

/// Field mapping from upstream `duckduckgo_videos.py` `elements_replace`: every
/// result field is copied from the same JSON key with `item.get(key)` (missing
/// → `null`): `content`, `description`, `duration`, `embed_html`, `embed_url`,
/// `image_token`, `images`, `provider`, `published`, `publisher`,
/// `statistics`, `title`, `uploader`. Only `title` and `publisher` are
/// normalized; `images`/`statistics` are copied verbatim (JSON types and key
/// order kept). No filtering or dedup, as upstream.
fn parse_videos(body: &str) -> Result<Vec<VideoResult>> {
    let response: VideoResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("duckduckgo videos json: {err}")))?;

    let results: Vec<VideoResult> = response
        .results
        .iter()
        .map(|item| {
            VideoResult {
                title: json_field(item, "title"),
                content: json_field(item, "content"),
                description: json_field(item, "description"),
                duration: json_field(item, "duration"),
                embed_html: json_field(item, "embed_html"),
                embed_url: json_field(item, "embed_url"),
                image_token: json_field(item, "image_token"),
                images: json_raw(item, "images"),
                provider: json_field(item, "provider"),
                published: json_field(item, "published"),
                publisher: json_field(item, "publisher"),
                statistics: json_raw(item, "statistics"),
                uploader: json_field(item, "uploader"),
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
        let plain = build_url(&VideoSearchRequest::new("rust"), "V").unwrap();
        // `f` is always sent; moderate → `p=-1`.
        assert_eq!(
            plain.query(),
            Some("l=us-en&o=json&q=rust&vqd=V&f=%2C%2C%2C&p=-1")
        );
        let request = VideoSearchRequest::new("rust")
            .time_limit(Some(TimeLimit::Day))
            .duration("short")
            .page(2);
        assert_eq!(
            build_url(&request, "V").unwrap().query(),
            Some(
                "l=us-en&o=json&q=rust&vqd=V&f=publishedAfter%3Ad%2C%2CvideoDuration%3Ashort%2C&p=-1&s=60"
            )
        );
    }

    #[test]
    fn parses_videos_fixture() {
        let body = include_str!("../../../fixtures/duckduckgo_videos.json");
        let results = parse_videos(body).expect("parse");
        assert_eq!(results.len(), 2);

        let first = &results[0];
        assert_eq!(first.title.as_deref(), Some("Learn Rust Programming"));
        assert_eq!(
            first.content.as_deref(),
            Some("https://www.youtube.com/watch?v=abc123")
        );
        assert_eq!(first.provider.as_deref(), Some("Bing"));
        assert_eq!(first.duration.as_deref(), Some("1:23:45"));
        assert_eq!(first.publisher.as_deref(), Some("YouTube"));
        assert_eq!(first.uploader.as_deref(), Some("Example Channel"));
        assert_eq!(
            first.published.as_deref(),
            Some("2024-05-21T16:00:07.0000000")
        );
        assert_eq!(first.image_token.as_deref(), Some("token-abc"));
        assert!(
            first
                .embed_html
                .as_deref()
                .unwrap_or("")
                .starts_with("<iframe")
        );
        // `description` is not normalized upstream, so markup is kept.
        assert_eq!(
            first.description.as_deref(),
            Some("Learn <b>Rust</b> programming from scratch.")
        );
        assert_eq!(
            first.images["large"],
            serde_json::Value::from("https://example.com/video-large.jpg")
        );
        // Raw copy: integers and nulls keep their JSON types.
        assert_eq!(
            first.statistics["viewCount"],
            serde_json::Value::from(12345)
        );
        assert_eq!(results[1].statistics["viewCount"], serde_json::Value::Null);
        // Key order of the copied object is the source order.
        let json = serde_json::to_string(&first.images).unwrap();
        assert!(json.starts_with(r#"{"large":"#) && json.contains(r#""motion":"","small""#));
        // Second item lacks `embed_html`/`image_token` → null.
        assert_eq!(results[1].image_token, None);
    }

    #[test]
    fn returns_every_result_without_a_cap() {
        // Upstream engines never see `max_results`; the orchestrator applies it.
        let body = include_str!("../../../fixtures/duckduckgo_videos.json");
        assert!(parse_videos(body).unwrap().len() > 1);
    }

    /// Verifies the vqd flow and live `v.js` response still parse.
    #[tokio::test]
    #[ignore = "hits live DuckDuckGo"]
    async fn live_duckduckgo_videos_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = DuckDuckGoVideos::new()
            .search(&http, &VideoSearchRequest::new("rust programming"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "duckduckgo videos returned no results");
        assert!(
            results[0]
                .content
                .as_deref()
                .unwrap_or("")
                .starts_with("http")
        );
    }
}
