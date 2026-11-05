//! DuckDuckGo text engine — port of `ddgs/engines/duckduckgo.py` (v9.16.1).
//!
//! Form-encoded `POST https://html.duckduckgo.com/html/` with
//! `{"q": query, "b": "", "l": region}`, plus `s` for later pages and `df` for
//! a time limit. Upstream sends no extra headers.
//!
//! Parsing is the generic `BaseSearchEngine` loop: items
//! `//div[contains(@class, 'body')]`, title `.//h2//text()`, href `./a/@href`,
//! body `./a//text()`. On the real page the item is `div.result__body`; its
//! only direct-child `<a>` is the `result__snippet` link, so the href is the
//! snippet's (direct) URL and the body is the snippet text. Upstream does not
//! unwrap `/l/?uddg=` redirects. `post_extract_results` drops ads
//! (`https://duckduckgo.com/y.js?` hrefs); there is no other filter.

use async_trait::async_trait;

use crate::engines::SearchEngine;
use crate::engines::util::text_result;
use crate::html;
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "duckduckgo";
const HTML_ENDPOINT: &str = "https://html.duckduckgo.com/html/";

/// DuckDuckGo HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct DuckDuckGo;

impl DuckDuckGo {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for DuckDuckGo {
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
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        let form = build_payload(request);
        let Some(body) = http.post_form_text(HTML_ENDPOINT, &form, &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        // No `max_results` cap here: upstream engines return the whole page and
        // the orchestrator applies the cap.
        Ok(post_extract_results(parse_text(&body)?))
    }
}

/// Upstream `build_payload`, in its dict order. The query and region are sent
/// verbatim.
fn build_payload(request: &TextSearchRequest) -> Vec<(String, String)> {
    let mut form = vec![
        ("q".to_owned(), request.query.clone()),
        ("b".to_owned(), String::new()),
        ("l".to_owned(), request.region.as_str().to_owned()),
    ];
    if request.page > 1 {
        // `10 + (page - 2) * 15`: page 2 → 10, page 3 → 25, ...
        form.push(("s".to_owned(), (10 + (request.page - 2) * 15).to_string()));
    }
    if let Some(limit) = &request.time_limit {
        form.push(("df".to_owned(), limit.as_duckduckgo_df().to_owned()));
    }
    form
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[contains(@class, 'body')]";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("title", ".//h2//text()"),
    ("href", "./a/@href"),
    ("body", "./a//text()"),
];

/// Upstream `BaseSearchEngine.extract_results` with this engine's XPaths,
/// evaluated by libxml2 exactly like lxml. Every item gives a result, even
/// one with empty fields (upstream does not filter here).
fn parse_text(body: &str) -> Result<Vec<TextResult>> {
    Ok(html::extract_results(body, ITEMS_XPATH, ELEMENTS_XPATH)?
        .into_iter()
        .map(|fields| text_result(&fields))
        .collect())
}

/// Upstream `post_extract_results` (applied to the normalized href).
fn post_extract_results(results: Vec<TextResult>) -> Vec<TextResult> {
    results
        .into_iter()
        .filter(|result| !result.href.starts_with("https://duckduckgo.com/y.js?"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Vec<TextResult> {
        post_extract_results(parse_text(body).unwrap())
    }

    #[test]
    fn matches_upstream_on_real_markup() {
        // Expected output was produced by ddgs 9.16.1's own DuckDuckGo
        // `extract_results` + `post_extract_results` on the same fixture.
        let expected: Vec<TextResult> = serde_json::from_str(include_str!(
            "../../../fixtures/duckduckgo_text_expected.json"
        ))
        .expect("expected json");
        assert_eq!(
            parse(include_str!("../../../fixtures/duckduckgo_text_page.html")),
            expected
        );
    }

    #[test]
    fn parses_text_fixture() {
        let results = parse(include_str!("../../../fixtures/duckduckgo_text_page.html"));
        // The ad is dropped; the empty `no-results body` div is kept.
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://rust-lang.org/");
        assert!(
            results[0]
                .body
                .starts_with("Rust is a fast, reliable, and productive programming language")
        );
        assert_eq!(
            results[1].href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert_eq!(results[3], TextResult::default());
    }

    #[test]
    fn drops_ads_only() {
        let all = parse_text(include_str!("../../../fixtures/duckduckgo_text_page.html")).unwrap();
        assert_eq!(all.len(), 5);
        assert!(all[0].href.starts_with("https://duckduckgo.com/y.js?"));
    }

    #[test]
    fn custom_time_limit_is_sent_raw() {
        let request = TextSearchRequest::new("q").time_limit(Some(crate::TimeLimit::Custom(
            "2024-01-01..2024-02-01".into(),
        )));
        let form = build_payload(&request);
        assert!(form.contains(&("df".to_owned(), "2024-01-01..2024-02-01".to_owned())));
    }

    #[test]
    fn payload_matches_upstream() {
        let request = TextSearchRequest::new(" rust ")
            .region("de-de".parse().unwrap())
            .time_limit(Some(crate::TimeLimit::Month))
            .page(3);
        let form = build_payload(&request);
        let expected: Vec<(String, String)> = [
            ("q", " rust "),
            ("b", ""),
            ("l", "de-de"),
            ("s", "25"),
            ("df", "m"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        assert_eq!(form, expected);
        let first = build_payload(&TextSearchRequest::new("q").page(2));
        assert_eq!(first[3], ("s".to_owned(), "10".to_owned()));
    }

    /// Verifies the selectors still match DuckDuckGo's live HTML markup.
    #[tokio::test]
    #[ignore = "hits live DuckDuckGo"]
    async fn live_duckduckgo_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let request =
            TextSearchRequest::new("rust programming language").region("us-en".parse().unwrap());
        let results = DuckDuckGo::new()
            .search(&http, &request)
            .await
            .expect("search");
        assert!(!results.is_empty(), "duckduckgo returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
