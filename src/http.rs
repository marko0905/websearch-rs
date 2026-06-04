//! The single HTTP choke point for all engines.
//!
//! Every engine request goes through [`HttpClient`], which is built on
//! [`wreq`] + BoringSSL so requests reproduce a real browser's JA3/JA4 TLS and
//! HTTP/2 fingerprints — not just headers. Plain TLS stacks (e.g. rustls) are
//! fingerprint-detected and soft-blocked by Bing/Brave/Yandex regardless of the
//! `User-Agent`; impersonation is what makes parity with Python `ddgs`
//! (which uses `primp`) achievable.
//!
//! Construction picks a random browser + OS emulation profile per client, the
//! parity equivalent of `primp`'s `impersonate="random", impersonate_os="random"`.
//! Engines must never build their own client; route new requests through here so
//! retries, proxy, and timeout stay centralized.

use std::collections::HashMap;
use std::time::Duration;

use wreq::redirect::Policy;
use wreq::{Client, Method};
use wreq_util::Emulation;

use crate::{Result, WebSearchError};

/// Transport configuration for [`HttpClient`].
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// Per-request timeout.
    pub timeout: Duration,
    /// Optional HTTP/HTTPS/SOCKS proxy URL.
    pub proxy: Option<String>,
    /// Maximum retries for transient failures (timeouts, connects, 429, 5xx).
    pub max_retries: usize,
    /// Initial retry delay; later retries use linear backoff.
    pub retry_delay: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            proxy: None,
            max_retries: 2,
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// A reusable, browser-impersonating HTTP client shared by all engines.
#[derive(Clone)]
pub struct HttpClient {
    inner: Client,
    max_retries: usize,
    retry_delay: Duration,
}

impl std::fmt::Debug for HttpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpClient")
            .field("max_retries", &self.max_retries)
            .field("retry_delay", &self.retry_delay)
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    /// Build a client with a random browser/OS emulation profile.
    pub fn new(config: &HttpConfig) -> Result<Self> {
        let mut builder = Client::builder()
            .timeout(config.timeout)
            .emulation(Emulation::random())
            .redirect(Policy::limited(10));

        if let Some(proxy) = &config.proxy {
            builder = builder.proxy(wreq::Proxy::all(proxy).map_err(WebSearchError::from_wreq)?);
        }

        let inner = builder.build().map_err(WebSearchError::from_wreq)?;

        Ok(Self {
            inner,
            max_retries: config.max_retries,
            retry_delay: config.retry_delay,
        })
    }

    /// Issue a `GET` and return the response body as text, with retries.
    ///
    /// `headers` are extra per-request headers (e.g. `Referer`, JSON `Accept`);
    /// the browser-default headers are already supplied by the emulation profile.
    pub async fn get_text(&self, url: &str, headers: &[(&str, &str)]) -> Result<String> {
        self.send_text(Method::GET, url, headers, None).await
    }

    /// Issue a form-encoded `POST` and return the response body as text.
    ///
    /// Used for engines that paginate via POST (e.g. DuckDuckGo's HTML "next"
    /// form).
    pub async fn post_form_text(
        &self,
        url: &str,
        form: &HashMap<String, String>,
        headers: &[(&str, &str)],
    ) -> Result<String> {
        self.send_text(Method::POST, url, headers, Some(form)).await
    }

    async fn send_text(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        form: Option<&HashMap<String, String>>,
    ) -> Result<String> {
        let mut attempt = 0;
        loop {
            let mut builder = self.inner.request(method.clone(), url);
            for (name, value) in headers {
                builder = builder.header(*name, *value);
            }
            if let Some(form) = form {
                builder = builder.form(form);
            }

            let result = match builder.send().await {
                Ok(response) => match response.error_for_status() {
                    Ok(response) => response.text().await.map_err(WebSearchError::from_wreq),
                    Err(error) => Err(WebSearchError::from_wreq(error)),
                },
                Err(error) => Err(WebSearchError::from_wreq(error)),
            };

            match result {
                Ok(body) => return Ok(body),
                Err(error) => {
                    if attempt >= self.max_retries || !is_retryable(&error) {
                        return Err(error);
                    }
                    let backoff = self.retry_delay * (attempt as u32 + 1);
                    tokio::time::sleep(backoff).await;
                    attempt += 1;
                }
            }
        }
    }
}

/// Retry transient failures only: timeouts, connection errors, 429, and 5xx.
fn is_retryable(error: &WebSearchError) -> bool {
    match error {
        WebSearchError::Timeout(_) => true,
        WebSearchError::EngineBlocked(_) => true, // 429 (block on 403 too, but a brief backoff is cheap)
        WebSearchError::HttpStatus { status, .. } => *status >= 500,
        WebSearchError::Http(message) => {
            let lower = message.to_lowercase();
            lower.contains("connect") || lower.contains("connection")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_with_default_config() {
        let client = HttpClient::new(&HttpConfig::default());
        assert!(client.is_ok());
    }

    #[test]
    fn rejects_bad_proxy() {
        let config = HttpConfig {
            proxy: Some("not a url".to_string()),
            ..HttpConfig::default()
        };
        assert!(HttpClient::new(&config).is_err());
    }

    #[test]
    fn retry_classification() {
        assert!(is_retryable(&WebSearchError::Timeout("x".into())));
        assert!(is_retryable(&WebSearchError::HttpStatus {
            status: 503,
            message: "x".into()
        }));
        assert!(!is_retryable(&WebSearchError::HttpStatus {
            status: 404,
            message: "x".into()
        }));
        assert!(!is_retryable(&WebSearchError::Parse("x".into())));
    }
}
