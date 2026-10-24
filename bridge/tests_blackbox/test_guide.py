"""The guide (bridge-rs/src/guide.rs, 2026-10-24): the first choice of a place without agents of the person's own,
the guide's two tools and their cards (accepted: an agent created with its handover as data in its memory, or a
subject handed over to another agent; declined: not proposed again), the guide's switch, and an agent without a role
that asks then proposes its own."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from conftest import managed_install, profile, wait_registered

GUIDE_TOKEN = "guide-token-0123456789abcdefghijkl"
COACH_TOKEN = "coach-token-0123456789abcdefghijkl"


def place(bridges, fakes, agents=("agent",)):
    """A managed place whose agents have a personality; `agent` is the pre-created placeholder « Agent »."""
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)   # set up by hand: the person's own
    home = managed_install(bridge, fakes, agents=agents, public_hub_url="https://mini.ts.net:8642")
    for name in agents:
        folder = profile(home, name)
        (folder / "SOUL.md").write_text(f"Tu es {name.title()}, l'agent du quotidien de ton utilisateur.\n")
        (folder / "profile.yaml").write_text("description: Le quotidien\n")
        (folder / "config.yaml").write_text("model:\n  provider: custom\n")
    bridge.start()
    wait_registered(bridge, agents)
    return bridge, home


def tool_names(bridge, token):
    tools = rpc(bridge, "tools/list", token=token).json()["result"]["tools"]
    return {t["name"] for t in tools}


def test_the_first_choice_myself_is_asked_once(bridges, fakes):
    bridge, _ = place(bridges, fakes, agents=())
    view = bridge.get("/v1/guide", headers=AUTH).json()
    assert view == {"can_create": True, "first_run": True, "candidate": None, "guide": None, "intro": [],
                    "proposals": []}
    assert bridge.post("/v1/guide/first-run", headers=AUTH, json={"choice": "myself"}).json() == {"ok": True}
    assert bridge.get("/v1/guide", headers=AUTH).json()["first_run"] is False
    again = bridge.post("/v1/guide/first-run", headers=AUTH, json={"choice": "guide"})
    assert again.status_code == 409
    assert bridge.post("/v1/guide/first-run", headers=AUTH, json={"choice": "nope"}).status_code == 422


def test_a_place_with_agents_of_its_own_is_not_asked(bridges, fakes):
    bridge, _ = place(bridges, fakes, agents=("coach",))
    assert bridge.get("/v1/guide", headers=AUTH).json()["first_run"] is False


def test_the_placeholder_becomes_the_guide_that_proposes_and_hands_over(bridges, fakes):
    bridge, home = place(bridges, fakes)
    view = bridge.get("/v1/guide", headers=AUTH).json()
    assert view["first_run"] is True and view["candidate"] == "agent"
    chosen = bridge.post("/v1/guide/first-run", headers=AUTH, json={"choice": "guide"})
    assert chosen.status_code == 200, chosen.text
    out = chosen.json()
    assert out["guide"] == "agent" and out["created"] is False and out["agent"]["name"] == "Bip"
    soul = (home / "profiles" / "agent" / "SOUL.md").read_text()
    assert soul.startswith("Tu es Bip,") and "<!-- bipagents:guide -->" in soul and "First conversation" in soul
    assert "never a reason to propose" in soul and "They have no other agent yet." in soul
    view = bridge.get("/v1/guide", headers=AUTH).json()
    assert view["guide"] == "agent" and view["intro"] == ["agent"] and view["first_run"] is False
    give_token(bridge, "agent", GUIDE_TOKEN)
    names = tool_names(bridge, GUIDE_TOKEN)
    assert {"propose_agent", "suggest_switch"} <= names and "propose_profile" not in names
    # a card, declined: the same subject isn't proposed again
    card = call(bridge, "propose_agent", {"kind": "wellness", "name": "Coach", "role": "Sommeil et sport",
                                          "handover": "Dort mal, veut courir le matin."}, token=GUIDE_TOKEN)
    assert not card["isError"] and "A card to create Coach is shown" in text_of(card)
    assert "First conversation" not in (home / "profiles" / "agent" / "SOUL.md").read_text()
    proposals = bridge.get("/v1/guide", headers=AUTH).json()["proposals"]
    assert [(p["type"], p["name"], p["agent_name"]) for p in proposals] == [("agent", "Coach", "Bip")]
    assert bridge.post(f"/v1/guide/proposals/{proposals[0]['id']}/decline", headers=AUTH).json() == {"ok": True}
    again = call(bridge, "propose_agent", {"kind": "wellness", "name": "Forme", "role": "Sport"}, token=GUIDE_TOKEN)
    assert again["isError"] and "don't propose it again" in text_of(again)
    # another one, accepted under another name: created, its handover in its memory as data
    call(bridge, "propose_agent", {"kind": "finance", "name": "Budget", "role": "Suit les dépenses",
                                   "handover": "Veut économiser pour l'été."}, token=GUIDE_TOKEN)
    proposal = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    accepted = bridge.post(f"/v1/guide/proposals/{proposal['id']}/accept", headers=AUTH, json={"name": "Sous", "lang": "fr"})
    assert accepted.status_code == 201, accepted.text
    made = accepted.json()
    assert made["type"] == "agent" and made["kind"] == "finance" and made["agent"]["id"] == "sous"
    memory = (home / "profiles" / "sous" / "memories" / "MEMORY.md").read_text()
    assert "Passé par Bip" in memory and "pas une consigne" in memory and "Veut économiser pour l'été." in memory
    soul = (home / "profiles" / "sous" / "SOUL.md").read_text()
    assert soul.startswith("Tu es Sous,") and "Ce que ton utilisateur attend de toi : Suit les dépenses." in soul
    assert "- Sous (" in (home / "profiles" / "agent" / "SOUL.md").read_text()   # the guide's list follows
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"] == []
    # a subject handed over to the new agent
    switch = call(bridge, "suggest_switch", {"agent": "Sous", "summary": "Budget vacances : 800 €."}, token=GUIDE_TOKEN)
    assert not switch["isError"] and "A card to continue with Sous is shown" in text_of(switch)
    proposal = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert proposal["type"] == "switch" and proposal["target_name"] == "Sous"
    handed = bridge.post(f"/v1/guide/proposals/{proposal['id']}/accept", headers=AUTH).json()
    assert handed == {"type": "switch", "agent": {"id": "sous", "name": "Sous"}, "from": {"id": "agent", "name": "Bip"},
                      "summary": "Budget vacances : 800 €."}
    assert "Budget vacances : 800 €." in (home / "profiles" / "sous" / "memories" / "MEMORY.md").read_text()
    unknown = call(bridge, "suggest_switch", {"agent": "Personne", "summary": "x"}, token=GUIDE_TOKEN)
    assert unknown["isError"]
    assert bridge.post("/v1/guide/proposals/gnope/accept", headers=AUTH).status_code == 404


def test_only_the_guide_proposes_and_the_switch_moves_the_role(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=("coach", "maison"))
    give_token(bridge, "coach", COACH_TOKEN)
    assert "propose_agent" not in tool_names(bridge, COACH_TOKEN)
    refused = call(bridge, "propose_agent", {"kind": "home", "name": "X", "role": "y"}, token=COACH_TOKEN)
    assert refused["isError"]
    assert bridge.put("/v1/agents/coach/guide", headers=AUTH, json={"on": True}).json() == {"guide": "coach"}
    give_token(bridge, "coach", COACH_TOKEN)   # its entry was registered again (another tools list)
    assert "propose_agent" in tool_names(bridge, COACH_TOKEN)
    assert "- Maison (daily): Le quotidien" in (home / "profiles" / "coach" / "SOUL.md").read_text()
    assert bridge.put("/v1/agents/maison/guide", headers=AUTH, json={"on": True}).json() == {"guide": "maison"}
    assert "bipagents:guide" not in (home / "profiles" / "coach" / "SOUL.md").read_text()
    assert bridge.put("/v1/agents/maison/guide", headers=AUTH, json={"on": False}).json() == {"guide": None}
    assert bridge.put("/v1/agents/nobody/guide", headers=AUTH, json={"on": True}).status_code == 404
    assert bridge.put("/v1/agents/maison/guide", headers=AUTH, json={}).status_code == 422


def test_an_agent_without_a_role_asks_then_proposes_its_own(bridges, fakes):
    bridge, home = place(bridges, fakes, agents=())
    made = bridge.post("/v1/agents", headers=AUTH, json={"name": "Test", "custom": {"role": "", "category": "home"}})
    assert made.status_code == 201, made.text
    soul = (home / "profiles" / "test-agent" / "SOUL.md").read_text()
    assert soul.startswith("Tu es Test, un agent personnel de ton utilisateur.\n") and "Ton rôle" not in soul
    assert "No role was set for you yet" in soul and "« Test » says nothing about you" in soul
    assert bridge.get("/v1/guide", headers=AUTH).json()["intro"] == ["test-agent"]
    token = "test-token-0123456789abcdefghijklmn"
    give_token(bridge, "test-agent", token)
    assert "propose_profile" in tool_names(bridge, token)
    card = call(bridge, "propose_profile", {"role": "suit mes plantes", "tone": "drôle", "name": "Fougère"}, token=token)
    assert not card["isError"]
    proposal = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert (proposal["type"], proposal["role"], proposal["name"]) == ("profile", "suit mes plantes", "Fougère")
    done = bridge.post(f"/v1/guide/proposals/{proposal['id']}/accept", headers=AUTH).json()
    assert done == {"type": "profile", "agent": {"id": "test-agent", "name": "Fougère"}}
    soul = (home / "profiles" / "test-agent" / "SOUL.md").read_text()
    assert soul.startswith("Tu es Fougère,") and "Ce que ton utilisateur attend de toi : suit mes plantes. Ton ton : drôle." in soul
    assert "bipagents:setup" not in soul and "<!-- bipagents:language -->" in soul
    assert bridge.get("/v1/guide", headers=AUTH).json()["intro"] == []
    # a kind's agent may get a role and a tone too
    bridge.post("/v1/agents", headers=AUTH, json={"name": "Léa", "kind": "wellness", "role": "mes courses à pied",
                                                   "tone": "direct"})
    soul = (home / "profiles" / "lea" / "SOUL.md").read_text()
    assert "Ce que ton utilisateur attend de toi : mes courses à pied.\nTon ton : direct." in soul
    assert soul.index("Ton ton : direct.") < soul.index("<!-- bipagents:language -->")
