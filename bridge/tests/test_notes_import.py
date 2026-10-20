import base64
import io
import json
import zipfile

from bipbridge.config import AgentConfig, parse_config
from conftest import AUTH


def with_agent(client, config_dict, tmp_path):
    config_dict["hermes"] = {"bin": "/opt/hermes", "home": str(tmp_path / ".hermes"), "hub_url": "http://127.0.0.1:8642"}
    config = parse_config(config_dict)
    config.agents["coach"] = AgentConfig(name="coach", display_name="Coach", hermes_url="http://127.0.0.1:8642/p/coach",
                                         hermes_key="k")
    client.app.state.services.config = config
    return tmp_path / ".hermes" / "profiles" / "coach" / "knowledge" / "notes"


def takeout():
    out = io.BytesIO()
    with zipfile.ZipFile(out, "w") as archive:
        archive.writestr("Takeout/Keep/Courses.json", json.dumps({
            "title": "Courses", "listContent": [{"text": "Lait", "isChecked": True}, {"text": "Pain", "isChecked": False}],
            "labels": [{"name": "Maison"}], "userEditedTimestampUsec": 1760000000000000, "isTrashed": False}))
        archive.writestr("Takeout/Keep/Courses.html", "<html><body>Courses</body></html>")   # the JSON wins
        archive.writestr("Takeout/Keep/Idee.json", json.dumps({"title": "", "textContent": "Offrir un vélo à Léa",
                                                               "isTrashed": False}))
        archive.writestr("Takeout/Keep/Vieux.json", json.dumps({"title": "Vieux", "textContent": "x", "isTrashed": True}))
        archive.writestr("Takeout/Keep/photo.jpg", b"\xff\xd8")
    return base64.b64encode(out.getvalue()).decode()


def test_keep_s_takeout_and_text_notes_become_knowledge_pages(client, config_dict, tmp_path):
    notes = with_agent(client, config_dict, tmp_path)
    done = client.post("/v1/agents/coach/memory/import", json={"name": "takeout-20261020.zip", "data": takeout()},
                       headers=AUTH).json()
    assert done == {"notes": 2, "skipped": 2}                          # the trashed note and the photo left out
    courses = (notes / "keep" / "courses.md").read_text()
    assert courses.startswith("# Courses\n") and "- [x] Lait" in courses and "- [ ] Pain" in courses
    assert "Étiquettes : Maison" in courses and "2025-10-09" in courses
    assert "Offrir un vélo à Léa" in (notes / "keep" / "offrir-un-velo-a-lea.md").read_text()
    text = base64.b64encode("Penser à rappeler le plombier.".encode()).decode()
    client.post("/v1/agents/coach/memory/import", json={"name": "Plombier.txt", "data": text, "source": "apple-notes"},
                headers=AUTH)
    index = (notes / "index.md").read_text()
    assert "## apple-notes (1)" in index and "## keep (2)" in index and "[Courses](keep/courses.md)" in index
    pages = [p["path"] for p in client.get("/v1/agents/coach/memory", headers=AUTH).json()["pages"]]
    assert "notes/apple-notes/plombier.md" in pages and "notes/index.md" in pages
    empty = base64.b64encode(b"").decode()
    assert client.post("/v1/agents/coach/memory/import", json={"name": "vide.txt", "data": empty},
                       headers=AUTH).status_code == 422
    assert client.post("/v1/agents/coach/memory/import", json={"name": "x.zip", "data": text},
                       headers=AUTH).status_code == 400
