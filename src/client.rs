use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{
    BooksResult, BooksSearchRequest, ImageResult, ImageSearchRequest, NewsResult,
    NewsSearchRequest, Result, TextResult, TextSearchRequest, VideoResult, VideoSearchRequest,
    WebSearchConfig, WebSearchError,
    engines::{
        Backend, EngineRegistry, EngineSet, Orchestrated, OrchestrationOptions, orchestrate,
    },
    http::{HttpClient, HttpConfig, Verify, resolve_proxy},
    ranking,
};

/// Per-engine HTTP clients, keyed by `(category, engine name)`.
type EngineClients = Mutex<HashMap<(&'static str, &'static str), HttpClient>>;

/// Async web search client — the Rust counterpart of upstream `DDGS`.
///
/// Each category method selects engines per the request `backend`, runs them
/// through the orchestrator, aggregates and ranks the results. Like a `DDGS`
/// instance (which caches one engine instance per engine class), the client
/// keeps one HTTP client per engine — with its own browser fingerprint and
/// cookie store — for its whole life. Clones share them.
#[derive(Clone)]
pub struct WebSearchClient {
    config: WebSearchConfig,
    registry: Arc<EngineRegistry>,
    engine_clients: Arc<EngineClients>,
}

impl std::fmt::Debug for WebSearchClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSearchClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
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
        self.run(request, self.registry.text()).await
    }

    /// Run a video search with default request settings.
    pub async fn videos(&self, query: impl Into<String>) -> Result<Vec<VideoResult>> {
        self.videos_with(VideoSearchRequest::new(query)).await
    }

    /// Run a video search with a full request object.
    pub async fn videos_with(&self, request: VideoSearchRequest) -> Result<Vec<VideoResult>> {
        self.run(request, self.registry.videos()).await
    }

    /// Run an image search with default request settings.
    pub async fn images(&self, query: impl Into<String>) -> Result<Vec<ImageResult>> {
        self.images_with(ImageSearchRequest::new(query)).await
    }

    /// Run an image search with a full request object.
    pub async fn images_with(&self, request: ImageSearchRequest) -> Result<Vec<ImageResult>> {
        self.run(request, self.registry.images()).await
    }

    /// Run a news search with default request settings.
    pub async fn news(&self, query: impl Into<String>) -> Result<Vec<NewsResult>> {
        self.news_with(NewsSearchRequest::new(query)).await
    }

    /// Run a news search with a full request object.
    pub async fn news_with(&self, request: NewsSearchRequest) -> Result<Vec<NewsResult>> {
        self.run(request, self.registry.news()).await
    }

    /// Run a books search with default request settings.
    pub async fn books(&self, query: impl Into<String>) -> Result<Vec<BooksResult>> {
        self.books_with(BooksSearchRequest::new(query)).await
    }

    /// Run a books search with a full request object.
    pub async fn books_with(&self, request: BooksSearchRequest) -> Result<Vec<BooksResult>> {
        self.run(request, self.registry.books()).await
    }

    /// Upstream `DDGS.extract(url, fmt)`: fetch `url` with this client's
    /// proxy, timeout and TLS settings and return its content in `format`.
    /// A status other than 200 is an error ("Failed to fetch {url}: HTTP
    /// {status}").
    pub async fn extract(
        &self,
        url: &str,
        format: crate::ExtractFormat,
    ) -> Result<crate::ExtractResult> {
        crate::extract::extract(&http_config(&self.config), url, format).await
    }

    /// The engine's own HTTP client, created on first use and then reused
    /// (upstream `_engines_cache`).
    fn engine_http(&self, category: &'static str, name: &'static str) -> Result<HttpClient> {
        let mut clients = self
            .engine_clients
            .lock()
            .map_err(|_| WebSearchError::Internal("engine client cache poisoned".into()))?;
        if let Some(client) = clients.get(&(category, name)) {
            return Ok(client.clone());
        }
        let client = HttpClient::new(&http_config(&self.config))?;
        clients.insert((category, name), client.clone());
        Ok(client)
    }

    /// Port of upstream `DDGS._search_sync`: validate → select engines for
    /// the request `backend` → orchestrate → extract by frequency → rank →
    /// truncate, or raise when nothing was found.
    async fn run<Req, Res>(&self, request: Req, engines: &EngineSet<Req, Res>) -> Result<Vec<Res>>
    where
        Req: SearchParams + Send + Sync + 'static,
        Res: ranking::Rankable + Send + 'static,
    {
        request.validate()?;
        let selected = engines
            .select(&Backend::parse(request.backend()))
            .into_iter()
            .map(|engine| {
                let http = self.engine_http(engines.category(), engine.name())?;
                Ok((engine, http))
            })
            .collect::<Result<Vec<_>>>()?;
        let options = OrchestrationOptions {
            max_results: request.max_results(),
            timeout: self.config.timeout,
            threads: self.config.threads,
        };
        let query = request.query().to_owned();
        let Orchestrated {
            aggregator,
            last_error,
        } = orchestrate(selected, request, &options).await?;
        let results = ranking::rank(aggregator.extract(), &query)?;
        finish(results, options.max_results, last_error)
    }
}

/// The tail of upstream `_search_sync`:
///
/// ```python
/// if results:
///     return results[:max_results] if max_results else results
/// if "timed out" in f"{err}":
///     raise TimeoutException(err)
/// raise DDGSException(err or "No results found.")
/// ```
pub(crate) fn finish<Res>(
    mut results: Vec<Res>,
    max_results: Option<usize>,
    last_error: Option<WebSearchError>,
) -> Result<Vec<Res>> {
    if !results.is_empty() {
        if let Some(max_results) = max_results.filter(|&n| n > 0) {
            results.truncate(max_results);
        }
        return Ok(results);
    }
    Err(match last_error {
        Some(error @ WebSearchError::Timeout(_)) => error,
        Some(error) if error.to_string().contains("timed out") => {
            WebSearchError::Timeout(error.to_string())
        }
        Some(error) => error,
        None => WebSearchError::NoResults,
    })
}

fn http_config(config: &WebSearchConfig) -> HttpConfig {
    HttpConfig {
        timeout: config.timeout,
        proxy: config.proxy.clone(),
        verify: config.verify.clone(),
        max_retries: config.max_retries,
        retry_delay: config.retry_delay,
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
    /// Maximum results to return (`None`/`0`: all).
    fn max_results(&self) -> Option<usize>;
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
            fn max_results(&self) -> Option<usize> {
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
///
/// Defaults follow upstream `DDGS()`: 5 s timeout, proxy from `DDGS_PROXY`,
/// certificate verification on, automatic concurrency, and no retries.
#[derive(Clone, Debug)]
pub struct WebSearchClientBuilder {
    timeout: Option<Duration>,
    proxy: Option<String>,
    verify: Verify,
    threads: Option<usize>,
    max_retries: usize,
    retry_delay: Duration,
}

impl WebSearchClientBuilder {
    /// Set the per-request timeout (also how long each engine batch is
    /// awaited). Default 5 s.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// No timeout (upstream `DDGS(timeout=None)`).
    pub fn no_timeout(mut self) -> Self {
        self.timeout = None;
        self
    }

    /// Set an HTTP/HTTPS/SOCKS proxy URL. `"tb"` is an alias for
    /// `socks5h://127.0.0.1:9150` (Tor Browser).
    pub fn proxy(mut self, proxy: impl Into<String>) -> Self {
        self.proxy = Some(proxy.into());
        self
    }

    /// Clear the explicit proxy. As upstream, the `DDGS_PROXY` environment
    /// variable still applies when it is set.
    pub fn no_proxy(mut self) -> Self {
        self.proxy = None;
        self
    }

    /// TLS certificate verification (upstream `verify`). Default: verify.
    pub fn verify(mut self, verify: Verify) -> Self {
        self.verify = verify;
        self
    }

    /// Cap the number of engines run at once (upstream `DDGS.threads`).
    /// `0` means automatic (the default).
    pub fn threads(mut self, threads: usize) -> Self {
        self.threads = (threads > 0).then_some(threads);
        self
    }

    /// Rust-only: retries for transient network failures. Upstream never
    /// retries, so the default is `0`.
    pub fn max_retries(mut self, max_retries: usize) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Rust-only: initial retry delay; later retries use linear backoff.
    pub fn retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = retry_delay;
        self
    }

    /// Build the client.
    ///
    /// Unlike upstream (which builds engine clients lazily and so reports a
    /// bad proxy only when searching), the transport settings are checked
    /// here by building one throwaway client.
    pub fn build(self) -> Result<WebSearchClient> {
        if self.timeout.is_some_and(|timeout| timeout.is_zero()) {
            return Err(WebSearchError::InvalidConfig(
                "timeout must be greater than zero".into(),
            ));
        }

        let config = WebSearchConfig {
            timeout: self.timeout,
            proxy: resolve_proxy(self.proxy.as_deref()),
            verify: self.verify,
            threads: self.threads,
            max_retries: self.max_retries,
            retry_delay: self.retry_delay,
        };

        HttpClient::new(&http_config(&config))?;

        Ok(WebSearchClient {
            config,
            registry: Arc::new(EngineRegistry::new()),
            engine_clients: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

impl Default for WebSearchClientBuilder {
    fn default() -> Self {
        let config = WebSearchConfig::default();
        Self {
            timeout: config.timeout,
            proxy: None,
            verify: config.verify,
            threads: config.threads,
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
    fn builds_with_upstream_defaults() {
        let client = WebSearchClient::new().expect("default client should build");
        let config = client.config();
        assert_eq!(config.max_retries, 0);
        assert_eq!(config.timeout, Some(Duration::from_secs(5)));
        assert_eq!(config.verify, Verify::Enabled);
        assert_eq!(config.threads, None);
    }

    #[test]
    fn tb_proxy_alias_expands() {
        let client = WebSearchClient::builder()
            .proxy("tb")
            .build()
            .expect("client");
        assert_eq!(
            client.config().proxy.as_deref(),
            Some("socks5h://127.0.0.1:9150")
        );
    }

    #[test]
    fn engine_clients_are_cached_per_engine() {
        let client = WebSearchClient::new().expect("client");
        client.engine_http("text", "brave").expect("brave");
        client.engine_http("text", "brave").expect("brave again");
        client.engine_http("news", "duckduckgo").expect("ddg news");
        client.engine_http("text", "duckduckgo").expect("ddg text");
        let cache = client.engine_clients.lock().expect("lock");
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn finish_truncates_or_returns_all() {
        assert_eq!(finish(vec![1, 2, 3], Some(2), None).unwrap(), [1, 2]);
        assert_eq!(finish(vec![1, 2, 3], None, None).unwrap(), [1, 2, 3]);
        assert_eq!(finish(vec![1, 2, 3], Some(0), None).unwrap(), [1, 2, 3]);
        // Results win over a recorded error.
        let err = Some(WebSearchError::Http("x".into()));
        assert_eq!(finish(vec![1], Some(5), err).unwrap(), [1]);
    }

    #[test]
    fn finish_maps_empty_results_to_upstream_errors() {
        let none: Vec<u8> = Vec::new();
        assert!(matches!(
            finish(none.clone(), Some(10), None),
            Err(WebSearchError::NoResults)
        ));
        assert_eq!(WebSearchError::NoResults.to_string(), "No results found.");
        // "timed out" anywhere in the last error → timeout.
        assert!(matches!(
            finish(
                none.clone(),
                Some(10),
                Some(WebSearchError::Http("operation timed out".into()))
            ),
            Err(WebSearchError::Timeout(_))
        ));
        assert!(matches!(
            finish(
                none.clone(),
                Some(10),
                Some(WebSearchError::Timeout("t".into()))
            ),
            Err(WebSearchError::Timeout(_))
        ));
        // Otherwise the last error itself.
        assert!(matches!(
            finish(none, Some(10), Some(WebSearchError::Parse("bad".into()))),
            Err(WebSearchError::Parse(message)) if message == "bad"
        ));
    }

    mod pipeline {
        use std::sync::Arc;

        use super::*;
        use crate::engines::tests::{Behavior, Probe, mock};

        fn set(engines: Vec<(&'static str, Behavior)>) -> EngineSet<TextSearchRequest, TextResult> {
            let probe = Arc::new(Probe::default());
            EngineSet::new(
                "news",
                engines
                    .into_iter()
                    .map(|(name, behavior)| mock(name, name, behavior, &probe))
                    .collect(),
            )
        }

        #[tokio::test]
        async fn ranks_by_frequency_then_bucket_and_truncates() {
            let client = WebSearchClient::new().expect("client");
            let engines = set(vec![
                (
                    "e1",
                    Behavior::Results(0, vec!["https://a.com", "https://b.com"]),
                ),
                (
                    "e2",
                    Behavior::Results(0, vec!["https://b.com", "https://rust.dev"]),
                ),
            ]);
            // Unranked by frequency: b (2), then a, rust.dev (first-seen
            // order, which depends on the shuffled engine order).
            let request = TextSearchRequest::new("rust").max_results(None);
            let results = client.run(request, &engines).await.expect("results");
            let hrefs: Vec<_> = results.iter().map(|r| r.href.as_str()).collect();
            // The ranker pulls the title match ("e2 https://rust.dev") first.
            assert_eq!(hrefs[0], "https://rust.dev");
            assert_eq!(hrefs[1], "https://b.com");
            assert_eq!(hrefs.len(), 3);

            let request = TextSearchRequest::new("rust").max_results(2);
            let results = client.run(request, &engines).await.expect("results");
            assert_eq!(results.len(), 2);
        }

        #[tokio::test]
        async fn no_results_and_no_error_is_no_results_found() {
            let client = WebSearchClient::new().expect("client");
            let engines = set(vec![("e1", Behavior::Results(0, vec![]))]);
            let err = client
                .run(TextSearchRequest::new("q"), &engines)
                .await
                .expect_err("empty");
            assert!(matches!(err, WebSearchError::NoResults));
        }

        #[tokio::test]
        async fn all_failed_reports_the_last_error() {
            let client = WebSearchClient::new().expect("client");
            let engines = set(vec![("e1", Behavior::Fail(0))]);
            let err = client
                .run(TextSearchRequest::new("q"), &engines)
                .await
                .expect_err("failed");
            assert!(matches!(err, WebSearchError::Http(m) if m == "boom"));
        }
    }

    #[tokio::test]
    async fn empty_query_is_mandatory() {
        let client = WebSearchClient::new().expect("client");
        let err = client.text("").await.expect_err("empty query");
        assert!(matches!(err, WebSearchError::InvalidConfig(m) if m == "query is mandatory."));
    }
}
