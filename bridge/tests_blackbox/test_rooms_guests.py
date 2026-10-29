"""Group discussions, phase 2 (rooms_remote.rs), over two linked bridges: a relative's agent takes part only along
links both people ticked; its turn runs on its own bridge, in a hidden conversation there, and comes back as text."""
import time

from bbkit.common import AUTH
from test_households_and_peers import household
from test_shared_agents import link


def test_a_relatives_agent_answers_in_the_room(bridges, fakes):
    a = household(bridges, fakes, "alex", ["vie"])
    b = household(bridges, fakes, "marie", ["cuisine"])
    link(a, b)
    room = {"participants": ["vie", "cuisine@marie"], "title": "Dîner"}
    refused = a.post("/v1/rooms", headers=AUTH, json=room)
    assert refused.status_code == 403
    assert a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie", "to": "cuisine__marie"}]}).status_code == 200
    assert b.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie__alex", "to": "cuisine"}]}).status_code == 200
    made = a.post("/v1/rooms", headers=AUTH, json=room)
    assert made.status_code == 200, made.text
    guest = made.json()["participants"][1]
    assert guest["id"] == "cuisine__marie" and guest["name"] == "Cuisine (Marie)" and guest["remote"] is True
    rid = made.json()["id"]

    fakes.hermes.run_output = "Une raclette !\nMEDIA:/root/raclette.png"
    posted = a.post(f"/v1/rooms/{rid}/messages", headers=AUTH, json={"text": "@cuisine une idée pour ce soir ?"})
    assert posted.status_code == 200 and posted.json()["route"]["agents"] == ["cuisine__marie"]
    end = time.time() + 40
    messages = {}
    while time.time() < end:
        messages = a.get(f"/v1/rooms/{rid}/messages", headers=AUTH).json()
        if messages["answering"] == [] and len(messages["messages"]) >= 2:
            break
        time.sleep(0.5)
    last = messages["messages"][-1]
    assert (last["from"], last["kind"], last["text"], last["files"]) == ("cuisine__marie", "agent", "Une raclette !", [])
    runs = [r for r in fakes.hermes.started.values() if r["agent"] == "cuisine"]
    assert len(runs) == 1 and "a relative of your person" in runs[0]["input"]
    assert "Alex is not your person" in runs[0]["instructions"]
    # Alex's words drive Cuisine's turn on Marie's place: the run is closed like a relative's (patch 0018's mark)
    from test_shared_agents import RELATIVE_RUN_TOOLS
    assert runs[0]["guest_turn"] == {"tools": RELATIVE_RUN_TOOLS}
    assert a.get(f"/v1/rooms/{rid}", headers=AUTH).json()["tokens"] == {"input": 0, "output": 0}
    assert b.post("/family/r/cuisine/turn", headers={"Authorization": "Bearer nope"}, json={}).status_code == 401
