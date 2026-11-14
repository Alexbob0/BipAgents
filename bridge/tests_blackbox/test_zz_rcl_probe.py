"""Independent re-review of cards-lab-fixes (8de44ad67), 14/11: probes that failed on 8de44ad67, kept as tests
(test_zz_: they run last)."""
import time

from bbkit.common import AUTH, boite_items
from bbkit.servers import wait_until
from test_purchase_ready import purchase_turns, said, setup, step

REPLY = "[REHEARSAL OK]\nTout est prêt jusqu'à la page de paiement."


def test_rcl_rehearsal_code_word_never_in_the_watchers_conversation(bridges, fakes):
    """The person opens the watcher's conversation from the Boîte: the rehearsal turn's reply is one of its rows (the
    bridge's note hidden). Its code word must not show there either."""
    bridge, conn, _ = setup(bridges, fakes)
    fakes.hermes.rounds = 4
    fakes.hermes.run_output = REPLY
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH).status_code == 202
    assert wait_until(lambda: purchase_turns(fakes), timeout=20)
    [(run, turn)] = purchase_turns(fakes).items()
    agent, session = turn["agent"], turn["session_id"]
    # what Hermes keeps of that turn in the watcher's conversation: the bridge's hidden note, then the agent's reply
    fakes.hermes.messages[(agent, session)] = [{"role": "user", "content": turn["input"], "timestamp": time.time() - 5},
                                               {"role": "assistant", "content": REPLY, "timestamp": time.time() - 1}]
    kept = fakes.hermes.sessions.get(agent, {}).get(session)
    if kept is not None:
        kept.update(message_count=2, last_active=time.time())
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["state"] == "ok", timeout=40)
    assert wait_until(lambda: said(bridge), timeout=20)
    assert not [t for t in said(bridge) if "REHEARSAL" in t], said(bridge)   # the Boîte: fine
    resolved = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "vie", "session_id": session})
    assert resolved.status_code == 200, resolved.text
    thread = resolved.json()["thread"]["id"]
    page = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": "vie"})
    assert page.status_code == 200, page.text
    shown = [str(i.get("text") or "") for i in page.json()["items"]]
    assert any("Tout est prêt" in t for t in shown), shown   # the conversation was read
    assert not [t for t in shown if "REHEARSAL" in t], f"the code word in the watcher's conversation: {shown}"


def test_rcl_code_word_after_an_emoji_is_read_and_never_shown(bridges, fakes):
    """GLM opens many replies with an emoji: « ✅ [REHEARSAL OK] ». The verdict must be read (ok) and the code word
    never reach the person."""
    bridge, conn, _ = setup(bridges, fakes)
    fakes.hermes.rounds = 1
    fakes.hermes.run_output = "✅ [REHEARSAL OK]\nTout est prêt jusqu'à la page de paiement."
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH).status_code == 202
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["state"] in ("ok", "blocked"), timeout=30)
    assert wait_until(lambda: said(bridge), timeout=20)
    got = step(bridge, conn, "rehearsal")
    assert got["state"] == "ok", f"checklist: {got}; the person read: {said(bridge)}"
    assert not [t for t in said(bridge) if "REHEARSAL" in t], said(bridge)


def test_rcl_a_task_report_is_never_hermes_interrupt_line(bridges, fakes):
    """jobs.rs reads a task's end without ended_with: a task resumed after the brake paused it (« loop ») runs on in
    the same conversation, where the model may echo Hermes' placeholder as its whole reply (as the watcher's did)."""
    from test_jobs import TASK, view, with_device
    bridge = with_device(bridges)
    fakes.hermes.rounds = 1
    fakes.hermes.run_output = "[interrupt: no assistant output for this turn]"
    started = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK)
    assert started.status_code == 200, started.text
    job = started.json()
    assert wait_until(lambda: view(bridge, job["id"])["state"] != "running", timeout=30)
    ended = view(bridge, job["id"])
    assert "[interrupt" not in str(ended.get("report") or ""), ended
    time.sleep(3)
    told = [i for i in boite_items(bridge) if "[interrupt" in str(i.get("text"))]
    assert not told, told
