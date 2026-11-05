"""The badges (the app icon's, the Boîte tab's) count what the Boîte shows and nothing else (bridge-rs/src/
notifications.rs `counts`, `read_gone`; card_place.rs `watch`). Bug of 2026-11-05 (Alex, iPhone only): « Boîte 1 »
for ever on an empty Boîte (« Rien de neuf »), Ghibli's dot with it: a journal row the Boîte never lists was counted.

- A card's row whose card waits no more (typed in its field, never placed, a Hermes approval lost with its run): read
  within a minute, `gone`; the badge and the agent's dot go.
- A card still waiting, even for days (the Boîte lists it): its row stays unread, counted.
- A room's reply, a held e-mail, a date to find: the home's dots (`agents`, `threads`), never the badge (`total`).
- `boite`: the badge by agent, for an app to add its own agents' (an agent not on the phone never counts there).
- A Hermes approval answered (`/v1/approve`): its row read at once."""
import threading
from datetime import datetime, timedelta, timezone

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until

NOW = "2026-11-02T20:00:00.000Z"
TOKEN = "a" * 64


def ago(**delta):
    when = datetime.now(timezone.utc) - timedelta(**delta)
    return when.strftime("%Y-%m-%dT%H:%M:%S.") + f"{when.microsecond // 1000:03d}Z"


def row(bridge, id, agent, kind, ref=None, thread=None, created=None):
    with bridge.db() as db:
        db.execute("INSERT INTO notifications (id, agent, type, thread_id, ref_id, title, body_key, state, created_at, "
                   "push, dedupe_key) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                   (id, agent, kind, thread, ref, agent, f"notif.{kind}", "open", created or ago(minutes=10), "sent:1",
                    f"{kind}:{agent}:{ref or id}"))


def unread(bridge):
    return bridge.get("/v1/notifications/unread", headers=AUTH).json()


def one(bridge, id):
    return bridge.get(f"/v1/notifications/{id}", headers=AUTH).json()


def started(bridges, fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat",
                             last_active=10, started_at=1)
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "production", "[]", NOW, NOW))])
    return bridge.start()


def waiting_approval(bridge, fakes, run_id="r_wait", request_id="req_wait"):
    """A Hermes approval waiting (the run held at the gate: each holds 30 s at most, three outlast the minute's sweep)."""
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("approval.request", run_id=run_id, request_id=request_id, command="rm notes.txt",
                                     choices=["once", "deny"]), gate, gate, gate, gate,
                                 sse("run.completed", run_id=run_id, output="Fait.")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "wellness", "run_id": run_id, "session_id": "api_main"}).status_code == 200
    assert wait_until(lambda: [a for a in bridge.get("/v1/approvals", headers=AUTH).json()["items"]
                               if a.get("request_id") == request_id], timeout=10)
    return gate


def test_the_badge_counts_only_what_the_boite_shows_and_a_gone_cards_mark_goes(bridges, fakes):
    bridge = started(bridges, fakes)
    gate = waiting_approval(bridge, fakes)
    listed = lambda: [r for r in bridge.get("/v1/notifications", headers=AUTH).json()["notifications"]  # noqa: E731
                      if r["type"] == "approval"]
    assert wait_until(listed)
    [waiting] = listed()
    # a card the Boîte lists, waiting for long (its row as if made 10 minutes ago: the sweep leaves fresh rows)
    with bridge.db() as db:
        db.execute("UPDATE notifications SET created_at = ? WHERE id = ?", (ago(minutes=10), waiting["id"]))
    # Alex's stuck row: a secret card typed (or withdrawn) where nothing read its notification
    row(bridge, "n_gone", "wellness", "secret", ref="sec_gone")
    # what the Boîte never lists: a room's reply, a held e-mail, a date to find
    row(bridge, "n_room", "wellness", "room_reply", ref="r1", thread="room:r1")
    row(bridge, "n_held", "wellness", "email_held", ref="held1")
    row(bridge, "n_meet", "wellness", "meet", ref="meet1")
    # a message of an agent not on this phone (another install's, a deleted one): its own key in `boite`
    row(bridge, "n_other", "ancien", "reply", ref="r_old", created=ago(days=5))
    counts = unread(bridge)
    assert counts["total"] == 3, counts                      # the approval, the stuck secret, the old message
    assert counts["boite"] == {"wellness": 2, "ancien": 1}, counts
    assert counts["agents"] == {"wellness": 5, "ancien": 1}, counts   # the home's dots: every unread row
    assert counts["threads"]["room:r1"] == 1
    # within a minute (the watch's sweep): the stuck card's row read, `gone`; the waiting card's kept
    assert wait_until(lambda: one(bridge, "n_gone")["read_at"], timeout=75), one(bridge, "n_gone")
    assert one(bridge, "n_gone")["state"] == "gone"
    assert one(bridge, waiting["id"])["read_at"] is None
    for id in ("n_room", "n_held", "n_meet", "n_other"):
        assert one(bridge, id)["read_at"] is None, id      # not the cards' sweep
    counts = unread(bridge)
    assert counts["total"] == 2 and counts["boite"] == {"wellness": 1, "ancien": 1}, counts
    # the devices told: the badge follows
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("reason") == "read" and "n_gone" in p.get("n", "")])
    silent = [p for p in fakes.apns.payloads() if p.get("reason") == "read" and "n_gone" in p.get("n", "")][0]
    assert silent["unread"] == 2
    # the approval answered from the Boîte: its row read at once, the badge with it
    answered = bridge.post("/v1/approve", headers=AUTH,
                           json={"agent": "wellness", "run_id": "r_wait", "request_id": "req_wait", "choice": "once"})
    assert answered.status_code == 200, answered.text
    assert wait_until(lambda: one(bridge, waiting["id"])["read_at"], timeout=5)
    assert unread(bridge)["boite"] == {"ancien": 1}
    gate.set()


def test_a_push_carries_the_boites_count_as_its_badge(bridges, fakes):
    bridge = started(bridges, fakes)
    row(bridge, "n_room", "wellness", "room_reply", ref="r1", thread="room:r1")
    row(bridge, "n_held", "wellness", "email_held", ref="held1")
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Le train de 9 h.")]
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Quel train ?"},
                                          {"role": "assistant", "content": "Le train de 9 h."}], title="Bot Chat",
                             last_active=10, started_at=1)
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_main"})
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("t") == "reply"])
    [reply] = [p for p in fakes.apns.payloads() if p.get("t") == "reply"]
    assert reply["aps"]["badge"] == 1      # the reply only: not the room's reply nor the held e-mail
