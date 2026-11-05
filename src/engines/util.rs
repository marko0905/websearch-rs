//! Shared helpers for engines: the DuckDuckGo vqd bootstrap, Python-compatible
//! helpers (region split, truthiness, `repr`, OS randomness), JSON field
//! copies, and the glue that turns [`crate::html::extract_results`] rows into
//! results. HTML parsing and XPath live in [`crate::html`].

use url::Url;

use crate::http::HttpClient;
use crate::{Result, WebSearchError};

/// DuckDuckGo home page, the vqd bootstrap source.
pub(crate) const DDG_HOME: &str = "https://duckduckgo.com/";

/// Upstream `duckduckgo_images.py` `headers_update`, in upstream order.
///
/// Upstream sets these on the engine's client, so they go out on **both** the
/// vqd bootstrap and the `i.js` request. The DuckDuckGo news and videos engines
/// have no `headers_update` and send no extra headers.
///
/// `Connection: keep-alive` is sent on HTTP/1.1; on HTTP/2 wreq strips
/// connection-specific headers before sending (wreq-proto
/// `strip_connection_headers`), as hyper-based `primp` does.
pub(crate) const DDG_IMAGES_HEADERS: &[(&str, &str)] = &[
    ("Accept", "*/*"),
    ("Accept-Language", "en-US,en;q=0.5"),
    ("Referer", "https://duckduckgo.com/"),
    ("Sec-GPC", "1"),
    ("Connection", "keep-alive"),
    ("Sec-Fetch-Dest", "empty"),
    ("Sec-Fetch-Mode", "cors"),
    ("Sec-Fetch-Site", "same-origin"),
    ("Priority", "u=4"),
];

/// Upstream `_get_vqd`: `GET https://duckduckgo.com` with `params={"q":
/// query}` and the engine's own headers (`headers`), then `_extract_vqd`.
///
/// The response status is not checked (`.content` of any page is searched).
pub(crate) async fn fetch_ddg_vqd(
    http: &HttpClient,
    query: &str,
    headers: &[(&str, &str)],
) -> Result<String> {
    let mut url = Url::parse(DDG_HOME)?;
    url.query_pairs_mut().append_pair("q", query);
    let body = http.get(url.as_str(), headers).await?.text;
    extract_vqd(&body).ok_or_else(|| {
        // Upstream `DDGSException(f"_extract_vqd() {query=} Could not extract vqd.")`.
        WebSearchError::Internal(format!(
            "_extract_vqd() query={} Could not extract vqd.",
            py_repr(query)
        ))
    })
}

/// Python `repr()` of a `str` (quote choice and `\\`/quote escaping; other
/// non-printable characters are left as is).
pub(crate) fn py_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(value.len() + 2);
    out.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// A field copied with upstream's `item.get(key)` into a string field: a
/// missing key or JSON `null` is Python `None` (serialized as `null`), a string
/// is kept.
///
/// Other JSON types do not occur in these engines' responses; upstream would
/// store them unchanged, so a number/bool is rendered in its JSON form here
/// (Rust keeps these fields typed as strings).
pub(crate) fn json_field(
    item: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    match item.get(key) {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) => Some(text.clone()),
        Some(other) => Some(other.to_string()),
    }
}

/// A field copied with upstream's `item.get(key)` verbatim, keeping its JSON
/// type (missing → `null`).
pub(crate) fn json_raw(
    item: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> serde_json::Value {
    item.get(key).cloned().unwrap_or(serde_json::Value::Null)
}

/// Extract a DuckDuckGo `vqd` token from a bootstrap page.
///
/// DuckDuckGo's `i.js`/`v.js`/`news.js` JSON endpoints require this token, which
/// is embedded in the home page in several shapes depending on the response:
/// `vqd="4-123..."`, `vqd='4-123...'`, `vqd=4-123...`, or `"vqd":"4-123..."`.
/// Markers are tried most-specific first so the canonical assignment wins.
pub(crate) fn extract_vqd(body: &str) -> Option<String> {
    for marker in ["vqd=\"", "vqd='", "\"vqd\":\"", "vqd\":", "vqd:", "vqd="] {
        if let Some(start) = body.find(marker) {
            let rest = body[start + marker.len()..].trim_start_matches(['"', '\'', ' ']);
            let token: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '-')
                .collect();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

/// Upstream `country, lang = region.lower().split("-")`.
///
/// Python raises `ValueError` unless the region has exactly one `-`, so any
/// other shape is an error here too (the orchestrator then skips the engine,
/// as upstream does for an engine that raises). `str.lower()` is Unicode-aware,
/// so this uses [`str::to_lowercase`].
pub(crate) fn split_region(region: &str) -> Result<(String, String)> {
    let lowered = region.to_lowercase();
    let mut parts = lowered.split('-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(country), Some(lang), None) => Ok((country.to_owned(), lang.to_owned())),
        _ => Err(WebSearchError::InvalidConfig(format!(
            "region `{region}` must have the form `<country>-<lang>`, like `us-en`"
        ))),
    }
}

/// Python's truthiness of a JSON value (`if not value:`).
pub(crate) fn py_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(flag) => *flag,
        serde_json::Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        serde_json::Value::String(text) => !text.is_empty(),
        serde_json::Value::Array(items) => !items.is_empty(),
        serde_json::Value::Object(map) => !map.is_empty(),
    }
}

/// `nbytes` bytes from the OS random source (Python `os.urandom`, used by
/// `secrets` and `random.SystemRandom`).
pub(crate) fn os_random_bytes(nbytes: usize) -> Result<Vec<u8>> {
    let mut bytes = vec![0u8; nbytes];
    getrandom::fill(&mut bytes)
        .map_err(|error| WebSearchError::Internal(format!("os random source failed: {error}")))?;
    Ok(bytes)
}

/// Python `random.SystemRandom().randint(low, high)` (both ends included).
pub(crate) fn system_randint(low: u64, high: u64) -> Result<u64> {
    let span = high - low + 1;
    // Rejection sampling, like CPython's `_randbelow`, so the result is uniform.
    let bits = 64 - span.leading_zeros();
    loop {
        let bytes = os_random_bytes(8)?;
        let mut value = u64::from_le_bytes(bytes.try_into().unwrap_or([0; 8]));
        if bits < 64 {
            value &= (1u64 << bits) - 1;
        }
        if value < span {
            return Ok(low + value);
        }
    }
}

/// Port of Python `secrets.token_urlsafe(nbytes)`: `nbytes` bytes from the OS
/// random source, URL-safe base64 without `=` padding.
pub(crate) fn token_urlsafe(nbytes: usize) -> Result<String> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = os_random_bytes(nbytes)?;

    let mut out = String::with_capacity(nbytes.div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let sextets = [
            b0 >> 2,
            ((b0 & 0x03) << 4) | (b1 >> 4),
            ((b1 & 0x0f) << 2) | (b2 >> 6),
            b2 & 0x3f,
        ];
        // 1 byte → 2 chars, 2 → 3, 3 → 4 (no padding).
        for sextet in &sextets[..chunk.len() + 1] {
            out.push(char::from(ALPHABET[usize::from(*sextet)]));
        }
    }
    Ok(out)
}

/// The value of `key` in one item of [`crate::html::extract_results`].
pub(crate) fn field(fields: &[(&'static str, String)], key: &str) -> String {
    fields
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

/// Upstream `result = TextResult(); result.__setattr__(key, data)` for each
/// `elements_xpath` key (title, href, body). The constructor applies the
/// same normalizers as `BaseResult.__setattr__`.
pub(crate) fn text_result(fields: &[(&'static str, String)]) -> crate::TextResult {
    crate::TextResult::new(
        field(fields, "title"),
        field(fields, "href"),
        field(fields, "body"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_urlsafe_lengths_match_python() {
        // secrets.token_urlsafe(18) → 24 chars, token_urlsafe(35) → 47 chars.
        assert_eq!(token_urlsafe(18).unwrap().len(), 24);
        assert_eq!(token_urlsafe(35).unwrap().len(), 47);
        assert!(
            token_urlsafe(35)
                .unwrap()
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert_ne!(token_urlsafe(18).unwrap(), token_urlsafe(18).unwrap());
    }

    #[test]
    fn system_randint_stays_in_range() {
        for _ in 0..200 {
            let value = system_randint(1_000_000, 9_999_999).unwrap();
            assert!((1_000_000..=9_999_999).contains(&value));
        }
        assert_eq!(system_randint(5, 5).unwrap(), 5);
    }
}
