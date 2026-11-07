"""An agent's memory seen and edited by its person (`/v1/agents/{id}/memory…`), notes imported into its knowledge
pages (`/v1/agents/{id}/memory/import`), the knowledge and reminder tools it uses over MCP, and the iPhone's
reminders routes (tests/test_memory_edit.py, test_notes_import.py, test_reminders.py, over HTTP)."""
import base64
import io
import json
import os
import time
import zipfile

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from conftest import managed_install, profile, wait_registered


def coach(bridges, fakes):
    """An agent this install manages (served by the hub under /p/), its profile in the test's HERMES_HOME."""
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    return bridge, profile(home, "coach")


def test_the_person_reads_and_edits_what_the_agent_remembers(bridges, fakes):
    bridge, folder = coach(bridges, fakes)
    (folder / "memories").mkdir(parents=True)
    (folder / "memories" / "MEMORY.md").write_text("Alex court le dimanche.\n§\nLe chat s'appelle Pixel.")
    (folder / "config.yaml").write_text("memory:\n  user_char_limit: 40\n")
    bridge.start()
    seen = bridge.get("/v1/agents/coach/memory", headers=AUTH).json()
    assert seen["notes"] == {"entries": ["Alex court le dimanche.", "Le chat s'appelle Pixel."], "limit": 2200,
                             "used": len("Alex court le dimanche.\n§\nLe chat s'appelle Pixel.")}
    assert seen["user"] == {"entries": [], "limit": 40, "used": 0} and seen["pages"] == []
    edited = bridge.put("/v1/agents/coach/memory/notes", json={"entries": ["Le chat s'appelle Pixel.", " ", "Né en 1985."]},
                        headers=AUTH).json()
    assert edited["entries"] == ["Le chat s'appelle Pixel.", "Né en 1985."]
    assert (folder / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n§\nNé en 1985."
    assert (folder / "memories" / "MEMORY.md.lock").exists()                  # Hermes' own lock, taken
    too_long = bridge.put("/v1/agents/coach/memory/user", json={"entries": ["x" * 41]}, headers=AUTH)
    assert too_long.status_code == 413 and too_long.json() == {"detail": "41 characters for 40 at most"}
    sneaky = bridge.put("/v1/agents/coach/memory/user", json={"entries": ["a\n§\nb"]}, headers=AUTH).json()
    assert sneaky["entries"] == ["ab"]                                        # one entry stays one entry
    assert (folder / "memories" / "USER.md").read_text() == "ab"
    which = bridge.put("/v1/agents/coach/memory/soul", json={"entries": []}, headers=AUTH)
    assert which.status_code == 404 and which.json() == {"detail": "notes or user"}
    nobody = bridge.get("/v1/agents/nobody/memory", headers=AUTH)
    assert nobody.status_code == 404 and nobody.json() == {"detail": "not an agent this install manages"}
    assert bridge.get("/v1/agents/wellness/memory", headers=AUTH).status_code == 404   # set up by hand
    assert bridge.get("/v1/agents/coach/memory").status_code == 401


NOTES_FR = ["Elle préfère les rappels le matin, avant 8h30, et jamais le dimanche.",
            "Le loyer de 1 240 € est payé le 5 de chaque mois pour l'appartement de la rue des Lilas."]
NOTES_EN = ["She prefers reminders in the morning, before 8:30, and never on Sundays.",
            "The rent of €1,240 is paid on the 5th of each month for the flat on rue des Lilas."]


def test_a_memory_in_the_old_language_is_translated_once_and_can_be_put_back(bridges, fakes):
    bridge, folder = coach(bridges, fakes)
    (folder / "memories").mkdir(parents=True)
    (folder / "memories" / "MEMORY.md").write_text("\n§\n".join(NOTES_FR))
    (folder / "config.yaml").write_text(f"model:\n  provider: custom\n  default: small\n  base_url: {fakes.model.url}/v1\n"
                                        "  key_env: CUSTOM_MODEL_API_KEY\n")
    (folder / ".env").write_text("CUSTOM_MODEL_API_KEY=sk-test-key\n")
    fakes.model.translation_answer = {"notes": NOTES_EN, "user": []}
    bridge.start()
    wait_registered(bridge, ["coach"])
    assert bridge.put("/v1/agents/coach/language", headers=AUTH, json={"lang": "fr"}).status_code == 200
    time.sleep(0.5)
    assert fakes.model.chats == []                                            # its first language: nothing to do
    assert bridge.put("/v1/agents/coach/language", headers=AUTH, json={"lang": "en"}).status_code == 200
    for _ in range(50):
        if (folder / "memories" / "MEMORY.md.before-translation").exists():
            break
        time.sleep(0.1)
    assert bridge.get("/v1/agents/coach/memory", headers=AUTH).json()["notes"]["entries"] == NOTES_EN
    assert (folder / "memories" / "MEMORY.md.before-translation").read_text() == "\n§\n".join(NOTES_FR)
    [chat] = fakes.model.chats
    assert chat["authorization"] == "Bearer sk-test-key" and "into English" in chat["body"]["messages"][0]["content"]
    assert "memory translated" in bridge.log_path.read_text()


def test_knowledge_pages_are_listed_read_edited_and_removed_inside_their_folder(bridges, fakes):
    bridge, folder = coach(bridges, fakes)
    (folder / "knowledge" / "mail" / "people").mkdir(parents=True)
    (folder / "knowledge" / "mail" / "index.md").write_text("# Index\n- [Sam](people/sam.md)\n")
    (folder / "knowledge" / "mail" / "people" / "sam.md").write_text("# Sam\nVoisin.\n")
    (folder / "knowledge" / "mail" / "run.sh").write_text("not a page")
    (folder / "secret.md").write_text("outside")
    bridge.start()
    listed = bridge.get("/v1/agents/coach/memory", headers=AUTH).json()["pages"]
    assert [p["path"] for p in listed] == ["mail/index.md", "mail/people/sam.md"]
    assert listed[1]["size"] == len("# Sam\nVoisin.\n") and isinstance(listed[1]["updated"], float)
    page = bridge.get("/v1/agents/coach/memory/pages/mail/people/sam.md", headers=AUTH).json()
    assert page == {"path": "mail/people/sam.md", "text": "# Sam\nVoisin.\n"}
    put = bridge.put("/v1/agents/coach/memory/pages/mail/people/sam.md", json={"text": "# Sam\nVoisin, jardinier.\n"},
                     headers=AUTH)
    assert put.json() == {"path": "mail/people/sam.md", "size": len("# Sam\nVoisin, jardinier.\n".encode())}
    assert (folder / "knowledge" / "mail" / "people" / "sam.md").read_text() == "# Sam\nVoisin, jardinier.\n"
    created = bridge.put("/v1/agents/coach/memory/pages/notes/new.txt", json={"text": "nouveau"}, headers=AUTH)
    assert created.status_code == 200 and (folder / "knowledge" / "notes" / "new.txt").read_text() == "nouveau"
    escape = bridge.get("/v1/agents/coach/memory/pages/..%2F..%2Fsecret.md", headers=AUTH)
    assert escape.status_code == 404 and escape.json() == {"detail": "not a knowledge page"}
    assert bridge.put("/v1/agents/coach/memory/pages/mail/run.sh", json={"text": "x"}, headers=AUTH).status_code == 404
    missing = bridge.get("/v1/agents/coach/memory/pages/mail/nobody.md", headers=AUTH)
    assert missing.status_code == 404 and missing.json() == {"detail": "no such page"}
    assert bridge.delete("/v1/agents/coach/memory/pages/mail/people/sam.md", headers=AUTH).json() == {"ok": True}
    assert not (folder / "knowledge" / "mail" / "people" / "sam.md").exists()
    assert bridge.delete("/v1/agents/coach/memory/pages/mail/people/sam.md", headers=AUTH).status_code == 404


def test_the_agent_reads_and_searches_its_knowledge_pages(bridges, fakes):
    bridge, folder = coach(bridges, fakes)
    (folder / "knowledge" / "mail").mkdir(parents=True)
    (folder / "knowledge" / "mail" / "index.md").write_text("# Index\nSam est le voisin.\nLa banque appelle lundi.\n")
    bridge.start()
    wait_registered(bridge, ["coach"])
    give_token(bridge, "coach", "coach-token-" + "c" * 30)
    token = "coach-token-" + "c" * 30
    assert text_of(call(bridge, "knowledge_read", {}, token=token)) == "mail/index.md"
    assert text_of(call(bridge, "knowledge_read", {"path": "/mail/index.md"}, token=token)).startswith("# Index")
    missing = call(bridge, "knowledge_read", {"path": "../secret.md"}, token=token)
    assert missing["isError"] and text_of(missing) == "no page ../secret.md (knowledge_read without a path lists them)"
    assert text_of(call(bridge, "knowledge_search", {"words": "SAM voisin"}, token=token)) == "mail/index.md: Sam est le voisin."
    assert text_of(call(bridge, "knowledge_search", {"words": "zèbre"}, token=token)) == "Nothing found."
    assert call(bridge, "knowledge_search", {"words": "a"}, token=token)["isError"] is True


def takeout():
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as archive:
        archive.writestr("Takeout/Keep/Courses.json", json.dumps({
            "title": "Courses", "listContent": [{"text": "Lait", "isChecked": True}, {"text": "Pain", "isChecked": False}],
            "labels": [{"name": "Maison"}], "userEditedTimestampUsec": 1760000000000000, "isTrashed": False}))
        archive.writestr("Takeout/Keep/Courses.html", "<html><body>Courses</body></html>")   # the JSON wins
        archive.writestr("Takeout/Keep/Idee.json", json.dumps({"title": "", "textContent": "Offrir un vélo à Léa",
                                                               "isTrashed": False}))
        archive.writestr("Takeout/Keep/Vieux.json", json.dumps({"title": "Vieux", "textContent": "x", "isTrashed": True}))
        archive.writestr("Takeout/Keep/photo.jpg", b"\xff\xd8")
    return base64.b64encode(out.getvalue()).decode()


def test_keep_s_takeout_and_text_notes_become_knowledge_pages(bridges, fakes):
    bridge, folder = coach(bridges, fakes)
    bridge.start()
    notes = folder / "knowledge" / "notes"
    done = bridge.post("/v1/agents/coach/memory/import", json={"name": "takeout-20261020.zip", "data": takeout()},
                       headers=AUTH)
    assert done.status_code == 200 and done.json() == {"notes": 2, "skipped": 2}   # the trashed note and the photo
    courses = (notes / "keep" / "courses.md").read_text()
    assert courses.startswith("# Courses\n") and "- [x] Lait" in courses and "- [ ] Pain" in courses
    assert "Étiquettes : Maison" in courses and "2025-10-09" in courses
    assert "Offrir un vélo à Léa" in (notes / "keep" / "offrir-un-velo-a-lea.md").read_text()
    text = base64.b64encode("Penser à rappeler le plombier.".encode()).decode()
    bridge.post("/v1/agents/coach/memory/import", json={"name": "Plombier.txt", "data": text, "source": "apple-notes"},
                headers=AUTH)
    index = (notes / "index.md").read_text()
    assert "## apple-notes (1)" in index and "## keep (2)" in index and "[Courses](keep/courses.md)" in index
    pages = [p["path"] for p in bridge.get("/v1/agents/coach/memory", headers=AUTH).json()["pages"]]
    assert "notes/apple-notes/plombier.md" in pages and "notes/index.md" in pages
    empty = base64.b64encode(b"").decode()
    nothing = bridge.post("/v1/agents/coach/memory/import", json={"name": "vide.txt", "data": empty}, headers=AUTH)
    assert nothing.status_code == 422 and nothing.json() == {"detail": "no note found in this file"}
    not_zip = bridge.post("/v1/agents/coach/memory/import", json={"name": "x.zip", "data": text}, headers=AUTH)
    assert not_zip.status_code == 400 and not_zip.json() == {"detail": "not a zip archive"}
    not_b64 = bridge.post("/v1/agents/coach/memory/import", json={"name": "x.txt", "data": "%%%"}, headers=AUTH)
    assert not_b64.status_code == 400 and not_b64.json() == {"detail": "not base64"}
    assert bridge.post("/v1/agents/nobody/memory/import", json={"name": "x.txt", "data": text},
                       headers=AUTH).status_code == 404


def test_the_iphone_shares_its_reminders_and_does_what_the_agents_ask(bridges, fakes):
    bridge, _ = coach(bridges, fakes)
    bridge.start()
    wait_registered(bridge, ["coach"])
    token = "coach-token-" + "c" * 30
    give_token(bridge, "coach", token)
    not_yet = call(bridge, "reminders_list", {}, token=token)
    assert not_yet["isError"] and text_of(not_yet).startswith("The person hasn't shared their reminders with you")
    assert bridge.put("/v1/reminders/agents", json={"agents": ["Coach"]}, headers=AUTH).json() == {"agents": ["coach"]}
    unknown = bridge.put("/v1/reminders/agents", json={"agents": ["ghost"]}, headers=AUTH)
    assert unknown.status_code == 400 and unknown.json() == {"detail": "unknown agent"}
    snapshot = {"lists": [{"id": "L1", "title": "Courses"}, {"id": "L2", "title": "Maison"}],
                "items": [{"id": "r1", "list": "L1", "title": "Lait", "due": "2026-10-21"},
                          {"id": "r2", "list": "L2", "title": "Appeler le plombier", "notes": "Fuite sous l'évier"},
                          {"id": "r3", "list": "L1", "title": "Pain", "completed": True}]}
    assert bridge.put("/v1/reminders", json=snapshot, headers=AUTH).json() == {"items": 3}
    listed = text_of(call(bridge, "reminders_list", {}, token=token))
    assert "id r1 · Courses · Lait · due 2026-10-21" in listed and "Fuite sous l'évier" in listed and "Pain" not in listed
    courses = text_of(call(bridge, "reminders_list", {"list": "courses", "include_done": True}, token=token))
    assert "Pain" in courses and "plombier" not in courses and "· done" in courses
    asked = call(bridge, "reminders_add", {"title": "Rendez-vous banque", "due": "2026-10-30T10:00", "list": "Maison"},
                 token=token)
    assert not asked["isError"] and "next sync" in text_of(asked)
    assert call(bridge, "reminders_complete", {"id": "zz"}, token=token)["isError"]
    assert call(bridge, "reminders_add", {"title": " "}, token=token)["isError"]
    call(bridge, "reminders_complete", {"id": "r1"}, token=token)
    queue = bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]
    assert [(op["op"], op["args"].get("title") or op["args"].get("id")) for op in queue] == [
        ("add", "Rendez-vous banque"), ("complete", "r1")]
    assert set(queue[0]) == {"id", "op", "args", "agent", "created"} and queue[0]["agent"] == "coach"
    # the time as the phone reads it (7/11: the iPhone took « 2026-10-30T10:00 » for the day alone)
    assert queue[0]["args"] == {"title": "Rendez-vous banque", "due": "2026-10-30T10:00:00", "list": "Maison", "notes": ""}
    assert bridge.post(f"/v1/reminders/queue/{queue[0]['id']}", json={"done": True}, headers=AUTH).json() == {"ok": True}
    assert bridge.post("/v1/reminders/queue/nope", json={"done": True}, headers=AUTH).status_code == 404
    assert [op["op"] for op in bridge.get("/v1/reminders/queue", headers=AUTH).json()["queue"]] == ["complete"]
    seen = bridge.get("/v1/reminders", headers=AUTH).json()
    assert seen["agents"] == ["coach"] and seen["items"] == 3 and seen["pending"] == 1 and seen["lists"] == 2
    assert isinstance(seen["at"], float)
    stored = bridge.state_dir / "reminders.json"
    assert oct(os.stat(stored).st_mode & 0o777) == "0o600"
    # A reminder by place: the person's phone does it, whichever it is (7/11: « the person's iPhone » for an Android one).
    by_place = text_of(call(bridge, "reminders_add", {"title": "Acheter du pain", "place": "Boulangerie Dupont"}, token=token))
    assert by_place.startswith("Asked: the person's phone reminds them on arriving at « Boulangerie Dupont »")
    assert "iPhone" not in by_place
    assert "the iPhone's Reminders (Rappels) for 2026-10-30 10:00, with an alert at that time" in text_of(asked)
    give_token(bridge)
    assert call(bridge, "reminders_add", {"title": "x"})["isError"]   # another agent (wellness): not shared

    # Shared with no agent any more: the copy goes (the requests stay their 14 days).
    assert bridge.put("/v1/reminders/agents", json={"agents": []}, headers=AUTH).json() == {"agents": []}
    seen = bridge.get("/v1/reminders", headers=AUTH).json()
    assert seen["items"] == 0 and seen["lists"] == 0 and seen["at"] is None and seen["pending"] == 2
    assert "Lait" not in stored.read_text() and "plombier" not in stored.read_text()


def test_a_reminder_at_two_goes_to_the_phone_that_does_it_and_the_agent_says_where(bridges, fakes):
    """7/11, Alex: on the iPhone « rappelle-moi à 14 h » came without its time; on Android the agent said « c'est
    noté » for a reminder no phone would do, then gave iPhone steps. Now: an iPhone's Reminders when one shares them,
    else the Android phone's calendar (« Dans l'agenda »), never both; with neither, plainly not set."""
    bridge, _ = coach(bridges, fakes)
    bridge.start()
    wait_registered(bridge, ["coach"])
    token = "coach-token-" + "c" * 30
    give_token(bridge, "coach", token)
    # The tool an agent sees, from either phone: the time goes in due, nothing sends the person to a phone's settings.
    tools = {t["name"]: t for t in rpc(bridge, "tools/list", token=token).json()["result"]["tools"]}
    add = tools["reminders_add"]
    assert add["inputSchema"]["additionalProperties"] is False
    assert "2026-11-07T14:00" in add["inputSchema"]["properties"]["due"]["description"]
    assert all("Settings" not in json.dumps(tools[n]) for n in ("reminders_list", "reminders_add", "reminders_complete"))
    not_shared = text_of(call(bridge, "reminders_add", {"title": "Appeler Paul", "due": "2026-11-07T14:00"}, token=token))
    assert "tool on your page" in not_shared and "Settings" not in not_shared and "iPhone" not in not_shared
    assert bridge.put("/v1/reminders/agents", json={"agents": ["coach"]}, headers=AUTH).status_code == 200
    queue = lambda phone=None: bridge.get("/v1/reminders/queue" + (f"?phone={phone}" if phone else ""),
                                          headers=AUTH).json()["queue"]

    # No phone does reminders at a time yet (an Android app from before 7/11): not asked, said plainly.
    none = call(bridge, "reminders_add", {"title": "Appeler Paul", "due": "2026-11-07T14:00"}, token=token)
    assert none["isError"] and text_of(none).startswith("Not added") and "isn't set" in text_of(none)
    assert "Settings" not in text_of(none) and queue() == [] and queue("android") == []
    # The time somewhere the bridge doesn't read: refused, not lost.
    lost = call(bridge, "reminders_add", {"title": "Appeler Paul", "due": "2026-11-07", "time": "14:00"}, token=token)
    assert lost["isError"] and "unknown argument time" in text_of(lost)

    # The Android app says it may write to its calendar: there, at 14:00, with an alert, for it alone.
    assert bridge.put("/v1/reminders/android", json={"calendar": True, "items": []}, headers=AUTH).json() == {"items": 0}
    android = call(bridge, "reminders_add", {"title": "Appeler Paul", "due": "2026-11-07T14:00"}, token=token)
    said = text_of(android)
    assert not android["isError"] and "in their calendar at 2026-11-07 14:00, with an alert" in said
    assert "iPhone" not in said
    asked = queue("android")
    assert [(o["op"], o["args"]["due"], o["phone"], o["agent"]) for o in asked] == [
        ("add", "2026-11-07T14:00:00", "android", "coach")]
    assert queue() == []   # an iPhone never sees it
    assert bridge.post(f"/v1/reminders/queue/{asked[0]['id']}", json={"done": True}, headers=AUTH).json() == {"ok": True}
    # Its copy: the reminders it put in the calendar, listed for the agent, ticked off there.
    copy = {"calendar": True, "items": [{"id": asked[0]["id"], "title": "Appeler Paul", "due": "2026-11-07T14:00:00+01:00"}]}
    assert bridge.put("/v1/reminders/android", json=copy, headers=AUTH).json() == {"items": 1}
    listed = text_of(call(bridge, "reminders_list", {}, token=token))
    assert "Android phone's calendar" in listed and f"id {asked[0]['id']} · Appeler Paul · due 2026-11-07T14:00" in listed
    again = call(bridge, "reminders_add", {"title": "appeler Paul", "due": "2026-11-07T14:00"}, token=token)
    assert text_of(again).startswith("Already in the reminders")
    call(bridge, "reminders_complete", {"id": asked[0]["id"]}, token=token)
    assert [(o["op"], o["args"], o["phone"]) for o in queue("android")] == [("complete", {"id": asked[0]["id"]}, "android")]
    # A reminder by place: either phone (the Android app reads it with its own).
    call(bridge, "reminders_add", {"title": "Lait", "place": "Monoprix République"}, token=token)
    assert [o["args"].get("title") for o in queue("android")] == [None, "Lait"]
    assert [o["args"]["title"] for o in queue()] == ["Lait"]

    # The calendar not allowed on the Android phone: it waits, and the agent says so (no iPhone steps).
    bridge.put("/v1/reminders/android", json={"calendar": False, "items": []}, headers=AUTH)
    waits = text_of(call(bridge, "reminders_add", {"title": "Vaccin", "due": "2026-11-20"}, token=token))
    assert waits.startswith("Asked, but not set yet") and "iPhone" not in waits

    # An iPhone shares its Reminders too: the next reminder goes there, never also to the Android calendar.
    assert bridge.put("/v1/reminders", json={"lists": [], "items": []}, headers=AUTH).json() == {"items": 0}
    iphone = text_of(call(bridge, "reminders_add", {"title": "Banque", "due": "2026-11-08T10:00"}, token=token))
    assert "the iPhone's Reminders (Rappels) for 2026-11-08 10:00, with an alert at that time" in iphone
    assert "Banque" in [o["args"]["title"] for o in queue()]
    assert "Banque" not in [o["args"]["title"] for o in queue("android")]
