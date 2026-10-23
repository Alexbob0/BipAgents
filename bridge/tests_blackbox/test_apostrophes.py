"""Apostrophes and accents (the owner, 2026-10-23: « l'enquête » came back as « l enquete »): straight (') and curly
(’) apostrophes and accents kept everywhere a person reads them — Carnet pages written by the agents (titles, facts,
« En bref »), the app's views of them (home, lists, search, page), records, the agents' pages (create_page: title,
body, the list) and the records the extraction from an archive writes. File names may be simplified; what is shown
never is."""
import json

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, notebook

TITLE = "L'enquête d’aujourd’hui"
FACT = "C'est l’été, aujourd’hui on relit l'enquête"


def test_a_carnet_page_keeps_its_apostrophes(bridge):
    give_token(bridge)
    in_french(bridge)
    made = call(bridge, "notebook_note", {"page": TITLE, "kind": "subject", "section": "Décisions", "facts": [FACT],
                                          "summary": "C'est l’affaire du voisin."})
    assert made["isError"] is False, text_of(made)
    assert TITLE in text_of(made) and FACT in text_of(made)
    files = list((notebook(bridge) / "carnet" / "Sujets").glob("*.md"))
    assert [f.name for f in files] == [f"{TITLE}.md"]
    text = files[0].read_text()
    assert f"# {TITLE}\n" in text and FACT in text and "C'est l’affaire du voisin." in text
    path = f"Sujets/{TITLE}.md"
    view = bridge.get("/v1/notebook/page", params={"path": path}, headers=AUTH).json()
    assert view["title"] == TITLE
    assert FACT in json.dumps(view, ensure_ascii=False)
    listed = bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"]
    assert [p["title"] for p in listed] == [TITLE]
    found = bridge.get("/v1/notebook/search", params={"q": "enquête"}, headers=AUTH).json()
    assert [p["title"] for p in found["pages"]] == [TITLE]
    # the agent finds it with or without the apostrophes, and reads it back as written
    for query in (TITLE, "L'enquete d'aujourd'hui", "l enquete d aujourd hui"):
        read = call(bridge, "notebook_read", {"page": query})
        assert read["isError"] is False and FACT in text_of(read), query
    home = bridge.get("/v1/notebook", headers=AUTH).json()
    assert TITLE in json.dumps(home["recent"], ensure_ascii=False)
    # a correction keeps them too
    fixed = call(bridge, "notebook_edit", {"page": TITLE, "old": FACT, "new": "Aujourd’hui c'est fini"})
    assert fixed["isError"] is False
    assert "Aujourd’hui c'est fini" in (notebook(bridge) / "carnet" / path).read_text()


def test_records_keep_their_apostrophes(bridge):
    give_token(bridge)
    in_french(bridge)
    added = call(bridge, "notebook_record", {"action": "add", "type": "purchase",
                                             "fields": {"date": "2026-01-10", "item": "Machine à café l’Expresso",
                                                        "store": "L'atelier d'Émile", "price": "189 €"}})
    assert added["isError"] is False
    page = (notebook(bridge) / "carnet" / "Fiches" / "Achats.md").read_text()
    assert "Machine à café l’Expresso" in page and "L'atelier d'Émile" in page
    view = bridge.get("/v1/notebook/records", headers=AUTH).json()
    item = next(t for t in view["types"] if t["type"] == "purchase")["items"][0]
    assert (item["item"], item["store"]) == ("Machine à café l’Expresso", "L'atelier d'Émile")
    listed = text_of(call(bridge, "notebook_record", {"action": "list", "type": "purchase"}))
    assert "Machine à café l’Expresso" in listed and "L'atelier d'Émile" in listed


def test_an_agent_page_keeps_its_apostrophes(bridge):
    give_token(bridge)
    html = f"<h1>{TITLE}</h1><p class=\"sub\">{FACT}</p><script>const t = 'l\\'été';</script>"
    made = call(bridge, "create_page", {"html": html, "title": TITLE})
    assert made["isError"] is False
    page_id = text_of(made).split("id ")[1].split(",")[0]
    assert page_id == "l-enquete-d-aujourd-hui"
    listed = text_of(call(bridge, "list_pages", {}))
    assert TITLE in listed
    served = bridge.get("/v1/media", params={"path": f"/bipagents/pages/wellness/{page_id}.html"}, headers=AUTH).text
    assert f"<h1>{TITLE}</h1>" in served and FACT in served and "'l\\'été'" in served
    record = json.loads((bridge.state_dir / "agent-pages" / "wellness" / f"{page_id}.json").read_text())
    assert record["title"] == TITLE
    # updated without a title: the one it has stays (never the id's words)
    call(bridge, "create_page", {"html": "<p>v2</p>", "title": "", "page": page_id})
    record = json.loads((bridge.state_dir / "agent-pages" / "wellness" / f"{page_id}.json").read_text())
    assert record["title"] == TITLE
    pages = bridge.get("/v1/agents/wellness/pages", headers=AUTH)
    if pages.status_code == 200:
        assert TITLE in json.dumps(pages.json(), ensure_ascii=False)
