//! Mojeek text engine — port of `ddgs/engines/mojeek.py` (v9.16.1).
//!
//! `GET https://www.mojeek.com/search` with `q`, `safe=1` (only for safe
//! search `on`) and `s` (`(page - 1) * 10 + 1`, later pages only). Region goes
//! into the `arc`/`lb` cookies. The time limit is ignored, and upstream sends
//! no extra headers.
//!
//! Parsing is the generic `BaseSearchEngine` loop: items
//! `//ul[contains(@class, 'results')]/li`, title `.//h2//text()`, href
//! `.//h2/a/@href`, body `.//p[@class='s']//text()`. There is no
//! `post_extract_results` filter.

use async_trait::async_trait;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{split_region, text_result};
use crate::html;
use crate::http::HttpClient;
use crate::{Result, SafeSearch, TextResult, TextSearchRequest};

const NAME: &str = "mojeek";
const SEARCH_ENDPOINT: &str = "https://www.mojeek.com/search";

/// Mojeek HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Mojeek;

impl Mojeek {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Mojeek {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "mojeek"`.
    fn provider(&self) -> &'static str {
        "mojeek"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        let (url, cookies) = build_request(request)?;
        // Upstream `self.http_client.client.set_cookies(...)` in
        // `build_payload`: the cookies go into the engine's persistent cookie
        // store, so they (and any from an earlier search, e.g. a `safesearch`
        // cookie a later `moderate` search does not reset) are sent from then on.
        http.set_cookies("https://www.mojeek.com", &cookies);
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        // No `max_results` cap here: upstream engines return the whole page and
        // the orchestrator applies the cap.
        Ok(parse_text(&body)?)
    }
}

/// Upstream `build_payload`: the cookies it sets on the client for
/// `https://www.mojeek.com` (`arc` = country, `lb` = language) and the params
/// in dict order. The query is sent verbatim.
fn build_request(request: &TextSearchRequest) -> Result<(Url, Vec<(String, String)>)> {
    let (country, lang) = split_region(request.region.as_str())?;
    let cookies = vec![("arc".to_owned(), country), ("lb".to_owned(), lang)];
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", &request.query);
        if request.safe_search == SafeSearch::On {
            query.append_pair("safe", "1");
        }
        if request.page > 1 {
            query.append_pair("s", &((request.page - 1) * 10 + 1).to_string());
        }
    }
    Ok((url, cookies))
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//ul[contains(@class, 'results')]/li";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("title", ".//h2//text()"),
    ("href", ".//h2/a/@href"),
    ("body", ".//p[@class='s']//text()"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_upstream_output() {
        // Expected output was produced by ddgs 9.16.1's own Mojeek
        // `extract_results` + `post_extract_results` on the same fixture
        // (which includes hand-made XPath edge cases).
        let expected: Vec<TextResult> =
            serde_json::from_str(include_str!("../../../fixtures/mojeek_text_expected.json"))
                .expect("expected json");
        let results = parse_text(include_str!("../../../fixtures/mojeek_text_page.html")).unwrap();
        assert_eq!(results, expected);
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://rust-lang.org/");
        assert_eq!(results[3].href, "");
    }

    #[test]
    fn request_matches_upstream() {
        let request = TextSearchRequest::new("rust lang")
            .region("UK-en".parse().unwrap())
            .safe_search(SafeSearch::On)
            .time_limit(Some(crate::TimeLimit::Day))
            .page(2);
        let (url, cookie) = build_request(&request).unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.mojeek.com/search?q=rust+lang&safe=1&s=11"
        );
        assert_eq!(
            cookie,
            [
                ("arc".to_owned(), "uk".to_owned()),
                ("lb".to_owned(), "en".to_owned())
            ]
        );
        // Only `on` sends `safe=1` (moderate does not).
        let moderate = TextSearchRequest::new("q").region("us-en".parse().unwrap());
        assert_eq!(
            build_request(&moderate).unwrap().0.as_str(),
            "https://www.mojeek.com/search?q=q"
        );
        assert!(build_request(&TextSearchRequest::new("q").region("wt".parse().unwrap())).is_err());
    }

    /// Verifies the selectors still match Mojeek's live markup.
    #[tokio::test]
    #[ignore = "hits live Mojeek"]
    async fn live_mojeek_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let request =
            TextSearchRequest::new("rust programming language").region("us-en".parse().unwrap());
        let results = Mojeek::new().search(&http, &request).await.expect("search");
        assert!(!results.is_empty(), "mojeek returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
