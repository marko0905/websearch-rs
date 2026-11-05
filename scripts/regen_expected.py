"""Regenerate the `fixtures/*_expected.json` files with upstream ddgs.

The Rust parser tests compare our output with what upstream `ddgs` extracts
from the same fixture pages. Run this script when a fixture changes or when
the tracked upstream version changes.

Usage (from the repository root):

    uv run --no-project --with ddgs==9.16.1 python scripts/regen_expected.py

Optional arguments: the fixtures directory (default `fixtures`) and the
output directory (default: the fixtures directory).

For each upstream HTML engine (text bing, brave, duckduckgo, mojeek,
startpage, yahoo, yandex; news bing and yahoo; images bing; books
annasarchive) the script reads the engine's HTML fixture, runs the upstream
`extract_results` and `post_extract_results`, and writes
`<fixture stem>_expected.json`. The news engines post-process dates relative
to "now", so for them it writes the raw `extract_results` output to
`<fixture stem>_raw_expected.json` instead.
It also writes `grokipedia_text_expected.json` and three `wikipedia_*_expected.json`
files (normal, disambiguation, and "may also refer to:").

Parser parity: for every `*.html` fixture it writes `parser/<stem>.json`, a
dump of the whole tree that `lxml.html.fromstring` (with the ddgs parser)
builds: every node of `//node()` in document order, elements as
`["e", tag, [[attr, value], ...]]` and text as `["t", text]`, plus the tag
of the element `fromstring` returned. The Rust test `html::parser_parity`
compares our libxml2 tree with these dumps.
"""

import dataclasses
import importlib.metadata
import json
import sys
from pathlib import Path

from ddgs.engines.annasarchive import AnnasArchive
from ddgs.engines.bing import Bing
from ddgs.engines.bing_images import BingImages
from ddgs.engines.bing_news import BingNews
from ddgs.engines.brave import Brave
from ddgs.engines.duckduckgo import Duckduckgo
from ddgs.engines.grokipedia import Grokipedia
from ddgs.engines.mojeek import Mojeek
from ddgs.engines.startpage import Startpage
from ddgs.engines.wikipedia import Wikipedia
from ddgs.engines.yahoo import Yahoo
from ddgs.engines.yahoo_news import YahooNews
from ddgs.engines.yandex import Yandex

# (fixture stem, engine class, run post_extract_results?)
HTML_ENGINES = [
    ("bing_text_page", Bing, True),
    ("brave_text_page", Brave, True),
    ("duckduckgo_text_page", Duckduckgo, True),
    ("mojeek_text_page", Mojeek, True),
    ("startpage_text_page", Startpage, True),
    ("yahoo_text_page", Yahoo, True),
    ("yandex_text_page", Yandex, True),
    ("bing_news_page", BingNews, False),
    ("yahoo_news_page", YahooNews, False),
    ("bing_images_page", BingImages, True),
    ("annasarchive_books_page", AnnasArchive, True),
]

# Anna's Archive picks a random mirror at import time; pin one for stable output.
ANNAS_SEARCH_URL = "https://annas-archive.gd/search"

# (opensearch fixture, extracts fixture, expected output name)
WIKIPEDIA_CASES = [
    ("wikipedia_opensearch.json", "wikipedia_extracts.json", "wikipedia_text_expected.json"),
    (
        "wikipedia_opensearch_disambiguation.json",
        "wikipedia_extracts_disambiguation.json",
        "wikipedia_disambiguation_expected.json",
    ),
    (
        "wikipedia_opensearch_may_also_refer.json",
        "wikipedia_extracts_may_also_refer.json",
        "wikipedia_may_also_refer_expected.json",
    ),
]


def dump(results, path):
    rows = [dataclasses.asdict(result) for result in results]
    path.write_text(json.dumps(rows, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(rows)} results)", file=sys.stderr)


def dump_tree(root, path):
    nodes = []
    for node in root.xpath("//node()"):
        if isinstance(node, str):
            nodes.append(["t", str(node)])
        else:
            nodes.append(["e", node.tag, [[k, v] for k, v in node.attrib.items()]])
    data = {"root": root.tag, "nodes": nodes}
    path.write_text(json.dumps(data, ensure_ascii=False, indent=0) + "\n", encoding="utf-8")
    print(f"wrote {path} ({len(nodes)} nodes)", file=sys.stderr)


def main():
    fixtures = Path(sys.argv[1] if len(sys.argv) > 1 else "fixtures")
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else fixtures
    print("ddgs", importlib.metadata.version("ddgs"), file=sys.stderr)

    for stem, engine_class, post in HTML_ENGINES:
        engine = engine_class()
        if engine_class is AnnasArchive:
            engine.search_url = ANNAS_SEARCH_URL
        html = (fixtures / f"{stem}.html").read_text(encoding="utf-8")
        results = engine.extract_results(html)
        if post:
            results = engine.post_extract_results(results)
            name = stem.removesuffix("_page") + "_expected.json"
        else:
            name = stem.removesuffix("_page") + "_raw_expected.json"
        dump(results, out / name)

    parser_dir = out / "parser"
    parser_dir.mkdir(exist_ok=True)
    for path in sorted(fixtures.glob("*.html")):
        html = path.read_text(encoding="utf-8")
        if path.name == "annasarchive_books_page.html":
            html = AnnasArchive().pre_process_html(html)
        dump_tree(Brave().extract_tree(html), parser_dir / f"{path.stem}.json")

    grokipedia = Grokipedia()
    body = (fixtures / "grokipedia_typeahead.json").read_text(encoding="utf-8")
    dump(grokipedia.extract_results(body), out / "grokipedia_text_expected.json")

    for opensearch, extracts, expected in WIKIPEDIA_CASES:
        wikipedia = Wikipedia()
        wikipedia.lang = "en"
        extracts_body = (fixtures / extracts).read_text(encoding="utf-8")
        # Upstream fetches the extracts with a second request; replay the fixture.
        wikipedia.request = lambda *args, _body=extracts_body, **kwargs: _body
        results = wikipedia.extract_results((fixtures / opensearch).read_text(encoding="utf-8"))
        dump(results, out / expected)


if __name__ == "__main__":
    main()
