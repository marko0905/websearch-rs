"""Regenerate `fixtures/api/reference.json` from the upstream `ddgs api` server.

The upstream FastAPI app (`ddgs/api_server/api.py`) runs under uvicorn in a
subprocess, with `DDGS.text/images/videos/news/books` replaced by methods that
return the rows in `fixtures/mcp/search_rows.json` (so no network is needed).
`/extract` runs for real against a localhost server that serves `fixtures/`.

For each request the script records the status, the response headers (except
`date`) and the body. It also stores the generated `/openapi.json` and the
`/docs`, `/redoc` and `/docs/oauth2-redirect` pages in `src/server/api_static/`, which the Rust server
embeds. The Rust test `src/server/api.rs` replays the requests and compares.

Usage (from the repository root):

    uv run --no-project --with "ddgs[api]==9.16.1" python scripts/regen_api_reference.py
"""

import functools
import http.client
import http.server
import json
import socket
import subprocess
import sys
import threading
import time
from pathlib import Path

SERVER = r"""
import json, sys
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

import uvicorn
uvicorn.run("ddgs.api_server:fastapi_app", host="127.0.0.1", port=int(sys.argv[1]), log_level="warning")
"""


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def main():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory="fixtures")
    files = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=files.serve_forever, daemon=True).start()
    files_base = f"http://127.0.0.1:{files.server_address[1]}"

    port = free_port()
    proc = subprocess.Popen([sys.executable, "-c", SERVER, str(port)], stderr=subprocess.DEVNULL)
    for _ in range(100):
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
            break
        except OSError:
            time.sleep(0.1)

    q = "/search/text"
    requests = [
        ("GET", "/", None, {}),
        ("GET", "/health", None, {}),
        ("GET", "/nope", None, {}),
        ("DELETE", "/health", None, {}),
        ("POST", q, {"query": "rust"}, {}),
        ("POST", q, {"query": ""}, {}),
        ("POST", q, {}, {}),
        ("POST", q, {"query": 5, "max_results": "x", "page": None}, {}),
        ("POST", q, {"query": "rust", "max_results": None}, {}),
        ("POST", q, "not json", {"Content-Type": "application/json"}),
        ("POST", q, None, {}),
        ("GET", q + "?query=rust&max_results=2", None, {}),
        ("GET", q, None, {}),
        ("GET", q + "?query=rust&max_results=x", None, {}),
        ("GET", q + "?query=", None, {}),
        ("POST", "/search/images", {"query": "rust", "size": "Large"}, {}),
        ("GET", "/search/images?query=rust", None, {}),
        ("POST", "/search/videos", {"query": "rust"}, {}),
        ("GET", "/search/videos?query=rust", None, {}),
        ("POST", "/search/news", {"query": "rust"}, {}),
        ("GET", "/search/news?query=rust", None, {}),
        ("POST", "/search/books", {"query": "rust"}, {}),
        ("GET", "/search/books?query=rust", None, {}),
        ("POST", "/search/books", {"query": ""}, {}),
        ("GET", "/search/news?query=", None, {}),
        ("POST", "/extract", {"url": f"{files_base}/extract_sample.html"}, {}),
        ("POST", "/extract", {"url": f"{files_base}/extract_sample.html", "format": "text_plain"}, {}),
        ("GET", f"/extract?url={files_base}/extract_sample.html&fmt=text_rich", None, {}),
        ("GET", f"/extract?url={files_base}/missing.html", None, {}),
        ("POST", "/extract", {}, {}),
        ("GET", "/health", None, {"Origin": "https://example.com"}),
        (
            "OPTIONS",
            q,
            None,
            {"Origin": "https://example.com", "Access-Control-Request-Method": "POST", "Access-Control-Request-Headers": "content-type"},
        ),
        ("OPTIONS", q, None, {}),
        ("GET", "/openapi.json", None, {}),
        ("GET", "/docs", None, {}),
        ("GET", "/redoc", None, {}),
        ("GET", "/docs/oauth2-redirect", None, {}),
        ("GET", "/health/", None, {"Host": "localhost:4479"}),
        ("HEAD", "/health", None, {}),
        ("GET", q + "?query=a&query=rust&max_results=1", None, {}),
        ("GET", q + "?query=rust%20lang&region=%20x", None, {}),
        ("POST", q, [1, 2], {}),
        ("POST", q, '"str"', {"Content-Type": "application/json"}),
        ("POST", q, '{"query": "rust"', {"Content-Type": "application/json"}),
        ("POST", q, '{"query": "rust",}', {"Content-Type": "application/json"}),
        ("POST", q, '{"query": "rust"} x', {"Content-Type": "application/json"}),
        ("POST", q, '{"query": "rust"}', {"Content-Type": "text/plain"}),
        ("POST", q, '{"query": "rust"}', {}),
        ("POST", q, {"query": "rust", "page": "2", "max_results": 1.0}, {}),
        ("POST", q, {"query": "rust", "max_results": 1.5}, {}),
        ("GET", "/search/text?query=rust&page=", None, {}),
        ("GET", "/search/images?query=rust&size=", None, {}),
        ("GET", "/extract", None, {}),
    ]
    out = []
    for method, path, body, headers in requests:
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
        payload = None
        hdrs = dict(headers)
        if isinstance(body, (dict, list)):
            payload = json.dumps(body).encode()
            hdrs.setdefault("Content-Type", "application/json")
        elif isinstance(body, str):
            payload = body.encode()
        conn.request(method, path, body=payload, headers=hdrs)
        resp = conn.getresponse()
        data = resp.read().decode("utf-8")
        out.append(
            {
                "method": method,
                "path": path,
                "body": body,
                "headers": headers,
                "status": resp.status,
                "response_headers": [[k.lower(), v] for k, v in resp.getheaders() if k.lower() != "date"],
                "response": data,
            }
        )
        conn.close()

    static = Path("src/server/api_static")
    static.mkdir(parents=True, exist_ok=True)
    for path, name in (
        ("/openapi.json", "openapi.json"),
        ("/docs", "docs.html"),
        ("/redoc", "redoc.html"),
        ("/docs/oauth2-redirect", "oauth2-redirect.html"),
    ):
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
        conn.request("GET", path)
        resp = conn.getresponse()
        (static / name).write_bytes(resp.read())
        conn.close()

    proc.terminate()
    proc.wait(timeout=10)
    files.shutdown()
    text = json.dumps(out, ensure_ascii=False, indent=1).replace(files_base, "http://127.0.0.1:PORT")
    Path("fixtures/api").mkdir(parents=True, exist_ok=True)
    Path("fixtures/api/reference.json").write_text(text + "\n", encoding="utf-8")
    print(f"wrote fixtures/api/reference.json ({len(out)} requests)", file=sys.stderr)


if __name__ == "__main__":
    main()
