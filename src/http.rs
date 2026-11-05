//! The single HTTP choke point for all engines.
//!
//! Every engine request goes through [`HttpClient`], which is built on
//! [`wreq`] + BoringSSL so requests reproduce a real browser's JA3/JA4 TLS and
//! HTTP/2 fingerprints — not just headers. Plain TLS stacks (e.g. rustls) are
//! fingerprint-detected and soft-blocked by Bing News/Images and Brave regardless of the
//! `User-Agent`; impersonation is what makes parity with Python `ddgs`
//! (which uses `primp`) achievable.
//!
//! Port of upstream `http_client.py` (ddgs v9.16.1) plus the transport parts of
//! `base.py`:
//! - each engine owns its own client (upstream `BaseSearchEngine.__init__`
//!   builds one `HttpClient` per engine instance), with its own random
//!   browser/OS emulation profile (`impersonate="random", impersonate_os="random"`)
//!   and its own persistent cookie store (`primp` defaults to `cookie_store=True`);
//! - redirects are followed, up to 20 (`primp` defaults);
//! - a transport failure is an error ([`WebSearchError::Timeout`] for a timeout,
//!   [`WebSearchError::Http`] otherwise); an HTTP status is **never** an error
//!   at this layer — [`HttpClient::get_text`] / [`HttpClient::post_form_text`]
//!   port `BaseSearchEngine.request` and give `None` for any status but 200;
//! - no retries by default (upstream has none). [`HttpConfig::max_retries`] is a
//!   Rust-only opt-in.
//!
//! Engines must never build their own client; route new requests through here so
//! proxy, timeout, and TLS settings stay centralized.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use wreq::cookie::Jar;
use wreq::redirect::Policy;
use wreq::tls::trust::CertStore;
use wreq::{Client, Method};
use wreq_util::Emulation;

use crate::{Result, WebSearchError};

/// The proxy URL that the `"tb"` alias expands to (Tor Browser's SOCKS port).
pub const TOR_BROWSER_PROXY: &str = "socks5h://127.0.0.1:9150";

/// Environment variable read when no proxy is given (upstream `DDGS_PROXY`).
pub const PROXY_ENV_VAR: &str = "DDGS_PROXY";

/// Upstream `_expand_proxy_tb_alias(proxy) or os.environ.get("DDGS_PROXY")`.
///
/// `"tb"` expands to [`TOR_BROWSER_PROXY`]. A missing or empty proxy (Python
/// falsy) falls back to the `DDGS_PROXY` environment variable, which is used
/// verbatim (upstream does not expand `"tb"` in it). An empty result means no
/// proxy.
pub fn resolve_proxy(proxy: Option<&str>) -> Option<String> {
    let explicit = proxy.map(|value| {
        if value == "tb" {
            TOR_BROWSER_PROXY.to_owned()
        } else {
            value.to_owned()
        }
    });
    explicit
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var(PROXY_ENV_VAR).ok())
        .filter(|value| !value.is_empty())
}

/// TLS certificate verification (upstream `verify: bool | str`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Verify {
    /// Verify certificates against the default trust store (`verify=True`).
    #[default]
    Enabled,
    /// Skip certificate verification (`verify=False`).
    Disabled,
    /// Verify certificates against the CA certificates in this PEM file
    /// (`verify="/path/to/ca.pem"`, passed to `primp` as `ca_cert_file`).
    CaFile(PathBuf),
}

/// Transport configuration for [`HttpClient`].
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// Total per-request timeout; `None` means no timeout (upstream `timeout=None`).
    pub timeout: Option<Duration>,
    /// Optional HTTP/HTTPS/SOCKS proxy URL, used as given (resolve aliases and
    /// the environment fallback with [`resolve_proxy`] first).
    pub proxy: Option<String>,
    /// TLS certificate verification.
    pub verify: Verify,
    /// Retries for transient failures (timeouts, connection errors, HTTP 429
    /// and 5xx). Upstream never retries; the default is `0`.
    pub max_retries: usize,
    /// Initial retry delay; later retries use linear backoff.
    pub retry_delay: Duration,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout: Some(Duration::from_secs(5)),
            proxy: None,
            verify: Verify::Enabled,
            max_retries: 0,
            retry_delay: Duration::from_millis(500),
        }
    }
}

/// An HTTP response: the status and the decoded body, whatever the status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body decoded as text.
    pub text: String,
}

/// A raw HTTP response: status, headers (in wire order), final URL and the
/// undecoded body (upstream `primp.Response` without the text helpers).
#[derive(Clone, Debug)]
pub struct RawResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response headers, names lowercased.
    pub headers: Vec<(String, String)>,
    /// URL after redirects.
    pub url: String,
    /// Body bytes.
    pub body: Vec<u8>,
}

impl RawResponse {
    /// The first header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A browser-impersonating HTTP client with a persistent cookie store.
///
/// Cloning shares the connection pool and the cookie store, as two references
/// to one upstream `HttpClient` would.
#[derive(Clone)]
pub struct HttpClient {
    inner: Client,
    cookies: Arc<Jar>,
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
    /// Build a client with a random browser/OS emulation profile and an empty
    /// cookie store.
    pub fn new(config: &HttpConfig) -> Result<Self> {
        let cookies = Arc::new(Jar::default());
        let mut builder = Client::builder()
            .emulation(Emulation::random())
            .cookie_provider(cookies.clone())
            .redirect(Policy::limited(20));

        if let Some(timeout) = config.timeout {
            builder = builder.timeout(timeout);
        }

        match &config.verify {
            Verify::Enabled => {}
            Verify::Disabled => builder = builder.tls_cert_verification(false),
            Verify::CaFile(path) => {
                let pem = std::fs::read(path)?;
                let store = CertStore::from_pem_stack(pem).map_err(WebSearchError::from_wreq)?;
                builder = builder.tls_cert_store(store);
            }
        }

        if let Some(proxy) = &config.proxy {
            builder = builder.proxy(wreq::Proxy::all(proxy).map_err(WebSearchError::from_wreq)?);
        }

        let inner = builder.build().map_err(WebSearchError::from_wreq)?;

        Ok(Self {
            inner,
            cookies,
            max_retries: config.max_retries,
            retry_delay: config.retry_delay,
        })
    }

    /// Port of `primp.Client.set_cookies(url, cookies)`: store `name=value`
    /// cookies for `url` in this client's persistent cookie store. They are
    /// sent on every later matching request and keep living across searches.
    pub fn set_cookies(&self, url: &str, cookies: &[(String, String)]) {
        for (name, value) in cookies {
            self.cookies.add(format!("{name}={value}").as_str(), url);
        }
    }

    /// `GET` and return the status and body, whatever the status
    /// (upstream `http_client.request("GET", ...)`).
    pub async fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<HttpResponse> {
        self.send(Method::GET, url, headers, None).await
    }

    /// Form-encoded `POST`; returns the status and body, whatever the status.
    ///
    /// `form` is an ordered list of pairs, so the encoded body keeps the field
    /// order of the upstream payload dict (Python dicts keep insertion order).
    pub async fn post_form(
        &self,
        url: &str,
        form: &[(String, String)],
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse> {
        self.send(Method::POST, url, headers, Some(form)).await
    }

    /// `GET` and return the raw response (status, headers, final URL, bytes),
    /// whatever the status. No retries.
    pub async fn fetch_raw(&self, url: &str, headers: &[(&str, &str)]) -> Result<RawResponse> {
        let mut builder = self.inner.request(Method::GET, url);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = builder.send().await.map_err(WebSearchError::from_wreq)?;
        let status = response.status().as_u16();
        let final_url = response.uri().to_string();
        let response_headers = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect();
        let body = response
            .bytes()
            .await
            .map_err(WebSearchError::from_wreq)?
            .to_vec();
        Ok(RawResponse {
            status,
            headers: response_headers,
            url: final_url,
            body,
        })
    }

    /// Upstream `BaseSearchEngine.request` for a `GET`: the body when the
    /// status is exactly 200, `None` for any other status (including 202 and
    /// other 2xx). Transport failures are errors.
    pub async fn get_text(&self, url: &str, headers: &[(&str, &str)]) -> Result<Option<String>> {
        Ok(only_200(self.get(url, headers).await?))
    }

    /// Upstream `BaseSearchEngine.request` for a form `POST`; see
    /// [`HttpClient::get_text`].
    pub async fn post_form_text(
        &self,
        url: &str,
        form: &[(String, String)],
        headers: &[(&str, &str)],
    ) -> Result<Option<String>> {
        Ok(only_200(self.post_form(url, form, headers).await?))
    }

    async fn send(
        &self,
        method: Method,
        url: &str,
        headers: &[(&str, &str)],
        form: Option<&[(String, String)]>,
    ) -> Result<HttpResponse> {
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
                Ok(response) => {
                    let status = response.status().as_u16();
                    match response.text().await {
                        Ok(text) => Ok(HttpResponse { status, text }),
                        Err(error) => Err(WebSearchError::from_wreq(error)),
                    }
                }
                Err(error) => Err(WebSearchError::from_wreq(error)),
            };

            let retry = attempt < self.max_retries
                && match &result {
                    Ok(response) => is_retryable_status(response.status),
                    Err(error) => is_retryable(error),
                };
            if !retry {
                return result;
            }
            let backoff = self.retry_delay * (attempt as u32 + 1);
            tokio::time::sleep(backoff).await;
            attempt += 1;
        }
    }
}

/// `if resp.status_code == 200: return resp.text` / `return None`.
fn only_200(response: HttpResponse) -> Option<String> {
    (response.status == 200).then_some(response.text)
}

/// Opt-in retries only: HTTP 429 and 5xx.
fn is_retryable_status(status: u16) -> bool {
    status == 429 || status >= 500
}

/// Opt-in retries only: timeouts and connection errors.
fn is_retryable(error: &WebSearchError) -> bool {
    match error {
        WebSearchError::Timeout(_) => true,
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
    fn defaults_match_upstream() {
        let config = HttpConfig::default();
        assert_eq!(config.timeout, Some(Duration::from_secs(5)));
        assert_eq!(config.max_retries, 0);
        assert_eq!(config.verify, Verify::Enabled);
    }

    #[test]
    fn builds_without_timeout_or_verification() {
        let config = HttpConfig {
            timeout: None,
            verify: Verify::Disabled,
            ..HttpConfig::default()
        };
        assert!(HttpClient::new(&config).is_ok());
    }

    #[test]
    fn missing_ca_file_is_an_error() {
        let config = HttpConfig {
            verify: Verify::CaFile(PathBuf::from("/nonexistent/websearch-rs/ca.pem")),
            ..HttpConfig::default()
        };
        assert!(matches!(
            HttpClient::new(&config),
            Err(WebSearchError::Io(_))
        ));
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
    fn socks_proxy_is_accepted() {
        let config = HttpConfig {
            proxy: Some(TOR_BROWSER_PROXY.to_string()),
            ..HttpConfig::default()
        };
        assert!(HttpClient::new(&config).is_ok());
    }

    #[test]
    fn tb_alias_expands() {
        assert_eq!(
            resolve_proxy(Some("tb")).as_deref(),
            Some(TOR_BROWSER_PROXY)
        );
        assert_eq!(
            resolve_proxy(Some("http://p:1")).as_deref(),
            Some("http://p:1")
        );
    }

    #[test]
    fn only_status_200_has_a_body() {
        let response = |status| HttpResponse {
            status,
            text: "body".into(),
        };
        assert_eq!(only_200(response(200)).as_deref(), Some("body"));
        for status in [201, 202, 204, 301, 403, 429, 500] {
            assert_eq!(only_200(response(status)), None, "status {status}");
        }
    }

    #[test]
    fn set_cookies_reach_the_store() {
        let client = HttpClient::new(&HttpConfig::default()).expect("client");
        client.set_cookies(
            "https://search.brave.com",
            &[
                ("us".into(), "us".into()),
                ("useLocation".into(), "0".into()),
            ],
        );
        let cookie = client
            .cookies
            .get("useLocation", "https://search.brave.com/")
            .expect("cookie stored");
        assert_eq!(cookie.value(), "0");
    }

    #[test]
    fn retry_classification() {
        assert!(is_retryable(&WebSearchError::Timeout("x".into())));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(429));
        assert!(!is_retryable_status(404));
        assert!(!is_retryable_status(202));
        assert!(!is_retryable(&WebSearchError::Parse("x".into())));
    }
}
