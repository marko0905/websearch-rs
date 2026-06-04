use std::time::Duration;

use crate::{
    BooksResult, BooksSearchRequest, ImageResult, ImageSearchRequest, NewsResult,
    NewsSearchRequest, Result, TextResult, TextSearchRequest, VideoResult, VideoSearchRequest,
    WebSearchConfig, WebSearchError,
    engines::{Backend, EngineRegistry, OrchestrationOptions, orchestrate},
    http::{HttpClient, HttpConfig},
    ranking,
};

/// Async web search client.
///
/// A metasearch client: each category method selects engines per the request
/// `backend`, runs them concurrently through the orchestrator, then deduplicates
/// and ranks the merged results.
#[derive(Clone, Debug)]
pub struct WebSearchClient {
    config: WebSearchConfig,
    /// Browser-impersonating transport shared by all engines.
    http: HttpClient,
}

impl WebSearchClient {
    /// Create a client builder.
    pub fn builder() -> WebSearchClientBuilder {
        WebSearchClientBuilder::default()
    }

    /// Create a client with default configuration.
    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    /// Return the active client configuration.
    pub fn config(&self) -> &WebSearchConfig {
        &self.config
    }

    /// Run a text search with default request settings.
    pub async fn text(&self, query: impl Into<String>) -> Result<Vec<TextResult>> {
        self.text_with(TextSearchRequest::new(query)).await
    }

    /// Run a text search with a full request object.
    pub async fn text_with(&self, request: TextSearchRequest) -> Result<Vec<TextResult>> {
        let registry = EngineRegistry::new();
        self.run(request, registry.text()).await
    }

    /// Run a video search with default request settings.
    pub async fn videos(&self, query: impl Into<String>) -> Result<Vec<VideoResult>> {
        self.videos_with(VideoSearchRequest::new(query)).await
    }

    /// Run a video search with a full request object.
    pub async fn videos_with(&self, request: VideoSearchRequest) -> Result<Vec<VideoResult>> {
        let registry = EngineRegistry::new();
        self.run(request, registry.videos()).await
    }

    /// Run an image search with default request settings.
    pub async fn images(&self, query: impl Into<String>) -> Result<Vec<ImageResult>> {
        self.images_with(ImageSearchRequest::new(query)).await
    }

    /// Run an image search with a full request object.
    pub async fn images_with(&self, request: ImageSearchRequest) -> Result<Vec<ImageResult>> {
        let registry = EngineRegistry::new();
        self.run(request, registry.images()).await
    }

    /// Run a news search with default request settings.
    pub async fn news(&self, query: impl Into<String>) -> Result<Vec<NewsResult>> {
        self.news_with(NewsSearchRequest::new(query)).await
    }

    /// Run a news search with a full request object.
    pub async fn news_with(&self, request: NewsSearchRequest) -> Result<Vec<NewsResult>> {
        let registry = EngineRegistry::new();
        self.run(request, registry.news()).await
    }

    /// Run a books search with default request settings.
    pub async fn books(&self, query: impl Into<String>) -> Result<Vec<BooksResult>> {
        self.books_with(BooksSearchRequest::new(query)).await
    }

    /// Run a books search with a full request object.
    pub async fn books_with(&self, request: BooksSearchRequest) -> Result<Vec<BooksResult>> {
        let registry = EngineRegistry::new();
        self.run(request, registry.books()).await
    }

    /// Shared metasearch pipeline: validate → select engines for the request
    /// `backend` → orchestrate concurrently → deduplicate → rank.
    async fn run<Req, Res>(
        &self,
        request: Req,
        engines: &crate::engines::EngineSet<Req, Res>,
    ) -> Result<Vec<Res>>
    where
        Req: SearchParams + Clone + Send + Sync + 'static,
        Res: ranking::Rankable + Send + 'static,
    {
        request.validate()?;
        let selected = engines.select(&Backend::parse(request.backend()));
        let options = OrchestrationOptions {
            max_results: request.max_results(),
            per_engine_timeout: self.config.timeout,
        };
        let query = request.query().to_string();
        let results =
            orchestrate(&self.http, selected, request, &options, ranking::dedup_key).await;
        Ok(ranking::rank(results, &query))
    }
}

/// The request fields the orchestration pipeline needs, uniform across
/// categories.
pub trait SearchParams {
    /// Validate the request before dispatching it.
    fn validate(&self) -> Result<()>;
    /// The search query (used for ranking).
    fn query(&self) -> &str;
    /// The `backend` selection string.
    fn backend(&self) -> &str;
    /// Maximum results to aggregate.
    fn max_results(&self) -> usize;
}

macro_rules! impl_search_params {
    ($($ty:ty),+ $(,)?) => {
        $(impl SearchParams for $ty {
            fn validate(&self) -> Result<()> {
                <$ty>::validate(self)
            }
            fn query(&self) -> &str {
                &self.query
            }
            fn backend(&self) -> &str {
                &self.backend
            }
            fn max_results(&self) -> usize {
                self.max_results
            }
        })+
    };
}

impl_search_params!(
    TextSearchRequest,
    VideoSearchRequest,
    ImageSearchRequest,
    NewsSearchRequest,
    BooksSearchRequest,
);

impl Default for WebSearchClient {
    fn default() -> Self {
        Self::new().unwrap_or_else(|err| {
            panic!("failed to construct default web search client: {err}");
        })
    }
}

/// Builder for [`WebSearchClient`].
#[derive(Clone, Debug)]
pub struct WebSearchClientBuilder {
    timeout: Duration,
    proxy: Option<String>,
    max_retries: usize,
    retry_delay: Duration,
}

impl WebSearchClientBuilder {
    /// Set per-request timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set an HTTP/HTTPS/SOCKS proxy URL.
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    /// Disable proxy usage.
    pub fn no_proxy(mut self) -> Self {
        self.proxy = None;
        self
    }

    /// Set maximum retries for transient network failures.
    pub fn max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set initial retry delay; later retries use linear backoff.
    pub fn retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = retry_delay;
        self
    }

    /// Build the client.
    pub fn build(self) -> Result<WebSearchClient> {
        if self.timeout.is_zero() {
            return Err(WebSearchError::InvalidConfig(
                "timeout must be greater than zero".into(),
            ));
        }

        let config = WebSearchConfig {
            timeout: self.timeout,
            proxy: self.proxy,
            max_retries: self.max_retries,
            retry_delay: self.retry_delay,
        };

        let http = HttpClient::new(&HttpConfig {
            timeout: config.timeout,
            proxy: config.proxy.clone(),
            max_retries: config.max_retries,
            retry_delay: config.retry_delay,
        })?;

        Ok(WebSearchClient { config, http })
    }
}

impl Default for WebSearchClientBuilder {
    fn default() -> Self {
        let config = WebSearchConfig::default();
        Self {
            timeout: config.timeout,
            proxy: None,
            max_retries: config.max_retries,
            retry_delay: config.retry_delay,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_timeout() {
        let err = WebSearchClient::builder()
            .timeout(Duration::ZERO)
            .build()
            .expect_err("zero timeout should fail");

        assert!(matches!(err, WebSearchError::InvalidConfig(_)));
    }

    #[test]
    fn builds_with_defaults() {
        let client = WebSearchClient::new().expect("default client should build");
        assert_eq!(client.config().max_retries, 2);
    }
}
