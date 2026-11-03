"""Who answers a follow-up in a discussion, after Alex's room of 2026-11-03 (Wellness and Vie): Wellness answered
« que penser de ces derniers jours » and ended « je le fais au prochain tour »; « J'ai fait ma course aujourd'hui » then
went to Vie (the rule's « courses », the errands), who passed, and the room showed only « Vie n'a rien à ajouter. ».
Here Coach (wellness) and Cuisine (home, whose words have « courses ») play them (rooms.rs `continuity`, `pass_next`,
`NO_DEFERRAL`)."""
from bbkit.common import AUTH
from bbkit.servers import wait_until

from test_rooms import answering, kind, messages, quiet, replies, room_of, runs_of, send, start

FOLLOW_UP = "J'ai fait ma course aujourd'hui"


def post(bridge, room, words):
    sent = send(bridge, room["id"], words)
    assert sent.status_code == 200, sent.text
    return sent.json()


def test_a_follow_up_goes_to_the_agent_who_just_answered_and_its_turn_finishes_the_work(bridges, fakes):
    bridge = start(bridges, fakes, notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb", "model": "small-eu"})
    fakes.hermes.rounds = 1
    fakes.hermes.run_outputs = {"r1": "Sommeil 7 h 40 en moyenne, 3 sorties.\nNEXT: person",
                                "r2": "Bravo pour la course ! Allure en progrès.\nNEXT: person"}
    room = room_of(bridge, "coach", "cuisine")
    fakes.model.route_answer = "coach"
    first = post(bridge, room, "Hello, que penser de ces derniers jours")
    assert first["route"] == {"by": "model", "agents": ["coach"]}
    assert wait_until(lambda: len(replies(bridge, room["id"])) == 1 and quiet(bridge, room["id"]), timeout=20)
    # the room turn's instructions: finish the work in the turn, never « au prochain tour »
    rules = runs_of(fakes, "coach")[0]["instructions"]
    assert "never end by promising to do it later or « at the next turn »" in rules
    assert "propose a background task (propose_task)" in rules
    assert "a few sentences" not in rules

    chats = len(fakes.model.chats)
    second = post(bridge, room, FOLLOW_UP)
    # before: {"by": "rule", "agents": ["cuisine"]} (« course » / « courses »)
    assert second["route"] == {"by": "continuity", "agents": ["coach"]}
    assert len(fakes.model.chats) == chats   # no router call
    assert wait_until(lambda: len(replies(bridge, room["id"])) == 2 and quiet(bridge, room["id"]), timeout=20)
    last = replies(bridge, room["id"])[-1]
    assert (last["from"], kind(last), last["text"]) == ("coach", "agent", "Bravo pour la course ! Allure en progrès.")
    assert runs_of(fakes, "cuisine") == []

    # a message that clearly concerns the other agent is not continuity
    third = post(bridge, room, "Une recette de cuisine pour la maison ce soir ?")
    assert third["route"] == {"by": "rule", "agents": ["cuisine"]}


def test_a_pass_by_the_picked_agent_hands_the_message_to_the_next_concerned(bridges, fakes):
    bridge = start(bridges, fakes, rooms={"router_model": False})
    fakes.hermes.rounds = 1
    fakes.hermes.run_outputs = {"r1": "PASS", "r2": "Bravo ! Combien de kilomètres ?\nNEXT: person"}
    room = room_of(bridge, "coach", "cuisine")
    sent = post(bridge, room, FOLLOW_UP)
    assert sent["route"] == {"by": "rule", "agents": ["cuisine"]}   # nobody spoke before: the rule's pick
    assert wait_until(lambda: len(replies(bridge, room["id"])) == 2 and quiet(bridge, room["id"]), timeout=25)
    passed, answer = replies(bridge, room["id"])
    assert passed["kind"] == "notice" and passed["from"] == "cuisine" and passed["notice"]["code"] == "pass"
    assert passed["room"]["handoff"] == {"to": "coach", "why": "", "by": "pass"}   # the apps say who goes on
    assert (answer["from"], kind(answer), answer["room"]["hop"]) == ("coach", "agent", 0)
    assert answer["text"] == "Bravo ! Combien de kilomètres ?"
    assert len(fakes.hermes.started) == 2

    # both pass: two notices, no third turn (two participants)
    fakes.hermes.run_outputs = {"r3": "PASS", "r4": "Rien à ajouter."}
    post(bridge, room, "Merci à vous")
    assert wait_until(lambda: len(replies(bridge, room["id"])) == 4 and quiet(bridge, room["id"]), timeout=25)
    assert [kind(m) for m in replies(bridge, room["id"])[2:]] == ["notice", "notice"]
    assert len(fakes.hermes.started) == 4

    # a PASS of the agent the person named: nobody else is asked
    fakes.hermes.run_outputs = {"r5": "PASS"}
    post(bridge, room, "@cuisine tu as couru toi ?")
    assert wait_until(lambda: len(replies(bridge, room["id"])) == 5 and quiet(bridge, room["id"]), timeout=25)
    assert len(fakes.hermes.started) == 5


def test_a_follow_up_while_the_agent_still_answers_queues_for_it(bridges, fakes):
    bridge = start(bridges, fakes, notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb", "model": "small-eu"})
    fakes.hermes.rounds = 10_000
    fakes.hermes.run_outputs = {"r1": "Tes derniers jours : bon sommeil, 3 sorties.\nNEXT: person",
                                "r2": "Bien joué pour la course du jour.\nNEXT: person"}
    room = room_of(bridge, "coach", "cuisine")
    fakes.model.route_answer = "coach"
    post(bridge, room, "Hello, que penser de ces derniers jours")
    assert wait_until(lambda: runs_of(fakes, "coach"), timeout=10)
    second = post(bridge, room, FOLLOW_UP)
    assert second["route"] == {"by": "continuity", "agents": ["coach"]} and second["state"] == "queued"
    assert [(a["agent"], a["state"]) for a in answering(bridge, room["id"])] == [("coach", "writing"), ("coach", "queued")]
    fakes.hermes.rounds = 1
    assert wait_until(lambda: len(replies(bridge, room["id"])) == 2 and quiet(bridge, room["id"]), timeout=30)
    said = replies(bridge, room["id"])
    asked = [m["id"] for m in messages(bridge, room["id"])["items"] if kind(m) == "person"]
    assert [(m["from"], kind(m), m["turn"]) for m in said] == [("coach", "agent", asked[0]),
                                                              ("coach", "agent", asked[1])]
    assert said[1]["text"] == "Bien joué pour la course du jour."
    assert FOLLOW_UP in runs_of(fakes, "coach")[1]["input"]
    assert runs_of(fakes, "cuisine") == []
