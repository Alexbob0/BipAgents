import asyncio
import json
import os

import pytest

from bipbridge import agent_tools
from bipbridge.config import parse_config
from conftest import AUTH


def test_a_long_text_is_read_in_short_pieces_paragraph_by_paragraph():
    text = ("Bonjour et bienvenue. " * 30).strip() + "\n\nDeuxième partie, plus courte.\nMEDIA:/tmp/old.mp3\n"
    chunks = agent_tools.split_for_speech(text, limit=120)
    assert all(len(piece) <= 120 for piece, _ in chunks)
    assert " ".join(piece for piece, _ in chunks).count("Bonjour et bienvenue.") == 30   # nothing lost or repeated
    assert chunks[-1] == ("Deuxième partie, plus courte.", agent_tools.PAUSE_PARAGRAPH_MS)
    assert [pause for _, pause in chunks].count(agent_tools.PAUSE_PARAGRAPH_MS) == 2      # one per paragraph
    # A sentence longer than a piece is cut at its commas, else between words.
    long_one = ", ".join(["une idée assez longue"] * 20) + "."
    assert all(len(piece) <= 60 for piece, _ in agent_tools.split_for_speech(long_one, limit=60))


@pytest.fixture
def managed(config_dict, tmp_path):
    hub = tmp_path / ".hermes"
    (hub / "profiles" / "vie").mkdir(parents=True)
    (hub / "profiles" / "vie" / "config.yaml").write_text("model:\n  default: x\n")
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(hub), "hub_url": "http://127.0.0.1:8642"}
    config_dict["agents"]["Vie"] = {"hermes_url": "http://127.0.0.1:8642/p/vie", "hermes_key": "k" * 32}
    config = parse_config(config_dict)
    commands = []

    async def run(args, env):
        commands.append((list(args), dict(env)))
        return ""

    return config, hub, run, commands


def test_each_agent_of_the_install_gets_the_tools_with_a_token_of_its_own(managed):
    config, hub, run, commands = managed
    asyncio.run(agent_tools.register_all(config, run))
    # Only the agents this install manages (Wellness is set up by hand): its Hermes, its profile, its HERMES_HOME.
    assert {args[2] for args, _ in commands} == {"vie"}
    assert all(env == {"HERMES_HOME": str(hub)} for _, env in commands)
    values = {args[5]: args[6] for args, _ in commands}
    assert values["mcp_servers.bipagents.url"] == (f"http://127.0.0.1:{config.port}/v1/agent-tools/mcp"
                                                   f"?v={agent_tools.tools_version()}")
    assert values["mcp_servers.bipagents.timeout"] == str(agent_tools.TOOL_TIMEOUT)
    token = values["mcp_servers.bipagents.headers.Authorization"].removeprefix("Bearer ")
    assert agent_tools.agent_for(config, token).name == "vie" and agent_tools.agent_for(config, "nope") is None
    assert oct(agent_tools.store_path(config).stat().st_mode & 0o777) == "0o600"
    # Once Hermes holds it: nothing to do at the next start, and the same token.
    (hub / "profiles" / "vie" / "config.yaml").write_text(
        "mcp_servers:\n  bipagents:\n    url: " + values["mcp_servers.bipagents.url"]
        + "\n    headers:\n      Authorization: Bearer " + token + f"\n    timeout: {agent_tools.TOOL_TIMEOUT}\n")
    commands.clear()
    asyncio.run(agent_tools.register_all(config, run))
    assert commands == []
    # New tools in a later version: a new entry, so Hermes fetches the list again (the token stays).
    real = agent_tools.TOOLS
    agent_tools.TOOLS = real + [{"name": "later_tool", "inputSchema": {"type": "object"}}]
    try:
        asyncio.run(agent_tools.register_all(config, run))
    finally:
        agent_tools.TOOLS = real
    changed = {args[5]: args[6] for args, _ in commands}
    assert changed["mcp_servers.bipagents.url"] != values["mcp_servers.bipagents.url"]
    assert changed["mcp_servers.bipagents.headers.Authorization"] == f"Bearer {token}"
    # A deleted agent's token no longer opens anything.
    agent_tools.forget(config, "vie")
    assert agent_tools.agent_for(config, token) is None


def _rpc(client, token, method, params=None, request_id=1, accept="application/json"):
    body = {"jsonrpc": "2.0", "method": method, "params": params or {}}
    if request_id is not None:
        body["id"] = request_id
    return client.post("/v1/agent-tools/mcp", json=body,
                       headers={"Authorization": f"Bearer {token}", "Accept": accept})


def test_an_agent_turns_a_text_into_audio_with_its_voice(client, config, backend):
    token = "agent-token-for-wellness-0123456789"
    agent_tools.save_tokens(config, {"wellness": agent_tools._digest(token)})
    assert _rpc(client, "wrong", "initialize").status_code == 401
    init = _rpc(client, token, "initialize", {"protocolVersion": "2025-06-18"}).json()["result"]
    assert init["protocolVersion"] == "2025-06-18" and init["capabilities"]["tools"] is not None
    assert _rpc(client, token, "notifications/initialized", request_id=None).status_code == 202
    tools = _rpc(client, token, "tools/list").json()["result"]["tools"]
    assert [t["name"] for t in tools][:3] == ["create_audio", "create_podcast", "list_voices"]

    text = "Bonjour et bienvenue dans ce podcast.\n\nVoici les nouvelles du jour."
    result = _rpc(client, token, "tools/call", {"name": "create_audio",
                                                 "arguments": {"text": text, "title": "Podcast du matin"}}).json()["result"]
    assert result["isError"] is False
    media = result["content"][0]["text"].split("MEDIA:")[1].strip()
    assert media.startswith("/bipagents/audio/wellness/") and "podcast-du-matin" in media
    # Read with the agent's reading voice, one paragraph at a time, in the background queue.
    assert [body["voice"] for body in backend.kyutai_inputs] == ["4193", "4193"]
    assert len(backend.kyutai_inputs) == 2
    # The app gets the file like any MEDIA: file.
    audio = client.get("/v1/media", params={"path": media}, headers=AUTH)
    assert audio.status_code == 200 and len(audio.content) > 44
    assert client.get("/v1/media", params={"path": "/bipagents/audio/../bridge.db"}, headers=AUTH).status_code == 404


def test_a_long_call_is_streamed_and_errors_are_the_tools_not_the_protocols(client, config, backend):
    token = "agent-token-for-wellness-0123456789"
    agent_tools.save_tokens(config, {"wellness": agent_tools._digest(token)})
    streamed = _rpc(client, token, "tools/call", {"name": "create_audio", "arguments": {"text": "Une phrase."}},
                    accept="application/json, text/event-stream")
    assert streamed.headers["content-type"].startswith("text/event-stream")
    event = [line for line in streamed.text.splitlines() if line.startswith("data: ")][-1]
    assert json.loads(event[len("data: "):])["result"]["isError"] is False
    too_long = _rpc(client, token, "tools/call",
                    {"name": "create_audio", "arguments": {"text": "x" * (agent_tools.MAX_CHARS + 1)}}).json()
    assert too_long["result"]["isError"] is True and "too long" in too_long["result"]["content"][0]["text"]
    backend.kyutai_status = 500
    failed = _rpc(client, token, "tools/call", {"name": "create_audio", "arguments": {"text": "Autre phrase."}}).json()
    assert failed["result"]["isError"] is True
    assert _rpc(client, token, "resources/list").json()["error"]["code"] == -32601


def test_old_audio_files_are_purged(config, tmp_path):
    folder = agent_tools.audio_root(config) / "wellness"
    folder.mkdir(parents=True)
    old, new = folder / "old.m4a", folder / "new.m4a"
    old.write_bytes(b"x")
    new.write_bytes(b"y")
    os.utime(old, (1, 1))
    assert agent_tools.purge_audio(config, retention_days=30) == 1
    assert not old.exists() and new.exists()


def test_a_podcast_gives_each_host_a_voice_of_their_own(client, config, backend):
    token = "agent-token-for-wellness-0123456789"
    agent_tools.save_tokens(config, {"wellness": agent_tools._digest(token)})
    listed = _rpc(client, token, "tools/call", {"name": "list_voices", "arguments": {}}).json()["result"]
    assert listed["content"][0]["text"].splitlines()[:2] == ["4193 (your voice)", "pocket:colibri (Bip)"]
    turns = [{"speaker": "Max", "text": "Bonjour à tous, bienvenue."},
             {"speaker": "Léa", "text": "Salut Max ! Alors, quoi de neuf ?"},
             {"speaker": "Max", "text": "Trois nouvelles aujourd'hui."}]
    result = _rpc(client, token, "tools/call", {"name": "create_podcast",
                                                 "arguments": {"title": "Le point", "turns": turns}}).json()["result"]
    assert result["isError"] is False
    assert "Max: 4193" in result["content"][0]["text"] and "Léa: pocket:colibri" in result["content"][0]["text"]
    # The first host with the agent's voice, the other with another one; turns read in order.
    assert [body["voice"] for body in backend.kyutai_inputs] == ["4193", "4193"]
    assert [body["voice"] for body in backend.pocket_inputs] == ["colibri"]
    # A voice picked by the agent, from list_voices; an unknown one is refused.
    picked = _rpc(client, token, "tools/call", {"name": "create_podcast", "arguments": {
        "turns": turns, "voices": {"Léa": "pocket:galet"}}}).json()["result"]
    assert picked["isError"] is False and backend.pocket_inputs[-1]["voice"] == "galet"
    bad = _rpc(client, token, "tools/call", {"name": "create_podcast", "arguments": {
        "turns": turns, "voices": {"Léa": "pocket:dragon"}}}).json()["result"]
    assert bad["isError"] is True and "unknown voice" in bad["content"][0]["text"]
    empty = _rpc(client, token, "tools/call", {"name": "create_podcast", "arguments": {"turns": [{"speaker": "Max"}]}}).json()
    assert empty["result"]["isError"] is True


def test_with_a_voice_model_the_hosts_get_its_natural_voices(config_dict):
    config_dict["voice_server"] = {"url": "http://voices.test"}
    config_dict["agents"]["Wellness"]["voice"] = "pocket:mousse"
    config = parse_config(config_dict)
    maker = agent_tools.AudioMaker(config, tts=None)

    async def voices(agent):
        return [{"voice": "pocket:mousse", "kind": "your voice"},
                {"voice": "server:fr/fr-mascotte-s17", "kind": "voice model"},
                {"voice": "server:fr/fr-naturelle-s15", "kind": "voice model"},
                {"voice": "server:fr/fr-naturelle-s21", "kind": "voice model"},
                {"voice": "pocket:galet", "kind": "Bip"}]

    maker.voices = voices
    cast = asyncio.run(maker.cast(config.agent("wellness"), ["Max", "Léa"], {}))
    # The agent speaks with a Bip, but a podcast takes the model's natural voices, two different ones.
    assert cast == {"Max": "server:fr/fr-naturelle-s15", "Léa": "server:fr/fr-naturelle-s21"}
    config.voice_server.uses["reading"] = False   # the person keeps the voice model away from reading: Bips
    assert asyncio.run(maker.cast(config.agent("wellness"), ["Max", "Léa"], {})) == {"Max": "pocket:mousse",
                                                                                    "Léa": "pocket:galet"}
