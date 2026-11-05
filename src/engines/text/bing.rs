//! Bing text engine — port of `ddgs/engines/bing.py` (v9.16.1).
//!
//! **DISABLED to match upstream ddgs v9.16.1** (irrelevant results, since ddgs
//! v9.9.2, commit 17d51cd): upstream sets `disabled = True` and skips the
//! engine in its registry, so it is not registered in
//! [`crate::engines::registry::EngineRegistry`] and cannot be selected by
//! `auto` or by name. The code is kept 1:1 with upstream; to re-enable,
//! register `Bing` in `registry.rs` once upstream does.
//!
//! `GET https://www.bing.com/search` with `q`, `pq`, `cc` (the region's
//! language), `filters` (time limit), and `first`/`FORM` for later pages.
//! `build_payload` also sets the `_EDGE_CD`/`_EDGE_S` cookies on the engine's
//! persistent cookie store. Parsing is the generic `BaseSearchEngine` loop
//! with upstream's XPaths; `post_extract_results` drops `aclick` ads and
//! unwraps `/ck/a?...&u=a1<base64url>` redirects.

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::text_result;
use crate::html;
use crate::http::HttpClient;
use crate::normalize::{normalize_url, unquote_plus};
use crate::{Result, TextResult, TextSearchRequest, TimeLimit, WebSearchError};

const NAME: &str = "bing";
const SEARCH_ENDPOINT: &str = "https://www.bing.com/search";

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//li[contains(@class, 'b_algo')]";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("title", ".//h2/a//text()"),
    ("href", ".//h2/a/@href"),
    ("body", ".//p//text()"),
];

/// Bing HTML text engine.
#[derive(Clone, Debug, Default)]
#[allow(dead_code)] // disabled upstream; not registered (see module docs)
pub struct Bing;

impl Bing {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Bing {
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
        let (url, cookies) = build_request(request, unix_days())?;
        http.set_cookies("https://www.bing.com", &cookies);
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        post_extract_results(parse_text(&body)?)
    }
}

/// `int(time() // 86400)`.
fn unix_days() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

/// Upstream `build_payload`: the params (dict order) and the cookies it sets
/// for `https://www.bing.com`. `region.lower().split("-")` must give exactly
/// two parts, else upstream raises `ValueError`.
fn build_request(request: &TextSearchRequest, days: u64) -> Result<(Url, Vec<(String, String)>)> {
    let lowered = request.region.as_str().to_lowercase();
    let parts: Vec<&str> = lowered.split('-').collect();
    let [country, lang] = parts.as_slice() else {
        return Err(WebSearchError::InvalidConfig(format!(
            "too many values to unpack (expected 2): region `{}`",
            request.region.as_str()
        )));
    };
    let cookies = vec![
        (
            "_EDGE_CD".to_owned(),
            format!("m={lang}-{country}&u={lang}-{country}"),
        ),
        (
            "_EDGE_S".to_owned(),
            format!("mkt={lang}-{country}&ui={lang}-{country}"),
        ),
    ];
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("q", &request.query);
        query.append_pair("pq", &request.query);
        query.append_pair("cc", lang);
        if let Some(limit) = &request.time_limit {
            let code = match limit {
                TimeLimit::Year => format!("ez5_{}_{}", days.saturating_sub(365), days),
                TimeLimit::Day => "ez1".to_owned(),
                TimeLimit::Week => "ez2".to_owned(),
                TimeLimit::Month => "ez3".to_owned(),
                // `{"d": "1", "w": "2", "m": "3"}[timelimit]` raises KeyError.
                TimeLimit::Custom(_) => return Err(limit.key_error()),
            };
            query.append_pair("filters", &format!("ex1:\"{code}\""));
        }
        if request.page > 1 {
            query.append_pair("first", &((request.page - 1) * 10).to_string());
            let form = if request.page > 2 {
                format!("PERE{}", request.page - 2)
            } else {
                "PERE".to_owned()
            };
            query.append_pair("FORM", &form);
        }
    }
    Ok((url, cookies))
}

/// Upstream `BaseSearchEngine.extract_results` with Bing's XPaths, evaluated
/// by libxml2 exactly like lxml.
fn parse_text(body: &str) -> Result<Vec<TextResult>> {
    Ok(html::extract_results(body, ITEMS_XPATH, ELEMENTS_XPATH)?
        .into_iter()
        .map(|fields| text_result(&fields))
        .collect())
}

/// Upstream `post_extract_results`. A decode failure in `unwrap_bing_url`
/// raises upstream, which fails the engine; it does here too.
fn post_extract_results(results: Vec<TextResult>) -> Result<Vec<TextResult>> {
    let mut post = Vec::with_capacity(results.len());
    for mut result in results {
        if result.href.starts_with("https://www.bing.com/aclick?") {
            continue;
        }
        if result.href.starts_with("https://www.bing.com/ck/a?")
            && let Some(unwrapped) = unwrap_bing_url(&result.href)?
        {
            // `result.href = ...` goes through `__setattr__` → `_normalize_url`
            // (only for a non-empty value).
            result.href = if unwrapped.is_empty() {
                unwrapped
            } else {
                normalize_url(&unwrapped)
            };
        }
        post.push(result);
    }
    Ok(post)
}

/// Upstream `unwrap_bing_url`: the first non-blank `u` query value (`parse_qs`),
/// minus its first two characters, padded and decoded with
/// `base64.urlsafe_b64decode`, then UTF-8 decoded. `None` when there is no `u`
/// or it has at most two characters.
fn unwrap_bing_url(raw_url: &str) -> Result<Option<String>> {
    // urlparse(raw_url).query: after the first '?', before the first '#'.
    let query = raw_url
        .split_once('?')
        .map(|(_, rest)| rest.split('#').next().unwrap_or(""))
        .unwrap_or("");
    // parse_qs: split on '&', drop pairs without '=' and blank values.
    let u = query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        let value = unquote_plus(value);
        (unquote_plus(name) == "u" && !value.is_empty()).then_some(value)
    });
    let Some(u) = u else {
        return Ok(None);
    };
    let chars: Vec<char> = u.chars().collect();
    if chars.len() <= 2 {
        return Ok(None);
    }
    let b64_part: String = chars[2..].iter().collect();
    let padding = "=".repeat((4 - b64_part.chars().count() % 4) % 4);
    let decoded = urlsafe_b64decode(&format!("{b64_part}{padding}"))?;
    String::from_utf8(decoded).map(Some).map_err(|error| {
        WebSearchError::Internal(format!("'utf-8' codec can't decode bytes: {error}"))
    })
}

/// Python `base64.urlsafe_b64decode` = `binascii.a2b_base64` in non-strict
/// mode after mapping `-_` to `+/`: invalid characters are skipped, decoding
/// stops once padding completes a quad, and a dangling quad raises.
fn urlsafe_b64decode(input: &str) -> Result<Vec<u8>> {
    if !input.is_ascii() {
        return Err(WebSearchError::Internal(
            "string argument should contain only ASCII characters".to_owned(),
        ));
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut quad_pos = 0u8;
    let mut left = 0u8;
    let mut pads = 0u8;
    let mut data_chars = 0usize;
    for byte in input.bytes() {
        if byte == b'=' {
            pads += 1;
            if quad_pos >= 2 && quad_pos + pads >= 4 {
                return Ok(out);
            }
            continue;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => continue,
        };
        pads = 0;
        data_chars += 1;
        match quad_pos {
            0 => {
                quad_pos = 1;
                left = value;
            }
            1 => {
                quad_pos = 2;
                out.push((left << 2) | (value >> 4));
                left = value & 0x0f;
            }
            2 => {
                quad_pos = 3;
                out.push((left << 4) | (value >> 2));
                left = value & 0x03;
            }
            _ => {
                quad_pos = 0;
                out.push((left << 6) | value);
                left = 0;
            }
        }
    }
    match quad_pos {
        0 => Ok(out),
        1 => Err(WebSearchError::Internal(format!(
            "Invalid base64-encoded string: number of data characters ({data_chars}) cannot be 1 more than a multiple of 4"
        ))),
        _ => Err(WebSearchError::Internal("Incorrect padding".to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_upstream_output() {
        // Expected output was produced by ddgs 9.16.1's own Bing
        // `extract_results` + `post_extract_results` on the same fixture.
        let expected: Vec<TextResult> =
            serde_json::from_str(include_str!("../../../fixtures/bing_text_expected.json"))
                .expect("expected json");
        let results = post_extract_results(
            parse_text(include_str!("../../../fixtures/bing_text_page.html")).unwrap(),
        )
        .unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn request_matches_upstream() {
        let request = TextSearchRequest::new("rust lang")
            .region("DE-de".parse().unwrap())
            .time_limit(Some(TimeLimit::Year))
            .page(3);
        let (url, cookies) = build_request(&request, 20_000).unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.bing.com/search?q=rust+lang&pq=rust+lang&cc=de&filters=ex1%3A%22ez5_19635_20000%22&first=20&FORM=PERE1"
        );
        assert_eq!(
            cookies[0],
            ("_EDGE_CD".to_owned(), "m=de-de&u=de-de".to_owned())
        );
        assert_eq!(
            cookies[1],
            ("_EDGE_S".to_owned(), "mkt=de-de&ui=de-de".to_owned())
        );

        let request = TextSearchRequest::new("q")
            .time_limit(Some(TimeLimit::Week))
            .page(2);
        let (url, _) = build_request(&request, 1).unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.bing.com/search?q=q&pq=q&cc=en&filters=ex1%3A%22ez2%22&first=10&FORM=PERE"
        );
        assert!(
            build_request(
                &TextSearchRequest::new("q").region("wt".parse().unwrap()),
                1
            )
            .is_err()
        );
    }

    #[test]
    fn unwrap_bing_url_like_python() {
        // base64url("https://example.com/a") = aHR0cHM6Ly9leGFtcGxlLmNvbS9h
        assert_eq!(
            unwrap_bing_url(
                "https://www.bing.com/ck/a?!&&p=x&u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9h&ntb=1"
            )
            .unwrap()
            .as_deref(),
            Some("https://example.com/a")
        );
        assert_eq!(
            unwrap_bing_url("https://www.bing.com/ck/a?p=x").unwrap(),
            None
        );
        assert_eq!(
            unwrap_bing_url("https://www.bing.com/ck/a?u=a1").unwrap(),
            None
        );
        // One data character more than a multiple of 4 raises.
        assert!(unwrap_bing_url("https://www.bing.com/ck/a?u=a1aHR0c").is_err());
    }

    #[test]
    fn b64decode_skips_invalid_and_stops_at_padding() {
        // Python: base64.urlsafe_b64decode("aG*k=") == b"hi" (invalid chars skipped).
        assert_eq!(urlsafe_b64decode("aG*k=").unwrap(), b"hi");
        assert!(
            urlsafe_b64decode("aGk")
                .unwrap_err()
                .to_string()
                .contains("Incorrect padding")
        );
        assert_eq!(urlsafe_b64decode("aGk=").unwrap(), b"hi");
        assert_eq!(urlsafe_b64decode("a!G!k=").unwrap(), b"hi");
        assert_eq!(urlsafe_b64decode("aGk=garbage").unwrap(), b"hi");
    }

    /// Verifies the XPaths on Bing's live markup.
    #[tokio::test]
    #[ignore = "hits live Bing"]
    async fn live_bing_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Bing::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "bing returned no results");
    }
}
