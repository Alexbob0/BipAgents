"""The questions waiting in the Boîte's list (`GET /v1/approvals` `questions`, 13/11) and their notification rows,
across the family agent's one conversation (12/11: each card is answered by the person whose turn asked it).

- A member's question (her turn, followed from her bridge): in HER list, never in the owner's nor another member's;
  its notification row kept while it waits (the minute's sweep marked it `gone` and its silent push took it off her
  lock screen: review of fix-clarify-card, 13/11).
- The owner's own question in the family conversation: in his list, never in a member's.
- A question of this place's own agent: its row kept while it waits, a stale one read."""
import threading
import time

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until
from test_boite_badge import ago, one
from test_family_agent import make_family_agent
from test_family_thread import discover, family, page


def questions(bridge, agent=None):
    answer = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent} if agent else None)
    assert answer.status_code == 200, answer.text
    return answer.json()["questions"]


def asked(run, prompt, words):
    return sse("clarify.request", run_id=run, kind="clarify", prompt_id=prompt, execution_generation=1,
               question=words, choices=["Samedi", "Dimanche"], multi_select=False)


def test_a_members_question_is_hers_and_its_row_stays_while_it_waits(bridges, fakes):
    a, b, c = family(bridges, fakes)
    make_family_agent(a)
    discover(b, c)
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), asked("r1", "c_9", "Samedi ou dimanche ?"),
                               gate, gate, gate, gate, gate, gate, sse("run.completed", run_id="r1", output="Ok.")]
    sent = b.post("/v1/threads/main/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000C2",
                        "text": "On fait quoi ce week-end ?"})
    assert sent.status_code == 200, sent.text
    rows = lambda: [r for r in b.get("/v1/notifications", headers=AUTH).json()["notifications"]  # noqa: E731
                    if r["type"] == "question"]
    assert wait_until(rows, timeout=30), b.get("/v1/notifications", headers=AUTH).json()
    [row] = rows()
    # hers: in her list (all agents, and the family agent's), with her conversation
    assert wait_until(lambda: [q for q in questions(b) if q.get("request_id") == "c_9"], timeout=15), questions(b)
    [mine] = [q for q in questions(b, "famille__alex") if q.get("request_id") == "c_9"]
    assert mine["question"].startswith("Samedi ou dimanche") and mine["agent"] == "famille__alex"
    assert mine["created_at"].endswith("Z") and mine["id"].startswith("a_")
    # never the owner's, nor another member's
    assert all(q.get("run_id") != "r1" for q in questions(a))
    assert all(q.get("run_id") != "r1" for q in questions(a, "famille"))
    assert questions(c) == [] and questions(c, "famille__alex") == []
    # as if asked 10 minutes ago (the sweep leaves rows younger than 2 minutes), still waiting on Alex's place
    with b.db() as db:
        db.execute("UPDATE notifications SET created_at = ? WHERE id = ?", (ago(minutes=10), row["id"]))
    time.sleep(80)   # one sweep at least
    assert any(q.get("request_id") == "c_9" for q in page(a, "famille")["pending"]["questions"])
    kept = one(b, row["id"])
    assert kept["read_at"] is None and kept["state"] != "gone", kept
    gate.set()


def test_the_owners_question_in_the_family_conversation_is_his(bridges, fakes):
    a, b, c = family(bridges, fakes)
    make_family_agent(a)
    discover(b, c)
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), asked("r1", "c_10", "On part quand ?"),
                               gate, gate, gate, sse("run.completed", run_id="r1", output="Ok.")]
    sent = a.post("/v1/threads/main/messages", headers=AUTH,
                  json={"agent": "famille", "client_id": "C0FFEE00-0000-4000-8000-0000000000C3", "text": "Vacances ?"})
    assert sent.status_code == 200, sent.text
    assert wait_until(lambda: [q for q in questions(a) if q.get("request_id") == "c_10"], timeout=15), questions(a)
    assert wait_until(lambda: page(b, "famille__alex")["pending"]["questions"], timeout=15)
    assert all(q.get("request_id") != "c_10" for q in questions(b))
    assert questions(b, "famille__alex") == [] and questions(c) == []
    gate.set()


def test_an_owners_waiting_question_row_is_kept_a_stale_one_read(bridges, fakes):
    from test_boite_badge import row, started
    bridge = started(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["run_w"] = [sse("clarify.request", run_id="run_w", kind="clarify", prompt_id="c_w",
                                      execution_generation=1, question="Lequel ?", choices=["A", "B"]),
                                  gate, gate, gate, gate, gate, sse("run.completed", run_id="run_w", output="Fait.")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "wellness", "run_id": "run_w", "session_id": "api_main"}).status_code == 200
    assert wait_until(lambda: questions(bridge), timeout=15)
    mine = lambda: [r for r in bridge.get("/v1/notifications", headers=AUTH).json()["notifications"]  # noqa: E731
                    if r["type"] == "question"]
    assert wait_until(mine, timeout=15)
    [waiting] = mine()
    with bridge.db() as db:
        db.execute("UPDATE notifications SET created_at = ? WHERE id = ?", (ago(minutes=10), waiting["id"]))
    row(bridge, "n_qgone2", "wellness", "question", ref="c_old")
    assert wait_until(lambda: one(bridge, "n_qgone2")["read_at"], timeout=80)
    assert one(bridge, waiting["id"])["read_at"] is None, one(bridge, waiting["id"])
    gate.set()
