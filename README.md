# websearch-rs

A Rust rewrite of the Python [`ddgs`](https://github.com/deedy5/ddgs) metasearch
library — one interface that aggregates results from many search engines,
deduplicates, ranks, and merges them. Ships as an **async client**, a
**blocking client**, and a **CLI**.

[![CI](https://github.com/marko0905/websearch-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/marko0905/websearch-rs/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust 2024](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)

The goal is **behavioral parity with Python `ddgs`** (tracking **v9.16.1**): same engines, same parameters, same result field names.

## Highlights

- **Five categories** — text, news, images, videos, books — each fanned out
  across multiple engines concurrently, then deduplicated and ranked.
- **Browser-grade HTTP transport.** All traffic goes through
  [`wreq`](https://github.com/0x676e67/wreq), a BoringSSL client that reproduces
  real browser JA3/JA4 TLS and HTTP/2 fingerprints. This is what makes
  fingerprint-blocking engines (Bing News/Images, Brave, Startpage) reachable —
  header spoofing alone is not enough.
- **Resilient orchestration.** Engines run concurrently, like upstream's thread
  pool. A failing or empty engine is logged and skipped. The client waits at most
  `timeout` for each batch of engines.
- **Same surfaces as upstream** — `WebSearchClient` (async),
  `WebSearchBlockingClient` (sync), `extract` (fetch a page as Markdown, plain
  or rich text), and the `websearch` CLI: a 1:1 port of the `ddgs` CLI with the
  `mcp` (stdio MCP server) and `api` (REST server) commands.
- **Upstream's own parser.** HTML engines run upstream's XPath strings on the
  same libxml2 version that lxml uses (vendored), so they see the same tree.

## Engine roster

| Category | Engines                                                                          |
| -------- | -------------------------------------------------------------------------------- |
| text     | brave, duckduckgo, grokipedia, mojeek, startpage, yahoo, wikipedia               |
| news     | bing, duckduckgo, yahoo                                                          |
| images   | bing, duckduckgo                                                                 |
| videos   | duckduckgo                                                                       |
| books    | annasarchive                                                                     |

Most engines return results reliably. `annasarchive` (Cloudflare), `brave`
(HTTP 429), `mojeek` (captcha), and `startpage` (Anubis challenge) are often
blocked from a given IP. The orchestrator skips an engine that fails or returns
nothing.

Engine names are the upstream registry keys. They are case-sensitive. `all` is a
synonym of `auto`.

Text `bing` and `yandex` are disabled to match upstream `ddgs` v9.16.1. They are
not in `auto` and cannot be chosen by name. An unknown or disabled `backend`
name logs a warning. If no valid engine remains, the search uses `auto`.

> **Out of scope** (not parity with Python `ddgs`): suggestions, instant
> answers, and Python bindings. Google is deferred — it is disabled upstream.

## Install

### Build prerequisite

`wreq` compiles BoringSSL from source, and `build.rs` compiles the vendored
libxml2 2.14.6 (the HTML parser that Python ddgs uses through lxml) with CMake.
After cloning, fetch the libxml2 submodule:

```bash
git submodule update --init vendor/libxml2
```

You also need a C/C++ toolchain:

- **macOS:** `brew install cmake perl pkg-config`
- **Linux (Debian/Ubuntu):** `apt install build-essential cmake perl pkg-config libclang-dev` (add `musl-tools` for musl targets)

> Do **not** add dependencies that pull in `openssl-sys` — its symbols conflict
> with BoringSSL's `boring-sys`.

### Library

```toml
[dependencies]
websearch-rs = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

### CLI

```bash
cargo install --path .          # from a checkout
cargo run -- text -q "rust"     # or run without installing
```

## Library usage

### Quick start (async)

```rust
use websearch_rs::WebSearchClient;

#[tokio::main]
async fn main() -> websearch_rs::Result<()> {
    let client = WebSearchClient::new()?;
    let results = client.text("rust async http").await?;

    for result in results {
        println!("{} — {}", result.title, result.href);
    }

    Ok(())
}
```

### With request options

```rust
use websearch_rs::{WebSearchClient, Region, SafeSearch, TextSearchRequest, TimeLimit};

# async fn example() -> websearch_rs::Result<()> {
let client = WebSearchClient::builder()
    .max_retries(2)
    .retry_delay(std::time::Duration::from_millis(500))
    .build()?;

let request = TextSearchRequest::new("rust programming")
    .max_results(20)
    .region(Region::new("us-en")?)
    .safe_search(SafeSearch::Moderate)
    .time_limit(Some(TimeLimit::Month))
    .backend("brave,duckduckgo")   // "auto" or a comma-separated engine list
    .page(1);

let results = client.text_with(request).await?;
# Ok(())
# }
```

### Other categories

```rust
# async fn example() -> websearch_rs::Result<()> {
let client = websearch_rs::WebSearchClient::new()?;
let images = client.images("rust programming").await?;
let videos = client.videos("rust programming").await?;
let news   = client.news("rust programming").await?;
let books  = client.books("rust programming").await?;
# Ok(())
# }
```

### Blocking client

```rust
# fn example() -> websearch_rs::Result<()> {
let client = websearch_rs::WebSearchBlockingClient::new()?;
let results = client.text("rust programming")?;
# Ok(())
# }
```

### Extract a page

```rust
# async fn example() -> websearch_rs::Result<()> {
use websearch_rs::{ExtractContent, ExtractFormat, WebSearchClient};
let client = WebSearchClient::new()?;
let page = client.extract("https://example.com", ExtractFormat::TextMarkdown).await?;
if let ExtractContent::Text(markdown) = page.content {
    println!("{markdown}");
}
# Ok(())
# }
```

## CLI usage

The CLI is a port of upstream's `ddgs` CLI: same commands, flags, help pages,
error messages, output formats, and exit codes. Commands can be chained
(`websearch text -q a news -q b`).

```bash
websearch text   -q "rust programming" -m 10
websearch text   -q "rust programming" -b brave -b duckduckgo -o json   # save text_<query>_<time>.json
websearch text   -q "rust programming" -r us-en -s moderate -t m -o results.csv
websearch images -q "rust logo" -size Large -c Red -d -dd ./images       # download the images
websearch videos -q "rust programming" -res high -d short
websearch news   -q "rust programming" -t w
websearch books  -q "rust programming" -m 5
websearch extract -u https://example.com -f text_markdown
websearch version
websearch mcp                                  # MCP server over stdio
websearch api --host 127.0.0.1 --port 4479     # REST API (-d to detach, -s to stop)
```

Run `websearch <command> --help` for every flag. Results print as a numbered
list (in a terminal: colored, one result per Enter key). `-o csv|json` or
`-o file.csv|file.json` saves them instead. On a search error the CLI prints the
upstream exception line (for example `DDGSException: DDGSException('No results
found.')`) and exits with code 0, as upstream does; usage errors go to stderr
with exit code 2.

### MCP server

`websearch mcp` speaks JSON-RPC over stdio with the tools `search_text`,
`search_images`, `search_news`, `search_videos`, `search_books` and
`extract_content` (the same definitions as upstream's server). Client
configuration:

```json
{"mcpServers": {"websearch": {"command": "websearch", "args": ["mcp"]}}}
```

### REST API

`websearch api` serves `GET`/`POST` `/search/{text,images,news,videos,books}`
and `/extract`, plus `/health`, `/docs`, `/redoc` and `/openapi.json`, with the
same requests, responses and errors as upstream's FastAPI app.

## Parameters

| Param         | Values                                       | Notes                    |
| ------------- | -------------------------------------------- | ------------------------ |
| `query`       | string                                       | required                 |
| `region`      | e.g. `us-en`                                 | default `us-en`          |
| `safesearch`  | `on` \| `moderate` \| `off`                  | note: `on`, not `strict` |
| `timelimit`   | `d` \| `w` \| `m` \| `y` \| custom             | optional; custom values pass through like upstream |
| `backend`     | `auto` \| `all` \| comma-list of engine names | default `auto`           |
| `page`        | integer >= 1                                 | default `1`              |
| `max_results` | `Option<usize>`                              | default `Some(10)`; `None` or `0` returns all |
| `timeout`     | client option                                | default 5 s; `no_timeout()` waits forever |

Image requests also take `time_limit`, `size`, `color`, `type_image`, `layout`,
and `license_image`. Video requests take `resolution`, `duration`, and
`license_videos`. These values are passed to the engine as given, without
validation, as upstream does.

Client options (`WebSearchClient::builder()`): `timeout`, `no_timeout`, `proxy`
(falls back to the `DDGS_PROXY` environment variable; `"tb"` means
`socks5h://127.0.0.1:9150`), `no_proxy`, `verify(Verify::Enabled | Disabled |
CaFile(path))`, `threads`, and the Rust-only `max_retries` (default 0) and
`retry_delay`.

A search with no results is an error: `WebSearchError::Timeout`, the last engine
error, or `WebSearchError::NoResults` ("No results found.").

Result fields mirror the Python `ddgs` dataclasses exactly: same names, same
order (the JSON key order of `-o json`, the API and MCP), and the same JSON value types upstream emits:

| Type | Fields |
| ---- | ------ |
| text | `title`, `href`, `body` |
| news | `date`, `title`, `body`, `url`, `image`, `source` |
| images | `title`, `image`, `thumbnail`, `url`, `height`, `width`, `source` |
| videos | `title`, `content`, `description`, `duration`, `embed_html`, `embed_url`, `image_token`, `images`, `provider`, `published`, `publisher`, `statistics`, `uploader` |
| books | `title`, `author`, `publisher`, `info`, `url`, `thumbnail` |

Text and books fields are always strings. News, images and videos fields are
`Option<String>`: an engine that copies JSON keys (DuckDuckGo, Bing image
metadata) yields `null` for a missing key, as upstream does. Image
`height`/`width` and video `images`/`statistics` are `serde_json::Value`
copied verbatim (DuckDuckGo dimensions are integers, Bing's are strings; view
counts stay integers or `null`; object key order is preserved).

Fields are normalized as upstream does: text fields (`title`, `body`, `author`,
`publisher`, `info`) have HTML tags stripped, entities decoded, Unicode NFC
applied, control/format characters removed, and whitespace collapsed; URL
fields (`href`, `url`, `image`, `thumbnail`) are percent-decoded with spaces as
`+`; news `date` is ISO-8601 (e.g. `2024-03-09T16:00:00+00:00`) when the engine
gives a timestamp or a recognizable date (Bing absolute dates are read in the
host's local time zone, like Python's `astimezone()`). Results do not carry the
producing engine's name.

## Architecture

```
public client → orchestrator → engine registry → N engines (concurrent) → dedup + rank → merged results
```

- **`src/http.rs`** — the single HTTP choke point. Each engine has its own
  `wreq` client with a random browser profile and a persistent cookie store.
  It also handles the proxy, timeout, and TLS verification. Any status other
  than 200 gives no results. Retries are a Rust-only opt-in (default 0). Every
  engine request routes through here.
- **`src/engines/`** — per-category traits (`TextEngine`, `NewsEngine`, …), the
  `EngineRegistry` (category → engines), and the orchestrator, a port of
  upstream's thread-pool search loop. One file per `(engine, category)`.
- **`src/normalize.rs`** — ports of upstream's text, URL, and date normalizers.
- **`src/ranking.rs`** — `ResultsAggregator` (key = first of
  `href`/`image`/`url`/`embed_url`, frequency order) + `SimpleFilterRanker`
  (port of Python `similarity.py`).
- **`src/request.rs` / `src/results.rs`** — request builders + validation and
  the public, serializable result structs.
- **`src/html.rs`** — libxml2 (vendored 2.14.6) parsing set up like upstream's
  lxml parser, and XPath; engines use upstream's XPath strings.
- **`src/extract.rs`** — `WebSearchClient::extract` (upstream `DDGS.extract`).
- **`src/cli/`, `src/server/`** — the CLI (click port), the MCP server and the
  REST API server.

See [`docs/`](docs/) for the compatibility matrix and live-test notes.

## Development

```bash
cargo test                                                # unit + fixture + integration tests (skips live)
cargo fmt --all --check                                   # formatting gate
cargo clippy --all-targets --all-features -- -D warnings  # lint gate (warnings are errors)
```

Live network tests are `#[ignore]`d (they hit real engines and are flaky):

```bash
cargo test -- --ignored
```

CI enforces all three gates (`fmt --check`, `test`, `clippy -D warnings`). The
toolchain is pinned to stable, Rust **2024 edition** (MSRV 1.85+). See
[`docs/live-tests.md`](docs/live-tests.md), [`docs/compatibility.md`](docs/compatibility.md),
and [`docs/compatibility-matrix.md`](docs/compatibility-matrix.md) for testing
and parity details, and [`docs/release.md`](docs/release.md) for release steps.

## License

Licensed under either of [MIT](LICENSE-MIT) or
[Apache-2.0](LICENSE-APACHE) at your option.
