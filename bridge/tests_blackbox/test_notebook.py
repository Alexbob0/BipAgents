"""The Carnet (notebook/ in the Rust bridge, 2026-10-22): the agents' tools over MCP, the Markdown pages on disk, the
app's routes (home, search with a short answer, page, edit, history and restore, export), tasks linked to the phone's
reminders both ways, the private health part, and the nightly pass: its free pre-filter, one call to the small model,
its cost and its monthly cap."""
import io
import json
import re
import time
import zipfile
from datetime import datetime, timezone

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from conftest import managed_install, profile

TODAY = datetime.now(timezone.utc).strftime("%Y-%m-%d")


def notebook(bridge):
    return bridge.state_dir / "notebook"


def in_french(bridge):
    settings = bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH)
    assert settings.status_code == 200 and settings.json()["language"] == "fr"


def marc(bridge):
    return call(bridge, "notebook_note", {"page": "Marc Dupont", "kind": "person", "section": "Infos",
                                          "facts": ["Anniversaire : 14 novembre"], "summary": "Ami d'enfance.",
                                          "aliases": ["Marco"], "contact": "C-1"})


def test_an_agent_files_facts_that_read_well_in_obsidian(bridge):
    give_token(bridge)
    names = [t["name"] for t in rpc(bridge, "tools/list").json()["result"]["tools"]]
    assert {"notebook_search", "notebook_read", "notebook_note", "notebook_edit", "notebook_task"} <= set(names)
    in_french(bridge)
    filed = marc(bridge)
    assert filed["isError"] is False
    assert text_of(filed).startswith("Filed in the Carnet: Marc Dupont (new page, Personnes/Marc Dupont.md), 1 fact(s).")
    assert "« Noté dans ton Carnet : Marc Dupont — Anniversaire : 14 novembre »" in text_of(filed)
    page = (notebook(bridge) / "carnet" / "Personnes" / "Marc Dupont.md").read_text()
    assert re.fullmatch(
        r"---\nid: p[0-9a-f]{10}\ntype: person\naliases:\n  - Marco\ncontact: C-1\nupdated: " + TODAY + r"\n---\n"
        r"# Marc Dupont\n\n## En bref\nAmi d'enfance\.\n\n## Dates\n- Anniversaire : 14 novembre — \*Wellness, "
        + TODAY + r"\*\n", page), page
    # Found again by a nickname or the contact: the same page, nothing repeated.
    again = call(bridge, "notebook_note", {"page": "Marco", "facts": ["anniversaire: 14 Novembre", "Aime le jazz"]})
    assert "Marc Dupont (updated" in text_of(again) and "1 fact(s)" in text_of(again)
    assert text_of(call(bridge, "notebook_note", {"page": "C-1", "facts": ["Aime le jazz"]})).startswith("Already in the Carnet")
    found = text_of(call(bridge, "notebook_search", {"words": "marco"}))
    assert found.startswith("Marc Dupont (Personnes/Marc Dupont.md) — also: Marco")
    assert "jazz" in text_of(call(bridge, "notebook_read", {"page": "marc dupont"}))
    fixed = call(bridge, "notebook_edit", {"page": "Marc Dupont", "old": "14 novembre", "new": "Anniversaire : 15 novembre",
                                           "summary": "Ami d'enfance, fan de jazz."})
    assert text_of(fixed).startswith("Corrected in the Carnet: Marc Dupont (Anniversaire : 14 novembre → Anniversaire : 15 novembre).")
    page = (notebook(bridge) / "carnet" / "Personnes" / "Marc Dupont.md").read_text()
    assert "## En bref\nAmi d'enfance, fan de jazz.\n\n## Dates\n- Anniversaire : 15 novembre — *Wellness" in page
    # One page outline: the jazz under « Goûts et préférences », after « Dates »; a section is rewritten by its id.
    assert page.index("## Dates") < page.index("## Goûts et préférences\n- Aime le jazz")
    rewritten = call(bridge, "notebook_edit", {"page": "Marc Dupont", "section": "who", "text": "- Voisin, rencontré en 2010"})
    assert text_of(rewritten).startswith("Corrected in the Carnet: Marc Dupont (Qui c'est : - Voisin")
    page = (notebook(bridge) / "carnet" / "Personnes" / "Marc Dupont.md").read_text()
    assert page.index("## En bref") < page.index("## Qui c'est") < page.index("## Dates")
    wrong = call(bridge, "notebook_edit", {"page": "Marc Dupont", "section": "Voyages", "text": "- Japon"})
    assert wrong["isError"] and "one of who, dates" in text_of(wrong)
    # The person turned the mentions off: the agent is told to say nothing.
    bridge.patch("/v1/notebook/settings", json={"mentions": False}, headers=AUTH)
    quiet = call(bridge, "notebook_note", {"page": "Rénovation cuisine", "kind": "subject", "section": "Décisions",
                                           "facts": ["Devis plombier accepté : 480 € [[Marc Dupont]]"]})
    assert "turned these mentions off" in text_of(quiet) and "Noté" not in text_of(quiet)
    # Health is not this agent's: refused.
    for args in ({"page": "Santé", "kind": "area", "facts": ["Tension 12/8"]},
                 {"page": "Marc Dupont", "facts": ["x"], "private": True}):
        refused = call(bridge, "notebook_note", args)
        assert refused["isError"] and "health" in text_of(refused)
    # The language is set once the Carnet has pages.
    assert bridge.patch("/v1/notebook/settings", json={"language": "en"}, headers=AUTH).status_code == 400


def test_the_app_reads_searches_edits_and_restores(bridge):
    give_token(bridge)
    in_french(bridge)
    marc(bridge)
    call(bridge, "notebook_note", {"page": "Rénovation cuisine", "kind": "subject", "section": "Décisions",
                                   "facts": ["Devis accepté avec [[Marco]]"]})
    home = bridge.get("/v1/notebook", headers=AUTH).json()
    counts = {s["kind"]: s["count"] for s in home["sections"]}
    assert counts == {"self": 0, "person": 1, "subject": 1, "trip": 0, "area": 0, "tasks": 0, "documents": 0}
    assert [s["title"] for s in home["sections"]][:4] == ["Toi", "Personnes", "Sujets", "Voyages"]
    # The person themself: their own « Toi » page, never among « Personnes ».
    me = call(bridge, "notebook_note", {"page": "moi", "facts": ["Veut courir un marathon"]})
    assert text_of(me).startswith("Filed in the Carnet: Toi (new page, Toi.md), 1 fact(s). Under: Objectifs.")
    with_me = bridge.get("/v1/notebook", headers=AUTH).json()
    assert with_me["sections"][0] == {"kind": "self", "title": "Toi", "count": 1, "path": "Toi.md"}
    assert with_me["sections"][1]["count"] == 1
    assert [s["id"] for s in bridge.get("/v1/notebook/page", params={"path": "Toi.md"}, headers=AUTH).json()["outline"]] == [
        "about", "family", "work", "habits", "goals", "agents", "other"]
    recent = home["recent"][0]
    assert recent["title"] == "Rénovation cuisine" and recent["name"] == "Wellness" and recent["agent"] == "wellness"
    assert recent["action"] == "create" and recent["summary"] == "Devis accepté avec [[Marco]]"
    # A short answer on top when a fact answers the question.
    found = bridge.get("/v1/notebook/search", params={"q": "anniversaire marc"}, headers=AUTH).json()
    assert found["answers"][0]["answer"] == "14 novembre" and found["answers"][0]["page"] == "Marc Dupont"
    assert found["answers"][0]["by"] == "Wellness" and found["pages"][0]["title"] == "Marc Dupont"
    view = bridge.get("/v1/notebook/page", params={"path": "Personnes/Marc Dupont.md"}, headers=AUTH).json()
    assert view["summary"] == "Ami d'enfance." and view["aliases"] == ["Marco"] and view["contact"] == "C-1"
    assert view["sections"] == [{"id": "dates", "title": "Dates", "facts": [{"text": "Anniversaire : 14 novembre",
                                 "by": "Wellness", "date": TODAY, "item": True, "links": []}]}]
    assert [s["id"] for s in view["outline"]] == ["who", "dates", "family", "work", "tastes", "gifts", "agreed",
                                                   "exchanges", "remember", "other"]
    # « Modifier » on one section: the unchanged line keeps its attribution, the new one is the person's.
    edited = bridge.put("/v1/notebook/section", json={"path": "Personnes/Marc Dupont.md", "section": "dates",
                                                      "text": "- Anniversaire : 14 novembre\n- Fête : 25 avril"}, headers=AUTH)
    assert edited.status_code == 200, edited.text
    facts = edited.json()["sections"][0]["facts"]
    assert [(f["text"], f["by"]) for f in facts] == [("Anniversaire : 14 novembre", "Wellness"), ("Fête : 25 avril", "toi")]
    restored_section = bridge.get("/v1/notebook/history", params={"path": "Personnes/Marc Dupont.md"}, headers=AUTH).json()
    bridge.post("/v1/notebook/restore", json={"change": restored_section["changes"][1]["id"]}, headers=AUTH)
    view = bridge.get("/v1/notebook/page", params={"path": "Personnes/Marc Dupont.md"}, headers=AUTH).json()
    assert view["backlinks"] == [{"title": "Rénovation cuisine", "path": "Sujets/Rénovation cuisine.md", "kind": "subject"}]
    assert view["frontmatter"]["type"] == "person" and view["id"].startswith("p") and view["shared_with"] == []
    subject = bridge.get("/v1/notebook/page", params={"path": "Sujets/Rénovation cuisine.md"}, headers=AUTH).json()
    assert subject["links"] == [{"title": "Marco", "path": "Personnes/Marc Dupont.md", "kind": "person"}]
    assert bridge.get("/v1/notebook/page", params={"path": "../state.json"}, headers=AUTH).status_code == 404
    # The person's own edit, recorded as theirs, then undone.
    edited = view["text"].replace("14 novembre", "16 novembre")
    put = bridge.put("/v1/notebook/page", json={"path": "Personnes/Marc Dupont.md", "text": edited}, headers=AUTH)
    assert put.status_code == 200 and put.json()["change"]["by"] == "you" and put.json()["change"]["name"] == "toi"
    changes = bridge.get("/v1/notebook/history", params={"path": "Personnes/Marc Dupont.md"}, headers=AUTH).json()["changes"]
    assert [(c["action"], c["by"]) for c in changes][:2] == [("edit", "you"), ("restore", "you")]
    assert changes[-1]["action"] == "create"
    restored = bridge.post("/v1/notebook/restore", json={"change": changes[-1]["id"]}, headers=AUTH)
    assert restored.status_code == 200 and restored.json()["change"]["action"] == "restore"
    assert "14 novembre" in (notebook(bridge) / "carnet" / "Personnes" / "Marc Dupont.md").read_text()
    assert bridge.post("/v1/notebook/restore", json={"change": changes[-1]["id"]}, headers=AUTH).status_code == 400
    # Documents: the files the agents were sent, with the pages that name them.
    folder = bridge.root / "uploads" / "wellness" / "2026-10"
    folder.mkdir(parents=True)
    (folder / "0123456789ab-devis-plombier.pdf").write_bytes(b"%PDF-1.4")
    call(bridge, "notebook_note", {"page": "Rénovation cuisine", "facts": ["Le devis-plombier.pdf est signé"]})
    docs = bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"]
    assert [(d["name"], d["type"], d["origin"], d["agent"]) for d in docs] == [
        ("devis-plombier.pdf", "pdf", "received", "wellness")]
    assert docs[0]["subjects"] == [{"title": "Rénovation cuisine", "path": "Sujets/Rénovation cuisine.md", "kind": "subject"}]
    # Export for Obsidian: the vault as a zip, with its Documents page.
    export = bridge.get("/v1/notebook/export", headers=AUTH)
    assert export.status_code == 200 and export.headers["content-type"] == "application/zip"
    assert export.headers["content-disposition"] == 'attachment; filename="Carnet.zip"'
    names = sorted(zipfile.ZipFile(io.BytesIO(export.content)).namelist())
    assert names == ["Documents.md", "Personnes/Marc Dupont.md", "Sujets/Rénovation cuisine.md", "Toi.md"]
    deleted = bridge.delete("/v1/notebook/page", params={"path": "Sujets/Rénovation cuisine.md"}, headers=AUTH)
    assert deleted.status_code == 200 and not (notebook(bridge) / "carnet" / "Sujets" / "Rénovation cuisine.md").exists()
    assert bridge.get("/v1/notebook", headers=AUTH).status_code == 200
    assert bridge.get("/v1/notebook").status_code == 401


def test_tasks_stay_few_and_follow_the_phone_reminders(bridge):
    give_token(bridge)
    in_french(bridge)
    assert bridge.put("/v1/reminders/agents", json={"agents": ["Wellness"]}, headers=AUTH).status_code == 200
    added = call(bridge, "notebook_task", {"action": "add", "title": "Appeler le plombier", "due": "2026-11-03T09:00",
                                           "page": "Maison", "origin": "asked"})
    assert "phone reminders" in text_of(added)
    task_id = re.search(r"id (t[0-9a-f]+)", text_of(added)).group(1)
    undated = call(bridge, "notebook_task", {"action": "add", "title": "Trier les photos"})
    assert "phone reminders" not in text_of(undated)   # no date: in the Carnet only
    queue = bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]
    assert [(op["op"], op["args"]["title"], op["args"]["due"]) for op in queue] == [
        ("add", "Appeler le plombier Maison", "2026-11-03T09:00")]
    assert queue[0]["args"]["notes"] == f"BipAgents · Carnet ^{task_id}"
    tasks_page = (notebook(bridge) / "carnet" / "Tâches" / "Tâches.md").read_text()
    assert f"- [ ] Appeler le plombier [[Maison]] 📅 2026-11-03 — *Wellness, {TODAY}* ^{task_id}" in tasks_page
    # Ticked on the phone: done in the Carnet, nothing sent back.
    bridge.post(f"/v1/reminders/queue/{queue[0]['id']}", json={"done": True}, headers=AUTH)
    snapshot = {"lists": [], "items": [{"id": "r9", "title": "Appeler le plombier", "notes": queue[0]["args"]["notes"],
                                        "completed": True}]}
    bridge.put("/v1/reminders", json=snapshot, headers=AUTH)
    tasks = bridge.get("/v1/notebook/tasks", headers=AUTH).json()
    assert [t["title"] for t in tasks["open"]] == ["Trier les photos"] and tasks["done"][0]["id"] == task_id
    assert tasks["done"][0]["origin"] == "asked"
    assert bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"] == []
    # Done in the Carnet before the phone got it: the request is cancelled. After: the reminder is ticked.
    second = bridge.post("/v1/notebook/tasks", json={"title": "Rendez-vous banque", "due": "2026-11-05"}, headers=AUTH).json()
    assert second["reminder"] is True and second["task"]["origin"] == "asked"
    bridge.post(f"/v1/notebook/tasks/{second['task']['id']}", json={"done": True}, headers=AUTH)
    assert bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"] == []
    third = text_of(call(bridge, "notebook_task", {"action": "add", "title": "Payer la cantine", "due": "2026-11-06",
                                                   "origin": "own"}))
    third_id = re.search(r"id (t[0-9a-f]+)", third).group(1)
    op = bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"][0]
    bridge.post(f"/v1/reminders/queue/{op['id']}", json={"done": True}, headers=AUTH)
    bridge.put("/v1/reminders", json={"lists": [], "items": [{"id": "r10", "title": "Payer la cantine",
                                                              "notes": op["args"]["notes"]}]}, headers=AUTH)
    assert text_of(call(bridge, "notebook_task", {"action": "done", "id": third_id})) == "Done: Payer la cantine."
    asked = bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]
    assert [(o["op"], o["args"]) for o in asked] == [("complete", {"id": "r10"})]
    # Proposals: only with the Carnet's go, a few a week, never a kind the person declined.
    no_id = call(bridge, "notebook_task", {"action": "add", "title": "Cadeau Marc", "origin": "accepted"})
    assert no_id["isError"]
    proposed = text_of(call(bridge, "notebook_task", {"action": "propose", "kind": "birthday", "title": "Rappel anniversaire Marc"}))
    proposal = re.search(r"proposal (q[0-9a-f]+)", proposed).group(1)
    declined = call(bridge, "notebook_task", {"action": "decline", "proposal": proposal, "never": True})
    assert "Pas de propositions de tâches pour les anniversaires" in text_of(declined)
    again = call(bridge, "notebook_task", {"action": "propose", "kind": "birthday", "title": "Rappel anniversaire Paul"})
    assert again["isError"] and "doesn't want proposals of this kind" in text_of(again)
    listed = bridge.get("/v1/notebook/proposals", headers=AUTH).json()
    assert listed["declined"] == ["birthday"] and listed["proposals"][0]["state"] == "declined"
    assert "## Ce que tes agents doivent savoir\n- Pas de propositions" in (notebook(bridge) / "carnet" / "Toi.md").read_text()


def test_a_task_is_added_once_and_takes_its_reminder_when_it_leaves(bridge):
    give_token(bridge)
    in_french(bridge)
    assert bridge.put("/v1/reminders/agents", json={"agents": ["Wellness"]}, headers=AUTH).status_code == 200
    queue = lambda: bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]
    first = text_of(call(bridge, "notebook_task", {"action": "add", "title": "Appeler le dentiste", "due": "2026-11-03"}))
    task_id = re.search(r"id (t[0-9a-f]+)", first).group(1)
    # Asked again while filling the Carnet from memory: the same task, said so, no second reminder.
    again = call(bridge, "notebook_task", {"action": "add", "title": "appeler le Dentiste.", "due": "2026-11-03"})
    assert again["isError"] is False
    assert text_of(again).startswith(f"Already in the Carnet as ^{task_id}: Appeler le dentiste, due 2026-11-03.")
    same = bridge.post("/v1/notebook/tasks", json={"title": "Appeler le dentiste"}, headers=AUTH).json()
    assert same["existing"] is True and same["task"]["id"] == task_id
    assert len(queue()) == 1
    # Removed while its add still waits: cancelled.
    call(bridge, "notebook_task", {"action": "add", "title": "Appeler le garage", "due": "2026-11-04"})
    assert len(queue()) == 2
    call(bridge, "notebook_edit", {"page": "Tâches", "old": "Appeler le garage", "new": ""})
    assert [op["args"]["title"] for op in queue()] == ["Appeler le dentiste"]
    # On the phone: removed by its id and mark (the agent's edit, or the person's in the app).
    op = queue()[0]
    bridge.post(f"/v1/reminders/queue/{op['id']}", json={"done": True}, headers=AUTH)
    mark = f"BipAgents · Carnet ^{task_id}"
    assert op["args"]["notes"] == mark
    bridge.put("/v1/reminders", json={"lists": [], "items": [{"id": "r1", "title": "Appeler le dentiste", "notes": mark}]},
               headers=AUTH)
    page = (notebook(bridge) / "carnet" / "Tâches" / "Tâches.md").read_text()
    edited = "\n".join(line for line in page.splitlines() if task_id not in line) + "\n"
    assert bridge.put("/v1/notebook/page", json={"path": "Tâches/Tâches.md", "text": edited}, headers=AUTH).status_code == 200
    asked = queue()
    assert [(o["op"], o["args"]) for o in asked] == [("remove", {"id": "r1", "mark": mark})]
    # An app from before « remove » answers « unknown request »: the request waits for its update.
    bridge.post(f"/v1/reminders/queue/{asked[0]['id']}", json={"done": False, "error": "unknown request"}, headers=AUTH)
    assert [o["id"] for o in queue()] == [asked[0]["id"]]
    bridge.post(f"/v1/reminders/queue/{asked[0]['id']}", json={"done": True}, headers=AUTH)
    assert queue() == []


def test_reminders_left_by_removed_tasks_are_cleaned_at_start(bridge):
    give_token(bridge)
    in_french(bridge)
    assert bridge.put("/v1/reminders/agents", json={"agents": ["Wellness"]}, headers=AUTH).status_code == 200
    kept = re.search(r"id (t[0-9a-f]+)", text_of(call(bridge, "notebook_task", {
        "action": "add", "title": "Rendez-vous banque", "due": "2026-11-05"}))).group(1)
    # Archived (moved under « Archivées »): not gone, nothing asked.
    op = bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"][0]
    bridge.post(f"/v1/reminders/queue/{op['id']}", json={"done": True}, headers=AUTH)
    path = notebook(bridge) / "carnet" / "Tâches" / "Tâches.md"
    page = path.read_text()
    line = next(l for l in page.splitlines() if kept in l)
    moved = page.replace(line + "\n", "") + f"\n## Archivées\n{line}\n"
    assert bridge.put("/v1/notebook/page", json={"path": "Tâches/Tâches.md", "text": moved}, headers=AUTH).status_code == 200
    assert bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"] == []
    # What an older bridge left: a link to a task removed, and the phone with duplicates and another Carnet's mark.
    bridge.stop()
    state_path = notebook(bridge) / "state.json"
    state = json.loads(state_path.read_text())
    state["reminders"]["t0000aaaa"] = {"op": "old", "part": "main", "complete": False}
    state["tasks"]["t0000aaaa"] = {"origin": "asked"}
    state_path.write_text(json.dumps(state))
    reminders_path = bridge.state_dir / "reminders.json"
    data = json.loads(reminders_path.read_text())
    mark = lambda t: f"BipAgents · Carnet ^{t}"
    data["items"] = [{"id": "r2", "notes": mark("t0000aaaa")}, {"id": "r3", "notes": mark(kept)},
                     {"id": "r4", "notes": mark(kept)}, {"id": "r5", "notes": mark("t9999ffff")}]
    reminders_path.write_text(json.dumps(data))
    bridge.start()
    deadline = time.time() + 10
    while time.time() < deadline and len(bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]) < 2:
        time.sleep(0.2)
    asked = bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]
    assert sorted((o["op"], o["args"]["id"]) for o in asked) == [("remove", "r2"), ("remove", "r4")]
    assert "t0000aaaa" not in json.loads(state_path.read_text())["reminders"]
    # Once: a restart or the phone's next share asks nothing more.
    bridge.put("/v1/reminders", json={"lists": [], "items": data["items"]}, headers=AUTH)
    assert len(bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]) == 2


def test_dates_to_come_are_facts_of_close_people(bridge):
    give_token(bridge)
    in_french(bridge)
    soon = datetime.now(timezone.utc).replace(hour=12)
    day = (soon.toordinal() + 10)
    date = datetime.fromordinal(day)
    months = ["janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre",
              "novembre", "décembre"]
    fact = f"Anniversaire : {date.day} {months[date.month - 1]}"
    call(bridge, "notebook_note", {"page": "Zoé", "kind": "person", "facts": [fact]})
    assert bridge.get("/v1/notebook/dates", headers=AUTH).json()["dates"] == []   # not marked close
    assert len(bridge.get("/v1/notebook/dates", params={"all": 1}, headers=AUTH).json()["dates"]) == 1
    call(bridge, "notebook_note", {"page": "Zoé", "facts": [], "close": True})
    dates = bridge.get("/v1/notebook/dates", headers=AUTH).json()["dates"]
    assert [(d["page"], d["in_days"], d["yearly"], d["fact"]) for d in dates] == [("Zoé", 10, True, fact)]
    assert bridge.get("/v1/notebook", headers=AUTH).json()["dates"][0]["page"] == "Zoé"
    # What the person's card shows in the « Personnes » mosaic.
    card = bridge.get("/v1/notebook/pages", params={"kind": "person"}, headers=AUTH).json()["pages"][0]
    assert (card["close"], card["next_date"]["in_days"], card["next_date"]["fact"], card["facts"]) == (True, 10, fact, 1)
    call(bridge, "notebook_note", {"page": "Jardin", "kind": "subject", "section": "status", "facts": ["Haie taillée"]})
    subject = bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"][0]
    assert (subject["status"], subject["next_date"]) == ("Haie taillée", None)
    assert bridge.get("/v1/notebook/tasks", headers=AUTH).json()["open"] == []   # a fact is not a task
    assert bridge.post("/v1/notebook/close", json={"path": "Personnes/Zoé.md", "close": False}, headers=AUTH).status_code == 200
    assert bridge.get("/v1/notebook/dates", headers=AUTH).json()["dates"] == []


def test_the_person_picks_a_subjects_icon_and_agents_keep_it(bridge):
    give_token(bridge)
    in_french(bridge)
    call(bridge, "notebook_note", {"page": "Voyage Japon", "kind": "subject", "section": "status", "facts": ["Vols à réserver"]})
    card = bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"][0]
    assert card["icon"] is None   # the apps pick one from its words
    chosen = bridge.post("/v1/notebook/icon", json={"path": "Sujets/Voyage Japon.md", "icon": "gifts"}, headers=AUTH)
    assert chosen.status_code == 200 and chosen.json()["path"] == "Sujets/Voyage Japon.md"
    assert "\nicon: gifts\n" in (notebook(bridge) / "carnet" / "Sujets" / "Voyage Japon.md").read_text()
    # an agent's next note keeps it; the page, the list and the search return it
    call(bridge, "notebook_note", {"page": "Voyage Japon", "section": "decisions", "facts": ["Avril, 12 jours"]})
    assert bridge.get("/v1/notebook/page", params={"path": "Sujets/Voyage Japon.md"}, headers=AUTH).json()["icon"] == "gifts"
    assert bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"][0]["icon"] == "gifts"
    assert bridge.get("/v1/notebook/search", params={"q": "japon"}, headers=AUTH).json()["pages"][0]["icon"] == "gifts"
    assert bridge.post("/v1/notebook/icon", json={"path": "Sujets/Voyage Japon.md", "icon": "Not An Id!"},
                       headers=AUTH).status_code == 400
    assert bridge.post("/v1/notebook/icon", json={"path": "Sujets/Nulle part.md", "icon": "travel"},
                       headers=AUTH).status_code == 404
    assert bridge.post("/v1/notebook/icon", json={"path": "Sujets/Voyage Japon.md", "icon": ""}, headers=AUTH).status_code == 200
    assert bridge.get("/v1/notebook/page", params={"path": "Sujets/Voyage Japon.md"}, headers=AUTH).json()["icon"] is None


def test_the_health_part_is_private_to_the_health_agent_and_the_person(bridge, fakes):
    health = bridge.state_dir / "health" / "agents.json"
    health.parent.mkdir(parents=True, exist_ok=True)
    health.write_text(json.dumps({"agents": ["wellness"], "categories": {"wellness": ["sleep"]}}))
    give_token(bridge)
    in_french(bridge)
    tools = {t["name"]: t for t in rpc(bridge, "tools/list").json()["result"]["tools"]}
    assert "private" in tools["notebook_note"]["inputSchema"]["properties"]
    filed = call(bridge, "notebook_note", {"page": "Santé", "kind": "area", "facts": ["Allergie à la pénicilline"]})
    assert "[private health part]" in text_of(filed)
    assert (notebook(bridge) / "private" / "Domaines" / "Santé.md").exists()
    assert not (notebook(bridge) / "carnet" / "Domaines").exists()
    home = bridge.get("/v1/notebook", headers=AUTH).json()
    assert all(s["count"] == 0 for s in home["sections"]) and home["recent"] == []
    assert bridge.get("/v1/notebook/search", params={"q": "pénicilline"}, headers=AUTH).json()["pages"] == []
    private = bridge.get("/v1/notebook/pages", params={"private": 1}, headers=AUTH).json()["pages"]
    assert [p["title"] for p in private] == ["Santé"]
    assert "pénicilline" in text_of(call(bridge, "notebook_search", {"words": "pénicilline"}))
    export = zipfile.ZipFile(io.BytesIO(bridge.get("/v1/notebook/export", headers=AUTH).content)).namelist()
    assert not any("Santé" in n for n in export)
    with_health = bridge.get("/v1/notebook/export", params={"health": 1}, headers=AUTH).content
    assert "Santé (privé)/Domaines/Santé.md" in zipfile.ZipFile(io.BytesIO(with_health)).namelist()
    # The health agent's conversations never go to the nightly pass's model.
    fakes.hermes.add_session("s1", [{"role": "user", "content": "Mon cardiologue Dr Martin me revoit le 12 décembre"},
                                    {"role": "assistant", "content": "Noté."}], last_active=time.time())
    run = bridge.post("/v1/notebook/tidy", headers=AUTH).json()["run"]
    assert run["turns_seen"] == 0 and run["model_called"] is False and fakes.model.chats == []


def nightly(bridges, fakes, **notebook_config):
    return bridges.new(change=lambda c: c.update(notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb",
                                                           "model": "small-eu", **notebook_config})).start()


def tidy(bridge):
    response = bridge.post("/v1/notebook/tidy", headers=AUTH)
    assert response.status_code == 200, response.text
    return response.json()["run"]


def test_the_nightly_pass_calls_no_model_when_there_is_nothing_to_file(bridges, fakes):
    bridge = nightly(bridges, fakes)
    run = tidy(bridge)
    assert run["reason"] == "no conversation" and run["model_called"] is False and fakes.model.chats == []
    fakes.hermes.add_session("s1", [{"role": "user", "content": "merci !"}, {"role": "assistant", "content": "Avec plaisir."},
                                    {"role": "user", "content": "quel temps fait-il ?"},
                                    {"role": "assistant", "content": "Il fait beau."}], last_active=time.time())
    run = tidy(bridge)
    assert (run["reason"], run["turns_seen"], run["turns_kept"], run["model_called"]) == ("nothing to file", 2, 0, False)
    assert fakes.model.chats == []
    runs = bridge.get("/v1/notebook/runs", headers=AUTH).json()
    assert runs["spend"]["eur"] == 0 and runs["spend"]["model_calls"] == 0 and len(runs["runs"]) == 2


def test_the_nightly_pass_files_what_the_agents_missed_and_counts_its_cost(bridges, fakes):
    bridge = nightly(bridges, fakes)
    in_french(bridge)
    filed = {"role": "assistant", "content": "Noté.",
             "tool_calls": [{"function": {"name": "mcp_bipagents_notebook_note", "arguments": "{}"}}]}
    fakes.hermes.add_session("s1", [
        {"role": "user", "content": "L'anniversaire de Marc est le 14 novembre"}, filed,
        {"role": "user", "content": "Mon frère Paul arrive le 3 novembre"}, {"role": "assistant", "content": "Super !"},
        {"role": "user", "content": "merci"}, {"role": "assistant", "content": "De rien."}], last_active=time.time())
    fakes.model.notebook_lines = [{"op": "note", "agent": "wellness", "kind": "person", "page": "Paul",
                                   "section": "Famille", "facts": ["Frère, arrive le 3 novembre"]},
                                  {"op": "task", "agent": "wellness", "origin": "asked", "title": "x", "due": "bad"}]
    run = tidy(bridge)
    assert (run["reason"], run["turns_seen"], run["turns_kept"], run["model_called"]) == ("filed", 3, 1, True)
    assert (run["tokens_in"], run["tokens_out"], run["eur"], run["applied"], run["model"]) == (3000, 300, 0.00195, 1, "small-eu")  # 0.50 € / 1.50 € a million (defaults)
    chat = fakes.model.chats[-1]
    assert chat["authorization"] == "Bearer sk-nb" and chat["body"]["model"] == "small-eu"
    sent = chat["body"]["messages"][1]["content"]
    assert "Mon frère Paul arrive le 3 novembre" in sent and "Marc" not in sent and "merci" not in sent
    page = (notebook(bridge) / "carnet" / "Personnes" / "Paul.md").read_text()
    assert f"## Famille et proches\n- Frère, arrive le 3 novembre — *Wellness, {TODAY}*" in page
    change = bridge.get("/v1/notebook/history", headers=AUTH).json()["changes"][0]
    assert change["by"] == "nightly" and change["agent"] == "wellness" and change["nightly"] is True
    # The same conversation is not read again.
    again = tidy(bridge)
    assert again["model_called"] is False and len(fakes.model.chats) == 1
    spend = bridge.get("/v1/notebook/runs", headers=AUTH).json()["spend"]
    assert (spend["eur"], spend["tokens_in"], spend["model_calls"]) == (0.00195, 3000, 1)


def test_the_monthly_cap_stops_the_nightly_pass(bridges, fakes):
    bridge = nightly(bridges, fakes, monthly_cap=0.00001)
    fakes.hermes.add_session("s1", [{"role": "user", "content": "Ma sœur Léa se marie le 12 juin"},
                                    {"role": "assistant", "content": "Félicitations !"}], last_active=time.time())
    run = tidy(bridge)
    assert run["reason"] == "monthly cap reached" and run["model_called"] is False and fakes.model.chats == []
    settings = bridge.get("/v1/notebook/settings", headers=AUTH).json()
    assert settings["monthly_cap"] == 0.00001 and settings["nightly"] == "nightly" and settings["mentions"] is True
    assert bridge.patch("/v1/notebook/settings", json={"nightly": "weekly"}, headers=AUTH).json()["nightly"] == "weekly"
    assert bridge.patch("/v1/notebook/settings", json={"nightly": "hourly"}, headers=AUTH).status_code == 422


def test_the_agents_memories_are_copied_once_for_the_nightly_pass_to_sort(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb"}))
    home = managed_install(bridge, fakes)
    memories = profile(home, "coach") / "memories"
    memories.mkdir(parents=True)
    (memories / "MEMORY.md").write_text("Alex court le dimanche\n§\nSa fille Zoé a 8 ans")
    (memories / "USER.md").write_text("Alex, vit à Lyon")
    bridge.start()
    bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH)
    fakes.model.notebook_lines = [{"op": "note", "agent": "coach", "kind": "person", "page": "Zoé",
                                   "section": "Infos", "facts": ["Fille d'Alex, 8 ans"]}]
    run = tidy(bridge)
    assert run["seeded"] == 3 and run["inbox"] == 3 and run["model_called"] is True
    sent = fakes.model.chats[-1]["body"]["messages"][1]["content"]
    assert "To sort" in sent and "Sa fille Zoé a 8 ans" in sent
    inbox = (notebook(bridge) / "carnet" / "À ranger.md").read_text()
    assert "Zoé" not in inbox   # sorted
    assert (notebook(bridge) / "carnet" / "Personnes" / "Zoé.md").exists()
    assert "seeded" not in tidy(bridge)   # once


def test_the_agents_web_pages_live_in_the_carnet(bridge):
    give_token(bridge)
    in_french(bridge)
    call(bridge, "notebook_note", {"page": "Voyage Japon", "kind": "subject", "section": "status", "facts": ["Billets pris"]})
    made = call(bridge, "create_page", {"html": "<p>Budget</p>", "title": "Budget du voyage", "notebook": "Voyage Japon"})
    assert "Linked to the Carnet page Voyage Japon (Sujets/Voyage Japon.md)." in text_of(made)
    lost = call(bridge, "create_page", {"html": "<p>x</p>", "title": "Autre", "notebook": "Nulle part"})
    assert "No Carnet page « Nulle part »: the page is not linked" in text_of(lost)
    docs = bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"]
    budget = next(d for d in docs if d["page"] == "budget-du-voyage")
    assert (budget["type"], budget["name"], budget["origin"]) == ("page", "Budget du voyage", "made")
    assert budget["linked"] == {"title": "Voyage Japon", "path": "Sujets/Voyage Japon.md", "kind": "subject"}
    assert budget["subjects"][0]["title"] == "Voyage Japon"
    assert budget["path"] == "/bipagents/pages/wellness/budget-du-voyage.html"
    view = bridge.get("/v1/notebook/page", params={"path": "Sujets/Voyage Japon.md"}, headers=AUTH).json()
    assert [d["page"] for d in view["documents"]] == ["budget-du-voyage"]
    # An update keeps the link; the Carnet page deleted leaves the web page in Documents, unlinked.
    call(bridge, "create_page", {"html": "<p>v2</p>", "title": "Budget du voyage", "page": "budget-du-voyage"})
    pages = bridge.get("/v1/pages", params={"agent": "wellness"}, headers=AUTH).json()["pages"]
    # by name: both pages may carry the same second, and a tie keeps the folder's order (not the same on Linux)
    assert next(p for p in pages if p["page"] == "budget-du-voyage")["notebook"]["part"] == "main"
    bridge.delete("/v1/notebook/page", params={"path": "Sujets/Voyage Japon.md"}, headers=AUTH)
    docs = bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"]
    assert next(d for d in docs if d["page"] == "budget-du-voyage")["linked"] is None
    # The web page deleted: gone from the Carnet.
    bridge.delete("/v1/pages/budget-du-voyage", params={"agent": "wellness"}, headers=AUTH)
    assert [d["page"] for d in bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"]] == ["autre"]


def test_the_health_agents_web_pages_stay_in_the_private_part(bridge):
    health = bridge.state_dir / "health" / "agents.json"
    health.parent.mkdir(parents=True, exist_ok=True)
    health.write_text(json.dumps({"agents": ["wellness"], "categories": {"wellness": ["sleep"]}}))
    give_token(bridge)
    in_french(bridge)
    call(bridge, "notebook_note", {"page": "Santé", "kind": "area", "facts": ["Prise de sang tous les 6 mois"]})
    made = call(bridge, "create_page", {"html": "<p>Ferritine</p>", "title": "Prises de sang", "notebook": "Domaines/Santé"})
    assert "Linked to the Carnet page Santé (Domaines/Santé.md)." in text_of(made)
    assert bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"] == []
    assert bridge.get("/v1/notebook", headers=AUTH).json()["sections"][6] == {"kind": "documents", "title": "Documents", "count": 0}
    private = bridge.get("/v1/notebook/documents", params={"private": 1}, headers=AUTH).json()["documents"]
    assert [(d["page"], d["linked"]["path"]) for d in private] == [("prises-de-sang", "Domaines/Santé.md")]
    view = bridge.get("/v1/notebook/page", params={"path": "Domaines/Santé.md", "private": 1}, headers=AUTH).json()
    assert [d["page"] for d in view["documents"]] == ["prises-de-sang"]
    # Not linked, still the health agent's: private all the same.
    call(bridge, "create_page", {"html": "<p>Sommeil</p>", "title": "Sommeil"})
    assert bridge.get("/v1/notebook/documents", headers=AUTH).json()["documents"] == []
    assert len(bridge.get("/v1/notebook/documents", params={"private": 1}, headers=AUTH).json()["documents"]) == 2


def test_old_pages_are_refiled_and_the_rest_sorted_by_the_nightly_pass(bridges, fakes):
    bridge = nightly(bridges, fakes)
    in_french(bridge)
    folder = notebook(bridge) / "carnet" / "Personnes"
    folder.mkdir(parents=True)
    (folder / "Léa.md").write_text("---\nid: p0000000001\ntype: person\naliases: []\n---\n# Léa\n\n## Infos\n"
                                   "- Anniversaire : 12 mars — *Wellness, 2026-10-01*\n- Grande voyageuse — *Wellness, 2026-10-01*\n")
    # Shown in the outline at once; on disk at the nightly pass, without a model for what is obvious.
    view = bridge.get("/v1/notebook/page", params={"path": "Personnes/Léa.md"}, headers=AUTH).json()
    assert [(s["id"], s["title"]) for s in view["sections"]] == [("dates", "Dates"), ("other", "Autres")]
    fakes.model.notebook_lines = [{"op": "refile", "page": "Léa", "fact": "Grande voyageuse", "section": "tastes"}]
    run = tidy(bridge)
    assert run["refiled"] == 1 and run["unsorted"] == 1 and run["model_called"] is True and run["applied"] == 1
    sent = fakes.model.chats[-1]["body"]["messages"][1]["content"]
    assert "Filed under « other »" in sent and "- [Léa] (person) Grande voyageuse" in sent
    page = (folder / "Léa.md").read_text()
    assert page.endswith("# Léa\n\n## Dates\n- Anniversaire : 12 mars — *Wellness, 2026-10-01*\n\n"
                         "## Goûts et préférences\n- Grande voyageuse — *Wellness, 2026-10-01*\n")
    changes = bridge.get("/v1/notebook/history", params={"path": "Personnes/Léa.md"}, headers=AUTH).json()["changes"]
    assert [(c["action"], c["name"]) for c in changes] == [("refile", "rangement"), ("refile", "rangement")]
    # The tidy has no agent: the apps still get strings (the iPhone failed to read a null agent, 2026-10-22).
    recent = bridge.get("/v1/notebook", headers=AUTH).json()["recent"]
    assert recent and all(isinstance(c[k], str) for c in recent for k in ("id", "by", "name", "agent", "title", "summary"))
    # Once: not again, and a line the model already saw is not sent again.
    again = tidy(bridge)
    assert "refiled" not in again and again["unsorted"] == 0 and again["model_called"] is False
