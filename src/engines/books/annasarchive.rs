//! Anna's Archive books engine — scrapes `/search` result cards.
//!
//! Anna's Archive wraps result markup in comment blocks in some responses, so
//! parsing first strips HTML comment delimiters, matching Python `ddgs`.

use std::sync::OnceLock;

use async_trait::async_trait;
use url::Url;

use crate::engines::util::field;
use crate::engines::{SearchEngine, SplitMix64};
use crate::html;
use crate::http::HttpClient;
use crate::normalize::normalize_url;
use crate::{BooksResult, BooksSearchRequest, Result};

const NAME: &str = "annasarchive";
#[cfg(test)]
const DEFAULT_BASE: &str = "https://annas-archive.org/";
const LIVE_BASES: &[&str] = &[
    "https://annas-archive.gd/",
    "https://annas-archive.gl/",
    "https://annas-archive.pk/",
];

/// Anna's Archive books engine.
#[derive(Clone, Debug, Default)]
pub struct AnnasArchive;

impl AnnasArchive {
    /// Construct the engine.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SearchEngine<BooksSearchRequest, BooksResult> for AnnasArchive {
    fn name(&self) -> &'static str {
        NAME
    }

    /// Upstream `provider = "annasarchive"`.
    fn provider(&self) -> &'static str {
        "annasarchive"
    }

    async fn search(
        &self,
        http: &HttpClient,
        request: &BooksSearchRequest,
    ) -> Result<Vec<BooksResult>> {
        // One request to the process-wide mirror; no fallback to other mirrors.
        let base = live_base();
        let url = build_url(request, base)?;
        let Some(body) = http.get_text(url.as_str(), &[]).await? else {
            // Upstream `BaseSearchEngine.request`: non-200 → `None` → no results.
            return Ok(Vec::new());
        };
        parse_books_with_base(&body, base)
    }
}

/// Upstream `search_url = f"https://annas-archive.{random.choice(['gd', 'gl',
/// 'pk'])}/search"` is a class attribute, evaluated once at import: one
/// randomly chosen mirror for the whole process.
fn live_base() -> &'static str {
    static BASE: OnceLock<&'static str> = OnceLock::new();
    BASE.get_or_init(|| {
        let index = (SplitMix64::from_clock().next_u64() % LIVE_BASES.len() as u64) as usize;
        LIVE_BASES
            .get(index)
            .copied()
            .unwrap_or("https://annas-archive.gd/")
    })
}

fn build_url(request: &BooksSearchRequest, base: &str) -> Result<Url> {
    let mut url = Url::parse(base)?.join("search")?;
    url.query_pairs_mut()
        .append_pair("q", &request.query)
        .append_pair("page", &request.page.to_string());
    Ok(url)
}

#[cfg(test)]
fn parse_books(body: &str) -> Result<Vec<BooksResult>> {
    parse_books_with_base(body, DEFAULT_BASE)
}

/// Upstream `items_xpath`.
const ITEMS_XPATH: &str = "//div[contains(@class, 'record-list-outer')]/div";
/// Upstream `elements_xpath` (dict order).
const ELEMENTS_XPATH: &[(&str, &str)] = &[
    ("title", ".//a[contains(@class, 'text-lg')]//text()"),
    ("author", ".//a[span[contains(@class, 'user')]]//text()"),
    (
        "publisher",
        ".//a[span[contains(@class, 'company')]]//text()",
    ),
    ("info", ".//div[contains(@class, 'text-gray-800')]/text()"),
    ("url", "./a/@href"),
    ("thumbnail", ".//img/@src"),
];

fn parse_books_with_base(body: &str, base_url: &str) -> Result<Vec<BooksResult>> {
    // Upstream `pre_process_html`: drop comment markers so the commented-out
    // cards are parsed.
    let html = body.replace("<!--", "").replace("-->", "");
    // Upstream: `base_url = search_url.split("/search")[0]` (no trailing
    // slash) and `post_extract_results` sets `url = f"{base_url}{url}"`.
    let base = base_url.trim_end_matches('/');
    Ok(html::extract_results(&html, ITEMS_XPATH, ELEMENTS_XPATH)?
        .into_iter()
        .map(|f| {
            // The url is normalized on extraction (`__setattr__`, non-empty
            // only), then again when post-processing re-assigns it with the
            // base (the constructor applies that second pass).
            let href = field(&f, "url");
            let href = if href.is_empty() {
                href
            } else {
                normalize_url(&href)
            };
            BooksResult::new(
                field(&f, "title"),
                field(&f, "author"),
                field(&f, "publisher"),
                field(&f, "info"),
                format!("{base}{href}"),
                field(&f, "thumbnail"),
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_upstream_output() {
        // ddgs 9.16.1's own AnnasArchive `extract_results` +
        // `post_extract_results` on the same fixture, with the mirror pinned
        // to annas-archive.gd.
        let expected: Vec<BooksResult> = serde_json::from_str(include_str!(
            "../../../fixtures/annasarchive_books_expected.json"
        ))
        .expect("expected json");
        let results = parse_books_with_base(
            include_str!("../../../fixtures/annasarchive_books_page.html"),
            "https://annas-archive.gd/",
        )
        .unwrap();
        assert_eq!(results, expected);
    }

    #[test]
    fn parses_annasarchive_fixture() {
        let body = include_str!("../../../fixtures/annasarchive_books_page.html");
        let results = parse_books(body).expect("parse");
        // Every card div is a result (no filtering), including the empty one.
        assert_eq!(results.len(), 3);
        let first = &results[0];
        assert_eq!(first.title, "The Rust Programming Language");
        assert_eq!(first.author, "Steve Klabnik, Carol Nichols");
        assert_eq!(first.publisher, "No Starch Press");
        // Own text of the `text-gray-800` div only (the nested span is not).
        assert_eq!(first.info, "English [en], pdf, 5.2MB,");
        assert_eq!(
            first.url,
            "https://annas-archive.org/md5/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        // Relative cover paths stay relative, as upstream.
        assert_eq!(first.thumbnail, "/covers/rust-book.jpg");
        let second = &results[1];
        assert_eq!(second.thumbnail, "https://example.com/programming-rust.jpg");
        assert_eq!(second.publisher, "");
        // Both `text-lg` anchors' text is joined.
        assert_eq!(second.title, "Programming Rust2nd Edition");
        // A card without a direct `<a>` gets the bare base URL, as upstream.
        assert_eq!(results[2].url, "https://annas-archive.org");
        assert_eq!(results[2].title, "");
    }

    #[tokio::test]
    #[ignore = "hits live Anna's Archive"]
    async fn live_annasarchive_returns_results() {
        let http = HttpClient::new(&crate::http::HttpConfig::default()).expect("client");
        let results = AnnasArchive::new()
            .search(&http, &BooksSearchRequest::new("rust programming"))
            .await
            .expect("search");
        assert!(!results.is_empty(), "annasarchive returned no results");
        assert!(results[0].url.starts_with("http"));
    }
}
