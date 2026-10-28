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
