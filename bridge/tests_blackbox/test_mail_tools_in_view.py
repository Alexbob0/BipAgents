"""An agent's mail tools when it is given a mailbox (5/11, Alex's new « Admin Kerfy »: a custom agent with his Proton
box through hydroxide). Its mail tools were all behind Hermes' tool search; GLM's calls, their names constrained to the
tools in view, ended on map_image ({"id": "<box>|INBOX|12"}), knowledge_search ({"sender": …}) or notebook_search,
then waited out the brake; its background triage never reached the box. And every word search came back empty:
hydroxide's SEARCH TEXT matches nothing. So:

- an agent given a mailbox keeps mail_overview, mail_search, mail_read, mail_bulk and mail_draft out of the tool search
  (its profile's `tools.tool_search.defer`), from the moment the mailbox is added or given to it, until it has none;
- a mail's id under another tool's name reads that mail; a wrong id says what an id looks like;
- a word search that TEXT can't answer finds the words in the subject."""
import yaml

from bbkit.common import AUTH
from bbkit.mcp import call, text_of
from test_mailbox import TOKEN, add_mailbox, mail_bridge

IN_VIEW = ["mail_overview", "mail_search", "mail_read", "mail_bulk", "mail_draft"]


def in_view(folder):
    defer = yaml.safe_load((folder / "config.yaml").read_text())["tools"]["tool_search"]["defer"]
    return {name for name in IN_VIEW + ["mail_attachment"] if f"!mcp__bipagents__{name}" in defer}


def test_an_agent_given_a_mailbox_keeps_its_mail_tools_in_view_until_it_has_none(bridges, fakes):
    fakes.imap.as_hydroxide(own=["Factures"])
    bridge, folder = mail_bridge(bridges, fakes)
    assert in_view(folder) == set()                                   # no mailbox: behind the search, as before
    added = add_mailbox(bridge, fakes)
    assert added.status_code == 201, added.text
    assert in_view(folder) == set(IN_VIEW)                            # mail_attachment stays behind the search
    # taken from it, given back, the mailbox removed
    account = added.json()["id"]
    assert bridge.patch(f"/v1/mail/accounts/{account}", json={"agents": []}, headers=AUTH).status_code == 200
    assert in_view(folder) == set()
    assert bridge.patch(f"/v1/mail/accounts/{account}", json={"agents": ["Coach"]}, headers=AUTH).status_code == 200
    assert in_view(folder) == set(IN_VIEW)
    assert bridge.delete(f"/v1/mail/accounts/{account}", headers=AUTH).status_code == 200
    assert in_view(folder) == set()


def test_mail_calls_that_ended_on_another_tools_name_reach_the_mailbox(bridges, fakes):
    fakes.imap.as_hydroxide(own=["Factures"])
    bridge, _ = mail_bridge(bridges, fakes)
    account = add_mailbox(bridge, fakes).json()["id"]
    # 5/11's calls: map_image with mail_read's id, knowledge_search with mail_search's sender
    read = call(bridge, "map_image", {"id": f"{account}|INBOX|102"}, token=TOKEN)
    assert not read.get("isError"), read
    assert "Subject: Repas dimanche" in text_of(read) and "Apporte le dessert" in text_of(read)
    found = call(bridge, "knowledge_search", {"sender": "sam@example.com"}, token=TOKEN)
    assert not found.get("isError") and f"{account}|INBOX|102" in text_of(found), found
    # an id map_image can't take and that is no mail's: no map, and what was meant
    refused = call(bridge, "map_image", {"id": "12"}, token=TOKEN)
    assert refused["isError"] and "only draws maps" in text_of(refused), refused
    # a wrong id: its shape, with this mailbox's own example
    wrong = call(bridge, "mail_read", {"id": "102"}, token=TOKEN)
    assert wrong["isError"] and f"{account}|INBOX|12" in text_of(wrong) and "« 102 »" in text_of(wrong), wrong
    # with mail_bulk in view, the lab's agent filed one sender per call until the brake: told to group them
    one = text_of(call(bridge, "mail_bulk", {"action": "archive", "domain": "shop.example"}, token=TOKEN))
    assert one.startswith("Card made") and "all of them in groups" in one, one
    grouped = text_of(call(bridge, "mail_bulk", {"action": "archive", "groups": ["domain:example.com"]}, token=TOKEN))
    assert grouped.startswith("Added to the card already waiting") and "all of them in groups" not in grouped, grouped


def test_a_word_search_hydroxide_cannot_answer_finds_the_words_in_the_subject(bridges, fakes):
    fakes.imap.as_hydroxide(own=["Factures"])
    bridge, _ = mail_bridge(bridges, fakes)
    account = add_mailbox(bridge, fakes).json()["id"]
    found = call(bridge, "mail_search", {"query": "dimanche"}, token=TOKEN)
    assert not found.get("isError") and f"{account}|INBOX|102" in text_of(found), found
    assert any("TEXT" in c for c in fakes.imap.commands) and any("SUBJECT" in c for c in fakes.imap.commands)
    # words in no subject: the sender's name is tried too, then nothing
    assert f"{account}|INBOX|103" in text_of(call(bridge, "mail_search", {"query": "shop"}, token=TOKEN))
    assert text_of(call(bridge, "mail_search", {"query": "introuvable"}, token=TOKEN)) == "No e-mail found."
