"""An agent of the shared hub moves into its own space by itself the first time it needs to keep a secret
(bridge-rs/src/own_space.rs): the secrets' and connectors' tools are offered to it (never to the family's agent nor a
project agent, never on an install that doesn't protect the vault), its first ask_secret moves it at once with no
card, keeping its conversations, memory and files, then raises the secret card as usual; a move that fails is one
plain answer, not retried at once."""
import json
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor

import yaml

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from conftest import managed_install, profile, wait_registered

TOKEN = ""
TOOLS = "/v1/agent-tools/connectors"
ASK = {"name": "garmin", "site": "connect.garmin.example", "kind": "password",
       "identifier_label": "E-mail de ton compte Garmin", "reason": "Pour tes séances"}


def hub_place(bridges, fakes, agents=("coach",), start=None, protected=True):
    bridge = bridges.new()
    bridge.config["sandbox"] = {"isolate": "on"}   # its tools in a container (the default on Linux)
    hub = managed_install(bridge, fakes, agents=agents, isolated=protected,
                          python=["/usr/bin/false"])
    (hub / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\n")
    (hub / ".env").write_text("CUSTOM_MODEL_API_KEY=sk-hub\n")
    for name in agents:
        folder = profile(hub, name)
        (folder / "SOUL.md").write_text(f"Tu es {name.title()}.\n")
        (folder / "config.yaml").write_text(yaml.safe_dump({"terminal": {"backend": "docker"}}))
        (folder / "memories").mkdir()
        (folder / "memories" / "MEMORY.md").write_text("Alex court le dimanche.")
    bridge.config["spaces"] = {"root": str(bridge.root / "spaces"), "first_port": free_port(),
                               "start": start or f"{bridge.fakebin}/fake-gateway {{home}}"}
    return bridge, hub


def use_token_of(hub):
    """The agent's token as its Hermes has it (its profile, copied as it is when it moves: the same token)."""
    global TOKEN
    TOKEN = entries(hub, "coach")["bipagents"]["headers"]["Authorization"].split()[1]


def tool(bridge, name, arguments):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
    resp = bridge.post(TOOLS, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=180)
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]


def text(result):
    return result["content"][0]["text"]


def secret_cards(bridge, agent="coach"):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-secret-")]


def entries(hub, name):
    path = hub / "profiles" / name / "config.yaml"
    return yaml.safe_load(path.read_text()).get("mcp_servers", {}) if path.exists() else {}


def test_a_hub_agent_asks_a_secret_moves_by_itself_then_gets_the_card(bridges, fakes):
    bridge, hub = hub_place(bridges, fakes)
    bridge.start()
    wait_registered(bridge, ["coach"])
    use_token_of(hub)
    # offered in the hub: the connectors' and the vault's account tools
    servers = entries(hub, "coach")
    assert "bipagents_connectors" in servers and "bipagents_vault" in servers
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(tool, bridge, "ask_secret", ASK)
        # no card for the move: the secret's card comes once it's done
        assert wait_until(lambda: secret_cards(bridge), timeout=60), bridge.log_tail(4000)
        assert not [i for i in bridge.get("/v1/approvals", headers=AUTH).json()["items"]
                    if not i["run_id"].startswith("bip-secret-")]
        root = bridge.root / "spaces" / "coach"
        section = tomllib.loads(bridge.toml_text())["agents"]["coach"]
        assert section["isolated"] is True and section["space"] == str(root)
        moved = root / "profiles" / "coach"
        assert (moved / "memories" / "MEMORY.md").read_text() == "Alex court le dimanche."
        assert (moved / "SOUL.md").read_text().startswith("Tu es Coach.")
        card = secret_cards(bridge)[0]
        assert card["description"] == "Coach a besoin de ton accès Garmin (connect.garmin.example)"
        sent = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
        assert sent.status_code == 200, sent.text
        result = call.result(timeout=60)
    assert not result["isError"] and "Saved as « garmin »" in text(result), text(result)
    assert "Fake-Pa55word" not in json.dumps(result)
    # the hub forgets it once no turn of it goes on there
    assert wait_until(lambda: any(e["argv"] == ["profile", "delete", "coach", "--yes"]
                                  for e in bridge.commands.entries("hermes")), timeout=20)
    vault = {a["id"]: a.get("why") for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert vault["coach"] is None
    # in its space now: the next secret is asked at once, nothing moves again
    with ThreadPoolExecutor(1) as pool:
        again = pool.submit(tool, bridge, "ask_secret", {**ASK, "name": "strava", "site": "strava.example"})
        assert wait_until(lambda: secret_cards(bridge) or again.done(), timeout=20)
        assert not again.done(), again.result()
        card = secret_cards(bridge)[0]
        assert bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": False}).status_code == 200
        assert again.result(timeout=30)["isError"]


def test_a_move_that_fails_is_one_plain_answer_and_not_retried_at_once(bridges, fakes):
    bridge, hub = hub_place(bridges, fakes, start="exit 3")
    bridge.start()
    wait_registered(bridge, ["coach"])
    use_token_of(hub)
    first = tool(bridge, "ask_secret", ASK)
    assert first["isError"] and "try again a bit later" in text(first) and "space" not in text(first)
    assert not secret_cards(bridge)
    assert not (bridge.root / "spaces" / "coach").exists() and (hub / "profiles" / "coach").is_dir()
    starts = len(bridge.commands.entries("hermes"))
    second = tool(bridge, "ask_secret", ASK)
    assert second["isError"] and text(second) == text(first)
    assert len(bridge.commands.entries("hermes")) == starts      # nothing tried again within two minutes
    # a call the tool refuses anyway moves nothing
    bad = tool(bridge, "ask_secret", {**ASK, "name": "G"})
    assert bad["isError"] and "name:" in text(bad)


def test_family_project_and_unprotected_agents_are_never_offered_the_tools(bridges, fakes):
    bridge, hub = hub_place(bridges, fakes, agents=("coach", "famille", "fete"))
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "family-agent.json").write_text(json.dumps({"agent": "famille", "created": 1.0, "lang": "fr"}))
    (bridge.state_dir / "project-agents.json").write_text(json.dumps({"projects": {"fete": {
        "created": time.time(), "ends": time.time() + 86400, "archived": None, "from": None, "lang": "fr"}}}))
    bridge.start()
    wait_registered(bridge, ["coach", "famille", "fete"])
    assert "bipagents_connectors" in entries(hub, "coach")
    for name in ("famille", "fete"):
        assert "bipagents_connectors" not in entries(hub, name), name


def test_an_install_that_does_not_protect_the_vault_offers_none(bridges, fakes):
    other, other_hub = hub_place(bridges, fakes, protected=False)
    other.start()
    wait_registered(other, ["coach"])
    servers = entries(other_hub, "coach")
    assert "bipagents_connectors" not in servers and "bipagents_vault" not in servers
