import json

from bipbridge import place_backup
from conftest import AUTH


def test_the_app_gets_the_key_and_when_the_last_backup_worked(client, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    assert client.get("/v1/backup", headers=AUTH).json() == {"enabled": False}
    assert client.get("/v1/backup").status_code == 401
    (tmp_path / place_backup.SETTINGS).parent.mkdir(parents=True)
    (tmp_path / place_backup.SETTINGS).write_text(json.dumps({"url": "https://g.test/v1/backup/", "token": "t",
                                                              "password": "the-key"}))
    assert client.get("/v1/backup", headers=AUTH).json() == {"enabled": True, "key": "the-key",
                                                              "allowance_bytes": place_backup.ALLOWANCE_BYTES}
    (tmp_path / place_backup.STATE).parent.mkdir(parents=True)
    (tmp_path / place_backup.STATE).write_text(json.dumps({"last_ok": 1790000000.0, "snapshot": "abc"}))
    status = client.get("/v1/backup", headers=AUTH).json()
    assert status == {"enabled": True, "key": "the-key", "last_ok": 1790000000.0,   # the token never leaves
                      "allowance_bytes": place_backup.ALLOWANCE_BYTES}


def test_a_place_with_its_5_gb_used_up_refuses_files_and_notes(client, tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    (tmp_path / place_backup.SETTINGS).parent.mkdir(parents=True)
    (tmp_path / place_backup.SETTINGS).write_text(json.dumps({"url": "https://g.test/v1/backup/", "token": "t",
                                                              "password": "the-key"}))
    (tmp_path / place_backup.STATE).parent.mkdir(parents=True)
    (tmp_path / place_backup.STATE).write_text(json.dumps({"last_ok": 1790000000.0, "bytes": 1_288_490_189}))
    assert client.get("/v1/backup", headers=AUTH).json()["used_bytes"] == 1_288_490_189
    assert not place_backup.storage_full(tmp_path)
    (tmp_path / place_backup.STATE).write_text(json.dumps({"last_ok": 1790000000.0, "bytes": place_backup.ALLOWANCE_BYTES}))
    assert place_backup.storage_full(tmp_path)
    full = client.post("/v1/files", headers=AUTH, data={"agent": "wellness"}, files={"file": ("a.pdf", b"%PDF-1.4", "application/pdf")})
    assert full.status_code == 507 and "5 GB" in full.json()["detail"]
