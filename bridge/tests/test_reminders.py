import dataclasses

from bipbridge import reminders
from bipbridge.config import AgentConfig
from conftest import AUTH


def with_agent(client, config):
    config = dataclasses.replace(config)
    config.agents["coach"] = AgentConfig(name="coach", display_name="Coach", hermes_url="http://127.0.0.1:8642/p/coach",
                                         hermes_key="k")
    client.app.state.services.config = config
    return config


def test_the_iphone_shares_its_reminders_and_does_what_the_agents_ask(client, config):
    config = with_agent(client, config)
    assert reminders.call(config, "coach", "reminders_list", {})["isError"]          # not shared with it yet
    assert client.put("/v1/reminders/agents", json={"agents": ["coach"]}, headers=AUTH).json() == {"agents": ["coach"]}
    snapshot = {"lists": [{"id": "L1", "title": "Courses"}, {"id": "L2", "title": "Maison"}],
                "items": [{"id": "r1", "list": "L1", "title": "Lait", "due": "2026-10-21"},
                          {"id": "r2", "list": "L2", "title": "Appeler le plombier", "notes": "Fuite sous l'évier"},
                          {"id": "r3", "list": "L1", "title": "Pain", "completed": True}]}
    assert client.put("/v1/reminders", json=snapshot, headers=AUTH).json() == {"items": 3}
    listed = reminders.call(config, "coach", "reminders_list", {})["content"][0]["text"]
    assert "id r1 · Courses · Lait · due 2026-10-21" in listed and "Fuite sous l'évier" in listed and "Pain" not in listed
    courses = reminders.call(config, "coach", "reminders_list", {"list": "courses", "include_done": True})["content"][0]["text"]
    assert "Pain" in courses and "plombier" not in courses
    asked = reminders.call(config, "coach", "reminders_add", {"title": "Rendez-vous banque", "due": "2026-10-30T10:00",
                                                              "list": "Maison"})
    assert not asked["isError"] and "next sync" in asked["content"][0]["text"]
    assert reminders.call(config, "coach", "reminders_complete", {"id": "zz"})["isError"]
    reminders.call(config, "coach", "reminders_complete", {"id": "r1"})
    queue = client.get("/v1/reminders/queue", headers=AUTH).json()["queue"]
    assert [(op["op"], op["args"].get("title") or op["args"].get("id")) for op in queue] == [
        ("add", "Rendez-vous banque"), ("complete", "r1")]
    assert client.post(f"/v1/reminders/queue/{queue[0]['id']}", json={"done": True}, headers=AUTH).json() == {"ok": True}
    assert [op["op"] for op in client.get("/v1/reminders/queue", headers=AUTH).json()["queue"]] == ["complete"]
    seen = client.get("/v1/reminders", headers=AUTH).json()
    assert seen["agents"] == ["coach"] and seen["items"] == 3 and seen["pending"] == 1
    assert reminders.call(config, "vie", "reminders_add", {"title": "x"})["isError"]  # another agent: not shared
