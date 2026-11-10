"""Carnet v2, B2 (internal/notes/carnet-v2/contract.md §3.2-3.5, §6): the search's direct answer split by the bridge,
the offline copy (ETag, 304), « Ce que tes agents savent de toi », and « Oublie ça »: an agent only shows it (a card,
never a token), the app's preview gives a token for 10 minutes, once, and the confirmation erases the fact from the
Carnet, its history and the agents' memories, so that an old turn cannot write it back. Scripted, no model."""
import json

from bbkit.common import AUTH
from bbkit.mcp import call as mcp_call, give_token, text_of


def in_french(bridge):
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200


def carnet(bridges):
    bridge = bridges.new().start()
    give_token(bridge)
    in_french(bridge)
    done = mcp_call(bridge, "notebook_write", {"page": "Léa Martin", "kind": "person", "aliases": ["Léa"],
                                               "facts": ["Pointure : 36", "Aime le chocolat"]})
    assert done["isError"] is False, text_of(done)
    return bridge


def test_the_search_answers_directly_and_the_phone_keeps_a_copy(bridges, fakes):
    bridge = carnet(bridges)
    found = bridge.get("/v1/notebook/search", params={"q": "Léa chausse du combien ?", "lang": "fr"}, headers=AUTH)
    answer = found.json()["answers"][0]
    assert {k: answer[k] for k in ("label", "answer", "page", "path", "part")} == {
        "label": "Pointure", "answer": "36", "page": "Léa Martin", "path": "Personnes/Léa Martin.md", "part": "main"}
    assert answer["when"].startswith("noté le ")
    page = bridge.get("/v1/notebook/page", params={"path": "Personnes/Léa Martin.md"}, headers=AUTH).json()
    fact = next(f for s in page["sections"] for f in s["facts"] if f["text"] == "Pointure : 36")
    assert (fact["label"], fact["answer"]) == ("Pointure", "36") and fact["when"].startswith("noté le ")
    copy = bridge.get("/v1/notebook/offline", params={"lang": "fr"}, headers=AUTH)
    assert copy.status_code == 200 and copy.headers["cache-control"] == "no-store"
    etag = copy.headers["etag"]
    body = copy.json()
    assert body["v"] == 1 and body["as_of"].startswith("Hors connexion · à jour du ")
    assert any(f["answer"] == "36" and f["words"] == ["lea", "martin", "pointure", "36"] for f in body["facts"])
    assert "chausse du" in body["labels"]["pointure"] and "de" in body["stop"]
    again = bridge.get("/v1/notebook/offline", params={"lang": "fr"}, headers={**AUTH, "If-None-Match": etag})
    assert again.status_code == 304 and again.headers["etag"] == etag
    # the home's tile
    home = bridge.get("/v1/notebook", params={"lang": "fr"}, headers=AUTH).json()
    assert home["known_tile"]["title"] == "Ce que tes agents savent"


def test_what_the_agents_know(bridges, fakes):
    bridge = carnet(bridges)
    assert text_of(mcp_call(bridge, "notebook_read", {"page": "Léa"})).startswith("# Léa Martin")
    known = bridge.get("/v1/notebook/known", params={"lang": "fr"}, headers=AUTH)
    assert known.status_code == 200, known.text
    view = known.json()
    assert view["title"] == "Ce que tes agents savent de toi" and view["agents"][0] == {"id": "", "name": "Tous"}
    day = view["reads"]["days"][0]
    assert day["day"] == "Aujourd'hui" and day["rows"][0]["text"] == "Léa"
    assert day["rows"][0]["pages"] == [{"title": "Léa", "path": "Personnes/Léa Martin.md"}]
    assert view["reads"]["older"] == "Plus ancien : effacé après 90 jours." and view["reads"]["empty"] is None
    assert "36" not in json.dumps(view) and "chocolat" not in json.dumps(view), "names only, never a page's text"
    assert view["footer"].endswith("Kerfy Studio ne lit pas ton Carnet. Pas de pub.")


def test_forget_is_shown_by_the_agent_and_confirmed_only_in_the_app(bridges, fakes):
    bridge = carnet(bridges)
    # the agent: a preview put in its turn's card, nothing erased, no token
    shown = mcp_call(bridge, "notebook_write", {"page": "Léa", "forget": ["Pointure : 36"]})
    assert text_of(shown) == "Shown to the person: they confirm in the app.", text_of(shown)
    assert "fg-" not in json.dumps(shown)
    lea = bridge.state_dir / "notebook" / "carnet" / "Personnes" / "Léa Martin.md"
    assert "Pointure : 36" in lea.read_text()
    tried = mcp_call(bridge, "notebook_write", {"page": "Léa", "forget": ["fg-0123456789abcdef01234567"]})
    assert text_of(tried) == "Only the person confirms, in the app."
    # the app: the preview, then the token, once
    preview = bridge.post("/v1/notebook/forget/preview", params={"lang": "fr"},
                          json={"texts": ["Pointure : 36"], "page": "Léa"}, headers=AUTH)
    assert preview.status_code == 200, preview.text
    view = preview.json()
    assert view["title"] == "Oublier « Pointure : 36 » ?" and view["found_title"] == "Trouvé dans :"
    assert {"text": "Carnet · Léa"} in view["places"] and {"text": "Historique du Carnet"} in view["places"]
    assert view["notes"][0] == "Reste dans la conversation où tu l'as dit."
    assert (view["confirm"], view["cancel"], view["warning"]) == ("Oublier", "Annuler", "On ne pourra pas le remettre.")
    etag = bridge.get("/v1/notebook/offline", headers=AUTH).headers["etag"]
    done = bridge.post("/v1/notebook/forget/confirm", params={"lang": "fr"}, json={"token": view["token"]}, headers=AUTH)
    assert done.status_code == 200 and done.json() == {"done": True, "text": "Oublié."}
    again = bridge.post("/v1/notebook/forget/confirm", params={"lang": "fr"}, json={"token": view["token"]}, headers=AUTH)
    assert again.status_code == 410 and again.json()["detail"] == {"code": "token_expired",
                                                                   "text": "C'est trop ancien. Recommence."}
    # gone from the page, its history, the search and the copy (whose ETag changed); the rest of the page kept
    assert "Pointure" not in lea.read_text() and "chocolat" in lea.read_text()
    history = bridge.state_dir / "notebook" / "history"
    for file in [history / "log.jsonl", *(history / "versions").glob("*.md")]:
        assert "Pointure : 36" not in file.read_text(), file
    assert bridge.get("/v1/notebook/search", params={"q": "Pointure de Léa"}, headers=AUTH).json()["answers"] == []
    assert bridge.get("/v1/notebook/offline", headers={**AUTH, "If-None-Match": etag}).status_code == 200
    # an old turn writes it back, said another way: not written
    mcp_call(bridge, "notebook_write", {"page": "Léa", "facts": ["Chausse du 36"]})
    assert "36" not in lea.read_text()
    # nothing to forget, or two Léa: said so
    nothing = bridge.post("/v1/notebook/forget/preview", json={"texts": ["Aime le ski"], "page": "Léa"}, headers=AUTH)
    assert nothing.status_code == 404 and nothing.json()["detail"]["code"] == "nothing_found"
    mcp_call(bridge, "notebook_write", {"page": "Léa Petit", "kind": "person", "facts": ["Aime le thé"]})
    two = bridge.post("/v1/notebook/forget/preview", params={"lang": "fr"}, json={"texts": ["Aime le thé"], "page": "Léa"},
                      headers=AUTH)
    assert two.status_code == 409 and two.json()["detail"]["code"] == "several_pages", two.text
