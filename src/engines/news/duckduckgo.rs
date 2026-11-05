//! DuckDuckGo news engine — queries the `news.js` JSON endpoint.
//!
//! Like the DuckDuckGo image/video endpoints, `news.js` requires a `vqd` token
//! first scraped from the home page, and XHR-style headers, or it returns 403.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{fetch_ddg_vqd, json_field};
use crate::http::HttpClient;
use crate::normalize::timestamp_to_iso;
use crate::{NewsResult, NewsSearchRequest, Result, WebSearchError};

/// Upstream `name` (the key in `ENGINES[category]`).
const NAME: &str = "duckduckgo";
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

    /// Upstream `provider = "bing"`.
    fn provider(&self) -> &'static str {
        "bing"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &NewsSearchRequest,
    ) -> Result<Vec<NewsResult>> {
        // Upstream has no `headers_update` for this engine: no extra headers.
        let vqd = fetch_ddg_vqd(http, &request.query, &[]).await?;
        let url = build_url(request, &vqd)?;
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        parse_news(&body)
    }
}

/// Upstream `build_payload`, in its dict order: `l, o, noamp, q, vqd, p`,
/// then `df` and `s` when set. The query and region are sent verbatim.
fn build_url(request: &NewsSearchRequest, vqd: &str) -> Result<Url> {
    let mut url = Url::parse(NEWS_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("l", request.region.as_str());
        query.append_pair("o", "json");
        query.append_pair("noamp", "1");
        query.append_pair("q", &request.query);
        query.append_pair("vqd", vqd);
        // Upstream `{"on": "1", "moderate": "-1", "off": "-2"}`.
        query.append_pair("p", request.safe_search.as_duckduckgo_kp());
        if let Some(time_limit) = &request.time_limit {
            query.append_pair("df", time_limit.as_duckduckgo_df());
        }
        if request.page > 1 {
            query.append_pair("s", &((request.page - 1) * 30).to_string());
        }
    }
    Ok(url)
}

#[derive(Deserialize)]
struct NewsResponse {
    #[serde(default)]
    results: Vec<serde_json::Map<String, Value>>,
}

/// Field mapping from upstream `duckduckgo_news.py` `elements_replace`:
/// `date`→`date`, `title`→`title`, `excerpt`→`body`, `url`→`url`,
/// `image`→`image`, `source`→`source`, each via `item.get(key)` (so a missing
/// key is `None`/`null`). No filtering or dedup, as upstream.
fn parse_news(body: &str) -> Result<Vec<NewsResult>> {
    let response: NewsResponse = serde_json::from_str(body)
        .map_err(|err| WebSearchError::Parse(format!("duckduckgo news json: {err}")))?;

    let mut results = Vec::new();
    for item in response.results {
        results.push(
            NewsResult {
                date: news_date(item.get("date"))?,
                title: json_field(&item, "title"),
                body: json_field(&item, "excerpt"),
                url: json_field(&item, "url"),
                image: json_field(&item, "image"),
                source: json_field(&item, "source"),
            }
            .normalized(),
        );
    }
    Ok(results)
}

/// `_normalize_date`, applied only to a truthy value: an `int` unix timestamp
/// becomes `datetime.fromtimestamp(ts, timezone.utc).isoformat()`; anything else
/// is kept. A timestamp outside `datetime`'s range raises upstream, failing the
/// engine, so it is an error here too.
///
/// Edge cases the `Option<String>` field cannot hold exactly (never seen in
/// `news.js` responses): upstream keeps a falsy `0` and a float as JSON numbers;
/// here they are rendered as strings (`"0"`, `"1.5"`).
fn news_date(value: Option<&Value>) -> Result<Option<String>> {
    Ok(match value {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Number(number)) => match number.as_i64() {
            Some(0) => Some("0".to_owned()),
            Some(ts) => Some(timestamp_to_iso(ts).ok_or_else(|| {
                WebSearchError::Parse(format!("duckduckgo news: timestamp {ts} out of range"))
            })?),
            None if number.is_u64() => {
                return Err(WebSearchError::Parse(format!(
                    "duckduckgo news: timestamp {number} out of range"
                )));
            }
            None => Some(number.to_string()),
        },
        Some(other) => Some(other.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_matches_upstream_order() {
        let request = NewsSearchRequest::new("rust")
            .time_limit(Some(crate::TimeLimit::Month))
            .page(2);
        assert_eq!(
            build_url(&request, "V").unwrap().query(),
            Some("l=us-en&o=json&noamp=1&q=rust&vqd=V&p=-1&df=m&s=30")
        );
    }

    #[test]
    fn parses_news_fixture() {
        let body = include_str!("../../../fixtures/duckduckgo_news.json");
        let results = parse_news(body).expect("parse");
        assert_eq!(results.len(), 2);

        assert_eq!(
            results[0].title.as_deref(),
            Some("Rust language release improves developer experience")
        );
        assert_eq!(
            results[0].url.as_deref(),
            Some("https://example.com/rust-release")
        );
        assert_eq!(results[0].source.as_deref(), Some("Example News"));
        // datetime.fromtimestamp(1710000000, timezone.utc).isoformat()
        assert_eq!(
            results[0].date.as_deref(),
            Some("2024-03-09T16:00:00+00:00")
        );
        assert_eq!(
            results[0].image.as_deref(),
            Some("https://example.com/rust-release.jpg")
        );
        // HTML tags in excerpt are stripped.
        assert_eq!(
            results[0].body.as_deref(),
            Some("The Rust project announced a new release for reliable software.")
        );
        // A missing `image` key is upstream's `None` → JSON null.
        assert_eq!(results[1].image, None);
        assert!(
            serde_json::to_string(&results[1])
                .unwrap()
                .contains(r#""image":null"#)
        );
    }

    #[test]
    fn keeps_items_upstream_keeps() {
        // No title/url filtering and no per-engine dedup, as upstream.
        let body = r#"{"results":[{"url":"https://a.com"},{"url":"https://a.com"},{}]}"#;
        let results = parse_news(body).expect("parse");
        assert_eq!(results.len(), 3);
        assert_eq!(results[2].url, None);
        assert_eq!(results[0].title, None);
    }

    #[test]
    fn date_values_follow_normalize_date() {
        assert_eq!(news_date(None).unwrap(), None);
        assert_eq!(
            news_date(Some(&Value::from("2 hours ago")))
                .unwrap()
                .as_deref(),
            Some("2 hours ago")
        );
        assert!(news_date(Some(&Value::from(i64::MAX))).is_err());
    }

    #[test]
    fn page_two_sets_offset() {
        let request = NewsSearchRequest::new("rust").page(2);
        let url = build_url(&request, "4-1").expect("url");
        assert!(url.as_str().ends_with("&s=30"), "{url}");
    }

    #[test]
    fn returns_every_result_without_a_cap() {
        // Upstream engines never see `max_results`; the orchestrator applies it.
        let body = include_str!("../../../fixtures/duckduckgo_news.json");
        assert!(parse_news(body).unwrap().len() > 1);
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
        assert!(results[0].url.as_deref().unwrap_or("").starts_with("http"));
    }
}
