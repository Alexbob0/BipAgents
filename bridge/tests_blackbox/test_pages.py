"""Pages the agents make (create_page and its kin over MCP), kept locked, served to the app through /v1/media, listed
and deleted from the app (tests/test_pages.py, over HTTP)."""
import json
import re

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc

DASHBOARD = """<!DOCTYPE html>
<html lang="fr"><head><title>Prises de sang</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js"></script>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter">
</head><body><canvas id="c"></canvas>
<script>const data = {"ferritine": [[\"2026-03-02\", 41], [\"2026-09-15\", 58]]};</script></body></html>"""


def pages_dir(bridge):
    return bridge.state_dir / "agent-pages" / "wellness"


def test_an_agent_makes_a_page_and_the_app_gets_it_locked(bridge):
    give_token(bridge)
    names = [t["name"] for t in rpc(bridge, "tools/list").json()["result"]["tools"]]
    assert {"create_page", "list_pages", "read_page", "delete_page"} <= set(names)
    result = call(bridge, "create_page", {"html": DASHBOARD, "title": "Prises de sang"})
    text = result["content"][0]["text"]
    assert result["isError"] is False and "id prises-de-sang" in text and "fonts.googleapis.com" in text
    assert text.startswith("Page ready: id prises-de-sang, version 1.\nWarning: the page asks for things on the network")
    media = text.split("MEDIA:")[1].strip()
    assert media == "/bipagents/pages/wellness/prises-de-sang.html"
    page = bridge.get("/v1/media", params={"path": media}, headers=AUTH)
    assert page.status_code == 200 and page.headers["content-type"].startswith("text/html")
    assert page.headers["content-security-policy"].startswith("sandbox allow-scripts; default-src 'none'")
    assert page.headers["cache-control"] == "no-cache" and page.headers["x-content-type-options"] == "nosniff"
    # kept by the app, asked again with its ETag or date: unchanged, a 304 without the body
    again = bridge.get("/v1/media", params={"path": media}, headers={**AUTH, "If-None-Match": page.headers["etag"]})
    assert again.status_code == 304 and again.content == b"" and again.headers["etag"] == page.headers["etag"]
    since = bridge.get("/v1/media", params={"path": media}, headers={**AUTH, "If-Modified-Since": page.headers["last-modified"]})
    assert since.status_code == 304
    assert bridge.get("/v1/media", params={"path": media}, headers={**AUTH, "If-None-Match": '"old"'}).status_code == 200
    assert "Chart.js v4" in page.text
    # The policy comes before anything the agent wrote, the doctype stays first; Chart.js inline instead of the CDN's.
    kept = (pages_dir(bridge) / "prises-de-sang.html").read_text()
    assert kept == page.text
    assert kept.startswith("<!DOCTYPE html>\n<meta http-equiv=\"Content-Security-Policy\"")
    assert kept.index("Content-Security-Policy") < kept.index("<style id=\"bip-kit\">") < kept.index("<html") < kept.index("<canvas")
    assert "connect-src 'none'" in kept and "form-action 'none'" in kept
    assert "cdn.jsdelivr.net" not in kept and "<\\/script" not in kept[:200]
    # The agent's own source is kept beside it, never served; its record too.
    assert (pages_dir(bridge) / "prises-de-sang.source").read_text() == DASHBOARD.strip()
    record = json.loads((pages_dir(bridge) / "prises-de-sang.json").read_text())
    assert record["title"] == "Prises de sang" and record["version"] == 1 and record["bytes"] == len(kept.encode())
    assert record["created"] == record["updated"] and re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\+00:00", record["created"])
    for other in ("prises-de-sang.source", "prises-de-sang.json"):
        assert bridge.get("/v1/media", params={"path": f"/bipagents/pages/wellness/{other}"}, headers=AUTH).status_code == 404
    # Updated with its id; listed, read back, deleted, by the agent or from the app.
    updated = call(bridge, "create_page", {"html": "<p>v2</p>", "title": "Prises de sang", "page": "prises-de-sang"})
    assert updated["content"][0]["text"].startswith("Page updated: id prises-de-sang, version 2.")
    listed = call(bridge, "list_pages", {})["content"][0]["text"]
    assert listed.startswith("prises-de-sang: Prises de sang (changed now ago, version 2)")
    assert call(bridge, "read_page", {"page": "prises-de-sang"})["content"][0]["text"] == "<p>v2</p>"
    nope = call(bridge, "read_page", {"page": "nope"})
    assert nope["isError"] is True and nope["content"][0]["text"] == "unknown page id: nope (see list_pages)"
    app_list = bridge.get("/v1/pages", params={"agent": "wellness"}, headers=AUTH).json()["pages"]
    assert [(p["page"], p["path"], p["version"], p["title"]) for p in app_list] == [
        ("prises-de-sang", media, 2, "Prises de sang")]
    assert set(app_list[0]) == {"page", "path", "title", "created", "updated", "version", "bytes"}
    assert bridge.get("/v1/pages", params={"agent": "nobody"}, headers=AUTH).status_code == 404
    assert bridge.get("/v1/pages", headers=AUTH).status_code == 422
    deleted = bridge.delete("/v1/pages/prises-de-sang", params={"agent": "wellness"}, headers=AUTH)
    assert deleted.status_code == 200 and deleted.json() == {"deleted": "prises-de-sang"}
    assert bridge.delete("/v1/pages/prises-de-sang", params={"agent": "wellness"}, headers=AUTH).status_code == 404
    assert call(bridge, "list_pages", {})["content"][0]["text"] == "No page yet."
    assert list(pages_dir(bridge).iterdir()) == []


def test_every_page_gets_the_kit_and_goes_inside_the_reply(bridge):
    """The page kit (the app's look, SVG charts, diagrams, sortable tables, what-ifs) is inline in every page, after the
    policy and before the agent's markup, so a page made in a few lines looks like the app and still opens offline; the
    tool's description teaches it with examples, and its result says to put the MEDIA: line where the page belongs."""
    give_token(bridge)
    tools = {t["name"]: t for t in rpc(bridge, "tools/list").json()["result"]["tools"]}
    described = tools["create_page"]["description"]
    assert "shown INSIDE your reply" in described and "THE KIT" in described and "Bip.bars(" in described
    assert '<pre class="diagram">' in described and 'class="kpi' in described and 'table class="sort"' in described
    source = ('<h1>Tes sorties</h1><div class="grid"><div class="kpi main"><span class="v">86</span></div></div>'
              '<div id="w"></div><script>Bip.bars("#w", {labels: ["S1", "S2"], values: [18, 21]})</script>')
    made = call(bridge, "create_page", {"html": source, "title": "Tes sorties"})
    text = made["content"][0]["text"]
    assert "put this line alone on its own line in your reply, where the page belongs in your text" in text
    assert text.endswith("\nMEDIA:/bipagents/pages/wellness/tes-sorties.html")
    page = bridge.get("/v1/media", params={"path": "/bipagents/pages/wellness/tes-sorties.html"}, headers=AUTH)
    assert page.headers["content-security-policy"].startswith("sandbox allow-scripts; default-src 'none'")
    kept = page.text
    assert kept.startswith("<!doctype html>\n<meta http-equiv=\"Content-Security-Policy\"")
    assert kept.index("connect-src 'none'") < kept.index('<style id="bip-kit">') < kept.index('<script id="bip-kit-js">')
    assert kept.index('<script id="bip-kit-js">') < kept.index("<h1>Tes sorties</h1>")
    assert ":where(.kpi .v)" in kept and "window.Bip = {" in kept and "Chart.js v4" not in kept
    # nothing of the kit reaches the network: no URL but the SVG namespace
    kit = kept[kept.index('<style id="bip-kit">'):kept.index("<title>")]
    assert re.findall(r"https?://[^\s\"')]+", kit) == ["http://www.w3.org/2000/svg"]


def test_a_fragment_becomes_a_whole_locked_document(bridge):
    give_token(bridge)
    made = call(bridge, "create_page", {"html": "<h1>Hello</h1><script>document.body.append('x')</script>",
                                        "title": "Mon <b>titre</b>"})
    assert "id mon-b-titre-b," in made["content"][0]["text"] and "Warning" not in made["content"][0]["text"]
    kept = (pages_dir(bridge) / "mon-b-titre-b.html").read_text()
    assert kept.startswith("<!doctype html>\n<meta http-equiv=\"Content-Security-Policy\"")
    assert "<title>Mon &lt;b&gt;titre&lt;/b&gt;</title><body><h1>Hello</h1>" in kept


def test_an_update_keeps_the_path_and_a_new_page_never_takes_another_s(bridge):
    give_token(bridge)
    first = call(bridge, "create_page", {"html": "<p>v1</p>", "title": "Suivi"})
    second = call(bridge, "create_page", {"html": "<p>autre</p>", "title": "Suivi"})
    assert "id suivi," in first["content"][0]["text"] and "id suivi-2," in second["content"][0]["text"]
    created = json.loads((pages_dir(bridge) / "suivi.json").read_text())["created"]
    call(bridge, "create_page", {"html": "<p>v2</p>", "title": "", "page": "suivi"})
    record = json.loads((pages_dir(bridge) / "suivi.json").read_text())
    assert record["version"] == 2 and record["title"] == "Suivi" and record["created"] == created
    for bad in ("../x", "A B", "x" * 61):
        refused = call(bridge, "create_page", {"html": "<p>x</p>", "title": "x", "page": bad})
        assert refused["isError"] is True and refused["content"][0]["text"].startswith("unknown page id: ")
    big = call(bridge, "create_page", {"html": "x" * 1_000_001, "title": "big"})
    assert big["isError"] is True and "too big" in big["content"][0]["text"]
    empty = call(bridge, "create_page", {"html": " ", "title": "vide"})
    assert empty["content"][0]["text"] == "the page is empty: give its whole HTML"
    assert call(bridge, "delete_page", {"page": "suivi-2"})["content"][0]["text"] == "Page suivi-2 deleted."
    gone = call(bridge, "delete_page", {"page": "suivi-2"})
    assert gone["isError"] is True and gone["content"][0]["text"] == "unknown page id: suivi-2 (see list_pages)"
