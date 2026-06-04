//! Engine abstraction and orchestration — the metasearch core.
//!
//! Mirrors Python `ddgs`'s `BaseSearchEngine` / `EngineRegistry` / orchestrator
//! model. One engine = one struct implementing [`SearchEngine`] for a category.
//! The orchestrator fans requests out to the selected engines concurrently,
//! skips engines that error (the fallback mechanism), deduplicates, and stops
//! once enough results are collected.
//!
//! Concrete engines live under `engines/<category>/` and are registered in
//! [`EngineRegistry`] (added in the engine-porting phase). This module holds the
//! category-agnostic machinery, kept generic over request/result type so the
//! same orchestrator serves text, news, images, videos, and books.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::task::JoinSet;

use crate::http::HttpClient;
use crate::{Result, WebSearchError};

pub mod books;
pub mod images;
pub mod news;
pub mod registry;
pub mod text;
pub mod util;
pub mod videos;

pub use registry::EngineRegistry;

/// A single search engine for one category.
///
/// `Req`/`Res` are the category's request and result types (e.g.
/// `TextSearchRequest` / `TextResult`). Engines never construct their own HTTP
/// client; they receive the shared [`HttpClient`] so impersonation, retries,
/// proxy, and timeout stay centralized.
#[async_trait::async_trait]
pub trait SearchEngine<Req, Res>: Send + Sync
where
    Req: Send + Sync,
    Res: Send,
{
    /// Stable engine identifier used in `backend` selection and logs
    /// (e.g. `"bing"`, `"duckduckgo_news"`). Must match Python `ddgs`.
    fn name(&self) -> &'static str;

    /// Selection/ordering priority; lower runs earlier. Ties are broken
    /// randomly. Defaults to a neutral middle value; engines that should lead
    /// (e.g. `wikipedia`, `grokipedia` for text) override with a smaller value.
    fn priority(&self) -> u8 {
        100
    }

    /// Run the search and return this engine's results.
    async fn search(&self, http: &HttpClient, request: &Req) -> Result<Vec<Res>>;
}

/// Convenience aliases for boxed per-category engines.
pub type DynEngine<Req, Res> = Arc<dyn SearchEngine<Req, Res>>;

/// Backend selection, parsed from the `backend` parameter.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Backend {
    /// Use every engine registered for the category.
    #[default]
    Auto,
    /// Use the named engines, in registry order. Unknown names are ignored
    /// (with a warning); if none are valid the orchestrator falls back to
    /// [`Backend::Auto`].
    Engines(Vec<String>),
}

impl Backend {
    /// Parse a `backend` string: `"auto"` (or empty) → [`Backend::Auto`];
    /// otherwise a comma-separated engine list.
    pub fn parse(value: &str) -> Self {
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
            return Self::Auto;
        }
        let engines: Vec<String> = trimmed
            .split(',')
            .map(|name| name.trim().to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        if engines.is_empty() {
            Self::Auto
        } else {
            Self::Engines(engines)
        }
    }
}

/// An ordered set of engines for one category, plus selection logic.
pub struct EngineSet<Req, Res> {
    engines: Vec<DynEngine<Req, Res>>,
}

impl<Req, Res> EngineSet<Req, Res>
where
    Req: Send + Sync,
    Res: Send,
{
    /// Build a set from the engines registered for a category.
    pub fn new(engines: Vec<DynEngine<Req, Res>>) -> Self {
        Self { engines }
    }

    /// Engine names registered for this category, in declaration order.
    pub fn names(&self) -> Vec<&'static str> {
        self.engines.iter().map(|engine| engine.name()).collect()
    }

    /// Resolve a [`Backend`] into the concrete engines to run, ordered by
    /// `(priority, random)`.
    pub fn select(&self, backend: &Backend) -> Vec<DynEngine<Req, Res>> {
        let chosen: Vec<DynEngine<Req, Res>> = match backend {
            Backend::Auto => self.engines.clone(),
            Backend::Engines(names) => {
                let mut chosen = Vec::new();
                for requested in names {
                    match self
                        .engines
                        .iter()
                        .find(|engine| engine.name().eq_ignore_ascii_case(requested))
                    {
                        Some(engine) => chosen.push(engine.clone()),
                        None => tracing::warn!(
                            engine = requested.as_str(),
                            "unknown backend engine; ignoring"
                        ),
                    }
                }
                if chosen.is_empty() {
                    tracing::warn!("no valid backend engines; falling back to auto");
                    self.engines.clone()
                } else {
                    chosen
                }
            }
        };

        order_by_priority(chosen)
    }
}

/// Tunables for one orchestrated search.
#[derive(Clone, Debug)]
pub struct OrchestrationOptions {
    /// Stop once this many (deduplicated) results are aggregated.
    pub max_results: usize,
    /// Per-engine wall-clock budget; an engine exceeding it is skipped.
    pub per_engine_timeout: Duration,
}

impl Default for OrchestrationOptions {
    fn default() -> Self {
        Self {
            max_results: 10,
            per_engine_timeout: Duration::from_secs(10),
        }
    }
}

/// Run the selected engines concurrently and aggregate their results.
///
/// Behavior (matching Python `ddgs`):
/// - engines run concurrently, each under [`OrchestrationOptions::per_engine_timeout`];
/// - an engine that errors or times out is logged and skipped (fallback);
/// - results are deduplicated via `dedup_key` (a `None` key is never deduped);
/// - aggregation stops once `max_results` is reached, and the result is
///   truncated to `max_results`.
///
/// `dedup_key` is a hook for the URL-normalizing dedup added in the ranking
/// phase; ordering/ranking of the kept set is applied by the caller afterwards.
pub async fn orchestrate<Req, Res, K>(
    http: &HttpClient,
    engines: Vec<DynEngine<Req, Res>>,
    request: Req,
    options: &OrchestrationOptions,
    dedup_key: K,
) -> Vec<Res>
where
    Req: Clone + Send + Sync + 'static,
    Res: Send + 'static,
    K: Fn(&Res) -> Option<String>,
{
    let mut tasks: JoinSet<(&'static str, Result<Vec<Res>>)> = JoinSet::new();
    for engine in engines {
        let http = http.clone();
        let request = request.clone();
        let timeout = options.per_engine_timeout;
        tasks.spawn(async move {
            let name = engine.name();
            let outcome = match tokio::time::timeout(timeout, engine.search(&http, &request)).await
            {
                Ok(result) => result,
                Err(_) => Err(WebSearchError::Timeout(format!(
                    "engine `{name}` timed out"
                ))),
            };
            (name, outcome)
        });
    }

    let mut aggregated: Vec<Res> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((name, Ok(results))) => {
                tracing::debug!(engine = name, count = results.len(), "engine returned");
                for item in results {
                    if let Some(key) = dedup_key(&item)
                        && !seen.insert(key)
                    {
                        continue;
                    }
                    aggregated.push(item);
                }
                if aggregated.len() >= options.max_results {
                    break;
                }
            }
            Ok((name, Err(error))) => {
                tracing::warn!(engine = name, error = %error, "engine failed; skipping");
            }
            Err(join_error) => {
                tracing::warn!(error = %join_error, "engine task panicked; skipping");
            }
        }
    }

    tasks.abort_all();
    aggregated.truncate(options.max_results);
    aggregated
}

/// Order engines by `(priority, random)`: priority ascending, ties broken with
/// a clock-seeded shuffle so load spreads across equal-priority engines.
fn order_by_priority<Req, Res>(engines: Vec<DynEngine<Req, Res>>) -> Vec<DynEngine<Req, Res>>
where
    Req: Send + Sync,
    Res: Send,
{
    let mut rng = SplitMix64::from_clock();
    let mut tagged: Vec<(u8, u64, DynEngine<Req, Res>)> = engines
        .into_iter()
        .map(|engine| (engine.priority(), rng.next_u64(), engine))
        .collect();
    tagged.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    tagged.into_iter().map(|(_, _, engine)| engine).collect()
}

/// Minimal SplitMix64 PRNG — enough to shuffle engine order without a `rand`
/// dependency (which would pull a separate RNG stack alongside BoringSSL's).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn from_clock() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct Req;

    #[derive(Clone, Debug, PartialEq)]
    struct Res {
        url: String,
    }

    /// A configurable mock engine: emits fixed results, or errors, or stalls.
    struct Mock {
        name: &'static str,
        priority: u8,
        behavior: Behavior,
    }

    #[derive(Clone)]
    enum Behavior {
        Results(Vec<&'static str>),
        Fail,
        Stall,
    }

    #[async_trait::async_trait]
    impl SearchEngine<Req, Res> for Mock {
        fn name(&self) -> &'static str {
            self.name
        }
        fn priority(&self) -> u8 {
            self.priority
        }
        async fn search(&self, _http: &HttpClient, _request: &Req) -> Result<Vec<Res>> {
            match &self.behavior {
                Behavior::Results(urls) => {
                    Ok(urls.iter().map(|u| Res { url: u.to_string() }).collect())
                }
                Behavior::Fail => Err(WebSearchError::Parse("boom".into())),
                Behavior::Stall => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok(vec![])
                }
            }
        }
    }

    fn engine(name: &'static str, priority: u8, behavior: Behavior) -> DynEngine<Req, Res> {
        Arc::new(Mock {
            name,
            priority,
            behavior,
        })
    }

    fn http() -> HttpClient {
        HttpClient::new(&crate::http::HttpConfig::default()).expect("client")
    }

    #[test]
    fn backend_parsing() {
        assert_eq!(Backend::parse(""), Backend::Auto);
        assert_eq!(Backend::parse("auto"), Backend::Auto);
        assert_eq!(Backend::parse("AUTO"), Backend::Auto);
        assert_eq!(
            Backend::parse("bing, Brave ,duckduckgo"),
            Backend::Engines(vec!["bing".into(), "brave".into(), "duckduckgo".into()])
        );
        assert_eq!(Backend::parse(" , "), Backend::Auto);
    }

    #[test]
    fn select_auto_orders_by_priority() {
        let set = EngineSet::new(vec![
            engine("c", 100, Behavior::Results(vec![])),
            engine("a", 10, Behavior::Results(vec![])),
            engine("b", 50, Behavior::Results(vec![])),
        ]);
        let names: Vec<_> = set
            .select(&Backend::Auto)
            .iter()
            .map(|e| e.name())
            .collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    #[test]
    fn select_named_filters_and_falls_back() {
        let set = EngineSet::new(vec![
            engine("bing", 100, Behavior::Results(vec![])),
            engine("brave", 100, Behavior::Results(vec![])),
        ]);
        let chosen = set.select(&Backend::Engines(vec!["brave".into(), "nope".into()]));
        assert_eq!(
            chosen.iter().map(|e| e.name()).collect::<Vec<_>>(),
            ["brave"]
        );

        // All-invalid → fall back to every engine.
        let fallback = set.select(&Backend::Engines(vec!["nope".into()]));
        assert_eq!(fallback.len(), 2);
    }

    #[tokio::test]
    async fn orchestrate_dedups_skips_errors_and_caps() {
        let engines = vec![
            engine("a", 100, Behavior::Results(vec!["u1", "u2"])),
            engine("b", 100, Behavior::Fail),
            engine("c", 100, Behavior::Results(vec!["u2", "u3", "u4"])),
        ];
        let options = OrchestrationOptions {
            max_results: 3,
            per_engine_timeout: Duration::from_secs(5),
        };
        let results = orchestrate(&http(), engines, Req, &options, |r| Some(r.url.clone())).await;

        // u2 deduped across a and c; failing engine b skipped; capped at 3.
        assert_eq!(results.len(), 3);
        let urls: HashSet<_> = results.into_iter().map(|r| r.url).collect();
        assert!(urls.is_subset(&HashSet::from([
            "u1".to_string(),
            "u2".to_string(),
            "u3".to_string(),
            "u4".to_string()
        ])));
    }

    #[tokio::test(start_paused = true)]
    async fn orchestrate_times_out_stalled_engine() {
        let engines = vec![
            engine("fast", 100, Behavior::Results(vec!["u1"])),
            engine("slow", 100, Behavior::Stall),
        ];
        let options = OrchestrationOptions {
            max_results: 10,
            per_engine_timeout: Duration::from_millis(50),
        };
        let results = orchestrate(&http(), engines, Req, &options, |r| Some(r.url.clone())).await;

        // The stalled engine is skipped after its timeout; the fast one survives.
        assert_eq!(results, vec![Res { url: "u1".into() }]);
    }
}
