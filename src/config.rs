use std::time::Duration;

use crate::http::Verify;

/// Crate version, used to build the public `websearch-rs/x.y.z` identifier.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runtime configuration for [`crate::WebSearchClient`].
///
/// Mirrors the `DDGS(proxy=None, timeout=5, *, verify=True)` constructor and
/// the `DDGS.threads` class attribute of Python `ddgs` v9.16.1. The HTTP
/// transport ([`crate::http::HttpClient`], built on `wreq`) supplies a random
/// browser User-Agent and fingerprint per engine, so there is no user-agent
/// knob here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSearchConfig {
    /// Per-request HTTP timeout, and the time the orchestrator waits for each
    /// batch of engines. `None` waits forever. Default 5 s.
    pub timeout: Option<Duration>,
    /// HTTP/HTTPS/SOCKS proxy URL actually used: the builder's proxy with the
    /// `"tb"` alias expanded, else the `DDGS_PROXY` environment variable.
    pub proxy: Option<String>,
    /// TLS certificate verification. Default: verify.
    pub verify: Verify,
    /// Upper bound on concurrent engines per search (upstream `DDGS.threads`).
    /// `None` (or `0`) means automatic, based on `max_results`.
    pub threads: Option<usize>,
    /// Rust-only: retries for transient network failures. Upstream never
    /// retries, so the default is `0`.
    pub max_retries: usize,
    /// Rust-only: initial retry delay; later retries use linear backoff.
    pub retry_delay: Duration,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(5)),
            proxy: None,
            verify: Verify::Enabled,
            threads: None,
            max_retries: 0,
            retry_delay: Duration::from_millis(500),
        }
    }
}
