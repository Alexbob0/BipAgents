"""Auth, /health, /v1/tts/*, /v1/files, /v1/devices (tests/test_http_routes.py, over HTTP)."""
import os
import stat
import time

import pytest

from bbkit.common import AUTH, BRIDGE_KEY, REMOTE, pcm_for
from bbkit.ws import InvalidStatus, open_ws


# -- auth ------------------------------------------------------------------------------------------

def test_health_is_public_and_reports_reachability(bridge):
    resp = bridge.get("/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["ok"] is True and body["kyutai"] is True and body["ntfy"] is True and body["apns"] is True
    assert "version" in body
    assert body.pop("features", []) in ([], ["agent_groups", "files_kept", "calendar"])   # the Rust bridge's own
    assert body == {"ok": True, "version": body["version"], "kyutai": True, "pocket": True, "voice_server": None,
                    "drama": None, "ntfy": True, "apns": True, "push": "apns", "push_relay": False, "fcm": False,
                    "tts_queue": 0}


@pytest.mark.parametrize("headers", [{}, {"Authorization": "Bearer nope"}, {"Authorization": BRIDGE_KEY},
                                     {"Authorization": f"Basic {BRIDGE_KEY}"}])
def test_routes_require_bridge_key(bridge, headers):
    for method, path in [("get", "/v1/outbox"), ("post", "/v1/tts/sentence"), ("post", "/v1/devices"),
                         ("delete", "/v1/devices/abc"), ("post", "/v1/files"), ("get", "/v1/agents"),
                         ("post", "/v1/watch"), ("post", "/v1/approve"), ("get", "/v1/approvals")]:
        resp = bridge.request(method.upper(), path, headers=headers)
        assert resp.status_code == 401, (method, path)
        assert resp.headers.get("www-authenticate") == "Bearer"
        assert resp.json() == {"detail": "unauthorized"}


def test_the_bearer_scheme_is_case_insensitive(bridge):
    assert bridge.get("/v1/agents", headers={"Authorization": f"bearer {BRIDGE_KEY}"}).status_code == 200
    assert bridge.get("/v1/agents", headers={"Authorization": f"BEARER   {BRIDGE_KEY}"}).status_code == 200


def test_websocket_requires_bridge_key(bridge):
    for headers in ({"Authorization": "Bearer wrong"}, {}):
        with pytest.raises(InvalidStatus) as exc:
            with open_ws(bridge.ws_url("/v1/voice"), headers):
                pass
        assert exc.value.response.status_code == 403   # closed (1008) before the handshake: HTTP 403


def test_agents_listing(bridge):
    resp = bridge.get("/v1/agents", headers=AUTH)
    listing = resp.json()
    assert all(a.pop("group", "") == "" for a in listing["agents"])   # agent groups: Rust only
    assert listing == {"bridge": {"url": None, "lan": None},
                           "agents": [{"id": "wellness", "name": "Wellness", "voice": "4193", "server_voice": None,
                                       "category": None, "color": None, "uploads": True,
                                       "inbox": True, "url": None, "key": None}],
                           "unclaimed": []}


def test_unknown_routes_and_methods(bridge):
    assert bridge.get("/v1/nothing-here", headers=AUTH).status_code == 404
    assert bridge.get("/v1/nothing-here", headers=AUTH).json() == {"detail": "Not Found"}
    resp = bridge.put("/v1/agents", headers=AUTH, json={})
    assert resp.status_code == 405 and resp.json() == {"detail": "Method Not Allowed"}
    # Starlette's `Allow`: the methods of the first route declared for the path, not every method the path has.
    allow = lambda method, path: bridge.request(method, path, headers=AUTH).headers.get("allow")   # noqa: E731
    assert allow("PUT", "/v1/agents") == "GET" and allow("HEAD", "/v1/agents") == "GET"
    assert allow("PUT", "/v1/vault") == "GET" and allow("GET", "/v1/vault/x") == "PATCH"
    assert allow("GET", "/v1/agents/claim") == "POST" and allow("POST", "/v1/agents/x/settings") == "GET"
    assert allow("PATCH", "/v1/agents/x/memory/pages/a/b.md") == "GET"
    # A path with another method falls through to a later route: DELETE /v1/family/{household}, no `Allow`.
    resp = bridge.delete("/v1/family/name", headers=AUTH)
    assert resp.status_code == 404 and resp.json() == {"detail": "no such household"} and "allow" not in resp.headers


def test_validation_errors_are_422_with_a_list(bridge):
    resp = bridge.post("/v1/tts/sentence", headers=AUTH, json={"voice": "x"})
    assert resp.status_code == 422
    detail = resp.json()["detail"]
    assert isinstance(detail, list) and detail[0]["loc"] == ["body", "text"] and detail[0]["type"] == "missing"
    bad_json = bridge.post("/v1/tts/sentence", headers={**AUTH, "content-type": "application/json"}, content=b"{nope")
    assert bad_json.status_code == 422 and isinstance(bad_json.json()["detail"], list)
    too_long = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "x" * 20001})
    assert too_long.status_code == 422 and too_long.json()["detail"][0]["type"] == "string_too_long"


def test_wrong_keys_from_the_internet_get_the_address_blocked(bridge):
    """guard.py: 20 wrong keys in 10 minutes from one address (not this machine's) block it for 15 minutes, even with
    the right key; requests without a key don't count, and the bridge's own address is never counted."""
    remote = {"X-Forwarded-For": REMOTE}
    for _ in range(5):
        assert bridge.get("/v1/agents", headers=remote).status_code == 401           # no key: not counted
    for _ in range(19):
        assert bridge.get("/v1/agents", headers={**remote, "Authorization": "Bearer nope"}).status_code == 401
    assert bridge.get("/v1/agents", headers={**remote, **AUTH}).status_code == 200
    assert bridge.get("/v1/agents", headers={**remote, "Authorization": "Bearer nope"}).status_code == 401
    blocked = bridge.get("/v1/agents", headers={**remote, **AUTH})
    assert blocked.status_code == 429 and blocked.json() == {"detail": "too many wrong keys, try again later"}
    assert bridge.get("/v1/agents", headers={"X-Forwarded-For": "203.0.113.51", **AUTH}).status_code == 200
    for _ in range(25):
        bridge.get("/v1/agents", headers={"Authorization": "Bearer nope"})          # 127.0.0.1: never counted
    assert bridge.get("/v1/agents", headers=AUTH).status_code == 200
    warned = bridge.logged("wrong key")
    assert warned and warned[0]["address"] == REMOTE and warned[0]["what"] == "bridge key" and warned[0]["count"] == 1
    assert bridge.logged("address blocked")[0]["minutes"] == 15


# -- TTS -------------------------------------------------------------------------------------------

def test_tts_sentence_pcm16(bridge, fakes):
    resp = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "Bonjour *toi*", "format": "pcm16"})
    assert resp.status_code == 200
    assert resp.content == pcm_for("Bonjour toi.")
    assert resp.headers["x-sample-rate"] == "24000" and resp.headers["x-channels"] == "1"
    assert resp.headers["x-sample-format"] == "s16le" and resp.headers["x-cache"] == "miss"
    assert resp.headers["cache-control"] == "no-store"
    assert resp.headers["content-type"] == "application/octet-stream"
    assert fakes.kyutai.inputs[-1]["voice"] == "5476"
    again = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "Bonjour *toi*"})
    assert again.headers["x-cache"] == "hit" and again.content == resp.content


def test_tts_sentence_agent_voice_and_formats(bridge, fakes):
    resp = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "Salut.", "agent": "Wellness", "format": "opus"})
    assert resp.status_code == 200 and resp.headers["content-type"] == "audio/ogg"
    assert fakes.kyutai.inputs[-1] == {"model": "kyutai-tts", "input": "Salut.", "voice": "4193",
                                       "response_format": "opus"}
    assert "x-sample-format" not in resp.headers
    flac = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "x", "format": "flac"})
    assert flac.status_code == 400 and flac.json() == {"detail": "format must be pcm16, opus or wav"}
    unknown = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "x", "agent": "zzz"})
    assert unknown.status_code == 404 and unknown.json() == {"detail": "unknown agent"}
    assert bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "---"}).status_code == 400
    wav = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "Un wav.", "format": "wav"})
    assert wav.status_code == 200 and wav.content[:4] == b"RIFF" and wav.headers["content-type"] == "audio/wav"


def test_a_bip_s_text_is_prepared_for_the_phone_to_speak_it(bridge):
    resp = bridge.post("/v1/tts/prepare", headers=AUTH, json={"text": "Rendez-vous à 9h30 avec *Garmin* (le RDV).",
                                                               "voice": "pocket:galet"})
    data = resp.json()
    assert resp.status_code == 200 and data["local"] is True and data["language"] == "fr" and data["bip"] == "galet"
    assert "neuf" in data["text"] and "*" not in data["text"]
    assert data == {"local": True, "voice": "pocket:galet", "text": "Rendez-vous à neuf heures trente avec Garmine, le rendez-vous.",
                    "language": "fr", "bip": "galet"}
    english = bridge.post("/v1/tts/prepare", headers=AUTH, json={"text": "See you at 5.", "voice": "pocket:en/lumen"})
    assert english.json()["language"] == "en" and english.json()["bip"] == "lumen"
    kyutai = bridge.post("/v1/tts/prepare", headers=AUTH, json={"text": "Salut.", "agent": "wellness"})
    assert kyutai.json() == {"local": False, "voice": "4193"}                  # not a Bip: audio from the bridge
    assert bridge.post("/v1/tts/prepare", headers=AUTH, json={"text": "---", "voice": "pocket:galet"}).status_code == 400
    assert bridge.post("/v1/tts/prepare", json={"text": "x"}).status_code == 401
    assert bridge.post("/v1/tts/prepare", headers=AUTH, json={"text": "x", "agent": "nope"}).status_code == 404


# -- files -----------------------------------------------------------------------------------------

def test_upload_maps_path_into_container(bridge):
    resp = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("Relevé de compte (oct).pdf", b"%PDF-1.4 data", "application/pdf")})
    assert resp.status_code == 200, resp.text
    body = resp.json()
    month = time.strftime("%Y-%m", time.gmtime())
    assert body["filename"] == "Relevé_de_compte_(oct).pdf"
    assert body["size"] == len(b"%PDF-1.4 data") and body["content_type"] == "application/pdf"
    prefix = f"/home/hermes/.hermes/profiles/wellness/uploads/{month}/"
    assert body["path"].startswith(prefix)
    stored = body["path"][len(prefix):]
    assert stored.endswith("-Relevé_de_compte_(oct).pdf") and len(stored.split("-", 1)[0]) == 12
    host_path = os.path.join(bridge.config["agents"]["Wellness"]["upload_dir_host"], month, stored)
    with open(host_path, "rb") as fh:
        assert fh.read() == b"%PDF-1.4 data"
    assert stat.S_IMODE(os.stat(host_path).st_mode) == 0o640
    assert set(body) == {"path", "filename", "size", "content_type"}


@pytest.mark.parametrize("raw,expected", [
    ("../../etc/passwd", "passwd"),
    ("C:\\Users\\alex\\notes.txt", "notes.txt"),
    (".bashrc", "bashrc"),
    ("rapport  final 2026.xlsx", "rapport_final_2026.xlsx"),
    ("a;b|c$(rm).sh", "a_b_c_(rm).sh"),
    ("..", "fichier"),
    ("é" * 200 + ".pdf", "é" * 116 + ".pdf"),
])
def test_uploaded_names_are_sanitized(bridge, raw, expected):
    """files.sanitize_filename, seen from the route's answer."""
    resp = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": (raw, b"x", "text/plain")})
    assert resp.status_code == 200, resp.text
    assert resp.json()["filename"] == expected
    assert resp.json()["path"].endswith("-" + expected)


def test_upload_size_limit(bridge):
    too_big = b"x" * (1024 * 1024 + 1)
    resp = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("big.bin", too_big, "application/octet-stream")})
    assert resp.status_code == 413
    assert resp.json() == {"detail": f"file too large (max {1024 * 1024} bytes)"}
    month_dir = os.path.join(bridge.config["agents"]["Wellness"]["upload_dir_host"], time.strftime("%Y-%m", time.gmtime()))
    assert not os.path.isdir(month_dir) or os.listdir(month_dir) == []


def test_upload_size_limit_by_content_length(bridge):
    big = b"y" * (1024 * 1024 + 70 * 1024)
    resp = bridge.post("/v1/files", headers={**AUTH, "content-type": "multipart/form-data; boundary=b0undary"},
                       content=big)
    assert resp.status_code == 413 and resp.json()["detail"].startswith("file too large")


def test_upload_size_limit_without_content_length(bridge):
    limit = 1024 * 1024

    def body():
        boundary = b"--b0undary"
        yield boundary + b'\r\nContent-Disposition: form-data; name="agent"\r\n\r\nwellness\r\n'
        yield boundary + b'\r\nContent-Disposition: form-data; name="file"; filename="a.bin"\r\n\r\n'
        for _ in range(3):
            yield b"y" * (limit // 2)
        yield b"\r\n" + boundary + b"--\r\n"

    resp = bridge.post("/v1/files", headers={**AUTH, "content-type": "multipart/form-data; boundary=b0undary"},
                       content=body())
    assert resp.status_code == 413


def test_upload_errors(bridge):
    unknown = bridge.post("/v1/files", headers=AUTH, data={"agent": "inconnu"}, files={"file": ("a.txt", b"x", "text/plain")})
    assert unknown.status_code == 404 and unknown.json() == {"detail": "unknown agent"}
    missing = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"})
    assert missing.status_code == 400
    garbage = bridge.post("/v1/files", headers={**AUTH, "content-type": "multipart/form-data; boundary=zz"},
                          content=b"not multipart at all")
    assert garbage.status_code in (400, 404)


def test_upload_to_an_agent_without_uploads(bridges, fakes):
    b = bridges.new()
    b.config["agents"]["Vie"] = {"hermes_url": fakes.hermes.url + "/p/vie", "hermes_key": "v" * 32}
    b.start()
    resp = b.post("/v1/files", headers=AUTH, data={"agent": "vie"}, files={"file": ("a.txt", b"x", "text/plain")})
    assert resp.status_code == 409 and resp.json() == {"detail": "uploads not configured for this agent"}


# -- devices ---------------------------------------------------------------------------------------

TOKEN = "a" * 64


def test_device_register_update_delete(bridge):
    resp = bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN.upper(), "environment": "sandbox",
                                                           "agent_ids": ["Wellness"]})
    assert resp.status_code == 200
    device = resp.json()
    assert device["token"] == TOKEN and device["agent_ids"] == ["wellness"] and device["environment"] == "sandbox"
    assert set(device) == {"token", "environment", "agent_ids", "created_at", "updated_at"}
    assert device["created_at"].endswith("Z") and len(device["created_at"]) == 24
    resp = bridge.post("/v1/devices", headers=AUTH, json={"token": f"<{TOKEN[:32]} {TOKEN[32:]}>",
                                                           "environment": "production", "agent_ids": []})
    assert resp.json()["environment"] == "production" and resp.json()["agent_ids"] == []
    assert resp.json()["created_at"] == device["created_at"]           # the same device, updated
    with bridge.db() as db:
        rows = db.execute("SELECT token, environment, agent_ids FROM devices").fetchall()
    assert rows == [(TOKEN, "production", "[]")]
    deleted = bridge.delete(f"/v1/devices/{TOKEN}", headers=AUTH)
    assert deleted.status_code == 204 and deleted.content == b""
    assert bridge.delete(f"/v1/devices/{TOKEN}", headers=AUTH).status_code == 204  # idempotent
    with bridge.db() as db:
        assert db.execute("SELECT count(*) FROM devices").fetchone()[0] == 0


def test_device_validation(bridge):
    hex_ = bridge.post("/v1/devices", headers=AUTH, json={"token": "not-hex"})
    assert hex_.status_code == 400 and hex_.json() == {"detail": "invalid device token (hex expected)"}
    env = bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "dev"})
    assert env.status_code == 400 and env.json() == {"detail": "environment must be sandbox, production or fcm"}
    resp = bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "agent_ids": ["ghost"]})
    assert resp.status_code == 400 and resp.json()["detail"]["agent_ids"] == ["ghost"]
    assert resp.json() == {"detail": {"error": "unknown agent_ids", "agent_ids": ["ghost"]}}
    # environment defaults to [apns].environment
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN}).json()["environment"] == "sandbox"
    assert bridge.post("/v1/devices", headers=AUTH, json={"agent_ids": []}).status_code == 422


def test_device_registration_is_logged_without_the_token(bridge):
    bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "sandbox", "agent_ids": ["wellness"]})
    entry = bridge.logged("device registered")[0]
    assert entry["level"] == "info" and entry["logger"] == "bipbridge.app"
    assert entry["token"] == "aaaaaa…" and entry["env"] == "sandbox" and entry["agents"] == ["wellness"]
    assert TOKEN not in bridge.log_path.read_text()
    started = bridge.logged("bridge started")[0]
    assert started["agents"] == ["wellness"] and started["apns"] is True and "version" in started
    assert started["ts"].endswith("Z")


def test_tts_message_returns_one_mp3_for_a_whole_reply(bridge, fakes):
    text = "Premier conseil : coupe les écrans. " * 30  # well over the per-sentence limit
    resp = bridge.post("/v1/tts/message", headers=AUTH, json={"text": text, "agent": "Wellness"})
    assert resp.status_code == 200
    assert resp.headers["content-type"] == "audio/mpeg"
    assert resp.headers["x-cache"] == "miss" and resp.headers["cache-control"] == "no-store"
    assert fakes.kyutai.inputs[-1]["response_format"] == "mp3"
    assert len(fakes.kyutai.inputs[-1]["input"]) > 1000
    long = bridge.post("/v1/tts/message", headers=AUTH, json={"text": "x" * 9000})
    assert long.status_code == 413
    flac = bridge.post("/v1/tts/message", headers=AUTH, json={"text": "x", "format": "flac"})
    assert flac.status_code == 400 and flac.json() == {"detail": "format must be mp3, opus or wav"}
    assert bridge.post("/v1/tts/message", headers=AUTH, json={"text": "x", "agent": "nope"}).status_code == 404


def test_tts_stream_relays_kyutai_pcm_chunks(bridge, fakes):
    text = "Bonjour Alex. On commence doucement."
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": text, "agent": "Wellness"})
    assert resp.status_code == 200
    assert resp.headers["x-sample-rate"] == "24000" and resp.headers["x-channels"] == "1"
    assert resp.headers["x-sample-format"] == "s16le" and resp.headers["content-type"] == "application/octet-stream"
    sent = fakes.kyutai.inputs[-1]
    assert sent["format"] == "pcm16" and "response_format" not in sent
    assert resp.content == pcm_for(sent["input"])
    calls = len(fakes.kyutai.inputs)
    again = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": text, "agent": "Wellness"})
    assert again.content == resp.content and len(fakes.kyutai.inputs) == calls  # cached


def test_tts_stream_falls_back_to_whole_file_on_old_kyutai(bridge, fakes):
    fakes.kyutai.streaming = False
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Un ancien Kyutai."})
    assert resp.status_code == 200
    assert fakes.kyutai.inputs[-1]["response_format"] == "wav"
    assert resp.content == pcm_for(fakes.kyutai.inputs[-1]["input"])


def test_tts_stream_errors_before_audio_are_http_errors(bridge, fakes):
    assert bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "  "}).status_code == 400
    assert bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "x" * 9000}).status_code == 413
    assert bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "a", "agent": "Nope"}).status_code == 404
    fakes.kyutai.status = 500
    assert bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Panne."}).status_code == 502


def test_pocket_voices_go_to_pocket_tts(bridge, fakes):
    text = "Coucou, on marche un peu ?"
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": text, "voice": "pocket:loutre"})
    assert resp.status_code == 200
    sent = fakes.pocket.inputs[-1]
    assert sent["voice"] == "loutre" and sent["format"] == "pcm16"
    assert resp.content == pcm_for(sent["input"][::-1])
    kyutai_calls = len(fakes.kyutai.inputs)
    whole = bridge.post("/v1/tts/message", headers=AUTH, json={"text": text, "voice": "pocket:loutre"})
    assert whole.status_code == 200 and whole.content.startswith(b"ID3pocket")
    assert fakes.pocket.inputs[-1]["response_format"] == "mp3"
    assert len(fakes.kyutai.inputs) == kyutai_calls  # Kyutai untouched
    assert bridge.get("/health").json()["pocket"] is True


def test_pocket_down_falls_back_to_kyutai_default_voice(bridge, fakes):
    fakes.pocket.status = 503
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Pocket est en panne.", "voice": "pocket:lutin"})
    assert resp.status_code == 200
    assert fakes.kyutai.inputs[-1]["voice"] == "5476"
    assert resp.content == pcm_for(fakes.kyutai.inputs[-1]["input"])
    sentence = bridge.post("/v1/tts/sentence", headers=AUTH, json={"text": "Toujours en panne.", "voice": "pocket:lutin"})
    assert sentence.status_code == 200 and fakes.kyutai.inputs[-1]["voice"] == "5476"
    assert bridge.get("/health").json()["pocket"] is False
    # The fallback audio is not cached under the Pocket voice: once Pocket is back, it is used.
    fakes.pocket.status = 200
    again = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Pocket est en panne.", "voice": "pocket:lutin"})
    assert fakes.pocket.inputs[-1]["voice"] == "lutin" and again.content != resp.content


def test_numbers_are_spelled_before_synthesis(bridge, fakes):
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Ta moyenne : **91** bpm à 14h30.", "voice": "pocket:loutre"})
    assert resp.status_code == 200
    assert fakes.pocket.inputs[-1]["input"] == "Ta moyenne : quatre-vingt-onze battements par minute à quatorze heures trente."


def test_pocket_hiccup_is_retried_before_falling_back(bridge, fakes):
    fakes.pocket.statuses = [503]
    kyutai_calls = len(fakes.kyutai.inputs)
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Un petit hoquet.", "voice": "pocket:ours"})
    assert resp.status_code == 200
    assert resp.content == pcm_for(fakes.pocket.inputs[-1]["input"][::-1])
    assert len(fakes.pocket.inputs) == 2 and len(fakes.kyutai.inputs) == kyutai_calls


def test_pocket_language_voices_get_their_language(bridge, fakes):
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "You spent **$420.50** at 14:30.", "voice": "pocket:en/loutre"})
    assert resp.status_code == 200
    sent = fakes.pocket.inputs[-1]
    assert sent["voice"] == "en/loutre"
    assert sent["input"] == "You spent four hundred and twenty dollars and fifty cents at fourteen thirty."
    bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Am 3. Oktober um 23:15 Uhr.", "voice": "pocket:de/ours"})
    assert fakes.pocket.inputs[-1]["input"] == "Am dritten Oktober um dreiundzwanzig Uhr fünfzehn."


def test_pocket_down_fails_for_other_languages_instead_of_a_french_voice(bridge, fakes):
    fakes.pocket.status = 503
    kyutai_calls = len(fakes.kyutai.inputs)
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Hola, ¿qué tal?", "voice": "pocket:es/colibri"})
    assert resp.status_code >= 500
    assert len(fakes.kyutai.inputs) == kyutai_calls


def test_unknown_pocket_language_voice_is_reported_as_unavailable(bridge, fakes):
    fakes.pocket.status = 404
    resp = bridge.post("/v1/tts/stream", headers=AUTH, json={"text": "Hallo!", "voice": "pocket:de/ours"})
    assert resp.status_code >= 500  # the app switches to the iPhone's voice on 5xx only
