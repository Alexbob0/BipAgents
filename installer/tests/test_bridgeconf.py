"""The bridge's config written by the installer (bridgeconf.py): one writer at a time across processes (the bridge takes
the same lock, bridge-rs/src/config.rs), whole, and never into a file that lost what it held. aibox, 2026-10-26: the
file cut in two, then replaced by four agents' voices."""
import multiprocessing
import os
import re
import sys
import threading
import time
import tomllib
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bipinstall import bridgeconf, family, moving, shared_hermes  # noqa: E402

AGENTS = 8
KEY = "k" * 40


def config_text() -> str:
    text = f'# keys inside: chmod 600.\nbridge_key = "{KEY}"\nport = 9210\n'
    for i in range(AGENTS):
        text += f'\n[agents.a{i}]\ndisplay_name = "A{i}"\nhermes_url = "http://127.0.0.1:{8600 + i}"\n'
    return text


def whole(text: str) -> dict:
    data = tomllib.loads(text)
    assert data["bridge_key"] == KEY
    for i in range(AGENTS):
        assert data["agents"][f"a{i}"]["hermes_url"] == f"http://127.0.0.1:{8600 + i}"
    return data


def set_key(text: str, table: str, key: str, value: str) -> str:
    """`key = value` in `[table]` (the table added when missing), as the bridge's set_section_keys does."""
    lines = text.splitlines()
    try:
        start = lines.index(f"[{table}]")
    except ValueError:
        return text.rstrip("\n") + f"\n\n[{table}]\n{key} = {value}\n"
    end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("[")), len(lines))
    body = [line for line in lines[start + 1:end] if line.split("=")[0].strip() != key and line.strip()]
    rest = [""] + lines[end:] if end < len(lines) else []
    return "\n".join(lines[:start + 1] + body + [f"{key} = {value}"] + rest) + "\n"


def writer(path: str, worker: int, rounds: int) -> None:
    for round_ in range(rounds):
        bridgeconf.update(Path(path), lambda t: set_key(t, f"agents.a{worker % AGENTS}", "voice", f'"pocket:w{worker}-{round_}"'))
        bridgeconf.update(Path(path), lambda t: set_key(t, f"worker{worker}", "round", str(round_)))


def test_many_processes_and_threads_writing_keep_the_file_whole(tmp_path):
    path = tmp_path / "bridge.toml"
    path.write_text(config_text())
    done = threading.Event()
    seen = []

    def reader():
        while not done.is_set():
            seen.append(whole(path.read_text()))

    watching = threading.Thread(target=reader)
    watching.start()
    context = multiprocessing.get_context("spawn")
    processes = [context.Process(target=writer, args=(str(path), w, 15)) for w in range(6)]
    threads = [threading.Thread(target=writer, args=(str(path), 100 + w, 15)) for w in range(3)]
    for p in processes:
        p.start()
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    for p in processes:
        p.join()
        assert p.exitcode == 0
    done.set()
    watching.join()
    assert seen
    text = path.read_text()
    data = whole(text)
    for w in list(range(6)) + [100, 101, 102]:
        assert data[f"worker{w}"]["round"] == 14, text   # no write lost
    assert text.startswith("# keys inside: chmod 600.")
    assert path.stat().st_mode & 0o777 == 0o600
    assert not [p.name for p in tmp_path.iterdir() if p.name.endswith(".tmp")]


def hold_lock(path: str, held: "multiprocessing.synchronize.Event", seconds: float) -> None:
    with bridgeconf.locked(Path(path)):
        held.set()
        time.sleep(seconds)


def test_a_write_waits_for_another_process_lock(tmp_path):
    path = tmp_path / "bridge.toml"
    path.write_text(config_text())
    context = multiprocessing.get_context("spawn")
    held = context.Event()
    other = context.Process(target=hold_lock, args=(str(path), held, 0.6))
    other.start()
    assert held.wait(20)
    started = time.monotonic()
    bridgeconf.update(path, lambda t: set_key(t, "agents.a0", "voice", '"pocket:galet"'))
    assert time.monotonic() - started >= 0.3
    other.join()
    assert whole(path.read_text())["agents"]["a0"]["voice"] == "pocket:galet"
    assert (tmp_path / "bridge.toml.lock").stat().st_mode & 0o777 == 0o600


INCIDENT = ('[agents.vie]\nvoice = "pocket:alba"\n\n[agents.chi]\nvoice = "pocket:colibri"\n\n'
            '[agents.kerfy-studio]\nvoice = "pocket:azur"\n\n[agents.coach]\nvoice = "pocket:sable"\ngalet"\n')


@pytest.mark.parametrize("change, why", [
    (lambda t: INCIDENT, "isn't valid TOML"),
    (lambda t: "e: chmod 600.\n" + t, "isn't valid TOML"),
    (lambda t: t.replace(f'bridge_key = "{KEY}"\n', ""), "bridge_key"),
    (lambda t: t.replace('hermes_url = "http://127.0.0.1:8603"\n', ""), "agent a3"),
    (lambda t: t.split("\n[agents.a5]")[0] + "\n", "agent a5"),
    (lambda t: t + '\n[agents.new]\ndisplay_name = "New"\n', "agent new has no hermes_url"),
])
def test_a_change_that_loses_what_the_file_holds_is_refused(tmp_path, change, why, capsys):
    path = tmp_path / "bridge.toml"
    path.write_text(config_text())
    with pytest.raises(bridgeconf.BridgeConfigError, match=why):
        bridgeconf.update(path, change)
    assert path.read_text() == config_text()
    assert "the file stays as it was" in capsys.readouterr().err


def test_agents_named_may_go_and_any_when_the_file_is_replaced(tmp_path):
    path = tmp_path / "bridge.toml"
    path.write_text(config_text())
    assert bridgeconf.update(path, lambda t: t.split("\n[agents.a7]")[0] + "\n", removing=["A7"])
    assert "a7" not in tomllib.loads(path.read_text())["agents"]
    other = f'bridge_key = "{KEY}"\n\n[agents.z]\nhermes_url = "http://127.0.0.1:9000"\n'
    with pytest.raises(bridgeconf.BridgeConfigError):
        bridgeconf.write(path, other)
    assert bridgeconf.write(path, other, removing=bridgeconf.ANY)
    assert not bridgeconf.write(path, other, removing=bridgeconf.ANY)   # nothing to change
    # a new file
    fresh = tmp_path / "new" / "bridge.toml"
    assert bridgeconf.write(fresh, other)
    assert fresh.stat().st_mode & 0o777 == 0o600


def test_installer_writers_go_through_the_lock(tmp_path, monkeypatch):
    """The installer's writers change the file through bridgeconf.update (locked, checked), keeping the rest."""
    calls = []
    real = bridgeconf.update

    def counted(path, change, removing=()):
        calls.append(Path(path).name)
        return real(path, change, removing)
    monkeypatch.setattr(bridgeconf, "update", counted)
    path = tmp_path / "bridge.toml"
    path.write_text(config_text() + '\n[hermes]\nhub_url = "http://127.0.0.1:9209"\n\n[pocket]\nurl = "http://127.0.0.1:8098"\n')
    family.ensure_clients_file(path, path.read_text())
    assert shared_hermes.bridge_config(path, tmp_path / "hermes")
    moving.rewrite(iter([path]), [(re.compile("9209"), "BIPAGENTSPORT0X")], [(re.compile("BIPAGENTSPORT0X"), "9309")])
    data = whole(path.read_text())
    assert data["relay"]["clients_file"].endswith("relay-clients")
    assert data["hermes"]["hub_url"] == "http://127.0.0.1:9309" and data["hermes"]["root"]
    assert calls == ["bridge.toml"] * 3
