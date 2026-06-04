use tokio::runtime::{Builder, Runtime};

use crate::{
    BooksResult, BooksSearchRequest, ImageResult, ImageSearchRequest, NewsResult,
    NewsSearchRequest, Result, TextResult, TextSearchRequest, VideoResult, VideoSearchRequest,
    WebSearchClient,
};

/// Synchronous wrapper around [`WebSearchClient`].
///
/// This is useful for CLI-style integrations and future Python bindings. It
/// owns a Tokio runtime and blocks on the async client methods.
pub struct WebSearchBlockingClient {
    client: WebSearchClient,
    runtime: Runtime,
}

impl WebSearchBlockingClient {
    /// Create a blocking client with default async client configuration.
    pub fn new() -> Result<Self> {
        Self::from_client(WebSearchClient::new()?)
    }

    /// Wrap an existing async client in a blocking client.
    pub fn from_client(client: WebSearchClient) -> Result<Self> {
        let runtime = Builder::new_multi_thread().enable_all().build()?;
        Ok(Self { client, runtime })
    }

    /// Return the underlying async client.
    pub fn client(&self) -> &WebSearchClient {
        &self.client
    }

    /// Run a text search with default request settings.
    pub fn text(&self, query: impl Into<String>) -> Result<Vec<TextResult>> {
        self.text_with(TextSearchRequest::new(query))
    }

    /// Run a text search with a full request object.
    pub fn text_with(&self, request: TextSearchRequest) -> Result<Vec<TextResult>> {
        self.runtime.block_on(self.client.text_with(request))
    }

    /// Run an image search with default request settings.
    pub fn images(&self, query: impl Into<String>) -> Result<Vec<ImageResult>> {
        self.images_with(ImageSearchRequest::new(query))
    }

    /// Run an image search with a full request object.
    pub fn images_with(&self, request: ImageSearchRequest) -> Result<Vec<ImageResult>> {
        self.runtime.block_on(self.client.images_with(request))
    }

    /// Run a video search with default request settings.
    pub fn videos(&self, query: impl Into<String>) -> Result<Vec<VideoResult>> {
        self.videos_with(VideoSearchRequest::new(query))
    }

    /// Run a video search with a full request object.
    pub fn videos_with(&self, request: VideoSearchRequest) -> Result<Vec<VideoResult>> {
        self.runtime.block_on(self.client.videos_with(request))
    }

    /// Run a news search with default request settings.
    pub fn news(&self, query: impl Into<String>) -> Result<Vec<NewsResult>> {
        self.news_with(NewsSearchRequest::new(query))
    }

    /// Run a news search with a full request object.
    pub fn news_with(&self, request: NewsSearchRequest) -> Result<Vec<NewsResult>> {
        self.runtime.block_on(self.client.news_with(request))
    }

    /// Run a books search with default request settings.
    pub fn books(&self, query: impl Into<String>) -> Result<Vec<BooksResult>> {
        self.books_with(BooksSearchRequest::new(query))
    }

    /// Run a books search with a full request object.
    pub fn books_with(&self, request: BooksSearchRequest) -> Result<Vec<BooksResult>> {
        self.runtime.block_on(self.client.books_with(request))
    }
}

impl Default for WebSearchBlockingClient {
    fn default() -> Self {
        Self::new().unwrap_or_else(|err| {
            panic!("failed to construct default blocking web search client: {err}");
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_client_wraps_async_client() {
        let async_client = WebSearchClient::builder()
            .max_retries(0)
            .build()
            .expect("async client should build");
        let blocking = WebSearchBlockingClient::from_client(async_client)
            .expect("blocking client should build");

        assert_eq!(blocking.client().config().max_retries, 0);
    }
}
