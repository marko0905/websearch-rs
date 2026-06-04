use std::time::Duration;

use websearch_rs::{NewsSearchRequest, Region, SafeSearch, TextSearchRequest, WebSearchClient};

#[tokio::main]
async fn main() -> websearch_rs::Result<()> {
    let client = WebSearchClient::builder()
        .timeout(Duration::from_secs(10))
        .max_retries(2)
        .retry_delay(Duration::from_millis(500))
        .build()?;

    let text_request = TextSearchRequest::new("rust programming")
        .max_results(5)
        .region(Region::new("us-en")?)
        .safe_search(SafeSearch::Moderate);
    let text_results = client.text_with(text_request).await?;
    println!("text results: {}", text_results.len());

    let image_results = client.images("rust programming").await?;
    println!("image results: {}", image_results.len());

    let video_results = client.videos("rust programming").await?;
    println!("video results: {}", video_results.len());

    let news_request = NewsSearchRequest::new("rust programming").max_results(5);
    let news_results = client.news_with(news_request).await?;
    println!("news results: {}", news_results.len());

    let books_results = client.books("rust programming").await?;
    println!("books results: {}", books_results.len());

    Ok(())
}
