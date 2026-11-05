"""Regenerate `fixtures/mcp/reference.json` from the upstream `ddgs mcp` server.

The upstream server (`ddgs/api_server/mcp.py`, built on the official `mcp`
Python SDK) runs in a subprocess and speaks JSON-RPC over stdio. This script
sends one request at a time and records the exact response line.

To capture how search results are framed without the network, the server
subprocess first replaces `DDGS.text/images/videos/news/books` with methods
that return rows from our fixtures (the same rows the Rust test feeds its
stubbed search). `extract_content` runs for real against a localhost server
that serves `fixtures/`.

The Rust test `src/server/mcp.rs` replays the same requests and compares.

Usage (from the repository root):

    uv run --no-project --with "ddgs[mcp]==9.16.1" python scripts/regen_mcp_reference.py
"""

import functools
import http.server
import json
import subprocess
import sys
import threading
from pathlib import Path

SERVER = r"""
import asyncio, json, sys
from pathlib import Path
import ddgs.ddgs as impl

rows = json.loads(Path("fixtures/mcp/search_rows.json").read_text(encoding="utf-8"))

def stub(category):
    def method(self, query=None, **kwargs):
        if not query:
            raise impl.DDGSException("query is mandatory.")
        return rows[category]
    return method

for category in ("text", "images", "videos", "news", "books"):
    setattr(impl.DDGS, category, stub(category))

from ddgs.api_server.mcp import mcp
asyncio.run(mcp.run_stdio_async())
"""


def load(name):
    return json.loads((Path("fixtures") / name).read_text(encoding="utf-8"))


def main():
    rows = {
        "text": load("brave_text_expected.json")[:2],
        "images": load("bing_images_expected.json"),
        "videos": [],
        "news": load("bing_news_raw_expected.json")[:1],
        "books": load("annasarchive_books_expected.json")[:1],
    }
    Path("fixtures/mcp").mkdir(parents=True, exist_ok=True)
    Path("fixtures/mcp/search_rows.json").write_text(json.dumps(rows, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")

    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory="fixtures")
    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{httpd.server_address[1]}"

    def call(name, arguments, req_id):
        return {"jsonrpc": "2.0", "id": req_id, "method": "tools/call", "params": {"name": name, "arguments": arguments}}

    init = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}},
    }
    requests = [
        init,
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        {"jsonrpc": "2.0", "id": 3, "method": "ping"},
        {"jsonrpc": "2.0", "id": 4, "method": "prompts/list"},
        {"jsonrpc": "2.0", "id": 5, "method": "resources/list"},
        {"jsonrpc": "2.0", "id": 6, "method": "resources/templates/list"},
        {"jsonrpc": "2.0", "id": 7, "method": "no/such/method"},
        call("no_such_tool", {}, 8),
        call("search_text", {}, 9),
        call("search_text", {"query": "rust"}, 10),
        call("search_text", {"query": ""}, 11),
        call("search_text", {"query": "rust", "max_results": "x"}, 12),
        call("search_images", {"query": "rust", "size": "Large"}, 13),
        call("search_videos", {"query": "rust"}, 14),
        call("search_news", {"query": "rust", "timelimit": "d"}, 15),
        call("search_books", {"query": "rust"}, 16),
        call("extract_content", {"url": f"{base}/extract_sample.html"}, 17),
        call("extract_content", {"url": f"{base}/extract_sample.html", "fmt": "text_plain"}, 18),
        call("extract_content", {"url": f"{base}/missing.html"}, 19),
        call("extract_content", {"url": f"{base}/extract_sample.html", "fmt": "content"}, 20),
        {"jsonrpc": "2.0", "id": 21, "method": "initialize", "params": {**init["params"], "protocolVersion": "2024-11-05"}},
        {"jsonrpc": "2.0", "id": 22, "method": "initialize", "params": {**init["params"], "protocolVersion": "1999-01-01"}},
        {"jsonrpc": "2.0", "id": 23, "method": "initialize", "params": {**init["params"], "protocolVersion": "2025-03-26"}},
        {"jsonrpc": "2.0", "id": 24, "method": "initialize", "params": {**init["params"], "protocolVersion": "2025-11-25"}},
        # Argument validation (pydantic, lax mode).
        call("search_text", {"query": 123}, 30),
        call("search_text", {"query": None}, 31),
        call("search_text", {"query": "rust", "max_results": 1.5}, 32),
        call("search_text", {"query": "rust", "max_results": 2.0}, 33),
        call("search_text", {"query": "rust", "max_results": True}, 34),
        call("search_text", {"query": "rust", "max_results": "7"}, 35),
        call("search_text", {"query": "rust", "max_results": " 7 "}, 36),
        call("search_text", {"query": "rust", "page": None}, 37),
        call("search_text", {"query": "rust", "timelimit": 5}, 38),
        call("search_text", {"query": "rust", "unknown": 1}, 39),
        call("search_text", {"max_results": "x", "page": [1]}, 40),
        call("search_text", {"region": "x" * 120}, 41),
        call("search_text", {"query": "rust", "region": {"a": 1}}, 42),
        call("search_text", {"query": "rust", "max_results": 10**20}, 43),
        call("search_text", {"query": "rust", "max_results": "1e3"}, 44),
        call("search_text", {"query": True}, 45),
        call("search_text", {"query": "rust", "max_results": -3}, 46),
        {"jsonrpc": "2.0", "id": 47, "method": "tools/call", "params": {"name": "search_text"}},
        call("extract_content", {"url": 5}, 48),
    ]

    proc = subprocess.Popen(
        [sys.executable, "-c", SERVER], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True
    )
    exchanges = []
    for request in requests:
        line = json.dumps(request)
        assert proc.stdin is not None and proc.stdout is not None
        proc.stdin.write(line + "\n")
        proc.stdin.flush()
        response = None
        if "id" in request:
            response = proc.stdout.readline().rstrip("\n")
        exchanges.append({"request": request, "response": response})
    assert proc.stdin is not None
    proc.stdin.close()
    proc.wait(timeout=30)
    httpd.shutdown()

    # The tool definitions are static data generated by upstream's MCP SDK from
    # the tool signatures; the Rust server embeds them verbatim.
    tools = json.loads(exchanges[2]["response"])["result"]["tools"]
    Path("src/server/mcp_tools.json").write_text(json.dumps(tools, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")

    # The localhost port changes on every run; store it as a placeholder.
    text = json.dumps(exchanges, ensure_ascii=False, indent=1).replace(base, "http://127.0.0.1:PORT")
    Path("fixtures/mcp/reference.json").write_text(text + "\n", encoding="utf-8")
    print(f"wrote fixtures/mcp/reference.json ({len(exchanges)} exchanges)", file=sys.stderr)


if __name__ == "__main__":
    main()
