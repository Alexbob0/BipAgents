"""18/11 (Alex, Vie, iPhone): a photo sent on a turn whose preflight compaction ran in place. Hermes keeps that person's
row with its raw parts (agent/turn_context.py: « compaction inserted the raw parts »), written `\\x00json:` + JSON
(hermes_state_messages.py `_encode_content`), the photo's data URL inside. The bridge read it as words: a text of
hundreds of thousands of characters (a giant black bubble on the iPhone, after quitting too) with no photo, so the
message sent from the phone never matched it and stayed at the end, « Pas envoyé · Toucher pour réessayer », although
the agent had answered it."""
import base64
import json
import sqlite3
import time
from pathlib import Path

from bbkit.common import AUTH
from conftest import managed_install, profile

# a real JPEG's bytes (any: the bridge serves them back as they were sent)
JPEG = base64.b64decode(
    "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgy"
    "PC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AKp//2Q==")


def get(bridge, path, **params):
    resp = bridge.get(path, params=params, headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()


def test_a_photo_hermes_kept_as_raw_parts_is_the_photo_sent(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    now = time.time()
    db = sqlite3.connect(profile(home, "coach") / "state.db")
    db.executescript("""
        CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, parent_session_id TEXT, model_config TEXT,
            started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, title TEXT, hidden INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL,
            content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL NOT NULL, reasoning TEXT,
            active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0, display_kind TEXT,
            display_order INTEGER, message_uid TEXT, _compressed_summary INTEGER NOT NULL DEFAULT 0);
    """)
    saved = f"{profile(home, 'coach')}/native-inputs/ab/api_ab.jpg"
    parts = [{"type": "text", "text": f"<!--bip:now mercredi 18 novembre, 11:44-->\n\n[Image attached at: {saved}]"},
             {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64," + base64.b64encode(JPEG).decode()}}]
    db.execute("INSERT INTO sessions (id, source, started_at, title) VALUES ('api_main', 'api_server', ?, 'Bot Chat')",
               (now - 600,))
    db.executemany("INSERT INTO messages (session_id, role, content, timestamp, message_uid, display_order) "
                   "VALUES (?,?,?,?,?,?)", [
                       ("api_main", "user", "Bonjour", now - 600, "u1", 1),
                       ("api_main", "assistant", "Bonjour !", now - 590, "u2", 2),
                       ("api_main", "user", "\x00json:" + json.dumps(parts), now - 60, "u3", 3),
                       ("api_main", "assistant", "Hy4 flash gagne le rapport qualité-prix.", now - 30, "u4", 4),
                   ])
    db.commit()
    bridge.start()
    main = get(bridge, "/v1/threads/main", agent="coach")
    # the phone's send of that photo, kept by the bridge until Hermes stores it (send.rs `keep`)
    sent = [{"kind": "image", "name": "photo.jpg", "mime": "image/jpeg", "size": len(JPEG), "path": None}]
    with bridge.db() as conn:
        conn.execute("INSERT INTO thread_sends (agent, client_id, thread_id, text, attachments, message_id, created_at) "
                     "VALUES (?,?,?,?,?,NULL,?)",
                     ("coach", "C0FFEE00-1111-2222-3333-444455556666", main["id"], "", json.dumps(sent),
                      time.strftime("%Y-%m-%dT%H:%M:%S.000000Z", time.gmtime(now - 61))))
    page = get(bridge, "/v1/threads/main/timeline", agent="coach")
    items = page["items"]
    shown = [(i["id"], i.get("text", "")[:60], len(i.get("attachments") or []), i.get("not_sent"), i.get("sending"))
             for i in items]
    photo = next(i for i in items if i["id"] == "m_u3")
    assert photo["text"] == "", shown
    assert "base64" not in json.dumps(items), "no data URL in the timeline"
    assert [a["kind"] for a in photo["attachments"]] == ["image"], shown
    assert photo.get("client_id") == "C0FFEE00-1111-2222-3333-444455556666", "the photo sent is this row"
    assert [i["id"] for i in items if i["id"].startswith("s_")] == [], f"no « Pas envoyé » copy at the end: {shown}"
    # its file is the photo sent
    resp = bridge.get(f"/v1/threads/{main['id']}/attachments/m_u3.a0", params={"agent": "coach"}, headers=AUTH)
    assert resp.status_code == 200, resp.text
    assert resp.content == JPEG
