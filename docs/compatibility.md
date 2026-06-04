# Python `ddgs` Compatibility Notes

`websearch-rs` targets public-behavior parity with Python `ddgs` 9.14.x where practical, while using idiomatic Rust APIs. See `docs/compatibility-matrix.md` for the release comparison matrix.

## Implemented surfaces

| Python-style surface | Rust API | CLI | Status |
| --- | --- | --- | --- |
| text/web search | `WebSearchClient::text`, `text_with` | `websearch text` | Full engine roster implemented except Google, which is disabled upstream |
| image search | `WebSearchClient::images`, `images_with` | `websearch images` | Full engine roster implemented |
| video search | `WebSearchClient::videos`, `videos_with` | `websearch videos` | Full engine roster implemented |
| news search | `WebSearchClient::news`, `news_with` | `websearch news` | Full engine roster implemented |
| books search | `WebSearchClient::books`, `books_with` | `websearch books` | Full engine roster implemented |

Suggestions and instant answers are intentionally out of the parity surface for the metasearch rewrite. Google search is intentionally omitted until Python `ddgs` re-enables it.

## Parameter mapping

| Concept | Rust | Notes |
| --- | --- | --- |
| query | `query` | Empty/whitespace queries return `WebSearchError::InvalidConfig`. |
| max results | `max_results` | Must be greater than zero. |
| region | `Region` | Defaults to `wt-wt`; accepts provider region codes such as `us-en`. |
| safesearch | `SafeSearch::{Off, Moderate, On}` | Values match Python-style `off` / `moderate` / `on`; `strict` parses as a legacy alias for `on`. |
| timelimit | `TimeLimit::{Day, Week, Month, Year}` | Supported for text/news/videos where upstream supports it. |
| backend | request `.backend(...)` / CLI `--backend` | `auto` or comma-separated engine names. Unknown names warn and fall back when none are valid. |
| page | request `.page(...)` / CLI `--page` | 1-based page number. |
| timeout | `WebSearchClientBuilder::timeout` | Also used as the per-engine orchestration budget. |
| proxy | `WebSearchClientBuilder::proxy` | Routed through the shared `wreq` client. |
| retries | `WebSearchClientBuilder::max_retries` | Retries timeout/connect/429/5xx errors. |
| retry delay | `WebSearchClientBuilder::retry_delay` | Linear retry backoff base delay. |

## Intentional Rust differences

- APIs are async and return typed `Result<T, WebSearchError>` values.
- `WebSearchBlockingClient` provides synchronous wrappers for non-async Rust integrations.
- Request configuration is expressed through typed request structs and enums.
- The CLI exposes stable JSON for scripting with `--json`.
- All engine HTTP goes through a shared `wreq`/BoringSSL client with browser impersonation.
- Live provider tests are ignored by default; parser behavior is covered with sanitized fixtures.

## Known gaps

- Pagination and result ordering need more real-world comparison against Python `ddgs`.
- News result field names still use the current Rust `href` shape; Python compatibility should be reviewed before freezing the 1.0 JSON schema.
- Live tests are ignored by default and should be run periodically because upstream markup can change.
