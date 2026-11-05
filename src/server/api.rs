//! `ddgs api` — the REST API server (port of `ddgs/api_server/api.py`, a
//! FastAPI 0.162 app on Starlette 1.10, served by uvicorn 0.54).
//!
//! Behavior copied from the upstream server (see `fixtures/api/reference.json`,
//! recorded by `scripts/regen_api_reference.py`):
//!
//! - routes `/`, `/health`, `GET`/`POST` `/search/{text,images,news,videos,books}`,
//!   `GET`/`POST` `/extract`, plus FastAPI's `/openapi.json`, `/docs`,
//!   `/docs/oauth2-redirect` and `/redoc` (embedded verbatim);
//! - FastAPI request validation (pydantic lax mode) with the same 422 body,
//!   including Python `json` decode errors and their character positions;
//! - `500 {"detail": "<Kind> failed: <str(e)>"}` when a search raises;
//! - Starlette 404/405/307 (trailing slash) responses and the CORS
//!   middleware (`allow_origins=["*"]`, credentials, all methods/headers);
//! - uvicorn's `date`/`server` headers and its log lines.

use std::collections::HashMap;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::mcp::{Arg, Runner, SearchRunner};
use crate::ExtractContent;
use crate::cli::PyError;
use crate::cli::click::Value as CliValue;

const OPENAPI_JSON: &str = include_str!("api_static/openapi.json");
const DOCS_HTML: &str = include_str!("api_static/docs.html");
const REDOC_HTML: &str = include_str!("api_static/redoc.html");
const OAUTH2_REDIRECT_HTML: &str = include_str!("api_static/oauth2-redirect.html");

const JSON: &str = "application/json";
const HTML: &str = "text/html; charset=utf-8";

/// A parsed HTTP request.
#[derive(Debug, Default)]
pub(crate) struct Request {
    pub method: String,
    /// The request target as sent (path and query).
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn path(&self) -> String {
        let raw = self.target.split('?').next().unwrap_or("");
        crate::normalize::unquote(raw)
    }

    /// Query parameters, last value wins (Starlette `QueryParams.get`).
    fn query(&self) -> HashMap<String, String> {
        let query = self.target.split_once('?').map(|(_, q)| q).unwrap_or("");
        form_urlencoded::parse(query.as_bytes())
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }
}

/// An HTTP response (headers in wire order, before uvicorn's own).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    fn new(
        status: u16,
        content_type: Option<&str>,
        body: Vec<u8>,
        extra: Vec<(String, String)>,
    ) -> Self {
        let mut headers = extra;
        headers.push(("content-length".to_owned(), body.len().to_string()));
        if let Some(content_type) = content_type {
            headers.push(("content-type".to_owned(), content_type.to_owned()));
        }
        Self {
            status,
            headers,
            body,
        }
    }

    fn json(status: u16, value: &Value) -> Self {
        Self::new(
            status,
            Some(JSON),
            value.to_string().into_bytes(),
            Vec::new(),
        )
    }

    fn detail(status: u16, detail: &str) -> Self {
        Self::json(status, &json!({ "detail": detail }))
    }
}

// ---------------------------------------------------------------------------
// Python `json.loads` error messages (CPython `json.decoder`/`_json.c`)

/// The first decode error as `(message, character position)`, or `None` when
/// CPython's `json.loads` would accept the text.
fn python_json_error(text: &str) -> Option<(&'static str, usize)> {
    let s: Vec<char> = text.chars().collect();
    let ws = |mut i: usize| {
        while i < s.len() && matches!(s[i], ' ' | '\t' | '\n' | '\r') {
            i += 1;
        }
        i
    };
    let start = ws(0);
    match scan_value(&s, start) {
        Ok(end) => {
            let end = ws(end);
            if end != s.len() {
                Some(("Extra data", end))
            } else {
                None
            }
        }
        Err(e) => Some(e),
    }
}

type ScanResult = Result<usize, (&'static str, usize)>;

fn scan_value(s: &[char], idx: usize) -> ScanResult {
    let Some(&c) = s.get(idx) else {
        return Err(("Expecting value", idx));
    };
    let starts = |lit: &str| {
        s[idx..]
            .iter()
            .take(lit.chars().count())
            .copied()
            .eq(lit.chars())
    };
    match c {
        '"' => scan_string(s, idx + 1),
        '{' => scan_object(s, idx + 1),
        '[' => scan_array(s, idx + 1),
        'n' if starts("null") => Ok(idx + 4),
        't' if starts("true") => Ok(idx + 4),
        'f' if starts("false") => Ok(idx + 5),
        'N' if starts("NaN") => Ok(idx + 3),
        'I' if starts("Infinity") => Ok(idx + 8),
        '-' if s[idx..]
            .iter()
            .skip(1)
            .take(8)
            .copied()
            .eq("Infinity".chars()) =>
        {
            Ok(idx + 9)
        }
        _ => scan_number(s, idx).ok_or(("Expecting value", idx)),
    }
}

/// `NUMBER_RE = r'(-?(?:0|[1-9]\d*))(\.\d+)?([eE][-+]?\d+)?'`
fn scan_number(s: &[char], idx: usize) -> Option<usize> {
    let mut i = idx;
    if s.get(i) == Some(&'-') {
        i += 1;
    }
    match s.get(i) {
        Some('0') => i += 1,
        Some(c) if c.is_ascii_digit() => {
            while s.get(i).is_some_and(char::is_ascii_digit) {
                i += 1;
            }
        }
        _ => return None,
    }
    if s.get(i) == Some(&'.') && s.get(i + 1).is_some_and(char::is_ascii_digit) {
        i += 1;
        while s.get(i).is_some_and(char::is_ascii_digit) {
            i += 1;
        }
    }
    if matches!(s.get(i), Some('e' | 'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some('+' | '-')) {
            j += 1;
        }
        if s.get(j).is_some_and(char::is_ascii_digit) {
            while s.get(j).is_some_and(char::is_ascii_digit) {
                j += 1;
            }
            i = j;
        }
    }
    Some(i)
}

/// `scanstring` starting after the opening quote.
fn scan_string(s: &[char], mut i: usize) -> ScanResult {
    let begin = i - 1;
    loop {
        let Some(&c) = s.get(i) else {
            return Err(("Unterminated string starting at", begin));
        };
        match c {
            '"' => return Ok(i + 1),
            '\\' => {
                let Some(&esc) = s.get(i + 1) else {
                    return Err(("Unterminated string starting at", begin));
                };
                if esc == 'u' {
                    let hex = s.get(i + 2..i + 6);
                    if hex.is_none_or(|h| !h.iter().all(char::is_ascii_hexdigit)) {
                        return Err(("Invalid \\uXXXX escape", i + 1));
                    }
                    i += 6;
                } else if matches!(esc, '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                    i += 2;
                } else {
                    return Err(("Invalid \\escape", i));
                }
            }
            c if (c as u32) < 0x20 => return Err(("Invalid control character at", i)),
            _ => i += 1,
        }
    }
}

fn skip_ws(s: &[char], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], ' ' | '\t' | '\n' | '\r') {
        i += 1;
    }
    i
}

fn scan_object(s: &[char], i: usize) -> ScanResult {
    let mut i = skip_ws(s, i);
    match s.get(i) {
        Some('}') => return Ok(i + 1),
        Some('"') => {}
        _ => return Err(("Expecting property name enclosed in double quotes", i)),
    }
    loop {
        i = scan_string(s, i + 1)?;
        i = skip_ws(s, i);
        if s.get(i) != Some(&':') {
            return Err(("Expecting ':' delimiter", i));
        }
        i = skip_ws(s, i + 1);
        i = scan_value(s, i)?;
        i = skip_ws(s, i);
        match s.get(i) {
            Some('}') => return Ok(i + 1),
            Some(',') => {}
            _ => return Err(("Expecting ',' delimiter", i)),
        }
        let comma = i;
        i = skip_ws(s, i + 1);
        match s.get(i) {
            Some('"') => {}
            Some('}') => return Err(("Illegal trailing comma before end of object", comma)),
            _ => return Err(("Expecting property name enclosed in double quotes", i)),
        }
    }
}

fn scan_array(s: &[char], i: usize) -> ScanResult {
    let mut i = skip_ws(s, i);
    if s.get(i) == Some(&']') {
        return Ok(i + 1);
    }
    loop {
        i = scan_value(s, i)?;
        i = skip_ws(s, i);
        match s.get(i) {
            Some(']') => return Ok(i + 1),
            Some(',') => {}
            _ => return Err(("Expecting ',' delimiter", i)),
        }
        let comma = i;
        i = skip_ws(s, i + 1);
        if s.get(i) == Some(&']') {
            return Err(("Illegal trailing comma before end of array", comma));
        }
    }
}

// ---------------------------------------------------------------------------
// FastAPI validation (pydantic lax mode)

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Str,
    OptStr,
    Int,
    OptInt,
}

struct Field {
    name: &'static str,
    kind: Kind,
    default: Option<Arg>,
}

fn field(name: &'static str, kind: Kind, default: Option<Arg>) -> Field {
    Field {
        name,
        kind,
        default,
    }
}

/// The request model (POST body) or the query parameters (GET) of a route.
fn fields(route: &str, post: bool) -> Vec<Field> {
    let s = |v: &str| Some(Arg::Str(v.to_owned()));
    let max_results = if post { Kind::OptInt } else { Kind::Int };
    let base = |query_kind| {
        vec![
            field("query", query_kind, None),
            field("region", Kind::Str, s("us-en")),
            field("safesearch", Kind::Str, s("moderate")),
            field("timelimit", Kind::OptStr, Some(Arg::None)),
            field("max_results", max_results, Some(Arg::Int(10))),
            field("page", Kind::Int, Some(Arg::Int(1))),
            field("backend", Kind::Str, s("auto")),
        ]
    };
    match route {
        "text" | "news" => base(Kind::Str),
        "images" => {
            let mut f = base(Kind::Str);
            for name in ["size", "color", "type_image", "layout", "license_image"] {
                f.push(field(name, Kind::OptStr, Some(Arg::None)));
            }
            f
        }
        "videos" => {
            let mut f = base(Kind::Str);
            for name in ["resolution", "duration", "license_videos"] {
                f.push(field(name, Kind::OptStr, Some(Arg::None)));
            }
            f
        }
        "books" => vec![
            field("query", Kind::Str, None),
            field("max_results", max_results, Some(Arg::Int(10))),
            field("page", Kind::Int, Some(Arg::Int(1))),
            field("backend", Kind::Str, s("auto")),
        ],
        "extract" if post => vec![
            field("url", Kind::Str, None),
            field("format", Kind::Str, s("text_markdown")),
        ],
        "extract" => vec![
            field("url", Kind::Str, None),
            field("fmt", Kind::Str, s("text_markdown")),
        ],
        _ => Vec::new(),
    }
}

fn error_item(kind: &str, loc: Value, msg: &str, input: Value) -> Value {
    json!({"type": kind, "loc": loc, "msg": msg, "input": input})
}

/// pydantic lax `str → int`.
fn str_to_int(text: &str) -> Option<i128> {
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

fn convert(field: &Field, value: &Value, loc: Value) -> Result<Arg, Value> {
    let fail = |kind: &str, msg: &str| Err(error_item(kind, loc.clone(), msg, value.clone()));
    match (field.kind, value) {
        (Kind::OptStr | Kind::OptInt, Value::Null) => Ok(Arg::None),
        (Kind::Str | Kind::OptStr, Value::String(s)) => Ok(Arg::Str(s.clone())),
        (Kind::Str | Kind::OptStr, _) => fail("string_type", "Input should be a valid string"),
        (Kind::Int | Kind::OptInt, Value::Bool(b)) => Ok(Arg::Int(i128::from(*b))),
        (Kind::Int | Kind::OptInt, Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Ok(Arg::Int(i128::from(i)))
            } else if let Some(u) = n.as_u64() {
                Ok(Arg::Int(i128::from(u)))
            } else {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if f.is_finite() && f.fract() == 0.0 {
                    Ok(Arg::Int(f as i128))
                } else {
                    fail(
                        "int_from_float",
                        "Input should be a valid integer, got a number with a fractional part",
                    )
                }
            }
        }
        (Kind::Int | Kind::OptInt, Value::String(s)) => match str_to_int(s) {
            Some(n) => Ok(Arg::Int(n)),
            None => fail(
                "int_parsing",
                "Input should be a valid integer, unable to parse string as an integer",
            ),
        },
        (Kind::Int | Kind::OptInt, _) => fail("int_type", "Input should be a valid integer"),
    }
}

/// Validate a JSON body against the route's request model.
fn validate_body(route: &str, request: &Request) -> Result<HashMap<&'static str, Arg>, Response> {
    let unprocessable = |errors: Vec<Value>| Response::json(422, &json!({ "detail": errors }));
    if request.body.is_empty() {
        return Err(unprocessable(vec![error_item(
            "missing",
            json!(["body"]),
            "Field required",
            Value::Null,
        )]));
    }
    let text = String::from_utf8_lossy(&request.body).into_owned();
    let is_json = request.header("content-type").is_some_and(|ct| {
        let mime = ct
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        mime == "application/json" || (mime.starts_with("application/") && mime.ends_with("+json"))
    });
    let body: Value = if is_json {
        if let Some((message, position)) = python_json_error(&text) {
            return Err(unprocessable(vec![json!({
                "type": "json_invalid",
                "loc": ["body", position],
                "msg": "JSON decode error",
                "input": {},
                "ctx": {"error": message}
            })]));
        }
        serde_json::from_str(&text).unwrap_or(Value::Null)
    } else {
        Value::String(text)
    };
    let object = match &body {
        Value::Null => {
            return Err(unprocessable(vec![error_item(
                "missing",
                json!(["body"]),
                "Field required",
                Value::Null,
            )]));
        }
        Value::Object(map) => map.clone(),
        other => {
            return Err(unprocessable(vec![error_item(
                "model_attributes_type",
                json!(["body"]),
                "Input should be a valid dictionary or object to extract fields from",
                other.clone(),
            )]));
        }
    };
    let mut values = HashMap::new();
    let mut errors = Vec::new();
    for f in fields(route, true) {
        match object.get(f.name) {
            Some(value) => match convert(&f, value, json!(["body", f.name])) {
                Ok(arg) => {
                    values.insert(f.name, arg);
                }
                Err(e) => errors.push(e),
            },
            None => match f.default {
                Some(default) => {
                    values.insert(f.name, default);
                }
                None => errors.push(error_item(
                    "missing",
                    json!(["body", f.name]),
                    "Field required",
                    Value::Object(object.clone()),
                )),
            },
        }
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(unprocessable(errors))
    }
}

/// Validate the query parameters of a GET route.
fn validate_query(route: &str, request: &Request) -> Result<HashMap<&'static str, Arg>, Response> {
    let query = request.query();
    let mut values = HashMap::new();
    let mut errors = Vec::new();
    for f in fields(route, false) {
        match query.get(f.name) {
            Some(raw) => match convert(&f, &Value::String(raw.clone()), json!(["query", f.name])) {
                Ok(arg) => {
                    values.insert(f.name, arg);
                }
                Err(e) => errors.push(e),
            },
            None => match f.default {
                Some(default) => {
                    values.insert(f.name, default);
                }
                None => errors.push(error_item(
                    "missing",
                    json!(["query", f.name]),
                    "Field required",
                    Value::Null,
                )),
            },
        }
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(Response::json(422, &json!({ "detail": errors })))
    }
}

// ---------------------------------------------------------------------------
// Routing

/// `(path, methods)` in FastAPI's route order: the docs routes are added by
/// `FastAPI.setup()` first and also answer `HEAD`; every decorated endpoint
/// is its own route, so a method mismatch reports only the first route's
/// methods in `Allow` (Starlette's first partial match). Starlette joins a
/// route's method *set*, so upstream's order for `GET, HEAD` varies by hash
/// seed; this port uses `GET, HEAD`.
const ROUTES: &[(&str, &[&str])] = &[
    ("/openapi.json", &["GET", "HEAD"]),
    ("/docs", &["GET", "HEAD"]),
    ("/docs/oauth2-redirect", &["GET", "HEAD"]),
    ("/redoc", &["GET", "HEAD"]),
    ("/", &["GET"]),
    ("/health", &["GET"]),
    ("/search/text", &["POST"]),
    ("/search/text", &["GET"]),
    ("/search/images", &["POST"]),
    ("/search/images", &["GET"]),
    ("/search/news", &["POST"]),
    ("/search/news", &["GET"]),
    ("/search/videos", &["POST"]),
    ("/search/videos", &["GET"]),
    ("/search/books", &["POST"]),
    ("/search/books", &["GET"]),
    ("/extract", &["POST"]),
    ("/extract", &["GET"]),
];

fn health() -> Response {
    Response::json(
        200,
        &json!({"status": "healthy", "version": "1.0.0", "service": "DDGS API"}),
    )
}

/// `(log prefix, detail prefix)` per route.
fn labels(route: &str) -> (&'static str, &'static str) {
    match route {
        "text" => ("Error in text search", "Search failed"),
        "images" => ("Error in image search", "Image search failed"),
        "news" => ("Error in news search", "News search failed"),
        "videos" => ("Error in video search", "Video search failed"),
        "books" => ("Error in book search", "Book search failed"),
        _ => ("Error extracting content", "Extraction failed"),
    }
}

async fn run_route(
    runner: &dyn Runner,
    route: &str,
    post: bool,
    args: HashMap<&'static str, Arg>,
) -> Response {
    let (log, detail) = labels(route);
    let log = if post {
        log.to_owned()
    } else {
        format!("{log} (GET)")
    };
    let fail = |error: PyError| {
        log_warning(&format!("{log}: {}", error.message()));
        Response::detail(500, &format!("{detail}: {}", error.message()))
    };
    if route == "extract" {
        let url = match args.get("url") {
            Some(Arg::Str(s)) => s.clone(),
            _ => String::new(),
        };
        let fmt = match args.get(if post { "format" } else { "fmt" }) {
            Some(Arg::Str(s)) => s.clone(),
            _ => "text_markdown".to_owned(),
        };
        return match runner.extract(&url, &fmt).await {
            Ok(result) => {
                let content = match result.content {
                    ExtractContent::Text(text) => text,
                    // FastAPI's jsonable_encoder: `bytes.decode()`; invalid
                    // UTF-8 raises outside the handler's try → plain 500.
                    ExtractContent::Bytes(bytes) => match String::from_utf8(bytes) {
                        Ok(text) => text,
                        Err(_) => {
                            return Response::new(
                                500,
                                Some("text/plain; charset=utf-8"),
                                b"Internal Server Error".to_vec(),
                                Vec::new(),
                            );
                        }
                    },
                };
                Response::json(200, &json!({"url": result.url, "content": content}))
            }
            Err(error) => fail(error),
        };
    }
    match runner.search(&format!("search_{route}"), &args).await {
        Ok(rows) => Response::json(
            200,
            &json!({ "results": rows.into_iter().map(Value::Object).collect::<Vec<_>>() }),
        ),
        Err(error) => fail(error),
    }
}

/// The app without CORS: route, validate, run.
async fn app(runner: &dyn Runner, request: &Request) -> Response {
    let path = request.path();
    let full_match = ROUTES
        .iter()
        .find(|(p, m)| *p == path && m.contains(&request.method.as_str()));
    let Some((route_path, methods)) =
        full_match.or_else(|| ROUTES.iter().find(|(p, _)| *p == path))
    else {
        // Starlette `redirect_slashes`.
        let other = if path.ends_with('/') && path != "/" {
            path.trim_end_matches('/').to_owned()
        } else {
            format!("{path}/")
        };
        if ROUTES.iter().any(|(p, _)| *p == other) {
            let host = request.header("host").unwrap_or("127.0.0.1");
            let query = request
                .target
                .split_once('?')
                .map(|(_, q)| format!("?{q}"))
                .unwrap_or_default();
            return Response {
                status: 307,
                headers: vec![
                    ("content-length".to_owned(), "0".to_owned()),
                    (
                        "location".to_owned(),
                        format!("http://{host}{other}{query}"),
                    ),
                ],
                body: Vec::new(),
            };
        }
        return Response::detail(404, "Not Found");
    };
    if !methods.contains(&request.method.as_str()) {
        let mut response = Response::detail(405, "Method Not Allowed");
        response
            .headers
            .insert(0, ("allow".to_owned(), methods.join(", ")));
        return response;
    }
    let head = request.method == "HEAD";
    let page = |content_type: &str, body: &str| {
        let mut r = Response::new(
            200,
            Some(content_type),
            body.as_bytes().to_vec(),
            Vec::new(),
        );
        if head {
            r.body.clear();
        }
        r
    };
    match *route_path {
        "/openapi.json" => page(JSON, OPENAPI_JSON),
        "/docs" => page(HTML, DOCS_HTML),
        "/docs/oauth2-redirect" => page(HTML, OAUTH2_REDIRECT_HTML),
        "/redoc" => page(HTML, REDOC_HTML),
        "/" | "/health" => health(),
        other => {
            let route = other.trim_start_matches("/search/").trim_start_matches('/');
            let post = request.method == "POST";
            let args = if post {
                validate_body(route, request)
            } else {
                validate_query(route, request)
            };
            match args {
                Ok(args) => run_route(runner, route, post, args).await,
                Err(response) => response,
            }
        }
    }
}

/// Starlette's `CORSMiddleware(allow_origins=["*"], allow_credentials=True,
/// allow_methods=["*"], allow_headers=["*"])` around the app.
pub(crate) async fn handle(runner: &dyn Runner, request: &Request) -> Response {
    let origin = request.header("origin").map(str::to_owned);
    if request.method == "OPTIONS"
        && let Some(origin) = &origin
        && request.header("access-control-request-method").is_some()
    {
        let mut headers = vec![
            (
                "vary".to_owned(),
                "Origin, Access-Control-Request-Method, Access-Control-Request-Headers, Access-Control-Request-Private-Network"
                    .to_owned(),
            ),
            (
                "access-control-allow-methods".to_owned(),
                "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT, QUERY".to_owned(),
            ),
            ("access-control-max-age".to_owned(), "600".to_owned()),
            ("access-control-allow-credentials".to_owned(), "true".to_owned()),
            ("access-control-allow-origin".to_owned(), origin.clone()),
        ];
        if let Some(requested) = request.header("access-control-request-headers") {
            headers.push((
                "access-control-allow-headers".to_owned(),
                requested.to_owned(),
            ));
        }
        return Response::new(
            200,
            Some("text/plain; charset=utf-8"),
            b"OK".to_vec(),
            headers,
        );
    }
    let mut response = app(runner, request).await;
    if let Some(origin) = origin {
        response
            .headers
            .push(("access-control-allow-origin".to_owned(), origin));
        response.headers.push((
            "access-control-allow-credentials".to_owned(),
            "true".to_owned(),
        ));
    }
    response
        .headers
        .push(("vary".to_owned(), "Origin".to_owned()));
    response
}

// ---------------------------------------------------------------------------
// HTTP/1.1 server (uvicorn's role)

fn status_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        307 => "Temporary Redirect",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Content Too Large",
        422 => "Unprocessable Content",
        500 => "Internal Server Error",
        _ => "",
    }
}

/// uvicorn's `levelprefix`: the level name, colored on a terminal, padded to 8.
fn level_prefix(level: &str, tty: bool) -> String {
    let pad = " ".repeat(8usize.saturating_sub(level.len()));
    let name = if tty {
        let color = match level {
            "INFO" => "32",
            "WARNING" => "33",
            "ERROR" => "31",
            _ => "36",
        };
        format!("\x1b[{color}m{level}\x1b[0m")
    } else {
        level.to_owned()
    };
    format!("{name}:{pad}")
}

/// uvicorn's error logger (stderr): `"%(levelprefix)s %(message)s"`.
fn log_info(message: &str) {
    eprintln!(
        "{} {message}",
        level_prefix("INFO", std::io::stderr().is_terminal())
    );
}

fn log_error(message: &str) {
    eprintln!(
        "{} {message}",
        level_prefix("ERROR", std::io::stderr().is_terminal())
    );
}

/// `logger.warning(...)` in `api.py` (root `basicConfig` format).
fn log_warning(message: &str) {
    eprintln!("WARNING:ddgs.api_server.api:{message}");
}

/// uvicorn's access logger (stdout).
fn log_access(peer: &SocketAddr, request: &Request, version: &str, status: u16) {
    let tty = std::io::stdout().is_terminal();
    let line = format!("{} {} HTTP/{version}", request.method, request.target);
    let line = if tty {
        format!("\x1b[1m{line}\x1b[0m")
    } else {
        line
    };
    let status = format!("{status} {}", status_phrase(status));
    println!(
        "{} {}:{} - \"{line}\" {status}",
        level_prefix("INFO", tty),
        peer.ip(),
        peer.port()
    );
}

/// RFC 7231 date, as uvicorn sends it.
fn http_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = crate::normalize::civil_from_days(days);
    let weekday = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let month = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m as usize).saturating_sub(1).min(11)];
    format!(
        "{weekday}, {d:02} {month} {y:04} {:02}:{:02}:{:02} GMT",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Read one request; `None` at end of stream.
async fn read_request(
    stream: &mut TcpStream,
    buffer: &mut Vec<u8>,
) -> std::io::Result<Option<(Request, String, bool)>> {
    loop {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut parsed = httparse::Request::new(&mut headers);
        if let Ok(httparse::Status::Complete(head_len)) = parsed.parse(buffer) {
            let method = parsed.method.unwrap_or("GET").to_owned();
            let target = parsed.path.unwrap_or("/").to_owned();
            let version = if parsed.version == Some(0) {
                "1.0"
            } else {
                "1.1"
            };
            let headers: Vec<(String, String)> = parsed
                .headers
                .iter()
                .map(|h| {
                    (
                        h.name.to_ascii_lowercase(),
                        String::from_utf8_lossy(h.value).into_owned(),
                    )
                })
                .collect();
            let find = |name: &str| {
                headers
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.clone())
            };
            let keep_alive = match find("connection").map(|c| c.to_ascii_lowercase()) {
                Some(c) if c.contains("close") => false,
                Some(c) if c.contains("keep-alive") => true,
                _ => version == "1.1",
            };
            buffer.drain(..head_len);
            let chunked = find("transfer-encoding")
                .is_some_and(|t| t.to_ascii_lowercase().contains("chunked"));
            let body = if chunked {
                read_chunked(stream, buffer).await?
            } else {
                let length: usize = find("content-length")
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                while buffer.len() < length {
                    let mut chunk = [0u8; 8192];
                    let n = stream.read(&mut chunk).await?;
                    if n == 0 {
                        return Ok(None);
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                buffer.drain(..length).collect()
            };
            return Ok(Some((
                Request {
                    method,
                    target,
                    headers,
                    body,
                },
                version.to_owned(),
                keep_alive,
            )));
        }
        let mut chunk = [0u8; 8192];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk[..n]);
        if buffer.len() > 1 << 20 {
            return Ok(None);
        }
    }
}

async fn read_chunked(stream: &mut TcpStream, buffer: &mut Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        while !buffer.windows(2).any(|w| w == b"\r\n") {
            let mut chunk = [0u8; 8192];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(body);
            }
            buffer.extend_from_slice(&chunk[..n]);
        }
        let line_end = buffer.windows(2).position(|w| w == b"\r\n").unwrap_or(0);
        let size_line = String::from_utf8_lossy(&buffer[..line_end]).into_owned();
        let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        buffer.drain(..line_end + 2);
        while buffer.len() < size + 2 {
            let mut chunk = [0u8; 8192];
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                return Ok(body);
            }
            buffer.extend_from_slice(&chunk[..n]);
        }
        if size == 0 {
            buffer.drain(..2.min(buffer.len()));
            return Ok(body);
        }
        body.extend(buffer.drain(..size));
        buffer.drain(..2);
    }
}

async fn serve_connection(runner: Arc<dyn Runner>, mut stream: TcpStream, peer: SocketAddr) {
    let mut buffer = Vec::new();
    while let Ok(Some((request, version, keep_alive))) =
        read_request(&mut stream, &mut buffer).await
    {
        let response = handle(runner.as_ref(), &request).await;
        log_access(&peer, &request, &version, response.status);
        let mut head = format!(
            "HTTP/1.1 {} {}\r\ndate: {}\r\nserver: uvicorn\r\n",
            response.status,
            status_phrase(response.status),
            http_date()
        );
        for (name, value) in &response.headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if !keep_alive {
            head.push_str("connection: close\r\n");
        }
        head.push_str("\r\n");
        let mut bytes = head.into_bytes();
        if request.method != "HEAD" {
            bytes.extend_from_slice(&response.body);
        }
        if stream.write_all(&bytes).await.is_err() || !keep_alive {
            break;
        }
    }
}

/// `uvicorn.run(app, host, port, log_level="info")`.
pub(crate) async fn serve(runner: Arc<dyn Runner>, host: &str, port: u16) -> Result<(), String> {
    let pid = std::process::id();
    log_info(&format!("Started server process [{pid}]"));
    log_info("Waiting for application startup.");
    log_info("Application startup complete.");
    let listener = match TcpListener::bind((host, port)).await {
        Ok(listener) => listener,
        Err(error) => {
            let errno = error.raw_os_error().unwrap_or(0);
            let text = std::io::Error::from_raw_os_error(errno).to_string();
            let text = text
                .split(" (os error")
                .next()
                .unwrap_or(&text)
                .to_lowercase();
            log_error(&format!(
                "[Errno {errno}] error while attempting to bind on address ('{host}', {port}): {text}"
            ));
            log_info("Waiting for application shutdown.");
            log_info("Application shutdown complete.");
            return Err(text);
        }
    };
    log_info(&format!(
        "Uvicorn running on http://{host}:{port} (Press CTRL+C to quit)"
    ));
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, peer)) = accepted {
                    tokio::spawn(serve_connection(runner.clone(), stream, peer));
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    log_info("Shutting down");
    log_info("Waiting for application shutdown.");
    log_info("Application shutdown complete.");
    log_info(&format!("Finished server process [{pid}]"));
    Ok(())
}

// ---------------------------------------------------------------------------
// The `api` CLI command

/// `Path.home() / ".cache" / "ddgs" / "api.pid"`.
fn pid_file() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    home.join(".cache").join("ddgs").join("api.pid")
}

fn opt_str<'a>(values: &'a HashMap<&'static str, CliValue>, key: &str) -> Option<&'a str> {
    values.get(key).and_then(CliValue::as_str)
}

/// The `api` CLI command (upstream `cli.api`). `--reload` is accepted and has
/// no effect (it restarts uvicorn on Python source changes).
pub(crate) async fn cli_api(values: &HashMap<&'static str, CliValue>) -> Result<(), PyError> {
    use crate::cli::output::{echo, echo_err};
    let path = pid_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| PyError::os(&e, ""))?;
    }
    if values.get("stop").is_some_and(CliValue::as_bool) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            echo_err("No detached server is running (PID file not found)");
            return Ok(());
        };
        let trimmed = text.trim();
        let pid: i32 = trimmed.parse().map_err(|_| {
            PyError::new(
                "ValueError",
                &format!(
                    "invalid literal for int() with base 10: {}",
                    crate::engines::util::py_repr(trimmed)
                ),
            )
        })?;
        stop_process(pid);
        let _ = std::fs::remove_file(&path);
        return Ok(());
    }
    let host = opt_str(values, "host").unwrap_or("0.0.0.0").to_owned();
    let port = values
        .get("port")
        .and_then(CliValue::as_int)
        .unwrap_or(4479);
    let proxy = opt_str(values, "proxy").filter(|p| !p.is_empty());
    let proxy_env =
        proxy.map(|p| crate::http::resolve_proxy(Some(p)).unwrap_or_else(|| p.to_owned()));
    if values.get("detach").is_some_and(CliValue::as_bool) {
        let exe = std::env::current_exe().map_err(|e| PyError::os(&e, ""))?;
        let mut command = std::process::Command::new(exe);
        command
            .args(["api", "--host", &host, "--port", &port.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(p) = &proxy_env {
            command.env(crate::http::PROXY_ENV_VAR, p);
        }
        let mut child = command.spawn().map_err(|e| PyError::os(&e, ""))?;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if let Ok(Some(status)) = child.try_wait() {
            echo_err(&format!(
                "Failed to start server: process exited with code {}",
                status.code().unwrap_or(-1)
            ));
            return Ok(());
        }
        std::fs::write(&path, child.id().to_string()).map_err(|e| PyError::os(&e, ""))?;
        echo(&format!(
            "DDGS API server started in detached mode on http://{host}:{port} (PID: {})",
            child.id()
        ));
        if let Some(p) = &proxy_env {
            echo(&format!("Using proxy: {p}"));
        }
        return Ok(());
    }
    echo(&format!("Starting DDGS API server on http://{host}:{port}"));
    if let Some(p) = &proxy_env {
        echo(&format!("Using proxy: {p}"));
        // SAFETY (Rust 2024 `set_var`): set before the server starts tasks.
        unsafe { std::env::set_var(crate::http::PROXY_ENV_VAR, p) };
    }
    echo("Press Ctrl+C to stop");
    let port =
        u16::try_from(port).map_err(|_| PyError::new("OverflowError", "port must be 0-65535."))?;
    if serve(Arc::new(SearchRunner), &host, port).await.is_err() {
        // uvicorn exits with status 1 when it cannot bind.
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(unix)]
fn stop_process(pid: i32) {
    use crate::cli::output::{echo, echo_err};
    // SAFETY: kill(2) with a plain pid and SIGTERM has no memory effects.
    let result = unsafe { libc::kill(pid, libc::SIGTERM) };
    if result == 0 {
        echo(&format!("DDGS API server stopped (PID: {pid})"));
        return;
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        echo(&format!(
            "Server process (PID: {pid}) was not running, cleaning up PID file"
        ));
    } else {
        echo_err(&format!(
            "Failed to stop server: {}",
            PyError::os(&error, "").message()
        ));
    }
}

#[cfg(not(unix))]
fn stop_process(pid: i32) {
    crate::cli::output::echo_err(&format!(
        "Failed to stop server: cannot signal PID {pid} on this platform"
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::Map;

    /// The same stub as the MCP test: fixture rows for searches, fixture
    /// files for `/extract` (404 when missing).
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
            let query = match args.get("query") {
                Some(Arg::Str(s)) => s.clone(),
                _ => String::new(),
            };
            if query.is_empty() {
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
            Ok(crate::ExtractResult {
                url: url.to_owned(),
                content: crate::extract::convert(&response, fmt.parse().unwrap())
                    .map_err(PyError::from)?,
            })
        }
    }

    /// Every request of the upstream recording gets the same status,
    /// headers (uvicorn's `server` aside, which the transport adds) and body.
    #[tokio::test]
    async fn matches_upstream_api_reference() {
        let reference: Vec<Value> =
            serde_json::from_str(include_str!("../../fixtures/api/reference.json")).unwrap();
        let runner = StubRunner {
            rows: serde_json::from_str(include_str!("../../fixtures/mcp/search_rows.json"))
                .unwrap(),
        };
        assert!(reference.len() >= 50);
        for case in &reference {
            let method = case["method"].as_str().unwrap().to_owned();
            let target = case["path"].as_str().unwrap().to_owned();
            let mut headers: Vec<(String, String)> = case["headers"]
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.to_ascii_lowercase(), v.as_str().unwrap().to_owned()))
                .collect();
            let body = match &case["body"] {
                Value::Null => Vec::new(),
                Value::String(s) => s.clone().into_bytes(),
                other => {
                    if !headers.iter().any(|(k, _)| k == "content-type") {
                        headers.push(("content-type".to_owned(), "application/json".to_owned()));
                    }
                    other.to_string().into_bytes()
                }
            };
            if !headers.iter().any(|(k, _)| k == "host") {
                headers.push(("host".to_owned(), "127.0.0.1".to_owned()));
            }
            let request = Request {
                method: method.clone(),
                target: target.clone(),
                headers,
                body,
            };
            let response = handle(&runner, &request).await;
            let expected_body = case["response"].as_str().unwrap();
            let expected_headers: Vec<(String, String)> = case["response_headers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|h| {
                    (
                        h[0].as_str().unwrap().to_owned(),
                        h[1].as_str().unwrap().to_owned(),
                    )
                })
                .filter(|(k, _)| k != "server")
                // The recording stores the localhost port as "PORT"; the
                // length upstream sent counted the real port digits.
                .map(|(k, v)| {
                    if k == "content-length" && expected_body.contains("127.0.0.1:PORT") {
                        (k, expected_body.len().to_string())
                    } else {
                        (k, v)
                    }
                })
                .collect();
            let body = if method == "HEAD" {
                String::new()
            } else {
                String::from_utf8(response.body.clone()).unwrap()
            };
            assert_eq!(
                response.status,
                case["status"].as_u64().unwrap() as u16,
                "{method} {target}"
            );
            assert_eq!(response.headers, expected_headers, "{method} {target}");
            assert_eq!(body, expected_body, "{method} {target}");
        }
    }

    #[test]
    fn python_json_errors() {
        assert_eq!(python_json_error("not json"), Some(("Expecting value", 0)));
        assert_eq!(
            python_json_error("{\"a\": 1,}"),
            Some(("Illegal trailing comma before end of object", 7))
        );
        assert_eq!(
            python_json_error("[1,]"),
            Some(("Illegal trailing comma before end of array", 2))
        );
        assert_eq!(
            python_json_error("{a}"),
            Some(("Expecting property name enclosed in double quotes", 1))
        );
        assert_eq!(
            python_json_error("{\"a\" 1}"),
            Some(("Expecting ':' delimiter", 5))
        );
        assert_eq!(
            python_json_error("\"abc"),
            Some(("Unterminated string starting at", 0))
        );
        assert_eq!(python_json_error("{\"a\": 1} x"), Some(("Extra data", 9)));
        assert_eq!(python_json_error(" {\"a\": [1, 2.5e3, null, true]} "), None);
    }
}
