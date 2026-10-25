"""« Nouvelles passées » (`GET /v1/family/history`, family_news.rs): what this install's agents passed on to relatives
(sent, refused by the person, failed) and what they received, the newest first, over two linked bridges."""
import json
import time

from bbkit.common import AUTH
from test_households_and_peers import household


def link(a, b):
    assert a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert b.put("/v1/family/name", headers=AUTH, json={"name": "Léa"}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200, joined.text


def propose(bridge, pid, to, text):
    """A proposal as `tell_family` keeps it (the agent's MCP call is covered by the Rust tests)."""
    path = bridge.state_dir / "family-news.json"
    data = json.loads(path.read_text()) if path.exists() else {}
    data.setdefault("proposals", []).append({"id": pid, "agent": "vie", "text": text, "to": to, "created": time.time()})
    path.write_text(json.dumps(data))


def test_past_news_lists_what_went_and_came_newest_first(bridges, fakes):
    a = household(bridges, fakes, "alex", ["vie"])
    b = household(bridges, fakes, "lea", ["cuisine"])
    link(a, b)
    assert a.get("/v1/family/history", headers=AUTH).json() == {"items": [], "more": False}

    propose(a, "f1", [{"id": "lea", "name": "Léa"}], "Je rentre tard")
    sent = a.post("/v1/family/proposals/f1/send", headers=AUTH, json={"text": "Je rentre vers 21 h"})
    assert sent.status_code == 200 and sent.json() == {"sent": ["Léa"], "failed": []}
    propose(a, "f2", [{"id": "cuisine__lea", "name": "Cuisine (Léa)"}], "Plus de lait")
    assert a.delete("/v1/family/proposals/f2", headers=AUTH).status_code == 200
    propose(a, "f3", [{"id": "paul", "name": "Paul"}], "Coucou")           # a household not linked: not reached
    assert a.post("/v1/family/proposals/f3/send", headers=AUTH).status_code == 502

    past = a.get("/v1/family/history", headers=AUTH).json()
    assert past["more"] is False
    lines = [(x["state"], x["household"], x["household_name"], x["other_agent"], x["text"]) for x in past["items"]]
    assert lines == [("failed", "paul", "Paul", None, "Coucou"),
                     ("refused", "lea", "Léa", "Cuisine", "Plus de lait"),
                     ("sent", "lea", "Léa", None, "Je rentre vers 21 h")]
    first = past["items"][0]
    assert set(first) == {"id", "at", "direction", "agent", "agent_name", "household", "household_name",
                          "other_agent", "text", "state"}
    assert first["direction"] == "sent" and first["agent"] == "vie" and first["agent_name"] == "Vie"

    page = a.get("/v1/family/history?limit=2", headers=AUTH).json()
    assert len(page["items"]) == 2 and page["more"] is True
    rest = a.get(f"/v1/family/history?limit=2&before={page['items'][-1]['at']}", headers=AUTH).json()
    assert [x["state"] for x in rest["items"]] == ["sent"] and rest["more"] is False
    assert a.get("/v1/family/history?agent=ghost", headers=AUTH).status_code == 404
    assert a.get("/v1/family/history?direction=both", headers=AUTH).status_code == 422
    assert a.get("/v1/family/history?limit=0", headers=AUTH).status_code == 422
    assert a.get("/v1/family/history").status_code == 401

    theirs = b.get("/v1/family/history", headers=AUTH).json()["items"]
    assert [(x["direction"], x["agent"], x["household_name"], x["other_agent"], x["text"], x["state"])
            for x in theirs] == [("received", "cuisine", "Alex", "Vie", "Je rentre vers 21 h", "received")]
    assert b.get("/v1/family/history?direction=sent", headers=AUTH).json()["items"] == []
