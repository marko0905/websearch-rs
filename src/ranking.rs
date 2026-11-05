//! Cross-engine aggregation and ranking.
//!
//! Port of Python `ddgs` v9.16.1 `results.py` (`ResultsAggregator`) and
//! `similarity.py` (`SimpleFilterRanker`):
//!
//! - [`ResultsAggregator`] keys each result by the string value of the first of
//!   its fields (in dataclass order) that is one of `href`, `image`, `url`,
//!   `embed_url`; keeps the variant with the longest `body`; counts how often
//!   each key was seen; and extracts results by descending count.
//! - [`rank`] drops Wikimedia category pages, pulls `wikipedia.org` hrefs to the
//!   top, then buckets the rest by where query tokens occur (title and body,
//!   title only, body only, neither).
//!
//! Upstream works on result dataclasses / dicts, so a field can be missing
//! (the type has no such field), `None` (JSON null), or a string. [`Rankable`]
//! exposes exactly that view ([`PyValue`]), so the Python semantics — including
//! the exceptions upstream raises on a `None` field — are reproduced.

use std::collections::HashMap;

use icu_properties::props::{GeneralCategory, GeneralCategoryGroup};
use icu_properties::{CodePointMapData, CodePointMapDataBorrowed};

use crate::{Result, WebSearchError};

/// Upstream `ResultsAggregator({"href", "image", "url", "embed_url"})`.
pub const CACHE_FIELDS: &[&str] = &["href", "image", "url", "embed_url"];

/// Minimum query-token length (`SimpleFilterRanker(min_token_length=3)`).
const MIN_TOKEN_LEN: usize = 3;

/// A result field as Python sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PyValue<'a> {
    /// The result type has no such field (`dict.get` gives the default).
    Missing,
    /// The field holds `None`.
    None,
    /// The field holds a string.
    Str(&'a str),
}

impl<'a> PyValue<'a> {
    /// `d.get(key, "")`: a missing field reads as `""`; `None` stays `None`.
    fn or_empty(self) -> Option<&'a str> {
        match self {
            Self::Missing => Some(""),
            Self::None => None,
            Self::Str(value) => Some(value),
        }
    }
}

/// A result that can be aggregated and ranked, viewed as its upstream
/// dataclass.
pub trait Rankable {
    /// The upstream dataclass field names, in declaration (`__dict__`) order.
    const FIELDS: &'static [&'static str];

    /// The value of a string field; [`PyValue::Missing`] for names that are
    /// not fields of this type.
    fn get(&self, field: &str) -> PyValue<'_>;
}

/// Python `TypeError` raised by `len(None)`.
fn none_len() -> WebSearchError {
    WebSearchError::Internal("object of type 'NoneType' has no len()".into())
}

/// Python `TypeError` raised by `"x" in None` (CPython 3.14+ wording; older
/// versions say "is not iterable").
fn none_not_iterable() -> WebSearchError {
    WebSearchError::Internal("argument of type 'NoneType' is not a container or iterable".into())
}

/// Python `AttributeError` raised by `None.lower()`.
fn none_lower() -> WebSearchError {
    WebSearchError::Internal("'NoneType' object has no attribute 'lower'".into())
}

/// Port of upstream `ResultsAggregator`.
#[derive(Debug)]
pub struct ResultsAggregator<R> {
    /// `(key, item, count)` in first-seen order. One vector serves as both
    /// upstream's `_cache` dict and `_counter` (both keyed in first-seen order).
    entries: Vec<(String, R, usize)>,
    index: HashMap<String, usize>,
}

impl<R: Rankable> Default for ResultsAggregator<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Rankable> ResultsAggregator<R> {
    /// An empty aggregator keyed on [`CACHE_FIELDS`].
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::new(),
        }
    }

    /// Number of distinct keys (upstream `len(self._cache)`).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been aggregated.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Upstream `_get_key`: `str(value)` of the first field in `__dict__`
    /// order that is a cache field. `str(None)` is `"None"`, so every result
    /// with a `None` key field shares the key `"None"` (upstream quirk).
    fn key(item: &R) -> Result<String> {
        for field in R::FIELDS {
            if CACHE_FIELDS.contains(field) {
                return Ok(match item.get(field) {
                    PyValue::Str(value) => value.to_owned(),
                    PyValue::None | PyValue::Missing => "None".to_owned(),
                });
            }
        }
        Err(WebSearchError::Internal(format!(
            "Item has none of the cache fields {CACHE_FIELDS:?}"
        )))
    }

    /// Upstream `append`: store the item the first time its key is seen, or
    /// replace the stored one when the new item has a longer `body`; always
    /// bump the key's count.
    ///
    /// `len(item.__dict__.get("body", ""))` counts code points; a `None` body
    /// on a repeated key raises `TypeError` (upstream quirk, reproduced).
    pub fn append(&mut self, item: R) -> Result<()> {
        let key = Self::key(&item)?;
        match self.index.get(&key) {
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, item, 1));
            }
            Some(&slot) => {
                let new_len = body_len(&item)?;
                let old_len = body_len(&self.entries[slot].1)?;
                if new_len > old_len {
                    self.entries[slot].1 = item;
                }
                self.entries[slot].2 += 1;
            }
        }
        Ok(())
    }

    /// Upstream `extend`: append in order; the first error stops it, keeping
    /// the items appended before it.
    pub fn extend(&mut self, items: Vec<R>) -> Result<()> {
        for item in items {
            self.append(item)?;
        }
        Ok(())
    }

    /// Upstream `extract_dicts`: items by descending count
    /// (`Counter.most_common()`, a stable sort, so equal counts keep
    /// first-seen order).
    pub fn extract(self) -> Vec<R> {
        let mut entries = self.entries;
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.2));
        entries.into_iter().map(|(_, item, _)| item).collect()
    }
}

fn body_len<R: Rankable>(item: &R) -> Result<usize> {
    item.get("body")
        .or_empty()
        .map(|body| body.chars().count())
        .ok_or_else(none_len)
}

/// Python `re.compile(r"\W+")` word character: `str.isalnum()` or `_`, i.e. a
/// letter (L*) or number (N*) code point.
fn is_py_word_char(ch: char, categories: CodePointMapDataBorrowed<'_, GeneralCategory>) -> bool {
    if ch == '_' {
        return true;
    }
    let category = categories.get(ch);
    GeneralCategoryGroup::Letter.contains(category)
        || GeneralCategoryGroup::Number.contains(category)
}

/// Upstream `_extract_tokens`: `{t for t in re.split(r"\W+", query.lower()) if
/// len(t) >= 3}`.
pub fn query_tokens(query: &str) -> Vec<String> {
    let categories = CodePointMapData::<GeneralCategory>::new();
    let lower = query.to_lowercase();
    let mut tokens: Vec<String> = Vec::new();
    for token in lower.split(|ch: char| !is_py_word_char(ch, categories)) {
        if token.chars().count() >= MIN_TOKEN_LEN && !tokens.iter().any(|t| t == token) {
            tokens.push(token.to_owned());
        }
    }
    tokens
}

/// Upstream `_has_any_token`: any token is a substring of `text.lower()`.
fn has_any_token(text: &str, tokens: &[String]) -> bool {
    let lower = text.to_lowercase();
    tokens.iter().any(|token| lower.contains(token.as_str()))
}

/// Port of upstream `SimpleFilterRanker.rank`.
///
/// Per result: `href = d.get("href", "")`, `title = d.get("title", "")`,
/// `body = d.get("body", d.get("description", ""))`. A title containing both
/// `"Category:"` and `"Wikimedia"` is dropped; `"wikipedia.org"` in href goes
/// first; the rest go to title+body / title-only / body-only / neither buckets.
/// Order within each bucket is kept. A `None` title, href (when reached) or
/// body raises as upstream does (`TypeError` / `AttributeError`).
pub fn rank<R: Rankable>(docs: Vec<R>, query: &str) -> Result<Vec<R>> {
    let tokens = query_tokens(query);

    let mut wiki_hits = Vec::new();
    let mut both = Vec::new();
    let mut title_only = Vec::new();
    let mut body_only = Vec::new();
    let mut neither = Vec::new();

    for doc in docs {
        let href = doc.get("href").or_empty();
        let title = doc.get("title").or_empty();
        let body = match doc.get("body") {
            PyValue::Missing => doc.get("description").or_empty(),
            value => value.or_empty(),
        };

        // Skip Wikimedia category pages.
        let title_text = title.ok_or_else(none_not_iterable)?;
        if title_text.contains("Category:") && title_text.contains("Wikimedia") {
            continue;
        }

        // Wikipedia check.
        if href
            .ok_or_else(none_not_iterable)?
            .contains("wikipedia.org")
        {
            wiki_hits.push(doc);
            continue;
        }

        let hit_title = has_any_token(title_text, &tokens);
        let hit_body = has_any_token(body.ok_or_else(none_lower)?, &tokens);

        match (hit_title, hit_body) {
            (true, true) => both.push(doc),
            (true, false) => title_only.push(doc),
            (false, true) => body_only.push(doc),
            (false, false) => neither.push(doc),
        }
    }

    let mut ranked = wiki_hits;
    ranked.extend(both);
    ranked.extend(title_only);
    ranked.extend(body_only);
    ranked.extend(neither);
    Ok(ranked)
}

fn opt(value: &Option<String>) -> PyValue<'_> {
    match value {
        Some(value) => PyValue::Str(value),
        None => PyValue::None,
    }
}

impl Rankable for crate::TextResult {
    const FIELDS: &'static [&'static str] = &["title", "href", "body"];
    fn get(&self, field: &str) -> PyValue<'_> {
        match field {
            "title" => PyValue::Str(&self.title),
            "href" => PyValue::Str(&self.href),
            "body" => PyValue::Str(&self.body),
            _ => PyValue::Missing,
        }
    }
}

impl Rankable for crate::NewsResult {
    const FIELDS: &'static [&'static str] = &["date", "title", "body", "url", "image", "source"];
    fn get(&self, field: &str) -> PyValue<'_> {
        match field {
            "date" => opt(&self.date),
            "title" => opt(&self.title),
            "body" => opt(&self.body),
            "url" => opt(&self.url),
            "image" => opt(&self.image),
            "source" => opt(&self.source),
            _ => PyValue::Missing,
        }
    }
}

impl Rankable for crate::ImageResult {
    // `height`/`width` are not strings and never read by the pipeline.
    const FIELDS: &'static [&'static str] = &[
        "title",
        "image",
        "thumbnail",
        "url",
        "height",
        "width",
        "source",
    ];
    fn get(&self, field: &str) -> PyValue<'_> {
        match field {
            "title" => opt(&self.title),
            "image" => opt(&self.image),
            "thumbnail" => opt(&self.thumbnail),
            "url" => opt(&self.url),
            "source" => opt(&self.source),
            _ => PyValue::Missing,
        }
    }
}

impl Rankable for crate::VideoResult {
    // `content` is not a cache field, so videos key on `embed_url`.
    // `images`/`statistics` are not strings and never read by the pipeline.
    const FIELDS: &'static [&'static str] = &[
        "title",
        "content",
        "description",
        "duration",
        "embed_html",
        "embed_url",
        "image_token",
        "images",
        "provider",
        "published",
        "publisher",
        "statistics",
        "uploader",
    ];
    fn get(&self, field: &str) -> PyValue<'_> {
        match field {
            "title" => opt(&self.title),
            "content" => opt(&self.content),
            "description" => opt(&self.description),
            "duration" => opt(&self.duration),
            "embed_html" => opt(&self.embed_html),
            "embed_url" => opt(&self.embed_url),
            "image_token" => opt(&self.image_token),
            "provider" => opt(&self.provider),
            "published" => opt(&self.published),
            "publisher" => opt(&self.publisher),
            "uploader" => opt(&self.uploader),
            _ => PyValue::Missing,
        }
    }
}

impl Rankable for crate::BooksResult {
    const FIELDS: &'static [&'static str] =
        &["title", "author", "publisher", "info", "url", "thumbnail"];
    fn get(&self, field: &str) -> PyValue<'_> {
        match field {
            "title" => PyValue::Str(&self.title),
            "author" => PyValue::Str(&self.author),
            "publisher" => PyValue::Str(&self.publisher),
            "info" => PyValue::Str(&self.info),
            "url" => PyValue::Str(&self.url),
            "thumbnail" => PyValue::Str(&self.thumbnail),
            _ => PyValue::Missing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BooksResult, ImageResult, NewsResult, TextResult, VideoResult};

    fn text(href: &str, title: &str, body: &str) -> TextResult {
        TextResult {
            title: title.into(),
            href: href.into(),
            body: body.into(),
        }
    }

    fn hrefs(results: &[TextResult]) -> Vec<&str> {
        results.iter().map(|r| r.href.as_str()).collect()
    }

    #[test]
    fn aggregator_keys_on_first_cache_field_in_field_order() {
        // News: `url` precedes `image` in field order.
        let news = NewsResult {
            url: Some("u".into()),
            image: Some("i".into()),
            ..NewsResult::default()
        };
        assert_eq!(ResultsAggregator::key(&news).unwrap(), "u");
        // Images: `image` precedes `url`.
        let image = ImageResult {
            image: Some("img".into()),
            url: Some("page".into()),
            ..ImageResult::default()
        };
        assert_eq!(ResultsAggregator::key(&image).unwrap(), "img");
        // Videos: `content` is not a cache field; `embed_url` is.
        let video = VideoResult {
            content: Some("c".into()),
            embed_url: Some("e".into()),
            ..VideoResult::default()
        };
        assert_eq!(ResultsAggregator::key(&video).unwrap(), "e");
        // `str(None)` is "None".
        let no_url = NewsResult {
            url: None,
            ..NewsResult::default()
        };
        assert_eq!(ResultsAggregator::key(&no_url).unwrap(), "None");
        let book = BooksResult {
            url: "b".into(),
            ..BooksResult::default()
        };
        assert_eq!(ResultsAggregator::key(&book).unwrap(), "b");
    }

    #[test]
    fn aggregator_is_exact_not_normalized() {
        let mut agg = ResultsAggregator::new();
        agg.append(text("https://a.com/", "t", "b")).unwrap();
        agg.append(text("https://a.com", "t", "b")).unwrap();
        assert_eq!(agg.len(), 2);
    }

    #[test]
    fn aggregator_keeps_longer_body_in_first_slot() {
        let mut agg = ResultsAggregator::new();
        agg.append(text("a", "first", "short")).unwrap();
        agg.append(text("b", "other", "x")).unwrap();
        agg.append(text("a", "second", "much longer")).unwrap();
        // Equal length: no replacement.
        agg.append(text("a", "third", "same-length")).unwrap();
        assert_eq!(agg.len(), 2);
        let out = agg.extract();
        // `a` seen 3 times → first; it holds the longer-body variant.
        assert_eq!(out[0].title, "second");
        assert_eq!(out[1].href, "b");
    }

    #[test]
    fn aggregator_body_length_counts_code_points() {
        let mut agg = ResultsAggregator::new();
        agg.append(text("a", "first", "ééé")).unwrap(); // 3 chars, 6 bytes
        agg.append(text("a", "second", "abcd")).unwrap(); // 4 chars
        assert_eq!(agg.extract()[0].title, "second");
    }

    #[test]
    fn aggregator_orders_by_count_then_first_seen() {
        let mut agg = ResultsAggregator::new();
        for href in ["x", "y", "z", "y", "z", "w"] {
            agg.append(text(href, "t", "b")).unwrap();
        }
        assert_eq!(hrefs(&agg.extract()), ["y", "z", "x", "w"]);
    }

    #[test]
    fn aggregator_none_body_on_repeat_raises() {
        let news = |body: Option<&str>| NewsResult {
            url: Some("u".into()),
            body: body.map(Into::into),
            ..NewsResult::default()
        };
        let mut agg = ResultsAggregator::new();
        // First sighting never checks the body.
        agg.append(news(None)).unwrap();
        let err = agg.append(news(Some("b"))).unwrap_err();
        assert_eq!(err.to_string(), "object of type 'NoneType' has no len()");

        // `extend` stops at the first error, keeping earlier items.
        let mut agg = ResultsAggregator::new();
        let first = NewsResult {
            url: Some("v".into()),
            ..NewsResult::default()
        };
        let result = agg.extend(vec![first, news(Some("b")), news(None)]);
        assert!(result.is_err());
        assert_eq!(agg.len(), 2);
    }

    #[test]
    fn aggregator_types_without_body_never_replace() {
        let book = |title: &str| BooksResult {
            title: title.into(),
            url: "u".into(),
            info: "long long info".into(),
            ..BooksResult::default()
        };
        let mut agg = ResultsAggregator::new();
        agg.append(book("first")).unwrap();
        agg.append(book("second")).unwrap();
        assert_eq!(agg.extract()[0].title, "first");
    }

    #[test]
    fn tokens_split_on_python_non_word() {
        let mut tokens = query_tokens("Rust-lang's  async_await, go! C++ München 2024");
        tokens.sort();
        assert_eq!(tokens, ["2024", "async_await", "lang", "münchen", "rust"]);
        // Duplicates collapse (a set upstream).
        assert_eq!(query_tokens("rust RUST"), ["rust"]);
    }

    #[test]
    fn ranks_into_buckets_in_order() {
        let results = vec![
            text("https://a.com", "nothing here", "nothing here"),
            text("https://b.com", "irrelevant", "rust language guide"),
            text("https://en.wikipedia.org/wiki/Rust", "Rust", "metal"),
            text("https://c.com", "rust tutorial", "all about rust"),
            text("https://d.com", "rust tutorial", "irrelevant"),
        ];
        let ranked = rank(results, "rust").unwrap();
        assert_eq!(
            hrefs(&ranked),
            [
                "https://en.wikipedia.org/wiki/Rust",
                "https://c.com",
                "https://d.com",
                "https://b.com",
                "https://a.com",
            ]
        );
    }

    #[test]
    fn any_token_matches_and_short_tokens_are_ignored() {
        // "go"/"to" are below the min length; any one token is enough.
        let results = vec![
            text("https://b.com", "cooking", "recipes"),
            text("https://a.com", "learn python", "go to"),
        ];
        let ranked = rank(results, "go to rust python").unwrap();
        assert_eq!(ranked[0].href, "https://a.com");
    }

    #[test]
    fn wikipedia_check_is_a_plain_substring() {
        let results = vec![
            text("https://x.com", "rust", "rust"),
            text("https://x.com/?ref=wikipedia.org", "zzz", "zzz"),
        ];
        let ranked = rank(results, "rust").unwrap();
        assert_eq!(ranked[0].href, "https://x.com/?ref=wikipedia.org");
    }

    #[test]
    fn drops_wikimedia_category_pages() {
        let results = vec![
            text(
                "https://commons.wikimedia.org/x",
                "Category:Rust - Wikimedia Commons",
                "b",
            ),
            text("https://a.com", "Category: rust", "b"),
            text("https://b.com", "Wikimedia rust", "b"),
        ];
        let ranked = rank(results, "rust").unwrap();
        assert_eq!(hrefs(&ranked), ["https://a.com", "https://b.com"]);
    }

    #[test]
    fn non_text_categories_rank_on_title_and_body_or_description() {
        let video = |title: &str, description: &str| VideoResult {
            title: Some(title.into()),
            description: Some(description.into()),
            ..VideoResult::default()
        };
        let ranked = rank(
            vec![
                video("zzz", "zzz"),
                video("zzz", "rust"),
                video("rust", "zzz"),
            ],
            "rust",
        )
        .unwrap();
        let titles: Vec<_> = ranked.iter().map(|v| v.title.as_deref().unwrap()).collect();
        assert_eq!(titles, ["rust", "zzz", "zzz"]);
        assert_eq!(ranked[1].description.as_deref(), Some("rust"));
    }

    #[test]
    fn none_fields_raise_like_upstream() {
        let none_title = NewsResult {
            title: None,
            body: Some("b".into()),
            ..NewsResult::default()
        };
        assert_eq!(
            rank(vec![none_title], "q").unwrap_err().to_string(),
            "argument of type 'NoneType' is not a container or iterable"
        );
        let none_body = NewsResult {
            title: Some("t".into()),
            body: None,
            ..NewsResult::default()
        };
        assert_eq!(
            rank(vec![none_body], "q").unwrap_err().to_string(),
            "'NoneType' object has no attribute 'lower'"
        );
        // Images have no body/description field: reads as "".
        let image = ImageResult {
            title: Some("t".into()),
            ..ImageResult::default()
        };
        assert_eq!(rank(vec![image], "q").unwrap().len(), 1);
    }

    #[test]
    fn rank_is_stable_within_bucket() {
        let results = vec![
            text("https://1.com", "x", "y"),
            text("https://2.com", "x", "y"),
            text("https://3.com", "x", "y"),
        ];
        let ranked = rank(results, "zzz").unwrap();
        assert_eq!(
            hrefs(&ranked),
            ["https://1.com", "https://2.com", "https://3.com"]
        );
    }
}
