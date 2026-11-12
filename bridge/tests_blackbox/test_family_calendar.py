"""The family agent and its members' calendars (family_calendar.rs, Alex 12/11), over three linked bridges: Alex's
place holds « Famille »; Marie picks it among the agents that may use her calendar (`famille__alex` on her side), Paul
doesn't. Its calendar_list gets Marie's events (asked on her bridge, under her name) and Alex's own copy, never Paul's;
a write lands in Marie's queue only with her right to change it, by default in her own turn; her bridge down gives a
note, never a failure; a personal agent of Alex's never reaches her calendar; off, the creator stops at once."""
import json
import threading

from bbkit.common import AUTH, sse
from bbkit.mcp import TOKEN, call, give_token, rpc, text_of
from test_family_agent import make_family_agent
from test_family_thread import join
from test_households_and_peers import household
from test_shared_agents import link, wait

PERIOD = {"from": "2026-10-21", "to": "2026-10-24"}
SHARED = {"agent": "famille__alex"}


def copy(title, event_id):
    return {"from": "2026-10-14", "to": "2026-12-20", "calendars": [{"id": "C1", "title": "Perso"}],
            "events": [{"id": event_id, "calendar": "C1", "title": title, "start": "2026-10-22T10:00:00+02:00",
                        "end": "2026-10-22T10:30:00+02:00", "all_day": False}]}


def family(bridges, fakes):
    a = household(bridges, fakes, "alex", ["famille", "budget"])
    b = household(bridges, fakes, "marie", ["vie"])
    c = household(bridges, fakes, "paul", ["sport"])
    link(a, b)
    join(a, c, "Paul")
    fakes.hermes.rounds = 1000
    make_family_agent(a)
    for x in (b, c):
        assert any(v["id"] == "famille__alex" for v in x.get("/v1/family-agent", headers=AUTH).json()["agents"])
    give_token(a, "famille", TOKEN)
    for bridge, title, event_id in ((b, "Dentiste de Marie", "m1"), (c, "Foot de Paul", "p1")):
        assert bridge.put("/v1/calendar", headers=AUTH, json=copy(title, event_id)).status_code == 200
    # Paul shares his calendar with his own agent only
    assert c.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["sport"]}).status_code == 200
    return a, b, c


def flags(a):
    data = json.loads((a.state_dir / "family-agent.json").read_text())
    return data.get("calendars", {}).get("famille", {})


def calendar_tools(a):
    give_token(a, "famille", TOKEN)   # its tools' entry may have been written again (a new token)
    tools = rpc(a, "tools/list", token=TOKEN).json()["result"]["tools"]
    return [t["name"] for t in tools if t["name"].startswith("calendar_")]


def listed(a):
    give_token(a, "famille", TOKEN)
    result = call(a, "calendar_list", PERIOD)
    assert result["isError"] is False, result
    return text_of(result)


def test_the_family_agent_reads_and_writes_only_the_calendars_shared_with_it(bridges, fakes):
    a, b, c = family(bridges, fakes)

    # nobody shares yet: no calendar tool, and a plain answer if one is called
    assert calendar_tools(a) == []
    assert "Nobody in the family shares their calendar" in text_of(call(a, "calendar_list", PERIOD))

    # Marie picks the family agent (another household's personal agent can't be picked)
    assert b.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["sport__paul"]}).status_code == 400
    picked = b.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["vie", "famille__alex"]})
    assert picked.status_code == 200 and picked.json() == {"agents": ["vie", "famille__alex"]}, picked.text
    # a family agent picked reads only: changing her events is a choice of its own
    assert b.get("/v1/calendar", headers=AUTH).json()["rights"]["famille__alex"] == {"read": True, "write": False}
    assert wait(lambda: flags(a).get("marie") == {"read": True, "write": False}, 15), flags(a)
    assert wait(lambda: calendar_tools(a) == ["calendar_list", "calendar_add", "calendar_update", "calendar_delete"], 15)
    tools = {t["name"]: t for t in rpc(a, "tools/list", token=TOKEN).json()["result"]["tools"]}
    assert "who" in tools["calendar_add"]["inputSchema"]["properties"]

    # its list: Marie's events under her name, nothing of Paul's (he didn't share with it)
    text = listed(a)
    assert "## Marie" in text and "Dentiste de Marie" in text and "id m1" in text, text
    assert "Paul" not in text and "Foot" not in text

    # Alex shares his own too: under his name
    assert a.put("/v1/calendar", headers=AUTH, json=copy("Garage d'Alex", "a1")).status_code == 200
    assert a.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["famille"]}).status_code == 200
    # his family agent too reads only at first; he lets it add to his calendar
    assert a.get("/v1/calendar", headers=AUTH).json()["rights"]["famille"] == {"read": True, "write": False}
    assert a.put("/v1/calendar/rights", headers=AUTH, json={"agent": "famille", "rights": {"write": True}}).status_code == 200
    text = listed(a)
    assert "## Alex" in text and "Garage d'Alex" in text and "## Marie" in text, text

    # a personal agent of Alex's (or a wrong key) never reaches Marie's calendar through her bridge
    key = json.loads((a.state_dir / "households.json").read_text())["households"]["marie"]["key_out"]
    body = {"name": "calendar_list", "arguments": PERIOD}
    personal = b.post("/family/calendar/budget", json=body, headers={"Authorization": f"Bearer {key}"})
    assert personal.status_code == 404
    assert b.post("/family/calendar/famille", json=body, headers={"Authorization": "Bearer nope"}).status_code == 401
    # Paul's bridge answers the family agent nothing: he didn't pick it
    pauls_key = json.loads((a.state_dir / "households.json").read_text())["households"]["paul"]["key_out"]
    nothing = c.post("/family/calendar/famille", json=body, headers={"Authorization": f"Bearer {pauls_key}"})
    assert nothing.status_code == 200 and nothing.json() == {"allowed": False, "read": False, "write": False}

    # not hers to change yet: said plainly, nothing queued
    give_token(a, "famille", TOKEN)
    refused = call(a, "calendar_add", {"who": "Marie", "title": "Cinéma", "start": "2026-10-30T20:00"})
    assert refused["isError"] is True and "Marie doesn't let you change" in text_of(refused), refused
    assert b.get("/v1/calendar/queue", headers=AUTH).json()["queue"] == []
    write = b.put("/v1/calendar/rights", headers=AUTH, json={"agent": "famille__alex", "rights": {"write": True}})
    assert write.status_code == 200 and write.json() == {"agent": "famille__alex", "rights": {"read": True, "write": True}}
    assert wait(lambda: flags(a).get("marie") == {"read": True, "write": True}, 15), flags(a)

    # a write on Marie's calendar, named: queued on HER bridge, under the agent's id there
    give_token(a, "famille", TOKEN)
    asked = call(a, "calendar_add", {"who": "marie", "title": "Piscine", "start": "2026-10-30T17:00"})
    assert asked["isError"] is False and "Marie's phone" in text_of(asked), asked
    queue = b.get("/v1/calendar/queue", headers=AUTH).json()["queue"]
    assert [(q["op"], q["agent"], q["args"]["title"]) for q in queue] == [("add", "famille__alex", "Piscine")]
    assert a.get("/v1/calendar/queue", headers=AUTH).json()["queue"] == []
    # in Alex's own turn, no who: his calendar
    assert call(a, "calendar_add", {"title": "Vidange", "start": "2026-10-31"})["isError"] is False
    assert [q["args"]["title"] for q in a.get("/v1/calendar/queue", headers=AUTH).json()["queue"]] == ["Vidange"]

    # in Marie's turn, no who: hers
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"),
                               sse("tool.started", run_id="r1", tool="mcp__bipagents__calendar_add"), gate,
                               sse("run.completed", run_id="r1", output="C'est noté.")]
    sent = b.post("/v1/threads/main/messages", headers=AUTH,
                  json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D1", "text": "Ajoute judo jeudi"})
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    assert wait(lambda: "r1" in fakes.hermes.started, 15)
    in_her_turn = call(a, "calendar_add", {"title": "Judo", "start": "2026-10-29T18:00"})
    assert in_her_turn["isError"] is False, in_her_turn
    titles = [q["args"]["title"] for q in b.get("/v1/calendar/queue", headers=AUTH).json()["queue"]]
    assert titles == ["Piscine", "Judo"]
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    assert wait(lambda: b.get("/v1/threads/main/timeline", headers=AUTH, params=SHARED).json()["pending"]["run"] is None, 15)

    # Marie takes the right to change back: her requests waiting are cancelled, the agent is told plainly
    rights = b.put("/v1/calendar/rights", headers=AUTH, json={"agent": "famille__alex", "rights": {"write": False}})
    assert rights.status_code == 200 and rights.json() == {"agent": "famille__alex", "rights": {"read": True, "write": False}}
    assert b.get("/v1/calendar/queue", headers=AUTH).json()["queue"] == []
    assert wait(lambda: flags(a).get("marie") == {"read": True, "write": False}, 15), flags(a)
    give_token(a, "famille", TOKEN)
    refused = call(a, "calendar_add", {"who": "Marie", "title": "Cinéma", "start": "2026-10-30T20:00"})
    assert refused["isError"] is True and "Marie doesn't let you change" in text_of(refused), refused
    assert "Dentiste de Marie" in listed(a)   # reading still allowed

    # her bridge down: one line, never a failure; Alex's own still there
    b.stop()
    text = listed(a)
    assert "Marie's calendar did not answer" in text and "Garage d'Alex" in text, text
    b.start()

    # she turns it off: the creator's flag goes at once, her events no more
    assert b.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["vie"]}).status_code == 200
    assert wait(lambda: "marie" not in flags(a), 15), flags(a)
    text = listed(a)
    assert "Dentiste" not in text and "Marie" not in text and "Garage d'Alex" in text, text
