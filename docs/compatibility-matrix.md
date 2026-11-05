# Python `ddgs` Compatibility Matrix

This matrix tracks public behavior against Python `ddgs` **v9.16.1**. Details, accepted differences, and reproduced quirks are in `docs/compatibility.md`.

## Surfaces

| Surface | Rust status | Engines (registry keys) | Parameters | Notes |
| --- | --- | --- | --- | --- |
| Text search | Implemented | brave, duckduckgo, grokipedia, mojeek, startpage, yahoo, wikipedia | query, region, safesearch, timelimit, backend, page, max_results | `bing` and `yandex` are disabled upstream: code kept, not registered |
| Image search | Implemented | bing, duckduckgo | text parameters plus `size`, `color`, `type_image`, `layout`, `license_image`, `time_limit` | Bing images fails on any time limit (upstream quirk) |
| Video search | Implemented | duckduckgo | text parameters plus `resolution`, `duration`, `license_videos` | CLI `--time-limit` takes `d`/`w`/`m` |
| News search | Implemented | bing, duckduckgo, yahoo | query, region, safesearch, timelimit, backend, page, max_results | Result field `url` (not `href`) |
| Books search | Implemented | annasarchive | query, backend, page, max_results | One random mirror per process |
| Client options | Implemented | n/a | timeout (5 s), proxy (`DDGS_PROXY`, `tb`), verify, threads | Rust-only: `max_retries`, `retry_delay` (default off) |
| CLI | Implemented | n/a | all `ddgs` commands and flags | 1:1 with upstream (checked against recordings) |
| Blocking API | Implemented | n/a | same as async | `WebSearchBlockingClient` |

Not implemented by design: suggestions, instant answers, Python bindings, and `google` (disabled upstream).

## Result fields

Names and order match upstream `results.py`.

| Type | Fields |
| --- | --- |
| text | `title`, `href`, `body` |
| news | `date`, `title`, `body`, `url`, `image`, `source` |
| images | `title`, `image`, `thumbnail`, `url`, `height`, `width`, `source` |
| videos | `title`, `content`, `description`, `duration`, `embed_html`, `embed_url`, `image_token`, `images`, `provider`, `published`, `publisher`, `statistics`, `uploader` |
| books | `title`, `author`, `publisher`, `info`, `url`, `thumbnail` |

## Parity check procedure

1. Update the tracked version: run `/update-project`.
2. Regenerate the upstream expected output for the text fixtures: `uv run --no-project --with ddgs==9.16.1 python scripts/regen_expected.py`.
3. Run `cargo test`. The text engine tests compare our parsers with `fixtures/*_expected.json`.
4. For live comparison, use the same query, region, safesearch, timelimit, proxy, timeout, backend, page, and max results in both tools. Compare field presence and broad meaning, not exact order, because live results change.
5. Capture a sanitized fixture for any markup or API variant that breaks Rust parsing.

Example Rust CLI commands:

```bash
websearch text -q "rust programming" -m 5 -o json
websearch text -q "rust programming" -b brave -b duckduckgo -o json
websearch images -q "rust programming" -m 5 -o json
websearch videos -q "rust programming" -m 5 -o json
websearch news -q "rust programming" -m 5 -o json
websearch books -q "rust programming" -m 5 -o json
```

## Release gate

For a 1.0 release, each implemented surface needs:

- Fixture tests for normal and empty responses.
- At least one ignored live integration test.
- Documented Python compatibility expectations.
- Stable JSON schema examples.
