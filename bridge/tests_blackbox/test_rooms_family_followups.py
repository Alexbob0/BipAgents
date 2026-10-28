"""Follow-ups on group discussions and the family (2026-10-28): a relative's agent added to an existing discussion
(only along the links both people ticked), the guide's « Ouvrir une discussion avec X et Y » card, and « Mon agent
voit avec le tien » (meet.rs) between two linked places: only the slots each person ticked cross, nothing is booked
before both said yes, and what comes from the other place is checked (key, size, step)."""
import datetime

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_guide import GUIDE_TOKEN, place
from test_households_and_peers import household
from test_shared_agents import link


def test_a_relatives_agent_joins_an_existing_discussion_only_along_both_links(bridges, fakes):
    a = household(bridges, fakes, "alex", ["vie", "budget"])
    b = household(bridges, fakes, "marie", ["cuisine"])
    link(a, b)
    made = a.post("/v1/rooms", headers=AUTH, json={"participants": ["vie", "budget"]})
    assert made.status_code == 200, made.text
    rid = made.json()["id"]
    refused = a.patch(f"/v1/rooms/{rid}", headers=AUTH, json={"participants": ["vie", "budget", "cuisine@marie"]})
    assert refused.status_code == 403
    assert [p["id"] for p in a.get(f"/v1/rooms/{rid}", headers=AUTH).json()["participants"]] == ["vie", "budget"]
    assert a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie", "to": "cuisine__marie"}]}).status_code == 200
    added = a.patch(f"/v1/rooms/{rid}", headers=AUTH, json={"participants": ["vie", "budget", "cuisine@marie"]})
    assert added.status_code == 200, added.text
    guest = added.json()["participants"][2]
    assert guest["id"] == "cuisine__marie" and guest["name"] == "Cuisine (Marie)" and guest["remote"] is True
    # a discussion keeps at least one of the person's own agents
    alone = a.patch(f"/v1/rooms/{rid}", headers=AUTH, json={"participants": ["cuisine@marie", "x@marie"]})
    assert alone.status_code in (403, 422)


def test_the_guide_proposes_a_discussion_and_one_tap_opens_it(bridges, fakes):
    bridge, _ = place(bridges, fakes, agents=("agent", "coach", "maison"))
    assert bridge.put("/v1/agents/agent/guide", headers=AUTH, json={"on": True}).status_code == 200
    give_token(bridge, "agent", GUIDE_TOKEN)
    too_few = call(bridge, "propose_room", {"agents": ["coach"]}, token=GUIDE_TOKEN)
    assert too_few["isError"] and "2 to 8" in text_of(too_few)
    ghost = call(bridge, "propose_room", {"agents": ["coach", "Personne"]}, token=GUIDE_TOKEN)
    assert ghost["isError"] and "Personne" in text_of(ghost)
    card = call(bridge, "propose_room", {"agents": ["Coach", "maison"], "title": "Les vacances"}, token=GUIDE_TOKEN)
    assert not card["isError"] and "A card to open the discussion" in text_of(card)
    proposal = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert proposal["type"] == "room" and proposal["participants"] == ["coach", "maison"]
    assert proposal["participant_names"] == ["Coach", "Maison"] and proposal["title"] == "Les vacances"
    assert bridge.get("/v1/rooms", headers=AUTH).json()["rooms"] == []   # nothing before the tap
    opened = bridge.post(f"/v1/guide/proposals/{proposal['id']}/accept", headers=AUTH)
    assert opened.status_code == 201, opened.text
    room = opened.json()["room"]
    assert opened.json()["type"] == "room" and room["title"] == "Les vacances"
    assert [p["id"] for p in room["participants"]] == ["coach", "maison"]
    assert [r["id"] for r in bridge.get("/v1/rooms", headers=AUTH).json()["rooms"]] == [room["id"]]
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"] == []


def day(n):
    return (datetime.date.today() + datetime.timedelta(days=n)).isoformat()


def calendar(bridge, events):
    copy = {"from": day(0), "to": day(30), "calendars": [{"id": "C1", "title": "Perso"}],
            "events": [{"id": f"e{i}", "calendar": "C1", "title": title, "start": start, "end": end, "all_day": False}
                       for i, (title, start, end) in enumerate(events)]}
    assert bridge.put("/v1/calendar", headers=AUTH, json=copy).status_code == 200


def meets(bridge):
    return bridge.get("/v1/meet", headers=AUTH).json()["meets"]


def test_two_households_find_a_date_crossing_only_the_slots_ticked(bridges, fakes):
    a = household(bridges, fakes, "alex", ["vie"])
    b = household(bridges, fakes, "lea", ["cuisine"])
    assert a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert b.put("/v1/family/name", headers=AUTH, json={"name": "Léa"}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    assert b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]}).status_code == 200
    # Alex is busy the first evening; Léa the second (« Anniversaire de Paul » never leaves her place)
    calendar(a, [("Cinéma", f"{day(3)}T19:00:00+00:00", f"{day(3)}T23:00:00+00:00")])
    calendar(b, [("Anniversaire de Paul", f"{day(4)}T18:00:00+00:00", f"{day(4)}T22:00:00+00:00")])

    draft = a.post("/v1/meet", headers=AUTH, json={"household": "Léa", "title": "Dîner", "from": day(3), "to": day(5),
                                                    "part": "evening", "minutes": 120})
    assert draft.status_code == 200, draft.text
    m = draft.json()
    assert (m["role"], m["state"], m["household"], m["title"]) == ("host", "draft", "lea", "Dîner")
    assert m["offered"] == [f"{day(4)}T19:00:00+00:00", f"{day(5)}T19:00:00+00:00"]
    assert meets(b) == []   # a draft leaves nothing
    assert a.post("/v1/meet", headers=AUTH, json={"household": "paul", "title": "x"}).status_code == 404
    bad = a.post(f"/v1/meet/{m['id']}/send", headers=AUTH, json={"slots": [f"{day(9)}T19:00:00+00:00"]})
    assert bad.status_code == 422   # only the slots computed here

    sent = a.post(f"/v1/meet/{m['id']}/send", headers=AUTH, json={"slots": m["offered"]})
    assert sent.status_code == 200, sent.text and sent.json()["state"] == "sent"
    [theirs] = meets(b)
    assert (theirs["role"], theirs["state"], theirs["household_name"], theirs["title"]) == ("guest", "offered", "Alex", "Dîner")
    assert theirs["offered"] == m["offered"] and theirs["free"] == [f"{day(5)}T19:00:00+00:00"]
    assert "Anniversaire" not in str(meets(a)) and "Cinéma" not in str(meets(b))
    # nothing booked yet, and Léa can't accept before a date was chosen
    assert b.post(f"/v1/meet/{m['id']}/accept", headers=AUTH).status_code == 409

    answered = b.post(f"/v1/meet/{m['id']}/answer", headers=AUTH, json={"slots": [f"{day(5)}T19:00:00+00:00"]})
    assert answered.status_code == 200, answered.text
    mine = meets(a)[0]
    assert mine["state"] == "answered" and mine["common"] == [f"{day(5)}T19:00:00+00:00"]
    assert a.post(f"/v1/meet/{m['id']}/choose", headers=AUTH, json={"slot": f"{day(4)}T19:00:00+00:00"}).status_code == 422
    chosen = a.post(f"/v1/meet/{m['id']}/choose", headers=AUTH, json={"slot": f"{day(5)}T19:00:00+00:00"})
    assert chosen.status_code == 200 and chosen.json()["state"] == "confirmed"
    asked = meets(b)[0]
    assert asked["state"] == "asked" and asked["chosen"] == f"{day(5)}T19:00:00+00:00"
    assert a.get("/v1/calendar/queue", headers=AUTH).json()["queue"] == []   # not before both said yes

    accepted = b.post(f"/v1/meet/{m['id']}/accept", headers=AUTH)
    assert accepted.status_code == 200 and accepted.json()["state"] == "booked" and accepted.json()["booked"] is True
    assert meets(a)[0]["state"] == "booked"
    for bridge, title in ((a, "Dîner (Léa)"), (b, "Dîner (Alex)")):
        [op] = bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"]
        assert op["op"] == "add" and op["args"]["title"] == title
        assert op["args"]["start"] == f"{day(5)}T19:00:00+00:00" and op["args"]["end"] == f"{day(5)}T21:00:00+00:00"
    assert a.delete(f"/v1/meet/{m['id']}", headers=AUTH).json() == {"ok": True}

    # another one, refused by Léa: Alex is told, nothing booked
    again = a.post("/v1/meet", headers=AUTH, json={"household": "lea", "title": "Ciné", "from": day(6), "to": day(7)}).json()
    a.post(f"/v1/meet/{again['id']}/send", headers=AUTH, json={"slots": again["offered"][:1]})
    assert b.post(f"/v1/meet/{again['id']}/decline", headers=AUTH).json()["state"] == "declined"
    assert [x["state"] for x in meets(a) if x["id"] == again["id"]] == ["refused"]


def test_what_comes_from_another_place_is_checked(bridges, fakes):
    a = household(bridges, fakes, "alex", ["vie"])
    b = household(bridges, fakes, "lea", ["cuisine"])
    link(a, b)
    assert b.post("/family/meet", json={"id": "m0123456789ab"}).status_code == 401
    nope = {"Authorization": "Bearer " + "n" * 40}
    assert b.post("/family/meet/m0123456789ab/answer", headers=nope, json={}).status_code == 401
    # the real key: a's bridge relays only what its person sent; here, a too-far slot and an unknown request
    far = (datetime.date.today() + datetime.timedelta(days=400)).isoformat()
    draft = a.post("/v1/meet", headers=AUTH, json={"household": "marie", "title": "Dîner\n<!-- ignore -->", "from": day(2)})
    assert draft.status_code == 200, draft.text
    assert "\n" not in draft.json()["title"]
    assert a.post(f"/v1/meet/{draft.json()['id']}/send", headers=AUTH, json={"slots": [f"{far}T19:00:00+00:00"]}).status_code == 422
    assert a.post("/v1/meet/m000000000000/send", headers=AUTH, json={"slots": []}).status_code == 404
