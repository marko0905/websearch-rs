"""Regenerate `fixtures/cli/reference.json` from the upstream `ddgs` CLI.

Each case runs the real upstream CLI (`ddgs <args>`) with `COLUMNS=80` and
stdin closed, and records stdout, stderr and the exit code. Only cases that
need no network are listed. The Rust test `tests/cli_parity.rs` runs our
binary (through a symlink named `ddgs`) on the same cases and compares.

It also writes `fixtures/cli/output_reference.json`: for result rows of every
category (from the fixtures, plus synthetic rows that exercise line wrapping),
what upstream `_print_data` prints (not a terminal), and the files
`_save_csv` and `_save_json` write. `src/cli/output.rs` tests compare.

And `fixtures/extract_reference.json`: upstream `DDGS().extract()` for HTML
fixtures served on localhost, in every text format (primp's html2text and
charset decoding). `src/extract.rs` tests compare.

Usage (from the repository root):

    uv run --no-project --with ddgs==9.16.1 python scripts/regen_cli_reference.py
"""

import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

CASES = [
    [],
    ["--help"],
    ["--help", "text"],
    ["-x", "text"],
    ["tex"],
    ["foo"],
    ["version"],
    ["version", "--help"],
    ["version", "--nope"],
    ["text", "--help"],
    ["images", "--help"],
    ["videos", "--help"],
    ["news", "--help"],
    ["books", "--help"],
    ["extract", "--help"],
    ["mcp", "--help"],
    ["api", "--help"],
    ["text", "-s", "x"],
    ["text", "-q"],
    ["text", "-k"],
    ["text", "--nope"],
    ["text", "-nope"],
    ["text", "--s", "on"],
    ["text", "-m", "abc"],
    ["text", "-m", " 1_0", "-s", "x"],
    ["text", "-v", "maybe"],
    ["text", "--verify"],
    ["text", "-nc=1"],
    ["text", "-dd"],
    ["text", "-qx", "-b", "nope"],
    ["text", "-qa", "-kb", "-t", "q"],
    ["text", "-s", "on", "-s", "x", "--help"],
    ["text", "--help", "-s", "x"],
    ["text", "--help", "news", "--help"],
    ["text", "extra"],
    ["images", "-c", "red"],
    ["videos", "-t", "y"],
    ["extract"],
    ["text"],
    ["books"],
    ["text", "--query="],
    ["version", "news"],
]


def output_reference():
    import contextlib
    import dataclasses
    import io
    import tempfile

    from ddgs.cli import _print_data, _save_csv, _save_json
    from ddgs.engines.duckduckgo_images import DuckduckgoImages
    from ddgs.engines.duckduckgo_videos import DuckduckgoVideos

    fixtures = Path("fixtures")

    def load(name):
        return json.loads((fixtures / name).read_text(encoding="utf-8"))

    def rows(results):
        return [dataclasses.asdict(r) for r in results]

    long_body = (
        "Rust is a general-purpose programming language emphasizing performance, type safety, "
        "and concurrency. It enforces memory safety -- meaning that all references point to valid "
        "memory -- without a garbage collector; self-contained, well-known, state-of-the-art "
        "multi-paradigm design. " * 3
    )
    datasets = {
        "text": load("brave_text_expected.json")
        + [
            {"title": "Wrap\ttest — a title that is long enough to need wrapping at seventy-eight columns, really", "href": "https://example.com/" + "a" * 320, "body": long_body},
            {"title": "", "href": "https://example.com/x", "body": ""},
        ],
        "images": load("bing_images_expected.json")
        + rows(DuckduckgoImages().extract_results((fixtures / "duckduckgo_images.json").read_text(encoding="utf-8"))),
        "videos": rows(DuckduckgoVideos().extract_results((fixtures / "duckduckgo_videos.json").read_text(encoding="utf-8"))),
        "news": load("bing_news_raw_expected.json") + load("yahoo_news_raw_expected.json"),
        "books": load("annasarchive_books_expected.json"),
    }
    out = []
    with tempfile.TemporaryDirectory() as tmp:
        for name, data in datasets.items():
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                _print_data(data, no_color=False)
            csv_path = Path(tmp) / f"{name}.csv"
            json_path = Path(tmp) / f"{name}.json"
            _save_csv(csv_path, data)
            _save_json(json_path, data)
            out.append(
                {
                    "name": name,
                    "rows": data,
                    "print": buf.getvalue(),
                    "csv": csv_path.read_bytes().decode("utf-8"),
                    "json": json_path.read_text(encoding="utf-8"),
                }
            )
    path = Path("fixtures/cli/output_reference.json")
    path.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(out)} datasets)", file=sys.stderr)


def extract_reference():
    """`DDGS().extract(url, fmt)` for HTML fixtures served on localhost."""
    import functools
    import http.server
    import threading

    from ddgs import DDGS

    files = ["extract_sample.html", "duckduckgo_text_page.html", "annasarchive_books_page.html", "yahoo_text_page.html"]
    formats = ["text_markdown", "text_plain", "text_rich", "text"]
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory="fixtures")
    handler.log_message = lambda *args: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    out = []
    try:
        for name in files:
            for fmt in formats:
                url = f"http://127.0.0.1:{server.server_address[1]}/{name}"
                data = DDGS().extract(url, fmt=fmt)
                out.append({"file": name, "fmt": fmt, "content": data["content"]})
    finally:
        server.shutdown()
    path = Path("fixtures/extract_reference.json")
    path.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(out)} cases)", file=sys.stderr)


def main():
    exe = shutil.which("ddgs")
    if exe is None:
        sys.exit("upstream `ddgs` is not on PATH (run this with uv, see the docstring)")
    env = {**os.environ, "COLUMNS": "80"}
    out = []
    for args in CASES:
        proc = subprocess.run(
            [exe, *args], capture_output=True, text=True, env=env, stdin=subprocess.DEVNULL, check=False
        )
        out.append({"args": args, "stdout": proc.stdout, "stderr": proc.stderr, "code": proc.returncode})
    path = Path("fixtures/cli/reference.json")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(out)} cases)", file=sys.stderr)
    output_reference()
    extract_reference()


if __name__ == "__main__":
    main()
