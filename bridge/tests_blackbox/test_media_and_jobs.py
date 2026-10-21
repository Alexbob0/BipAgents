"""MEDIA: files (`/v1/media`), a podcast's own mp3 as an outbox message's audio, the scheduled tasks' push setting
(tests/test_media_and_jobs.py, over HTTP)."""
import json

from bbkit.common import AUTH
from bbkit.servers import wait_until

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


def test_media_route_and_podcast_audio_in_the_outbox(bridges, fakes):
    bridge = with_media(bridges)
    fakes.ntfy.lines["hermes-wellness-out"] = [json.dumps({
        "id": "n1", "event": "message", "message": "MEDIA:/home/hermes/simplex-files/point.mp3\n\nPoint du matin"})]
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
    assert wait_until(lambda: bridge.get("/v1/outbox", headers=AUTH).json()["items"])
    item = bridge.get("/v1/outbox", headers=AUTH).json()["items"][0]
    assert item["text"] == "Point du matin" and item["has_audio"] is True
    audio = bridge.get(f"/v1/outbox/{item['id']}/audio", headers=AUTH)
    assert audio.status_code == 200 and audio.content == b"ID3podcast-audio"  # the podcast, not a synthesis
    assert fakes.kyutai.inputs == [] and fakes.pocket.inputs == []


def test_a_media_only_message_reads_as_headphones(bridges, fakes):
    bridge = with_media(bridges)
    fakes.ntfy.lines["hermes-wellness-out"] = [json.dumps({
        "id": "n2", "event": "message", "message": "MEDIA:/home/hermes/simplex-files/point.mp3"})]
    bridge.start()
    assert wait_until(lambda: bridge.get("/v1/outbox", headers=AUTH).json()["items"])
    assert bridge.get("/v1/outbox", headers=AUTH).json()["items"][0]["text"] == "🎧"


def test_cron_job_notification_preference(bridges):
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO cron_jobs (agent, job, name, notify, last_seen) VALUES (?,?,?,?,?)",
                     ("wellness", "fc344ed09026", "Scan des e-mails", 1, NOW)),
                    ("INSERT INTO cron_jobs (agent, job, name, notify, last_seen) VALUES (?,?,?,?,?)",
                     ("wellness", "0681ac73805e", None, 0, "2026-10-20T08:00:00.000Z"))])
    bridge.start()
    jobs = bridge.get("/v1/cron-jobs", headers=AUTH).json()["jobs"]
    assert jobs == [{"agent": "wellness", "job": "fc344ed09026", "name": "Scan des e-mails", "notify": True,
                     "last_seen": NOW},
                    {"agent": "wellness", "job": "0681ac73805e", "name": None, "notify": False,
                     "last_seen": "2026-10-20T08:00:00.000Z"}]          # the last seen first
    resp = bridge.put("/v1/cron-jobs/wellness/fc344ed09026", headers=AUTH, json={"notify": False})
    assert resp.status_code == 200 and resp.json() == {"agent": "wellness", "job": "fc344ed09026", "notify": False}
    with bridge.db() as db:
        assert db.execute("SELECT notify FROM cron_jobs WHERE job='fc344ed09026'").fetchone() == (0,)
    assert bridge.get("/v1/cron-jobs?agent=wellness", headers=AUTH).json()["jobs"][0]["name"] == "Scan des e-mails"
    assert bridge.get("/v1/cron-jobs?agent=ghost", headers=AUTH).status_code == 404
    unknown = bridge.put("/v1/cron-jobs/wellness/nope", headers=AUTH, json={"notify": False})
    assert unknown.status_code == 404 and unknown.json() == {"detail": "unknown job"}
    assert bridge.put("/v1/cron-jobs/ghost/fc344ed09026", headers=AUTH, json={"notify": True}).status_code == 404
    assert bridge.put("/v1/cron-jobs/wellness/fc344ed09026", headers=AUTH, json={}).status_code == 422
