"""`bipinstall browser-refresh`: the agents' browser kept up to date with security fixes, each night, by itself.

The image (browser/Containerfile) installs Chrome (Chromium on ARM) and Fedora's packages once, when it is built; a
container engine reuses that layer at each rebuild, so Chrome would stay as it was on the first day. This job, a timer
of each install that has browsers (a self-hosted one, each place of the offer), runs apart from BipAgents' own updates
(the offer's places only get those in waves):

1. it waits for the server's other installs (one build at a time per server: a lock in /tmp), at the lowest priority;
2. it asks dnf, in a throwaway container of the current image, whether Chrome or Chromium has a newer version, or any
   of the image's packages a security update. Nothing: « up to date », and that is all;
3. else it builds the image again under a temporary tag, with a new BROWSER_REFRESH (the Containerfile's packages
   layer is installed again, the pinned stealth-browser-mcp layers stay as pinned) and the newest base image; checks
   that Chrome starts (`--version`) and the MCP server's Python is there; and only then names it `bipagents-browser`.
   A failure (network, Google's repository down) keeps the current image and says so; the next night tries again;
4. running browsers are never interrupted: each one starts on the new image next time (its unit runs `podman run
   --rm … bipagents-browser`). One still running on an old image and unused for 30 minutes (an `idle_minutes = 0`
   browser never stops by itself) is restarted now; its profile, and so its logins, stay;
5. images no longer used are removed, so the disk does not grow.

What it did goes to the journal (`journalctl --user -u bipagents-browser-refresh`).
"""
from __future__ import annotations

import fcntl
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, List, Optional, Sequence, Tuple

IMAGE = "bipagents-browser"   # hermes.BROWSER_IMAGE (not imported: this runs from a timer, with little around it)
NEW = f"{IMAGE}:refresh"     # the image being built, named IMAGE only once it works
UNIT = "bipagents-browser-refresh"
LOCK = Path("/tmp/bipagents-browser-refresh.lock")   # shared by every account of the server
LOCK_WAIT = 3 * 3600        # another place's build takes minutes; past this, tomorrow
IDLE_RESTART = 30 * 60      # a browser on an old image, unused this long, is restarted on the new one
BROWSERS = ("google-chrome-stable", "chromium")
VERSION = re.compile(r"\b(\d+\.\d+\.\d+\.\d+)\b")

# Run as root in a throwaway container of the current image: which browser it has, then dnf's own answer (exit code
# 100: updates, 0: none, anything else: it could not tell).
CHECK = r"""pkg=
for p in google-chrome-stable chromium; do rpm -q "$p" >/dev/null 2>&1 && pkg=$p && break; done
echo "browser $pkg $(rpm -q --qf '%{VERSION}' "$pkg" 2>/dev/null)"
out=$(dnf -q check-upgrade "$pkg" 2>&1); code=$?
echo "browser-update $code $(printf '%s\n' "$out" | awk -v p="$pkg." 'index($1, p) == 1 {print $2; exit}')"
dnf -q check-upgrade --security >/dev/null 2>&1; echo "security $?"
"""

# Chrome (or Chromium) answers, and the MCP server's Python finds its packages.
SMOKE = ("google-chrome-stable --version 2>/dev/null || chromium-browser --version "
         "&& /opt/stealth/venv/bin/python -c 'import nodriver'")

Run = Callable[[Sequence[str], int], Tuple[int, str]]


def say(text: str) -> None:
    print(text, flush=True)


def run(args: Sequence[str], timeout: int = 600) -> Tuple[int, str]:
    """A command's exit code and its output (stdout then stderr); 124 when it took too long, 127 when missing."""
    try:
        done = subprocess.run(list(args), capture_output=True, text=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        return 124, f"{args[0]} gave no answer in {timeout} s"
    except OSError as exc:
        return 127, str(exc)
    return done.returncode, (done.stdout + done.stderr).strip()


# -- What the check says ---------------------------------------------------------------------------------------------

@dataclass
class Check:
    browser: str = ""          # google-chrome-stable or chromium
    version: str = ""          # installed
    newer: str = ""            # its newer version, when there is one
    browser_code: int = -1     # dnf check-upgrade's exit code: 0, 100, else an error
    security_code: int = -1


def parse_check(output: str) -> Check:
    check = Check()
    for line in output.splitlines():
        words = line.split()
        if not words:
            continue
        if words[0] == "browser" and len(words) >= 2:
            check.browser = words[1] if words[1] in BROWSERS else ""
            check.version = words[2] if len(words) > 2 and check.browser else ""
        elif words[0] == "browser-update" and len(words) >= 2 and words[1].lstrip("-").isdigit():
            check.browser_code = int(words[1])
            check.newer = words[2] if len(words) > 2 else ""
        elif words[0] == "security" and len(words) >= 2 and words[1].lstrip("-").isdigit():
            check.security_code = int(words[1])
    return check


def label(check: Check) -> str:
    name = "Chromium" if check.browser == "chromium" else "Chrome"
    return f"{name} {check.version}" if check.version else name


def decide(check: Check, force: bool = False) -> Tuple[Optional[bool], str]:
    """(True: rebuild, False: up to date, None: could not tell — the image stays), and why."""
    if force:
        return True, f"rebuild asked ({label(check)} now)"
    if not check.browser:
        return None, "no Chrome or Chromium found in the image"
    if check.browser_code not in (0, 100):
        return None, f"could not ask for {label(check)}'s updates (dnf: {check.browser_code}; network, or a repository down)"
    if check.browser_code == 100:
        return True, f"{label(check)}: {check.newer or 'a newer version'} is out"
    if check.security_code == 100:
        return True, f"{label(check)} is current; security updates for the image's packages"
    if check.security_code != 0:
        return False, f"{label(check)}: up to date (the security check gave no answer: {check.security_code})"
    return False, f"{label(check)}: up to date"


# -- Commands ---------------------------------------------------------------------------------------------------------

def is_podman(engine: str) -> bool:
    return Path(engine).name == "podman"


def build_command(engine: str, repo: Path, tag: str = IMAGE, stamp: Optional[str] = None) -> List[str]:
    """The image built with today's packages: a new BROWSER_REFRESH installs the packages layer again instead of
    taking it from the cache, and the base image (fedora) is pulled when the registry has a newer one."""
    stamp = stamp or time.strftime("%Y%m%d%H%M")
    pull = ["--pull=newer"] if is_podman(engine) else ["--pull"]
    return [engine, "build", *pull, "--build-arg", f"BROWSER_REFRESH={stamp}", "-t", tag,
            "-f", str(repo / "browser" / "Containerfile"), str(repo / "browser")]


def check_command(engine: str, image: str = IMAGE) -> List[str]:
    return [engine, "run", "--rm", "--pull=never", "--user", "0", "--entrypoint", "/bin/sh", image, "-c", CHECK]


def smoke_command(engine: str, image: str) -> List[str]:
    return [engine, "run", "--rm", "--pull=never", "--entrypoint", "/bin/sh", image, "-c", SMOKE]


def image_id(engine: str, image: str, run: Run) -> str:
    code, out = run([engine, "image", "inspect", "--format", "{{.Id}}", image], 60)
    return out.strip().removeprefix("sha256:") if code == 0 else ""


# -- The swap ---------------------------------------------------------------------------------------------------------

def browsers_root(home: Path) -> Path:
    """Where the bridge keeps each browser's folder (its `busy` file): bridge.toml's [browser] root, else its default."""
    for config in (home / ".config" / "bipagents" / "bridge.toml", home / ".config" / "hermes-ios" / "bridge.toml"):
        try:
            text = config.read_text(encoding="utf-8")
        except OSError:
            continue
        section = re.search(r"^\[browser\]\n((?:(?!\[).*\n?)*)", text, re.M)
        found = re.search(r"^root\s*=\s*['\"]([^'\"]+)['\"]", section.group(1), re.M) if section else None
        if found:
            return Path(os.path.expanduser(found.group(1)))
    return home / ".local" / "share" / "hermes-bridge" / "browsers"


def restart_old(engine: str, home: Path, run: Run, now: Optional[float] = None) -> List[str]:
    """This install's running browsers on an image older than the current one, restarted when unused for
    IDLE_RESTART (their profiles stay; a browser in use keeps going, and takes the new image at its next start).
    Returns the agents restarted."""
    now = time.time() if now is None else now
    current = image_id(engine, IMAGE, run)
    code, out = run([engine, "ps", "--filter", "name=bipagents-browser-", "--format", "{{.Names}}"], 60)
    if not current or code != 0:
        return []
    units = home / ".config" / "systemd" / "user"
    root = browsers_root(home)
    restarted = []
    for name in out.split():
        agent = name.removeprefix("bipagents-browser-")
        if name == agent or not (units / f"{name}.service").is_file():
            continue   # another account's (one docker for the whole machine), or not a browser of ours
        code, used = run([engine, "inspect", "--format", "{{.Image}}", name], 60)
        if code != 0 or used.strip().removeprefix("sha256:") == current:
            continue
        try:
            unused = now - (root / agent / "busy").stat().st_mtime
        except OSError:
            unused = float("inf")
        if unused < IDLE_RESTART:
            say(f"{agent}'s browser is in use: on the new image at its next start")
            continue
        code, said = run(["systemctl", "--user", "restart", f"{name}.service"], 180)
        if code == 0:
            restarted.append(agent)
            say(f"{agent}'s browser restarted on the new image (its profile and logins kept)")
        else:
            say(f"{agent}'s browser could not be restarted ({said[-200:]}): on the new image at its next start")
    return restarted


def prune(engine: str, run: Run) -> None:
    """Images no longer named nor used (yesterday's, a failed build's layers) removed."""
    code, out = run([engine, "image", "prune", "-f"], 600)
    if code != 0:
        say(f"old images not removed: {out[-200:]}")
    if not is_podman(engine):
        run([engine, "builder", "prune", "-f", "--filter", "until=168h"], 600)


# -- The lock ---------------------------------------------------------------------------------------------------------

def server_lock(path: Path = LOCK, wait: float = LOCK_WAIT, sleep: Callable[[float], None] = time.sleep) -> Optional[int]:
    """One refresh at a time on the server (its places are accounts of their own, each with its own images): a
    file every account can open, locked with flock. None after `wait` seconds (another one is stuck: tomorrow)."""
    try:
        fd = os.open(path, os.O_RDONLY)
    except FileNotFoundError:
        try:
            fd = os.open(path, os.O_RDONLY | os.O_CREAT | os.O_EXCL, 0o644)
        except FileExistsError:
            fd = os.open(path, os.O_RDONLY)
    deadline = time.monotonic() + wait
    said = False
    while True:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            return fd
        except BlockingIOError:
            if time.monotonic() >= deadline:
                os.close(fd)
                return None
            if not said:
                say("another install of this server is refreshing its browser: waiting for it")
                said = True
            sleep(30)


# -- The whole refresh ------------------------------------------------------------------------------------------------

def refresh(engine: str, repo: Path, home: Path, run: Run = run, force: bool = False,
            stamp: Optional[str] = None) -> int:
    """0: up to date or refreshed; 1: the image stays as it was (said why); the next night tries again."""
    before = image_id(engine, IMAGE, run)
    if not before:
        say("no agent browser image here: nothing to refresh")
        return 0
    code, out = run(check_command(engine), 900)
    check = parse_check(out)
    rebuild, why = decide(check, force)
    if rebuild is None:
        say(f"current image kept: {why}" + (f" — {out[-300:]}" if code not in (0,) and out else ""))
        return 1
    if not rebuild:
        say(why)
        restart_old(engine, home, run)
        prune(engine, run)
        return 0
    say(f"{why}: building the browser again (the current one keeps working meanwhile)")
    run([engine, "rmi", NEW], 120)   # a failed night's leftover
    code, out = run(build_command(engine, repo, NEW, stamp), 3600)
    if code == 0:
        code, version = run(smoke_command(engine, NEW), 300)
        found = VERSION.search(version)
        if code != 0 or not found:
            say(f"current image kept: the new one does not start ({version[-300:]})")
            code = 1
    else:
        say(f"current image kept: the build failed ({out[-400:]})")
    if code != 0:
        run([engine, "rmi", NEW], 120)
        prune(engine, run)
        return 1
    code, out = run([engine, "tag", NEW, IMAGE], 120)
    if code != 0:
        say(f"current image kept: {out[-200:]}")
        run([engine, "rmi", NEW], 120)
        return 1
    run([engine, "rmi", NEW], 120)   # only the name: the image is IMAGE now
    old = check.version or "?"
    say(f"browser refreshed: {label(check).split()[0]} {old} → {found.group(1)}"
        if old != found.group(1) else f"browser refreshed: {label(check)}, system packages updated")
    restart_old(engine, home, run)
    prune(engine, run)
    return 0


def units(repo: Path, python: Path) -> List[Tuple[str, str]]:
    """The systemd user service and its nightly timer: between 1:00 and 3:00 (spread: the places of a server don't
    all check at once, and before the 4:30 update), at the lowest priority."""
    service = ("[Unit]\nDescription=BipAgents: the agents' browser kept up to date (Chrome, security fixes)\n"
               "After=network-online.target\n\n"
               "[Service]\nType=oneshot\nNice=19\nIOSchedulingClass=idle\n"
               f"Environment=PYTHONPATH={repo / 'installer'}\nWorkingDirectory={repo}\n"
               f"ExecStart={python} -m bipinstall browser-refresh\n")
    timer = ("[Unit]\nDescription=BipAgents: refresh the agents' browser each night\n\n[Timer]\n"
             "OnCalendar=*-*-* 01:00:00\nRandomizedDelaySec=2h\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n")
    return [(f"{UNIT}.service", service), (f"{UNIT}.timer", timer)]


def install_timer(runner, repo: Path, python: Path, home: Optional[Path] = None) -> None:
    """Linux only (the browsers are containers; a Mac has none)."""
    folder = (home or Path.home()) / ".config" / "systemd" / "user"
    for name, text in units(repo, python):
        runner.log.append(f"write {folder / name}")
        if not runner.dry:
            folder.mkdir(parents=True, exist_ok=True)
            (folder / name).write_text(text, encoding="utf-8")
    runner.run(["systemctl", "--user", "daemon-reload"], check=False)
    runner.run(["systemctl", "--user", "enable", "--now", f"{UNIT}.timer"], check=False)


def version(engine: str, run: Run = run) -> str:
    """The browser of the current image, e.g. « Chrome 155.0.8059.84 », or ''."""
    code, out = run([engine, "run", "--rm", "--pull=never", "--entrypoint", "/bin/sh", IMAGE, "-c",
                     "google-chrome-stable --version 2>/dev/null || chromium-browser --version"], 120)
    found = VERSION.search(out) if code == 0 else None
    return f"{'Chromium' if 'Chromium' in out else 'Chrome'} {found.group(1)}" if found else ""


def main(argv: List[str]) -> int:
    import argparse
    parser = argparse.ArgumentParser(prog="bipinstall browser-refresh", description=__doc__.split("\n\n")[0])
    parser.add_argument("--force", action="store_true", help="build again even when nothing is new")
    parser.add_argument("--version", action="store_true", help="only say which Chrome the browser has")
    args = parser.parse_args(argv)
    if platform.system() != "Linux":
        say("the agents' browsers only run on Linux: nothing to refresh")
        return 0
    engine = shutil.which("podman") or shutil.which("docker")
    if not engine:
        say("no podman or docker here: nothing to refresh")
        return 0
    if args.version:
        say(version(engine) or "no agent browser image here")
        return 0
    lock = server_lock()
    if lock is None:
        say("another refresh of this server has not finished for hours: tomorrow")
        return 1
    try:
        return refresh(engine, Path(__file__).resolve().parents[2], Path.home(), force=args.force)
    finally:
        os.close(lock)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
