"""A card answered, whatever its kind and whenever it is answered: the agent goes on exactly once with the answer
(bridge-rs/src/card_place.rs `settle`). Bug of 2026-11-05 evening (Alex): a site's card validated « tout de suite »,
while the turn that asked still streamed its reply, never reached the agent: a site's or a mail card's tool call never
waits, and the bridge only resumed the agent for an answer given after the turn had ended.

- answered while a tool call waits for it (`ask_secret`, `connector_register`, Hermes' own approvals): that call
  returns it, no other turn;
- answered while the turn goes on and nothing waits (its reply streaming, other tools running): one resumed turn
  once it ends (completed, failed or stopped), none if the turn asked the card's tool again after the answer;
- answered just after the turn ended, or long after: one resumed turn;
- across a bridge restart (the turn still going on, or ended while the bridge was off): one resumed turn;
- several answers close together in one conversation: one turn telling all of them."""
import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor

import pytest

from bbkit.common import AUTH, sse
from bbkit.mcp import call as mcp_call
from bbkit.servers import wait_until
from test_cards_inline import page, waiting_in
from test_connectors import REGISTER, cards as connectors_cards, setup as connectors_setup, tool as connectors_tool
from test_mail_cards import answer as mail_answer, mail_cards, thread_of
from test_mailbox import TOKEN as MAIL_TOKEN, add_mailbox, mail_bridge
from test_sandbox import call as sandbox_call, give_tokens, isolated, site_cards, started

QUIET = 5   # seconds a second turn would have shown up in


def resumed(fakes, session):
    """The turns the bridge started in `session` to tell an answer (card_place.rs `resume_turn`)."""
    return [r for r in list(fakes.hermes.started.values())
            if r.get("session_id") == session and "<!--bip:card" in str(r.get("input"))]


def once(fakes, session):
    """Exactly one resumed turn in `session`, and still one a while later: its words."""
    assert wait_until(lambda: resumed(fakes, session), timeout=20), list(fakes.hermes.started.values())
    time.sleep(QUIET)
    turns = resumed(fakes, session)
    assert len(turns) == 1, turns
    return turns[0]["input"]


def none_for_a_while(fakes, session):
    time.sleep(QUIET)
    assert resumed(fakes, session) == []


def follow(bridge, fakes, run_id, session, agent, script):
    """A turn of `agent` in `session` the bridge follows (the app's `/v1/watch`), its events `script`."""
    fakes.hermes.runs[run_id] = script
    assert bridge.post("/v1/watch", headers=AUTH,
                       json={"agent": agent, "run_id": run_id, "session_id": session}).status_code == 200
    # a profile of the managed Hermes (`/p/<agent>`), or the default bridge's own Hermes
    assert wait_until(lambda: fakes.hermes.seen(f"/p/{agent}/v1/runs/{run_id}/events")
                      or fakes.hermes.seen(f"/v1/runs/{run_id}/events"))
    time.sleep(0.3)


SITES_TOOL = "mcp_bipagents_files_sandbox_sites_request"


def site_in_a_turn(bridges, fakes, session, run, script):
    """Vie asks a site in its turn `run` (events `script` after the call that asks): the card, placed there."""
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    give_tokens(bridge, ["vie"])
    fakes.hermes.add_session(session, [{"role": "user", "content": "Installe-moi le connecteur Garmin"}], title="Garmin")
    follow(bridge, fakes, run, session, "vie", [sse("tool.started", run_id=run, tool=SITES_TOOL, preview="x"), *script])
    thread = thread_of(bridge, session, agent="vie")
    text, error = sandbox_call(bridge, "vie", "sandbox_sites_request", host="connect.garmin.example", reason="Garmin")
    assert not error, text
    assert wait_until(lambda: waiting_in(bridge, thread, "vie", "bip-site-"), timeout=10), page(bridge, thread, "vie")
    [card] = site_cards(bridge, "vie")
    return bridge, card


def allow_site(bridge, card):
    """« Autoriser » in the conversation: the sites' own route."""
    answered = bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": True, "hosts": card["bip_sites"]})
    assert answered.status_code == 200, answered.text


def refuse_from_the_boite(bridge, card, agent="vie"):
    """« Refuser » in the Boîte: `/v1/approve`."""
    answered = bridge.post("/v1/approve", headers=AUTH, json={"agent": agent, "run_id": card["run_id"],
                                                              "request_id": card["request_id"], "choice": "deny"})
    assert answered.status_code == 200, answered.text


# -- a site: its call never waits ------------------------------------------------------------------------------------

def test_a_site_allowed_while_the_reply_still_streams_resumes_the_agent_once_it_ends(bridges, fakes):
    """Alex's case: « Autoriser » at once, the agent still writing « je te demande l'accès… »."""
    gate = threading.Event()
    bridge, card = site_in_a_turn(bridges, fakes, "api_stream", "run_stream", [
        sse("tool.completed", run_id="run_stream", tool=SITES_TOOL),
        sse("message.delta", run_id="run_stream", delta="Je te demande l'accès à Garmin"), gate,
        sse("message.delta", run_id="run_stream", delta=", valide la carte."),
        sse("run.completed", run_id="run_stream", output="Je te demande l'accès à Garmin, valide la carte.")])
    allow_site(bridge, card)
    none_for_a_while(fakes, "api_stream")   # the turn goes on: it may learn it yet
    gate.set()
    told = once(fakes, "api_stream")
    assert "connect.garmin.example" in told and "ALLOWED" in told, told


def test_a_site_refused_from_the_boite_while_other_tools_run_resumes_once(bridges, fakes):
    gate = threading.Event()
    bridge, card = site_in_a_turn(bridges, fakes, "api_tools", "run_tools", [
        sse("tool.completed", run_id="run_tools", tool=SITES_TOOL),
        sse("tool.started", run_id="run_tools", tool="terminal", preview="pip install garminconnect"), gate,
        sse("tool.completed", run_id="run_tools", tool="terminal"),
        sse("run.completed", run_id="run_tools", output="Installé, il manque l'accès au site.")])
    refuse_from_the_boite(bridge, card)
    time.sleep(2)
    assert resumed(fakes, "api_tools") == []
    gate.set()
    told = once(fakes, "api_tools")
    assert "REFUSED" in told and "connect.garmin.example" in told, told


@pytest.mark.parametrize("route", ["own_route", "boite"])
def test_a_site_answered_the_instant_it_is_made_resumes_once(bridges, fakes, route):
    """The main-next battery (GLM lab, 6/11): a sites card answered less than a second after it appeared, before the
    1-second watch placed it, was never told (2 runs of 2). Placed when made now, before its notification."""
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    give_tokens(bridge, ["vie"])
    fakes.hermes.add_session("api_fast", [{"role": "user", "content": "Installe-moi le connecteur Garmin"}], title="Garmin")
    gate = threading.Event()
    follow(bridge, fakes, "run_fast", "api_fast", "vie", [
        sse("tool.started", run_id="run_fast", tool=SITES_TOOL, preview="x"),
        sse("tool.completed", run_id="run_fast", tool=SITES_TOOL), gate,
        sse("run.completed", run_id="run_fast", output="Valide la carte.")])
    text, error = sandbox_call(bridge, "vie", "sandbox_sites_request", host="connect.garmin.example", reason="Garmin")
    assert not error, text
    [card] = site_cards(bridge, "vie")   # at once: no wait for the placing watch
    if route == "own_route":
        allow_site(bridge, card)
    else:
        answered = bridge.post("/v1/approve", headers=AUTH, json={"agent": "vie", "run_id": card["run_id"],
                                                                  "request_id": card["request_id"], "choice": "always"})
        assert answered.status_code == 200, answered.text
    time.sleep(2)
    assert resumed(fakes, "api_fast") == []   # the turn goes on
    gate.set()
    told = once(fakes, "api_fast")
    assert "connect.garmin.example" in told and "ALLOWED" in told, told


def test_a_site_answered_just_as_the_turn_ends_resumes_once(bridges, fakes):
    gate = threading.Event()
    bridge, card = site_in_a_turn(bridges, fakes, "api_edge", "run_edge",
                                  [gate, sse("run.completed", run_id="run_edge", output="Valide la carte.")])
    gate.set()
    allow_site(bridge, card)   # the end and the answer reach the bridge together
    assert "ALLOWED" in once(fakes, "api_edge")


@pytest.mark.parametrize("end", ["run.failed", "run.cancelled"])
def test_a_turn_that_failed_or_was_stopped_still_resumes_once(bridges, fakes, end):
    gate = threading.Event()
    run = "run_" + end.split(".")[1]
    bridge, card = site_in_a_turn(bridges, fakes, "api_end", run, [gate, sse(end, run_id=run, error="model error")])
    allow_site(bridge, card)
    time.sleep(2)
    assert resumed(fakes, "api_end") == []
    gate.set()
    assert "ALLOWED" in once(fakes, "api_end")


def test_a_turn_that_asked_the_site_again_after_the_answer_is_not_resumed(bridges, fakes):
    """The turn learnt the answer itself (« You may send to it already »): nothing repeated."""
    gate = threading.Event()
    bridge, card = site_in_a_turn(bridges, fakes, "api_again", "run_again", [
        sse("tool.completed", run_id="run_again", tool=SITES_TOOL), gate,
        sse("tool.started", run_id="run_again", tool=SITES_TOOL, preview="x"),
        sse("tool.completed", run_id="run_again", tool=SITES_TOOL),
        sse("run.completed", run_id="run_again", output="C'est autorisé, j'envoie.")])
    allow_site(bridge, card)
    time.sleep(2)
    text, error = sandbox_call(bridge, "vie", "sandbox_sites_request", host="connect.garmin.example", reason="Garmin")
    assert not error and "already" in text, text
    gate.set()
    none_for_a_while(fakes, "api_again")


def test_a_site_answered_while_the_turn_runs_then_a_restart_resumes_once_it_ends(bridges, fakes):
    gate = threading.Event()
    bridge, card = site_in_a_turn(bridges, fakes, "api_rs", "run_rs",
                                  [gate, sse("run.completed", run_id="run_rs", output="Valide la carte.")])
    allow_site(bridge, card)
    time.sleep(2)   # the answer kept with the card's place
    bridge.stop()
    fakes.hermes.run_info["run_rs"] = {"status": "running", "session_id": "api_rs"}   # still going on in Hermes
    bridge.start(write=False)
    none_for_a_while(fakes, "api_rs")
    fakes.hermes.run_info["run_rs"] = {"status": "completed", "output": "Valide la carte."}
    gate.set()
    assert "ALLOWED" in once(fakes, "api_rs")


def test_a_site_answered_while_the_turn_runs_resumes_once_after_it_ended_with_the_bridge_off(bridges, fakes):
    gate = threading.Event()
    bridge, card = site_in_a_turn(bridges, fakes, "api_off", "run_off",
                                  [gate, sse("run.completed", run_id="run_off", output="Valide la carte.")])
    refuse_from_the_boite(bridge, card)
    time.sleep(2)
    bridge.stop()
    gate.set()   # ended while the bridge was off: Hermes says so
    fakes.hermes.run_info["run_off"] = {"status": "completed", "output": "Valide la carte."}
    bridge.start(write=False)
    assert "REFUSED" in once(fakes, "api_off")


# -- a mail card: its call never waits -------------------------------------------------------------------------------

def mail_in_a_turn(bridges, fakes, session, run, actions):
    bridge, _ = mail_bridge(bridges, fakes)
    assert add_mailbox(bridge, fakes).status_code == 201
    fakes.hermes.add_session(session, [{"role": "user", "content": "Range mes mails"}], title="Courrier")
    gate = threading.Event()
    follow(bridge, fakes, run, session, "coach", [
        sse("tool.started", run_id=run, tool="mcp_bipagents_mail_bulk", preview="x"),
        sse("tool.completed", run_id=run, tool="mcp_bipagents_mail_bulk"),
        sse("message.delta", run_id=run, delta="C'est prêt, "), gate,
        sse("run.completed", run_id=run, output="C'est prêt, à toi de valider.")])
    for action, domain in actions:
        made = mcp_call(bridge, "mail_bulk", {"action": action, "domain": domain}, token=MAIL_TOKEN)
        assert not made.get("isError"), made
    assert wait_until(lambda: len(mail_cards(bridge)) == len(actions), timeout=10), mail_cards(bridge)
    return bridge, gate, mail_cards(bridge)


def test_mail_cards_answered_together_while_the_turn_goes_on_make_one_turn_after_it(bridges, fakes):
    bridge, gate, cards = mail_in_a_turn(bridges, fakes, "api_mail", "run_mail",
                                         [("move:Banque", "banque.example"), ("archive", "shop.example")])
    assert {c["session_id"] for c in cards} == {"api_mail"}
    assert mail_answer(bridge, cards[0]).status_code == 200
    assert mail_answer(bridge, cards[1], "deny").status_code == 200
    none_for_a_while(fakes, "api_mail")
    gate.set()
    told = once(fakes, "api_mail")
    assert "banque.example" in told and "shop.example" in told, told
    assert "ALLOWED" in told and "REFUSED" in told, told


def test_a_mail_card_answered_after_the_turn_ended_resumes_once(bridges, fakes):
    bridge, gate, [card] = mail_in_a_turn(bridges, fakes, "api_late", "run_late", [("move:Banque", "banque.example")])
    gate.set()
    time.sleep(3)
    assert mail_answer(bridge, card).status_code == 200
    told = once(fakes, "api_late")
    assert "banque.example" in told and "ALLOWED" in told, told


# -- a secret, a connector: their call waits (up to 330 s) -----------------------------------------------------------

def connectors_in_a_turn(bridges, fakes, session, run, tool):
    bridge, _, _ = connectors_setup(bridges, fakes)
    fakes.hermes.add_session(session, [{"role": "user", "content": "Installe Garmin"}], title="Garmin")
    gate = threading.Event()
    follow(bridge, fakes, run, session, "vie", [sse("tool.started", run_id=run, tool=tool, preview="x"), gate,
                                                sse("run.completed", run_id=run, output="C'est fait.")])
    return bridge, gate


SECRET = {"name": "garmin", "site": "connect.garmin.com", "kind": "password", "reason": "Garmin"}


def register_then_restart(bridge, thread):
    """connector_register called in the turn, its card placed there, then the bridge stopped: the call goes with it
    (left to fail in the background, its client's timeout is long)."""
    threading.Thread(target=lambda: _quietly(connectors_tool, bridge, "connector_register", REGISTER), daemon=True).start()
    assert wait_until(lambda: waiting_in(bridge, thread, "vie", "bip-conn-"), timeout=20)
    bridge.stop()


def _quietly(f, *args):
    try:
        f(*args)
    except Exception:  # noqa: BLE001 - the call dies with the bridge, as expected
        pass


@pytest.mark.parametrize("route", ["secure_field", "boite_refusal"])
def test_a_secret_answered_while_its_call_waits_is_told_by_that_call_only(bridges, fakes, route):
    bridge, gate = connectors_in_a_turn(bridges, fakes, "api_sec", "run_sec", "mcp_bipagents_connectors_ask_secret")
    thread = thread_of(bridge, "api_sec", agent="vie")
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(connectors_tool, bridge, "ask_secret", SECRET)
        assert wait_until(lambda: waiting_in(bridge, thread, "vie", "bip-secret-"), timeout=15)
        [card] = connectors_cards(bridge, "bip-secret-")
        if route == "secure_field":
            sent = bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                               json={"allow": True, "value": "Fake-Pa55word", "identifier": "alex@example.org"})
            assert sent.status_code == 200, sent.text
            assert not asked.result(timeout=60).get("isError")
        else:
            refuse_from_the_boite(bridge, card)
            assert asked.result(timeout=60).get("isError")   # « The person refused… »
    time.sleep(2)
    gate.set()
    none_for_a_while(fakes, "api_sec")


def test_a_connector_approved_while_its_call_waits_is_told_by_that_call_only(bridges, fakes):
    bridge, gate = connectors_in_a_turn(bridges, fakes, "api_con", "run_con", "mcp_bipagents_connectors_connector_register")
    thread = thread_of(bridge, "api_con", agent="vie")
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(connectors_tool, bridge, "connector_register", REGISTER)
        assert wait_until(lambda: waiting_in(bridge, thread, "vie", "bip-conn-"), timeout=20)
        [card] = connectors_cards(bridge, "bip-conn-")
        sent = bridge.post(f"/v1/connectors/requests/{card['request_id']}", headers=AUTH, json={"allow": True})
        assert sent.status_code == 200, sent.text
        result = asked.result(timeout=60)
        assert not result.get("isError") and "approved" in result["content"][0]["text"], result
    gate.set()
    none_for_a_while(fakes, "api_con")


def test_a_connector_whose_call_died_in_a_restart_resumes_once_its_turn_ends(bridges, fakes):
    """The bridge restarted while connector_register waited (its call gone with it), the turn still going on in
    Hermes: answered then, the agent is told once the turn ends."""
    bridge, gate = connectors_in_a_turn(bridges, fakes, "api_cr", "run_cr", "mcp_bipagents_connectors_connector_register")
    thread = thread_of(bridge, "api_cr", agent="vie")
    register_then_restart(bridge, thread)
    fakes.hermes.run_info["run_cr"] = {"status": "running", "session_id": "api_cr"}
    bridge.start(write=False)
    [card] = connectors_cards(bridge, "bip-conn-")
    sent = bridge.post(f"/v1/connectors/requests/{card['request_id']}", headers=AUTH, json={"allow": True})
    assert sent.status_code == 200, sent.text
    none_for_a_while(fakes, "api_cr")   # the turn goes on
    fakes.hermes.run_info["run_cr"] = {"status": "completed", "output": "C'est fait."}
    gate.set()
    told = once(fakes, "api_cr")
    assert "ALLOWED" in told and "Garmin" in told, told


def test_a_connector_answered_long_after_its_turn_ended_resumes_once(bridges, fakes):
    """Its call stopped waiting (here: a restart), the turn over: answered later, one turn."""
    bridge, gate = connectors_in_a_turn(bridges, fakes, "api_cl", "run_cl", "mcp_bipagents_connectors_connector_register")
    thread = thread_of(bridge, "api_cl", agent="vie")
    register_then_restart(bridge, thread)
    gate.set()
    bridge.start(write=False)
    time.sleep(3)
    [card] = connectors_cards(bridge, "bip-conn-")
    refuse_from_the_boite(bridge, card)
    assert "REFUSED" in once(fakes, "api_cl")


# -- Hermes' own approvals: Hermes' call waits -----------------------------------------------------------------------

def test_a_hermes_approval_answered_while_it_waits_reaches_hermes_once_and_starts_no_turn(bridge, fakes):
    hermes = fakes.hermes
    hermes.add_session("api_h", [{"role": "user", "content": "Nettoie le dossier build"}], title="Build", agent="")
    gate = threading.Event()
    follow(bridge, fakes, "run_h", "api_h", "wellness", [
        sse("approval.request", run_id="run_h", request_id="req_h", command="rm -rf build/", choices=["once", "deny"]),
        gate, sse("run.completed", run_id="run_h", output="Dossier supprimé.")])
    assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"])
    hermes.run_info["run_h"] = {"status": "waiting_for_approval", "execution_generation": 1,
                                "pending_controls": [{"kind": "approval", "prompt_id": "req_h",
                                                      "execution_generation": 1, "choices": ["once", "deny"]}]}
    sent = bridge.post("/v1/approve", headers=AUTH, json={"agent": "wellness", "run_id": "run_h",
                                                          "request_id": "req_h", "choice": "once"})
    assert sent.status_code == 200, sent.text
    assert [a["body"]["choice"] for a in hermes.approvals] == ["once"]
    gate.set()
    time.sleep(QUIET)
    assert [r for r in hermes.started.values() if "<!--bip:card" in str(r.get("input"))] == []
    assert len(hermes.approvals) == 1


# -- Alex's Garmin connector, 5/11 ~20:50 (Android, release 7d1154f6): a code typed in a secret card and a card of
# several sites validated in quick succession while the turn ran; the turn « ran endlessly », he had to validate cards
# again, stop and ask it to go on. Here: the waiting call gets the code at once, every answered card stays gone, and
# the sites' answer (no call waits for it) reaches the agent once the turn ends, stopped or not. ---------------------

GARMIN_HOSTS = ["connect.garmin.example", "sso.garmin.example", "diauth.garmin.example", "services.garmin.example",
                "static.garmin.example"]


def answered_cards_stay_gone(bridge, thread, run_ids):
    """For a few seconds, none of `run_ids` listed again in the Boîte or the conversation."""
    for _ in range(6):
        listed = {i["run_id"] for i in bridge.get("/v1/approvals", headers=AUTH, params={"agent": "vie"}).json()["items"]}
        inline = {a["run_id"] for a in page(bridge, thread, "vie")["pending"]["approvals"] or []}
        assert not (set(run_ids) & (listed | inline)), (run_ids, listed, inline)
        time.sleep(0.5)


@pytest.mark.parametrize("order", ["secret_then_sites", "sites_then_secret", "stopped_after"])
def test_alex_garmin_a_secret_and_five_sites_answered_quickly_continue_the_agent_once(bridges, fakes, order):
    bridge, _, _ = connectors_setup(bridges, fakes)
    fakes.hermes.add_session("api_gar", [{"role": "user", "content": "Connecte-toi à Garmin"}], title="Garmin")
    gate = threading.Event()
    end = "run.cancelled" if order == "stopped_after" else "run.completed"
    follow(bridge, fakes, "run_gar", "api_gar", "vie", [
        sse("tool.started", run_id="run_gar", tool=SITES_TOOL, preview="x"),
        sse("tool.completed", run_id="run_gar", tool=SITES_TOOL),
        sse("tool.started", run_id="run_gar", tool="mcp_bipagents_connectors_ask_secret", preview="x"), gate,
        sse("tool.completed", run_id="run_gar", tool="mcp_bipagents_connectors_ask_secret"),
        sse("message.delta", run_id="run_gar", delta="Je teste la connexion…"),
        sse(end, run_id="run_gar", output="Je teste la connexion…")])
    thread = thread_of(bridge, "api_gar", agent="vie")
    for host in GARMIN_HOSTS:
        text, error = sandbox_call(bridge, "vie", "sandbox_sites_request", host=host, reason="le connecteur Garmin")
        assert not error, text
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(connectors_tool, bridge, "ask_secret",
                            {"name": "garmin_code", "site": "connect.garmin.com", "kind": "code", "reason": "Le code reçu"})
        assert wait_until(lambda: waiting_in(bridge, thread, "vie", "bip-secret-")
                          and waiting_in(bridge, thread, "vie", "bip-site-"), timeout=20), page(bridge, thread, "vie")
        [secret] = connectors_cards(bridge, "bip-secret-")
        sites = site_cards(bridge, "vie")
        assert sorted(h for c in sites for h in c["bip_sites"]) == sorted(GARMIN_HOSTS), sites

        def give_code():
            sent = bridge.post(f"/v1/vault/secrets/requests/{secret['request_id']}", headers=AUTH,
                               json={"allow": True, "value": "MFA-7Q4K2Z"})
            assert sent.status_code == 200, sent.text

        if order == "sites_then_secret":
            for card in sites:
                allow_site(bridge, card)
            give_code()
        else:
            give_code()
            for card in sites:
                allow_site(bridge, card)
        started_at = time.time()
        result = asked.result(timeout=30)   # the waiting call has the code at once, never 330 s
        assert not result.get("isError") and time.time() - started_at < 10, result
    answered_cards_stay_gone(bridge, thread, [secret["run_id"], *(c["run_id"] for c in sites)])
    assert site_cards(bridge, "vie") == [] and connectors_cards(bridge, "bip-secret-") == []
    # what the turn already did is not repeated: the code was the call's (a code card's answer told once)
    assert "garmin_code" not in json.dumps(resumed(fakes, "api_gar"))
    listed = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert all(h in json.dumps(listed) for h in GARMIN_HOSTS), listed
    gate.set()   # the turn ends (or the person stops it): the sites' answer, which no call waited for, told once
    told = once(fakes, "api_gar")
    assert told.count("»: ALLOWED") + told.count("your request: ALLOWED") == len(sites), told
    assert "REFUSED" not in told and "garmin.example" in told, told
    answered_cards_stay_gone(bridge, thread, [secret["run_id"], *(c["run_id"] for c in sites)])


def test_a_connector_whose_files_changed_before_its_answer_keeps_its_call_on_the_new_card(bridges, fakes):
    """Approved after its files changed: a new card (the files approved are the ones shown), and the call waiting for
    the first one waits for it, answered as soon as it is (5/11: it waited 5 min for a card gone)."""
    bridge, _, work = connectors_setup(bridges, fakes)
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(connectors_tool, bridge, "connector_register", REGISTER)
        assert wait_until(lambda: connectors_cards(bridge, "bip-conn-"), timeout=20)
        [first] = connectors_cards(bridge, "bip-conn-")
        (work / "README.md").write_text("Garmin connector, changed\n")
        changed = bridge.post(f"/v1/connectors/requests/{first['request_id']}", headers=AUTH, json={"allow": True})
        assert changed.status_code == 409 and changed.json()["detail"] == {"code": "changed"}, changed.text
        assert wait_until(lambda: [c for c in connectors_cards(bridge, "bip-conn-") if c["run_id"] != first["run_id"]])
        [again] = connectors_cards(bridge, "bip-conn-")
        sent = bridge.post(f"/v1/connectors/requests/{again['request_id']}", headers=AUTH, json={"allow": True})
        assert sent.status_code == 200, sent.text
        result = asked.result(timeout=30)
        assert not result.get("isError") and "approved" in result["content"][0]["text"], result


def test_a_bridge_cards_notification_offers_no_button_and_names_the_card(bridges, fakes):
    """« La notification ouvre la carte » (Alex 5/11): a bridge card's push has the category `BRIDGE_CARD`, which no
    released app gives a button (« CARD » had « Autoriser une fois » on the iPhone: a sites card allowed its first host
    only), and names the card (`t`, `r`, `n`) for the tap to open it. Hermes' approvals keep `APPROVAL`."""
    bridge, _, _ = connectors_setup(bridges, fakes)
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": "f" * 64, "environment": "sandbox",
                                                           "agent_ids": ["vie"]}).status_code == 200
    for host in GARMIN_HOSTS[:2]:
        text, error = sandbox_call(bridge, "vie", "sandbox_sites_request", host=host, reason="le connecteur Garmin")
        assert not error, text
    with ThreadPoolExecutor(1) as pool:
        asked = pool.submit(connectors_tool, bridge, "connector_register", REGISTER)
        assert wait_until(lambda: {p.get("t") for p in fakes.apns.payloads()} >= {"site", "connector"}, timeout=20), \
            fakes.apns.payloads()
        [card] = connectors_cards(bridge, "bip-conn-")
        assert bridge.post(f"/v1/connectors/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": False}).status_code == 200
        asked.result(timeout=30)
    [site] = site_cards(bridge, "vie")
    pushed = {p["t"]: p for p in fakes.apns.payloads() if p.get("t") in ("site", "connector")}
    for t, payload in pushed.items():
        assert payload["aps"]["category"] == "BRIDGE_CARD", payload
        assert payload["agent"] == "vie" and payload["n"].startswith("n_") and payload["r"], payload
        assert "choices" not in payload, payload   # nothing to answer from it
    assert pushed["site"]["r"] == site["request_id"] and pushed["connector"]["r"] == card["request_id"]
