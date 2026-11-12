"""A reply cut by the model's length limit mid tool-call (reply_cut.rs, Alex 12/11: an agent blamed « la limite du
pont » and went through the terminal): Hermes ends the turn with its English copy; the person reads one plain line
instead, and the agent's next turn in that conversation is told what cut it and to write in parts."""
import json
import uuid

from bbkit.common import AUTH
from bbkit.fakes import CLOSE, sse, taken_notes, waiting_notes
from bbkit.servers import wait_until

# Hermes v0.23.1's copy (agent/turn_failure_copy.py "truncated"), as its run.failed carries it (measured 2026-11-12)
TRUNCATED = ("The model's reply was cut off before it finished (it hit its output length limit), so Hermes didn't run "
             "the incomplete action. Nothing was changed. Send `continue`, ask for the work in smaller steps, or raise "
             "max_tokens for this model.")


def failed_turn(bridge, error, agent="wellness", session="s1"):
    """One message in `session` whose run fails with `error`; the run's end as the apps read it."""
    hermes = bridge.fakes.hermes
    thread = bridge.get("/v1/threads/resolve", headers=AUTH,
                        params={"agent": agent, "session_id": session}).json()["thread"]["id"]
    run = f"r{len(hermes.started) + 1}"
    hermes.runs[run] = [sse("run.failed", run_id=run, error=error, partial=True, completed=False), CLOSE]
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": agent, "client_id": uuid.uuid4().hex, "text": "Écris le plan complet dans plan.md"})
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    run_id = sent.json()["run_id"]
    page = lambda: bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH,   # noqa: E731
                              params={"agent": agent, "limit": 1}).json()
    assert wait_until(lambda: not page()["pending"]["run"]), "the turn never ended"
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent={agent}", headers=AUTH) as resp:
        text = b"".join(resp.iter_raw()).decode()
    return json.loads(text.split("event: run.failed\ndata: ", 1)[1].split("\n", 1)[0])


def test_a_reply_cut_by_the_models_limit_is_said_plainly_and_the_agent_told_to_write_in_parts(bridge):
    ended = failed_turn(bridge, TRUNCATED)
    assert ended["bip_reply_cut"] is True
    assert "max_tokens" not in ended["error"] and "Hermes" not in ended["error"]
    # the place speaks English here
    assert ended["error"].startswith("Wellness's reply was cut by the model's length limit (not by BipAgents)"), ended
    # the agent's next turn in that conversation: the note, once
    note = waiting_notes(taken_notes(bridge, session="s1"))
    assert note.startswith("<!--bip:cut\n") and "length limit (not by BipAgents)" in note, note
    assert "in parts" in note and "append" in note
    assert waiting_notes(taken_notes(bridge, session="s1")) is None


def test_another_failure_keeps_its_words_and_no_note(bridge):
    ended = failed_turn(bridge, "agent run failed", session="s2")
    assert ended["error"] == "agent run failed" and "bip_reply_cut" not in ended
    assert waiting_notes(taken_notes(bridge, session="s2")) is None
