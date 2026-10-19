import asyncio
import json

import httpx
import pytest
import yaml

from test_agents_admin import managed  # noqa: F401  (fixture)


def skill(folder, name, description="Does a thing", frontmatter=True):
    folder.mkdir(parents=True, exist_ok=True)
    head = f"---\nname: {name}\ndescription: {description}\n---\n" if frontmatter else ""
    (folder / "SKILL.md").write_text(head + "# Steps\n", encoding="utf-8")


def make_profile(tmp_path):
    profile = tmp_path / "profiles" / "budget"
    skills = profile / "skills"
    skill(skills / "productivity" / "arxiv", "arxiv", "Search papers")          # shipped with Hermes
    skill(skills / "hub-thing", "hub-thing")                                      # from the hub
    skill(skills / "shared-data", "shared-data")                                  # the bridge's
    skill(skills / "finance" / "monthly-budget", "monthly-budget", "Sums  the\n month's spending")
    skill(skills / "plant-care", "plant-care", "Waters the ficus")
    skill(skills / "plant-care" / "templates" / "nested", "nested")              # a file inside a skill
    skill(skills / ".archive" / "old", "old")                                     # archived
    skill(skills / "no-front", "x", frontmatter=False)                            # name from its folder
    (skills / ".bundled_manifest").write_text("arxiv:abc123\n", encoding="utf-8")
    (skills / ".hub").mkdir()
    (skills / ".hub" / "lock.json").write_text(json.dumps({"installed": {"hub-thing": {"source": "hub"}}}))
    (profile / "config.yaml").write_text("model:\n  provider: custom\nskills:\n  disabled: [plant-care, arxiv]\n")
    return profile


def test_only_the_agents_own_skills_are_listed(tmp_path):
    from bipbridge.own_skills import listing

    found = listing(make_profile(tmp_path))
    assert [s["name"] for s in found] == ["monthly-budget", "no-front", "plant-care"]
    assert found[0] == {"name": "monthly-budget", "description": "Sums the month's spending", "enabled": True}
    assert found[2]["enabled"] is False
    assert listing(tmp_path / "nowhere") == []


def test_turning_skills_on_and_off_keeps_the_rest_of_the_config(tmp_path):
    from bipbridge.own_skills import listing, set_enabled

    profile = make_profile(tmp_path)
    set_enabled(profile, {"plant-care": True, "monthly-budget": False})
    data = yaml.safe_load((profile / "config.yaml").read_text())
    assert data["model"] == {"provider": "custom"}
    assert data["skills"]["disabled"] == ["arxiv", "monthly-budget"]   # Hermes' own choice stays
    assert {s["name"]: s["enabled"] for s in listing(profile)} == {"monthly-budget": False, "no-front": True,
                                                                     "plant-care": True}
    with pytest.raises(LookupError):
        set_enabled(profile, {"arxiv": True})   # not the agent's: not this switch's


def test_settings_show_and_change_the_agents_skills(managed):  # noqa: F811
    from bipbridge.agents_admin import AgentSettings, NewAgent, create_agent, read_settings, write_settings

    config, _, tmp = managed

    async def run(args, env):
        return ""

    def gateway(request):
        if request.url.path.endswith("/v1/toolsets"):
            return httpx.Response(200, json={"data": []})
        return httpx.Response(201 if request.method == "POST" else 200, json={})

    async def go():
        async with httpx.AsyncClient(transport=httpx.MockTransport(gateway)) as http:
            agent = await create_agent(config, NewAgent(name="Budget", kind="finance"), http, run=run)
            skill(tmp / ".hermes" / "profiles" / "budget" / "skills" / "receipts", "receipts", "Files receipts")
            before = await read_settings(config, agent, http)
            await write_settings(config, agent, AgentSettings(skills={"receipts": False}), run=run)
            after = await read_settings(config, agent, http)
            with pytest.raises(ValueError):
                await write_settings(config, agent, AgentSettings(skills={"../etc": False}), run=run)
            return before, after

    before, after = asyncio.run(go())
    assert before["skills"] == [{"name": "receipts", "description": "Files receipts", "enabled": True}]
    assert before["browser"] is None   # this install has no browser for its agents: Hermes' own tool shows
    assert after["skills"][0]["enabled"] is False
