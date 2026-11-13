"""A message sent while the agent answers (bridge-rs/src/turn_restart.rs, Alex 12/11): before the turn did anything
(it thinks, writes, reads) the turn is stopped and started again with the new words, one answer for both, in the same
stream the apps follow; once it acted (a mail sent, a card asked) the words go in at its next step (Hermes' steer)."""
import json
import threading

import httpx

from bbkit.common import AUTH, run_has, sse
from bbkit.servers import wait_until

SESSION = "api_main"
Q1 = "C1D00000-0000-4000-8000-0000000000R1"
P1 = "C1D00000-0000-4000-8000-0000000000R2"
P2 = "C1D00000-0000-4000-8000-0000000000R3"


def user(text):
    return {"role": "user", "content": text}


def agent(text):
    return {"role": "assistant", "content": text}


def setup(bridges, fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.rounds = 1000
    fakes.hermes.add_session(SESSION, [user("Bonjour"), agent("Salut")], title="Bot Chat", last_active=10, started_at=1)
    bridge = bridges.new().start()
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "wellness"}).json()
    return bridge, main["id"]


def send(bridge, thread, client_id, text):
    body = {"agent": "wellness", "client_id": client_id, "text": text}
    answer = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH, json=body)
    assert answer.status_code == 200, answer.text
    return answer.json()


def page(bridge, thread):
    answer = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": "wellness"})
    assert answer.status_code == 200, answer.text
    return answer.json()


def follow(bridge, run_id, out):
    """An app following the run (its own connection: the test's client is not shared across threads), its events
    kept as they come."""
    path = f"/v1/runs/{run_id}/events?agent=wellness&session_id={SESSION}"
    with httpx.Client(base_url=bridge.url, timeout=60.0) as client, client.stream("GET", path, headers=AUTH) as resp:
        assert resp.status_code == 200
        name = None
        for line in resp.iter_lines():
            if line.startswith("event:"):
                name = line[6:].strip()
            elif line.startswith("data:") and name:
                out.append((name, json.loads(line[5:].strip())))
                if name in ("run.completed", "run.failed", "run.cancelled"):
                    return


def test_words_before_any_action_restart_the_turn_with_both(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    # the turn reads (a search changes nothing) and starts answering; Hermes ends it cancelled once stopped
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"),
                               sse("tool.started", run_id="r1", tool="web_search", preview="écoles"),
                               sse("tool.completed", run_id="r1", tool="web_search"),
                               sse("message.delta", run_id="r1", delta="Voici trois éc"),
                               gate, sse("run.cancelled", run_id="r1")]
    answer = "Les trois écoles publiques : A, B et C."
    fakes.hermes.runs["r2"] = [sse("run.started", run_id="r2"), sse("message.delta", run_id="r2", delta=answer),
                               sse("run.completed", run_id="r2", output=answer)]
    assert send(bridge, thread, Q1, "Compare trois écoles")["state"] == "started"
    app = []
    reader = threading.Thread(target=follow, args=(bridge, "r1", app), daemon=True)
    reader.start()
    assert wait_until(lambda: any(name == "message.delta" for name, _ in app), timeout=10)
    sent = send(bridge, thread, P1, "Seulement les publiques")
    # taken into the turn going on (the apps as before), which starts again with it
    assert (sent["state"], sent["run_id"], sent["restarted"]) == ("steered", "r1", True), sent
    assert fakes.hermes.started["r1"]["status"] == "cancelled"
    assert fakes.hermes.steers == [], "no steer: Hermes would take it only after a tool call"
    gate.set()
    assert wait_until(lambda: "r2" in fakes.hermes.started, timeout=15)
    restarted = fakes.hermes.started["r2"]
    assert restarted["session_id"] == SESSION
    assert restarted["input"].startswith("<!--bip:restarted\n") and restarted["input"].endswith("\nSeulement les publiques")
    # the app following the first run sees the new one go on in the same stream, and its end
    reader.join(timeout=30)
    names = [name for name, _ in app]
    notice = [data for name, data in app if name == "bridge.notice"]
    assert [(n["code"], n["run_id"], n["reason"]) for n in notice] == [("loop_rescue_started", "r2", "restarted")]
    assert names[-1] == "run.completed" and app[-1][1]["output"] == answer
    assert "run.cancelled" not in names
    # one conversation: the two messages and one answer (the cut words and Hermes' line out)
    fakes.hermes.messages[("", SESSION)] += [
        user("<!--bip:now lundi 9 h-->\nCompare trois écoles"), agent("Voici trois éc"),
        agent("Your request was not processed. Send it again if you still want me to carry it out."),
        user(f"<!--bip:now lundi 9 h-->\n{fakes.hermes.started['r2']['input']}"), agent(answer)]
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    items = [i for i in page(bridge, thread)["items"] if i["kind"] == "message"]
    assert [(i["role"], i["text"]) for i in items[-3:]] == [
        ("person", "Compare trois écoles"), ("person", "Seulement les publiques"), ("agent", answer)]
    assert [i.get("client_id") for i in items[-3:-1]] == [Q1, P1]
    assert "bip:" not in str(items)


def test_words_after_an_action_go_into_the_turn_at_its_next_step(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"),
                               sse("tool.started", run_id="r1", tool="mcp__bipagents__mail_send", preview="Léa"),
                               sse("tool.completed", run_id="r1", tool="mcp__bipagents__mail_send"),
                               gate, sse("run.completed", run_id="r1", output="Envoyé.")]
    assert send(bridge, thread, Q1, "Écris à Léa")["state"] == "started"
    assert run_has(bridge, "r1", "tool.completed")
    sent = send(bridge, thread, P1, "Et mets Paul en copie")
    assert (sent["state"], sent["run_id"]) == ("steered", "r1") and "restarted" not in sent
    assert fakes.hermes.steers[-1]["body"] == {"input": "Et mets Paul en copie"}
    assert fakes.hermes.started["r1"]["status"] == "running", "an action is never cut in the middle"
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    assert list(fakes.hermes.started) == ["r1"]


def test_a_card_waiting_counts_as_an_action_and_a_stop_wins_over_a_restart(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"),
                               sse("approval.request", run_id="r1", request_id="a1", command="rm notes.txt"),
                               gate, sse("run.completed", run_id="r1", output="Fait.")]
    assert send(bridge, thread, Q1, "Range mes notes")["state"] == "started"
    assert run_has(bridge, "r1", "approval.request")
    assert "restarted" not in send(bridge, thread, P1, "Garde celles de mai")
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    # a turn that only thinks, then the person stops it right after a new message: nothing starts again
    stop = threading.Event()
    fakes.hermes.runs["r2"] = [sse("run.started", run_id="r2"), sse("reasoning.delta", run_id="r2", text="Hmm"),
                               stop, sse("run.cancelled", run_id="r2")]
    assert send(bridge, thread, "C1D00000-0000-4000-8000-0000000000R4", "Un menu pour ce soir")["state"] == "started"
    assert run_has(bridge, "r2", "reasoning.delta")
    assert send(bridge, thread, P2, "Sans viande")["restarted"] is True
    assert bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={"agent": "wellness"}).json()["stopped"] is True
    stop.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    assert list(fakes.hermes.started) == ["r1", "r2"], "the person's stop: no new turn"
