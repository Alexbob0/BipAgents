"""Alex's answers of 2026-11-03 (carnet review): light person pages folded at night into their group's page (their
names kept, a fact never lost), and a trip whose page says it was called off archived at night."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, notebook, tidy


def note(bridge, page, facts, section="who"):
    done = call(bridge, "notebook_write", {"page": page, "kind": "person", "section": section, "facts": facts})
    assert done["isError"] is False, text_of(done)


def test_light_pages_fold_into_their_group_and_keep_their_names(bridge):
    give_token(bridge)
    in_french(bridge)
    note(bridge, "Paul Lambert", ["Parrain d'Emma"])
    note(bridge, "Emma", ["Cousine, sœur de Julien", "Infirmière à Lille"])
    note(bridge, "Julien", ["Cousin", "Aime le vélo", "Cherche un appartement à Bordeaux"])
    note(bridge, "Françoise", ["Ma mère"])
    run = tidy(bridge)
    assert run, run
    people = {p["title"] for p in bridge.get("/v1/notebook/pages", params={"kind": "person"}, headers=AUTH).json()["pages"]}
    assert {"Julien", "Françoise"} <= people and not {"Paul Lambert", "Emma"} & people, people
    family = (notebook(bridge) / "carnet" / "Domaines" / "Famille élargie.md").read_text()
    assert "- Emma : Infirmière à Lille — *" in family and "  - Emma" in family, family
    others = (notebook(bridge) / "carnet" / "Domaines" / "Connaissances.md").read_text()
    assert "Paul Lambert : Parrain d'Emma" in others
    # the old name still opens something, and an agent's new fact about her lands on the group
    found = text_of(call(bridge, "notebook_read", {"page": "Emma"}))
    assert "Famille élargie" in found
    note(bridge, "Emma", ["De garde ce week-end"], section="")
    assert "Emma : De garde ce week-end" in (notebook(bridge) / "carnet" / "Domaines" / "Famille élargie.md").read_text()


def test_a_trip_called_off_is_archived_at_night(bridge):
    give_token(bridge)
    in_french(bridge)
    for page, facts in [("Lyon, nov. 2030", ["Week-end chez mes parents du 27 au 29 novembre 2030"]),
                        ("Rome, mai 2030", ["Voyage du 3 au 8 mai 2030", "Vol AZ333 annulé, remplacé par AZ335"])]:
        done = call(bridge, "notebook_write", {"page": page, "kind": "trip", "section": "dates", "facts": facts,
                                              "start": facts[0][-4:] + ("-11-27" if "Lyon" in page else "-05-03"),
                                              "end": facts[0][-4:] + ("-11-29" if "Lyon" in page else "-05-08")})
        assert done["isError"] is False, text_of(done)
    edit = call(bridge, "notebook_write", {"page": "Lyon, nov. 2030", "kind": "trip",
                                          "facts": ["Week-end annulé : mes parents ne seront pas là"]})
    assert edit["isError"] is False, text_of(edit)
    tidy(bridge)
    trips = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    by = {p["title"]: p for p in trips}
    assert by["Lyon, nov. 2030"]["path"].startswith("Archives/"), by["Lyon, nov. 2030"]
    assert by["Rome, mai 2030"]["path"].startswith("Voyages/"), "a cancelled flight is not a cancelled trip"
    found = text_of(call(bridge, "notebook_search", {"words": "Lyon"}))
    assert "Lyon, nov. 2030" in found


# -- Carnet v2, B3 (contract.md §4): a follow-up only for what matters, at the head of the agent's next answer ---------

def test_an_important_task_is_followed_up_once_and_runs_nothing(bridges, fakes):
    import json
    import threading
    import time
    from datetime import date, timedelta

    from bbkit.common import sse
    from test_notebook_ask import no_quiet_hours, pushes
    from test_threads_send import page, send, setup, stored, user, agent

    bridge, thread = setup(bridges, fakes)
    give_token(bridge)
    no_quiet_hours(bridge)
    due = (date.today() + timedelta(days=3)).isoformat()
    # not important, an agent's idea: never; important and the person's own: once
    call(bridge, "notebook_task", {"action": "add", "title": "Acheter du pain", "due": due, "origin": "asked"})
    added = call(bridge, "notebook_task", {"action": "add", "title": "Rappeler le plombier", "due": due, "origin": "own",
                                           "important": True})
    assert added["isError"] is False, text_of(added)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="D'accord.")]
    t0 = time.time() - 1
    assert send(bridge, thread, "C5D00000-0000-4000-8000-0000000000B1", "Salut").json()["state"] == "started"
    gate.set()
    stored(fakes, dict(user("Salut"), timestamp=t0 + 2), dict(agent("D'accord."), timestamp=time.time() + 1))
    items = page(bridge, thread)["items"]
    cards = [i for i in items if i.get("card") == "follow"]
    assert len(cards) == 1, [i["kind"] for i in items]
    card = cards[0]
    assert card["text"] == "Tu voulais rappeler le plombier cette semaine."
    assert [c["text"] for c in card["chips"]] == ["C'est fait", "Demain", "Laisse tomber"]
    assert card["menu"] == [{"id": "never", "text": "Ne plus me relancer"}]
    # at the head of the answer: right after the person's message
    assert items[items.index(card) - 1].get("text") == "Salut"
    runs, pushed = len(fakes.hermes.started), pushes(fakes)
    dropped = bridge.post(f"/v1/notebook/follow/{card['id']}", params={"lang": "fr"}, json={"answer": "drop"}, headers=AUTH)
    assert dropped.status_code == 200, dropped.text
    assert (dropped.json()["state"], dropped.json()["text"]) == ("dropped", "Laissé tombé")
    time.sleep(0.5)
    assert len(fakes.hermes.started) == runs and pushes(fakes) == pushed
    tasks = bridge.get("/v1/notebook/tasks", headers=AUTH).json()
    assert "Rappeler le plombier" not in json.dumps(tasks.get("open", tasks))
    state = json.loads((bridge.state_dir / "notebook" / "state.json").read_text())
    assert state["followups"]["last_at"] and "plombier" not in json.dumps(state["cards"])
    assert bridge.post(f"/v1/notebook/follow/{card['id']}", json={"answer": "done"}, headers=AUTH).status_code == 410
    assert bridge.post(f"/v1/notebook/follow/{card['id']}", json={"answer": "maybe"}, headers=AUTH).status_code == 422
