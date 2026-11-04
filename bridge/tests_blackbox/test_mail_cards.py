"""The bridge's own mail cards (bridge-rs/src/mail_tidy.rs `mail_bulk`, card_place.rs) answered the way the apps answer
them: `GET /v1/approvals` lists the card (the Boîte), the conversation whose turn made it shows it in its `pending`
(a background task's: the conversation it reports to as well), `POST /v1/approve` with its `run_id` does it or refuses
it, and the card is gone everywhere at once: the Boîte, the conversations, its notification. One decision is one card:
the same action on the same folder asked again while a card waits joins it. Bug of 2026-11-04 (Alex, agent Vie in its
own space): ~15 cards for one decision, each « Oui » answered « Introuvable », the cards never left."""
import json
import threading
import time

import pytest

from bbkit.common import AUTH, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered
from test_mailbox import PASSWORD, TOKEN, add_mailbox, mail_bridge, mails


def mail_cards(bridge, agent="coach"):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [c for c in items if c["run_id"].startswith("bip-mail-")]


def answer(bridge, card, choice="once", agent="coach"):
    return bridge.post("/v1/approve", headers=AUTH, json={"agent": agent, "run_id": card["run_id"], "choice": choice,
                                                          "request_id": card["request_id"]})


def card_rows(bridge):
    return [r for r in bridge.get("/v1/notifications", headers=AUTH).json()["notifications"] if r["type"] == "mail_card"]


def pending(bridge, thread, agent="coach"):
    page = bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": agent}).json()
    return page, [a for a in page["pending"]["approvals"] or [] if a["run_id"].startswith("bip-mail-")]


def thread_of(bridge, session, agent="coach"):
    found = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": agent, "session_id": session})
    assert found.status_code == 200, found.text
    return found.json()["thread"]["id"]


def own_space_bridge(bridges, fakes):
    """`coach` already in a space of its own (agents_admin.rs `move_into_space`'s config: its own Hermes, its tools in
    a sandbox): what Vie is on Alex's machine."""
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",), isolated=True)
    root = bridge.root / "spaces" / "coach"
    moved = profile(root, "coach")
    bridge.config["agents"]["coach"].update(space=str(root), isolated=True, sandboxed=True, hermes_home=str(moved))
    assert home.is_dir()
    fakes.imap.users["alex@proton.me"] = PASSWORD
    mails(fakes.imap)
    bridge.start()
    give_token(bridge, "coach", TOKEN)
    return bridge


@pytest.mark.parametrize("space", [False, True], ids=["hub", "own_space"])
def test_one_card_per_decision_answered_from_the_boite_is_gone_everywhere(bridges, fakes, space):
    bridge = own_space_bridge(bridges, fakes) if space else mail_bridge(bridges, fakes)[0]
    assert add_mailbox(bridge, fakes).status_code == 201
    # one call per sender, as Vie did: one card, its count and its senders growing
    made = call(bridge, "mail_bulk", {"action": "move:Tri", "domain": "banque.example"}, token=TOKEN)
    assert not made.get("isError") and text_of(made).startswith("Card made"), made
    joined = call(bridge, "mail_bulk", {"action": "move:Tri", "groups": ["domain:shop.example"]}, token=TOKEN)
    assert not joined.get("isError") and text_of(joined).startswith("Added to the card already waiting"), joined
    again = call(bridge, "mail_bulk", {"action": "move:Tri", "domain": "banque.example"}, token=TOKEN)
    assert not again.get("isError"), again   # the same sender again: counted once
    [card] = mail_cards(bridge)
    assert card["bip_mail"]["count"] == 2 and card["agent"] == "coach"
    assert card["bip_summary"] == "Coach wants to move 2 e-mails to “Tri”: 1 from banque.example, 1 from shop.example"
    assert "Relevé d'octobre" in card["bip_detail"] and "-50 %" in card["bip_detail"]
    # another decision: its own card, in plain words (« 1 e-mail », the sender in brackets)
    call(bridge, "mail_bulk", {"action": "archive", "from": "sam@example.com"}, token=TOKEN)
    other = [c for c in mail_cards(bridge) if c["run_id"] != card["run_id"]][0]
    assert other["bip_summary"] == "Coach wants to archive 1 e-mail (from example.com)"
    # one notification per card (the join is not notified again), its `ref` the card's run id
    assert wait_until(lambda: len(card_rows(bridge)) == 2), card_rows(bridge)
    assert {r["ref"] for r in card_rows(bridge)} == {card["run_id"], other["run_id"]}

    # « Oui » from the Boîte: done, gone from the list and its notification read; answered again: not found
    ok = answer(bridge, card)
    assert ok.status_code == 200 and ok.json()["count"] == 2, ok.text
    assert [c["run_id"] for c in mail_cards(bridge)] == [other["run_id"]]
    row = [r for r in card_rows(bridge) if r["ref"] == card["run_id"]][0]
    assert row["read_at"] and row["state"] == "answered"
    assert answer(bridge, card).status_code == 404
    assert wait_until(lambda: len(fakes.imap.message_ids("Tri")) == 2, timeout=20), fakes.imap.commands
    # « Non »: gone too, nothing moved
    assert answer(bridge, other, "deny").status_code == 200
    assert mail_cards(bridge) == [] and "<s1@example.com>" in fakes.imap.message_ids("INBOX")


def watched_run(bridge, fakes, run_id, session, agent="coach"):
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("tool.started", run_id=run_id, tool="mcp_bipagents_mail_bulk", preview="x"), gate,
                                 sse("run.completed", run_id=run_id, output="C'est prêt, à toi de valider.")]
    watched = bridge.post("/v1/watch", headers=AUTH, json={"agent": agent, "run_id": run_id, "session_id": session})
    assert watched.status_code == 200
    assert wait_until(lambda: fakes.hermes.seen(f"/p/{agent}/v1/runs/{run_id}/events"))
    time.sleep(0.3)
    return gate


def test_a_card_shows_in_the_conversation_that_asked_and_answered_there_leaves_the_boite(bridges, fakes):
    bridge, _ = mail_bridge(bridges, fakes)
    assert add_mailbox(bridge, fakes).status_code == 201
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "Range mes mails de la banque"}], title="Courrier")
    gate = watched_run(bridge, fakes, "run_c", "api_c")
    made = call(bridge, "mail_bulk", {"action": "move:Banque", "domain": "banque.example"}, token=TOKEN)
    assert not made.get("isError"), made
    gate.set()
    [card] = mail_cards(bridge)
    assert card["session_id"] == "api_c"            # the Boîte's row opens the conversation
    thread = thread_of(bridge, "api_c")
    page, [inline] = pending(bridge, thread)
    assert inline["run_id"] == card["run_id"] and inline["bip_summary"] == card["bip_summary"]
    assert inline["id"].startswith("a_")             # the answered item's id: one row on every device
    # not in another conversation
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "coach"}).json()["id"]
    assert main == thread or pending(bridge, main)[1] == []
    before = page["version"]
    # answered in the conversation: gone from it and from the Boîte, the decision one of its items
    assert answer(bridge, inline).status_code == 200
    assert mail_cards(bridge) == []
    page, waiting = pending(bridge, thread)
    assert waiting == [] and page["version"] > before
    [item] = [i for i in page["items"] if i["kind"] == "approval"]
    assert item["id"] == inline["id"] and item["approval"]["choice"] == "once"
    assert item["approval"]["run_id"] == card["run_id"]


@pytest.mark.slow
def test_a_background_tasks_card_shows_where_the_task_reports(bridges, fakes):
    """A card made during a background task (its own hidden conversation) shows in the task's conversation and in the
    one the task was asked from; answered there, gone from both and from the Boîte."""
    bridge, _ = mail_bridge(bridges, fakes)
    assert add_mailbox(bridge, fakes).status_code == 201
    hermes = fakes.hermes
    hermes.add_session("api_c", [{"role": "user", "content": "Trie ma boîte en arrière-plan"}], title="Courrier")
    asking = watched_run(bridge, fakes, "run_c", "api_c")
    task_gate = threading.Event()
    hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_mail_bulk", preview="x"), task_gate,
                         sse("run.completed", run_id="r1", output="Carte proposée.")]
    started = call(bridge, "start_task", {"title": "Tri du courrier", "instructions": "Range les mails de la banque"},
                   token=TOKEN)
    assert not started["isError"], text_of(started)
    asking.set()
    job = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"][0]
    assert job["from_session"] == "api_c"
    assert wait_until(lambda: hermes.seen("/p/coach/v1/runs/r1/events"), timeout=20)
    time.sleep(0.5)
    made = call(bridge, "mail_bulk", {"action": "move:Banque", "domain": "banque.example"}, token=TOKEN)
    assert not made.get("isError"), made
    [card] = mail_cards(bridge)
    assert card["session_id"] == "api_c"            # the conversation the task reports to
    asked_in = thread_of(bridge, "api_c")
    assert [a["run_id"] for a in pending(bridge, asked_in)[1]] == [card["run_id"]]
    task_session = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"][0]["session_id"]
    task_thread = thread_of(bridge, task_session)
    assert task_thread != asked_in and [a["run_id"] for a in pending(bridge, task_thread)[1]] == [card["run_id"]]
    assert answer(bridge, card).status_code == 200
    assert mail_cards(bridge) == [] and pending(bridge, asked_in)[1] == [] and pending(bridge, task_thread)[1] == []
    task_gate.set()


def test_cards_of_the_same_kind_fold_into_one_answered_at_once(bridges, fakes):
    """Generic folding (card_place.rs `fold`), two kinds: standing mail rules doing the same thing, and mail tidy cards
    made before a card joined the waiting one (Alex's 15): one card each, its sentence saying how many more, « Voir la
    liste » every one of them; its « Oui » answers them all. Another kind of card is never folded with them."""
    bridge, _ = mail_bridge(bridges, fakes)
    assert add_mailbox(bridge, fakes).status_code == 201
    for sender in ("banque.example", "shop.example"):
        made = call(bridge, "mail_bulk", {"action": "move:Tri", "domain": sender, "rule": True}, token=TOKEN)
        assert not made.get("isError"), made
    made = call(bridge, "mail_bulk", {"action": "archive", "domain": "banque.example"}, token=TOKEN)
    assert not made.get("isError"), made
    # two tidy cards for one decision, as the bridge made them before (one per sender)
    bridge.stop()
    store = bridge.state_dir / "mail" / "tidy.json"
    kept = json.loads(store.read_text())
    [first] = kept["cards"]
    second = {**first, "id": "0123456789ab", "uids": [103], "message_ids": ["<p1@shop.example>"],
              "groups": [["shop.example", 1]], "sample": ["shop"], "at": first["at"] + 1}
    kept["cards"].append(second)
    store.write_text(json.dumps(kept))
    bridge.start(write=False)

    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "coach"}).json()["items"]
    rules = [i for i in items if i["run_id"].startswith("bip-mailrule-")]
    tidy = [i for i in items if i["run_id"].startswith("bip-mail-")]
    assert len(rules) == 1 and len(tidy) == 1, items
    [rule], [card] = rules, tidy
    assert rule["bip_summary"].endswith("(+1 more of the same kind)") and len(rule["bip_folded"]) == 2
    assert "hetzner" not in rule["bip_detail"] and "banque.example" in rule["bip_detail"] and "shop.example" in rule["bip_detail"]
    assert card["run_id"] == f"bip-mail-{first['id']}" and card["bip_folded"] == [card["run_id"], "bip-mail-0123456789ab"]
    assert card["bip_summary"] == "Coach wants to archive 1 e-mail (from banque.example) (+1 more of the same kind)"
    # one « Oui »: both rules on; one « Oui »: both archived
    assert answer(bridge, rule).status_code == 200
    on = bridge.get("/v1/mail/rules", headers=AUTH, params={"agent": "coach"}).json()["items"]
    assert sorted(r["domain"] for r in on) == ["banque.example", "shop.example"]
    assert answer(bridge, card).status_code == 200
    assert bridge.get("/v1/approvals", headers=AUTH, params={"agent": "coach"}).json()["items"] == []
    assert wait_until(lambda: "<p1@shop.example>" not in fakes.imap.message_ids("INBOX")
                      and "<r1@banque.example>" not in fakes.imap.message_ids("INBOX"), timeout=20), fakes.imap.commands
