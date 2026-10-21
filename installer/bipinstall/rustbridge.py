"""The bridge in Rust (bridge-rs/, decided 2026-10-21): the same routes, files and settings as the Python one
(bridge/bipbridge), in far less memory. GitHub Actions builds its binaries (.github/workflows/bridge-release.yml) for
Linux x86_64 and ARM — Windows runs BipAgents in WSL2, so Linux too — and for macOS, and publishes them as the release
`bridge-<key>`. The key is a hash of the sources they were built from (bridge-rs/ and what it embeds, bridge/bipbridge/),
so a checkout knows which release is its own without asking anyone.

The engine is each account's choice: ~/.config/bipagents/bridge-engine says « rust » or « python » (none: python, until
the Rust one has passed its tests on real installs).

    bipagents bridge rust      this checkout's binaries, checked against the release's SHA256SUMS, into
                               ~/.local/share/bipagents/bridge/<key>/; the service then runs them
    bipagents bridge python    back to the Python bridge (the binaries stay, for a later switch)
    bipagents bridge           which one runs, and whether this version's binaries are out yet

`bipagents update` keeps the binaries in step with the code: a new key, its binaries, a restart.
"""
from __future__ import annotations

import hashlib
import io
import os
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path
from typing import Callable, List, Optional

REPO = Path(__file__).resolve().parents[2]
SLUG = "Alexbob0/BipAgents"
ENGINE = Path(".config") / "bipagents" / "bridge-engine"
BINARIES = Path(".local") / "share" / "bipagents" / "bridge"
KEEP = 2                     # binaries kept: this version's and the one before (a quick way back)
SUMS = "SHA256SUMS"

Opener = Callable[[str], bytes]


class RustBridgeError(Exception):
    """Safe to show."""


def key(repo: Path = REPO) -> Optional[str]:
    """This checkout's release key: the same hash the workflow computes (None without bridge-rs/)."""
    trees = []
    for path in ("bridge-rs", "bridge/bipbridge"):
        done = subprocess.run(["git", "-C", str(repo), "rev-parse", f"HEAD:{path}"], capture_output=True, text=True)
        if done.returncode != 0:
            return None
        trees.append(done.stdout.strip())
    return hashlib.sha256(" ".join(trees).encode()).hexdigest()[:12]


def asset(system: Optional[str] = None, machine: Optional[str] = None) -> Optional[str]:
    """The archive for this machine (None: no build for it)."""
    system = system or platform.system()
    machine = (machine or platform.machine()).lower()
    if system == "Darwin":
        return "bipbridge-macos-universal.tar.gz"
    if system == "Linux" and machine in ("x86_64", "amd64"):
        return "bipbridge-linux-x86_64.tar.gz"
    if system == "Linux" and machine in ("aarch64", "arm64"):
        return "bipbridge-linux-aarch64.tar.gz"
    return None


def engine(home: Optional[Path] = None) -> str:
    try:
        value = ((home or Path.home()) / ENGINE).read_text(encoding="utf-8").strip()
    except OSError:
        return "python"
    return value if value in ("rust", "python") else "python"


def set_engine(home: Path, value: str) -> None:
    path = home / ENGINE
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(value + "\n", encoding="utf-8")


def binary(home: Path, release: str) -> Path:
    return home / BINARIES / release / "bipbridge"


def _download(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as resp:   # GitHub answers with a redirect to its file store
        return resp.read()


def fetch(home: Path, release: str, name: str, opener: Opener = _download) -> Path:
    """This release's binaries, checked, in place; the oldest beyond KEEP removed."""
    target = binary(home, release)
    if target.exists():
        return target
    base = f"https://github.com/{SLUG}/releases/download/bridge-{release}"
    try:
        sums = opener(f"{base}/{SUMS}").decode()
        archive = opener(f"{base}/{name}")
    except Exception as exc:   # noqa: BLE001 - 404 while the workflow still builds, or no network
        raise RustBridgeError(f"the Rust bridge of this version isn't out yet ({type(exc).__name__}); "
                              "`bipagents update` again in a few minutes") from exc
    expected = next((line.split()[0] for line in sums.splitlines() if line.strip().endswith(name)), None)
    if expected is None or hashlib.sha256(archive).hexdigest() != expected:
        raise RustBridgeError(f"{name} doesn't match the release's {SUMS}: not installed")
    folder = target.parent
    folder.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f".{release}-", dir=folder.parent))
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
            for member in tar.getmembers():
                if member.name not in ("bipbridge", "bip-browser-mcp") or not member.isfile():
                    continue   # only the two programs, nothing else written anywhere
                data = tar.extractfile(member)
                (staging / member.name).write_bytes(data.read() if data else b"")
                (staging / member.name).chmod(0o755)
        if not (staging / "bipbridge").exists():
            raise RustBridgeError(f"{name} holds no bipbridge program")
        os.replace(staging, folder)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    kept = sorted((p for p in folder.parent.iterdir() if p.is_dir() and not p.name.startswith(".")),
                  key=lambda p: p.stat().st_mtime, reverse=True)
    for old in kept[KEEP:]:
        if old != folder:
            shutil.rmtree(old, ignore_errors=True)
    return target


def program(home: Path, repo: Path = REPO) -> List[str]:
    """What the bridge's service runs: the Rust binary of this checkout when chosen and there, else Python."""
    release = key(repo)
    if engine(home) == "rust" and release and binary(home, release).exists():
        return [str(binary(home, release)), "serve"]
    return [str(repo / "bridge" / ".venv" / "bin" / "python"), "-m", "bipbridge", "serve"]


def cli(home: Path, repo: Path = REPO) -> List[str]:
    """The bridge's own command line (`browser add --all`, `qr`…), from the engine that runs."""
    run = program(home, repo)
    return run[:-1]


def rewrite_service(home: Path, run: List[str]) -> Optional[Path]:
    """The installed service, its program swapped (systemd's ExecStart, launchd's ProgramArguments)."""
    if sys.platform == "darwin":
        for plist in sorted((home / "Library" / "LaunchAgents").glob("*.plist")):
            text = plist.read_text(errors="ignore")
            if "bipbridge" in text:
                args = "".join(f"<string>{part}</string>" for part in run)
                plist.write_text(re.sub(r"(<key>ProgramArguments</key>\s*<array>).*?(</array>)",
                                        lambda m: m.group(1) + args + m.group(2), text, count=1, flags=re.S))
                return plist
        return None
    for unit in sorted((home / ".config" / "systemd" / "user").glob("*.service")):
        text = unit.read_text(errors="ignore")
        if re.search(r"^ExecStart=.*bipbridge", text, flags=re.M):
            unit.write_text(re.sub(r"^ExecStart=.*$", "ExecStart=" + " ".join(run), text, count=1, flags=re.M))
            return unit
    return None


def restart(home: Path) -> None:
    from .family import bridge_restart
    if sys.platform != "darwin":
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=False)
    else:   # launchd reads the plist again only when loaded again
        for plist in sorted((home / "Library" / "LaunchAgents").glob("*.plist")):
            if "bipbridge" in plist.read_text(errors="ignore"):
                subprocess.run(["launchctl", "unload", str(plist)], check=False)
                subprocess.run(["launchctl", "load", str(plist)], check=False)
                return
    subprocess.run(bridge_restart(home), check=False)


def keep_in_step(home: Path, repo: Path = REPO) -> Optional[str]:
    """After a pull (`bipagents update`): this version's binaries for a Rust bridge, the service on them. Says what it
    did, or why it kept the previous ones (None: the Python bridge runs, nothing to do)."""
    if engine(home) != "rust":
        return None
    release, name = key(repo), asset()
    if not release or not name:
        return "The Rust bridge has no build for this machine: the previous one keeps running."
    try:
        fetch(home, release, name)
    except RustBridgeError as exc:
        return str(exc)
    rewrite_service(home, program(home, repo))
    return f"Rust bridge {release} in place."


def main(argv: List[str]) -> int:
    from . import ui
    home = Path.home()
    choice = argv[0] if argv else ""
    release, name = key(), asset()
    if choice not in ("rust", "python"):
        print(f"Bridge: {engine(home)}" + (f" (Rust {release}, {'downloaded' if release and binary(home, release).exists() else 'not downloaded'})"
                                         if release else " (this version has no Rust bridge)"))
        print("Change it: bipagents bridge rust | bipagents bridge python")
        return 0 if not choice else 2
    if choice == "rust":
        if not release or not name:
            print(ui.fail("No Rust bridge for this machine or this version."))
            return 1
        try:
            fetch(home, release, name)
        except RustBridgeError as exc:
            print(ui.fail(str(exc)))
            return 1
    set_engine(home, choice)
    if rewrite_service(home, program(home)) is None:
        print(ui.fail("I can't find the bridge's service in this account."))
        return 1
    restart(home)
    browsers_follow(home)
    print(ui.done(f"The bridge now runs in {'Rust' if choice == 'rust' else 'Python'}."
                  + ("  Back: bipagents bridge python" if choice == "rust" else "")))
    return 0


def browsers_follow(home: Path) -> None:
    """Each agent's browser tools (its `stealth` MCP entry) run the helper of the bridge that runs: the Rust bridge's
    bip-browser-mcp, or Python's browser_mcp.py. Written again by the bridge's own `browser add --all`."""
    from .family import find_config
    if not list((home / ".config" / "systemd" / "user").glob("bipagents-browser-*.service")) and sys.platform != "darwin":
        return   # no agent browser here
    try:
        config = find_config(None)
    except SystemExit:
        return
    subprocess.run(cli(home) + ["browser", "add", "--all", "--config", str(config)], cwd=REPO / "bridge", check=False)

