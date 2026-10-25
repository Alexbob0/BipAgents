"""The apps' search through every conversation (`GET /v1/search/messages`): each agent's Hermes store
(`<profile>/state.db`) read as Hermes writes it, newest first, only what the person and the agent said."""
import sqlite3

from bbkit.common import AUTH
from conftest import managed_install, profile


def store(folder, sessions, messages):
    db = sqlite3.connect(folder / "state.db")
    db.executescript("""
        CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, parent_session_id TEXT, started_at REAL NOT NULL,
            end_reason TEXT, title TEXT, hidden INTEGER NOT NULL DEFAULT 0, model_config TEXT);
        CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL,
            content TEXT, timestamp REAL NOT NULL, active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0,
            display_kind TEXT, display_metadata TEXT, _compressed_summary INTEGER NOT NULL DEFAULT 0, tool_name TEXT);
        PRAGMA journal_mode=WAL;
    """)
    for sid, parent, end, title, hidden in sessions:
        db.execute("INSERT INTO sessions (id, source, parent_session_id, started_at, end_reason, title, hidden)"
                   " VALUES (?, 'api_server', ?, 0, ?, ?, ?)", (sid, parent, end, title, hidden))
    for sid, role, content, ts, extra in messages:
        db.execute(f"INSERT INTO messages (session_id, role, content, timestamp{''.join(', ' + k for k in extra)})"
                   f" VALUES (?, ?, ?, ?{', ?' * len(extra)})", (sid, role, content, ts, *extra.values()))
    db.commit()
    return db


def test_every_conversation_of_every_agent_is_searched_newest_first(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach", "chef"))
    store(profile(home, "coach"), [
        ("old", None, None, "Vacances", 0),
        ("new", None, "compression", "Rentrée", 0),
        ("new2", "new", None, None, 0),          # its continuation: shown as « new »
        ("sub", "new2", None, None, 0),          # a sub-agent's run
        ("bot", None, None, "Bot Chat", 1),      # the Discussion
        ("peer", None, None, "agents", 1),       # Bot Mode's own
    ], [
        ("old", "user", "On réserve l'École de voile ?", 1_000_000.0, {}),
        ("old", "assistant", "Oui, l'ecole de VOILE ouvre en juin.", 1_000_100.0, {}),
        ("old", "tool", "ecole voile ecole voile", 1_000_200.0, {"tool_name": "web"}),
        ("new", "user", "Rien à voir", 2_000_000.0, {}),
        ("new2", "user", "Et l'école de voile, c'est payé ?", 2_000_100.0, {}),
        ("new2", "user", "école de voile (caché)", 2_000_200.0, {"display_kind": "hidden"}),
        ("new2", "user", "résumé : école de voile", 2_000_300.0, {"_compressed_summary": 1}),
        ("new2", "user", "école de voile annulée", 2_000_400.0, {"active": 0}),
        ("sub", "assistant", "école de voile (sous-agent)", 2_000_500.0, {}),
        ("bot", "user", "Rappelle l'école de voile", 3_000_000.0, {}),
        ("peer", "user", "école de voile entre agents", 3_000_100.0, {}),
    ])
    store(profile(home, "chef"), [("c1", None, None, None, 0)], [
        ("c1", "assistant", "Une école de cuisine, pas de voile… ou si : voile !", 1_500_000.0, {}),
    ])
    bridge.start()
    assert "message_search" in bridge.get("/health").json()["features"]
    found = bridge.get("/v1/search/messages", params={"q": "ECOLE voile"}, headers=AUTH).json()
    seen = [(r["agent"], r["session_id"], r["role"], r["timestamp"]) for r in found["results"]]
    assert seen == [("coach", "bot", "person", 3_000_000.0), ("coach", "new", "person", 2_000_100.0),
                    ("chef", "c1", "agent", 1_500_000.0), ("coach", "old", "agent", 1_000_100.0),
                    ("coach", "old", "person", 1_000_000.0)]
    first = found["results"][1]
    assert first["session_title"] == "Rentrée" and first["date"] == "1970-01-24T03:35:00.000Z"
    assert first["snippet"] == "Et l'école de voile, c'est payé ?"
    assert [p for p in first["parts"] if p["hit"]] == [{"text": "école", "hit": True}, {"text": "voile", "hit": True}]
    assert isinstance(first["message_id"], int)
    assert sorted(found["searched"]) == ["chef", "coach"] and found["unsearched"] == ["wellness"]
    assert found["partial"] is False and found["query"] == "ECOLE voile"
    # one agent, a limit
    one = bridge.get("/v1/search/messages", params={"q": "voile", "agent": "chef"}, headers=AUTH).json()
    assert [r["agent"] for r in one["results"]] == ["chef"] and one["searched"] == ["chef"]
    two = bridge.get("/v1/search/messages", params={"q": "voile", "limit": 2}, headers=AUTH).json()
    assert len(two["results"]) == 2
    # every word must match
    none = bridge.get("/v1/search/messages", params={"q": "voile piscine"}, headers=AUTH).json()
    assert none["results"] == []


def test_the_search_is_for_the_bridge_key_and_known_agents(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    store(profile(home, "coach"), [("s", None, None, None, 0)], [("s", "user", "bonjour", 1.0, {})])
    store(profile(home, "stranger"), [("s", None, None, None, 0)], [("s", "user", "bonjour", 1.0, {})])  # not claimed
    bridge.start()
    assert bridge.get("/v1/search/messages", params={"q": "bonjour"}).status_code == 401
    found = bridge.get("/v1/search/messages", params={"q": "bonjour"}, headers=AUTH).json()
    assert [r["agent"] for r in found["results"]] == ["coach"]
    assert bridge.get("/v1/search/messages", params={"q": "x", "agent": "stranger"}, headers=AUTH).status_code == 404
    assert bridge.get("/v1/search/messages", headers=AUTH).status_code == 422
    assert bridge.get("/v1/search/messages", params={"q": "x", "limit": 0}, headers=AUTH).status_code == 422
    empty = bridge.get("/v1/search/messages", params={"q": "   "}, headers=AUTH).json()
    assert empty["results"] == []
