//! `DDGS.extract(url, fmt)` — fetch a URL and return its content.
//!
//! Upstream (`ddgs/ddgs.py`) builds a fresh `HttpClient` (primp 2.0.1) with
//! the instance's proxy/timeout/verify, `GET`s the URL, raises
//! `DDGSException("Failed to fetch {url}: HTTP {status}")` for any status
//! other than 200, and returns `{"url": url, "content": ...}` where content
//! comes from primp:
//!
//! - `text_markdown` (default, also for unknown formats):
//!   `html2text::from_read(bytes, 100)`;
//! - `text_plain`: `from_read_with_decorator(bytes, 100, TrivialDecorator)`;
//! - `text_rich`: `from_read_with_decorator(bytes, 100, RichDecorator)`;
//! - `text`: the body decoded with the `Content-Type` charset (`mime` +
//!   `encoding_rs`, UTF-8 when absent or unknown);
//! - `content`: the raw bytes.
//!
//! This crate uses the same `html2text` (0.17.1), `mime` and `encoding_rs`
//! versions as primp 2.0.1, so the text is identical.

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use serde::Serialize;

use crate::cli::PyError;
use crate::cli::click::Value;
use crate::http::{HttpClient, HttpConfig, RawResponse};
use crate::{Result, WebSearchError};

/// The `fmt` argument of `DDGS.extract`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtractFormat {
    /// `"text_markdown"` (the default).
    #[default]
    TextMarkdown,
    /// `"text_plain"`.
    TextPlain,
    /// `"text_rich"`.
    TextRich,
    /// `"text"`: the decoded HTML.
    Text,
    /// `"content"`: the raw bytes.
    Content,
}

impl ExtractFormat {
    /// The upstream name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TextMarkdown => "text_markdown",
            Self::TextPlain => "text_plain",
            Self::TextRich => "text_rich",
            Self::Text => "text",
            Self::Content => "content",
        }
    }
}

impl FromStr for ExtractFormat {
    type Err = WebSearchError;

    /// Upstream's `if/elif/else`: any other value falls back to
    /// `text_markdown`, so this never fails.
    fn from_str(value: &str) -> Result<Self> {
        Ok(match value {
            "text_plain" => Self::TextPlain,
            "text_rich" => Self::TextRich,
            "text" => Self::Text,
            "content" => Self::Content,
            _ => Self::TextMarkdown,
        })
    }
}

impl fmt::Display for ExtractFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The extracted content: text for every format except `content`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ExtractContent {
    /// Text (`text_markdown`, `text_plain`, `text_rich`, `text`).
    Text(String),
    /// Raw bytes (`content`).
    Bytes(Vec<u8>),
}

/// `{"url": url, "content": content}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ExtractResult {
    /// The URL as given (not the final URL after redirects).
    pub url: String,
    /// The content in the requested format.
    pub content: ExtractContent,
}

/// primp `extract_encoding`: the `Content-Type` charset, else UTF-8.
fn response_encoding(response: &RawResponse) -> &'static encoding_rs::Encoding {
    response
        .header("content-type")
        .and_then(|value| value.parse::<mime::Mime>().ok())
        .and_then(|mime| {
            mime.get_param("charset")
                .and_then(|charset| encoding_rs::Encoding::for_label(charset.as_str().as_bytes()))
        })
        .unwrap_or(encoding_rs::UTF_8)
}

/// Convert a fetched body to the requested format (primp's getters).
pub(crate) fn convert(response: &RawResponse, format: ExtractFormat) -> Result<ExtractContent> {
    let bytes = response.body.as_slice();
    let html2text_error = |e: html2text::Error| WebSearchError::Parse(e.to_string());
    Ok(match format {
        ExtractFormat::TextMarkdown => {
            ExtractContent::Text(html2text::from_read(bytes, 100).map_err(html2text_error)?)
        }
        ExtractFormat::TextPlain => ExtractContent::Text(
            html2text::from_read_with_decorator(
                bytes,
                100,
                html2text::render::TrivialDecorator::new(),
            )
            .map_err(html2text_error)?,
        ),
        ExtractFormat::TextRich => ExtractContent::Text(
            html2text::from_read_with_decorator(
                bytes,
                100,
                html2text::render::RichDecorator::new(),
            )
            .map_err(html2text_error)?,
        ),
        ExtractFormat::Text => {
            let (text, _, _) = response_encoding(response).decode(bytes);
            ExtractContent::Text(text.into_owned())
        }
        ExtractFormat::Content => ExtractContent::Bytes(response.body.clone()),
    })
}

/// `DDGS.extract(url, fmt)` with the client's proxy/timeout/verify.
pub(crate) async fn extract(
    config: &HttpConfig,
    url: &str,
    format: ExtractFormat,
) -> Result<ExtractResult> {
    // Upstream builds a fresh primp client per call; no retries.
    let config = HttpConfig {
        max_retries: 0,
        ..config.clone()
    };
    let client = HttpClient::new(&config)?;
    let response = client.fetch_raw(url, &[]).await?;
    if response.status != 200 {
        return Err(WebSearchError::Internal(format!(
            "Failed to fetch {url}: HTTP {}",
            response.status
        )));
    }
    Ok(ExtractResult {
        url: url.to_owned(),
        content: convert(&response, format)?,
    })
}

/// Python `bytes.decode()` (strict UTF-8).
fn decode_utf8(bytes: &[u8]) -> std::result::Result<String, PyError> {
    String::from_utf8(bytes.to_vec()).map_err(|e| {
        let at = e.utf8_error().valid_up_to();
        PyError::new(
            "UnicodeDecodeError",
            &format!(
                "'utf-8' codec can't decode byte 0x{:02x} in position {at}: invalid start byte",
                bytes.get(at).copied().unwrap_or(0)
            ),
        )
    })
}

/// The `extract` CLI command.
#[doc(hidden)]
pub(crate) async fn cli_extract(
    values: &HashMap<&'static str, Value>,
) -> std::result::Result<(), PyError> {
    let url = values
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let format: ExtractFormat = values
        .get("fmt")
        .and_then(Value::as_str)
        .unwrap_or("text_markdown")
        .parse()
        .unwrap_or_default();
    let verify = values.get("verify").map(Value::as_bool).unwrap_or(true);
    let mut builder = crate::WebSearchClient::builder().verify(if verify {
        crate::Verify::Enabled
    } else {
        crate::Verify::Disabled
    });
    if let Some(proxy) = values.get("proxy").and_then(Value::as_str) {
        builder = builder.proxy(proxy);
    }
    let client = builder.build()?;
    let data = client.extract(&url, format).await?;
    let content = match &data.content {
        ExtractContent::Text(text) => text.clone(),
        ExtractContent::Bytes(bytes) => decode_utf8(bytes)?,
    };
    match values
        .get("output")
        .and_then(Value::as_str)
        .filter(|o| !o.is_empty())
    {
        Some(output) => {
            let mut row = serde_json::Map::new();
            row.insert(
                "url".to_owned(),
                serde_json::Value::String(data.url.clone()),
            );
            row.insert("content".to_owned(), serde_json::Value::String(content));
            crate::cli::output::save_data(
                &crate::cli::output::sanitize_query(&url),
                &[row],
                "extract",
                Some(output),
            )
        }
        None => {
            crate::cli::output::echo(&format!("URL: {url}\n"));
            crate::cli::output::echo(&content);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(content_type: Option<&str>, body: &[u8]) -> RawResponse {
        RawResponse {
            status: 200,
            headers: content_type
                .map(|c| vec![("content-type".to_owned(), c.to_owned())])
                .unwrap_or_default(),
            url: "https://example.com/".to_owned(),
            body: body.to_vec(),
        }
    }

    /// Every text format equals upstream `DDGS().extract()` (primp 2.0.1)
    /// on the same pages (`fixtures/extract_reference.json`; served by
    /// Python's http.server, so `Content-Type: text/html`).
    #[test]
    fn matches_upstream_extract_reference() {
        let reference: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../fixtures/extract_reference.json")).unwrap();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        assert!(reference.len() >= 16);
        for case in &reference {
            let file = case["file"].as_str().unwrap();
            let fmt: ExtractFormat = case["fmt"].as_str().unwrap().parse().unwrap();
            let body = std::fs::read(dir.join(file)).unwrap();
            let got = convert(&response(Some("text/html"), &body), fmt).unwrap();
            assert_eq!(
                got,
                ExtractContent::Text(case["content"].as_str().unwrap().to_owned()),
                "{file} {fmt}"
            );
        }
    }

    #[test]
    fn unknown_format_is_markdown() {
        assert_eq!(
            "nope".parse::<ExtractFormat>().unwrap(),
            ExtractFormat::TextMarkdown
        );
        assert_eq!(
            "content".parse::<ExtractFormat>().unwrap(),
            ExtractFormat::Content
        );
    }

    #[test]
    fn text_uses_content_type_charset() {
        let latin1 = response(Some("text/html; charset=ISO-8859-1"), b"caf\xe9");
        assert_eq!(
            convert(&latin1, ExtractFormat::Text).unwrap(),
            ExtractContent::Text("café".to_owned())
        );
        let default = response(None, "café".as_bytes());
        assert_eq!(
            convert(&default, ExtractFormat::Text).unwrap(),
            ExtractContent::Text("café".to_owned())
        );
    }
}
