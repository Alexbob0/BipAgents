"""A project agent's thread shown to friends (read_shares.rs, Alex 5/11 « Oui, sauf fichiers »): once shared, it still
makes pages and friends' cards (their content comes from the shared conversation itself, the person's own data stays
closed), and `give_file` gives only a file it made in that shared conversation — never one of the person's files, nor
one made elsewhere, even named by its exact path."""
import time

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_project_agents import share_body
from test_read_shares import FakeGuichet

TOKEN = "project-token-0123456789abcdefghij"


def test_a_shared_project_thread_makes_pages_and_cards_and_gives_only_its_own_files(bridges, fakes):
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

        # a page and a friends' card: made in the shared thread
        page = call(bridge, "create_page", {"html": "<h1>Week-end à Barcelone</h1><p>Samedi : Sagrada.</p>",
                                            "title": "Week-end à Barcelone"}, token=TOKEN)
        assert page["isError"] is False, text_of(page)
        assert text_of(page).startswith(f"MEDIA:/bipagents/pages/{agent}/")
        card = call(bridge, "friends_card", {"type": "vote", "title": "Où dormir ?",
                                             "options": ["Gràcia", "Born"]}, token=TOKEN)
        assert card["isError"] is False, text_of(card)
        # the person's own data stays closed, said plainly
        closed = call(bridge, "mail_search", {"query": "banque"}, token=TOKEN)
        assert closed["isError"] is True

        # a map made here: its picture may be given in the shared conversation
        here = call(bridge, "map_image", {"markers": [{"place": "41.3809, 2.1228", "label": "Camp Nou"},
                                                      {"place": "41.4145, 2.1527", "label": "Park Güell"}]},
                    token=TOKEN)
        assert here["isError"] is False, text_of(here)
        picture = next(line.split("sandbox: ", 1)[1].split()[0] for line in text_of(here).splitlines()
                       if line.startswith("Picture in your sandbox: "))
        given = call(bridge, "give_file", {"path": picture, "title": "Carte"}, token=TOKEN)
        assert given["isError"] is False, text_of(given)
        assert "MEDIA:" in text_of(given)
        assert "not a file you made in this shared conversation" not in text_of(given)

        # the person's own file in the agent's folder (sent to it before), and the map made before the share: refused
        uploads = next(p for p in bridge.state_dir.parent.rglob(picture.rsplit("/", 1)[1]) if p.is_file()).parent.parent
        (uploads / "2026-10").mkdir(exist_ok=True)
        (uploads / "2026-10" / "0123456789ab-releve.pdf").write_bytes(b"%PDF releve")
        personal = f"{picture.rsplit('/cartes/', 1)[0]}/2026-10/0123456789ab-releve.pdf"
        for path in (personal, old_picture, "/envoi/devis.pdf", "MEDIA:" + personal):
            refused = call(bridge, "give_file", {"path": path}, token=TOKEN)
            assert refused["isError"] is True, (path, text_of(refused))
            assert "not a file you made in this shared conversation" in text_of(refused), text_of(refused)
            assert "MEDIA:" not in text_of(refused)
        assert "give_file refused in a shared conversation" in bridge.log_tail(8000)
    finally:
        guichet.stop()
