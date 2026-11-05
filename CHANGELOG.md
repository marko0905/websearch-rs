# Changelog

## 9.16.1 - Unreleased (tracks Python ddgs 9.16.1)

### CLI, extract, MCP and REST API: 1:1 with upstream (breaking)

- **The CLI is now a port of upstream's `ddgs` CLI** (click 8.5): the same
  chained commands (`version`, `text`, `images`, `videos`, `news`, `books`,
  `extract`, `mcp`, `api`), flags (`-q`, `-k`, `-r`, `-s`, `-t`, `-m`, `-p`,
  `-b` repeatable, `-o`, `-d`, `-dd`, `-th`, `-pr`, `-v`, `-nc`, image and
  video filters), help pages, usage errors (exit code 2), output (numbered
  list, colors and paging in a terminal; `-o` CSV/JSON files; downloads with a
  progress bar), and error lines (`DDGSException: DDGSException('...')`, exit
  code 0). The old flags (`--max-results`, `--json`, positional query, ...)
  are gone. `clap` is removed.
- **New `WebSearchClient::extract(url, ExtractFormat)`** (upstream
  `DDGS.extract`) and the `ExtractFormat`, `ExtractContent`, `ExtractResult`
  types. Content formats use `html2text` 0.17.1, `mime` and `encoding_rs`
  exactly as primp 2.0.1 does.
- **New `websearch mcp`**: the upstream MCP server over stdio (same tools,
  schemas, results framing and errors).
- **New `websearch api`**: the upstream REST API (FastAPI app) on a built-in
  HTTP/1.1 server, with the same routes, validation errors, CORS, docs pages
  and uvicorn-style logs; `-d`/`-s` manage a detached server through
  `~/.cache/ddgs/api.pid`.
- **`TimeLimit` matches upstream's string**: `FromStr` takes exactly `d`, `w`,
  `m`, `y`; any other non-empty value is the new `TimeLimit::Custom(String)`,
  passed raw to engines that send it raw (custom date ranges) and failing the
  engines that map it through a table (`KeyError`). The aliases (`day`,
  `week`, ...) are gone, and `Display` prints the upstream value. `TimeLimit`
  is no longer `Copy`.
- Parity is checked against recordings of the real upstream package:
  `fixtures/cli/`, `fixtures/extract_reference.json`, `fixtures/mcp/`,
  `fixtures/api/` (scripts in `scripts/`).

### HTML parsing: libxml2, 1:1 with lxml

- HTML engines now parse with the vendored **libxml2 2.14.6** (git submodule
  `vendor/libxml2`), the same version that lxml 6.1.3 bundles, set up like
  upstream's `HTMLParser(remove_blank_text=True, remove_comments=True,
  remove_pis=True, collect_ids=False)` and `lxml.html.fromstring`.
- Every HTML engine (text bing, brave, duckduckgo, mojeek, startpage, yahoo,
  yandex; news bing, yahoo; images bing; books annasarchive) uses upstream's
  XPath strings verbatim. The `scraper`/`html5ever` dependencies are removed.
- Closes the parser gaps: the Yahoo unclosed `relsrch` div and
  `remove_blank_text` now behave as upstream.
- The disabled text `bing` and `yandex` engines are re-ported 1:1 (request,
  cookies, post-processing) so they can be enabled again when upstream does.
- New build requirement: `git submodule update --init vendor/libxml2` and
  CMake (already needed for BoringSSL).
- Random tokens (`token_urlsafe`, yandex `searchid`) use the OS random source
  (`getrandom`), like Python's `secrets`/`SystemRandom`.

First release of the `websearch-rs` metasearch rewrite: a Rust port of Python `ddgs`
that aggregates results from many engines behind one client, deduplicates, and
ranks them.

Tracked upstream: Python `ddgs` v9.16.1.

Breaking changes (summary; the sections below give details):
- Engine keys are renamed to upstream's: news `bing`/`duckduckgo`/`yahoo`,
  images `bing`/`duckduckgo`, videos `duckduckgo` (were `bing_news`,
  `duckduckgo_news`, `yahoo_news`, `bing_images`, `duckduckgo_images`,
  `duckduckgo_videos`).
- Text `bing` and `yandex` are no longer registered (disabled upstream).
- `max_results` is `Option<usize>` (`None` or `0`: all results).
- `WebSearchConfig.timeout` is `Option<Duration>`; the default is 5 s (was 10 s).
- `max_retries` defaults to 0 (was 2).
- The default region is `us-en` (was `wt-wt`).
- An empty search is an error (`Timeout`, the last engine error, or the new
  `NoResults`).
- Result schemas changed: upstream field names, order, `null` for missing
  values, raw JSON values; the engine name is removed from results.
- New fields: image `time_limit`/`size`/`color`/`type_image`/`layout`/
  `license_image`, video `resolution`/`duration`/`license_videos`.
- `HttpClient::post_form_text` (public, in the public `http` module) takes
  ordered pairs (`form: &[(String, String)]`) instead of
  `&HashMap<String, String>`, to keep the upstream field order. It and
  `get_text` return `Result<Option<String>>` (was `Result<String>`; `None` for
  any status other than 200).
- `ranking::normalize_url` and `ranking::dedup_key` are removed.
- CLI: `videos --time-limit` accepts only `d`, `w`, `m` (upstream choice); the
  library still accepts a year limit.

Implemented:
- Async client (`WebSearchClient`), blocking wrapper (`WebSearchBlockingClient`), and a
  `websearch` CLI, with typed request/result models.
- Five search categories — text, news, images, videos, books — each running its
  engines concurrently through a shared orchestrator with skip-on-error
  fallback, `ResultsAggregator` frequency aggregation, and `SimpleFilterRanker` ranking.
- Engine roster:
  - text: duckduckgo, brave, mojeek, yahoo, wikipedia, startpage, grokipedia
    (text `bing` and `yandex` are disabled to match upstream ddgs v9.16.1:
    code kept, not registered)
  - news: duckduckgo, bing, yahoo
  - images: duckduckgo, bing
  - videos: duckduckgo
  - books: annasarchive
- HTTP transport built on `wreq` + BoringSSL, reproducing real browser JA3/JA4
  TLS and HTTP/2 fingerprints with a random browser/OS profile per client. This
  is what gets past the fingerprint-based soft-blocking (e.g. Bing) that plain
  TLS stacks cannot.
- Parameters matching Python `ddgs`: `query`, `region`, `safesearch`
  (`off`/`moderate`/`on`), `timelimit`, `backend` (`auto` or comma-separated
  engine list), `page`, and `max_results`, plus `proxy`, `timeout`, and
  transient-retry controls (`max_retries`, `retry_delay`).
- JSON/CSV output for scripting (`-o json`, `-o csv`).
- A sanitized fixture, parser tests, and an ignored live integration test for
  every engine.
- CI gates: `cargo fmt --check`, `cargo test`, and `cargo clippy -D warnings`.

Result schema (breaking, for parity with ddgs v9.16.1 `results.py`):
- Result structs now serialize exactly like the upstream dataclasses: same
  field names and order, and the same JSON value types upstream emits.
  - `TextResult`: removed `source`.
  - `NewsResult`: `href` → `url`; added `image`; removed `provider`; field order
    is now `date, title, body, url, image, source`; every field is
    `Option<String>` (`null` where DuckDuckGo's JSON lacks the key); `date` is
    an ISO-8601 string (was `Option<u64>`).
  - `ImageResult`: `title`/`image`/`thumbnail`/`url`/`source` are
    `Option<String>`; `height`/`width` are `serde_json::Value` (integers from
    DuckDuckGo, strings from Bing; were `Option<u64>`), order is `height` then
    `width`; removed `provider`.
  - `VideoResult`: removed `thumbnail` and `source`; added `embed_html`,
    `image_token`, `images`, `statistics`; string fields are `Option<String>`;
    `images`/`statistics` are verbatim `serde_json::Value` objects (key order
    kept via serde_json `preserve_order`); `provider` now holds the upstream
    data provider (e.g. `"Bing"`), not the engine name.
  - `BooksResult`: removed `provider`.
  - Constructors changed to match (no engine-name argument); every result type
    gains a public `normalized()` (not idempotent: apply once, to raw values).
- Upstream field normalization applied to every engine's results: HTML
  tag-strip + entity decode + NFC + control-char removal + Python-style
  whitespace collapse for text fields; percent-decode with spaces as `+` for
  URL fields; integer timestamps to ISO-8601 UTC dates.
- Per-engine field mappings and selectors ported from upstream XPaths, upstream
  quirks included (news `image`; Bing news date/image post-processing with
  host-local-time-zone dates; Yahoo news `li[a]` items, `/RU=` URLs and
  stop-on-first-error post-processing; Bing images item/dimension/source
  XPaths; Anna's Archive author/publisher/info/url/thumbnail XPaths;
  DuckDuckGo news `s` paging offset). News, images, videos and books engines no
  longer filter or dedup their own results; where upstream raises (bad Bing
  image dimensions, Bing news date overflow) the engine now fails.
- HTML text fields join text nodes with no separator before collapsing
  whitespace, as upstream's `"".join(xpath(".//text()"))`.

Search pipeline (breaking, for parity with ddgs v9.16.1 `ddgs.py`, `base.py`,
`http_client.py`, `results.py`, `similarity.py`):
- Defaults: region `us-en` (was `wt-wt`), timeout 5 s (was 10 s), no retries
  (`max_retries` default 0, was 2). `WebSearchConfig.timeout` is now
  `Option<Duration>` (`None`: no timeout, via `no_timeout()`). New builder
  options `verify(Verify)` (`Enabled` / `Disabled` / `CaFile(path)`) and
  `threads(n)` (upstream `DDGS.threads`). The proxy falls back to the
  `DDGS_PROXY` environment variable, and `"tb"` expands to
  `socks5h://127.0.0.1:9150`. CLI: `--timeout` default 5, `--max-retries`
  default 0, new `--verify true|false`.
- Requests: `max_results` is `Option<usize>`; `None` or `0` returns every
  result. Validation is upstream's: only an empty query fails
  (`"query is mandatory."`); a whitespace query is searched; `max_results` is
  not checked. `page` must still be >= 1.
- Engine names are upstream's keys: news `bing`/`duckduckgo`/`yahoo`, images
  `bing`/`duckduckgo`, videos `duckduckgo` (were `bing_news`, etc.). Backend
  names are matched case-sensitively; `"all"` is a synonym of `"auto"`; any
  `auto`/`all` item in the list selects every engine.
- `SearchEngine` gains `provider()`; `priority()` is now `f64` with upstream's
  values (default 1, wikipedia 2, grokipedia 1.9) and runs in descending order.
  `EngineSet::new` takes the category. Selection shuffles, puts wikipedia and
  grokipedia first for text, then sorts stably by priority.
- Orchestration ports `_search_sync`: `max_workers = min(providers,
  ceil(max_results/10)+1)`, batch `wait(timeout, FIRST_EXCEPTION)` with
  pending engines carried into the next wait, the trailing sentinel wait, and
  provider skipping (a provider counts once an engine of it returned results).
  The search waits for every submitted engine before returning, as the
  executor's `shutdown(wait=True)` does. There is no per-engine timeout any more.
- Each engine has its own HTTP client (own random browser fingerprint) with a
  persistent cookie store, kept for the life of the `WebSearchClient`. Brave
  and Mojeek cookies go into it (so an earlier `safesearch` cookie persists, as
  upstream). Redirect limit 20. New wreq features: `cookies`, `socks`.
- HTTP: a status is never a transport error; an engine gets no results for any
  status other than 200 (including 202). `HttpClient::get_text` /
  `post_form_text` return `Option<String>`; new `get` / `post_form` return
  `HttpResponse { status, text }`; new `set_cookies`.
- Engines do one request per search: Anna's Archive uses one random mirror per
  process (no mirror fallback, no `Referer`); engines no longer truncate to
  `max_results`; Bing images always requests `count=35` (upstream quirk); the
  DuckDuckGo text block-page check is gone (a 202 anomaly page gives no
  results).
- Aggregation replaces normalized-URL dedup: `ResultsAggregator` keys on the
  exact value of the first of `href`/`image`/`url`/`embed_url` in field order
  (`None` → `"None"`), keeps the longest `body`, and orders by frequency.
  `ranking::normalize_url` and `ranking::dedup_key` are removed; `Rankable` is
  now a field view (`FIELDS`, `get`).
- Ranking ports `SimpleFilterRanker` exactly: tokens from `\W+` split, any
  token matches (was: all tokens), plain `"wikipedia.org"` substring check,
  Wikimedia category pages dropped, `body` falls back to `description`.
- Results: an empty search is now an error — `WebSearchError::Timeout` when the
  last engine error says "timed out", else that error, else the new
  `WebSearchError::NoResults` ("No results found."). New
  `WebSearchError::Internal` reproduces exceptions upstream raises from its own
  code (e.g. `TypeError` on a `None` news `body`/`title`).

Engine request parity (ddgs v9.16.1 `build_payload` / `headers_update`):
- Queries are sent verbatim (no `trim()`) by every engine.
- Only DuckDuckGo images has engine headers: its full upstream set (with
  `Connection: keep-alive` and `Priority: u=4`) goes on both the vqd bootstrap
  and `i.js`. DuckDuckGo news/videos, Bing images/news and Yahoo news now send
  no extra headers (the `Referer` headers were not upstream). A missing vqd
  gives upstream's `_extract_vqd() query='…' Could not extract vqd.`.
- DuckDuckGo images: `o, q, l, vqd, p, ct, [f], [s]`; `p` is `1` for on and
  moderate, `-1` for off; `f` only when a filter is set; `s=(page-1)*100`.
- DuckDuckGo videos: `l, o, q, vqd, f, p, [s]`; `f` is always
  `publishedAfter:…,videoDefinition:…,videoDuration:…,videoLicense:…` with
  empty parts (was `df`); `s=(page-1)*60`.
- DuckDuckGo news: `l, o, noamp, q, vqd, p, [df], [s]`.
- Bing news sends the region as is (`wt-wt` → `cc=wt&setlang=wt`) and fails
  on a region without exactly one `-`, as upstream.
- Bing images with any time limit fails with upstream's `KeyError` (`'d'`),
  an upstream quirk.
- New request options (passed as is, not validated, as upstream):
  `ImageSearchRequest` `time_limit`, `size`, `color`, `type_image`, `layout`,
  `license_image`; `VideoSearchRequest` `resolution`, `duration`,
  `license_videos`. CLI flags `--time-limit/--size/--color/--type-image/
  --layout/--license-image` (images) and `--resolution/--duration/
  --license-videos` (videos) take upstream's CLI choices.
- CLI errors print with `Display` (e.g. `Error: No results found.`) and exit 1.
- A dropped search aborts its still-running engine tasks.

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
- `annasarchive` (Cloudflare), `brave` (429), `mojeek` (captcha), and
  `startpage` (Anubis challenge) are correct but often blocked from a given IP.
  The orchestrator skips them and uses the other engines.
- Accepted differences from upstream and reproduced upstream quirks are listed
  in `docs/compatibility.md`.
- Pagination and result ordering still need broader live comparison with
  Python `ddgs`. The JSON schema should be reviewed before a 1.0 freeze.
