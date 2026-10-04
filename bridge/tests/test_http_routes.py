"""Auth, /health, /v1/tts/sentence, /v1/files, /v1/devices."""
import os
import stat
import time

import pytest
from starlette.websockets import WebSocketDisconnect

from bipbridge.files import purge_uploads, sanitize_filename
from conftest import AUTH, BRIDGE_KEY, pcm_for


# -- auth ------------------------------------------------------------------------------------------

def test_health_is_public_and_reports_reachability(client):
    resp = client.get("/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True and body["kyutai"] is True and body["ntfy"] is True and body["apns"] is True
    assert "version" in body


@pytest.mark.parametrize("headers", [{}, {"Authorization": "Bearer nope"}, {"Authorization": BRIDGE_KEY},
                                     {"Authorization": f"Basic {BRIDGE_KEY}"}])
def test_routes_require_bridge_key(client, headers):
    for method, path in [("get", "/v1/outbox"), ("post", "/v1/tts/sentence"), ("post", "/v1/devices"),
                         ("delete", "/v1/devices/abc"), ("post", "/v1/files"), ("get", "/v1/agents"),
                         ("post", "/v1/watch"), ("post", "/v1/approve"), ("get", "/v1/approvals")]:
        resp = getattr(client, method)(path, headers=headers)
        assert resp.status_code == 401, (method, path)
        assert resp.headers.get("www-authenticate") == "Bearer"


def test_websocket_requires_bridge_key(client):
    with pytest.raises(WebSocketDisconnect) as exc:
        with client.websocket_connect("/v1/voice", headers={"Authorization": "Bearer wrong"}):
            pass
    assert exc.value.code == 1008


def test_agents_listing(client):
    resp = client.get("/v1/agents", headers=AUTH)
    assert resp.json() == {"agents": [{"id": "wellness", "name": "Wellness", "voice": "4193", "uploads": True,
                                       "inbox": True}]}


# -- TTS -------------------------------------------------------------------------------------------

def test_tts_sentence_pcm16(client):
    resp = client.post("/v1/tts/sentence", headers=AUTH, json={"text": "Bonjour *toi*", "format": "pcm16"})
    assert resp.status_code == 200
    assert resp.content == pcm_for("Bonjour toi.")
    assert resp.headers["x-sample-rate"] == "24000" and resp.headers["x-channels"] == "1"
    assert resp.headers["x-sample-format"] == "s16le" and resp.headers["x-cache"] == "miss"
    assert client.backend.kyutai_inputs[-1]["voice"] == "5476"
    again = client.post("/v1/tts/sentence", headers=AUTH, json={"text": "Bonjour *toi*"})
    assert again.headers["x-cache"] == "hit"


def test_tts_sentence_agent_voice_and_formats(client):
    resp = client.post("/v1/tts/sentence", headers=AUTH, json={"text": "Salut.", "agent": "Wellness", "format": "opus"})
    assert resp.status_code == 200 and resp.headers["content-type"] == "audio/ogg"
    assert client.backend.kyutai_inputs[-1] == {"model": "kyutai-tts", "input": "Salut.", "voice": "4193",
                                                "response_format": "opus"}
    assert client.post("/v1/tts/sentence", headers=AUTH, json={"text": "x", "format": "flac"}).status_code == 400
    assert client.post("/v1/tts/sentence", headers=AUTH, json={"text": "x", "agent": "zzz"}).status_code == 404
    assert client.post("/v1/tts/sentence", headers=AUTH, json={"text": "---"}).status_code == 400


# -- files -----------------------------------------------------------------------------------------

def test_upload_maps_path_into_container(client, config):
    resp = client.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("Relevé de compte (oct).pdf", b"%PDF-1.4 data", "application/pdf")})
    assert resp.status_code == 200, resp.text
    body = resp.json()
    month = time.strftime("%Y-%m", time.gmtime())
    assert body["filename"] == "Relevé_de_compte_(oct).pdf"
    assert body["size"] == len(b"%PDF-1.4 data")
    prefix = f"/home/hermes/.hermes/profiles/wellness/uploads/{month}/"
    assert body["path"].startswith(prefix)
    stored = body["path"][len(prefix):]
    assert stored.endswith("-Relevé_de_compte_(oct).pdf") and len(stored.split("-", 1)[0]) == 12
    host_path = os.path.join(config.agents["wellness"].upload_dir_host, month, stored)
    with open(host_path, "rb") as fh:
        assert fh.read() == b"%PDF-1.4 data"
    assert stat.S_IMODE(os.stat(host_path).st_mode) == 0o640


def test_upload_size_limit(client, config):
    too_big = b"x" * (config.limits.upload_max_bytes + 1)
    resp = client.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("big.bin", too_big, "application/octet-stream")})
    assert resp.status_code == 413
    month_dir = os.path.join(config.agents["wellness"].upload_dir_host, time.strftime("%Y-%m", time.gmtime()))
    assert not os.path.isdir(month_dir) or os.listdir(month_dir) == []


def test_upload_size_limit_without_content_length(client, config):
    def body():
        boundary = b"--b0undary"
        yield boundary + b'\r\nContent-Disposition: form-data; name="agent"\r\n\r\nwellness\r\n'
        yield boundary + b'\r\nContent-Disposition: form-data; name="file"; filename="a.bin"\r\n\r\n'
        for _ in range(3):
            yield b"y" * (config.limits.upload_max_bytes // 2)
        yield b"\r\n" + boundary + b"--\r\n"

    resp = client.post("/v1/files", headers={**AUTH, "content-type": "multipart/form-data; boundary=b0undary"},
                       content=body())
    assert resp.status_code == 413


def test_upload_errors(client):
    assert client.post("/v1/files", headers=AUTH, data={"agent": "inconnu"},
                       files={"file": ("a.txt", b"x", "text/plain")}).status_code == 404
    assert client.post("/v1/files", headers=AUTH, data={"agent": "wellness"}).status_code == 400


@pytest.mark.parametrize("raw,expected", [
    ("../../etc/passwd", "passwd"),
    ("C:\\Users\\alex\\notes.txt", "notes.txt"),
    (".bashrc", "bashrc"),
    ("rapport final\n2026.xlsx", "rapport_final2026.xlsx"),
    ("a;b|c$(rm).sh", "a_b_c_(rm).sh"),
    ("", "fichier"),
    ("..", "fichier"),
    ("é" * 200 + ".pdf", "é" * 116 + ".pdf"),
])
def test_sanitize_filename(raw, expected):
    assert sanitize_filename(raw) == expected


def test_purge_only_old_bridge_files(tmp_path):
    month = tmp_path / "2026-01"
    month.mkdir()
    old = month / "0123456789ab-old.pdf"
    new = month / "0123456789ac-new.pdf"
    foreign = month / "notes.txt"
    for f in (old, new, foreign):
        f.write_bytes(b"x")
    long_ago = time.time() - 40 * 86400
    os.utime(old, (long_ago, long_ago))
    os.utime(foreign, (long_ago, long_ago))
    assert purge_uploads(str(tmp_path), 30) == 1
    assert not old.exists() and new.exists() and foreign.exists()


# -- devices ---------------------------------------------------------------------------------------

TOKEN = "a" * 64


def test_device_register_update_delete(client):
    resp = client.post("/v1/devices", headers=AUTH, json={"token": TOKEN.upper(), "environment": "sandbox",
                                                          "agent_ids": ["Wellness"]})
    assert resp.status_code == 200
    assert resp.json()["token"] == TOKEN and resp.json()["agent_ids"] == ["wellness"]
    resp = client.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "production",
                                                          "agent_ids": []})
    assert resp.json()["environment"] == "production" and resp.json()["agent_ids"] == []
    assert client.delete(f"/v1/devices/{TOKEN}", headers=AUTH).status_code == 204
    assert client.delete(f"/v1/devices/{TOKEN}", headers=AUTH).status_code == 204  # idempotent


def test_device_validation(client):
    assert client.post("/v1/devices", headers=AUTH, json={"token": "not-hex"}).status_code == 400
    assert client.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "dev"}).status_code == 400
    resp = client.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "agent_ids": ["ghost"]})
    assert resp.status_code == 400 and resp.json()["detail"]["agent_ids"] == ["ghost"]
    # environment defaults to [apns].environment
    assert client.post("/v1/devices", headers=AUTH, json={"token": TOKEN}).json()["environment"] == "sandbox"


def test_tts_message_returns_one_mp3_for_a_whole_reply(client):
    text = "Premier conseil : coupe les écrans. " * 30  # well over the per-sentence limit
    resp = client.post("/v1/tts/message", headers=AUTH, json={"text": text, "agent": "Wellness"})
    assert resp.status_code == 200
    assert resp.headers["content-type"] == "audio/mpeg"
    assert client.backend.kyutai_inputs[-1]["response_format"] == "mp3"
    assert len(client.backend.kyutai_inputs[-1]["input"]) > 1000
    assert client.post("/v1/tts/message", headers=AUTH, json={"text": "x" * 9000}).status_code == 413
    assert client.post("/v1/tts/message", headers=AUTH, json={"text": "x", "format": "flac"}).status_code == 400



def test_tts_stream_relays_kyutai_pcm_chunks(client):
    text = "Bonjour Alex. On commence doucement."
    resp = client.post("/v1/tts/stream", headers=AUTH, json={"text": text, "agent": "Wellness"})
    assert resp.status_code == 200
    assert resp.headers["x-sample-rate"] == "24000"
    assert resp.headers["x-sample-format"] == "s16le"
    sent = client.backend.kyutai_inputs[-1]
    assert sent["format"] == "pcm16" and "response_format" not in sent
    assert resp.content == pcm_for(sent["input"])
    calls = len(client.backend.kyutai_inputs)
    again = client.post("/v1/tts/stream", headers=AUTH, json={"text": text, "agent": "Wellness"})
    assert again.content == resp.content and len(client.backend.kyutai_inputs) == calls  # cached


def test_tts_stream_falls_back_to_whole_file_on_old_kyutai(client):
    client.backend.kyutai_streaming = False
    resp = client.post("/v1/tts/stream", headers=AUTH, json={"text": "Un ancien Kyutai."})
    assert resp.status_code == 200
    assert client.backend.kyutai_inputs[-1]["response_format"] == "wav"
    assert resp.content == pcm_for(client.backend.kyutai_inputs[-1]["input"])


def test_tts_stream_errors_before_audio_are_http_errors(client):
    assert client.post("/v1/tts/stream", headers=AUTH, json={"text": "  "}).status_code == 400
    assert client.post("/v1/tts/stream", headers=AUTH, json={"text": "x" * 9000}).status_code == 413
    assert client.post("/v1/tts/stream", headers=AUTH, json={"text": "a", "agent": "Nope"}).status_code == 404
    client.backend.kyutai_status = 500
    assert client.post("/v1/tts/stream", headers=AUTH, json={"text": "Panne."}).status_code == 502


def test_pocket_voices_go_to_pocket_tts(client):
    text = "Coucou, on marche un peu ?"
    resp = client.post("/v1/tts/stream", headers=AUTH, json={"text": text, "voice": "pocket:loutre"})
    assert resp.status_code == 200
    sent = client.backend.pocket_inputs[-1]
    assert sent["voice"] == "loutre" and sent["format"] == "pcm16"
    assert resp.content == pcm_for(sent["input"][::-1])
    kyutai_calls = len(client.backend.kyutai_inputs)
    whole = client.post("/v1/tts/message", headers=AUTH, json={"text": text, "voice": "pocket:loutre"})
    assert whole.status_code == 200 and whole.content.startswith(b"ID3pocket")
    assert client.backend.pocket_inputs[-1]["response_format"] == "mp3"
    assert len(client.backend.kyutai_inputs) == kyutai_calls  # Kyutai untouched
    assert client.get("/health").json()["pocket"] is True


def test_pocket_down_falls_back_to_kyutai_default_voice(client):
    client.backend.pocket_status = 503
    resp = client.post("/v1/tts/stream", headers=AUTH, json={"text": "Pocket est en panne.", "voice": "pocket:lutin"})
    assert resp.status_code == 200
    assert client.backend.kyutai_inputs[-1]["voice"] == "5476"
    assert resp.content == pcm_for(client.backend.kyutai_inputs[-1]["input"])
    sentence = client.post("/v1/tts/sentence", headers=AUTH, json={"text": "Toujours en panne.", "voice": "pocket:lutin"})
    assert sentence.status_code == 200 and client.backend.kyutai_inputs[-1]["voice"] == "5476"
    # The fallback audio is not cached under the Pocket voice: once Pocket is back, it is used.
    client.backend.pocket_status = 200
    again = client.post("/v1/tts/stream", headers=AUTH, json={"text": "Pocket est en panne.", "voice": "pocket:lutin"})
    assert client.backend.pocket_inputs[-1]["voice"] == "lutin" and again.content != resp.content
