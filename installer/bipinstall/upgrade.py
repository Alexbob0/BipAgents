"""`bipinstall update` (`install.sh update`): BipAgents brought up to date in this account, and only what changed
restarted.

1. The repository: what is new since the last update (install.sh pulled it; run directly, this pulls). Local changes
   that block it are shown with the commands to put them aside.
2. The bridge (its dependencies when they changed), the Bips' voices (Pocket) when this account runs them, the agents'
   browsers when their image changed (rebuilt, the containers started again: their profiles stay).
3. The settings a fresh install now makes (migrations.py).
4. What this account cannot do itself, as commands to run: another account's Pocket, the family's other installs,
   Hermes' own update.
5. Once, the offer to do all this each night (a timer of this account, `--auto`: no questions).

`--check` only says what is new.
"""
from __future__ import annotations

import json
import os
import platform
import subprocess
from pathlib import Path
from typing import List, Optional

from . import hermes, migrations, ui
from .existing import RECORD

REPO = Path(__file__).resolve().parents[2]
TIMER = "bipagents-update"


def say(text: str = "") -> None:
    print(text, flush=True)


def git(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(REPO), *args], capture_output=True, text=True)


def head() -> str:
    return git("rev-parse", "HEAD").stdout.strip()


def blocked_pull() -> List[str]:
    """Why `git pull --ff-only` cannot go on, and the commands that put the local changes aside."""
    status = git("status", "--porcelain").stdout.splitlines()
    changed = [line[3:] for line in status if not line.startswith("??")]
    untracked = [line[3:] for line in status if line.startswith("??")]
    lines = []
    if changed:
        lines += ["Files changed by hand in ~/BipAgents (kept aside, then the update):",
                  *[f"  {path}" for path in changed[:10]], f"  cd {REPO} && git stash"]
    if untracked:
        shown = untracked[:10]
        lines += ["Files not part of BipAgents in ~/BipAgents (the update may bring files of the same name): move "
                  "them aside, then update again:",
                  *[f"  {path}" for path in shown],
                  f"  cd {REPO} && mkdir -p ~/bipagents-aside && mv {' '.join(shown)} ~/bipagents-aside/"]
    if not lines:
        lines = [git("pull", "--ff-only").stderr.strip() or "git pull failed"]
    return lines


def own_hermes() -> bool:
    """Hermes came with BipAgents (False: the person's own Hermes, whose settings stay theirs)."""
    record = Path.home() / ".config" / "bipagents" / RECORD
    try:
        return not json.loads(record.read_text(encoding="utf-8")).get("preexisting")
    except (OSError, ValueError):
        return True


def systemd_unit_exists(name: str) -> bool:
    return (Path.home() / ".config" / "systemd" / "user" / f"{name}.service").exists()


def pocket_owner() -> Optional[str]:
    """The account running the machine's Pocket server, if any."""
    out = subprocess.run(["ps", "-eo", "user=,args="], capture_output=True, text=True).stdout
    return next((line.split()[0] for line in out.splitlines() if "pocket_server.py" in line), None)


def family_members(config: Path) -> List[str]:
    try:
        blocks = json.loads((config.parent / "family-ports.json").read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []
    me = os.environ.get("USER", "")
    return sorted(member for member in blocks if member != me)


def as_account(account: str, command: str) -> str:
    return (f"sudo -u {account} bash -c 'cd /home/{account} && export HOME=/home/{account} "
            f"PATH=/home/{account}/.local/bin:$PATH XDG_RUNTIME_DIR=/run/user/$(id -u) "
            f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus && {command}'")


def timer_installed() -> bool:
    if platform.system() == "Darwin":
        return (Path.home() / "Library" / "LaunchAgents" / f"io.github.bipagents.update.plist").exists()
    return (Path.home() / ".config" / "systemd" / "user" / f"{TIMER}.timer").exists()


def install_timer(runner: hermes.Runner) -> None:
    """Each night at 4:30, `install.sh update --auto` in this account (its output in the journal / a log)."""
    command = f"{REPO / 'install.sh'} update --auto"
    if platform.system() == "Darwin":
        from .stack import launchd_plist
        path = Path.home() / "Library" / "LaunchAgents" / "io.github.bipagents.update.plist"
        plist = launchd_plist("io.github.bipagents.update", ["/bin/bash", "-lc", command], REPO, {},
                              Path.home() / "Library" / "Logs" / "bipagents-update.log")
        plist = plist.replace("<key>KeepAlive</key><true/>", "").replace(
            "<key>RunAtLoad</key><true/>",
            "<key>StartCalendarInterval</key><dict><key>Hour</key><integer>4</integer>"
            "<key>Minute</key><integer>30</integer></dict>")
        runner.log.append(f"write {path}")
        if not runner.dry:
            path.write_text(plist, encoding="utf-8")
        runner.run(["launchctl", "bootstrap", f"gui/{os.getuid()}", str(path)], check=False)
        return
    folder = Path.home() / ".config" / "systemd" / "user"
    service = ("[Unit]\nDescription=BipAgents update (install.sh update --auto)\n\n[Service]\nType=oneshot\n"
               f"ExecStart=/bin/bash -lc '{command}'\n")
    timer = ("[Unit]\nDescription=BipAgents update, each night\n\n[Timer]\nOnCalendar=*-*-* 04:30:00\n"
             "RandomizedDelaySec=15min\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n")
    for name, text in ((f"{TIMER}.service", service), (f"{TIMER}.timer", timer)):
        runner.log.append(f"write {folder / name}")
        if not runner.dry:
            folder.mkdir(parents=True, exist_ok=True)
            (folder / name).write_text(text, encoding="utf-8")
    runner.run(["systemctl", "--user", "daemon-reload"])
    runner.run(["systemctl", "--user", "enable", "--now", f"{TIMER}.timer"])


def main(argv: List[str]) -> int:
    import argparse
    from .__main__ import venv
    from .family import bridge_restart, confirm, find_config
    parser = argparse.ArgumentParser(prog="bipinstall update", description=__doc__.split("\n\n")[0])
    parser.add_argument("--check", action="store_true", help="only say what is new")
    parser.add_argument("--auto", action="store_true", help="no questions (the nightly timer)")
    parser.add_argument("--apply-to-my-hermes", action="store_true",
                        help="apply the new settings to a Hermes you had before BipAgents too")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    runner = hermes.Runner(dry=args.dry_run)
    ask = (lambda question: False) if args.auto else confirm
    commands: List[str] = []

    # 1. The repository.
    if os.environ.get("BIPAGENTS_PULL_FAILED"):
        say(ui.fail("The update could not be downloaded:"))
        for line in blocked_pull():
            say(f"  {line}")
        return 1
    before = os.environ.get("BIPAGENTS_BEFORE") or head()
    if not os.environ.get("BIPAGENTS_BEFORE"):   # run directly, not through install.sh
        git("fetch", "--quiet")
        if args.check:
            behind = git("log", "--oneline", "HEAD..@{u}").stdout.splitlines()
            say(f"{len(behind)} change(s) to bring:" if behind else "Up to date.")
            for line in behind[:15]:
                say(f"  {line}")
            return 0
        if git("pull", "--ff-only", "--quiet").returncode != 0:
            say(ui.fail("The update could not be downloaded:"))
            for line in blocked_pull():
                say(f"  {line}")
            return 1
    after = head()
    changed = git("diff", "--name-only", before, after).stdout.split() if before != after else []
    news = git("log", "--format=%h %s", f"{before}..{after}").stdout.splitlines() if before != after else []
    say(ui.bip("happy", f"BipAgents: {len(news)} change(s)." if news else "BipAgents is up to date."))
    for line in news[:15]:
        say(f"  {line[:100]}")
    if args.check:
        return 0
    touched = lambda *prefixes: any(path.startswith(prefixes) for path in changed)

    try:
        config = find_config(None)
    except SystemExit:
        say(ui.note("No BipAgents bridge in this account: nothing else to update here."))
        return 0

    # 2. What runs here.
    if touched("bridge/"):
        if touched("bridge/requirements.txt"):
            venv(REPO / "bridge", runner)
        runner.run(bridge_restart(), check=False)
        say(ui.done("Bridge restarted."))
    if touched("pocket/", "voices/"):
        if systemd_unit_exists("bipagents-pocket") or platform.system() == "Darwin":
            if touched("pocket/requirements.txt"):
                venv(REPO / "pocket", runner)
            restart = (["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/io.github.bipagents.pocket"]
                       if platform.system() == "Darwin" else ["systemctl", "--user", "restart", "bipagents-pocket"])
            runner.run(restart, check=False)
            say(ui.done("The Bips' voices restarted."))
        else:
            owner = pocket_owner()
            if owner and owner != os.environ.get("USER"):
                commands.append(f"# The Bips' voices run from the account {owner}: update and restart them there")
                commands.append(f"cd ~/BipAgents && git pull --ff-only && systemctl --user list-units | grep -i pocket"
                                f"   # as {owner}, then: systemctl --user restart <that unit>")
    browsers = sorted((Path.home() / ".config" / "systemd" / "user").glob("bipagents-browser-*.service"))
    if touched("browser/") and browsers:
        if args.auto or ask("The agents' browser changed: rebuild it and restart the browsers now (a few minutes; "
                            "their profiles stay)?"):
            engine = hermes.sandbox_runtime() or "podman"
            hermes.build_browser(runner, engine, REPO)
            runner.run([str(REPO / "bridge" / ".venv" / "bin" / "python"), "-m", "bipbridge", "browser", "add", "--all",
                        "--config", str(config)], cwd=REPO / "bridge")
            say(ui.done("Browsers rebuilt and restarted."))
        else:
            commands.append(f"{REPO / 'install.sh'} update   # later, when the agents are not browsing")

    # 3. Settings a fresh install now makes.
    for line in migrations.run(runner, own_hermes=own_hermes() or args.apply_to_my_hermes):
        say(ui.done(line))

    # 4. What this account cannot do itself.
    if touched("hermes/patches/"):
        commands.append(f"{REPO / 'install.sh'} hermes-update   # BipAgents' Hermes patches changed")
    members = family_members(config)
    if members and not args.auto:
        commands.append("# The family's other installs (from an account allowed to use sudo):")
        commands += [as_account(member, "bash BipAgents/install.sh update --auto") for member in members]
    if touched("App/", "Packages/", "Widgets/", "NotificationService/"):
        say(ui.note("The app changed too: its new version comes through TestFlight or the App Store."))
    if commands:
        say("\nTo run yourself:")
        for line in commands:
            say(f"  {line}")

    # 5. Each night.
    if not args.auto and not timer_installed() and ask("Update BipAgents by itself each night (at 4:30)?"):
        install_timer(runner)
        say(ui.done("Updated each night. `systemctl --user list-timers` shows it; `systemctl --user disable --now "
                    f"{TIMER}.timer` stops it."))
    if runner.dry:
        say("\nCommands it would run:")
        for line in runner.log:
            say(f"  $ {line}")
    return 0
