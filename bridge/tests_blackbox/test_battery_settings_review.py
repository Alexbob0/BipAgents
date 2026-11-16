"""Review of battery-settings (16/11): what the six battery settings must not break, as failing tests first
(BipAgents-reprise/passation-pro/briefs/review-battery-settings.md)."""
import json
import threading
import time
import uuid

import httpx

from bbkit.common import AUTH, app_turn, sse
from bbkit.servers import wait_until

SESSION = "api_q"


def turn(run_id, gate=None, end="run.completed", **end_data):
    script = [sse("run.started", run_id=run_id)]
    if gate is not None:
        script.append(gate)
    return script + [sse(end, run_id=run_id, **end_data)]


def queue(bridge):
    answer = bridge.get("/v1/agents/wellness/queue", headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()


def thread_of(bridge):
    answer = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "wellness", "session_id": SESSION})
    assert answer.status_code == 200, answer.text
    return answer.json()["thread"]["id"]


def test_a_message_the_queue_starts_is_told_to_its_open_conversation(bridges, fakes):
    """R4 (D1). An open conversation no longer asks its agent's queue every 5 s while its event stream is open: what
    changes must come through that stream. A message the bridge starts from the queue (« Reprendre » after a Stop,
    its 10 s tick, the end of a turn this screen did not follow) said nothing there until the turn it started ended:
    the iPhone showed it « en attente », no turn going on, for the whole turn, then the answer at once. Before 16/11
    the 5 s poll saw `running` and followed the turn. The open conversation must be told when the turn starts."""
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = turn("r1", gate, end="run.cancelled")   # the person stops it: what waits is paused
    app_turn(bridge, fakes, "Compare trois écoles", SESSION)
    thread = thread_of(bridge)
    after = {"agent": "wellness", "client_id": str(uuid.uuid4()), "text": "Puis range le Carnet", "after": True}
    answer = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH, json=after)
    assert answer.status_code == 200 and answer.json()["state"] == "queued", answer.text
    gate.set()
    assert wait_until(lambda: queue(bridge)["paused"] is True, timeout=10)
    hints, stop = [], threading.Event()

    def watch():   # the conversation on the iPhone's screen: its thread's event stream
        with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=60) as client:
            with client.stream("GET", "/v1/threads/events", params={"agent": "wellness", "thread": thread}) as resp:
                name = None
                for line in resp.iter_lines():
                    if line.startswith("event: "):
                        name = line[len("event: "):]
                    elif line.startswith("data: ") and name == "thread":
                        hints.append(json.loads(line[len("data: "):]))
                    if stop.is_set():
                        return

    threading.Thread(target=watch, daemon=True).start()
    time.sleep(2.0)   # what the stopped turn's end said is behind
    told = len(hints)
    gate2 = threading.Event()
    fakes.hermes.runs["r2"] = turn("r2", gate2, output="Rangé.")   # the turn the queue starts goes on a while
    try:
        assert bridge.post("/v1/agents/wellness/queue/resume", headers=AUTH).json()["paused"] is False   # « Reprendre »
        assert wait_until(lambda: "r2" in fakes.hermes.started, timeout=10)
        assert wait_until(lambda: (queue(bridge)["running"] or {}).get("run_id") == "r2", timeout=5)
        assert wait_until(lambda: any(h.get("thread") == thread for h in hints[told:]), timeout=5), \
            f"the queue started « Puis range le Carnet » (r2): its open conversation heard nothing ({hints[told:]})"
    finally:
        gate2.set()
        stop.set()
