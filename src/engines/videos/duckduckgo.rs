//! DuckDuckGo video engine — queries the `v.js` JSON endpoint.
//!
//! Shares the `vqd`-token + XHR-headers flow with the DuckDuckGo news and image
//! engines; without them `v.js` returns 403.

use std::collections::HashSet;

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{DDG_JSON_HEADERS, clean_html, fetch_ddg_vqd};
use crate::http::HttpClient;
use crate::{Result, VideoResult, VideoSearchRequest, WebSearchError};

const NAME: &str = "duckduckgo_videos";
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

    async fn search(
        &self,
        http: &HttpClient,
        request: &VideoSearchRequest,
    ) -> Result<Vec<VideoResult>> {
        let vqd = fetch_ddg_vqd(http, request.query.trim()).await?;
        let url = build_url(request, &vqd)?;
        let body = http.get_text(url.as_str(), DDG_JSON_HEADERS).await?;
        parse_videos(&body, request.max_results)
    }
}

fn build_url(request: &VideoSearchRequest, vqd: &str) -> Result<Url> {
    let mut url = Url::parse(VIDEO_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", request.query.trim());
        query.append_pair("l", request.region.as_str());
        query.append_pair("o", "json");
        query.append_pair("vqd", vqd);
        query.append_pair("p", request.safe_search.as_duckduckgo_kp());
        if let Some(time_limit) = request.time_limit {
            query.append_pair("df", time_limit.as_duckduckgo_df());
        }
    }
    Ok(url)
}

#[derive(Deserialize)]
struct VideoResponse {
    #[serde(default)]
    results: Vec<VideoItem>,
}

#[derive(Deserialize)]
struct VideoItem {
    content: String,
    description: Option<String>,
    duration: Option<String>,
    embed_url: Option<String>,
    images: Option<VideoImages>,
    provider: Option<String>,
    published: Option<String>,
    publisher: Option<String>,
    title: String,
    uploader: Option<String>,
}

#[derive(Deserialize)]
struct VideoImages {
    large: Option<String>,
    medium: Option<String>,
    small: Option<String>,
}

impl VideoImages {
    /// Prefer the largest available thumbnail.
    fn best(&self) -> Option<String> {
        self.large
            .as_ref()
            .or(self.medium.as_ref())
            .or(self.small.as_ref())
            .cloned()
    }
}

fn parse_videos(body: &str, max_results: usize) -> Result<Vec<VideoResult>> {
    let response: VideoResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("duckduckgo videos json: {err}")))?;

    let mut results: Vec<VideoResult> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for item in response.results {
        let title = clean_html(&item.title);
        let content = item.content.trim().to_owned();
        if title.is_empty() || content.is_empty() || !seen.insert(content.clone()) {
            continue;
        }
        let description = item
            .description
            .as_deref()
            .map(clean_html)
            .unwrap_or_default();
        let thumbnail = item
            .images
            .as_ref()
            .and_then(VideoImages::best)
            .unwrap_or_default();
        let publisher = item
            .publisher
            .as_deref()
            .map(clean_html)
            .unwrap_or_default();
        let uploader = item.uploader.as_deref().map(clean_html).unwrap_or_default();
        let source = item.provider.as_deref().map(clean_html).unwrap_or_default();

        results.push(VideoResult::new(
            title,
            content,
            description,
            thumbnail,
            item.embed_url,
            item.duration,
            item.published,
            publisher,
            uploader,
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
    fn parses_videos_fixture() {
        let body = include_str!("../../../fixtures/duckduckgo_videos.json");
        let results = parse_videos(body, 10).expect("parse");
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].title, "Learn Rust Programming");
        assert_eq!(results[0].content, "https://www.youtube.com/watch?v=abc123");
        assert_eq!(results[0].provider, NAME);
        // Largest thumbnail chosen.
        assert_eq!(results[0].thumbnail, "https://example.com/video-large.jpg");
        assert_eq!(results[0].duration.as_deref(), Some("1:23:45"));
        assert_eq!(results[0].publisher, "YouTube");
        // Falls back to medium when large is absent.
        assert_eq!(results[1].thumbnail, "https://example.com/second-video.jpg");
    }

    #[test]
    fn respects_max_results() {
        let body = include_str!("../../../fixtures/duckduckgo_videos.json");
        assert_eq!(parse_videos(body, 1).unwrap().len(), 1);
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
        assert!(results[0].content.starts_with("http"));
    }
}
