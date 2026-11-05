//! Yahoo text engine — port of `ddgs/engines/yahoo.py` (v9.16.1).
//!
//! `GET https://search.yahoo.com/search;_ylt={token_urlsafe(18)};_ylu={token_urlsafe(35)}`
//! with `p` (query), `b` (`(page - 1) * 7 + 1` for later pages) and `btf`
//! (time limit). The region and safe-search settings are ignored, and upstream
//! sends no extra headers.
//!
//! Parsing is the generic `BaseSearchEngine` loop: items
//! `//div[contains(@class, 'relsrch')]`, title
//! `.//div[contains(@class, 'Title')]//h3//text()`, href
//! `.//div[contains(@class, 'Title')]//a/@href`, body
//! `.//div[contains(@class, 'Text')]//text()`.
//!
//! The XPaths run on libxml2 2.14.6 (the version lxml 6.1.3 bundles), so the
//! tree is the same as upstream's. On real pages Yahoo leaves some `relsrch`
//! divs open at `</li>`; libxml2 ignores that end tag and nests the following
//! items inside the open div, so (as upstream) the outer item joins the texts
//! and hrefs of the nested ones, and `extract_url` keeps only the first
//! `/RU=` target of a joined href.
//!
//! `post_extract_results` drops `https://www.bing.com/aclick?` ads and unwraps
//! `/RU=` redirects; there is no other filter.

use async_trait::async_trait;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{text_result, token_urlsafe};
use crate::html;
use crate::http::HttpClient;
use crate::normalize::{normalize_url, unquote_plus};
use crate::{Result, TextResult, TextSearchRequest};

const NAME: &str = "yahoo";

/// Yahoo HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Yahoo;

impl Yahoo {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Yahoo {
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
        let url = build_url(request)?;
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        // No `max_results` cap here: upstream engines return the whole page and
        // the orchestrator applies the cap.
        Ok(post_extract_results(parse_text(&body)?))
    }
}

/// Upstream `build_payload`: a fresh random `_ylt`/`_ylu` path on every call,
/// then the params in dict order. The query is sent verbatim.
fn build_url(request: &TextSearchRequest) -> Result<Url> {
    // `token_urlsafe(24 * 3 // 4)` and `token_urlsafe(47 * 3 // 4)`.
    let mut url = Url::parse(&format!(
        "https://search.yahoo.com/search;_ylt={};_ylu={}",
        token_urlsafe(24 * 3 / 4)?,
        token_urlsafe(47 * 3 / 4)?
    ))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("p", &request.query);
        if request.page > 1 {
            query.append_pair("b", &((request.page - 1) * 7 + 1).to_string());
        }
        if let Some(limit) = &request.time_limit {
            query.append_pair("btf", limit.as_duckduckgo_df());
        }
    }
    Ok(url)
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[contains(@class, 'relsrch')]";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("title", ".//div[contains(@class, 'Title')]//h3//text()"),
    ("href", ".//div[contains(@class, 'Title')]//a/@href"),
    ("body", ".//div[contains(@class, 'Text')]//text()"),
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

/// Upstream module-level `extract_url`:
/// `unquote_plus(u.split("/RU=", 1)[1].split("/RK=", 1)[0].split("/RS=", 1)[0])`.
fn extract_url(href: &str) -> String {
    let target = href.split_once("/RU=").map_or("", |(_, rest)| rest);
    let target = target.split_once("/RK=").map_or(target, |(head, _)| head);
    let target = target.split_once("/RS=").map_or(target, |(head, _)| head);
    unquote_plus(target)
}

/// Upstream `post_extract_results`, on the already-normalized href.
///
/// Quirk (reproduced): the href is unquoted three times — by the `href`
/// setter during extraction, by `unquote_plus` in `extract_url`, and by the
/// setter again on reassignment.
fn post_extract_results(results: Vec<TextResult>) -> Vec<TextResult> {
    results
        .into_iter()
        .filter(|result| !result.href.starts_with("https://www.bing.com/aclick?"))
        .map(|mut result| {
            if result.href.contains("/RU=") {
                // `result.href = ...` runs the setter (normalize a truthy value).
                result.href = normalize_url(&extract_url(&result.href));
            }
            result
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_upstream_on_real_markup() {
        // Expected output was produced by ddgs 9.16.1's own Yahoo
        // `extract_results` + `post_extract_results` on the same fixture.
        let body = include_str!("../../../fixtures/yahoo_text_page.html");
        let expected: Vec<TextResult> =
            serde_json::from_str(include_str!("../../../fixtures/yahoo_text_expected.json"))
                .expect("expected json");
        let results = post_extract_results(parse_text(body).unwrap());
        // The ad is dropped. Item 2 is a real item whose `relsrch` div is
        // still open at `</li>`: libxml2 ignores that end tag and nests the
        // next items inside it, so their titles/bodies join into item 2. We
        // parse with the same libxml2, so this matches upstream exactly.
        assert_eq!(results.len(), 7);
        assert_eq!(results, expected);
    }

    #[test]
    fn extract_url_matches_upstream() {
        assert_eq!(
            extract_url("https://r.search.yahoo.com/_ylt=X/RU=https://a.example/x+y/RK=2/RS=Z"),
            "https://a.example/x y"
        );
        assert_eq!(extract_url("a/RU=b/RS=c/RK=d"), "b");
        // `href` setter, `unquote_plus`, setter: `%2525` → `%25` → `%` ...
        let results = post_extract_results(vec![TextResult::new(
            "t",
            "https://r.example/RU=https%253A%252F%252Fx.example%252F%25253F/RK=2",
            "",
        )]);
        assert_eq!(results[0].href, "https://x.example/?");
    }

    #[test]
    fn url_matches_upstream() {
        let request = TextSearchRequest::new("rust lang")
            .time_limit(Some(crate::TimeLimit::Day))
            .page(3);
        let url = build_url(&request).unwrap();
        let path = url.path();
        let (_, tokens) = path.split_once(";_ylt=").expect("ylt");
        let (ylt, ylu) = tokens.split_once(";_ylu=").expect("ylu");
        assert_eq!((ylt.len(), ylu.len()), (24, 47));
        assert_eq!(url.query(), Some("p=rust+lang&b=15&btf=d"));
        assert_eq!(
            build_url(&TextSearchRequest::new("q")).unwrap().query(),
            Some("p=q")
        );
    }

    /// Verifies the selectors and RU decoding work on Yahoo's live markup.
    ///
    /// Yahoo intermittently serves a challenge page to datacenter IPs, which
    /// gives no items (upstream gets an empty list too). An empty result is
    /// tolerated; when results come back, the first must look right.
    #[tokio::test]
    #[ignore = "hits live Yahoo"]
    async fn live_yahoo_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Yahoo::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        if results.is_empty() {
            eprintln!("yahoo returned no results (intermittent bot-wall); skipping assertions");
            return;
        }
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
