"""The Live calls' brief (`/v1/live/…`): the last exchanges, the key points the agent keeps up to date in a throwaway
session (the bridge's background loop, 30 s after it starts), the calls that continued a conversation, and the
recaps waiting for the next written message (tests/test_live_brief.py, over HTTP)."""
import json
import time

import pytest

from bbkit.common import AUTH
from bbkit.servers import wait_until


def talk(n, start=0):
    """`n` messages, the person and the agent in turn (with a tool call in between now and then)."""
    out = []
    for i in range(start, start + n):
        out.append({"role": "user" if i % 2 == 0 else "assistant", "content": f"message {i}"})
        if i % 5 == 4:
            out.append({"role": "tool", "content": "{\"result\": 1}"})
    return out


def brief(bridge, **params):
    return bridge.get("/v1/live/brief", params={"agent": "Wellness", **params}, headers=AUTH)


def test_a_brief_is_the_last_exchanges_without_tools_or_hidden_text(bridge, fakes):
    messages = talk(12)
    messages.append({"role": "user", "content": "<!--bip:call\nThe person: hello\n-->\nEt demain ?"})
    messages.append({"role": "assistant", "content": [{"type": "text", "text": "Grand"}, {"type": "text", "text": "soleil."}]})
    messages.append({"role": "user", "content": "invisible", "display_kind": "hidden"})
    fakes.hermes.add_session("s1", messages)
    got = brief(bridge, session="s1").json()
    assert got["session"] == "s1" and got["points_at"] is None
    text = got["text"]
    assert text.startswith("This is a voice call with the person. It continues the conversation summed up below")
    assert "Its last exchanges:\n" in text and "- The person: Et demain ?" in text and "- You: Grand soleil." in text
    assert "message 11" in text and "message 5" not in text   # the last ones only
    assert "result" not in text and "bip:" not in text and "invisible" not in text


def test_routes(bridge, fakes):
    fakes.hermes.add_session("api_old", talk(4), last_active=1)
    fakes.hermes.add_session("api_new", talk(6), last_active=2)
    fakes.hermes.add_session("live_1", talk(2), title="Live · 11:00", last_active=3)
    fakes.hermes.add_session("cron_x_20261021_070000", talk(2), last_active=4)
    assert bridge.get("/v1/live/brief", params={"agent": "Wellness"}).status_code == 401
    latest = brief(bridge).json()
    assert latest["session"] == "api_new" and "message 5" in latest["text"]   # the latest conversation, not the call
    assert brief(bridge, session="api_old").json()["session"] == "api_old"
    assert brief(bridge, agent="Nobody").status_code == 404
    assert bridge.get("/v1/live/brief", headers=AUTH).status_code == 422
    done = bridge.post("/v1/live/calls", json={"agent": "Wellness", "session": "api_old", "live_session": "live_1"},
                       headers=AUTH)
    assert done.json() == {"ok": True}
    kept = json.loads((bridge.state_dir / "briefs" / "wellness" / "api_old.json").read_text())
    assert kept["calls"][0]["live_session"] == "live_1" and abs(kept["calls"][0]["at"] - time.time()) < 60
    assert "Earlier voice calls that continued it:\nVoice call of " in brief(bridge, session="api_old").json()["text"]
    # What the call said waits here for the next written message, whichever device writes it, then is gone.
    first = "<!--bip:call\nVoice call of 10:05 in between:\nThe person: je pars à Lyon\nYou: Bon voyage\n-->"
    second = "<!--bip:call\nVoice call of 18:40 in between:\nThe person: je suis rentré\nYou: Super\n-->"
    for recap in (first, second):
        assert bridge.post("/v1/live/recaps", json={"agent": "Wellness", "session": "api_old", "recap": recap},
                           headers=AUTH).json() == {"ok": True}
    stored = bridge.state_dir / "call-recaps" / "wellness" / "api_old.txt"
    assert stored.read_text() == first + "\x1e" + second and oct(stored.stat().st_mode & 0o777) == "0o600"
    taken = bridge.post("/v1/live/recaps/take", json={"agent": "Wellness", "session": "api_old"}, headers=AUTH).json()
    assert taken == {"recap": first + "\n" + second}
    assert bridge.post("/v1/live/recaps/take", json={"agent": "Wellness", "session": "api_old"},
                       headers=AUTH).json() == {"recap": None}
    assert bridge.post("/v1/live/recaps", json={"agent": "Wellness", "session": "api_old", "recap": "x"}).status_code == 401
    assert bridge.post("/v1/live/recaps", json={"agent": "Wellness", "session": "api_old", "recap": ""},
                       headers=AUTH).status_code == 422
    assert bridge.post("/v1/live/calls", json={"agent": "ghost", "session": "a", "live_session": "b"},
                       headers=AUTH).status_code == 404


def test_no_conversation_yet_and_hermes_errors(bridge, fakes):
    assert brief(bridge).json() == {"session": None, "text": "", "points_at": None}
    fakes.hermes.keys[""] = "other"
    refused = brief(bridge, session="s1")
    assert refused.status_code == 401 and refused.json() == {"detail": "hermes"}


def test_a_call_continues_in_the_next_briefs(bridge, fakes):
    fakes.hermes.add_session("s1", talk(4))
    fakes.hermes.add_session("live1", [{"role": "user", "content": "<!--bip:call-start-->"},
                                       {"role": "assistant", "content": "Coucou, ça va ?"},
                                       {"role": "user", "content": "Oui, je pars à Lyon demain."},
                                       {"role": "assistant", "content": "Bon voyage !"}], title="Live · 10:05")
    bridge.post("/v1/live/calls", json={"agent": "wellness", "session": "s1", "live_session": "live1"}, headers=AUTH)
    text = brief(bridge, session="s1").json()["text"]
    assert "Earlier voice calls" in text and "je pars à Lyon demain" in text and "bip:" not in text
    # A day old at most: the bridge's own file, aged.
    path = bridge.state_dir / "briefs" / "wellness" / "s1.json"
    data = json.loads(path.read_text())
    data["calls"][0]["at"] = time.time() - 2 * 86400
    path.write_text(json.dumps(data))
    assert "Earlier voice calls" not in brief(bridge, session="s1").json()["text"]


@pytest.mark.slow
def test_the_key_points_are_written_in_a_throwaway_session_then_kept(bridge, fakes):
    """The background loop (30 s after the start, one conversation at a time): a hidden scratch session, one chat with
    reasoning off, deleted; the points then lead the brief."""
    hermes = fakes.hermes
    hermes.add_session("s1", talk(14), last_active=time.time() - 900)
    hermes.add_session("busy", talk(20), last_active=time.time())                 # a turn is going on
    hermes.add_session("live", talk(20), title="Live · 09:00", last_active=time.time() - 900)
    assert wait_until(lambda: hermes.chats, timeout=45)
    assert wait_until(lambda: hermes.deleted, timeout=10)
    chat = hermes.chats[0]
    assert len(hermes.chats) == 1 and chat["session"].startswith("api_s")
    assert chat["model_options"] == {"reasoning": {"enabled": False}}
    assert "message 13" in chat["message"] and "(none yet)" in chat["message"] and "Use no tool" in chat["message"]
    assert chat["message"].startswith("<!--bip:notes-->")
    # A unique title, set again by a PATCH (Hermes' « user » provenance: its titler leaves the session alone), hidden.
    (agent, created), = hermes.created
    title = created["title"]
    assert agent == "" and title.startswith("BipAgents · notes · ") and len(title) == len("BipAgents · notes · ") + 8
    assert hermes.patched == [{"id": chat["session"], "title": title, "hidden": True}]
    assert hermes.deleted == [chat["session"]]      # nothing left in the agent's conversations
    assert wait_until(lambda: brief(bridge, session="s1").json()["points_at"])
    got = brief(bridge, session="s1").json()
    assert "Key points of the conversation so far (your own notes):\n- Alex prépare un podcast" in got["text"]
    kept = json.loads((bridge.state_dir / "briefs" / "wellness" / "s1.json").read_text())
    assert kept["upto"] == len(talk(14)) == kept["count"] and kept["points"].startswith("- Alex")   # Hermes' count
    assert bridge.logged("live brief points")[0]["agent"] == "wellness"


@pytest.mark.slow
def test_a_scratch_session_written_back_is_deleted_again(bridge, fakes):
    """Hermes' titler can write the scratch session back after its deletion (an empty row titled from the request):
    the bridge deletes it again a few seconds later, and keeps checking it for a while."""
    hermes = fakes.hermes
    hermes.revive, hermes.revive_after = 1, 1.0
    hermes.add_session("s1", talk(14), last_active=time.time() - 900)
    assert wait_until(lambda: hermes.chats, timeout=45)
    scratch = hermes.chats[0]["session"]
    assert wait_until(lambda: scratch in hermes.sessions.get("", {}), timeout=10)   # back, as on the owner's phone
    assert wait_until(lambda: len(hermes.deleted) == 2, timeout=30)
    assert hermes.deleted == [scratch, scratch] and scratch not in hermes.sessions[""]
    assert "s1" in hermes.sessions[""]
    assert bridge.logged("notes session still there, deleted again")
    kept = json.loads((bridge.state_dir / "briefs" / "scratch.json").read_text())
    assert [s["id"] for s in kept["sessions"]] == [scratch]   # still checked over the next half hour


@pytest.mark.slow
def test_the_empty_rows_written_back_are_swept(bridge, fakes):
    """20 s after the start: only `api_` rows with Hermes' accounting source « unknown », no message (count and
    transcript), not pinned, ten minutes old."""
    hermes = fakes.hermes
    old = time.time() - 3600

    def row(session_id, messages=(), **extra):
        fields = {"title": "Notes on a cable", "last_active": old, "started_at": old, "source": "unknown",
                  "message_count": 0, **extra}
        hermes.add_session(session_id, list(messages), **fields)

    row("api_1761200000_ab12cd34")                                         # the ghost
    row("api_young", last_active=time.time(), started_at=time.time())     # written back a moment ago
    row("api_counted0", talk(2))                                           # a transcript after all
    row("api_talk", talk(4), message_count=4)
    row("api_pinned", pinned=True)
    row("cron_x_20261023_070000")
    row("20261023_101500_ab12cd")                                          # not minted by the API server
    row("api_new", source="api_server", title="Nouvelle")                  # nobody has written in it yet
    assert wait_until(lambda: hermes.deleted, timeout=45)
    time.sleep(1)
    assert hermes.deleted == ["api_1761200000_ab12cd34"]
    assert bridge.logged("empty sessions written back by Hermes deleted")[0]["count"] == 1


def test_a_written_back_row_is_not_the_latest_conversation(bridge, fakes):
    fakes.hermes.add_session("api_talk", talk(4), last_active=1)
    fakes.hermes.add_session("api_ghost", [], title="Notes on a cable", last_active=2, source="unknown")
    assert brief(bridge).json()["session"] == "api_talk"
