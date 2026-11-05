//! Startpage text engine — port of `ddgs/engines/startpage.py` (v9.16.1).
//!
//! Upstream flow:
//! 1. `build_payload` splits the region (`country, lang = region.lower().split("-")`)
//!    and only then calls `get_sc()`, so a bad region raises before any
//!    network call.
//! 2. `get_sc`: `GET https://www.startpage.com/` and read
//!    `//form[@id="search"]//input[@name="sc"]/@value` (first match, else `""`).
//! 3. Form-encoded `POST https://www.startpage.com/sp/search` with the payload
//!    keys in upstream dict order.
//!
//! The engine-wide `headers_update` (`Referer: https://www.startpage.com/`) is
//! sent on both requests. Parsing is the generic `BaseSearchEngine` loop:
//! items `//div[contains(@class, 'result css')][./a]`, title `.//h2//text()`,
//! href `./a/@href`, body `.//p//text()`. `post_extract_results` drops hrefs
//! that start with `https://www.google.com`; there is no other filter.

use async_trait::async_trait;

use crate::engines::SearchEngine;
use crate::engines::util::{split_region, text_result};
use crate::html;
use crate::http::HttpClient;
use crate::{Result, SafeSearch, TextResult, TextSearchRequest};

const NAME: &str = "startpage";
const SEARCH_ENDPOINT: &str = "https://www.startpage.com/sp/search";
const HOME: &str = "https://www.startpage.com/";
/// Upstream `headers_update`, applied to every request of the engine's client.
const HEADERS: &[(&str, &str)] = &[("Referer", HOME)];

/// Startpage HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Startpage;

impl Startpage {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Startpage {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "google"`.
    fn provider(&self) -> &'static str {
        "google"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        // Upstream evaluates the region split before `get_sc()`.
        let (country, lang) = split_region(request.region.as_str())?;
        let sc = fetch_sc(http).await?;
        let form = build_payload(request, &country, &lang, &sc);
        let Some(body) = http.post_form_text(SEARCH_ENDPOINT, &form, HEADERS).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        // No `max_results` cap here: upstream engines return the whole page and
        // the orchestrator applies the cap.
        Ok(post_extract_results(parse_text(&body)?))
    }
}

/// Upstream `get_sc`.
///
/// `http_client.request` raises on a transport failure, so that error passes
/// through. It does not check the status: any page is parsed as is, and an
/// empty page fails like lxml's `ParserError`. Cookies
/// the home page sets stay in the engine's cookie store and go out with the
/// search `POST`.
async fn fetch_sc(http: &HttpClient) -> Result<String> {
    extract_sc(&http.get(HOME, HEADERS).await?.text)
}

/// `tree = self.extract_tree(resp_text)` then the `sc` XPath. Like lxml, an
/// empty page raises ("Document is empty"), which fails the engine.
fn extract_sc(body: &str) -> Result<String> {
    let document = html::Document::fromstring(body)?;
    let values = document
        .root()
        .xpath_strings(r#"//form[@id="search"]//input[@name="sc"]/@value"#)?;
    Ok(values.into_iter().next().unwrap_or_default())
}

/// Upstream `build_payload`, in its dict order. The query is sent verbatim
/// (upstream does not strip it).
fn build_payload(
    request: &TextSearchRequest,
    country: &str,
    lang: &str,
    sc: &str,
) -> Vec<(String, String)> {
    let qadf = match request.safe_search {
        SafeSearch::On => "heavy",
        SafeSearch::Moderate => "moderate",
        SafeSearch::Off => "none",
    };
    let mut form: Vec<(String, String)> = [
        ("query", request.query.clone()),
        ("cat", "web".to_owned()),
        ("t", "device".to_owned()),
        ("sc", sc.to_owned()),
        ("lui", "english".to_owned()),
        ("language", "english".to_owned()),
        ("abp", "1".to_owned()),
        ("abd", "0".to_owned()),
        ("abe", "0".to_owned()),
        // `country.upper()` is Unicode-aware, like `to_uppercase`.
        ("qsr", format!("{lang}_{}", country.to_uppercase())),
        ("qadf", qadf.to_owned()),
        ("segment", "organic".to_owned()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    if request.page > 1 {
        form.push(("page".to_owned(), request.page.to_string()));
    }
    if let Some(limit) = &request.time_limit {
        form.push(("with_date".to_owned(), limit.as_duckduckgo_df().to_owned()));
    }
    form
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[contains(@class, 'result css')][./a]";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("title", ".//h2//text()"),
    ("href", "./a/@href"),
    ("body", ".//p//text()"),
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
        .filter(|result| !result.href.starts_with("https://www.google.com"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Vec<TextResult> {
        post_extract_results(parse_text(body).unwrap())
    }

    #[test]
    fn parses_startpage_fixture() {
        let results = parse(include_str!("../../../fixtures/startpage_text_page.html"));
        // The google.com link is dropped by the post-filter; the anchor-less
        // `result css` div and the `results-header` div are not items.
        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|r| !r.href.contains("google.com")));

        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[0].href, "https://rust-lang.org/");
        assert!(results[0].body.contains("blazingly fast"));

        assert_eq!(
            results[1].href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
    }

    #[test]
    fn matches_upstream_output() {
        // Expected output was produced by ddgs 9.16.1's own Startpage
        // `extract_results` + `post_extract_results` on the same fixture.
        let expected: Vec<TextResult> = serde_json::from_str(include_str!(
            "../../../fixtures/startpage_text_expected.json"
        ))
        .expect("expected json");
        assert_eq!(
            parse(include_str!("../../../fixtures/startpage_text_page.html")),
            expected
        );
    }

    #[test]
    fn keeps_items_upstream_keeps() {
        // Upstream has no empty-field, `http`-prefix, or duplicate filter, and
        // `./a/@href` joins the hrefs of every direct-child anchor.
        let results = parse(include_str!("../../../fixtures/startpage_text_page.html"));
        let last = results.last().expect("last item");
        assert_eq!(last.title, "");
        assert_eq!(last.href, "/relative/first/second");
        assert_eq!(results[2].href, results[3].href);
    }

    #[test]
    fn payload_matches_upstream() {
        let request = TextSearchRequest::new(" rust ")
            .region("de-de".parse().unwrap())
            .time_limit(Some(crate::TimeLimit::Week))
            .page(2);
        let form = build_payload(&request, "de", "de", "tok");
        let keys: Vec<&str> = form.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "query",
                "cat",
                "t",
                "sc",
                "lui",
                "language",
                "abp",
                "abd",
                "abe",
                "qsr",
                "qadf",
                "segment",
                "page",
                "with_date"
            ]
        );
        let get = |key: &str| {
            form.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str())
                .unwrap()
        };
        assert_eq!(get("query"), " rust ");
        assert_eq!(get("qsr"), "de_DE");
        assert_eq!(get("qadf"), "moderate");
        assert_eq!(get("sc"), "tok");
        assert_eq!(get("page"), "2");
        assert_eq!(get("with_date"), "w");
    }

    #[test]
    fn extracts_sc_token() {
        let html = r#"<form id="search"><input type="hidden" name="sc"><input type="hidden" name="sc" value="abc123"></form>"#;
        assert_eq!(extract_sc(html).unwrap(), "abc123");
        assert_eq!(extract_sc("<html></html>").unwrap(), "");
        // lxml raises ParserError("Document is empty") on an empty page.
        assert!(extract_sc("").is_err());
    }

    /// Upstream splits the region before `get_sc()`, so a bad region fails
    /// before any network call. The client points at a closed local proxy
    /// with no retries: if a request were made, the error would be a
    /// transport error, not `InvalidConfig`.
    #[tokio::test]
    async fn invalid_region_fails_before_any_request() {
        let http = HttpClient::new(&crate::http::HttpConfig {
            proxy: Some("http://127.0.0.1:9".to_owned()),
            max_retries: 0,
            timeout: Some(std::time::Duration::from_secs(2)),
            ..crate::http::HttpConfig::default()
        })
        .expect("client");
        for region in ["wt", "us-en-x"] {
            let request = TextSearchRequest::new("rust").region(region.parse().unwrap());
            let error = Startpage::new()
                .search(&http, &request)
                .await
                .expect_err("bad region");
            assert!(
                matches!(error, crate::WebSearchError::InvalidConfig(_)),
                "{error:?}"
            );
        }
    }

    /// Verifies the selectors still match Startpage's live markup.
    #[tokio::test]
    #[ignore = "hits live Startpage"]
    async fn live_startpage_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let request =
            TextSearchRequest::new("rust programming language").region("us-en".parse().unwrap());
        let results = Startpage::new()
            .search(&http, &request)
            .await
            .expect("search");
        assert!(!results.is_empty(), "startpage returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
