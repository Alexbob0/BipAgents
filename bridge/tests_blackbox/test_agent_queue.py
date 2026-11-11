"""An agent's waiting messages (bridge-rs/src/msg_queue.rs, 2026-11-02), through the one sending route
(`POST /v1/threads/{id}/messages`, threads/send.rs): words steered into the turn going on (Hermes'
`POST /v1/runs/{id}/steer`), « Après » kept by the bridge per agent and started as the next turns one after another,
reordered, cancelled, sent now, paused and resumed; the same list for every device (the timeline's `pending.queue`, the run
stream's `bridge.queue` events), kept across a restart; « reply ready » pushed once, at the end of the queue."""
import json
import threading
import time
import uuid

from bbkit.common import AUTH, app_turn, sse, in_session, words_of
from bbkit.servers import wait_until

SESSION = "api_q"
TOKEN = "f" * 64


def start(bridge, fakes, text="Compare trois écoles", session=SESSION):
    """A turn the app starts (the hub follows it)."""
    return app_turn(bridge, fakes, text, session)


def queue(bridge):
    answer = bridge.get("/v1/agents/wellness/queue", headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()


def thread_of(bridge, session=SESSION):
    answer = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "wellness", "session_id": session})
    assert answer.status_code == 200, answer.text
    return answer.json()["thread"]["id"]


def send(bridge, text, after=False, session=SESSION, **extra):
    """The app's message (its own id), « Après » with `after`."""
    body = {"agent": "wellness", "client_id": str(uuid.uuid4()), "text": text, "after": after, **extra}
    return bridge.post(f"/v1/threads/{thread_of(bridge, session)}/messages", headers=AUTH, json=body)


def add(bridge, text, run_id=None, session=SESSION):
    """« Après »: waits for the turn going on (the bridge knows which), the queue as every device reads it."""
    answer = send(bridge, text, after=True, session=session)
    assert answer.status_code == 200, answer.text
    assert answer.json()["state"] == "queued"
    return queue(bridge)


def texts(snapshot):
    return [item["text"] for item in snapshot["items"]]


def started_inputs(fakes):
    return [words_of(run["input"]) for run in fakes.hermes.started.values()]


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
    run_id = start(bridge, fakes)
    assert run_id == "r1"
    snapshot = add(bridge, "  Ensuite range le Carnet  ", run_id="r1")
    assert texts(snapshot) == ["Ensuite range le Carnet"] and snapshot["paused"] is False
    item = snapshot["items"][0]
    assert item["session_id"] == SESSION and item["late"] is False and item["id"].startswith("q")
    time.sleep(1.5)
    assert started_inputs(fakes) == ["Compare trois écoles"], "nothing starts while the turn goes on"
    # every device reads the same list with the conversation's timeline
    thread = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "wellness", "session_id": SESSION}).json()
    page = bridge.get(f"/v1/threads/{thread['thread']['id']}/timeline", headers=AUTH, params={"agent": "wellness"}).json()
    assert texts(page["pending"]["queue"]) == ["Ensuite range le Carnet"]
    assert page["pending"]["queue"]["version"] == snapshot["version"]
    gate.set()
    assert wait_until(lambda: len(fakes.hermes.started) == 2, timeout=15)
    second = fakes.hermes.started["r2"]
    assert words_of(second["input"]) == "Ensuite range le Carnet" and second["session_id"] == SESSION
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
    start(bridge, fakes)
    assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH,
                                          json={"agent": "wellness", "run_id": "r1"}).status_code == 200)
    answer = send(bridge, "Seulement les publiques")
    assert answer.status_code == 200, answer.text
    assert answer.json()["state"] == "steered" and answer.json()["run_id"] == "r1"
    assert fakes.hermes.steers == [{"agent": "", "run_id": "r1", "body": {"input": "Seulement les publiques"}}]
    assert queue(bridge)["items"] == [], "into the turn, not the queue"
    # the turn no longer takes words (it is ending): first in line instead, and it goes right after
    fakes.hermes.steer_status = 409
    answer = send(bridge, "Et près de chez nous").json()
    assert answer["state"] == "queued"
    assert texts(queue(bridge)) == ["Et près de chez nous"]
    fakes.hermes.runs["r2"] = turn("r2", output="Deux.")
    gate.set()
    assert wait_until(lambda: started_inputs(fakes)[1:] == ["Et près de chez nous"], timeout=15)
    # bad requests
    assert send(bridge, "x", agent="nope").status_code == 404
    assert send(bridge, "x", client_id="../x").status_code == 422
    assert send(bridge, "   ").status_code == 422


def test_words_the_turn_did_not_take_come_back_first(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    # Hermes hands back what came after the agent's last tool calls (`pending_steer` on the run's end)
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Fini.", pending_steer="Et en bio")
    fakes.hermes.runs["r2"] = turn("r2", output="Bio.")
    start(bridge, fakes)
    assert send(bridge, "Et en bio").json()["state"] == "steered"
    gate.set()
    assert wait_until(lambda: started_inputs(fakes)[1:] == ["Et en bio"], timeout=15)
    assert fakes.hermes.started["r2"]["session_id"] == SESSION


def test_reorder_cancel_pause_send_now_and_resume(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Un.")
    start(bridge, fakes)
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
    start(bridge, fakes)
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
    start(bridge, fakes)
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
    """An app following the turn hears each change of the queue in the run's stream (`bridge.queue`, live, not
    replayed)."""
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Un.")
    start(bridge, fakes)
    events = []

    def listen():
        with bridge.stream("GET", "/v1/runs/r1/events?agent=wellness", headers=AUTH) as resp:
            name = None
            for line in resp.iter_lines():
                if line.startswith("event: "):
                    name = line[len("event: "):]
                elif line.startswith("data: "):
                    events.append((name, json.loads(line[len("data: "):])))
                    if len([n for n, _ in events if n == "bridge.queue"]) >= 2:
                        return

    listener = threading.Thread(target=listen, daemon=True)
    listener.start()
    assert wait_until(lambda: any(name == "run.started" for name, _ in events), timeout=10)
    add(bridge, "Un", run_id="r1")
    add(bridge, "Deux", run_id="r1")
    listener.join(15)
    told = [data for name, data in events if name == "bridge.queue"]
    assert len(told) == 2 and all(data["kind"] == "queue" for data in told)
    assert texts(told[1]) == ["Un", "Deux"] and told[1]["version"] > told[0]["version"]
    gate.set()


def test_reply_ready_is_pushed_once_at_the_end_of_the_queue(bridges, fakes):
    bridge = bridges.new().start()
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "sandbox",
                                                         "agent_ids": ["wellness"]}).status_code == 200
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Voici les écoles.")
    fakes.hermes.runs["r2"] = turn("r2", output="Carnet rangé.")
    start(bridge, fakes)
    # the app followed its turn, then left (screen locked): its end would be pushed
    assert bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1",
                                                        "session_id": SESSION}).status_code == 200
    add(bridge, "Range le Carnet", run_id="r1")
    gate.set()
    assert wait_until(lambda: len(fakes.apns.pushes()) >= 1, timeout=15)
    time.sleep(1.0)
    payloads = fakes.apns.payloads()
    assert len(payloads) == 1, payloads
    assert payloads[0]["r"] == "r2" and payloads[0]["t"] == "reply" and in_session(bridge, payloads[0], SESSION)


def test_the_queue_answers_for_known_agents_only_and_keeps_its_limits(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate)
    start(bridge, fakes)
    assert bridge.get("/v1/agents/nope/queue", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents/wellness/queue").status_code == 401
    assert send(bridge, "x" * 20_001, after=True).status_code == 422
    for i in range(30):
        add(bridge, f"m{i}", run_id="r1")
    assert send(bridge, "one more", after=True).status_code == 409
    assert bridge.post("/v1/agents/wellness/queue/zzz/now", headers=AUTH).status_code == 404
    gate.set()


PHOTO = {"kind": "image", "name": "veste.jpg", "mime": "image/jpeg",
         "data": "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAP" + "A" * 40 + "/9k="}


def photo_parts(run_input):
    """The image parts of a turn's input (Hermes gets the photo itself, not words about it)."""
    if not isinstance(run_input, list):
        return []
    return [p for m in run_input for p in (m.get("content") if isinstance(m.get("content"), list) else [])
            if p.get("type") == "image_url"]


def test_a_photo_pushed_into_a_running_turn_is_told_to_it_and_goes_right_after(bridges, fakes):
    """10/11 (Alex): a photo sent while the agent worked on his voice note waited « En attente »; « Pousser » did
    nothing he could see (Hermes' steer takes words only: the photo only went first in line, without a word), and the
    turn went on without it. Now the turn is told a photo waits, then, pushed, to wrap up; the photo starts the moment
    the turn ends, even stopped, and its row says it was pushed."""
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, end="run.cancelled")
    fakes.hermes.runs["r2"] = turn("r2", output="Sur ta photo : la verte.")
    start(bridge, fakes, text="(note vocale) Laquelle je prends ?")
    assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH,
                                          json={"agent": "wellness", "run_id": "r1"}).status_code == 200)
    sent = send(bridge, "", attachments=[PHOTO], kinds=["image"])
    assert sent.status_code == 200, sent.text
    assert sent.json()["state"] == "queued"
    # the turn knows a photo waits for it (hidden from the apps: the bridge's mark), and no words were steered
    assert wait_until(lambda: len(fakes.hermes.steers) == 1)
    told = fakes.hermes.steers[0]
    assert told["run_id"] == "r1" and told["body"]["input"].startswith("[bipagents:files_waiting] Your person has just sent you a photo")
    [item] = queue(bridge)["items"]
    assert item["attachments"][0]["kind"] == "image" and "pushed" not in item
    # « Pousser »: the turn is told to wrap up, the photo is first in line and marked pushed
    now = bridge.post(f"/v1/agents/wellness/queue/{item['id']}/now", headers=AUTH)
    assert now.status_code == 200, now.text
    answer = now.json()
    assert answer["steered"] is False and answer["pushed"] is True and answer["run_id"] == "r1"
    assert answer["queue"]["items"][0]["id"] == item["id"] and answer["queue"]["items"][0]["pushed"] is True
    assert fakes.hermes.steers[-1]["body"]["input"].startswith("[bipagents:files_next] Your person has just sent you a photo")
    assert "Wrap up now" in fakes.hermes.steers[-1]["body"]["input"]
    assert len(fakes.hermes.started) == 1, "the turn going on is not doubled"
    # the turn ends stopped: no pause behind it for a pushed photo, which starts with the photo itself
    gate.set()
    assert wait_until(lambda: "r2" in fakes.hermes.started, timeout=15)
    run = fakes.hermes.started["r2"]
    assert run["session_id"] == SESSION and len(photo_parts(run["input"])) == 1
    assert wait_until(lambda: queue(bridge)["items"] == [] and queue(bridge)["paused"] is False, timeout=10)


def test_a_turn_that_does_not_wrap_up_for_a_pushed_photo_is_stopped(bridges, fakes):
    """A turn told to wrap up that still goes on (a tool running for minutes: the steer waits for it) is stopped a
    while later (`BIPAGENTS_PUSH_STOP_SECONDS` here), so the photo the person pushed goes."""
    bridge = bridges.new()
    bridge.env_extra.update({"BIPAGENTS_PUSH_STOP_SECONDS": "1"})
    bridge.start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate)
    start(bridge, fakes)
    assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH,
                                          json={"agent": "wellness", "run_id": "r1"}).status_code == 200)
    assert send(bridge, "Et celle-ci ?", attachments=[PHOTO], kinds=["image"]).json()["state"] == "queued"
    [item] = queue(bridge)["items"]
    assert bridge.post(f"/v1/agents/wellness/queue/{item['id']}/now", headers=AUTH).json()["pushed"] is True
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/stop", "POST"), timeout=10)
    gate.set()


def test_a_photo_pushed_when_the_turn_takes_no_more_words_still_goes_next(bridges, fakes):
    """The turn refuses the steer (it is ending): the photo is still first in line, pushed, and goes at its end."""
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, output="Fini.")
    fakes.hermes.runs["r2"] = turn("r2", output="Vu.")
    start(bridge, fakes)
    assert wait_until(lambda: bridge.post("/v1/watch", headers=AUTH,
                                          json={"agent": "wellness", "run_id": "r1"}).status_code == 200)
    add(bridge, "Avant", run_id="r1")
    fakes.hermes.steer_status = 409
    assert send(bridge, "", attachments=[PHOTO], kinds=["image"]).json()["state"] == "queued"
    photo = [i for i in queue(bridge)["items"] if i.get("attachments")][0]
    answer = bridge.post(f"/v1/agents/wellness/queue/{photo['id']}/now", headers=AUTH).json()
    assert answer["pushed"] is True and [i["id"] for i in answer["queue"]["items"]][0] == photo["id"]
    gate.set()
    assert wait_until(lambda: "r2" in fakes.hermes.started, timeout=15)
    assert len(photo_parts(fakes.hermes.started["r2"]["input"])) == 1
