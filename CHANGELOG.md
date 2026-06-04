# Changelog

## 0.1.0 - Unreleased

First release of the `websearch-rs` metasearch rewrite: a Rust port of Python `ddgs`
that aggregates results from many engines behind one client, deduplicates, and
ranks them.

Implemented:
- Async client (`WebSearchClient`), blocking wrapper (`WebSearchBlockingClient`), and a
  `websearch` CLI, with typed request/result models.
- Five search categories — text, news, images, videos, books — each running its
  engines concurrently through a shared orchestrator with skip-on-error
  fallback, normalized-URL deduplication, and `SimpleFilterRanker` ranking.
- Engine roster:
  - text: duckduckgo, bing, brave, mojeek, yahoo, wikipedia, startpage,
    grokipedia, yandex
  - news: duckduckgo_news, bing_news, yahoo_news
  - images: duckduckgo_images, bing_images
  - videos: duckduckgo_videos
  - books: annasarchive
- HTTP transport built on `wreq` + BoringSSL, reproducing real browser JA3/JA4
  TLS and HTTP/2 fingerprints with a random browser/OS profile per client. This
  is what gets past the fingerprint-based soft-blocking (e.g. Bing) that plain
  TLS stacks cannot.
- Parameters matching Python `ddgs`: `query`, `region`, `safesearch`
  (`off`/`moderate`/`on`), `timelimit`, `backend` (`auto` or comma-separated
  engine list), `page`, and `max_results`, plus `proxy`, `timeout`, and
  transient-retry controls (`max_retries`, `retry_delay`).
- Stable JSON output for scripting (`--json`).
- A sanitized fixture, parser tests, and an ignored live integration test for
  every engine.
- CI gates: `cargo fmt --check`, `cargo test`, and `cargo clippy -D warnings`.

Engine fixes (live-verified):
- DuckDuckGo `i.js`/`v.js`/`news.js` (images/videos/news) no longer 403: the
  `vqd` bootstrap now sends only `q` (an `ia=` param suppresses the token), the
  JSON endpoints use browser fetch-metadata headers (`Accept: */*`,
  `Sec-Fetch-*`, `Sec-GPC`) instead of `X-Requested-With`, and image search
  sends the required `ct=AT`. Shared `vqd`/header helpers centralized in
  `engines/util.rs`.
- Anna's Archive books: updated to the 2024+ markup (`js-aarecord-list-outer`
  cards, `text-lg` `/md5/` title anchors, `/search` author/publisher links,
  `text-gray-600` info), with the legacy container kept as a fallback.
- Yahoo text: parser confirmed correct against live markup; its intermittent
  datacenter-IP bot-walling is documented (the orchestrator skips an empty
  engine; the live test tolerates it).
- Live tests should run with `--test-threads=1` to avoid self-induced upstream
  rate limiting; see `docs/live-tests.md`.

Intentionally out of scope:
- Suggestions and instant answers (not categories in Python `ddgs`).
- Python bindings (the canonical Python package is upstream `ddgs`).
- The `google` engine, which is disabled upstream.

Known limitations:
- `yandex` and `annasarchive` are correct but anti-bot/Cloudflare-sensitive and
  often return nothing from a given IP; the orchestrator skips blocked engines
  and falls back to the rest, so a search is never broken by one of them.
- Pagination and result ordering still need broader comparison against Python
  `ddgs`, and the JSON schema (notably news `href` vs Python `url`) should be
  reviewed before a 1.0 freeze.
