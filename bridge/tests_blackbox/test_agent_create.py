"""Creating an agent with Bip, or adopting one (bridge-rs/src/agent_create.rs, templates_catalogue.rs, 2026-11-12;
internal/notes/agent-create-bip.md): « Demander à Bip » makes the guide when there is none and starts its questions
in its main thread; « crée un agent » typed to another agent puts « Continuer avec Bip » under its reply; the card
« Créer X » carries the form's fields and its tap makes the agent as the form does; Kerfy's catalogue lists 8 agents
and adopting one makes it with its routines paused."""
import uuid

from bbkit.common import AUTH
from bbkit.common import sse
from bbkit.fakes import CLOSE
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_guide import place

GUIDE_TOKEN = "guide-token-0123456789abcdefghijkl"


def next_run(fakes):
    """The id the fake Hermes gives the next run, with an empty answer ready for it."""
    run = f"r{len(fakes.hermes.started) + 1}"
    fakes.hermes.runs[run] = [sse("run.completed", run_id=run, output=""), CLOSE]
    return run


def send(bridge, fakes, agent, text):
    """The person's message to `agent`'s main thread; what Hermes got as input."""
    next_run(fakes)
    sent = bridge.post("/v1/threads/main/messages", headers=AUTH,
                       json={"agent": agent, "client_id": uuid.uuid4().hex, "text": text})
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    return fakes.hermes.started[sent.json()["run_id"]]["input"]


def test_ask_bip_makes_the_guide_first_and_starts_its_questions(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))   # an own agent, no guide (« je crée mes agents moi-même »)
    assert bridge.get("/v1/guide", headers=AUTH).json()["guide"] is None
    next_run(fakes)
    started = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "fr"})
    assert started.status_code == 200, started.text
    out = started.json()
    assert out["created"] is True and out["agent"]["name"] == "Bip" and out["state"] == "started", out
    bip = out["agent"]["id"]
    assert bridge.get("/v1/guide", headers=AUTH).json()["guide"] == bip
    sent = fakes.hermes.started[out["run_id"]]
    # the person's short visible message, the rules hidden before it
    assert sent["input"].endswith("Je veux créer un agent"), sent["input"]
    assert "<!--bip:create-agent" in sent["input"] and "propose_agent" in sent["input"]
    assert "for whom" not in sent["input"]   # no family linked: not asked
    timeline = bridge.get(f"/v1/threads/{out['thread']}/timeline", headers=AUTH, params={"agent": bip}).json()
    assert [i["text"] for i in timeline["items"]] == ["Je veux créer un agent"]
    assert "<!-- bipagents:guide -->" in (home / "profiles" / bip / "SOUL.md").read_text()
    # again: the same Bip, no new agent
    assert wait_until(lambda: not bridge.get(f"/v1/threads/{out['thread']}/timeline", headers=AUTH,
                                             params={"agent": bip}).json()["pending"]["run"])
    next_run(fakes)
    again = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "en"}).json()
    assert again["created"] is False and again["agent"]["id"] == bip
    assert fakes.hermes.started[again["run_id"]]["input"].endswith("I want to create an agent")


def test_the_words_in_another_agents_conversation_give_continue_with_bip(bridges, fakes):
    bridge, _ = place(bridges, fakes, agents=("coach",))
    # no guide yet: the card is there all the same, its tap makes Bip
    sent = send(bridge, fakes, "coach", "crée-moi un agent de cuisine")
    assert "Continuer avec Bip" in sent and "don't propose an agent yourself" in sent
    card = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert (card["type"], card["agent"], card["target"], card["target_name"], card["create"]) == \
        ("switch", "coach", None, "Bip", True)
    assert card["summary"] == "crée-moi un agent de cuisine"
    next_run(fakes)
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 200, accepted.text
    out = accepted.json()
    assert out["type"] == "switch" and out["create"] is True and out["created"] is True
    assert out["agent"]["name"] == "Bip" and out["from"] == {"id": "coach", "name": "Coach"} and out["thread"]
    first = fakes.hermes.started[f"r{len(fakes.hermes.started)}"]["input"]
    assert first.endswith("Je veux créer un agent") and "They first asked Coach" in first and "cuisine" in first
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"] == []
    # with a guide: the card targets it; anything else said: no card
    send(bridge, fakes, "coach", "Create an agent for my budget")
    card = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert card["target"] == out["agent"]["id"] and card["target_name"] == "Bip"
    assert "bip:create-agent" not in send(bridge, fakes, "coach", "Quel temps fait-il ?")
    assert len(bridge.get("/v1/guide", headers=AUTH).json()["proposals"]) == 1


def test_the_card_carries_the_forms_fields_and_makes_the_same_agent(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    bip = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "fr"}).json()["agent"]["id"]
    give_token(bridge, bip, GUIDE_TOKEN)
    made = call(bridge, "propose_agent", {"kind": "wellness", "name": "Forme", "role": "Programme mes séances",
                                          "tone": "motivant", "for": "me", "tools": ["web", "health", "agenda", "zzz"],
                                          "handover": "Court le matin."}, token=GUIDE_TOKEN)
    assert not made["isError"] and "A card to create Forme is shown" in text_of(made)
    card = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert (card["name"], card["kind"], card["role"], card["tone"], card["for"]) == \
        ("Forme", "wellness", "Programme mes séances", "motivant", "me")
    assert card["members"] == [] and card["tools"] == ["web", "health", "calendar"] and card["services"] == []
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201, accepted.text
    out = accepted.json()
    assert out["type"] == "agent" and out["family"] is False and out["kind"] == "wellness"
    assert out["suggested"] == ["health", "calendar"] and out["agent"]["category"] == "wellness"
    # the form, with the same fields: the same personality (its name aside)
    form = bridge.post("/v1/agents", headers=AUTH, json={"name": "Forme bis", "kind": "wellness", "lang": "fr",
                                                         "role": "Programme mes séances", "tone": "motivant"})
    assert form.status_code == 201, form.text
    soul = lambda a: (home / "profiles" / a / "SOUL.md").read_text()   # noqa: E731
    card_soul = soul(out["agent"]["id"]).replace("Forme", "X")
    form_soul = soul(form.json()["id"]).replace("Forme bis", "X")
    assert card_soul == form_soul
    assert "motivant" in card_soul
    # for the family: a first name that isn't in the family is said, nothing made
    call(bridge, "propose_agent", {"kind": "home", "name": "Foyer", "role": "Les courses", "for": "family",
                                   "members": ["Léa"]}, token=GUIDE_TOKEN)
    family = next(p for p in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if p["name"] == "Foyer")
    assert family["for"] == "family" and family["members"] == ["Léa"]
    refused = bridge.post(f"/v1/guide/proposals/{family['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert refused.status_code == 409 and "Léa n'est pas dans ta famille" in refused.json()["detail"]


def test_the_catalogue_lists_eight_and_adopting_one_pauses_its_routines(bridges, fakes):
    bridge, _ = place(bridges, fakes, agents=("coach",))
    listed = bridge.get("/v1/templates/catalogue", headers=AUTH, params={"lang": "fr"}).json()["templates"]
    assert [t["id"] for t in listed] == ["quotidien", "sante", "budget", "voyages", "cuisine", "devoirs", "papiers",
                                         "maison"]
    cuisine = listed[4]
    assert cuisine["name"] == "Cuisine" and cuisine["look"]["category"] == "home" and cuisine["source"] == "kerfy"
    assert len(cuisine["routines"]) == 2 and cuisine["languages"] == ["fr", "en", "de", "es"]
    assert [t["family"] for t in listed].count(True) == 1
    assert bridge.get("/v1/templates/catalogue", headers=AUTH, params={"lang": "es"}).json()["templates"][4]["name"] == "Cocina"
    adopted = bridge.post("/v1/templates/import", headers=AUTH, json={"catalogue": "cuisine", "lang": "fr"})
    assert adopted.status_code == 201, adopted.text
    out = adopted.json()
    agent = out["agent"]["id"]
    assert out["agent"]["name"] == "Cuisine" and out["agent"]["category"] == "home" and out["family"] is False
    assert [r["on"] for r in out["routines"]] == [False, False]
    assert out["first_message"].startswith("Bonjour, je suis Cuisine.")
    routines = bridge.get(f"/v1/agents/{agent}/template-routines", headers=AUTH).json()["routines"]
    assert [r["name"] for r in routines] == ["Menus de la semaine", "Liste de courses"]
    assert not any(r["on"] for r in routines)
    welcome = bridge.get("/v1/guide", headers=AUTH).json()["welcome"][agent]
    assert welcome["text"] == out["first_message"]
    assert bridge.post("/v1/templates/import", headers=AUTH, json={"catalogue": "nope"}).status_code == 404
    assert bridge.post("/v1/templates/import", headers=AUTH,
                       json={"catalogue": "cuisine", "family": True}).status_code == 400
