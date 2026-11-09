"""« Dépenses partagées » (shared_expenses.rs, 9/11): a project agent shared with friends notes who paid what with its
`expense` tool; the bridge keeps the account and does the sums (balances, the fewest transfers, to the cent). The
friends following the shared thread get it as a `card` item sealed with the share's key, ready to draw, and can't tap
it; the owner deletes an expense from their app (the friends' copy follows, the agent hears of it once)."""
import json
import time

from bbkit.common import AUTH
from bbkit.fakes import taken_notes
from bbkit.mcp import call, give_token, rpc, text_of
from test_friends_cards import SHARE, tap
from test_project_agents import share_body
from test_read_shares import FakeGuichet, Phone, open_item

TOKEN = "project-token-expenses-0123456789ab"


def test_the_bridge_keeps_the_account_and_the_friends_read_it(bridges, fakes):
    from test_agents_admin import managed

    bridge, _ = managed(bridges, fakes, isolated=True)
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Week-end à Lisbonne",
                                                                 "ends": time.time() + 10 * 86400, "lang": "fr"})
    assert made.status_code == 201, made.text
    agent = made.json()["agent"]
    give_token(bridge, agent, TOKEN)

    # the tool is listed for a project agent, with its short description
    tools = {t["name"]: t for t in rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]}
    assert "expense" in tools and len(tools["expense"]["description"]) < 260

    # before the share, nobody else is known: the agent names who shares it
    first = call(bridge, "expense", {"paid_by": "Léa", "amount": 90, "what": "Dîner", "for": ["Léa", "Sam", "Alex"]},
                 token=TOKEN)
    assert first["isError"] is False, first
    text = text_of(first)
    assert "To settle: Alex → Léa 30,00 €; Sam → Léa 30,00 €" in text, text
    line = next(l for l in text.splitlines() if l.startswith("MEDIA:"))
    card = line.rsplit("/", 1)[1]
    taxi = call(bridge, "expense", {"paid_by": "Sam", "amount": "31", "what": "Taxi", "for": ["Sam", "Léa"]}, token=TOKEN)
    assert taxi["isError"] is False and line in text_of(taxi), taxi
    assert call(bridge, "expense", {"paid_by": "Sam", "amount": 5, "what": "Café", "currency": "USD"},
                token=TOKEN)["isError"] is True
    fakes.hermes.add_session("bot-1", [
        {"id": 1, "role": "user", "content": "Sam a payé le taxi, on en est où ?"},
        {"id": 2, "role": "assistant", "content": f"Voici les comptes :\n{line}"},
    ], title="Bot Chat", agent=agent, started_at=100.0)

    guichet = FakeGuichet().start()
    try:
        shared = bridge.post("/v1/read-shares", headers=AUTH, json=share_body(guichet, agent))
        assert shared.status_code == 200, shared.text
        link = bridge.post(f"/v1/read-shares/{SHARE}/invites", headers=AUTH, json={"name": "Léa Martin"}).json()
        lea = Phone(link["link"])
        guichet.redeem(lea.invite, lea.public, lea.proof(), "1")

        def sync():
            done = bridge.post(f"/v1/read-shares/{SHARE}/sync", headers=AUTH)
            assert done.status_code == 200, done.text

        sync()
        key = lea.unwrap(next(v for v in guichet.viewers if v["viewer"] == "1")["wrapped"], SHARE, 1)

        def item():
            return open_item(key, guichet.items[f"c-{card}"]["data"])

        # the friends' copy: linked from the message, the account ready to draw
        message = open_item(key, guichet.items["m-2"]["data"])
        assert message["media"] == [f"c-{card}"] and "MEDIA:" not in message["text"]
        account = item()
        assert account["kind"] == "card" and account["type"] == "expenses" and account["open"] is True
        assert account["total"] == "121,00 €"
        assert [e["what"] for e in account["expenses"]] == ["Taxi", "Dîner"]
        assert sum(b["cents"] for b in account["balances"]) == 0
        assert account["transfers"] == [{"from": "Alex", "to": "Léa", "amount": "30,00 €"},
                                        {"from": "Sam", "to": "Léa", "amount": "14,50 €"}]

        # a friend can't tap it (dropped, taken)
        before = item()
        tap(guichet, lea, "1", key, card, "e1", kind="tick")
        sync()
        assert item() == before and guichet.actions == []

        # the owner deletes the taxi in their app: the friends' copy follows, the agent hears of it once
        gone = bridge.delete(f"/v1/project-agents/{agent}/cards/{card}/expenses/e2", headers=AUTH)
        assert gone.status_code == 200, gone.text
        assert [e["id"] for e in gone.json()["expenses"]] == ["e1"]
        sync()
        assert item()["total"] == "90,00 €"
        recap = taken_notes(bridge, agent)
        assert "- Alex a supprimé la dépense e2 « Taxi » (31,00 €)" in recap, recap
        assert "supprimé" not in taken_notes(bridge, agent)

        # the project ended: nothing changes any more
        path = bridge.state_dir / "project-agents.json"
        data = json.loads(path.read_text())
        data["projects"][agent]["ends"] = time.time() - 1
        path.write_text(json.dumps(data))
        assert bridge.delete(f"/v1/project-agents/{agent}/cards/{card}/expenses/e1",
                             headers=AUTH).status_code in (409, 404)
        assert call(bridge, "expense", {"paid_by": "Sam", "amount": 5, "what": "Café"}, token=TOKEN)["isError"] is True
    finally:
        guichet.stop()
