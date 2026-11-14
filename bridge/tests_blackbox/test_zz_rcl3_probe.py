"""Third re-review of cards-lab-fixes (0bdc7263a), 14/11: its probe, which failed on 0bdc7263a, kept as a test."""
from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_purchase_ready import said, setup, step


def test_rcl3_code_word_at_the_end_of_a_line_is_read_and_never_shown(bridges, fakes):
    """The model puts its code word after its sentence, on the same line: the checklist says ok, the person never
    reads the code word."""
    bridge, conn, _ = setup(bridges, fakes)
    fakes.hermes.rounds = 1
    fakes.hermes.run_output = "Tout est prêt jusqu'à la page de paiement. [REHEARSAL OK]"
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH).status_code == 202
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["state"] in ("ok", "blocked"), timeout=30)
    assert wait_until(lambda: said(bridge), timeout=20)
    got = step(bridge, conn, "rehearsal")
    assert got["state"] == "ok" and not [t for t in said(bridge) if "REHEARSAL" in t], \
        f"checklist: {got}; the person read: {said(bridge)}"
