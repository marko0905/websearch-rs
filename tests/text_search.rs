use std::time::Duration;

use websearch_rs::{
    BooksSearchRequest, ImageSearchRequest, NewsSearchRequest, Region, SafeSearch,
    TextSearchRequest, VideoSearchRequest, WebSearchClient, WebSearchError,
};

#[test]
fn request_validation_rejects_empty_query() {
    let request = TextSearchRequest::new("");
    let err = request
        .validate()
        .expect_err("empty query should be rejected");

    assert!(matches!(err, WebSearchError::InvalidConfig(_)));
}

#[test]
fn request_builder_sets_expected_fields() {
    let request = TextSearchRequest::new("rust")
        .max_results(3)
        .region(Region::new("us-en").expect("valid region"))
        .safe_search(SafeSearch::On)
        .backend("brave,duckduckgo")
        .page(2);

    assert_eq!(request.query, "rust");
    assert_eq!(request.max_results, Some(3));
    assert_eq!(request.region.as_str(), "us-en");
    assert_eq!(request.safe_search, SafeSearch::On);
    assert_eq!(request.backend, "brave,duckduckgo");
    assert_eq!(request.page, 2);
}

#[test]
fn video_request_validation_rejects_empty_query() {
    let request = VideoSearchRequest::new("");
    let err = request
        .validate()
        .expect_err("empty query should be rejected");

    assert!(matches!(err, WebSearchError::InvalidConfig(_)));
}

#[test]
fn video_request_builder_sets_expected_fields() {
    let request = VideoSearchRequest::new("rust")
        .max_results(4)
        .region(Region::new("us-en").expect("valid region"))
        .safe_search(SafeSearch::Off)
        .backend("duckduckgo")
        .page(2);

    assert_eq!(request.query, "rust");
    assert_eq!(request.max_results, Some(4));
    assert_eq!(request.region.as_str(), "us-en");
    assert_eq!(request.safe_search, SafeSearch::Off);
    assert_eq!(request.backend, "duckduckgo");
    assert_eq!(request.page, 2);
}

#[test]
fn image_request_validation_rejects_empty_query() {
    let request = ImageSearchRequest::new("");
    let err = request
        .validate()
        .expect_err("empty query should be rejected");

    assert!(matches!(err, WebSearchError::InvalidConfig(_)));
}

#[test]
fn image_request_builder_sets_expected_fields() {
    let request = ImageSearchRequest::new("rust")
        .max_results(4)
        .region(Region::new("us-en").expect("valid region"))
        .safe_search(SafeSearch::Off)
        .backend("duckduckgo")
        .page(2);

    assert_eq!(request.query, "rust");
    assert_eq!(request.max_results, Some(4));
    assert_eq!(request.region.as_str(), "us-en");
    assert_eq!(request.safe_search, SafeSearch::Off);
    assert_eq!(request.backend, "duckduckgo");
    assert_eq!(request.page, 2);
}

#[test]
fn news_request_validation_rejects_empty_query() {
    let request = NewsSearchRequest::new("");
    let err = request
        .validate()
        .expect_err("empty query should be rejected");

    assert!(matches!(err, WebSearchError::InvalidConfig(_)));
}

#[test]
fn news_request_builder_sets_expected_fields() {
    let request = NewsSearchRequest::new("rust")
        .max_results(4)
        .region(Region::new("us-en").expect("valid region"))
        .safe_search(SafeSearch::Off)
        .backend("duckduckgo")
        .page(2);

    assert_eq!(request.query, "rust");
    assert_eq!(request.max_results, Some(4));
    assert_eq!(request.region.as_str(), "us-en");
    assert_eq!(request.safe_search, SafeSearch::Off);
    assert_eq!(request.backend, "duckduckgo");
    assert_eq!(request.page, 2);
}

#[test]
fn books_request_validation_rejects_empty_query() {
    let request = BooksSearchRequest::new("");
    let err = request
        .validate()
        .expect_err("empty query should be rejected");

    assert!(matches!(err, WebSearchError::InvalidConfig(_)));
}

#[test]
fn books_request_builder_sets_expected_fields() {
    let request = BooksSearchRequest::new("rust")
        .max_results(4)
        .backend("annasarchive")
        .page(2);

    assert_eq!(request.query, "rust");
    assert_eq!(request.max_results, Some(4));
    assert_eq!(request.backend, "annasarchive");
    assert_eq!(request.page, 2);
}

#[test]
fn client_builder_accepts_basic_configuration() {
    let client = WebSearchClient::builder()
        .timeout(Duration::from_secs(5))
        .max_retries(3)
        .retry_delay(Duration::from_millis(25))
        .build()
        .expect("client should build");

    assert_eq!(client.config().timeout, Some(Duration::from_secs(5)));
    assert_eq!(client.config().max_retries, 3);
    assert_eq!(client.config().retry_delay, Duration::from_millis(25));
}

#[tokio::test]
#[ignore = "live provider test; depends on network and upstream HTML"]
async fn live_text_search_returns_results() {
    let client = WebSearchClient::new().expect("client should build");
    let results = client
        .text("rust programming language")
        .await
        .expect("search should work");

    assert!(!results.is_empty());
    assert!(results.iter().all(|result| !result.title.is_empty()));
    assert!(results.iter().all(|result| !result.href.is_empty()));
}

#[tokio::test]
#[ignore = "live provider test; depends on network and upstream videos API"]
async fn live_video_search_returns_results() {
    let client = WebSearchClient::new().expect("client should build");
    let results = client
        .videos("rust programming language")
        .await
        .expect("videos should work");

    assert!(!results.is_empty());
    assert!(results.iter().all(|result| {
        result
            .title
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }));
    assert!(results.iter().all(|result| {
        result
            .content
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }));
}

#[tokio::test]
#[ignore = "live provider test; depends on network and upstream images API"]
async fn live_image_search_returns_results() {
    let client = WebSearchClient::new().expect("client should build");
    let results = client
        .images("rust programming language")
        .await
        .expect("images should work");

    assert!(!results.is_empty());
    assert!(results.iter().all(|result| {
        result
            .image
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }));
    assert!(
        results
            .iter()
            .all(|result| result.url.as_deref().is_some_and(|value| !value.is_empty()))
    );
}

#[tokio::test]
#[ignore = "live provider test; depends on network and upstream news API"]
async fn live_news_search_returns_results() {
    let client = WebSearchClient::new().expect("client should build");
    let results = client
        .news("rust programming language")
        .await
        .expect("news should work");

    assert!(!results.is_empty());
    assert!(results.iter().all(|result| {
        result
            .title
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    }));
    assert!(
        results
            .iter()
            .all(|result| result.url.as_deref().is_some_and(|value| !value.is_empty()))
    );
}

#[tokio::test]
#[ignore = "live provider test; depends on network and upstream books API"]
async fn live_books_search_returns_results() {
    let client = WebSearchClient::new().expect("client should build");
    let results = client
        .books("rust programming")
        .await
        .expect("books should work");

    assert!(!results.is_empty());
    assert!(results.iter().all(|result| !result.title.is_empty()));
    assert!(results.iter().all(|result| !result.url.is_empty()));
}

#[test]
fn request_defaults_match_upstream() {
    let request = TextSearchRequest::new("q");
    assert_eq!(request.region.as_str(), "us-en");
    assert_eq!(Region::default().as_str(), "us-en");
    assert_eq!(request.max_results, Some(10));
    assert_eq!(request.backend, "auto");
    assert_eq!(request.page, 1);
    // `max_results` None/0 means "all" and is valid; a whitespace query is
    // searched as is (upstream only rejects an empty one).
    assert!(
        TextSearchRequest::new("q")
            .max_results(None)
            .validate()
            .is_ok()
    );
    assert!(
        TextSearchRequest::new("q")
            .max_results(0)
            .validate()
            .is_ok()
    );
    assert!(TextSearchRequest::new(" ").validate().is_ok());
    assert!(TextSearchRequest::new("").validate().is_err());
}
