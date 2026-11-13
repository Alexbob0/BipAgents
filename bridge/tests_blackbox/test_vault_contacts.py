"""« Coordonnées » in the vault (`/v1/vault`, kind `address`, and the agents' `vault_contact_details` /
`vault_save_contact_details` on `/v1/agent-tools/vault`): a name, an address, phones and e-mails with a label and a
hint, checked by the bridge, read back by the app to edit them, written into the Hermes vault of the agents the person
chose (for their browser fill), and read by an agent only when it was given the entry (an agent other people talk to:
only after the person's confirmation for it). All values are made up."""
import json
import threading

import yaml

from cryptography.fernet import Fernet

from bbkit.common import AUTH, app_turn, follow_run, sse
from bbkit.mcp import TOKEN, give_token
from bbkit.servers import wait_until
from conftest import wait_registered


VIE = "agent-token-for-vie-0123456789abcdef"
COACH = "agent-token-for-coach-0123456789abcd"
VAULT = "/v1/agent-tools/vault"
ANSWER = json.dumps({"id": "vault_1", "removed": True}) + "\n"
PRO = {"kind": "address", "label": "Kerfy Studio (pro)", "hint": "pour tout ce qui concerne la société",
       "full_name": "Camille Exemple", "address_line1": "1 rue de l'Essai", "address_line2": "Bât. B",
       "city": "Testville", "state": "", "postal_code": "00100", "country": "fr",
       "phones": ["+33 6 00 00 00 01", "01 00 00 00 02"], "emails": ["camille@example.org"]}


def browser(home, agent, on):
    """The agent's own browser in its Hermes profile (`home`/profiles/`agent`), on or off."""
    folder = home / "profiles" / agent
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "config.yaml").write_text("mcp_servers:\n  stealth:\n    url: http://127.0.0.1:9601/mcp\n"
                                        f"    enabled: {'true' if on else 'false'}\n")


def contacts_bridge(bridges, fakes, shared=False, vie_browser=True, vie_space=None):
    """Vie and Coach in their own containers; Vie's browser on unless `vie_browser` is False (its « Coordonnées »
    reach its Hermes vault only then); `vie_space`: Vie served from a space of its own (the app may then switch its
    browser)."""
    bridge = bridges.new()
    for name in ("vie", "coach"):
        bridge.config["agents"][name.capitalize()] = {
            "hermes_url": fakes.down.url, "hermes_key": "k" * 32, "isolated": True,
            "python": f"{bridge.fakebin}/podman exec -i hermes-{name} python", "hermes_home": "/home/hermes/.hermes"}
        bridge.commands.rule("podman", ["exec", "-i", f"hermes-{name}"], stdout=ANSWER)
    if vie_space is not None:
        bridge.config["agents"]["Vie"].update({"space": str(vie_space), "sandboxed": True})
        (bridge.data / "browsers" / "vie").mkdir(parents=True)          # set up before: switched on and off
        (bridge.data / "browsers" / "vie" / "ports.json").write_text('{"devtools": 9600, "vnc": 9601}')
    browser(vie_space if vie_space is not None else bridge.home / ".hermes", "vie", vie_browser)
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
    assert item["contact"] == {"full_name": "Camille Exemple", "birth_date": "", "address_line1": "1 rue de l'Essai",
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
    app_turn(bridge, fakes, "Garde mon adresse pro", "api_c")
    assert wait_until(lambda: fakes.hermes.event_requests)
    said, error = tool(bridge, TOKEN, "vault_save_contact_details",
                       {"label": "Kerfy Studio (pro)", "full_name": "Camille Exemple", "emails": ["pro@example.org"]})
    assert not error, said
    [item] = bridge.get("/v1/vault", headers=AUTH).json()["items"]
    mark = f"<!--bip:vault {item['id']} Kerfy Studio (pro)-->"
    gate.set()
    events = follow_run(bridge, "r1")
    end = events[-1][1]
    assert events[-1][0] == "run.completed" and end["output"] == f"{reply}\n{mark}"
    streamed = "".join(d.get("delta", "") for n, d in events if n == "message.delta")
    assert streamed.endswith(f"\n\n{mark}") and "pro@example.org" not in mark


HOME_ENTRY = {"kind": "address", "label": "Domicile", "full_name": "Camille Exemple", "address_line1": "3 rue du Test",
              "city": "Lyon", "postal_code": "69001", "country": "FR", "phones": ["04 00 00 00 09"]}


def turn_with_a_listing(bridge, fakes, run_id, reply):
    """A turn of Wellness that lists its « Coordonnées » (no label: what the bench's models did), then ends with
    `reply`: the end's output as the app gets it."""
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("tool.started", run_id=run_id, tool="mcp_bipagents_vault_vault_contact_details",
                                     preview="{}"),
                                 gate, sse("message.delta", run_id=run_id, delta=reply),
                                 sse("run.completed", run_id=run_id, output=reply)]
    seen = len(fakes.hermes.event_requests)
    assert app_turn(bridge, fakes, "Écris une lettre", f"api_{run_id}") == run_id
    assert wait_until(lambda: len(fakes.hermes.event_requests) > seen)
    listed, error = tool(bridge, TOKEN, "vault_contact_details", {})
    assert not error and "<!--bip:vault" not in listed, listed     # the listing itself carries no chip
    gate.set()
    events = follow_run(bridge, run_id)
    assert events[-1][0] == "run.completed"
    return events[-1][1]["output"]


def test_a_listing_used_in_the_reply_gets_the_chip_of_the_entry_used(bridges, fakes):
    """Model bench, 2026-10-29: GLM, DeepSeek and a local model listed the entries (no label) and wrote the letter with
    the address. The entries whose details the reply uses get their chip at the turn's end; never one it doesn't use."""
    bridge = bridges.new()
    bridge.config["agents"]["Wellness"].update({"isolated": True, "hermes_home": "/home/hermes/.hermes",
                                                "python": f"{bridge.fakebin}/podman exec -i hermes-wellness python"})
    bridge.config["hermes"] = {"isolated": True, "bin": str(bridge.fakebin / "hermes"), "hub_url": fakes.down.url}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-wellness"], stdout=ANSWER)
    bridge.start()
    give_token(bridge)
    pro = bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["wellness"]}).json()
    pro_mark = f"<!--bip:vault {pro['id']} Kerfy Studio (pro)-->"
    # one entry listed, the letter carries its address (another case, accents dropped, a non-breaking space)
    letter = "Kerfy Studio\n1 RUE DE L'ESSAI, Bat. B\n00100 Testville\n\nObjet : résiliation"
    assert turn_with_a_listing(bridge, fakes, "r1", letter) == f"{letter}\n{pro_mark}"
    # two entries listed, the reply uses the home one (its phone, written otherwise): its chip only
    home = bridge.post("/v1/vault", headers=AUTH, json={**HOME_ENTRY, "agents": ["wellness"]}).json()
    home_mark = f"<!--bip:vault {home['id']} Domicile-->"
    call = "Je t'ai noté de rappeler le 04.00.00.00.09 depuis la maison."
    assert turn_with_a_listing(bridge, fakes, "r2", call) == f"{call}\n{home_mark}"
    # listed, then a reply with none of their details: no chip, the reply as it was
    plain = "Tu as deux adresses dans ton coffre : laquelle je mets sur la lettre ?"
    assert turn_with_a_listing(bridge, fakes, "r3", plain) == plain


def test_contact_details_reach_the_agents_hermes_vault_only_while_its_browser_is_on(bridges, fakes, tmp_path):
    """Each entry in an agent's Hermes vault makes Hermes show its five `browser_vault_*` tools on every call (≈ 1 220
    tokens): an entry is written there only while the agent's browser is on, taken out when it is turned off or the
    entry unshared. The agent reads it from the bridge either way; a login keeps going into Hermes."""
    space = tmp_path / "spaces" / "vie"
    bridge = contacts_bridge(bridges, fakes, vie_browser=False, vie_space=space)
    wait_registered(bridge, ["vie"])     # its tools written into its profile first: the switches then write theirs
    give_token(bridge, "vie", VIE)       # the test's token, after the bridge's own
    made =bridge.post("/v1/vault", headers=AUTH, json={**PRO, "agents": ["vie"]})
    assert made.status_code == 201, made.text
    item = made.json()
    assert hermes_requests(bridge, "vie") == []                      # its browser is off
    assert [r["agents"] for r in records(bridge)] == [{"vie": "bridge"}]
    read, error = tool(bridge, VIE, "vault_contact_details", {"label": "Kerfy Studio (pro)"})
    assert not error and "Camille Exemple" in read
    bridge.post("/v1/vault", headers=AUTH, json={"label": "SNCF", "site": "sncf-connect.com", "identifier": "a@example.org",
                                                  "password": "s3cret-Pass!", "agents": ["vie"]})
    assert [r["kind"] for r in hermes_requests(bridge, "vie")] == ["login"]
    bridge.commands.clear()
    # the person turns its browser on: the entry is written into its Hermes vault
    on = bridge.patch("/v1/agents/vie/settings", headers=AUTH, json={"browser": True})
    assert on.status_code == 200, on.text
    [added] = hermes_requests(bridge, "vie")
    assert added["op"] == "add" and added["kind"] == "address" and added["secret"]["city"] == "Testville"
    assert records(bridge)[0]["agents"] == {"vie": "vault_1"}
    bridge.commands.clear()
    # and off: taken out of it, kept in the bridge, the login left
    off = bridge.patch("/v1/agents/vie/settings", headers=AUTH, json={"browser": False})
    assert off.status_code == 200, off.text
    assert [(r["op"], r["id"]) for r in hermes_requests(bridge, "vie")] == [("remove", "vault_1")]
    assert [r["agents"]["vie"] for r in records(bridge)] == ["bridge", "vault_1"]
    read, error = tool(bridge, VIE, "vault_contact_details", {})
    assert not error and "Kerfy Studio (pro)" in read
    # on again, then unshared: removed from Hermes
    bridge.patch("/v1/agents/vie/settings", headers=AUTH, json={"browser": True})
    bridge.commands.clear()
    assert bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"agents": []}).status_code == 200
    assert [r["op"] for r in hermes_requests(bridge, "vie")] == ["remove"]


def test_a_birth_date_is_kept_read_by_the_agent_and_never_put_in_the_browser_fill(bridges, fakes):
    """Alex 13/11: a pharmacy's sign-up form required a birth date and the agent made data up. Optional, « YYYY-MM-DD »
    from the apps (a real day from 1900 to today, "" clears it); the agent reads it with its other details and types
    it; its Hermes vault (the browser fill, which has no such field) gets the name, the phone and the e-mail only."""
    bridge = contacts_bridge(bridges, fakes)
    made = bridge.post("/v1/vault", headers=AUTH, json={**PRO, "birth_date": "1991-03-14", "agents": ["vie"]})
    assert made.status_code == 201, made.text
    item = made.json()
    assert item["contact"]["birth_date"] == "1991-03-14"
    [request] = hermes_requests(bridge, "vie")
    assert "birth_date" not in request["secret"]
    # the phone reaches the browser fill (its first one), as the e-mail and the name
    assert (request["secret"]["phone"], request["secret"]["email"]) == ("+33 6 00 00 00 01", "camille@example.org")
    used, error = tool(bridge, VIE, "vault_contact_details", {"label": "Kerfy Studio (pro)"})
    assert not error and "birth date (YYYY-MM-DD): 1991-03-14" in used and "phones: +33 6 00 00 00 01" in used
    for bad in ("14/03/1991", "1991-02-30", "1899-12-31", "2999-01-01", "1991-3-14"):
        resp = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"birth_date": bad})
        assert resp.status_code == 400 and "birth date" in resp.json()["detail"], (bad, resp.text)
    cleared = bridge.patch(f"/v1/vault/{item['id']}", headers=AUTH, json={"birth_date": ""})
    assert cleared.status_code == 200 and cleared.json()["contact"]["birth_date"] == ""
    assert cleared.json()["contact"]["city"] == "Testville"
    # one the person gives the agent in the conversation: kept in an entry of the agent's
    saved, error = tool(bridge, VIE, "vault_save_contact_details",
                        {"label": "Naissance", "full_name": "Camille Exemple", "birth_date": "1991-03-14"})
    assert not error, saved
    mine = [i for i in bridge.get("/v1/vault", headers=AUTH).json()["items"] if i["label"] == "Naissance"]
    assert mine and mine[0]["contact"]["birth_date"] == "1991-03-14"
    # the agents' tools tell to never make a detail up
    listing = bridge.post(VAULT, json={"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                          headers={"Authorization": f"Bearer {VIE}"}).json()["result"]["tools"]
    read = next(t for t in listing if t["name"] == "vault_contact_details")
    assert "Never make a detail up" in read["description"] and "birth" in read["description"]
    create = next(t for t in listing if t["name"] == "vault_create_login")
    assert "you never type a password yourself, not even a temporary one" in create["description"]


def test_a_browsing_agent_has_its_account_and_details_tools_in_view(bridges, fakes, tmp_path):
    """13/11: behind Hermes' tool search, no « Créer un compte » card (the agent typed its own password, or asked the
    person to type it) and the person's details looked for in the Carnet until the brake stopped the turn. An agent
    with its browser that may keep logins has them in view (its profile's `tools.tool_search.defer`)."""
    space = tmp_path / "spaces" / "vie"
    bridge = contacts_bridge(bridges, fakes, vie_space=space)
    wait_registered(bridge, ["vie"])
    config = space / "profiles" / "vie" / "config.yaml"

    def defer():
        data = yaml.safe_load(config.read_text()) or {}
        return ((data.get("tools") or {}).get("tool_search") or {}).get("defer") or []
    assert wait_until(lambda: "!mcp__bipagents_vault__vault_create_login" in defer(), timeout=15), defer()
    for name in ("vault_create_login", "vault_confirm_login", "vault_contact_details", "vault_save_contact_details"):
        assert f"!mcp__bipagents_vault__{name}" in defer(), name
    assert "!mcp__stealth__type_text" in defer()
    # and a drop-down list's choice (select-option-direct, 13/11: behind the search, the agents typed into the
    # birth date's <select>s or read the page with a value)
    assert "!mcp__stealth__select_option" in defer()
