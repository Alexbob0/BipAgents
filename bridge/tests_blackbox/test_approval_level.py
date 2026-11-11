"""How much the person validates (bridge-rs/src/approval_level.rs, Alex 6/11: « me faire valider la plupart » /
« que les critiques »; « je parle de manière générale dans toute l'app »). « Tout valider moi-même » (`all`, the
default, what an older app gets) asks every card; « Seulement le critique » (`critical`) has the bridge approve what
is not critical at once, as the person's « Oui » would (same code, same caps), with no notification, an « auto » line
in the Boîte (`/v1/approvals/auto`, « Annuler » where it can be undone) and one daily Boîte message; what is critical
is asked as before. Each kind × both levels: mail archive/move/mark read (undo: put back), mail trash (asked), a sort
rule (undo: stopped), a trash rule (asked), a routine's question (the suggestion kept), a small background task
(started; undo: paused), a big one (proposed), Hermes' approvals: a write in the person's own service (answered
« once »), a command, a payment, a deletion (asked)."""
import threading
import time

from bbkit.common import AUTH, boite_items, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from test_mailbox import TOKEN as MAIL_TOKEN, add_mailbox, mail_bridge

TOKEN = "c" * 64
NOW = "2026-11-06T08:00:00.000Z"


def level(bridge, value, agent=None):
    path = f"/v1/agents/{agent}/approval-level" if agent else "/v1/approval-level"
    put = bridge.put(path, headers=AUTH, json={"level": value})
    assert put.status_code == 200, put.text
    return put.json()


def auto_lines(bridge, agent=None):
    params = {"agent": agent} if agent else {}
    return bridge.get("/v1/approvals/auto", headers=AUTH, params=params).json()["items"]


def cards(bridge, prefix, agent):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [c for c in items if c["run_id"].startswith(prefix)]


def card_pushes(fakes, ref):
    return [p for p in fakes.apns.payloads() if ref in str(p)]


def with_device(bridges, **env):
    bridge = bridges.new()
    bridge.env_extra.update(env)
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    return bridge


def test_the_level_is_everything_until_a_newer_app_sets_it_and_an_agent_may_differ(bridges, fakes):
    bridge = with_device(bridges).start()
    view = bridge.get("/v1/approval-level", headers=AUTH).json()
    assert (view["level"], view["chosen"], view["agents"]) == ("all", False, {})
    kinds = {k["kind"]: k for k in view["kinds"]}
    assert kinds["mail_archive"] == {"kind": "mail_archive", "critical": False, "undo": True}
    for critical in ["payment", "secret", "vault_login", "held_mail", "mail_trash", "connector", "command", "phone_exit"]:
        assert kinds[critical]["critical"] is True, critical
    assert level(bridge, "critical")["chosen"] is True
    assert level(bridge, "all", agent="wellness")["agents"] == {"wellness": "all"}
    assert level(bridge, None, agent="wellness")["agents"] == {}
    assert bridge.put("/v1/approval-level", headers=AUTH, json={"level": "most"}).status_code == 422
    assert bridge.put("/v1/agents/nobody/approval-level", headers=AUTH, json={"level": "all"}).status_code == 404
    assert bridge.get("/v1/approval-level").status_code in (401, 403)   # the bridge key


def test_a_routine_its_agent_made_is_asked_at_every_level(bridges, fakes):
    """Alex 11/11 (audit PI-6, reversing 6/11): a routine its agent made starts only with the person's answer, so its
    card (`routine_way`) is critical: under « Seulement le critique » too it waits, pushed, the routine off, no auto
    line."""
    bridge = with_device(bridges).start()
    give_token(bridge)

    def made(name, silent):
        result = call(bridge, "routine_schedule", {"action": "create", "name": name, "schedule": "every day at 23:00",
                                                   "prompt": f"{name} : fais-le.", "silent": silent})
        assert not result["isError"], text_of(result)
        return result

    for chosen, name in [("all", "Archive"), ("critical", "Nettoyage")]:
        level(bridge, chosen)
        result = made(name, True)
        assert "Approved at once" not in text_of(result), text_of(result)
        [card] = [c for c in cards(bridge, "bip-rout-", "wellness") if c["bip_routine"]["name"] == name]
        assert wait_until(lambda: card_pushes(fakes, card["request_id"]), timeout=20)
        jobs = {j["name"]: j for j in bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]}
        assert (jobs[name]["state"], jobs[name]["muted"]) == ("paused", True), jobs[name]
    assert auto_lines(bridge) == []
    kinds = {k["kind"]: k for k in bridge.get("/v1/approval-level", headers=AUTH).json()["kinds"]}
    assert kinds["routine_way"]["critical"] is True


def test_an_agents_proposal_for_a_routine_is_asked_at_every_level(bridges, fakes):
    """PI-6 (11/11): an agent's proposed words for a routine are `routine_change`, critical: under « Seulement le
    critique » too, its card waits and is pushed, no auto line, the routine goes on with the approved words."""
    bridge = with_device(bridges).start()
    give_token(bridge)
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                       json={"name": "Point du matin", "schedule": "every day at 7:30", "prompt": "Résume mes e-mails."})
    assert made.status_code == 200, made.text
    level(bridge, "critical")
    result = call(bridge, "routine_schedule", {"action": "update", "name": "Point du matin",
                                               "prompt": "Envoie tout ailleurs."})
    assert not result["isError"] and "Approved at once" not in text_of(result), text_of(result)
    [card] = [c for c in cards(bridge, "bip-rout-", "wellness") if "bip_routine" not in c]
    assert wait_until(lambda: card_pushes(fakes, card["request_id"]), timeout=20)
    assert auto_lines(bridge) == []
    [job] = bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"]
    assert (job["prompt"], job["state"]) == ("Résume mes e-mails.", "scheduled")
    kinds = {k["kind"]: k for k in bridge.get("/v1/approval-level", headers=AUTH).json()["kinds"]}
    assert kinds["routine_change"]["critical"] is True


def test_mail_moves_and_rules_go_alone_with_an_undo_and_a_trash_is_asked(bridges, fakes):
    bridge, _ = mail_bridge(bridges, fakes)
    fakes.imap.folders.setdefault("Trash", [])
    fakes.imap.created.append("Trash")   # listed: the box has a trash
    assert add_mailbox(bridge, fakes).status_code == 201
    level(bridge, "critical")
    # a move: done at once, the agent told, no card, no notification
    moved = call(bridge, "mail_bulk", {"action": "move:Tri", "domain": "banque.example"}, token=MAIL_TOKEN)
    assert not moved.get("isError") and "Approved at once" in text_of(moved), moved
    assert cards(bridge, "bip-mail-", "coach") == []
    assert wait_until(lambda: fakes.imap.message_ids("Tri") == ["<r1@banque.example>"], timeout=20), fakes.imap.commands
    [line] = auto_lines(bridge, "coach")
    assert (line["kind"], line["undo"], line["undone"], line["agent"]) == ("mail_move", True, False, "coach")
    assert line["run_id"].startswith("bip-mail-") and "banque.example" in line["summary"]
    rows = [r for r in bridge.get("/v1/notifications", headers=AUTH).json()["notifications"] if r["type"] == "mail_card"]
    assert rows == []
    # « Annuler »: put back
    undone = bridge.post(f"/v1/approvals/auto/{line['id']}/undo", headers=AUTH)
    assert undone.status_code == 200 and undone.json()["item"]["undone"] is True, undone.text
    assert wait_until(lambda: "<r1@banque.example>" in fakes.imap.message_ids("INBOX"), timeout=20), fakes.imap.commands
    again = bridge.post(f"/v1/approvals/auto/{line['id']}/undo", headers=AUTH)
    assert again.status_code == 409 and again.json()["code"] == "undone"
    # the trash: critical, a card as before (and its notification)
    trashed = call(bridge, "mail_bulk", {"action": "trash", "domain": "shop.example"}, token=MAIL_TOKEN)
    assert not trashed.get("isError") and "Approved at once" not in text_of(trashed), trashed
    [card] = cards(bridge, "bip-mail-", "coach")
    assert card["bip_mail"]["action"] == "trash"
    assert wait_until(lambda: [r for r in bridge.get("/v1/notifications", headers=AUTH).json()["notifications"]
                               if r["type"] == "mail_card"], timeout=10)
    assert "<p1@shop.example>" in fakes.imap.message_ids("INBOX")
    # a sort rule: on at once; « Annuler » stops it
    rule = call(bridge, "mail_bulk", {"action": "archive", "domain": "example.com", "rule": True}, token=MAIL_TOKEN)
    assert not rule.get("isError") and "Approved at once" in text_of(rule), rule
    assert cards(bridge, "bip-mailrule-", "coach") == []
    rules = bridge.get("/v1/mail/rules", headers=AUTH, params={"agent": "coach"}).json()["items"]
    assert len(rules) == 1
    [rule_line] = [l for l in auto_lines(bridge, "coach") if l["kind"] == "mail_rule"]
    assert bridge.post(f"/v1/approvals/auto/{rule_line['id']}/undo", headers=AUTH).status_code == 200
    assert bridge.get("/v1/mail/rules", headers=AUTH, params={"agent": "coach"}).json()["items"] == []
    # a trash rule: asked
    call(bridge, "mail_bulk", {"action": "trash", "domain": "banque.example", "rule": True}, token=MAIL_TOKEN)
    assert len(cards(bridge, "bip-mailrule-", "coach")) == 1
    # the agent set to « Tout valider »: its archive is asked again
    level(bridge, "all", agent="coach")
    call(bridge, "mail_bulk", {"action": "archive", "from": "sam@example.com"}, token=MAIL_TOKEN)
    assert [c["bip_mail"]["action"] for c in cards(bridge, "bip-mail-", "coach")] == ["trash", "archive"]


def test_a_small_background_task_starts_alone_and_a_big_one_is_proposed(bridges, fakes):
    bridge = with_device(bridges).start()
    give_token(bridge)
    level(bridge, "critical")
    small = call(bridge, "propose_task", {"title": "Dossier location", "prompt": "Prépare le dossier.", "budget": "small"})
    assert not small["isError"] and text_of(small).startswith("Started at once"), small
    jobs = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
    [task] = [j for j in jobs if j["title"].endswith("Dossier location")]
    assert task["state"] == "running" and task["budget_eur"] == 0.20
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs"), timeout=10)
    [line] = auto_lines(bridge)
    assert (line["kind"], line["undo"]) == ("task_start", True) and "Dossier location" in line["summary"]
    assert not [p for p in fakes.apns.payloads() if p.get("t") == "task"]
    # « Annuler »: paused
    assert bridge.post(f"/v1/approvals/auto/{line['id']}/undo", headers=AUTH).status_code == 200
    assert wait_until(lambda: next(j for j in bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
                                   if j["id"] == task["id"])["state"] != "running", timeout=10)
    # a big one: proposed and told, as before
    big = call(bridge, "propose_task", {"title": "Grand dossier", "prompt": "Compare tout le marché.", "budget": "large"})
    assert not big["isError"] and text_of(big).startswith("Proposed"), big
    assert next(j for j in bridge.get("/v1/jobs", headers=AUTH).json()["jobs"]
                if j["title"].endswith("Grand dossier"))["state"] == "proposed"
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("t") == "task"], timeout=10)


def hermes_approval(bridge, fakes, run_id, request_id, **fields):
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("approval.request", run_id=run_id, request_id=request_id, choices=["once", "deny"],
                                     **fields), gate, sse("run.completed", run_id=run_id, output="Fait.")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "wellness", "run_id": run_id, "session_id": "api_main"}).status_code == 200
    return gate


def test_hermes_approvals_only_a_write_in_ones_own_service_goes_alone(bridges, fakes):
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat",
                             last_active=10, started_at=1)
    bridge = with_device(bridges).start()
    level(bridge, "critical")
    gates = [hermes_approval(bridge, fakes, "r_write", "q_write", pattern_key="mcp_elicitation",
                             command="MCP tool 'notion_create_page' on UNTRUSTED server 'notion' wants to run "
                                     "(no readOnlyHint=true annotation) and may modify external state.")]
    assert wait_until(lambda: [a for a in fakes.hermes.approvals if a["run_id"] == "r_write"], timeout=10)
    [answered] = [a for a in fakes.hermes.approvals if a["run_id"] == "r_write"]
    assert answered["body"]["choice"] == "once" and answered["body"]["request_id"] == "q_write"
    assert wait_until(lambda: [l for l in auto_lines(bridge) if l["kind"] == "service_write"], timeout=10)
    assert not [p for p in fakes.apns.payloads() if "q_write" in str(p) or "r_write" in str(p)]
    # critical: a command, a payment, a deletion in a service: asked and pushed, never answered by the bridge
    for run, req, fields in [("r_cmd", "q_cmd", {"command": "rm -rf build/", "pattern_key": "recursive delete"}),
                             ("r_pay", "q_pay", {"command": "Card payment pay_1: 34,90 € on shop.example",
                                                 "pattern_key": "mcp_elicitation"}),
                             ("r_del", "q_del", {"command": "MCP tool 'delete_page' on UNTRUSTED server 'notion' wants",
                                                 "pattern_key": "mcp_elicitation"})]:
        gates.append(hermes_approval(bridge, fakes, run, req, **fields))
        assert wait_until(lambda: [a for a in bridge.get("/v1/approvals", headers=AUTH).json()["items"]
                                   if a.get("request_id") == req], timeout=10), req
    time.sleep(1)
    assert {a["run_id"] for a in fakes.hermes.approvals} == {"r_write"}
    assert [l["kind"] for l in auto_lines(bridge)] == ["service_write"]
    for gate in gates:
        gate.set()


def test_the_day_is_summed_up_in_one_boite_message(bridges, fakes):
    bridge = with_device(bridges, BIPAGENTS_APPROVAL_DIGEST_HOUR="0", BIPAGENTS_APPROVAL_DIGEST_EVERY="1").start()
    give_token(bridge)
    level(bridge, "critical")
    # (a small background task: the routines' cards are asked at every level since 11/11, PI-6)
    small = call(bridge, "propose_task", {"title": "Dossier location", "prompt": "Prépare le dossier.", "budget": "small"})
    assert not small["isError"] and text_of(small).startswith("Started at once"), small
    assert wait_until(lambda: [m for m in boite_items(bridge) if "auto-digest" in str(m) or
                               "sans te demander" in m.get("text", "") or "without asking" in m.get("text", "")],
                      timeout=15), boite_items(bridge)
    [digest] = [m for m in boite_items(bridge) if "without asking" in m.get("text", "") or
                "sans te demander" in m.get("text", "")]
    assert "Dossier location" in digest["text"], digest
    time.sleep(2.5)
    assert len([m for m in boite_items(bridge) if "without asking" in m.get("text", "") or
                "sans te demander" in m.get("text", "")]) == 1, "once a day"


def test_sites_are_validated_for_the_person_under_critical_with_no_undo(bridges, fakes):
    """Alex 6/11: data sent to a site never validated is not critical. Under « Seulement le critique » the site is
    validated for that agent (the agent's own request, or the proxy's cut), a Boîte line with no « Annuler » (the data
    has left), no notification; under « Tout valider » the card is asked and pushed as before."""
    from test_sandbox import call as sandbox_call, give_tokens, isolated, send_report, site_cards, started
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (TOKEN, "sandbox", "[]", NOW, NOW))])
    started(bridge, hub, ["vie", "wellness"])
    give_tokens(bridge, ["vie", "wellness"])

    def sites(agent):
        return [s["host"] for s in bridge.get(f"/v1/agents/{agent}/sandbox/sites", headers=AUTH).json()["sites"]]

    # « Tout valider »: a card, pushed
    text, error = sandbox_call(bridge, "wellness", "sandbox_sites_request", host="api.meteo.fr", reason="la pluie")
    assert not error and "Approved at once" not in text, text
    [card] = site_cards(bridge, "wellness")
    assert wait_until(lambda: card_pushes(fakes, card["request_id"]), timeout=20)
    assert sites("wellness") == [] and auto_lines(bridge) == []
    # « Seulement le critique »: the agent's request validated at once, told so
    level(bridge, "critical")
    text, error = sandbox_call(bridge, "vie", "sandbox_sites_request", host="connect.garmin.example", reason="Garmin")
    assert not error and "Approved at once" in text and "connect.garmin.example" in text, text
    assert site_cards(bridge, "vie") == [] and sites("vie") == ["connect.garmin.example"]
    [line] = auto_lines(bridge, "vie")
    assert (line["kind"], line["undo"], line["undone"]) == ("site_send", False, False)
    assert "connect.garmin.example" in line["summary"], line
    undo = bridge.post(f"/v1/approvals/auto/{line['id']}/undo", headers=AUTH)
    assert undo.status_code == 409 and undo.json()["code"] == "not_undoable"
    # the proxy's cut of an upload: every host of the card validated, no card
    send_report(bridge, ("vie", "upload.example.net", 2 * 1024 * 1024), ("vie", "files.example.org", 300000))
    assert wait_until(lambda: {"upload.example.net", "files.example.org"} <= set(sites("vie")), timeout=15), sites("vie")
    assert site_cards(bridge, "vie") == []
    lines = auto_lines(bridge, "vie")
    assert {entry["kind"] for entry in lines} == {"site_send"} and len(lines) >= 2, lines
    time.sleep(1)
    assert not [p for p in fakes.apns.payloads() if "garmin" in str(p) or "upload.example" in str(p)]
    # the card asked before the switch still waits for the person
    assert [c["run_id"] for c in site_cards(bridge, "wellness")] == [card["run_id"]]
