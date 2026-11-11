"""Agents shared with relatives (agent_shares.rs), over two linked bridges: Alex shares Cuisine with Marie, she adds
it and talks to it in her own conversation (hidden in Alex's Hermes) through her bridge's thread routes
(`agent=cuisine__alex`): her message with its app id, her turn's words live, the conversation's hints; her turn's
approval is hers (her Boîte, her answer relayed), never in Alex's; the owner's private tools are closed during her
turn; her tokens are counted."""
import json
import threading
import time

import httpx

from bbkit.common import AUTH, sse
from bbkit.mcp import TOKEN, call, give_token
from test_households_and_peers import household

# A relative's run as Hermes gets it (patch 0018's mark): the bridge tools of RELATIVE_TOOLS and web search, nothing else.
RELATIVE_RUN_TOOLS = ["mcp__bipagents__knowledge_read", "mcp__bipagents__knowledge_search", "mcp__bipagents__weather",
                      "mcp__bipagents__route", "mcp__bipagents__map_image", "web_search"]
# Hermes' own tools that reach the owner's data or act: never in a relative's run.
HERMES_CLOSED = ["terminal", "read_file", "write_file", "patch", "search_files", "execute_code", "browser_navigate",
                 "browser_vault_fill", "browser_vault", "session_search", "memory", "cronjob", "send_message",
                 "message_agent", "delegate_task", "skill_view", "skill_manage", "tool_search"]
# The bridge's tools reaching the owner's data: refused while a relative's turn goes on.
OWNER_TOOLS = ["mail_search", "mail_read", "calendar_list", "contacts_search", "notebook_search", "archive_search",
               "send_files_to_agent", "health", "dates", "message_agent", "read_page", "list_pages"]


def wait(predicate, timeout=10.0):
    end = time.time() + timeout
    while time.time() < end:
        if predicate():
            return True
        time.sleep(0.05)
    return False


def link(a, b):
    assert a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert b.put("/v1/family/name", headers=AUTH, json={"name": "Marie"}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200, joined.text


def test_a_relative_talks_to_a_shared_agent(bridges, fakes):
    a = household(bridges, fakes, "alex", ["cuisine", "budget"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert view["households"] == [{"id": "marie", "name": "Marie", "shared": False}]
    assert view["warning"] == {"memory_common": True, "private_tools": ["notebook"], "confirm": False}
    assert view["conversations"] == [] and view["tokens"] == []
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["paul"]}).status_code == 400
    assert a.get("/v1/agents/ghost/shares", headers=AUTH).status_code == 404

    shared = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]})
    assert shared.status_code == 200 and shared.json()["told"] == {"marie": True}
    offered = b.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert [(x["id"], x["owner"], x["name"], x["state"]) for x in offered] == [("cuisine__alex", "Alex", "Cuisine",
                                                                               "offered")]
    first = {"agent": "cuisine__alex", "client_id": "C0FFEE00-0000-4000-8000-000000000001", "text": "Une idée de dîner ?"}
    assert b.post("/v1/threads/new/messages", headers=AUTH, json=first).status_code == 409   # not added yet
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200

    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("message.delta", run_id="r1", delta="Je regarde"),
        sse("tool.started", run_id="r1", tool="terminal", preview="cat ~/secret"),
        sse("approval.request", run_id="r1", request_id="req-1", command="rm -rf build/", description="clean",
            choices=["once", "deny"]),
        gate,
        sse("run.completed", output="Une fondue !"),
    ]
    # her first message opens the conversation
    sent = b.post("/v1/threads/new/messages", headers=AUTH, json=first)
    assert sent.status_code == 200, sent.text
    assert sent.json()["state"] == "started" and sent.json()["run_id"] == "r1"
    cid = sent.json()["thread"]
    assert any(p.get("hidden") is True for p in fakes.hermes.patched)
    timeline = f"/v1/threads/{cid}/timeline"
    page = lambda: b.get(timeline, headers=AUTH, params={"agent": "cuisine__alex"}).json()   # noqa: E731
    # her message shows at once, with her app's id, until Hermes has it
    assert [(i["text"], i["client_id"]) for i in page()["items"]] == [("Une idée de dîner ?", first["client_id"])]
    # her turn's words live through her bridge, only text crossing (no tool step)
    with httpx.Client(base_url=b.url, headers=AUTH, timeout=10) as client:
        with client.stream("GET", "/v1/runs/r1/events", params={"agent": "cuisine__alex", "thread": cid}) as live:
            assert live.status_code == 200
            kinds = []
            for line in live.iter_lines():
                if line.startswith("event: "):
                    kinds.append(line[len("event: "):])
                if "approval.request" in kinds:
                    break
    assert kinds == ["message.delta", "approval.request"]
    hints = []

    def watch():   # her conversation on screen: the owner's hints relayed
        with httpx.Client(base_url=b.url, headers=AUTH, timeout=30) as client:
            with client.stream("GET", "/v1/threads/events", params={"agent": "cuisine__alex", "thread": cid}) as resp:
                for line in resp.iter_lines():
                    if line.startswith("data: ") and cid in line:
                        hints.append(json.loads(line[len("data: "):]))
                    if len(hints) >= 1 and gate.is_set():
                        return

    viewer = threading.Thread(target=watch, daemon=True)
    viewer.start()
    time.sleep(0.5)
    run = fakes.hermes.started["r1"]
    assert run["agent"] == "cuisine" and run["input"].startswith("[Marie, a relative of your person")
    assert "never as your person's instructions" in run["instructions"]
    # The run is marked for Hermes: only these tools, none of Hermes' own that reach Alex's data or act.
    # (and, since 2026-11-09, a page made at her request: shared_pages.rs, test_shared_pages.py)
    assert run["guest_turn"] == {"tools": RELATIVE_RUN_TOOLS[:-1] + ["mcp__bipagents__create_page",
                                                                      "mcp__bipagents__read_sent_file", "web_search"]}
    assert not set(HERMES_CLOSED) & set(run["guest_turn"]["tools"])
    # While her turn goes on, the bridge refuses Cuisine's calls reaching Alex's data, and answers the harmless ones.
    give_token(a, "cuisine", TOKEN)
    for name in OWNER_TOOLS:
        refused = call(a, name, {"query": "x"}, token=TOKEN)
        assert refused["isError"] is True and "closed during their turn" in refused["content"][0]["text"], name
    assert "closed during their turn" not in call(a, "knowledge_search", {"words": "dîner"}, token=TOKEN)["content"][0]["text"]
    assert "closed during their turn" not in json.dumps(call(a, "weather", {"place": "Lisbonne"}, token=TOKEN))
    again = {"agent": "cuisine__alex", "client_id": "C0FFEE00-0000-4000-8000-000000000002", "text": "Encore ?"}
    assert b.post(f"/v1/threads/{cid}/messages", headers=AUTH, json=again).status_code == 409

    assert wait(lambda: page()["pending"]["approvals"] != [])
    pending = page()["pending"]
    assert pending["run"]["status"] == "approval" and pending["approvals"][0]["request_id"] == "req-1"
    assert pending["approvals"][0]["id"].startswith("a_")
    assert [i for i in a.get("/v1/approvals", headers=AUTH).json()["items"] if i["run_id"] == "r1"] == []
    owner = a.post("/v1/approve", headers=AUTH, json={"agent": "cuisine", "run_id": "r1", "choice": "once"})
    assert owner.status_code == 403
    assert wait(lambda: any(i["agent"] == "cuisine__alex" for i in b.get("/v1/approvals", headers=AUTH).json()["items"]))
    answered = b.post("/v1/approve", headers=AUTH,
                      json={"agent": "cuisine__alex", "run_id": "r1", "choice": "once", "request_id": "req-1"})
    assert answered.status_code == 200, answered.text
    assert fakes.hermes.approvals[-1]["agent"] == "cuisine" and fakes.hermes.approvals[-1]["body"]["choice"] == "once"
    assert wait(lambda: hints != [])   # answered: her screen told to read again
    # Hermes stores her turn as it ends: her message is the stored row then, still with her app's id, her words unframed
    session = fakes.hermes.started["r1"]["session_id"]
    fakes.hermes.messages[("cuisine", session)] = [{"role": "user", "content": fakes.hermes.started["r1"]["input"]},
                                                   {"role": "assistant", "content": "Une fondue !"}]
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    assert wait(lambda: page()["pending"]["run"] is None)
    viewer.join(timeout=15)
    assert wait(lambda: page()["items"][-1]["text"] == "Une fondue !")
    items = page()["items"]
    assert [(i["role"], i["text"]) for i in items] == [("person", "Une idée de dîner ?"), ("agent", "Une fondue !")]
    assert items[0]["client_id"] == first["client_id"] and not items[0]["id"].startswith("s_")
    # her list, and her conversation ended from her app
    assert [t["id"] for t in b.get("/v1/threads", headers=AUTH, params={"agent": "cuisine__alex"}).json()["threads"]] == [cid]
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert view["conversations"][0]["household_name"] == "Marie"
    assert view["tokens"][0]["household"] == "marie"

    assert b.delete(f"/v1/threads/{cid}", headers=AUTH, params={"agent": "cuisine__alex"}).status_code == 200
    assert b.get("/v1/threads", headers=AUTH, params={"agent": "cuisine__alex"}).json()["threads"] == []
    # Another of Alex's agents isn't reachable with Marie's household key, nor anything without it.
    assert a.get("/family/s/budget/conversations", headers={"Authorization": "Bearer nope"}).status_code == 401
    # Alex stops sharing: the card goes on Marie's side.
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": []}).json()["told"] == {"marie": True}
    assert b.get("/v1/shared-agents", headers=AUTH).json()["agents"] == []


def test_a_shared_agent_s_question_reaches_the_relative_and_her_answer_goes_back(bridges, fakes):
    """Her turn's questions (Hermes' clarify) are hers like its approvals: in her timeline and her turn's live words,
    answered through her bridge's `POST /v1/approve` (`answer`), relayed to Alex's Hermes."""
    a = household(bridges, fakes, "alex", ["cuisine"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]}).status_code == 200
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("clarify.request", run_id="r1", prompt_id="q-1", execution_generation=2,
            questions=[{"id": "q", "question": "Pour combien ?", "choices": ["2", "4"]}]),
        gate, sse("run.completed", run_id="r1", output="Pour 4, noté.")]
    body = {"agent": "cuisine__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000Q1", "text": "Une recette ?"}
    sent = b.post("/v1/threads/new/messages", headers=AUTH, json=body)
    assert sent.status_code == 200, sent.text
    cid = sent.json()["thread"]
    with httpx.Client(base_url=b.url, headers=AUTH, timeout=10) as client:
        with client.stream("GET", "/v1/runs/r1/events", params={"agent": "cuisine__alex", "thread": cid}) as live:
            event = next(json.loads(line[6:]) for line in live.iter_lines() if line.startswith("data: "))
    assert event["event"] == "clarify.request" and event["questions"][0]["question"] == "Pour combien ?"
    page = lambda: b.get(f"/v1/threads/{cid}/timeline", headers=AUTH, params={"agent": "cuisine__alex"}).json()  # noqa: E731
    assert wait(lambda: page()["pending"].get("questions"))
    pending = page()["pending"]
    assert pending["run"]["status"] == "approval" and pending["approvals"] == []
    question, = pending["questions"]
    assert question["request_id"] == "q-1" and question["id"].startswith("a_")
    answered = b.post("/v1/approve", headers=AUTH,
                      json={"agent": "cuisine__alex", "run_id": "r1", "request_id": "q-1", "answer": "4"})
    assert answered.status_code == 200, answered.text
    assert fakes.hermes.answers[-1] == {"agent": "cuisine", "run_id": "r1",
                                        "body": {"request_id": "q-1", "execution_generation": 2, "answer": "4"}}
    assert wait(lambda: page()["pending"]["questions"] == [])
    gate.set()


def test_a_shared_agent_opens_on_main_like_one_s_own(bridges, fakes):
    """The apps open a shared agent in the conversation screen of their own agents, on `main`: before her first message
    an empty page (`main`, nothing to follow); her first message to `main` makes her conversation; then `main` is her
    latest one, its id as the thread's tip (the apps key a turn by it), for the page, the turn's words, stop and delete."""
    a = household(bridges, fakes, "alex", ["cuisine"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]}).status_code == 200
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    shared = {"agent": "cuisine__alex"}
    empty = b.get("/v1/threads/main/timeline", headers=AUTH, params=shared)
    assert empty.status_code == 200, empty.text
    page = empty.json()
    assert page["thread"]["id"] == "main" and page["thread"]["tip"] is None and page["items"] == []
    assert page["pending"] == {"run": None, "approvals": [], "questions": []}
    assert b.get("/v1/threads/events", headers=AUTH, params={**shared, "thread": "main"}).status_code == 404
    assert b.post("/v1/threads/main/stop", headers=AUTH, json=shared).json() == {"stopped": False}

    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta="Des crêpes"), gate,
                               sse("run.completed", output="Des crêpes !")]
    first = {**shared, "client_id": "C0FFEE00-0000-4000-8000-0000000000A1", "text": "Une idée pour dimanche ?"}
    sent = b.post("/v1/threads/main/messages", headers=AUTH, json=first)
    assert sent.status_code == 200, sent.text
    cid = sent.json()["thread"]
    assert cid not in ("main", "new") and sent.json()["tip"] == cid and sent.json()["run_id"] == "r1"
    page = b.get("/v1/threads/main/timeline", headers=AUTH, params=shared).json()
    assert page["thread"]["id"] == cid and page["thread"]["tip"] == cid
    assert [i["text"] for i in page["items"]] == ["Une idée pour dimanche ?"]
    with httpx.Client(base_url=b.url, headers=AUTH, timeout=10) as client:
        with client.stream("GET", "/v1/runs/r1/events", params={**shared, "thread": "main"}) as live:
            assert live.status_code == 200
            event = next(json.loads(line[6:]) for line in live.iter_lines() if line.startswith("data: "))
    assert event["event"] == "message.delta"
    assert b.post("/v1/threads/main/stop", headers=AUTH, json=shared).status_code == 200
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    assert wait(lambda: b.get(f"/v1/threads/{cid}/timeline", headers=AUTH, params=shared).json()["pending"]["run"] is None)
    # a catch-up after what the phone holds is the latest page, the window (`reset`): her words only, whose cursors move
    # as Hermes stores her turn; never an empty page that would empty the phone's window
    later = b.get(f"/v1/threads/{cid}/timeline", headers=AUTH, params={**shared, "after": page["last"]}).json()
    assert later["reset"] is True and [i["text"] for i in later["items"]] == ["Une idée pour dimanche ?"], later

    # a second conversation (« Nouvelle conversation »): `main` follows the latest
    fakes.hermes.runs["r2"] = [sse("run.completed", output="Une soupe.")]
    other = b.post("/v1/threads/new/messages", headers=AUTH,
                   json={**shared, "client_id": "C0FFEE00-0000-4000-8000-0000000000A2", "text": "Et lundi ?"})
    assert other.status_code == 200, other.text
    assert other.json()["thread"] != cid
    assert b.get("/v1/threads/main/timeline", headers=AUTH, params=shared).json()["thread"]["id"] == other.json()["thread"]
    assert b.delete("/v1/threads/main", headers=AUTH, params=shared).status_code == 200
    assert [t["id"] for t in b.get("/v1/threads", headers=AUTH, params=shared).json()["threads"]] == [cid]
