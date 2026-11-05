//! Selection and orchestration tests with fake engines (no network).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::time::Instant;

use super::*;
use crate::TextResult;

/// What a fake engine does when searched.
#[derive(Clone)]
pub(crate) enum Behavior {
    /// Sleep, then return one text result per href.
    Results(u64, Vec<&'static str>),
    /// Sleep, then fail.
    Fail(u64),
    /// Sleep, then panic.
    Panic(u64),
}

/// Shared log of engine starts, plus a concurrency gauge.
#[derive(Default)]
pub(crate) struct Probe {
    starts: Mutex<Vec<(&'static str, Instant)>>,
    running: AtomicUsize,
    peak: AtomicUsize,
}

impl Probe {
    fn started(&self) -> Vec<&'static str> {
        self.starts
            .lock()
            .map(|starts| starts.iter().map(|(name, _)| *name).collect())
            .unwrap_or_default()
    }

    fn start_of(&self, name: &str) -> Option<Instant> {
        self.starts
            .lock()
            .ok()?
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, at)| *at)
    }
}

/// A fake engine.
pub(crate) struct Mock {
    pub(crate) name: &'static str,
    pub(crate) provider: &'static str,
    pub(crate) priority: f64,
    pub(crate) behavior: Behavior,
    pub(crate) probe: Arc<Probe>,
}

#[async_trait::async_trait]
impl<Req: Send + Sync> SearchEngine<Req, TextResult> for Mock {
    fn name(&self) -> &'static str {
        self.name
    }
    fn provider(&self) -> &'static str {
        self.provider
    }
    fn priority(&self) -> f64 {
        self.priority
    }
    async fn search(&self, _http: &HttpClient, _request: &Req) -> Result<Vec<TextResult>> {
        if let Ok(mut starts) = self.probe.starts.lock() {
            starts.push((self.name, Instant::now()));
        }
        let now = self.probe.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.probe.peak.fetch_max(now, Ordering::SeqCst);
        let (delay, outcome) = match &self.behavior {
            Behavior::Results(delay, hrefs) => (
                *delay,
                Ok(hrefs
                    .iter()
                    .map(|href| TextResult {
                        title: format!("{} {href}", self.name),
                        href: (*href).to_owned(),
                        body: String::new(),
                    })
                    .collect()),
            ),
            Behavior::Fail(delay) => (*delay, Err(WebSearchError::Http("boom".into()))),
            Behavior::Panic(delay) => (*delay, Ok(Vec::new())),
        };
        tokio::time::sleep(Duration::from_millis(delay)).await;
        self.probe.running.fetch_sub(1, Ordering::SeqCst);
        if matches!(self.behavior, Behavior::Panic(_)) {
            panic!("engine {} panicked", self.name);
        }
        outcome
    }
}

pub(crate) fn mock<Req: Send + Sync + 'static>(
    name: &'static str,
    provider: &'static str,
    behavior: Behavior,
    probe: &Arc<Probe>,
) -> DynEngine<Req, TextResult> {
    Arc::new(Mock {
        name,
        provider,
        priority: 1.0,
        behavior,
        probe: probe.clone(),
    })
}

fn with_priority(name: &'static str, priority: f64) -> DynEngine<(), TextResult> {
    Arc::new(Mock {
        name,
        provider: name,
        priority,
        behavior: Behavior::Results(0, vec![]),
        probe: Arc::default(),
    })
}

fn http() -> HttpClient {
    HttpClient::new(&crate::http::HttpConfig::default()).expect("client")
}

fn names<Req, Res>(engines: &[DynEngine<Req, Res>]) -> Vec<&'static str>
where
    Req: Send + Sync,
    Res: Send,
{
    engines.iter().map(|engine| engine.name()).collect()
}

async fn run(
    engines: Vec<DynEngine<(), TextResult>>,
    max_results: Option<usize>,
    timeout_ms: u64,
) -> Orchestrated<TextResult> {
    let client = http();
    let engines = engines.into_iter().map(|e| (e, client.clone())).collect();
    let options = OrchestrationOptions {
        max_results,
        timeout: Some(Duration::from_millis(timeout_ms)),
        threads: None,
    };
    orchestrate(engines, (), &options)
        .await
        .expect("orchestrate")
}

fn hrefs(outcome: Orchestrated<TextResult>) -> Vec<String> {
    outcome
        .aggregator
        .extract()
        .into_iter()
        .map(|r| r.href)
        .collect()
}

// ---- Backend parsing and selection -------------------------------------

#[test]
fn backend_parsing_matches_upstream() {
    assert_eq!(Backend::parse("auto"), Backend::Auto);
    assert_eq!(Backend::parse("all"), Backend::Auto);
    assert_eq!(Backend::parse("brave, auto"), Backend::Auto);
    assert_eq!(
        Backend::parse(" bing , Brave"),
        Backend::Engines(vec!["bing".into(), "Brave".into()])
    );
    // Case-sensitive, and empty items stay (they are invalid names).
    assert_eq!(
        Backend::parse("AUTO"),
        Backend::Engines(vec!["AUTO".into()])
    );
    assert_eq!(Backend::parse(""), Backend::Engines(vec!["".into()]));
    assert_eq!(
        Backend::parse("brave,,brave"),
        Backend::Engines(vec!["brave".into(), "".into(), "brave".into()])
    );
}

#[test]
fn text_auto_puts_wikipedia_and_grokipedia_first() {
    // Equal priorities: only the prepend puts them first.
    let set = EngineSet::new(
        "text",
        vec![
            with_priority("a", 1.0),
            with_priority("wikipedia", 1.0),
            with_priority("b", 1.0),
            with_priority("grokipedia", 1.0),
        ],
    );
    for _ in 0..20 {
        let chosen = names(&set.select(&Backend::Auto));
        assert_eq!(&chosen[..2], ["wikipedia", "grokipedia"]);
        assert_eq!(chosen.len(), 4);
    }
    // Other categories get no prepend.
    let set = EngineSet::new(
        "news",
        vec![with_priority("wikipedia", 1.0), with_priority("b", 1.0)],
    );
    let mut first = HashSet::new();
    for _ in 0..64 {
        first.insert(names(&set.select(&Backend::Auto))[0]);
    }
    assert_eq!(first.len(), 2, "auto order is shuffled");
}

#[test]
fn sorts_by_priority_descending_and_keeps_order_on_ties() {
    let set = EngineSet::new(
        "news",
        vec![
            with_priority("c", 1.0),
            with_priority("a", 2.0),
            with_priority("b", 1.9),
            with_priority("d", 1.0),
        ],
    );
    let chosen = set.select(&Backend::Engines(vec![
        "d".into(),
        "c".into(),
        "b".into(),
        "a".into(),
    ]));
    // Descending priority; `d` before `c` as given (stable sort, upstream's
    // `(priority, random)` key compares the `random` function as equal).
    assert_eq!(names(&chosen), ["a", "b", "d", "c"]);
}

#[test]
fn named_selection_is_exact_keeps_duplicates_and_falls_back() {
    let set = EngineSet::new(
        "text",
        vec![with_priority("bing", 1.0), with_priority("brave", 1.0)],
    );
    assert_eq!(
        names(&set.select(&Backend::Engines(vec!["brave".into(), "nope".into()]))),
        ["brave"]
    );
    assert_eq!(
        names(&set.select(&Backend::Engines(vec!["brave".into(), "brave".into()]))),
        ["brave", "brave"]
    );
    // Case-sensitive: "Brave" is not an engine → auto.
    assert_eq!(set.select(&Backend::Engines(vec!["Brave".into()])).len(), 2);
    assert_eq!(set.select(&Backend::parse("")).len(), 2);
}

#[test]
fn registry_names_providers_and_priorities_match_upstream() {
    let registry = EngineRegistry::new();
    let table = |names: Vec<(&'static str, &'static str, f64)>| {
        let mut names = names;
        names.sort_by(|a, b| a.0.cmp(b.0));
        names
    };
    let text = table(
        registry
            .text()
            .engines
            .iter()
            .map(|e| (e.name(), e.provider(), e.priority()))
            .collect(),
    );
    assert_eq!(
        text,
        [
            ("brave", "brave", 1.0),
            ("duckduckgo", "bing", 1.0),
            ("grokipedia", "grokipedia", 1.9),
            ("mojeek", "mojeek", 1.0),
            ("startpage", "google", 1.0),
            ("wikipedia", "wikipedia", 2.0),
            ("yahoo", "bing", 1.0),
        ]
    );
    let news = table(
        registry
            .news()
            .engines
            .iter()
            .map(|e| (e.name(), e.provider(), e.priority()))
            .collect(),
    );
    assert_eq!(
        news,
        [
            ("bing", "bing", 1.0),
            ("duckduckgo", "bing", 1.0),
            ("yahoo", "yahoo", 1.0)
        ]
    );
    let images = table(
        registry
            .images()
            .engines
            .iter()
            .map(|e| (e.name(), e.provider(), e.priority()))
            .collect(),
    );
    assert_eq!(images, [("bing", "bing", 1.0), ("duckduckgo", "bing", 1.0)]);
    let videos: Vec<_> = registry
        .videos()
        .engines
        .iter()
        .map(|e| (e.name(), e.provider()))
        .collect();
    assert_eq!(videos, [("duckduckgo", "bing")]);
    let books: Vec<_> = registry
        .books()
        .engines
        .iter()
        .map(|e| (e.name(), e.provider()))
        .collect();
    assert_eq!(books, [("annasarchive", "annasarchive")]);
    // Text auto: wikipedia (priority 2) then grokipedia (1.9) lead.
    let chosen = registry.text().select(&Backend::Auto);
    assert_eq!(&names(&chosen)[..2], ["wikipedia", "grokipedia"]);
}

#[test]
fn disabled_text_engines_are_not_registered() {
    let registry = EngineRegistry::new();
    let names = registry.text().names();
    assert!(!names.contains(&"bing"));
    assert!(!names.contains(&"yandex"));
    // bing news / images stay enabled.
    assert!(registry.news().names().contains(&"bing"));
    assert!(registry.images().names().contains(&"bing"));
    for disabled in ["bing", "yandex"] {
        let chosen = registry
            .text()
            .select(&Backend::Engines(vec![disabled.into()]));
        assert_eq!(chosen.len(), names.len());
        assert!(chosen.iter().all(|e| e.name() != disabled));
    }
}

// ---- max_workers ----------------------------------------------------------

#[test]
fn max_workers_matches_upstream_formula() {
    assert_eq!(max_workers(7, Some(10), None), 2);
    assert_eq!(max_workers(7, Some(1), None), 2);
    assert_eq!(max_workers(7, Some(11), None), 3);
    assert_eq!(max_workers(7, Some(25), None), 4);
    assert_eq!(max_workers(2, Some(100), None), 2);
    assert_eq!(max_workers(7, None, None), 7);
    assert_eq!(max_workers(7, Some(0), None), 7);
    assert_eq!(max_workers(7, None, Some(3)), 3);
    assert_eq!(max_workers(7, Some(10), Some(5)), 2);
    assert_eq!(max_workers(7, None, Some(0)), 7);
}

// ---- Orchestration --------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn provider_seen_after_results_skips_later_engines() {
    let probe = Arc::new(Probe::default());
    let outcome = run(
        vec![
            mock("a", "x", Behavior::Results(10, vec!["u1"]), &probe),
            mock("b", "x", Behavior::Results(10, vec!["u2"]), &probe),
        ],
        None,
        5_000,
    )
    .await;
    // One provider → one worker; `a` returns results, so `b` is skipped.
    assert_eq!(probe.started(), ["a"]);
    assert_eq!(hrefs(outcome), ["u1"]);
}

#[tokio::test(start_paused = true)]
async fn empty_or_failed_engines_do_not_mark_their_provider() {
    let probe = Arc::new(Probe::default());
    let outcome = run(
        vec![
            mock("a", "x", Behavior::Results(10, vec![]), &probe),
            mock("b", "x", Behavior::Fail(10), &probe),
            mock("c", "x", Behavior::Results(10, vec!["u"]), &probe),
            mock("d", "x", Behavior::Results(10, vec!["v"]), &probe),
        ],
        None,
        5_000,
    )
    .await;
    assert_eq!(probe.started(), ["a", "b", "c"]);
    assert!(matches!(outcome.last_error, Some(WebSearchError::Http(_))));
    assert_eq!(hrefs(outcome), ["u"]);
}

#[tokio::test(start_paused = true)]
async fn first_batch_runs_together_then_one_at_a_time() {
    // 5 providers, max_results 10 → max_workers = min(5, 2) = 2. The first
    // `wait` happens at i == 2 with two futures; from i == 3 on, `i >=
    // max_workers` forces a `wait` after every single submission.
    let probe = Arc::new(Probe::default());
    let engines = ["e1", "e2", "e3", "e4", "e5"]
        .into_iter()
        .zip(["p1", "p2", "p3", "p4", "p5"])
        .zip(["u1", "u2", "u3", "u4", "u5"])
        .map(|((name, provider), href)| {
            mock(name, provider, Behavior::Results(100, vec![href]), &probe)
        })
        .collect();
    let started = Instant::now();
    let outcome = run(engines, Some(10), 5_000).await;
    assert_eq!(started.elapsed(), Duration::from_millis(400));
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2);
    assert_eq!(hrefs(outcome), ["u1", "u2", "u3", "u4", "u5"]);
}

#[tokio::test(start_paused = true)]
async fn stops_submitting_once_max_results_is_reached() {
    let probe = Arc::new(Probe::default());
    let outcome = run(
        vec![
            mock("a", "pa", Behavior::Results(10, vec!["u1", "u2"]), &probe),
            mock("b", "pb", Behavior::Results(10, vec!["u3"]), &probe),
            mock("c", "pc", Behavior::Results(10, vec!["u4"]), &probe),
        ],
        Some(2),
        5_000,
    )
    .await;
    // max_workers = min(3, ceil(2/10)+1) = 2: a and b run, then the
    // aggregator holds 3 >= 2 and the loop breaks before c.
    assert_eq!(probe.started(), ["a", "b"]);
    // The aggregator is not truncated here (the caller slices).
    assert_eq!(hrefs(outcome), ["u1", "u2", "u3"]);
}

#[tokio::test(start_paused = true)]
async fn timed_out_engine_is_collected_by_the_sentinel_wait() {
    // 3 providers, max_results None → 3 workers; one wait at i == 3 times out
    // at 5 s with the slow engine pending; the sentinel wait collects it.
    let probe = Arc::new(Probe::default());
    let started = Instant::now();
    let outcome = run(
        vec![
            mock("slow", "p1", Behavior::Results(8_000, vec!["s"]), &probe),
            mock("f1", "p2", Behavior::Results(1_000, vec!["a"]), &probe),
            mock("f2", "p3", Behavior::Results(1_000, vec!["b"]), &probe),
        ],
        None,
        5_000,
    )
    .await;
    assert_eq!(started.elapsed(), Duration::from_millis(8_000));
    assert_eq!(hrefs(outcome), ["a", "b", "s"]);
}

#[tokio::test(start_paused = true)]
async fn engine_slower_than_two_waits_is_dropped_but_awaited() {
    let probe = Arc::new(Probe::default());
    let started = Instant::now();
    let outcome = run(
        vec![
            mock("slow", "p1", Behavior::Results(12_000, vec!["s"]), &probe),
            mock("fast", "p2", Behavior::Results(1_000, vec!["a"]), &probe),
        ],
        None,
        5_000,
    )
    .await;
    // Waits end at 5 s and 10 s; `shutdown(wait=True)` still waits for the
    // slow engine (12 s) and discards its results.
    assert_eq!(started.elapsed(), Duration::from_millis(12_000));
    assert_eq!(hrefs(outcome), ["a"]);
}

#[tokio::test(start_paused = true)]
async fn first_exception_ends_the_wait_early() {
    let probe = Arc::new(Probe::default());
    let started = Instant::now();
    let outcome = run(
        vec![
            mock("fail", "p1", Behavior::Fail(1_000), &probe),
            mock("slow", "p2", Behavior::Results(3_000, vec!["s"]), &probe),
            mock("late", "p3", Behavior::Results(0, vec!["l"]), &probe),
        ],
        Some(10),
        60_000,
    )
    .await;
    // max_workers = 2. The i == 2 wait returns at 1 s on the failure, so
    // `late` is submitted at 1 s (not at 3 s) and its wait (i == 3) holds
    // `slow` and `late`.
    let late = probe.start_of("late").expect("late started");
    assert_eq!(late - started, Duration::from_millis(1_000));
    assert!(outcome.last_error.is_some());
    assert_eq!(hrefs(outcome), ["s", "l"]);
}

#[tokio::test(start_paused = true)]
async fn panicking_engine_counts_as_an_error() {
    let probe = Arc::new(Probe::default());
    let outcome = run(
        vec![
            mock("boom", "p1", Behavior::Panic(10), &probe),
            mock("ok", "p2", Behavior::Results(20, vec!["u"]), &probe),
        ],
        None,
        5_000,
    )
    .await;
    assert!(matches!(
        outcome.last_error,
        Some(WebSearchError::Internal(_))
    ));
    assert_eq!(hrefs(outcome), ["u"]);
}

#[tokio::test(start_paused = true)]
async fn done_engines_are_collected_in_submission_order() {
    let probe = Arc::new(Probe::default());
    let outcome = run(
        vec![
            mock("first", "p1", Behavior::Results(300, vec!["a"]), &probe),
            mock("second", "p2", Behavior::Results(100, vec!["b"]), &probe),
        ],
        None,
        5_000,
    )
    .await;
    // `second` finished first, but `futures.items()` is in submission order.
    assert_eq!(hrefs(outcome), ["a", "b"]);
}

#[tokio::test(start_paused = true)]
async fn queued_engines_start_when_a_worker_frees() {
    // 2 workers, 1 s waits. a and b (5 s) are still running when c is
    // submitted, so c queues until a worker frees at 5 s.
    let probe = Arc::new(Probe::default());
    let started = Instant::now();
    let outcome = run(
        vec![
            mock("a", "p1", Behavior::Results(5_000, vec!["a"]), &probe),
            mock("b", "p2", Behavior::Results(5_000, vec!["b"]), &probe),
            mock("c", "p3", Behavior::Results(100, vec!["c"]), &probe),
        ],
        Some(10),
        1_000,
    )
    .await;
    let c = probe.start_of("c").expect("c started");
    assert_eq!(c - started, Duration::from_millis(5_000));
    assert_eq!(probe.peak.load(Ordering::SeqCst), 2);
    // Waits end at 1 s, 2 s, then 3 s (sentinel): nothing collected; the
    // executor shutdown still runs everything.
    assert_eq!(started.elapsed(), Duration::from_millis(5_100));
    assert!(outcome.aggregator.is_empty());
}

#[tokio::test(start_paused = true)]
async fn threads_caps_concurrency() {
    let probe = Arc::new(Probe::default());
    let client = http();
    let engines = vec![
        (
            mock("a", "p1", Behavior::Results(100, vec!["a"]), &probe),
            client.clone(),
        ),
        (
            mock("b", "p2", Behavior::Results(100, vec!["b"]), &probe),
            client.clone(),
        ),
        (
            mock("c", "p3", Behavior::Results(100, vec!["c"]), &probe),
            client,
        ),
    ];
    let options = OrchestrationOptions {
        max_results: None,
        timeout: Some(Duration::from_secs(5)),
        threads: Some(1),
    };
    orchestrate(engines, (), &options)
        .await
        .expect("orchestrate");
    assert_eq!(probe.peak.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn dropping_a_search_aborts_its_engine_tasks() {
    let probe = Arc::new(Probe::default());
    let client = http();
    let engines = vec![
        (
            mock("slow", "p1", Behavior::Results(60_000, vec!["s"]), &probe),
            client.clone(),
        ),
        (
            mock("slow2", "p2", Behavior::Results(60_000, vec!["t"]), &probe),
            client,
        ),
    ];
    let options = OrchestrationOptions {
        max_results: None,
        timeout: None,
        threads: None,
    };
    let search = orchestrate(engines, (), &options);
    assert!(
        tokio::time::timeout(Duration::from_secs(1), search)
            .await
            .is_err()
    );
    // Both engines started and are still "running" by the gauge; once the
    // tasks are aborted no one decrements it, and the clock can jump past
    // their sleep without any task finishing.
    assert_eq!(probe.running.load(Ordering::SeqCst), 2);
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert_eq!(probe.running.load(Ordering::SeqCst), 2);
}
