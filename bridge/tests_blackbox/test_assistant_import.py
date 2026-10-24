"""« Venir d'un autre assistant » (bridge-rs/src/assistant_import.rs, 2026-10-24): the message to paste into Grok Bot,
the zip it writes read into a plan (nothing saved), then the parts the person kept saved into a new agent or an
existing one. Synthetic exports only."""
import base64
import io
import zipfile

import yaml

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install, profile

EXPORT = {
    "bipagents-export/SOUL.md": "Tu es Max, l'assistant de la maison. Tu réponds en deux phrases.",
    "bipagents-export/USER.md": "- Alex vit à Lyon.\n- Il préfère les messages courts.",
    "bipagents-export/MEMORY.md": "- 2026-09-01 : chaudière révisée.\n- Mot de passe du routeur : Zx81!pass",
    "bipagents-export/people.md": "## Léa\n- sa fille, 9 ans\n",
    "bipagents-export/tasks.md": "- Renouveler l'assurance (avant le 30/11)\n",
    "bipagents-export/routines.md": "## Point du matin\nChaque jour à 7 h 30 (Europe/Paris), résume l'agenda.\n",
    "bipagents-export/apps.md": "- Gmail\n- Notion\n",
    "bipagents-export/conversations.md": "## Vacances (été 2026)\nOn a choisi la Bretagne.\n",
    "bipagents-export/skills/weekly-report/SKILL.md":
        "---\nname: weekly-report\ndescription: Le bilan du vendredi\n---\n1. Lire la semaine.\n2. Envoyer le bilan.\n",
    "bipagents-export/photo.jpg": "not text",
}


def zipped(files):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, text in files.items():
            archive.writestr(name, text)
    return base64.b64encode(buffer.getvalue()).decode()


def test_the_message_to_paste_comes_in_the_apps_language(bridge):
    bridge.start()
    french = bridge.get("/v1/assistant-import/prompt", params={"lang": "fr-FR"}, headers=AUTH).json()
    assert french["lang"] == "fr" and french["file"] == "bipagents-export.zip"
    assert french["text"].startswith("Je change d'assistant") and "skills/<nom>/SKILL.md" in french["text"]
    assert len(french["text"]) <= 2500
    assert bridge.get("/v1/assistant-import/prompt", params={"lang": "it"}, headers=AUTH).json()["lang"] == "en"
    assert bridge.get("/v1/assistant-import/prompt").status_code == 401


def test_an_export_is_read_without_saving_anything(bridge):
    bridge.start()
    plan = bridge.post("/v1/assistant-import/parse", headers=AUTH,
                       json={"files": [{"name": "bipagents-export.zip", "data": zipped(EXPORT)}]})
    assert plan.status_code == 200, plan.text
    plan = plan.json()
    assert plan["name"] == "Max" and plan["soul"].startswith("Tu es Max")
    assert plan["user"] == ["Alex vit à Lyon.", "Il préfère les messages courts."]
    assert plan["memory"] == ["2026-09-01 : chaudière révisée.", "Mot de passe du routeur : [•••]"]
    assert plan["removed"] == 1 and plan["skipped"] == 1
    assert plan["people"] == ["Léa — sa fille, 9 ans"] and plan["apps"] == ["Gmail", "Notion"]
    assert plan["routines"] == [{"name": "Point du matin",
                                 "text": "Chaque jour à 7 h 30 (Europe/Paris), résume l'agenda."}]
    assert plan["skills"] == [{"name": "weekly-report", "description": "Le bilan du vendredi",
                               "text": "1. Lire la semaine.\n2. Envoyer le bilan."}]
    assert plan["conversations"] == [{"title": "Vacances (été 2026)", "text": "On a choisi la Bretagne."}]
    # loose Markdown files work too
    loose = bridge.post("/v1/assistant-import/parse", headers=AUTH, json={"files": [
        {"name": "USER.md", "data": base64.b64encode("- Alex court le dimanche.".encode()).decode()}]}).json()
    assert loose["user"] == ["Alex court le dimanche."] and loose["soul"] == ""
    empty = bridge.post("/v1/assistant-import/parse", headers=AUTH, json={"files": [
        {"name": "x.zip", "data": zipped({"a/photo.jpg": "x"})}]})
    assert empty.status_code == 422 and empty.json() == {"detail": "nothing to import in these files"}
    bad = bridge.post("/v1/assistant-import/parse", headers=AUTH,
                      json={"files": [{"name": "x.zip", "data": base64.b64encode(b"nope").decode()}]})
    assert bad.status_code == 400 and bad.json() == {"detail": "not a zip archive"}


def test_what_the_person_kept_fills_an_existing_agent(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(home, "coach")
    (folder / "SOUL.md").write_text("Tu es Coach.\n")
    (folder / "config.yaml").write_text("model:\n  provider: custom\n")
    bridge.start()
    plan = bridge.post("/v1/assistant-import/parse", headers=AUTH,
                       json={"files": [{"name": "bipagents-export.zip", "data": zipped(EXPORT)}]}).json()
    plan["others"] = []
    # what the app sends back: the person's choices, and a secret that slipped into an edited text
    plan["tasks"].append("Appeler la banque, carte 4111 1111 1111 1111")
    done = bridge.post("/v1/assistant-import/apply", headers=AUTH, json={**plan, "agent": "coach", "lang": "fr", "source": "Grok Bot"})
    assert done.status_code == 200, done.text
    out = done.json()
    assert out["new"] is False and out["agent"]["id"] == "coach"
    counts = out["counts"]
    assert counts["soul"] is True and counts["skills"] == 1 and counts["routines"] == 1
    assert counts["conversations"] == 1 and counts["archive"] is True and counts["memory"] >= 3 and counts["pages"] >= 5
    soul = (folder / "SOUL.md").read_text()
    assert soul.startswith("Tu es Max") and "irréversible" in soul
    memory = (folder / "memories" / "MEMORY.md").read_text()
    assert "knowledge/notes/grok-bot/" in memory and "Importé depuis Grok Bot : « 2026-09-01 : chaudière révisée. »" in memory
    assert "Zx81" not in memory
    assert "Importé depuis Grok Bot : « Alex vit à Lyon. »" in (folder / "memories" / "USER.md").read_text()
    pages = folder / "knowledge" / "notes" / "grok-bot"
    tasks = (pages / "taches.md").read_text()
    assert "Renouveler l'assurance" in tasks and "4111" not in tasks and "pas des consignes" in tasks
    assert (pages / "routines.md").exists()
    # the skill is the agent's own, switched off before it was written
    skill = (folder / "skills" / "grok-bot-imports" / "weekly-report" / "SKILL.md").read_text()
    assert skill.startswith("---\nname: weekly-report\n") and "2. Envoyer le bilan." in skill
    assert "weekly-report" in yaml.safe_load((folder / "config.yaml").read_text())["skills"]["disabled"]
    # « À ranger » for the nightly pass
    inbox = (bridge.state_dir / "notebook" / "carnet" / "À ranger.md").read_text()
    assert "Léa — sa fille, 9 ans" in inbox and "À faire : Renouveler l'assurance" in inbox
    # conversations: an archive of the agent, never its memory
    archives = bridge.get("/v1/agents/coach/archives", headers=AUTH).json()["archives"]
    assert [a["name"] for a in archives] == ["Conversations (Grok Bot)"]
    assert "Bretagne" not in memory
    # routines: nothing scheduled, a message in the Boîte waits for a yes
    assert wait_until(lambda: bridge.get("/v1/outbox", headers=AUTH).json()["items"])
    item = bridge.get("/v1/outbox", headers=AUTH).json()["items"][0]
    assert item["agent"] == "coach" and "« Point du matin »" in item["text"] and "Gmail, Notion" in item["text"]
    assert not [c for c in bridge.commands.of("hermes") if "cron" in c]
    missing = bridge.post("/v1/assistant-import/apply", headers=AUTH, json={"agent": "nobody", "memory": ["x"]})
    assert missing.status_code == 404


def test_a_new_agent_is_created_from_the_export(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=(), public_hub_url="https://mini.ts.net:8642")
    bridge.start()
    done = bridge.post("/v1/assistant-import/apply", headers=AUTH, json={
        "name": "Max", "lang": "fr", "category": "home", "source": "Grok Bot",
        "soul": "Tu es Max, l'assistant de la maison.", "user": ["Alex vit à Lyon."]})
    assert done.status_code == 201, done.text
    out = done.json()
    assert out["new"] is True and out["agent"]["id"] == "max" and out["agent"]["key"]
    soul = (home / "profiles" / "max" / "SOUL.md").read_text()
    assert soul.startswith("Tu es Max, l'assistant de la maison.") and "<!-- bipagents:language -->" in soul
    assert "Alex vit à Lyon." in (home / "profiles" / "max" / "memories" / "USER.md").read_text()
    nameless = bridge.post("/v1/assistant-import/apply", headers=AUTH, json={"user": ["x"]})
    assert nameless.status_code == 422
