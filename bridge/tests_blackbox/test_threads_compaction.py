"""An agent's main conversation after Hermes compacted it, read from state.db: the app pages back to its very first
message (Alex 4/11: « Discussion » stopped at yesterday). The fixtures are state.db rows written by a real Hermes
(v0.22.5 + BipAgents' patches, on the build server, a fake model answering « ok <n> » and a 12k-token compression
threshold): `in_place.json`, 40 notes compacted in place 8 times; `updated_032.json` / `updated_033.json`, 30 notes
written on the old pin 517b5e10f (which compacts in place too, and had not yet), then turns on v0.22.5 on the same store:
the store after its 32nd turn (two compactions) and after the 33rd (a third, whose summary took « Note 25 »'s uid).
Words are cut to « Note <n> » / « ok <n> » (the model's padding left out); internal/notes/history-compaction.md."""
import json
import sqlite3
from pathlib import Path

from bbkit.common import AUTH
from conftest import managed_install, profile

FIXTURES = Path(__file__).parent / "fixtures" / "hermes_compaction"
GOLDEN = Path(__file__).parents[2] / "bridge-rs" / "tests" / "golden" / "hermes_state_schema.sql"


def state_db(folder: Path, fixture: str) -> dict:
    data = json.loads((FIXTURES / fixture).read_text())
    db = sqlite3.connect(folder / "state.db")
    db.executescript(GOLDEN.read_text().replace("REFERENCES system_prompts(hash)", "REFERENCES sessions(id)"))
    for table in ("sessions", "messages"):
        for row in data[table]:
            cols = list(row)
            db.execute(f"INSERT INTO {table} ({', '.join(cols)}) VALUES ({', '.join('?' * len(cols))})",
                       [row[c] for c in cols])
    db.commit()
    return data


def whole_timeline(bridge, agent, limit=7):
    """Every item, the way an app scrolls up: the last page, then `before` its first cursor until nothing is left."""
    page = bridge.get("/v1/threads/main/timeline", params={"agent": agent, "limit": limit}, headers=AUTH).json()
    items = page["items"]
    for _ in range(500):
        if not page["more_before"]:
            break
        page = bridge.get("/v1/threads/main/timeline", params={"agent": agent, "limit": limit, "before": page["first"]},
                          headers=AUTH).json()
        items = page["items"] + items
    return items


def check_every_note(items, notes):
    words = [i.get("text") for i in items if i.get("kind") == "message"]
    want = [w for n in range(1, notes + 1) for w in (f"Note {n}", f"ok {n}")]
    assert words == want, words
    ids = [i["id"] for i in items]
    assert len(ids) == len(set(ids)), "an item twice"
    assert all(i["id"].startswith("m_") and len(i["id"]) == 34 for i in items), "not Hermes' message_uid"
    assert not any("PRIOR CONTEXT" in (i.get("text") or "") or "CONTEXT COMPACTION" in (i.get("text") or "")
                   for i in items), "a summary shown"


def test_a_conversation_compacted_in_place_pages_back_to_its_first_message(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    state_db(profile(home, "coach"), "in_place.json")
    bridge.start()
    items = whole_timeline(bridge, "coach")
    check_every_note(items, 40)
    # the same read again gives the same ids (the apps keep them)
    assert [i["id"] for i in whole_timeline(bridge, "coach", limit=50)] == [i["id"] for i in items]


def cursors(bridge, agent):
    return {i["id"]: i["cursor"] for i in whole_timeline(bridge, agent, limit=50)}


def test_a_compaction_leaves_every_message_where_it_was(bridges, fakes):
    """The cursors the apps hold stay right after Hermes compacts the conversation again: no message moves, none goes
    ahead of the ones said after it (the iPhone kept its top message and asked the same older page again and again
    when a page brought a message it held further down: « Discussion » stopped there)."""
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(home, "coach")
    state_db(folder, "updated_032.json")
    bridge.start()
    before = cursors(bridge, "coach")
    (folder / "state.db").unlink()
    state_db(folder, "updated_033.json")    # the 33rd turn, its compaction: the bridge reads state.db at each request
    items = whole_timeline(bridge, "coach")
    check_every_note(items, 33)
    after = {i["id"]: i["cursor"] for i in items}
    assert {k: v for k, v in after.items() if k in before} == before, "a message moved"
    order = [i["id"] for i in items if i["id"] in before]
    assert order == [k for k in before if k in after]
