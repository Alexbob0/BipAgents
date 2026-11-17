"""The person's groups of agents (`PUT /v1/agents/{id}/group`, `group` in the listing and at creation) and how long
the files sent to the agents are kept (`/v1/files/settings`, used by the purge). `/health` lists both features."""
import json
import os
import time

import pytest

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install

FIVE_GB = 5 * (1 << 30)


def offers(bridge, feature):
    assert feature in bridge.get("/health").json().get("features", []), f"this bridge has no {feature}"


def groups_of(bridge):
    return {a["id"]: a["group"] for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]}


def test_an_agent_moved_to_a_group_and_out_of_it(bridge):
    offers(bridge, "agent_groups")
    assert groups_of(bridge) == {"wellness": ""}
    moved = bridge.put("/v1/agents/wellness/group", headers=AUTH, json={"group": "  Perso   santé "})
    assert moved.status_code == 200 and moved.json() == {"id": "wellness", "group": "Perso santé"}
    assert groups_of(bridge) == {"wellness": "Perso santé"}
    bridge.restart()
    assert groups_of(bridge) == {"wellness": "Perso santé"}   # kept
    assert bridge.put("/v1/agents/wellness/group", headers=AUTH, json={"group": ""}).json()["group"] == ""
    assert groups_of(bridge) == {"wellness": ""}


def test_group_names_checked(bridge):
    offers(bridge, "agent_groups")
    assert bridge.put("/v1/agents/nobody/group", headers=AUTH, json={"group": "Pro"}).status_code == 404
    assert bridge.put("/v1/agents/wellness/group", headers=AUTH, json={"group": "x" * 41}).status_code == 422
    assert bridge.put("/v1/agents/wellness/group", json={"group": "Pro"}).status_code in (401, 403)
    assert groups_of(bridge) == {"wellness": ""}


def test_a_new_agent_in_a_group_and_its_group_gone_with_it(bridges, fakes):
    bridge = bridges.new()
    managed_install(bridge, fakes, agents=(), public_hub_url="https://mini.ts.net:8642")
    bridge.start()
    offers(bridge, "agent_groups")
    resp = bridge.post("/v1/agents", headers=AUTH, json={"name": "Budget", "kind": "finance", "group": " Pro "})
    assert resp.status_code == 201, resp.text
    assert resp.json()["id"] == "budget" and resp.json()["group"] == "Pro"
    assert groups_of(bridge)["budget"] == "Pro"
    assert bridge.get("/v1/agents/budget/settings", headers=AUTH).json()["group"] == "Pro"
    long = bridge.post("/v1/agents", headers=AUTH, json={"name": "Autre", "kind": "finance", "group": "x" * 41})
    assert long.status_code == 422
    assert bridge.delete("/v1/agents/budget", headers=AUTH).status_code == 204
    assert "budget" not in (bridge.state_dir / "agent_groups.json").read_text()


def test_files_kept_thirty_days_by_default_then_the_persons_choice(bridge):
    offers(bridge, "files_kept")
    sent = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("a.txt", b"12345", "text/plain")})
    assert sent.status_code == 200, sent.text
    first = bridge.get("/v1/files/settings", headers=AUTH).json()
    assert first["kept_days"] == 30 and first["choices"] == [30, 365, 0], first
    assert "allowance_bytes" not in first, first   # a self-install: no 5 GB said
    assert first["used_bytes"] in (None, 5) and "measured_at" in first, first   # the place's last measure, if any
    assert bridge.post("/v1/storage/measure", headers=AUTH).status_code == 200
    measured = bridge.get("/v1/files/settings", headers=AUTH).json()
    assert measured["used_bytes"] == 5 and measured["measured_at"] > 0, measured
    for days in (365, 0):
        changed = bridge.patch("/v1/files/settings", headers=AUTH, json={"kept_days": days})
        assert changed.status_code == 200 and changed.json()["kept_days"] == days
    assert bridge.patch("/v1/files/settings", headers=AUTH, json={"kept_days": 12}).status_code == 422
    assert bridge.get("/v1/files/settings").status_code in (401, 403)
    bridge.restart()
    assert bridge.get("/v1/files/settings", headers=AUTH).json()["kept_days"] == 0


def test_files_settings_answer_from_the_last_measure_without_walking_the_uploads(bridge):
    """17/11 (Alex: « Garder les fichiers » vanished on a self-install): the bridge walked every uploaded file inside
    each GET and PATCH, longer than the app waits. It answers from the place's last measure now (storage_watch.rs)."""
    offers(bridge, "files_kept")
    sent = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                       files={"file": ("a.txt", b"12345", "text/plain")})
    assert sent.status_code == 200, sent.text
    assert bridge.post("/v1/storage/measure", headers=AUTH).status_code == 200
    month = bridge.root / "uploads" / "wellness" / "2026-11"
    month.mkdir(parents=True, exist_ok=True)
    (month / "0123456789ab-big.bin").write_bytes(b"\0" * 2_000_000)
    started = time.monotonic()
    got = bridge.get("/v1/files/settings", headers=AUTH).json()
    assert time.monotonic() - started < 2
    assert got["used_bytes"] == 5, got   # the measure's figure: nothing walked in the request
    changed = bridge.patch("/v1/files/settings", headers=AUTH, json={"kept_days": 365}).json()
    assert changed["kept_days"] == 365 and changed["used_bytes"] == 5, changed
    assert bridge.post("/v1/storage/measure", headers=AUTH).status_code == 200
    assert bridge.get("/v1/files/settings", headers=AUTH).json()["used_bytes"] == 2_000_005


def test_files_settings_say_the_5_gb_only_on_the_offer(bridge):
    offers(bridge, "files_kept")
    assert "allowance_bytes" not in bridge.get("/v1/files/settings", headers=AUTH).json()
    settings = bridge.home / ".config" / "bipagents" / "backup.json"   # a place of the offer (test_storage.py `offer`)
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(json.dumps({"url": "https://g.test/v1/backup/", "token": "t", "password": "the-key"}))
    assert bridge.get("/v1/files/settings", headers=AUTH).json()["allowance_bytes"] == FIVE_GB


@pytest.mark.slow
def test_the_purge_follows_the_choice(bridge):
    offers(bridge, "files_kept")
    month = bridge.root / "uploads" / "wellness" / "2026-01"
    month.mkdir(parents=True, exist_ok=True)
    old = month / "0123456789ab-old.txt"
    old.write_bytes(b"old")
    forty_days_ago = time.time() - 40 * 86400
    os.utime(old, (forty_days_ago, forty_days_ago))
    assert bridge.patch("/v1/files/settings", headers=AUTH, json={"kept_days": 0}).status_code == 200
    bridge.restart()
    time.sleep(8)   # the purge runs 5 s after the start
    assert old.exists()   # kept always
    assert bridge.patch("/v1/files/settings", headers=AUTH, json={"kept_days": 30}).status_code == 200
    bridge.restart()
    assert wait_until(lambda: not old.exists(), timeout=15)
