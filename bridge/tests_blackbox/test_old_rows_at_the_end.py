"""5/11 (Alex, Wellness, iPhone): after his message of the morning, the main Discussion showed four empty day
dividers, « Dimanche 1 novembre » … « Hier », under it. Wellness has daily routines (Hermes cron jobs before 3/11,
the bridge's since). Whatever the routines left (their Boîte reports of 1/11–4/11, their sessions in Hermes' store,
with or without a parent), nothing dated before the person's last message may come after it in the main thread."""
import sqlite3
import time

from bbkit.common import AUTH
from conftest import managed_install, profile

DAY = 86400.0


def store(home, now, parented):
    db = sqlite3.connect(profile(home, "coach") / "state.db")
    db.executescript("""
        CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, parent_session_id TEXT, model_config TEXT,
            started_at REAL NOT NULL, ended_at REAL, end_reason TEXT, title TEXT, hidden INTEGER NOT NULL DEFAULT 0);
        CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL, role TEXT NOT NULL,
            content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL NOT NULL, reasoning TEXT,
            active INTEGER NOT NULL DEFAULT 1, compacted INTEGER NOT NULL DEFAULT 0, display_kind TEXT,
            display_order INTEGER, message_uid TEXT, _compressed_summary INTEGER NOT NULL DEFAULT 0);
    """)
    sessions = [("api_main", "api_server", None, None, now - 10 * DAY, None, None, "Bot Chat")]
    messages = [("api_main", "user", "Bonjour", now - 10 * DAY, "u0", 1),
                ("api_main", "assistant", "Bonjour !", now - 10 * DAY + 5, "u1", 2)]
    order = 3
    for n in range(4, 0, -1):    # 1/11 … 4/11: a daily routine's run, its silent answer
        cron = f"cron_daily_{n}"
        at = now - n * DAY
        sessions.append((cron, "cron", "api_main" if parented else None, None, at, at + 30, None, None))
        messages += [(cron, "user", "Regarde mes données de santé. Rien de neuf : ne dis rien.", at, f"c{n}a", order),
                     (cron, "assistant", "[SILENT]", at + 20, f"c{n}b", order + 1)]
        order += 2
    messages.append(("api_main", "user", "Message vocal de ce matin", now - 60, "u9", order))
    db.executemany("INSERT INTO sessions (id, source, parent_session_id, model_config, started_at, ended_at, end_reason, "
                   "title) VALUES (?,?,?,?,?,?,?,?)", sessions)
    db.executemany("INSERT INTO messages (session_id, role, content, timestamp, message_uid, display_order) "
                   "VALUES (?,?,?,?,?,?)", messages)
    db.commit()


def check(bridges, fakes, parented, report_session):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    now = time.time()
    store(home, now, parented)
    rows = []
    for n in range(4, 0, -1):    # the reports the routines left in the Boîte (one with nothing to read: silent)
        at = time.strftime("%Y-%m-%dT%H:%M:%S.000Z", time.gmtime(now - n * DAY + 25))
        session = {"cron": f"cron_daily_{n}", "main": "api_main", "none": None}[report_session]
        rows.append(("INSERT INTO outbox (id, agent, title, text, created_at, session_id) VALUES (?,?,?,?,?,?)",
                     (f"o{n}", "coach", "Santé", "[SILENT]" if n % 2 else "Tout va bien.", at, session)))
    bridge.seed_db(rows)
    bridge.start()
    page = bridge.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": "coach", "limit": 100}).json()
    items = page["items"]
    said = [i for i in items if i.get("text") == "Message vocal de ce matin"]
    assert said, [(i["kind"], i.get("text"), i.get("at")) for i in items]
    after = items[items.index(said[0]) + 1:]
    assert after == [], [(i["kind"], i.get("text"), i.get("at")) for i in after]
    assert page["thread"]["tip"] == "api_main"
    return items


def test_routine_runs_without_parent_and_reports_in_their_sessions(bridges, fakes):
    check(bridges, fakes, parented=False, report_session="cron")


def test_routine_runs_whose_session_names_the_main_one_as_parent(bridges, fakes):
    check(bridges, fakes, parented=True, report_session="cron")


def test_reports_tagged_with_the_main_session(bridges, fakes):
    check(bridges, fakes, parented=False, report_session="main")


def test_reports_without_a_session(bridges, fakes):
    check(bridges, fakes, parented=True, report_session="none")
