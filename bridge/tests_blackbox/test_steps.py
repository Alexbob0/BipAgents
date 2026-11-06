"""« Étapes » (docs/conversations.md §6.3b, steps.rs): the agent's task list as one card per turn. Stored: one `steps`
item per turn from its `todo_list` calls (direct or through the tool search), none of those calls as `tool` items.
Live: Hermes' `todo.updated` (patch 0026) reaches the apps as the bridge's `steps.updated` in the run's stream, the
thread's open screens get a `steps` hint without a new version, a background task's activity has the list."""
import json
import threading
import time

import httpx

from bbkit.common import AUTH, read_sse, sse
from bbkit.servers import wait_until

from test_threads_send import page, send, setup, stored, user, agent

TODOS = [{"id": "1", "content": "Lire les e-mails", "status": "completed"},
         {"id": "2", "content": "Trier par urgence", "status": "in_progress"},
         {"id": "3", "content": "Écrire le résumé", "status": "pending"}]


def call(call_id, name, arguments):
    return {"id": call_id, "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}


def result(call_id, content, name="todo_list"):
    return {"role": "tool", "tool_call_id": call_id, "tool_name": name, "content": content}


def todo_result(todos, revision):
    return json.dumps({"todos": todos, "revision": revision, "summary": {"total": len(todos)}}, ensure_ascii=False)


def test_a_turns_task_list_is_one_card_where_its_last_call_is(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    first = [dict(t, status="pending") for t in TODOS]
    first[0]["status"] = "in_progress"
    done = [dict(t, status="completed") for t in TODOS]
    wrapped = {"calls": [{"name": "todo_list", "arguments": {"todos": done, "merge": True}}]}
    stored(fakes,
           user("Trie mes e-mails"),
           {"role": "assistant", "content": "Je m'en occupe.", "tool_calls": [call("t1", "todo_list", {"todos": first})]},
           result("t1", todo_result(first, 1)),
           {"role": "assistant", "content": "", "tool_calls": [call("m1", "mail_search", {"q": "urgent"})]},
           result("m1", "12 e-mails", name="mail_search"),
           {"role": "assistant", "content": "", "tool_calls": [call("t2", "tool_call", wrapped)]},
           result("t2", todo_result(done, 2)),
           agent("C'est trié : 3 urgents."))
    items = page(bridge, thread)["items"]
    turn = next(i for i in items if i.get("text") == "Trie mes e-mails")
    kinds = [i["kind"] for i in items[items.index(turn):]]
    assert kinds == ["message", "message", "tool", "steps", "message"]
    steps = next(i for i in items if i["kind"] == "steps")
    assert steps["id"] == f"{turn['id']}.steps" and steps["turn"] == turn["id"]
    assert steps["steps"]["done"] == 3 and steps["steps"]["total"] == 3
    assert [s["text"] for s in steps["steps"]["items"]] == ["Lire les e-mails", "Trier par urgence", "Écrire le résumé"]
    assert {s["status"] for s in steps["steps"]["items"]} == {"done"}
    assert not any(i["kind"] == "tool" and i["tool"]["name"] in ("todo_list", "tool_call") for i in items)
    # no day of its own (an older app never loses a day divider to it); the reply after it is its turn's
    assert "day" not in steps
    # one item: the same id read alone
    one = bridge.get(f"/v1/threads/{thread}/items/{steps['id']}", headers=AUTH, params={"agent": "wellness"})
    assert one.status_code == 200 and one.json()["steps"] == steps["steps"]


def test_the_live_list_reaches_the_runs_stream_and_the_open_screens(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("run.started", run_id="r1"),
        sse("tool.started", run_id="r1", tool="todo_list", preview="planning 3 task(s)"),
        sse("tool.completed", run_id="r1", tool="todo_list", duration=0, error=False, preview=""),
        sse("todo.updated", run_id="r1", todos=TODOS, revision=2),
        gate,
        sse("run.completed", run_id="r1", output="Fini.")]
    hints, stop = [], threading.Event()

    def watch():
        with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
            with client.stream("GET", "/v1/threads/events", params={"agent": "wellness", "thread": thread}) as resp:
                for line in resp.iter_lines():
                    if line.startswith("data: ") and thread in line:
                        hints.append(json.loads(line[len("data: "):]))
                    if stop.is_set():
                        return

    other = threading.Thread(target=watch, daemon=True)
    other.start()
    time.sleep(0.5)
    version = page(bridge, thread)["version"]
    assert send(bridge, thread, "C5D00000-0000-4000-8000-0000000000E1", "Trie mes e-mails").json()["state"] == "started"
    seen = []
    with bridge.stream("GET", "/v1/runs/r1/events?agent=wellness", headers=AUTH, timeout=30) as resp:
        for name, data in read_sse(resp):
            seen.append((name, data))
            if name == "steps.updated":
                break
    names = [n for n, _ in seen]
    assert "todo.updated" not in names, "Hermes' own event is not passed on"
    live = seen[-1][1]
    assert live["run_id"] == "r1"
    assert live["steps"]["items"][1] == {"text": "Trier par urgence", "status": "in_progress"}
    assert (live["steps"]["done"], live["steps"]["total"]) == (1, 3)
    # the open screen's hint: the list, the run, the version it already holds (apps from before skip it)
    assert wait_until(lambda: any(h.get("reason") == "steps" for h in hints), timeout=10)
    hint = next(h for h in hints if h.get("reason") == "steps")
    assert hint["run_id"] == "r1" and hint["steps"] == live["steps"] and hint["version"] <= version + 1
    gate.set()
    stop.set()


def test_a_background_tasks_activity_has_its_list(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"),
                               sse("tool.started", run_id="r1", tool="web_search", preview="mutuelle"),
                               sse("todo.updated", run_id="r1", todos=TODOS, revision=2),
                               gate, sse("run.completed", run_id="r1", output="Rapport.")]
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH,
                      json={"title": "Mutuelles", "prompt": "Compare trois mutuelles", "budget_eur": 0.2}).json()

    def activity():
        jobs = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
        found = next((j for j in jobs if j["id"] == job["id"]), {})
        return found.get("activity") or {}

    assert wait_until(lambda: "steps" in activity(), timeout=20)
    got = activity()
    assert got["step"] == "Trier par urgence" and got["steps"]["total"] == 3
    assert got["tool"] == "web_search", "the last tool step is still there"
    gate.set()
