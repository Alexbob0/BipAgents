"""The person's phone calendar shared with the agents they pick (`/v1/calendar…`) and the calendar tools over MCP,
listed only for those agents. Rust only: skipped when `/health` does not list the feature (the Python bridge)."""
import os

import pytest

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from conftest import managed_install, profile, wait_registered

TOKEN = "coach-token-" + "k" * 30
COPY = {"from": "2026-10-14", "to": "2026-12-20",
        "calendars": [{"id": "C1", "title": "Perso", "color": "#3CC37A"}, {"id": "C2", "title": "Travail"}],
        "events": [
            {"id": "e1", "calendar": "C1", "title": "Dentiste", "start": "2026-10-22T10:00:00+02:00",
             "end": "2026-10-22T10:30:00+02:00", "all_day": False, "location": "12 rue Haute"},
            {"id": "e2", "calendar": "C2", "title": "Salon", "start": "2026-10-23", "end": "2026-10-25",
             "all_day": True, "notes": "Stand B4"},
            {"id": "e3", "calendar": "C2", "title": "Point équipe", "start": "2026-10-21T09:00:00+02:00",
             "end": "2026-10-21T09:30:00+02:00", "recurring": True}]}


def coach(bridges, fakes):
    bridge = bridges.new()
    profile(managed_install(bridge, fakes, agents=("coach",)), "coach")
    bridge.start()
    if "calendar" not in bridge.get("/health").json().get("features", []):
        pytest.skip("this bridge has no calendar")
    wait_registered(bridge, ["coach"])
    give_token(bridge, "coach", TOKEN)
    return bridge


def names(bridge):
    return [t["name"] for t in rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]]


def test_the_phone_shares_its_calendar_and_does_what_the_agents_ask(bridges, fakes):
    bridge = coach(bridges, fakes)
    assert bridge.get("/v1/calendar").status_code == 401
    assert not any(n.startswith("calendar_") for n in names(bridge))
    not_yet = call(bridge, "calendar_list", {}, token=TOKEN)
    assert not_yet["isError"] and "hasn't shared their phone calendar" in text_of(not_yet)
    unknown = bridge.put("/v1/calendar/agents", json={"agents": ["ghost"]}, headers=AUTH)
    assert unknown.status_code == 400 and unknown.json() == {"detail": "unknown agent"}
    assert bridge.put("/v1/calendar/agents", json={"agents": ["Coach"]}, headers=AUTH).json() == {"agents": ["coach"]}
    give_token(bridge, "coach", TOKEN)   # its tools' entry was written again (a new version), with a token of its own
    assert [n for n in names(bridge) if n.startswith("calendar_")] == [
        "calendar_list", "calendar_add", "calendar_update", "calendar_delete"]

    assert bridge.put("/v1/calendar", json=COPY, headers=AUTH).json() == {"events": 3}
    listed = text_of(call(bridge, "calendar_list", {"from": "2026-10-21", "to": "2026-10-24"}, token=TOKEN))
    assert "- id e1 · Perso · 2026-10-22 10:00–10:30 · Dentiste · at 12 rue Haute" in listed
    assert "- id e2 · Travail · 2026-10-23 → 2026-10-25 all day · Salon" in listed and "Stand B4" in listed
    assert "· repeats" in listed and "for 2026-10-14 to 2026-12-20" in listed
    work = text_of(call(bridge, "calendar_list", {"from": "2026-10-21", "to": "2026-10-30", "calendar": "perso"},
                        token=TOKEN))
    assert "Dentiste" in work and "Salon" not in work

    asked = call(bridge, "calendar_add", {"title": "Banque", "start": "2026-10-30T10:00", "duration_minutes": 30,
                                          "calendar": "Perso"}, token=TOKEN)
    assert not asked["isError"] and "next sync" in text_of(asked)
    assert call(bridge, "calendar_add", {"title": "x", "start": "plus tard"}, token=TOKEN)["isError"]
    assert call(bridge, "calendar_delete", {"id": "zz"}, token=TOKEN)["isError"]
    assert "repeating" in text_of(call(bridge, "calendar_delete", {"id": "e3"}, token=TOKEN))
    assert not call(bridge, "calendar_update", {"id": "e1", "title": "Dentiste (Dr Roy)"}, token=TOKEN)["isError"]
    queue = bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"]
    assert [op["op"] for op in queue] == ["add", "update"]
    assert set(queue[0]) == {"id", "op", "args", "agent", "created"} and queue[0]["agent"] == "coach"
    assert queue[0]["args"] == {"title": "Banque", "start": "2026-10-30T10:00:00", "end": "2026-10-30T10:30:00",
                                "all_day": False, "calendar": "Perso", "location": "", "notes": ""}
    assert queue[1]["args"] == {"id": "e1", "title": "Dentiste (Dr Roy)"}
    assert bridge.post(f"/v1/calendar/queue/{queue[0]['id']}", json={"done": True}, headers=AUTH).json() == {"ok": True}
    assert bridge.post("/v1/calendar/queue/nope", json={"done": True}, headers=AUTH).status_code == 404
    seen = bridge.get("/v1/calendar", headers=AUTH).json()
    assert seen["agents"] == ["coach"] and seen["events"] == 3 and seen["calendars"] == 2 and seen["pending"] == 1
    assert isinstance(seen["at"], float) and seen["from"] == "2026-10-14"
    assert oct(os.stat(bridge.state_dir / "calendar.json").st_mode & 0o777) == "0o600"

    assert bridge.put("/v1/calendar/agents", json={"agents": []}, headers=AUTH).json() == {"agents": []}
    give_token(bridge, "coach", TOKEN)
    assert not any(n.startswith("calendar_") for n in names(bridge))
    assert call(bridge, "calendar_list", {}, token=TOKEN)["isError"]


def test_the_copy_is_checked(bridges, fakes):
    bridge = coach(bridges, fakes)
    assert bridge.put("/v1/calendar", json={"events": [{"id": "x" * 301, "start": "2026-10-22"}]},
                      headers=AUTH).status_code == 422
    assert bridge.put("/v1/calendar", json={"events": [{"id": "e"}]}, headers=AUTH).status_code == 422   # no start
    assert bridge.put("/v1/calendar", json={"events": [{}] * 5001}, headers=AUTH).status_code == 422
