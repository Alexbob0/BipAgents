"""« Coordonnées » in the vault (`/v1/vault`, kind `address`, and the agents' `vault_contact_details` /
`vault_save_contact_details` on `/v1/agent-tools/vault`): a name, an address, phones and e-mails with a label and a
hint, checked by the bridge, read back by the app to edit them, written into the Hermes vault of the agents the person
chose (for their browser fill), and read by an agent only when it was given the entry (an agent other people talk to:
only after the person's confirmation for it). All values are made up."""
import json
import threading

from cryptography.fernet import Fernet

from bbkit.common import AUTH, HERMES_KEY, read_sse, sse
from bbkit.mcp import TOKEN, give_token
from bbkit.servers import wait_until

HERMES_AUTH = {"Authorization": f"Bearer {HERMES_KEY}"}

VIE = "agent-token-for-vie-0123456789abcdef"
COACH = "agent-token-for-coach-0123456789abcd"
VAULT = "/v1/agent-tools/vault"
ANSWER = json.dumps({"id": "vault_1", "removed": True}) + "\n"
PRO = {"kind": "address", "label": "Kerfy Studio (pro)", "hint": "pour tout ce qui concerne la société",
       "full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai", "address_line2": "Bât. B",
       "city": "Testville", "state": "", "postal_code": "00100", "country": "fr",
       "phones": ["+33 6 00 00 00 01", "01 00 00 00 02"], "emails": ["camille@example.org"]}


def contacts_bridge(bridges, fakes, shared=False):
    bridge = bridges.new()
    for name in ("vie", "coach"):
        bridge.config["agents"][name.capitalize()] = {
            "hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
            "python": f"{bridge.fakebin}/podman exec -i hermes-{name} python", "hermes_home": "/home/hermes/.hermes"}
        bridge.commands.rule("podman", ["exec", "-i", f"hermes-{name}"], stdout=ANSWER)
    bridge.config["agents"]["Budget"] = {"hermes_url": f"{fakes.hermes.url}/p/budget", "hermes_key": "b" * 32}
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.hermes.url}
    if shared:
        bridge.state_dir.mkdir(parents=True, exist_ok=True)
        (bridge.state_dir / "households.json").write_text(json.dumps({"households": {"lea": {"name": "Léa"}}}))
        (bridge.state_dir / "agent-shares.json").write_text(json.dumps({"shared": {"coach": ["lea"]}}))
    bridge.start()
    give_token(bridge, "vie", VIE)
    give_token(bridge, "coach", COACH)
    return bridge


def tool(bridge, token, name, arguments):
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
    resp = bridge.post(VAULT, json=body, headers={"Authorization": f"Bearer {token}"}, timeout=60)
    result = resp.json()["result"]
    return result["content"][0]["text"], result["isError"]


def hermes_requests(bridge, agent):
    return [json.loads(e["stdin"]) for e in bridge.commands.entries("podman") if f"hermes-{agent}" in e["argv"]]


def records(bridge):
    folder = bridge.state_dir / "vault"
    key = Fernet((folder / "vault.key").read_bytes().strip())
    return json.loads(key.decrypt((folder / "vault.json.enc").read_bytes()))


def test_add_edit_and_list_contact_details(bridges, fakes):
    bridge = contacts_bridge(bridges, fakes)
    made = bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["vie"]})
    assert made.status_code == 201, made.text
    item = made.json()
    assert item["kind"] == "address" and item["label"] == "Kerfy Studio (pro)" and item["agents"] == ["vie"]
    assert item["hint"] == "pour tout ce qui concerne la société"
    assert item["contact"] == {"full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai",
                               "address_line2": "Bât. B", "city": "Testville", "state": "", "postal_code": "00100",
                               "country": "FR", "phones": ["+33 6 00 00 00 01", "01 00 00 00 02"],
                               "emails": ["camille@example.org"]}
    # Vie's Hermes vault gets an address item its browser fills: strings, the first phone and e-mail
    [request] = hermes_requests(bridge, "vie")
    assert request["op"] == "add" and request["kind"] == "address" and request["label"] == "Kerfy Studio (pro)"
    assert request["secret"] == {"full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai",
                                 "address_line2": "Bât. B", "city": "Testville", "postal_code": "00100",
                                 "country": "FR", "phone": "+33 6 00 00 00 01", "email": "camille@example.org"}
    # an edit: an e-mail more, a phone less; the rest kept
    edited = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH,
                          json={"phones": ["+33 6 00 00 00 01"], "emails": ["camille@example.org", "factures@example.org"]})
    assert edited.status_code == 200, edited.text
    assert edited.json()["contact"]["emails"] == ["camille@example.org", "factures@example.org"]
    assert edited.json()["contact"]["phones"] == ["+33 6 00 00 00 01"]
    assert edited.json()["contact"]["city"] == "Testville"
    listed = bridge.get("/v1/vault", headers=AUTH).json()["items"]
    assert listed == [edited.json()]
    # kept encrypted on disk like the rest
    assert b"Camille" not in (bridge.state_dir / "vault" / "vault.json.enc").read_bytes()
    # what the bridge refuses, in plain words
    for body, detail in (({"country": "France"}, "country must be a two-letter country code"),
                         ({"phones": ["appelle-moi"]}, "not a valid phone number: appelle-moi"),
                         ({"emails": ["camille@example"]}, "not a valid e-mail address: camille@example"),
                         ({"postal_code": "75<011"}, "postal code: letters, digits, spaces and dashes only"),
                         ({"hint": "x" * 301}, "the hint is too long (at most 300 characters)")):
        resp = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json=body)
        assert resp.status_code == 400 and resp.json() == {"detail": detail}, detail
    empty = bridge.post("/v1/vault", headers=AUTH, json={"kind": "address", "label": "Vide", "agents": []})
    assert empty.status_code == 400 and empty.json() == {"detail": "add at least one detail"}
    assert bridge.delete(f"/v1/vault/{item['id']}", headers=AUTH).status_code == 204
    assert bridge.get("/v1/vault", headers=AUTH).json()["items"] == []


def test_per_agent_access_and_what_the_agent_reads(bridges, fakes):
    bridge = contacts_bridge(bridges, fakes)
    refused = bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["vie", "budget"]})
    assert refused.status_code == 409 and refused.json()["detail"]["code"] == "cannot_hold"
    item = bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["vie"]}).json()
    bridge.post("/v1/vault", headers=AUTH, json={"kind": "address", "label": "Maison", "city": "Lyon",
                                                  "agents": ["coach"]})
    # a login next to it: never readable by the model
    bridge.post("/v1/vault", headers=AUTH, json={"label": "SNCF", "site": "sncf-connect.com", "identifier": "a@example.org",
                                                  "password": "s3cret-Pass!", "agents": ["vie"]})
    listed, error = tool(bridge, VIE, "vault_contact_details", {})
    assert not error and "« Kerfy Studio (pro) »" in listed and "when to use it: pour tout ce qui concerne" in listed
    assert "Maison" not in listed and "s3cret" not in listed and "SNCF" not in listed
    assert "<!--bip:vault" not in listed           # a listing is not a use: no chip
    used, error = tool(bridge, VIE, "vault_contact_details", {"label": "kerfy studio (PRO)"})
    assert not error
    assert "name: Camille Exemple" in used and "phones: +33 6 00 00 00 01, 01 00 00 00 02" in used
    assert "e-mails: camille@example.org" in used and "country (ISO code): FR" in used
    assert used.endswith(f"<!--bip:vault {item['id']} Kerfy Studio (pro)-->")
    missing, error = tool(bridge, VIE, "vault_contact_details", {"label": "Maison"})
    assert error and missing == "No entry « Maison ». Yours: « Kerfy Studio (pro) »."
    coach, _ = tool(bridge, COACH, "vault_contact_details", {})
    assert "Maison" in coach and "Kerfy" not in coach
    # taken back from Vie: nothing to read any more
    bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"agents": []})
    none, error = tool(bridge, VIE, "vault_contact_details", {})
    assert not error and none.startswith("The person keeps no contact details for you")


def test_an_agent_saves_what_the_person_gives_it(bridges, fakes):
    bridge = contacts_bridge(bridges, fakes)
    args = {"label": "Adresse pro", "hint": "factures", "address_line1": "3 rue Test", "city": "Testville",
            "postal_code": "00100", "country": "FR", "emails": ["pro@example.org"]}
    said, error = tool(bridge, VIE, "vault_save_contact_details", args)
    assert not error and said.startswith("Saved as « Adresse pro » in the person's vault"), said
    [saved] = [i for i in bridge.get("/v1/vault", headers=AUTH).json()["items"] if i["label"] == "Adresse pro"]
    assert saved["created_by"] == "vie" and saved["agents"] == ["vie"] and saved["hint"] == "factures"
    assert said.endswith(f"<!--bip:vault {saved['id']} Adresse pro-->")
    # the person edits it in the app like any entry
    edited = bridge.patch(f"/v1/vault/{saved['id']}", headers=AUTH, json={"city": "Autreville"})
    assert edited.status_code == 200 and edited.json()["contact"]["city"] == "Autreville"
    # the agent's own entry, changed again by the same label; the person's own never
    tool(bridge, VIE, "vault_save_contact_details", {"label": "adresse pro", "phones": ["06 00 00 00 03"]})
    again = [i for i in bridge.get("/v1/vault", headers=AUTH).json()["items"] if i["id"] == saved["id"]][0]
    assert again["contact"]["phones"] == ["06 00 00 00 03"] and again["contact"]["city"] == "Autreville"
    bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["vie"]})
    refused, error = tool(bridge, VIE, "vault_save_contact_details", {"label": "Kerfy Studio (pro)", "city": "X"})
    assert error and "only the person changes it" in refused
    bad, error = tool(bridge, VIE, "vault_save_contact_details", {"label": "Y", "emails": ["pas-une-adresse"]})
    assert error and bad == "Not saved: not a valid e-mail address: pas-une-adresse."
    assert [r["label"] for r in records(bridge)] == ["Adresse pro", "Kerfy Studio (pro)"]


def test_an_agent_other_people_talk_to_reads_only_what_was_confirmed_for_it(bridges, fakes):
    bridge = contacts_bridge(bridges, fakes, shared=True)
    agents = {a["id"]: a for a in bridge.get("/v1/vault", headers=AUTH).json()["agents"]}
    assert agents["coach"]["others"] is True and agents["coach"]["others_names"] == ["Léa"]
    refused = bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["coach"]})
    assert refused.status_code == 409 and refused.json()["detail"]["code"] == "confirm_others"
    none, _ = tool(bridge, COACH, "vault_contact_details", {})
    assert none.startswith("The person keeps no contact details for you")
    made = bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["coach"], "confirmed_others": True})
    assert made.status_code == 201, made.text
    read, error = tool(bridge, COACH, "vault_contact_details", {"label": "Kerfy Studio (pro)"})
    assert not error and "Camille Exemple" in read
    # taken back, then given again without the confirmation: refused, and nothing read meanwhile
    bridge.patch(f"/v1/vault/{made.json()['id']}", headers=AUTH, json={"agents": []})
    assert records(bridge)[0].get("confirmed_for") == []
    again = bridge.patch(f"/v1/vault/{made.json()['id']}", headers=AUTH, json={"agents": ["coach"]})
    assert again.status_code == 409
    none, _ = tool(bridge, COACH, "vault_contact_details", {})
    assert none.startswith("The person keeps no contact details for you")


def test_the_reply_of_the_turn_carries_the_entrys_chip(bridges, fakes):
    """The entry an agent saved (or used) in a turn: its hidden mark `<!--bip:vault <id> <label>-->` ends the turn's
    reply whatever the model wrote (made_files.rs), the apps' chip « 🔒 <label> ». The id and label only."""
    bridge = bridges.new()
    bridge.config["agents"]["Wellness"].update({"isolated": True, "hermes_home": "/home/hermes/.hermes",
                                                "python": f"{bridge.fakebin}/podman exec -i hermes-wellness python"})
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.down.url}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-wellness"], stdout=ANSWER)
    bridge.start()
    give_token(bridge)
    gate = threading.Event()
    reply = "C'est noté, j'ai gardé tes coordonnées pro dans ton coffre."
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_vault_vault_save_contact_details",
                                   preview="{}"),
                               gate, sse("message.delta", run_id="r1", delta=reply),
                               sse("run.completed", run_id="r1", output=reply)]
    posted = bridge.post("/hermes/wellness/v1/runs", json={"input": "Garde mon adresse pro", "session_id": "api_c"},
                         headers=HERMES_AUTH)
    assert posted.status_code == 202, posted.text
    assert wait_until(lambda: fakes.hermes.event_requests)
    said, error = tool(bridge, TOKEN, "vault_save_contact_details",
                       {"label": "Kerfy Studio (pro)", "full_name": "Camille Exemple", "emails": ["pro@example.org"]})
    assert not error, said
    [item] = bridge.get("/v1/vault", headers=AUTH).json()["items"]
    mark = f"<!--bip:vault {item['id']} Kerfy Studio (pro)-->"
    gate.set()
    with bridge.stream("GET", "/hermes/wellness/v1/runs/r1/events", headers=HERMES_AUTH) as resp:
        events = read_sse(resp)
    end = events[-1][1]
    assert events[-1][0] == "run.completed" and end["output"] == f"{reply}\n{mark}"
    streamed = "".join(d.get("delta", "") for n, d in events if n == "message.delta")
    assert streamed.endswith(f"\n\n{mark}") and "pro@example.org" not in mark
