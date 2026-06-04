//! Cross-engine deduplication and ranking.
//!
//! Port of Python `ddgs`'s aggregation step (`results.py` + `similarity.py`):
//! results from all engines are deduplicated by normalized URL, then ordered by
//! [`SimpleFilterRanker`] into five buckets — wikipedia, then query-token match
//! in title+body, title-only, body-only, and finally no match — using
//! token-substring matching with a minimum token length of 3.
//!
//! The ranker is generic over [`Rankable`] so it serves any category that has a
//! URL, title, and body. [`dedup_key`] is the function the orchestrator uses to
//! deduplicate as engine results stream in; [`rank`] is applied afterward to the
//! surviving set.

use url::Url;

/// Minimum query-token length considered for match bucketing.
const MIN_TOKEN_LEN: usize = 3;

/// A result that can be deduplicated and ranked.
pub trait Rankable {
    /// Destination URL used for dedup and the wikipedia bucket.
    fn rank_url(&self) -> &str;
    /// Title text matched against query tokens.
    fn rank_title(&self) -> &str;
    /// Body/snippet text matched against query tokens.
    fn rank_body(&self) -> &str;
}

/// Normalize a URL for cross-engine deduplication.
///
/// Two URLs that point at the same page from different engines should collapse:
/// scheme is dropped, host is lowercased without a leading `www.`, a trailing
/// slash and any fragment are removed, and the query string is preserved (so
/// genuinely different pages on the same path stay distinct). Unparseable input
/// falls back to a trimmed, lowercased form.
pub fn normalize_url(raw: &str) -> String {
    match Url::parse(raw.trim()) {
        Ok(url) => {
            let host = url
                .host_str()
                .unwrap_or("")
                .trim_start_matches("www.")
                .to_ascii_lowercase();
            let path = url.path().trim_end_matches('/');
            let query = url
                .query()
                .map(|query| format!("?{query}"))
                .unwrap_or_default();
            format!("{host}{path}{query}")
        }
        Err(_) => raw.trim().trim_end_matches('/').to_ascii_lowercase(),
    }
}

/// Deduplication key for the orchestrator: the normalized URL, or `None` when
/// there is no URL to key on (such results are never deduplicated).
pub fn dedup_key<R: Rankable>(result: &R) -> Option<String> {
    let url = result.rank_url().trim();
    if url.is_empty() {
        None
    } else {
        Some(normalize_url(url))
    }
}

/// Reorder results by relevance bucket (a stable sort, so within-bucket order —
/// which reflects engine priority and arrival — is preserved).
pub fn rank<R: Rankable>(mut results: Vec<R>, query: &str) -> Vec<R> {
    let tokens = query_tokens(query);
    results.sort_by_key(|result| bucket(result, &tokens));
    results
}

/// Query tokens worth matching on: lowercased, length >= [`MIN_TOKEN_LEN`].
fn query_tokens(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|token| token.to_lowercase())
        .filter(|token| token.chars().count() >= MIN_TOKEN_LEN)
        .collect()
}

/// Relevance bucket; lower sorts earlier.
fn bucket<R: Rankable>(result: &R, tokens: &[String]) -> u8 {
    if is_wikipedia(result.rank_url()) {
        return 0;
    }

    let title = result.rank_title().to_lowercase();
    let body = result.rank_body().to_lowercase();
    let in_title = tokens.iter().all(|token| title.contains(token.as_str()));
    let in_body = tokens.iter().all(|token| body.contains(token.as_str()));

    match (in_title, in_body) {
        (true, true) => 1,
        (true, false) => 2,
        (false, true) => 3,
        (false, false) => 4,
    }
}

/// Whether a URL points at Wikipedia (any language host).
fn is_wikipedia(raw: &str) -> bool {
    Url::parse(raw.trim())
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
        .is_some_and(|host| host.contains("wikipedia.org"))
}

impl Rankable for crate::TextResult {
    fn rank_url(&self) -> &str {
        &self.href
    }
    fn rank_title(&self) -> &str {
        &self.title
    }
    fn rank_body(&self) -> &str {
        &self.body
    }
}

impl Rankable for crate::NewsResult {
    fn rank_url(&self) -> &str {
        &self.href
    }
    fn rank_title(&self) -> &str {
        &self.title
    }
    fn rank_body(&self) -> &str {
        &self.body
    }
}

impl Rankable for crate::ImageResult {
    // Images dedup by their full image URL; they have no body text to match.
    fn rank_url(&self) -> &str {
        &self.image
    }
    fn rank_title(&self) -> &str {
        &self.title
    }
    fn rank_body(&self) -> &str {
        ""
    }
}

impl Rankable for crate::VideoResult {
    // Videos dedup by their content URL; the description serves as body.
    fn rank_url(&self) -> &str {
        &self.content
    }
    fn rank_title(&self) -> &str {
        &self.title
    }
    fn rank_body(&self) -> &str {
        &self.description
    }
}

impl Rankable for crate::BooksResult {
    // Books dedup by detail URL; metadata serves as body text.
    fn rank_url(&self) -> &str {
        &self.url
    }
    fn rank_title(&self) -> &str {
        &self.title
    }
    fn rank_body(&self) -> &str {
        &self.info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct R {
        url: String,
        title: String,
        body: String,
    }

    impl R {
        fn new(url: &str, title: &str, body: &str) -> Self {
            Self {
                url: url.into(),
                title: title.into(),
                body: body.into(),
            }
        }
    }

    impl Rankable for R {
        fn rank_url(&self) -> &str {
            &self.url
        }
        fn rank_title(&self) -> &str {
            &self.title
        }
        fn rank_body(&self) -> &str {
            &self.body
        }
    }

    #[test]
    fn normalizes_for_dedup() {
        let a = normalize_url("https://www.Example.com/Path/");
        let b = normalize_url("http://example.com/Path#frag");
        assert_eq!(a, b);
        assert_eq!(a, "example.com/Path");
    }

    #[test]
    fn query_string_keeps_pages_distinct() {
        assert_ne!(
            normalize_url("https://site.com/a?id=1"),
            normalize_url("https://site.com/a?id=2"),
        );
    }

    #[test]
    fn dedup_key_skips_empty_urls() {
        assert!(dedup_key(&R::new("", "t", "b")).is_none());
        assert_eq!(
            dedup_key(&R::new("https://x.com/", "t", "b")),
            Some("x.com".to_string())
        );
    }

    #[test]
    fn ranks_into_buckets_in_order() {
        let results = vec![
            R::new("https://a.com", "nothing here", "nothing here"), // no-match
            R::new("https://b.com", "irrelevant", "rust language guide"), // body-only
            R::new("https://en.wikipedia.org/wiki/Rust", "Rust", "metal"), // wikipedia
            R::new("https://c.com", "rust tutorial", "all about rust"), // title+body
            R::new("https://d.com", "rust tutorial", "irrelevant"),  // title-only
        ];
        let ranked = rank(results, "rust");
        let order: Vec<&str> = ranked.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "https://en.wikipedia.org/wiki/Rust",
                "https://c.com",
                "https://d.com",
                "https://b.com",
                "https://a.com",
            ]
        );
    }

    #[test]
    fn ignores_short_query_tokens() {
        // "to" and "go" are below the min length, so only "rust" filters.
        let results = vec![
            R::new("https://a.com", "learn rust", "go to rust"),
            R::new("https://b.com", "cooking", "recipes"),
        ];
        let ranked = rank(results, "go to rust");
        assert_eq!(ranked[0].url, "https://a.com");
    }

    #[test]
    fn rank_is_stable_within_bucket() {
        // All no-match: original order must be preserved.
        let results = vec![
            R::new("https://1.com", "x", "y"),
            R::new("https://2.com", "x", "y"),
            R::new("https://3.com", "x", "y"),
        ];
        let ranked = rank(results, "zzz");
        let order: Vec<&str> = ranked.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            order,
            vec!["https://1.com", "https://2.com", "https://3.com"]
        );
    }
}
