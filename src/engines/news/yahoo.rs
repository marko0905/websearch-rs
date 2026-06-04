//! Yahoo News engine — scrapes `news.search.yahoo.com/search`.
//!
//! Yahoo wraps result URLs in `/RU=<encoded>/RK=` redirect paths; this module
//! decodes those back to the publisher URL before returning results.

use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use scraper::Html;
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, percent_decode, selector, selectors};
use crate::http::HttpClient;
use crate::{NewsResult, NewsSearchRequest, Result};

const NAME: &str = "yahoo_news";
const SEARCH_ENDPOINT: &str = "https://news.search.yahoo.com/search";
const HOME: &str = "https://news.search.yahoo.com/";
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

    async fn search(
        &self,
        http: &HttpClient,
        request: &NewsSearchRequest,
    ) -> Result<Vec<NewsResult>> {
        let url = build_url(request)?;
        let body = http.get_text(url.as_str(), &[("Referer", HOME)]).await?;
        parse_news(&body, request.max_results)
    }
}

fn build_url(request: &NewsSearchRequest) -> Result<Url> {
    let mut url = Url::parse(SEARCH_ENDPOINT)?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("p", request.query.trim());
        if request.page > 1 {
            query.append_pair("b", &((request.page - 1) * PAGE_SIZE + 1).to_string());
        }
        if let Some(time_limit) = request.time_limit {
            query.append_pair("btf", time_limit.as_duckduckgo_df());
        }
    }
    Ok(url)
}

fn parse_news(body: &str, max_results: usize) -> Result<Vec<NewsResult>> {
    let document = Html::parse_document(body);
    let item_selector = selector("div#web li, section.NewsArticle div.content.s-item")?;
    let anchor_selectors = selectors(&["h4 a[href]", "a.bkgLink[href]", "a[href]"])?;
    let title_selectors = selectors(&["h4", "h3", ".s-title", ".title"])?;
    let snippet_selectors = selectors(&["p", ".s-desc", ".compText"])?;
    let source_selectors = selectors(&["span.source", ".s-source"])?;
    let date_selectors = selectors(&["span.time", ".s-time", "span.fc-2nd"])?;

    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for item in document.select(&item_selector) {
        let Some(anchor) = first_match(item, &anchor_selectors) else {
            continue;
        };
        let Some(raw_href) = anchor.value().attr("href") else {
            continue;
        };
        let href = resolve_url(raw_href);
        if href.is_empty() || !seen.insert(href.clone()) {
            continue;
        }
        let title = first_match(item, &title_selectors)
            .map(element_text)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| element_text(anchor));
        if title.is_empty() {
            continue;
        }
        let body_text = first_match(item, &snippet_selectors)
            .map(element_text)
            .unwrap_or_default();
        let source = first_match(item, &source_selectors)
            .map(element_text)
            .map(|text| {
                text.split(" · via Yahoo")
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_owned()
            })
            .unwrap_or_default();
        let date =
            first_match(item, &date_selectors).and_then(|node| relative_date(&element_text(node)));

        results.push(NewsResult::new(title, href, body_text, date, source, NAME));
        if results.len() >= max_results {
            break;
        }
    }

    Ok(results)
}

fn resolve_url(raw_href: &str) -> String {
    let href = raw_href.trim();
    if let Some(start) = href.find("/RU=") {
        let rest = &href[start + 4..];
        let end = rest
            .find("/RK=")
            .or_else(|| rest.find("/RS="))
            .or_else(|| rest.find('?'))
            .unwrap_or(rest.len());
        let decoded = percent_decode(&rest[..end]);
        if decoded.starts_with("http") {
            return decoded;
        }
    }
    if href.starts_with("http") {
        return href.to_string();
    }
    String::new()
}

fn relative_date(value: &str) -> Option<u64> {
    let lower = value.to_ascii_lowercase();
    let mut iter = lower.split_whitespace();
    let number = iter.next()?.parse::<u64>().ok()?;
    let unit = iter.next()?;
    let seconds = if unit.starts_with("minute") {
        number * 60
    } else if unit.starts_with("hour") {
        number * 60 * 60
    } else if unit.starts_with("day") {
        number * 24 * 60 * 60
    } else if unit.starts_with("week") {
        number * 7 * 24 * 60 * 60
    } else if unit.starts_with("month") {
        number * 30 * 24 * 60 * 60
    } else if unit.starts_with("year") {
        number * 365 * 24 * 60 * 60
    } else {
        return None;
    };
    SystemTime::now()
        .checked_sub(Duration::from_secs(seconds))
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_yahoo_news_fixture_and_decodes_urls() {
        let body = include_str!("../../../fixtures/yahoo_news_page.html");
        let results = parse_news(body, 10).expect("parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust project announces new release");
        assert_eq!(results[0].href, "https://example.com/rust-release");
        assert_eq!(results[0].source, "Example News");
        assert_eq!(results[0].provider, NAME);
        assert!(results[0].body.contains("Rust team"));
        assert!(results[0].date.is_some());
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
        assert!(results[0].href.starts_with("http"));
    }
}
