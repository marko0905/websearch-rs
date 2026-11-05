//! Wikipedia text engine — port of `ddgs/engines/wikipedia.py` (v9.16.1).
//!
//! Upstream flow:
//! 1. `GET https://{lang}.wikipedia.org/w/api.php?action=opensearch&profile=fuzzy&limit=1&search={quote(query)}`
//!    (no other params). `lang` is the second half of `region.lower().split("-")`.
//! 2. If the opensearch list of titles (`json[1]`) is empty, no result.
//!    Otherwise one result: `title = json[1][0]`, `href = json[3][0]`.
//! 3. `GET https://{lang}.wikipedia.org/w/api.php?action=query&format=json&prop=extracts&titles={quote(title)}&explaintext=0&exintro=0&redirects=1`
//!    and use the first page's `extract` as the body (non-200 → body stays `""`).
//! 4. A body that contains `"may refer to:"` (a disambiguation page) gives no
//!    result.
//!
//! So this engine returns at most ONE result. Upstream sends no extra headers.

use async_trait::async_trait;
use serde_json::Value;

use crate::engines::SearchEngine;
use crate::engines::util::{py_truthy, split_region};
use crate::http::HttpClient;
use crate::normalize::{normalize_text, quote};
use crate::{Result, TextResult, TextSearchRequest, WebSearchError};

const NAME: &str = "wikipedia";

/// Wikipedia opensearch + extracts engine.
#[derive(Clone, Debug, Default)]
pub struct Wikipedia;

impl Wikipedia {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Wikipedia {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "wikipedia"`.
    fn provider(&self) -> &'static str {
        "wikipedia"
    }

    fn priority(&self) -> f64 {
        // Upstream `priority = 2` (engines run in descending priority).
        2.0
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &TextSearchRequest,
    ) -> Result<Vec<TextResult>> {
        // Upstream `build_payload`: `_country, lang = region.lower().split("-")`.
        // A region such as `wt-wt` gives lang `wt` and the host
        // `wt.wikipedia.org`, which does not exist; upstream has the same
        // behavior (its default region is `us-en`).
        let (_country, lang) = split_region(request.region.as_str())?;
        // The query is sent verbatim (upstream does not strip it).
        let Some(body) = http
            .get_text(&search_url(&lang, &request.query), &[])
            .await?
        else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        let Some(mut result) = parse_opensearch(&body)? else {
            return Ok(Vec::new());
        };

        // `self.request(...)` again: non-200 → `None`.
        let extract = http
            .get_text(&extracts_url(&lang, &result.title), &[])
            .await?;
        // Upstream `if resp_data:` — an empty body is skipped too.
        if let Some(extract) = extract.filter(|text| !text.is_empty()) {
            result.body = parse_extract(&extract)?;
        }
        Ok(finish(result))
    }
}

fn search_url(lang: &str, query: &str) -> String {
    format!(
        "https://{lang}.wikipedia.org/w/api.php?action=opensearch&profile=fuzzy&limit=1&search={}",
        quote(query)
    )
}

/// The extracts request. `title` is the already-normalized result title, as
/// upstream reads `result.title` after its setter normalized it.
fn extracts_url(lang: &str, title: &str) -> String {
    format!(
        "https://{lang}.wikipedia.org/w/api.php?action=query&format=json&prop=extracts&titles={}&explaintext=0&exintro=0&redirects=1",
        quote(title)
    )
}

fn parse_error(message: impl Into<String>) -> WebSearchError {
    WebSearchError::Parse(format!("wikipedia: {}", message.into()))
}

/// `json.loads(text)` then `json_data[1]` / `[1][0]` / `[3][0]`.
///
/// Returns `Ok(None)` when `json_data[1]` is falsy. Every lookup that raises
/// in Python (`KeyError`/`IndexError`/`TypeError`) is a parse error here.
fn parse_opensearch(body: &str) -> Result<Option<TextResult>> {
    let json: Value = serde_json::from_str(body).map_err(|err| parse_error(err.to_string()))?;
    let titles = json
        .get(1)
        .ok_or_else(|| parse_error("opensearch response has no title list"))?;
    if !py_truthy(titles) {
        return Ok(None);
    }
    let title = titles
        .get(0)
        .and_then(Value::as_str)
        .ok_or_else(|| parse_error("opensearch title is missing or not a string"))?;
    let href = json
        .get(3)
        .and_then(|links| links.get(0))
        .and_then(Value::as_str)
        .ok_or_else(|| parse_error("opensearch link is missing or not a string"))?;
    // Constructor = upstream setter normalization of `title` and `href`.
    Ok(Some(TextResult::new(title, href, "")))
}

/// `next(iter(json["query"]["pages"].values())).get("extract", "")`, then the
/// `body` setter (normalize a truthy string).
///
/// A missing key, an empty `pages` map (`StopIteration`), and a `null` or
/// non-string extract (later `TypeError`) all raise upstream, so they are
/// parse errors here.
fn parse_extract(body: &str) -> Result<String> {
    let json: Value = serde_json::from_str(body).map_err(|err| parse_error(err.to_string()))?;
    let page = json
        .get("query")
        .and_then(|query| query.get("pages"))
        .and_then(Value::as_object)
        .ok_or_else(|| parse_error("extracts response has no query.pages"))?
        .values()
        .next()
        .ok_or_else(|| parse_error("extracts response has no page"))?;
    match page.get("extract") {
        None => Ok(String::new()),
        Some(Value::String(text)) if text.is_empty() => Ok(String::new()),
        Some(Value::String(text)) => Ok(normalize_text(text)),
        Some(_) => Err(parse_error("extract is not a string")),
    }
}

/// Upstream `if "may refer to:" in result.body: return []`.
fn finish(result: TextResult) -> Vec<TextResult> {
    if result.body.contains("may refer to:") {
        Vec::new()
    } else {
        vec![result]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_upstream_urls() {
        assert_eq!(
            search_url("en", "rust programming/lang?"),
            "https://en.wikipedia.org/w/api.php?action=opensearch&profile=fuzzy&limit=1&search=rust%20programming/lang%3F"
        );
        assert_eq!(
            extracts_url("de", "Rust (programming language)"),
            "https://de.wikipedia.org/w/api.php?action=query&format=json&prop=extracts&titles=Rust%20%28programming%20language%29&explaintext=0&exintro=0&redirects=1"
        );
    }

    #[test]
    fn parses_opensearch_and_extract_fixtures() {
        let open = include_str!("../../../fixtures/wikipedia_opensearch.json");
        let mut result = parse_opensearch(open).unwrap().expect("one result");
        assert_eq!(result.title, "Rust (programming language)");
        assert_eq!(
            result.href,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        result.body =
            parse_extract(include_str!("../../../fixtures/wikipedia_extracts.json")).unwrap();
        assert!(
            result
                .body
                .starts_with("Rust is a general-purpose programming language")
        );
        let results = finish(result);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn disambiguation_page_gives_no_result() {
        let open = include_str!("../../../fixtures/wikipedia_opensearch_disambiguation.json");
        let mut result = parse_opensearch(open).unwrap().expect("one result");
        result.body = parse_extract(include_str!(
            "../../../fixtures/wikipedia_extracts_disambiguation.json"
        ))
        .unwrap();
        assert!(finish(result).is_empty());
    }

    #[test]
    fn may_also_refer_to_is_kept() {
        // Quirk (reproduced): upstream checks only the exact substring
        // "may refer to:", so a real disambiguation page whose extract says
        // "may also refer to:" (here: Mercury) is returned as the result.
        let mut result = TextResult::new("Mercury", "https://en.wikipedia.org/wiki/Mercury", "");
        result.body = parse_extract(include_str!(
            "../../../fixtures/wikipedia_extracts_may_also_refer.json"
        ))
        .unwrap();
        assert!(result.body.ends_with("The Mercury may also refer to:"));
        assert_eq!(finish(result).len(), 1);
    }

    #[test]
    fn matches_upstream_expected_output() {
        // Expected files: upstream ddgs 9.16.1 output (`scripts/regen_expected.py`).
        let cases = [
            (
                include_str!("../../../fixtures/wikipedia_opensearch.json"),
                include_str!("../../../fixtures/wikipedia_extracts.json"),
                include_str!("../../../fixtures/wikipedia_text_expected.json"),
            ),
            (
                include_str!("../../../fixtures/wikipedia_opensearch_disambiguation.json"),
                include_str!("../../../fixtures/wikipedia_extracts_disambiguation.json"),
                include_str!("../../../fixtures/wikipedia_disambiguation_expected.json"),
            ),
            (
                include_str!("../../../fixtures/wikipedia_opensearch_may_also_refer.json"),
                include_str!("../../../fixtures/wikipedia_extracts_may_also_refer.json"),
                include_str!("../../../fixtures/wikipedia_may_also_refer_expected.json"),
            ),
        ];
        for (open, extracts, expected) in cases {
            let expected: Vec<TextResult> = serde_json::from_str(expected).expect("expected json");
            let mut got = Vec::new();
            if let Some(mut result) = parse_opensearch(open).unwrap() {
                result.body = parse_extract(extracts).unwrap();
                got = finish(result);
            }
            assert_eq!(got, expected);
        }
    }

    #[test]
    fn empty_title_list_gives_no_result() {
        assert!(
            parse_opensearch(r#"["zzqqxx",[],[],[]]"#)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn upstream_errors_are_parse_errors() {
        // `json_data[1]` on an error object raises KeyError upstream.
        assert!(parse_opensearch(r#"{"error":{"code":"x"}}"#).is_err());
        // `json_data[3][0]` IndexError.
        assert!(parse_opensearch(r#"["q",["T"],[""],[]]"#).is_err());
        // Empty `pages` → StopIteration.
        assert!(parse_extract(r#"{"query":{"pages":{}}}"#).is_err());
        // Missing extract → "".
        assert_eq!(
            parse_extract(r#"{"query":{"pages":{"-1":{"missing":""}}}}"#).unwrap(),
            ""
        );
    }

    #[test]
    fn region_must_have_two_parts() {
        assert_eq!(
            split_region("DE-de").unwrap(),
            ("de".to_owned(), "de".to_owned())
        );
        assert!(split_region("wt").is_err());
        assert!(split_region("a-b-c").is_err());
    }

    /// Verifies the live opensearch + extracts flow.
    #[tokio::test]
    #[ignore = "hits live Wikipedia"]
    async fn live_wikipedia_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let request =
            TextSearchRequest::new("rust programming language").region("us-en".parse().unwrap());
        let results = Wikipedia::new()
            .search(&http, &request)
            .await
            .expect("search");
        assert_eq!(results.len(), 1, "wikipedia returns exactly one result");
        assert!(results[0].href.contains("wikipedia.org/wiki/"));
        assert!(!results[0].title.is_empty());
        assert!(!results[0].body.is_empty());

        // A "may refer to:" disambiguation page gives no result.
        let request = TextSearchRequest::new("pitch").region("us-en".parse().unwrap());
        let results = Wikipedia::new()
            .search(&http, &request)
            .await
            .expect("search");
        assert!(results.is_empty(), "disambiguation page must be dropped");
    }
}
