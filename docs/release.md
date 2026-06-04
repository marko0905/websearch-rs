# Release Checklist

Use this checklist before publishing `websearch-rs` or shipping binaries.

## Pre-release validation

```bash
cargo fmt --all --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test -- --ignored
cargo run -- text "rust programming" --max-results 5 --json
cargo run -- text "rust programming" --backend bing,brave --max-results 5 --json
cargo run -- images "rust programming" --max-results 5 --json
cargo run -- videos "rust programming" --max-results 5 --json
cargo run -- news "rust programming" --max-results 5 --json
cargo run -- books "rust programming" --max-results 5 --json
```

## Crate release

1. Update `Cargo.toml` version.
2. Update `CHANGELOG.md`.
3. Verify `README.md` and `docs/compatibility.md` reflect current behavior.
4. Run the full validation commands above.
5. Publish with:

```bash
cargo publish --dry-run
cargo publish
```

## Binary release

The CLI binary is named `websearch` and can be installed from source with:

```bash
cargo install --path .
```

For tagged releases, build optimized artifacts with:

```bash
cargo build --release --bin websearch
```

Pushing a `v*` tag triggers `.github/workflows/release.yml`, which validates the crate and uploads per-platform CLI artifacts.

## Notes

Python bindings are intentionally not part of the current release surface. The canonical Python package remains upstream `ddgs`; this crate targets Rust library and CLI distribution.
