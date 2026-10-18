"""What the AI Act's transparency rules (art. 50) and the privacy policy ask of the bridge: site icons fetched here
(not by the phone), agents saying they are AI agents when they write to others, generated audio marked as such."""
import wave

from conftest import AUTH

from bipbridge import souls
from bipbridge.agent_tools import AI_SOURCE, ai_mark, tag_wav


def test_site_icons_come_through_the_bridge_and_are_kept(client, backend):
    resp = client.get("/v1/favicon/Example.com", headers=AUTH)
    assert resp.status_code == 200 and resp.content.startswith(b"\x89PNG") and resp.headers["content-type"] == "image/png"
    assert client.get("/v1/favicon/example.com", headers=AUTH).status_code == 200
    assert backend.icon_requests == ["https://icons.duckduckgo.com/ip3/example.com.ico"]   # asked once, then kept
    # A site without an icon: asked once a day at most.
    assert client.get("/v1/favicon/nothing.example", headers=AUTH).status_code == 404
    assert client.get("/v1/favicon/nothing.example", headers=AUTH).status_code == 404
    assert len(backend.icon_requests) == 2
    # Only site names, and only for the app.
    for bad in ("127.0.0.1", "localhost", "a..b", "x" * 300 + ".com"):
        assert client.get(f"/v1/favicon/{bad}", headers=AUTH).status_code == 400
    assert client.get("/v1/favicon/example.com").status_code == 401


def test_agents_say_they_are_ai_when_they_write_to_others():
    block = souls.language_block("fr")
    assert souls.AI_RULE in block and "never present yourself as a human" in block
    soul = souls.with_language("Tu es Vie.", "fr")
    assert souls.split_language(soul) == ("Tu es Vie.\n", "fr")


def test_generated_audio_says_what_it_is(tmp_path):
    path = tmp_path / "podcast.wav"
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(24000)
        out.writeframes(b"\x01\x00" * 2400)
    tag_wav(path, ai_mark("Vie"))
    data = path.read_bytes()
    assert int.from_bytes(data[4:8], "little") == len(data) - 8
    assert b"INFO" in data and AI_SOURCE.encode() in data and b"BipAgents" in data
    with wave.open(str(path), "rb") as back:   # still a valid WAV, same audio
        assert back.getnframes() == 2400 and back.readframes(2400) == b"\x01\x00" * 2400
