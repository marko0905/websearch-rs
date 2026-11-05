# Live Provider Tests

Normal CI uses fixture tests only. Live tests hit real search engines and can fail because of network issues, markup/API changes, throttling, or regional behavior.

Run all live tests **serially** — run in parallel and the whole suite hammers a
handful of upstream hosts (DuckDuckGo, Yahoo) from one IP at once, which trips
their rate limiters and produces spurious failures. Each engine passes on its
own; `--test-threads=1` spaces the requests out:

```bash
cargo test -- --ignored --test-threads=1
```

(Real usage never sees this: `WebSearchClient` runs one category's engines at a time,
not all engines across all categories simultaneously.)

Run one live surface (these use `backend=auto`):

```bash
cargo test live_text_search_returns_results -- --ignored
cargo test live_image_search_returns_results -- --ignored
cargo test live_video_search_returns_results -- --ignored
cargo test live_news_search_returns_results -- --ignored
cargo test live_books_search_returns_results -- --ignored
```

Run one engine-specific live test:

```bash
cargo test live_brave_returns_results -- --ignored
cargo test live_duckduckgo_returns_results -- --ignored
cargo test live_grokipedia_returns_results -- --ignored
cargo test live_mojeek_returns_results -- --ignored
cargo test live_startpage_returns_results -- --ignored
cargo test live_wikipedia_returns_results -- --ignored
cargo test live_yahoo_returns_results -- --ignored
cargo test live_bing_news_returns_results -- --ignored
cargo test live_duckduckgo_news_returns_results -- --ignored
cargo test live_yahoo_news_returns_results -- --ignored
cargo test live_bing_images_returns_results -- --ignored
cargo test live_duckduckgo_images_returns_results -- --ignored
cargo test live_duckduckgo_videos_returns_results -- --ignored
cargo test live_annasarchive_returns_results -- --ignored
```

Two live tests exist for engines that upstream v9.16.1 disables (text `bing` and `yandex`). The registry does not include these engines, so `auto` never uses them. The tests call the engine modules directly. They help to check the old code, and they can fail:

```bash
cargo test live_bing_returns_results -- --ignored
cargo test live_yandex_returns_results -- --ignored
```

## Engines that are often blocked

A live failure from these engines is often a block, not a parser bug:

- `brave`: HTTP 429.
- `mojeek`: captcha page.
- `startpage`: Anubis challenge.
- `annasarchive`: Cloudflare.
- `yahoo` (text): bot wall on datacenter IPs.

The orchestrator skips an engine that fails. Check a block with a different IP or a proxy (`DDGS_PROXY`) before you change a parser.

Recommended CLI checks while debugging:

```bash
cargo run -- text -q "rust programming" -m 10 -b auto -o json
cargo run -- images -q "rust programming" -m 10 -o json
cargo run -- videos -q "rust programming" -m 10 -o json
cargo run -- news -q "rust programming" -m 10 -o json
cargo run -- books -q "rust programming" -m 10 -o json
```

Keep live tests ignored by default. If a live response changes, capture a small sanitized fixture and update parser tests before changing production parsing logic.
