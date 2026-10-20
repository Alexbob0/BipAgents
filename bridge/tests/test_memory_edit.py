import dataclasses

from bipbridge.config import AgentConfig, parse_config
from conftest import AUTH


def with_agent(client, config_dict, tmp_path):
    """An agent this install manages (served by the hub under /p/), its profile in a temporary HERMES_HOME."""
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(tmp_path / ".hermes"), "hub_url": "http://127.0.0.1:8642"}
    config = parse_config(config_dict)
    config.agents["coach"] = AgentConfig(name="coach", display_name="Coach", hermes_url="http://127.0.0.1:8642/p/coach",
                                         hermes_key="k")
    client.app.state.services.config = config
    return tmp_path / ".hermes" / "profiles" / "coach"


def test_the_person_reads_and_edits_what_the_agent_remembers(client, config_dict, tmp_path):
    folder = with_agent(client, config_dict, tmp_path)
    (folder / "memories").mkdir(parents=True)
    (folder / "memories" / "MEMORY.md").write_text("Alex court le dimanche.\n§\nLe chat s'appelle Pixel.")
    (folder / "config.yaml").write_text("memory:\n  user_char_limit: 40\n")
    seen = client.get("/v1/agents/coach/memory", headers=AUTH).json()
    assert seen["notes"]["entries"] == ["Alex court le dimanche.", "Le chat s'appelle Pixel."]
    assert seen["notes"]["limit"] == 2200 and seen["user"] == {"entries": [], "limit": 40, "used": 0}
    edited = client.put("/v1/agents/coach/memory/notes", json={"entries": ["Le chat s'appelle Pixel.", " ", "Né en 1985."]},
                        headers=AUTH).json()
    assert edited["entries"] == ["Le chat s'appelle Pixel.", "Né en 1985."]
    assert (folder / "memories" / "MEMORY.md").read_text() == "Le chat s'appelle Pixel.\n§\nNé en 1985."
    assert (folder / "memories" / "MEMORY.md.lock").exists()                  # Hermes' own lock, taken
    too_long = client.put("/v1/agents/coach/memory/user", json={"entries": ["x" * 41]}, headers=AUTH)
    assert too_long.status_code == 413
    sneaky = client.put("/v1/agents/coach/memory/user", json={"entries": ["a\n§\nb"]}, headers=AUTH).json()
    assert sneaky["entries"] == ["ab"]                                        # one entry stays one entry
    assert client.get("/v1/agents/nobody/memory", headers=AUTH).status_code == 404
    assert client.get("/v1/agents/coach/memory").status_code == 401


def test_knowledge_pages_are_listed_read_edited_and_removed_inside_their_folder(client, config_dict, tmp_path):
    folder = with_agent(client, config_dict, tmp_path)
    (folder / "knowledge" / "mail" / "people").mkdir(parents=True)
    (folder / "knowledge" / "mail" / "index.md").write_text("# Index\n- [Sam](people/sam.md)\n")
    (folder / "knowledge" / "mail" / "people" / "sam.md").write_text("# Sam\nVoisin.\n")
    (folder / "secret.md").write_text("outside")
    listed = client.get("/v1/agents/coach/memory", headers=AUTH).json()["pages"]
    assert [p["path"] for p in listed] == ["mail/index.md", "mail/people/sam.md"]
    assert client.get("/v1/agents/coach/memory/pages/mail/people/sam.md", headers=AUTH).json()["text"] == "# Sam\nVoisin.\n"
    client.put("/v1/agents/coach/memory/pages/mail/people/sam.md", json={"text": "# Sam\nVoisin, jardinier.\n"}, headers=AUTH)
    assert (folder / "knowledge" / "mail" / "people" / "sam.md").read_text() == "# Sam\nVoisin, jardinier.\n"
    assert client.get("/v1/agents/coach/memory/pages/..%2F..%2Fsecret.md", headers=AUTH).status_code == 404
    assert client.put("/v1/agents/coach/memory/pages/mail/run.sh", json={"text": "x"}, headers=AUTH).status_code == 404
    assert client.delete("/v1/agents/coach/memory/pages/mail/people/sam.md", headers=AUTH).json() == {"ok": True}
    assert not (folder / "knowledge" / "mail" / "people" / "sam.md").exists()
