"""The Live calls' brief (`/v1/live/…`): the last exchanges, the key points the agent keeps up to date in a throwaway
session (the bridge's background loop, 30 s after it starts), the calls that continued a conversation, and the
recaps waiting for the next written message (tests/test_live_brief.py, over HTTP)."""
import json
import time

import httpx
import pytest

from bbkit.common import AUTH
from bbkit.fakes import split_brief_now, split_now, taken_notes, waiting_notes
from bbkit.servers import wait_until


def talk(n, start=0):
    """`n` messages, the person and the agent in turn (with a tool call in between now and then)."""
    out = []
    for i in range(start, start + n):
        out.append({"role": "user" if i % 2 == 0 else "assistant", "content": f"message {i}"})
        if i % 5 == 4:
            out.append({"role": "tool", "content": "{\"result\": 1}"})
    return out


# 4/11 (Alex asked a pronunciation in a call; the agent started an audio file while he waited): a call's turns and its
# warm-up have the tools that make media or long outputs refused (Hermes patch 0024, `model_options.closed_tools`).
LIVE_CLOSED = ["browser_*", "clarify", "computer_use", "delegate_task", "image_generate",
               "mcp__bipagents__create_audio", "mcp__bipagents__create_page", "mcp__bipagents__create_podcast",
               "mcp__bipagents__heavy_work", "mcp__bipagents__start_task", "mcp__bipagents_files__shared_files_write", "mcp__stealth__*", "patch",
               "text_to_speech", "video_generate", "write_file", "xai_video_edit", "xai_video_extend"]
LIVE_OPTIONS = {"reasoning": {"enabled": False}, "closed_tools": LIVE_CLOSED}
# 12/11: a question card nobody sees in a call (the call screens show none): `clarify` closed, the call's rules (the
# bridge's words, first in the prelude's note) say to ask out loud.
ASK_ALOUD = ("No clarify tool in this call (the person sees no card): to ask something, ask it out loud in one short "
             "sentence and wait for the person's spoken answer. ")
# The quick ones a call keeps.
LIVE_KEPT = ["web_search", "web_extract", "mcp__bipagents__notebook_search", "mcp__bipagents__notebook_write",
             "mcp__bipagents__calendar_list", "mcp__bipagents__calendar_add", "mcp__bipagents__reminders_add",
             "mcp__bipagents__mail_search", "mcp__bipagents__mail_read", "mcp__bipagents__map_image",
             "mcp__bipagents__route", "mcp__bipagents__weather", "mcp__bipagents__message_agent",
             "mcp__bipagents__propose_task", "mcp__bipagents__knowledge_search", "mcp__bipagents__after_call", "memory"]


def closes(closed, name):
    """Whether Hermes 0024 closes `name` with the list `closed` (a trailing `*` closes a prefix)."""
    return any(name.startswith(c[:-1]) if c.endswith("*") else name == c for c in closed)


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
    now, text = split_brief_now(got["text"])
    assert now is not None   # the call's start time, last (bridge-rs/src/dates/now.rs; what changes most, last)
    assert text.startswith("This is a voice call with the person. It continues the conversation summed up below")
    assert "Something to keep (a fact, a date, a size): file it at once with notebook_write" in text.split("\n\n")[0]
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
    stored = bridge.state_dir / "call-recaps" / "wellness" / "api_old.txt"   # kept by the calls' ends (live_calls.rs)
    assert wait_until(stored.exists)   # that call's own, put in its conversation in the background
    assert "bip:call" in taken_notes(bridge, "Wellness", "api_old")
    stored.write_text(first + "\x1e" + second)
    assert waiting_notes(taken_notes(bridge, "Wellness", "api_old")) == first + "\n" + second
    assert waiting_notes(taken_notes(bridge, "Wellness", "api_old")) is None   # the current time alone
    assert bridge.post("/v1/live/calls", json={"agent": "ghost", "session": "a", "live_session": "b"},
                       headers=AUTH).status_code == 404


def test_no_conversation_yet_and_hermes_errors(bridge, fakes):
    # model_options: what the call's turns send (thinking off; « low » for GLM through EUrouter, test_agents_admin)
    got = brief(bridge).json()
    now, rest = split_brief_now(got.pop("text"))
    # the call's Carnet line (filing at once, no search first), its radio rules (spoken.rs) and its start time
    assert now is not None and "Something to keep" in rest and "notebook_write" in rest and "Spoken, for the ear" in rest
    assert got == {"session": None, "points_at": None, "model_options": {"reasoning": {"enabled": False}}}
    fakes.hermes.keys[""] = "other"
    refused = brief(bridge, session="s1")
    assert refused.status_code == 401 and refused.json() == {"detail": "your agent couldn't answer this: try again in a moment"}


def test_a_call_continues_in_the_next_briefs(bridge, fakes):
    fakes.hermes.add_session("s1", talk(4))
    fakes.hermes.add_session("live1", [{"role": "user", "content": "<!--bip:call-start-->"},
                                       {"role": "assistant", "content": "Coucou, ça va ?"},
                                       {"role": "user", "content": "Oui, je pars à Lyon demain."},
                                       {"role": "assistant", "content": "Bon voyage !"}], title="Live · 10:05")
    bridge.post("/v1/live/calls", json={"agent": "wellness", "session": "s1", "live_session": "live1"}, headers=AUTH)
    text = split_brief_now(brief(bridge, session="s1").json()["text"])[1]
    assert "Earlier voice calls" in text and "je pars à Lyon demain" in text and "bip:" not in text
    # A day old at most: the bridge's own file, aged.
    path = bridge.state_dir / "briefs" / "wellness" / "s1.json"
    data = json.loads(path.read_text())
    data["calls"][0]["at"] = time.time() - 2 * 86400
    path.write_text(json.dumps(data))
    assert "Earlier voice calls" not in brief(bridge, session="s1").json()["text"]


def test_a_call_start_has_the_model_read_its_prelude_without_waiting(bridge, fakes):
    """3/11: the call start (apps since then) sends what opens the call's first message; the bridge answers at once and
    has the model read it in a hidden throwaway session (no instructions: the agent's own system prompt; the call's
    reasoning), deleted."""
    hermes = fakes.hermes
    hermes.add_session("s1", talk(4))
    prelude = "<!--bip:live\nRules\n\nBrief of s1\n-->"
    started = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness", "session": "s1", "prelude": prelude})
    assert started.status_code == 200 and started.json() == {"ok": True, "live_session": "api_s1"}   # the call's own
    assert wait_until(lambda: hermes.deleted, timeout=10)
    chat, = hermes.chats
    ruled = "<!--bip:live\n" + ASK_ALOUD + "Rules\n\nBrief of s1\n-->"
    assert chat["message"].startswith(ruled + "\n\n<!--bip:warm") and not chat.get("instructions")
    assert chat["model_options"] == LIVE_OPTIONS   # what the call's turns send: the same tools in the cache
    (_, call), (_, warm) = hermes.created
    assert call["title"].startswith("Live · ") and warm["title"].startswith("BipAgents · live · ")
    assert hermes.deleted == [chat["session"]] and chat["session"] != "api_s1"   # never the call's own session
    # no prelude: no warm-up; no conversation: a call that lands nowhere
    assert bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"}).json()["live_session"] == "api_s3"
    time.sleep(1)
    assert len(hermes.chats) == 1


def test_a_call_s_turns_go_through_the_bridge(bridge, fakes):
    """3/11: the apps no longer call Hermes in a Live call: each turn goes to Hermes' chat/stream in the call's own
    session through `POST /v1/live/turns`, its events passed on as they come (the first words before the turn ends),
    with the agent's quick thinking; photos inline; a turn stopped through the bridge (live_turns.rs)."""
    hermes = fakes.hermes
    hermes.stream_script = [hermes.stream_script[0], 2.0, hermes.stream_script[1]]
    live = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"}).json()["live_session"]
    asked = time.monotonic()
    with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
        with client.stream("POST", "/v1/live/turns", json={"agent": "wellness", "live_session": live,
                                                             "text": "<!--bip:live\nRules\n-->\n\nBonjour"}) as resp:
            assert resp.status_code == 200 and resp.headers["content-type"].startswith("text/event-stream")
            lines = resp.iter_lines()
            first = next(line for line in lines if line.startswith("data: "))
            assert time.monotonic() - asked < 1.5, "the first words wait for the turn's end"
            assert json.loads(first[6:]) == {"event": "assistant.delta", "run_id": "run_l1", "delta": "Salut"}
            assert any("run.completed" in line for line in lines)
    turn, = hermes.streams
    assert turn == {"agent": "", "session": live, "input": "<!--bip:live\n" + ASK_ALOUD + "Rules\n-->\n\nBonjour",
                    "model_options": LIVE_OPTIONS}
    hermes.stream_script = []
    photo = {"kind": "image", "mime": "image/png", "data": "AAAA"}
    sent = bridge.post("/v1/live/turns", headers=AUTH, json={"agent": "wellness", "live_session": live,
                                                            "text": "Et ça ?", "attachments": [photo]})
    assert sent.status_code == 200
    assert hermes.streams[1]["input"] == [{"type": "text", "text": "Et ça ?"},
                                          {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}]
    stopped = bridge.post("/v1/live/turns/stop", headers=AUTH, json={"agent": "wellness", "run_id": "run_l1"})
    assert stopped.json() == {"stopped": True} and hermes.seen("/v1/runs/run_l1/stop", "POST")
    for bad in ({"agent": "wellness", "live_session": "a b", "text": "x"}, {"agent": "wellness", "live_session": live}):
        assert bridge.post("/v1/live/turns", headers=AUTH, json=bad).status_code == 422
    assert bridge.post("/v1/live/turns", headers=AUTH, json={"agent": "ghost", "live_session": live, "text": "x"}).status_code == 404
    assert bridge.post("/v1/live/turns", json={"agent": "wellness", "live_session": live, "text": "x"}).status_code == 401


def test_a_second_call_in_the_same_second_gets_its_own_session(bridge, fakes):
    """3/11 22:25 (Alex, agent Vie): « Ça n'a pas été accepté » right after his first words. Hermes refuses a title
    another session holds (400), and the calls' sessions stay: a call starting in the same second (in the apps up to
    3/11, the same minute) as an earlier one was refused, then its first turn. Now it gets a unique title."""
    hermes = fakes.hermes
    hermes.unique_titles = True
    now = time.time()
    for k in range(4):   # the earlier calls' sessions, whatever second this call starts in
        stamp = time.strftime("%Y-%m-%d %H:%M:%S", time.localtime(now + k))
        hermes.add_session(f"live_{k}", talk(2), title=f"Live · {stamp}")
    started = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"})
    assert started.status_code == 200, started.text
    live = started.json()["live_session"]
    titles = [body["title"] for _, body in hermes.created]
    assert titles[-1].startswith("Live · ") and titles.count(titles[-1]) == 1
    again = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"})
    assert again.status_code == 200 and again.json()["live_session"] != live
    sent = bridge.post("/v1/live/turns", headers=AUTH, json={"agent": "wellness", "live_session": live, "text": "Hello"})
    assert sent.status_code == 200 and hermes.streams[-1]["input"] == "Hello"


def test_a_turn_to_file_is_told_the_carnet_s_tool_by_the_bridge(bridge, fakes):
    """10/11: the phones' voice decider (9/11) sends a turn to file, not answer, with a hidden note naming a Carnet v1
    tool (`notebook_note`, gone): the bridge puts its own words in the note, whatever the app wrote (live_turns.rs)."""
    hermes = fakes.hermes
    live = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"}).json()["live_session"]
    old = ("<!--bip:note\nYour person said this to have it filed, not answered: keep it in the Carnet (notebook_note), "
           "then answer only « OK, noté » in your language, nothing more.\n-->\npour info le plombier vient jeudi")
    assert bridge.post("/v1/live/turns", headers=AUTH, json={"agent": "wellness", "live_session": live,
                                                              "text": old}).status_code == 200
    said = hermes.streams[-1]["input"]
    assert said.startswith("<!--bip:note\n") and said.endswith("-->\npour info le plombier vient jeudi")
    assert "notebook_write" in said and "notebook_note" not in said and said.count("-->") == 1


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
    # logged once Hermes answered the delete (the fake counts it when asked): waited for, not read at once
    assert wait_until(lambda: bridge.logged("notes session still there, deleted again"), timeout=10)
    def kept():
        return [s["id"] for s in json.loads((bridge.state_dir / "briefs" / "scratch.json").read_text())["sessions"]]
    assert wait_until(lambda: kept() == [scratch], timeout=10), kept()   # still checked over the next half hour


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


def test_a_call_s_turns_close_the_media_and_long_tools_and_keep_the_quick_ones(bridge, fakes):
    """4/11: in a call the agent speaks (a pronunciation said, not an audio file made): the call's turns ask Hermes to
    refuse audio, speech, images, pages, files, the browsers and long work, never the quick tools."""
    hermes = fakes.hermes
    live = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"}).json()["live_session"]
    sent = bridge.post("/v1/live/turns", headers=AUTH, json={"agent": "wellness", "live_session": live,
                                                            "text": "Comment on prononce « Hermès » ?"})
    assert sent.status_code == 200
    closed = hermes.streams[-1]["model_options"]["closed_tools"]
    for media in ["mcp__bipagents__create_audio", "mcp__bipagents__create_podcast", "text_to_speech", "image_generate",
                  "mcp__bipagents__create_page", "write_file", "mcp__stealth__navigate", "browser_navigate",
                  "delegate_task", "mcp__bipagents__start_task", "clarify"]:
        assert closes(closed, media), media
    for quick in LIVE_KEPT:
        assert not closes(closed, quick), quick


def test_a_written_turn_keeps_every_tool(bridges, fakes):
    """A message of a conversation is the person's usual turn: nothing closed."""
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat", last_active=10,
                             started_at=1)
    bridge = bridges.new().start()
    thread = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "wellness"}).json()["id"]
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C1D00000-0000-4000-8000-0000000000aa",
                             "text": "Fais-moi un podcast sur la voile"})
    assert sent.status_code == 200, sent.text
    run, = fakes.hermes.started.values()
    assert "closed_tools" not in (run["model_options"] or {})


def test_what_the_agent_noted_in_a_call_is_made_once_it_ends(bridge, fakes):
    """4/11: in a call the agent notes what to make (`after_call`, live_calls.rs) instead of making it while the person
    waits; once the call is in its conversation, ONE background turn there asks it to make what was noted (its reply and
    files delivered as a message). A call that noted nothing: no turn."""
    from bbkit.mcp import call, give_token, text_of
    hermes = fakes.hermes
    hermes.rounds = 1
    hermes.run_output = "Voici l'audio. MEDIA:/bipagents/audio/hermes.mp3"
    give_token(bridge)
    hermes.add_session("s1", talk(4))
    # no call under way: said so, nothing noted
    refused = call(bridge, "after_call", {"what": "un audio"})
    assert refused["isError"] and "no voice call" in text_of(refused)
    live = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness", "session": "s1"}).json()["live_session"]
    noted = call(bridge, "after_call", {"what": "Un audio de 1 min où je prononce « Hermès » lentement, en français"})
    assert not noted["isError"] and "as soon as the call ends" in text_of(noted)
    assert call(bridge, "after_call", {"what": " "})["isError"]
    hermes.messages[("", live)] = [{"role": "user", "content": "Comment on prononce Hermès ?"},
                                   {"role": "assistant", "content": "Er-mès. Je te prépare un audio après l'appel."}]
    ended = bridge.post("/v1/live/calls", headers=AUTH, json={"agent": "wellness", "session": "s1", "live_session": live})
    assert ended.status_code == 200

    def after_turns():
        return [r for r in hermes.started.values() if "bip:after-call" in (r["input"] or "")]

    assert wait_until(lambda: after_turns(), timeout=20)
    turn, = after_turns()
    assert turn["session_id"] == "s1"   # in the call's conversation, not the call's own session
    assert "prononce « Hermès » lentement" in turn["input"] and "Comment on prononce Hermès ?" in turn["input"]
    assert not turn.get("instructions")   # the rules in the message: the prompt cache stays the conversation's
    # a second call that noted nothing: no turn after it
    other = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness", "session": "s1"}).json()["live_session"]
    hermes.messages[("", other)] = [{"role": "user", "content": "Merci !"}, {"role": "assistant", "content": "Avec plaisir."}]
    bridge.post("/v1/live/calls", headers=AUTH, json={"agent": "wellness", "session": "s1", "live_session": other})
    calls_file = bridge.state_dir / "live-calls" / "wellness" / "s1.json"
    assert wait_until(lambda: calls_file.exists() and other in calls_file.read_text(), timeout=20)
    time.sleep(3)
    assert len(after_turns()) == 1
