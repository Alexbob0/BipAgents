"""The release journeys of « Le coffre rassure » (passation-pro briefs/design-coffre-rassurer.md; docs/release-journeys.md
J64–J65, column « Pont »), as the apps do them over HTTP, each page checked as the person SEES it
(bbkit/displayable.py):

- J64: the agent logs in with the vault (`#coffre`): under its answer, the vault's line « Rempli par le coffre sur
  fournisseur.example, sans le donner à l'agent », with the vault item a tap opens; the hidden mark never shown;
- J65: « Je le taperai moi-même » (`#connexion`): the card says the person may type it themselves; they answer `self`,
  take the agent's browser and hand it back; the agent goes on in the same turn; nothing is kept in the vault; the
  answered card reads `self` in the conversation.

The journeys' bridge has no Chrome: `vault_login`'s answer is the one the bridge gives on a page with a login form
(bbkit/journeys.py `vault_login`); its making is test_vault_login.py's. Each test is one journey `Jnn`."""
import json

import pytest

from bbkit import displayable
from bbkit.common import AUTH
from bbkit.journeys import AGENT, journey_bridge
from bbkit.servers import wait_until


@pytest.fixture
def journey(bridges, fakes):
    made = journey_bridge(bridges, fakes, browser=True)
    # the agent speaks French (its lines' words: handback::language_of)
    assert made.bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200
    return made


def cid(n: int) -> str:
    return f"J0URNEY0-0000-4000-8064-{n:012d}"


def started(resp):
    assert resp.status_code == 200, resp.text
    assert resp.json()["state"] in ("started", "steered", "queued"), resp.json()
    return resp.json()


def reply_done(journey, words, timeout=60):
    def found():
        page = journey.page()
        agents = [i for i in page["items"] if i["kind"] == "message" and i.get("role") == "agent"]
        return page if agents and words in agents[-1].get("text", "") and not page.get("run") else None
    assert wait_until(found, timeout=timeout), [(i["kind"], i.get("text", "")[:60]) for i in journey.page()["items"]]
    return found()


def secret_cards(journey):
    return [c for c in journey.page()["pending"].get("approvals") or [] if c.get("run_id", "").startswith("bip-secret-")]


def test_j64_the_vaults_line_under_the_answer_opens_the_login(journey):
    started(journey.send(cid(64), "Connecte-toi au fournisseur #coffre"))
    page = reply_done(journey, "Je suis connecté au fournisseur.")
    lines = [i for i in page["items"] if i["kind"] == "vault_line"]
    assert len(lines) == 1, [(i["kind"], i.get("text", "")[:40]) for i in page["items"]]
    line = lines[0]
    assert line["text"] == "Rempli par le coffre sur fournisseur.example, sans le donner à l'agent"
    assert line["sites"] == ["fournisseur.example"]
    item = line["item"]
    vault = {i["id"]: i for i in journey.bridge.get("/v1/vault", headers=AUTH).json()["items"]}
    assert vault[item]["label"] == "Fournisseur" and vault[item]["domain"] == "fournisseur.example"
    # under the answer, the hidden mark nowhere
    items = page["items"]
    assert items[items.index(line) - 1]["text"] == "Je suis connecté au fournisseur."
    assert "bip:filled" not in json.dumps(page)
    displayable.check(journey.bridge, page, AGENT)


def test_j65_i_ll_type_it_myself_the_agent_goes_on_once_handed_back(journey):
    started(journey.send(cid(65), "Connecte-toi au fournisseur #connexion"))
    assert wait_until(lambda: secret_cards(journey), timeout=30), journey.page()["pending"]
    [card] = secret_cards(journey)
    assert (card["bip_secret_use"], card["bip_secret_self"]) == ("browser", True)
    displayable.check(journey.bridge, journey.page(), AGENT)
    chosen = journey.bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                                 json={"allow": False, "way": "self"})
    assert chosen.status_code == 200 and chosen.json() == {"ok": True, "choice": "self"}, chosen.text
    assert wait_until(lambda: not secret_cards(journey), timeout=15)
    # the app opens the desktop with the hand taken; leaving it hands the browser back
    taken = journey.bridge.post(f"/v1/agents/{AGENT}/desktop/control", headers=AUTH, json={"take": True})
    assert taken.status_code == 200 and taken.json()["you_hold"] is True, taken.text
    back = journey.bridge.post(f"/v1/agents/{AGENT}/desktop/control", headers=AUTH, json={"take": False})
    assert back.status_code == 200, back.text
    page = reply_done(journey, "tu es connecté")
    answered = [i for i in page["items"] if i["kind"] == "approval"
                and (i.get("approval") or {}).get("run_id") == card["run_id"]]
    assert [a["approval"]["choice"] for a in answered] == ["self"], answered
    vault = journey.bridge.get("/v1/vault", headers=AUTH).json()["items"]
    assert not [i for i in vault if i["kind"] == "login"], "nothing kept in the vault"
    displayable.check(journey.bridge, page, AGENT)
