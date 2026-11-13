"""A turn that ends on a promise with no work done (bridge-rs/src/go_on.rs, runs.rs `go_on`; Alex 13/11, his « Vie »:
« Je te prépare 2-3 trouvailles pour 5 € », the turn over with no search). An answer without a tool call ends a Hermes
turn, so the bridge starts ONE continuation in the same conversation, told out of sight to do it now; its events follow
in the turn's stream after a `loop_rescue_started` notice, and its end is the turn's end. `[brake] go_on` in
bridge.toml."""
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
