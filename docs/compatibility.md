# Python `ddgs` Compatibility Notes

`websearch-rs` tracks Python `ddgs` **v9.16.1** (released 2026-10-23). The aim is strict, continuous parity: the same engine roster, parameters, result fields, and search pipeline. Parity includes upstream bugs and quirks. A difference from upstream is a bug unless this file lists it. See `docs/compatibility-matrix.md` for the per-surface matrix.

## Implemented surfaces

| Python-style surface | Rust API | CLI | Engines (registry keys) |
| --- | --- | --- | --- |
| text | `WebSearchClient::text`, `text_with` | `websearch text` | brave, duckduckgo, grokipedia, mojeek, startpage, yahoo, wikipedia |
| images | `images`, `images_with` | `websearch images` | bing, duckduckgo |
| videos | `videos`, `videos_with` | `websearch videos` | duckduckgo |
| news | `news`, `news_with` | `websearch news` | bing, duckduckgo, yahoo |
| books | `books`, `books_with` | `websearch books` | annasarchive |

Disabled upstream, so not registered: text `bing`, text `yandex` (the code is kept in `src/engines/text/`), and `google` (not implemented).

Out of the parity surface: suggestions, instant answers, and Python bindings.

## Parameter mapping

| Concept | Rust | Notes |
| --- | --- | --- |
| query | `query` | An empty string fails (`query is mandatory.`). A whitespace query is searched. |
| max results | `max_results: Option<usize>` | Default `Some(10)`. `None` or `0` returns all results. |
| region | `Region` | Default `us-en`. |
| safesearch | `SafeSearch::{Off, Moderate, On}` | `off` / `moderate` / `on`. `strict` parses as a legacy alias for `on`. |
| timelimit | `TimeLimit::{Day, Week, Month, Year}` | Passed to the engine. CLI `videos --time-limit` takes only `d`, `w`, `m`. |
| backend | request `.backend(...)` / CLI `--backend` | `auto`, `all`, or a comma list. |
| page | request `.page(...)` / CLI `--page` | Must be >= 1. |
| image filters | `time_limit`, `size`, `color`, `type_image`, `layout`, `license_image` | Passed as given. |
| video filters | `time_limit`, `resolution`, `duration`, `license_videos` | Passed as given. |
| timeout | `WebSearchClientBuilder::timeout` / `no_timeout` | Default 5 s. See "Timeout semantics". |
| proxy | `WebSearchClientBuilder::proxy` | `"tb"` means `socks5h://127.0.0.1:9150`. Falls back to `DDGS_PROXY`. |
| verify | `WebSearchClientBuilder::verify` | `Verify::Enabled`, `Disabled`, or `CaFile(path)`. |
| threads | `WebSearchClientBuilder::threads` | Upstream `DDGS.threads`. |
| retries | `max_retries`, `retry_delay` | Rust-only opt-in. Default 0 (upstream never retries). |

## Accepted differences

These differences are deliberate or cannot be closed without extra work.

1. `page >= 1` is enforced. Upstream does not check it. Engine offsets use `page - 1` on unsigned numbers.
2. A zero timeout is rejected (`InvalidConfig`). Use `no_timeout()` to wait forever.
3. A bad proxy or CA file fails at `build()`. Upstream builds engine clients lazily and fails at the first search.
4. After a wait times out, the engines that are still pending are collected in submission order. Upstream keeps them in a set, so their order is arbitrary.
5. Library errors use the Rust error type and do not copy Python exception texts. A few messages copy upstream (`query is mandatory.`, `No results found.`, `Failed to fetch <url>: HTTP <status>`). The CLI, MCP server and REST API copy upstream's messages wherever upstream's own code (DDGS, click, pydantic, FastAPI, Python `json`) produces them; for other Python exceptions (for example `UnicodeDecodeError` details, primp transport errors) the text is close but not identical.
6. Rust-only retries (`max_retries`, `retry_delay`), default off.
7. A negative `max_results` (upstream `results[:-n]`) cannot be expressed by the Rust request; the CLI, MCP and API treat it as "all results".
8. The binary is `websearch`; help and usage lines show the invoked program name, as click does (run it through a link named `ddgs` for byte-identical output).
9. `api --reload` is accepted and has no effect (uvicorn reloads Python sources). Starlette joins a route's method set in hash order, so upstream's `Allow` value for the docs routes can be `GET, HEAD` or `HEAD, GET`; this port always sends `GET, HEAD`. `api -s` (stop) works on Unix; on Windows it reports that it cannot signal the process.
10. In a terminal, the download progress bar follows click's layout and ETA rules but is not checked byte for byte (the reference tests run without a terminal).

## Parity checks against upstream recordings

Each surface is compared with output recorded from the real upstream package (see `scripts/`):

| Surface | Recording | Test |
| --- | --- | --- |
| HTML parser (lxml/libxml2 tree) | `fixtures/parser/*.json` | `html::tests::parser_parity` |
| Engine results | `fixtures/*_expected.json`, `*_raw_expected.json` | engine `matches_upstream_*` tests |
| CLI help, usage errors, exit codes | `fixtures/cli/reference.json` | `tests/cli_parity.rs` |
| CLI print / CSV / JSON output | `fixtures/cli/output_reference.json` | `cli::output::tests::output_matches_upstream_reference` |
| `extract` formats | `fixtures/extract_reference.json` | `extract::tests::matches_upstream_extract_reference` |
| MCP server | `fixtures/mcp/reference.json` | `server::mcp::tests::matches_upstream_mcp_reference` |
| REST API | `fixtures/api/reference.json` | `server::api::tests::matches_upstream_api_reference` |

## Timeout semantics

- Each `wait` for a batch of engines is bounded by `timeout`.
- When the loop ends, the search waits for **all** submitted engines (upstream `ThreadPoolExecutor` `shutdown(wait=True)`). Worst case is about (requests per engine) x `timeout`.
- Each HTTP request also has `timeout` as its own limit.
- There is no per-engine timeout.
- `no_timeout()` can hang, as upstream can.
- Dropping a search future aborts its still-running engine tasks.

## Breaking changes (from the pre-parity API)

Also listed in `CHANGELOG.md`.

- Engine keys use upstream's names: news `bing`/`duckduckgo`/`yahoo`, images `bing`/`duckduckgo`, videos `duckduckgo`. The old `*_news`, `*_images`, `*_videos` names are gone.
- `max_results` is `Option<usize>`. `timeout` is `Option<Duration>`.
- `max_retries` defaults to 0. The default region is `us-en`.
- An empty search is an error: `Timeout` (the last engine error says "timed out"), else the last engine error, else `NoResults`.
- Result schemas follow upstream: field names and order, `null` for missing values, raw JSON values. The engine name is removed from results.
- New image filters and `time_limit`; new video filters.
- `HttpClient::post_form_text` takes ordered pairs. `http` is a public module.

## Reproduced upstream quirks

Each quirk has a code comment that names it. Remove it when upstream fixes it.

| Engine | Quirk |
| --- | --- |
| all | The `(priority, random)` sort key uses the `random` function, not a call. The tie-break never applies; ties keep the shuffled order (stable sort). |
| all | Any HTTP status other than 200 gives no results (also 202). |
| duckduckgo text | No block-marker check. A 202 anomaly page gives no results. |
| all (aggregator) | The key is `str(value)`. Every result with a `None` key field shares the key `"None"`, so they merge into one result. |
| all (aggregator, ranker) | A `None` body on a repeated key, or a `None` field in the ranker, raises `TypeError` upstream. Here it is `WebSearchError::Internal`. |
| bing images | `count=35` is always sent. |
| bing images | Bad dimensions text fails the whole engine (upstream raises). |
| duckduckgo news | A timestamp outside the `datetime` range fails the whole engine (upstream raises). |
| bing images | Any time limit fails with `KeyError` (`'d'`). |
| bing news | The relative-date regex treats the unit as optional. "5 hours ago" becomes 5 days. |
| bing news | A region without exactly one `-` fails. |
| bing news | Absolute dates are read in the host's local time zone. |
| yahoo news | The post-processing loop sits in one `try`. The first failure stops it. |
| yahoo news | The source split (`" ·  via Yahoo"`) never matches collapsed text. |
| yahoo text | The href is unquoted three times. |
| annasarchive | One random mirror per process. The URL is normalized twice (on extraction and in the result constructor). |
| wikipedia | Only the exact text `may refer to:` marks a disambiguation page. "may also refer to:" is kept. The engine returns at most one result. |
| grokipedia | `_normalize_text` deletes control characters, so `"\n\n"` without a space joins words. |
| brave | The cookie is named after the country value (`us=us`), not `country=us`. An earlier `safesearch` cookie persists in the cookie store. |

## Engine and HTTP behavior

- Each engine has its own HTTP client (own random browser profile) and persistent cookie store. They live as long as the `WebSearchClient`. Clones share them.
- Redirects: at most 20.
- On HTTP/2, `wreq` strips the `Connection` header. Upstream (hyper-based `primp`) does the same.
- Proxy: `DDGS_PROXY` is the fallback and is used as given. The `tb` alias expands only for an explicit proxy.
- `Verify` can enable, disable, or point to a CA file.
- Backend names are case-sensitive. Duplicates are kept. `all` equals `auto`. An `auto`/`all` item in a list selects every engine. Unknown names log a warning. With no valid name, the search uses `auto`.
- Queries are sent as given (no trimming).

## Fixture provenance

- The DuckDuckGo, Yahoo, Grokipedia, and Wikipedia text fixtures are sanitized real captures.
- The Brave, Mojeek, and Startpage text fixtures are **synthetic**. The live engines were blocked from the development IP.
- `fixtures/*_expected.json` hold the output of upstream ddgs 9.16.1 (`extract_results` + `post_extract_results`) on the fixtures, for every HTML engine (text bing, brave, duckduckgo, mojeek, startpage, yahoo, yandex; images bing; books annasarchive), Grokipedia, and three Wikipedia cases. The news engines post-process dates relative to "now", so for them `*_raw_expected.json` holds the `extract_results` output only. The Rust tests compare with all of them.
- `fixtures/parser/*.json` hold the whole tree that upstream's lxml parser builds for each `*.html` fixture. The test `html::tests::parser_parity` compares our libxml2 tree with it node by node (elements, attributes, text).

Regenerate them with:

```bash
uv run --no-project --with ddgs==9.16.1 python scripts/regen_expected.py
```

Pass a fixtures directory and an output directory as optional arguments. `wikipedia_opensearch_may_also_refer.json` is a synthetic one-line fixture, and `wikipedia_extracts_may_also_refer.json` is a trimmed extract of the real "Mercury" page. Then run `cargo test`. Review any change in the JSON before you accept it.

## Known gaps

- Pagination and result ordering need more live comparison with Python `ddgs`.
- Live tests are ignored by default. Run them often, because upstream markup changes.
