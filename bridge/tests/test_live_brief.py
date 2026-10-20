"""The Live calls' brief (live_brief.py): the last exchanges, the key points the agent keeps up to date in a throwaway
session, the calls that continued a conversation, and its routes."""
import asyncio
import json
import time
from typing import Any, Dict, List

import httpx
import pytest

from bipbridge.hermes import HermesClient
from bipbridge.live_brief import MIN_MESSAGES, POINTS_TITLE, Briefs, is_conversation, visible

from conftest import BRIDGE_KEY, HERMES_KEY


class FakeHermes:
    """Sessions, their messages, and the throwaway notes sessions (created, hidden, one chat, deleted)."""

    def __init__(self) -> None:
        self.sessions: Dict[str, Dict[str, Any]] = {}
        self.messages: Dict[str, List[Dict[str, Any]]] = {}
        self.chats: List[Dict[str, Any]] = []
        self.patched: List[Dict[str, Any]] = []
        self.deleted: List[str] = []
        self.reply = "- Alex prépare un podcast pour jeudi.\n- Il dort mal depuis lundi."

    def add(self, session_id: str, messages: List[Dict[str, Any]], title: str = "Discussion",
            last_active: float = 0.0) -> None:
        self.sessions[session_id] = {"id": session_id, "title": title, "message_count": len(messages),
                                     "last_active": last_active}
        self.messages[session_id] = messages

    def transport(self) -> httpx.MockTransport:
        return httpx.MockTransport(self.handle)

    def handle(self, request: httpx.Request) -> httpx.Response:
        assert request.headers["authorization"] == f"Bearer {HERMES_KEY}"
        parts = request.url.path.strip("/").split("/")
        body = json.loads(request.content) if request.content else {}
        if parts == ["api", "sessions"] and request.method == "GET":
            listed = sorted(self.sessions.values(), key=lambda s: -s["last_active"])
            return httpx.Response(200, json={"sessions": listed})
        if parts == ["api", "sessions"] and request.method == "POST":
            session_id = f"api_scratch_{len(self.chats)}"
            self.add(session_id, [], title=body["title"], last_active=time.time())
            return httpx.Response(201, json={"object": "hermes.session", "session": {"id": session_id}})
        if len(parts) == 3 and request.method == "PATCH":
            self.patched.append({"id": parts[2], **body})
            return httpx.Response(200, json={})
        if len(parts) == 3 and request.method == "DELETE":
            self.deleted.append(parts[2])
            self.sessions.pop(parts[2], None)
            return httpx.Response(200, json={})
        if len(parts) == 4 and parts[3] == "messages":
            return httpx.Response(200, json={"messages": self.messages.get(parts[2], [])})
        if len(parts) == 4 and parts[3] == "chat":
            self.chats.append({"session": parts[2], **body})
            return httpx.Response(200, json={"message": {"role": "assistant", "content": self.reply}})
        return httpx.Response(404)


def talk(n: int, start: int = 0) -> List[Dict[str, Any]]:
    """`n` messages, the person and the agent in turn (with a tool call in between now and then)."""
    out: List[Dict[str, Any]] = []
    for i in range(start, start + n):
        out.append({"role": "user" if i % 2 == 0 else "assistant", "content": f"message {i}"})
        if i % 5 == 4:
            out.append({"role": "tool", "content": "{\"result\": 1}"})
    return out


@pytest.fixture
def hermes():
    return FakeHermes()


@pytest.fixture
def setup(tmp_path, config, hermes):
    client = HermesClient(httpx.AsyncClient(transport=hermes.transport()))
    return Briefs(str(tmp_path / "briefs"), client, config.agents), config.agent("Wellness")


def run(coro):
    return asyncio.run(coro)


def test_a_brief_is_the_last_exchanges_without_tools_or_hidden_text(setup, hermes):
    briefs, agent = setup
    messages = talk(12)
    messages.append({"role": "user", "content": "<!--bip:call\nThe person: hello\n-->\nEt demain ?"})
    hermes.add("s1", messages)
    brief = run(briefs.brief(agent, "s1"))
    assert brief["session"] == "s1" and brief["points_at"] is None
    assert "This is a voice call" in brief["text"]
    assert "- The person: Et demain ?" in brief["text"]
    assert "message 11" in brief["text"] and "message 3" not in brief["text"]   # the last ones only
    assert "result" not in brief["text"] and "bip:" not in brief["text"]


def test_the_key_points_are_written_in_a_throwaway_session_then_kept(setup, hermes):
    briefs, agent = setup
    hermes.add("s1", talk(14))
    assert run(briefs.refresh(agent, "s1", 14))
    chat = hermes.chats[0]
    assert chat["session"].startswith("api_scratch") and chat["model_options"] == {"reasoning": {"enabled": False}}
    assert "message 13" in chat["message"] and "(none yet)" in chat["message"] and "Use no tool" in chat["message"]
    assert hermes.patched == [{"id": chat["session"], "hidden": True}]
    assert hermes.deleted == [chat["session"]]      # nothing left in the agent's conversations
    brief = run(briefs.brief(agent, "s1"))
    assert "Alex prépare un podcast" in brief["text"] and brief["points_at"]
    # Next time: the previous points and only the messages since.
    hermes.add("s1", talk(14) + talk(6, start=14))
    hermes.reply = "- Podcast jeudi, invité confirmé."
    assert run(briefs.refresh(agent, "s1", 20))
    again = hermes.chats[1]["message"]
    assert "Alex prépare un podcast" in again and "message 15" in again and "message 2\n" not in again
    assert "invité confirmé" in run(briefs.brief(agent, "s1"))["text"]


def test_which_conversations_are_due(setup, hermes):
    briefs, agent = setup
    now = time.time()
    long_quiet = {"id": "a", "title": "Discussion", "message_count": 30, "last_active": now - 600}
    assert briefs.due(agent.name, long_quiet, now)
    assert not briefs.due(agent.name, {**long_quiet, "message_count": MIN_MESSAGES - 1}, now)   # short: no points
    assert not briefs.due(agent.name, {**long_quiet, "last_active": now - 5}, now)   # a turn is going on
    for title in ("Live · 10:05", POINTS_TITLE):
        assert not briefs.due(agent.name, {**long_quiet, "title": title}, now)
    assert not briefs.due(agent.name, {**long_quiet, "id": "cron_podcast_20261020_075000"}, now)
    briefs.save(agent.name, "a", {"count": 27})
    assert not briefs.due(agent.name, long_quiet, now)        # 3 new messages only
    briefs.save(agent.name, "a", {"count": 24})
    assert briefs.due(agent.name, long_quiet, now)            # 6 new
    briefs.save(agent.name, "a", {"count": 50})
    assert briefs.due(agent.name, long_quiet, now)            # compressed since: fewer than covered


def test_one_conversation_at_a_time_in_the_background(setup, hermes):
    briefs, agent = setup
    hermes.add("old", talk(20), last_active=time.time() - 900)
    hermes.add("busy", talk(20), last_active=time.time())
    hermes.add("live", talk(20), title="Live · 09:00", last_active=time.time() - 900)
    assert run(briefs.tick())
    assert len(hermes.chats) == 1 and "message 19" in hermes.chats[0]["message"]
    assert briefs.load(agent.name, "old")["count"] == hermes.sessions["old"]["message_count"]
    assert not run(briefs.tick())   # nothing new since
    assert len(hermes.chats) == 1


def test_a_call_continues_in_the_next_briefs(setup, hermes):
    briefs, agent = setup
    hermes.add("s1", talk(4))
    hermes.add("live1", [{"role": "user", "content": "<!--bip:call-start-->"},
                         {"role": "assistant", "content": "Coucou, ça va ?"},
                         {"role": "user", "content": "Oui, je pars à Lyon demain."},
                         {"role": "assistant", "content": "Bon voyage !"}], title="Live · 10:05")
    briefs.note_call(agent.name, "s1", "live1")
    text = run(briefs.brief(agent, "s1"))["text"]
    assert "Earlier voice calls" in text and "je pars à Lyon demain" in text and "bip:" not in text
    briefs.save(agent.name, "s1", {"calls": [{"live_session": "live1", "at": time.time() - 2 * 86400}]})
    assert "Earlier voice calls" not in run(briefs.brief(agent, "s1"))["text"]   # a day old at most


def test_hidden_text_and_conversations():
    assert visible("<!--bip:call\nsecret\n-->\nBonjour") == "Bonjour"
    assert is_conversation({"id": "api_1", "title": "Bot Chat"})
    assert not is_conversation({"id": "api_2", "title": "Discussion", "hidden": True})


def test_routes(client, tmp_path, config, hermes):
    services = client.app.state.services
    services.briefs = Briefs(str(tmp_path / "routes"), HermesClient(httpx.AsyncClient(transport=hermes.transport())),
                             config.agents)
    hermes.add("api_old", talk(4), last_active=1)
    hermes.add("api_new", talk(6), last_active=2)
    hermes.add("Live", talk(2), title="Live · 11:00", last_active=3)
    auth = {"Authorization": f"Bearer {BRIDGE_KEY}"}
    assert client.get("/v1/live/brief", params={"agent": "Wellness"}).status_code == 401
    latest = client.get("/v1/live/brief", params={"agent": "Wellness"}, headers=auth).json()
    assert latest["session"] == "api_new" and "message 5" in latest["text"]   # the latest conversation, not the call
    chosen = client.get("/v1/live/brief", params={"agent": "Wellness", "session": "api_old"}, headers=auth).json()
    assert chosen["session"] == "api_old"
    assert client.get("/v1/live/brief", params={"agent": "Nobody"}, headers=auth).status_code == 404
    done = client.post("/v1/live/calls", json={"agent": "Wellness", "session": "api_old", "live_session": "Live"},
                       headers=auth)
    assert done.json() == {"ok": True}
    assert services.briefs.load(config.agent("Wellness").name, "api_old")["calls"][0]["live_session"] == "Live"
