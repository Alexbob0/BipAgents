"""The black-box suite's fixtures: the fakes (started once), the fake executables, an APNs key, and the bridges of each
test (separate processes, `BRIDGE_CMD`). Nothing here imports the bridge's code."""
from __future__ import annotations

import json
import os
import signal
import sys
import time
from pathlib import Path
from typing import Any

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from bbkit.bridge import Bridge, Bridges  # noqa: E402
from bbkit.fakebin import install  # noqa: E402
from bbkit.fakes import Fakes  # noqa: E402


def pytest_configure(config):
    config.addinivalue_line("markers", "slow: waits for the bridge's own timers (10 s and more); -m 'not slow' skips them")
    # A run stopped with `kill` ends like Ctrl-C: the test's bridges are stopped. They run in a session of their own
    # (`start_new_session`), so a run killed plainly left its bridge running on, timers and ports (2026-11-01).
    signal.signal(signal.SIGTERM, _interrupted)


def _interrupted(signum, frame):
    raise KeyboardInterrupt(f"signal {signum}")


@pytest.fixture(scope="session")
def all_fakes():
    fakes = Fakes().start()
    yield fakes
    fakes.stop()


@pytest.fixture
def fakes(all_fakes):
    all_fakes.reset()
    yield all_fakes
    all_fakes.release()


@pytest.fixture(scope="session")
def fakebin(tmp_path_factory) -> Path:
    return install(tmp_path_factory.mktemp("bin"))


@pytest.fixture(scope="session")
def p8_path(tmp_path_factory) -> str:
    key = ec.generate_private_key(ec.SECP256R1())
    pem = key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                            serialization.NoEncryption())
    path = tmp_path_factory.mktemp("apns") / "AuthKey_TESTKEY123.p8"
    path.write_bytes(pem)
    os.chmod(path, 0o600)
    return str(path)


@pytest.hookimpl(hookwrapper=True)
def pytest_runtest_makereport(item, call):
    outcome = yield
    report = outcome.get_result()
    setattr(item, f"report_{report.when}", report)


@pytest.fixture
def bridges(request, tmp_path, fakes, fakebin, p8_path):
    made = Bridges(tmp_path, fakes, fakebin, p8_path)
    yield made
    made.stop()
    failed = getattr(request.node, "report_call", None)
    if failed is not None and failed.failed:
        for bridge in made.made:   # shown under « Captured stdout teardown »
            print(f"\n----- {bridge.name}'s log ({bridge.log_path}) -----\n{bridge.log_tail(80)}")


@pytest.fixture
def bridge(bridges) -> Bridge:
    """The default bridge (tests/conftest.py's `client`), started."""
    return bridges.new().start()


def managed_install(bridge: Bridge, fakes, agents=("coach",), **hermes: Any) -> Path:
    """`[hermes]` set: this install manages its own Hermes (the fake one, its profiles under the test's folder) and
    serves `agents` from the hub (`<hub>/p/<name>`). Returns the hub's HERMES_HOME."""
    home = bridge.root / ".hermes"
    (home / "profiles").mkdir(parents=True, exist_ok=True)
    bridge.config["hermes"] = {"bin": str(bridge.fakebin / "hermes"), "home": str(home), "hub_url": fakes.hermes.url,
                               **hermes}
    for name in agents:
        bridge.config["agents"][name] = {"display_name": name.title(), "hermes_url": f"{fakes.hermes.url}/p/{name}",
                                         "hermes_key": "k" * 32}
    return home


def profile(home: Path, name: str) -> Path:
    folder = home / "profiles" / name
    folder.mkdir(parents=True, exist_ok=True)
    return folder


def marked(folder: Path) -> bool:
    """Whether a profile has Hermes' Bot Mode mark (`ui_meta.hermes-bots` in its profile.yaml): its Bot Chat then
    gets Hermes' own message_agent. The bridge marks only an agent that writes to another household (peers.rs)."""
    import yaml
    path = folder / "profile.yaml"
    data = yaml.safe_load(path.read_text()) if path.exists() else None
    return "hermes-bots" in ((data or {}).get("ui_meta") or {})


def servers(folder: Path) -> dict:
    """The `mcp_servers` of a profile's config.yaml (the bridge writes its entries there itself)."""
    import yaml
    path = folder / "config.yaml"
    data = yaml.safe_load(path.read_text()) if path.exists() else None
    return (data or {}).get("mcp_servers") or {}


def wait_registered(bridge: Bridge, agents) -> None:
    """At start, the bridge gives each agent it manages its tools (its entry written into the profile, in the
    background, then the card payment and vault account entries where they are offered): waited for, so a test's
    own commands and token come after them."""
    from bbkit.servers import wait_until

    def done() -> bool:
        tokens = bridge.state_dir / "agent-tools.json"   # written after the entry: a test's own token comes after
        kept = set(json.loads(tokens.read_text())["tokens"]) if tokens.exists() else set()
        return set(agents) <= kept
    assert wait_until(done, timeout=15), bridge.commands.of("hermes")
    # The other entries follow the `bipagents` one: done when no new command came for a while (a slow runner).
    count = [len(bridge.commands.of("hermes")), time.monotonic()]

    def quiet() -> bool:
        now = len(bridge.commands.of("hermes"))
        if now != count[0]:
            count[:] = [now, time.monotonic()]
        return time.monotonic() - count[1] > 0.5
    assert wait_until(quiet, timeout=15), bridge.commands.of("hermes")
