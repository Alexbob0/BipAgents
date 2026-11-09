"""A project agent's thread shown to friends (read_shares.rs): its person's turns there are theirs (Alex, 2026-11-09,
« quand Alex demande, tout est permis », shared_pages.rs): pages and friends' cards are made, a page made there is
marked as the group's (no « Partager » in the apps), and `give_file` gives what the person asks for as in their own
conversations; what a project agent never has (mail…) stays closed by its own scope (project_agent.rs)."""
import time

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_project_agents import share_body
from test_read_shares import FakeGuichet

TOKEN = "project-token-0123456789abcdefghij"


def test_a_shared_project_thread_is_its_persons_with_pages_marked_for_the_group(bridges, fakes):
    from test_agents_admin import managed

    bridge, _ = managed(bridges, fakes, isolated=True)
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Pour les queens",
                                                                 "ends": time.time() + 10 * 86400, "lang": "fr"})
    assert made.status_code == 201, made.text
    agent = made.json()["agent"]
    give_token(bridge, agent, TOKEN)
    fakes.hermes.add_session("bot-1", [{"id": 1, "role": "user", "content": "Un week-end à Barcelone ?"}],
                             title="Bot Chat", agent=agent, started_at=100.0)

    # before the share: a map made (its picture in the agent's sandbox), so a file of the thread but not of the share
    before = call(bridge, "map_image", {"markers": [{"place": "41.4036, 2.1744", "label": "Sagrada Família"}]},
                  token=TOKEN)
    assert before["isError"] is False, text_of(before)
    old_picture = next(line.split("sandbox: ", 1)[1].split()[0] for line in text_of(before).splitlines()
                       if line.startswith("Picture in your sandbox: "))

    guichet = FakeGuichet().start()
    try:
        shared = bridge.post("/v1/read-shares", headers=AUTH, json=share_body(guichet, agent))
        assert shared.status_code == 200, shared.text

        # a page and a friends' card: made in the shared thread, the page marked as the group's
        page = call(bridge, "create_page", {"html": "<h1>Week-end à Barcelone</h1><p>Samedi : Sagrada.</p>",
                                            "title": "Week-end à Barcelone"}, token=TOKEN)
        assert page["isError"] is False, text_of(page)
        assert text_of(page).startswith(f"MEDIA:/bipagents/pages/{agent}/")
        listed = {p["page"]: p for p in bridge.get("/v1/pages", headers=AUTH, params={"agent": agent}).json()["pages"]}
        made = listed["week-end-a-barcelone"]
        assert made["group"] is True and "asked_by" not in made
        html = (bridge.state_dir / "agent-pages" / agent / "week-end-a-barcelone.html").read_text()
        assert '<meta name="bipagents-group" content="1">' in html and "bip-asked" not in html
        card = call(bridge, "friends_card", {"type": "vote", "title": "Où dormir ?",
                                             "options": ["Gràcia", "Born"]}, token=TOKEN)
        assert card["isError"] is False, text_of(card)
        # what a project agent never reaches stays closed: its own scope, not the share's
        closed = call(bridge, "mail_search", {"query": "banque"}, token=TOKEN)
        assert closed["isError"] is True

        # the person's turn gives the file they ask for, as in their own conversations, the map made before too
        uploads = next(p for p in bridge.state_dir.parent.rglob(old_picture.rsplit("/", 1)[1]) if p.is_file()).parent.parent
        (uploads / "2026-10").mkdir(exist_ok=True)
        (uploads / "2026-10" / "0123456789ab-programme.pdf").write_bytes(b"%PDF programme")
        personal = f"{old_picture.rsplit('/cartes/', 1)[0]}/2026-10/0123456789ab-programme.pdf"
        for path in (personal, old_picture):
            given = call(bridge, "give_file", {"path": path, "title": "Fichier"}, token=TOKEN)
            assert given["isError"] is False, (path, text_of(given))
            assert "MEDIA:" in text_of(given)
    finally:
        guichet.stop()
