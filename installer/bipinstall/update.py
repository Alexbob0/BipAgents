"""``bipinstall hermes-update [--check]``: update Hermes whenever the person wants, and keep BipAgents working.

1. Before: which of BipAgents' patches still apply on the new Hermes, checked on a temporary copy of it (nothing of
   the install changes). The person sees it and can stop there.
2. The update: our patches taken off, `hermes update` (its own backup, dependencies and config migration), then our
   patches put back, those that apply.
3. The gateways restarted: the hub and the agents' own spaces.
4. Checks: every agent of the bridge answers with its key, and has what the app relies on. When an agent no longer
   answers, going back to the previous version is offered.

`--check` does 1 and 4 only, without changing anything. `--fixes-only` puts on the BipAgents patches the current Hermes
doesn't have yet (a BipAgents update brought new ones), without updating Hermes, then does 3 and 4; the same is offered
when Hermes is already up to date.
"""
from __future__ import annotations

import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Dict, List, Optional, Tuple

import httpx

from . import hermes, ui
from .existing import RECORD
from .family import confirm, find_config

REPO = Path(__file__).resolve().parents[2]
PATCHES = REPO / "hermes" / "patches"


def say(text: str = "") -> None:
    print(text, flush=True)


def git(*args: str, check: bool = False) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(hermes.hermes_checkout()), *args], capture_output=True, text=True,
                          check=check)


def patches() -> List[Path]:
    return sorted(p for p in PATCHES.glob("*.patch") if not p.name.startswith("."))


def applied_now() -> List[Path]:
    """Our patches the Hermes code has right now (in whichever version fits it)."""
    return [v for p in patches() for v in hermes.patch_variants(p)
            if git("apply", "--reverse", "--check", str(v)).returncode == 0]


def channel(out: str) -> str:
    found = re.search(r"Update channel: (\S+)", out)
    return found.group(1) if found else "main"


def behind(out: str) -> Optional[int]:
    found = re.search(r"(\d+) commits? behind", out)
    return int(found.group(1)) if found else (0 if "up to date" in out.lower() else None)


def preflight(branch: str, wanted: List[Path]) -> Tuple[str, Dict[str, bool]]:
    """(the new version's commit, patch name → applies there), on a temporary copy of origin/<branch>."""
    git("fetch", "--quiet", "origin", branch)
    target = git("rev-parse", "--short=9", f"origin/{branch}").stdout.strip()
    folder = Path(tempfile.mkdtemp(prefix="bipagents-hermes-"))
    try:
        git("worktree", "add", "--detach", "--force", str(folder / "copy"), f"origin/{branch}", check=True)
        result = {p.name: any(subprocess.run(["git", "-C", str(folder / "copy"), "apply", "--check", str(v)],
                                             capture_output=True).returncode == 0 for v in hermes.patch_variants(p))
                  for p in wanted}
    finally:
        git("worktree", "remove", "--force", str(folder / "copy"))
        shutil.rmtree(folder, ignore_errors=True)
    return target, result


def gateway_units() -> List[Tuple[str, str]]:
    """(how, name) of every gateway serving this person's agents: the hub (Hermes' own service, or ours), then each
    agent's own space."""
    home = Path.home()
    found: List[Tuple[str, str]] = []
    if platform.system() == "Darwin":
        agents = home / "Library" / "LaunchAgents"
        ours = agents / "io.github.bipagents.hermes.plist"
        found.append(("launchd", "io.github.bipagents.hermes") if ours.exists() else ("hermes", "gateway"))
        found += [("launchd", p.stem) for p in sorted(agents.glob("io.github.bipagents.space.*.plist"))]
    else:
        units = home / ".config" / "systemd" / "user"
        found.append(("systemd", "bipagents-hermes") if (units / "bipagents-hermes.service").exists()
                     else ("hermes", "gateway"))
        found += [("systemd", p.stem) for p in sorted(units.glob("bipagents-space-*.service"))]
    return found


def restart(runner: hermes.Runner) -> None:
    for how, name in gateway_units():
        if how == "systemd":
            runner.run(["systemctl", "--user", "restart", name], check=False)
        elif how == "launchd":
            runner.run(["launchctl", "kickstart", "-k", f"gui/{os.getuid()}/{name}"], check=False)
        else:
            runner.run([hermes.hermes_bin(), "gateway", "restart"], check=False)


def agents_of(config_path: Path) -> List[Dict[str, str]]:
    try:
        import tomllib
    except ImportError:  # pragma: no cover
        import tomli as tomllib   # type: ignore[no-redef]
    data = tomllib.loads(config_path.read_text(encoding="utf-8"))
    return [{"id": name, "name": t.get("display_name", name), "url": t["hermes_url"], "key": t.get("hermes_key", "")}
            for name, t in (data.get("agents") or {}).items() if isinstance(t, dict) and t.get("hermes_url")]


def check_agents(agents: List[Dict[str, str]], expect_clarify: bool, client=None, attempts: int = 30) -> Dict[str, str]:
    """agent id → "" when it answers with what the app needs, else what is wrong."""
    import time
    client = client or httpx.Client(timeout=5)
    problems: Dict[str, str] = {}
    for agent in agents:
        problem = "isn't answering"
        for _ in range(attempts):
            try:
                resp = client.get(f"{agent['url'].rstrip('/')}/v1/capabilities",
                                  headers={"Authorization": f"Bearer {agent['key']}"})
            except httpx.HTTPError:
                time.sleep(2)
                continue
            if resp.status_code == 401:
                problem = "refuses its key"
                break
            if resp.status_code != 200:
                time.sleep(2)
                continue
            caps = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
            features = json.dumps(caps)
            if "run" not in features:
                problem = "no longer announces runs (the app can't talk to it anymore)"
            elif expect_clarify and "run_clarify" not in features:
                problem = "no longer has the agent's questions in the app (fix 0001)"
            else:
                problem = ""
            break
        problems[agent["id"]] = problem
    return problems


def report(agents: List[Dict[str, str]], problems: Dict[str, str]) -> bool:
    for agent in agents:
        problem = problems.get(agent["id"], "")
        say(ui.done(f"{agent['name']} answers.") if not problem
            else ui.fail(f"{agent['name']} {problem}."))
    return not any(problems.values())


def missing_fixes(wanted: List[Path], mine: List[Path]) -> List[Path]:
    """The patches wanted that the Hermes code doesn't have yet."""
    have = {p.name for p in mine}
    return [p for p in wanted if p.name not in have]


def put_on(runner: hermes.Runner, missing: List[Path]) -> Tuple[List[str], List[str]]:
    """Each missing patch in the version that fits the current Hermes: (the files applied, the names that don't fit)."""
    applied: List[str] = []
    skipped: List[str] = []
    for patch in missing:
        fit = next((v for v in hermes.patch_variants(patch) if git("apply", "--check", str(v)).returncode == 0), None)
        if fit is None:
            skipped.append(patch.name)
            continue
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", str(fit)])
        applied.append(str(fit))
    return applied, skipped


def fixes_only(args, agents: List[Dict[str, str]], mine: List[Path], missing: List[Path],
               chosen: Optional[List[str]]) -> int:
    """The missing fixes put on the current Hermes, the gateways restarted, the agents checked."""
    say(ui.note(f"BipAgents fixes this Hermes doesn't have yet: {', '.join(p.name[:4] for p in missing)}."))
    if not args.yes and not confirm("Put them on now, without updating Hermes? Your agents restart (1 to 2 minutes)."):
        say("Nothing was changed.")
        return 1
    runner = hermes.Runner()
    applied, skipped = put_on(runner, missing)
    if chosen is not None and applied:
        remember_patches(list(chosen) + applied)
    if skipped:
        say(ui.note(f"Fixes that don't fit this Hermes: {', '.join(skipped)}."))
    if not applied:
        return 1
    say(ui.done(f"Fixes put on: {', '.join(Path(p).name[:4] for p in applied)}."))
    restart(runner)
    every = [str(p) for p in mine] + applied
    if report(agents, check_agents(agents, expect_clarify=any(Path(p).name.startswith("0001") for p in every))):
        say(ui.finale("Your agents have BipAgents' latest fixes."))
        return 0
    say(ui.sad("Something is wrong after the fixes."))
    if args.yes or not confirm("Take them back off?"):
        return 1
    for patch in reversed(applied):
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", "--reverse", patch], check=False)
    if chosen is not None:
        remember_patches(list(chosen))
    restart(runner)
    return 1


def recorded_patches() -> Optional[List[str]]:
    """The patches the person chose on a Hermes they had before BipAgents (None: Hermes came with BipAgents)."""
    record = Path.home() / ".config" / "bipagents" / RECORD
    try:
        return json.loads(record.read_text(encoding="utf-8")).get("patched", [])
    except (OSError, ValueError):
        return None


def remember_patches(paths: List[str]) -> None:
    record = Path.home() / ".config" / "bipagents" / RECORD
    try:
        data = json.loads(record.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return
    data["patched"] = list(paths)   # the files applied, so an uninstall takes off exactly those
    record.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def main(argv: List[str]) -> int:
    import argparse
    parser = argparse.ArgumentParser(prog="bipinstall hermes-update",
                                     description="Update Hermes and keep BipAgents working (patches, restart, checks)")
    parser.add_argument("--check", action="store_true", help="only check: what an update would keep, and the agents")
    parser.add_argument("--config", help="the bridge's config (default: found by itself)")
    parser.add_argument("--yes", action="store_true", help="no question (for a scheduled update)")
    parser.add_argument("--fixes-only", action="store_true",
                        help="only put on the BipAgents fixes the current Hermes doesn't have yet (no Hermes update)")
    args = parser.parse_args(argv)
    if (hermes.hermes_home() / "shared").is_symlink():
        say(ui.note("This Hermes is shared by the server's places and read-only: its administrator updates it "
                    "(BipAgents' own update moves this place to each new version)."))
        return 0
    if not Path(hermes.hermes_bin()).exists():
        say(ui.sad("Hermes isn't installed in this account."))
        return 1
    config = find_config(args.config)
    agents = agents_of(config)
    say(ui.bip("busy", "Let's look at your Hermes version."))

    before = git("rev-parse", "--short=9", "HEAD").stdout.strip()
    mine = applied_now()
    chosen = recorded_patches()
    # A Hermes that came with BipAgents gets every patch; one the person had before, those they chose.
    chosen_names = {Path(c).name for c in chosen} if chosen is not None else set()
    wanted = [p for p in patches() if chosen is None or p.name in chosen_names or p.name in {m.name for m in mine}]
    missing = missing_fixes(wanted, mine)
    fixes = ", ".join(p.name[:4] for p in mine)
    say(ui.note(f"Current version: {before}; fixes in place: {fixes or 'none'}."))
    if args.fixes_only:
        if not missing:
            say(ui.done("Every BipAgents fix is already on this Hermes."))
            return 0
        return fixes_only(args, agents, mine, missing, chosen)
    out = subprocess.run([hermes.hermes_bin(), "update", "--check"], capture_output=True, text=True).stdout
    branch = channel(out)
    late = behind(out)
    if late == 0:
        say(ui.done("Hermes is already up to date."))
    elif late is not None:
        target, keeps = preflight(branch, wanted)
        say(ui.note(f"New version available ({target}, {late} changes on \"{branch}\")."))
        for name, ok in keeps.items():
            say(ui.done(f"{name} still applies.") if ok else
                ui.fail(f"{name} no longer applies: its feature will be missing until a BipAgents update."))
        if missing:
            say(ui.note(f"New BipAgents fixes: {', '.join(p.name[:4] for p in missing)}. To put them on without "
                        "updating Hermes: bipagents hermes-update --fixes-only"))
    if late == 0 and missing and not args.check:
        return fixes_only(args, agents, mine, missing, chosen)
    if args.check or not late:
        if missing:
            say(ui.note(f"BipAgents fixes not on this Hermes yet: {', '.join(p.name[:4] for p in missing)} "
                        "(bipagents hermes-update --fixes-only)."))
        say("\nYour agents, now:")
        report(agents, check_agents(agents, expect_clarify=any(p.name.startswith("0001") for p in mine)))
        return 0
    if not args.yes:
        if not confirm("Update Hermes now? Your agents restart (1 to 2 minutes)."):
            say("Nothing was changed.")
            return 1

    runner = hermes.Runner()
    say(ui.step(1, 3, "Updating Hermes"))
    for patch in reversed(mine):   # its own update then starts from a clean copy
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", "--reverse", str(patch)], check=False)
    try:
        runner.run([hermes.hermes_bin(), "update", "--yes", "--no-gateway-restart"])
    except RuntimeError as exc:
        say(ui.fail("The Hermes update failed: putting the fixes back on the current version."))
        say(ui.note(str(exc)[-400:]))
        for patch in mine:
            runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", str(patch)], check=False)
        restart(runner)
        return 1
    applied: List[str] = []
    skipped = hermes.apply_patches(runner, PATCHES, applied=applied) if chosen is None else []
    if chosen is not None:   # only those the person chose on their own Hermes
        for patch in wanted:
            fit = next((v for v in hermes.patch_variants(patch) if git("apply", "--check", str(v)).returncode == 0),
                       None)
            if fit is None:
                skipped.append(patch.name)
                continue
            runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", str(fit)])
            applied.append(str(fit))
        remember_patches(applied)
    after = git("rev-parse", "--short=9", "HEAD").stdout.strip()
    say(ui.done(f"Hermes {before} → {after}."))
    if skipped:
        say(ui.note(f"Fixes not put back (they no longer apply): {', '.join(skipped)}."))

    say(ui.step(2, 3, "Restarting the agents"))
    restart(runner)

    say(ui.step(3, 3, "Checks"))
    problems = check_agents(agents, expect_clarify=any(Path(p).name.startswith("0001") for p in applied))
    if report(agents, problems):
        say(ui.finale("Hermes is up to date, and so are your agents."))
        return 0
    say(ui.sad("Something is wrong after the update."))
    if args.yes or not confirm(f"Go back to the previous version ({before})?"):
        say(f"To go back later: git -C {hermes.hermes_checkout()} checkout {before}, then run this command again "
               "with --check.")
        return 1
    for patch in reversed([Path(p) for p in applied]):
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", "--reverse", str(patch)], check=False)
    runner.run(["git", "-C", str(hermes.hermes_checkout()), "checkout", "--quiet", before])
    for patch in mine:
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", str(patch)], check=False)
    restart(runner)
    back = report(agents, check_agents(agents, expect_clarify=any(p.name.startswith("0001") for p in mine)))
    say(ui.note(f"Back to {before}."
                + ("" if back else " If an agent still isn't answering: hermes doctor.")))
    return 0 if back else 1
