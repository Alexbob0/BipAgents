"""A turn that ends on a promise with no work done (bridge-rs/src/go_on.rs, runs.rs `go_on`; Alex 13/11, his « Vie »:
« Je te prépare 2-3 trouvailles pour 5 € », the turn over with no search). An answer without a tool call ends a Hermes
turn, so the bridge starts ONE continuation in the same conversation, told out of sight to do it now; its events follow
in the turn's stream after a `loop_rescue_started` notice, and its end is the turn's end. `[brake] go_on` in
bridge.toml."""
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
        assert wait_until(lambda: len(bridge.logged("promise with no work: the continuation answered")) == i + 1)
    assert all(asked == got and status == "cancelled" for asked, got, status in targets), targets


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
