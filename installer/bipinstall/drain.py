"""Before an update restarts what runs (the bridge, the agents' Hermes), the turns going on end first.

2026-11-06: right after a release, `bipagents update` restarted Alex's bridge in the middle of a turn; its tool calls
failed, Hermes' MCP breaker paused the bridge's tools for a minute, and the agent said « j'ai un souci d'appel
d'outil ». Now, just before a restart:

- the bridge is asked (`POST /v1/update/drain`, bridge-rs/src/update_drain.rs): it answers once no turn of the
  person's goes on, or after the seconds given, and then answers the agents' tool calls « Mise à jour en cours :
  réessaie dans une minute » (not an error) until it restarts; `what: "gateways"` before the agents' Hermes restart,
  whose restart ends every run (`done()` after: the bridge serves the tools again);
- a bridge from before that route (the update that brings it): its run hub asked every few seconds instead
  (`GET /v1/place/move`, `turns_running`), until none or the seconds pass;
- no bridge answering (stopped, or restarting for `ANSWER_WAIT`): nothing to wait for.

One update waits `CEILING` in all at most (`Budget`): a place's update, one of a server's in a row (the seat agent),
never waits much longer than before.
"""
from __future__ import annotations

import time
from pathlib import Path
from typing import Callable, Optional, Tuple

import httpx

CEILING = 300.0       # seconds one update waits for turns, in all
ANSWER_WAIT = 20.0    # a bridge restarting: its answer waited for this long
POLL = 3.0            # an older bridge's run hub asked this often


def say(text: str = "") -> None:
    print(text, flush=True)


class Budget:
    """What is left of one update's wait (`CEILING`), shared by its drains."""

    def __init__(self, seconds: float = CEILING, clock: Callable[[], float] = time.monotonic):
        self.clock = clock
        self.deadline = clock() + seconds

    def left(self) -> float:
        return max(0.0, self.deadline - self.clock())


_budget: Optional[Budget] = None


def start_budget(seconds: float = CEILING) -> Budget:
    """The update's budget (upgrade.py, at its start): every drain of this run takes from it."""
    global _budget
    _budget = Budget(seconds)
    return _budget


def seconds_left(default: float = CEILING) -> float:
    return _budget.left() if _budget is not None else default


def address(config: Path) -> Tuple[str, str]:
    """(the bridge's base URL on this machine, its key), from its settings."""
    import tomllib
    conf = tomllib.loads(config.read_text(encoding="utf-8"))
    host = str(conf.get("host") or "127.0.0.1")
    host = "127.0.0.1" if host in ("0.0.0.0", "::", "") else host
    host = f"[{host}]" if ":" in host and not host.startswith("[") else host
    key = str(conf.get("bridge_key") or "")
    if not key and conf.get("bridge_key_file"):
        try:
            key = Path(str(conf["bridge_key_file"])).expanduser().read_text(encoding="utf-8").strip()
        except OSError:
            key = ""
    return f"http://{host}:{int(conf.get('port') or 8643)}", key


def _found_config() -> Optional[Path]:
    from .family import find_config
    try:
        return find_config(None)
    except SystemExit:
        return None


def until_quiet(config: Optional[Path] = None, what: str = "bridge", seconds: Optional[float] = None,
                client: Optional[httpx.Client] = None, sleep: Callable[[float], None] = time.sleep,
                clock: Callable[[], float] = time.monotonic) -> str:
    """Waits (at most `seconds`, by default what is left of the update's budget) until no turn goes on that a restart
    of `what` ("bridge" or "gateways") would cut. Says what it did, for the update's output."""
    config = config or _found_config()
    if config is None or not config.is_file():
        return "No bridge here: nothing to wait for."
    seconds = seconds_left() if seconds is None else max(0.0, seconds)
    try:
        base, key = address(config)
    except (OSError, ValueError) as exc:
        return f"The bridge's settings don't read ({type(exc).__name__}): nothing to wait for."
    own = client is None
    client = client or httpx.Client(timeout=httpx.Timeout(seconds + 60.0, connect=5.0))
    headers = {"Authorization": f"Bearer {key}"}
    started = clock()
    try:
        # A bridge restarting (the gateways' drain right after the bridge's own restart): its answer waited for.
        while True:
            try:
                resp = client.post(f"{base}/v1/update/drain", json={"seconds": seconds, "what": what},
                                   headers=headers, timeout=seconds + 60.0)
                break
            except httpx.TransportError:
                if clock() - started >= ANSWER_WAIT:
                    return "The bridge doesn't answer: nothing to wait for."
                sleep(2.0)
        if resp.status_code == 200:
            said = resp.json()
            waited = float(said.get("waited") or 0)
            if said.get("drained"):
                return (f"Waited {waited:.0f} s for the turns going on to end." if waited >= 1
                        else "No turn going on.")
            return f"{said.get('turns', 1)} turn(s) still going on after {waited:.0f} s: the update goes on."
        if resp.status_code not in (404, 405):
            return f"The bridge refused the wait ({resp.status_code}): the update goes on."
        return _older_bridge(client, base, headers, seconds, sleep, clock)
    except (httpx.HTTPError, ValueError) as exc:
        return f"The bridge's wait stopped ({type(exc).__name__}): the update goes on."
    finally:
        if own:
            client.close()


def _older_bridge(client: httpx.Client, base: str, headers: dict, seconds: float,
                  sleep: Callable[[float], None], clock: Callable[[], float]) -> str:
    """A bridge without the drain: its run hub asked (`GET /v1/place/move`) until no turn goes on, or `seconds`."""
    started = clock()
    while True:
        try:
            resp = client.get(f"{base}/v1/place/move", headers=headers, timeout=30.0)
            turns = int(resp.json().get("turns_running") or 0) if resp.status_code == 200 else None
        except (httpx.HTTPError, ValueError, TypeError, AttributeError):
            turns = None
        if turns is None:
            return "The bridge doesn't say whether a turn goes on: the update goes on."
        waited = clock() - started
        if turns == 0:
            return f"Waited {waited:.0f} s for the turns going on to end." if waited >= 1 else "No turn going on."
        if waited >= seconds:
            return f"{turns} turn(s) still going on after {waited:.0f} s: the update goes on."
        sleep(min(POLL, max(0.1, seconds - waited)))


def done(config: Optional[Path] = None, client: Optional[httpx.Client] = None) -> None:
    """The bridge serves the agents' tools again (after the gateways' restart; a restarted bridge needs none)."""
    config = config or _found_config()
    if config is None or not config.is_file():
        return
    try:
        base, key = address(config)
        own = client is None
        client = client or httpx.Client(timeout=10.0)
        try:
            client.post(f"{base}/v1/update/drain", json={"done": True}, headers={"Authorization": f"Bearer {key}"})
        finally:
            if own:
                client.close()
    except (OSError, ValueError, httpx.HTTPError):
        pass   # it goes back to normal by itself within two minutes
