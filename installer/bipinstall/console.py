"""`bipinstall console`: a one-time link opening the web console (the bridge's `/console/`) in a computer's browser.

In an account with an install, its bridge makes the link. From another account (the machine's administrator, whose
installs live in the family's accounts), one sudo password, then the install to open is asked for (or `--account`).
"""
from __future__ import annotations

import argparse
import shlex
import subprocess
from pathlib import Path
from typing import List, Optional

from . import ui
from .upgrade import REPO, as_account, installed_accounts

say = print


def bridge_command(repo: Path, config: str) -> List[str]:
    return [str(repo / "bridge" / ".venv" / "bin" / "python"), "-m", "bipbridge", "console", "--config", config]


def own_config(given: Optional[str]) -> Optional[Path]:
    from .family import find_config
    try:
        return find_config(given)
    except SystemExit:
        return None


def choose(accounts: List[str]) -> Optional[str]:
    if len(accounts) == 1:
        return accounts[0]
    for number, account in enumerate(accounts, 1):
        say(f"  {number}. {account}")
    answer = input(ui.question("Which install? (number or name) ")).strip()
    if answer.isdigit() and 1 <= int(answer) <= len(accounts):
        return accounts[int(answer) - 1]
    return answer if answer in accounts else None


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser(prog="bipinstall console", description=__doc__)
    parser.add_argument("--config", help="the bridge's config (default: this account's)")
    parser.add_argument("--account", help="from another account: the install's account (asks with sudo)")
    args = parser.parse_args(argv)

    config = None if args.account else own_config(args.config)
    if config is not None and (REPO / "bridge" / ".venv" / "bin" / "python").exists():
        return subprocess.run(bridge_command(REPO, str(config)), cwd=REPO / "bridge", check=False).returncode

    say(ui.note("No BipAgents install in this account: looking for the machine's installs (sudo)."))
    if subprocess.run(["sudo", "-v"]).returncode != 0:
        say(ui.fail("sudo refused. Run `bipagents console` in the account of the install instead."))
        return 1
    accounts = installed_accounts()
    account = args.account if args.account in accounts else (choose(accounts) if accounts else None)
    if account is None:
        say(ui.fail("No BipAgents install found on this machine." if not accounts else "Unknown install."))
        return 1
    home = Path("/home") / account
    command = " ".join(shlex.quote(part) for part in bridge_command(home / "BipAgents", str(home / ".config" / "bipagents" / "bridge.toml")))
    return subprocess.run(["bash", "-c", as_account(account, f"cd BipAgents/bridge && {command}")], check=False).returncode
