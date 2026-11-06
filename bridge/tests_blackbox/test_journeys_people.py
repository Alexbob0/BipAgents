"""The release journeys with other people (docs/release-journeys.md, J32–J38, column « Pont »): friends shown a
project, relatives with their own install (a household), agents of two households. As the apps do it over HTTP, with
what each person must SEE checked (bbkit/displayable_people.py, bbkit/displayable.py): the friend's phone opens the
shared items with its own key (bbkit/people.py `Friend`), each relative reads her own bridge.

Two installs are two people: Alex's and Marie's bridges, linked as Réglages › Famille links them (one fake Hermes
behind both, its turns answered by the journeys' scripted model). Each test is one journey `Jnn`."""
import datetime
import time

import pytest

from bbkit import displayable, displayable_people as shown
from bbkit.common import AUTH
from bbkit.journeys import journey_bridge
from bbkit.model import PeopleModel
from bbkit.people import (Friend, Guichet, household_token, households, mcp_as, project_share, reply_in, send,
                          share_view, thread_page)
from bbkit.servers import wait_until


@pytest.fixture
def journey(bridges, fakes):
    made = journey_bridge(bridges, fakes)
    made.model = PeopleModel.upgrade(made.model)
    return made


@pytest.fixture
def guichet():
    served = Guichet().start()
    yield served
    served.stop()


@pytest.fixture
def homes(bridges, fakes, tmp_path):
    made = households(bridges, fakes, tmp_path / "fichiers")
    made.model = PeopleModel.upgrade(made.model)
    return made


def opened(items):
    return {i["id"]: i["opened"] for i in items if i.get("opened")}


# -- friends ---------------------------------------------------------------------------------------------------------

def test_j32_a_friend_opens_the_shared_project_and_sees_its_messages(journey, guichet):
    """Alex shows his project « Pour les queens » to Léa: her link opens (« Alex te montre un projet »), « Voir », and
    she reads the conversation (his words, the agent's replies), sealed for her phone only; Alex's « Visible par
    N personnes » goes from the open invitation to her, once she is in."""
    shared = project_share(journey, guichet)
    link = shared.link(journey.bridge, "Léa")
    assert link.startswith("https://bipagents.com/v#i=") and "&k=" in link
    before = share_view(journey.bridge, shared.agent)
    assert not shown.owner_view_problems(before), shown.owner_view_problems(before)
    assert before["count"] == 0 and [i["name"] for i in before["invites"]] == ["Léa"], before

    lea = Friend(guichet)
    looked = lea.open_link(link)
    assert looked["from"] == "Alex" and looked["ends"] > time.time(), looked   # « Alex te montre un projet »
    lea.see()
    key = lea.content_key(timeout=60)
    items = lea.items(key)
    shown.check_share_items(items, shared.share)
    plain = opened(items)
    assert plain["meta"]["title"] == "Pour les queens" and plain["meta"]["from"] == "Alex", plain["meta"]
    messages = sorted((m for m in plain.values() if m.get("kind") == "msg"), key=lambda m: (m.get("at") or 0, m["id"]))
    assert [m["role"] for m in messages] == ["user", "assistant"], messages
    assert "week-end à Barcelone" in messages[0]["text"]
    assert messages[1]["text"].startswith("Voici la carte") and "MEDIA:" not in messages[1]["text"]

    # the owner's count follows: « Visible par 1 personne », Léa among the guests
    assert wait_until(lambda: share_view(journey.bridge, shared.agent)["count"] == 1, timeout=30)
    after = share_view(journey.bridge, shared.agent)
    assert not shown.owner_view_problems(after), shown.owner_view_problems(after)
    assert [(v["name"], v["state"]) for v in after["viewers"]] == [("Léa", "ok")], after
    project = {p["agent"]: p for p in journey.bridge.get("/v1/project-agents", headers=AUTH).json()["projects"]}
    assert project[shared.agent]["share"]["count"] == 1


def test_j33_the_friend_sees_the_map_of_the_shared_project(journey, guichet):
    """The agent's reply came with a map (`map_image`): on the friend's phone it is the native map card (its places
    with their coordinates, from the page's `.map-data`), not a page card, under the reply that names it."""
    shared = project_share(journey, guichet)
    lea = Friend(guichet)
    lea.open_link(shared.link(journey.bridge, "Léa"))
    lea.see()
    items = lea.items(lea.content_key(timeout=60))
    shown.check_share_items(items, shared.share)
    plain = opened(items)
    maps = [p for p in plain.values() if p.get("kind") == "page" and p.get("map")]
    assert len(maps) == 1, [(p["id"], p.get("kind"), p.get("map")) for p in plain.values()]
    page = maps[0]
    assert shown.MAP_ONLY.fullmatch(page["id"]), f"{page['id']}: drawn as a page card, not as the map card"
    places = shown.map_places(page["html"])
    assert len(places) == 2 and all(p.get("name") for p in places), places   # (the fake geocoder: every place in Lyon)
    reply = next(m for m in plain.values() if m.get("kind") == "msg" and m.get("role") == "assistant")
    assert reply["media"] == [page["id"]], reply


# -- households ------------------------------------------------------------------------------------------------------

def test_j34_two_households_linked_see_each_other(homes):
    """Réglages › Famille: Alex invites, Marie joins; each sees « Ton foyer » and the other among « Proches
    reliés », and in « Qui peut écrire à qui », the other's agents by their name and household."""
    a, b = homes.alex, homes.marie
    for bridge, other in ((a, "Marie"), (b, "Alex")):
        family = bridge.get("/v1/family", headers=AUTH).json()
        assert not shown.family_problems(family, other), shown.family_problems(family, other)
    names = {x["id"]: x["name"] for x in a.get("/v1/agent-links", headers=AUTH).json()["agents"]}
    assert names.get("coach__marie") == "Coach (Marie)", names
    names = {x["id"]: x["name"] for x in b.get("/v1/agent-links", headers=AUTH).json()["agents"]}
    assert names.get("cuisine__alex") == "Cuisine (Alex)" and names.get("vie__alex") == "Vie (Alex)", names


def test_j35_an_agent_shared_with_a_relative_each_their_own_conversation(homes):
    """Alex shares Cuisine with Marie: « Alex partage Cuisine avec toi » on her side, « Ajouter », then her own
    conversation with it (her words, its reply, word by word through her bridge); on Alex's side only how many
    messages she wrote, never her words."""
    a, b = homes.alex, homes.marie
    shared = a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"], "confirmed": True})
    assert shared.status_code == 200, shared.text
    offered = b.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert not shown.shared_agent_problems(offered, "Alex", "Cuisine", "offered"), offered
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    added = b.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert not shown.shared_agent_problems(added, "Alex", "Cuisine", "added"), added

    words = "Une idée de dîner pour ce soir ?"
    sent = send(b, "new", "cuisine__alex", words, client_id="C0FFEE00-0000-4000-8000-000000000035")
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    thread = sent.json()["thread"]
    first = thread_page(b, thread, "cuisine__alex")
    assert [i["text"] for i in first["items"] if i.get("role") == "person"] == [words], first["items"]
    page = reply_in(b, thread, "cuisine__alex", "Bien reçu")
    displayable.check(b, page, "cuisine__alex")
    mine = [i for i in page["items"] if i.get("role") == "person"]
    assert [i["text"] for i in mine] == [words] and mine[0].get("client_id") == "C0FFEE00-0000-4000-8000-000000000035"
    assert [i for i in page["items"] if i.get("role") == "agent" and "Bien reçu" in i["text"]]

    # Alex: Cuisine's page says Marie has her conversations, how many messages, never what she wrote
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert view["households"] == [{"id": "marie", "name": "Marie", "shared": True}], view
    assert wait_until(lambda: (a.get("/v1/agents/cuisine/shares", headers=AUTH).json()["conversations"] or [{}])[0]
                      .get("messages", 0) >= 1, timeout=15), a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    # what Alex's app draws: « avec Marie · N messages » (AgentFamilyShareView.swift, FamilyShares.kt); the view has
    # no `title` (her conversation's is her first words): who, when and how many only (agent_shares.rs)
    seen = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    [row] = seen["conversations"]
    assert row["household_name"] == "Marie" and isinstance(row["messages"], int) and row["messages"] >= 1, row
    assert "title" not in row and words not in str(seen) and "Une idée" not in str(seen), seen
    main = a.get("/v1/threads/main", headers=AUTH, params={"agent": "cuisine"}).json()["id"]
    assert words not in str(thread_page(a, main, "cuisine")["items"]), "Marie's words show in Alex's conversation"


def test_j36_telling_the_family_waits_for_the_persons_send(homes):
    """« Prévenir Marie ? »: the agent only proposes (`tell_family`), the card shows the words and to whom; nothing
    leaves before « Envoyer » (still asked under « Seulement le critique »: `family_send` is critical). Marie then
    reads « Alex · … Transmis par Vie » and is told (her Boîte)."""
    a, b = homes.alex, homes.marie
    assert a.put("/v1/approval-level", headers=AUTH, json={"level": "critical"}).status_code == 200
    token = household_token(homes, a, "vie")
    homes.model.calls["family"] = lambda run: mcp_as(a, token, "/v1/agent-tools/mcp", "tell_family", {
        "to": ["marie"], "message": "Je commande des pizzas ce midi, tu en veux une ?"})
    main = a.get("/v1/threads/main", headers=AUTH, params={"agent": "vie"}).json()["id"]
    assert send(a, main, "vie", "Préviens Marie pour le déjeuner #famille").status_code == 200
    page = reply_in(a, main, "vie", "rien ne part sans toi")
    displayable.check(a, page, "vie")
    news = a.get("/v1/family/news", headers=AUTH, params={"agent": "vie"}).json()
    assert not shown.family_news_problems(news, proposals=1), shown.family_news_problems(news, proposals=1)
    [proposal] = news["proposals"]
    assert proposal["to"] == [{"id": "marie", "name": "Marie"}] and "pizzas" in proposal["text"], proposal
    assert b.get("/v1/family/news", headers=AUTH, params={"agent": "coach"}).json()["received"] == [], \
        "a family message left before the person's « Envoyer »"
    assert not [x for x in a.get("/v1/approvals/auto", headers=AUTH).json()["items"] if x["kind"] == "family_send"]

    sent = a.post(f"/v1/family/proposals/{proposal['id']}/send", headers=AUTH,
                  json={"text": "Pizzas ce midi, tu en veux une ?", "to": ["marie"]})
    assert sent.status_code == 200 and sent.json() == {"sent": ["Marie"], "failed": []}, sent.text
    assert a.get("/v1/family/news", headers=AUTH, params={"agent": "vie"}).json()["proposals"] == []
    received = b.get("/v1/family/news", headers=AUTH, params={"agent": "coach"}).json()
    assert not shown.family_news_problems(received, received=1), shown.family_news_problems(received, received=1)
    note = received["received"][0]
    assert (note["from"], note["from_agent"], note["text"]) == ("Alex", "Vie", "Pizzas ce midi, tu en veux une ?")
    rows = b.get("/v1/notifications", headers=AUTH, params={"limit": 50}).json()["notifications"]
    assert [r for r in rows if r["type"] == "family"], [r["type"] for r in rows]
    assert b.post(f"/v1/family/received/{note['id']}/seen", headers=AUTH).status_code == 200   # « OK »
    assert b.get("/v1/family/news", headers=AUTH, params={"agent": "coach"}).json()["received"] == []


def day(n):
    return (datetime.date.today() + datetime.timedelta(days=n)).isoformat()


def test_j37_finding_a_date_with_another_household(homes):
    """« Trouver une date »: Alex offers the evenings he is free; Marie's Boîte shows « Dîner » from Alex with the
    dates free for both (her own events never cross); she answers, Alex keeps one, she says « D'accord »: booked in
    both calendars."""
    from test_rooms_family_followups import calendar

    a, b = homes.alex, homes.marie
    calendar(a, [("Cinéma", f"{day(3)}T19:00:00+00:00", f"{day(3)}T23:00:00+00:00")])
    calendar(b, [("Anniversaire de Paul", f"{day(4)}T18:00:00+00:00", f"{day(4)}T22:00:00+00:00")])
    draft = a.post("/v1/meet", headers=AUTH, json={"household": "marie", "title": "Dîner", "from": day(3), "to": day(5),
                                                    "part": "evening", "minutes": 120})
    assert draft.status_code == 200, draft.text
    m = draft.json()
    assert m["offered"], m
    assert a.post(f"/v1/meet/{m['id']}/send", headers=AUTH, json={"slots": m["offered"]}).status_code == 200
    [theirs] = b.get("/v1/meet", headers=AUTH).json()["meets"]
    assert (theirs["state"], theirs["household_name"], theirs["title"]) == ("offered", "Alex", "Dîner"), theirs
    assert theirs["free"] == [f"{day(5)}T19:00:00+00:00"], theirs
    assert "Anniversaire" not in a.get("/v1/meet", headers=AUTH).text and "Cinéma" not in b.get("/v1/meet", headers=AUTH).text
    rows = b.get("/v1/notifications", headers=AUTH, params={"limit": 50}).json()["notifications"]
    assert [r for r in rows if r["type"] == "meet"], [r["type"] for r in rows]   # her Boîte
    assert b.post(f"/v1/meet/{m['id']}/answer", headers=AUTH, json={"slots": theirs["free"]}).status_code == 200
    chosen = a.post(f"/v1/meet/{m['id']}/choose", headers=AUTH, json={"slot": theirs["free"][0]})
    assert chosen.status_code == 200 and chosen.json()["state"] == "confirmed", chosen.text
    accepted = b.post(f"/v1/meet/{m['id']}/accept", headers=AUTH)
    assert accepted.status_code == 200 and accepted.json()["state"] == "booked", accepted.text
    for bridge, title in ((a, "Dîner (Marie)"), (b, "Dîner (Alex)")):
        [op] = bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"]
        assert op["args"]["title"] == title and op["args"]["start"] == theirs["free"][0], op


def test_j38_agents_of_two_households_write_to_each_other(homes, fakes):
    """Alex lets Vie write to Coach (Marie), Marie lets Coach take it: Vie's message reaches Coach in their own
    exchange (not Marie's conversation), and Marie's app shows « Vie (Alex) ⇄ Coach · 2 messages · Voir l'échange »,
    then the exchange read-only."""
    from test_households_and_peers import give_peer_token

    a, b = homes.alex, homes.marie
    # Coach's own conversation with Marie, found by its title in the fake's listing (`{"sessions": [...]}`; Hermes
    # v0.22.5 answers `{"object": "list", "data": [...]}`, as test_households_and_peers.py's Hermes: peers.rs
    # `bot_chat` reads both)
    fakes.hermes.add_session("bot-coach", [], title="Bot Chat", agent="coach")
    assert a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie", "to": "coach__marie"}]}).status_code == 200
    assert b.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie__alex", "to": "coach"}]}).status_code == 200
    token = "vie-gateway-token-" + "j" * 22
    give_peer_token(a, "manual:vie", token)
    asked = a.post("/peer/p/coach__marie/v1/runs", headers={"Authorization": f"Bearer {token}"},
                   json={"session_id": "bot-coach", "message": "Message from Vie (@vie): une séance douce samedi ?"})
    assert asked.status_code in (200, 202), asked.text

    def pair():
        pairs = b.get("/v1/agent-pairs", headers=AUTH, params={"agent": "coach"}).json()["pairs"]
        if not pairs:
            return None
        thread = b.get(f"/v1/agent-pairs/{pairs[0]['id']}", headers=AUTH).json()
        return (pairs, thread) if len(thread.get("thread") or []) >= 2 else None
    assert wait_until(pair, timeout=60), b.get("/v1/agent-pairs", headers=AUTH, params={"agent": "coach"}).json()
    pairs, thread = pair()
    assert not shown.pair_problems(pairs, thread), shown.pair_problems(pairs, thread)
    assert [a_["name"] for a_ in pairs[0]["agents"] if a_["id"] == "vie__alex"] == ["Vie (Alex)"], pairs[0]
    ask, answer = thread["thread"][:2]
    assert (ask["kind"], ask["text"]) == ("ask", "une séance douce samedi ?"), thread
    assert answer["kind"] == "answer" and answer["text"].startswith("Bien reçu"), thread
    assert "séance douce" not in str(fakes.hermes.messages.get(("coach", "bot-coach"))), \
        "the exchange landed in Marie's own conversation with Coach"
