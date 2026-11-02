"""The skills library (bridge-rs/src/skill_library.rs): a skill one agent wrote, shared with the person's other agents
as a copy in each profile's skills/; an agent's change is a new version only it moves to; the others update when
asked; versions compared and brought back; a skill with a key inside is never shared; removing takes the copies
back, the maker keeps its own. The agents' side is the `skill_library` MCP tool."""
from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered
from test_sandbox import TOKENS, give_tokens

MCP = "/v1/agent-tools/mcp"
SKILL = "---\nname: monthly-budget\ndescription: Sums the month's spending\n---\n# Steps\n1. Read the statements\n2. Sum\n"


def rpc(bridge, who, method, params=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    resp = bridge.post(MCP, json=body, headers={"Authorization": f"Bearer {TOKENS[who]}"})
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]


def names(bridge, who):
    return [t["name"] for t in rpc(bridge, who, "tools/list")["tools"]]


def tool(bridge, who, **arguments):
    result = rpc(bridge, who, "tools/call", {"name": "skill_library", "arguments": arguments})
    return result["content"][0]["text"], result["isError"]


def registrations(bridge, name):
    return sum(1 for args in bridge.commands.of("hermes")
               if len(args) > 4 and args[:2] == ["-p", name] and args[4].startswith("mcp_servers.bipagents.url"))


def reregistered(bridge, name, before):
    """Its tools given again (a new list): waited for, then the test's tokens given back."""
    assert wait_until(lambda: registrations(bridge, name) > before, timeout=15), bridge.commands.of("hermes")
    wait_registered(bridge, [name])
    give_tokens(bridge, ["budget", "vie", "wellness"])


def setup(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    hub = managed_install(bridge, fakes, agents=("budget", "vie", "wellness"))
    for name in ("budget", "vie", "wellness"):
        folder = profile(hub, name)
        (folder / "skills").mkdir(parents=True, exist_ok=True)
        (folder / "SOUL.md").write_text(f"Tu es {name}.\n")
    skill = profile(hub, "budget") / "skills" / "finance" / "monthly-budget"
    (skill / "scripts").mkdir(parents=True)
    (skill / "SKILL.md").write_text(SKILL)
    (skill / "scripts" / "sum.py").write_text("print(sum([1, 2]))\n")
    (skill / "scripts" / "sum.py").chmod(0o755)
    bridge.start()
    wait_registered(bridge, ["budget", "vie", "wellness"])
    give_tokens(bridge, ["budget", "vie", "wellness"])
    return bridge, hub


def test_a_skill_is_shared_versioned_compared_restored_and_removed(bridges, fakes):
    bridge, hub = setup(bridges, fakes)
    assert bridge.get("/v1/skill-library").status_code == 401
    listing = bridge.get("/v1/skill-library", headers=AUTH).json()
    assert listing["skills"] == [] and listing["shareable"] == [
        {"agent": "budget", "name": "monthly-budget", "description": "Sums the month's spending"}]
    assert "skill_library" in names(bridge, "budget") and "skill_library" not in names(bridge, "vie")

    # shared with vie only: a copy in its skills, its tools and personality follow
    before = registrations(bridge, "vie")
    resp = bridge.post("/v1/skill-library/share", headers=AUTH,
                       json={"agent": "budget", "name": "monthly-budget", "agents": ["vie"]})
    assert resp.status_code == 200, resp.text
    detail = resp.json()
    assert detail["shared"] is True and detail["made_by"] == "budget" and detail["latest"] == 1
    has = {a["agent"]: a["has"] for a in detail["agents"]}
    assert has == {"budget": True, "vie": True, "wellness": False}
    copy = profile(hub, "vie") / "skills" / "monthly-budget"
    assert (copy / "SKILL.md").read_text() == SKILL and (copy / "scripts" / "sum.py").stat().st_mode & 0o111
    assert not (profile(hub, "wellness") / "skills" / "monthly-budget").exists()
    assert wait_until(lambda: "skill_library propose" in (profile(hub, "vie") / "SOUL.md").read_text(), timeout=10)
    assert "skill_library" not in (profile(hub, "wellness") / "SOUL.md").read_text()
    # vie's own settings list it as a skill of the library
    skills = bridge.get("/v1/agents/vie/settings", headers=AUTH).json()["skills"]
    assert skills == [{"name": "monthly-budget", "description": "Sums the month's spending", "enabled": True,
                       "library": {"made_by": "budget"}}]
    reregistered(bridge, "vie", before)
    assert "skill_library" in names(bridge, "vie")
    text, error = tool(bridge, "vie", action="list")
    assert not error and "monthly-budget: version 1, from budget" in text

    # vie changes its copy (skill_manage), then says why: version 2, budget keeps version 1
    (copy / "SKILL.md").write_text(SKILL.replace("2. Sum", "2. Group by category\n3. Sum"))
    text, error = tool(bridge, "vie", action="propose", name="monthly-budget", why="par catégorie")
    assert not error and "Version 2" in text, text
    detail = bridge.get("/v1/skill-library/skills/monthly-budget", headers=AUTH).json()
    assert [(v["n"], v["agent"], v["why"]) for v in detail["versions"]] == [(2, "vie", "par catégorie"), (1, "budget", "")]
    budget = next(a for a in detail["agents"] if a["agent"] == "budget")
    assert budget["version"] == 1 and budget["behind"] is True
    original = profile(hub, "budget") / "skills" / "finance" / "monthly-budget" / "SKILL.md"
    assert original.read_text() == SKILL
    listing = bridge.get("/v1/skill-library", headers=AUTH).json()
    assert listing["skills"][0]["behind"] == 1 and listing["skills"][0]["used_by"] == 2

    diff = bridge.get("/v1/skill-library/skills/monthly-budget/compare?from=1&to=2", headers=AUTH).json()
    assert [f["path"] for f in diff["files"]] == ["SKILL.md"]
    assert {"op": "+", "text": "2. Group by category"} in diff["files"][0]["lines"]
    assert {"op": "-", "text": "2. Sum"} in diff["files"][0]["lines"]

    # « Mettre à jour » for budget, then the person goes back to version 1 for both
    resp = bridge.post("/v1/skill-library/skills/monthly-budget/update", headers=AUTH, json={"agent": "budget"})
    assert resp.status_code == 200 and "Group by category" in original.read_text()
    resp = bridge.post("/v1/skill-library/skills/monthly-budget/restore", headers=AUTH, json={"version": 1})
    assert resp.status_code == 200, resp.text
    assert resp.json()["versions"][0]["restored_from"] == 1
    assert original.read_text() == SKILL and (copy / "SKILL.md").read_text() == SKILL

    # every agent now, then removed: the copies go, the maker keeps its own
    resp = bridge.put("/v1/skill-library/skills/monthly-budget/agents", headers=AUTH, json={"all": True})
    assert resp.status_code == 200 and (profile(hub, "wellness") / "skills" / "monthly-budget" / "SKILL.md").is_file()
    assert bridge.delete("/v1/skill-library/skills/monthly-budget", headers=AUTH).status_code == 200
    assert not copy.exists() and not (profile(hub, "wellness") / "skills" / "monthly-budget").exists()
    assert original.read_text() == SKILL
    assert wait_until(lambda: "skill_library" not in (profile(hub, "vie") / "SOUL.md").read_text(), timeout=10)
    assert bridge.get("/v1/skill-library/skills/monthly-budget", headers=AUTH).status_code == 404


def test_a_skill_with_a_key_is_never_shared_and_an_agent_proposes_its_own(bridges, fakes):
    bridge, hub = setup(bridges, fakes)
    bank = profile(hub, "vie") / "skills" / "bank"
    bank.mkdir(parents=True)
    (bank / "SKILL.md").write_text("---\nname: bank\ndescription: Bank\n---\napi_key = sk-live-4f9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c\n")
    resp = bridge.post("/v1/skill-library/share", headers=AUTH, json={"agent": "vie", "name": "bank", "all": True})
    assert resp.status_code == 422 and resp.json()["detail"] == "secret"
    assert not (profile(hub, "budget") / "skills" / "bank").exists()
    give_tokens(bridge, ["budget", "vie", "wellness"])
    text, error = tool(bridge, "vie", action="propose", name="bank", why="x")
    assert error and "password" in text

    # its own clean skill: proposed, then the person shares it
    (bank / "SKILL.md").write_text("---\nname: bank\ndescription: Reads the bank's export\n---\n# Steps\n")
    text, error = tool(bridge, "vie", action="propose", name="bank", why="utile pour le budget")
    assert not error, text
    listing = bridge.get("/v1/skill-library", headers=AUTH).json()
    assert listing["proposed"][0]["name"] == "bank" and listing["proposed"][0]["why"] == "utile pour le budget"
    detail = bridge.get("/v1/skill-library/skills/bank?agent=vie", headers=AUTH).json()
    assert detail["shared"] is False and detail["why"] == "utile pour le budget"
    resp = bridge.post("/v1/skill-library/share", headers=AUTH, json={"agent": "vie", "name": "bank", "all": True})
    assert resp.status_code == 200
    assert resp.json()["versions"][0]["why"] == "utile pour le budget"
    assert bridge.get("/v1/skill-library", headers=AUTH).json()["proposed"] == []
    assert (profile(hub, "budget") / "skills" / "bank" / "SKILL.md").is_file()
