//! Anna's Archive books engine — scrapes `/search` result cards.
//!
//! Anna's Archive wraps result markup in comment blocks in some responses, so
//! parsing first strips HTML comment delimiters, matching Python `ddgs`.

use std::collections::HashSet;

use async_trait::async_trait;
use scraper::Html;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
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

    async fn search(
        &self,
        http: &HttpClient,
        request: &BooksSearchRequest,
    ) -> Result<Vec<BooksResult>> {
        // Try mirrors in turn. A mirror that errors *or* returns an empty parse
        // (e.g. a Cloudflare challenge page served with 200) is treated as a
        // miss, so we fall through to the next mirror instead of giving up.
        let mut last_error = None;
        for base in LIVE_BASES {
            let url = build_url(request, base)?;
            match http.get_text(url.as_str(), &[("Referer", *base)]).await {
                Ok(body) => match parse_books_with_base(&body, request.max_results, base) {
                    Ok(results) if !results.is_empty() => return Ok(results),
                    Ok(_) => continue,
                    Err(error) => last_error = Some(error),
                },
                Err(error) => last_error = Some(error),
            }
        }
        // All mirrors returned empty (no error) or failed; surface the last
        // error if there was one, otherwise an empty result set.
        match last_error {
            Some(error) => Err(error),
            None => Ok(Vec::new()),
        }
    }
}

fn build_url(request: &BooksSearchRequest, base: &str) -> Result<Url> {
    let mut url = Url::parse(base)?.join("search")?;
    url.query_pairs_mut()
        .append_pair("q", request.query.trim())
        .append_pair("page", &request.page.to_string());
    Ok(url)
}

#[cfg(test)]
fn parse_books(body: &str, max_results: usize) -> Result<Vec<BooksResult>> {
    parse_books_with_base(body, max_results, DEFAULT_BASE)
}

fn parse_books_with_base(
    body: &str,
    max_results: usize,
    base_url: &str,
) -> Result<Vec<BooksResult>> {
    let html = body.replace("<!--", "").replace("-->", "");
    let document = Html::parse_document(&html);
    // Anna's Archive markup (2024+): cards are direct children of the
    // `js-aarecord-list-outer` container; the title is an `/md5/` anchor with a
    // `text-lg` class, and authors/publishers are `/search` links. The legacy
    // `record-list-outer` selector is kept as a fallback for older mirrors.
    let item_selector = selector("div.js-aarecord-list-outer > div, div.record-list-outer > div")?;
    let title_selectors = selectors(&[
        "a[href*='/md5/'][class*='text-lg']",
        "a.text-lg",
        "a[class*=text-lg]",
    ])?;
    let search_link_selector = selector("a[href*='/search']")?;
    let info_selectors =
        selectors(&["div[class*='text-gray-600']", "div[class*='text-gray-800']"])?;
    let image_selector = selector("img[src]")?;

    let base = Url::parse(base_url)?;
    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for item in document.select(&item_selector) {
        let Some(title_anchor) = first_match(item, &title_selectors) else {
            continue;
        };
        let Some(raw_url) = title_anchor.value().attr("href") else {
            continue;
        };
        let url = join_url(&base, raw_url);
        let title = element_text(title_anchor);
        if title.is_empty() || url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        // Author and publisher are the `/search` links, in document order
        // (author first, publisher second). Either may be absent.
        let mut search_links = item
            .select(&search_link_selector)
            .map(element_text)
            .filter(|text| !text.is_empty());
        let author = search_links.next().unwrap_or_default();
        let publisher = search_links.next().unwrap_or_default();
        let info = first_match(item, &info_selectors)
            .map(element_text)
            .unwrap_or_default();
        let thumbnail = item
            .select(&image_selector)
            .next()
            .and_then(|node| node.value().attr("src"))
            .map(|src| join_url(&base, src))
            .unwrap_or_default();

        results.push(BooksResult::new(
            title, author, publisher, info, url, thumbnail, NAME,
        ));
        if results.len() >= max_results {
            break;
        }
    }

    Ok(results)
}

fn join_url(base: &Url, raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with("http") {
        return trimmed.to_string();
    }
    base.join(trimmed)
        .map(|url| url.into())
        .unwrap_or_else(|_| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_annasarchive_fixture() {
        let body = include_str!("../../../fixtures/annasarchive_books_page.html");
        let results = parse_books(body, 10).expect("parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "The Rust Programming Language");
        assert_eq!(results[0].author, "Steve Klabnik, Carol Nichols");
        assert_eq!(results[0].publisher, "No Starch Press");
        assert!(results[0].info.contains("English"));
        assert_eq!(
            results[0].url,
            "https://annas-archive.org/md5/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(results[0].provider, NAME);
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
