"""The brake on a turn going round in circles (bridge-rs/src/loop_brake.rs, 2026-10-27): a run calling one tool too
many times, the same call again and again, or far too many tools is stopped (Hermes' `POST /v1/runs/{id}/stop`) and
the conversation gets a plain line (`bridge.notice`, code `loop_stopped`); the agents' write tools refuse the same
write twice in a turn, and too many writes. `[brake]` in bridge.toml."""
from bbkit.common import AUTH, read_sse, sse
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until


def follow(bridge, run_id):
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        return read_sse(resp)


def looping(run_id, count, tool="mcp_bipagents_calendar_add"):
    # the token lab's turn: the calendar again and again, each call's preview its own
    return ([sse("tool.started", run_id=run_id, tool=tool, preview=f"Test {i}") for i in range(count)]
            + [sse("run.completed", run_id=run_id, output="J'arrête là les vérifications calendrier.")])


def test_a_turn_going_round_in_circles_is_stopped_and_said(bridges, fakes):
    bridge = bridges.new().start()
    fakes.hermes.runs["run_loop"] = looping("run_loop", 20)
    events = follow(bridge, "run_loop")
    names = [name for name, _ in events]
    assert names.index("bridge.notice") == 11, "right after the 11th call of the same tool"
    notice = events[11][1]
    assert notice["code"] == "loop_stopped" and notice["reason"] == "same tool too many times (calendar_add)"
    assert "agenda" in notice["text"] or "calendar" in notice["text"]
    assert names.count("bridge.notice") == 1
    assert wait_until(lambda: fakes.hermes.seen("/v1/runs/run_loop/stop", "POST"))
    assert not fakes.hermes.steers, "stopped, not steered as long work"
    assert bridge.logged("going round in circles: run stopped")
    # replayed to an app that comes back
    assert "bridge.notice" in [name for name, _ in follow(bridge, "run_loop")]


def test_the_same_call_three_times_and_the_limits_in_the_config(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"same_tool": 50}
    bridge.config["jobs"] = {"handoff": False}   # no long-work steer here
    bridge.start()
    fakes.hermes.runs["run_same"] = (
        [sse("tool.started", run_id="run_same", tool="memory", preview="add: le code du portail") for _ in range(3)]
        + [sse("run.completed", run_id="run_same", output="ok")])
    notices = [data for name, data in follow(bridge, "run_same") if name == "bridge.notice"]
    assert [n["reason"] for n in notices] == ["same call repeated (memory)"]
    # 20 different calendar calls: under the configured 50
    fakes.hermes.runs["run_ok"] = looping("run_ok", 20)
    assert "bridge.notice" not in [name for name, _ in follow(bridge, "run_ok")]
    assert not fakes.hermes.seen("/v1/runs/run_ok/stop", "POST")


def test_the_brake_switched_off(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"enabled": False}
    bridge.config["jobs"] = {"handoff": False}
    bridge.start()
    fakes.hermes.runs["run_off"] = looping("run_off", 15)
    assert "bridge.notice" not in [name for name, _ in follow(bridge, "run_off")]


def test_writes_in_a_turn_are_capped(bridges, fakes):
    bridge = bridges.new()
    bridge.config["brake"] = {"writes": 2}
    bridge.start()
    give_token(bridge)
    page = {"title": "Plan", "html": "<h1>Plan</h1>"}
    first = call(bridge, "create_page", page)
    assert not first["isError"], text_of(first)
    # with no run followed, the same write again is not refused (the person may have asked again): only counted
    # (the same write twice within a run: the Rust unit tests, loop_brake.rs)
    assert not call(bridge, "create_page", page)["isError"]
    refused = call(bridge, "create_page", {**page, "title": "Plan 3"})
    assert refused["isError"] and "you already made 2 changes" in text_of(refused)
    # reading is never capped
    assert not call(bridge, "list_pages", {})["isError"]
