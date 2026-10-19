from bipbridge import agent_tools, pages
from conftest import AUTH
from test_agent_tools import _rpc

TOKEN = "agent-token-for-wellness-0123456789"

DASHBOARD = """<!DOCTYPE html>
<html lang="fr"><head><title>Prises de sang</title>
<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4.1/dist/chart.umd.min.js"></script>
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Inter">
</head><body><canvas id="c"></canvas>
<script>const data = {"ferritine": [[\"2026-03-02\", 41], [\"2026-09-15\", 58]]};</script></body></html>"""


def _call(client, name, arguments):
    return _rpc(client, TOKEN, "tools/call", {"name": name, "arguments": arguments}).json()["result"]


def test_a_page_is_locked_before_it_is_kept(config):
    made = pages.save(config, "wellness", DASHBOARD, "Prises de sang")
    kept = (pages.root(config) / "wellness" / f"{made['page']}.html").read_text()
    # The policy comes before anything the agent wrote, the doctype stays first.
    assert kept.startswith("<!DOCTYPE html>\n<meta http-equiv=\"Content-Security-Policy\"")
    assert kept.index("Content-Security-Policy") < kept.index("<html") < kept.index("<script")
    assert "connect-src 'none'" in kept and "form-action 'none'" in kept
    # Chart.js from the bridge, inline, instead of the CDN's; what else it asks from the network is reported.
    assert "cdn.jsdelivr.net" not in kept and "Chart.js v4" in kept and "<\\/script" not in kept[:200]
    assert made["outside"] == ["https://fonts.googleapis.com/css2?family=Inter"]
    # The agent's own source is kept beside it, never served.
    assert pages.source(config, "wellness", made["page"]) == DASHBOARD.strip()
    assert made["page"] == "prises-de-sang" and made["version"] == 1


def test_a_fragment_becomes_a_whole_locked_document(config):
    made = pages.save(config, "wellness", "<h1>Hello</h1><script>document.body.append('x')</script>", "Mon <b>titre</b>")
    kept = (pages.root(config) / "wellness" / "mon-b-titre-b.html").read_text()
    assert kept.startswith("<!doctype html>\n<meta http-equiv=\"Content-Security-Policy\"")
    assert "<title>Mon &lt;b&gt;titre&lt;/b&gt;</title><body><h1>Hello</h1>" in kept
    assert made["outside"] == []


def test_an_update_keeps_the_path_and_a_new_page_never_takes_another_s(config):
    first = pages.save(config, "wellness", "<p>v1</p>", "Suivi")
    second = pages.save(config, "wellness", "<p>autre</p>", "Suivi")
    assert (first["page"], second["page"]) == ("suivi", "suivi-2")
    updated = pages.save(config, "wellness", "<p>v2</p>", "", page="suivi")
    assert updated["path"] == first["path"] and updated["version"] == 2 and updated["title"] == "Suivi"
    assert updated["created"] == first["created"]
    assert pages.source(config, "wellness", "suivi") == "<p>v2</p>"
    assert [p["page"] for p in pages.list_pages(config, "wellness")][0] in ("suivi", "suivi-2")
    for bad in ("../x", "A B", "x" * 61):
        try:
            pages.save(config, "wellness", "<p>x</p>", "x", page=bad)
            raise AssertionError(bad)
        except ValueError:
            pass
    try:
        pages.save(config, "wellness", "x" * (pages.MAX_BYTES + 1), "big")
        raise AssertionError("too big")
    except ValueError as exc:
        assert "too big" in str(exc)
    assert pages.delete(config, "wellness", "suivi-2") and not pages.delete(config, "wellness", "suivi-2")
    pages.forget(config, "wellness")
    assert pages.list_pages(config, "wellness") == []


def test_an_agent_makes_a_page_and_the_app_gets_it_locked(client, config):
    agent_tools.save_tokens(config, {"wellness": agent_tools._digest(TOKEN)})
    names = [t["name"] for t in _rpc(client, TOKEN, "tools/list").json()["result"]["tools"]]
    assert {"create_page", "list_pages", "read_page", "delete_page"} <= set(names)
    result = _call(client, "create_page", {"html": DASHBOARD, "title": "Prises de sang"})
    text = result["content"][0]["text"]
    assert result["isError"] is False and "id prises-de-sang" in text and "fonts.googleapis.com" in text
    media = text.split("MEDIA:")[1].strip()
    assert media == "/bipagents/pages/wellness/prises-de-sang.html"
    page = client.get("/v1/media", params={"path": media}, headers=AUTH)
    assert page.status_code == 200 and page.headers["content-type"].startswith("text/html")
    assert page.headers["content-security-policy"].startswith("sandbox allow-scripts; default-src 'none'")
    assert page.headers["cache-control"] == "no-store" and page.headers["x-content-type-options"] == "nosniff"
    assert "Chart.js v4" in page.text
    # Its source and its record are not served.
    for other in ("prises-de-sang.source", "prises-de-sang.json"):
        assert client.get("/v1/media", params={"path": f"/bipagents/pages/wellness/{other}"}, headers=AUTH).status_code == 404
    # Updated with its id; listed, read back, deleted, by the agent or from the app.
    _call(client, "create_page", {"html": "<p>v2</p>", "title": "Prises de sang", "page": "prises-de-sang"})
    listed = _call(client, "list_pages", {})["content"][0]["text"]
    assert listed.startswith("prises-de-sang: Prises de sang (changed now ago, version 2)")
    assert _call(client, "read_page", {"page": "prises-de-sang"})["content"][0]["text"] == "<p>v2</p>"
    assert _call(client, "read_page", {"page": "nope"})["isError"] is True
    app_list = client.get("/v1/pages", params={"agent": "wellness"}, headers=AUTH).json()["pages"]
    assert [(p["page"], p["path"], p["version"]) for p in app_list] == [("prises-de-sang", media, 2)]
    assert client.get("/v1/pages", params={"agent": "nobody"}, headers=AUTH).status_code == 404
    assert client.delete("/v1/pages/prises-de-sang", params={"agent": "wellness"}, headers=AUTH).status_code == 200
    assert client.delete("/v1/pages/prises-de-sang", params={"agent": "wellness"}, headers=AUTH).status_code == 404
    assert _call(client, "list_pages", {})["content"][0]["text"] == "No page yet."
