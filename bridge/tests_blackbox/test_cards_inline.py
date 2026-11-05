"""Every card that waits for the person shows in the conversation where the agent asked, not only in the Boîte
(bridge-rs/src/card_place.rs `watch`, `cards_in`). Bug of 2026-11-04/05 (Alex, iPhone and Android): an agent asked for
sites and then for an access in a secret card; neither showed in the conversation (« Je te demande un accès… » for
minutes), and an old card dated 28/10 was drawn at the end of the Discussion.

- A card made while the tool call still runs (`ask_secret` waits for the answer, a site asked through the sandbox's
  tools) is in the turn's conversation's `pending.approvals` within a second, with the Boîte's row pointing there.
- Answered through its own route (the secure field, the sites' screen): gone from the conversation (its version moves)
  and from the Boîte, its notification read.
- A card made before placements existed (never placed) or waiting for days stays in the Boîte only."""
import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor

from bbkit.common import AUTH, sse
from bbkit.mcp import call as mcp_call, text_of
from bbkit.servers import wait_until
from test_connectors import setup as connectors_setup, tool as connectors_tool, cards as connectors_cards
from test_mail_cards import mail_cards, pending as mail_pending, thread_of
from test_mailbox import TOKEN as MAIL_TOKEN, add_mailbox, mail_bridge
from test_sandbox import call as sandbox_call, give_tokens, isolated, site_cards, started


def watched(bridge, fakes, run_id, session, agent, tool="mcp_bipagents_connectors_ask_secret"):
    """A run of `agent` in `session` the bridge follows, in a tool call until the gate opens."""
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [sse("tool.started", run_id=run_id, tool=tool, preview="x"), gate,
                                 sse("run.completed", run_id=run_id, output="C'est fait.")]
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": agent, "run_id": run_id, "session_id": session}).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen(f"/p/{agent}/v1/runs/{run_id}/events"))
    time.sleep(0.3)
    return gate


def page(bridge, thread, agent):
    return bridge.get(f"/v1/threads/{thread}/timeline", headers=AUTH, params={"agent": agent}).json()


def waiting_in(bridge, thread, agent, prefix):
    return [a for a in page(bridge, thread, agent)["pending"]["approvals"] or [] if a["run_id"].startswith(prefix)]


def journal(bridge):
    listed = bridge.get("/v1/notifications", headers=AUTH, params={"limit": 100})
    assert listed.status_code == 200, listed.text   # the Boîte reads it first: never an error (Android hid every card)
    return listed.json()["notifications"]


def test_a_secret_card_shows_in_the_conversation_while_the_agent_waits_and_goes_once_typed(bridges, fakes):
    bridge, _, _ = connectors_setup(bridges, fakes)
    fakes.hermes.add_session("api_s", [{"role": "user", "content": "Envoie-moi une carte pour mon identifiant"}],
                             title="Accès")
    gate = watched(bridge, fakes, "run_s", "api_s", "vie")
    thread = thread_of(bridge, "api_s", agent="vie")
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(connectors_tool, bridge, "ask_secret",
                            {"name": "exemple", "site": "exemple.com", "kind": "password", "reason": "Pour ton compte"})
        assert wait_until(lambda: connectors_cards(bridge, "bip-secret-"), timeout=20)
        [card] = connectors_cards(bridge, "bip-secret-")
        # while the tool call still waits: in the conversation that asked, the Boîte's row opening it
        assert wait_until(lambda: waiting_in(bridge, thread, "vie", "bip-secret-"), timeout=10), page(bridge, thread, "vie")
        [inline] = waiting_in(bridge, thread, "vie", "bip-secret-")
        assert inline["request_id"] == card["request_id"] and inline["id"].startswith("a_")
        assert isinstance(inline["created_at"], (int, float))   # drawn at its own time
        assert connectors_cards(bridge, "bip-secret-")[0]["session_id"] == "api_s"
        main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "vie"}).json()["id"]
        assert main == thread or waiting_in(bridge, main, "vie", "bip-secret-") == []
        assert [r for r in journal(bridge) if r["type"] == "secret"], journal(bridge)
        before = page(bridge, thread, "vie")["version"]
        # typed in the card (its own route, not /v1/approve): gone from the conversation and the Boîte
        sent = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
        assert sent.status_code == 200, sent.text
        assert not asked.result(timeout=60).get("isError")
    assert connectors_cards(bridge, "bip-secret-") == []
    assert wait_until(lambda: page(bridge, thread, "vie")["version"] > before, timeout=10)
    assert waiting_in(bridge, thread, "vie", "bip-secret-") == []
    row = [r for r in journal(bridge) if r["type"] == "secret"][0]
    assert wait_until(lambda: [r for r in journal(bridge) if r["type"] == "secret"][0]["read_at"], timeout=10), row
    gate.set()


def test_a_site_asked_from_a_background_task_shows_where_the_task_reports(bridges, fakes):
    """« Les 4 sites d'un coup », asked by a task installing a connector: in the task's conversation and in the one
    it was asked from, answered there, gone from both."""
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    give_tokens(bridge, ["vie"])
    from test_sandbox import TOKENS
    hermes = fakes.hermes
    hermes.add_session("api_a", [{"role": "user", "content": "Installe Garmin en arrière-plan"}], title="Garmin")
    asking = watched(bridge, fakes, "run_a", "api_a", "vie", tool="mcp_bipagents_start_task")
    task_gate = threading.Event()
    hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp_bipagents_files_sandbox_sites_request", preview="x"),
                         task_gate, sse("run.completed", run_id="r1", output="Sites demandés.")]
    begun = mcp_call(bridge, "start_task", {"title": "Connecteur Garmin", "instructions": "Installe le connecteur"},
                     token=TOKENS["vie"])
    assert not begun["isError"], text_of(begun)
    asking.set()
    assert wait_until(lambda: hermes.seen("/p/vie/v1/runs/r1/events"), timeout=20)
    time.sleep(0.5)
    for host in ("connect.garmin.example", "sso.garmin.example"):
        text, error = sandbox_call(bridge, "vie", "sandbox_sites_request", host=host, reason="le connecteur Garmin")
        assert not error, text
    [card] = site_cards(bridge, "vie")
    assert card["bip_sites"] == ["connect.garmin.example", "sso.garmin.example"]
    asked_in = thread_of(bridge, "api_a", agent="vie")
    task_session = bridge.get("/v1/jobs", headers=AUTH).json()["jobs"][0]["session_id"]
    task_thread = thread_of(bridge, task_session, agent="vie")
    assert wait_until(lambda: waiting_in(bridge, asked_in, "vie", "bip-site-"), timeout=10)
    assert [a["run_id"] for a in waiting_in(bridge, task_thread, "vie", "bip-site-")] == [card["run_id"]]
    assert site_cards(bridge, "vie")[0]["session_id"] == "api_a"
    before = page(bridge, asked_in, "vie")["version"]
    # « Autoriser » in the conversation: the sites' own route with the hosts shown, as the Boîte answers it
    answered = bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": True, "hosts": card["bip_sites"]})
    assert answered.status_code == 200, answered.text
    assert wait_until(lambda: page(bridge, asked_in, "vie")["version"] > before, timeout=10)
    assert site_cards(bridge, "vie") == []
    assert waiting_in(bridge, asked_in, "vie", "bip-site-") == [] and waiting_in(bridge, task_thread, "vie", "bip-site-") == []
    task_gate.set()


def test_a_card_made_before_placements_or_waiting_for_days_stays_in_the_boite_only(bridges, fakes):
    """Alex's iPhone, 5/11: a card dated 28/10 drawn at the end of the Discussion. A card never placed, or one waiting
    for days, is in the Boîte, not in any conversation; a fresh one with no turn (a routine run by Hermes' cron) is in
    the main one."""
    bridge, _ = mail_bridge(bridges, fakes)
    assert add_mailbox(bridge, fakes).status_code == 201
    for action, sender in (("move:Tri", "banque.example"), ("archive", "shop.example")):
        made = mcp_call(bridge, "mail_bulk", {"action": action, "domain": sender}, token=MAIL_TOKEN)
        assert not made.get("isError"), made
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "coach"}).json()["id"]
    assert len(mail_pending(bridge, main)[1]) == 2   # no turn: the main conversation
    bridge.stop()
    store = bridge.state_dir / "mail" / "tidy.json"
    kept = json.loads(store.read_text())
    old, stale = kept["cards"]
    old["at"] = time.time() - 8 * 86400            # 28/10: before placements
    stale["at"] = time.time() - 5 * 86400          # placed, then left waiting
    store.write_text(json.dumps(kept))
    places = bridge.state_dir / "card-places.json"
    placed = json.loads(places.read_text())
    placed.pop(f"bip-mail-{old['id']}")
    places.write_text(json.dumps(placed))
    bridge.start(write=False)
    time.sleep(2.5)                                 # the placing watch ran: an old card is not placed late
    assert sorted(c["run_id"] for c in mail_cards(bridge)) == sorted(f"bip-mail-{c['id']}" for c in (old, stale))
    assert mail_pending(bridge, main)[1] == []
    assert f"bip-mail-{old['id']}" not in json.loads(places.read_text())
