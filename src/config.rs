use std::time::Duration;

/// Crate version, used to build the public `websearch-rs/x.y.z` identifier.
pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Runtime configuration for [`crate::WebSearchClient`].
///
/// The HTTP transport ([`crate::http::HttpClient`], built on `wreq`) supplies a
/// random browser User-Agent and fingerprint per client, so there is no
/// user-agent knob here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSearchConfig {
    /// Per-request timeout (also the per-engine budget in the orchestrator).
    pub timeout: Duration,
    /// Optional HTTP/HTTPS/SOCKS proxy URL.
    pub proxy: Option<String>,
    /// Maximum retries for transient network failures.
    pub max_retries: usize,
    /// Initial retry delay; later retries use linear backoff.
    pub retry_delay: Duration,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            proxy: None,
            max_retries: 2,
            retry_delay: Duration::from_millis(500),
        }
    }
}
