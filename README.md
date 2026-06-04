# websearch-rs

A Rust rewrite of the Python [`ddgs`](https://github.com/deedy5/ddgs) metasearch
library — one interface that aggregates results from many search engines,
deduplicates, ranks, and merges them. Ships as an **async client**, a
**blocking client**, and a **CLI**.

[![CI](https://github.com/marko0905/websearch-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/marko0905/websearch-rs/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust 2024](https://img.shields.io/badge/rust-2024%20edition-orange.svg)](https://www.rust-lang.org)

The goal is **behavioral parity with Python `ddgs`** (tracking the `9.14.x`
line): same engines, same parameters, same result field names.

## Highlights

- **Five categories** — text, news, images, videos, books — each fanned out
  across multiple engines concurrently, then deduplicated and ranked.
- **Browser-grade HTTP transport.** All traffic goes through
  [`wreq`](https://github.com/0x676e67/wreq), a BoringSSL client that reproduces
  real browser JA3/JA4 TLS and HTTP/2 fingerprints. This is what makes
  fingerprint-blocking engines (Bing, Brave, Yandex, Startpage) reachable —
  header spoofing alone is not enough.
- **Resilient orchestration.** Engines run concurrently with per-engine
  timeouts; a blocked or failing engine is logged and skipped, so one bad engine
  never breaks a search.
- **Three surfaces** — `WebSearchClient` (async), `WebSearchBlockingClient` (sync), and the
  `websearch` CLI with JSON output for scripting.

## Engine roster

| Category | Engines                                                                          |
| -------- | -------------------------------------------------------------------------------- |
| text     | bing, brave, duckduckgo, grokipedia, mojeek, startpage, yandex, yahoo, wikipedia |
| news     | bing_news, duckduckgo_news, yahoo_news                                           |
| images   | bing_images, duckduckgo_images                                                   |
| videos   | duckduckgo_videos                                                                |
| books    | annasarchive                                                                     |

Most engines return results reliably. `yandex` and `annasarchive` are
anti-bot/Cloudflare-sensitive and are often blocked from a given IP; the
orchestrator falls back to the remaining engines.

> **Out of scope** (not parity with Python `ddgs`): suggestions, instant
> answers, and Python bindings. Google is deferred — it is disabled upstream.

## Install

### Build prerequisite

`wreq` compiles BoringSSL from source, so you need a C/C++ toolchain:

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
cargo run -- text "rust"        # or run without installing
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
    .backend("bing,brave")   // "auto" or a comma-separated engine list
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

## CLI usage

```bash
websearch text   "rust programming" --max-results 10
websearch text   "rust programming" --backend bing,brave --json
websearch text   "rust programming" --region us-en --safe moderate --time-limit month
websearch images "rust programming" --max-results 5 --json
websearch videos "rust programming" --max-results 5
websearch news   "rust programming" --time-limit week
websearch books  "rust programming" --max-results 5
```

Shared flags: `--max-results`, `--region`, `--safe`, `--backend`, `--page`,
`--timeout`, `--proxy`, `--max-retries`, `--retry-delay-ms`, `--json`.
`--time-limit` applies to `text`, `news`, and `videos`.

## Parameters

| Param         | Values                                       | Notes                    |
| ------------- | -------------------------------------------- | ------------------------ |
| `query`       | string                                       | required                 |
| `region`      | e.g. `us-en`                                 | default `us-en`          |
| `safesearch`  | `on` \| `moderate` \| `off`                  | note: `on`, not `strict` |
| `timelimit`   | `d` \| `w` \| `m` \| `y` (news: `d`/`w`/`m`) | optional                 |
| `backend`     | `auto` \| comma-list of engine names         | default `auto`           |
| `page`        | integer                                      | default `1`              |
| `max_results` | integer                                      | default `10`             |

Result fields mirror Python `ddgs`: text → `title`/`href`/`body`; news →
`title`/`href`/`body`/`date`/`source`; images/videos/books per their upstream
shapes. All result types are `Serialize` (this drives `--json`).

## Architecture

```
public client → orchestrator → engine registry → N engines (concurrent) → dedup + rank → merged results
```

- **`src/http.rs`** — the single HTTP choke point: owns the `wreq` client, random
  browser emulation, retries-with-backoff, proxy, and timeout. Every engine
  request routes through here.
- **`src/engines/`** — per-category traits (`TextEngine`, `NewsEngine`, …), the
  `EngineRegistry` (category → engines), and the concurrent orchestrator with
  fallback. One file per `(engine, category)`.
- **`src/ranking.rs`** — `Aggregator` (dedup by normalized URL) +
  `SimpleFilterRanker` (port of Python `similarity.py`).
- **`src/request.rs` / `src/results.rs`** — request builders + validation and
  the public, serializable result structs.

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
