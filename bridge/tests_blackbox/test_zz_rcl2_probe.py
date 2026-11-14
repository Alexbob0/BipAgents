"""Second re-review of cards-lab-fixes (aeb2dc414), 14/11: its probes, which failed on aeb2dc414, kept as tests
(test_zz_: they run last)."""
import time

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_purchase_ready import purchase_turns, said, setup, step

REPLY = "[REHEARSAL OK]\nTout est prêt jusqu'à la page de paiement."


def conversation(bridge, session):
    resolved = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "vie", "session_id": session})
    assert resolved.status_code == 200, resolved.text
    thread = resolved.json()["thread"]["id"]
    page = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": "vie"})
    assert page.status_code == 200, page.text
    return [str(i.get("text") or "") for i in page.json()["items"]]


def test_rcl2_code_word_stays_hidden_once_the_watcher_is_removed(bridges, fakes):
    """The person removes the « Billets Stromae » watcher after buying; its Boîte messages stay and lead to its
    conversation. The rehearsal's code word must not show there then either."""
    bridge, conn, _ = setup(bridges, fakes)
    fakes.hermes.rounds = 4
    fakes.hermes.run_output = REPLY
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH).status_code == 202
    assert wait_until(lambda: purchase_turns(fakes), timeout=20)
    [(run, turn)] = purchase_turns(fakes).items()
    agent, session = turn["agent"], turn["session_id"]
    fakes.hermes.messages[(agent, session)] = [{"role": "user", "content": turn["input"], "timestamp": time.time() - 5},
                                               {"role": "assistant", "content": REPLY, "timestamp": time.time() - 1}]
    kept = fakes.hermes.sessions.get(agent, {}).get(session)
    if kept is not None:
        kept.update(message_count=2, last_active=time.time())
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["state"] == "ok", timeout=40)
    assert wait_until(lambda: said(bridge), timeout=20)
    shown = conversation(bridge, session)
    assert not [t for t in shown if "REHEARSAL" in t], f"before the removal already: {shown}"   # the fix
    assert bridge.delete(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH).status_code == 204
    assert said(bridge), "its Boîte messages are still there (they lead to its conversation)"
    shown = conversation(bridge, session)
    assert any("Tout est prêt" in t for t in shown), shown
    assert not [t for t in shown if "REHEARSAL" in t], f"the code word back in the conversation: {shown}"


def test_rcl2_a_rehearsal_that_went_through_is_never_told_blocked(bridges, fakes):
    """The code word says OK; the agent adds a translated line that reassures « rien n'a été bloqué ». The checklist
    must say ok, and the person must not be told it was blocked."""
    bridge, conn, _ = setup(bridges, fakes)
    fakes.hermes.rounds = 1
    fakes.hermes.run_output = ("[REHEARSAL OK]\nRépétition réussie : rien n'a été bloqué, le formulaire de carte "
                               "s'est affiché.")
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH).status_code == 202
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["state"] in ("ok", "blocked"), timeout=30)
    assert wait_until(lambda: said(bridge), timeout=20)
    got = step(bridge, conn, "rehearsal")
    assert got["state"] == "ok", f"checklist: {got}; the person read: {said(bridge)}"
