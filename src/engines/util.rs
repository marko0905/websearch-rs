//! Shared HTML-parsing helpers for engines.
//!
//! Engine-specific URL unwrapping and field extraction stay in each engine
//! module; this holds only the generic selector/text utilities every
//! HTML-scraping engine needs.

use scraper::{ElementRef, Selector};
use url::Url;

use crate::http::HttpClient;
use crate::{Result, WebSearchError};

/// DuckDuckGo home page — both the vqd bootstrap source and the `Referer` the
/// JSON endpoints expect.
pub(crate) const DDG_HOME: &str = "https://duckduckgo.com/";

/// Headers the DuckDuckGo `i.js`/`v.js`/`news.js` JSON endpoints require.
///
/// These mirror what a browser's in-page XHR sends (matching Python `ddgs`):
/// a wildcard `Accept`, the `Sec-Fetch-*` / `Sec-GPC` fetch-metadata headers,
/// and a DuckDuckGo `Referer`. Sending an `X-Requested-With`/JSON `Accept`
/// instead (the older shape) now draws a 403.
pub(crate) const DDG_JSON_HEADERS: &[(&str, &str)] = &[
    ("Accept", "*/*"),
    ("Accept-Language", "en-US,en;q=0.5"),
    ("Referer", DDG_HOME),
    ("Sec-GPC", "1"),
    ("Sec-Fetch-Dest", "empty"),
    ("Sec-Fetch-Mode", "cors"),
    ("Sec-Fetch-Site", "same-origin"),
];

/// Fetch a DuckDuckGo `vqd` token for `query` from the bootstrap page.
///
/// The bootstrap request carries **only** `q` — adding `ia=` (images/news/...)
/// makes DuckDuckGo serve a page without the token. Shared by the image, video,
/// and news engines so the flow stays identical.
pub(crate) async fn fetch_ddg_vqd(http: &HttpClient, query: &str) -> Result<String> {
    let mut url = Url::parse(DDG_HOME)?;
    url.query_pairs_mut().append_pair("q", query);
    let body = http
        .get_text(url.as_str(), &[("Referer", DDG_HOME)])
        .await?;
    extract_vqd(&body)
        .ok_or_else(|| WebSearchError::Parse("DuckDuckGo bootstrap missing vqd token".into()))
}

/// Collapse runs of whitespace into single spaces and trim.
pub(crate) fn normalize_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse a single CSS selector, mapping failures to a typed parse error.
pub(crate) fn selector(value: &str) -> Result<Selector> {
    Selector::parse(value)
        .map_err(|err| WebSearchError::Parse(format!("invalid selector `{value}`: {err}")))
}

/// Parse several CSS selectors at once.
pub(crate) fn selectors(values: &[&str]) -> Result<Vec<Selector>> {
    values.iter().map(|value| selector(value)).collect()
}

/// First descendant of `node` matching any of `selectors`, in order.
pub(crate) fn first_match<'a>(
    node: ElementRef<'a>,
    selectors: &[Selector],
) -> Option<ElementRef<'a>> {
    selectors
        .iter()
        .find_map(|selector| node.select(selector).next())
}

/// Concatenated, whitespace-normalized text content of an element.
pub(crate) fn element_text(node: ElementRef<'_>) -> String {
    normalize_text(&node.text().collect::<Vec<_>>().join(" "))
}

/// Strip HTML tags, decode entities, and normalize whitespace.
///
/// For text that arrives as an HTML fragment inside JSON (e.g. Wikipedia's
/// `<span class="searchmatch">` highlighted snippets).
pub(crate) fn clean_html(value: &str) -> String {
    let mut stripped = String::with_capacity(value.len());
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(ch),
            _ => {}
        }
    }
    normalize_text(&html_escape::decode_html_entities(&stripped))
}

/// Decode base64 (URL-safe or standard alphabet, padding optional).
///
/// Used by engines whose result links are base64-wrapped redirects (e.g. Bing's
/// `/ck/a?...&u=a1<base64url>`). Returns `None` on any invalid character.
pub(crate) fn decode_base64(input: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' | b'-' => Some(62),
            b'/' | b'_' => Some(63),
            _ => None,
        }
    }

    let bytes: Vec<u8> = input.bytes().filter(|byte| *byte != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut buf = [0u8; 4];
        for (slot, &byte) in buf.iter_mut().zip(chunk) {
            *slot = sextet(byte)?;
        }
        match chunk.len() {
            4 => {
                out.push((buf[0] << 2) | (buf[1] >> 4));
                out.push((buf[1] << 4) | (buf[2] >> 2));
                out.push((buf[2] << 6) | buf[3]);
            }
            3 => {
                out.push((buf[0] << 2) | (buf[1] >> 4));
                out.push((buf[1] << 4) | (buf[2] >> 2));
            }
            2 => out.push((buf[0] << 2) | (buf[1] >> 4)),
            _ => return None,
        }
    }
    Some(out)
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

/// Percent-decode a string (`%XX` → byte). Unlike form decoding, `+` is left
/// intact. Invalid escapes are passed through literally. Used by engines whose
/// redirect links carry percent-encoded destinations (e.g. Yahoo's `/RU=`).
pub(crate) fn percent_decode(input: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push((hi << 4) | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
