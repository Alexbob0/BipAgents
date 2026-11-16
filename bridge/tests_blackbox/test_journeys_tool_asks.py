"""The release journeys of « Puis-je utiliser Rappels ? » (tool_asks.rs, 16/11; docs/release-journeys.md J59–J63,
column « Pont »), as the apps do them over HTTP, each page checked as the person SEES it (bbkit/displayable.py):

- J59: the agent asks for Rappels on a card in the conversation; « Oui » (the phone's part done): the answered line,
  the agent goes on in the same turn;
- J60: Mails, nothing connected: « Oui » is refused until the mailbox is connected in the app's own screen, then the
  card closes and the agent goes on;
- J61: « Non »: the answered line, the agent does without; asked again in the same conversation, no second card;
- J62: a server tool (Météo): on at once;
- J63: an older app's generic card: « Oui, vas-y » on Rappels no phone shares says where to go, the card stays.

Each test is one journey `Jnn`."""
import pytest

from bbkit import displayable as shown_page
from bbkit.common import AUTH
from bbkit.journeys import AGENT, journey_bridge
from bbkit.model import PeopleModel, tool_asked
from bbkit.people import mcp_as, reply_in
from bbkit.servers import wait_until
from test_mailbox import PASSWORD

WHY = {
    "reminders": "Pour te rappeler le pain demain à 18 h",
    "mail": "Pour chercher la facture de mars dans ta boîte",
    "weather": "Pour savoir s'il pleut demain",
}


@pytest.fixture
def journey(bridges, fakes):
    made = journey_bridge(bridges, fakes, tool_asks=True)
    made.model = PeopleModel.upgrade(made.model)
    # the agent speaks French (its cards' words: handback::language_of)
    assert made.bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200

    def ask(run):
        tool = tool_asked(run.words.lower())
        return mcp_as(made.bridge, made.token, "/v1/agent-tools/mcp", "ask_for_tool", {"tool": tool, "why": WHY[tool]},
                      timeout=400)

    made.model.calls["tool_ask"] = ask
    return made


def cid(n: int, k: int = 0) -> str:
    return f"J0URNEY0-0000-4000-8{k:03d}-{n:012d}"


def tool_cards(journey):
    return [c for c in journey.page()["pending"].get("approvals") or [] if c.get("run_id", "").startswith("bip-tool-")]


def waiting_tool_card(journey, timeout=30):
    assert wait_until(lambda: tool_cards(journey), timeout=timeout), journey.page()["pending"]
    cards = tool_cards(journey)
    assert len(cards) == 1, cards
    problems = shown_page.card_problems(cards[0], "the tool card")
    assert not problems, problems
    return cards[0]


def answer(journey, card, body):
    return journey.bridge.post(f"/v1/tools/requests/{card['request_id']}", headers=AUTH, json=body)


def answered_rows(journey):
    """The answered tool cards in the conversation, as the apps draw them (timeline items of kind `approval`)."""
    items = journey.page().get("items") or []
    return [i for i in items if i.get("kind") == "approval"
            and str((i.get("approval") or {}).get("run_id") or "").startswith("bip-tool-")]


def test_j59_vie_asks_for_rappels_on_a_card_and_goes_on_after_oui(journey):
    run = journey.send(cid(59), "Rappelle-moi d'acheter du pain demain à 18 h #demande-outil rappels")
    assert run.status_code == 200, run.text
    card = waiting_tool_card(journey)
    tool = card["bip_tool"]
    # the tool as the registry names it, the agent's reason in one line
    assert (tool["tool"], tool["kind"], tool["label"], tool["risk"]) == ("reminders", "phone", "Rappels", "check")
    assert tool["reason"] == WHY["reminders"]
    assert card["bip_summary"] == "Vie demande l'outil Rappels"
    assert card["choices"] == ["once", "deny"]
    # the system's permission given on the phone (simulated), then « Oui »
    done = answer(journey, card, {"allow": True, "phone_ready": True})
    assert done.status_code in (200, 404), done.text   # 404: closed by itself a moment before (on already)
    page = reply_in(journey.bridge, journey.thread, AGENT, "C’est fait", timeout=60)
    shown_page.check(journey.bridge, page, AGENT)
    assert not tool_cards(journey), "answered: gone from the conversation"
    rows = answered_rows(journey)
    assert rows and rows[-1]["approval"]["description"] == "Rappels activé pour Vie", rows
    assert rows[-1]["approval"]["choice"] == "once"


def test_j60_mails_oui_connecter_waits_for_the_mailbox_then_vie_goes_on(journey, fakes):
    fakes.imap.users["vie@proton.me"] = PASSWORD
    run = journey.send(cid(60), "Retrouve ma facture de mars #demande-outil mails")
    assert run.status_code == 200, run.text
    card = waiting_tool_card(journey)
    assert card["bip_tool"]["kind"] == "account"
    assert card["bip_tool"]["connected"] is False   # the button says « Oui, connecter »
    early = answer(journey, card, {"allow": True})
    assert early.status_code == 409 and early.json()["detail"]["code"] == "needs_connection", early.text
    assert len(tool_cards(journey)) == 1, "nothing connected: the card waits"
    # the app's own screen « Ajouter une boîte mail », Vie ticked
    added = journey.bridge.post("/v1/mail/accounts", headers=AUTH, json={
        "provider": "proton", "username": "vie@proton.me", "password": PASSWORD, "port": fakes.imap.port,
        "agents": [AGENT]})
    assert added.status_code == 201, added.text
    done = answer(journey, card, {"allow": True})
    assert done.status_code in (200, 404), done.text   # 404: the bridge saw the mailbox first and closed it
    reply_in(journey.bridge, journey.thread, AGENT, "C’est fait", timeout=60)
    assert not tool_cards(journey)
    rows = answered_rows(journey)
    assert rows and rows[-1]["approval"]["description"] == "Mails activé pour Vie", rows


def test_j61_non_vie_does_without_and_asks_no_more_here(journey):
    journey.send(cid(61), "Rappelle-moi la réunion #demande-outil rappels")
    card = waiting_tool_card(journey)
    refused = answer(journey, card, {"allow": False})
    assert refused.status_code == 200 and refused.json()["state"] == "refused", refused.text
    reply_in(journey.bridge, journey.thread, AGENT, "je fais sans", timeout=60)
    rows = answered_rows(journey)
    assert rows and rows[-1]["approval"]["choice"] == "deny" and rows[-1]["approval"]["description"] == "Rappels"
    # asked again in the same conversation: no second card
    journey.send(cid(61, 1), "Et pour demain ? #demande-outil rappels")
    page = reply_in(journey.bridge, journey.thread, AGENT, "je fais sans", timeout=60)
    assert not tool_cards(journey), page["pending"]


def test_j62_a_server_tool_is_on_at_once(journey):
    journey.send(cid(62), "Il pleut demain ? #demande-outil meteo")
    card = waiting_tool_card(journey)
    assert card["bip_tool"]["kind"] == "server"
    done = answer(journey, card, {"allow": True})
    assert done.status_code == 200 and done.json()["state"] == "on", done.text
    reply_in(journey.bridge, journey.thread, AGENT, "C’est fait", timeout=60)
    listed = journey.bridge.post("/v1/agent-tools/mcp", headers={"Authorization": f"Bearer {journey.token}"},
                                 json={"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}).json()
    names = [t["name"] for t in listed["result"]["tools"]]
    assert "weather" in names, names


def test_j63_an_older_apps_generic_card_says_where_to_go_and_stays(journey):
    journey.send(cid(63), "Rappelle-moi le dentiste #demande-outil rappels")
    card = waiting_tool_card(journey)
    # what an older app reads: the generic card's words, « Oui, vas-y / Non »
    assert card["bip_summary"] and card["choices"] == ["once", "deny"] and "Applis et outils" in card["description"]
    yes = journey.bridge.post("/v1/approve", headers=AUTH, json={"agent": AGENT, "run_id": card["run_id"], "choice": "once"})
    assert yes.status_code == 409, yes.text
    assert "Active « Rappels » dans Applis et outils" in yes.json()["detail"], yes.text
    assert len(tool_cards(journey)) == 1, "the card stays"
    no = journey.bridge.post("/v1/approve", headers=AUTH, json={"agent": AGENT, "run_id": card["run_id"], "choice": "deny"})
    assert no.status_code == 200, no.text
    assert wait_until(lambda: not tool_cards(journey), timeout=10)
