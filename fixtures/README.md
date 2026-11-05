# Fixtures

Store sanitized provider HTML/JSON fixtures here.

Generated files (do not edit by hand; run `scripts/regen_expected.py` with
upstream ddgs, see `docs/compatibility.md`):
- `*_expected.json`: upstream `extract_results` + `post_extract_results`
  output for a fixture. `*_raw_expected.json`: `extract_results` only (news
  engines, whose post-processing depends on the current time).
- `parser/<fixture>.json`: the whole tree that upstream's lxml parser builds
  for `<fixture>.html`. `html::tests::parser_parity` compares our libxml2
  tree with it.

Rules:
- Do not commit cookies, tokens, proxy credentials, or personal data.
- Prefer small fixtures that exercise parser behavior.
- Keep live network tests ignored by default; use fixtures for normal CI.
