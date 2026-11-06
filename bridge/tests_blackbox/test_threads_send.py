"""Sending through the bridge (docs/conversations.md §6.7, threads/send.rs): every message with the app's id
(`client_id`), one route deciding (a turn started, words into the turn going on, the queue); the message shown at once
as an item carrying that id, then as Hermes' stored row with the same id (never matched by the apps); a retry never
runs twice; stop; approvals and questions answered through the bridge, the answer seen by every device (the waiting
card and the answered item share one id, the thread's hint tells the others)."""
import base64
import json
import threading
import time

import httpx

from bbkit.common import AUTH, read_sse, sse
from bbkit.fakes import CLOSE
from bbkit.servers import wait_until

SESSION = "api_main"


def user(text):
    return {"role": "user", "content": text}


def agent(text):
    return {"role": "assistant", "content": text}


def setup(bridges, fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.rounds = 1000
    fakes.hermes.add_session(SESSION, [user("Bonjour"), agent("Salut")], title="Bot Chat", last_active=10, started_at=1)
    bridge = bridges.new().start()
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "wellness"}).json()
    return bridge, main["id"]


def send(bridge, thread, client_id, text="", **extra):
    body = {"agent": "wellness", "client_id": client_id, "text": text, **extra}
    return bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH, json=body)


def page(bridge, thread, **params):
    answer = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": "wellness", **params})
    assert answer.status_code == 200, answer.text
    return answer.json()


def stored(fakes, *rows):
    fakes.hermes.messages[("", SESSION)] += list(rows)


def test_a_message_is_its_app_id_from_the_send_to_the_stored_row(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Voie 4.")]
    sent = send(bridge, thread, "C1D00000-0000-4000-8000-000000000001", "Le train de 9 h ?", kinds=["voice"])
    assert sent.status_code == 200, sent.text
    assert sent.json() == {"client_id": "C1D00000-0000-4000-8000-000000000001", "thread": thread, "state": "started",
                           "run_id": "r1", "tip": SESSION}
    run = fakes.hermes.started["r1"]
    assert run["session_id"] == SESSION and run["input"].endswith("Le train de 9 h ?") and run["now"]
    # at once on every device: the message as an item with the app's id, after everything there is
    before = page(bridge, thread)
    last = before["items"][-1]
    assert last["id"] == "s_C1D00000-0000-4000-8000-000000000001" and last["client_id"] == last["id"][2:]
    assert last["text"] == "Le train de 9 h ?" and last["role"] == "person" and last["sending"] is True
    run = before["pending"]["run"]   # with its start (the working line's clock on every phone)
    assert (run["run_id"], run["status"]) == ("r1", "running") and abs(run["started"] - time.time()) < 60
    # the same message again (an answer lost on the way): nothing sent twice
    again = send(bridge, thread, "C1D00000-0000-4000-8000-000000000001", "Le train de 9 h ?")
    assert again.json()["state"] == "known" and len(fakes.hermes.started) == 1
    # and where it is now (thin apps, brick 3): in the turn going on, which the phone follows again
    assert again.json() == {"client_id": "C1D00000-0000-4000-8000-000000000001", "thread": thread, "state": "known",
                            "run_id": "r1", "tip": SESSION, "queued": False}
    # Hermes stored the turn: the stored row carries the id, `after=` the item before brings it
    stored(fakes, user("<!--bip:now lundi 9 h-->\n\nLe train de 9 h ?"), agent("Voie 4."))
    gate.set()
    # the run's end tells the apps (`thread` hint): what they read then
    assert wait_until(lambda: page(bridge, thread)["items"][-1]["text"] == "Voie 4.", timeout=15)
    after = page(bridge, thread, after=before["items"][-2]["cursor"])
    assert [(i["role"], i["text"]) for i in after["items"]] == [("person", "Le train de 9 h ?"), ("agent", "Voie 4.")]
    assert after["items"][0]["client_id"] == "C1D00000-0000-4000-8000-000000000001"
    assert after["items"][0]["id"].startswith("m_") and "sending" not in after["items"][0]
    # matched once, kept: the same on every read (a « Le train de 9 h ? » sent later is another message)
    assert page(bridge, thread)["items"][-2]["client_id"] == "C1D00000-0000-4000-8000-000000000001"


def test_photos_and_documents_go_with_their_message(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Vu.")]
    doc = {"kind": "document", "name": "devis.pdf", "mime": "application/pdf", "size": 12000,
           "path": "/uploads/2026-11/abc-devis.pdf"}
    photo = {"kind": "image", "name": "compteur.jpg", "mime": "image/jpeg", "data": "AAAA"}
    sent = send(bridge, thread, "C1D00000-0000-4000-8000-0000000000A1", "Regarde", attachments=[photo, doc])
    assert sent.status_code == 200, sent.text
    message = fakes.hermes.started["r1"]["input"][0]
    assert message["role"] == "user"
    words, image = message["content"]
    assert words["text"].endswith("Regarde\n\n[Pièce jointe : devis.pdf (application/pdf, 12 Ko) → /uploads/2026-11/abc-devis.pdf]")
    assert image == {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64,AAAA"}}
    shown = page(bridge, thread)["items"][-1]
    assert [a["name"] for a in shown["attachments"]] == ["compteur.jpg", "devis.pdf"]
    assert "data" not in json.dumps(shown["attachments"]), "never the photo's bytes"
    # a message needs words or a file, a photo its bytes
    assert send(bridge, thread, "C1D00000-0000-4000-8000-0000000000A2", "  ").status_code == 422
    assert send(bridge, thread, "C1D00000-0000-4000-8000-0000000000A3", "x", attachments=[{"kind": "image"}]).status_code == 422


def fetch(bridge, path):
    """A file of the timeline, its redirect to `/v1/media` followed."""
    answer = bridge.get(path, headers=AUTH)
    if answer.status_code in (302, 303, 307):
        answer = bridge.get(answer.headers["location"], headers=AUTH)
    return answer


def test_a_photo_sent_alone_shows_with_its_size_and_its_file(bridges, fakes):
    # 6/11 (Alex): every photo sent showed « 0 Ko », nothing to look at — its `s_` item had no size, no file, and the
    # bridge kept no bytes; sent while the agent worked, it waited in « En attente » as an empty row
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Jolie.")]
    jpeg = b"\xff\xd8\xff\xe0" + bytes(range(256)) * 12 + b"\xff\xd9"
    data = base64.b64encode(jpeg).decode()
    photo = {"kind": "image", "name": "IMG_0042.jpg", "mime": "image/jpeg", "data": data}
    sent = send(bridge, thread, "C1D00000-0000-4000-8000-0000000000F1", "", attachments=[photo], kinds=["image"])
    assert sent.status_code == 200, sent.text
    # Hermes still gets the photo itself (after the time's note)
    parts = fakes.hermes.started["r1"]["input"][0]["content"]
    assert parts[-1] == {"type": "image_url", "image_url": {"url": "data:image/jpeg;base64," + data}}
    assert [p["type"] for p in parts].count("image_url") == 1
    # at once: its item with the photo's real size and its file, downloadable
    item = page(bridge, thread)["items"][-1]
    assert item["id"] == "s_C1D00000-0000-4000-8000-0000000000F1" and item["text"] == ""
    shown = item["attachments"][0]
    assert (shown["kind"], shown["mime"], shown["size"]) == ("image", "image/jpeg", len(jpeg))
    assert shown["name"].endswith("IMG_0042.jpg") and shown["path"].startswith("/home/hermes/.hermes/profiles/wellness/uploads/")
    assert "data" not in json.dumps(item), "never the photo's bytes in the timeline"
    got = fetch(bridge, f"/v1/threads/{thread}/attachments/{shown['id']}")
    assert got.status_code == 200 and got.content == jpeg
    # stored by Hermes (its image part only): the stored row, matched by its id, shows the same file
    stored(fakes, {"role": "user", "content": [{"type": "image_url", "image_url": {"url": "data:image/jpeg;base64," + data}}]},
           agent("Jolie."))
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["items"][-1]["text"] == "Jolie.", timeout=15)
    row = page(bridge, thread)["items"][-2]
    assert row["id"].startswith("m_") and row["client_id"] == "C1D00000-0000-4000-8000-0000000000F1"
    kept = row["attachments"][0]
    assert (kept["size"], kept["path"], kept["id"]) == (len(jpeg), shown["path"], row["id"] + ".a0")
    got = fetch(bridge, f"/v1/threads/{thread}/attachments/{kept['id']}")
    assert got.status_code == 200 and got.content == jpeg


def test_a_document_read_back_has_its_size_in_bytes_and_opens(bridges, fakes):
    # 6/11 (J04): once Hermes stored it, the document's size came as words (« 3 Ko ») and both apps rejected the page
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Lu.")]
    pdf = b"%PDF-1.4\n% devis\n" + b"0" * 3210
    up = bridge.post("/v1/files", headers=AUTH, data={"agent": "wellness"},
                     files={"file": ("devis.pdf", pdf, "application/pdf")})
    assert up.status_code == 200, up.text
    path = up.json()["path"]
    doc = {"kind": "document", "name": "devis.pdf", "mime": "application/pdf", "size": len(pdf), "path": path}
    assert send(bridge, thread, "C1D00000-0000-4000-8000-0000000000D1", "Le devis", attachments=[doc]).status_code == 200
    words = fakes.hermes.started["r1"]["input"]
    assert words.endswith(f"[Pièce jointe : devis.pdf (application/pdf, 3 Ko) → {path}]")
    # with a document the bridge cannot see (another place): the line's size, about, still a number
    other = "/elsewhere/x-plan.pdf"
    stored(fakes, user(words), user(f"Et celui-ci\n\n[Pièce jointe : plan.pdf (application/pdf, 1,2 Mo) → {other}]"),
           agent("Lu."))
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["items"][-1]["text"] == "Lu.", timeout=15)
    items = page(bridge, thread)["items"]
    [row] = [i for i in items if i.get("text") == "Le devis"]
    assert row["id"].startswith("m_") and row["client_id"] == "C1D00000-0000-4000-8000-0000000000D1"
    [att] = row["attachments"]
    assert (att["kind"], att["name"], att["path"]) == ("document", "devis.pdf", path)
    assert att["size"] == len(pdf), "the file's own size, a number"
    got = fetch(bridge, att["url"])
    assert got.status_code == 200 and got.content == pdf
    [plan] = [i for i in items if i.get("text") == "Et celui-ci"]
    assert plan["attachments"][0]["size"] == 1_200_000


def test_a_waiting_message_says_its_files_and_its_voice(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Fini.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E1", "Compare trois écoles")
    jpeg = b"\xff\xd8" + b"\x01" * 3000 + b"\xff\xd9"
    photo = {"kind": "image", "name": "IMG_0043.jpg", "mime": "image/jpeg", "data": base64.b64encode(jpeg).decode()}
    queued = send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E2", "", attachments=[photo])
    assert queued.json()["state"] == "queued"
    voice = send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E3", "et appelle le garage", after=True, kinds=["voice"])
    assert voice.json()["state"] == "queued"
    items = bridge.get("/v1/agents/wellness/queue", headers=AUTH).json()["items"]
    assert [i["client_id"] for i in items] == ["C1D00000-0000-4000-8000-0000000000E2", "C1D00000-0000-4000-8000-0000000000E3"]
    files = items[0]["attachments"]
    assert items[0]["text"] == "" and len(files) == 1 and "voice" not in items[0]
    assert (files[0]["kind"], files[0]["size"]) == ("image", len(jpeg)) and files[0]["name"].endswith("IMG_0043.jpg")
    assert "data" not in json.dumps(items), "never the photo's bytes in the queue"
    assert items[1]["voice"] is True and "attachments" not in items[1]
    # the timeline's queue the same
    assert page(bridge, thread)["pending"]["queue"]["items"][0]["attachments"] == files
    gate.set()


def test_words_during_a_turn_go_into_it_files_wait(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Fini.")]
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="La photo.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000000B1", "Compare trois écoles")
    steered = send(bridge, thread, "C1D00000-0000-4000-8000-0000000000B2", "Seulement les publiques")
    assert steered.json()["state"] == "steered" and steered.json()["run_id"] == "r1"
    assert fakes.hermes.steers[-1]["body"] == {"input": "Seulement les publiques"}
    photo = {"kind": "image", "mime": "image/png", "data": "AAAA"}
    queued = send(bridge, thread, "C1D00000-0000-4000-8000-0000000000B3", "Et celle-ci", attachments=[photo])
    assert queued.json()["state"] == "queued"
    # sent again while it waits: known, still in the queue (no turn to follow for it)
    again = send(bridge, thread, "C1D00000-0000-4000-8000-0000000000B3", "Et celle-ci", attachments=[photo]).json()
    assert (again["state"], again["queued"], again["run_id"]) == ("known", True, None)
    now = page(bridge, thread)
    # waiting in the queue (its strip), not as a sent message; the steered one shows at once
    assert [i["client_id"] for i in now["pending"]["queue"]["items"]] == ["C1D00000-0000-4000-8000-0000000000B3"]
    assert [i["id"] for i in now["items"] if i["id"].startswith("s_")] == [
        "s_C1D00000-0000-4000-8000-0000000000B1", "s_C1D00000-0000-4000-8000-0000000000B2"]
    gate.set()
    assert wait_until(lambda: "r2" in fakes.hermes.started, timeout=15)
    assert fakes.hermes.started["r2"]["input"][0]["content"][-1]["image_url"]["url"] == "data:image/png;base64,AAAA"
    assert [i["id"] for i in page(bridge, thread)["items"] if i["id"].startswith("s_")][-1] == \
        "s_C1D00000-0000-4000-8000-0000000000B3"


def test_a_new_conversation_and_stop(bridges, fakes):
    bridge, _ = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.cancelled", run_id="r1")]
    sent = send(bridge, "new", "C1D00000-0000-4000-8000-0000000000C1", "Un voyage à Lisbonne")
    assert sent.status_code == 200, sent.text
    thread = sent.json()["thread"]
    assert thread.startswith("t_") and fakes.hermes.created[-1][1]["title"] == "Un voyage à Lisbonne"
    one = bridge.get(f"/v1/threads/{thread}", headers=AUTH).json()
    assert one["kind"] == "side" and one["tip"] == fakes.hermes.started["r1"]["session_id"]
    stopped = bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={})
    assert stopped.json() == {"stopped": True, "run_id": "r1"}
    assert fakes.hermes.started["r1"]["status"] == "cancelled"
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    assert bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={}).json() == {"stopped": False}



def test_a_new_conversation_with_the_same_first_words_as_an_earlier_one(bridges, fakes):
    """Release bench of 4/11: Hermes refuses a title another conversation holds, and a new conversation is titled by
    its first words: the second « Bonjour » failed (502) and left an empty thread."""
    bridge, _ = setup(bridges, fakes)
    fakes.hermes.unique_titles = True
    threads = []
    for n, run in enumerate(("r1", "r2", "r3"), start=1):
        fakes.hermes.runs[run] = [sse("run.started", run_id=run), sse("run.completed", run_id=run, output="Salut !")]
        sent = send(bridge, "new", f"C1D00000-0000-4000-8000-0000000000D{n}", "Bonjour")
        assert sent.status_code == 200, sent.text
        threads.append(sent.json()["thread"])
    assert len(set(threads)) == 3
    titles = [body["title"] for _, body in fakes.hermes.created[-3:]]
    assert titles == ["Bonjour", "Bonjour (2)", "Bonjour (3)"]
    assert len({fakes.hermes.started[r]["session_id"] for r in ("r1", "r2", "r3")}) == 3

def watch(bridge, thread, hints, stop):
    """Another device showing the conversation: its hints."""
    with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
        with client.stream("GET", "/v1/threads/events", params={"agent": "wellness", "thread": thread}) as resp:
            for line in resp.iter_lines():
                if line.startswith("data: ") and thread in line:
                    hints.append(json.loads(line[len("data: "):]))
                if stop.is_set():
                    return


def test_an_approval_answered_on_one_phone_is_answered_on_the_other(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("approval.request", run_id="r1", request_id="req-1", command="rm -rf build/", choices=["once", "deny"]),
        gate, sse("run.completed", run_id="r1", output="Fait.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000000D1", "Nettoie")
    assert wait_until(lambda: page(bridge, thread)["pending"]["approvals"] != [], timeout=10)
    card = page(bridge, thread)["pending"]["approvals"][0]
    assert card["request_id"] == "req-1" and card["id"].startswith("a_")
    # where the phones draw it (thin apps, brick 5): after the message that started the turn
    assert card["after"] == "s_C1D00000-0000-4000-8000-0000000000D1", card
    hints, stop = [], threading.Event()
    other = threading.Thread(target=watch, args=(bridge, thread, hints, stop), daemon=True)
    other.start()
    time.sleep(0.5)
    answered = bridge.post("/v1/approve", headers=AUTH,
                           json={"agent": "wellness", "run_id": "r1", "request_id": "req-1", "choice": "once"})
    assert answered.status_code == 200, answered.text
    assert wait_until(lambda: any(h.get("reason") == "answered" for h in hints), timeout=10)
    seen = page(bridge, thread)   # what the other phone reads on the hint
    assert seen["pending"]["approvals"] == []
    item = next(i for i in seen["items"] if i["kind"] == "approval")
    assert item["id"] == card["id"] and item["approval"]["state"] == "answered" and item["approval"]["choice"] == "once"
    stop.set()
    gate.set()
    # no phone keeps answers: the old routes are gone
    assert bridge.get("/v1/approvals/answered", headers=AUTH, params={"agent": "wellness"}).status_code in (404, 405)


def test_a_question_waits_on_every_phone_until_answered(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("clarify.request", run_id="r1", prompt_id="q-1", execution_generation=2,
            questions=[{"id": "q", "question": "Quel jour ?", "choices": ["Lundi", "Mardi"]}]),
        gate, sse("run.completed", run_id="r1", output="Lundi, noté.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E1", "Réserve")
    assert wait_until(lambda: page(bridge, thread)["pending"]["questions"] != [], timeout=10)
    question = page(bridge, thread)["pending"]["questions"][0]
    assert question["request_id"] == "q-1" and question["questions"][0]["question"] == "Quel jour ?"
    assert question["after"] == "s_C1D00000-0000-4000-8000-0000000000E1", question   # placed like a card
    assert page(bridge, thread)["pending"]["approvals"] == [], "a question is no approval"
    answered = bridge.post("/v1/approve", headers=AUTH,
                           json={"agent": "wellness", "run_id": "r1", "request_id": "q-1", "answer": "Lundi"})
    assert answered.status_code == 200, answered.text
    assert fakes.hermes.answers[-1]["body"] == {"request_id": "q-1", "execution_generation": 2, "answer": "Lundi"}
    assert page(bridge, thread)["pending"]["questions"] == []
    gate.set()


def test_a_skipped_question_goes_as_hermes_skip_and_closes_everywhere(bridges, fakes):
    """« Passer » (an empty answer from the apps) goes as Hermes' skip word: an empty answer is no reply to Hermes 0.22.5
    (the whole batch ends « timed out »), its skip word lets the next question come. The first choice comes without
    Hermes' English « (Recommended) » label."""
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("clarify.request", run_id="r1", prompt_id="q-1", execution_generation=2,
            question="Quand ?\nReply \"skip\" to skip this question.", choices=["Samedi matin (Recommended)", "Dimanche"]),
        gate, sse("run.completed", run_id="r1", output="D'accord.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E2", "Organise")
    assert wait_until(lambda: page(bridge, thread)["pending"]["questions"] != [], timeout=10)
    question = page(bridge, thread)["pending"]["questions"][0]
    assert question["question"] == "Quand ?" and question["choices"] == ["Samedi matin", "Dimanche"]
    skipped = bridge.post("/v1/approve", headers=AUTH,
                          json={"agent": "wellness", "run_id": "r1", "request_id": "q-1", "answer": ""})
    assert skipped.status_code == 200, skipped.text
    assert fakes.hermes.answers[-1]["body"] == {"request_id": "q-1", "execution_generation": 2, "answer": "\x00skipped"}
    assert page(bridge, thread)["pending"]["questions"] == []
    # gone: a second « Passer » is refused, nothing sent
    again = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "request_id": "q-gone", "answer": ""})
    assert again.status_code == 409 and len(fakes.hermes.answers) == 1
    gate.set()


def test_a_question_hermes_no_longer_waits_for_leaves_every_phone(bridges, fakes):
    """A card whose prompt Hermes settled meanwhile (its turn moved on): the answer is refused (409, nothing sent) and
    the question leaves the thread's `pending.questions`, not shown again on any phone."""
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [
        sse("clarify.request", run_id="r1", prompt_id="q-1", execution_generation=2, question="Quand ?", choices=["Lundi"]),
        gate, sse("run.completed", run_id="r1", output="D'accord.")]
    send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E3", "Organise")
    assert wait_until(lambda: page(bridge, thread)["pending"]["questions"] != [], timeout=10)
    fakes.hermes.run_info["r1"] = {"status": "running", "execution_generation": 2, "pending_controls": []}
    gone = bridge.post("/v1/approve", headers=AUTH,
                       json={"agent": "wellness", "run_id": "r1", "request_id": "q-1", "answer": ""})
    assert gone.status_code == 409 and fakes.hermes.answers == []
    assert page(bridge, thread)["pending"]["questions"] == []
    gate.set()


def test_a_phone_holding_the_sent_item_catches_up_to_one_message(bridges, fakes):
    # 4/11 (iPhone, release fb7195e): the phone read the thread while the turn ran (the send's `thread` hint) and held
    # `s_<client_id>`; the run's end brought, after that item's cursor, the stored row with the same `client_id`. The
    # phone's window must take that row for its `s_` item: one message (ThreadTimeline.apply drops the `s_` copy).
    bridge, thread = setup(bridges, fakes)
    client = "C1D00000-0000-4000-8000-0000000000D5"
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Voie 4.")]
    held = page(bridge, thread)["items"]
    assert send(bridge, thread, client, "Le train de 9 h ?").json()["state"] == "started"
    answer = page(bridge, thread, after=held[-1]["cursor"])
    during = answer["items"]
    assert [i["id"] for i in during] == [f"s_{client}"] and during[0]["client_id"] == client
    # the run going on, with its start: the working line's clock on every phone (thin apps, brick 2)
    run = answer["pending"]["run"]
    assert run["run_id"] == "r1" and run["status"] == "running" and abs(run["started"] - time.time()) < 60
    held += during
    stored(fakes, user("<!--bip:now lundi 9 h-->\n\nLe train de 9 h ?"), agent("Voie 4."))
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["items"][-1]["text"] == "Voie 4.", timeout=15)
    answer = page(bridge, thread, after=held[-1]["cursor"])
    # the `s_` item is gone (stored now): the phone gets the latest page, as its window (thin apps, brick 4)
    assert answer["reset"] is True and answer["joins"] is None
    caught = answer["items"][-2:]
    assert [(i["role"], i["text"], i.get("client_id")) for i in caught] == [
        ("person", "Le train de 9 h ?", client), ("agent", "Voie 4.", None)]
    assert caught[0]["id"].startswith("m_") and "sending" not in caught[0]
    assert [i["id"] for i in answer["items"] if i.get("client_id") == client] == [caught[0]["id"]]
    # and the item it replaces, named: the phones drop it without knowing about `s_` items (thin apps, 5/11)
    assert answer["replaced"] == [f"s_{client}"]
    after = page(bridge, thread, after=caught[-1]["cursor"])
    assert "replaced" not in after and after["joins"] == caught[-1]["cursor"] and after["items"] == []
    # an app from before (its own `limit`): after the `s_` cursor, the stored row and the reply, as before
    old = page(bridge, thread, after=held[-1]["cursor"], limit=500)["items"]
    assert [(i["role"], i.get("client_id")) for i in old] == [("person", client), ("agent", None)]
    # the latest page (a phone opening the thread again) has the message once
    latest = page(bridge, thread)["items"]
    assert [i["id"] for i in latest if i.get("client_id") == client] == [caught[0]["id"]]


PARTIAL = "This turn did not complete. Some actions may already have run; verify their effects before resending."


def test_a_stopped_turn_and_a_peers_message_come_as_decisions(bridges, fakes):
    # thin apps, brick 2: the apps never read Hermes' English failed-turn line nor « Message from … (@x): »; the bridge
    # says which failed turn it is (`stopped`, on the run's end and on the notice) and who wrote a peer's words
    bridge, thread = setup(bridges, fakes)
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), sse("run.completed", run_id="r1", output=PARTIAL), CLOSE]
    assert send(bridge, thread, "C2D00000-0000-4000-8000-0000000000D6", "Range le carnet").json()["state"] == "started"
    ends = []
    with bridge.stream("GET", "/v1/runs/r1/events?agent=wellness", headers=AUTH, timeout=30) as resp:
        for name, data in read_sse(resp):
            if name == "run.completed":
                ends.append(data)
                break
    assert ends and ends[0]["stopped"] == "partial" and ends[0]["output"] == PARTIAL
    stored(fakes, user("Range le carnet"), agent(PARTIAL), user("Message from 🤖 Vie (@vie): Alex dort bien ?"))
    items = page(bridge, thread)["items"]
    notice = next(i for i in items if i["kind"] == "notice")
    assert notice["notice"] == {"code": "failed_turn", "text": PARTIAL, "stopped": "partial"}
    peer = items[-1]
    assert (peer["role"], peer["from"], peer["from_name"], peer["text"]) == ("peer", "vie", "Vie", "Alex dort bien ?")


def test_a_message_steered_into_a_turn_that_stops_is_not_sent_and_goes_again(bridges, fakes):
    # 5/11 ~20:50 (Alex, Android, bridge 7d1154f6): a turn stuck (on a card); « Hello? » sent meanwhile went into it
    # (steered) and the turn never took it; Stop, then « tu as pu avancer ? » got its answer, but « Hello? » stayed at
    # the end of the conversation as a message being sent, on every read. Now: « not sent » once nothing runs, and the
    # same id sent again goes for real (thin apps, brick 4)
    bridge, thread = setup(bridges, fakes)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.cancelled", run_id="r1")]
    fakes.hermes.runs["r2"] = [sse("run.completed", run_id="r2", output="Pas encore.")]
    fakes.hermes.runs["r3"] = [sse("run.completed", run_id="r3", output="Oui, je suis là.")]
    assert send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E1", "Range le carnet").json()["state"] == "started"
    hello = "C1D00000-0000-4000-8000-0000000000E2"
    assert send(bridge, thread, hello, "Hello?").json()["state"] == "steered"
    during = page(bridge, thread)["items"][-1]
    assert during["id"] == f"s_{hello}" and during["sending"] is True and "not_sent" not in during
    assert bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={}).json()["stopped"] is True
    gate.set()
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    # the turn stopped without the steered words (no pending_steer from Hermes): not in the queue
    assert not (page(bridge, thread)["pending"]["queue"] or {}).get("items")
    stored(fakes, user("Range le carnet"))
    assert send(bridge, thread, "C1D00000-0000-4000-8000-0000000000E3", "tu as pu avancer ?").json()["state"] == "started"
    stored(fakes, user("tu as pu avancer ?"), agent("Pas encore."))
    assert wait_until(lambda: page(bridge, thread)["pending"]["run"] is None, timeout=15)
    time.sleep(11)   # a while after it was sent, nothing running: lost
    items = page(bridge, thread)["items"]
    lost = [i for i in items if i["id"] == f"s_{hello}"]
    assert len(lost) == 1 and lost[0]["not_sent"] is True and "sending" not in lost[0]
    assert [i["text"] for i in items if i.get("role") == "person"].count("Hello?") == 1
    # sent again under the same id (« Pas envoyé » tapped): this time for real
    again = send(bridge, thread, hello, "Hello?").json()
    assert again["state"] == "started" and again["run_id"] == "r3", again
    assert fakes.hermes.started["r3"]["input"].endswith("Hello?")
    stored(fakes, user("Hello?"), agent("Oui, je suis là."))
    assert wait_until(lambda: page(bridge, thread)["items"][-1]["text"] == "Oui, je suis là.", timeout=15)
    final = page(bridge, thread)["items"]
    assert not any(i["id"].startswith("s_") for i in final), "a sent message stays « being sent » or « not sent »"
    assert [i.get("client_id") for i in final if i.get("text") == "Hello?"] == [hello]
