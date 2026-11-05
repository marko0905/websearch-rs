# Release Checklist

Use this checklist before publishing `websearch-rs` or shipping binaries.

## Pre-release validation

```bash
cargo fmt --all --check
cargo test --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test -- --ignored
cargo run -- text -q "rust programming" -m 5 -o json
cargo run -- text -q "rust programming" -b brave -b duckduckgo -m 5 -o json
cargo run -- images -q "rust programming" -m 5 -o json
cargo run -- videos -q "rust programming" -m 5 -o json
cargo run -- news -q "rust programming" -m 5 -o json
cargo run -- books -q "rust programming" -m 5 -o json
```

## Crate release

1. Update `Cargo.toml` version.
2. Update `CHANGELOG.md`.
3. Verify `README.md`, `docs/compatibility.md`, and `docs/compatibility-matrix.md` match the code and the tracked upstream version (currently ddgs v9.16.1).
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
