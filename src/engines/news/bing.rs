//! Bing News engine — scrapes `www.bing.com/news/infinitescrollajax`.
//!
//! Bing's news endpoint returns HTML fragments for infinite-scroll pages. News
//! items expose most fields as attributes (`data-title`, `url`, `data-author`)
//! with snippet/date/image inside the block.

use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{field, split_region};
use crate::html;
use crate::http::HttpClient;
use crate::normalize::{
    days_from_civil, days_in_month, host_local, iso_seconds_ago, naive_local_to_utc, normalize_url,
    python_int, timestamp_to_iso,
};
use crate::{NewsResult, NewsSearchRequest, Result, TimeLimit, WebSearchError};

/// Upstream `name` (the key in `ENGINES[category]`).
const NAME: &str = "bing";
const SEARCH_ENDPOINT: &str = "https://www.bing.com/news/infinitescrollajax";

/// Bing News HTML-fragment engine.
#[derive(Clone, Debug, Default)]
pub struct BingNews;

impl BingNews {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<NewsSearchRequest, NewsResult> for BingNews {
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
    // Upstream `country, lang = region.lower().split("-")`: sent as is
    // (`wt-wt` gives `cc=wt`); any other shape raises before the request.
    let (country, lang) = split_region(request.region.as_str())?;
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", &request.query);
        query.append_pair("InfiniteScroll", "1");
        query.append_pair("first", &(request.page * 10 + 1).to_string());
        query.append_pair("SFX", &request.page.to_string());
        query.append_pair("cc", &country);
        query.append_pair("setlang", &lang);
        if let Some(time_limit) = &request.time_limit {
            query.append_pair("qft", bing_time_filter(time_limit)?);
        }
    }
    Ok(url)
}

/// Upstream's `{"d": ..., "w": ..., "m": ..., "y": ...}[timelimit]`; any
/// other value raises `KeyError` (the engine fails).
fn bing_time_filter(time_limit: &TimeLimit) -> Result<&'static str> {
    match time_limit {
        TimeLimit::Day => Ok("interval=\"4\""),
        TimeLimit::Week => Ok("interval=\"7\""),
        TimeLimit::Month | TimeLimit::Year => Ok("interval=\"9\""),
        TimeLimit::Custom(_) => Err(time_limit.key_error()),
    }
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[contains(@class, 'newsitem')]";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("date", ".//span[@aria-label]//@aria-label"),
    ("title", "@data-title"),
    ("body", ".//div[@class='snippet']//text()"),
    ("url", "@url"),
    ("image", ".//a[contains(@class, 'image')]//@src"),
    ("source", "@data-author"),
];

/// Upstream `BaseSearchEngine.extract_results` with Bing News' XPaths
/// (libxml2, like lxml; the response is an HTML fragment, so
/// `lxml.html.fromstring`'s fragment rules apply), then
/// `post_extract_results`.
fn parse_news(body: &str) -> Result<Vec<NewsResult>> {
    let mut results = extract_results(body)?;
    post_extract_results(&mut results, &host_local)?;
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

/// Upstream `post_extract_results`: re-assigns only `date` and `image` (each
/// re-assignment runs that field's normalizer again; `date` has none for
/// strings). Not in a `try`, so an `extract_date` error fails the engine.
fn post_extract_results(
    results: &mut [NewsResult],
    local: &impl Fn(i64) -> Option<i64>,
) -> Result<()> {
    for result in results.iter_mut() {
        if let Some(date) = result.date.as_deref() {
            result.date = Some(extract_date(date, local)?);
        }
        let image = result.image.as_deref().unwrap_or_default();
        result.image = Some(if image.is_empty() {
            String::new()
        } else {
            let path = image.split('&').next().unwrap_or_default();
            normalize_url(&format!("https://www.bing.com{path}"))
        });
    }
    Ok(())
}

/// `%d`, `%m`, `%Y` as CPython's `_strptime` matches them (`\d` is Unicode).
const STRPTIME_D: &str = r"(3[01]|[12]\d|0[1-9]|[1-9]| [1-9])";
const STRPTIME_M: &str = r"(1[0-2]|0[1-9]|[1-9])";
const STRPTIME_Y: &str = r"(\d\d\d\d)";

/// Upstream's `date_formats`: `%d.%m.%Y`, `%m/%d/%Y`, `%d/%m/%Y`, as
/// `(regex, day group, month group)`. `_strptime` compiles with IGNORECASE and
/// requires the match to consume the whole string.
static DATE_FORMATS: LazyLock<Vec<(Regex, usize, usize)>> = LazyLock::new(|| {
    [
        (
            format!(r"(?i)^{STRPTIME_D}\.{STRPTIME_M}\.{STRPTIME_Y}$"),
            1,
            2,
        ),
        (
            format!(r"(?i)^{STRPTIME_M}/{STRPTIME_D}/{STRPTIME_Y}$"),
            2,
            1,
        ),
        (
            format!(r"(?i)^{STRPTIME_D}/{STRPTIME_M}/{STRPTIME_Y}$"),
            1,
            2,
        ),
    ]
    .into_iter()
    .filter_map(|(pattern, day, month)| Some((Regex::new(&pattern).ok()?, day, month)))
    .collect()
});

/// Upstream `DATE_RE`. Upstream quirk, reproduced: the unit group is optional,
/// so any standalone number is read as a day count ("5 hours ago" → 5 days ago,
/// "31.02.2026" → 31 days ago).
static RELATIVE_DATE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(\d+)\s*(days|tagen|jours|giorni|dias|días|дн\.|день)?\b").ok()
});

/// Port of upstream `bing_news.extract_date`, with the host-zone lookup
/// injectable (`local`, see [`naive_local_to_utc`]) so tests are deterministic.
///
/// - An absolute date is parsed as a naive midnight and converted with
///   `.astimezone(timezone.utc)`, i.e. read in the host's local time zone, as
///   upstream does (so the result depends on the host's `TZ`, like Python's).
/// - Otherwise the first number is a day count back from now.
/// - Otherwise the text is returned unchanged.
///
/// Where upstream raises (`OverflowError` for a day count beyond `timedelta`'s
/// range or before year 1, or a failing `localtime`), this returns an error,
/// which fails the engine as it does upstream.
fn extract_date(pub_date: &str, local: &impl Fn(i64) -> Option<i64>) -> Result<String> {
    let overflow = || WebSearchError::Parse(format!("bing news: date out of range: {pub_date}"));
    for (pattern, day_group, month_group) in DATE_FORMATS.iter() {
        let Some(caps) = pattern.captures(pub_date) else {
            continue;
        };
        let field = |group: usize| caps.get(group).map_or("", |m| m.as_str()).trim();
        let (Some(day), Some(month), Some(year)) = (
            python_int(field(*day_group)).and_then(|day| u32::try_from(day).ok()),
            python_int(field(*month_group)).and_then(|month| u32::try_from(month).ok()),
            python_int(field(3)),
        ) else {
            continue;
        };
        // `strptime` raises ValueError (suppressed upstream) for year 0 or a
        // day past the month's end; try the next format.
        if year < 1 || days_in_month(year, month).is_none_or(|max| day > max) {
            continue;
        }
        let naive = days_from_civil(year, month, day) * 86_400;
        let utc = naive_local_to_utc(naive, local).ok_or_else(overflow)?;
        return timestamp_to_iso(utc).ok_or_else(overflow);
    }

    if let Some(caps) = RELATIVE_DATE.as_ref().and_then(|re| re.captures(pub_date)) {
        let days = caps.get(1).map_or("", |m| m.as_str());
        // `timedelta(days=n)` raises past 999_999_999 days.
        return python_int(days)
            .filter(|days| *days <= 999_999_999)
            .and_then(|days| days.checked_mul(86_400))
            .and_then(iso_seconds_ago)
            .ok_or_else(overflow);
    }

    Ok(pub_date.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_sends_region_as_is() {
        let request = NewsSearchRequest::new(" rust").region("WT-WT".parse().unwrap());
        assert_eq!(
            build_url(&request).unwrap().query(),
            Some("q=+rust&InfiniteScroll=1&first=11&SFX=1&cc=wt&setlang=wt")
        );
        let bad = NewsSearchRequest::new("rust").region("wt".parse().unwrap());
        assert!(build_url(&bad).is_err());
    }

    /// A fixed host zone for deterministic tests.
    fn utc(u: i64) -> Option<i64> {
        Some(u)
    }

    #[test]
    fn extract_matches_upstream_output() {
        // ddgs 9.16.1's own BingNews `extract_results` on the same fixture
        // (before the time-dependent `post_extract_results`).
        let expected: Vec<NewsResult> = serde_json::from_str(include_str!(
            "../../../fixtures/bing_news_raw_expected.json"
        ))
        .expect("expected json");
        let results =
            extract_results(include_str!("../../../fixtures/bing_news_page.html")).unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn parses_bing_news_fixture() {
        let body = include_str!("../../../fixtures/bing_news_page.html");
        let results = parse_news(body).expect("parse");
        assert_eq!(results.len(), 2);
        let first = &results[0];
        assert_eq!(
            first.title.as_deref(),
            Some("Rust language release improves tooling")
        );
        assert_eq!(first.url.as_deref(), Some("https://example.com/rust-news"));
        assert_eq!(first.source.as_deref(), Some("Example News"));
        assert!(
            first
                .body
                .as_deref()
                .unwrap_or("")
                .contains("compiler and Cargo")
        );
        // `/th?id=ON.123&pid=News` → cut at `&`, prefixed with the Bing host.
        assert_eq!(
            first.image.as_deref(),
            Some("https://www.bing.com/th?id=ON.123")
        );
        assert_eq!(results[1].image.as_deref(), Some(""));
        // "2 days ago" → an ISO timestamp two days back.
        assert!(first.date.as_deref().unwrap_or("").ends_with("+00:00"));
    }

    #[test]
    fn title_and_url_are_normalized_once() {
        let body = r#"<div class="newsitem" data-title="a &amp;lt;b&amp;gt; c" url="https://e.com/x%2520y"></div>"#;
        let results = parse_news(body).expect("parse");
        // The attribute holds "a &lt;b&gt; c"; one unescape gives "a <b> c".
        assert_eq!(results[0].title.as_deref(), Some("a <b> c"));
        // One unquote of "%2520" gives "%20".
        assert_eq!(results[0].url.as_deref(), Some("https://e.com/x%20y"));
    }

    #[test]
    fn keeps_items_upstream_keeps() {
        let body = r#"<div class="newsitem"></div><div class="newsitem"></div>"#;
        assert_eq!(parse_news(body).expect("parse").len(), 2);
    }

    #[test]
    fn extract_date_absolute_formats() {
        let date = |text: &str| extract_date(text, &utc).expect("date");
        assert_eq!(date("04.11.2026"), "2026-11-04T00:00:00+00:00");
        assert_eq!(date("11/04/2026"), "2026-11-04T00:00:00+00:00");
        // %m/%d/%Y fails (no month 25) → %d/%m/%Y.
        assert_eq!(date("25/12/2026"), "2026-12-25T00:00:00+00:00");
        // Invalid calendar date falls through to the relative regex: "31" → days.
        assert!(date("31.02.2026").ends_with("+00:00"));
        // `%d` is `3[01]|[12]\d|0[1-9]|[1-9]| [1-9]`: only the second digit of
        // `[12]\d` may be a Unicode digit, which `int()` accepts.
        assert_eq!(date("1\u{664}.11.2026"), "2026-11-14T00:00:00+00:00");
        // A lone non-ASCII day fails `strptime`; the relative regex reads it as
        // a day count instead.
        assert_ne!(date("\u{664}.11.2026"), "2026-11-04T00:00:00+00:00");
    }

    #[test]
    fn extract_date_uses_host_local_zone() {
        // Host at UTC+2: local midnight is 22:00 UTC the previous day.
        let plus_two = |u: i64| Some(u + 7200);
        assert_eq!(
            extract_date("04.11.2026", &plus_two).expect("date"),
            "2026-11-03T22:00:00+00:00"
        );
        // Host at UTC-5.
        let minus_five = |u: i64| Some(u - 5 * 3600);
        assert_eq!(
            extract_date("11/04/2026", &minus_five).expect("date"),
            "2026-11-04T05:00:00+00:00"
        );
        // A failing localtime raises upstream.
        assert!(extract_date("04.11.2026", &|_| None).is_err());
    }

    #[test]
    fn extract_date_relative_and_passthrough() {
        let expected = crate::normalize::iso_seconds_ago(3 * 86_400).expect("iso");
        let got = extract_date("3 days ago", &utc).expect("date");
        // Allow for a second boundary between the two clock reads.
        assert_eq!(got.len(), expected.len());
        assert_eq!(got[..16], expected[..16]);
        assert_eq!(extract_date("yesterday", &utc).unwrap(), "yesterday");
        assert_eq!(extract_date("2h", &utc).unwrap(), "2h");
    }

    #[test]
    fn extract_date_overflow_fails_the_engine() {
        assert!(extract_date("1000000000 days", &utc).is_err());
        assert!(extract_date("99999999 days", &utc).is_err()); // before year 1
        let body = r#"<div class="newsitem"><span aria-label="1000000000 days"></span></div>"#;
        assert!(parse_news(body).is_err());
    }

    #[tokio::test]
    #[ignore = "hits live Bing News"]
    async fn live_bing_news_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = BingNews::new()
            .search(&http, &NewsSearchRequest::new("rust programming"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "bing news returned no results");
        assert!(results[0].url.as_deref().unwrap_or("").starts_with("http"));
    }
}
