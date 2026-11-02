"""The conversations the bridge owns (docs/conversations.md): one stable thread per conversation whatever Hermes does
with titles and session ids, a thread = the chain of sessions Hermes continued it in after compressing it, one
timeline with stable ids, the Boîte's messages among them, `around` a message with everything after it. The fake
Hermes plays v0.22.5 (`v0225`): continuations listed only with `include_children=1`, `message_uid` on every row; a
peer's own « Bot Chat » is a hidden `local-…` session (patch 0021, branch botchat-v0225)."""
import hashlib
import json
import sqlite3

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until
from conftest import managed_install, profile

NOW = "2026-11-02T20:00:00.000Z"


def uid(session, i, agent=""):
    return "m_" + hashlib.md5(f"{agent}/{session}/{i}".encode()).hexdigest()


def user(text):
    return {"role": "user", "content": text}


def agent(text):
    return {"role": "assistant", "content": text}


def get(bridge, path, **params):
    resp = bridge.get(path, params=params, headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()


def texts(page):
    return [i.get("text") for i in page["items"]]


def compressed_main(fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_old", [user("Bonjour"), agent("Salut")], title="Bot Chat", last_active=10,
                             started_at=1, end_reason="compression")
    fakes.hermes.add_session("api_new", [user("Et demain ?"), agent("Il pleut.")], title="Bot Chat", last_active=20,
                             started_at=2, parent_session_id="api_old", end_reason=None)
    fakes.hermes.add_session("api_side", [user("Vacances ?")], title="Vacances", last_active=15, started_at=1.5)


def test_the_main_thread_is_adopted_with_its_whole_line(bridges, fakes):
    compressed_main(fakes)
    bridge = bridges.new().start()
    assert "threads" in bridge.get("/health").json()["features"]
    listed = get(bridge, "/v1/threads", agent="wellness")["threads"]
    main = listed[0]
    assert main["kind"] == "main" and main["id"].startswith("t_") and len(main["id"]) == 28
    assert main["tip"] == "api_new"
    assert [t["kind"] for t in listed] == ["main", "side"] and listed[1]["title"] == "Vacances"
    page = get(bridge, f"/v1/threads/{main['id']}/timeline", agent="wellness", debug=1)
    assert texts(page) == ["Bonjour", "Salut", "Et demain ?", "Il pleut."]
    assert [i["id"] for i in page["items"]] == [uid("api_old", 0), uid("api_old", 1), uid("api_new", 0), uid("api_new", 1)]
    assert [i["role"] for i in page["items"]] == ["person", "agent", "person", "agent"]
    assert page["items"][1]["turn"] == uid("api_old", 0) and page["items"][3]["segment"] == "api_new"
    assert page["more_before"] is False and page["more_after"] is False and page["anchor"] is None
    # the same thread by its alias and by id, the same ids on a second read
    assert get(bridge, "/v1/threads/main", agent="wellness")["id"] == main["id"]
    assert get(bridge, f"/v1/threads/{main['id']}/timeline", debug=1)["items"] == page["items"]
    plain = get(bridge, f"/v1/threads/{main['id']}/timeline")["items"]
    assert "segment" not in plain[0] and "turn" not in plain[0] and "attachments" not in plain[0]
    # nothing after the last cursor; the older page before the first of the last two
    assert get(bridge, f"/v1/threads/{main['id']}/timeline", after=page["last"])["items"] == []
    last_two = get(bridge, f"/v1/threads/{main['id']}/timeline", limit=2)
    assert texts(last_two) == ["Et demain ?", "Il pleut."] and last_two["more_before"] is True
    older = get(bridge, f"/v1/threads/{main['id']}/timeline", limit=2, before=last_two["first"])
    assert texts(older) == ["Bonjour", "Salut"]
    # one item
    one = get(bridge, f"/v1/threads/{main['id']}/items/{uid('api_new', 1)}")
    assert one["text"] == "Il pleut." and one["cursor"] == page["items"][3]["cursor"]


def test_an_old_notification_opens_the_live_thread_with_everything_after(bridges, fakes):
    compressed_main(fakes)
    bridge = bridges.new().start()
    main = get(bridge, "/v1/threads/main", agent="wellness")
    # Hermes compresses again during a turn and goes on in a third session; the turn's end tells the bridge
    fakes.hermes.sessions[""]["api_new"]["end_reason"] = "compression"
    fakes.hermes.add_session("api_tip", [user("Et jeudi ?"), agent("Soleil.")], title=None, last_active=30,
                             started_at=3, parent_session_id="api_new")
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Soleil.")]
    events = []
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_tip"})
    assert wait_until(lambda: get(bridge, "/v1/threads/main", agent="wellness")["version"] > main["version"])
    # a push sent before (its session: the first segment, its message: « Salut »)
    found = get(bridge, "/v1/threads/resolve", agent="wellness", session_id="api_old")
    assert found["thread"]["id"] == main["id"] and found["tip"] == "api_tip"
    around = get(bridge, f"/v1/threads/{main['id']}/timeline", around=uid("api_old", 1), before_count=0)
    assert around["anchor"] == uid("api_old", 1)
    assert texts(around) == ["Salut", "Et demain ?", "Il pleut.", "Et jeudi ?", "Soleil."]
    assert around["more_after"] is False and around["more_before"] is True
    segments = get(bridge, f"/v1/threads/{main['id']}")["segments"]
    assert [s["session_id"] for s in segments] == ["api_old", "api_new", "api_tip"]
    assert [s["linked"] for s in segments] == ["adopted", "compression", "compression"]
    # the continuation never became a thread of its own
    assert [t["kind"] for t in get(bridge, "/v1/threads", agent="wellness")["threads"]] == ["main", "side"]
    del events


def test_a_routine_run_on_hermes_0225_is_a_hidden_routine_thread(bridges, fakes):
    fakes.hermes.v0225 = True
    routine = "local-" + "ab" * 32
    fakes.hermes.add_session(routine, [user("Fais le point"), agent("Tout va bien.")], title="Point du matin",
                             last_active=5, started_at=5, source="cron")
    bridge = bridges.new().start()
    found = get(bridge, "/v1/threads/resolve", agent="wellness", session_id=routine)["thread"]
    assert found["kind"] == "routine" and found["hidden"] is True and found["origin"]["kind"] == "routine"
    assert [t["kind"] for t in get(bridge, "/v1/threads", agent="wellness")["threads"]] == ["main"]
    hidden = get(bridge, "/v1/threads", agent="wellness", include_hidden=1)["threads"]
    assert sorted(t["kind"] for t in hidden) == ["main", "routine"]
    assert texts(get(bridge, f"/v1/threads/{found['id']}/timeline")) == ["Fais le point", "Tout va bien."]


def test_the_main_thread_never_depends_on_the_bot_chat_title(bridges, fakes):
    """botchat-v0225 moves the person's conversation off « Bot Chat » (Hermes v0.22.5's peer FIFO takes text runs
    there): its record wins over the title, and a later move keeps the thread's id."""
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("local-peer", [user("Message from bip/vie (@vie): le train ?")], title="Bot Chat",
                             last_active=50, started_at=1, hidden=True, source="cli", chat_id="local-peer")
    fakes.hermes.add_session("api_person", [user("Coucou"), agent("Bonjour !")], title="Discussion", last_active=40,
                             started_at=2)
    bridge = bridges.new()
    bridge.data.mkdir(parents=True, exist_ok=True)
    (bridge.data / "main-sessions.json").write_text(json.dumps({"wellness": "api_person"}))
    bridge.start()
    main = get(bridge, "/v1/threads/main", agent="wellness")
    assert main["tip"] == "api_person"
    assert texts(get(bridge, "/v1/threads/main/timeline", agent="wellness")) == ["Coucou", "Bonjour !"]
    # the peer conversation is a thread of its own; a peer's words read as the peer's
    peer = get(bridge, "/v1/threads/resolve", agent="wellness", session_id="local-peer")["thread"]
    assert peer["id"] != main["id"] and peer["kind"] == "pair" and peer["hidden"] is True
    item = get(bridge, f"/v1/threads/{peer['id']}/timeline")["items"][0]
    assert item["role"] == "peer" and item["from"] == "vie"
    # a migration moves the conversation to a new session: same thread, both segments read
    fakes.hermes.add_session("api_moved", [user("On continue ici")], title="Discussion", last_active=60, started_at=3)
    moved = bridge.post(f"/v1/threads/{main['id']}/segments", headers=AUTH, json={"session_id": "api_moved"}).json()
    assert moved["id"] == main["id"] and moved["tip"] == "api_moved"
    assert [s["linked"] for s in moved["segments"]] == ["adopted", "moved"]
    assert texts(get(bridge, "/v1/threads/main/timeline", agent="wellness")) == ["Coucou", "Bonjour !", "On continue ici"]


def test_markers_attachments_tools_and_boite_messages(bridges, fakes):
    fakes.hermes.v0225 = True
    photo = "data:image/png;base64,iVBORw0KGgo="
    fakes.hermes.add_session("api_main", [
        user("<!--bip:now lundi 9 h-->Range le carnet"),
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "c1", "function": {"name": "notebook_fill", "arguments": "{\"page\": \"x\"}"}}]},
        {"role": "tool", "content": "{\"ok\": true}", "tool_call_id": "c1", "tool_name": "notebook_fill"},
        agent("C'est rangé.\nMEDIA:/bipagents/pages/wellness/carnet.html"),
        {"role": "user", "content": [{"type": "text", "text": "Yes"}, {"type": "image_url", "image_url": {"url": photo}}]},
        user("[OUT-OF-BAND USER MESSAGE from the user]\n[bipagents:handoff] hand over\n[/OUT-OF-BAND USER MESSAGE]"),
        user("[OUT-OF-BAND USER MESSAGE from the user]\nEt la suite ?\n[/OUT-OF-BAND USER MESSAGE]"),
    ], title="Bot Chat", last_active=10, started_at=1)
    bridge = bridges.new().start()
    main = get(bridge, "/v1/threads/main", agent="wellness")
    # a routine's report in the Boîte, without a conversation of its own: shown in the main thread
    bridge.db().execute("INSERT INTO outbox (id, agent, ntfy_id, title, text, created_at, session_id) VALUES "
                        "(?,?,?,?,?,?,?)", ("o1", "wellness", "hermes-session:x", "Point", "Le point du soir.",
                                            "2099-01-01T00:00:00.000Z", None)).connection.commit()
    items = get(bridge, f"/v1/threads/{main['id']}/timeline")["items"]
    kinds = [(i["kind"], i.get("text")) for i in items]
    assert kinds == [("message", "Range le carnet"), ("tool", None), ("message", "C'est rangé."), ("message", "Yes"),
                     ("message", "Et la suite ?"), ("boite", "Le point du soir.")]
    tool = items[1]["tool"]
    assert tool["name"] == "notebook_fill" and tool["status"] == "done" and tool["call_id"] == "c1"
    assert items[1]["id"] == uid("api_main", 1) + ".t0"
    page = items[2]["attachments"][0]
    assert page["kind"] == "page" and page["path"] == "/bipagents/pages/wellness/carnet.html"
    image = items[3]["attachments"][0]
    assert image["kind"] == "image" and image["id"] == uid("api_main", 4) + ".a0"
    assert items[4]["added_during_run"] is True
    assert items[5]["id"] == "o_o1" and items[5]["boite"]["outbox_id"] == "o1"
    # the photo's bytes, by its id
    got = bridge.get(image["url"], headers=AUTH)
    assert got.status_code == 200 and got.headers["content-type"] == "image/png" and got.content.startswith(b"\x89PNG")


def test_the_hermes_store_is_read_without_calling_hermes(bridges, fakes):
    """A managed install: the profile's state.db is read directly (its `message_uid`s, its display order, its
    continuations); opening a conversation costs Hermes no call."""
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    db = sqlite3.connect(profile(home, "coach") / "state.db")
    db.executescript("""
        CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, parent_session_id TEXT, model_config TEXT,
            started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, title TEXT, hidden INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL,
            content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL NOT NULL, reasoning TEXT,
            active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0, display_kind TEXT,
            display_order INTEGER, message_uid TEXT, _compressed_summary INTEGER NOT NULL DEFAULT 0);
    """)
    db.executemany("INSERT INTO sessions (id, source, parent_session_id, model_config, started_at, end_reason, title) "
                   "VALUES (?,?,?,?,?,?,?)", [
                       ("s1", "api_server", None, None, 1, "compression", "Bot Chat"),
                       ("s2", "api_server", "s1", None, 2, None, None),
                       ("sub", "tool", "s2", '{"_delegate_from": "s2"}', 3, None, None),
                   ])
    db.executemany("INSERT INTO messages (session_id, role, content, timestamp, message_uid, display_order, active, "
                   "compacted, _compressed_summary) VALUES (?,?,?,?,?,?,?,?,?)", [
                       ("s1", "user", "Premier", 10, "u1", 1, 1, 0, 0),
                       ("s1", "assistant", "Réponse", 11, "u2", 2, 0, 1, 0),      # compacted: still shown
                       ("s1", "assistant", "Annulée", 12, "u3", 3, 0, 0, 0),      # rewound: not shown
                       ("s2", "user", "Résumé de ce qui précède", 13, "u4", 4, 1, 0, 1),
                       ("s2", "user", "Deuxième", 14, "u5", 5, 1, 0, 0),
                       ("sub", "assistant", "Sous-agent", 15, "u6", 6, 1, 0, 0),
                   ])
    db.commit()
    bridge.start()
    before = len(fakes.hermes.requests)
    page = get(bridge, "/v1/threads/main/timeline", agent="coach")
    assert [i["id"] for i in page["items"]] == ["m_u1", "m_u2", "m_u5"]
    assert page["thread"]["tip"] == "s2"
    assert [r.path for r in fakes.hermes.requests[before:] if "/api/sessions" in r.path] == []


def test_a_new_agents_main_thread_gets_its_first_conversation(bridges, fakes):
    """No conversation yet: the main thread exists, empty, with its id; the person's first conversation becomes its
    first segment (never a side thread)."""
    fakes.hermes.v0225 = True
    bridge = bridges.new().start()
    main = get(bridge, "/v1/threads/main", agent="wellness")
    assert main["tip"] is None and get(bridge, "/v1/threads/main/timeline", agent="wellness")["items"] == []
    fakes.hermes.add_session("api_first", [user("Bonjour")], title="Bot Chat", last_active=5, started_at=5)
    found = get(bridge, "/v1/threads/resolve", agent="wellness", session_id="api_first")
    assert found["thread"]["id"] == main["id"] and found["tip"] == "api_first"
    assert texts(get(bridge, "/v1/threads/main/timeline", agent="wellness")) == ["Bonjour"]
