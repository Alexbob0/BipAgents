"""WhatsApp chats imported into an agent (bridge-rs/src/whatsapp_import/, 2026-10-25): an export shared from the
phone read into a summary with its estimate (nothing kept but the upload), then kept whole in the agent's « WhatsApp »
archive and read for the Carnet by a model; a chat exported again adds only its new part; an agent shared with a
relative is refused. Fictional names, a fake model answering canned JSON."""
import base64
import io
import zipfile
from datetime import date, datetime, timedelta

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install, profile
from test_shared_agents import link
from test_households_and_peers import household

TODAY = date.today()
SOON = (TODAY + timedelta(days=12)).isoformat()


def stamp(days_ago, hour):
    """An iPhone line's date in French: [25/10/2026 14:03:12]."""
    at = datetime.combine(TODAY - timedelta(days=days_ago), datetime.min.time()).replace(hour=hour)
    return at.strftime("[%d/%m/%Y %H:%M:%S]")


FIRST = (
    f"‎{stamp(5, 9)} Léa Martin: ‎Les messages et les appels sont chiffrés de bout en bout.\n"
    f"{stamp(5, 10)} Léa Martin: Coucou ! On fête les 7 ans de Hugo chez Marc, tu viens ?\n"
    f"{stamp(5, 11)} Alex Dubois: Oui !\nJe t'envoie le devis du traiteur ce soir\n"
    f"‎{stamp(4, 9)} Léa Martin: ‎image absente\n"
    f"{stamp(4, 10)} Léa Martin: Mon code de carte : 4111 1111 1111 1111\n"
)
LATER = FIRST + (
    f"{stamp(1, 9)} Léa Martin: Marc a réservé la salle des fêtes\n"
    f"{stamp(1, 10)} Alex Dubois: Parfait\n"
)


def zipped(text):
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        archive.writestr("_chat.txt", text)
    return base64.b64encode(buffer.getvalue()).decode()


def started(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb",
                                                             "model": "small-model"}))
    home = managed_install(bridge, fakes, agents=("coach",))
    folder = profile(home, "coach")
    (folder / "SOUL.md").write_text("Tu es Coach.\n")
    (folder / "config.yaml").write_text("model:\n  provider: custom\n")
    (bridge.state_dir / "notebook").mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "notebook" / "state.json").write_text('{"language": "fr"}')   # a French Carnet
    bridge.start()
    return bridge, folder


def parse(bridge, text, agent="coach", name="WhatsApp Chat - Léa Martin.zip"):
    return bridge.post("/v1/import/whatsapp/parse", headers=AUTH,
                       json={"name": name, "data": zipped(text), "agent": agent})


def status(bridge):
    return bridge.get("/v1/import/whatsapp/status", params={"agent": "coach"}, headers=AUTH).json()


def whatsapp_chats(fakes):
    return [c for c in fakes.model.chats
            if c["body"]["messages"][0]["content"].startswith("You read a WhatsApp chat")]


def test_a_chat_is_archived_whole_and_fills_the_carnet_then_only_its_new_part(bridges, fakes):
    bridge, folder = started(bridges, fakes)
    summary = parse(bridge, FIRST)
    assert summary.status_code == 200, summary.text
    summary = summary.json()
    assert summary["chat_title"] == "Léa Martin" and summary["participants"] == ["Léa Martin", "Alex Dubois"]
    assert summary["messages"] == 3 and summary["media_skipped"] == 1 and summary["new_messages"] == 3
    assert summary["already_imported"] is False and summary["already_imported_until"] is None
    assert summary["period"]["from"].startswith((TODAY - timedelta(days=5)).isoformat())
    assert summary["notebook"] is True and summary["me"] == "Alex Dubois"
    assert summary["estimate"]["calls"] == 1 and summary["estimate"]["tokens_in"] > 300
    assert summary["model"]["local"] is True          # the fake model is on this machine: free
    assert not whatsapp_chats(fakes)                  # nothing sent before « Importer »

    fakes.model.whatsapp_answer = {
        "people": [{"name": "Léa Martin", "relation": "friend", "facts": ["Organise l'anniversaire de Hugo"]},
                   {"name": "Hugo", "relation": "family", "facts": ["Va avoir 7 ans"]},
                   {"name": "Personne Inventée", "relation": "friend", "facts": ["N'est pas dans la discussion"]}],
        "subjects": [{"title": "Anniversaire de Hugo", "facts": ["Fête chez Marc"]}],
        "dates": [{"date": SOON, "what": "Anniversaire de Hugo chez Marc", "person": "Léa Martin"}],
        "tasks": [{"title": "Envoyer le devis du traiteur à Léa", "due": ""}],
    }
    applied = bridge.post("/v1/import/whatsapp/apply", headers=AUTH,
                          json={"agent": "coach", "upload": summary["upload"]})
    assert applied.status_code == 202, applied.text
    assert applied.json()["filling"] is True and applied.json()["new_messages"] == 3
    assert wait_until(lambda: status(bridge)["state"] == "done", timeout=15), status(bridge)
    assert status(bridge)["counts"] == {"people": 2, "subjects": 1, "dates": 1, "tasks": 1}
    # the model got the chat's text, without its media lines nor the card number (scrubbed, as in the archive)
    prompt = whatsapp_chats(fakes)[0]["body"]["messages"][1]["content"]
    assert "Coucou ! On fête les 7 ans de Hugo" in prompt and "Alex Dubois" in prompt and "image absente" not in prompt
    assert "4111" not in prompt

    # the whole chat, in the agent's « WhatsApp » archive, a file a month, without the card number
    files = sorted((folder / "imports" / "whatsapp" / "lea-martin").glob("*.md"))
    assert files and all(f.stat().st_mode & 0o077 == 0 for f in files)
    kept = "".join(f.read_text() for f in files)
    assert "Je t'envoie le devis du traiteur ce soir" in kept and "4111" not in kept
    archives = bridge.get("/v1/agents/coach/archives", headers=AUTH).json()["archives"]
    assert [(a["name"], a["kind"]) for a in archives] == [("WhatsApp", "folder")]
    assert wait_until(lambda: bridge.get("/v1/agents/coach/archives", headers=AUTH).json()["archives"][0]["documents"]
                      == len(files))

    # the Carnet: who is who, the subject, the date, the task; each fact links its month of the chat
    carnet = bridge.state_dir / "notebook" / "carnet"
    lea = (carnet / "Personnes" / "Léa Martin.md").read_text()
    assert "Organise l'anniversaire de Hugo" in lea and f"Anniversaire de Hugo chez Marc : {SOON}" in lea
    assert "[[archive:coach/whatsapp/" in lea
    assert not (carnet / "Personnes" / "Personne Inventée.md").exists()
    assert (carnet / "Sujets" / "Anniversaire de Hugo.md").exists()
    assert "Envoyer le devis du traiteur à Léa" in (carnet / "Tâches" / "Tâches.md").read_text()
    reference = lea.split("[[archive:")[1].split("|")[0]
    document = bridge.get("/v1/archives/document", params={"ref": reference}, headers=AUTH)
    assert document.status_code == 200 and "On fête les 7 ans de Hugo" in document.json()["text"]
    # done: a line in the agent's Boîte
    assert wait_until(lambda: bridge.get("/v1/outbox", headers=AUTH).json()["items"])
    item = bridge.get("/v1/outbox", headers=AUTH).json()["items"][0]
    assert item["agent"] == "coach" and "2 personnes, 1 sujet, 1 date, 1 tâche ajoutés au Carnet" in item["text"]

    # exported again two days later: recognised, only the two new messages added and read
    again = parse(bridge, LATER, name="WhatsApp Chat - Léa ❤️.zip").json()
    assert again["already_imported"] is True and again["new_messages"] == 2
    assert again["already_imported_until"] == summary["period"]["to"]
    before = len(whatsapp_chats(fakes))
    fakes.model.whatsapp_answer = {"people": [], "subjects": [], "dates": [], "tasks": []}
    assert bridge.post("/v1/import/whatsapp/apply", headers=AUTH,
                       json={"agent": "coach", "upload": again["upload"]}).status_code == 202
    assert wait_until(lambda: status(bridge)["state"] == "done" and status(bridge)["new_messages"] == 2, timeout=15)
    new_prompts = [c["body"]["messages"][1]["content"] for c in whatsapp_chats(fakes)[before:]]
    assert len(new_prompts) == 1 and "salle des fêtes" in new_prompts[0] and "Coucou" not in new_prompts[0]
    kept = "".join(f.read_text() for f in sorted((folder / "imports" / "whatsapp" / "lea-martin").glob("*.md")))
    assert kept.count("Je t'envoie le devis du traiteur ce soir") == 1 and "salle des fêtes" in kept
    # the very same export once more: nothing new
    assert parse(bridge, LATER).json()["new_messages"] == 0

    # what isn't an export, an upload that is gone
    bad = bridge.post("/v1/import/whatsapp/parse", headers=AUTH,
                      json={"name": "notes.txt", "data": base64.b64encode(b"hello\nworld\n").decode(), "agent": "coach"})
    assert bad.status_code == 422
    gone = bridge.post("/v1/import/whatsapp/apply", headers=AUTH, json={"agent": "coach", "upload": "0" * 24})
    assert gone.status_code == 404
    assert parse(bridge, FIRST, agent="nobody").status_code == 404
    assert bridge.post("/v1/import/whatsapp/parse", json={}).status_code == 401


def test_an_agent_shared_with_a_relative_never_takes_a_chat(bridges, fakes):
    a = household(bridges, fakes, "alex", ["cuisine"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]}).status_code == 200
    refused = parse(a, FIRST, agent="cuisine")
    assert refused.status_code == 409 and refused.json()["reason"] == "shared"
    applied = a.post("/v1/import/whatsapp/apply", headers=AUTH, json={"agent": "cuisine", "upload": "0" * 24})
    assert applied.status_code == 409
