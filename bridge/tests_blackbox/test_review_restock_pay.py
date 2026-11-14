"""Review of fix-restock-pay-loop (b975de3e7): its tests, kept (the « finding » ones failed on b975de3e7).

R*: pay_with_card / confirm_payment / cancel_payment in view (tool_diet.rs PAY_DIRECT).
D*: propose_task never proposes the same task twice (jobs.rs same_task).
Tests named *_finding_* failed on b975de3e7 (each one a finding, fixed since); the others were evidence."""
import json
import time

import pytest
import yaml

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_card_payments import CARD
from test_jobs import TASK, view, with_device
from test_vault_contacts import VIE, contacts_bridge
from conftest import wait_registered

PAY_TOOLS = ("pay_with_card", "confirm_payment", "cancel_payment")


def profile(space):
    return space / "profiles" / "vie"


def config_of(space):
    return yaml.safe_load((profile(space) / "config.yaml").read_text()) or {}


def defer(space):
    return ((config_of(space).get("tools") or {}).get("tool_search") or {}).get("defer") or []


def pay_entries(space):
    return [d for d in defer(space) if "bipagents_pay__" in d]


def pay_server(space):
    return (config_of(space).get("mcp_servers") or {}).get("bipagents_pay")


def pay_tools_list(bridge):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}
    resp = bridge.post("/v1/agent-tools/pay", json=body, headers={"Authorization": f"Bearer {VIE}"})
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]["tools"]


def visible_pay_tools(bridge, space):
    """What Hermes shows the model directly from the pay server (patch 0009: an MCP tool is deferred unless
    `!<name>` is in tools.tool_search.defer; a server the profile lacks shows nothing)."""
    if not isinstance(pay_server(space), dict):
        return []
    entries = set(defer(space))
    return [t["name"] for t in pay_tools_list(bridge) if f"!mcp__bipagents_pay__{t['name']}" in entries]


# --- R: visibility ----------------------------------------------------------------------------------------------------

def test_R2_the_lab_case_pay_with_card_is_in_the_restock_turns_visible_tools(bridges, fakes, tmp_path):
    """Evidence: an agent its person lets pay (card for it, payments on) sees pay_with_card directly: its profile has
    the bipagents_pay server AND the `!` entries, and the server lists the tool. Also: the schemas' size."""
    space = tmp_path / "spaces" / "vie"
    bridge = contacts_bridge(bridges, fakes, vie_space=space)
    wait_registered(bridge, ["vie"])
    assert wait_until(lambda: "!mcp__bipagents_vault__vault_create_login" in defer(space), timeout=15), defer(space)
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 200
    assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "agents": ["vie"]}).status_code == 201
    assert wait_until(lambda: sorted(visible_pay_tools(bridge, space)) == sorted(PAY_TOOLS), timeout=15), \
        (pay_server(space), pay_entries(space))
    tools = {t["name"]: t for t in pay_tools_list(bridge)}
    sizes = {n: len(json.dumps(tools[n], ensure_ascii=False)) for n in PAY_TOOLS}
    print(f"\n[review] pay tools in view, schema chars: {sizes}, total {sum(sizes.values())} "
          f"(~{sum(sizes.values()) // 4} tokens per model call)")
    # the defer list is stable while nothing changes (no toggle per turn: the prompt cache holds)
    before = defer(space)
    time.sleep(2)
    assert defer(space) == before


def test_R3_visibility_follows_the_card_and_the_switch(bridges, fakes, tmp_path):
    """Evidence: the card deleted, the card given to another agent only, payments resumed, another agent."""
    space = tmp_path / "spaces" / "vie"
    bridge = contacts_bridge(bridges, fakes, vie_space=space)
    wait_registered(bridge, ["vie"])
    assert wait_until(lambda: "!mcp__bipagents_vault__vault_create_login" in defer(space), timeout=15), defer(space)
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 200
    made = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "agents": ["vie"]})
    assert made.status_code == 201, made.text
    assert wait_until(lambda: len(pay_entries(space)) == 3, timeout=15), defer(space)
    # suspended, then resumed: back in view
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": False}).status_code == 200
    assert wait_until(lambda: pay_entries(space) == [], timeout=15), defer(space)
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 200
    assert wait_until(lambda: len(pay_entries(space)) == 3, timeout=15), defer(space)
    # the card taken from Vie (given to Coach only): behind the search
    moved = bridge.patch(f"/v1/vault/{made.json()['id']}", headers=AUTH, json={"agents": ["coach"]})
    assert moved.status_code == 200, moved.text
    assert wait_until(lambda: pay_entries(space) == [], timeout=15), defer(space)
    # given back, then deleted: behind the search
    back = bridge.patch(f"/v1/vault/{made.json()['id']}", headers=AUTH, json={"agents": ["vie"]})
    assert back.status_code == 200, back.text
    assert wait_until(lambda: len(pay_entries(space)) == 3, timeout=15), defer(space)
    assert bridge.delete(f"/v1/vault/{made.json()['id']}", headers=AUTH).status_code in (200, 204)
    assert wait_until(lambda: pay_entries(space) == [], timeout=15), defer(space)


@pytest.mark.parametrize("closed", ["family", "project"])
def test_R1_finding_a_closed_agent_never_has_the_payment_tools_in_view(bridges, fakes, tmp_path, closed):
    """The family's agent (its members write in its one conversation) and a project agent (friends read it) are
    « closed » (agents_admin::soul_text `open`): their SOUL.md has no payments block. Once in a space of their own
    (own_space::apart_when_idle) they may hold a card (vault.rs `holder`; `confirmed` skips cards), and payments on,
    card_pay::shopping says Card: PAY_DIRECT then puts pay_with_card in view for them, though their rules never speak
    of paying (tool_diet.rs says « the payments block of its SOUL.md says so »)."""
    space = tmp_path / "spaces" / "vie"
    bridge = bridges.new()
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    if closed == "family":
        (bridge.state_dir / "family-agent.json").write_text(json.dumps({"agent": "vie", "agents": ["vie"]}))
    else:
        (bridge.state_dir / "project-agents.json").write_text(json.dumps(
            {"projects": {"vie": {"created": time.time(), "ends": "2099-12-31", "archived": False, "lang": "fr"}}}))
    bridges_new = bridges.new
    bridges.new = lambda: bridge   # contacts_bridge builds on this very bridge (its state seeded above)
    try:
        bridge = contacts_bridge(bridges, fakes, vie_space=space)
    finally:
        bridges.new = bridges_new
    wait_registered(bridge, ["vie"])
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 200
    made = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "agents": ["vie"]})
    assert made.status_code == 201, made.text   # a closed agent in its own space may be given a card
    time.sleep(3)
    print(f"\n[review] {closed}: pay server registered: {isinstance(pay_server(space), dict)}, "
          f"entries: {pay_entries(space)}")
    assert visible_pay_tools(bridge, space) == [], \
        f"the {closed} agent (SOUL without payments) has the payment tools in view: {visible_pay_tools(bridge, space)}"


# --- D: the same task never proposed twice ------------------------------------------------------------------------------

def proposed(bridge):
    return [j for j in bridge.get("/v1/jobs", headers=AUTH).json()["jobs"] if j["state"] == "proposed"]


def test_D1_finding_a_long_task_proposed_again_is_proposed_twice(bridges, fakes):
    """propose_tool stores tk::head(prompt, 8000) but same_task compares the whole prompt: a task over 8000
    characters never matches the one already waiting."""
    bridge = with_device(bridges)
    give_token(bridge)
    long = "Prépare le dossier de location complet. " + "Détaille chaque pièce justificative. " * 260
    assert len(long) > 8000
    args = {"title": "Dossier location", "prompt": long, "budget": "small"}
    first = call(bridge, "propose_task", args)
    assert text_of(first).startswith("Proposed"), first
    again = call(bridge, "propose_task", args)
    assert text_of(again).startswith("Already proposed"), f"proposed twice: {text_of(again)[:80]}"
    assert len(proposed(bridge)) == 1


def test_D2_evidence_the_lab_loop_is_cut(bridges, fakes):
    """Evidence: the lab's exact call five times: one task, never isError."""
    bridge = with_device(bridges)
    give_token(bridge)
    args = {"title": "Supprimer la page", "prompt": "Delete the page with id paiement-de-la-place-stromae.",
            "budget": "small"}
    answers = [call(bridge, "propose_task", args) for _ in range(5)]
    assert text_of(answers[0]).startswith("Proposed")
    assert all(not a["isError"] and text_of(a).startswith("Already proposed") for a in answers[1:]), answers
    assert len(proposed(bridge)) == 1


@pytest.mark.slow
def test_D3_finding_a_paused_task_is_proposed_again(bridges, fakes):
    """A task the person paused (or that reached its budget) is still in « Tâches de fond », waiting for their
    « Reprendre »: the agent proposing the very same words makes a second task (and a push) next to it."""
    bridge = with_device(bridges)
    give_token(bridge)
    fakes.hermes.rounds, fakes.hermes.step = 1000, 10
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json={**TASK, "title": "Long", "prompt": TASK["prompt"]}).json()
    assert bridge.post(f"/v1/jobs/{job['id']}/pause", headers=AUTH).status_code == 200
    assert wait_until(lambda: view(bridge, job["id"])["state"] == "paused", timeout=25)
    again = call(bridge, "propose_task", {"title": "Écoles", "prompt": TASK["prompt"], "budget": "small"})
    assert text_of(again).startswith("Already proposed"), f"a second task next to the paused one: {text_of(again)[:90]}"


def test_D4_note_a_trailing_full_stop_makes_another_task(bridges, fakes):
    """Not counted as a finding: « slightly reworded » (here only a final full stop) is a new task."""
    bridge = with_device(bridges)
    give_token(bridge)
    args = {"title": "Supprimer la page", "prompt": "Delete the page with id paiement-de-la-place", "budget": "small"}
    assert text_of(call(bridge, "propose_task", args)).startswith("Proposed")
    second = call(bridge, "propose_task", {**args, "prompt": args["prompt"] + "."})
    print(f"\n[review] with a final full stop: {text_of(second)[:60]}")
