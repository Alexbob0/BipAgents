"""An agent's Hermes restarted when there is a reason, only when idle (bridge-rs/src/agent_restart.rs, 2026-10-28):
unhealthy (its pings fail), memory (its gateway's unit too big, at the night's hour), requested by other code. No
reason, or an existing Hermes the person brought: never. A busy agent (a turn going on) waits, then is restarted once
free. The last restart and its reason show in /v1/status. The loop runs on short timers here (the config)."""
import json
import threading
import time

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until

FAST = {"tick_seconds": 0.3, "ping_seconds": 0.3, "measure_seconds": 0.3, "unhealthy_pings": 2,
        "retry_seconds": 1, "jitter_minutes": 0}


def start(bridges, fakes, existing=False, name="bridge", **settings):
    bridge = bridges.new(name)
    bridge.config["hermes"] = {"bin": str(bridge.fakebin / "hermes")}
    bridge.config["agent_restart"] = {**FAST, **settings}
    if existing:   # installer/bipinstall/existing.py's record: a Hermes the person had before
        (bridge.root / "existing-hermes.json").write_text(json.dumps({"preexisting": True, "gateway": "service"}))
    return bridge


def restarts(bridge):
    return [a for a in bridge.commands.of("hermes") if a[-2:] == ["gateway", "restart"]] + \
        [a for a in bridge.commands.of("systemctl") if a[:2] == ["--user", "restart"]]


def last_restart(bridge):
    return bridge.get("/v1/status", headers=AUTH).json()["agents"]["wellness"].get("last_restart")


def test_an_unhealthy_idle_agent_is_restarted_once(bridges, fakes):
    bridge = start(bridges, fakes).start()
    fakes.hermes.capabilities[""] = 503   # its Hermes stops answering
    assert wait_until(lambda: len(restarts(bridge)) == 1, timeout=15)
    time.sleep(2)
    assert len(restarts(bridge)) == 1   # at most once an hour
    fakes.hermes.capabilities[""] = 200
    last = last_restart(bridge)
    assert last["reason"].startswith("unhealthy") and last["at"] > time.time() - 60


def test_a_busy_agent_waits_then_is_restarted_when_free(bridges, fakes):
    bridge = start(bridges, fakes).start()
    gate = threading.Event()
    fakes.hermes.runs["run_busy"] = [gate, sse("run.completed", output="Fini.")]
    events = []

    def follow():
        with bridge.stream("GET", "/v1/runs/run_busy/events?agent=wellness", headers=AUTH, timeout=60) as resp:
            for chunk in resp.iter_raw():
                events.append(chunk)

    follower = threading.Thread(target=follow, daemon=True)
    follower.start()
    assert wait_until(lambda: fakes.hermes.event_requests, timeout=10)
    fakes.hermes.capabilities[""] = 503
    time.sleep(3)
    assert restarts(bridge) == []   # a turn goes on: not now
    assert "restart waits" in bridge.log_tail(200)
    gate.set()
    follower.join(timeout=10)
    assert wait_until(lambda: len(restarts(bridge)) == 1, timeout=15)
    fakes.hermes.capabilities[""] = 200


def test_too_much_memory_restarts_it_at_the_night_hour(bridges, fakes):
    bridge = start(bridges, fakes, window_start=0, window_end=24, hour=0)
    units = bridge.home / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    (units / "hermes-gateway.service").write_text("[Service]\n")
    bridge.commands.rule("systemctl", ["--user", "show"],
                         stdout="MemoryCurrent=2147483648\nActiveEnterTimestampMonotonic=5\n")
    bridge.start()
    assert wait_until(lambda: ["--user", "restart", "hermes-gateway"] in restarts(bridge), timeout=15)
    time.sleep(2)
    assert len(restarts(bridge)) == 1   # once a night
    assert last_restart(bridge)["reason"].startswith("memory: Hermes uses 2048 MB")


def test_no_reason_no_restart_and_an_existing_hermes_never(bridges, fakes):
    healthy = start(bridges, fakes, window_start=0, window_end=24, hour=0).start()
    time.sleep(3)
    assert restarts(healthy) == [] and last_restart(healthy) is None
    healthy.stop()

    theirs = start(bridges, fakes, existing=True, name="theirs").start()
    fakes.hermes.capabilities[""] = 503
    time.sleep(3)
    assert restarts(theirs) == []
    fakes.hermes.capabilities[""] = 200
    theirs.stop()


# -- a tools change (2026-11-06: right after a release, « tools » restarts while a turn went on) ---------------------

from bbkit.mcp import TOKEN, give_token  # noqa: E402


def listed_tools(bridge):
    """The agent's Hermes as it connects: a session, its tools listed (what the bridge compares its tools with)."""
    head = {"Authorization": f"Bearer {TOKEN}"}
    init = bridge.post("/v1/agent-tools/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
                       headers=head)
    session = init.headers["mcp-session-id"]
    listed = bridge.post("/v1/agent-tools/mcp", json={"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
                         headers={**head, "mcp-session-id": session})
    assert listed.status_code == 200 and listed.json()["result"]["tools"]


def change_tools(bridge):
    """A tool more for the agent (its calendar shared with it): its Hermes keeps the list it got."""
    assert bridge.put("/v1/calendar/agents", json={"agents": ["wellness"]}, headers=AUTH).status_code == 200


def follow_gated(bridge, fakes, run_id="run_long"):
    gate = threading.Event()
    fakes.hermes.runs[run_id] = [gate, sse("run.completed", output="Fini.")]

    def follow():
        with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent=wellness", headers=AUTH, timeout=60) as resp:
            for _ in resp.iter_raw():
                pass

    follower = threading.Thread(target=follow, daemon=True)
    follower.start()
    assert wait_until(lambda: fakes.hermes.event_requests, timeout=10)
    return gate, follower


def tools_bridge(bridges, fakes, **settings):
    bridge = start(bridges, fakes, stale_tools_seconds=0.5, **settings)
    give_token(bridge)
    return bridge.start()


def test_a_tools_change_waits_for_the_turn_going_on_then_restarts_once(bridges, fakes):
    bridge = tools_bridge(bridges, fakes)
    listed_tools(bridge)
    gate, follower = follow_gated(bridge, fakes)
    change_tools(bridge)
    assert wait_until(lambda: "restart waits" in bridge.log_tail(200), timeout=15)
    time.sleep(2)
    assert restarts(bridge) == []   # the turn goes on: not now
    gate.set()
    follower.join(timeout=10)
    assert wait_until(lambda: len(restarts(bridge)) == 1, timeout=15)
    time.sleep(1.5)
    assert len(restarts(bridge)) == 1   # one restart
    assert last_restart(bridge)["reason"] == "requested: tools"


def test_a_turn_only_its_hermes_knows_holds_the_restart_too(bridges, fakes):
    # a bridge restarted by the update doesn't follow the turns started before it: its Hermes' own count does
    bridge = tools_bridge(bridges, fakes)
    fakes.hermes.busy[""] = 1
    listed_tools(bridge)
    change_tools(bridge)
    assert wait_until(lambda: "its Hermes runs a turn" in bridge.log_tail(200), timeout=15)
    time.sleep(2)
    assert restarts(bridge) == []
    fakes.hermes.busy[""] = 0   # its turn ended
    assert wait_until(lambda: len(restarts(bridge)) == 1, timeout=15)
    # the status names the restart once it is over, a moment after its command ran (public CI 7/11)
    assert wait_until(lambda: last_restart(bridge) is not None, timeout=15)
    assert last_restart(bridge)["reason"] == "requested: tools"


def test_a_turn_past_the_ceiling_does_not_hold_the_restart_for_ever(bridges, fakes):
    bridge = tools_bridge(bridges, fakes, turn_wait_seconds=3)
    listed_tools(bridge)
    gate, follower = follow_gated(bridge, fakes)
    change_tools(bridge)
    assert wait_until(lambda: "restart waits" in bridge.log_tail(200), timeout=15)
    assert restarts(bridge) == []
    assert wait_until(lambda: len(restarts(bridge)) == 1, timeout=20)   # its 3 s ceiling
    assert "its ceiling reached" in bridge.log_tail(200)
    gate.set()
    follower.join(timeout=10)
