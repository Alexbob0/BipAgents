"""The bridge's side of the first form sent to a site (security audit 1.2, 17/11; Alex « Oui, 1re fois par site »):
`ask_to_send` on the vault's entry, after the agent's browser refused a form (test_form_send_chrome.py). One card
« Envoyer sur ce site ? » listed with the approvals (`bip-ok-…`, « Oui » / « Non » and « Toujours pour ce site »):

- « Oui »: one sending granted to the agent's browser (`send-sites.json` beside its `busy`), nothing kept for good;
- « Toujours pour ce site »: the site in « Sites autorisés » (validated for sending), and in the browser's file;
- « Non »: nothing granted, the agent told to stop.

No Chromium needed: the call only needs the agent's browser set up (its ports)."""
import json
import types
from concurrent.futures import ThreadPoolExecutor

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_card_payments import TOKEN, pay_bridge

VAULT = "/v1/agent-tools/vault"


def ask_to_send(bridge, site, what="un message au service client"):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "ask_to_send", "arguments": {"site": site, "what": what}}}
    resp = bridge.post(VAULT, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=120)
    return resp.json()["result"]


def cards(bridge):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-ok-")]


def answered(bridge, site, choice):
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(ask_to_send, bridge, site)
        assert wait_until(lambda: cards(bridge), timeout=30), "no card"
        [card] = cards(bridge)
        said = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": card["run_id"], "choice": choice})
        assert said.status_code == 200, said.text
        return call.result(timeout=120), card


def allowed(bridge):
    path = bridge.data / "browsers" / "vie" / "send-sites.json"
    return json.loads(path.read_text()) if path.exists() else {}


def test_oui_grants_one_sending_toujours_keeps_the_site_and_non_nothing(bridges, fakes):
    bridge = pay_bridge(bridges, fakes, types.SimpleNamespace(port=19400))
    # « Oui »: this once
    result, card = answered(bridge, "https://contact.example/nous-ecrire", "once")
    # in the agent's language (French, else English here)
    assert card["description"] in ("Envoyer sur ce site ? Vie veut envoyer un formulaire à contact.example",
                                   "Send to this site? Vie wants to send a form to contact.example"), card
    assert card["choices"] == ["once", "deny"], card
    assert card["bip_always"] in ("Toujours pour ce site", "Always for this site"), card
    assert card["bip_detail"].endswith("« un message au service client »"), card
    assert result["isError"] is False and "do the same step again" in result["content"][0]["text"], result
    file = allowed(bridge)
    assert [g["host"] for g in file["once"]] == ["contact.example"] and "contact.example" not in file["always"], file
    sites = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert "contact.example" not in json.dumps(sites["sites"]), sites
    # « Non »: nothing more
    result, _ = answered(bridge, "forum.example", "deny")
    assert result["isError"] is True and "said no" in result["content"][0]["text"], result
    assert "forum.example" not in json.dumps(allowed(bridge))
    # « Toujours pour ce site »: in « Sites autorisés » and in the browser's file; asked again, no card
    result, _ = answered(bridge, "forum.example", "always")
    assert result["isError"] is False and "for good" in result["content"][0]["text"], result
    assert "forum.example" in allowed(bridge)["always"]
    sites = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert "forum.example" in json.dumps(sites["sites"]), sites
    again = ask_to_send(bridge, "forum.example")
    assert again["isError"] is False and "allowed for sending already" in again["content"][0]["text"], again
    assert not cards(bridge)
    # removed by the person: out of the browser's file too
    gone = bridge.delete("/v1/agents/vie/sandbox/sites/forum.example", headers=AUTH)
    assert gone.status_code == 200, gone.text
    assert "forum.example" not in allowed(bridge)["always"]
