"""The release journeys of this release's new features (docs/release-journeys.md, J39–J44, column « Pont »), as the
apps do them over HTTP, each page and line checked as the person SEES it (bbkit/displayable_people.py,
bbkit/displayable.py):

- « Seulement le critique »: a choice that is not critical is made for the person (« Validé pour toi » in the Boîte,
  « Annuler »), a critical one is still asked;
- a routine the agent makes is on at once, then one question: « Veux-tu être au courant de ce que cette routine
  trouve ? »;
- « Étapes »: built from the agent's actions while it works, then its own list wins;
- the hop cap: agents writing to each other stop at the 5th message, one « Continuer l'échange ? » card, « Oui » starts
  the count over;
- Quick Talk follows a message to its reply (`GET /v1/threads/{t}/sends/{client_id}`), bridge only.

Each test is one journey `Jnn`."""
import time

import pytest

from bbkit import displayable_people as shown
from bbkit.common import AUTH
from bbkit.journeys import AGENT, journey_bridge
from bbkit.model import PeopleModel, ScriptedModel
from bbkit.people import mcp_as, reply_in, send, thread_page
from bbkit.servers import wait_until


@pytest.fixture
def journey(bridges, fakes):
    made = journey_bridge(bridges, fakes)
    made.model = PeopleModel.upgrade(made.model)
    # the agent speaks French (its steps, its cards' words: handback::language_of)
    assert made.bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200
    return made


def cid(n: int) -> str:
    return f"J0URNEY0-0000-4000-8000-{n:012d}"


def level(bridge, value):
    put = bridge.put("/v1/approval-level", headers=AUTH, json={"level": value})
    assert put.status_code == 200, put.text
    return put.json()


def auto_lines(bridge):
    return bridge.get("/v1/approvals/auto", headers=AUTH, params={"agent": AGENT}).json()["items"]


def reply_done(journey, words, timeout=60):
    return reply_in(journey.bridge, journey.thread, AGENT, words, timeout=timeout)


def waiting_card(journey, prefix, timeout=30):
    def found():
        return [c for c in journey.page()["pending"].get("approvals") or [] if c.get("run_id", "").startswith(prefix)]
    assert wait_until(found, timeout=timeout), journey.page()["pending"]
    return found()


# -- the approval level ----------------------------------------------------------------------------------------------

def test_j39_only_the_critical_a_small_task_is_validated_for_the_person_with_annuler(journey):
    """« Seulement le critique »: Vie starts a small background task without asking; no card, no notification, a
    « Validé pour toi » line in the Boîte saying what was started, and « Annuler » pauses it (then « Annulé »)."""
    assert level(journey.bridge, "critical")["level"] == "critical"
    journey.model.calls["task"] = lambda run: mcp_as(journey.bridge, journey.token, "/v1/agent-tools/mcp", "propose_task", {
        "title": "Dossier location", "prompt": "Prépare le dossier de location #lent", "budget": "small"})
    run = journey.send(cid(39), "Prépare mon dossier de location #tache")
    assert run.status_code == 200, run.text
    page = reply_done(journey, "La tâche est lancée")
    shown.check_page(journey.bridge, page, AGENT)
    assert not page["pending"].get("approvals"), page["pending"]   # nothing asked
    lines = [x for x in auto_lines(journey.bridge) if x["kind"] == "task_start"]
    assert len(lines) == 1, auto_lines(journey.bridge)
    line = lines[0]
    assert not shown.auto_line_problems(line), shown.auto_line_problems(line)
    assert "Dossier location" in line["summary"] and line["undo"] is True and line["undone"] is False, line
    rows = journey.bridge.get("/v1/notifications", headers=AUTH, params={"limit": 50}).json()["notifications"]
    assert not [r for r in rows if "Dossier location" in str(r)], rows   # no notification for it
    def task():
        return next(j for j in journey.bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
                    if j["title"].endswith("Dossier location"))
    assert task()["state"] == "running", task()
    undone = journey.bridge.post(f"/v1/approvals/auto/{line['id']}/undo", headers=AUTH)
    assert undone.status_code == 200 and undone.json()["item"]["undone"] is True, undone.text   # « Annulé »
    assert not shown.auto_line_problems(undone.json()["item"]), undone.json()
    assert wait_until(lambda: task()["state"] != "running", timeout=15), task()   # the task stopped


def test_j40_only_the_critical_a_critical_card_is_still_asked(journey):
    """« Seulement le critique »: a command that deletes is still asked (its card says what), nothing is validated for
    the person, « Oui, vas-y » and the agent goes on."""
    level(journey.bridge, "critical")
    run = journey.send(cid(40), "Fais le ménage #approbation").json()["run_id"]
    [card] = waiting_card(journey, "")
    assert "brouillons" in (card.get("command") or "") + (card.get("description") or ""), card
    shown.check_page(journey.bridge, journey.page(), AGENT)
    assert auto_lines(journey.bridge) == [], "a critical choice was validated for the person"
    answered = journey.bridge.post("/v1/approve", headers=AUTH, json={
        "agent": AGENT, "run_id": run, "request_id": card["request_id"], "choice": "once"})
    assert answered.status_code == 200, answered.text
    page = reply_done(journey, "le dossier est supprimé")
    assert not page["pending"].get("approvals")
    shown.check_page(journey.bridge, page, AGENT)


# -- a routine, then one question ------------------------------------------------------------------------------------

def test_j41_a_routine_is_made_first_then_one_question(journey):
    """The agent makes a routine: it is on at once (listed, scheduled), and its card in the conversation asks one
    thing: « Veux-tu être au courant de ce que cette routine trouve ? »; « Oui, préviens-moi » keeps it on, told."""
    journey.model.calls["routine"] = lambda run: mcp_as(journey.bridge, journey.token, "/v1/agent-tools/mcp",
                                                        "routine_schedule", {
        "action": "create", "name": "Le point du soir", "schedule": "every day at 19:00",
        "prompt": "Fais le point de la journée.", "silent": True})
    assert journey.send(cid(41), "Fais-moi le point chaque soir #routine").status_code == 200
    page = reply_done(journey, "la routine « Le point du soir » est créée")
    routines = {j["name"]: j for j in journey.bridge.get(f"/v1/agents/{AGENT}/routines", headers=AUTH).json()["jobs"]}
    assert routines["Le point du soir"]["state"] == "scheduled", routines   # on before any answer
    cards = [c for c in page["pending"].get("approvals") or [] if c["run_id"].startswith("bip-rout-")]
    assert len(cards) == 1, page["pending"]   # one question, in the conversation it was made in
    card = cards[0]
    assert not shown.routine_card_problems(card), shown.routine_card_problems(card)
    assert card["bip_routine"]["name"] == "Le point du soir"
    assert "au courant" in (card["bip_routine"].get("question") or card.get("bip_summary") or ""), card
    shown.check_page(journey.bridge, page, AGENT)
    answered = journey.bridge.post("/v1/approve", headers=AUTH, json={"agent": AGENT, "run_id": card["run_id"],
                                                                      "choice": "notify"})
    assert answered.status_code == 200 and answered.json()["muted"] is False, answered.text
    assert not [c for c in journey.page()["pending"].get("approvals") or [] if c["run_id"].startswith("bip-rout-")]
    routines = {j["name"]: j for j in journey.bridge.get(f"/v1/agents/{AGENT}/routines", headers=AUTH).json()["jobs"]}
    assert (routines["Le point du soir"]["state"], routines["Le point du soir"]["muted"]) == ("scheduled", False)
    assert journey.bridge.post("/v1/approve", headers=AUTH, json={
        "agent": AGENT, "run_id": card["run_id"], "choice": "notify"}).status_code == 404   # asked once


# -- « Étapes » -------------------------------------------------------------------------------------------------------

def test_j42_the_steps_card_follows_the_agents_actions_then_its_own_list(journey):
    """While Vie works, « Étapes » comes from what it does, from its 3rd useful step (a calculation never counts); once
    it writes its own list, that list is the card, live and once the turn is stored: one card for the turn."""
    run = journey.send(cid(42), "Organise le pique-nique de samedi #etapes").json()["run_id"]
    events = journey.run_events(run)
    updates = [d.get("steps") for name, d in events if name == "steps.updated"]
    assert updates, [name for name, _ in events]
    first = updates[0]
    assert first.get("auto") is True and len(first["items"]) == 3, first   # from its actions, the useful ones only
    assert not [s for s in first["items"] if "calcul" in s["text"].lower()], first
    final = updates[-1]
    assert not final.get("auto"), final   # its own list wins
    assert [s["text"] for s in final["items"]] == ["Regarder la météo", "Choisir le parc", "Préparer le pique-nique"]
    page = reply_done(journey, "pique-nique au parc")
    shown.check_page(journey.bridge, page, AGENT)
    turn = page["items"][[i.get("client_id") for i in page["items"]].index(cid(42)):]
    steps = [i for i in turn if i["kind"] == "steps"]
    assert len(steps) == 1, [i["kind"] for i in turn]
    kept = steps[0]["steps"]
    assert [(s["text"], s["status"]) for s in kept["items"]] == [
        ("Regarder la météo", "done"), ("Choisir le parc", "done"), ("Préparer le pique-nique", "in_progress")], kept
    assert not kept.get("auto") and (kept.get("done"), kept.get("total")) == (2, 3), kept
    assert not [i for i in turn if i["kind"] == "tool" and (i.get("tool") or {}).get("name") == "todo_list"]


# -- the hop cap -----------------------------------------------------------------------------------------------------

def test_j43_agents_writing_to_each_other_stop_at_the_cap_and_oui_goes_on(bridges, fakes, tmp_path):
    """Quotidien writes to Coach again and again in one turn: the 5th message is refused, and the person gets ONE card
    in Quotidien's conversation, « Quotidien et Coach ont échangé 4 messages entre eux sans toi. Continuer
    l'échange ? »; « Oui, vas-y »: the card goes, the agent goes on."""
    from test_agent_messages import TOKENS
    from test_agent_pairs import linked

    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    linked(bridge, fakes, [{"from": "quotidien", "to": "coach"}, {"from": "coach", "to": "quotidien"}])
    assert bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200
    model = PeopleModel.upgrade(ScriptedModel(fakes.hermes, tmp_path / "fichiers", word_gap=0.03))
    model.calls["relay"] = lambda run: mcp_as(bridge, TOKENS["quotidien"], "/v1/agent-tools/mcp", "message_agent", {
        "target": "bip/coach", "message": "On cale une séance samedi ?"}, timeout=60)
    thread = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "quotidien"}).json()["id"]
    assert send(bridge, thread, "quotidien", "Vois avec Coach pour samedi #relais").status_code == 200
    page = reply_in(bridge, thread, "quotidien", "J’ai échangé avec Coach", timeout=120)
    relayed = [r for r in model.step_results if "message_agent" in r["tool"]]
    assert len(relayed) == 5 and all("answered" in (r.get("result") or "") for r in relayed[:4]), relayed
    assert "Limite d'échanges" in (relayed[4].get("result") or relayed[4].get("error") or ""), relayed[4]
    cards = [c for c in page["pending"].get("approvals") or [] if c["run_id"].startswith("bip-hops-")]
    assert len(cards) == 1, page["pending"]
    card = cards[0]
    assert not shown.hops_card_problems(card), shown.hops_card_problems(card)
    assert "Continuer l'échange" in card["bip_summary"] or "Continuer l’échange" in card["bip_summary"], card["bip_summary"]
    shown.check_page(bridge, {**page, "pending": {**page["pending"], "approvals": [
        c for c in page["pending"]["approvals"] if not c["run_id"].startswith("bip-hops-")]}}, "quotidien")
    answered = bridge.post("/v1/approve", headers=AUTH, json={"agent": "quotidien", "run_id": card["run_id"],
                                                             "choice": "once"})
    assert answered.status_code == 200, answered.text
    assert not [c for c in thread_page(bridge, thread, "quotidien")["pending"].get("approvals") or []
                if c["run_id"].startswith("bip-hops-")]
    # the agent goes on (a turn told the person said yes), and may write to Coach again
    assert wait_until(lambda: len([r for r in model.inputs if r["agent"] == "quotidien"]) >= 2, timeout=30), model.inputs
    again = mcp_as(bridge, TOKENS["quotidien"], "/v1/agent-tools/mcp", "message_agent",
                   {"target": "bip/coach", "message": "Et dimanche ?"}, timeout=60)
    assert "answered" in again, again


# -- Quick Talk ------------------------------------------------------------------------------------------------------

def follow(journey, client_id, until, timeout=60):
    """Quick Talk's loop: the send's state asked again after `next` ms (else every 300 ms), every state checked as the
    apps decode it. Returns every state seen, the last one meeting `until`."""
    seen = []
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        resp = journey.bridge.get(f"/v1/threads/main/sends/{client_id}", headers=AUTH, params={"agent": AGENT})
        assert resp.status_code == 200, resp.text
        state = resp.json()
        assert not shown.sent_state_problems(state), shown.sent_state_problems(state)
        seen.append(state)
        if until(state):
            return seen
        time.sleep(min((state.get("next") or 300) / 1000, 0.3))
    raise AssertionError(f"Quick Talk never got there: {[s['state'] for s in seen]}")


def test_j44_quick_talk_follows_a_message_to_its_reply(journey):
    """Quick Talk sends words and asks the bridge where they are until the reply: unknown before, then sent/running
    (with the answer's words so far), then the reply's words to say aloud; a turn that asks a card is handed over."""
    unknown = journey.bridge.get(f"/v1/threads/main/sends/{cid(44)}", headers=AUTH, params={"agent": AGENT})
    assert unknown.status_code == 200 and unknown.json()["state"] == "unknown", unknown.text
    assert journey.bridge.get("/v1/threads/main/sends/x!", headers=AUTH, params={"agent": AGENT}).status_code == 422
    assert journey.send(cid(44), "Les horaires de la piscine ? #outil").status_code == 200
    seen = follow(journey, cid(44), lambda s: s["state"] == "reply")
    states = [s["state"] for s in seen]
    assert set(states[:-1]) <= {"queued", "sending", "running"} and states[:-1], states
    last = seen[-1]
    assert last["reply"]["text"].startswith("Bien reçu") or "Bien reçu" in last["reply"]["text"], last
    assert last["next"] is None and last["message"].startswith("m_") and last["reply"]["id"].startswith("m_"), last
    stored = [i for i in journey.page()["items"] if i.get("kind") == "message" and i.get("role") == "agent"]
    assert stored and stored[-1]["text"].endswith(last["reply"]["text"].strip()[-40:]), (stored[-1]["text"], last)

    # a turn that asks: handed over to the screen (« needs »), then followed to its reply once answered
    run = journey.send(cid(4400), "Fais le ménage #approbation").json()["run_id"]
    asked = follow(journey, cid(4400), lambda s: s["state"] == "needs")[-1]
    assert asked["needs"] == "approval" and asked["next"] is None, asked
    [card] = waiting_card(journey, "")
    assert journey.bridge.post("/v1/approve", headers=AUTH, json={"agent": AGENT, "run_id": run,
                                                                  "request_id": card["request_id"],
                                                                  "choice": "once"}).status_code == 200
    done = follow(journey, cid(4400), lambda s: s["state"] == "reply")[-1]
    assert "le dossier est supprimé" in done["reply"]["text"], done

