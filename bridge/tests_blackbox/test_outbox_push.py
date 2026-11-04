"""Boîte messages (outbox): ingestion, audio pre-synthesis, MESSAGE push, outbox routes (tests/test_outbox_push.py,
over HTTP). The messages are routines' reports, the way the bridge's own producers fill the Boîte."""
from bbkit.common import AUTH, boite_message, in_session
from bbkit.servers import wait_until

TOKEN = "b" * 64
DEAD = "d" * 64
NOW = "2026-10-21T08:00:00.000Z"


def devices_before_start(bridge):
    """Devices registered before the messages flow: put straight in the database (the Python bridge's schema)."""
    bridge.seed_db([
        ("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
         (TOKEN, "sandbox", '["wellness"]', NOW, NOW)),
        ("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
         (DEAD, "production", "[]", NOW, NOW)),
    ])


def test_a_message_lands_in_outbox_with_audio_and_push(bridges, fakes):
    bridge = bridges.new()
    devices_before_start(bridge)
    fakes.apns.responses[DEAD] = (410, "Unregistered")
    bridge.start()
    boite_message(bridge, fakes, "Bonjour **Sam**, as-tu bien dormi ?", name="Check-in")
    boite_message(bridge, fakes, "Pense à boire de l'eau.", name="Eau")
    apns = fakes.apns
    # the first -> TOKEN + DEAD (410, then deleted), the second -> TOKEN (+ DEAD if it raced the deletion)
    assert wait_until(lambda: len([r for r in apns.pushes() if apns.token(r) == TOKEN]) >= 2, timeout=15)

    items = bridge.get("/v1/outbox", headers=AUTH).json()["items"]
    assert [i["text"] for i in items] == ["Bonjour **Sam**, as-tu bien dormi ?", "Pense à boire de l'eau."]
    first = items[0]
    session = first["session_id"]
    assert first["agent"] == "wellness" and first["title"] == "Check-in"
    assert session and first["has_audio"] is True
    assert first["created_at"].endswith("Z") and first["sent_at"] is None
    assert first["audio_url"] == f"/v1/outbox/{first['id']}/audio" and len(first["id"]) == 32
    assert set(first) == {"id", "agent", "title", "text", "created_at", "sent_at", "session_id", "has_audio", "audio_url",
                          "attachments"}
    assert items[1]["title"] == "Eau" and items[1]["session_id"] != session

    # Audio was pre-synthesized as mp3 from the normalized text.
    mp3_calls = [b for b in fakes.pocket.inputs if b["response_format"] == "mp3"]
    assert mp3_calls[0]["input"] == "Bonjour Sam, as-tu bien dormi ?" and mp3_calls[0]["voice"] == "colibri"
    audio = bridge.get(f"/v1/outbox/{first['id']}/audio", headers=AUTH)
    assert audio.status_code == 200 and audio.headers["content-type"] == "audio/mpeg"
    assert audio.content == b"ID3Bonjour Sam, as-tu bien dormi ?"
    assert audio.headers["content-disposition"] == f'attachment; filename="{first["id"]}.mp3"'
    assert (bridge.data / "audio" / f"{first['id']}.mp3").exists()

    # Push: MESSAGE payload, sandbox host for the sandbox device, generic body (no preview).
    sandbox = [r for r in apns.pushes() if apns.environment(r) == "sandbox" and r.json()["m"] == f"o_{first['id']}"]
    payload = sandbox[0].json()
    # the pointer (docs/conversations.md §9.3): the journal's row, the conversation of the message's session
    n, c = payload.pop("n"), payload.pop("c")
    assert n.startswith("n_") and in_session(bridge, {"c": c}, session)
    badge = payload["aps"].pop("badge")
    assert badge in (1, 2)   # the journal's unread rows when it was sent (the other message may come first)
    assert payload == {"aps": {"alert": {"loc-key": "notif.boite", "body": "notif.boite"},
                               "thread-id": f"wellness/{c}", "mutable-content": 1, "category": "MESSAGE",
                               "sound": "default"},
                       "v": 2, "t": "boite", "agent": "wellness", "m": f"o_{first['id']}", "r": first["id"]}
    assert sandbox[0].header("apns-topic") == "io.github.bipagents"
    assert sandbox[0].header("apns-push-type") == "alert" and sandbox[0].header("apns-priority") == "10"
    assert sandbox[0].header("content-type") == "application/json"
    assert sandbox[0].path == f"/sandbox/3/device/{TOKEN}"
    assert sandbox[0].body.startswith(b'{"aps":{"alert":')   # compact JSON, aps first
    # The 410 token was deleted after its first failure; it was sent to the production host.
    dead_calls = [r for r in apns.pushes() if apns.token(r) == DEAD]
    assert 1 <= len(dead_calls) <= 2 and apns.environment(dead_calls[0]) == "production"
    with bridge.db() as db:
        tokens = [row[0] for row in db.execute("SELECT token FROM devices")]
        stored = db.execute("SELECT agent, ntfy_id, title, session_id, sent_at FROM outbox ORDER BY created_at").fetchall()
    assert tokens == [TOKEN]
    assert stored[0][0] == "wellness" and stored[0][1].startswith("routine:") and stored[0][2:] == ("Check-in", session, None)

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


def test_no_words_in_clear_even_with_the_old_previews_setting(bridges, fakes):
    # `[push] previews` is gone (sealed notifications): a phone without a key gets the neutral line only
    bridge = bridges.new(change=lambda c: c.update(push={"previews": True}))
    devices_before_start(bridge)
    bridge.start()
    boite_message(bridge, fakes, "Un message assez long pour être coupé. " * 10)
    assert wait_until(lambda: [r for r in fakes.apns.pushes() if fakes.apns.token(r) == TOKEN], timeout=15)
    alert = [r for r in fakes.apns.pushes() if fakes.apns.token(r) == TOKEN][0].json()["aps"]["alert"]
    assert alert == {"loc-key": "notif.boite", "body": "notif.boite"}


def test_outbox_audio_too_long_or_failing(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["outbox"].update(audio_max_chars=20))
    fakes.pocket.status = 500
    bridge.start()
    long_item = boite_message(bridge, fakes, "Un texte bien trop long pour être lu à voix haute.")
    short_item = boite_message(bridge, fakes, "Court.")
    assert long_item["has_audio"] is False
    none = bridge.get(f"/v1/outbox/{long_item['id']}/audio", headers=AUTH)
    assert none.status_code == 404 and none.json() == {"detail": "no audio for this message"}
    failed = bridge.get(f"/v1/outbox/{short_item['id']}/audio", headers=AUTH)
    assert failed.status_code == 503 and failed.json() == {"detail": "audio synthesis failed"}
    fakes.pocket.status = 200
    ok = bridge.get(f"/v1/outbox/{short_item['id']}/audio", headers=AUTH)
    assert ok.status_code == 200 and ok.content == b"ID3Court."


def test_the_journal_carries_the_boite_message_whole(bridge, fakes):
    """The apps read the Boîte from the notification journal alone: a `boite` row has its message's title, whole
    text, attachments and audio (the « Écouter » button)."""
    long = "Le point du matin. " + "Une phrase de plus pour dépasser l'aperçu. " * 8
    item = boite_message(bridge, fakes, long.strip(), name="Point du matin")
    assert wait_until(lambda: bridge.get(f"/v1/outbox/{item['id']}/audio", headers=AUTH).status_code == 200, timeout=20)
    rows = bridge.get("/v1/notifications", headers=AUTH, params={"agent": "wellness"}).json()["notifications"]
    [row] = [r for r in rows if r["type"] == "boite" and r["ref"] == item["id"]]
    assert row["boite"] == {"title": "Point du matin", "text": long.strip(), "attachments": [], "has_audio": True,
                            "audio_url": f"/v1/outbox/{item['id']}/audio"}
    one = bridge.get(f"/v1/notifications/{row['id']}", headers=AUTH).json()
    assert one["boite"] == row["boite"]
    assert all("boite" not in r for r in rows if r["type"] != "boite")
