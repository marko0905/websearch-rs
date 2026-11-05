//! Yandex text engine — port of `ddgs/engines/yandex.py` (v9.16.1).
//!
//! **DISABLED to match upstream ddgs v9.16.1** (since ddgs v9.15.0): upstream
//! sets `disabled = True` and skips the engine in its registry, so it is not
//! registered in [`crate::engines::registry::EngineRegistry`] and cannot be
//! selected by `auto` or by name. The code is kept 1:1 with upstream; to
//! re-enable, register `Yandex` in `registry.rs` once upstream does.
//!
//! `GET https://yandex.com/search/site/` with `text`, `web=1`, a random
//! 7-digit `searchid` (`SystemRandom().randint(1000000, 9999999)`), and `p`
//! for later pages. Parsing is the generic `BaseSearchEngine` loop with
//! upstream's XPaths; there is no `post_extract_results`.

use async_trait::async_trait;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{system_randint, text_result};
use crate::html;
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "yandex";
const SEARCH_ENDPOINT: &str = "https://yandex.com/search/site/";

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//li[contains(@class, 'serp-item')]";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("title", ".//h3//text()"),
    ("href", ".//h3//a/@href"),
    ("body", ".//div[contains(@class, 'text')]//text()"),
];

/// Yandex HTML text engine.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // disabled upstream; not registered (see module docs)
pub struct Yandex;

impl Yandex {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Yandex {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "yandex"`.
    fn provider(&self) -> &'static str {
        "yandex"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        let url = build_url(request, system_randint(1_000_000, 9_999_999)?)?;
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        parse_text(&body)
    }
}

/// Upstream `build_payload` (dict order). Region, safe search and time limit
/// are not used.
fn build_url(request: &TextSearchRequest, searchid: u64) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("text", &request.query);
        query.append_pair("web", "1");
        query.append_pair("searchid", &searchid.to_string());
        if request.page > 1 {
            query.append_pair("p", &(request.page - 1).to_string());
        }
    }
    Ok(url)
}

/// Upstream `BaseSearchEngine.extract_results` with Yandex's XPaths,
/// evaluated by libxml2 exactly like lxml.
fn parse_text(body: &str) -> Result<Vec<TextResult>> {
    Ok(html::extract_results(body, ITEMS_XPATH, ELEMENTS_XPATH)?
        .into_iter()
        .map(|fields| text_result(&fields))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_upstream_output() {
        // Expected output was produced by ddgs 9.16.1's own Yandex
        // `extract_results` on the same fixture.
        let expected: Vec<TextResult> =
            serde_json::from_str(include_str!("../../../fixtures/yandex_text_expected.json"))
                .expect("expected json");
        let results = parse_text(include_str!("../../../fixtures/yandex_text_page.html")).unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn request_matches_upstream() {
        let url = build_url(&TextSearchRequest::new("rust lang").page(3), 1234567).unwrap();
        assert_eq!(
            url.as_str(),
            "https://yandex.com/search/site/?text=rust+lang&web=1&searchid=1234567&p=2"
        );
        let url = build_url(&TextSearchRequest::new("q"), 7654321).unwrap();
        assert_eq!(
            url.as_str(),
            "https://yandex.com/search/site/?text=q&web=1&searchid=7654321"
        );
    }

    /// Verifies the XPaths on Yandex's live markup (anti-bot sensitive).
    #[tokio::test]
    #[ignore = "hits live Yandex (anti-bot sensitive)"]
    async fn live_yandex_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Yandex::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "yandex returned no results");
    }
}
