//! Grokipedia text engine — port of `ddgs/engines/grokipedia.py` (v9.16.1).
//!
//! `GET https://grokipedia.com/api/typeahead` with `{"query": query,
//! "limit": "1"}`; upstream sends no extra headers. `extract_results` builds
//! ONE result from `results[0]` only (any later items are ignored):
//! - `title = items[0].get("title", "").strip("_")`
//! - `body` = the `snippet` text after its first `"\n\n"` (or all of it)
//! - `href = f"https://grokipedia.com/page/{items[0]['slug']}"`
//!
//! There is no `post_extract_results` filter, so an empty title is kept.

use async_trait::async_trait;
use serde_json::Value;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::py_truthy;
use crate::http::HttpClient;
use crate::{Result, TextResult, TextSearchRequest, WebSearchError};

const NAME: &str = "grokipedia";
const SEARCH_ENDPOINT: &str = "https://grokipedia.com/api/typeahead";
const PAGE_BASE: &str = "https://grokipedia.com/page/";

/// Grokipedia typeahead engine.
#[derive(Clone, Debug, Default)]
pub struct Grokipedia;

impl Grokipedia {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<TextSearchRequest, TextResult> for Grokipedia {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "grokipedia"`.
    fn provider(&self) -> &'static str {
        "grokipedia"
    }

    fn priority(&self) -> f64 {
        // Upstream `priority = 1.9` (engines run in descending priority).
        1.9
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
        parse_results(&body)
    }
}

/// Upstream params, in dict order. The query is sent verbatim.
fn build_url(request: &TextSearchRequest) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("query", &request.query)
        .append_pair("limit", "1");
    Ok(url)
}

fn parse_error(message: impl Into<String>) -> WebSearchError {
    WebSearchError::Parse(format!("grokipedia: {}", message.into()))
}

/// A string field read with `.get(key, "")`: missing → `""`; a non-string
/// value makes the later `.strip`/`in`/`.split` raise upstream.
fn str_field<'a>(item: &'a Value, key: &str) -> Result<&'a str> {
    match item.get(key) {
        None => Ok(""),
        Some(Value::String(text)) => Ok(text),
        Some(_) => Err(parse_error(format!("`{key}` is not a string"))),
    }
}

/// `str(value)` for the slug inside the f-string.
fn py_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "None".to_owned(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        // Numbers print the same; arrays/objects do not occur in this API.
        other => other.to_string(),
    }
}

/// Upstream `extract_results`. Every lookup that raises in Python
/// (`json_data.get` on a non-object, `items[0]` on a non-list, a missing
/// `slug`) is a parse error here.
fn parse_results(body: &str) -> Result<Vec<TextResult>> {
    let json: Value = serde_json::from_str(body).map_err(|err| parse_error(err.to_string()))?;
    let object = json
        .as_object()
        .ok_or_else(|| parse_error("response is not an object"))?;
    // `json_data.get("results", [])`, then `if not items: return []`.
    let Some(items) = object.get("results").filter(|items| py_truthy(items)) else {
        return Ok(Vec::new());
    };
    let first = items
        .as_array()
        .and_then(|items| items.first())
        .ok_or_else(|| parse_error("`results` is not a list"))?;
    if !first.is_object() {
        return Err(parse_error("result item is not an object"));
    }

    let title = str_field(first, "title")?.trim_matches('_');
    let snippet = str_field(first, "snippet")?;
    // Upstream quirk: the snippet head ends at the first "\n\n"; later "\n\n" are
    // deleted by `_normalize_text` without a space, so words may join.
    let body = snippet.split_once("\n\n").map_or(snippet, |(_, rest)| rest);
    let slug = first
        .get("slug")
        .ok_or_else(|| parse_error("result item has no `slug`"))?;
    // The constructor applies the upstream setter normalization.
    Ok(vec![TextResult::new(
        title,
        format!("{PAGE_BASE}{}", py_str(slug)),
        body,
    )])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_multi_item_response_gives_one_result() {
        // Real two-item typeahead response: upstream reads `items[0]` only.
        let body = include_str!("../../../fixtures/grokipedia_typeahead.json");
        let results = parse_results(body).expect("parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Rust (programming language)");
        assert_eq!(
            results[0].href,
            "https://grokipedia.com/page/Rust_(programming_language)"
        );
        assert!(results[0].body.contains("systems programming language"));
    }

    #[test]
    fn matches_upstream_expected_output() {
        // `fixtures/grokipedia_text_expected.json`: upstream ddgs 9.16.1 output
        // (`scripts/regen_expected.py`).
        let expected: Vec<TextResult> = serde_json::from_str(include_str!(
            "../../../fixtures/grokipedia_text_expected.json"
        ))
        .expect("expected json");
        let results =
            parse_results(include_str!("../../../fixtures/grokipedia_typeahead.json")).unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn field_handling_matches_upstream() {
        let body = r#"{"results":[{"title":"__Foo_Bar__","slug":" Foo_Bar","snippet":"Head\n\nRest\n\nMore"},{"title":"Second","slug":"S"}]}"#;
        let results = parse_results(body).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Foo_Bar");
        // The slug is not stripped; the href setter keeps the space as `+`.
        assert_eq!(results[0].href, "https://grokipedia.com/page/+Foo_Bar");
        // `_normalize_text` deletes "C"-category characters (the inner
        // `\n\n`) before collapsing whitespace, as upstream does.
        assert_eq!(results[0].body, "RestMore");

        // Missing title and snippet default to "" and the result is kept.
        let results = parse_results(r#"{"results":[{"slug":"X"}]}"#).unwrap();
        assert_eq!(
            results,
            vec![TextResult::new("", "https://grokipedia.com/page/X", "")]
        );
    }

    #[test]
    fn empty_and_error_shapes() {
        assert!(parse_results(r#"{"results":[]}"#).unwrap().is_empty());
        assert!(parse_results(r#"{"results":null}"#).unwrap().is_empty());
        assert!(parse_results(r#"{}"#).unwrap().is_empty());
        // `items[0]['slug']` KeyError.
        assert!(parse_results(r#"{"results":[{"title":"T"}]}"#).is_err());
        // `.get` on a list → AttributeError.
        assert!(parse_results(r#"[1]"#).is_err());
        // `None.strip` → AttributeError.
        assert!(parse_results(r#"{"results":[{"title":null,"slug":"s"}]}"#).is_err());
    }

    #[test]
    fn url_matches_upstream() {
        let url = build_url(&TextSearchRequest::new(" rust lang")).unwrap();
        assert_eq!(
            url.as_str(),
            "https://grokipedia.com/api/typeahead?query=+rust+lang&limit=1"
        );
    }

    #[tokio::test]
    #[ignore = "hits live Grokipedia"]
    async fn live_grokipedia_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = Grokipedia::new()
            .search(&http, &TextSearchRequest::new("rust programming language"))
            .await
            .expect("search");
        assert_eq!(results.len(), 1, "grokipedia returns exactly one result");
        assert!(results[0].href.starts_with(PAGE_BASE));
    }
}
