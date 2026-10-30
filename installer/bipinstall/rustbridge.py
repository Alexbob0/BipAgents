"""The bridge (bridge-rs/, in Rust since 2026-10-21; the Python one it replaced stays in the history). GitHub Actions
builds its binaries (.github/workflows/bridge-release.yml) for Linux x86_64 and ARM — Windows runs BipAgents in WSL2,
so Linux too — and for macOS, and publishes them as the release `bridge-<key>`. The key is a hash of the sources they
were built from (the tree of bridge-rs/, which holds everything the binary embeds: bridge-rs/assets/), so a checkout
knows which release is its own without asking anyone.

    bipagents bridge           which version runs, and whether its binaries are here
    bipagents bridge rust      this checkout's binaries again (checked against the release's signed SHA256SUMS, into
                               ~/.local/share/bipagents/bridge/<key>/), the service put back on them, restarted
    bipagents bridge browser add --agent vie
                               the bridge's own command line (check, qr, peer-token, browser, spaces…), on this
                               account's config

A machine with no build of ours (another processor, another system) can run binaries built on it (`cargo build
--release` in bridge-rs/): BIPAGENTS_BRIDGE_BUILD=<their folder> for the install or `bipagents bridge rust`.

`bipagents update` keeps the binaries in step with the code: a new key, its binaries, a restart. An install still on
the Python bridge is moved to them by its first update (migrations.py, rust-bridge).
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
import time
import urllib.request
from pathlib import Path
from typing import Callable, List, Optional

from . import release_key

REPO = Path(__file__).resolve().parents[2]
SLUG = "Alexbob0/BipAgents"
ENGINE = Path(".config") / "bipagents" / "bridge-engine"   # « rust » or « python » while both existed; removed now
BINARIES = Path(".local") / "share" / "bipagents" / "bridge"
KEEP = 2                     # binaries kept: this version's and the one before (a quick way back)
SUMS = "SHA256SUMS"
BUILD = "BIPAGENTS_BRIDGE_BUILD"   # a folder of binaries built on this machine, for one with no build of ours
PROGRAMS = ("bipbridge", "bip-browser-mcp")
# The bridge's own subcommands, passed on by `bipagents bridge <command> …` (bridge-rs/src/main_cli.rs).
COMMANDS = ("check", "genkey", "qr", "peer-token", "browser", "shares-mounts", "console", "spaces", "adopt")

Opener = Callable[[str], bytes]


class RustBridgeError(Exception):
    """Safe to show."""


def key(repo: Path = REPO) -> Optional[str]:
    """This checkout's release key, the same hash the workflow computes: sha256 of the tree id of bridge-rs/ (the
    binaries are built from that folder alone, their embedded files in bridge-rs/assets/), 12 characters. None
    without bridge-rs/ (or outside a git checkout)."""
    return key_at(repo, "HEAD")


def key_at(repo: Path, commit: str) -> Optional[str]:
    """The release key of another commit of the checkout (`key`)."""
    done = subprocess.run(["git", "-C", str(repo), "rev-parse", f"{commit}:bridge-rs"], capture_output=True, text=True)
    if done.returncode != 0:
        return None
    return hashlib.sha256(done.stdout.strip().encode()).hexdigest()[:12]


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


def binary(home: Path, release: str) -> Path:
    return home / BINARIES / release / "bipbridge"


def missed(home: Path, repo: Path = REPO) -> bool:
    """This checkout's binaries were never fetched: an update ran before the bridge's release was out."""
    release = key(repo)
    return bool(release) and not binary(home, release).exists()


def _download(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as resp:   # GitHub answers with a redirect to its file store
        return resp.read()


def _prune(folder: Path) -> None:
    kept = sorted((p for p in folder.parent.iterdir() if p.is_dir() and not p.name.startswith(".")),
                  key=lambda p: p.stat().st_mtime, reverse=True)
    for old in kept[KEEP:]:
        if old != folder:
            shutil.rmtree(old, ignore_errors=True)


def fetch(home: Path, release: str, name: str, opener: Opener = _download) -> Path:
    """This release's binaries, checked, in place; the oldest beyond KEEP removed."""
    target = binary(home, release)
    if target.exists():
        return target
    base = f"https://github.com/{SLUG}/releases/download/bridge-{release}"
    try:
        raw_sums = opener(f"{base}/{SUMS}")
        archive = opener(f"{base}/{name}")
    except Exception as exc:   # noqa: BLE001 - 404 while the workflow still builds, or no network
        raise RustBridgeError(f"the bridge of this version isn't out yet ({type(exc).__name__}); "
                              "try again in a few minutes") from exc
    signature = None
    if release_key.pinned():   # the sums are trusted only once signed by BipAgents' release key
        try:
            signature = opener(f"{base}/{release_key.SIGNATURE}")
        except Exception:   # noqa: BLE001 - no signature published: refused just below
            signature = None
    try:
        release_key.check(raw_sums, signature)
    except release_key.ReleaseSignatureError as exc:
        raise RustBridgeError(f"bridge {release}: {exc}") from exc
    sums = raw_sums.decode()
    expected = next((line.split()[0] for line in sums.splitlines() if line.strip().endswith(name)), None)
    if expected is None or hashlib.sha256(archive).hexdigest() != expected:
        raise RustBridgeError(f"{name} doesn't match the release's {SUMS}: not installed")
    folder = target.parent
    folder.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f".{release}-", dir=folder.parent))
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as tar:
            for member in tar.getmembers():
                if member.name not in PROGRAMS or not member.isfile():
                    continue   # only the two programs, nothing else written anywhere
                data = tar.extractfile(member)
                (staging / member.name).write_bytes(data.read() if data else b"")
                (staging / member.name).chmod(0o755)
        if not (staging / "bipbridge").exists():
            raise RustBridgeError(f"{name} holds no bipbridge program")
        os.replace(staging, folder)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    _prune(folder)
    return target


def released(release: str, opener: Opener = _download) -> bool:
    """Whether the release of this key is out (its SHA256SUMS answers)."""
    try:
        opener(f"https://github.com/{SLUG}/releases/download/bridge-{release}/{SUMS}")
    except Exception:   # noqa: BLE001 - 404 while the workflow builds (or failed), or no network
        return False
    return True


# How far back a new install looks for a version whose bridge is out (commits of main, first parent).
LOOK_BACK = 40
# At most this many different bridges asked about (commits that leave bridge-rs/ alone share their key).
ASKED = 8


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=True)


def newest_released(repo: Path = REPO, opener: Opener = _download, look_back: int = LOOK_BACK) -> Optional[str]:
    """The newest commit of this checkout's line (HEAD first, then the ones before it) whose bridge release is out:
    HEAD itself most days; the commit before when the newest version's bridge is still being built, or its build
    failed (real-day bench 2026-10-28: the public main's release had failed in CI, every new install of the offer
    stopped at step 3/5). A light clone (`--depth 1`) is deepened first. None: none of the last `look_back`."""
    if _git(repo, "rev-parse", "--is-shallow-repository").stdout.strip() == "true":
        _git(repo, "fetch", "--quiet", f"--deepen={look_back}", "origin")
    commits = _git(repo, "rev-list", "--first-parent", f"--max-count={look_back}", "HEAD").stdout.split()
    answers: dict = {}
    for commit in commits:
        release = key_at(repo, commit)
        if not release:
            continue
        if release not in answers:
            if len(answers) >= ASKED:
                return None
            answers[release] = released(release, opener)
        if answers[release]:
            return commit
    return None


# A server of the offer (install.sh --cloud) installed just after a public push waits this long for the bridge of
# its version before moving back: the workflow takes ~29 min after a push (2026-10-30). Moving back there would give
# the place another version than the server's shared Hermes, built by root from the same push
# (installer/bipinstall/shared_hermes.py): it would then install a Hermes of its own, past the guichet's install time.
WAIT_MINUTES = 35
WAIT_EVERY = 60


def _waited(release: str, minutes: float, opener: Opener, sleep: Callable[[float], None],
            clock: Callable[[], float]) -> bool:
    """Whether the release of this key comes out within `minutes`, asked every `WAIT_EVERY` s."""
    until = clock() + minutes * 60
    print(f"The bridge of this version ({release}) isn't out yet: GitHub Actions is most likely building it, waiting "
          f"up to {minutes:.0f} min.", flush=True)
    while clock() + WAIT_EVERY <= until:
        sleep(WAIT_EVERY)
        if released(release, opener):
            return True
    return False


def pick_released(repo: Path = REPO, opener: Opener = _download, wait: float = 0,
                  sleep: Callable[[float], None] = time.sleep, clock: Callable[[], float] = time.monotonic) -> str:
    """Before a new install (install.sh): this checkout moved back to the newest version whose bridge is out, when the
    one it has isn't (never for an update, a developer's clone, a changed checkout, a machine with no build of ours or
    with its own build). With `wait` (minutes, a server of the offer), the bridge of this version is waited for that
    long first. Says what it did ("" when nothing). Never fails the install: at worst nothing changes."""
    from .checkout import is_install_clone
    if os.environ.get(BUILD) or not asset() or not is_install_clone(repo):
        return ""
    release = key(repo)
    if not release or released(release, opener):
        return ""
    if _git(repo, "diff", "--quiet", "HEAD", "--").returncode != 0:
        return ""
    if wait and _waited(release, wait, opener, sleep, clock):
        return f"The bridge of this version ({release}) is out."
    head = _git(repo, "rev-parse", "HEAD").stdout.strip()
    found = newest_released(repo, opener)
    if not found or found == head:
        return (f"The bridge of this version ({release}) isn't out yet, nor one of the versions just before: "
                "the install tries it anyway.")
    if _git(repo, "checkout", "--quiet", "-B", "main", found).returncode != 0:
        return ""
    return (f"The bridge of the newest version isn't out yet: installing the one before ({found[:9]}, bridge "
            f"{key(repo)}). `bipagents update` brings the newest once it is out.")


def local_build(home: Path, release: str, folder: Path) -> Path:
    """Binaries built on this machine (BIPAGENTS_BRIDGE_BUILD), copied where the release's would go."""
    if not (folder / "bipbridge").is_file():
        raise RustBridgeError(f"{folder} holds no bipbridge program ({BUILD})")
    target = binary(home, release)
    target.parent.mkdir(parents=True, exist_ok=True)
    for name in PROGRAMS:
        if (folder / name).is_file():
            shutil.copy2(folder / name, target.parent / name)
            (target.parent / name).chmod(0o755)
    _prune(target.parent)
    return target


def unsupported(system: Optional[str] = None, machine: Optional[str] = None) -> str:
    """What a machine with no build of ours is told."""
    system, machine = system or platform.system(), machine or platform.machine()
    return (f"BipAgents' bridge has ready-made builds for Linux (x86_64, ARM64), Windows through WSL2 and macOS, "
            f"not for this machine ({system} {machine}). It can be built on it (Rust: `cargo build --release` in "
            f"{REPO / 'bridge-rs'}), then this run again with {BUILD}={REPO / 'bridge-rs' / 'target' / 'release'}.")


def ensure(home: Path, repo: Path = REPO, runner=None) -> Path:
    """This checkout's bridge binaries in place (downloaded, or BIPAGENTS_BRIDGE_BUILD's): their path. With a
    runner, the step is logged (and only logged on a dry run). Raises RustBridgeError, safe to show, when there are
    none for this machine or this version."""
    release, name, built = key(repo), asset(), os.environ.get(BUILD)
    if not release:
        raise RustBridgeError(f"{repo} isn't a git checkout of BipAgents with bridge-rs/: I can't tell which bridge "
                              "it needs")
    target = binary(home, release)
    if runner is not None:
        source = built or f"https://github.com/{SLUG}/releases/download/bridge-{release}/{name or '?'}"
        runner.log.append(f"fetch the bridge {release} from {source} into {target.parent}")
        if runner.dry:
            return target
    if built:
        return local_build(home, release, Path(built).expanduser())
    if not name:
        raise RustBridgeError(unsupported())
    return fetch(home, release, name)


def program(home: Path, repo: Path = REPO) -> List[str]:
    """What the bridge's service runs: the binary of this checkout's release (there once `ensure` has fetched it)."""
    return [str(binary(home, key(repo) or "unknown")), "serve"]


def cli(home: Path, repo: Path = REPO) -> List[str]:
    """The bridge's own command line (`browser add --all`, `qr`, `spaces start`…): this checkout's release, or while
    it isn't fetched yet (an update ran before the release was out), the newest one here (Chi's `bipagents qr`,
    2026-10-26)."""
    wanted = program(home, repo)[:-1]
    if Path(wanted[0]).exists():
        return wanted
    folder = home / BINARIES
    found = sorted((p for p in folder.glob("*/bipbridge") if p.is_file()), key=lambda p: p.stat().st_mtime, reverse=True)
    return [str(found[0])] if found else wanted


def python_service(home: Path) -> bool:
    """Whether the bridge's service still runs the Python bridge (an install made before 2026-10-21)."""
    if sys.platform == "darwin":
        files = sorted((home / "Library" / "LaunchAgents").glob("*.plist"))
        pattern = r"<string>-m</string>\s*<string>bipbridge</string>"
    else:
        files = sorted((home / ".config" / "systemd" / "user").glob("*.service"))
        pattern = r"^ExecStart=\S*python\S* -m bipbridge"
    return any(re.search(pattern, path.read_text(errors="ignore"), flags=re.M) for path in files)


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
        if not os.environ.get("XDG_RUNTIME_DIR"):   # from `sudo -u`: the account's own systemd is still reachable
            from .account import session_env
            os.environ.update(session_env(os.getuid()))
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=False)
    else:   # launchd reads the plist again only when loaded again
        for plist in sorted((home / "Library" / "LaunchAgents").glob("*.plist")):
            if "bipbridge" in plist.read_text(errors="ignore"):
                subprocess.run(["launchctl", "unload", str(plist)], check=False)
                subprocess.run(["launchctl", "load", str(plist)], check=False)
                return
    subprocess.run(bridge_restart(home), check=False)


def keep_in_step(home: Path, repo: Path = REPO, wait: float = 0.0, sleep=None) -> Optional[str]:
    """After a pull (`bipagents update`): this version's binaries, the service on them. Says what it did, or why it
    kept the previous ones (None: the service still runs the Python bridge, which the migration rust-bridge moves).
    `wait`: seconds to wait for a release still being built (a nightly update, unattended), asked again every 30 s.
    The service is only rewritten once the binaries are here: an update never leaves the bridge on a missing one."""
    import time
    if python_service(home):
        return None
    sleep = sleep or time.sleep
    deadline = time.monotonic() + wait
    while True:
        try:
            ensure(home, repo)
            break
        except RustBridgeError as exc:
            if "isn't out yet" not in str(exc) or time.monotonic() >= deadline:
                return f"{exc}: the previous bridge keeps running (the next update fetches this one)."
            sleep(30)
    rewrite_service(home, program(home, repo))
    return f"Bridge {key(repo)} in place."


def switch(home: Path, repo: Path = REPO) -> str:
    """The bridge's service on this checkout's binaries (fetched first), restarted, the agents' browser tools on its
    helper. Raises RustBridgeError, nothing changed, when there are no binaries for this machine or this version."""
    ensure(home, repo)
    if rewrite_service(home, program(home, repo)) is None:
        raise RustBridgeError("I can't find the bridge's service in this account.")
    (home / ENGINE).unlink(missing_ok=True)   # there is only one bridge now
    restart(home)
    browsers_follow(home)
    return f"The bridge {key(repo)} runs."


def main(argv: List[str]) -> int:
    from . import ui
    home = Path.home()
    choice = argv[0] if argv else ""
    if choice in COMMANDS:   # the bridge's own command line, on this account's config unless another is given
        from .family import find_config
        rest = list(argv)
        if choice != "genkey" and "--config" not in rest:
            rest += ["--config", str(find_config(None))]
        return subprocess.run(cli(home) + rest, check=False).returncode
    if choice == "released-version":   # install.sh, before a new install (pick_released)
        said = pick_released(wait=WAIT_MINUTES if "--wait" in argv[1:] else 0)
        if said:
            print(ui.note(said))
        return 0
    if choice == "python":
        print(ui.fail("The Python bridge was retired on 2026-10-21 (it stays in BipAgents' history): the bridge is "
                      "the Rust one now."))
        return 2
    if choice != "rust":
        release = key()
        here = bool(release) and binary(home, release).exists()
        print(f"Bridge: {release or 'unknown version'} ({'downloaded' if here else 'not downloaded'})"
              + (" — its service still runs the Python bridge: `bipagents update` moves it" if python_service(home) else ""))
        print("Fetch it again and restart it: bipagents bridge rust")
        return 0 if not choice else 2
    try:
        print(ui.done(switch(home)))
    except RustBridgeError as exc:
        print(ui.fail(str(exc)))
        return 1
    return 0


def browsers_follow(home: Path) -> None:
    """Each agent's browser tools (its `stealth` MCP entry) run the helper next to the bridge's binary,
    bip-browser-mcp (the Python bridge's browser_mcp.py before). Written again by the bridge's own `browser add --all`."""
    from .family import find_config
    if not list((home / ".config" / "systemd" / "user").glob("bipagents-browser-*.service")) and sys.platform != "darwin":
        return   # no agent browser here
    try:
        config = find_config(None)
    except SystemExit:
        return
    subprocess.run(cli(home) + ["browser", "add", "--all", "--config", str(config)], cwd=REPO / "bridge", check=False)
