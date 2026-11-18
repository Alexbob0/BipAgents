"""The family agent deletes an event only with the calendar OWNER's « Oui » (security audit 1.3, Alex 18/11): the card
goes to the phone of the person whose calendar it is, never to whoever asked in the family conversation, nor to
another member. « Non », or no answer, deletes nothing; the card waits for her even when nobody follows the turn
(the agent's call returns at once), and her « Oui » later deletes it. Over three linked bridges, as in
test_family_calendar.py: Alex holds « Famille », Marie lets it change her calendar, Paul doesn't share his."""
from bbkit.common import AUTH
from bbkit.mcp import TOKEN, call, give_token, text_of
from test_family_calendar import copy, family, flags
from test_shared_agents import wait

SENTENCES = ("Famille veut supprimer « {t} » de ton agenda ?", "Famille wants to delete « {t} » from your calendar?")


def cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-ok-")]


def answer(bridge, card, choice):
    said = bridge.post("/v1/approve", headers=AUTH, json={"agent": card["agent"], "run_id": card["run_id"],
                                                           "choice": choice})
    assert said.status_code == 200, said.text


def queue(bridge):
    return [(q["op"], q["agent"], q["args"].get("id")) for q in bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"]]


def delete(a, who, event_id):
    give_token(a, "famille", TOKEN)
    return call(a, "calendar_delete", {"who": who, "id": event_id})


def test_only_the_calendars_owner_is_asked_and_only_her_yes_deletes(bridges, fakes):
    a, b, c = family(bridges, fakes)
    # Marie lets the family agent change her calendar; Alex his own
    assert b.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["vie", "famille__alex"]}).status_code == 200
    assert b.put("/v1/calendar/rights", headers=AUTH,
                 json={"agent": "famille__alex", "rights": {"write": True}}).status_code == 200
    assert wait(lambda: flags(a).get("marie") == {"read": True, "write": True}, 15), flags(a)
    assert a.put("/v1/calendar", headers=AUTH, json=copy("Garage d'Alex", "a1")).status_code == 200
    assert a.put("/v1/calendar/agents", headers=AUTH, json={"agents": ["famille"]}).status_code == 200
    assert a.put("/v1/calendar/rights", headers=AUTH, json={"agent": "famille", "rights": {"write": True}}).status_code == 200

    # Marie's event: the card on HER phone (her own agent's cards), nobody else's; nothing deleted meanwhile, and the
    # agent's call does not wait (the card waits for her, followed turn or not)
    asked = delete(a, "Marie", "m1")
    assert asked["isError"] is False and "Marie's phone" in text_of(asked), asked
    [card] = cards(b)
    assert card["agent"] == "vie" and card["bip_ok"]["kind"] == "calendar_delete", card
    assert card["description"] in [s.format(t="Dentiste de Marie") for s in SENTENCES], card
    assert card["choices"] == ["once", "deny"] and "bip_always" not in card, card
    assert cards(a) == [] and cards(c) == [], "another member, or the one who asked, never gets it"
    assert queue(b) == [], "nothing deleted before her answer"
    # « Non »: nothing
    answer(b, card, "deny")
    assert cards(b) == [] and queue(b) == []
    # asked again, « Oui » (later, the turn long over): deleted on her phone, under the agent's id there
    assert delete(a, "Marie", "m1")["isError"] is False
    [card] = cards(b)
    answer(b, card, "once")
    assert queue(b) == [("delete", "famille__alex", "m1")]
    assert cards(b) == []

    # Alex's own event: the card on HIS phone only
    asked = delete(a, "Alex", "a1")
    assert asked["isError"] is False, asked
    [card] = cards(a)
    assert card["agent"] == "budget" and card["description"] in [s.format(t="Garage d'Alex") for s in SENTENCES], card
    assert cards(b) == [] and cards(c) == []
    assert queue(a) == []
    answer(a, card, "once")
    assert queue(a) == [("delete", "famille", "a1")]

    # no card for what isn't deletable: an unknown event is said so at once
    unknown = delete(a, "Marie", "zz")
    assert unknown["isError"] is True and cards(b) == [], unknown
