"""Before an update restarts the bridge or the agents' Hermes, the turns going on end first (bipinstall/drain.py,
bridge-rs/src/update_drain.rs; 2026-11-06: a turn cut by `bipagents update` right after a release)."""
import json
import sys
from pathlib import Path

import httpx

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bipinstall import drain, update, upgrade  # noqa: E402

KEY = "k" * 40


class Clock:
    """time.monotonic and time.sleep, without waiting."""

    def __init__(self):
        self.now = 1000.0
        self.slept = []

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.slept.append(seconds)
        self.now += seconds


def settings(tmp_path, text=None):
    (tmp_path / "key").write_text(KEY + "\n")
    path = tmp_path / "bridge.toml"
    path.write_text(text or f'host = "0.0.0.0"\nport = 8700\nbridge_key_file = "{tmp_path / "key"}"\n')
    return path


def client(handler, seen):
    def record(request):
        seen.append(request)
        return handler(request)
    return httpx.Client(transport=httpx.MockTransport(record))


def test_the_bridge_is_asked_to_wait_with_its_key_on_this_machine(tmp_path):
    seen = []
    answer = {"drained": True, "turns": 0, "waited": 42.3, "closing_seconds": 120}
    said = drain.until_quiet(settings(tmp_path), "bridge", 300,
                             client(lambda r: httpx.Response(200, json=answer), seen))
    assert said == "Waited 42 s for the turns going on to end."
    (asked,) = seen
    assert str(asked.url) == "http://127.0.0.1:8700/v1/update/drain" and asked.method == "POST"
    assert asked.headers["authorization"] == f"Bearer {KEY}"
    assert json.loads(asked.content) == {"seconds": 300, "what": "bridge"}
    # nothing going on; a turn still going on at the ceiling: the update goes on either way
    quiet = {"drained": True, "turns": 0, "waited": 0.1}
    assert drain.until_quiet(settings(tmp_path), "gateways", 300,
                             client(lambda r: httpx.Response(200, json=quiet), [])) == "No turn going on."
    capped = {"drained": False, "turns": 2, "waited": 300.2}
    assert drain.until_quiet(settings(tmp_path), "bridge", 300, client(lambda r: httpx.Response(200, json=capped), [])) \
        == "2 turn(s) still going on after 300 s: the update goes on."


def test_an_older_bridge_has_its_run_hub_asked_until_no_turn_goes_on(tmp_path):
    clock, seen = Clock(), []
    turns = iter([2, 1, 0])

    def handler(request):
        if request.url.path == "/v1/update/drain":
            return httpx.Response(404, json={"detail": "Not Found"})
        return httpx.Response(200, json={"state": "idle", "turns_running": next(turns), "jobs_running": []})

    said = drain.until_quiet(settings(tmp_path, f'port = 8643\nbridge_key = "{KEY}"\n'), "bridge", 300,
                             client(handler, seen), sleep=clock.sleep, clock=clock)
    assert said == "Waited 6 s for the turns going on to end."
    assert [r.url.path for r in seen] == ["/v1/update/drain"] + ["/v1/place/move"] * 3
    assert all(r.headers["authorization"] == f"Bearer {KEY}" for r in seen)
    assert clock.slept == [drain.POLL, drain.POLL]

    # a turn that never ends: until the seconds given, then the update goes on
    clock = Clock()
    always = lambda r: (httpx.Response(404) if r.url.path == "/v1/update/drain"   # noqa: E731
                        else httpx.Response(200, json={"turns_running": 1}))
    said = drain.until_quiet(settings(tmp_path), "bridge", 10, client(always, []), sleep=clock.sleep, clock=clock)
    assert said == "1 turn(s) still going on after 10 s: the update goes on." and sum(clock.slept) == 10


def test_no_bridge_answering_is_nothing_to_wait_for(tmp_path):
    clock, seen = Clock(), []

    def down(request):
        raise httpx.ConnectError("refused", request=request)

    said = drain.until_quiet(settings(tmp_path), "gateways", 300, client(down, seen), sleep=clock.sleep, clock=clock)
    assert said == "The bridge doesn't answer: nothing to wait for."
    assert clock.now - 1000.0 >= drain.ANSWER_WAIT and len(seen) == len(clock.slept) + 1   # a restarting one waited for
    assert drain.until_quiet(tmp_path / "none.toml", "bridge", 300) == "No bridge here: nothing to wait for."
    refused = drain.until_quiet(settings(tmp_path), "bridge", 300, client(lambda r: httpx.Response(503), []))
    assert refused == "The bridge refused the wait (503): the update goes on."   # a place moving


def test_one_update_waits_five_minutes_in_all(monkeypatch):
    clock = Clock()
    budget = drain.Budget(drain.CEILING, clock)
    monkeypatch.setattr(drain, "_budget", budget)
    assert drain.seconds_left() == 300
    clock.now += 280   # the bridge's drain waited that long
    assert drain.seconds_left() == 20   # the gateways' gets the rest
    clock.now += 60
    assert drain.seconds_left() == 0
    monkeypatch.setattr(drain, "_budget", None)
    assert drain.seconds_left() == drain.CEILING   # `bipagents hermes-update` alone


def test_done_tells_the_bridge_to_serve_the_tools_again(tmp_path):
    seen = []
    drain.done(settings(tmp_path), client(lambda r: httpx.Response(200, json={"state": "idle"}), seen))
    assert [json.loads(r.content) for r in seen] == [{"done": True}]

    def down(request):
        raise httpx.ConnectError("refused", request=request)
    drain.done(settings(tmp_path), client(down, []))   # it goes back to normal by itself: nothing raised


class Runner:
    def __init__(self, order, dry=False):
        self.order, self.dry, self.log = order, dry, []

    def run(self, args, check=True, **kw):
        self.order.append(" ".join(args))


def test_the_update_drains_before_it_restarts_the_bridge(tmp_path, monkeypatch):
    order = []
    monkeypatch.setattr(drain, "until_quiet", lambda config=None, what="bridge", **kw: order.append(f"drain {what}")
                        or "No turn going on.")
    monkeypatch.setattr(upgrade.rustbridge, "restart", lambda home: order.append("restart the bridge's service"))
    upgrade.restart_bridge(Runner(order), tmp_path / "bridge.toml", "Bridge abc in place.")
    assert order == ["drain bridge", "restart the bridge's service"]
    order.clear()
    monkeypatch.setattr("bipinstall.family.bridge_restart", lambda home=None: ["systemctl", "--user", "restart", "b"])
    upgrade.restart_bridge(Runner(order), tmp_path / "bridge.toml", None)
    assert order == ["drain bridge", "systemctl --user restart b"]
    order.clear()
    upgrade.restart_bridge(Runner(order, dry=True), tmp_path / "bridge.toml", None)
    assert order == ["systemctl --user restart b"]   # a dry run asks nothing


def test_the_gateways_restart_after_their_turns_then_the_tools_are_served_again(monkeypatch):
    order = []
    monkeypatch.setattr(drain, "until_quiet", lambda config=None, what="bridge", **kw: order.append(f"drain {what}")
                        or "No turn going on.")
    monkeypatch.setattr(drain, "done", lambda config=None, client=None: order.append("done"))
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes"),
                                                          ("systemd", "bipagents-space-vie")])
    update.restart(Runner(order))
    assert order == ["drain gateways", "systemctl --user restart bipagents-hermes",
                     "systemctl --user restart bipagents-space-vie", "done"]
