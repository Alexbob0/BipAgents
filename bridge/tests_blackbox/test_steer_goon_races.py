"""Races between go_on (a turn that ends on a promise gets one continuation: runs.rs `go_on`, go_on.rs) and steer-interrupt
(new words restart a turn that did nothing yet: runs.rs `restart`, turn_restart.rs), found by the review of the merge
(int-release-next 4282c3283): each failed before its fix (fix-steer-goon-races)."""
import threading
import time

from bbkit.common import AUTH, read_sse, sse
from bbkit.servers import wait_until
from test_promise_stop import FOUND, PROMISE, thread_of, turn, window

W1 = "Non laisse tomber, j'irai en magasin"
W2 = "Et regarde aussi chez Action"


def say(bridge, thread, cid, text):
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": cid, "text": text})
    assert sent.status_code == 200, sent.text
    return sent.json()


def with_words(fakes, words):
    return [r for r, s in fakes.hermes.started.items() if words in str(s["input"])]


# F1: words sent while restart(B) (the one after go_on) starts the turn again: answered `restarted`, never started
def test_second_words_while_a_restart_starts_are_not_lost(bridges, fakes):
    bridge = bridges.new().start()
    thread = window(bridge, fakes)                   # go_on's first start refused, the app follows run_vie
    first = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000H1", W1)
    assert first["state"] == "steered" and first.get("restarted"), first
    fakes.hermes.start_failures = 3                  # Hermes still closing: the restart's own starts refused a while
    assert wait_until(lambda: bridge.logged("restart: new turn not started yet"), timeout=10)
    second = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000H2", W2)
    assert wait_until(lambda: with_words(fakes, W1), timeout=15), fakes.hermes.started
    started = wait_until(lambda: with_words(fakes, W2), timeout=10)
    queue = bridge.get("/v1/agents/wellness/queue", headers=AUTH).json()
    waiting = [i.get("text") for i in queue.get("items", [])]
    assert started or any(W2 in (t or "") for t in waiting), (
        f"second words answered {second}, then neither started nor waiting: "
        f"started={ {r: str(s['input'])[-60:] for r, s in fakes.hermes.started.items()} }, queue={waiting}")


# F2: Stop tapped while restart(B) starts the turn again: answered, yet the restarted turn runs
def test_stop_while_the_restart_after_go_on_starts_wins(bridges, fakes):
    bridge = bridges.new().start()
    thread = window(bridge, fakes)
    first = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000H3", W1)
    assert first.get("restarted"), first
    fakes.hermes.start_failures = 3
    assert wait_until(lambda: bridge.logged("restart: new turn not started yet"), timeout=10)
    stopped = bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={"agent": "wellness"})
    assert stopped.status_code == 200, stopped.text
    time.sleep(5)
    going = [r for r in with_words(fakes, W1) if fakes.hermes.started[r]["status"] != "cancelled"]
    assert going == [], f"Stop answered {stopped.json()}, then their restarted turn ran anyway: {going}"


# F2b: the same without go_on (steer-interrupt alone, restart(A)): already in int-release-next
def test_stop_while_a_restart_starts_wins_without_go_on(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    gate = threading.Event()
    fakes.hermes.runs["run_vie"] = [sse("message.delta", run_id="run_vie", delta="Je regarde la Fnac…"), gate,
                                    sse("run.cancelled", run_id="run_vie")]
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_vie/events"))
    fakes.hermes.start_failures = 3
    first = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000H4", W1)
    assert first.get("restarted"), first
    gate.set()                                      # Hermes ends the stopped turn
    assert wait_until(lambda: bridge.logged("restart: new turn not started yet"), timeout=10)
    stopped = bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={"agent": "wellness"})
    assert stopped.status_code == 200, stopped.text
    time.sleep(5)
    going = [r for r in with_words(fakes, W1) if fakes.hermes.started[r]["status"] != "cancelled"]
    assert going == [], f"Stop answered {stopped.json()}, then their restarted turn ran anyway: {going}"


# F4: a continuation restarted by new words (decision 3): on reload, the timeline drops the turn's own promise
def test_the_promise_stays_in_the_timeline_when_its_continuation_is_restarted(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    gate = threading.Event()
    bio = "En bio : un savon solide à 4,90 € chez Nocibé, avec le lien."
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    # the continuation only writes, then Hermes ends it cancelled (the bridge stopped it for their words)
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta="Je regarde…"), gate,
                               sse("run.cancelled", run_id="r1")]
    fakes.hermes.runs["r2"] = turn("r2", bio, tools=["web_search"])
    with bridge.stream("GET", "/v1/runs/run_vie/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/events"))
        time.sleep(0.3)
        sent = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000H5", "plutôt en bio")
        assert sent.get("restarted") and sent["run_id"] == "r1", sent
        gate.set()
        events = read_sse(resp)
    assert events[-1][1]["output"] == bio, events[-1]
    # Hermes' rows for the conversation as it keeps them
    fakes.hermes.add_session("api_c", [
        {"role": "user", "content": "Trouve-moi un achat test à 5 €."},
        {"role": "assistant", "content": PROMISE},
        {"role": "user", "content": fakes.hermes.started["r1"]["input"]},
        {"role": "assistant", "content": "Je regarde…"},
        {"role": "user", "content": fakes.hermes.started["r2"]["input"]},
        {"role": "assistant", "content": bio}], title="Achat test")
    page = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": "wellness"})
    assert page.status_code == 200, page.text
    shown = [(i["kind"], i.get("role"), i.get("text")) for i in page.json()["items"]]
    assert ("message", "agent", PROMISE) in shown, shown


# F5 (steer-interrupt alone, already in int-release-next) -- the restarted turn is not linked to the turn whose stream
# shows it (go_on links its continuation: `go_on_of`): its card is pushed while the app shows the conversation
def test_a_restarted_turns_card_is_not_pushed_while_the_app_shows_it(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": "e" * 64, "environment": "sandbox",
                                                          "agent_ids": ["wellness"]}).status_code == 200
    gate, finish = threading.Event(), threading.Event()
    fakes.hermes.runs["run_vie"] = [sse("message.delta", run_id="run_vie", delta="Je regarde la Fnac…"), gate,
                                    sse("run.cancelled", run_id="run_vie")]
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="terminal", preview="rm -rf build/"),
                               sse("approval.request", run_id="r1", request_id="req_1", command="rm -rf build/",
                                   choices=["once", "session", "deny"]),
                               finish, *turn("r1", FOUND)]
    with bridge.stream("GET", "/v1/runs/run_vie/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_vie/events"))
        sent = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000H6", "plutôt en bio")
        assert sent.get("restarted"), sent
        gate.set()
        assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"], timeout=10)
        time.sleep(0.5)
        pushed = [p for p in fakes.apns.payloads() if p["aps"].get("category") == "APPROVAL"]
        finish.set()
    assert pushed == [], "the restarted turn's card pushed while the app shows the conversation"


# -- re-review of 80bd6a227 (each failed on it) ------------------------------------------------------------------------

MEANT = "Non, je voulais dire chez Action"


def answered_or_waiting(bridge, fakes, words):
    began = wait_until(lambda: with_words(fakes, words), timeout=10)
    queue = bridge.get("/v1/agents/wellness/queue", headers=AUTH).json()
    waiting = [i.get("text") for i in queue.get("items", [])]
    return began, queue, waiting


# G1: Stop tapped, then new words while Hermes still ends the stopped turn: the words restart it (`restarted`),
# then restart() reads the earlier Stop (`stop_asked`) as a Stop of these words: nothing starts, nothing waits
def test_words_written_after_a_stop_are_not_lost(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    gate = threading.Event()
    fakes.hermes.runs["run_vie"] = [sse("message.delta", run_id="run_vie", delta="Je regarde la Fnac…"), gate,
                                    sse("run.cancelled", run_id="run_vie")]
    fakes.hermes.runs["r1"] = turn("r1", FOUND, tools=["web_search"])
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_vie/events"))
    stopped = bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={"agent": "wellness"})
    assert stopped.status_code == 200, stopped.text
    sent = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000J1", MEANT)   # what they meant, right after Stop
    gate.set()                                                                # Hermes ends the stopped turn
    began, queue, waiting = answered_or_waiting(bridge, fakes, MEANT)
    assert began or (MEANT in waiting and not queue.get("paused")), (
        f"Stop {stopped.json()}, then their words answered {sent}: never started, nothing waits "
        f"(started={list(fakes.hermes.started)}, queue={waiting}, paused={queue.get('paused')})")


# G1b: the same while go_on decides (the turn ended on its promise, its continuation not started yet)
def test_words_written_after_a_stop_while_go_on_decides_are_not_lost(bridges, fakes):
    bridge = bridges.new().start()
    thread = window(bridge, fakes)
    # go_on's second start refused too: it waits a second before its third
    assert wait_until(lambda: len(bridge.logged("promise with no work: continuation not started yet")) >= 2,
                      timeout=10)
    stopped = bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={"agent": "wellness"})
    assert stopped.status_code == 200, stopped.text
    sent = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000J2", MEANT)
    began, queue, waiting = answered_or_waiting(bridge, fakes, MEANT)
    assert began or (MEANT in waiting and not queue.get("paused")), (
        f"Stop {stopped.json()}, then their words answered {sent}: never started, nothing waits "
        f"(started={list(fakes.hermes.started)}, queue={waiting}, paused={queue.get('paused')})")


# G2: F5 one hop further: the continuation restarted by new words (decision 3); the restarted run's `restart_of` is
# the continuation, which no app follows (only go_on's forward listener): its card is pushed while the app shows it
def test_a_restarted_continuations_card_is_not_pushed_while_the_app_shows_it(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": "e" * 64, "environment": "sandbox",
                                                          "agent_ids": ["wellness"]}).status_code == 200
    gate, finish = threading.Event(), threading.Event()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta="Je regarde…"), gate,
                               sse("run.cancelled", run_id="r1")]
    fakes.hermes.runs["r2"] = [sse("tool.started", run_id="r2", tool="terminal", preview="rm -rf build/"),
                               sse("approval.request", run_id="r2", request_id="req_2", command="rm -rf build/",
                                   choices=["once", "session", "deny"]),
                               finish, *turn("r2", FOUND)]
    with bridge.stream("GET", "/v1/runs/run_vie/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/events"))
        time.sleep(0.3)
        sent = say(bridge, thread, "C0FFEE00-0000-4000-8000-0000000000J3", "plutôt en bio")
        assert sent.get("restarted") and sent["run_id"] == "r1", sent
        gate.set()
        assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"], timeout=10)
        time.sleep(0.5)
        pushed = [p for p in fakes.apns.payloads() if p["aps"].get("category") == "APPROVAL"]
        finish.set()
    assert pushed == [], "the restarted continuation's card pushed while the app shows the conversation"
