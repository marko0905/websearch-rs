//! Brave text engine — port of `ddgs/engines/brave.py` (v9.16.1).
//!
//! `GET https://search.brave.com/search` with `q`, `source=web`, `tf` (time
//! limit) and `offset` (`page - 1`, later pages only). Region and safe search
//! go into cookies, not params. Upstream sends no extra headers.
//!
//! Parsing is the generic `BaseSearchEngine` loop: items
//! `//div[@data-type='web']`, title
//! `.//div[(contains(@class,'title') or contains(@class,'sitename-container')) and position()=last()]//text()`,
//! href `.//a[div[contains(@class, 'title')]]/@href`, body
//! `.//div[contains(@class, 'snippet')]//div[contains(@class, 'content')]//text()`.
//! There is no `post_extract_results` filter.

use async_trait::async_trait;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{split_region, text_result};
use crate::html;
use crate::http::HttpClient;
use crate::{Result, SafeSearch, TextResult, TextSearchRequest};

const NAME: &str = "brave";
const SEARCH_ENDPOINT: &str = "https://search.brave.com/search";

/// Brave HTML text engine.
#[derive(Clone, Debug, Default)]
pub struct Brave;

impl Brave {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Brave {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "brave"`.
    fn provider(&self) -> &'static str {
        "brave"
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
        http.set_cookies("https://search.brave.com", &cookies);
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        // No `max_results` cap here: upstream engines return the whole page and
        // the orchestrator applies the cap.
        parse_text(&body)
    }
}

/// Upstream `build_payload`: the params (dict order) and the cookies it sets
/// on the client for `https://search.brave.com`.
///
/// Quirk (reproduced): upstream builds `{country: country, ...}`, so the
/// cookie is named after the country value (`us=us`), not `country=us`.
///
/// The cookies go into the engine's persistent cookie store, as upstream sets
/// them on its cached, long-lived client: a `safesearch` cookie from an earlier
/// call stays on later `moderate` calls (upstream quirk, reproduced).
fn build_request(request: &TextSearchRequest) -> Result<(Url, Vec<(String, String)>)> {
    let (country, _lang) = split_region(request.region.as_str())?;
    let mut cookies = vec![
        (country.clone(), country),
        ("useLocation".to_owned(), "0".to_owned()),
    ];
    match request.safe_search {
        SafeSearch::Moderate => {}
        SafeSearch::On => cookies.push(("safesearch".to_owned(), "strict".to_owned())),
        SafeSearch::Off => cookies.push(("safesearch".to_owned(), "off".to_owned())),
    }

    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", &request.query);
        query.append_pair("source", "web");
        if let Some(limit) = &request.time_limit {
            // `{"d": "pd", ...}[timelimit]`: any other value raises KeyError.
            let tf = match limit {
                crate::TimeLimit::Day => "pd",
                crate::TimeLimit::Week => "pw",
                crate::TimeLimit::Month => "pm",
                crate::TimeLimit::Year => "py",
                crate::TimeLimit::Custom(_) => return Err(limit.key_error()),
            };
            query.append_pair("tf", tf);
        }
        if request.page > 1 {
            query.append_pair("offset", &(request.page - 1).to_string());
        }
    }
    Ok((url, cookies))
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[@data-type='web']";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    (
        "title",
        ".//div[(contains(@class,'title') or contains(@class,'sitename-container')) and position()=last()]//text()",
    ),
    ("href", ".//a[div[contains(@class, 'title')]]/@href"),
    (
        "body",
        ".//div[contains(@class, 'snippet')]//div[contains(@class, 'content')]//text()",
    ),
];

/// Upstream `BaseSearchEngine.extract_results` with Brave's XPaths (libxml2,
/// like lxml). Every item gives a result, even one with empty fields
/// (upstream does not filter). There is no `post_extract_results`.
fn parse_text(body: &str) -> Result<Vec<TextResult>> {
    Ok(html::extract_results(body, ITEMS_XPATH, ELEMENTS_XPATH)?
        .into_iter()
        .map(|fields| text_result(&fields))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie_header(cookies: &[(String, String)]) -> String {
        cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    #[test]
    fn matches_upstream_output() {
        // Expected output was produced by ddgs 9.16.1's own Brave
        // `extract_results` + `post_extract_results` on the same fixture
        // (which includes hand-made XPath edge cases).
        let expected: Vec<TextResult> =
            serde_json::from_str(include_str!("../../../fixtures/brave_text_expected.json"))
                .expect("expected json");
        let results = parse_text(include_str!("../../../fixtures/brave_text_page.html")).unwrap();
        assert_eq!(results, expected);
        assert_eq!(results.len(), 6);
        assert_eq!(results[0].title, "Rust Programming Language");
        assert_eq!(results[3].title, "Example OrgExample title");
        assert_eq!(results[4].title, "");
        assert_eq!(results[4].body, "B1B2");
    }

    #[test]
    fn custom_time_limit_raises_key_error() {
        let request =
            TextSearchRequest::new("q").time_limit(Some(crate::TimeLimit::Custom("2024".into())));
        let error = build_request(&request).unwrap_err();
        assert_eq!(error.to_string(), "'2024'");
    }

    #[test]
    fn request_matches_upstream() {
        let request = TextSearchRequest::new("rust lang")
            .region("DE-de".parse().unwrap())
            .safe_search(SafeSearch::On)
            .time_limit(Some(crate::TimeLimit::Year))
            .page(3);
        let (url, cookie) = build_request(&request).unwrap();
        assert_eq!(
            url.as_str(),
            "https://search.brave.com/search?q=rust+lang&source=web&tf=py&offset=2"
        );
        assert_eq!(
            cookie_header(&cookie),
            "de=de; useLocation=0; safesearch=strict"
        );

        let (url, cookie) =
            build_request(&TextSearchRequest::new("q").region("us-en".parse().unwrap())).unwrap();
        assert_eq!(
            url.as_str(),
            "https://search.brave.com/search?q=q&source=web"
        );
        assert_eq!(cookie_header(&cookie), "us=us; useLocation=0");

        let off = TextSearchRequest::new("q")
            .region("us-en".parse().unwrap())
            .safe_search(SafeSearch::Off);
        assert_eq!(
            cookie_header(&build_request(&off).unwrap().1),
            "us=us; useLocation=0; safesearch=off"
        );
        // `region.lower().split("-")` must give two parts.
        assert!(build_request(&TextSearchRequest::new("q").region("wt".parse().unwrap())).is_err());
    }

    /// Verifies the selectors still match Brave's live (hashed-class) markup.
    #[tokio::test]
    #[ignore = "hits live Brave"]
    async fn live_brave_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let request =
            TextSearchRequest::new("rust programming language").region("us-en".parse().unwrap());
        let results = Brave::new().search(&http, &request).await.expect("search");
        assert!(!results.is_empty(), "brave returned no results");
        assert!(results[0].href.starts_with("http"));
        assert!(!results[0].title.is_empty());
    }
}
