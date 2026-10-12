"""A family member's own Linux account, ready for an install of their own: the account itself, the ID ranges rootless
podman needs (the agents' browser runs in containers), and « linger », so their services keep running when nobody
is logged in. Prepared by the administrator (`bipinstall famille ajouter`), each command shown and confirmed; the
install, run in that account, then checks it can drive its own systemd (`ensure_user_services`)."""
from __future__ import annotations

import os
import pwd
import shlex
import subprocess
import time
from pathlib import Path
from typing import Callable, Dict, List, Optional

SUBID_FIRST = 100_000
SUBID_COUNT = 65_536
LINGER_DIR = Path("/var/lib/systemd/linger")


def exists(login: str) -> bool:
    try:
        pwd.getpwnam(login)
        return True
    except KeyError:
        return False


def uid_of(login: str) -> int:
    return pwd.getpwnam(login).pw_uid


def subid_ranges(text: str) -> Dict[str, List[range]]:
    """`/etc/subuid` or `/etc/subgid`: « login:start:count » per line."""
    ranges: Dict[str, List[range]] = {}
    for line in text.splitlines():
        parts = line.strip().split(":")
        if len(parts) == 3 and parts[1].isdigit() and parts[2].isdigit():
            start, count = int(parts[1]), int(parts[2])
            ranges.setdefault(parts[0], []).append(range(start, start + count))
    return ranges


def next_subid_start(*texts: str) -> int:
    """The first ID after every range already given, in both files, so the new one overlaps none."""
    ends = [r.stop for text in texts for found in subid_ranges(text).values() for r in found]
    return max([SUBID_FIRST] + ends)


def linger_enabled(login: str, folder: Path = LINGER_DIR) -> bool:
    return (folder / login).exists()


def session_env(uid: int, run_dir: Path = Path("/run/user")) -> Dict[str, str]:
    """What `systemctl --user` needs to reach the account's own systemd, when the shell came from `sudo -iu`
    (which sets neither). Empty if that systemd is not running (no linger, no session)."""
    folder = run_dir / str(uid)
    if not (folder / "bus").exists():
        return {}
    return {"XDG_RUNTIME_DIR": str(folder), "DBUS_SESSION_BUS_ADDRESS": f"unix:path={folder / 'bus'}"}


def prepare_commands(login: str, *, account_exists: bool, subuid: str, subgid: str, linger: bool) -> List[List[str]]:
    """The root commands still missing for this account, in order."""
    if not account_exists:
        # First the account alone: useradd gives ID ranges on most systems, so the rest is decided after it.
        return [["sudo", "useradd", "--create-home", "--shell", "/bin/bash", login]]
    commands: List[List[str]] = []
    has_uids = login in subid_ranges(subuid)
    has_gids = login in subid_ranges(subgid)
    if not (has_uids and has_gids):
        start = next_subid_start(subuid, subgid)
        span = f"{start}-{start + SUBID_COUNT - 1}"
        extra = ([] if has_uids else ["--add-subuids", span]) + ([] if has_gids else ["--add-subgids", span])
        commands.append(["sudo", "usermod"] + extra + [login])
    if not linger:
        commands.append(["sudo", "loginctl", "enable-linger", login])
    return commands


def read(path: str) -> str:
    try:
        return Path(path).read_text(encoding="utf-8")
    except OSError:
        return ""


def prepare(login: str, confirm: Callable[[str], bool], say: Callable[[str], None], dry: bool = False) -> bool:
    """Brings the account to « ready »: each missing step shown, confirmed, run. False if one was refused or failed."""
    created = exists(login)
    for _ in range(2):   # after creating the account, look again: useradd may have given the ID ranges
        commands = prepare_commands(login, account_exists=created, subuid=read("/etc/subuid"),
                                    subgid=read("/etc/subgid"), linger=linger_enabled(login))
        if not commands:
            break
        for command in commands:
            say(f"  $ {shlex.join(command)}")
        if not confirm("Je lance ces commandes (ton mot de passe sudo peut être demandé) ?"):
            return False
        for command in commands:
            if dry:
                continue
            if subprocess.run(command).returncode != 0:
                say(f"Échec de : {shlex.join(command)}")
                return False
        if not created:
            created = True if dry else exists(login)
            continue
        break
    if not dry and linger_enabled(login):
        # Linger starts the account's systemd: give it a moment, the install needs it.
        for _ in range(20):
            if session_env(uid_of(login)):
                break
            time.sleep(0.25)
    return True


def install_command(login: str, uid: int, line: str) -> List[str]:
    """The install line run in the member's own account from the administrator's terminal, with what reaches that
    account's systemd: they have nothing to type."""
    folder = f"/run/user/{uid}"
    return ["sudo", "-iu", login, "env", f"XDG_RUNTIME_DIR={folder}", f"DBUS_SESSION_BUS_ADDRESS=unix:path={folder}/bus",
            "bash", "-c", line]


def ensure_user_services(say: Callable[[str], None], login: Optional[str] = None,
                         check: Optional[Callable[[], bool]] = None) -> bool:
    """At the start of an install on Linux: the account's own systemd must answer (its agents run as its services).
    Fills in what `sudo -iu` leaves out; otherwise says what to ask the administrator."""
    login = login or os.environ.get("USER") or pwd.getpwuid(os.getuid()).pw_name
    if not os.environ.get("XDG_RUNTIME_DIR"):
        os.environ.update(session_env(os.getuid()))
    reachable = check() if check else subprocess.run(["systemctl", "--user", "show-environment"],
                                                     capture_output=True).returncode == 0
    if not reachable:
        say("Ce compte ne peut pas lancer ses propres services (systemd utilisateur injoignable).")
        say(f"Demande à l'administrateur du serveur : sudo loginctl enable-linger {login}")
        say("(ou `bipinstall famille ajouter`, qui prépare le compte), puis relance.")
        return False
    if not linger_enabled(login):
        say(f"Attention : sans « linger », tes agents s'arrêteront quand tu te déconnecteras. "
            f"Demande à l'administrateur : sudo loginctl enable-linger {login}")
    return True
