//! Engine abstraction and orchestration — the metasearch core.
//!
//! Port of Python `ddgs` v9.16.1 `base.py` (`BaseSearchEngine`),
//! `engines/__init__.py` (`ENGINES`) and `ddgs.py` (`DDGS._get_engines`,
//! `DDGS._search_sync`). One engine = one struct implementing [`SearchEngine`]
//! for a category; [`EngineSet::select`] picks and orders engines for a
//! `backend`; [`orchestrate`] runs them the way upstream's `ThreadPoolExecutor`
//! loop does and feeds a [`ResultsAggregator`].
//!
//! Concrete engines live under `engines/<category>/` and are registered in
//! [`EngineRegistry`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::http::HttpClient;
use crate::ranking::{Rankable, ResultsAggregator};
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
/// client; they receive their own [`HttpClient`] (one per engine, kept for the
/// life of the [`crate::WebSearchClient`], as upstream caches one engine
/// instance per `DDGS`).
#[async_trait::async_trait]
pub trait SearchEngine<Req, Res>: Send + Sync
where
    Req: Send + Sync,
    Res: Send,
{
    /// Engine key used in `backend` selection and logs; the upstream `name`
    /// (the key in upstream `ENGINES[category]`).
    fn name(&self) -> &'static str;

    /// Source of the results (upstream `provider`, e.g. `"bing"` for the
    /// DuckDuckGo engines). Once an engine of a provider has returned results,
    /// later engines of the same provider are skipped.
    fn provider(&self) -> &'static str;

    /// Upstream `priority` (default `1`); engines run in descending priority.
    fn priority(&self) -> f64 {
        1.0
    }

    /// One upstream `search` call: one request (plus any token/bootstrap
    /// request the engine needs). A non-200 status gives no results (`Ok`
    /// with an empty list), a transport failure an error.
    async fn search(&self, http: &HttpClient, request: &Req) -> Result<Vec<Res>>;
}

/// Convenience aliases for boxed per-category engines.
pub type DynEngine<Req, Res> = Arc<dyn SearchEngine<Req, Res>>;

/// Backend selection, parsed from the `backend` parameter.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Backend {
    /// Every engine registered for the category (`"auto"` or `"all"`).
    #[default]
    Auto,
    /// The named engines, in the given order. Names are matched exactly
    /// (case-sensitive); unknown names are warned about and skipped, and if
    /// none is valid the selection falls back to [`Backend::Auto`].
    Engines(Vec<String>),
}

impl Backend {
    /// Parse a `backend` string as upstream does: split on `,`, strip each
    /// item; if any item is `"auto"` or `"all"` the result is
    /// [`Backend::Auto`], otherwise the list as is (empty items and
    /// duplicates included).
    pub fn parse(value: &str) -> Self {
        let items: Vec<String> = value
            .split(',')
            .map(|item| item.trim().to_owned())
            .collect();
        if items.iter().any(|item| item == "auto" || item == "all") {
            Self::Auto
        } else {
            Self::Engines(items)
        }
    }
}

/// An ordered set of engines for one category, plus selection logic.
pub struct EngineSet<Req, Res> {
    category: &'static str,
    engines: Vec<DynEngine<Req, Res>>,
}

impl<Req, Res> EngineSet<Req, Res>
where
    Req: Send + Sync,
    Res: Send,
{
    /// Build a set from the engines registered for `category` (`"text"`,
    /// `"news"`, `"images"`, `"videos"`, `"books"`).
    pub fn new(category: &'static str, engines: Vec<DynEngine<Req, Res>>) -> Self {
        Self { category, engines }
    }

    /// The category this set serves.
    pub fn category(&self) -> &'static str {
        self.category
    }

    /// Engine names registered for this category, in declaration order.
    pub fn names(&self) -> Vec<&'static str> {
        self.engines.iter().map(|engine| engine.name()).collect()
    }

    /// Port of upstream `DDGS._get_engines`.
    ///
    /// 1. The registered names are shuffled.
    /// 2. [`Backend::Auto`] takes them all; for `text`, `wikipedia` and
    ///    `grokipedia` come first. A named list is taken as given.
    /// 3. Unknown names are warned about; with no valid name the selection
    ///    is redone as `auto`.
    /// 4. The engines are sorted by priority, descending.
    ///
    /// Upstream sorts with `key=lambda e: (e.priority, random)` — the
    /// `random` *function*, not `random()` — so the "random" tie-break compares
    /// equal and the stable sort keeps the shuffled order among equal
    /// priorities. A stable sort reproduces that.
    pub fn select(&self, backend: &Backend) -> Vec<DynEngine<Req, Res>> {
        let mut engine_keys = self.names();
        shuffle(&mut engine_keys, &mut SplitMix64::from_clock());

        let keys: Vec<String> = match backend {
            Backend::Auto => {
                let mut keys: Vec<String> = Vec::new();
                if self.category == "text" {
                    keys.push("wikipedia".to_owned());
                    keys.push("grokipedia".to_owned());
                }
                keys.extend(
                    engine_keys
                        .iter()
                        .filter(|key| !keys.iter().any(|k| k == *key))
                        .map(|key| (*key).to_owned())
                        .collect::<Vec<_>>(),
                );
                keys
            }
            Backend::Engines(names) => names.clone(),
        };

        let mut chosen: Vec<DynEngine<Req, Res>> = Vec::new();
        let mut invalid: Vec<&str> = Vec::new();
        for key in &keys {
            match self.engines.iter().find(|engine| engine.name() == key) {
                Some(engine) => chosen.push(engine.clone()),
                None => invalid.push(key),
            }
        }

        if !invalid.is_empty() {
            invalid.sort_unstable();
            let mut available = engine_keys.clone();
            available.sort_unstable();
            // Upstream `logger = logging.getLogger("ddgs.ddgs")`.
            tracing::warn!(
                target: "ddgs.ddgs",
                "{} - backends do not exist or are disabled. Available: {}",
                invalid.join(", "),
                available.join(", ")
            );
        }

        if chosen.is_empty() {
            tracing::warn!(target: "ddgs.ddgs", "backend is not set. Using 'auto'");
            if *backend == Backend::Auto {
                // Unreachable with a non-empty registry; avoids unbounded
                // recursion on an empty one.
                return chosen;
            }
            return self.select(&Backend::Auto);
        }

        // Stable sort, descending priority (see above).
        chosen.sort_by(|a, b| b.priority().total_cmp(&a.priority()));
        chosen
    }
}

/// Tunables for one orchestrated search.
#[derive(Clone, Debug)]
pub struct OrchestrationOptions {
    /// Stop once this many distinct results are aggregated; `None` or `0`
    /// means no limit (upstream `max_results` falsy).
    pub max_results: Option<usize>,
    /// How long each batch `wait` blocks (upstream `DDGS._timeout`); `None`
    /// waits until the batch completes.
    pub timeout: Option<Duration>,
    /// Upper bound on concurrent engines (upstream `DDGS.threads`); `None` or
    /// `0` means automatic.
    pub threads: Option<usize>,
}

impl Default for OrchestrationOptions {
    fn default() -> Self {
        Self {
            max_results: Some(10),
            timeout: Some(Duration::from_secs(5)),
            threads: None,
        }
    }
}

/// What a search produced before ranking.
#[derive(Debug)]
pub struct Orchestrated<Res> {
    /// Every aggregated result.
    pub aggregator: ResultsAggregator<Res>,
    /// The last engine error seen (upstream `err`).
    pub last_error: Option<WebSearchError>,
}

/// Upstream `max_workers`:
/// `min(unique_providers, ceil(max_results / 10) + 1)` when `max_results` is
/// truthy, else `unique_providers`; then capped by `DDGS.threads` if set.
pub fn max_workers(
    unique_providers: usize,
    max_results: Option<usize>,
    threads: Option<usize>,
) -> usize {
    let mut workers = match max_results.filter(|&n| n > 0) {
        Some(n) => unique_providers.min(n.div_ceil(10) + 1),
        None => unique_providers,
    };
    if let Some(threads) = threads.filter(|&t| t > 0) {
        workers = workers.min(threads);
    }
    workers
}

/// Port of the engine loop in upstream `DDGS._search_sync`.
///
/// Upstream submits engines to a `ThreadPoolExecutor(max_workers)`:
///
/// ```python
/// for i, engine in enumerate([*engines, None], start=1):
///     if engine:
///         if engine.provider in seen_providers:
///             continue
///         futures[executor.submit(engine.search, ...)] = engine
///     if len(futures) >= max_workers or i >= max_workers:
///         done, not_done = wait(futures, timeout=self._timeout, return_when="FIRST_EXCEPTION")
///         for f, f_engine in futures.items():
///             if f in done:
///                 try:
///                     if r := f.result():
///                         results_aggregator.extend(r)
///                         seen_providers.add(f_engine.provider)
///                 except Exception as ex:
///                     err = ex
///         futures = {f: futures[f] for f in not_done}
///     if max_results and len(results_aggregator) >= max_results:
///         break
/// ```
///
/// Reproduced with tokio: at most `max_workers` engines run at once and the
/// rest queue FIFO; each `wait` returns when every pending engine finished,
/// when one failed (`FIRST_EXCEPTION`), or after `timeout`, and engines still
/// running stay pending for the next `wait` (the trailing `None` sentinel
/// gives one last `wait` after the final engine). A provider is marked seen
/// only after one of its engines returned results. Leaving the `with` block
/// runs `executor.shutdown(wait=True)`, so this function also waits for every
/// submitted engine — queued ones included — to finish before returning, and
/// discards their results.
///
/// Unavoidable difference: after a `wait` that times out, upstream rebuilds
/// `futures` by iterating the `not_done` *set*, so the order in which
/// still-pending engines are collected later is arbitrary; here it stays
/// submission order (one of upstream's possible orders).
pub async fn orchestrate<Req, Res>(
    engines: Vec<(DynEngine<Req, Res>, HttpClient)>,
    request: Req,
    options: &OrchestrationOptions,
) -> Result<Orchestrated<Res>>
where
    Req: Send + Sync + 'static,
    Res: Rankable + Send + 'static,
{
    let unique_providers = engines
        .iter()
        .map(|(engine, _)| engine.provider())
        .collect::<HashSet<_>>()
        .len();
    let max_results = options.max_results.filter(|&n| n > 0);
    let workers = max_workers(unique_providers, max_results, options.threads);
    if workers == 0 {
        // `ThreadPoolExecutor(max_workers=0)` raises.
        return Err(WebSearchError::Internal(
            "max_workers must be greater than 0".into(),
        ));
    }

    let request = Arc::new(request);
    let mut executor: Executor<Res> = Executor::new(workers);
    let mut aggregator = ResultsAggregator::new();
    let mut seen_providers: HashSet<&'static str> = HashSet::new();
    let mut last_error: Option<WebSearchError> = None;
    // Pending futures in `futures` dict order: (task id, engine).
    let mut pending: Vec<(usize, DynEngine<Req, Res>)> = Vec::new();

    let total = engines.len();
    let mut engines = engines.into_iter();
    for i in 1..=total + 1 {
        // `None` sentinel on the last iteration.
        if let Some((engine, http)) = engines.next() {
            if seen_providers.contains(engine.provider()) {
                continue;
            }
            let task_engine = engine.clone();
            let task_request = request.clone();
            let id = executor.submit(async move { task_engine.search(&http, &task_request).await });
            pending.push((id, engine));
        }

        if pending.len() >= workers || i >= workers {
            let ids: Vec<usize> = pending.iter().map(|(id, _)| *id).collect();
            let done = executor.wait(&ids, options.timeout).await;
            for (id, engine) in &pending {
                if !done.contains(id) {
                    continue;
                }
                let outcome = match executor.result(*id).await {
                    Ok(results) if !results.is_empty() => aggregator.extend(results),
                    Ok(_) => continue,
                    Err(error) => Err(error),
                };
                match outcome {
                    Ok(()) => {
                        seen_providers.insert(engine.provider());
                    }
                    Err(error) => {
                        tracing::info!(target: "ddgs.ddgs", "Error in engine {}: {:?}", engine.name(), error);
                        last_error = Some(error);
                    }
                }
            }
            pending.retain(|(id, _)| !done.contains(id));
        }

        if let Some(max_results) = max_results
            && aggregator.len() >= max_results
        {
            break;
        }
    }

    // `with ThreadPoolExecutor(...)` exit: `shutdown(wait=True)`.
    executor.shutdown().await;

    Ok(Orchestrated {
        aggregator,
        last_error,
    })
}

type EngineTask<Res> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<Res>>> + Send>>;

/// Sends a task's completion (and whether it failed) when dropped — also on
/// panic, which then counts as a failure.
struct DoneGuard {
    tx: mpsc::UnboundedSender<(usize, bool)>,
    id: usize,
    failed: bool,
}

impl Drop for DoneGuard {
    fn drop(&mut self) {
        let _ = self.tx.send((self.id, self.failed));
    }
}

/// A `ThreadPoolExecutor` stand-in: at most `max_workers` tasks run; the rest
/// wait in a FIFO queue and start as running tasks finish.
struct Executor<Res> {
    max_workers: usize,
    running: usize,
    next_id: usize,
    queue: VecDeque<(usize, EngineTask<Res>)>,
    handles: HashMap<usize, JoinHandle<Result<Vec<Res>>>>,
    /// Finished task ids → whether the task failed.
    finished: HashMap<usize, bool>,
    tx: mpsc::UnboundedSender<(usize, bool)>,
    rx: mpsc::UnboundedReceiver<(usize, bool)>,
}

impl<Res: Send + 'static> Executor<Res> {
    fn new(max_workers: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            max_workers,
            running: 0,
            next_id: 0,
            queue: VecDeque::new(),
            handles: HashMap::new(),
            finished: HashMap::new(),
            tx,
            rx,
        }
    }

    /// `executor.submit`: start now if a worker is free, else queue.
    fn submit<F>(&mut self, task: F) -> usize
    where
        F: std::future::Future<Output = Result<Vec<Res>>> + Send + 'static,
    {
        self.drain();
        let id = self.next_id;
        self.next_id += 1;
        self.queue.push_back((id, Box::pin(task)));
        self.fill();
        id
    }

    fn fill(&mut self) {
        while self.running < self.max_workers {
            let Some((id, task)) = self.queue.pop_front() else {
                break;
            };
            self.running += 1;
            let tx = self.tx.clone();
            let handle = tokio::spawn(async move {
                let mut guard = DoneGuard {
                    tx,
                    id,
                    failed: true,
                };
                let result = task.await;
                guard.failed = result.is_err();
                result
            });
            self.handles.insert(id, handle);
        }
    }

    fn complete(&mut self, id: usize, failed: bool) {
        self.running = self.running.saturating_sub(1);
        self.finished.insert(id, failed);
        self.fill();
    }

    /// Record completions that already happened, without blocking.
    fn drain(&mut self) {
        while let Ok((id, failed)) = self.rx.try_recv() {
            self.complete(id, failed);
        }
    }

    /// `concurrent.futures.wait(ids, timeout, return_when=FIRST_EXCEPTION)`:
    /// the ids that are done when every one is done, one has failed, or the
    /// timeout expires.
    async fn wait(&mut self, ids: &[usize], timeout: Option<Duration>) -> HashSet<usize> {
        let deadline = timeout.map(|timeout| tokio::time::Instant::now() + timeout);
        loop {
            self.drain();
            let done: HashSet<usize> = ids
                .iter()
                .copied()
                .filter(|id| self.finished.contains_key(id))
                .collect();
            let any_failed = done.iter().any(|id| self.finished.get(id) == Some(&true));
            if done.len() == ids.len() || any_failed {
                return done;
            }
            let message = match deadline {
                Some(deadline) => match tokio::time::timeout_at(deadline, self.rx.recv()).await {
                    Ok(message) => message,
                    Err(_) => {
                        self.drain();
                        return ids
                            .iter()
                            .copied()
                            .filter(|id| self.finished.contains_key(id))
                            .collect();
                    }
                },
                None => self.rx.recv().await,
            };
            match message {
                Some((id, failed)) => self.complete(id, failed),
                // Unreachable: the executor keeps a sender alive.
                None => return done,
            }
        }
    }

    /// `future.result()` of a finished task.
    async fn result(&mut self, id: usize) -> Result<Vec<Res>> {
        match self.handles.remove(&id) {
            Some(handle) => match handle.await {
                Ok(result) => result,
                Err(error) => Err(WebSearchError::Internal(error.to_string())),
            },
            None => Err(WebSearchError::Internal(format!("unknown task {id}"))),
        }
    }

    /// `executor.shutdown(wait=True)`: run the queue dry and wait for every
    /// task.
    async fn shutdown(&mut self) {
        loop {
            self.drain();
            if self.running == 0 && self.queue.is_empty() {
                break;
            }
            match self.rx.recv().await {
                Some((id, failed)) => self.complete(id, failed),
                None => break,
            }
        }
    }
}

/// A search future dropped mid-flight (e.g. a caller-side timeout) never
/// reaches `shutdown`; abort its leftover engine tasks instead of leaving them
/// detached. On the normal path every task has already finished, so this
/// does nothing.
impl<Res> Drop for Executor<Res> {
    fn drop(&mut self) {
        self.queue.clear();
        for (_, handle) in self.handles.drain() {
            handle.abort();
        }
    }
}

/// Fisher–Yates shuffle (upstream `random.shuffle`).
pub(crate) fn shuffle<T>(items: &mut [T], rng: &mut SplitMix64) {
    for i in (1..items.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// Minimal SplitMix64 PRNG — enough to shuffle engines and pick a mirror
/// without a `rand` dependency (which would pull a separate RNG stack alongside
/// BoringSSL's).
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn from_clock() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self {
            state: seed ^ 0x9E37_79B9_7F4A_7C15,
        }
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
pub(crate) mod tests;
