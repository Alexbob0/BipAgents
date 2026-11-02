"""The notification journal (docs/conversations.md §9): every notification is a row first, then a push pointing at it
(`v`, `n`, `t`, `c`, `m`) beside the legacy fields; a reply's push waits for its message and carries its id; the same
reply from two producers is one row; « Me prévenir » off silences replies but never approvals or questions; read
state shared through the journal."""
import hashlib
import time

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until

NOW = "2026-11-02T20:00:00.000Z"
TOKEN = "a" * 64


def uid(session, i, agent=""):
    return "m_" + hashlib.md5(f"{agent}/{session}/{i}".encode()).hexdigest()


def with_device(bridges):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "production", "[]", NOW, NOW))])
    return bridge


def journal(bridge, **params):
    return bridge.get("/v1/notifications", params=params, headers=AUTH).json()["notifications"]


def main_with_reply(fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Quel train ?"},
                                          {"role": "assistant", "content": "Le train de 9 h."}],
                             title="Bot Chat", last_active=10, started_at=1)


def test_a_reply_push_points_at_its_stored_message(bridges, fakes):
    main_with_reply(fakes)
    bridge = with_device(bridges).start()
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Le train de 9 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_main"})
    assert wait_until(lambda: fakes.apns.payloads())
    payload = fakes.apns.payloads()[0]
    main = bridge.get("/v1/threads/main", params={"agent": "wellness"}, headers=AUTH).json()
    assert payload["v"] == 2 and payload["t"] == "reply" and payload["n"].startswith("n_")
    assert payload["c"] == main["id"] and payload["m"] == uid("api_main", 1)
    # the legacy fields are all there for older apps
    assert payload["kind"] == "reply" and payload["run_id"] == "r1" and payload["session_id"] == "api_main"
    assert payload["reply_id"] == payload["n"]
    assert bridge.get(f"/v1/replies/{payload['reply_id']}", headers=AUTH).json()["text"] == "Le train de 9 h."
    row = bridge.get(f"/v1/notifications/{payload['n']}", headers=AUTH).json()
    assert row["m"] == payload["m"] and row["c"] == main["id"] and row["push"] == "sent:1" and row["read_at"] is None
    # another producer of the same reply (a hand-back, the watcher): one row, one push
    fakes.hermes.runs["r2"] = [sse("run.completed", output="Le train de 9 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r2", "session_id": "api_main"})
    time.sleep(2.5)
    assert len(fakes.apns.payloads()) == 1 and len(journal(bridge)) == 1
    assert bridge.logged("notification already sent")


def test_read_state_is_the_journals(bridges, fakes):
    main_with_reply(fakes)
    bridge = with_device(bridges).start()
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Le train de 9 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_main"})
    assert wait_until(lambda: journal(bridge))
    main = bridge.get("/v1/threads/main", params={"agent": "wellness"}, headers=AUTH).json()
    assert main["unread"] == 1
    unread = bridge.get("/v1/notifications/unread", headers=AUTH).json()
    assert unread == {"total": 1, "agents": {"wellness": 1}, "threads": {main["id"]: 1}}
    done = bridge.post("/v1/notifications/read", headers=AUTH, json={"agent": "wellness", "thread": "main"}).json()
    assert done["read"] == [journal(bridge)[0]["id"]] and done["unread"]["total"] == 0
    # the other devices are told (a silent push), the badge with it
    assert wait_until(lambda: any(p.get("reason") == "read" for p in fakes.apns.payloads()))
    silent = [p for p in fakes.apns.payloads() if p.get("reason") == "read"][0]
    assert silent["n"] == done["read"] and silent["unread"] == 0
    assert journal(bridge, unread=1) == []
    assert bridge.delete(f"/v1/notifications/{done['read'][0]}", headers=AUTH).status_code == 204
    assert journal(bridge) == []


def test_the_switch_silences_replies_never_approvals_or_questions(bridges, fakes):
    main_with_reply(fakes)
    bridge = with_device(bridges).start()
    assert bridge.put("/v1/agents/wellness/notify", headers=AUTH, json={"on": False}).status_code == 200
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Le train de 9 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_main"})
    assert wait_until(lambda: journal(bridge))
    assert journal(bridge)[0]["push"] == "skipped:switch" and fakes.apns.payloads() == []
    fakes.hermes.runs["r2"] = [
        sse("approval.request", run_id="r2", request_id="req_1", command="rm notes.txt", choices=["once", "deny"]),
        sse("clarify.request", run_id="r2", kind="clarify", prompt_id="c4", execution_generation=1,
            question="Quel jour ?", choices=["lundi", "mardi"]),
    ]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r2", "session_id": "api_main"})
    assert wait_until(lambda: len(fakes.apns.payloads()) == 2)
    kinds = sorted(p["t"] for p in fakes.apns.payloads())
    assert kinds == ["approval", "question"]
    question = [p for p in fakes.apns.payloads() if p["t"] == "question"][0]
    assert question["kind"] == "question" and question["request_id"] == "c4"
    rules = {(r["type"], r.get("from")): r["follows_switch"]
             for r in bridge.get("/v1/notifications/rules", headers=AUTH).json()["rules"]}
    assert rules[("question", None)] is False and rules[("reply", None)] is True
    assert rules[("boite", "task")] is True and rules[("boite", "routine")] is False
