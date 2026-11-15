"""Security review, second pass (branch family-relay-anywhere c18c86384): what the fixes of the first review leave.

- A household unlinked keeps the pass it was given at the link: the relay still lets it reach the mailbox (passes are
  never withdrawn, and their holder renews them itself), so it still sees whether the home's computer is on, and can
  still hold that mailbox's share of the relay's room (relay/tests/family_review2.rs).
- The scanning bridge's first try at the inviter's address is sealed only when the invitation carries a relay mailbox:
  an invitation made while the relay was out of reach carries none, and the code goes in clear again to whatever
  answers at that address on the scanner's network (the first review's M2, through another door).

Each test fails on c18c86384."""
import httpx

from bbkit.common import AUTH
from test_family_relay_anywhere import echo, households, link, link_agents, named, own_mailbox, relay  # noqa: F401
from test_review_relais_famille import Device, home
from bbkit.servers import wait_until


def test_a_household_unlinked_no_longer_reaches_the_mailbox(bridges, fakes, relay):  # noqa: F811
    fakes.hermes.override = echo
    alex = home(bridges, fakes, "alex", ["vie"], relay.url)
    lea = home(bridges, fakes, "lea", ["cuisine"], relay.url)
    paul = home(bridges, fakes, "paul", ["coach"], relay.url)
    named(alex, "Alex")
    named(lea, "Léa")
    named(paul, "Paul")
    link(alex, lea)
    link(paul, lea)   # Léa keeps polling the relay for Paul after Alex is gone
    assert wait_until(lambda: "cuisine__lea" in link_agents(alex), timeout=40), alex.log_tail()
    assert wait_until(lambda: "cuisine__lea" in link_agents(paul), timeout=40), paul.log_tail()
    # what Alex's bridge holds of the link: Léa's mailbox, the pass she gave, its own mailbox's token
    held = households(alex)["lea"]
    box, pass_, token = held["mailbox"], held["pass"], own_mailbox(alex)["token"]

    # Léa unlinks Alex
    assert lea.delete("/v1/family/alex", headers=AUTH).status_code in (200, 204)
    assert "alex" not in households(lea)
    assert wait_until(lambda: "coach__paul" in link_agents(lea), timeout=40), lea.log_tail()   # she still polls

    # Alex (or whoever has a copy of that pass) calls her mailbox: the relay should read it as a stranger's call
    got = httpx.post(f"{relay.url}/v1/family/call/{box}", content=b"\x01" + b"x" * 64, timeout=30,
                     headers={"Authorization": f"Bearer {token}", "x-bip-kind": "call", "x-bip-pass": pass_,
                              "content-type": "application/octet-stream"})
    assert got.status_code == 503, (
        f"after Léa unlinked Alex, his pass still reaches her bridge through the relay ({got.status_code} "
        f"{got.content[:80]!r}): he still tells whether her computer is on, and can still hold her share of the room")


def test_the_code_never_goes_in_clear_even_if_the_relay_was_out_of_reach_at_the_invitation(bridges, fakes,
                                                                                          relay):  # noqa: F811
    device = Device(status=404)
    try:
        fakes.hermes.override = echo
        alex = home(bridges, fakes, "alex", ["vie"], relay.url, device=device)
        lea = home(bridges, fakes, "lea", ["cuisine"], relay.url)
        named(alex, "Alex")
        named(lea, "Léa")
        relay.stop()   # a moment the relay can't be reached from Léa's home (or `relay_url = "off"`)
        invitation = lea.post("/v1/family/invite", headers=AUTH).json()
        relay.start()
        assert "relay" not in invitation, invitation
        alex.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
        sent = b"".join(device.received)
        assert invitation["code"].encode() not in sent, (
            f"an invitation made while the relay was out of reach: its code (and Alex's key) went in clear to a "
            f"device at {invitation['url']} on Alex's network: {sent[:400]!r}")
    finally:
        device.stop()
