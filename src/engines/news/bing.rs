//! Bing News engine — scrapes `www.bing.com/news/infinitescrollajax`.
//!
//! Bing's news endpoint returns HTML fragments for infinite-scroll pages. News
//! items expose most fields as attributes (`data-title`, `url`, `data-author`)
//! with snippet/date/image inside the block.

use std::collections::HashSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use scraper::{ElementRef, Html};
use url::Url;

use crate::engines::SearchEngine;
use crate::engines::util::{element_text, first_match, selector, selectors};
use crate::http::HttpClient;
use crate::{NewsResult, NewsSearchRequest, Result, TimeLimit};

const NAME: &str = "bing_news";
const SEARCH_ENDPOINT: &str = "https://www.bing.com/news/infinitescrollajax";
const HOME: &str = "https://www.bing.com/news";

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
        query.append_pair("q", request.query.trim());
        query.append_pair("InfiniteScroll", "1");
        query.append_pair("first", &(request.page * 10 + 1).to_string());
        query.append_pair("SFX", &request.page.to_string());
        let (country, lang) = request
            .region
            .as_str()
            .split_once('-')
            .unwrap_or(("us", "en"));
        query.append_pair("cc", if country == "wt" { "us" } else { country });
        query.append_pair("setlang", if lang == "wt" { "en" } else { lang });
        if let Some(time_limit) = request.time_limit {
            query.append_pair("qft", bing_time_filter(time_limit));
        }
    }
    Ok(url)
}

fn bing_time_filter(time_limit: TimeLimit) -> &'static str {
    match time_limit {
        TimeLimit::Day => "interval=\"4\"",
        TimeLimit::Week => "interval=\"7\"",
        TimeLimit::Month | TimeLimit::Year => "interval=\"9\"",
    }
}

fn parse_news(body: &str, max_results: usize) -> Result<Vec<NewsResult>> {
    let document = Html::parse_fragment(body);
    let item_selector = selector("div.newsitem")?;
    let snippet_selectors = selectors(&["div.snippet", ".snippet"])?;
    let date_selectors = selectors(&["span[aria-label]", "span.source"])?;
    let image_selectors = selectors(&["a.image img", "img"])?;

    let mut results = Vec::new();
    let mut seen = HashSet::new();

    for item in document.select(&item_selector) {
        let title = item.value().attr("data-title").unwrap_or_default().trim();
        let href = item.value().attr("url").unwrap_or_default().trim();
        if title.is_empty() || href.is_empty() || !seen.insert(href.to_string()) {
            continue;
        }
        let body_text = first_match(item, &snippet_selectors)
            .map(element_text)
            .unwrap_or_default();
        let date = first_match(item, &date_selectors).and_then(date_from_node);
        let source = item.value().attr("data-author").unwrap_or_default().trim();
        let _image = first_match(item, &image_selectors)
            .and_then(|node| node.value().attr("src"))
            .unwrap_or_default();

        results.push(NewsResult::new(title, href, body_text, date, source, NAME));
        if results.len() >= max_results {
            break;
        }
    }

    Ok(results)
}

fn date_from_node(node: ElementRef<'_>) -> Option<u64> {
    let text = node
        .value()
        .attr("aria-label")
        .map(str::to_owned)
        .unwrap_or_else(|| element_text(node));
    relative_date(&text)
}

fn relative_date(value: &str) -> Option<u64> {
    let lower = value.to_ascii_lowercase();
    let mut parts = lower.split_whitespace();
    let number = parts.next()?.parse::<u64>().ok()?;
    let unit = parts.next().unwrap_or("day");
    let seconds = if unit.starts_with("minute") {
        number * 60
    } else if unit.starts_with("hour") {
        number * 60 * 60
    } else if unit.starts_with("week") {
        number * 7 * 24 * 60 * 60
    } else if unit.starts_with("month") {
        number * 30 * 24 * 60 * 60
    } else if unit.starts_with("year") {
        number * 365 * 24 * 60 * 60
    } else {
        number * 24 * 60 * 60
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
    fn parses_bing_news_fixture() {
        let body = include_str!("../../../fixtures/bing_news_page.html");
        let results = parse_news(body, 10).expect("parse");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust language release improves tooling");
        assert_eq!(results[0].href, "https://example.com/rust-news");
        assert_eq!(results[0].source, "Example News");
        assert_eq!(results[0].provider, NAME);
        assert!(results[0].body.contains("compiler and Cargo"));
        assert!(results[0].date.is_some());
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
        assert!(results[0].href.starts_with("http"));
    }
}
