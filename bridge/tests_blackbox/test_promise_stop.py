"""A turn that ends on a promise with no work done (bridge-rs/src/go_on.rs, runs.rs `go_on`; Alex 13/11, his « Vie »:
« Je te prépare 2-3 trouvailles pour 5 € », the turn over with no search). An answer without a tool call ends a Hermes
turn, so the bridge starts ONE continuation in the same conversation, told out of sight to do it now; its events follow
in the turn's stream after a `loop_rescue_started` notice, and its end is the turn's end. `[brake] go_on` in
bridge.toml."""
import asyncio
import threading
import time

from bbkit.common import AUTH, read_sse, sse
from bbkit.servers import wait_until

PROMISE = "Je te prépare 2-3 trouvailles pour 5 €."
FOUND = "Voilà 3 idées à moins de 5 € : un carnet HEMA (3,69 €), un poche Folio (2,50 €), un gel mains (2,40 €)."


def follow(bridge, run_id):
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        return read_sse(resp)


def notices(events):
    return [data for name, data in events if name == "bridge.notice"]


def turn(run_id, reply, tools=()):
    return ([sse("tool.started", run_id=run_id, tool=tool, preview="") for tool in tools]
            + [sse("tool.completed", run_id=run_id, tool=tool) for tool in tools]
            + [sse("message.delta", run_id=run_id, delta=reply), sse("run.completed", run_id=run_id, output=reply)])


def test_a_turn_that_ends_on_a_promise_goes_on_once_and_ends_on_the_work(bridges, fakes):
    bridge = bridges.new().start()
    # the question card answered, then only the promise
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE, tools=["clarify"])
    fakes.hermes.runs["r1"] = turn("r1", FOUND, tools=["web_search"])
    events = follow(bridge, "run_vie")
    said = notices(events)
    assert [n["code"] for n in said] == ["loop_rescue_started"] and said[0]["run_id"] == "r1"
    names = [name for name, _ in events]
    assert names[-1] == "run.completed" and events[-1][1]["output"] == FOUND
    assert names.count("run.completed") == 1, "one end: the continuation's"
    deltas = [data.get("delta") for name, data in events if name == "message.delta"]
    assert deltas[0] == PROMISE and deltas[-1] == FOUND and "\n\n" in deltas
    # one continuation, in the same conversation, its instruction out of the person's sight
    assert list(fakes.hermes.started) == ["r1"]
    started = fakes.hermes.started["r1"]
    assert started["session_id"] == "api_c"
    assert "[bipagents:go_on]" in started["input"] and "start_task" in started["input"]
    assert f"« {PROMISE} »" in started["input"]
    assert bridge.logged("promise with no work: the continuation answered")
    # an app coming back replays the same
    again = follow(bridge, "run_vie")
    assert [n["code"] for n in notices(again)] == ["loop_rescue_started"] and again[-1][1]["output"] == FOUND


def test_the_plan_that_comes_back_in_a_few_minutes_goes_on_too(bridges, fakes):
    bridge = bridges.new().start()
    plan = ("Je regarde Sephora, Marionnaud et Nocibé :\n\n- miniatures\n- échantillons payants\n\n"
            "Je reviens dans quelques minutes avec le lien et le meilleur prix.")
    fakes.hermes.runs["run_go"] = turn("run_go", plan)
    fakes.hermes.runs["r1"] = turn("r1", FOUND, tools=["mcp__stealth__navigate"])
    events = follow(bridge, "run_go")
    assert [n["code"] for n in notices(events)] == ["loop_rescue_started"]
    assert events[-1][1]["output"] == FOUND


def test_a_continuation_that_promises_again_is_not_continued(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = turn("r1", "Je m'en occupe, je reviens vers toi dans un instant.")
    events = follow(bridge, "run_vie")
    assert [n["code"] for n in notices(events)] == ["loop_rescue_started"]
    assert events[-1][1]["run_id"] == "r1"
    assert list(fakes.hermes.started) == ["r1"], "never a continuation of the continuation"


def test_a_question_an_answer_or_work_done_end_the_turn_as_before(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_q"] = turn("run_q", "Quelques idées : un carnet, un livre. Je te prépare le panier sur la Fnac ?")
    fakes.hermes.runs["run_a"] = turn("run_a", FOUND)
    # it searched: what it says next is its own business
    fakes.hermes.runs["run_w"] = turn("run_w", "J'ai trouvé deux pistes, je te prépare la suite.", tools=["web_search"])
    # it handed the work over: « je reviens » is true
    fakes.hermes.runs["run_t"] = turn("run_t", "Je m'en occupe en arrière-plan, je reviens vers toi quand c'est fini.",
                                      tools=["mcp_bipagents_start_task"])
    for run in ("run_q", "run_a", "run_w", "run_t"):
        events = follow(bridge, run)
        assert notices(events) == [], run
        assert events[-1][1]["run_id"] == run
    assert list(fakes.hermes.started) == []


def test_go_on_switched_off(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"go_on": False}
    bridge.start()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    events = follow(bridge, "run_vie")
    assert notices(events) == [] and events[-1][1]["output"] == PROMISE
    assert list(fakes.hermes.started) == []


def test_the_continuation_s_instruction_is_not_in_the_timeline(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = turn("r1", FOUND, tools=["web_search"])
    follow(bridge, "run_vie")
    sent = fakes.hermes.started["r1"]["input"]
    stored = sent if sent.startswith("<!--bip:now") else f"<!--bip:now vendredi 13 novembre, 12 h 27-->\n{sent}"
    fakes.hermes.add_session("api_c", [
        {"role": "user", "content": "Trouve-moi un achat test à 5 €."},
        {"role": "assistant", "content": PROMISE},
        {"role": "user", "content": stored},
        {"role": "assistant", "content": FOUND}], title="Achat test")
    thread = bridge.get("/v1/threads/resolve", headers=AUTH, params={"agent": "wellness", "session_id": "api_c"}).json()
    page = bridge.get(f"/v1/threads/{thread['thread']['id']}/timeline", headers=AUTH, params={"agent": "wellness"})
    assert page.status_code == 200, page.text
    shown = [(i["kind"], i.get("role"), i.get("text")) for i in page.json()["items"]]
    assert shown == [("message", "person", "Trouve-moi un achat test à 5 €."), ("message", "agent", PROMISE),
                     ("message", "agent", FOUND)], shown
    assert "bipagents:" not in page.text and "A note from the app" not in page.text
    assert wait_until(lambda: bridge.logged("promise with no work: the continuation answered"))


# -- review of 13/11 (each failed before its fix) ---------------------------------------------------------------------

def thread_of(bridge, fakes):
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "Trouve-moi un achat test à 5 €."}], title="Achat test")
    return bridge.get("/v1/threads/resolve", headers=AUTH,
                      params={"agent": "wellness", "session_id": "api_c"}).json()["thread"]["id"]


def test_a_recipe_or_a_closing_line_is_not_continued(bridges, fakes):
    bridge = bridges.new().start()
    said = {
        "run_fr": "Ajoute la crème, couvre et laisse mijoter à feu doux : ce sera prêt dans 20 minutes. Bon appétit !",
        "run_en": "What a lovely plan! I'm looking forward to hearing all about your trip.",
        "run_nl": "Wat een leuk plan! Ik kijk ernaar uit om te horen hoe het was.",
    }
    for run, reply in said.items():
        fakes.hermes.runs[run] = turn(run, reply)
    continued = [run for run in said if notices(follow(bridge, run))]
    assert continued == [], {r: s["input"][:90] for r, s in fakes.hermes.started.items()}


def test_stop_during_the_continuation_stops_the_continuation(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    targets = []
    for i in range(6):
        first, nxt, gate = f"run_vie{i}", f"r{len(fakes.hermes.started) + 1}", threading.Event()
        fakes.hermes.runs[first] = turn(first, PROMISE)
        fakes.hermes.runs[nxt] = [sse("tool.started", run_id=nxt, tool="web_search", preview="idée à 5 €"), gate,
                                  *turn(nxt, FOUND)]
        watch = {"agent": "wellness", "run_id": first, "session_id": "api_c"}
        assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
        assert wait_until(lambda: fakes.hermes.seen(f"/v1/runs/{nxt}/events"))
        time.sleep(0.2)
        # the person taps Stop while the agent now does the work
        stopped = bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={"agent": "wellness"})
        assert stopped.status_code == 200, stopped.text
        targets.append((nxt, stopped.json().get("run_id"), fakes.hermes.started[nxt]["status"]))
        gate.set()
        # the turn ends stopped, not on the continuation's words nor on its promise
        assert wait_until(lambda: len(bridge.logged("promise with no work: the continuation was stopped")) == i + 1)
    assert all(asked == got and status == "cancelled" for asked, got, status in targets), targets
    assert not bridge.logged("promise with no work: the continuation answered")


def test_a_long_continuation_is_steered_to_hand_over_like_any_turn(bridges, fakes):
    bridge = bridges.new()
    bridge.config["jobs"] = {"handoff_tool_calls": 3}
    bridge.start()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = ([sse("tool.started", run_id="r1", tool="web_search", preview=f"idée {i}") for i in range(8)]
                               + turn("r1", FOUND))
    assert follow(bridge, "run_vie")[-1][1]["output"] == FOUND
    wait_until(lambda: fakes.hermes.steers, timeout=3)
    assert [s["run_id"] for s in fakes.hermes.steers] == ["r1"], "8 searches, never told to hand over"


def test_the_continuations_files_reach_the_app_once(bridges, fakes):
    from test_sandbox import isolated, started
    bridge, hub, volumes = isolated(bridges, fakes, agents=("wellness",))
    started(bridge, hub, ["wellness"])
    (volumes[("envoi", "wellness")] / "idees.md").write_text("# Idées à 5 €\n- un carnet\n")
    found = "Voilà mes 3 idées à moins de 5 €, tout est dans /envoi/idees.md."
    nxt = f"r{len(fakes.hermes.started) + 1}"
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs[nxt] = turn(nxt, found, tools=["web_search"])
    end = follow(bridge, "run_vie")[-1][1]
    assert end["run_id"] == nxt and end["output"] == found
    assert [a["path"] for a in end["attachments"]] == ["/envoi/idees.md"]


def test_the_family_conversation_never_goes_on(bridges, fakes):
    from test_family_agent import make_family_agent
    from test_households_and_peers import household
    a = household(bridges, fakes, "alex", ["famille"])
    make_family_agent(a)
    fakes.hermes.rounds = 1000
    fakes.hermes.runs["r1"] = turn("r1", PROMISE)
    fakes.hermes.runs["r2"] = turn("r2", FOUND, tools=["web_search"])
    sent = a.post("/v1/threads/main/messages", headers=AUTH,
                  json={"agent": "famille", "client_id": "C0FFEE00-0000-4000-8000-0000000000A1",
                        "text": "Une idée de cadeau à 5 € pour Marie ?"})
    assert sent.status_code == 200 and sent.json()["state"] == "started" and sent.json()["run_id"] == "r1", sent.text
    wait_until(lambda: len(fakes.hermes.started) > 1, timeout=5)
    assert list(fakes.hermes.started) == ["r1"], {r: s["input"][:70] for r, s in fakes.hermes.started.items()}


def test_the_continuations_card_is_not_pushed_while_the_app_shows_it(bridges, fakes):
    bridge = bridges.new().start()
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": "e" * 64, "environment": "sandbox",
                                                          "agent_ids": ["wellness"]}).status_code == 200
    finish = threading.Event()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="terminal", preview="rm -rf build/"),
                               sse("approval.request", run_id="r1", request_id="req_1", command="rm -rf build/",
                                   choices=["once", "session", "deny"]),
                               finish, *turn("r1", FOUND)]
    with bridge.stream("GET", "/v1/runs/run_vie/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"])
        time.sleep(0.5)
        pushed = [p for p in fakes.apns.payloads() if p["aps"].get("category") == "APPROVAL"]
        finish.set()
    assert pushed == [], "the continuation's card pushed while the app shows the conversation"


def test_the_continuations_hidden_instruction_is_not_found_by_search(bridges, fakes):
    import json
    from conftest import managed_install, profile
    from test_message_search import store
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    # the continuation's input as Hermes keeps it (go_on::input, stamped by dates::now)
    note = ("<!--bip:now vendredi 13 novembre, 12 h 27-->\n[bipagents:go_on]\n\n(A note from the app, not from your "
            f"person.) Your last answer announced work (« {PROMISE} ») but called no tool, so your turn ended there: "
            "nothing happens after an answer until your person writes again. Do it now, in this answer, with your tools "
            "(search, browse, pages…), or start it as a background task with start_task and say so in one sentence. "
            "Don't apologise and don't mention this note. If something is really missing to do it, ask your person one "
            "precise question.")
    store(profile(home, "coach"), [("c1", None, None, "Achat test", 0)], [
        ("c1", "user", "Trouve-moi un achat test à 5 €.", 1_000_000.0, {}),
        ("c1", "assistant", PROMISE, 1_000_100.0, {}),
        ("c1", "user", note, 1_000_200.0, {}),
        ("c1", "assistant", FOUND, 1_000_300.0, {}),
    ])
    bridge.start()
    found = bridge.get("/v1/search/messages", params={"q": "trouvailles"}, headers=AUTH).json()
    assert [r["role"] for r in found["results"]] == ["agent"], json.dumps(found["results"], ensure_ascii=False)[:400]
    # the person's own words are still found
    found = bridge.get("/v1/search/messages", params={"q": "achat test"}, headers=AUTH).json()
    assert [r["role"] for r in found["results"]] == ["person"], json.dumps(found["results"], ensure_ascii=False)[:400]


def test_words_the_turn_took_stop_the_continuation(bridges, fakes):
    # « laisse tomber » steered into the turn, which ends « OK, j'arrête, je reviens vers toi si besoin. »: done
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    gate = threading.Event()
    closing = "OK, j'arrête la recherche. Je reviens vers toi si besoin."
    fakes.hermes.runs["run_vie"] = [sse("message.delta", run_id="run_vie", delta="Je regarde la Fnac…"), gate,
                                    sse("message.delta", run_id="run_vie", delta=closing),
                                    sse("run.completed", run_id="run_vie", output=closing)]
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_vie/events"))
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C0FFEE00-0000-4000-8000-0000000000C2",
                             "text": "Laisse tomber"})
    assert sent.status_code == 200 and sent.json()["state"] == "steered", sent.text
    gate.set()
    time.sleep(2)
    assert all("[bipagents:go_on]" not in s["input"] for s in fakes.hermes.started.values()), fakes.hermes.started


# -- what the branch says it does (passed before the review too) ------------------------------------------------------

def test_words_the_turn_did_not_take_stop_the_continuation(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_vie"] = [sse("message.delta", run_id="run_vie", delta=PROMISE),
                                    sse("run.completed", run_id="run_vie", output=PROMISE, pending_steer="et en bio")]
    events = follow(bridge, "run_vie")
    assert notices(events) == [] and list(fakes.hermes.started) == []


def test_a_message_waiting_after_the_turn_stops_the_continuation(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    gate = threading.Event()
    fakes.hermes.runs["run_vie"] = [sse("message.delta", run_id="run_vie", delta=PROMISE), gate,
                                    sse("run.completed", run_id="run_vie", output=PROMISE)]
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_vie/events"))
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C0FFEE00-0000-4000-8000-0000000000C1",
                             "text": "Plutôt en bio", "after": True})
    assert sent.status_code == 200 and sent.json()["state"] == "queued", sent.text
    gate.set()
    time.sleep(2)
    assert not bridge.logged("promise with no work: continuation started")
    assert all("[bipagents:go_on]" not in s["input"] for s in fakes.hermes.started.values())


def test_a_members_turn_in_the_family_conversation_never_goes_on(bridges, fakes):
    from test_family_agent import make_family_agent
    from test_family_thread import discover, family
    a, b, c = family(bridges, fakes)
    make_family_agent(a)
    discover(b, c)
    fakes.hermes.rounds = 1000
    fakes.hermes.runs["r1"] = turn("r1", PROMISE)
    fakes.hermes.runs["r2"] = turn("r2", FOUND, tools=["web_search"])
    sent = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={"agent": "famille__alex", "client_id": "C0FFEE00-0000-4000-8000-0000000000A1",
                        "text": "Une idée de cadeau à 5 € ?"})
    assert sent.status_code == 200 and sent.json()["state"] == "started", sent.text
    wait_until(lambda: len(fakes.hermes.started) > 1, timeout=5)
    assert list(fakes.hermes.started) == ["r1"], {r: s["input"][:70] for r, s in fakes.hermes.started.items()}


# -- second review of 13/11 (each failed before its fix, but the `ok_` ones: the first fixes pushed further) ----------

DRAFT = ("Voici une proposition de réponse :\n\nBonjour Madame Durand,\n\nMerci pour votre message. Je regarde votre "
         "dossier ce week-end et je reviens vers vous lundi.\n\nBien cordialement,\nAlex")


def test_a_drafted_reply_is_not_continued(bridges, fakes):
    # « Écris-moi une réponse pour Mme Durand »: the draft's « je regarde… je reviens vers vous » are the person's words
    # to come, not the agent's work
    bridge = bridges.new().start()
    fakes.hermes.runs["run_draft"] = turn("run_draft", DRAFT)
    events = follow(bridge, "run_draft")
    started = {r: s["input"][:230] for r, s in fakes.hermes.started.items()}
    assert notices(events) == [] and started == {}, started


def window(bridge, fakes):
    """A turn that ends on the promise while Hermes is still closing it (go_on's own retries: about 1.5 s before the
    continuation starts), the app following it."""
    thread = thread_of(bridge, fakes)
    fakes.hermes.start_failures = 2
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = turn("r1", FOUND, tools=["web_search"])
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert wait_until(lambda: bridge.logged("promise with no work: continuation not started yet"))
    return thread


def test_words_sent_while_the_continuation_starts_hold_it(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.steer_status = 409   # Hermes: a run that ended takes no more words
    thread = window(bridge, fakes)
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C0FFEE00-0000-4000-8000-0000000000D1",
                             "text": "Non laisse tomber, j'irai en magasin"})
    assert sent.status_code == 200 and sent.json()["state"] == "queued", sent.text
    time.sleep(3)
    continued = [r for r, s in fakes.hermes.started.items() if "[bipagents:go_on]" in s["input"]]
    assert continued == [], f"the person's words wait in line, yet the continuation started: {continued}"
    assert bridge.logged("promise with no work: the person stepped in")


def test_stop_while_the_continuation_starts_is_kept(bridges, fakes):
    bridge = bridges.new().start()
    thread = window(bridge, fakes)
    stopped = bridge.post(f"/v1/threads/{thread}/stop", headers=AUTH, json={"agent": "wellness"})
    assert stopped.status_code == 200, stopped.text
    time.sleep(3)
    assert list(fakes.hermes.started) == [], f"Stop answered {stopped.json()}, then {list(fakes.hermes.started)} started"


def cited(bridges, fakes, go_on):
    from test_sandbox import isolated, started
    bridge, hub, volumes = isolated(bridges, fakes, agents=("wellness",))
    if not go_on:
        bridge.config["brake"] = {"go_on": False}
    started(bridge, hub, ["wellness"])
    (volumes[("envoi", "wellness")] / "idees.md").write_text("# Idées à 5 €\n- un carnet\n")
    promise = "Tes idées sont dans /envoi/idees.md. Je te prépare aussi les liens pour 5 €."
    nxt = f"r{len(fakes.hermes.started) + 1}"
    fakes.hermes.runs["run_vie"] = turn("run_vie", promise)
    fakes.hermes.runs[nxt] = turn(nxt, FOUND, tools=["web_search"])
    return follow(bridge, "run_vie")[-1][1]


def test_without_go_on_the_cited_file_is_handed_over(bridges, fakes):
    end = cited(bridges, fakes, go_on=False)
    assert [a["path"] for a in end.get("attachments") or []] == ["/envoi/idees.md"], end


def test_a_file_the_promise_cites_still_reaches_the_app(bridges, fakes):
    end = cited(bridges, fakes, go_on=True)
    assert end["output"] == FOUND
    assert [a["path"] for a in end.get("attachments") or []] == ["/envoi/idees.md"], end


def test_after_a_restart_the_continuation_is_never_continued(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000   # r1 still running for Hermes when the bridge comes back
    gate = threading.Event()
    again = "Je m'en occupe, je reviens vers toi dans un instant."
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta="Je cherche… "), gate,
                               sse("message.delta", run_id="r1", delta=again),
                               sse("run.completed", run_id="r1", output=again)]
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/events"))
    # Hermes keeps the continuation's input in the conversation, as the person's rows
    fakes.hermes.add_session("api_c", [{"role": "user", "content": "Trouve-moi un achat test à 5 €."},
                                       {"role": "assistant", "content": PROMISE},
                                       {"role": "user", "content": fakes.hermes.started["r1"]["input"]}], title="Achat")
    # the iOS app adopts the continuation (ConversationModel.adoptRun: POST /v1/watch of its run)
    assert bridge.post("/v1/watch", headers=AUTH, json={**watch, "run_id": "r1"}).status_code == 200
    time.sleep(0.5)
    bridge.restart()
    assert wait_until(lambda: bridge.logged("run watch resumed"), timeout=15)
    gate.set()
    time.sleep(3)
    continued = [r for r, s in fakes.hermes.started.items() if "[bipagents:go_on]" in s["input"]]
    assert continued == ["r1"], f"a continuation of the continuation: {continued}"


def test_the_continuation_keeps_the_turns_instructions(bridges, fakes):
    # a conversation shown read-only to invited people (read_shares.rs): the app's instructions for the turn hold for
    # its continuation too
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    fakes.hermes.runs["r1"] = turn("r1", PROMISE)
    fakes.hermes.runs["r2"] = turn("r2", FOUND, tools=["web_search"])
    shared = "This conversation is shown read-only to 3 people your person invited."
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C0FFEE00-0000-4000-8000-0000000000F1",
                             "text": "Trouve-moi un achat test à 5 €.", "instructions": shared})
    assert sent.status_code == 200 and sent.json()["run_id"] == "r1", sent.text
    assert wait_until(lambda: "r2" in fakes.hermes.started, timeout=10), fakes.hermes.started
    assert "[bipagents:go_on]" in fakes.hermes.started["r2"]["input"]
    assert fakes.hermes.started["r2"]["instructions"] == shared


def test_ok_words_written_during_the_continuation_go_into_it(bridges, fakes):
    bridge = bridges.new().start()
    thread = thread_of(bridge, fakes)
    gate = threading.Event()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="web_search", preview="idée"), gate,
                               *turn("r1", FOUND)]
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/events"))
    time.sleep(0.3)
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C0FFEE00-0000-4000-8000-0000000000E1",
                             "text": "plutôt en bio"})
    assert sent.status_code == 200 and sent.json()["state"] == "steered" and sent.json()["run_id"] == "r1", sent.text
    assert [s["run_id"] for s in fakes.hermes.steers] == ["r1"]
    gate.set()


def leave_with_a_card(bridges, fakes, card):
    bridge = bridges.new().start()
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": "e" * 64, "environment": "sandbox",
                                                          "agent_ids": ["wellness"]}).status_code == 200
    finish = threading.Event()
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="terminal", preview="rm -rf build/"), card,
                               finish, *turn("r1", FOUND)]
    with bridge.stream("GET", "/v1/runs/run_vie/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/events"))
        time.sleep(1.0)   # the card comes right after the continuation's first tool
        before = list(fakes.apns.payloads())
    # the app leaves the conversation (screen locked): the continuation's card, seen through the turn, is pushed now
    assert wait_until(lambda: len(fakes.apns.payloads()) > len(before), timeout=5), fakes.apns.payloads()
    finish.set()
    return before


def test_ok_the_continuations_approval_is_pushed_once_the_app_leaves(bridges, fakes):
    before = leave_with_a_card(bridges, fakes, sse("approval.request", run_id="r1", request_id="req_1",
                                                   command="rm -rf build/", choices=["once", "session", "deny"]))
    assert before == []


def test_ok_the_continuations_question_is_pushed_once_the_app_leaves(bridges, fakes):
    before = leave_with_a_card(bridges, fakes, sse("clarify.request", run_id="r1", request_id="q_1",
                                                   question="Quel budget ?", choices=["5 €", "10 €"]))
    assert before == []


# -- third review of 13/11 (each failed before its fix, but the `ok_` one) ---------------------------------------------

def test_words_sent_while_the_continuation_starts_are_not_paused_behind_it(bridges, fakes, monkeypatch):
    """The person writes while the continuation's POST /v1/runs is on its way: go_on's check after the start sees their
    message waiting and stops the continuation (as it should), but Hermes then ends that continuation `run.cancelled`,
    and the queue reads a cancelled run with a message waiting as the person's Stop: paused, their message never
    starts (until « Reprendre »)."""
    bridge = bridges.new().start()
    fakes.hermes.steer_status = 409   # Hermes: a run that ended takes no more words
    thread = thread_of(bridge, fakes)
    fakes.hermes.runs["run_vie"] = turn("run_vie", PROMISE)
    # the continuation, stopped by the bridge right after its start: Hermes ends it cancelled
    fakes.hermes.runs["r1"] = [sse("run.cancelled", run_id="r1")]
    fakes.hermes.runs["r2"] = turn("r2", FOUND)
    posted = threading.Event()
    real = fakes.hermes._runs

    async def slow_start(seen, agent, parts, method):
        body = seen.json() if method == "POST" and len(parts) == 2 else None
        if body and "[bipagents:go_on]" in str(body.get("input")):
            posted.set()
            await asyncio.sleep(1.5)   # Hermes takes a moment to start it (the turn still closing)
        return await real(seen, agent, parts, method)

    monkeypatch.setattr(fakes.hermes, "_runs", slow_start)
    watch = {"agent": "wellness", "run_id": "run_vie", "session_id": "api_c"}
    assert bridge.post("/v1/watch", headers=AUTH, json=watch).status_code == 200
    assert posted.wait(10), "no continuation started"
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C0FFEE00-0000-4000-8000-0000000000F9",
                             "text": "Non laisse tomber, j'irai en magasin"})
    assert sent.status_code == 200 and sent.json()["state"] == "queued", sent.text
    assert wait_until(lambda: bridge.logged("promise with no work: the person stepped in, continuation stopped"),
                      timeout=10)
    # their message is the next turn, as after any turn that ended
    began = wait_until(lambda: any("laisse tomber" in str(s["input"]) for s in fakes.hermes.started.values()),
                       timeout=8)
    queue = bridge.get("/v1/agents/wellness/queue", headers=AUTH).json()
    assert began, (f"the person's message never started: paused={queue.get('paused')}, "
                   f"waiting={[i.get('text') for i in queue.get('items', [])]}, started={list(fakes.hermes.started)}")


def test_after_a_restart_a_routines_turn_is_never_continued(bridges, fakes):
    """A routine's run going on while the bridge restarts: `resume()` watches it again like a person's turn (end_push,
    not unattended: only background tasks are followed again by their pipeline); a run followed again is never
    continued (runs.rs `resumed`)."""
    bridge = bridges.new().start()
    fakes.hermes.rounds = 1000   # the routine's run still going for Hermes when the bridge comes back
    gate = threading.Event()
    made = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                       json={"name": "Point du matin", "schedule": "every day at 7:30", "prompt": "Résume mes e-mails."})
    assert made.status_code in (200, 201), made.text
    rid = made.json()["job"]["id"]
    fakes.hermes.runs["r1"] = [gate, sse("message.delta", run_id="r1", delta=PROMISE),
                               sse("run.completed", run_id="r1", output=PROMISE)]
    assert bridge.post(f"/v1/routines/{rid}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/r1/events"), timeout=20), list(fakes.hermes.started)
    assert "<!--bip:routine" in fakes.hermes.started["r1"]["input"]
    time.sleep(0.5)
    bridge.restart()
    assert wait_until(lambda: bridge.logged("run watch resumed"), timeout=15)
    gate.set()
    time.sleep(3)
    continued = [r for r, s in fakes.hermes.started.items() if "[bipagents:go_on]" in str(s["input"])]
    assert continued == [], f"a routine's turn continued after a restart: {continued}"


def test_ok_words_sent_before_the_start_do_start(bridges, fakes):
    """Control (passes): the same words caught by the check before the start (Hermes still closing the turn) start as
    the next turn: what the test above expects."""
    bridge = bridges.new().start()
    fakes.hermes.steer_status = 409
    thread = window(bridge, fakes)
    sent = bridge.post(f"/v1/threads/{thread}/messages", headers=AUTH,
                       json={"agent": "wellness", "client_id": "C0FFEE00-0000-4000-8000-0000000000FA",
                             "text": "Non laisse tomber, j'irai en magasin"})
    assert sent.status_code == 200 and sent.json()["state"] == "queued", sent.text
    fakes.hermes.start_failures = 0   # Hermes takes runs again (else the queue's own start meets the fake's 502)
    assert wait_until(lambda: any("laisse tomber" in str(s["input"]) for s in fakes.hermes.started.values()), timeout=8)
    assert not bridge.get("/v1/agents/wellness/queue", headers=AUTH).json().get("paused")
