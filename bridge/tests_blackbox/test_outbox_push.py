"""ntfy -> outbox ingestion, audio pre-synthesis, MESSAGE push, outbox routes, the ntfy cursor across a restart
(tests/test_outbox_push.py, over HTTP)."""
import json

from bbkit.common import AUTH, NTFY_TOKEN
from bbkit.servers import wait_until

TOKEN = "b" * 64
DEAD = "d" * 64
NOW = "2026-10-21T08:00:00.000Z"


def ntfy_message(msg_id, text, title="Check-in", tags=None, t=1790000000):
    return json.dumps({"id": msg_id, "time": t, "event": "message", "topic": "hermes-wellness-out",
                       "message": text, "title": title, "tags": tags or []})


def devices_before_start(bridge):
    """Devices registered before the messages flow: put straight in the database (the Python bridge's schema)."""
    bridge.seed_db([
        ("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
         (TOKEN, "sandbox", '["wellness"]', NOW, NOW)),
        ("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
         (DEAD, "production", "[]", NOW, NOW)),
    ])


def test_ntfy_message_lands_in_outbox_with_audio_and_push(bridges, fakes):
    bridge = bridges.new()
    devices_before_start(bridge)
    fakes.apns.responses[DEAD] = (410, "Unregistered")
    fakes.ntfy.lines["hermes-wellness-out"] = [
        json.dumps({"id": "k1", "event": "keepalive"}),
        ntfy_message("m1", "Bonjour **Sam**, as-tu bien dormi ?", tags=["session:sess_42"]),
        ntfy_message("m1", "Bonjour **Sam**, as-tu bien dormi ?"),  # duplicate delivery
        ntfy_message("m2", "Pense à boire de l'eau.", title=None),
    ]
    bridge.start()
    apns = fakes.apns
    # m1 -> TOKEN + DEAD (410, then deleted), m2 -> TOKEN (+ DEAD if it raced the deletion)
    assert wait_until(lambda: len([r for r in apns.pushes() if apns.token(r) == TOKEN]) >= 2, timeout=15)
    req = fakes.ntfy.subscriptions[0]
    assert req.path == "/hermes-wellness-out/json"
    assert req.header("authorization") == f"Bearer {NTFY_TOKEN}"
    assert "since" not in req.params

    items = bridge.get("/v1/outbox", headers=AUTH).json()["items"]
    assert [i["text"] for i in items] == ["Bonjour **Sam**, as-tu bien dormi ?", "Pense à boire de l'eau."]
    first = items[0]
    assert first["agent"] == "wellness" and first["title"] == "Check-in"
    assert first["session_id"] == "sess_42" and first["has_audio"] is True
    assert first["created_at"].endswith("Z") and first["sent_at"] == "2026-09-21T14:13:20.000Z"
    assert first["audio_url"] == f"/v1/outbox/{first['id']}/audio" and len(first["id"]) == 32
    assert set(first) == {"id", "agent", "title", "text", "created_at", "sent_at", "session_id", "has_audio", "audio_url"}
    assert items[1]["title"] is None and items[1]["session_id"] is None

    # Audio was pre-synthesized as mp3 from the normalized text.
    mp3_calls = [b for b in fakes.kyutai.inputs if b["response_format"] == "mp3"]
    assert mp3_calls[0]["input"] == "Bonjour Sam, as-tu bien dormi ?" and mp3_calls[0]["voice"] == "4193"
    audio = bridge.get(f"/v1/outbox/{first['id']}/audio", headers=AUTH)
    assert audio.status_code == 200 and audio.headers["content-type"] == "audio/mpeg"
    assert audio.content == b"ID3Bonjour Sam, as-tu bien dormi ?"
    assert audio.headers["content-disposition"] == f'attachment; filename="{first["id"]}.mp3"'
    assert (bridge.data / "audio" / f"{first['id']}.mp3").exists()

    # Push: MESSAGE payload, sandbox host for the sandbox device, generic body (no preview).
    sandbox = [r for r in apns.pushes() if apns.environment(r) == "sandbox"]
    payload = sandbox[0].json()
    assert payload == {"aps": {"alert": {"title": "Wellness", "body": "Nouveau message", "loc-key": "Nouveau message"},
                               "thread-id": "wellness", "mutable-content": 1, "category": "MESSAGE",
                               "sound": "default"},
                       "outbox_id": first["id"], "agent": "wellness", "session_id": "sess_42"}
    assert sandbox[0].header("apns-topic") == "io.github.bipagents"
    assert sandbox[0].header("apns-push-type") == "alert" and sandbox[0].header("apns-priority") == "10"
    assert sandbox[0].header("content-type") == "application/json"
    assert sandbox[0].path == f"/sandbox/3/device/{TOKEN}"
    assert sandbox[0].body == json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode()
    # The 410 token was deleted after its first failure; it was sent to the production host.
    dead_calls = [r for r in apns.pushes() if apns.token(r) == DEAD]
    assert 1 <= len(dead_calls) <= 2 and apns.environment(dead_calls[0]) == "production"
    with bridge.db() as db:
        tokens = [row[0] for row in db.execute("SELECT token FROM devices")]
        cursor = db.execute("SELECT agent, last_id FROM ntfy_cursor").fetchall()
        stored = db.execute("SELECT agent, ntfy_id, title, session_id, sent_at FROM outbox ORDER BY created_at").fetchall()
    assert tokens == [TOKEN]
    assert cursor == [("wellness", "m2")]
    assert stored[0] == ("wellness", "m1", "Check-in", "sess_42", "2026-09-21T14:13:20.000Z")

    # Single item + since filtering + agent filter
    one = bridge.get(f"/v1/outbox/{first['id']}", headers=AUTH).json()
    assert one == first
    later = bridge.get("/v1/outbox", headers=AUTH, params={"since": first["created_at"]}).json()["items"]
    assert [i["text"] for i in later] == ["Pense à boire de l'eau."]
    assert bridge.get("/v1/outbox", headers=AUTH, params={"since": "1"}).json()["items"] == items   # Unix seconds
    assert len(bridge.get("/v1/outbox", headers=AUTH, params={"limit": 1}).json()["items"]) == 1
    assert bridge.get("/v1/outbox", headers=AUTH, params={"agent": "Wellness"}).json()["items"] == items
    ghost = bridge.get("/v1/outbox", headers=AUTH, params={"agent": "ghost"})
    assert ghost.status_code == 404 and ghost.json() == {"detail": "unknown agent"}
    hier = bridge.get("/v1/outbox", headers=AUTH, params={"since": "hier"})
    assert hier.status_code == 400 and hier.json() == {"detail": "invalid since (ISO 8601 expected)"}
    assert bridge.get("/v1/outbox", headers=AUTH, params={"limit": 0}).status_code == 422
    assert bridge.get("/v1/outbox", headers=AUTH, params={"limit": 501}).status_code == 422
    nope = bridge.get("/v1/outbox/nope", headers=AUTH)
    assert nope.status_code == 404 and nope.json() == {"detail": "not found"}
    assert bridge.get("/v1/outbox/nope/audio", headers=AUTH).status_code == 404
    listing = bridge.get("/v1/outbox", headers=AUTH).json()
    assert listing["server_time"].endswith("Z") and len(listing["server_time"]) == 24

    # Cursor persisted: a restart resumes with since=<last id>.
    fakes.ntfy.subscriptions.clear()
    bridge.restart()
    assert wait_until(lambda: fakes.ntfy.subscriptions, timeout=10)
    assert fakes.ntfy.subscriptions[0].params["since"] == "m2"


def test_push_previews_put_the_text_in_the_alert(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(push={"previews": True}))
    devices_before_start(bridge)
    long = "Un message assez long pour être coupé. " * 10
    fakes.ntfy.lines["hermes-wellness-out"] = [ntfy_message("p1", long)]
    bridge.start()
    assert wait_until(lambda: [r for r in fakes.apns.pushes() if fakes.apns.token(r) == TOKEN], timeout=15)
    alert = [r for r in fakes.apns.pushes() if fakes.apns.token(r) == TOKEN][0].json()["aps"]["alert"]
    preview = " ".join(long.split())
    assert alert == {"title": "Wellness", "body": preview[:159].rstrip() + "…"}


def test_outbox_audio_too_long_or_failing(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["outbox"].update(audio_max_chars=20))
    fakes.ntfy.lines["hermes-wellness-out"] = [ntfy_message("l1", "Un texte bien trop long pour être lu à voix haute."),
                                                ntfy_message("s1", "Court.")]
    fakes.kyutai.status = 500
    bridge.start()
    assert wait_until(lambda: len(bridge.get("/v1/outbox", headers=AUTH).json()["items"]) == 2)
    long_item, short_item = bridge.get("/v1/outbox", headers=AUTH).json()["items"]
    assert long_item["has_audio"] is False
    none = bridge.get(f"/v1/outbox/{long_item['id']}/audio", headers=AUTH)
    assert none.status_code == 404 and none.json() == {"detail": "no audio for this message"}
    failed = bridge.get(f"/v1/outbox/{short_item['id']}/audio", headers=AUTH)
    assert failed.status_code == 503 and failed.json() == {"detail": "audio synthesis failed"}
    fakes.kyutai.status = 200
    ok = bridge.get(f"/v1/outbox/{short_item['id']}/audio", headers=AUTH)
    assert ok.status_code == 200 and ok.content == b"ID3Court."


def test_cron_wrapped_ntfy_deliveries_keep_the_job_name_and_output(bridges, fakes):
    """ntfy.unwrap_cron: Hermes' « Cronjob Response » header and footer notes are taken off."""
    bridge = bridges.new()
    wrapped = ("Cronjob Response: Test push propre (job_id: 1a2b3c4d5e6f)\n-------------\n\nCoucou, test réussi.\n\n"
               'To stop or manage this job, send me a new message (e.g. "stop reminder Test push propre").'
               "\n\nNote: The agent cannot see this message, and therefore cannot respond to it.")
    fakes.ntfy.lines["hermes-wellness-out"] = [
        json.dumps({"id": "c1", "event": "message", "message": wrapped}),
        json.dumps({"id": "c2", "event": "message", "message": "Cronjob Response: Morning feeds\n-------------\n\nLe point.",
                    "title": "Titre ntfy"}),
        json.dumps({"id": "c3", "event": "message", "message": "   "}),                 # nothing to keep
        json.dumps({"id": "c4", "event": "open"}),
        "not json at all",
    ]
    bridge.start()
    assert wait_until(lambda: len(bridge.get("/v1/outbox", headers=AUTH).json()["items"]) == 2)
    items = bridge.get("/v1/outbox", headers=AUTH).json()["items"]
    assert [(i["title"], i["text"]) for i in items] == [("Test push propre", "Coucou, test réussi."),
                                                         ("Titre ntfy", "Le point.")]
    # The cursor follows the messages only (not the « open » or keepalive events, nor lines that are not JSON).
    assert wait_until(lambda: (bridge.db().execute("SELECT last_id FROM ntfy_cursor").fetchone() or [""])[0] == "c3")


def test_basic_auth_ntfy_tokens(bridges, fakes):
    """A « user:password » ntfy token is sent as Basic auth (ntfy.auth_header)."""
    bridge = bridges.new(change=lambda c: c["agents"]["Wellness"].update(ntfy_token="alex:s3cret"))
    bridge.start()
    assert wait_until(lambda: fakes.ntfy.subscriptions, timeout=10)
    assert fakes.ntfy.subscriptions[0].header("authorization") == "Basic YWxleDpzM2NyZXQ="
