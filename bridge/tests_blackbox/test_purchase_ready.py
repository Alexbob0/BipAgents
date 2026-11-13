"""« Prêt à acheter » (internal/notes/watcher-urgent-pay.md): a watcher made for a purchase (`watch.purchase`, given by
the agent's connector_register) shows a checklist in its detail (`GET /v1/agents/<a>/connectors/<id>`): the shop
account (a login of the vault for that site, one the agent may use), the « Coordonnées » a checkout needs (filled, not
only there), the card ready for that site and price, the rehearsal. Its actions:

- `POST …/connectors/<id>/purchase/account`: one turn of the agent in the watcher's conversation, told to create the
  account with vault_create_login and to fill from the « Coordonnées », buying nothing;
- `POST …/connectors/<id>/purchase/rehearse`: one turn up to the payment page without paying; pay_with_card is refused
  to that agent meanwhile, a second rehearsal too; its verdict (REHEARSAL OK / BLOCKED: …) is kept for the checklist,
  its words for the person go to the conversation without the verdict line;
- `POST /v1/agents/<a>/urgent-test`: « Faire un essai », the urgent payment's push marked test, no card.

Only the payment networks' published test numbers are used."""
import hashlib
import json
import threading
import time

from bbkit.common import AUTH, boite_items
from bbkit.servers import wait_until
from conftest import profile, wait_registered
from test_card_payments import CARD, PAY
from test_connectors import IMAGE, TOKEN, approve_next, text
from test_sandbox import isolated
from test_watchers import CHECK, NOW

DEVICE = "c" * 64
PURCHASE = {"site": "https://www.Shop.example/billets", "what": "2 billets Stromae", "price": "45,00", "currency": "eur"}
REGISTER = {"name": "Billets Stromae", "sites": ["www.shop.example"], "description": "La mise en vente des billets",
            "icon": "cart", "folder": "/workspace/connecteurs/billets", "entry": "python3 check.py",
            "watch": {"every": "15m", "notify": "on_change", "purchase": PURCHASE}}
PAY_ARGS = {"card": "petits achats", "amount": "45,00", "currency": "eur", "merchant": "2 billets Stromae"}
HOME = {"kind": "address", "label": "Perso", "full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai",
        "city": "Testville", "postal_code": "00100", "country": "fr", "agents": ["vie"]}


def setup(bridges, fakes):
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie",))
    bridge.config["agents"]["vie"].update({"isolated": True, "python": f"{bridge.fakebin}/podman exec -i hermes-vie python",
                                           "hermes_home": "/home/hermes/.hermes"})
    bridge.config["hermes"]["isolated"] = True
    bridge.config["watchers"] = {"tick_seconds": 3600, "wake_gap_minutes": 0, "failures_max": 3, "run_now_gap_seconds": 0}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-vie"], stdout=json.dumps({"id": "x"}) + "\n")
    bridge.commands.rule("podman", ["run", "--rm", "-i"], exec_image=IMAGE)
    value = bridge.root / "value.txt"
    value.write_text("rupture")
    workspace = profile(hub, "vie") / "sandboxes" / "docker" / "default" / "workspace" / "connecteurs"
    for name in ("billets", "meteo"):
        (workspace / name).mkdir(parents=True)
        (workspace / name / "check.py").write_text(CHECK.replace("__VALUE__", repr(str(value))))
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (DEVICE, "sandbox", "[]", NOW, NOW))])
    bridge.start()
    wait_registered(bridge, ["vie"])
    path = bridge.state_dir / "agent-tools.json"
    tokens = json.loads(path.read_text())["tokens"]
    tokens["vie"] = hashlib.sha256(TOKEN.encode()).hexdigest()
    path.write_text(json.dumps({"tokens": tokens}))
    result, card = approve_next(bridge, "connector_register", REGISTER)
    assert "approved" in text(result), text(result)
    conn = next(c for c in bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"]
                if c["name"] == "Billets Stromae")
    return bridge, conn, card


def purchase(bridge, conn):
    detail = bridge.get(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH)
    assert detail.status_code == 200, detail.text
    return detail.json()["watch"]["purchase"]


def step(bridge, conn, name):
    return next(s for s in purchase(bridge, conn)["steps"] if s["id"] == name)


def pay_tool(bridge, arguments):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "pay_with_card", "arguments": arguments}}
    resp = bridge.post(PAY, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=60)
    assert resp.status_code == 200, resp.text
    return resp.json()["result"]


def purchase_turns(fakes):
    return {run: r for run, r in fakes.hermes.started.items() if "<!--bip:purchase" in (r.get("input") or "")}


def said(bridge):
    return [i["text"] for i in boite_items(bridge) if i["title"] == "Billets Stromae"]


def vault_item(bridge, body):
    made = bridge.post("/v1/vault", headers=AUTH, json=body)
    assert made.status_code == 201, made.text
    return made.json()


def test_a_purchase_watcher_s_checklist_follows_the_vault_and_the_switch(bridges, fakes):
    bridge, conn, card = setup(bridges, fakes)
    # the agent's purchase, read as the bridge keeps it: the shop's host, the price in cents shown back « 45.00 »
    shown = purchase(bridge, conn)
    assert (shown["site"], shown["what"], shown["price"], shown["currency"]) == ("shop.example", "2 billets Stromae",
                                                                                 "45.00", "EUR")
    assert [s["id"] for s in shown["steps"]] == ["account", "address", "card", "rehearsal"]
    assert shown["payments_allowed"] is False
    assert step(bridge, conn, "account")["state"] == "missing"
    assert step(bridge, conn, "address") == {"id": "address", "state": "missing", "entries": 0}
    assert step(bridge, conn, "card")["state"] == "off"
    assert step(bridge, conn, "rehearsal")["state"] == "never"

    # the shop account: a login for that site the agent may use (one of another site, or kept from it, is none)
    vault_item(bridge, {"label": "Autre", "site": "shop.example.evil.test", "identifier": "camille@example.org",
                        "password": "x-Secret-1", "agents": ["vie"]})
    vault_item(bridge, {"label": "Shop (pas pour Vie)", "site": "www.shop.example", "identifier": "camille@example.org",
                        "password": "x-Secret-2", "agents": []})
    assert step(bridge, conn, "account")["state"] == "missing"
    vault_item(bridge, {"label": "Shop", "site": "secure.shop.example/login", "identifier": "camille@example.org",
                        "password": "x-Secret-3", "agents": ["vie"]})
    assert step(bridge, conn, "account")["state"] == "ok"

    # the « Coordonnées »: an entry there is not enough, the checkout needs its name, address and a phone
    home = vault_item(bridge, HOME)
    vault_item(bridge, {**HOME, "label": "Pas pour Vie", "phones": ["+33 6 00 00 00 00"], "agents": []})
    address = step(bridge, conn, "address")
    assert address == {"id": "address", "state": "incomplete", "entries": 0, "entry": "Perso", "entry_id": home["id"],
                       "missing": ["phone"]}, address
    filled = bridge.patch(f"/v1/vault/{home['id']}", headers=AUTH, json={"phones": ["+33 6 00 00 00 00"]})
    assert filled.status_code == 200, filled.text
    assert step(bridge, conn, "address") == {"id": "address", "state": "ok", "entries": 1, "entry": "Perso",
                                             "entry_id": home["id"]}

    # the card: payments on, then one for another site, one capped under the price, one that fits
    on = bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True})
    assert on.status_code == 200, on.text
    assert purchase(bridge, conn)["payments_allowed"] is True
    assert step(bridge, conn, "card") == {"id": "card", "state": "missing"}
    other = vault_item(bridge, {**CARD, "label": "Autre site", "sites": ["autre.example"]})
    assert step(bridge, conn, "card")["state"] == "other_site"
    capped = vault_item(bridge, {**CARD, "label": "Plafonnée", "cap_eur": 20})
    low = step(bridge, conn, "card")
    assert (low["state"], low["card"], low["card_id"], low["cap"]) == ("cap_low", "Plafonnée", capped["id"], "20.00")
    fits = vault_item(bridge, {**CARD, "label": "Petits achats"})
    ready = step(bridge, conn, "card")
    assert (ready["state"], ready["card"], ready["card_id"]) == ("ok", "Petits achats", fits["id"]), ready
    assert other["id"] != fits["id"]
    # « Suspendre tous les paiements »
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": False}).status_code == 200
    assert step(bridge, conn, "card")["state"] == "off" and purchase(bridge, conn)["payments_allowed"] is False
    # no card number anywhere in the detail
    assert "4242424242424242" not in json.dumps(purchase(bridge, conn))


def test_the_agent_creates_the_account_in_the_watchers_conversation(bridges, fakes):
    bridge, conn, card = setup(bridges, fakes)
    fakes.hermes.run_output = "Ton compte sur shop.example est prêt ; il me manque ton numéro de téléphone."
    started = bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/account", headers=AUTH, json={"way": "create"})
    assert started.status_code == 202 and started.json() == {"ok": True}, started.text
    assert wait_until(lambda: purchase_turns(fakes), timeout=20)
    [turn] = purchase_turns(fakes).values()
    note = turn["input"]
    assert note.startswith("<!--bip:purchase\n") and note.endswith("\n-->")
    assert "Create their customer account on shop.example now with vault_create_login" in note
    assert "« 2 billets Stromae »" in note and "Don't buy anything" in note and "ask only for what is missing" in note
    # in the watcher's own conversation, its reply there for the person
    created = [body for agent, body in fakes.hermes.created if agent == "vie"]
    assert created[-1]["title"] == "Watcher: Billets Stromae" and turn["session_id"]
    assert wait_until(lambda: said(bridge), timeout=30)
    assert said(bridge) == ["Ton compte sur shop.example est prêt ; il me manque ton numéro de téléphone."]
    # a watcher of the same agent made for no purchase, an unknown one, an unknown agent
    result, _ = approve_next(bridge, "connector_register", {**REGISTER, "name": "Météo", "sites": ["meteo.example"],
                                                            "folder": "/workspace/connecteurs/meteo",
                                                            "watch": {"every": "1h", "notify": "on_change"}},
                             seen=(card["request_id"],))
    assert "approved" in text(result), text(result)
    plain = next(c for c in bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"] if c["name"] == "Météo")
    assert "purchase" not in bridge.get(f"/v1/agents/vie/connectors/{plain['id']}", headers=AUTH).json()["watch"]
    for path in ("account", "rehearse"):
        refused = bridge.post(f"/v1/agents/vie/connectors/{plain['id']}/purchase/{path}", headers=AUTH)
        assert refused.status_code == 409 and refused.json()["detail"]["code"] == "not_a_purchase_watcher", refused.text
        assert bridge.post(f"/v1/agents/vie/connectors/nope/purchase/{path}", headers=AUTH).status_code == 404
        assert bridge.post(f"/v1/agents/nobody/connectors/{conn['id']}/purchase/{path}", headers=AUTH).status_code == 404
    # the bridge's key only
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/account").status_code == 401
    assert len(purchase_turns(fakes)) == 1


def test_a_rehearsal_never_pays_and_its_verdict_is_kept(bridges, fakes):
    bridge, conn, _ = setup(bridges, fakes)
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 200
    vault_item(bridge, CARD)
    fakes.hermes.rounds = 10**6   # the rehearsal's turn goes on until the test ends it
    started = bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH)
    assert started.status_code == 202, started.text
    assert wait_until(lambda: purchase_turns(fakes), timeout=20)
    [(run, turn)] = purchase_turns(fakes).items()
    note = turn["input"]
    assert "A rehearsal your person asked for: go once through shop.example" in note and "€45.00" in note
    assert "NEVER pay: pay_with_card is refused during a rehearsal" in note and "REHEARSAL OK" in note
    assert step(bridge, conn, "rehearsal")["state"] == "running"
    time.sleep(1)   # the bridge learns the run's id as Hermes answers its start
    # meanwhile: never a payment from the rehearsal's turn (refused before the person is asked), never a second one
    refused = pay_tool(bridge, PAY_ARGS)
    assert refused["isError"] is True and "A purchase rehearsal of yours is running: no payment now" in refused["content"][0]["text"], refused
    outcomes = [e["outcome"] for e in bridge.get("/v1/vault/payments", headers=AUTH).json()["journal"]]
    assert "asked" not in outcomes
    assert not [c for c in bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]
                if c["run_id"].startswith("bip-pay-")]
    again = bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH)
    assert again.status_code == 409 and again.json()["detail"]["code"] == "rehearsal_running", again.text
    # the agent's verdict: kept for the checklist; its words for the person, without the verdict line
    fakes.hermes.run_outputs[run] = "**REHEARSAL OK**\nTout est prêt : connexion, adresse et page de paiement."
    fakes.hermes.rounds = 1
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["state"] == "ok", timeout=30)
    done = step(bridge, conn, "rehearsal")
    assert done["at"] > 0 and done["blocked"] is None
    assert wait_until(lambda: said(bridge), timeout=20)
    assert said(bridge) == ["Tout est prêt : connexion, adresse et page de paiement."]
    # over: pay_with_card is no longer the rehearsal's refusal
    after = pay_tool(bridge, PAY_ARGS)
    assert "rehearsal" not in after["content"][0]["text"]

    # blocked: what blocked, in the agent's words, kept; another rehearsal may start
    fakes.hermes.run_output = "REHEARSAL BLOCKED: le site demande un code par SMS\nIl faudra ton téléphone."
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH).status_code == 202
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["state"] == "blocked", timeout=30)
    assert step(bridge, conn, "rehearsal")["blocked"] == "le site demande un code par SMS"
    assert wait_until(lambda: "Il faudra ton téléphone." in said(bridge), timeout=20)
    assert not [t for t in said(bridge) if "REHEARSAL" in t]
    # a reply without a verdict: blocked, nothing named
    fakes.hermes.run_output = "J'ai regardé le site."
    assert bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH).status_code == 202
    assert wait_until(lambda: "J'ai regardé le site." in said(bridge), timeout=30)
    assert wait_until(lambda: step(bridge, conn, "rehearsal")["blocked"] == "", timeout=10)
    assert step(bridge, conn, "rehearsal")["state"] == "blocked"


def test_a_test_alert_is_the_urgent_payment_push_marked_test(bridges, fakes):
    bridge, conn, _ = setup(bridges, fakes)
    sent = bridge.post("/v1/agents/vie/urgent-test", headers=AUTH)
    assert sent.status_code == 200 and sent.json() == {"ok": True, "sent": 1}, sent.text
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("test") == 1], timeout=10)
    [push] = [p for p in fakes.apns.payloads() if p.get("test") == 1]
    # urgent, « Payer » (PAYMENT), time-sensitive; the test's id in run_id, no payment anywhere
    assert (push["urgent"], push["t"], push["aps"]["category"]) == (1, "approval", "PAYMENT")
    assert push["aps"]["interruption-level"] == "time-sensitive" and push["aps"]["relevance-score"] == 1
    assert push["run_id"].startswith("bip-paytest-") and push["pay"] == push["request_id"] == push["run_id"][12:]
    # the words are never in clear (a device without a key gets the neutral line)
    assert push["aps"]["alert"]["loc-key"] == "notif.approval" and "pay fast" not in json.dumps(push)
    # no card: answering it is no payment
    assert not [c for c in bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]
                if c["run_id"].startswith(("bip-pay", "bip-cart-"))]
    assert bridge.post("/v1/agents/nobody/urgent-test", headers=AUTH).status_code == 404
    assert bridge.post("/v1/agents/vie/urgent-test").status_code == 401


def test_a_rehearsal_waiting_for_the_watchers_conversation_never_refuses_its_real_payment(bridges, fakes):
    """Review 13/11: a rehearsal tapped while the watcher's own turn runs (the tickets went on sale) waits for that
    conversation; meanwhile the watcher's real payment is never refused as a rehearsal's (only the rehearsal's own
    turn's pay_with_card is)."""
    bridge, conn, _ = setup(bridges, fakes)
    assert bridge.post("/v1/vault/payments", headers=AUTH, json={"enabled": True}).status_code == 200
    vault_item(bridge, CARD)
    value = bridge.root / "value.txt"
    run = f"/v1/agents/vie/connectors/{conn['id']}/run"
    baseline = bridge.post(run, headers=AUTH, params={"wait": "true"}, timeout=120)
    assert baseline.status_code == 200, baseline.text
    # the tickets go on sale: the watcher wakes the agent in its conversation; that turn goes on (to the checkout),
    # its events followed as Hermes streams them
    going = threading.Event()
    fakes.hermes.runs[f"r{len(fakes.hermes.started) + 1}"] = [going]
    fakes.hermes.rounds = 10**6
    value.write_text("en vente")
    assert bridge.post(run, headers=AUTH).status_code in (200, 202)
    watch_turns = lambda: [r for r in fakes.hermes.started.values() if "<!--bip:watch" in (r.get("input") or "")]
    assert wait_until(watch_turns, timeout=30), fakes.hermes.started
    try:
        # meanwhile the person taps « Répétition » in the checklist: its turn waits for the conversation
        started = bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/purchase/rehearse", headers=AUTH)
        assert started.status_code == 202, started.text
        time.sleep(3)
        assert not purchase_turns(fakes), "the rehearsal's turn waits for the watcher's"
        assert step(bridge, conn, "rehearsal")["state"] == "running"
        # the watcher's real turn reaches the payment page and pays: never refused as a rehearsal
        paid = pay_tool(bridge, PAY_ARGS)
        said_ = paid["content"][0]["text"]
        assert "rehearsal" not in said_.lower(), f"the watcher's real payment refused as a rehearsal: {said_}"
    finally:
        going.set()
