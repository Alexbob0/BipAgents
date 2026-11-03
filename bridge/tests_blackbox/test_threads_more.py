"""More of the bridge-owned conversations (docs/conversations.md): presence from the thread's event stream, rename
and delete, the read sync's one form, a shared agent's conversation through the owner's bridge, renames in items."""
import threading
import time

import httpx

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until
from test_households_and_peers import household
from test_shared_agents import link

NOW = "2026-11-02T20:00:00.000Z"


def get(bridge, path, **params):
    resp = bridge.get(path, params=params, headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()


def with_device(bridges):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     ("a" * 64, "production", "[]", NOW, NOW))])
    return bridge


def main_session(fakes, messages=None):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", messages or [{"role": "user", "content": "Quel train ?"},
                                                      {"role": "assistant", "content": "Le train de 9 h."}],
                             title="Bot Chat", last_active=10, started_at=1)


def test_a_thread_on_screen_is_not_pushed(bridges, fakes):
    main_session(fakes)
    bridge = with_device(bridges).start()
    main = get(bridge, "/v1/threads/main", agent="wellness")
    stop = threading.Event()

    def watch():   # the app shows the conversation: its event stream open with `thread=`
        with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
            with client.stream("GET", "/v1/threads/events", params={"agent": "wellness", "thread": "main"}) as resp:
                for _ in resp.iter_lines():
                    if stop.is_set():
                        return

    viewer = threading.Thread(target=watch, daemon=True)
    viewer.start()
    time.sleep(0.5)
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Le train de 9 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_main"})
    rows = lambda: bridge.get("/v1/notifications", headers=AUTH).json()["notifications"]   # noqa: E731
    assert wait_until(rows)
    assert rows()[0]["push"] == "skipped:on_screen" and rows()[0]["read_at"] and rows()[0]["c"] == main["id"]
    assert fakes.apns.payloads() == []
    stop.set()
    viewer.join(timeout=15)
    # the screen left: the next reply is pushed
    fakes.hermes.add_session("api_main", fakes.hermes.messages[("", "api_main")] +
                             [{"role": "assistant", "content": "Et le retour à 18 h."}], title="Bot Chat",
                             last_active=11, started_at=1)
    fakes.hermes.runs["r2"] = [sse("run.completed", output="Et le retour à 18 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r2", "session_id": "api_main"})
    assert wait_until(lambda: fakes.apns.payloads(), timeout=15)


def test_rename_delete_and_the_read_sync(bridges, fakes):
    main_session(fakes)
    fakes.hermes.add_session("api_side", [{"role": "user", "content": "Vacances ?"}], title="Vacances",
                             last_active=5, started_at=0.5)
    bridge = with_device(bridges).start()
    side = [t for t in get(bridge, "/v1/threads", agent="wellness")["threads"] if t["kind"] == "side"][0]
    renamed = bridge.patch(f"/v1/threads/{side['id']}", headers=AUTH, json={"title": "Été"}).json()
    assert renamed["title"] == "Été" and renamed["version"] > side["version"]
    assert fakes.hermes.patched[-1] == {"id": "api_side", "title": "Été"}
    main = bridge.patch("/v1/threads/main", headers=AUTH, json={"title": "Discussion", "agent": "wellness"}).json()
    assert main["title"] == "Discussion" and not [p for p in fakes.hermes.patched if p["id"] == "api_main"]
    assert bridge.delete(f"/v1/threads/{side['id']}", headers=AUTH).status_code == 204
    assert "api_side" in fakes.hermes.deleted and bridge.get(f"/v1/threads/{side['id']}", headers=AUTH).status_code == 404
    # the read sync: `n` one comma-separated string (APNs and FCM alike), `unread` the total
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Le train de 9 h.")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_main"})
    assert wait_until(lambda: fakes.apns.payloads())
    done = bridge.post("/v1/notifications/read", headers=AUTH, json={"agent": "wellness", "thread": "main"}).json()
    assert wait_until(lambda: any(p.get("reason") == "read" for p in fakes.apns.payloads()))
    silent = [p for p in fakes.apns.payloads() if p.get("reason") == "read"][0]
    assert silent["n"] == ",".join(done["read"]) and silent["unread"] == 0
    # deleting the main thread empties it, its id stays
    assert bridge.delete("/v1/threads/main", headers=AUTH, params={"agent": "wellness"}).status_code == 204
    assert "api_main" in fakes.hermes.deleted
    assert get(bridge, "/v1/threads/main", agent="wellness")["id"] == main["id"]


def test_an_agent_rename_reaches_the_app_through_its_item(bridges, fakes):
    main_session(fakes, [{"role": "user", "content": "Appelle-toi Léon"},
                         {"role": "assistant", "content": "D'accord !<!--bip:renamed wellness Léon-->"}])
    bridge = bridges.new().start()
    item = get(bridge, "/v1/threads/main/timeline", agent="wellness")["items"][1]
    assert item["text"] == "D'accord !" and item["renamed"] == [{"agent": "wellness", "name": "Léon"}]


def test_a_shared_agents_conversation_through_the_owners_bridge(bridges, fakes):
    fakes.hermes.v0225 = True
    a = household(bridges, fakes, "alex", ["cuisine"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]}).status_code == 200
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    # her first message opens the conversation (its words its title)
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Une fondue !")]
    first = b.post("/v1/threads/new/messages", headers=AUTH,
                   json={"agent": "cuisine__alex", "client_id": "C0FFEE00-0000-4000-8000-000000000010", "text": "Dîner"})
    assert first.status_code == 200, first.text
    cid = first.json()["thread"]
    session = fakes.hermes.started["r1"]["session_id"]
    fakes.hermes.messages[("cuisine", session)] = [
        {"role": "user", "content": "[Marie, a relative of your person, writes] Dîner"},
        {"role": "assistant", "content": "Une fondue !\nMEDIA:/bipagents/pages/cuisine/fondue.html"},
    ]
    assert wait_until(lambda: get(b, f"/v1/threads/{cid}/timeline", agent="cuisine__alex")["pending"]["run"] is None)
    listed = get(b, "/v1/threads", agent="cuisine__alex")["threads"]
    assert [(t["id"], t["kind"], t["title"]) for t in listed] == [(cid, "shared", "Dîner")]
    page = get(b, f"/v1/threads/{cid}/timeline", agent="cuisine__alex")
    assert page["thread"]["id"] == cid and [i["text"] for i in page["items"]] == ["Dîner", "Une fondue !"]
    assert page["items"][0]["client_id"] == "C0FFEE00-0000-4000-8000-000000000010"
    assert page["pending"] == {"run": None, "approvals": []}
    # a turn going on: its state and start, for the shared screen
    gate = threading.Event()
    fakes.hermes.runs["r2"] = [gate, sse("run.completed", output="Une tarte.")]
    sent = b.post(f"/v1/threads/{cid}/messages", headers=AUTH,
                  json={"agent": "cuisine__alex", "client_id": "C0FFEE00-0000-4000-8000-000000000011", "text": "Et le dessert ?"})
    assert sent.status_code == 200, sent.text
    running = get(b, f"/v1/threads/{cid}/timeline", agent="cuisine__alex")["pending"]["run"]
    assert running["status"] == "writing" and running["run_id"] == sent.json()["run_id"]
    assert isinstance(running["started"], (int, float))
    gate.set()
    assert all("attachments" not in i for i in page["items"])   # only text crosses
    assert page["items"][-1]["id"].startswith("m_")
    # another household's key reaches nothing
    assert a.get(f"/family/s/cuisine/conversations/{cid}/timeline",
                 headers={"Authorization": "Bearer nope"}).status_code == 401
