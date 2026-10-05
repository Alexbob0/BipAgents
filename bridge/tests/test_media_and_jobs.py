import os

from bipbridge.media import extract_media, resolve
from conftest import AUTH


def test_extract_and_resolve_media(tmp_path):
    text = "MEDIA:/home/hermes/simplex-files/point.mp3\n\n🎙️ Point du matin — lundi 5 octobre"
    assert extract_media(text) == ("🎙️ Point du matin — lundi 5 octobre", ["/home/hermes/simplex-files/point.mp3"])
    root = tmp_path / "simplex"
    root.mkdir()
    (root / "point.mp3").write_bytes(b"ID3podcast")
    (tmp_path / "secret.mp3").write_bytes(b"no")
    roots = {"/home/hermes/simplex-files": str(root)}
    assert resolve("/home/hermes/simplex-files/point.mp3", roots) == os.path.realpath(root / "point.mp3")
    assert resolve("/home/hermes/simplex-files/../secret.mp3", roots) is None  # traversal
    assert resolve("/home/hermes/simplex-files/missing.mp3", roots) is None
    assert resolve("/etc/passwd", roots) is None
    (root / "notes.sh").write_text("x")
    assert resolve("/home/hermes/simplex-files/notes.sh", roots) is None  # not media


def _client_with_media(config_dict, tmp_path, backend, apns):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app
    from bipbridge.config import parse_config
    root = tmp_path / "simplex"
    root.mkdir(exist_ok=True)
    (root / "point.mp3").write_bytes(b"ID3podcast-audio")
    config_dict = {**config_dict, "media": {"roots": {"/home/hermes/simplex-files": str(root)}}}
    return TestClient(create_app(parse_config(config_dict), transport=backend.transport(), apns_transport=apns.transport()))


def test_media_route_and_podcast_audio_in_the_outbox(config_dict, tmp_path, backend, apns):
    with _client_with_media(config_dict, tmp_path, backend, apns) as client:
        resp = client.get("/v1/media", params={"path": "/home/hermes/simplex-files/point.mp3"}, headers=AUTH)
        assert resp.status_code == 200 and resp.content == b"ID3podcast-audio"
        assert resp.headers["content-type"] == "audio/mpeg"
        assert client.get("/v1/media", params={"path": "/etc/passwd"}, headers=AUTH).status_code == 404
        services = client.app.state.services
        agent = services.config.agent("wellness")
        message = {"message": "MEDIA:/home/hermes/simplex-files/point.mp3\n\nPoint du matin", "id": "n1"}
        item = client.portal.call(services.outbox.ingest, agent, message)
        assert item["text"] == "Point du matin"
        audio = client.get(f"/v1/outbox/{item['id']}/audio", headers=AUTH)
        assert audio.status_code == 200 and audio.content == b"ID3podcast-audio"  # the podcast, not a synthesis
        assert backend.kyutai_inputs == [] and backend.pocket_inputs == []


def test_cron_job_notification_preference(client):
    store = client.app.state.services.store
    assert client.portal.call(store.note_cron_job, "wellness", "fc344ed09026", "Scan des e-mails") is True
    jobs = client.get("/v1/cron-jobs", headers=AUTH).json()["jobs"]
    assert [(j["job"], j["name"], j["notify"]) for j in jobs] == [("fc344ed09026", "Scan des e-mails", True)]
    resp = client.put("/v1/cron-jobs/wellness/fc344ed09026", headers=AUTH, json={"notify": False})
    assert resp.status_code == 200
    assert client.portal.call(store.note_cron_job, "wellness", "fc344ed09026", None) is False  # muted, name kept
    assert client.get("/v1/cron-jobs?agent=wellness", headers=AUTH).json()["jobs"][0]["name"] == "Scan des e-mails"
    assert client.put("/v1/cron-jobs/wellness/nope", headers=AUTH, json={"notify": False}).status_code == 404


def test_muted_job_reply_is_stored_without_push(client):
    import time
    assert client.post("/v1/devices", headers=AUTH, json={"token": "ab" * 32, "environment": "sandbox",
                                                          "agent_ids": ["wellness"]}).status_code == 200
    services = client.app.state.services
    agent = services.config.agent("wellness")
    item = client.portal.call(lambda: services.outbox.ingest(agent, {"message": "Tri des e-mails fait.", "id": "s1"}, notify=False))
    time.sleep(0.5)
    assert item is not None and client.apns.requests == []
    assert any(row["id"] == item["id"] for row in client.get("/v1/outbox", headers=AUTH).json()["items"])
