"""Boîte messages (outbox): ingestion, audio pre-synthesis, MESSAGE push, outbox routes (tests/test_outbox_push.py,
over HTTP). The messages are routines' reports, the way the bridge's own producers fill the Boîte."""
import time
from bbkit.common import AUTH, boite_message, in_session, boite_items
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

    items = boite_items(bridge)
    assert [i["text"] for i in items] == ["Bonjour **Sam**, as-tu bien dormi ?", "Pense à boire de l'eau."]
    first = items[0]
    session = first["session_id"]
    assert first["agent"] == "wellness" and first["title"] == "Check-in"
    assert session and first["has_audio"] is True
    assert first["created_at"].endswith("Z")
    assert first["audio_url"] == f"/v1/outbox/{first['id']}/audio" and len(first["id"]) == 32
    assert set(first) == {"id", "agent", "title", "text", "created_at", "session_id", "has_audio", "audio_url",
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

    assert bridge.get("/v1/outbox/nope/audio", headers=AUTH).status_code == 404
    assert bridge.get(f"/v1/outbox/{first['id']}", headers=AUTH).status_code == 404   # the journal has the message


def test_a_routine_report_is_read_on_demand_and_its_podcast_is_its_one_audio_file(bridges, fakes):
    """Alex 8/11: a routine's report came with an audio FILE instead of « Écouter », and a podcast in two audio files
    (the reading of its text and the podcast). A report's reading is its `has_audio` (« Écouter »), never one of its
    files; a report bringing its own audio (create_podcast's .m4a) gets no reading on top: one audio file, the podcast."""
    bridge = bridges.new().start()
    plain = boite_message(bridge, fakes, "Trois rendez-vous aujourd'hui, rien d'urgent.", name="Point du matin")
    assert plain["has_audio"] is True and plain["attachments"] == []
    podcast = "/bipagents/audio/wellness/20261109-070000-point-du-matin.m4a"
    item = boite_message(bridge, fakes, f"Ton podcast du matin est prêt.\nMEDIA:{podcast}", name="Podcast du matin")
    assert item["text"] == "Ton podcast du matin est prêt." and item["has_audio"] is False
    assert [(a["kind"], a["path"]) for a in item["attachments"]] == [("audio", podcast)]
    assert bridge.get(f"/v1/outbox/{item['id']}/audio", headers=AUTH).status_code == 404
    time.sleep(1.0)
    readings = [b["input"] for b in fakes.pocket.inputs if b["response_format"] == "mp3"]
    assert readings == ["Trois rendez-vous aujourd'hui, rien d'urgent."], readings


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


def test_a_muted_routine_leaves_the_journal_and_the_phone_alone(bridge, fakes):
    """Alex 5/11: a muted routine's report stays in its run's own conversation: no Boîte row in the journal, no push
    (the old « Me prévenir » off, which kept the row without a push, is the same switch now)."""
    bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "sandbox", "agent_ids": ["wellness"]})
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                       json={"name": "Discret", "schedule": "every day at 7:00", "prompt": "Fais le point."}).json()["job"]
    assert bridge.put(f"/v1/cron-jobs/wellness/{made['id']}", headers=AUTH, json={"notify": False}).status_code == 200
    fakes.hermes.run_output = "Rapport discret."
    assert bridge.post(f"/v1/routines/{made['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"][0]["last_status"]
                      == "muted", timeout=20)
    time.sleep(1.0)
    rows = bridge.get("/v1/notifications", headers=AUTH, params={"agent": "wellness"}).json()["notifications"]
    assert not [r for r in rows if r["type"] == "boite" and "Rapport discret." in str(r.get("boite"))]
    assert not fakes.apns.payloads()