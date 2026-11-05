//! `ddgs mcp` — the MCP server over stdio (port of `ddgs/api_server/mcp.py`,
//! which runs on the official `mcp` Python SDK 2.3).
//!
//! JSON-RPC 2.0, one message per line on stdin/stdout. Behavior copied from
//! the upstream server (see `fixtures/mcp/reference.json`, recorded by
//! `scripts/regen_mcp_reference.py`):
//!
//! - `initialize` echoes a supported protocol version (else the latest,
//!   `2025-11-25`) with the SDK's capabilities and `serverInfo`
//!   `{"name": "ddgs-search", "version": ""}`;
//! - `tools/list` returns the SDK-generated tool definitions, embedded
//!   verbatim (`mcp_tools.json`);
//! - `tools/call` validates arguments like pydantic (lax mode, same error
//!   text), runs the tool, and frames results like the SDK: one pretty JSON
//!   text block per row plus `structuredContent`. A tool that raises returns
//!   `"Error executing tool <name>"` without details;
//! - unknown methods get `-32601 Method not found`; notifications, invalid
//!   JSON and messages without `"jsonrpc"` get no response.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

use crate::cli::PyError;
use crate::cli::py::py_repr_value;
use crate::{
    BooksSearchRequest, ExtractContent, ExtractFormat, ImageSearchRequest, NewsSearchRequest,
    Region, TextSearchRequest, VideoSearchRequest, WebSearchClient,
};

/// The SDK-generated tool definitions of the upstream server.
const TOOLS_JSON: &str = include_str!("mcp_tools.json");

const SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"];
const LATEST_VERSION: &str = "2025-11-25";
const PYDANTIC_DOCS: &str = "https://errors.pydantic.dev/2.14/v/";

/// A tool parameter type (as declared in `mcp.py`).
#[derive(Clone, Copy)]
enum Kind {
    Str,
    OptStr,
    Int,
}

struct Param {
    name: &'static str,
    kind: Kind,
    default: Option<Value>,
}

const fn required(name: &'static str) -> Param {
    Param {
        name,
        kind: Kind::Str,
        default: None,
    }
}

fn opt(name: &'static str, kind: Kind, default: Value) -> Param {
    Param {
        name,
        kind,
        default: Some(default),
    }
}

fn tool_params(tool: &str) -> Option<Vec<Param>> {
    let base = || {
        vec![
            required("query"),
            opt("region", Kind::Str, json!("us-en")),
            opt("safesearch", Kind::Str, json!("moderate")),
            opt("timelimit", Kind::OptStr, Value::Null),
            opt("max_results", Kind::Int, json!(10)),
            opt("page", Kind::Int, json!(1)),
            opt("backend", Kind::Str, json!("auto")),
        ]
    };
    Some(match tool {
        "search_text" | "search_news" => base(),
        "search_images" => {
            let mut p = base();
            for name in ["size", "color", "type_image", "layout", "license_image"] {
                p.push(opt(name, Kind::OptStr, Value::Null));
            }
            p
        }
        "search_videos" => {
            let mut p = base();
            for name in ["resolution", "duration", "license_videos"] {
                p.push(opt(name, Kind::OptStr, Value::Null));
            }
            p
        }
        "search_books" => vec![
            required("query"),
            opt("max_results", Kind::Int, json!(10)),
            opt("page", Kind::Int, json!(1)),
            opt("backend", Kind::Str, json!("auto")),
        ],
        "extract_content" => vec![
            required("url"),
            opt("fmt", Kind::Str, json!("text_markdown")),
        ],
        _ => return None,
    })
}

/// Python type name of a JSON value (`input_type=`).
fn py_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// pydantic-core's `input_value` display: the repr, truncated to
/// `first 25 + "..." + last 24` characters when longer than 50.
fn input_repr(value: &Value) -> String {
    let repr = py_repr_value(value);
    let chars: Vec<char> = repr.chars().collect();
    if chars.len() > 50 {
        let head: String = chars[..25].iter().collect();
        let tail: String = chars[chars.len() - 24..].iter().collect();
        format!("{head}...{tail}")
    } else {
        repr
    }
}

/// pydantic lax `str → int`: whitespace stripped, optional sign, digits with
/// single underscores between them.
fn parse_int_str(text: &str) -> Option<i128> {
    let t = text.trim();
    let (negative, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let n: i128 = digits.replace('_', "").parse().ok()?;
    Some(if negative { -n } else { n })
}

/// A validated argument value.
#[derive(Clone, Debug)]
pub(crate) enum Arg {
    Str(String),
    None,
    Int(i128),
}

struct ValidationError {
    field: &'static str,
    kind: &'static str,
    message: &'static str,
    input: String,
    input_type: &'static str,
}

fn validate_value(param: &Param, value: &Value) -> Result<Arg, ValidationError> {
    let error = |kind: &'static str, message: &'static str| ValidationError {
        field: param.name,
        kind,
        message,
        input: input_repr(value),
        input_type: py_type(value),
    };
    match param.kind {
        Kind::Str => match value {
            Value::String(s) => Ok(Arg::Str(s.clone())),
            _ => Err(error("string_type", "Input should be a valid string")),
        },
        Kind::OptStr => match value {
            Value::Null => Ok(Arg::None),
            Value::String(s) => Ok(Arg::Str(s.clone())),
            _ => Err(error("string_type", "Input should be a valid string")),
        },
        Kind::Int => match value {
            Value::Bool(b) => Ok(Arg::Int(i128::from(*b))),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(Arg::Int(i128::from(i)))
                } else if let Some(u) = n.as_u64() {
                    Ok(Arg::Int(i128::from(u)))
                } else {
                    let f = n.as_f64().unwrap_or(f64::NAN);
                    if f.is_finite() && f.fract() == 0.0 {
                        Ok(Arg::Int(f as i128))
                    } else {
                        Err(error(
                            "int_from_float",
                            "Input should be a valid integer, got a number with a fractional part",
                        ))
                    }
                }
            }
            Value::String(s) => parse_int_str(s).map(Arg::Int).ok_or_else(|| {
                error(
                    "int_parsing",
                    "Input should be a valid integer, unable to parse string as an integer",
                )
            }),
            _ => Err(error("int_type", "Input should be a valid integer")),
        },
    }
}

/// Validate `arguments` like the SDK's generated pydantic model
/// (`<tool>Arguments`, extra fields ignored, errors in field order).
fn validate(
    tool: &str,
    params: &[Param],
    arguments: &Map<String, Value>,
) -> Result<HashMap<&'static str, Arg>, String> {
    let mut values = HashMap::new();
    let mut errors = Vec::new();
    for param in params {
        match arguments.get(param.name) {
            Some(value) => match validate_value(param, value) {
                Ok(arg) => {
                    values.insert(param.name, arg);
                }
                Err(e) => errors.push(e),
            },
            None => match &param.default {
                Some(Value::Null) => {
                    values.insert(param.name, Arg::None);
                }
                Some(Value::String(s)) => {
                    values.insert(param.name, Arg::Str(s.clone()));
                }
                Some(Value::Number(n)) => {
                    values.insert(param.name, Arg::Int(i128::from(n.as_i64().unwrap_or(0))));
                }
                _ => errors.push(ValidationError {
                    field: param.name,
                    kind: "missing",
                    message: "Field required",
                    input: input_repr(&Value::Object(arguments.clone())),
                    input_type: "dict",
                }),
            },
        }
    }
    if errors.is_empty() {
        return Ok(values);
    }
    let header = if errors.len() == 1 {
        format!("1 validation error for {tool}Arguments")
    } else {
        format!("{} validation errors for {tool}Arguments", errors.len())
    };
    let body: Vec<String> = errors
        .iter()
        .map(|e| {
            format!(
                "{}\n  {} [type={}, input_value={}, input_type={}]\n    For further information visit {PYDANTIC_DOCS}{}",
                e.field, e.message, e.kind, e.input, e.input_type, e.kind
            )
        })
        .collect();
    Err(format!(
        "Error executing tool {tool}: {header}\n{}",
        body.join("\n")
    ))
}

/// What the tools do; the real one searches, tests use a stub.
#[async_trait]
pub(crate) trait Runner: Send + Sync {
    /// Run one search tool; rows are the result dicts.
    async fn search(
        &self,
        tool: &str,
        args: &HashMap<&'static str, Arg>,
    ) -> Result<Vec<Map<String, Value>>, PyError>;
    /// `DDGS().extract(url, fmt)`.
    async fn extract(&self, url: &str, fmt: &str) -> Result<crate::ExtractResult, PyError>;
}

fn arg_str<'a>(args: &'a HashMap<&'static str, Arg>, key: &str) -> Option<&'a str> {
    match args.get(key) {
        Some(Arg::Str(s)) => Some(s),
        _ => None,
    }
}

fn arg_int(args: &HashMap<&'static str, Arg>, key: &str) -> i128 {
    match args.get(key) {
        Some(Arg::Int(i)) => *i,
        _ => 0,
    }
}

/// `max_results` passed to `DDGS`: `0` (falsy) means all. Negative values
/// cannot be expressed by the Rust request and also mean all here.
fn max_results(args: &HashMap<&'static str, Arg>) -> Option<usize> {
    let n = arg_int(args, "max_results");
    if n <= 0 {
        None
    } else {
        Some(usize::try_from(n).unwrap_or(usize::MAX))
    }
}

fn page(args: &HashMap<&'static str, Arg>) -> usize {
    usize::try_from(arg_int(args, "page")).unwrap_or(0)
}

fn rows<R: serde::Serialize>(results: Vec<R>) -> Result<Vec<Map<String, Value>>, PyError> {
    results
        .iter()
        .map(|r| match serde_json::to_value(r) {
            Ok(Value::Object(map)) => Ok(map),
            _ => Err(PyError::new("TypeError", "result is not a dict")),
        })
        .collect()
}

/// The real runner: `DDGS(proxy=_expand_proxy_tb_alias(os.environ.get("DDGS_PROXY")))`
/// (the builder expands `"tb"` the same way).
pub(crate) struct SearchRunner;

impl SearchRunner {
    fn client() -> Result<WebSearchClient, PyError> {
        let mut builder = WebSearchClient::builder();
        match std::env::var(crate::http::PROXY_ENV_VAR)
            .ok()
            .filter(|p| !p.is_empty())
        {
            Some(proxy) => builder = builder.proxy(proxy),
            None => builder = builder.no_proxy(),
        }
        builder.build().map_err(PyError::from)
    }
}

#[async_trait]
impl Runner for SearchRunner {
    async fn search(
        &self,
        tool: &str,
        args: &HashMap<&'static str, Arg>,
    ) -> Result<Vec<Map<String, Value>>, PyError> {
        let client = Self::client()?;
        let query = arg_str(args, "query").unwrap_or("").to_owned();
        let region =
            || Region::new(arg_str(args, "region").unwrap_or("us-en")).map_err(PyError::from);
        let safesearch = || {
            arg_str(args, "safesearch")
                .unwrap_or("moderate")
                .parse()
                .map_err(PyError::from)
        };
        let timelimit = || -> Result<_, PyError> {
            // `if timelimit:` upstream — an empty string means no limit.
            match arg_str(args, "timelimit").filter(|t| !t.is_empty()) {
                Some(t) => Ok(Some(t.parse().map_err(PyError::from)?)),
                None => Ok(None),
            }
        };
        let backend = arg_str(args, "backend").unwrap_or("auto").to_owned();
        match tool {
            "search_text" => rows(
                client
                    .text_with(
                        TextSearchRequest::new(query)
                            .region(region()?)
                            .safe_search(safesearch()?)
                            .time_limit(timelimit()?)
                            .max_results(max_results(args))
                            .page(page(args))
                            .backend(backend),
                    )
                    .await?,
            ),
            "search_news" => rows(
                client
                    .news_with(
                        NewsSearchRequest::new(query)
                            .region(region()?)
                            .safe_search(safesearch()?)
                            .time_limit(timelimit()?)
                            .max_results(max_results(args))
                            .page(page(args))
                            .backend(backend),
                    )
                    .await?,
            ),
            "search_images" => {
                let mut request = ImageSearchRequest::new(query)
                    .region(region()?)
                    .safe_search(safesearch()?)
                    .time_limit(timelimit()?)
                    .max_results(max_results(args))
                    .page(page(args))
                    .backend(backend);
                if let Some(v) = arg_str(args, "size") {
                    request = request.size(v);
                }
                if let Some(v) = arg_str(args, "color") {
                    request = request.color(v);
                }
                if let Some(v) = arg_str(args, "type_image") {
                    request = request.type_image(v);
                }
                if let Some(v) = arg_str(args, "layout") {
                    request = request.layout(v);
                }
                if let Some(v) = arg_str(args, "license_image") {
                    request = request.license_image(v);
                }
                rows(client.images_with(request).await?)
            }
            "search_videos" => {
                let mut request = VideoSearchRequest::new(query)
                    .region(region()?)
                    .safe_search(safesearch()?)
                    .time_limit(timelimit()?)
                    .max_results(max_results(args))
                    .page(page(args))
                    .backend(backend);
                if let Some(v) = arg_str(args, "resolution") {
                    request = request.resolution(v);
                }
                if let Some(v) = arg_str(args, "duration") {
                    request = request.duration(v);
                }
                if let Some(v) = arg_str(args, "license_videos") {
                    request = request.license_videos(v);
                }
                rows(client.videos_with(request).await?)
            }
            "search_books" => rows(
                client
                    .books_with(
                        BooksSearchRequest::new(query)
                            .max_results(max_results(args))
                            .page(page(args))
                            .backend(backend),
                    )
                    .await?,
            ),
            other => Err(PyError::new("KeyError", other)),
        }
    }

    async fn extract(&self, url: &str, fmt: &str) -> Result<crate::ExtractResult, PyError> {
        let format: ExtractFormat = fmt.parse().unwrap_or_default();
        Ok(Self::client()?.extract(url, format).await?)
    }
}

fn text_block(value: &Value) -> Value {
    json!({"text": serde_json::to_string_pretty(value).unwrap_or_default(), "type": "text"})
}

fn tool_error(text: &str) -> Value {
    json!({"content": [{"text": text, "type": "text"}], "isError": true})
}

/// Handle `tools/call` and return the `result` object.
async fn call_tool(runner: &dyn Runner, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let empty = Map::new();
    let arguments = params
        .get("arguments")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let Some(spec) = tool_params(name) else {
        return tool_error(&format!("Unknown tool: {name}"));
    };
    let args = match validate(name, &spec, arguments) {
        Ok(args) => args,
        Err(text) => return tool_error(&text),
    };
    let generic = || tool_error(&format!("Error executing tool {name}"));
    if name == "extract_content" {
        let url = arg_str(&args, "url").unwrap_or("");
        let fmt = arg_str(&args, "fmt").unwrap_or("text_markdown");
        let Ok(result) = runner.extract(url, fmt).await else {
            return generic();
        };
        // pydantic serializes `bytes` as UTF-8 text.
        let content = match result.content {
            ExtractContent::Text(text) => text,
            ExtractContent::Bytes(bytes) => match String::from_utf8(bytes) {
                Ok(text) => text,
                Err(_) => return generic(),
            },
        };
        let dict = json!({"url": result.url, "content": content});
        return json!({"content": [text_block(&dict)], "isError": false, "structuredContent": dict});
    }
    let Ok(rows) = runner.search(name, &args).await else {
        return generic();
    };
    let rows: Vec<Value> = rows.into_iter().map(Value::Object).collect();
    let blocks: Vec<Value> = rows.iter().map(text_block).collect();
    json!({"content": blocks, "isError": false, "structuredContent": {"result": rows}})
}

/// Handle one input line; `None` when no response is due.
pub(crate) async fn handle_line(runner: &dyn Runner, line: &str) -> Option<String> {
    let message: Value = serde_json::from_str(line).ok()?;
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return None;
    }
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = if SUPPORTED_VERSIONS.contains(&requested) {
                requested
            } else {
                LATEST_VERSION
            };
            json!({
                "capabilities": {
                    "prompts": {"listChanged": false},
                    "resources": {"listChanged": false, "subscribe": false},
                    "tools": {"listChanged": false}
                },
                "protocolVersion": version,
                "serverInfo": {"name": "ddgs-search", "version": ""}
            })
        }
        "ping" => json!({}),
        "tools/list" => {
            let tools: Value = serde_json::from_str(TOOLS_JSON).unwrap_or(Value::Array(Vec::new()));
            json!({"tools": tools})
        }
        "prompts/list" => json!({"prompts": []}),
        "resources/list" => json!({"resources": []}),
        "resources/templates/list" => json!({"resourceTemplates": []}),
        "tools/call" => call_tool(runner, &params).await,
        other => {
            let response = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": "Method not found", "data": other}
            });
            return Some(response.to_string());
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string())
}

/// Serve on stdin/stdout until stdin closes. Requests run concurrently and
/// responses are written as they complete.
pub(crate) async fn serve_stdio(runner: Arc<dyn Runner>) -> std::io::Result<()> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = rx.recv().await {
            if stdout
                .write_all(format!("{line}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
            let _ = stdout.flush().await;
        }
    });
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        let runner = runner.clone();
        let tx = tx.clone();
        tasks.spawn(async move {
            if let Some(response) = handle_line(runner.as_ref(), &line).await {
                let _ = tx.send(response);
            }
        });
    }
    while tasks.join_next().await.is_some() {}
    drop(tx);
    let _ = writer.await;
    Ok(())
}

/// The `mcp` CLI command.
pub(crate) async fn cli_mcp(proxy: Option<&str>) -> Result<(), PyError> {
    if let Some(proxy) = proxy.filter(|p| !p.is_empty()) {
        let expanded = crate::http::resolve_proxy(Some(proxy)).unwrap_or_else(|| proxy.to_owned());
        // SAFETY (Rust 2024 `set_var`): set once at startup, before the
        // server spawns any task that reads the environment.
        unsafe { std::env::set_var(crate::http::PROXY_ENV_VAR, expanded) };
    }
    serve_stdio(Arc::new(SearchRunner))
        .await
        .map_err(|e| PyError::os(&e, ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the rows the reference recording used, and serves extract
    /// requests from `fixtures/` like Python's http.server did.
    struct StubRunner {
        rows: Value,
    }

    #[async_trait]
    impl Runner for StubRunner {
        async fn search(
            &self,
            tool: &str,
            args: &HashMap<&'static str, Arg>,
        ) -> Result<Vec<Map<String, Value>>, PyError> {
            if arg_str(args, "query").unwrap_or("").is_empty() {
                return Err(PyError::new("DDGSException", "query is mandatory."));
            }
            let category = tool.trim_start_matches("search_");
            Ok(self.rows[category]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r.as_object().unwrap().clone())
                .collect())
        }

        async fn extract(&self, url: &str, fmt: &str) -> Result<crate::ExtractResult, PyError> {
            let file = url.rsplit('/').next().unwrap();
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join(file);
            let Ok(body) = std::fs::read(path) else {
                return Err(PyError::new(
                    "DDGSException",
                    &format!("Failed to fetch {url}: HTTP 404"),
                ));
            };
            let response = crate::http::RawResponse {
                status: 200,
                headers: vec![("content-type".to_owned(), "text/html".to_owned())],
                url: url.to_owned(),
                body,
            };
            let format: ExtractFormat = fmt.parse().unwrap();
            Ok(crate::ExtractResult {
                url: url.to_owned(),
                content: crate::extract::convert(&response, format).map_err(PyError::from)?,
            })
        }
    }

    /// Every request of the upstream recording gets the same response.
    #[tokio::test]
    async fn matches_upstream_mcp_reference() {
        let reference: Vec<Value> =
            serde_json::from_str(include_str!("../../fixtures/mcp/reference.json")).unwrap();
        let runner = StubRunner {
            rows: serde_json::from_str(include_str!("../../fixtures/mcp/search_rows.json"))
                .unwrap(),
        };
        assert!(reference.len() >= 40);
        for exchange in &reference {
            let request = exchange["request"].to_string();
            let response = handle_line(&runner, &request).await;
            let expected = exchange["response"].as_str().map(str::to_owned);
            assert_eq!(response, expected, "request {request}");
        }
    }

    #[tokio::test]
    async fn ignores_invalid_messages() {
        let runner = StubRunner { rows: json!({}) };
        assert_eq!(handle_line(&runner, "this is not json").await, None);
        assert_eq!(
            handle_line(&runner, r#"{"id":2,"method":"ping"}"#).await,
            None
        );
        assert_eq!(
            handle_line(&runner, r#"{"jsonrpc":"2.0","id":"abc","method":"ping"}"#)
                .await
                .as_deref(),
            Some(r#"{"jsonrpc":"2.0","id":"abc","result":{}}"#)
        );
    }
}
