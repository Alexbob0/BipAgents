"""Asking another agent from any conversation (`message_agent` of the bridge's MCP, bridge-rs/src/agent_messages.rs).

Real-day bench 2026-10-28: Quotidien, linked to Coach with `PUT /v1/agent-links` (200), never had `message_agent`:
Hermes injects its own only in an agent's « Bot Chat ». The bridge's tool is offered once a link lets the agent write
to another one, kept out of Hermes' tool search, checks the link, asks the target in the two agents' own conversation
(not its Bot Chat any more, 2026-10-29) signed like a peer's message and brings its answer back in the same call."""
import hashlib
import json

import yaml

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered

MCP = "/v1/agent-tools/mcp"
TOKENS = {"quotidien": "quotidien-token-0123456789abcdefghij", "coach": "coach-token-0123456789abcdefghijklmn",
          "budget": "budget-token-0123456789abcdefghijklm"}


def give_tokens(bridge, names):
    path = bridge.state_dir / "agent-tools.json"
    tokens = json.loads(path.read_text())["tokens"] if path.exists() else {}
    for name in names:
        tokens[name] = hashlib.sha256(TOKENS[name].encode()).hexdigest()
    path.write_text(json.dumps({"tokens": tokens}, indent=1) + "\n")


def rpc(bridge, who, method, params=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    return bridge.post(MCP, json=body, headers={"Authorization": f"Bearer {TOKENS[who]}"}).json()


def names(bridge, who):
    return [t["name"] for t in rpc(bridge, who, "tools/list")["result"]["tools"]]


def ask(bridge, who, **arguments):
    result = rpc(bridge, who, "tools/call", {"name": "message_agent", "arguments": arguments})["result"]
    return result["content"][0]["text"], result["isError"]


def test_a_linked_agent_gets_message_agent_and_its_call_brings_the_answer(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    hub = managed_install(bridge, fakes, agents=("quotidien", "coach", "budget"))
    for name in ("quotidien", "coach", "budget"):
        (profile(hub, name) / "config.yaml").write_text("model:\n  provider: custom\n  base_url: http://gpu:8888/v1\n")
    bridge.start()
    wait_registered(bridge, ["quotidien", "coach", "budget"])
    give_tokens(bridge, TOKENS)
    assert "message_agent" not in names(bridge, "quotidien")       # it may write to nobody yet
    refused, failed = ask(bridge, "quotidien", target="bip/coach", message="Combien d'heures ?")
    assert failed and "hasn't let you write to Coach" in refused
    links = [{"from": "quotidien", "to": "coach"}]
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": links}).status_code == 200
    give_tokens(bridge, TOKENS)                                     # registered again (its list changed)
    assert "message_agent" in names(bridge, "quotidien")
    assert "message_agent" not in names(bridge, "coach") and "message_agent" not in names(bridge, "budget")
    # its Hermes was given the new list (a new tools URL) and keeps the tool out of the tool search
    config = yaml.safe_load((profile(hub, "quotidien") / "config.yaml").read_text())
    assert "!mcp__bipagents__message_agent" in config["tools"]["tool_search"]["defer"]
    assert wait_until(lambda: any(
        args[:2] == ["-p", "quotidien"] and args[4] == "mcp_servers.bipagents.url"
        for args in bridge.commands.of("hermes") if len(args) > 5), timeout=10)
    # the call: in Quotidien and Coach's own conversation (agent_pairs.rs, test_agent_pairs.py), signed as a peer's
    # message, its answer brought back
    fakes.hermes.run_output = "Tu as dormi 7 h 10 en moyenne cette semaine."
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="Combien d'heures a-t-il dormi cette semaine ?")
    assert not failed, text
    assert "Coach (bip/coach) answered:" in text and "7 h 10" in text
    created = [body for agent, body in fakes.hermes.created if agent == "coach"]
    assert len(created) == 1 and created[0]["title"].startswith("BipAgents · ⇄ p")
    runs = [r for r in fakes.hermes.started.values() if r["agent"] == "coach"]
    assert runs[-1]["input"].endswith(
        "Message from Quotidien (@quotidien): Combien d'heures a-t-il dormi cette semaine ?")
    assert not [c for c in fakes.hermes.chats if c["agent"] == "coach"]      # nothing in its Bot Chat
    # the same conversation again, by name too
    text, failed = ask(bridge, "quotidien", target="Coach", message="Et la semaine d'avant ?")
    assert not failed and len([b for a, b in fakes.hermes.created if a == "coach"]) == 1
    runs = [r for r in fakes.hermes.started.values() if r["agent"] == "coach"]
    assert runs[-1]["session_id"] == runs[-2]["session_id"]
    # the other way round stays closed
    refused, failed = ask(bridge, "coach", target="bip/quotidien", message="Salut")
    assert failed
    text, failed = ask(bridge, "quotidien", target="bip/coach__lea", message="Salut")
    assert failed and "household" in text
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="  ")
    assert failed
