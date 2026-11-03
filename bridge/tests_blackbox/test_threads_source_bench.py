"""Where the bridge reads a conversation (bridge-rs/src/threads/source.rs): Hermes' API or the profile's state.db, the
same 400 messages over 3 segments both ways, against the fake Hermes on this machine. `-s` prints the table (times
say the order of magnitude; the fake answers from memory, a real Hermes reads its own state.db first)."""
import sqlite3
import time

from bbkit.common import AUTH
from conftest import managed_install, profile
from test_threads_bench import MESSAGES, thread

ROUNDS = 20


def state_db(path):
    per = MESSAGES // 3
    db = sqlite3.connect(path)
    db.executescript((__import__("pathlib").Path(__file__).parents[2] / "bridge-rs/tests/golden/hermes_state_schema.sql")
                     .read_text().replace("REFERENCES system_prompts(hash)", "REFERENCES sessions(id)"))
    db.executemany("INSERT INTO sessions (id, source, parent_session_id, started_at, end_reason, title) VALUES (?,?,?,?,?,?)",
                   [("s1", "api_server", None, 1, "compression", "Bot Chat"),
                    ("s2", "api_server", "s1", 2, "compression", "Bot Chat"),
                    ("s3", "api_server", "s2", 3, None, "Bot Chat")])
    db.executemany("INSERT INTO messages (session_id, role, content, timestamp, message_uid, display_order) "
                   "VALUES (?,?,?,?,?,?)",
                   [(f"s{1 + min(i // per, 2)}", "user" if i % 2 == 0 else "assistant",
                     f"Message {i} : " + "quelques mots de plus pour la taille d'un vrai message. " * 3,
                     1_000_000 + i, f"u{i}", i) for i in range(MESSAGES)])
    db.commit()


def timed(bridge, agent):
    started = time.perf_counter()
    resp = bridge.get("/v1/threads/main/timeline", params={"agent": agent, "limit": 50}, headers=AUTH)
    assert resp.status_code == 200, resp.text
    return (time.perf_counter() - started) * 1000, resp.json()


def test_a_conversation_read_through_hermes_api_and_through_state_db(bridges, fakes):
    thread(fakes)   # the API agent's (wellness) conversation in the fake Hermes
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    state_db(profile(home, "coach") / "state.db")
    bridge.start()
    rows = {}
    for agent, label in (("wellness", "Hermes API"), ("coach", "state.db")):
        before = len(fakes.hermes.requests)
        cold, page = timed(bridge, agent)
        cold_calls = len(fakes.hermes.requests) - before
        warm = sorted(timed(bridge, agent)[0] for _ in range(ROUNDS))[ROUNDS // 2]
        rows[label] = (cold, cold_calls, warm, len(page["items"]))
    print("\n| Source | first open ms | Hermes calls | next opens ms (median) | items |\n|---|---:|---:|---:|---:|")
    for label, (cold, calls, warm, items) in rows.items():
        print(f"| {label} | {cold:.1f} | {calls} | {warm:.1f} | {items} |")
    assert rows["state.db"][1] == 0                         # no call to Hermes at all
    assert rows["Hermes API"][3] == rows["state.db"][3] == 50
