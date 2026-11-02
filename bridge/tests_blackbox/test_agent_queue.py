"""An agent's waiting messages (bridge-rs/src/msg_queue.rs, 2026-11-02): words steered into the turn going on (Hermes'
`POST /v1/runs/{id}/steer`), « Après » kept by the bridge per agent and started as the next turns one after another,
reordered, cancelled, sent now, paused and resumed; the same list for every device (`session_state`, the queue's own
event stream), kept across a restart; « reply ready » pushed once, at the end of the queue."""
import json
import threading
import time

from bbkit.common import AUTH, HERMES_KEY, read_sse, sse
from bbkit.servers import wait_until

HERMES_AUTH = {"Authorization": f"Bearer {HERMES_KEY}"}
SESSION = "api_q"
TOKEN = "f" * 64


def start(bridge, text="Compare trois écoles", session=SESSION):
    """A turn the app starts, through the door's Hermes relay (the hub follows it)."""
    posted = bridge.post("/hermes/wellness/v1/runs", json={"input": text, "session_id": session}, headers=HERMES_AUTH)
    assert posted.status_code == 202, posted.text
    return posted.json()["run_id"]


def queue(bridge):
    answer = bridge.get("/v1/agents/wellness/queue", headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()


def add(bridge, text, run_id=None, session=SESSION, **extra):
    body = {"session_id": session, "text": text, **extra}
    if run_id:
        body["run_id"] = run_id
    answer = bridge.post("/v1/agents/wellness/queue", headers=AUTH, json=body)
    assert answer.status_code == 200, answer.text
    return answer.json()


def texts(snapshot):
    return [item["text"] for item in snapshot["items"]]


def started_inputs(fakes):
    return [run["input"] for run in fakes.hermes.started.values()]


def turn(run_id, gate=None, end="run.completed", **end_data):
    script = [sse("run.started", run_id=run_id)]
    if gate is not None:
        script.append(gate)
    return script + [sse(end, run_id=run_id, **end_data)]


def test_a_message_for_after_waits_for_the_turn_then_starts(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    fakes.hermes.add_session(SESSION, [])
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Voici les trois écoles.")
    fakes.hermes.runs["r2"] = turn("r2", output="Rangé.")
    run_id = start(bridge)
    assert run_id == "r1"
    snapshot = add(bridge, "  Ensuite range le Carnet  ", run_id="r1")
    assert texts(snapshot) == ["Ensuite range le Carnet"] and snapshot["paused"] is False
    item = snapshot["items"][0]
    assert item["session_id"] == SESSION and item["late"] is False and item["id"].startswith("q")
    time.sleep(1.5)
    assert started_inputs(fakes) == ["Compare trois écoles"], "nothing starts while the turn goes on"
    # every device reads the same list with the conversation's state
    state = bridge.get(f"/v1/sessions/{SESSION}/state?agent=wellness", headers=AUTH).json()
    assert texts(state["queue"]) == ["Ensuite range le Carnet"] and state["queue"]["version"] == snapshot["version"]
    gate.set()
    assert wait_until(lambda: len(fakes.hermes.started) == 2, timeout=15)
    second = fakes.hermes.started["r2"]
    assert second["input"] == "Ensuite range le Carnet" and second["session_id"] == SESSION
    assert wait_until(lambda: queue(bridge)["items"] == [] and queue(bridge)["running"] is None, timeout=15)
    assert bridge.logged("queue: next message started")
    # the turn the bridge started is followed like the app's (one Hermes subscription)
    assert len([s for s in fakes.hermes.event_requests if s.path == "/v1/runs/r2/events"]) == 1


def test_a_message_added_while_nothing_runs_goes_at_once(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    fakes.hermes.runs["r1"] = turn("r1", output="Bonjour.")
    add(bridge, "Bonjour")
    assert wait_until(lambda: started_inputs(fakes) == ["Bonjour"], timeout=10)
    assert wait_until(lambda: queue(bridge)["items"] == [], timeout=10)


def test_words_steered_reach_the_running_turn(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Fini.")
    start(bridge)
    assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH,
                                          json={"agent": "wellness", "run_id": "r1"}).status_code == 200)
    answer = bridge.post("/v1/agents/wellness/steer", headers=AUTH,
                         json={"session_id": SESSION, "text": "Seulement les publiques"})
    assert answer.status_code == 200, answer.text
    assert answer.json() == {"steered": True, "run_id": "r1"}
    assert fakes.hermes.steers == [{"agent": "", "run_id": "r1", "body": {"input": "Seulement les publiques"}}]
    assert queue(bridge)["items"] == [], "into the turn, not the queue"
    # the turn no longer takes words (it is ending): first in line instead, and it goes right after
    fakes.hermes.steer_status = 409
    answer = bridge.post("/v1/agents/wellness/steer", headers=AUTH,
                         json={"session_id": SESSION, "text": "Et près de chez nous", "run_id": "r1"}).json()
    assert answer["steered"] is False and answer["queued"] is True
    assert texts(answer["queue"]) == ["Et près de chez nous"]
    fakes.hermes.runs["r2"] = turn("r2", output="Deux.")
    gate.set()
    assert wait_until(lambda: started_inputs(fakes)[1:] == ["Et près de chez nous"], timeout=15)
    # bad requests
    assert bridge.post("/v1/agents/nope/steer", headers=AUTH, json={"session_id": SESSION, "text": "x"}).status_code == 404
    assert bridge.post("/v1/agents/wellness/steer", headers=AUTH,
                       json={"session_id": "../x", "text": "x"}).status_code == 400
    assert bridge.post("/v1/agents/wellness/steer", headers=AUTH,
                       json={"session_id": SESSION, "text": "   "}).status_code == 422


def test_words_the_turn_did_not_take_come_back_first(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    # Hermes hands back what came after the agent's last tool calls (`pending_steer` on the run's end)
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Fini.", pending_steer="Et en bio")
    fakes.hermes.runs["r2"] = turn("r2", output="Bio.")
    start(bridge)
    assert bridge.post("/v1/agents/wellness/steer", headers=AUTH,
                       json={"session_id": SESSION, "text": "Et en bio"}).json()["steered"] is True
    gate.set()
    assert wait_until(lambda: started_inputs(fakes)[1:] == ["Et en bio"], timeout=15)
    assert fakes.hermes.started["r2"]["session_id"] == SESSION


def test_words_steered_by_an_older_app_are_left_to_it(bridges, fakes):
    """An app that steers Hermes itself (through the relay) sends what the turn didn't take on its own."""
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    fakes.hermes.runs["r1"] = turn("r1", output="Fini.", pending_steer="Et en bio")
    start(bridge)
    with bridge.stream("GET", "/hermes/wellness/v1/runs/r1/events", headers=HERMES_AUTH) as resp:
        assert read_sse(resp)[-1][0] == "run.completed"
    time.sleep(1.0)
    assert len(fakes.hermes.started) == 1 and queue(bridge)["items"] == []


def test_reorder_cancel_pause_send_now_and_resume(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Un.")
    start(bridge)
    for text in ("A", "B", "C"):
        add(bridge, text, run_id="r1")
    ids = {item["text"]: item["id"] for item in queue(bridge)["items"]}
    reordered = bridge.post("/v1/agents/wellness/queue/order", headers=AUTH, json={"ids": [ids["C"], "nope", ids["A"]]})
    assert texts(reordered.json()) == ["C", "A", "B"]
    cancelled = bridge.delete(f"/v1/agents/wellness/queue/{ids['A']}", headers=AUTH)
    assert texts(cancelled.json()) == ["C", "B"]
    assert bridge.delete(f"/v1/agents/wellness/queue/{ids['A']}", headers=AUTH).status_code == 404
    paused = bridge.post("/v1/agents/wellness/queue/pause", headers=AUTH).json()
    assert paused["paused"] is True
    gate.set()
    time.sleep(2.0)
    assert len(fakes.hermes.started) == 1, "paused: the turn ended, nothing new starts"
    assert texts(queue(bridge)) == ["C", "B"]
    # « Envoyer maintenant » while nothing runs: started at once, even paused
    gate2 = threading.Event()
    fakes.hermes.runs["r2"] = turn("r2", gate2, output="Bé.")
    now = bridge.post(f"/v1/agents/wellness/queue/{ids['B']}/now", headers=AUTH).json()
    assert now["steered"] is False and now["run_id"] == "r2" and texts(now["queue"]) == ["C"]
    assert now["queue"]["running"]["run_id"] == "r2" and now["queue"]["running"]["text"] == "B"
    # « Envoyer maintenant » while a turn goes on: into that turn
    add(bridge, "D", run_id="r2")
    d = [i for i in queue(bridge)["items"] if i["text"] == "D"][0]
    now = bridge.post(f"/v1/agents/wellness/queue/{d['id']}/now", headers=AUTH).json()
    assert now["steered"] is True and now["run_id"] == "r2" and texts(now["queue"]) == ["C"]
    assert fakes.hermes.steers[-1]["body"] == {"input": "D"} and fakes.hermes.steers[-1]["run_id"] == "r2"
    # resumed: C goes once r2 ends
    fakes.hermes.runs["r3"] = turn("r3", output="Cé.")
    assert bridge.post("/v1/agents/wellness/queue/resume", headers=AUTH).json()["paused"] is False
    time.sleep(1.0)
    assert len(fakes.hermes.started) == 2
    gate2.set()
    assert wait_until(lambda: started_inputs(fakes)[2:] == ["C"], timeout=15)
    assert wait_until(lambda: queue(bridge)["items"] == [] and queue(bridge)["paused"] is False, timeout=10)


def test_a_turn_the_person_stopped_pauses_what_waits(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, end="run.cancelled")
    start(bridge)
    add(bridge, "Puis l'autre", run_id="r1")
    gate.set()
    assert wait_until(lambda: queue(bridge)["paused"] is True, timeout=10)
    time.sleep(1.0)
    assert len(fakes.hermes.started) == 1 and texts(queue(bridge)) == ["Puis l'autre"]


def test_the_queue_survives_a_bridge_restart(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Un.")
    fakes.hermes.runs["r2"] = turn("r2", output="Deux.")
    start(bridge)
    add(bridge, "Après le redémarrage", run_id="r1")
    add(bridge, "Et encore", run_id="r1")
    before = queue(bridge)
    path = bridge.root / "data" / "queue" / "wellness.json"
    assert path.exists() and oct(path.stat().st_mode & 0o777) == "0o600"
    bridge.restart()
    after = queue(bridge)
    assert texts(after) == ["Après le redémarrage", "Et encore"] and after["version"] == before["version"]
    time.sleep(1.5)
    assert len(fakes.hermes.started) == 1, "the turn the queue waits for still goes on (Hermes asked)"
    assert fakes.hermes.seen("/v1/runs/r1", "GET")
    gate.set()
    assert wait_until(lambda: started_inputs(fakes)[1:2] == ["Après le redémarrage"], timeout=30)


def test_every_device_follows_the_queue_live(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Un.")
    start(bridge)
    events = []

    def listen():
        with bridge.stream("GET", "/v1/agents/wellness/queue/events", headers=AUTH) as resp:
            name = None
            for line in resp.iter_lines():
                if line.startswith("event: "):
                    name = line[len("event: "):]
                elif line.startswith("data: "):
                    events.append((name, json.loads(line[len("data: "):])))
                    if len(events) >= 3:
                        return

    listener = threading.Thread(target=listen, daemon=True)
    listener.start()
    assert wait_until(lambda: len(events) >= 1, timeout=10)
    assert events[0][0] == "queue" and events[0][1]["items"] == []
    add(bridge, "Un", run_id="r1")
    add(bridge, "Deux", run_id="r1")
    listener.join(15)
    assert [name for name, _ in events] == ["queue", "queue", "queue"]
    assert texts(events[2][1]) == ["Un", "Deux"] and events[2][1]["version"] > events[1][1]["version"]
    # an app following the turn hears it in the run's stream too (live, not replayed)
    gate.set()


def test_reply_ready_is_pushed_once_at_the_end_of_the_queue(bridges, fakes):
    bridge = bridges.new().start()
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "sandbox",
                                                         "agent_ids": ["wellness"]}).status_code == 200
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Voici les écoles.")
    fakes.hermes.runs["r2"] = turn("r2", output="Carnet rangé.")
    start(bridge)
    # the app followed its turn, then left (screen locked): its end would be pushed
    assert bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1",
                                                        "session_id": SESSION}).status_code == 200
    add(bridge, "Range le Carnet", run_id="r1")
    gate.set()
    assert wait_until(lambda: len(fakes.apns.pushes()) >= 1, timeout=15)
    time.sleep(1.0)
    payloads = fakes.apns.payloads()
    assert len(payloads) == 1, payloads
    assert payloads[0]["run_id"] == "r2" and payloads[0]["kind"] == "reply" and payloads[0]["session_id"] == SESSION


def test_the_queue_answers_for_known_agents_only_and_keeps_its_limits(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate)
    start(bridge)
    assert bridge.get("/v1/agents/nope/queue", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents/wellness/queue").status_code == 401
    assert bridge.post("/v1/agents/wellness/queue", headers=AUTH,
                       json={"session_id": SESSION, "text": "x" * 20_001}).status_code == 422
    assert bridge.post("/v1/agents/wellness/queue", headers=AUTH,
                       json={"session_id": SESSION, "text": "x", "run_id": "bad id"}).status_code == 400
    for i in range(30):
        add(bridge, f"m{i}", run_id="r1")
    assert bridge.post("/v1/agents/wellness/queue", headers=AUTH,
                       json={"session_id": SESSION, "text": "one more"}).status_code == 409
    assert bridge.post("/v1/agents/wellness/queue/zzz/now", headers=AUTH).status_code == 404
    gate.set()
