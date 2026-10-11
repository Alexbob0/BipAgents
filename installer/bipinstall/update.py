"""``bipinstall hermes-update [--check]``: update Hermes whenever the person wants, and keep BipAgents working.

1. Before: which of BipAgents' patches still apply on the new Hermes, checked on a temporary copy of it (nothing of
   the install changes). The person sees it and can stop there.
2. The update: our patches taken off, `hermes update` (its own backup, dependencies and config migration), then our
   patches put back, those that apply.
3. The gateways restarted: the hub and the agents' own spaces.
4. Checks: every agent of the bridge answers with its key, and has what the app relies on. When an agent no longer
   answers, going back to the previous version is offered.

`--check` does 1 and 4 only, without changing anything.
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
from .family import find_config

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
        problem = "ne répond pas"
        for _ in range(attempts):
            try:
                resp = client.get(f"{agent['url'].rstrip('/')}/v1/capabilities",
                                  headers={"Authorization": f"Bearer {agent['key']}"})
            except httpx.HTTPError:
                time.sleep(2)
                continue
            if resp.status_code == 401:
                problem = "refuse sa clé"
                break
            if resp.status_code != 200:
                time.sleep(2)
                continue
            caps = resp.json() if resp.headers.get("content-type", "").startswith("application/json") else {}
            features = json.dumps(caps)
            if "run" not in features:
                problem = "n'annonce plus les runs (l'app ne peut plus lui parler)"
            elif expect_clarify and "run_clarify" not in features:
                problem = "n'a plus les questions de l'agent dans l'app (correctif 0001)"
            else:
                problem = ""
            break
        problems[agent["id"]] = problem
    return problems


def report(agents: List[Dict[str, str]], problems: Dict[str, str]) -> bool:
    for agent in agents:
        problem = problems.get(agent["id"], "")
        say(ui.done(f"{agent['name']} répond.") if not problem else ui.fail(f"{agent['name']} {problem}."))
    return not any(problems.values())


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
    args = parser.parse_args(argv)
    if not Path(hermes.hermes_bin()).exists():
        say(ui.sad("Hermes n'est pas installé dans ce compte."))
        return 1
    config = find_config(args.config)
    agents = agents_of(config)
    say(ui.bip("busy", "On regarde ta version d'Hermes."))

    before = git("rev-parse", "--short=9", "HEAD").stdout.strip()
    mine = applied_now()
    chosen = recorded_patches()
    # A Hermes that came with BipAgents gets every patch; one the person had before, those they chose.
    chosen_names = {Path(c).name for c in chosen} if chosen is not None else set()
    wanted = [p for p in patches() if chosen is None or p.name in chosen_names or p.name in {m.name for m in mine}]
    out = subprocess.run([hermes.hermes_bin(), "update", "--check"], capture_output=True, text=True).stdout
    branch = channel(out)
    late = behind(out)
    say(ui.note(f"Version actuelle : {before} ; correctifs en place : "
                f"{', '.join(p.name[:4] for p in mine) or 'aucun'}."))
    if late == 0:
        say(ui.done("Hermes est déjà à jour."))
    elif late is not None:
        target, keeps = preflight(branch, wanted)
        say(ui.note(f"Nouvelle version disponible ({target}, {late} changements sur « {branch} »)."))
        for name, ok in keeps.items():
            say(ui.done(f"{name} s'applique encore.") if ok else
                ui.fail(f"{name} ne s'applique plus : sa fonction manquera jusqu'à une mise à jour de BipAgents."))
    if args.check or not late:
        say("\nTes agents, maintenant :")
        report(agents, check_agents(agents, expect_clarify=any(p.name.startswith("0001") for p in mine)))
        return 0
    if not args.yes:
        answer = input(f"{ui.question('Mettre Hermes à jour maintenant ? Tes agents redémarrent (1 à 2 minutes). (o/n)')} : ")
        if answer.strip().lower() not in ("o", "oui", "y", "yes"):
            say("Rien n'a été changé.")
            return 1

    runner = hermes.Runner()
    say(ui.step(1, 3, "Mise à jour d'Hermes"))
    for patch in reversed(mine):   # its own update then starts from a clean copy
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", "--reverse", str(patch)], check=False)
    try:
        runner.run([hermes.hermes_bin(), "update", "--yes", "--no-gateway-restart"])
    except RuntimeError as exc:
        say(ui.fail("La mise à jour d'Hermes a échoué : je remets les correctifs sur la version actuelle."))
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
        say(ui.note(f"Correctifs non remis (ils ne s'appliquent plus) : {', '.join(skipped)}."))

    say(ui.step(2, 3, "Redémarrage des agents"))
    restart(runner)

    say(ui.step(3, 3, "Vérifications"))
    problems = check_agents(agents, expect_clarify=any(Path(p).name.startswith("0001") for p in applied))
    if report(agents, problems):
        say(ui.finale("Hermes est à jour, et tes agents aussi."))
        return 0
    say(ui.sad("Quelque chose ne va pas après la mise à jour."))
    if args.yes or input(f"{ui.question(f'Revenir à la version précédente ({before}) ? (o/n)')} : ").strip().lower() \
            not in ("o", "oui", "y", "yes"):
        say(f"Pour revenir plus tard : git -C {hermes.hermes_checkout()} checkout {before}, puis relance cette commande "
            "avec --check.")
        return 1
    for patch in reversed([Path(p) for p in applied]):
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", "--reverse", str(patch)], check=False)
    runner.run(["git", "-C", str(hermes.hermes_checkout()), "checkout", "--quiet", before])
    for patch in mine:
        runner.run(["git", "-C", str(hermes.hermes_checkout()), "apply", str(patch)], check=False)
    restart(runner)
    back = report(agents, check_agents(agents, expect_clarify=any(p.name.startswith("0001") for p in mine)))
    say(ui.note(f"Revenu à {before}." + ("" if back else " Si un agent ne répond toujours pas : hermes doctor.")))
    return 0 if back else 1
