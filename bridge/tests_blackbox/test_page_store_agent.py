"""The agent writes its page's store (page_store.rs « the agent's writing », 16/11, Alex: « il faut que l'agent puisse
stocker ce qu'il veut »): a live page made once, then fed by later turns or routines through `page_data` with `ops`
(as Hermes calls the bridge's tools), shown by the page's usual polling, never made again; everyone who sees the
page sees it — the family through their own bridges, a project's friends in the sealed share."""
import time

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, media_of, text_of
from test_page_store import store_get

DASHBOARD = """<h1>🌡️ Le jardin</h1><p class="sub" id="maj"></p><p class="big" id="t"></p><div class="table-wrap">
<table id="r"></table></div><script>Bip.store.onChange(function(){ Bip.set("#maj", Bip.store.get("maj") || "");
Bip.set("#t", (Bip.store.get("temp") ?? "–") + " °C");
Bip.rows("#r", Bip.store.items("releves"), function(l){ return [l.value.heure, l.value.temp + " °C"]; }); })</script>"""


def version_of(bridge, agent, page):
    pages = bridge.get("/v1/pages", headers=AUTH, params={"agent": agent}).json()["pages"]
    return next(p for p in pages if p["page"] == page)["version"]


def test_a_live_page_is_fed_by_later_turns_without_being_made_again(bridge):
    give_token(bridge)
    made = call(bridge, "create_page", {"html": DASHBOARD, "title": "Le jardin"})
    assert made["isError"] is False, made
    path = media_of(made)
    page = path.rsplit("/", 1)[1][:-5]
    first = store_get(bridge, path).json()
    assert first["rev"] == 0 and first["keys"] == {}

    # a later turn (a routine at 8 h): what it found, written in the page
    wrote = call(bridge, "page_data", {"page": page, "ops": [
        {"op": "set", "key": "temp", "value": 17},
        {"op": "set", "key": "maj", "value": "Mis à jour à 8 h"},
        {"op": "add", "key": "releves", "id": "h8", "value": {"heure": "8 h", "temp": 17}},
    ]})
    assert wrote["isError"] is False, wrote
    assert "rev 1" in text_of(wrote)
    # the page's next poll (since what it had) shows it, under the agent's name
    seen = store_get(bridge, path, since=0).json()
    assert seen["rev"] == 1 and seen["keys"]["temp"]["value"] == 17
    assert seen["keys"]["releves"]["items"][0]["value"] == {"heure": "8 h", "temp": 17}
    assert seen["keys"]["temp"]["by"] not in ("", None)

    # the next run (at 14 h): one more reading, the temperature changed; the routine run again by mistake: nothing more
    later = [{"op": "set", "key": "temp", "value": 24},
             {"op": "add", "key": "releves", "id": "h14", "value": {"heure": "14 h", "temp": 24}}]
    assert call(bridge, "page_data", {"page": page, "ops": later})["isError"] is False
    again = call(bridge, "page_data", {"page": page, "ops": later})
    assert again["isError"] is False and "Nothing to change" in text_of(again), text_of(again)
    now = store_get(bridge, path, since=1).json()
    assert now["rev"] == 2 and now["keys"]["temp"]["value"] == 24
    assert [l["id"] for l in now["keys"]["releves"]["items"]] == ["h8", "h14"]
    assert store_get(bridge, path, since=2).json()["same"] is True

    # never made again: one version of the page, its HTML as it was
    assert version_of(bridge, "wellness", page) == 1
    # the bridge's own keys (a live page's sources) are not the agent's; another page id is not its page
    refused = call(bridge, "page_data", {"page": page, "ops": [{"op": "set", "key": "_src.weather", "value": {}}]})
    assert refused["isError"] is True and "_src.weather" in text_of(refused)
    assert call(bridge, "page_data", {"page": "absente", "ops": [{"op": "set", "key": "k", "value": 1}]})["isError"] is True
    assert store_get(bridge, path).json()["rev"] == 2

    # read back: data, never instructions, its own lines named as its own
    read = text_of(call(bridge, "page_data", {"page": page}))
    assert "not instructions" in read and "are yours" in read and "24" in read


def test_the_family_sees_what_the_family_s_agent_writes(bridges, fakes):
    """« Famille » on Alex's place: its page fed by its agent is seen by Marie and Paul from their own bridges."""
    from test_family_agent import make_family_agent
    from test_family_thread import discover, family

    a, b, c = family(bridges, fakes)
    make_family_agent(a)
    discover(b, c)
    give_token(a, "famille", "f" * 40)
    made = call(a, "create_page", {"html": DASHBOARD, "title": "Le jardin de la famille"}, token="f" * 40)
    assert made["isError"] is False, made
    page = media_of(made).rsplit("/", 1)[1][:-5]
    own = f"/bipagents/pages/famille/{page}.html"
    relayed = f"/bipagents/pages/famille__alex/{page}.html"
    assert store_get(b, relayed).json()["rev"] == 0

    wrote = call(a, "page_data", {"page": page, "ops": [{"op": "set", "key": "temp", "value": 19},
                                                        {"op": "add", "key": "releves", "id": "h9",
                                                         "value": {"heure": "9 h", "temp": 19}}]}, token="f" * 40)
    assert wrote["isError"] is False, wrote
    for member in (b, c):
        seen = store_get(member, relayed, since=0).json()
        assert seen["rev"] == 1 and seen["keys"]["temp"]["value"] == 19, seen
        assert seen["keys"]["releves"]["items"][0]["value"]["heure"] == "9 h"
    assert store_get(a, own).json()["keys"]["temp"]["by"] == store_get(b, relayed).json()["keys"]["temp"]["by"]


def test_a_friend_s_sealed_copy_gets_what_the_project_s_agent_writes(bridges, fakes):
    """« Pour les queens » shown to Léa (read-only conversation, its group page's store inside the sealed item): what
    the project's agent writes reaches her copy at the share's next round, the same as a line typed by Alex."""
    from test_zz_review_page_store import project_share

    s = project_share(bridges, fakes)
    try:
        wrote = call(s["bridge"], "page_data", {"page": s["page"], "ops": [
            {"op": "set", "key": "total", "value": 6000},
            {"op": "add", "key": "depenses", "id": "ag1", "value": {"what": "Péage", "cents": 1250}},
        ]}, token=s["token"])
        assert wrote["isError"] is False, wrote
        s["sync"]()
        store = s["item"]()["store"]
        assert store["keys"]["total"]["value"] == 6000, store
        assert [l["value"]["what"] for l in store["keys"]["depenses"]["items"]] == ["Essence", "Péage"]
        # the owner's app sees the same
        seen = store_get(s["bridge"], s["path"]).json()
        assert seen["rev"] == store["rev"] and seen["keys"]["total"]["value"] == 6000
    finally:
        s["guichet"].stop()


def test_a_looping_routine_writing_a_page_is_held(bridge):
    """The page's budget for its agent: a routine writing in a loop is told « too fast », and the page's people still
    write."""
    give_token(bridge)
    path = media_of(call(bridge, "create_page", {"html": DASHBOARD, "title": "Le jardin en boucle"}))
    page = path.rsplit("/", 1)[1][:-5]
    told = None
    for n in range(80):
        result = call(bridge, "page_data", {"page": page, "ops": [{"op": "set", "key": "temp", "value": n}]})
        if result["isError"]:
            told = text_of(result)
            break
    assert told is not None, "never held"
    # held by the page's budget for its agent (« too fast »), or before it by the brake's writes a run (« Not done »)
    assert any(w in told for w in ("too fast", "Not done", "Refused")), told
    typed = bridge.post("/v1/page-store", headers=AUTH, json={"path": path, "op": "set", "key": "note", "value": "ok",
                                                              "a": f"p{int(time.time())}"})
    assert typed.status_code == 200, typed.text
