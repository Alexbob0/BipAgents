"""`bipinstall update` (`install.sh update`): BipAgents brought up to date in this account, and only what changed
restarted.

1. The repository: what is new since the last update (install.sh pulled it; run directly, this pulls). Local changes
   that block it are shown with the commands to put them aside.
2. The bridge (its dependencies when they changed), the Bips' voices (Pocket) when this account runs them, the agents'
   browsers when their image changed (rebuilt, the containers started again: their profiles stay).
3. The settings a fresh install now makes (migrations.py).
4. The machine's other installs (the family's), each in its own account, after one sudo password (offered, or
   --all). What no account here can do comes as commands: Hermes' own update, Pocket run from another account.
   New Hermes fixes (hermes/patches/) are put on a Hermes that came with BipAgents (`hermes-update --fixes-only`).
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

from . import hermes, migrations, rustbridge, ui
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


def pocket_restart() -> Optional[List[str]]:
    """The command restarting the Bips' voices when this account runs them (the installer's service, or one set up by
    hand that runs pocket_server.py), else None."""
    if platform.system() == "Darwin":
        plist = Path.home() / "Library" / "LaunchAgents" / "io.github.bipagents.pocket.plist"
        return ["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/io.github.bipagents.pocket"] if plist.exists() else None
    for unit in sorted((Path.home() / ".config" / "systemd" / "user").glob("*.service")):
        if "pocket_server.py" in unit.read_text(errors="ignore"):
            return ["systemctl", "--user", "restart", unit.stem]
    return None


def pocket_light() -> bool:
    """Whether this account's voices are the light ones (pocket_server.py --onnx, without PyTorch)."""
    places = [Path.home() / "Library" / "LaunchAgents" / "io.github.bipagents.pocket.plist",
              *sorted((Path.home() / ".config" / "systemd" / "user").glob("*.service"))]
    return any("pocket_server.py" in text and "--onnx" in text
               for text in (p.read_text(errors="ignore") for p in places if p.is_file()))


def pocket_step(runner: hermes.Runner, touched, venv) -> bool:
    """The Bips' voices this account runs, brought up to date: whether Pocket must restart. Its engine and its voices
    move together. Pocket's own models: the pocket-tts the repository pins (pocket/requirements.txt) put in its
    environment when it changed or an earlier update didn't finish it, with the voice states of the same commit
    (voices/). The light voices (--onnx: the phones' packages) keep their own requirements, PyTorch never comes back;
    their packages (engine, model and Bips together) come again from the guichet or the latest release when there are
    new ones. A failed install leaves Pocket running as it was (its voices loaded) and is tried again at the next
    update.

    A Pocket service set up by hand (its own pocket_server.py or Python) becomes the installer's bipagents-pocket
    (pocketunit.adopt; it starts there, no restart after). Pocket never restarts on voice states made for another
    Pocket: while its own can't move, it keeps its own version's states, taken from git (pocketunit.serve_matching)."""
    from . import pocketunit, voices
    restart = touched("pocket/", "voices/")
    unit = pocketunit.find(Path.home())
    if unit is not None and not unit.light and not unit.standard(REPO) and not runner.dry:
        if pocketunit.adopt(unit, REPO, Path.home(), runner, venv, say):
            return False
        unit = pocketunit.find(Path.home())   # the old one, put back
        restart = True   # restarted on the states of its own Pocket below (or not at all)
    light = pocket_light()
    requirements = "requirements-onnx.txt" if light else "requirements.txt"
    has_venv = (REPO / "pocket" / ".venv").exists()
    outdated = (not light and has_venv and not runner.dry
                and voices.installed_pocket(REPO) not in (None, voices.pinned_pocket(REPO)))
    if has_venv and (touched(f"pocket/{requirements}") or outdated):
        try:
            venv(REPO / "pocket", runner, requirements)
        except RuntimeError as exc:
            say(ui.fail(f"The Bips' voices couldn't be updated ({str(exc)[-200:]}): they stay as they are, "
                        "`bipagents update` tries again."))
            return False
        if not light:
            say(ui.done(f"Pocket {voices.pinned_pocket(REPO)} installed for the Bips' voices."))
        restart = True
    if light and voices.refresh(Path.home(), runner, say):
        say(ui.done("The Bips' new voices are here."))
        restart = True
    if unit is not None and not light and not runner.dry:
        engine = pocketunit.engine_of(unit)
        moved = pocketunit.serve_matching(unit, REPO, Path.home(), runner, say, engine)
        if moved is None:
            return False
        restart = restart or moved
        if moved:
            unit = pocketunit.find(Path.home()) or unit   # its unit as just written
        # A language added since its unit was written (Dutch, 2026-10-26), when it serves all the others.
        restart = pocketunit.serve_new_languages(unit, runner, say, engine) or restart
    return restart


def installed_accounts() -> List[str]:
    """The machine's other accounts with a BipAgents install (looked at with sudo: their homes are private)."""
    import pwd
    me = os.environ.get("USER", "")
    found = []
    for entry in pwd.getpwall():
        if entry.pw_uid < 1000 or entry.pw_name == me or not entry.pw_dir.startswith("/home/"):
            continue
        check = subprocess.run(["sudo", "-n", "test", "-f", f"{entry.pw_dir}/.config/bipagents/bridge.toml"])
        if check.returncode == 0:
            found.append(entry.pw_name)
    return found


def runs_bipagents(account: str) -> bool:
    """Whether the account's install runs: its bridge or its Hermes gateway. An install that was stopped and left
    there (aibox's old account after the move to one account per person, 2026-10-13) isn't updated: its Hermes would
    be patched for nothing, and its stopped services started again."""
    found = subprocess.run(["pgrep", "-u", account, "-f", "bipbridge|hermes.* gateway"], capture_output=True)
    return found.returncode == 0


def update_others(runner: hermes.Runner) -> None:
    """Each other install of the machine updated in its own account (`update --auto`), after one sudo password."""
    if subprocess.run(["sudo", "-v"]).returncode != 0:
        say(ui.fail("sudo refused: the other installs stay as they are."))
        return
    accounts = installed_accounts()
    stopped = [account for account in accounts if not runs_bipagents(account)]
    for account in stopped:
        say(ui.note(f"{account}: its BipAgents isn't running, left as it is."))
    accounts = [account for account in accounts if account not in stopped]
    if not accounts:
        say(ui.note("No other BipAgents install running on this machine."))
        return
    for account in accounts:
        say(ui.bip("happy", f"{account}…"))
        command = as_account(account, "bash BipAgents/install.sh update --auto").replace("sudo -u", "sudo -n -u", 1)
        runner.log.append(command)
        if not runner.dry:
            subprocess.run(["bash", "-c", command])   # its output as it goes


def pocket_owner() -> Optional[str]:
    """The account running the machine's Pocket server, if any."""
    out = subprocess.run(["ps", "-eo", "user=,args="], capture_output=True, text=True).stdout
    return next((line.split()[0] for line in out.splitlines() if "pocket_server.py" in line), None)


def as_account(account: str, command: str) -> str:
    return (f"sudo -u {account} bash -c 'cd /home/{account} && export HOME=/home/{account} "
            f"PATH=/home/{account}/.local/bin:$PATH XDG_RUNTIME_DIR=/run/user/$(id -u) "
            f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus && {command}'")


COMMAND_HELP = """BipAgents, in this account:
  bipagents update           bring it up to date (only what changed restarts)
  bipagents update --check   only say what is new
  bipagents qr [ACCOUNT]     show the code to scan in the app again (ACCOUNT: another install, with sudo)
  bipagents voice            connect your own voice model (a TTS server on the network)
  bipagents console          a one-time link opening your agents in a computer's browser
  bipagents hermes-update    update Hermes and keep BipAgents working
  bipagents bridge [CMD …]   which bridge version runs; `bridge rust` fetches it again; `bridge check`, `bridge
                             browser add --all`…: the bridge's own commands
  bipagents family add NAME  prepare a family member's install on this server
  bipagents export           everything your agents need in one encrypted file (a backup)
  bipagents export --moving  the same, then stop here: to move them to another machine
  bipagents resume           start again here after `export --moving`, if the move is called off
  bipagents send ADDR CODE   send your agents to your place of the all-included offer (the app shows both)
  bipagents backup           back this place up now (the all-included offer's places: every hour by themselves)
  bipagents browser-refresh  update the agents' browser now if Chrome has news (each night by itself);
                             `--version`: which Chrome it has
  bipagents encrypt          encrypt your space on this server's disk (it opens by itself after a reboot);
                             `encrypt status`, `rollback`: what it is, put it back
  bipagents uninstall        remove it"""


def install_command(runner: hermes.Runner, home: Optional[Path] = None) -> Path:
    """`bipagents` in ~/.local/bin: install.sh's commands (with no command, its help rather than a new install)."""
    path = (home or Path.home()) / ".local" / "bin" / "bipagents"
    script = (f"#!/bin/sh\n# BipAgents' commands ({REPO}/install.sh).\n"
              f"if [ $# -eq 0 ] || [ \"$1\" = help ] || [ \"$1\" = --help ]; then\n  cat <<'HELP'\n{COMMAND_HELP}\nHELP\n"
              f"  exit 0\nfi\nexec bash \"{REPO}/install.sh\" \"$@\"\n")
    runner.log.append(f"write {path}")
    if not runner.dry:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(script, encoding="utf-8")
        path.chmod(0o755)
    return path


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
    parser.add_argument("--all", action="store_true", help="the machine's other installs too, without asking (sudo)")
    parser.add_argument("--apply-to-my-hermes", action="store_true",
                        help="apply the new settings to a Hermes you had before BipAgents too")
    parser.add_argument("--dry-run", action="store_true")
    args = parser.parse_args(argv)
    if platform.system() == "Linux" and not os.environ.get("XDG_RUNTIME_DIR"):
        # From `sudo -iu <account>`: its own systemd found all the same (the bridge, Pocket and the browsers' units).
        from .account import session_env
        os.environ.update(session_env(os.getuid()))
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
        config: Optional[Path] = find_config(None)
    except SystemExit:
        config = None   # an account without its own install (an administrator's): its Pocket, the others

    # 2. What runs here (the bridge also when an earlier update ran before its release was out).
    bridge = bool(config) and (touched("bridge-rs/") or rustbridge.missed(Path.home()))
    if bridge and rustbridge.python_service(Path.home()):
        pass   # the Python bridge left the code: the migration rust-bridge (step 3) moves its service to Rust
    elif bridge:
        # its binaries for this version; unattended (the nightly timer, the guichet's waves), a release still being
        # built is waited for a few minutes; else, or past that, the current bridge keeps running
        rust = rustbridge.keep_in_step(Path.home(), wait=300 if args.auto else 0) if not runner.dry else None
        if rust:
            say(ui.note(rust))
            rustbridge.restart(Path.home())   # the service may name new binaries: read again, then restarted
        else:
            runner.run(bridge_restart(), check=False)
        say(ui.done("Bridge restarted."))
    if touched("installer/requirements.txt") and (REPO / "installer" / ".venv").exists():
        venv(REPO / "installer", runner)   # what the timers and the bridge run the installer with
    pocket = pocket_restart()
    if pocket and pocket_step(runner, touched, venv):
        runner.run(pocket, check=False)
        say(ui.done("The Bips' voices restarted."))
    elif touched("pocket/", "voices/") and not pocket:
        owner = pocket_owner()
        if owner and owner != os.environ.get("USER"):
            commands.append(f"# The Bips' voices run from the account {owner}: there, `bipagents update` restarts them")
    browsers = sorted((Path.home() / ".config" / "systemd" / "user").glob("bipagents-browser-*.service"))
    image_changed = any(path.startswith("browser/") and not path.endswith(".md") for path in changed)
    if config and image_changed and browsers and rustbridge.missed(Path.home(), REPO):
        commands.append("bipagents update   # in a few minutes, once this version's bridge is out: rebuilds the browsers")
    elif config and image_changed and browsers:
        if args.auto or ask("The agents' browser changed: rebuild it and restart the browsers now (a few minutes; "
                            "their profiles stay)?"):
            engine = hermes.sandbox_runtime() or "podman"
            hermes.build_browser(runner, engine, REPO)
            runner.run(rustbridge.cli(Path.home()) + ["browser", "add", "--all", "--config", str(config)],
                       cwd=REPO / "bridge")
            say(ui.done("Browsers rebuilt and restarted."))
        else:
            commands.append("bipagents update   # later, when the agents are not browsing")

    # 3. Settings a fresh install now makes.
    if config:
        for line in migrations.run(runner, own_hermes=own_hermes() or args.apply_to_my_hermes):
            say(ui.done(line))
    else:
        say(ui.note("No BipAgents install of its own in this account."))

    # 4. The machine's other installs, with one sudo; what no account here can do.
    if not args.auto and platform.system() == "Linux" and (args.all or ask(
            "Update the other BipAgents installs of this machine too (your sudo password, asked once)?")):
        update_others(runner)
    from . import shared_hermes
    shared = config and own_hermes() and shared_hermes.root() is not None
    if shared:
        # One Hermes for the server's places (shared_hermes.py): this place moves to the version its BipAgents pins
        # once root built it; its patches came with that build.
        moved = shared_hermes.follow(runner)
        if moved:
            say(ui.done(moved))
    elif touched("hermes/patches/") and config:
        # New Hermes fixes: on a Hermes that came with BipAgents, put on now (Hermes itself isn't updated); on the
        # person's own Hermes, theirs to choose.
        if platform.system() == "Linux" and not runs_bipagents(os.environ.get("USER", "")):
            say(ui.note("BipAgents isn't running in this account: its Hermes is left as it is."))
        elif own_hermes() and Path(hermes.hermes_bin()).exists() and not runner.dry:
            from . import update as hermes_update
            hermes_update.main(["--fixes-only", "--yes", "--config", str(config)])
        else:
            commands.append("bipagents hermes-update --fixes-only   # BipAgents' Hermes patches changed")
    if touched("App/", "Packages/", "Widgets/", "NotificationService/"):
        say(ui.note("The app changed too: its new version comes through TestFlight or the App Store."))
    if commands:
        say("\nTo run yourself:")
        for line in commands:
            say(f"  {line}")

    install_command(runner)
    if migrations.is_place(Path.home()):
        from .__main__ import tidy
        tidy(runner)   # what this update downloaded doesn't stay (a place's account is its own)

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
