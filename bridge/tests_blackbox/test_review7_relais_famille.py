"""Security review, confirmation of R6–R8 (branch family-relay-r6-r8 aa3867db7): the statements' numbers go back after
a restore.

A bridge numbers what it says of the passes it accepts (`x-bip-passes-seq`, `seq`), and the relay keeps the highest
number: an older statement never undoes a newer one (R6). The numbers come from a count of the bridge's starts kept in
`family-relay.json` (`starts`, shifted 32 bits, then counted up while it runs). Put back an older `family-relay.json` —
a computer restored from its backup, a place restored from its own — and the next start counts from the same number as
a start already made: its statements are lower than the last the relay kept, so the relay ignores them all, as long as
it remembers the mailbox (it never forgets one that keeps polling). A link removed then is never told: the household
withdrawn reaches the bridge again, tells whether it is on (the privacy paragraph's promise).

Fails on aa3867db7; should pass once a start's numbers are always above every earlier one (from the clock as a floor,
`max(starts + 1, now in seconds)`, or the relay told of a new start)."""
import httpx

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_family_relay_anywhere import (  # noqa: F401
    RECEIVED, allow_messages, echo, households, install, link, link_agents, named, own_mailbox, relay, write)


def test_a_withdrawal_after_a_restore_from_backup_still_holds(bridges, fakes, relay):  # noqa: F811
    RECEIVED.clear()
    fakes.hermes.override = echo
    alex = install(bridges, fakes, "alex", ["vie"], relay.url)
    lea = install(bridges, fakes, "lea", ["cuisine"], relay.url)
    paul = install(bridges, fakes, "paul", ["coach"], relay.url)
    named(alex, "Alex")
    named(lea, "Léa")
    named(paul, "Paul")
    link(alex, lea)
    link(paul, lea)
    assert wait_until(lambda: "cuisine__lea" in link_agents(alex), timeout=40), alex.log_tail()
    assert wait_until(lambda: "cuisine__lea" in link_agents(paul), timeout=40), paul.log_tail()
    coach, _ = allow_messages(paul, "coach", lea, "cuisine", "paul", "lea")
    held = households(alex)["lea"]
    box, alex_pass, token = held["mailbox"], held["pass"], own_mailbox(alex)["token"]

    # a backup of Léa's computer, taken while her bridge is stopped
    relay_state = lea.state_dir / "family-relay.json"
    lea.stop()
    backup = relay_state.read_bytes()
    lea.start()
    # her bridge runs a while: Paul writes to her (each call, another poll: another statement)
    for i in range(10):
        sent = write(paul, coach, "cuisine__lea", f"message {i}", "coach")
        assert sent.status_code == 200, sent.text
    # her computer is restored from that backup
    lea.stop()
    relay_state.write_bytes(backup)
    lea.start()
    assert wait_until(lambda: write(paul, coach, "cuisine__lea", "de retour ?", "coach").status_code == 200,
                      timeout=60), lea.log_tail()

    # she removes Alex: Paul gets a newer pass, the relay is told
    paul_pass = households(paul)["lea"]["pass"]
    assert lea.delete("/v1/family/alex", headers=AUTH).status_code in (200, 204)
    assert wait_until(lambda: households(paul)["lea"].get("pass") not in (None, paul_pass), timeout=40), lea.log_tail()
    assert wait_until(lambda: write(paul, coach, "cuisine__lea", "toujours là", "coach").status_code == 200,
                      timeout=40), paul.log_tail()

    got = httpx.post(f"{relay.url}/v1/family/call/{box}", content=b"\x01" + b"x" * 64, timeout=30,
                     headers={"Authorization": f"Bearer {token}", "x-bip-kind": "call", "x-bip-pass": alex_pass,
                              "content-type": "application/octet-stream"})
    assert got.status_code == 503, (
        f"after Léa's computer was restored from a backup, removing Alex was never told to the relay (her statements "
        f"numbered below those of the start before): his pass still reaches her bridge ({got.status_code})")
