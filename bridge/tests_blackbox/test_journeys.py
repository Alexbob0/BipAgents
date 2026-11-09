"""The release journeys, as the apps do them over HTTP (docs/release-journeys.md, column « Pont »): a real bridge, the
fake Hermes answered by the scripted model (bbkit/model.py), and every page the apps would read checked DISPLAYABLE
(bbkit/displayable.py: each photo with its size and its bytes, each file downloadable, each card with its fields).

Alex, 6/11: « Il faudra éviter à tout prix des régressions de ce type-là quand on fait des releases. » Since 3/11
(40794026, messages through the bridge) every photo sent showed « 0 Ko » and never displayed; the earlier tests checked
the model's input only. These tests check what the person would SEE, before and after Hermes stores the message.

Each test is one journey `Jnn` of the list (the same numbers in the doc and in the apps' journeys)."""
import json
import socket
import time

import pytest

from bbkit import displayable
from bbkit.displayable_people import check_page
from bbkit.common import AUTH
from bbkit.journeys import AGENT, journey_bridge, photo
from bbkit.servers import wait_until


@pytest.fixture
def journey(bridges, fakes):
    return journey_bridge(bridges, fakes)


def cid(n: int) -> str:
    return f"J0URNEY0-0000-4000-8000-{n:012d}"


def person_items(page):
    return [i for i in page["items"] if i["kind"] == "message" and i.get("role") == "person"]


def agent_items(page):
    return [i for i in page["items"] if i["kind"] == "message" and i.get("role") == "agent"]


def started(resp):
    assert resp.status_code == 200, resp.text
    body = resp.json()
    assert body["state"] in ("started", "steered", "queued"), body
    return body


def the_message(page, client_id):
    found = [i for i in page["items"] if i.get("client_id") == client_id]
    assert len(found) == 1, (client_id, [(i["id"], i.get("client_id")) for i in page["items"]])
    return found[0]


def reply_done(journey, words):
    def found():
        return [i for i in agent_items(journey.page()) if words in (i.get("text") or "")]
    assert wait_until(found, timeout=60), [i.get("text") for i in journey.page()["items"]]
    return journey.idle()


# -- sending ------------------------------------------------------------------------------------------------------

def test_j01_a_text_message_shows_and_its_reply_streams_word_by_word(journey):
    sent = started(journey.send(cid(1), "Bonjour Vie, tu es là ?"))
    events = journey.run_events(sent["run_id"])
    deltas = [d.get("delta") for name, d in events if name == "message.delta"]
    assert len(deltas) >= 8, events   # word by word, not the whole reply at the end
    assert events[-1][0] == "run.completed", events[-3:]
    page = reply_done(journey, "Bien reçu")
    displayable.check(journey.bridge, page, AGENT)
    mine = the_message(page, cid(1))
    assert mine["text"] == "Bonjour Vie, tu es là ?" and "sending" not in mine
    assert "".join(deltas).strip().startswith("Bien reçu")


def test_j02_a_photo_shows_with_its_size_and_opens(journey):
    """THE regression of 3/11: the photo's bubble showed « 0 Ko » and never the picture."""
    picture = photo(2)
    started(journey.send(cid(2), "", photos=[picture]))
    # at once, before Hermes stores it: the phone (and the person's other devices) draw it from the bridge
    first = journey.page()
    displayable.check(journey.bridge, first, AGENT, only=[the_message(first, cid(2))["id"]])
    page = reply_done(journey, "1 photo")
    mine = the_message(page, cid(2))
    displayable.check(journey.bridge, page, AGENT)
    [att] = mine["attachments"]
    status, body, _ = displayable._download(journey.bridge, att["url"])
    assert status == 200 and body == picture, "the photo shown is not the one sent"


def test_j03_several_photos_each_show_with_their_size(journey):
    pictures = [photo(n) for n in (3, 4, 5)]
    started(journey.send(cid(3), "", photos=pictures))
    first = journey.page()
    displayable.check(journey.bridge, first, AGENT, only=[the_message(first, cid(3))["id"]])
    page = reply_done(journey, "3 photos")
    displayable.check(journey.bridge, page, AGENT)
    atts = the_message(page, cid(3))["attachments"]
    assert [a["kind"] for a in atts] == ["image"] * 3
    shown = [displayable._download(journey.bridge, a["url"])[1] for a in atts]
    assert shown == pictures, "the photos are not the ones sent, in their order"


def test_j04_a_document_shows_with_its_size_and_opens(journey):
    pdf = b"%PDF-1.4\n% devis\n" + b"0" * 3000
    stored = journey.upload("devis-cuisine.pdf", pdf, "application/pdf")
    doc = {"name": "devis-cuisine.pdf", "mime": "application/pdf", "size": len(pdf), "path": stored["path"]}
    started(journey.send(cid(4), "Tu peux regarder ce devis ?", documents=[doc]))
    first = journey.page()
    displayable.check(journey.bridge, first, AGENT, only=[the_message(first, cid(4))["id"]])
    page = reply_done(journey, "1 document")
    displayable.check(journey.bridge, page, AGENT)
    [att] = the_message(page, cid(4))["attachments"]
    assert att["kind"] == "document" and att["name"] == "devis-cuisine.pdf"
    assert displayable._media(journey.bridge, att["path"], AGENT)[1] == pdf


def test_j05_a_voice_note_shows_its_words(journey):
    """The audio stays on the phone (VoiceNoteStore / VoiceNotes.kt): the bridge gets its words, marked `voice`."""
    started(journey.send(cid(5), "Rappelle-moi d’appeler le garage demain matin", kinds=["voice"]))
    page = reply_done(journey, "Bien reçu")
    displayable.check(journey.bridge, page, AGENT)
    assert the_message(page, cid(5))["text"] == "Rappelle-moi d’appeler le garage demain matin"


def test_j06_a_photo_with_words_shows_both(journey):
    started(journey.send(cid(6), "Le compteur ce matin", photos=[photo(6)]))
    first = journey.page()
    displayable.check(journey.bridge, first, AGENT, only=[the_message(first, cid(6))["id"]])
    page = reply_done(journey, "1 photo")
    displayable.check(journey.bridge, page, AGENT)
    mine = the_message(page, cid(6))
    assert mine["text"] == "Le compteur ce matin" and [a["kind"] for a in mine["attachments"]] == ["image"]


# -- receiving ----------------------------------------------------------------------------------------------------

def test_j07_a_reply_with_a_file_opens(journey):
    started(journey.send(cid(7), "Envoie-moi le rapport #fichier"))
    page = reply_done(journey, "Voici le rapport")
    displayable.check(journey.bridge, page, AGENT)
    [reply] = [i for i in agent_items(page) if "Voici le rapport" in i["text"]]
    [att] = reply["attachments"]
    assert att["kind"] == "file" and att["name"].endswith(".pdf") and "MEDIA:" not in reply["text"]
    assert displayable._media(journey.bridge, att["path"], AGENT)[1].startswith(b"%PDF")


def test_j08_a_reply_with_a_page_opens(journey):
    started(journey.send(cid(8), "Fais-moi une page de résumé #page"))
    page = reply_done(journey, "J’ai préparé une page")
    displayable.check(journey.bridge, page, AGENT)
    [reply] = [i for i in agent_items(page) if "préparé une page" in i["text"]]
    pages = [a for a in reply["attachments"] if a["kind"] == "page"]
    assert len(pages) == 1, reply
    assert b"sum" in displayable._media(journey.bridge, pages[0]["path"], AGENT)[1]   # « Résumé »


def test_j09_a_reply_with_a_map_shows(journey):
    started(journey.send(cid(9), "Montre-moi le trajet #carte"))
    page = reply_done(journey, "Voici la carte")
    displayable.check(journey.bridge, page, AGENT)
    [reply] = [i for i in agent_items(page) if "Voici la carte" in i["text"]]
    # map_image gives a map page (`/bipagents/pages/<agent>/carte-….html`), drawn as a live map by both apps
    # (MapCard.isMapPage / AgentMaps); a picture under `/bipagents/maps/` is a `map`
    [att] = reply["attachments"]
    assert att["kind"] == "map" or (att["kind"] == "page" and "/carte" in att["path"]), reply


def test_j09_a_trips_page_holding_its_map_shows_the_map_once(journey):
    # 7/11 (Alex's Fairphone): the map's card pasted into the trip's page (as the agents are told), the page's line in
    # the reply; the bridge handed over the map's own page at the end too (made_files.rs): the map twice
    started(journey.send(cid(91), "Prépare-moi la balade #trajet"))
    page = reply_done(journey, "Voici ton trajet")
    # two calls are no steps card any more (steps.rs: from 3 useful steps); the check knows `steps` items if any
    check_page(journey.bridge, page, AGENT)
    [reply] = [i for i in agent_items(page) if "Voici ton trajet" in i["text"]]
    [att] = reply["attachments"]
    assert att["kind"] == "page" and "/trajet-" in att["path"], reply


def test_j10_the_agents_steps_show_between_its_words(journey):
    started(journey.send(cid(10), "Les horaires de la piscine ? #outil"))
    page = reply_done(journey, "Bien reçu")
    displayable.check(journey.bridge, page, AGENT)
    turn = page["items"][[i.get("client_id") for i in page["items"]].index(cid(10)):]
    kinds = [(i["kind"], (i.get("tool") or {}).get("name")) for i in turn]
    assert ("tool", "web_search") in kinds, kinds
    assert turn[-1]["kind"] == "message" and turn[-1]["role"] == "agent", kinds


# -- cards --------------------------------------------------------------------------------------------------------

def waiting(journey, key, prefix="", timeout=30):
    def found():
        return [c for c in journey.page()["pending"].get(key) or [] if c.get("run_id", "").startswith(prefix)]
    assert wait_until(found, timeout=timeout), journey.page()["pending"]
    return found()[0]


def test_j11_a_question_shows_its_choices_and_the_answer_goes_on(journey):
    run = started(journey.send(cid(11), "Réserve un train #question"))["run_id"]
    question = waiting(journey, "questions")
    displayable.check(journey.bridge, journey.page(), AGENT)
    answered = journey.bridge.post("/v1/approve", headers=AUTH, json={
        "agent": AGENT, "run_id": run, "request_id": question["request_id"], "answer": "Le 9 h"})
    assert answered.status_code == 200, answered.text
    page = reply_done(journey, "Noté : Le 9 h")
    assert not page["pending"].get("questions")
    displayable.check(journey.bridge, page, AGENT)


def test_j12_an_approval_shows_what_it_asks_and_its_answer_stays(journey):
    run = started(journey.send(cid(12), "Fais le ménage #approbation"))["run_id"]
    card = waiting(journey, "approvals")
    assert "brouillons" in (card.get("command") or "") + (card.get("description") or "")
    displayable.check(journey.bridge, journey.page(), AGENT)
    answered = journey.bridge.post("/v1/approve", headers=AUTH, json={
        "agent": AGENT, "run_id": run, "request_id": card["request_id"], "choice": "once"})
    assert answered.status_code == 200, answered.text
    page = reply_done(journey, "le dossier est supprimé")
    assert not page["pending"].get("approvals")
    displayable.check(journey.bridge, page, AGENT)
    kept = [i for i in page["items"] if i["kind"] == "approval"]
    assert kept and kept[-1]["approval"]["choice"] == "once", "the answered card is not in the conversation"


def test_j13_a_secret_card_shows_in_the_conversation_and_goes_once_typed(journey):
    started(journey.send(cid(13), "Connecte-toi à mon compte #secret"))
    card = waiting(journey, "approvals", "bip-secret-")
    displayable.check(journey.bridge, journey.page(), AGENT)
    # a phone catching up on the card's hint: placed with no item, nothing comes after its last one (7/11: that page
    # was a 500, the iPhone never showed the card and said « Ton ordinateur a eu un souci »)
    caught_up = journey.page(after=journey.page()["items"][-1]["cursor"])
    assert [c["request_id"] for c in caught_up["pending"]["approvals"]
            if c["run_id"].startswith("bip-secret-")] == [card["request_id"]], caught_up["pending"]
    typed = journey.bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                                json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
    assert typed.status_code == 200, typed.text
    page = reply_done(journey, "Bien reçu")
    assert not [c for c in page["pending"].get("approvals") or [] if c["run_id"].startswith("bip-secret-")]
    displayable.check(journey.bridge, page, AGENT)


def test_j14_a_sites_card_shows_its_hosts(journey):
    started(journey.send(cid(14), "Installe Garmin #sites"))
    card = waiting(journey, "approvals", "bip-site-")
    assert card["bip_sites"] == ["connect.garmin.example"], card
    displayable.check(journey.bridge, journey.page(), AGENT)
    done = journey.bridge.post(f"/v1/agents/{AGENT}/sandbox/sites/requests/{card['request_id']}", headers=AUTH,
                               json={"allow": True, "hosts": card["bip_sites"]})
    assert done.status_code == 200, done.text
    assert wait_until(lambda: not [c for c in journey.page()["pending"].get("approvals") or []
                                   if c["run_id"].startswith("bip-site-")], timeout=15)
    displayable.check(journey.bridge, journey.idle(), AGENT)


def test_j15_a_connector_card_shows_and_is_answered(journey):
    started(journey.send(cid(15), "Ajoute le connecteur #connecteur"))
    card = waiting(journey, "approvals", "bip-conn-")
    displayable.check(journey.bridge, journey.page(), AGENT)
    done = journey.bridge.post(f"/v1/connectors/requests/{card['request_id']}", headers=AUTH, json={"allow": True})
    assert done.status_code == 200, done.text
    page = reply_done(journey, "Bien reçu")
    assert not [c for c in page["pending"].get("approvals") or [] if c["run_id"].startswith("bip-conn-")]
    displayable.check(journey.bridge, page, AGENT)


def test_j16_a_card_answered_from_the_boite_leaves_the_conversation(journey):
    started(journey.send(cid(16), "Connecte-toi #secret"))
    card = waiting(journey, "approvals", "bip-secret-")
    rows = journey.bridge.get("/v1/notifications", headers=AUTH, params={"limit": 50}).json()["notifications"]
    assert [r for r in rows if r["type"] == "secret"], rows   # the Boîte shows it too
    typed = journey.bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                                json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
    assert typed.status_code == 200, typed.text
    assert wait_until(lambda: not [c for c in journey.page()["pending"].get("approvals") or []
                                   if c["run_id"].startswith("bip-secret-")], timeout=15)
    assert wait_until(lambda: [r for r in journey.bridge.get("/v1/notifications", headers=AUTH, params={"limit": 50})
                               .json()["notifications"] if r["type"] == "secret"][0]["read_at"], timeout=10)


def test_j17_a_card_after_stop_leaves_a_conversation_that_still_shows(journey):
    started(journey.send(cid(17), "Fais le ménage #approbation"))
    waiting(journey, "approvals")
    stopped = journey.bridge.post(f"/v1/threads/{journey.thread}/stop", headers=AUTH, json={"agent": AGENT})
    assert stopped.status_code == 200 and stopped.json()["stopped"] is True, stopped.text
    page = journey.idle()
    assert not page["pending"].get("approvals"), "a card of a stopped turn still waits"
    displayable.check(journey.bridge, page, AGENT)
    # the conversation goes on
    started(journey.send(cid(170), "On reprend ?"))
    displayable.check(journey.bridge, reply_done(journey, "Bien reçu"), AGENT)


# -- « Pas envoyé », « En attente » ------------------------------------------------------------------------------

def thread_hints(journey, seconds):
    """The `thread` hints of the conversation's event stream for `seconds` (what an open conversation is told)."""
    import threading
    import httpx
    hints, stop = [], threading.Event()

    def watch():
        with httpx.Client(base_url=journey.bridge.url, headers=AUTH, timeout=30) as client:
            with client.stream("GET", "/v1/threads/events", params={"agent": AGENT, "thread": journey.thread}) as resp:
                name = None
                for line in resp.iter_lines():
                    if stop.is_set():
                        return
                    if line.startswith("event: "):
                        name = line[7:]
                    elif line.startswith("data: ") and name == "thread":
                        hints.append(json.loads(line[6:]))
    threading.Thread(target=watch, daemon=True).start()
    return hints, stop


def test_j18_a_message_no_turn_took_shows_not_sent_and_goes_again(journey):
    started(journey.send(cid(18), "Raconte-moi tout #lent"))
    assert journey.send(cid(180), "Hello?").json()["state"] == "steered"
    # a phone holding the conversation: its last item is the message being sent
    held = the_message(journey.page(), cid(180))
    assert held.get("sending") is True, held
    hints, stop_watching = thread_hints(journey, 20)
    assert journey.bridge.post(f"/v1/threads/{journey.thread}/stop", headers=AUTH,
                               json={"agent": AGENT}).json()["stopped"] is True
    journey.idle()
    # a while after it was sent, nothing running: « Pas envoyé », and the open conversation is told (6/11: Android
    # never showed it, nothing told the phone and its catch-up after that item brought nothing)
    assert wait_until(lambda: any(h.get("reason") == "lost" for h in hints), timeout=20), hints
    stop_watching.set()
    caught_up = journey.page(after=held["cursor"])
    assert [i.get("not_sent") for i in caught_up["items"] if i.get("client_id") == cid(180)] == [True], caught_up
    page = journey.page()
    lost = the_message(page, cid(180))
    assert lost.get("not_sent") is True, lost
    displayable.check(journey.bridge, page, AGENT)
    again = started(journey.send(cid(180), "Hello?"))
    assert again["state"] == "started", again
    page = reply_done(journey, "Bien reçu")
    assert the_message(page, cid(180)).get("not_sent") is None
    displayable.check(journey.bridge, page, AGENT)


def test_j18_a_deleted_conversation_forgets_its_messages_not_sent(journey):
    """6/11: « Pas envoyé » messages of a deleted conversation came back in the main thread started again (kept two
    days in `thread_sends`, by the thread's id the main one keeps)."""
    started(journey.send(cid(181), "Raconte-moi tout #lent"))
    assert journey.send(cid(182), "Hello?").json()["state"] == "steered"
    journey.bridge.post(f"/v1/threads/{journey.thread}/stop", headers=AUTH, json={"agent": AGENT})
    journey.idle()
    time.sleep(11)
    assert the_message(journey.page(), cid(182)).get("not_sent") is True
    deleted = journey.bridge.delete(f"/v1/threads/{journey.thread}", headers=AUTH, params={"agent": AGENT})
    assert deleted.status_code == 204, deleted.text
    assert journey.page()["items"] == [], journey.page()["items"]
    started(journey.send(cid(183), "On recommence"))
    page = reply_done(journey, "Bien reçu")
    assert [i.get("client_id") for i in person_items(page)] == [cid(183)], person_items(page)


def test_j19_the_queue_shows_each_kind_waiting(journey):
    """« En attente »: a photo, a document, words sent with « Après » while the agent answers: each row has
    something to show (a photo alone showed an empty row)."""
    run = started(journey.send(cid(19), "Raconte-moi tout #lent"))["run_id"]
    # a phone reopened mid-turn on its copy: its catch-up, with nothing new, says the turn goes on (7/11: that page
    # was a 500, the iPhone never showed the answer going on nor its « Arrêter »)
    caught_up = journey.page(after=journey.page()["items"][-1]["cursor"])
    assert (caught_up["pending"].get("run") or {}).get("run_id") == run, caught_up
    assert journey.send(cid(191), "", photos=[photo(19)]).json()["state"] == "queued"
    pdf = b"%PDF-1.4\n% facture\n" + b"1" * 800
    stored = journey.upload("facture.pdf", pdf, "application/pdf")
    doc = {"name": "facture.pdf", "mime": "application/pdf", "size": len(pdf), "path": stored["path"]}
    assert journey.send(cid(192), "", documents=[doc]).json()["state"] == "queued"
    assert journey.send(cid(193), "Et après, la météo", after=True).json()["state"] == "queued"
    queue = journey.page()["pending"]["queue"]
    assert len(queue["items"]) == 3, queue
    displayable.check(journey.bridge, journey.page(), AGENT)
    journey.bridge.post(f"/v1/threads/{journey.thread}/stop", headers=AUTH, json={"agent": AGENT})


def test_j19_a_message_whose_phone_goes_away_as_it_is_sent_still_gets_its_turn(journey):
    """J19 closes the app right after « Envoyer »: a phone gone mid-send (killed, its network lost) must not leave its
    message half-sent. 7/11: the request dropped with its connection, the message kept « being sent » with no turn
    started (cut 10 ms in), or not kept at all."""
    body = json.dumps({"agent": AGENT, "client_id": cid(194), "text": "Bonjour, tu es là ?", "phone": "ios"}).encode()
    with socket.create_connection(("127.0.0.1", journey.bridge.port), timeout=10) as phone:
        phone.sendall((f"POST /v1/threads/{journey.thread}/messages HTTP/1.1\r\nHost: 127.0.0.1\r\n"
                       f"Authorization: {AUTH['Authorization']}\r\nContent-Type: application/json\r\n"
                       f"Content-Length: {len(body)}\r\n\r\n").encode() + body)
    # gone at once: the bridge carries the message out whole all the same
    page = reply_done(journey, "Bien reçu")
    assert the_message(page, cid(194)).get("not_sent") is None
    displayable.check(journey.bridge, page, AGENT)


# -- the Boîte ---------------------------------------------------------------------------------------------------

def test_j20_a_routines_report_shows_in_the_boite_and_the_conversation(journey):
    made = journey.bridge.post(f"/v1/agents/{AGENT}/routines", headers=AUTH,
                               json={"name": "Le point", "schedule": "every day at 7:00", "prompt": "Fais le point #fichier"})
    assert made.status_code == 200, made.text
    assert journey.bridge.post(f"/v1/routines/{made.json()['job']['id']}/run", headers=AUTH).status_code == 200

    def boite():
        rows = journey.bridge.get("/v1/notifications", headers=AUTH, params={"limit": 50}).json()["notifications"]
        return [r for r in rows if r["type"] == "boite"]
    assert wait_until(boite, timeout=60), "no report in the Boîte"
    unread = journey.bridge.get("/v1/notifications/unread", headers=AUTH).json()
    assert unread["total"] >= 1, unread
    for thread in {r.get("c") for r in boite() if r.get("c")}:
        displayable.check(journey.bridge, journey.page(thread), AGENT)
    # its reading is « Écouter » (`audio`), never one of its files: only the PDF it made (Alex 8/11: a file to play)
    [row] = boite()
    assert row["boite"]["has_audio"] is True, row["boite"]
    assert [a["kind"] for a in row["boite"]["attachments"]] == ["file"], row["boite"]
    [item] = [i for i in journey.page(row["c"])["items"] if i["kind"] == "boite"]
    assert item["boite"]["audio"] is True and [a["kind"] for a in item["attachments"]] == ["file"], item


def test_j20_a_routines_podcast_is_its_one_audio_file(journey):
    """Alex 8/11: a podcast in two audio files, the reading of the report's text and the podcast. A report bringing its
    own audio gets no reading on top: one audio file, the podcast, and « Écouter » reads its words on the phone."""
    made = journey.bridge.post(f"/v1/agents/{AGENT}/routines", headers=AUTH,
                               json={"name": "Podcast du matin", "schedule": "every day at 7:00",
                                     "prompt": "Fais le podcast du matin #podcast"})
    assert made.status_code == 200, made.text
    assert journey.bridge.post(f"/v1/routines/{made.json()['job']['id']}/run", headers=AUTH).status_code == 200

    def boite():
        rows = journey.bridge.get("/v1/notifications", headers=AUTH, params={"limit": 50}).json()["notifications"]
        return [r for r in rows if r["type"] == "boite" and r["boite"]["title"] == "Podcast du matin"]
    assert wait_until(boite, timeout=60), "no report in the Boîte"
    [row] = boite()
    assert row["boite"]["has_audio"] is False, row["boite"]
    assert [a["kind"] for a in row["boite"]["attachments"]] == ["audio"], row["boite"]
    page = journey.page(row["c"])
    [item] = [i for i in page["items"] if i["kind"] == "boite" and i.get("title") == "Podcast du matin"]
    assert item["boite"]["audio"] is False and [a["kind"] for a in item["attachments"]] == ["audio"], item
    displayable.check(journey.bridge, page, AGENT)


# -- the screen during a turn -------------------------------------------------------------------------------------

def test_j45_a_turn_in_a_conversation_of_several_days(journey):
    """J45, the bridge's half (the apps check the screen): three days of history whose replies carry reasoning; while a
    GLM-paced reply streams, every page read keeps the other days above the person's message, and the reply comes in
    chunks of 1–3 words, stored with its reasoning."""
    started(journey.send(cid(45), "On reprend ? #jours"))
    page = reply_done(journey, "Noté, je garde")
    days = [i["day"] for i in page["items"] if i.get("day")]
    assert len(set(days)) == 3 and days == sorted(days), days
    assert sum(1 for i in agent_items(page) if i.get("reasoning")) >= 3, page["items"]
    displayable.check(journey.bridge, page, AGENT)
    run = started(journey.send(cid(46), "Un week-end à Lyon ? #reel"))["run_id"]
    pages, until = [], time.monotonic() + 60
    while journey.page()["pending"].get("run") is not None and time.monotonic() < until:
        pages.append(journey.page())
        time.sleep(0.3)
    for seen in pages + [journey.idle()]:
        ids = [i.get("client_id") for i in seen["items"]]
        after = seen["items"][ids.index(cid(46)):] if cid(46) in ids else []
        assert all(i.get("day", days[-1]) == days[-1] for i in after), [(i["id"], i.get("day")) for i in after]
    # as Hermes streamed them (the bridge's replay of an ended run gives its words whole)
    hermes_run = journey.model.runs.get(run) or list(journey.model.runs.values())[-1]
    events = [json.loads(e.split("data: ", 1)[1]) for e in hermes_run.events]
    deltas = [e["delta"] for e in events if e["event"] == "message.delta"]
    assert len(deltas) >= 15 and all(1 <= len(d.split()) <= 3 for d in deltas), deltas
    [reply] = [i for i in agent_items(journey.page()) if "Croix-Rousse" in (i.get("text") or "")]
    assert reply.get("reasoning") and 40 <= len(reply["text"].split()) <= 60, reply
