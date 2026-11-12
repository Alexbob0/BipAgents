"""What an agent asks while nobody is there, and « Toujours » (Alex 12/11: a night routine that tidies the mailbox
never moved anything — nobody approves at 3 a.m., the card expired after an hour, the work was lost).

- A turn nobody follows (a routine's run…) never waits inside Hermes: its approval becomes a bridge card
  (`bip-held-`, held_approvals.rs) kept after the turn ends and across a restart; Hermes is told and answered at
  once. The person's « Oui » later: the agent goes on in that conversation and the identical approval is answered
  « once » by the bridge; the routine's report in the Boîte says what was done. A question is answered at once.
- A mail card made by a routine: kept, done by the bridge on « Oui » hours later, the routine's report after it.
- « Toujours autoriser … » (approval_level.rs): offered on a mail move, never on a trash; once given, the next move
  goes alone with a « toujours autorisé » line; taken back from the agent's settings, asked again. A Hermes write in
  one service: per service, Hermes itself only ever answered « once »."""
import threading
import time

from bbkit.common import AUTH, boite_items, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered
from test_mailbox import TOKEN as MAIL_TOKEN, add_mailbox, mail_bridge

DEVICE = "c" * 64
NOW = "2026-11-12T03:00:00.000Z"
COMMAND = "rm -rf ~/Mail/vieux-brouillons"


def bridge_with_device(bridges, fakes):
    """A managed install (its cards are placed and resumed: card_place.rs `watch`) with one iPhone, agent « wellness »."""
    bridge = bridges.new()
    folder = profile(managed_install(bridge, fakes, agents=("wellness",)), "wellness")
    (folder / "config.yaml").write_text(f"model:\n  provider: custom\n  default: glm-5.3-flash\n"
                                        f"  base_url: {fakes.model.url}/v1\n  key_env: CUSTOM_MODEL_API_KEY\n")
    (folder / ".env").write_text("CUSTOM_MODEL_API_KEY=sk-test-key\n")
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat", agent="wellness")
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (DEVICE, "sandbox", "[]", NOW, NOW))])
    bridge.start()
    wait_registered(bridge, ["wellness"])
    return bridge


def approvals(bridge, agent="wellness"):
    return bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]


def held_cards(bridge, agent="wellness"):
    return [c for c in approvals(bridge, agent) if c["run_id"].startswith("bip-held-")]


def routine(bridge, name, agent="wellness"):
    made = bridge.post(f"/v1/agents/{agent}/routines", headers=AUTH,
                       json={"name": name, "schedule": "every day at 03:00", "prompt": "Range ma boîte mail."})
    assert made.status_code in (200, 201), made.text
    return made.json()["job"]


def run_now(bridge, job):
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200


def last_status(bridge, agent="wellness"):
    return bridge.get(f"/v1/agents/{agent}/routines", headers=AUTH).json()["jobs"][0]["last_status"]


def routine_turns(fakes):
    return [r for r, v in fakes.hermes.started.items() if "<!--bip:routine" in (v.get("input") or "")]


def resumed_turns(fakes):
    return [r for r, v in fakes.hermes.started.items() if "<!--bip:card" in (v.get("input") or "")]


def answered(fakes, run):
    return [a["body"]["choice"] for a in fakes.hermes.approvals if a["run_id"] == run]


def ask(run, request, command=COMMAND, **extra):
    return sse("approval.request", run_id=run, request_id=request, choices=["once", "session", "always", "deny"],
               command=command, description="recursive delete", pattern_key="recursive delete", **extra)


def test_a_routines_approval_is_kept_past_its_turn_and_done_after_a_late_oui(bridges, fakes):
    bridge = bridge_with_device(bridges, fakes)
    hermes = fakes.hermes
    job = routine(bridge, "Ménage de nuit")
    denied = threading.Event()
    hermes.runs["r1"] = [ask("r1", "q1"), denied, sse("run.completed", run_id="r1", output="Ça attend ton accord.")]
    hermes.run_outputs["r1"] = "Ça attend ton accord pour effacer les vieux brouillons."
    run_now(bridge, job)
    assert wait_until(lambda: routine_turns(fakes) == ["r1"], timeout=20)
    # Hermes answered at once (never an hour of waiting), told first in a hidden note why
    assert wait_until(lambda: answered(fakes, "r1") == ["deny"], timeout=20), fakes.hermes.approvals
    [steer] = [s for s in hermes.steers if s["run_id"] == "r1"]
    assert "<!--bip:held" in str(steer["body"]) and "not refused" in str(steer["body"])
    denied.set()
    # one card of the bridge's own, kept after the turn ended; one push for it, not Hermes' approval push
    [card] = held_cards(bridge)
    assert card["command"] == COMMAND and card["choices"] == ["once", "deny"] and card["bip_held"]["kind"] == "command"
    assert "bip_always" not in card, "a command is critical: never « Toujours »"
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if card["request_id"] in str(p)], timeout=10)
    assert not [p for p in fakes.apns.payloads() if "q1" in str(p)]
    assert wait_until(lambda: last_status(bridge), timeout=30)
    time.sleep(1.0)
    # still there after its turn, and after a restart (the night goes by)
    assert [c["run_id"] for c in held_cards(bridge)] == [card["run_id"]]
    bridge.restart()
    assert [c["run_id"] for c in held_cards(bridge)] == [card["run_id"]]
    # the morning « Oui »: the agent goes on in the routine's conversation; the same approval answered by the bridge
    hermes.runs["r2"] = [ask("r2", "q2"), 2.0, sse("run.completed", run_id="r2", output="")]
    hermes.run_outputs["r2"] = "C'est fait : les vieux brouillons sont effacés."
    yes = bridge.post("/v1/approve", headers=AUTH,
                      json={"agent": "wellness", "run_id": card["run_id"], "request_id": card["request_id"], "choice": "once"})
    assert yes.status_code == 200 and yes.json()["choice"] == "once", yes.text
    assert held_cards(bridge) == []
    assert wait_until(lambda: resumed_turns(fakes) == ["r2"], timeout=30), list(hermes.started)
    note = hermes.started["r2"]["input"]
    assert COMMAND in note and "ALLOWED" in note and "Ménage de nuit" in note, note
    assert hermes.started["r2"]["session_id"] == hermes.started["r1"]["session_id"]
    assert wait_until(lambda: answered(fakes, "r2") == ["once"], timeout=20), fakes.hermes.approvals
    # its report in the Boîte, as the routine's
    assert wait_until(lambda: [i for i in boite_items(bridge) if "brouillons sont effacés" in i["text"]], timeout=30)
    [report] = [i for i in boite_items(bridge) if "brouillons sont effacés" in i["text"]]
    assert report["title"] == "Ménage de nuit"
    # the grant was used once: the same command asked again is held again, never done alone
    assert answered(fakes, "r2") == ["once"]


def test_a_refused_held_card_is_told_and_nothing_is_done(bridges, fakes):
    bridge = bridge_with_device(bridges, fakes)
    hermes = fakes.hermes
    job = routine(bridge, "Ménage")
    hermes.runs["r1"] = [ask("r1", "q1"), 1.0, sse("run.completed", run_id="r1", output="")]
    run_now(bridge, job)
    assert wait_until(lambda: held_cards(bridge), timeout=30)
    [card] = held_cards(bridge)
    hermes.runs["r2"] = [ask("r2", "q2"), 1.0, sse("run.completed", run_id="r2", output="")]
    no = bridge.post("/v1/approve", headers=AUTH,
                     json={"agent": "wellness", "run_id": card["run_id"], "request_id": card["request_id"], "choice": "deny"})
    assert no.status_code == 200 and no.json()["choice"] == "deny", no.text
    assert wait_until(lambda: resumed_turns(fakes) == ["r2"], timeout=30)
    assert "REFUSED" in hermes.started["r2"]["input"]
    # its repeat in that turn is not approved by the bridge: held again for the person, never « once »
    time.sleep(3)
    assert "once" not in answered(fakes, "r2")


def test_a_question_in_a_routine_is_answered_at_once(bridges, fakes):
    bridge = bridge_with_device(bridges, fakes)
    hermes = fakes.hermes
    job = routine(bridge, "Tri du soir")
    hermes.runs["r1"] = [sse("clarify.request", run_id="r1", request_id="c1",
                             questions=[{"id": "q1", "question": "Je déplace les 40 newsletters ?", "choices": ["Oui", "Non"]}]),
                         2.0, sse("run.completed", run_id="r1", output="")]
    run_now(bridge, job)
    assert wait_until(lambda: [a for a in hermes.answers if a["run_id"] == "r1"], timeout=20)
    [said] = [a for a in hermes.answers if a["run_id"] == "r1"]
    assert said["body"]["answer"].startswith("Nobody can answer now"), said
    time.sleep(1)
    assert not [p for p in fakes.apns.payloads() if "c1" in str(p)], "no question pushed"


def test_a_watched_turns_approval_keeps_todays_way(bridges, fakes):
    bridge = bridge_with_device(bridges, fakes)
    hermes = fakes.hermes
    gate = threading.Event()
    hermes.runs["r_live"] = [ask("r_live", "q_live"), gate, sse("run.completed", run_id="r_live", output="Fait.")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": "wellness", "run_id": "r_live", "session_id": "api_main"}).status_code == 200
    assert wait_until(lambda: [a for a in approvals(bridge) if a.get("request_id") == "q_live"], timeout=10)
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if "q_live" in str(p)], timeout=10)
    time.sleep(1)
    assert answered(fakes, "r_live") == [] and held_cards(bridge) == []
    gate.set()


def mail_routine_bridge(bridges, fakes):
    bridge, _ = mail_bridge(bridges, fakes)
    assert add_mailbox(bridge, fakes).status_code == 201
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Bonjour"}], title="Bot Chat", agent="coach")
    return bridge


def mail_cards(bridge):
    return [c for c in approvals(bridge, "coach") if c["run_id"].startswith("bip-mail-")]


def test_a_routines_mail_move_waits_all_night_and_is_done_on_the_mornings_oui(bridges, fakes):
    bridge = mail_routine_bridge(bridges, fakes)
    hermes = fakes.hermes
    hermes.rounds = 10_000   # the night's run goes on until its card is made
    job = routine(bridge, "Tri de nuit", agent="coach")
    gate = threading.Event()
    hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_mail_bulk", preview=""), gate,
                         sse("run.completed", run_id="r1", output="")]
    run_now(bridge, job)
    assert wait_until(lambda: hermes.seen("/p/coach/v1/runs/r1/events") or hermes.seen("/v1/runs/r1/events"), timeout=20)
    time.sleep(0.5)
    made = call(bridge, "mail_bulk", {"action": "move:Banque", "domain": "banque.example"}, token=MAIL_TOKEN)
    assert not made.get("isError") and "Approved at once" not in text_of(made), made
    [card] = mail_cards(bridge)
    # « Toujours » offered on a move, in the agent's words
    assert card["bip_always_kind"] == "mail_move" and card["bip_always"], card
    hermes.run_outputs["r1"] = "Une carte t'attend pour ranger les e-mails de la banque."
    hermes.rounds = 1   # the night's turn ends with its report
    gate.set()
    assert wait_until(lambda: last_status(bridge, "coach") == "sent", timeout=30), last_status(bridge, "coach")
    time.sleep(1)
    assert [c["run_id"] for c in mail_cards(bridge)] == [card["run_id"]], "kept after the night's turn"
    assert "<r1@banque.example>" in fakes.imap.message_ids("INBOX")
    # the morning « Oui »: the bridge moves them, the agent tells it as the routine's report
    hermes.rounds = 3
    hermes.run_outputs["r2"] = "C'est rangé : les e-mails de la banque sont dans « Banque »."
    yes = bridge.post("/v1/approve", headers=AUTH,
                      json={"agent": "coach", "run_id": card["run_id"], "request_id": card["request_id"], "choice": "once"})
    assert yes.status_code == 200, yes.text
    assert wait_until(lambda: fakes.imap.message_ids("Banque") == ["<r1@banque.example>"], timeout=20), fakes.imap.commands
    assert wait_until(lambda: resumed_turns(fakes) == ["r2"], timeout=30), list(hermes.started)
    assert "don't do it again" in hermes.started["r2"]["input"] and "Tri de nuit" in hermes.started["r2"]["input"]
    assert wait_until(lambda: [i for i in boite_items(bridge, "coach") if "C'est rangé" in i["text"]], timeout=30)
    [report] = [i for i in boite_items(bridge, "coach") if "C'est rangé" in i["text"]]
    assert report["title"] == "Tri de nuit"


def test_toujours_lets_mail_moves_go_alone_until_taken_back_and_never_a_trash(bridges, fakes):
    bridge = mail_routine_bridge(bridges, fakes)
    fakes.imap.folders.setdefault("Trash", [])
    fakes.imap.created.append("Trash")
    made = call(bridge, "mail_bulk", {"action": "move:Banque", "domain": "banque.example"}, token=MAIL_TOKEN)
    assert "Approved at once" not in text_of(made), made
    [card] = mail_cards(bridge)
    assert card["bip_always"].startswith(("Toujours autoriser", "Always let")), card
    yes = bridge.post("/v1/approve", headers=AUTH,
                      json={"agent": "coach", "run_id": card["run_id"], "request_id": card["request_id"], "choice": "always"})
    assert yes.status_code == 200, yes.text
    assert wait_until(lambda: fakes.imap.message_ids("Banque") == ["<r1@banque.example>"], timeout=20)
    level = bridge.get("/v1/approval-level", headers=AUTH).json()
    [granted] = level["always"]["coach"]
    assert granted["kind"] == "mail_move" and granted["label"], level
    assert level["level"] == "all", "the level itself unchanged"
    # the next move: alone, its Boîte line says « toujours autorisé », no card, no notification
    again = call(bridge, "mail_bulk", {"action": "move:Achats", "domain": "shop.example"}, token=MAIL_TOKEN)
    assert "Approved at once" in text_of(again), again
    assert mail_cards(bridge) == []
    assert wait_until(lambda: "<p1@shop.example>" in fakes.imap.message_ids("Achats"), timeout=20), fakes.imap.commands
    [line] = bridge.get("/v1/approvals/auto", headers=AUTH, params={"agent": "coach"}).json()["items"]
    assert line["always"] is True and line["undo"] is True and line["kind"] == "mail_move"
    assert line["summary"].endswith(("(toujours autorisé)", "(always allowed)")), line
    # a trash: still asked, never « Toujours » on it
    call(bridge, "mail_bulk", {"action": "trash", "from": "sam@example.com"}, token=MAIL_TOKEN)
    [trash] = mail_cards(bridge)
    assert trash["bip_mail"]["action"] == "trash" and "bip_always" not in trash
    # a « Toujours » answer on a trash card is an ordinary « Oui », never a « Toujours » for trashing
    bridge.post("/v1/approve", headers=AUTH,
                json={"agent": "coach", "run_id": trash["run_id"], "request_id": trash["request_id"], "choice": "always"})
    assert [k["kind"] for k in bridge.get("/v1/approval-level", headers=AUTH).json()["always"]["coach"]] == ["mail_move"]
    # taken back from the agent's settings: asked again
    gone = bridge.delete("/v1/agents/coach/approval-always/mail_move", headers=AUTH)
    assert gone.status_code == 200 and "coach" not in gone.json()["always"], gone.text
    assert bridge.delete("/v1/agents/coach/approval-always/mail_move", headers=AUTH).status_code == 404
    asked = call(bridge, "mail_bulk", {"action": "move:Banque2", "domain": "example.com"}, token=MAIL_TOKEN)
    assert "Approved at once" not in text_of(asked), asked
    assert len(mail_cards(bridge)) == 1


def test_toujours_on_a_service_write_is_per_service_and_hermes_only_hears_once(bridges, fakes):
    bridge = bridge_with_device(bridges, fakes)
    hermes = fakes.hermes
    write = "MCP tool 'notion_create_page' on UNTRUSTED server '{s}' wants to run (no readOnlyHint=true annotation)."

    def watched(run, request, server):
        gate = threading.Event()
        hermes.runs[run] = [sse("approval.request", run_id=run, request_id=request, choices=["once", "deny"],
                                command=write.format(s=server), pattern_key="mcp_elicitation"), gate,
                            sse("run.completed", run_id=run, output="Fait.")]
        assert bridge.post("/v1/watch", headers=AUTH,
                           json={"agent": "wellness", "run_id": run, "session_id": "api_main"}).status_code == 200
        return gate

    gates = [watched("r_a", "q_a", "notion")]
    assert wait_until(lambda: [a for a in approvals(bridge) if a.get("request_id") == "q_a"], timeout=10)
    [first] = [a for a in approvals(bridge) if a.get("request_id") == "q_a"]
    assert first["bip_always_kind"] == "service_write:notion" and "Notion" in first["bip_always"], first
    yes = bridge.post("/v1/approve", headers=AUTH,
                      json={"agent": "wellness", "run_id": "r_a", "request_id": "q_a", "choice": "always"})
    assert yes.status_code == 200, yes.text
    assert answered(fakes, "r_a") == ["once"], "Hermes' own « always » never sent"
    # the same service: alone; another one: asked
    gates.append(watched("r_b", "q_b", "notion"))
    assert wait_until(lambda: answered(fakes, "r_b") == ["once"], timeout=10)
    gates.append(watched("r_c", "q_c", "airtable"))
    assert wait_until(lambda: [a for a in approvals(bridge) if a.get("request_id") == "q_c"], timeout=10)
    time.sleep(1)
    assert answered(fakes, "r_c") == []
    lines = bridge.get("/v1/approvals/auto", headers=AUTH, params={"agent": "wellness"}).json()["items"]
    assert [(l["kind"], l["always"]) for l in lines] == [("service_write", True)]
    for gate in gates:
        gate.set()
