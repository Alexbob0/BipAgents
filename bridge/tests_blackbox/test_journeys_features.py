"""The release journeys of this release's new features (docs/release-journeys.md, J39–J46, column « Pont »), as the
apps do them over HTTP, each page and line checked as the person SEES it (bbkit/displayable_people.py,
bbkit/displayable.py):

- « Seulement le critique »: a choice that is not critical is made for the person (« Validé pour toi » in the Boîte,
  « Annuler »), a critical one is still asked;
- a routine the agent makes is on at once, then one question: « Veux-tu être au courant de ce que cette routine
  trouve ? »;
- « Étapes »: built from the agent's actions while it works, then its own list wins;
- the hop cap: agents writing to each other stop at the 5th message, one « Continuer l'échange ? » card, « Oui » starts
  the count over;
- Quick Talk follows a message to its reply (`GET /v1/threads/{t}/sends/{client_id}`), bridge only;
- the Carnet under an answer (J47): « A consulté 1 page · Noté dans Léa », the question of the day answered in
  one tap (no agent run), « Annuler », then « Oublie ça » from the app (preview, confirmation) — the conversation kept.

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


def test_j46_a_running_task_forgotten_leaves_and_stays_gone(journey):
    """« Oublier cette tâche » (the Boîte's « En cours » › its page): the task the agent started, still running, is
    gone from `/v1/jobs` and « Ce que tes agents font tout seuls » at once, its « Validé pour toi » line « Annulé », and stays gone (its watch never saves it
    back once its file is removed)."""
    level(journey.bridge, "critical")
    journey.model.calls["task"] = lambda run: mcp_as(journey.bridge, journey.token, "/v1/agent-tools/mcp", "propose_task", {
        "title": "Dossier location", "prompt": "Prépare le dossier de location #lent", "budget": "small"})
    assert journey.send(cid(46), "Prépare mon dossier de location #tache").status_code == 200
    reply_done(journey, "La tâche est lancée")

    def jobs():
        return journey.bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
    [task] = [j for j in jobs() if j["title"].endswith("Dossier location")]
    assert task["state"] == "running", task
    started = time.time()
    gone = journey.bridge.delete(f"/v1/jobs/{task['id']}", headers=AUTH)
    assert gone.status_code == 200 and gone.json() == {"ok": True}, gone.text
    assert time.time() - started < 5, "« Oublier » waited on the run"
    # its « Validé pour toi » line: « Annulé », no « Annuler » left on a task that is gone (9/11)
    [line] = [x for x in auto_lines(journey.bridge) if x["kind"] == "task_start"]
    assert line["undone"] is True and line["undo"] is False, line
    assert not shown.auto_line_problems(line), shown.auto_line_problems(line)
    for _ in range(8):   # past a round of its watch (bbkit: a fast poll)
        assert task["id"] not in [j["id"] for j in jobs()], jobs()
        items = journey.bridge.get("/v1/autonomous", headers=AUTH).json()["items"]
        assert f"task:{task['id']}" not in [i["id"] for i in items], items
        time.sleep(0.5)


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
    """The agent makes a routine: listed, off until the person answers (Alex 11/11, PI-6), and its card in the
    conversation asks one thing: « Veux-tu être au courant de ce que cette routine trouve ? »; « Oui, préviens-moi »
    starts it, told."""
    journey.model.calls["routine"] = lambda run: mcp_as(journey.bridge, journey.token, "/v1/agent-tools/mcp",
                                                        "routine_schedule", {
        "action": "create", "name": "Le point du soir", "schedule": "every day at 19:00",
        "prompt": "Fais le point de la journée.", "silent": True})
    assert journey.send(cid(41), "Fais-moi le point chaque soir #routine").status_code == 200
    page = reply_done(journey, "la routine « Le point du soir » est créée")
    routines = {j["name"]: j for j in journey.bridge.get(f"/v1/agents/{AGENT}/routines", headers=AUTH).json()["jobs"]}
    assert routines["Le point du soir"]["state"] == "paused", routines   # off until the answer (Alex 11/11, PI-6)
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


# -- the Carnet (Carnet v2) --------------------------------------------------------------------------------------------

def test_j47_the_carnet_under_the_answer_a_question_in_one_tap_then_forgotten(journey):
    """Vie reads Léa's page, files a fact and asks one question: under its answer the person sees « A consulté 1 page ·
    Noté dans Léa » and the question card; a tap writes the answer in the person's name (no agent run),
    « Annuler » takes it back; « Oublie ça » from the app (preview, then confirm) erases the fact everywhere but the
    conversation."""
    bridge = journey.bridge
    assert bridge.patch("/v1/notebook/settings", headers=AUTH,
                        json={"quiet": {"from": "03:00", "to": "03:01"}}).status_code == 200   # the test may run at night
    made = mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "notebook_write",
                  {"page": "Léa Martin", "kind": "person", "aliases": ["Léa"], "facts": ["Sœur, vit à Lyon"]})
    assert made.startswith("Saved: Léa Martin"), made
    def as_vie(tool, arguments):
        return tool, arguments, lambda: mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", tool, arguments)
    journey.model.calls["carnet"] = lambda run: [
        as_vie("notebook_read", {"page": "Léa"}),
        as_vie("notebook_write", {"page": "Léa", "section": "who", "facts": ["Prof de musique"]}),
        as_vie("notebook_write", {"page": "Léa", "section": "tastes",
                                  "ask": {"question": "Léa chausse du combien ?", "chips": ["35", "36", "37"]}}),
    ]
    assert journey.send(cid(47), "Je cherche un cadeau pour Léa #carnet").status_code == 200
    page = reply_done(journey, "C’est noté dans ton Carnet")
    shown.check_page(bridge, page, AGENT)
    items = page["items"]
    lines = [i for i in items if i["kind"] == "carnet_line"]
    assert len(lines) == 1 and lines[0]["text"] == "A consulté 1 page · Noté dans Léa", lines
    assert not [i for i in items if i["kind"] == "steps"], [i["kind"] for i in items]   # backstage
    cards = [i for i in items if i["kind"] == "carnet_card" and i.get("card") == "ask"]
    assert len(cards) == 1 and cards[0]["question"] == "Léa chausse du combien ?", [i["kind"] for i in items]
    card = cards[0]
    # the tap: the fact in the person's name, no agent run
    runs = len(journey.fakes.hermes.started)
    tapped = bridge.post(f"/v1/notebook/ask/{card['id']}", params={"lang": "fr"}, json={"choice": "36"}, headers=AUTH)
    assert tapped.status_code == 200 and tapped.json()["text"] == "Noté : 36", tapped.text
    lea = bridge.state_dir / "notebook" / "carnet" / "Personnes" / "Léa Martin.md"
    assert "Pointure : 36" in lea.read_text() and "Prof de musique" in lea.read_text()
    time.sleep(0.5)
    assert len(journey.fakes.hermes.started) == runs
    # « Annuler »: only the answer goes, Vie's fact stays
    undone = bridge.post(f"/v1/notebook/ask/{card['id']}", params={"lang": "fr"}, json={"undo": True}, headers=AUTH)
    assert undone.status_code == 200 and undone.json()["state"] == "open", undone.text
    assert "Pointure" not in lea.read_text() and "Prof de musique" in lea.read_text()
    # « Oublie ça » from the app: preview, then confirm; the conversation keeps what was said
    preview = bridge.post("/v1/notebook/forget/preview", params={"lang": "fr"}, headers=AUTH,
                          json={"texts": ["Prof de musique"], "page": "Léa Martin"})
    assert preview.status_code == 200 and preview.json()["token"], preview.text
    confirmed = bridge.post("/v1/notebook/forget/confirm", headers=AUTH, json={"token": preview.json()["token"]})
    assert confirmed.status_code in (200, 204), confirmed.text
    assert "musique" not in lea.read_text()
    assert "Je cherche un cadeau pour Léa" in str(journey.page()["items"])


def test_j52_a_point_moved_from_famille_and_vie_files_the_next_one_there(journey):
    """« Famille » holds a work point of Vie's: the page shows it in one line (label, answer, `fresh`, `noted`); moved
    to « BipAgents » (proposed first) it leaves Famille with its line kept, both histories say so, « Annuler » puts it
    back; moved again, Vie files a like point on Famille: it lands on BipAgents and Vie is told."""
    bridge = journey.bridge
    def write(arguments):
        return mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "notebook_write", arguments)
    assert write({"page": "BipAgents", "kind": "subject", "section": "status", "facts": ["Bêta publique le 19 novembre"]}).startswith("Saved")
    assert write({"page": "Famille", "kind": "area", "section": "matters",
                  "facts": ["Judo de Zoé : mercredi 14 h", "BipAgents : panorama concurrentiel livré le 25/10"]}).startswith("Saved")
    famille = "Domaines/Famille.md"
    page = bridge.get("/v1/notebook/page", params={"path": famille, "lang": "fr"}, headers=AUTH).json()
    facts = [f for s in page["sections"] for f in s["facts"]]
    point = next(f for f in facts if f["label"] == "BipAgents")
    assert point["answer"] == "panorama concurrentiel livré le 25/10" and point["fresh"] is True, point
    assert point["noted"].startswith("Noté par Vie le "), point
    targets = bridge.get("/v1/notebook/targets", headers=AUTH,
                         params={"path": famille, "text": point["text"], "lang": "fr"}).json()
    assert targets["suggested"][0]["title"] == "BipAgents", targets["suggested"]
    to = targets["suggested"][0]
    def move():
        moved = bridge.post("/v1/notebook/move", params={"lang": "fr"}, headers=AUTH,
                            json={"path": famille, "texts": [point["text"]],
                                  "to": {"path": to["path"]}, "to_section": "status"})
        assert moved.status_code == 200, moved.text
        return moved.json()
    moved = move()
    assert moved["toast"] == "Déplacé vers « BipAgents »." and moved["learnt"] == "Vie range ça là désormais.", moved
    assert "panorama" not in str(moved["page"]["sections"])
    root = bridge.state_dir / "notebook" / "carnet"
    assert "- panorama concurrentiel livré le 25/10 — *Vie, " in (root / to["path"]).read_text()
    history = bridge.get("/v1/notebook/history", params={"path": famille}, headers=AUTH).json()["changes"]
    assert history[0]["action"] == "move" and "Déplacé vers « BipAgents »" in history[0]["summary"], history[0]
    # « Annuler »
    undone = bridge.post("/v1/notebook/move/undo", params={"lang": "fr"}, headers=AUTH, json={"move": moved["move"]})
    assert undone.status_code == 200 and "panorama" in str(undone.json()["page"]["sections"]), undone.text
    again = bridge.post("/v1/notebook/move/undo", params={"lang": "fr"}, headers=AUTH, json={"move": moved["move"]})
    assert again.status_code == 410 and again.json()["detail"]["code"] == "move_gone", again.text
    # moved again: Vie's next like point goes to BipAgents, and Vie is told
    move()
    told = write({"page": "Famille", "kind": "area", "section": "matters",
                  "facts": ["BipAgents : panorama concurrentiel mis à jour le 9/11"]})
    assert "Filed on « BipAgents » instead" in told, told
    assert "mis à jour le 9/11" in (root / to["path"]).read_text()
    assert "mis à jour" not in (root / famille).read_text()
    # a refused move says why
    gone = bridge.post("/v1/notebook/move", params={"lang": "fr"}, headers=AUTH,
                       json={"path": famille, "texts": ["Rien de tel"], "to": {"path": to["path"]}})
    assert gone.status_code == 404 and gone.json()["detail"]["code"] == "nothing_to_move", gone.text


def test_j52_a_work_agent_stays_off_famille_and_the_card_proposes_the_old_points(journey):
    """A work agent (group « Pro ») cannot write on « Famille »; what it wrote there before is proposed in the home's
    card « N points semblent mal rangés », moved only on « Tout déplacer »."""
    bridge = journey.bridge
    assert mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "notebook_write",
                  {"page": "Famille", "kind": "area", "section": "matters",
                   "facts": ["Promotion : Buffer, plan gratuit"]}).startswith("Saved")
    assert bridge.put(f"/v1/agents/{AGENT}/group", headers=AUTH, json={"group": "Pro"}).status_code == 200
    with pytest.raises(RuntimeError, match="^Not filed: work facts never go on the family's pages"):
        mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "notebook_write",
               {"page": "Famille", "kind": "area", "facts": ["Concurrents : aucun ne cumule tout"]})
    assert bridge.post("/v1/notebook/tidy", headers=AUTH).status_code in (200, 202)
    def card():
        return bridge.get("/v1/notebook", params={"lang": "fr"}, headers=AUTH).json().get("misplaced")
    assert wait_until(card, timeout=30), "no misplaced card"
    shown_card = card()
    assert shown_card["title"] == "Un point semble mal rangé", shown_card
    assert shown_card["groups"][0]["from"] == "Famille" and shown_card["groups"][0]["to"] == "Travail", shown_card
    answered = bridge.post("/v1/notebook/misplaced", params={"lang": "fr"}, headers=AUTH, json={"action": "move"})
    assert answered.status_code == 200 and answered.json() == {"moved": 1, "toast": "Déplacé.", "card": None}, answered.text
    root = bridge.state_dir / "notebook" / "carnet"
    assert "Promotion" in (root / "Domaines" / "Travail.md").read_text()
    assert "Promotion" not in (root / "Domaines" / "Famille.md").read_text()
