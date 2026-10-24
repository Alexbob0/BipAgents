"""The agents' MCP tools (`/v1/agent-tools/mcp`, each agent with its own token): the protocol, create_audio,
create_podcast and list_voices, the files served as MEDIA (tests/test_agent_tools.py, over HTTP; the fake ffmpeg
copies the WAV it is given)."""
import json
import re
import struct

from bbkit.common import AUTH
from bbkit.mcp import TOKEN, call, give_token, media_of, rpc

AI_SOURCE = "http://cv.iptc.org/newscodes/digitalsourcetype/trainedAlgorithmicMedia"


def wav_info(data):
    """(sample rate, channels, PCM bytes, {INFO key: text}) of a RIFF WAV."""
    assert data[:4] == b"RIFF" and data[8:12] == b"WAVE" and struct.unpack("<I", data[4:8])[0] == len(data) - 8
    pos, rate, channels, pcm, info = 12, 0, 0, b"", {}
    while pos + 8 <= len(data):
        tag, size = data[pos:pos + 4], struct.unpack("<I", data[pos + 4:pos + 8])[0]
        body = data[pos + 8:pos + 8 + size]
        if tag == b"fmt ":
            _, channels, rate = struct.unpack("<HHI", body[:8])
        elif tag == b"data":
            pcm = body
        elif tag == b"LIST" and body[:4] == b"INFO":
            sub = 4
            while sub + 8 <= len(body):
                key, length = body[sub:sub + 4].decode(), struct.unpack("<I", body[sub + 4:sub + 8])[0]
                info[key] = body[sub + 8:sub + 8 + length].rstrip(b"\x00").decode()
                sub += 8 + length + (length % 2)
        pos += 8 + size + (size % 2)
    return rate, channels, pcm, info


def test_the_protocol_and_the_tokens(bridge):
    give_token(bridge)
    wrong = rpc(bridge, "initialize", token="wrong")
    assert wrong.status_code == 401 and wrong.json() == {"detail": "unknown agent"}
    assert bridge.post("/v1/agent-tools/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "ping"}).status_code == 401
    init = rpc(bridge, "initialize", {"protocolVersion": "2025-06-18"}).json()
    assert init["jsonrpc"] == "2.0" and init["id"] == 1
    result = init["result"]
    assert result["protocolVersion"] == "2025-06-18" and result["capabilities"] == {"tools": {"listChanged": False}}
    assert result["serverInfo"]["name"] == "bipagents" and result["instructions"].startswith("BipAgents: create_audio")
    assert rpc(bridge, "initialize").json()["result"]["protocolVersion"] == "2025-06-18"   # the default
    notified = rpc(bridge, "notifications/initialized", request_id=None)
    assert notified.status_code == 202 and notified.content == b""
    assert rpc(bridge, "ping", request_id="abc").json() == {"jsonrpc": "2.0", "id": "abc", "result": {}}
    tools = rpc(bridge, "tools/list").json()["result"]["tools"]
    assert [t["name"] for t in tools] == ["create_audio", "create_podcast", "list_voices", "create_page", "list_pages",
                                          "read_page", "delete_page", "mail_search", "mail_read", "mail_draft",
                                          "knowledge_read", "knowledge_search", "reminders_list", "reminders_add",
                                          "reminders_complete", "propose_task", "start_task", "route", "map_image", "upcoming_dates",
                                          # the Carnet's (on by default)
                                          "notebook_search", "notebook_read", "notebook_note", "notebook_edit",
                                          "notebook_task", "notebook_record", "notebook_fill"]
    assert all(set(t) >= {"name", "description", "inputSchema"} for t in tools)
    missing = rpc(bridge, "resources/list").json()
    assert missing == {"jsonrpc": "2.0", "id": 1, "error": {"code": -32601, "message": "method not found: resources/list"}}
    unknown = call(bridge, "fly", {})
    assert unknown == {"content": [{"type": "text", "text": "unknown tool: fly"}], "isError": True}
    parse = bridge.post("/v1/agent-tools/mcp", content=b"{nope", headers={"Authorization": f"Bearer {TOKEN}"})
    assert parse.status_code == 400 and parse.json() == {"jsonrpc": "2.0", "id": None,
                                                          "error": {"code": -32700, "message": "parse error"}}
    invalid = bridge.post("/v1/agent-tools/mcp", json={"method": "ping", "id": 1}, headers={"Authorization": f"Bearer {TOKEN}"})
    assert invalid.status_code == 400 and invalid.json()["error"]["code"] == -32600
    get = bridge.get("/v1/agent-tools/mcp", headers={"Authorization": f"Bearer {TOKEN}"})
    assert get.status_code == 405 and get.headers["allow"] == "POST"
    assert bridge.delete("/v1/agent-tools/mcp", headers={"Authorization": f"Bearer {TOKEN}"}).status_code == 200
    assert bridge.get("/v1/agent-tools/mcp").status_code == 401
    # The query string Hermes is given (`?v=` the tools' version) changes nothing.
    assert bridge.post("/v1/agent-tools/mcp?v=0123456789", json={"jsonrpc": "2.0", "id": 2, "method": "ping"},
                       headers={"Authorization": f"Bearer {TOKEN}"}).json()["result"] == {}


def test_an_agent_turns_a_text_into_audio_with_its_voice(bridge, fakes):
    give_token(bridge)
    text = "Bonjour et bienvenue dans ce podcast.\n\nVoici les nouvelles du jour."
    result = call(bridge, "create_audio", {"text": text, "title": "Podcast du matin"})
    assert result["isError"] is False
    assert re.fullmatch(r"Audio ready \(\d+ s\)\.\nTo give it to the person, put this line alone at the end of your "
                        r"reply:\nMEDIA:/bipagents/audio/wellness/\d{8}-\d{6}-podcast-du-matin\.m4a",
                        result["content"][0]["text"]), result
    media = media_of(result)
    # Read with the agent's reading voice, one paragraph at a time, in the background queue.
    assert [body["voice"] for body in fakes.kyutai.inputs] == ["4193", "4193"]
    assert [body["input"] for body in fakes.kyutai.inputs] == ["Bonjour et bienvenue dans ce podcast.",
                                                              "Voici les nouvelles du jour."]
    assert all(body["response_format"] == "wav" for body in fakes.kyutai.inputs)
    # Encoded by ffmpeg (here the fake, which copies its input): its arguments, and the AI mark in the file.
    mark = f"AI-generated: synthetic voice by the AI agent Wellness (BipAgents). digitalsourcetype={AI_SOURCE}"
    ffmpeg = bridge.commands.of("ffmpeg")[0]
    source = ffmpeg[ffmpeg.index("-i") + 1]
    assert ffmpeg == ["-nostdin", "-y", "-loglevel", "error", "-i", source, "-c:a", "aac", "-b:a", "64k",
                      "-metadata", f"comment={mark}", "-metadata", f"description={mark}", "-metadata",
                      "encoder=BipAgents", source[:-4] + ".m4a"]
    folder = bridge.state_dir / "agent-audio" / "wellness"
    assert [p.name for p in folder.iterdir()] == [media.rsplit("/", 1)[1]]          # the WAV removed
    # The app gets the file like any MEDIA: file.
    audio = bridge.get("/v1/media", params={"path": media}, headers=AUTH)
    assert audio.status_code == 200 and audio.headers["content-type"] == "audio/mp4"
    rate, channels, pcm, info = wav_info(audio.content)
    assert (rate, channels) == (24000, 1)
    pause = b"\x00\x00" * (24000 * 500 // 1000)                                    # a paragraph's pause
    first, second = (b"".join(struct.pack("<h", ord(c) % 3000) for c in t)
                     for t in ("Bonjour et bienvenue dans ce podcast.", "Voici les nouvelles du jour."))
    assert pcm == first + pause + second
    assert info == {"ICMT": mark, "ISFT": "BipAgents"}
    assert bridge.get("/v1/media", params={"path": "/bipagents/audio/../bridge.db"}, headers=AUTH).status_code == 404
    logged = bridge.logged("agent audio made")[0]
    assert logged["agent"] == "wellness" and logged["chars"] == len(text)


def test_without_ffmpeg_the_wav_is_kept(bridges, fakes):
    bridge = bridges.new()
    bridge.commands.rule("ffmpeg", [], exit=1, stderr="no encoder")
    bridge.start()
    give_token(bridge)
    result = call(bridge, "create_audio", {"text": "Une phrase.", "title": "Été ! 2026"})
    media = media_of(result)
    assert re.fullmatch(r"/bipagents/audio/wellness/\d{8}-\d{6}-ete-2026\.wav", media)
    audio = bridge.get("/v1/media", params={"path": media}, headers=AUTH)
    assert audio.headers["content-type"] == "audio/wav" and wav_info(audio.content)[3]["ISFT"] == "BipAgents"
    assert bridge.logged("audio not encoded, kept as WAV")


def test_a_long_call_is_streamed_and_errors_are_the_tools_not_the_protocols(bridge, fakes):
    give_token(bridge)
    streamed = rpc(bridge, "tools/call", {"name": "create_audio", "arguments": {"text": "Une phrase."}},
                   accept="application/json, text/event-stream")
    assert streamed.headers["content-type"].startswith("text/event-stream")
    assert streamed.headers["cache-control"] == "no-cache"
    event = [line for line in streamed.text.splitlines() if line.startswith("data: ")][-1]
    assert "event: message" in streamed.text
    assert json.loads(event[len("data: "):])["result"]["isError"] is False
    too_long = call(bridge, "create_audio", {"text": "x" * 20001})
    assert too_long["isError"] is True and "too long" in too_long["content"][0]["text"]
    assert too_long["content"][0]["text"] == "the text is too long (20001 characters, at most 20000): split it"
    empty = call(bridge, "create_audio", {"text": "  "})
    assert empty == {"content": [{"type": "text", "text": "the text is empty"}], "isError": True}
    only_media = call(bridge, "create_audio", {"text": "MEDIA:/tmp/x.mp3"})
    assert only_media["content"][0]["text"] == "nothing to read once MEDIA: lines are removed"
    fakes.kyutai.status = 500
    failed = call(bridge, "create_audio", {"text": "Autre phrase."})
    assert failed["isError"] is True and failed["content"][0]["text"].startswith("the voice failed (")


def test_a_podcast_gives_each_host_a_voice_of_their_own(bridge, fakes):
    give_token(bridge)
    listed = call(bridge, "list_voices", {})
    assert listed["content"][0]["text"].splitlines() == ["4193 (your voice)", "pocket:colibri (Bip)", "pocket:galet (Bip)",
                                                         "pocket:lumen (Bip)", "pocket:mousse (Bip)", "pocket:ours (Bip)"]
    turns = [{"speaker": "Max", "text": "Bonjour à tous, bienvenue."},
             {"speaker": "Léa", "text": "Salut Max ! Alors, quoi de neuf ?"},
             {"speaker": "Max", "text": "Trois nouvelles aujourd'hui."}]
    result = call(bridge, "create_podcast", {"title": "Le point", "turns": turns})
    assert result["isError"] is False
    text = result["content"][0]["text"]
    assert "Voices:\n- Max: 4193\n- Léa: pocket:colibri\n" in text and "-le-point.m4a" in text
    # The first host with the agent's voice, the other with another one; turns read in order.
    assert [body["voice"] for body in fakes.kyutai.inputs] == ["4193", "4193"]
    assert [body["voice"] for body in fakes.pocket.inputs] == ["colibri"]
    # A voice picked by the agent, from list_voices; an unknown one is refused.
    picked = call(bridge, "create_podcast", {"turns": turns, "voices": {"Léa": "pocket:galet"}})
    assert picked["isError"] is False and fakes.pocket.inputs[-1]["voice"] == "galet"
    assert "-podcast.m4a" in picked["content"][0]["text"]
    bad = call(bridge, "create_podcast", {"turns": turns, "voices": {"Léa": "pocket:dragon"}})
    assert bad["isError"] is True and "unknown voice" in bad["content"][0]["text"]
    empty = call(bridge, "create_podcast", {"turns": [{"speaker": "Max"}]})
    assert empty["isError"] is True
    assert empty["content"][0]["text"] == "no turn to read: each turn needs a speaker and a text"
    crowd = call(bridge, "create_podcast", {"turns": [{"speaker": f"S{i}", "text": "Bonjour."} for i in range(5)]})
    assert crowd["content"][0]["text"] == "too many speakers (5, at most 4)"


def test_with_a_voice_model_the_hosts_get_its_natural_voices(bridges, fakes):
    def change(c):
        c["voice_server"] = {"url": fakes.voices.url}
        c["agents"]["Wellness"]["voice"] = "pocket:mousse"
    bridge = bridges.new(change=change).start()
    give_token(bridge)
    fakes.voices.voices = ["fr-mascotte-s17", "fr-naturelle-s15", "fr-naturelle-s21", "en-naturelle-s47"]
    fakes.voices.mode = "wav24k"
    listed = call(bridge, "list_voices", {})["content"][0]["text"].splitlines()
    assert listed[:4] == ["pocket:mousse (your voice)", "server:fr/fr-mascotte-s17 (voice model)",
                          "server:fr/fr-naturelle-s15 (voice model)", "server:fr/fr-naturelle-s21 (voice model)"]
    assert "pocket:galet (Bip)" in listed and not any("en-naturelle" in line for line in listed)
    turns = [{"speaker": "Max", "text": "Bonjour."}, {"speaker": "Léa", "text": "Salut."}]
    result = call(bridge, "create_podcast", {"turns": turns})
    # The agent speaks with a Bip, but a podcast takes the model's natural voices, two different ones.
    assert "- Max: server:fr/fr-naturelle-s15\n- Léa: server:fr/fr-naturelle-s21" in result["content"][0]["text"]
    # (The Python bridge sends a French server voice as « fr/<id> »: tts.server_voice_id only strips en/es/de, against
    # its docstring. Not pinned here; see README, « Found while writing this suite ».)
    assert [b["voice"].rsplit("/", 1)[-1] for b in fakes.voices.inputs] == ["fr-naturelle-s15", "fr-naturelle-s21"]
    assert all(b["model"] == "tts-1" and b["response_format"] == "wav" for b in fakes.voices.inputs)
    # The person keeps the voice model away from reading: Bips.
    assert bridge.put("/v1/voices/uses", headers=AUTH, json={"reading": False}).status_code == 200
    again = call(bridge, "create_podcast", {"turns": turns})
    assert "- Max: pocket:mousse\n- Léa: pocket:colibri" in again["content"][0]["text"]   # the first other Bip
