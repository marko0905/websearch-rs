//! Yahoo News engine — scrapes `news.search.yahoo.com/search`.
//!
//! Yahoo wraps result URLs in `/RU=<encoded>/RK=` redirect paths; this module
//! decodes those back to the publisher URL before returning results.

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::field;
use crate::html;
use crate::http::HttpClient;
use crate::normalize::{iso_seconds_ago, normalize_url, python_int, unquote_plus};
use crate::{NewsResult, NewsSearchRequest, Result};

/// Upstream `name` (the key in `ENGINES[category]`).
const NAME: &str = "yahoo";
const SEARCH_ENDPOINT: &str = "https://news.search.yahoo.com/search";
const PAGE_SIZE: usize = 10;

/// Yahoo News HTML engine.
#[derive(Clone, Debug, Default)]
pub struct YahooNews;

impl YahooNews {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<NewsSearchRequest, NewsResult> for YahooNews {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "yahoo"`.
    fn provider(&self) -> &'static str {
        "yahoo"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &NewsSearchRequest,
    ) -> Result<Vec<NewsResult>> {
        let url = build_url(request)?;
        // Upstream has no `headers_update` for this engine: no extra headers.
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        parse_news(&body)
    }
}

fn build_url(request: &NewsSearchRequest) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("p", &request.query);
        if request.page > 1 {
            query.append_pair("b", &((request.page - 1) * PAGE_SIZE + 1).to_string());
        }
        if let Some(time_limit) = &request.time_limit {
            query.append_pair("btf", time_limit.as_duckduckgo_df());
        }
    }
    Ok(url)
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[@id='web']//li[a]";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("date", ".//span[contains(@class, 'time')]//text()"),
    ("title", ".//h4//text()"),
    ("body", ".//p//text()"),
    ("url", ".//h4/a/@href"),
    ("image", "(.//img/@data-src | .//img/@src)[1]"),
    ("source", ".//span[contains(@class, 'source')]//text()"),
];

/// Upstream `BaseSearchEngine.extract_results` with Yahoo News' XPaths
/// (libxml2, like lxml), then `post_extract_results`.
fn parse_news(body: &str) -> Result<Vec<NewsResult>> {
    let mut results = extract_results(body)?;
    post_extract_results(&mut results);
    Ok(results)
}

/// The raw `extract_results` step (before `post_extract_results`).
fn extract_results(body: &str) -> Result<Vec<NewsResult>> {
    Ok(html::extract_results(body, ITEMS_XPATH, ELEMENTS_XPATH)?
        .into_iter()
        .map(|f| {
            NewsResult::new(
                field(&f, "date"),
                field(&f, "title"),
                field(&f, "body"),
                field(&f, "url"),
                field(&f, "image"),
                field(&f, "source"),
            )
        })
        .collect())
}

/// Port of upstream `post_extract_results`. Upstream quirk, reproduced: the
/// whole loop sits in one `try`, so the first exception (a URL without a
/// `/RU=` redirect → `IndexError`, or a date overflow → `OverflowError`)
/// leaves that result's remaining fields, and every later result,
/// unprocessed.
fn post_extract_results(results: &mut [NewsResult]) {
    for result in results.iter_mut() {
        let Some(date) = extract_date(result.date.as_deref().unwrap_or_default()) else {
            return;
        };
        result.date = Some(date);
        let Some(url) = extract_url(result.url.as_deref().unwrap_or_default()) else {
            return;
        };
        result.url = Some(normalize_url(&url));
        result.image = Some(normalize_url(extract_image(
            result.image.as_deref().unwrap_or_default(),
        )));
        result.source =
            Some(extract_source(result.source.as_deref().unwrap_or_default()).to_owned());
    }
}

/// Upstream `extract_url`: the `/RU=` target up to `/RK=` and `?`,
/// `unquote_plus`-decoded. `None` where upstream raises (`IndexError`).
fn extract_url(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("/RU=")?;
    let target = rest.split("/RK=").next().unwrap_or_default();
    let target = target.split('?').next().unwrap_or_default();
    Some(unquote_plus(target))
}

/// Upstream `extract_image`: the part after the first `-/`, if any.
fn extract_image(image: &str) -> &str {
    image.find("-/").map_or(image, |index| &image[index + 2..])
}

/// Upstream `extract_source`. Upstream quirk, reproduced: the separator has
/// two spaces after `·`, but the value was already whitespace-collapsed, so
/// the split never matches and " · via Yahoo" is kept.
fn extract_source(source: &str) -> &str {
    source.split(" ·  via Yahoo").next().unwrap_or_default()
}

/// Upstream `DATE_RE`.
static RELATIVE_DATE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"(?i)\b(\d+)\s*(year|month|week|day|hour|minute)s?\b").ok());

/// Port of upstream `yahoo_news.extract_date`: "N <unit>s ago" → ISO-8601 UTC
/// of now minus that span (month = 30 days, year = 365 days); otherwise the
/// input unchanged. `None` where upstream raises `OverflowError` (the span
/// exceeds `timedelta`'s 999_999_999 days, or the result is before year 1).
fn extract_date(pub_date: &str) -> Option<String> {
    let Some(caps) = RELATIVE_DATE.as_ref().and_then(|re| re.captures(pub_date)) else {
        return Some(pub_date.to_owned());
    };
    let unit = caps.get(2).map(|m| m.as_str().to_lowercase());
    let (unit_seconds, unit_days): (i64, i64) = match unit.as_deref() {
        Some("minute") => (60, 0),
        Some("hour") => (3_600, 0),
        Some("day") => (86_400, 1),
        Some("week") => (7 * 86_400, 7),
        Some("month") => (30 * 86_400, 30),
        Some("year") => (365 * 86_400, 365),
        _ => return Some(pub_date.to_owned()),
    };
    let number = python_int(caps.get(1)?.as_str())?;
    // `timedelta` raises past 999_999_999 days (sub-day units too, via their
    // day total).
    let days = if unit_days == 0 {
        number.checked_mul(unit_seconds)? / 86_400
    } else {
        number.checked_mul(unit_days)?
    };
    if days > 999_999_999 {
        return None;
    }
    iso_seconds_ago(number.checked_mul(unit_seconds)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_matches_upstream_output() {
        // ddgs 9.16.1's own YahooNews `extract_results` on the same fixture
        // (before the time-dependent `post_extract_results`).
        let expected: Vec<NewsResult> = serde_json::from_str(include_str!(
            "../../../fixtures/yahoo_news_raw_expected.json"
        ))
        .expect("expected json");
        let results =
            extract_results(include_str!("../../../fixtures/yahoo_news_page.html")).unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn parses_yahoo_news_fixture_and_decodes_urls() {
        let body = include_str!("../../../fixtures/yahoo_news_page.html");
        let results = parse_news(body).expect("parse");
        // The third `li` has no direct `<a>` child, so `li[a]` skips it.
        assert_eq!(results.len(), 2);
        let first = &results[0];
        assert_eq!(
            first.title.as_deref(),
            Some("Rust project announces new release")
        );
        assert_eq!(
            first.url.as_deref(),
            Some("https://example.com/rust-release")
        );
        // Upstream's " ·  via Yahoo" split never matches collapsed text.
        assert_eq!(first.source.as_deref(), Some("Example News · via Yahoo"));
        assert!(first.body.as_deref().unwrap_or("").contains("Rust team"));
        assert!(first.date.as_deref().unwrap_or("").ends_with("+00:00"));
        assert_eq!(
            first.image.as_deref(),
            Some("https://example.com/rust-release.jpg")
        );
        assert_eq!(results[1].image.as_deref(), Some(""));
    }

    #[test]
    fn post_extract_stops_at_first_url_without_redirect() {
        let mut results = vec![
            NewsResult::new(
                "1 day ago",
                "a",
                "",
                "https://x.com/RU=https%3A%2F%2Fa.com/RK=2",
                "",
                "",
            ),
            NewsResult::new("2 days ago", "b", "", "https://b.com/direct", "", ""),
            NewsResult::new(
                "3 days ago",
                "c",
                "",
                "https://x.com/RU=https%3A%2F%2Fc.com/RK=2",
                "",
                "",
            ),
        ];
        post_extract_results(&mut results);
        assert_eq!(results[0].url.as_deref(), Some("https://a.com"));
        // The raising result keeps its processed date but its raw URL...
        assert!(results[1].date.as_deref().unwrap_or("").ends_with("+00:00"));
        assert_eq!(results[1].url.as_deref(), Some("https://b.com/direct"));
        // ...and later results are left untouched.
        assert_eq!(results[2].date.as_deref(), Some("3 days ago"));
        assert_eq!(
            results[2].url.as_deref(),
            Some("https://x.com/RU=https://c.com/RK=2")
        );
    }

    #[test]
    fn post_extract_stops_at_date_overflow() {
        let mut results = vec![
            NewsResult::new(
                "1000000000 days ago",
                "a",
                "",
                "https://x.com/RU=https%3A%2F%2Fa.com/RK=2",
                "",
                "",
            ),
            NewsResult::new(
                "1 day ago",
                "b",
                "",
                "https://x.com/RU=https%3A%2F%2Fb.com/RK=2",
                "",
                "",
            ),
        ];
        post_extract_results(&mut results);
        assert_eq!(results[0].date.as_deref(), Some("1000000000 days ago"));
        assert_eq!(
            results[0].url.as_deref(),
            Some("https://x.com/RU=https://a.com/RK=2")
        );
        assert_eq!(results[1].date.as_deref(), Some("1 day ago"));
    }

    #[test]
    fn extract_helpers_match_upstream() {
        assert_eq!(
            extract_url("https://r.search.yahoo.com/_ylt=x/RU=https://e.com/a+b/RK=2/RS=z")
                .as_deref(),
            Some("https://e.com/a b")
        );
        assert_eq!(extract_url("https://e.com/a"), None);
        assert_eq!(
            extract_image("https://s.yimg.com/x/-/https://e.com/i.jpg"),
            "https://e.com/i.jpg"
        );
        assert_eq!(extract_image("https://e.com/i.jpg"), "https://e.com/i.jpg");
        assert_eq!(extract_date("yesterday").as_deref(), Some("yesterday"));
        assert!(
            extract_date("4 Hours ago")
                .unwrap_or_default()
                .ends_with("+00:00")
        );
        assert!(
            extract_date("\u{664} hours ago")
                .unwrap_or_default()
                .ends_with("+00:00")
        );
        assert_eq!(extract_date("1000000 years ago"), None);
        // 999_999_999 days fits `timedelta` but lands before year 1.
        assert_eq!(extract_date("999999999 days ago"), None);
    }

    #[tokio::test]
    #[ignore = "hits live Yahoo News"]
    async fn live_yahoo_news_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = YahooNews::new()
            .search(&http, &NewsSearchRequest::new("rust programming"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "yahoo news returned no results");
        assert!(results[0].url.as_deref().unwrap_or("").starts_with("http"));
    }
}
