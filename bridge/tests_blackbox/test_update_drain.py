"""An update gentle with the turns going on (bridge-rs/src/update_drain.rs, 2026-11-06): the updater asks the bridge
to wait (`POST /v1/update/drain`) before it restarts it or the agents' Hermes; the bridge answers once no turn goes on
(or at the seconds given), then answers the agents' tool calls « try again in a minute », a plain result Hermes'
breaker doesn't count, until it restarts (or `done`)."""
import threading
import time

from bbkit.common import AUTH, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until

UPDATE_TEXTS = ("Mise à jour en cours : réessaie dans une minute.", "Update in progress: try again in a minute.")


def follow_gated(bridge, fakes, run_id="run_long"):
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [gate, sse("run.completed", output="Fini.")]

    def follow():
        with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness", headers=AUTH, timeout=60) as resp:
            for _ in resp.iter_raw():
                pass

    follower = threading.Thread(target=follow, daemon=True)
    follower.start()
    assert wait_until(lambda: fakes.hermes.event_requests, timeout=10)
    return gate, follower


def drain_in_background(bridge, **body):
    answer = {}

    def ask():
        answer["resp"] = bridge.post("/v1/update/drain", json=body, headers=AUTH, timeout=60)

    asking = threading.Thread(target=ask, daemon=True)
    asking.start()
    return asking, answer


def test_the_drain_waits_for_the_turn_then_tool_calls_are_told_to_try_again(bridge, fakes):
    give_token(bridge)
    assert bridge.post("/v1/update/drain", json={}).status_code == 401   # the bridge's key
    assert bridge.get("/v1/update/drain", headers=AUTH).json() == {"state": "idle", "since": None, "until": None,
                                                                    "turns": 0}
    gate, follower = follow_gated(bridge, fakes)
    asking, answer = drain_in_background(bridge, seconds=30)
    assert wait_until(lambda: bridge.get("/v1/update/drain", headers=AUTH).json()["state"] == "draining", timeout=5)
    time.sleep(1.5)
    assert "resp" not in answer   # the turn goes on: the update waits
    status = bridge.get("/v1/update/drain", headers=AUTH).json()
    assert status["state"] == "draining" and status["turns"] == 1, status
    # the turn's tool calls are served as usual meanwhile
    served = call(bridge, "knowledge_read", {})
    assert not served["isError"] and text_of(served) == "No knowledge page yet."
    gate.set()
    follower.join(timeout=10)
    asking.join(timeout=15)
    said = answer["resp"].json()
    assert answer["resp"].status_code == 200 and said["drained"] is True and said["turns"] == 0, said
    assert 1.0 <= said["waited"] < 20, said
    assert "update waits for the turns going on" in bridge.log_tail(200)
    # closing: a tool call now (the bridge restarts in a moment) is told to try again, not failed
    told = call(bridge, "knowledge_read", {})
    assert told["isError"] is False and text_of(told).startswith(UPDATE_TEXTS), told
    assert bridge.get("/v1/update/drain", headers=AUTH).json()["state"] == "closing"
    # the update restarted something else (the gateways): back to normal
    assert bridge.post("/v1/update/drain", json={"done": True}, headers=AUTH).json() == {"state": "idle"}
    assert text_of(call(bridge, "knowledge_read", {})) == "No knowledge page yet."


def test_nothing_going_on_answers_at_once_and_a_long_turn_only_until_the_ceiling(bridge, fakes):
    give_token(bridge)
    started = time.time()
    quiet = bridge.post("/v1/update/drain", json={"seconds": 30}, headers=AUTH).json()
    assert quiet["drained"] is True and quiet["turns"] == 0 and time.time() - started < 3, quiet
    bridge.post("/v1/update/drain", json={"done": True}, headers=AUTH)
    gate, follower = follow_gated(bridge, fakes)
    started = time.time()
    capped = bridge.post("/v1/update/drain", json={"seconds": 2}, headers=AUTH).json()
    assert capped["drained"] is False and capped["turns"] == 1 and 1.5 <= time.time() - started < 8, capped
    assert "turns still going on at the drain's ceiling" in bridge.log_tail(200)
    told = call(bridge, "knowledge_read", {})
    assert told["isError"] is False and text_of(told).startswith(UPDATE_TEXTS)
    gate.set()
    follower.join(timeout=10)


def test_before_the_gateways_restart_their_own_count_is_waited_for(bridge, fakes):
    # a gateway's restart ends every run: its Hermes' own count too (a turn no bridge follows, a background task)
    fakes.hermes.busy[""] = 1
    asking, answer = drain_in_background(bridge, seconds=30, what="gateways")
    time.sleep(2.5)
    assert "resp" not in answer
    fakes.hermes.busy[""] = 0
    asking.join(timeout=15)
    said = answer["resp"].json()
    assert said["drained"] is True and said["waited"] >= 2, said
    bridge.post("/v1/update/drain", json={"done": True}, headers=AUTH)


def test_a_turn_waiting_for_its_persons_answer_does_not_hold_the_bridges_restart(bridge, fakes):
    # Hermes keeps it, the hub resumes it after the restart: nothing lost (the gateways' restart would end it: waited)
    gate = threading.Event()
    fakes.hermes.runs["run_card"] = [
        sse("approval.request", run_id="run_card", request_id="req_c", command="rm -rf build/", choices=["once", "deny"]),
        gate, sse("run.completed", run_id="run_card")]
    assert bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_card"}).status_code == 200
    assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"], timeout=10)
    started = time.time()
    said = bridge.post("/v1/update/drain", json={"seconds": 20}, headers=AUTH).json()
    assert said["drained"] is True and time.time() - started < 3, said
    bridge.post("/v1/update/drain", json={"done": True}, headers=AUTH)
    capped = bridge.post("/v1/update/drain", json={"seconds": 1.5, "what": "gateways"}, headers=AUTH).json()
    assert capped["drained"] is False and capped["turns"] == 1, capped
    bridge.post("/v1/update/drain", json={"done": True}, headers=AUTH)
    gate.set()
