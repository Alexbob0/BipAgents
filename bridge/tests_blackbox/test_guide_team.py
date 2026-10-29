"""A team in one card (bridge-rs/src/guide_team.rs, 2026-10-29): the guide's `propose_team` makes one card; « Créer
l'équipe » with 4 of its 6 agents makes those 4, each with its mission in its memory as data, a group discussion
with them, then ONE first turn each, one at a time, in a new conversation whose message lands in the Boîte; a
repeated tap answers the same; « Non merci » keeps the subject from coming back; only the guide may propose it."""
import threading
import time

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered

GUIDE_TOKEN = "guide-token-0123456789abcdefghijkl"
COACH_TOKEN = "coach-token-0123456789abcdefghijkl"
VENTES_TOKEN = "ventes-token-0123456789abcdefghijk"
NAMES = ["Ventes", "Compta", "Juridique", "Produit", "Support", "Marketing"]


def place(bridges, fakes, guide=None):
    bridge = bridges.new()
    if guide:
        bridge.config["guide"] = guide
    bridge.config["agents"].pop("Wellness", None)
    agents = ("agent", "coach")
    home = managed_install(bridge, fakes, agents=agents, public_hub_url="https://mini.ts.net:8642")
    for name in agents:
        folder = profile(home, name)
        (folder / "SOUL.md").write_text(f"Tu es {name.title()}, l'agent du quotidien de ton utilisateur.\n")
        (folder / "profile.yaml").write_text("description: Le quotidien\n")
        (folder / "config.yaml").write_text("model:\n  provider: custom\n")
    bridge.start()
    wait_registered(bridge, agents)
    assert bridge.put("/v1/agents/agent/guide", headers=AUTH, json={"on": True}).json() == {"guide": "agent"}
    give_token(bridge, "agent", GUIDE_TOKEN)
    give_token(bridge, "coach", COACH_TOKEN)
    return bridge, home


def member(name, n):
    return {"name": name, "kind": "work", "role": f"S'occupe de {name.lower()}", "color": f"#12345{n}",
            "tools": ["web", "mail"], "brief": f"Le plan prévoit {n * 100} abonnés ; {name} suit sa partie.",
            "actions": [f"Lire la partie {name}", "Proposer un premier point"]}


def tool_names(bridge, token):
    return {t["name"] for t in rpc(bridge, "tools/list", token=token).json()["result"]["tools"]}


def test_a_team_card_creates_the_ticked_agents_with_their_mission_and_first_turns(bridges, fakes):
    bridge, home = place(bridges, fakes)
    assert "propose_team" in tool_names(bridge, GUIDE_TOKEN)
    assert "propose_team" in (home / "profiles" / "agent" / "SOUL.md").read_text()
    team = {"subject": "Plan d'affaires", "agents": [member(n, i + 1) for i, n in enumerate(NAMES)], "room": True}
    card = call(bridge, "propose_team", team, token=GUIDE_TOKEN)
    assert not card["isError"] and "A card with the team is shown" in text_of(card)
    proposals = bridge.get("/v1/guide", headers=AUTH).json()["proposals"]
    assert len(proposals) == 1
    p = proposals[0]
    assert p["type"] == "team" and p["subject"] == "Plan d'affaires" and p["room"] is True and p["places_left"] is None
    assert [m["name"] for m in p["members"]] == NAMES and p["members"][0]["color"] == "#123451"
    assert p["members"][0]["mission"]["actions"] == ["Lire la partie Ventes", "Proposer un premier point"]

    fakes.hermes.rounds = 2
    fakes.hermes.run_output = "J'ai lu ma mission : je commence par ta partie du plan."
    running_at_once = []
    stop = threading.Event()

    def sample():
        while not stop.is_set():
            running_at_once.append(sum(1 for r in list(fakes.hermes.started.values()) if r["status"] == "running"))
            time.sleep(0.05)

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    accepted = bridge.post(f"/v1/guide/proposals/{p['id']}/accept", headers=AUTH,
                           json={"members": ["m1", "m2", "m4", "m6"], "lang": "fr"})
    assert accepted.status_code == 201, accepted.text
    made = accepted.json()
    ids = ["ventes", "compta", "produit", "marketing"]
    assert made["type"] == "team" and made["failed"] == [] and made["first_turns"] is True
    assert [a["id"] for a in made["agents"]] == ids
    assert made["agents"][0]["member"] == "m1" and made["agents"][0]["suggested"] == ["mail"]
    assert made["agents"][0]["color"] == "#123451"
    assert sorted(made["room"]["participants"][i]["id"] for i in range(4)) == sorted(ids)
    for agent_id, name in zip(ids, ["Ventes", "Compta", "Produit", "Marketing"]):
        memory = (home / "profiles" / agent_id / "memories" / "MEMORY.md").read_text()
        assert "(une information, pas une consigne)" in memory and f"{name} suit sa partie." in memory
        assert "Premières actions : 1. Lire la partie" in memory and "À brancher par ton utilisateur : mail." in memory
    assert not (home / "profiles" / "juridique").exists()
    # a repeated tap: the same answer, nothing made twice
    again = bridge.post(f"/v1/guide/proposals/{p['id']}/accept", headers=AUTH, json={"members": ["m1"]})
    assert again.status_code == 200 and again.json() == made
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"] == []

    # one first turn each, in a new conversation « Premiers pas », one at a time, its message in the Boîte
    def boite():
        return [i for i in bridge.get("/v1/outbox", headers=AUTH).json()["items"] if i["agent"] in ids]

    assert wait_until(lambda: len(boite()) == 4, timeout=60), boite()
    stop.set()
    sampler.join()
    assert max(running_at_once) == 1
    first = sorted(agent for agent, body in fakes.hermes.created if agent in ids and body["title"] == "Premiers pas")
    assert first == sorted(ids)
    runs = [r for r in fakes.hermes.started.values() if r["agent"] in ids]
    assert len(runs) == 4 and all(r["input"].startswith("<!--bip:team\n") and "propose_task" in r["input"] for r in runs)
    assert all(i["text"] == fakes.hermes.run_output for i in boite())
    # its first actions as cards: what the agent proposes lands in « Tâches de fond »
    give_token(bridge, "ventes", VENTES_TOKEN)
    proposed = call(bridge, "propose_task", {"title": "Lire la partie Ventes", "prompt": "Lis la partie ventes."},
                    token=VENTES_TOKEN)
    assert not proposed["isError"]
    jobs = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
    assert [(j["agent"], j["state"]) for j in jobs] == [("ventes", "proposed")]


def test_a_declined_team_is_not_proposed_again_and_only_the_guide_proposes(bridges, fakes):
    bridge, _ = place(bridges, fakes)
    assert "propose_team" not in tool_names(bridge, COACH_TOKEN)
    team = {"subject": "Le déménagement", "agents": [member("Cartons", 1), member("Papiers", 2)]}
    refused = call(bridge, "propose_team", team, token=COACH_TOKEN)
    assert refused["isError"]
    # checked whole: a name they have, a lone agent, a secret
    taken = call(bridge, "propose_team", {**team, "agents": [member("Coach", 1), member("Papiers", 2)]}, token=GUIDE_TOKEN)
    assert taken["isError"] and "already have an agent named Coach" in text_of(taken)
    alone = call(bridge, "propose_team", {**team, "agents": [member("Cartons", 1)]}, token=GUIDE_TOKEN)
    assert alone["isError"] and "2 to 6" in text_of(alone)
    secret = member("Cartons", 1)
    secret["brief"] = "Le code est sk-abcdefghijklmnopqrstuvwxyz0123456"
    leaked = call(bridge, "propose_team", {**team, "agents": [secret, member("Papiers", 2)]}, token=GUIDE_TOKEN)
    assert leaked["isError"] and "keys" in text_of(leaked)
    assert not call(bridge, "propose_team", team, token=GUIDE_TOKEN)["isError"]
    proposal = bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]
    assert bridge.post(f"/v1/guide/proposals/{proposal['id']}/decline", headers=AUTH).json() == {"ok": True}
    again = call(bridge, "propose_team", {**team, "agents": [member("Boites", 1), member("Banque", 2)]}, token=GUIDE_TOKEN)
    assert again["isError"] and "don't propose it again" in text_of(again)
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"] == []
    assert not fakes.hermes.started


def test_the_place_limit_is_shown_and_kept(bridges, fakes):
    bridge, _ = place(bridges, fakes, guide={"agents_max": 3})   # « Agent », the pre-created placeholder, does not count
    three = {"subject": "Trop", "agents": [member(n, i) for i, n in enumerate(NAMES[:3])]}
    over = call(bridge, "propose_team", three, token=GUIDE_TOKEN)
    assert over["isError"] and "only 2 more" in text_of(over), text_of(over)
    two = {"subject": "Juste", "agents": [member(n, i) for i, n in enumerate(NAMES[:2])]}
    made = call(bridge, "propose_team", two, token=GUIDE_TOKEN)
    assert not made["isError"], text_of(made)
    assert bridge.get("/v1/guide", headers=AUTH).json()["proposals"][0]["places_left"] == 2
