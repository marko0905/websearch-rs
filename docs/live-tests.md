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

Run one live surface:

```bash
cargo test live_text_search_returns_results -- --ignored
cargo test live_image_search_returns_results -- --ignored
cargo test live_video_search_returns_results -- --ignored
cargo test live_news_search_returns_results -- --ignored
cargo test live_books_search_returns_results -- --ignored
```

Run one engine-specific live test:

```bash
cargo test live_bing_returns_results -- --ignored
cargo test live_brave_returns_results -- --ignored
cargo test live_duckduckgo_returns_results -- --ignored
cargo test live_mojeek_returns_results -- --ignored
cargo test live_startpage_returns_results -- --ignored
cargo test live_wikipedia_returns_results -- --ignored
cargo test live_yahoo_returns_results -- --ignored
cargo test live_yandex_returns_results -- --ignored
cargo test live_grokipedia_returns_results -- --ignored
cargo test live_bing_news_returns_results -- --ignored
cargo test live_yahoo_news_returns_results -- --ignored
cargo test live_bing_images_returns_results -- --ignored
cargo test live_annasarchive_returns_results -- --ignored
```

Recommended CLI checks while debugging:

```bash
cargo run -- text "rust programming" --max-results 10 --backend auto --json
cargo run -- images "rust programming" --max-results 10 --json
cargo run -- videos "rust programming" --max-results 10 --json
cargo run -- news "rust programming" --max-results 10 --json
cargo run -- books "rust programming" --max-results 10 --json
```

Keep live tests ignored by default. If a live response changes, capture a small sanitized fixture and update parser tests before changing production parsing logic.
