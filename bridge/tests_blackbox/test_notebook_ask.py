"""Carnet v2, B3 (internal/notes/carnet-v2/contract.md §3.3, §4): the question of the day. An agent asks it with
`notebook_write {ask}` in a turn that read the page; the bridge puts a Carnet card under its answer; a tap writes the
fact in the person's name — no agent run, no notification, nothing in the Boîte — and « Annuler » takes it back.
One a day. Scripted, no model."""
import json
import threading
import time

from bbkit.common import AUTH, sse
from bbkit.mcp import call as mcp_call, give_token, text_of

from test_threads_send import page, send, setup, stored, user, agent

LEA = "Personnes/Léa Martin.md"


def no_quiet_hours(bridge):
    """The test may run at night: quiet hours of one minute, at 03:00."""
    done = bridge.patch("/v1/notebook/settings", json={"language": "fr", "quiet": {"from": "03:00", "to": "03:01"}},
                        headers=AUTH)
    assert done.status_code == 200, done.text
    assert done.json()["quiet"] == {"from": "03:00", "to": "03:01"} and done.json()["questions"] is True


def pushes(fakes):
    return len(fakes.apns.pushes()) + len(fakes.fcm.requests) + len(fakes.push_service.requests)


def test_a_tap_writes_the_fact_and_runs_nothing(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    give_token(bridge)
    no_quiet_hours(bridge)
    made = mcp_call(bridge, "notebook_write", {"page": "Léa Martin", "kind": "person", "aliases": ["Léa"],
                                               "facts": ["Sœur, vit à Lyon"]})
    assert made["isError"] is False, text_of(made)
    # outside a turn: no question
    asked = {"page": "Léa", "section": "tastes", "ask": {"question": "Léa chausse du combien ?", "chips": ["35", "36", "37"]}}
    assert text_of(mcp_call(bridge, "notebook_write", asked)) == "No question now."
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Bonne idée.")]
    t0 = time.time() - 1
    assert send(bridge, thread, "C5D00000-0000-4000-8000-0000000000A1", "Je cherche des chaussures pour Léa").json()["state"] == "started"
    assert text_of(mcp_call(bridge, "notebook_read", {"page": "Léa"})).startswith("# Léa Martin")
    assert text_of(mcp_call(bridge, "notebook_write", asked)) == "Question shown to the person under your answer."
    # the day's card is taken: a second question is refused
    again = {"page": "Léa", "section": "gifts", "ask": {"question": "Léa aime quelle couleur ?", "chips": ["Bleu", "Vert"]}}
    assert text_of(mcp_call(bridge, "notebook_write", again)) == "No question now."
    gate.set()
    stored(fakes, dict(user("Je cherche des chaussures pour Léa"), timestamp=t0 + 0.5),
           dict(agent("Bonne idée."), timestamp=time.time() + 1))
    items = page(bridge, thread)["items"]
    cards = [i for i in items if i["kind"] == "carnet_card" and i.get("card") == "ask"]
    assert len(cards) == 1, [i["kind"] for i in items]
    card = cards[0]
    assert (card["state"], card["top"], card["question"]) == ("open", "Pour ton Carnet", "Léa chausse du combien ?")
    assert [c["text"] for c in card["chips"]] == ["35", "36", "37", "Passer"]
    assert card["menu"] == [{"id": "never", "text": "Ne plus me demander ça"}]
    # the tap: the fact written by the person, nothing run, nothing pushed, nothing waiting in the Boîte
    runs, pushed = len(fakes.hermes.started), pushes(fakes)
    tapped = bridge.post(f"/v1/notebook/ask/{card['id']}", params={"lang": "fr"}, json={"choice": "36"}, headers=AUTH)
    assert tapped.status_code == 200, tapped.text
    assert tapped.json() == {"kind": "carnet_card", "card": "ask", "id": card["id"], "state": "done",
                             "text": "Noté : 36", "undo": "Annuler"}
    lea = bridge.state_dir / "notebook" / "carnet" / LEA
    assert "- Pointure : 36 — *toi," in lea.read_text()
    time.sleep(0.5)
    assert len(fakes.hermes.started) == runs and pushes(fakes) == pushed
    waiting = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "wellness"}).json()["items"]
    assert not [w for w in waiting if "bip-ask" in json.dumps(w)]
    # the timeline shows it folded; a second tap gets the card as it is (410)
    folded = next(i for i in page(bridge, thread)["items"] if i.get("card") == "ask")
    assert (folded["state"], folded["text"]) == ("done", "Noté : 36")
    twice = bridge.post(f"/v1/notebook/ask/{card['id']}", json={"skip": True}, headers=AUTH)
    assert twice.status_code == 410 and twice.json()["detail"]["code"] == "card_gone"
    # « Annuler »: the page as it was
    undone = bridge.post(f"/v1/notebook/ask/{card['id']}", params={"lang": "fr"}, json={"undo": True}, headers=AUTH)
    assert undone.status_code == 200 and undone.json()["state"] == "open", undone.text
    assert "Pointure" not in lea.read_text()
    # « Ne plus me demander ça »: the card goes
    never = bridge.post(f"/v1/notebook/ask/{card['id']}", json={"never": True}, headers=AUTH)
    assert never.json()["state"] == "never"
    assert not [i for i in page(bridge, thread)["items"] if i.get("card") == "ask"]
    state = json.loads((bridge.state_dir / "notebook" / "state.json").read_text())
    assert state["ask"]["declined"] == ["person:tastes"]
    assert bridge.post("/v1/notebook/ask/bip-ask-nope", json={"skip": True}, headers=AUTH).status_code == 404


def test_the_setting_and_the_quiet_hours(bridges, fakes):
    bridge = bridges.new().start()
    assert bridge.get("/v1/notebook/settings", headers=AUTH).json()["quiet"] == {"from": "22:00", "to": "08:00"}
    off = bridge.patch("/v1/notebook/settings", json={"questions": False}, headers=AUTH)
    assert off.status_code == 200 and off.json()["questions"] is False
    bad = bridge.patch("/v1/notebook/settings", json={"quiet": {"from": "25:00", "to": "08:00"}}, headers=AUTH)
    assert bad.status_code == 422
