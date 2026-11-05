"""Mail moves through hydroxide (Proton without Proton Mail Bridge; bug of 2026-11-04: Alex's « Oui » to moves into
« Copro St Ex », a folder of his Proton, ended « the mailbox refused part of it », 0 moved, and his 4 rules filed
nothing). hydroxide lists only the Proton folders its API answer gave it at login, makes none (CREATE « not yet
implemented ») and answers « No such mailbox » to a MOVE anywhere else (measured on its own code,
internal/notes/hydroxide-moves.md). So: a move or a rule to a folder it doesn't show is refused before any card, in
plain words with the folders it can use; one it shows is used under its listed name; a refusal keeps the server's
answer for the agent (mail_bulk status) and the rule's error."""
from bbkit.common import AUTH
from bbkit.mcp import call, text_of
from bbkit.servers import wait_until
from test_mail_cards import answer, mail_cards
from test_mailbox import TOKEN, add_mailbox, mail_bridge
from test_triggers import mail


def syndic(imap):
    for uid in (130, 131, 132):
        imap.add("INBOX", uid, mail(uid, "Syndic <syndic@copro.example>", f"AG {uid}", "Convocation", f"<c{uid}@copro.example>"))


def test_a_folder_hydroxide_does_not_show_is_refused_in_plain_words_before_any_card(bridges, fakes):
    fakes.imap.as_hydroxide()
    bridge, _ = mail_bridge(bridges, fakes)
    syndic(fakes.imap)
    assert add_mailbox(bridge, fakes).status_code == 201
    for args in ({"action": "move:Copro St Ex", "domain": "copro.example"},
                 {"action": "move:Copro St Ex", "domain": "copro.example", "rule": True}):
        result = call(bridge, "mail_bulk", args, token=TOKEN)
        said = text_of(result)
        assert result["isError"], result
        assert "hydroxide" in said and "« Archive »" in said and "« All Mail »" not in said, said
        assert "Proton Mail Bridge" in said and "refused part" not in said, said
    assert mail_cards(bridge) == []
    assert not any(c.startswith(("UID MOVE", "UID COPY", "CREATE")) for c in fakes.imap.commands), fakes.imap.commands
    # Archive, which hydroxide has: a card as usual, moved after « Oui »
    made = call(bridge, "mail_bulk", {"action": "archive", "domain": "copro.example"}, token=TOKEN)
    assert not made.get("isError"), made
    [card] = mail_cards(bridge)
    assert answer(bridge, card).status_code == 200
    assert wait_until(lambda: len(fakes.imap.folders["Archive"]) == 3, timeout=20), fakes.imap.commands


def test_a_folder_hydroxide_shows_is_filed_into_by_cards_and_rules(bridges, fakes):
    fakes.imap.as_hydroxide(own=["Copro St Ex"])
    bridge, _ = mail_bridge(bridges, fakes)
    syndic(fakes.imap)
    assert add_mailbox(bridge, fakes).status_code == 201
    made = call(bridge, "mail_bulk", {"action": "move:copro st ex", "domain": "copro.example"}, token=TOKEN)
    assert not made.get("isError"), made
    [card] = mail_cards(bridge)
    assert answer(bridge, card).status_code == 200
    assert wait_until(lambda: len(fakes.imap.folders["Copro St Ex"]) == 3, timeout=20), fakes.imap.commands
    assert any(c.startswith("UID MOVE") and '"Copro St Ex"' in c for c in fakes.imap.commands)
    assert not any(c.startswith("CREATE") for c in fakes.imap.commands)

    # a rule: on « Oui », new mail filed by itself
    rule = call(bridge, "mail_bulk", {"action": "move:Copro St Ex", "domain": "copro.example", "rule": True}, token=TOKEN)
    assert not rule.get("isError"), rule
    [rule_card] = [c for c in bridge.get("/v1/approvals", headers=AUTH).json()["items"]
                   if c["run_id"].startswith("bip-mailrule-")]
    assert bridge.post("/v1/approve", headers=AUTH, json={"agent": "coach", "run_id": rule_card["run_id"],
                                                          "choice": "once"}).status_code == 200
    assert wait_until(lambda: fakes.imap.idling >= 1, timeout=20)
    fakes.imap.add("INBOX", 140, mail(140, "Syndic <syndic@copro.example>", "Travaux", "x", "<c140@copro.example>"))
    assert wait_until(lambda: "<c140@copro.example>" in fakes.imap.message_ids("Copro St Ex"), timeout=20)


def test_a_refused_move_tells_the_agent_the_servers_answer(bridges, fakes):
    fakes.imap.as_hydroxide(own=["Copro St Ex"])
    bridge, _ = mail_bridge(bridges, fakes)
    syndic(fakes.imap)
    assert add_mailbox(bridge, fakes).status_code == 201
    made = call(bridge, "mail_bulk", {"action": "move:Copro St Ex", "domain": "copro.example"}, token=TOKEN)
    assert not made.get("isError"), made
    [card] = mail_cards(bridge)
    # hydroxide restarted meanwhile and no longer sees the folder (the Proton side changed)
    fakes.imap.hydroxide_own = []
    assert answer(bridge, card).status_code == 200

    def status():
        return text_of(call(bridge, "mail_bulk", {"action": "status"}, token=TOKEN))
    assert wait_until(lambda: "failed" in status(), timeout=20), status()
    said = status()
    assert "hydroxide" in said and "« NO not yet implemented »" in said, said
    assert len(fakes.imap.folders["INBOX"]) >= 3 and fakes.imap.folders["Copro St Ex"] == []
