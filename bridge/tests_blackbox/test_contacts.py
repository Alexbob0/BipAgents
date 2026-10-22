"""The person's address book shared with the agents they pick (`/v1/contacts…`): the copy the phone sends, the
person's choice (all or some groups, the notes or not), and the contact tools over MCP, listed only for those agents.
Rust only: skipped when `/health` does not list the feature (the Python bridge)."""
import datetime
import os

import pytest

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from conftest import managed_install, profile, wait_registered

TOKEN = "coach-token-" + "c" * 30


def soon(days, year):
    """A birthday `days` from today (YYYY-MM-DD, in `year`)."""
    return f"{year:04d}-" + (datetime.date.today() + datetime.timedelta(days=days)).strftime("%m-%d")


def copy():
    return {"groups": [{"id": "g1", "title": "Famille"}, {"id": "g2", "title": "Travail"}],
            "contacts": [
                {"id": "c1", "name": "Hélène Martin", "nickname": "Lili",
                 "phones": [{"label": "mobile", "value": "+33 6 12 34 56 78"}],
                 "emails": [{"value": "helene@example.org"}], "birthday": soon(2, 1990), "groups": ["g1"],
                 "notes": "aime les tulipes"},
                {"id": "c2", "name": "Paul Durand", "organization": "Acme", "groups": ["g2"]}]}


def coach(bridges, fakes):
    bridge = bridges.new()
    profile(managed_install(bridge, fakes, agents=("coach",)), "coach")
    bridge.start()
    if "contacts" not in bridge.get("/health").json().get("features", []):
        pytest.skip("this bridge has no contacts")
    wait_registered(bridge, ["coach"])
    give_token(bridge, "coach", TOKEN)
    return bridge


def names(bridge):
    return [t["name"] for t in rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]]


def test_the_phone_shares_its_address_book_and_does_what_the_agents_ask(bridges, fakes):
    bridge = coach(bridges, fakes)
    assert bridge.get("/v1/contacts").status_code == 401
    assert not any(n.startswith("contacts_") for n in names(bridge))
    not_yet = call(bridge, "contacts_search", {"query": "x"}, token=TOKEN)
    assert not_yet["isError"] and "hasn't shared their address book" in text_of(not_yet)
    assert bridge.get("/v1/contacts", headers=AUTH).json()["scope"] == {"all": True, "groups": [], "notes": False}
    unknown = bridge.put("/v1/contacts/agents", json={"agents": ["ghost"]}, headers=AUTH)
    assert unknown.status_code == 400 and unknown.json() == {"detail": "unknown agent"}
    assert bridge.put("/v1/contacts/agents", json={"agents": ["Coach"]}, headers=AUTH).json() == {"agents": ["coach"]}
    give_token(bridge, "coach", TOKEN)   # its tools' entry was written again (a new version), with a token of its own
    assert [n for n in names(bridge) if n.startswith("contacts_")] == [
        "contacts_search", "contacts_birthdays", "contacts_list_group", "contacts_add", "contacts_update"]

    assert bridge.put("/v1/contacts", json=copy(), headers=AUTH).json() == {"contacts": 2}
    found = text_of(call(bridge, "contacts_search", {"query": "helene"}, token=TOKEN))
    assert "- id c1 · Hélène Martin (« Lili »)" in found and "phones: mobile: +33 6 12 34 56 78" in found
    assert "groups: Famille" in found and "tulipes" not in found   # the notes only when ticked
    assert "c1" in text_of(call(bridge, "contacts_search", {"query": "0612345678"}, token=TOKEN))
    birthdays = text_of(call(bridge, "contacts_birthdays", {"days": 10}, token=TOKEN))
    assert "(in 2 days) · Hélène Martin · turns" in birthdays and "Paul" not in birthdays
    groups = text_of(call(bridge, "contacts_list_group", {}, token=TOKEN))
    assert "- Famille (1)" in groups and "- Travail (1)" in groups
    work = text_of(call(bridge, "contacts_list_group", {"group": "travail"}, token=TOKEN))
    assert "- id c2 · Paul Durand" in work

    asked = call(bridge, "contacts_add", {"name": "Marc Petit", "phones": ["06 99 99 99 99"], "group": "Famille"},
                 token=TOKEN)
    assert not asked["isError"] and "next sync" in text_of(asked)
    assert call(bridge, "contacts_add", {"name": "x", "birthday": "bientôt"}, token=TOKEN)["isError"]
    assert call(bridge, "contacts_update", {"id": "zz", "job": "x"}, token=TOKEN)["isError"]
    assert not call(bridge, "contacts_update", {"id": "c2", "job": "comptable"}, token=TOKEN)["isError"]
    queue = bridge.get("/v1/contacts/queue", headers=AUTH).json()["queue"]
    assert [op["op"] for op in queue] == ["add", "update"]
    assert set(queue[0]) == {"id", "op", "args", "agent", "created"} and queue[0]["agent"] == "coach"
    assert queue[0]["args"] == {"name": "Marc Petit", "phones": ["06 99 99 99 99"], "group": "g1"}
    assert queue[1]["args"] == {"id": "c2", "job": "comptable"}
    assert bridge.post(f"/v1/contacts/queue/{queue[0]['id']}", json={"done": True}, headers=AUTH).json() == {"ok": True}
    assert bridge.post("/v1/contacts/queue/nope", json={"done": True}, headers=AUTH).status_code == 404
    seen = bridge.get("/v1/contacts", headers=AUTH).json()
    assert seen["agents"] == ["coach"] and seen["contacts"] == 2 and seen["groups"] == 2 and seen["pending"] == 1
    assert isinstance(seen["at"], float)
    assert oct(os.stat(bridge.state_dir / "contacts.json").st_mode & 0o777) == "0o600"

    # Only the family, with the notes: what isn't covered goes at once.
    scope = {"all": False, "groups": ["g1"], "notes": True}
    assert bridge.put("/v1/contacts/scope", json=scope, headers=AUTH).json() == {"scope": scope}
    assert bridge.get("/v1/contacts", headers=AUTH).json()["contacts"] == 1
    assert bridge.put("/v1/contacts", json=copy(), headers=AUTH).json() == {"contacts": 1}
    assert "notes: aime les tulipes" in text_of(call(bridge, "contacts_search", {"query": "lili"}, token=TOKEN))
    assert text_of(call(bridge, "contacts_search", {"query": "paul"}, token=TOKEN)).startswith("No contact")

    assert bridge.put("/v1/contacts/agents", json={"agents": []}, headers=AUTH).json() == {"agents": []}
    give_token(bridge, "coach", TOKEN)
    assert not any(n.startswith("contacts_") for n in names(bridge))
    assert call(bridge, "contacts_search", {"query": "lili"}, token=TOKEN)["isError"]
    assert bridge.get("/v1/contacts", headers=AUTH).json()["contacts"] == 0


def test_the_copy_is_checked(bridges, fakes):
    bridge = coach(bridges, fakes)
    assert bridge.put("/v1/contacts", json={"contacts": [{"id": "x" * 301}]}, headers=AUTH).status_code == 422
    assert bridge.put("/v1/contacts", json={"contacts": [{"name": "no id"}]}, headers=AUTH).status_code == 422
    assert bridge.put("/v1/contacts", json={"contacts": [{"id": "x", "emails": [{"value": "a"}] * 31}]},
                      headers=AUTH).status_code == 422
    assert bridge.put("/v1/contacts/scope", json={"groups": ["g"] * 201}, headers=AUTH).status_code == 422
