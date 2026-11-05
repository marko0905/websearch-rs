//! The `websearch` command line — a 1:1 port of the upstream `ddgs` CLI
//! (`ddgs/cli.py`, click 8.5): a chained command group with `version`,
//! `text`, `images`, `videos`, `news`, `books`, `extract`, `mcp` and `api`.
//!
//! Like upstream's `safe_entry_point`, an exception while a command runs is
//! printed to stdout as `ExceptionName: ExceptionName('message')` and the
//! exit code stays 0; click usage errors go to stderr with exit code 2.

pub(crate) mod click;
pub(crate) mod output;
pub(crate) mod py;

use std::collections::HashMap;

use click::{CommandSpec, Default as D, Dispatch, OptSpec, ParamType as T, Value};
use output::Row;

use crate::{
    BooksSearchRequest, ImageSearchRequest, NewsSearchRequest, Region, SafeSearch,
    TextSearchRequest, TimeLimit, Verify, VideoSearchRequest, WebSearchClient, WebSearchError,
};

/// The crate version, which equals the tracked Python `ddgs` version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A Python exception as `safe_entry_point` prints it:
/// `f"{type(ex).__name__}: {ex!r}"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyError {
    name: String,
    /// The `repr()` of the exception arguments, already joined.
    args: String,
    /// `str(exception)`.
    message: String,
}

impl PyError {
    pub(crate) fn new(name: &str, message: &str) -> Self {
        Self {
            name: name.to_owned(),
            args: crate::engines::util::py_repr(message),
            message: message.to_owned(),
        }
    }

    pub(crate) fn bare(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            args: String::new(),
            message: String::new(),
        }
    }

    /// `str(exception)`.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// `OSError` subclasses: `FileNotFoundError(2, 'No such file or directory')`.
    pub(crate) fn os(error: &std::io::Error, _path: &str) -> Self {
        let code = error.raw_os_error();
        let name = match error.kind() {
            std::io::ErrorKind::NotFound => "FileNotFoundError",
            std::io::ErrorKind::PermissionDenied => "PermissionError",
            std::io::ErrorKind::AlreadyExists => "FileExistsError",
            std::io::ErrorKind::IsADirectory => "IsADirectoryError",
            std::io::ErrorKind::NotADirectory => "NotADirectoryError",
            _ => "OSError",
        };
        let strerror = match code {
            Some(code) => {
                let text = std::io::Error::from_raw_os_error(code).to_string();
                text.split(" (os error").next().unwrap_or(&text).to_owned()
            }
            None => error.to_string(),
        };
        Self {
            name: name.to_owned(),
            args: format!(
                "{}, {}",
                code.unwrap_or(0),
                crate::engines::util::py_repr(&strerror)
            ),
            message: format!("[Errno {}] {strerror}", code.unwrap_or(0)),
        }
    }

    /// The line `safe_entry_point` prints.
    pub fn render(&self) -> String {
        format!("{}: {}({})", self.name, self.name, self.args)
    }
}

impl From<WebSearchError> for PyError {
    fn from(error: WebSearchError) -> Self {
        match error {
            WebSearchError::Timeout(message) => Self::new("TimeoutException", &message),
            WebSearchError::NoResults => Self::new("DDGSException", "No results found."),
            WebSearchError::InvalidConfig(message) => Self::new("DDGSException", &message),
            WebSearchError::Internal(message) => Self::new("DDGSException", &message),
            other => Self::new("DDGSException", &other.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Command specs (copied from `ddgs/cli.py`, docstrings after inspect.cleandoc)

const GROUP_HELP: &str = "DDGS CLI tool.";
const SAFESEARCH: &[&str] = &["on", "moderate", "off"];
const TIMELIMIT: &[&str] = &["d", "w", "m", "y"];
const PROXY_HELP: &str = "the proxy to send requests, example: socks5h://127.0.0.1:9150";
const OUTPUT_HELP: &str = "csv, json or filename.csv|json (save the results to a csv or json file)";

fn query_opts(kind: &'static str) -> [OptSpec; 2] {
    let (q, k): (&'static str, &'static str) = match kind {
        "text" => ("text search query", "(Deprecated) text search query"),
        "images" => ("images search query", "(Deprecated) images search query"),
        "videos" => ("videos search query", "(Deprecated) videos search query"),
        "news" => ("news search query", "(Deprecated) news search query"),
        _ => ("books search query", "(Deprecated) books search query"),
    };
    [
        OptSpec::value("query", &["-q", "--query"], T::Text, D::None, q),
        OptSpec::value("keywords", &["-k", "--keywords"], T::Text, D::None, k),
    ]
}

fn region_opts(timelimit: &'static [&'static str], timelimit_help: &'static str) -> [OptSpec; 3] {
    [
        OptSpec::value(
            "region",
            &["-r", "--region"],
            T::Text,
            D::Str("us-en"),
            "us-en, ru-ru, etc.",
        ),
        OptSpec::value(
            "safesearch",
            &["-s", "--safesearch"],
            T::Choice(SAFESEARCH),
            D::Str("moderate"),
            "",
        ),
        OptSpec::value(
            "timelimit",
            &["-t", "--timelimit"],
            T::Choice(timelimit),
            D::None,
            timelimit_help,
        ),
    ]
}

fn paging_opts(backends: &'static [&'static str]) -> [OptSpec; 3] {
    [
        OptSpec::value(
            "max_results",
            &["-m", "--max_results"],
            T::Int,
            D::Int(10),
            "maximum number of results",
        ),
        OptSpec::value(
            "page",
            &["-p", "--page"],
            T::Int,
            D::Int(1),
            "page number of results",
        ),
        OptSpec::value(
            "backend",
            &["-b", "--backend"],
            T::Choice(backends),
            D::List(&["auto"]),
            "",
        )
        .multiple(),
    ]
}

fn output_opt() -> OptSpec {
    OptSpec::value("output", &["-o", "--output"], T::Text, D::None, OUTPUT_HELP)
}

fn download_opts() -> [OptSpec; 3] {
    [
        OptSpec::flag(
            "download",
            &["-d", "--download"],
            "download results. -dd to set custom directory",
        ),
        OptSpec::value(
            "download_directory",
            &["-dd", "--download-directory"],
            T::Text,
            D::None,
            "Specify custom download directory",
        ),
        OptSpec::value(
            "threads",
            &["-th", "--threads"],
            T::Int,
            D::Int(10),
            "download threads, default=10",
        ),
    ]
}

fn net_opts(no_color: bool) -> Vec<OptSpec> {
    let mut opts = vec![
        OptSpec::value("proxy", &["-pr", "--proxy"], T::Text, D::None, PROXY_HELP),
        OptSpec::value(
            "verify",
            &["-v", "--verify"],
            T::Bool,
            D::Bool(true),
            "verify SSL when making the request",
        ),
    ];
    if no_color {
        opts.push(OptSpec::flag(
            "no_color",
            &["-nc", "--no-color"],
            "disable color output",
        ));
    }
    opts
}

pub(crate) fn commands() -> Vec<CommandSpec> {
    let mut text = Vec::new();
    text.extend(query_opts("text"));
    text.extend(region_opts(TIMELIMIT, "day, week, month, year"));
    text.extend(paging_opts(&[
        "auto",
        "all",
        "bing",
        "brave",
        "duckduckgo",
        "google",
        "grokipedia",
        "mojeek",
        "startpage",
        "yandex",
        "yahoo",
        "wikipedia",
    ]));
    text.push(output_opt());
    text.extend(download_opts());
    text.extend(net_opts(true));

    let mut images = Vec::new();
    images.extend(query_opts("images"));
    images.extend(region_opts(TIMELIMIT, ""));
    images.extend(paging_opts(&["auto", "all", "bing", "duckduckgo"]));
    images.push(OptSpec::value(
        "size",
        &["-size", "--size"],
        T::Choice(&["Small", "Medium", "Large", "Wallpaper"]),
        D::None,
        "",
    ));
    images.push(OptSpec::value(
        "color",
        &["-c", "--color"],
        T::Choice(&[
            "color",
            "Monochrome",
            "Red",
            "Orange",
            "Yellow",
            "Green",
            "Blue",
            "Purple",
            "Pink",
            "Brown",
            "Black",
            "Gray",
            "Teal",
            "White",
        ]),
        D::None,
        "",
    ));
    images.push(OptSpec::value(
        "type_image",
        &["-type", "--type_image"],
        T::Choice(&["photo", "clipart", "gif", "transparent", "line"]),
        D::None,
        "",
    ));
    images.push(OptSpec::value(
        "layout",
        &["-l", "--layout"],
        T::Choice(&["Square", "Tall", "Wide"]),
        D::None,
        "",
    ));
    images.push(OptSpec::value(
        "license_image",
        &["-lic", "--license_image"],
        T::Choice(&[
            "any",
            "Public",
            "Share",
            "ShareCommercially",
            "Modify",
            "ModifyCommercially",
        ]),
        D::None,
        "",
    ));
    images.push(output_opt());
    images.extend(download_opts());
    images.extend(net_opts(true));

    let mut videos = Vec::new();
    videos.extend(query_opts("videos"));
    videos.extend(region_opts(&["d", "w", "m"], "day, week, month"));
    videos.extend(paging_opts(&["auto", "all", "duckduckgo"]));
    videos.push(OptSpec::value(
        "resolution",
        &["-res", "--resolution"],
        T::Choice(&["high", "standart"]),
        D::None,
        "",
    ));
    videos.push(OptSpec::value(
        "duration",
        &["-d", "--duration"],
        T::Choice(&["short", "medium", "long"]),
        D::None,
        "",
    ));
    videos.push(OptSpec::value(
        "license_videos",
        &["-lic", "--license_videos"],
        T::Choice(&["creativeCommon", "youtube"]),
        D::None,
        "",
    ));
    videos.push(output_opt());
    videos.extend(net_opts(true));

    let mut news = Vec::new();
    news.extend(query_opts("news"));
    news.extend(region_opts(TIMELIMIT, "day, week, month, year"));
    news.extend(paging_opts(&["auto", "all", "bing", "duckduckgo", "yahoo"]));
    news.push(output_opt());
    news.extend(net_opts(true));

    let mut books = Vec::new();
    books.extend(query_opts("books"));
    books.extend(paging_opts(&["auto", "all", "annasarchive"]));
    books.push(output_opt());
    books.extend(net_opts(true));

    let mut extract = vec![
        OptSpec::value(
            "url",
            &["-u", "--url"],
            T::Text,
            D::None,
            "URL to extract content from",
        )
        .required(),
        OptSpec::value(
            "fmt",
            &["-f", "--format"],
            T::Choice(&[
                "text_markdown",
                "text_plain",
                "text_rich",
                "text",
                "content",
            ]),
            D::Str("text_markdown"),
            "Output format",
        ),
        OptSpec::value(
            "output",
            &["-o", "--output"],
            T::Text,
            D::None,
            "json or filename.json (save the results to a file)",
        ),
    ];
    extract.extend(net_opts(false));

    vec![
        CommandSpec {
            name: "version",
            help: "Print and return version.",
            options: Vec::new(),
        },
        CommandSpec {
            name: "text",
            help: "CLI function to perform a DDGS text metasearch.",
            options: text,
        },
        CommandSpec {
            name: "images",
            help: "CLI function to perform a DDGS images metasearch.",
            options: images,
        },
        CommandSpec {
            name: "videos",
            help: "CLI function to perform a DDGS videos metasearch.",
            options: videos,
        },
        CommandSpec {
            name: "news",
            help: "CLI function to perform a DDGS news metasearch.",
            options: news,
        },
        CommandSpec {
            name: "books",
            help: "CLI function to perform a DDGS books metasearch.",
            options: books,
        },
        CommandSpec {
            name: "extract",
            help: "CLI function to extract content from a URL.",
            options: extract,
        },
        CommandSpec {
            name: "mcp",
            help: MCP_HELP,
            options: vec![OptSpec::value(
                "proxy",
                &["-pr", "--proxy"],
                T::Text,
                D::None,
                PROXY_HELP,
            )],
        },
        CommandSpec {
            name: "api",
            help: API_HELP,
            options: vec![
                OptSpec::flag(
                    "detach",
                    &["-d", "--detach"],
                    "Run the server in detached mode (background)",
                ),
                OptSpec::flag("stop", &["-s", "--stop"], "Stop the detached server"),
                OptSpec::value(
                    "host",
                    &["--host"],
                    T::Text,
                    D::Str("0.0.0.0"),
                    "Host to bind the server to",
                ),
                OptSpec::value(
                    "port",
                    &["--port"],
                    T::Int,
                    D::Int(4479),
                    "Port to bind the server to",
                ),
                OptSpec::flag(
                    "reload",
                    &["--reload"],
                    "Enable auto-reload on code changes",
                ),
                OptSpec::value("proxy", &["-pr", "--proxy"], T::Text, D::None, PROXY_HELP),
            ],
        },
    ]
}

const MCP_HELP: &str = "Start DDGS MCP server over stdio for local MCP clients.

Examples:
    ddgs mcp                            # Start MCP server using stdio transport
    ddgs mcp -pr socks5h://127.0.0.1:9150  # With proxy

MCP client configuration:
    {
      \"mcpServers\": {
        \"ddgs\": {
          \"command\": \"ddgs\",
          \"args\": [\"mcp\"]
        }
      }
    }";

const API_HELP: &str = "Start/stop the DDGS API server.

Starts a FastAPI server with REST endpoints for search tools.
Supports text, image, news, video, and book search.

Examples:
    ddgs api              # Start server in foreground
    ddgs api -d           # Start server in detached mode
    ddgs api -s           # Stop the detached server
    ddgs api --host 127.0.0.1 --port 9000  # Bind to specific host/port
    ddgs api -pr socks5h://127.0.0.1:9150  # Use proxy";

// ---------------------------------------------------------------------------
// Logging: `logging.basicConfig(level=logging.WARNING)` on stderr.

struct PyLogging;

struct MessageVisitor(String);

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for PyLogging {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::WARN
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = MessageVisitor(String::new());
        event.record(&mut visitor);
        let level = match *event.metadata().level() {
            tracing::Level::ERROR => "ERROR",
            _ => "WARNING",
        };
        eprintln!("{level}:{}:{}", event.metadata().target(), visitor.0);
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

// ---------------------------------------------------------------------------
// Running commands

fn opt_str<'a>(values: &'a HashMap<&'static str, Value>, key: &str) -> Option<&'a str> {
    values.get(key).and_then(Value::as_str)
}

fn opt_int(values: &HashMap<&'static str, Value>, key: &str) -> i64 {
    values.get(key).and_then(Value::as_int).unwrap_or(0)
}

fn opt_bool(values: &HashMap<&'static str, Value>, key: &str) -> bool {
    values.get(key).is_some_and(Value::as_bool)
}

fn backend(values: &HashMap<&'static str, Value>) -> String {
    match values.get("backend") {
        Some(Value::List(items)) => items.join(","),
        _ => String::new(),
    }
}

/// `DDGS(proxy=_expand_proxy_tb_alias(proxy), verify=verify)`.
fn client(values: &HashMap<&'static str, Value>) -> Result<WebSearchClient, PyError> {
    let verify = values.get("verify").map(Value::as_bool).unwrap_or(true);
    let mut builder = WebSearchClient::builder().verify(if verify {
        Verify::Enabled
    } else {
        Verify::Disabled
    });
    if let Some(proxy) = opt_str(values, "proxy") {
        builder = builder.proxy(proxy);
    }
    builder.build().map_err(PyError::from)
}

/// `query = keywords or query` (an empty string is falsy).
fn effective_query(values: &HashMap<&'static str, Value>) -> String {
    opt_str(values, "keywords")
        .filter(|k| !k.is_empty())
        .or_else(|| opt_str(values, "query"))
        .unwrap_or("")
        .to_owned()
}

/// Upstream passes `max_results` straight to `DDGS`: `0` (falsy) means all.
fn max_results(values: &HashMap<&'static str, Value>) -> Option<usize> {
    usize::try_from(opt_int(values, "max_results"))
        .ok()
        .filter(|n| *n > 0)
}

fn page(values: &HashMap<&'static str, Value>) -> usize {
    usize::try_from(opt_int(values, "page")).unwrap_or(0)
}

fn region(values: &HashMap<&'static str, Value>) -> Result<Region, PyError> {
    Region::new(opt_str(values, "region").unwrap_or("us-en")).map_err(PyError::from)
}

fn safesearch(values: &HashMap<&'static str, Value>) -> SafeSearch {
    opt_str(values, "safesearch")
        .and_then(|s| s.parse().ok())
        .unwrap_or_default()
}

fn timelimit(values: &HashMap<&'static str, Value>) -> Option<TimeLimit> {
    opt_str(values, "timelimit").and_then(|s| s.parse().ok())
}

fn to_rows<R: serde::Serialize>(results: &[R]) -> Result<Vec<Row>, PyError> {
    results
        .iter()
        .map(|r| match serde_json::to_value(r) {
            Ok(serde_json::Value::Object(map)) => Ok(map),
            Ok(_) => Ok(Row::new()),
            Err(e) => Err(PyError::new("TypeError", &e.to_string())),
        })
        .collect()
}

/// After the search: save, download, or print (text and images).
async fn finish_with_download(
    name: &str,
    values: &HashMap<&'static str, Value>,
    data: &[Row],
) -> Result<(), PyError> {
    let query = output::sanitize_query(&effective_query(values));
    let out = opt_str(values, "output").filter(|o| !o.is_empty());
    let download = opt_bool(values, "download");
    if let Some(out) = out {
        output::save_data(&query, data, name, Some(out))?;
    }
    if download {
        output::download_results(
            &query,
            data,
            name,
            opt_str(values, "proxy"),
            opt_int(values, "threads"),
            opt_str(values, "download_directory").filter(|d| !d.is_empty()),
            values.get("verify").map(Value::as_bool).unwrap_or(true),
        )
        .await?;
    }
    if out.is_none() && !download {
        output::print_data(data, opt_bool(values, "no_color"))?;
    }
    Ok(())
}

/// After the search: save or print (videos, news, books).
fn finish_simple(
    name: &str,
    query: &str,
    values: &HashMap<&'static str, Value>,
    data: &[Row],
) -> Result<(), PyError> {
    match opt_str(values, "output").filter(|o| !o.is_empty()) {
        Some(out) => output::save_data(query, data, name, Some(out)),
        None => output::print_data(data, opt_bool(values, "no_color")),
    }
}

async fn run_command(name: &str, values: &HashMap<&'static str, Value>) -> Result<(), PyError> {
    match name {
        "version" => {
            println!("{VERSION}");
            Ok(())
        }
        "text" => {
            let request = TextSearchRequest::new(effective_query(values))
                .region(region(values)?)
                .safe_search(safesearch(values))
                .time_limit(timelimit(values))
                .max_results(max_results(values))
                .page(page(values))
                .backend(backend(values));
            let results = client(values)?.text_with(request).await?;
            finish_with_download("text", values, &to_rows(&results)?).await
        }
        "images" => {
            let mut request = ImageSearchRequest::new(effective_query(values))
                .region(region(values)?)
                .safe_search(safesearch(values))
                .time_limit(timelimit(values))
                .max_results(max_results(values))
                .page(page(values))
                .backend(backend(values));
            if let Some(v) = opt_str(values, "size") {
                request = request.size(v);
            }
            if let Some(v) = opt_str(values, "color") {
                request = request.color(v);
            }
            if let Some(v) = opt_str(values, "type_image") {
                request = request.type_image(v);
            }
            if let Some(v) = opt_str(values, "layout") {
                request = request.layout(v);
            }
            if let Some(v) = opt_str(values, "license_image") {
                request = request.license_image(v);
            }
            let results = client(values)?.images_with(request).await?;
            finish_with_download("images", values, &to_rows(&results)?).await
        }
        "videos" => {
            let mut request = VideoSearchRequest::new(effective_query(values))
                .region(region(values)?)
                .safe_search(safesearch(values))
                .time_limit(timelimit(values))
                .max_results(max_results(values))
                .page(page(values))
                .backend(backend(values));
            if let Some(v) = opt_str(values, "resolution") {
                request = request.resolution(v);
            }
            if let Some(v) = opt_str(values, "duration") {
                request = request.duration(v);
            }
            if let Some(v) = opt_str(values, "license_videos") {
                request = request.license_videos(v);
            }
            let results = client(values)?.videos_with(request).await?;
            let query = output::sanitize_query(&effective_query(values));
            finish_simple("videos", &query, values, &to_rows(&results)?)
        }
        "news" => {
            let request = NewsSearchRequest::new(effective_query(values))
                .region(region(values)?)
                .safe_search(safesearch(values))
                .time_limit(timelimit(values))
                .max_results(max_results(values))
                .page(page(values))
                .backend(backend(values));
            let results = client(values)?.news_with(request).await?;
            let query = output::sanitize_query(&effective_query(values));
            finish_simple("news", &query, values, &to_rows(&results)?)
        }
        "books" => {
            let request = BooksSearchRequest::new(effective_query(values))
                .max_results(max_results(values))
                .page(page(values))
                .backend(backend(values));
            let results = client(values)?.books_with(request).await?;
            // Upstream uses the raw `query` here (not sanitized; `None` when
            // only `-k` was given).
            let query = opt_str(values, "query").unwrap_or("None").to_owned();
            finish_simple("books", &query, values, &to_rows(&results)?)
        }
        "extract" => crate::extract::cli_extract(values).await,
        "mcp" => crate::server::mcp::cli_mcp(opt_str(values, "proxy")).await,
        "api" => crate::server::api::cli_api(values).await,
        other => Err(PyError::new("KeyError", other)),
    }
}

/// Run the CLI with `args` (without the program name). Returns the exit code.
pub fn run(prog: &str, args: &[String]) -> i32 {
    let commands = commands();
    let dispatch = match click::dispatch(prog, GROUP_HELP, &commands, args) {
        Ok(dispatch) => dispatch,
        Err(error) => {
            eprint!("{}", error.render());
            return 2;
        }
    };
    let runs = match dispatch {
        Dispatch::Help(page) => {
            print!("{page}");
            return 0;
        }
        Dispatch::HelpError(page) => {
            eprint!("{page}");
            return 2;
        }
        Dispatch::Run(runs) => runs,
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            output::echo(&PyError::os(&error, "").render());
            return 0;
        }
    };
    let _guard = tracing::subscriber::set_global_default(PyLogging);
    runtime.block_on(async {
        for (name, values) in &runs {
            if let Err(error) = run_command(name, values).await {
                // `safe_entry_point`: print and stop; exit code 0.
                output::echo(&error.render());
                break;
            }
        }
    });
    0
}

/// Entry point used by the `websearch` binary.
pub fn main() -> i32 {
    let mut args = std::env::args();
    let prog = args
        .next()
        .map(|p| {
            std::path::Path::new(&p)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(p)
        })
        .unwrap_or_else(|| "websearch".to_owned());
    let args: Vec<String> = args.collect();
    run(&prog, &args)
}
