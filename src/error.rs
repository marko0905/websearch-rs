use thiserror::Error;

/// Convenient result alias for this crate.
pub type Result<T> = std::result::Result<T, WebSearchError>;

/// Errors returned by websearch-rs.
///
/// HTTP-layer variants carry rendered messages rather than a specific HTTP
/// crate's error type, so the public API stays independent of whether a request
/// went through `wreq` (the impersonating client used by engines) or any other
/// transport. Classify transport errors with [`WebSearchError::from_wreq`].
#[derive(Debug, Error)]
pub enum WebSearchError {
    /// The client or request configuration is invalid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// The requested parameter combination is not supported by an engine.
    #[error("unsupported parameter: {0}")]
    UnsupportedParameter(String),

    /// HTTP request timed out.
    #[error("request timed out: {0}")]
    Timeout(String),

    /// Engine returned an unsuccessful HTTP status.
    #[error("http status {status}: {message}")]
    HttpStatus {
        /// HTTP status code.
        status: u16,
        /// Rendered transport error.
        message: String,
    },

    /// HTTP client, transport, or response-body error.
    #[error("http error: {0}")]
    Http(String),

    /// URL construction or parsing error.
    #[error("url error: {0}")]
    Url(#[from] url::ParseError),

    /// I/O error, commonly runtime construction for blocking clients.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Engine response parsing failed.
    #[error("parse error: {0}")]
    Parse(String),

    /// Engine appears to have throttled, blocked, or challenged the request.
    #[error("engine blocked request: {0}")]
    EngineBlocked(String),
}

impl WebSearchError {
    /// Classify a `wreq` transport error into a specific websearch-rs error variant.
    ///
    /// HTTP 403/429 map to [`WebSearchError::EngineBlocked`] because engines use them
    /// to throttle or challenge non-browser-looking traffic.
    pub(crate) fn from_wreq(error: wreq::Error) -> Self {
        if error.is_timeout() {
            return Self::Timeout(error.to_string());
        }

        if let Some(status) = error.status() {
            if status.as_u16() == 403 || status.as_u16() == 429 {
                return Self::EngineBlocked(format!("engine returned HTTP status {status}"));
            }

            return Self::HttpStatus {
                status: status.as_u16(),
                message: error.to_string(),
            };
        }

        Self::Http(error.to_string())
    }
}
