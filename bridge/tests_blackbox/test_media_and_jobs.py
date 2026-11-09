"""MEDIA: files (`/v1/media`), a podcast in a Boîte message (its file, no reading on top), the scheduled tasks' push setting
(tests/test_media_and_jobs.py, over HTTP)."""

from bbkit.common import AUTH, boite_message

NOW = "2026-10-21T08:00:00.000Z"


def with_media(bridges):
    bridge = bridges.new()
    root = bridge.root / "simplex"
    root.mkdir(exist_ok=True)
    (root / "point.mp3").write_bytes(b"ID3podcast-audio")
    (root / "chart.png").write_bytes(b"\x89PNG\r\n\x1a\nchart")
    (root / "notes.sh").write_text("x")
    (bridge.root / "secret.mp3").write_bytes(b"no")
    (root / "link.mp3").symlink_to(bridge.root / "secret.mp3")
    bridge.config["media"] = {"roots": {"/home/hermes/simplex-files": str(root)}}
    return bridge


def test_media_route_and_a_podcast_in_the_outbox(bridges, fakes):
    bridge = with_media(bridges)
    bridge.start()
    resp = bridge.get("/v1/media", params={"path": "/home/hermes/simplex-files/point.mp3"}, headers=AUTH)
    assert resp.status_code == 200 and resp.content == b"ID3podcast-audio"
    assert resp.headers["content-type"] == "audio/mpeg" and resp.headers["x-content-type-options"] == "nosniff"
    assert resp.headers["content-disposition"] == 'attachment; filename="point.mp3"'
    image = bridge.get("/v1/media", params={"path": "/home/hermes/simplex-files/chart.png"}, headers=AUTH)
    assert image.status_code == 200 and image.headers["content-type"] == "image/png"
    for path in ("/etc/passwd", "/home/hermes/simplex-files/../secret.mp3", "/home/hermes/simplex-files/missing.mp3",
                 "/home/hermes/simplex-files/notes.sh", "/home/hermes/simplex-files/link.mp3", "/home/hermes/other/point.mp3"):
        missing = bridge.get("/v1/media", params={"path": path}, headers=AUTH)
        assert missing.status_code == 404 and missing.json() == {"detail": "not found"}, path
    assert bridge.get("/v1/media", headers=AUTH).status_code == 422
    assert bridge.get("/v1/media", params={"path": "x" * 1025}, headers=AUTH).status_code == 422
    assert bridge.get("/v1/media", params={"path": "/home/hermes/simplex-files/point.mp3"}).status_code == 401
    # a report bringing its podcast: the podcast is its one audio file, and no reading of its words is made on top
    # (Alex 8/11: a podcast in two audio files, the reading of the text and the podcast)
    item = boite_message(bridge, fakes, "MEDIA:/home/hermes/simplex-files/point.mp3\n\nPoint du matin")
    assert item["text"] == "Point du matin" and item["has_audio"] is False
    assert [(a["kind"], a["path"]) for a in item["attachments"]] == [("audio", "/home/hermes/simplex-files/point.mp3")]
    assert bridge.get(f"/v1/outbox/{item['id']}/audio", headers=AUTH).status_code == 404
    assert fakes.pocket.inputs == []


def test_a_media_only_message_reads_as_headphones(bridges, fakes):
    bridge = with_media(bridges)
    bridge.start()
    assert boite_message(bridge, fakes, "MEDIA:/home/hermes/simplex-files/point.mp3")["text"] == "🎧"


def test_cron_job_notification_preference(bridges):
    """Each routine's « Me prévenir » (routines.rs, the routines the bridge keeps), as the apps' list reads it."""
    bridge = bridges.new().start()

    def made(name, schedule):
        resp = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                           json={"name": name, "schedule": schedule, "prompt": f"{name}, comme d'habitude"})
        assert resp.status_code == 200, resp.text
        return resp.json()["job"]["id"]

    scan, tidy = made("Scan des e-mails", "0 8 * * *"), made("Rangement", "every 2h")
    jobs = {j["job"]: j for j in bridge.get("/v1/cron-jobs", headers=AUTH).json()["jobs"]}
    assert set(jobs) == {scan, tidy} and jobs[scan]["name"] == "Scan des e-mails" and jobs[scan]["notify"] is True
    resp = bridge.put(f"/v1/cron-jobs/wellness/{scan}", headers=AUTH, json={"notify": False})
    assert resp.status_code == 200 and resp.json() == {"agent": "wellness", "job": scan, "notify": False}
    listed = bridge.get("/v1/cron-jobs?agent=wellness", headers=AUTH).json()["jobs"]
    assert [j["notify"] for j in listed if j["job"] == scan] == [False]
    assert bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"][0]["muted"] is True  # one switch
    assert bridge.get("/v1/cron-jobs?agent=ghost", headers=AUTH).status_code == 404
    unknown = bridge.put("/v1/cron-jobs/wellness/nope", headers=AUTH, json={"notify": False})
    assert unknown.status_code == 404 and unknown.json() == {"detail": "unknown job"}
    assert bridge.put(f"/v1/cron-jobs/ghost/{scan}", headers=AUTH, json={"notify": True}).status_code == 404
    assert bridge.put(f"/v1/cron-jobs/wellness/{scan}", headers=AUTH, json={}).status_code == 422
