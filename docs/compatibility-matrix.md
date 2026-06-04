# Python `ddgs` Compatibility Matrix

This matrix tracks public behavior to compare against Python `ddgs` before a production release.

| Surface | Rust status | Compatibility target | Remaining work |
| --- | --- | --- | --- |
| Text search | Implemented | Query, region, safesearch, timelimit, backend, page, max_results, proxy, timeout | Compare pagination/result ordering with Python fixtures |
| Image search | Implemented | Query, region, safesearch, backend, page, max_results, proxy, timeout | Compare image field naming and filters |
| Video search | Implemented | Query, region, safesearch, timelimit, backend, page, max_results, proxy, timeout | Compare date/duration/source fields |
| News search | Implemented | Query, region, safesearch, timelimit, backend, page, max_results, proxy, timeout | Compare timelimit behavior and source fields; review Rust `href` vs Python `url` naming |
| Books search | Implemented | Query, backend, page, max_results, proxy, timeout | Compare Anna's Archive field extraction against Python fixtures |
| CLI JSON | Implemented for current categories | Script-friendly JSON output | Freeze schema before 1.0 |
| Blocking API | Implemented for current categories | Synchronous Rust integration surface | Keep in sync as categories land |

Suggestions, instant answers, Python bindings, and Google search are intentionally not implemented in the current parity surface.

## Manual comparison procedure

1. Install Python `ddgs` and build `websearch-rs`.
2. Use the same query, region, safesearch, timelimit, proxy, timeout, backend/page, and max result settings where supported.
3. Compare field presence and broad semantics rather than exact ordering, because upstream provider results change frequently.
4. Capture sanitized fixtures for any markup/API variant that breaks Rust parsing.

Example Rust CLI commands:

```bash
websearch text "rust programming" --max-results 5 --json
websearch text "rust programming" --backend bing,brave --json
websearch images "rust programming" --max-results 5 --json
websearch videos "rust programming" --max-results 5 --json
websearch news "rust programming" --max-results 5 --json
websearch books "rust programming" --max-results 5 --json
```

## Release gate

For a 1.0 release, each implemented surface should have:

- Fixture tests for normal and empty responses.
- At least one ignored live integration test.
- Documented Python compatibility expectations.
- Stable JSON schema examples.
