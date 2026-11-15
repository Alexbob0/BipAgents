"""Creating an agent with Bip, or adopting one (bridge-rs/src/agent_create.rs, templates_catalogue.rs, 2026-11-12;
internal/notes/agent-create-bip.md): « Demander à Bip » makes the guide when there is none and starts its questions
in its main thread; « crée un agent » typed to another agent puts « Continuer avec Bip » under its reply; the card
« Créer X » carries the form's fields and its tap makes the agent as the form does; Kerfy's catalogue lists 8 agents
and adopting one makes it with its routines paused; what the person asked for is the agent's description,
personality and first words (bug 15/11)."""
import json
import uuid

from bbkit.common import AUTH
from bbkit.common import sse
from bbkit.fakes import CLOSE
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_cloud_setup import claim_body, cloud_bridge
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


def test_a_name_or_role_that_starts_with_a_dash_makes_the_agent(bridges, fakes):
    """Bug 11/11: « - Budget ». The guide's « Créer X » card and the catalogue make it as the form does: an id with no
    dash at either end, and its role (Hermes' description) never read as an option."""
    bridge, home = place(bridges, fakes, agents=("coach",))
    bip = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "fr"}).json()["agent"]["id"]
    give_token(bridge, bip, GUIDE_TOKEN)
    made = call(bridge, "propose_agent", {"kind": "finance", "name": "- Budget", "role": "-budget du foyer",
                                          "for": "me"}, token=GUIDE_TOKEN)
    assert not made["isError"], text_of(made)
    card = next(p for p in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if p["type"] == "agent")
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201, accepted.text
    agent = accepted.json()["agent"]
    assert agent["id"] == "budget" and (home / "profiles" / "budget" / "SOUL.md").is_file()
    adopted = bridge.post("/v1/templates/import", headers=AUTH, json={"catalogue": "cuisine", "lang": "fr", "name": "-x"})
    assert adopted.status_code == 201, adopted.text
    assert adopted.json()["agent"]["id"] == "x" and adopted.json()["agent"]["name"] == "-x"
    creates = [c for c in bridge.commands.of("hermes") if c[:2] == ["profile", "create"]]
    assert creates and all(not c[2].startswith("-") and not c[2].endswith("-") for c in creates), creates
    assert all(c[-1].startswith("--description=") for c in creates), creates


# -- the person's wish makes the agent, not its kind's default (bug 15/11) ------------------------------------------
# Alex's wife asked Bip for a « Training coach » with something specific; the agent she opened greeted her with the
# wellness kind's default words. What she asked for must be what the agent is: its description, its personality and
# its first words, on a place with spare agents too (agent_pool.rs: the card takes a spare, files only).

WISH = ("Me préparer à mon semi-marathon de mars : 3 sorties par semaine (mardi, jeudi, dimanche matin), un plan qui "
        "monte doucement jusqu'à 18 km, un rappel la veille de chaque séance, et un mot gentil quand j'en rate une")
KIND_DESCRIPTION = "Sommeil, sport, alimentation, bien-être"           # agent_templates.yaml, wellness
KIND_WELCOME = "Je t'accompagne pour ta santé et ton bien-être."       # welcome.rs, wellness
KIND_CHIPS = {"Je voudrais me remettre au sport, doucement.", "Aide-moi à mieux dormir.",
              "Rappelle-moi de boire de l'eau dans la journée."}


def coach_from_a_card(bridge, wish=WISH, **extra):
    """Bip's card « Créer Training coach » with the person's wish, tapped; the new agent's id."""
    args = {"kind": "wellness", "name": "Training coach", "role": wish, "tone": "doux et encourageant", "for": "me",
            "handover": "Elle court déjà 8 km sans s'arrêter.", **extra}
    made = call(bridge, "propose_agent", args, token=GUIDE_TOKEN)
    assert not made["isError"], text_of(made)
    card = next(p for p in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if p.get("name") == args["name"])
    assert card["role"] == wish   # the whole wish on the card, as she reads it before her tap
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201, accepted.text
    return accepted.json()["agent"]["id"]


def description_of(home, agent):
    import yaml
    return (yaml.safe_load((home / "profiles" / agent / "profile.yaml").read_text()) or {}).get("description") or ""


def assert_short(description, wish):
    """The wish as a description: whole, or cut at a word with « … » (review 15/11: « … un plan qui monte doucem »)."""
    if description == wish:
        return
    kept = description.removesuffix("…").rstrip()
    assert description.endswith("…") and len(description) <= 120, description
    assert wish.startswith(kept) and wish[len(kept)] in " ,;:.)", f"cut mid-word: « {description} »"


def assert_made_as_asked(bridge, home, agent, wish=WISH):
    # its description (the guide's list of agents, Hermes' profile): her words, not the kind's
    description = description_of(home, agent)
    assert description and KIND_DESCRIPTION not in description, description
    assert_short(description, wish)
    # its personality (what the model reads, and « Qui il est » in the apps): her whole wish and tone
    soul = (home / "profiles" / agent / "SOUL.md").read_text()
    assert wish in soul and "doux et encourageant" in soul, soul
    # its first words when she opens it (welcome.rs, shown by both apps as its first bubble): her wish, not the kind's
    welcome = bridge.get("/v1/guide", headers=AUTH).json()["welcome"][agent]
    assert "semi-marathon de mars" in welcome["text"] and KIND_WELCOME not in welcome["text"], welcome["text"]
    assert not KIND_CHIPS & set(welcome["suggestions"]), welcome["suggestions"]


def test_the_persons_wish_makes_the_agent_not_its_kinds_default(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    bip = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "fr"}).json()["agent"]["id"]
    give_token(bridge, bip, GUIDE_TOKEN)
    agent = coach_from_a_card(bridge)
    assert agent == "training-coach"
    assert_made_as_asked(bridge, home, agent)


def test_the_persons_wish_makes_the_agent_from_a_spare_too(bridges, fakes):
    """A place of the offer keeps spare agents made ahead (agent_pool.rs): the card's agent takes one, files only."""
    bridge, cloud = cloud_bridge(bridges, fakes, start=False)
    bridge.config["place"] = {"spare_agents": 2}
    cloud.done()
    bridge.start()
    pool = bridge.root / "spare-agents.json"
    spares = lambda: [s["id"] for s in json.loads(pool.read_text())] if pool.exists() else []   # noqa: E731
    assert wait_until(lambda: len(spares()) == 2, timeout=30)
    claimed = bridge.post("/v1/setup/claim", json=claim_body(cloud.challenge()))
    assert claimed.status_code == 200, claimed.text
    bip = claimed.json()["agents"][0]["id"]   # « Quotidien », a spare already, the place's guide
    assert bridge.get("/v1/guide", headers=AUTH).json()["guide"] == bip
    assert wait_until(lambda: len(spares()) == 2, timeout=30)   # the pool made up again, in the background
    ready = spares()
    give_token(bridge, bip, GUIDE_TOKEN)
    bridge.commands.clear()
    agent = coach_from_a_card(bridge)
    assert agent in ready and agent not in spares()   # taken from the pool, files only
    assert not any(c[:3] == ["profile", "create", agent] for c in bridge.commands.of("hermes"))
    assert_made_as_asked(bridge, bridge.root / ".hermes", agent)


def test_the_forms_role_is_the_agents_first_words_too(bridges, fakes):
    """The form (« Le créer moi-même ») with a role: the same first words as the card's, a kind's or « Autre »."""
    bridge, home = place(bridges, fakes, agents=("coach",))
    made = bridge.post("/v1/agents", headers=AUTH, json={"name": "Foulée", "kind": "wellness", "lang": "fr",
                                                         "role": WISH, "tone": "doux et encourageant"})
    assert made.status_code == 201, made.text
    welcome = bridge.get("/v1/guide", headers=AUTH).json()["welcome"][made.json()["id"]]
    assert "semi-marathon de mars" in welcome["text"] and KIND_WELCOME not in welcome["text"], welcome["text"]
    own = bridge.post("/v1/agents", headers=AUTH, json={"name": "Potager", "lang": "fr",
                                                        "custom": {"role": "Suivre mes semis de tomates et me dire quand arroser",
                                                                   "category": "home"}})
    assert own.status_code == 201, own.text
    welcome = bridge.get("/v1/guide", headers=AUTH).json()["welcome"][own.json()["id"]]
    assert "semis de tomates" in welcome["text"] and "Je suis là pour t'aider." not in welcome["text"], welcome["text"]
    # without a role: its kind's own welcome, as before
    plain = bridge.post("/v1/agents", headers=AUTH, json={"name": "Forme", "kind": "wellness", "lang": "fr"})
    assert KIND_WELCOME in bridge.get("/v1/guide", headers=AUTH).json()["welcome"][plain.json()["id"]]["text"]
    assert KIND_DESCRIPTION in description_of(home, plain.json()["id"])


def test_a_family_card_keeps_the_wish_as_the_agents_description(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach",))
    bip = bridge.post("/v1/guide/create-agent", headers=AUTH, json={"lang": "fr"}).json()["agent"]["id"]
    give_token(bridge, bip, GUIDE_TOKEN)
    wish = "Les menus de la semaine et la liste de courses, avec un repas sans viande le lundi"
    call(bridge, "propose_agent", {"kind": "home", "name": "Cantine", "role": wish, "for": "family"}, token=GUIDE_TOKEN)
    card = next(p for p in bridge.get("/v1/guide", headers=AUTH).json()["proposals"] if p.get("name") == "Cantine")
    accepted = bridge.post(f"/v1/guide/proposals/{card['id']}/accept", headers=AUTH, json={"lang": "fr"})
    assert accepted.status_code == 201, accepted.text
    agent = accepted.json()["agent"]["id"]
    assert accepted.json()["family"] is True
    assert wish in (home / "profiles" / agent / "SOUL.md").read_text()
    description = description_of(home, agent)
    assert len(description) > 20, description
    assert_short(description, wish)
