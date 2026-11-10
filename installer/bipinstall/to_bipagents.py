"""`bipagents hermes-update --to-bipagents`: a Hermes the person had before BipAgents (existing.py) moved onto the
version BipAgents is built and tested on (hermes.HERMES_COMMIT), with every BipAgents fix. Their model, providers,
platforms, skills, profiles, memory and conversations stay as they are: only Hermes' code and our fixes change.

1. The plan (`plan`), shown and asked: from where to where, the fixes, the backup, what restarts. Nothing is done
   when something stands in the way (`Plan.blockers`): a Hermes newer than BipAgents' version (going back isn't safe
   for its data), changes in its code that aren't our fixes, a gateway run by hand, too little disk.
2. A backup first, in ~/.local/state/bipagents/hermes-move/<time>/ (mode 700):
   - `hermes-data.tar.gz`: every Hermes home of this account, the hub's and each agent's own space, without code
     or caches (moving.py's inventory, the same as `bipagents export`): conversations (state.db, copied with
     SQLite's backup), profiles, memory, skills, config.yaml and .env;
   - `code/`: Hermes' code and environment as they are (the checkout with its .git, installs/, tools/, the
     `hermes` commands in ~/.local/bin), copied whole, so the way back needs no network and no rebuild;
   - `units/`: the gateways' services; `existing-hermes.json` as it was; `move.json`, the journal of the move.
3. The turns going on end (the bridge's drain, drain.py), then the gateways stop: the hub and each agent's space.
4. The move, the way Hermes itself installs and updates: our fixes taken off, the target commit fetched, then that
   version's own installer (`scripts/install.sh --commit <target>`, what a fresh BipAgents install runs): it pins the
   checkout, syncs the dependencies to its lockfile, builds, syncs the bundled skills and runs Hermes' own config
   migrations. Never a bare checkout that leaves the dependencies behind.
5. Every BipAgents fix put on (hermes.apply_patches) and written in existing-hermes.json, so later updates keep them.
6. The settings checked (`settings_problems`): each home's .env byte for byte, its model section byte for byte, and
   every other setting it had still there with the same value (Hermes' migration may add settings, or drop an empty
   one); memory and the person's own files unchanged.
7. The gateways started again; every agent of the bridge answers (update.check_agents); every home's conversations
   are all still there.
8. Anything failing on the way (Ctrl-C included): back where it was (`roll_back`), code and environment from the
   copy, the backup put back, the gateways started, the agents checked, and said plainly.

`--undo-move` does 8 later, by hand, from the last move's backup.
"""
from __future__ import annotations

import datetime
import json
import os
import platform
import re
import shutil
import sqlite3
import subprocess
import tarfile
import tempfile
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Callable, Dict, List, Optional, Sequence, Tuple

import yaml

from . import hermes, ui
from .existing import RECORD

MOVES = Path(".local") / "state" / "bipagents" / "hermes-move"
JOURNAL = "move.json"
DATA = "hermes-data.tar.gz"
# Hermes' code and environment in its home (beside the checkout): its Python environments, its tools (Python, Node,
# uv…), its package manager's runtime. Copied whole before the move, put back as they were by a roll back.
CODE_PARTS = ("installs", "tools", "pm-runtime")
UNIT_GLOBS = ("hermes-gateway*.service", "bipagents-hermes.service", "bipagents-space-*.service")
PLIST_GLOBS = ("ai.hermes.gateway*.plist", "io.github.bipagents.hermes.plist", "io.github.bipagents.space.*.plist")
# config.yaml: what says which model answers and how it is reached, compared byte for byte.
MODEL_KEYS = ("model", "providers", "custom_providers", "fallback_model", "fallback_providers", "auxiliary",
              "delegation")
# Hermes manages these itself (its migrations' schema version).
MANAGED_KEYS = {"_config_version"}
# Per home, the person's own files that must not change (beside config.yaml and .env).
KEPT_FILES = ("SOUL.md", "profile.yaml", "memories")
SPARE_GB = 1.5   # beyond the copies: the target's own new tools (a Python, a Node: ~0.6 GB measured)
# Hermes files that BipAgents fixes of earlier sets changed and today's don't (0001, 0004, 0012, 0014, older variants;
# on the move to v0.23.1: 0006, 0019, the routine parts of 0008 and 0015; from the history of hermes/patches/): a
# Hermes patched with an older BipAgents may still carry them.
EARLIER_FIX_FILES = frozenset({
    "agent/image_store.py", "agent/session_persistence.py", "cli-config.yaml.example", "gateway/run.py",
    "gateway/run_state_db_housekeeping.py", "hermes_state_maintenance.py", "hermes_state_sessions.py",
    "tests/agent/test_image_store.py", "tests/agent/test_image_store_local.py", "tests/tools/test_cronjob_accents.py",
    "cron/jobs.py", "cron/scheduler_prompt.py", "hermes_cli/config_defaults.py", "tools/cronjob_job_args.py",
    "tools/cronjob_tools.py", "tools/tool_labels.py", "tests/cron/test_cron_wall_clock_once_and_zone.py",
    "tests/gateway/test_api_server_jobs.py", "tests/tools/test_bot_mode_dm_sync_peer.py",
    "tests/tools/test_cronjob_session_reports.py",
})


def say(text: str = "") -> None:
    print(text, flush=True)


class MoveFailed(RuntimeError):
    """A step failed: what to tell the person (the roll back follows)."""


def git(checkout: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(checkout), *args], capture_output=True, text=True)


def full_target(checkout: Path, wanted: str, base_file: Optional[Path] = None) -> str:
    """The 40 characters of the commit BipAgents pins: from hermes/patches/BASE when it names it, else as the checkout
    resolves it."""
    if base_file and base_file.exists():
        first = (base_file.read_text(encoding="utf-8").split() or [""])[0]
        if re.fullmatch(r"[0-9a-f]{40}", first) and first.startswith(wanted):
            return first
    done = git(checkout, "rev-parse", "--verify", "--quiet", f"{wanted}^{{commit}}")
    return done.stdout.strip() if done.returncode == 0 else wanted


# -- what is there ---------------------------------------------------------------------------------------------------

@dataclass
class Home:
    """A HERMES_HOME of this account: the hub's, or an agent's own space; with its profiles."""
    path: Path
    label: str

    def profile_dirs(self) -> List[Path]:
        found = [self.path]
        profiles = self.path / "profiles"
        if profiles.is_dir():
            found += sorted(p for p in profiles.iterdir() if (p / "config.yaml").exists())
        return found


@dataclass
class Plan:
    checkout: str
    hermes_home: str
    current: str                     # the checkout's commit now (40 characters)
    target: str                      # BipAgents' pinned commit (40 characters)
    relation: str                    # "same" | "behind" | "ahead" | "diverged" | "unknown"
    behind: Optional[int]            # commits between them, when the checkout is behind
    applied: List[str]               # our fixes on now (their files)
    fixes: List[str]                 # every BipAgents fix (names)
    homes: List[Tuple[str, str]]     # (label, path) of every Hermes home
    units: List[Tuple[str, str]]     # (how, name) of every gateway service, hub first
    gateway: str                     # existing.gateway_state(): "service" | "manual" | "stopped"
    code_bytes: int
    free_bytes: int
    backup: str                      # the folder the backup goes to
    foreign: List[str] = field(default_factory=list)   # changed files of the checkout that aren't our fixes'
    older: List[str] = field(default_factory=list)     # changed files of BipAgents fixes no longer on as they are
    blockers: List[str] = field(default_factory=list)

    @property
    def needed_bytes(self) -> int:
        return int(self.code_bytes * 1.1 + SPARE_GB * 1024 ** 3)


def gateway_services(home: Optional[Path] = None, system: Optional[str] = None) -> List[Tuple[str, str]]:
    """(how, name) of every gateway serving this account's agents: Hermes' own services (`hermes gateway install`),
    the installer's, each agent's own space."""
    home = home or Path.home()
    system = system or platform.system()
    found: List[Tuple[str, str]] = []
    if system == "Darwin":
        agents = home / "Library" / "LaunchAgents"
        for pattern in PLIST_GLOBS:
            found += [("launchd", p.stem) for p in sorted(agents.glob(pattern))]
    else:
        units = home / ".config" / "systemd" / "user"
        for pattern in UNIT_GLOBS:
            found += [("systemd", p.stem) for p in sorted(units.glob(pattern))]
    return list(dict.fromkeys(found))


def hermes_homes(config: Optional[Path], hub: Path) -> List[Home]:
    """The hub's home, then each agent's own space (bridge-rs/src/spaces.rs: `<spaces root>/<agent>`)."""
    homes = [Home(hub, "Hermes")]
    if config and config.is_file():
        from .moving import read_inventory
        try:
            inv = read_inventory(config)
            homes += [Home(p, f"space {p.name}") for p in inv.hermes_homes()[1:] if (p / "config.yaml").exists()]
        except (OSError, ValueError):
            pass
    return homes


def code_paths(checkout: Path, home: Path, user_home: Optional[Path] = None) -> List[Path]:
    """Hermes' code and environment: the checkout, the parts in its home, its commands in ~/.local/bin."""
    bin_dir = (user_home or Path.home()) / ".local" / "bin"
    found = [checkout] + [home / part for part in CODE_PARTS if (home / part).exists()]
    if bin_dir.is_dir():
        found += sorted(p for p in bin_dir.glob("hermes*") if p.is_file() or p.is_symlink())
    return found


def size_of(paths: Sequence[Path]) -> int:
    total = 0
    for path in paths:
        if path.is_symlink() or path.is_file():
            total += path.lstat().st_size
            continue
        for folder, _, files in os.walk(path):
            for name in files:
                try:
                    total += os.lstat(os.path.join(folder, name)).st_size
                except OSError:
                    pass
    return total


def relation(checkout: Path, current: str, target: str) -> Tuple[str, Optional[int]]:
    if current == target:
        return "same", 0
    if git(checkout, "cat-file", "-e", f"{target}^{{commit}}").returncode != 0:
        return "unknown", None
    if git(checkout, "merge-base", "--is-ancestor", current, target).returncode == 0:
        count = git(checkout, "rev-list", "--count", f"{current}..{target}").stdout.strip()
        return "behind", int(count) if count.isdigit() else None
    if git(checkout, "merge-base", "--is-ancestor", target, current).returncode == 0:
        return "ahead", None
    return "diverged", None


def changed_files(checkout: Path) -> List[str]:
    """Tracked files changed and untracked files (not ignored) of the checkout."""
    out = git(checkout, "status", "--porcelain", "--untracked-files=all").stdout
    return sorted({line[3:].split(" -> ")[-1].strip('"') for line in out.splitlines() if len(line) > 3})


def patch_files(patch: Path) -> List[str]:
    """The files a patch touches (from its diff headers)."""
    found: List[str] = []
    for line in patch.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("diff --git a/"):
            found.append(line.split(" b/", 1)[-1])
    return found


def fix_files(patches_dir: Path) -> frozenset:
    """Every Hermes file a BipAgents fix touches, today's or an earlier set's."""
    return frozenset(name for patch in hermes.set_patches(patches_dir) for name in patch_files(patch)) | EARLIER_FIX_FILES


def sort_changes(changed: List[str], applied: Sequence[Path], ours: frozenset) -> Tuple[List[str], List[str]]:
    """(the person's own changes, changes of older BipAgents fixes): the checkout's changes our applied fixes don't
    explain, split by whether a BipAgents fix touches that file."""
    explained = {name for patch in applied for name in patch_files(Path(patch))}
    left = [name for name in changed if name not in explained]
    return [n for n in left if n not in ours], [n for n in left if n in ours]


def plan(config: Optional[Path], patches_dir: Path, applied: List[Path], gateway: str,
         now: Optional[datetime.datetime] = None) -> Plan:
    checkout, home = hermes.hermes_checkout(), hermes.hermes_home()
    current = git(checkout, "rev-parse", "HEAD").stdout.strip()
    target = full_target(checkout, hermes.HERMES_COMMIT, patches_dir / "BASE")
    if relation(checkout, current, target)[0] == "unknown":
        git(checkout, "fetch", "--quiet", "origin", target)   # objects only: nothing of the install changes
    rel, behind = relation(checkout, current, target)
    stamp = (now or datetime.datetime.now()).strftime("%Y%m%d-%H%M%S")
    paths = code_paths(checkout, home)
    p = Plan(checkout=str(checkout), hermes_home=str(home), current=current, target=target, relation=rel,
             behind=behind, applied=[str(a) for a in applied], fixes=[f.name for f in hermes.set_patches(patches_dir)],
             homes=[(h.label, str(h.path)) for h in hermes_homes(config, home)], units=gateway_services(),
             gateway=gateway, code_bytes=size_of(paths), free_bytes=shutil.disk_usage(Path.home()).free,
             backup=str(Path.home() / MOVES / stamp))
    p.foreign, p.older = sort_changes(changed_files(checkout), applied, fix_files(patches_dir))
    p.blockers = blockers(p)
    return p


def blockers(p: Plan) -> List[str]:
    found: List[str] = []
    if p.relation == "ahead":
        found.append(f"Your Hermes ({p.current[:9]}) is newer than BipAgents' version ({p.target[:10]}): going back "
                     "isn't safe for its data. Nothing to do; a BipAgents update will catch up.")
    elif p.relation == "diverged":
        found.append(f"Your Hermes ({p.current[:9]}) isn't on the line of BipAgents' version ({p.target[:10]}) "
                     "(a fork, or commits of your own): I won't move it.")
    elif p.relation == "unknown":
        found.append(f"I can't fetch BipAgents' Hermes version ({p.target[:10]}) from your Hermes' origin: check "
                     "the network, then run this again.")
    if p.foreign:
        shown = ", ".join(p.foreign[:5]) + (" …" if len(p.foreign) > 5 else "")
        found.append(f"Hermes' code has changes that aren't BipAgents' fixes ({shown}): commit or move them aside "
                     "first, nothing was changed.")
    if p.gateway == "manual":
        found.append("Your gateway runs by hand (not as a service): stop it, run this again, then start it.")
    if p.free_bytes < p.needed_bytes:
        found.append(f"Not enough free disk: {gb(p.needed_bytes)} needed (a copy of Hermes' code for the way back, "
                     f"and its new tools), {gb(p.free_bytes)} free.")
    return found


def gb(n: int) -> str:
    return f"{n / 1024 ** 3:.1f} GB"


def show(p: Plan, say: Callable[[str], None] = say) -> None:
    say(ui.note(f"Your Hermes: {p.current[:9]}. BipAgents' version: {p.target[:10]}"
                + (f" ({p.behind:,} changes ahead of yours)." if p.behind else ".")))
    say(ui.note(f"BipAgents fixes on now: {len(p.applied)} of {len(p.fixes)}; after the move: all {len(p.fixes)}."))
    if p.older:
        say(ui.note(f"Older BipAgents fixes are still in Hermes' code ({len(p.older)} files): the move takes them off "
                    "(parked in `git stash`, and kept in the backup); today's versions go on instead."))
    say("\nWhat I'll do:")
    say(ui.note(f"back up first, in {p.backup}: your conversations, profiles, memory, skills and settings "
                f"({', '.join(label for label, _ in p.homes)}), and Hermes' code as it is ({gb(p.code_bytes)})"))
    say(ui.note("wait for the turns going on to end, then stop " + (", ".join(name for _, name in p.units)
                                                                 or "your gateway")))
    say(ui.note(f"move Hermes to {p.target[:10]} with Hermes' own installer (its dependencies, its migrations)"))
    say(ui.note(f"put on all {len(p.fixes)} BipAgents fixes, and remember them for later updates"))
    say(ui.note("check your model and settings are unchanged, restart, check every agent answers"))
    say(ui.note("anything wrong on the way: everything put back as it was, by itself"))
    say(ui.note("never touched: your model, providers, platforms, skills, profiles, memory and conversations"))


# -- the backup -------------------------------------------------------------------------------------------------------

def copy_tree(source: Path, target: Path) -> None:
    """A whole copy, links and modes kept (a clone where the disk can: btrfs, APFS, XFS)."""
    target.parent.mkdir(parents=True, exist_ok=True)
    if platform.system() == "Darwin":
        command = ["cp", "-a", "-c", str(source), str(target)]
    else:
        command = ["cp", "-a", "--reflink=auto", str(source), str(target)]
    done = subprocess.run(command, capture_output=True, text=True)
    if done.returncode != 0 and platform.system() == "Darwin":
        done = subprocess.run(["cp", "-a", str(source), str(target)], capture_output=True, text=True)
    if done.returncode != 0:
        raise MoveFailed(f"the copy of {source} failed: {(done.stderr or done.stdout).strip()[-300:]}")


def write_journal(folder: Path, data: Dict) -> None:
    """The move's journal, without the settings' snapshot (kept in memory for the check: its .env hold keys, and the
    backup has them already)."""
    path = folder / JOURNAL
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps({k: v for k, v in data.items() if k != "settings"}, indent=2) + "\n", encoding="utf-8")
    tmp.chmod(0o600)
    tmp.replace(path)


def read_journal(folder: Path) -> Dict:
    return json.loads((folder / JOURNAL).read_text(encoding="utf-8"))


def snapshot(homes: Sequence[Path]) -> Dict[str, Dict[str, str]]:
    """What the settings check compares: per profile folder, config.yaml and .env, and the person's own files."""
    found: Dict[str, Dict[str, str]] = {}
    for home in homes:
        for folder in Home(home, "").profile_dirs():
            files: Dict[str, str] = {}
            for name in ("config.yaml", ".env"):
                if (folder / name).is_file():
                    files[name] = (folder / name).read_text(encoding="utf-8", errors="surrogateescape")
            for name in KEPT_FILES:
                path = folder / name
                items = sorted(p for p in path.rglob("*") if p.is_file()) if path.is_dir() else \
                    ([path] if path.is_file() else [])
                for item in items:
                    files[str(item.relative_to(folder))] = item.read_text(encoding="utf-8", errors="surrogateescape")
            found[str(folder)] = files
    return found


def conversations(homes: Sequence[Path]) -> Dict[str, int]:
    """Per profile folder, how many messages its state.db holds (read-only)."""
    found: Dict[str, int] = {}
    for home in homes:
        for folder in Home(home, "").profile_dirs():
            db = folder / "state.db"
            if not db.is_file():
                continue
            try:
                link = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=30)
                try:
                    found[str(folder)] = int(link.execute("SELECT count(*) FROM messages").fetchone()[0])
                finally:
                    link.close()
            except sqlite3.Error:
                found[str(folder)] = -1
    return found


def back_up(p: Plan, config: Optional[Path], record: Path, runner: hermes.Runner) -> Dict:
    """Everything the way back needs, written before anything changes. Returns the journal."""
    folder = Path(p.backup)
    folder.mkdir(parents=True, exist_ok=True)
    folder.chmod(0o700)
    homes = [Path(path) for _, path in p.homes]
    journal: Dict = {"plan": asdict(p), "step": "backup", "started": datetime.datetime.now().isoformat(timespec="seconds"),
                     "code": {}, "units": {}, "data": {}, "record": None,
                     "settings": snapshot(homes), "messages": conversations(homes)}
    write_journal(folder, journal)
    # 1. the person's data (moving.py's inventory: what `bipagents export` takes, without code or caches)
    from .moving import Inventory, Tally, add_tree
    toml: Dict = {}
    if config and config.is_file():
        import tomllib
        toml = tomllib.loads(config.read_text(encoding="utf-8"))
    toml.setdefault("hermes", {}).setdefault("home", p.hermes_home)
    inv = Inventory(config or Path.home() / ".config" / "bipagents" / "bridge.toml", toml, Path.home())
    tally = Tally()
    roots = {f"home-{i}": Path(path) for i, (_, path) in enumerate(p.homes)}
    with tempfile.TemporaryDirectory(prefix="bipagents-move-") as scratch, \
            tarfile.open(folder / DATA, "w:gz", compresslevel=1) as tar:
        for name, root in roots.items():
            add_tree(tar, inv, name, root, tally, Path(scratch))
    (folder / DATA).chmod(0o600)
    journal["data"] = {name: str(root) for name, root in roots.items()}
    runner.log.append(f"backed up {tally.files} files ({gb(tally.size)}) into {folder / DATA}")
    # 2. Hermes' code and environment, whole
    for i, path in enumerate(code_paths(Path(p.checkout), Path(p.hermes_home))):
        copy = folder / "code" / f"{i:02d}-{path.name}"
        copy_tree(path, copy)
        journal["code"][str(path)] = str(copy)
    # 3. the services, and what BipAgents noted about this Hermes
    for how, name in p.units:
        source = service_file(how, name)
        if source and source.exists():
            copy = folder / "units" / source.name
            copy.parent.mkdir(exist_ok=True)
            shutil.copy2(source, copy)
            journal["units"][str(source)] = str(copy)
    if record.exists():
        shutil.copy2(record, folder / RECORD)
        journal["record"] = str(record)
    journal["step"] = "backed-up"
    write_journal(folder, journal)
    return journal


def service_file(how: str, name: str, home: Optional[Path] = None) -> Optional[Path]:
    home = home or Path.home()
    if how == "systemd":
        return home / ".config" / "systemd" / "user" / f"{name}.service"
    if how == "launchd":
        return home / "Library" / "LaunchAgents" / f"{name}.plist"
    return None


# -- the gateways -----------------------------------------------------------------------------------------------------

def active_units(units: Sequence[Tuple[str, str]], runner: hermes.Runner) -> List[Tuple[str, str]]:
    """Those running now: the only ones started again afterwards."""
    found = []
    for how, name in units:
        if how == "systemd":
            state = subprocess.run(["systemctl", "--user", "is-active", name], capture_output=True, text=True)
            if state.stdout.strip() in ("active", "activating", "reloading"):
                found.append((how, name))
        elif how == "launchd":
            state = subprocess.run(["launchctl", "print", f"gui/{os.getuid()}/{name}"], capture_output=True)
            if state.returncode == 0:
                found.append((how, name))
    return found


def stop_units(units: Sequence[Tuple[str, str]], runner: hermes.Runner) -> None:
    for how, name in units:
        if how == "systemd":
            runner.run(["systemctl", "--user", "stop", name], check=False)
        elif how == "launchd":
            runner.run(["launchctl", "bootout", f"gui/{os.getuid()}/{name}"], check=False)


def start_units(units: Sequence[Tuple[str, str]], runner: hermes.Runner) -> None:
    systemd = [name for how, name in units if how == "systemd"]
    if systemd:
        runner.run(["systemctl", "--user", "daemon-reload"], check=False)
    for how, name in units:
        if how == "systemd":
            runner.run(["systemctl", "--user", "start", name], check=False)
        elif how == "launchd":
            plist = service_file(how, name)
            runner.run(["launchctl", "bootstrap", f"gui/{os.getuid()}", str(plist)], check=False)


# -- the move ---------------------------------------------------------------------------------------------------------

def installer_flags() -> List[str]:
    """No question asked, nothing else: the person's own opt-outs (`--skip-browser` and the like, recorded by Hermes'
    package manager in installs/<id>/declined-packages.json) hold for this run too, as for `hermes update`."""
    return ["--non-interactive"]


def move_code(p: Plan, runner: hermes.Runner) -> None:
    """Our fixes off, the target fetched, then the target's own installer pins it and does the rest."""
    checkout = Path(p.checkout)
    for patch in reversed(p.applied):
        runner.run(["git", "-C", str(checkout), "apply", "--reverse", patch], check=False)
    left = changed_files(checkout)
    if left:
        known = set(p.older) | {name for patch in p.applied for name in patch_files(Path(patch))}
        theirs = [name for name in left if name not in known]
        if theirs:
            raise MoveFailed(f"Hermes' code still has changes once our fixes are off ({', '.join(theirs[:5])})")
        # Older BipAgents fixes (or ours that no longer reverse cleanly): parked, never carried onto the new version.
        runner.run(["git", "-C", str(checkout), "stash", "push", "--include-untracked", "--quiet", "-m",
                    f"bipagents-move-{Path(p.backup).name}: older BipAgents fixes"])
    runner.run(["git", "-C", str(checkout), "fetch", "--quiet", "origin", p.target])
    script = Path(p.backup) / "hermes-install.sh"
    shown = git(checkout, "show", f"{p.target}:scripts/install.sh")
    if shown.returncode != 0 or not shown.stdout.startswith("#!"):
        raise MoveFailed(f"Hermes {p.target[:10]} has no installer (scripts/install.sh) to move with")
    script.write_text(shown.stdout, encoding="utf-8")
    log = Path(p.backup) / "hermes-install.log"
    command = ["bash", str(script), *installer_flags(), "--commit", p.target,
               "--dir", str(checkout), "--hermes-home", p.hermes_home]
    runner.log.append(" ".join(command))
    with open(log, "w", encoding="utf-8") as out:
        done = subprocess.run(command, stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                              env={**os.environ, **runner.env, "HERMES_HOME": p.hermes_home})
    if done.returncode != 0:
        raise MoveFailed(f"Hermes' installer stopped ({done.returncode}): {tail(log)}")
    now = git(checkout, "rev-parse", "HEAD").stdout.strip()
    if now != p.target:
        raise MoveFailed(f"Hermes' installer left the code at {now[:9]}, not {p.target[:10]}")


def tail(path: Path, lines: int = 3) -> str:
    try:
        text = path.read_text(encoding="utf-8", errors="replace").replace("\r", "\n")
    except OSError:
        return ""
    kept = [line.strip() for line in text.splitlines() if line.strip()]
    return " / ".join(kept[-lines:])[-400:]


def put_fixes_on(p: Plan, patches_dir: Path, runner: hermes.Runner) -> List[str]:
    """Every fix on, or the move fails (the version they are made for takes them all). Returns the files applied, in
    order: what existing-hermes.json keeps (a fix stacked under later ones no longer reverse-applies alone)."""
    applied: List[str] = []
    skipped = hermes.apply_patches(runner, patches_dir, applied=applied)
    if skipped:
        raise MoveFailed(f"BipAgents fixes that don't apply on {p.target[:10]}: {', '.join(skipped)}")
    have = {Path(a).name for a in applied}
    missing = [f.name for f in hermes.set_patches(patches_dir) if f.name not in have]
    if missing:   # « already there »: only on a checkout our fixes were left on, which move_code refuses
        raise MoveFailed(f"BipAgents fixes not put on by this move: {', '.join(missing)}")
    return applied


def _section(text: str, key: str) -> str:
    """A top-level section of a YAML file, as written: its line and the indented ones under it (comments included)."""
    lines = text.splitlines(keepends=True)
    out: List[str] = []
    inside = False
    for line in lines:
        if re.match(rf"^{re.escape(key)}\s*:", line):
            inside = True
            out.append(line)
            continue
        if inside:
            if line.strip() and not line[0].isspace():
                break   # the next section, or a comment between sections
            out.append(line)
    while out and not out[-1].strip():
        out.pop()
    return "".join(out)


def _leaves(data, prefix: str = "") -> Dict[str, object]:
    if isinstance(data, dict):
        found: Dict[str, object] = {}
        for key, value in data.items():
            found.update(_leaves(value, f"{prefix}{key}."))
        return found if found or not prefix else {prefix[:-1]: data}
    return {prefix[:-1]: data}


def _empty(value) -> bool:
    return value is None or value == "" or value == [] or value == {}


def settings_problems(before: Dict[str, Dict[str, str]], after: Dict[str, Dict[str, str]]) -> List[str]:
    """What changed that must not: a .env or the model's section not byte for byte the same, a setting lost or
    changed, a file of the person's changed or gone. Hermes may add settings, and drop empty ones."""
    problems: List[str] = []
    for folder, files in before.items():
        now = after.get(folder, {})
        where = Path(folder).name
        for name, text in files.items():
            if name == "config.yaml":
                continue
            if now.get(name) != text:
                problems.append(f"{where}: {name} changed")
        old, new = files.get("config.yaml"), now.get("config.yaml")
        if old is None:
            continue
        if new is None:
            problems.append(f"{where}: config.yaml is gone")
            continue
        for key in MODEL_KEYS:
            if _section(old, key) != _section(new, key):
                problems.append(f"{where}: config.yaml's « {key} » changed")
        try:
            old_data, new_data = yaml.safe_load(old) or {}, yaml.safe_load(new) or {}
        except yaml.YAMLError:
            problems.append(f"{where}: config.yaml doesn't read anymore")
            continue
        old_leaves, new_leaves = _leaves(old_data), _leaves(new_data)
        for key, value in old_leaves.items():
            if key.split(".")[0] in MANAGED_KEYS or _empty(value):
                continue
            if key not in new_leaves:
                problems.append(f"{where}: setting {key} is gone")
            elif new_leaves[key] != value:
                problems.append(f"{where}: setting {key} changed")
    return problems


def conversation_problems(before: Dict[str, int], after: Dict[str, int]) -> List[str]:
    problems = []
    for folder, count in before.items():
        now = after.get(folder)
        if now is None or now < 0:
            problems.append(f"{Path(folder).name}: its conversations don't read")
        elif now < count:
            problems.append(f"{Path(folder).name}: {count - now} message(s) missing")
    return problems


# -- the way back -----------------------------------------------------------------------------------------------------

def _safe_member(name: str) -> bool:
    return not (name.startswith("/") or ".." in Path(name).parts)


def put_data_back(folder: Path, journal: Dict) -> int:
    """The backed-up files written back over the homes (databases first stripped of their -wal/-shm, which belong to
    the newer copy). Files the newer Hermes added stay: the older one ignores them."""
    roots = {name: Path(path) for name, path in journal.get("data", {}).items()}
    count = 0
    with tarfile.open(folder / DATA, "r:gz") as tar:
        for member in tar:
            if not member.isfile() or not _safe_member(member.name):
                continue
            root_name, _, rel = member.name.partition("/")
            root = roots.get(root_name)
            if root is None or not rel:
                continue
            target = root / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            if target.name.endswith(".db"):
                for sidecar in ("-wal", "-shm"):
                    Path(str(target) + sidecar).unlink(missing_ok=True)
            source = tar.extractfile(member)
            if source is None:
                continue
            tmp = target.with_name(f".{target.name}.bipagents-restore")
            with open(tmp, "wb") as out:
                shutil.copyfileobj(source, out)
            os.chmod(tmp, member.mode & 0o7777)
            tmp.replace(target)
            count += 1
    return count


def put_code_back(journal: Dict, runner: hermes.Runner) -> List[str]:
    """Each copied part moved back in place of the moved one (a rename: the copy is on the same disk). Returns what
    could not be put back."""
    failed: List[str] = []
    for original, copy in journal.get("code", {}).items():
        original_path, copy_path = Path(original), Path(copy)
        if not copy_path.exists() and not copy_path.is_symlink():
            failed.append(original)
            continue
        aside = original_path.with_name(f".{original_path.name}.bipagents-moved")
        try:
            if aside.exists() or aside.is_symlink():
                remove(aside)
            if original_path.exists() or original_path.is_symlink():
                original_path.rename(aside)
            shutil.move(str(copy_path), str(original_path))
            if aside.exists() or aside.is_symlink():
                remove(aside)
            runner.log.append(f"put back {original}")
        except OSError as exc:
            failed.append(f"{original} ({exc})")
    return failed


def remove(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path, ignore_errors=True)
    else:
        path.unlink(missing_ok=True)


def put_units_back(journal: Dict) -> None:
    for original, copy in journal.get("units", {}).items():
        if Path(copy).exists():
            shutil.copy2(copy, original)


def put_record_back(folder: Path, journal: Dict) -> None:
    if journal.get("record") and (folder / RECORD).exists():
        shutil.copy2(folder / RECORD, journal["record"])


def roll_back(folder: Path, runner: hermes.Runner, check: Callable[[], bool]) -> bool:
    """Back where it was before the move: gateways stopped, code and environment from the copy, the person's data
    from the backup, the services and BipAgents' notes as they were, the gateways that ran started again. True when
    every agent answers again."""
    journal = read_journal(folder)
    p = journal["plan"]
    units = [tuple(u) for u in p["units"]]
    running = [tuple(u) for u in journal.get("running", units)]
    say(ui.note("Putting everything back as it was…"))
    stop_units(units, runner)
    failed = put_code_back(journal, runner)
    if failed:
        say(ui.fail(f"Not put back: {', '.join(failed)}"))
    restored = put_data_back(folder, journal)
    put_units_back(journal)
    put_record_back(folder, journal)
    say(ui.done(f"Hermes back at {p['current'][:9]}, {restored} files of your data put back from the backup."))
    start_units(running, runner)
    journal["step"] = "rolled-back"
    journal["rolled_back"] = datetime.datetime.now().isoformat(timespec="seconds")
    write_journal(folder, journal)
    return check() and not failed


def last_move(home: Optional[Path] = None) -> Optional[Path]:
    root = (home or Path.home()) / MOVES
    found = sorted((p for p in root.glob("*") if (p / JOURNAL).exists()), reverse=True) if root.is_dir() else []
    return found[0] if found else None


# -- the command ------------------------------------------------------------------------------------------------------

def run(args, config: Optional[Path], agents: List[Dict[str, str]], applied: List[Path], patches_dir: Path,
        check_agents: Callable, remember: Callable[[List[str]], None], drain_module=None) -> int:
    from .existing import gateway_state
    from .family import confirm
    if drain_module is None:
        from . import drain as drain_module
    record = Path.home() / ".config" / "bipagents" / RECORD
    say(ui.bip("busy", "Let's look at moving your Hermes to BipAgents' version."))
    p = plan(config, patches_dir, applied, gateway_state())
    if p.relation == "same":
        say(ui.done(f"Your Hermes is already on BipAgents' version ({p.target[:10]})"
                    + (" with every fix." if len(applied) >= len(p.fixes) else
                       ": the fixes it lacks go on with bipagents hermes-update --fixes-only")))
        return 0
    show(p)
    if p.blockers:
        say("")
        for line in p.blockers:
            say(ui.fail(line))
        say("Nothing was changed.")
        return 1
    say(ui.note(f"Your agents stop for a few minutes (measured: about 3). Backup: {gb(p.code_bytes)} on disk "
                f"({gb(p.free_bytes)} free)."))
    if not args.yes and not confirm("Move your Hermes now?"):
        say("Nothing was changed.")
        return 1
    runner = hermes.Runner()
    folder = Path(p.backup)
    expect_clarify = False

    def agents_answer() -> bool:
        from .update import report
        return report(agents, check_agents(agents, expect_clarify=expect_clarify)) if agents else True

    journal: Dict = {}
    try:
        say(ui.step(1, 5, "Backing up"))
        journal = back_up(p, config, record, runner)
        say(ui.done(f"Backed up in {folder}."))
        say(ui.step(2, 5, "Stopping your agents"))
        say(ui.note(drain_module.until_quiet(what="gateways")))
        journal["running"] = active_units(p.units, runner)
        stop_units(p.units, runner)
        journal["step"] = "stopped"
        write_journal(folder, journal)
        say(ui.step(3, 5, f"Moving Hermes to {p.target[:10]}"))
        journal["step"] = "moving"
        write_journal(folder, journal)
        failing_at("install")
        move_code(p, runner)
        say(ui.done(f"Hermes {p.current[:9]} → {p.target[:10]} (dependencies and migrations by Hermes' installer)."))
        failing_at("fixes")
        every = put_fixes_on(p, patches_dir, runner)
        say(ui.done(f"All {len(p.fixes)} BipAgents fixes on."))
        homes = [Path(path) for _, path in p.homes]
        problems = settings_problems(journal["settings"], snapshot(homes))
        if problems:
            raise MoveFailed("your settings changed: " + "; ".join(problems[:5]))
        say(ui.done("Your model and settings are as they were."))
        say(ui.step(4, 5, "Starting your agents"))
        start_units(journal["running"], runner)
        journal["step"] = "started"
        write_journal(folder, journal)
        failing_at("check")
        say(ui.step(5, 5, "Checks"))
        if not journal["running"]:
            say(ui.note("No gateway was running before the move: none started, the agents not checked."))
        elif not agents_answer():
            raise MoveFailed("an agent doesn't answer after the move")
        lost = conversation_problems(journal["messages"], conversations(homes))
        if lost:
            raise MoveFailed("; ".join(lost))
        say(ui.done("Your conversations are all there."))
        out_of_reach = vault_problems(config)
        if out_of_reach:
            raise MoveFailed("the vault of " + ", ".join(out_of_reach) + " is out of reach after the move")
        say(ui.done("Your agents' vault answers."))
    except (MoveFailed, RuntimeError, OSError, KeyboardInterrupt) as exc:
        reason = "stopped by hand (Ctrl-C)" if isinstance(exc, KeyboardInterrupt) else str(exc)
        say(ui.sad(f"The move didn't go through: {reason}"))
        if not journal:
            say("Nothing was changed.")
            return 1
        back = roll_back(folder, runner, agents_answer)
        drain_module.done()
        if back:
            say(ui.finale(f"Everything is back as it was before (Hermes {p.current[:9]}). Nothing of yours was lost."))
        else:
            say(ui.fail("Hermes is back at its old version, but an agent still doesn't answer: hermes doctor, "
                        f"and the backup is in {folder}."))
        return 1
    drain_module.done()
    remember(every)
    journal["step"] = "done"
    journal["done"] = datetime.datetime.now().isoformat(timespec="seconds")
    write_journal(folder, journal)
    report_done(p, folder)
    return 0


def vault_problems(config: Optional[Path], run=subprocess.run) -> List[str]:
    """The agents that may keep credentials but whose vault is out of reach (`bipbridge vault-check`: each one's
    Hermes Python run the way the vault writes). 2026-11-09: a move pinned a newer Python and every item given to a
    space was refused, though every agent answered. Empty when there is no bridge here, or its version has no
    vault-check yet (2: unknown command)."""
    if config is None or not Path(config).is_file():
        return []
    from . import rustbridge
    try:
        done = run(rustbridge.cli(Path.home()) + ["vault-check", "--config", str(config)], capture_output=True,
                   text=True, timeout=180)
    except (OSError, subprocess.SubprocessError):
        return []
    if done.returncode != 1:
        return []
    marker = ": vault out of reach"
    return [line.split(marker)[0].strip() for line in (done.stdout or "").splitlines() if marker in line]


def failing_at(step: str) -> None:
    """For the lab: BIPAGENTS_MOVE_FAIL_AT=<step> makes the move fail there (its roll back shown)."""
    if os.environ.get("BIPAGENTS_MOVE_FAIL_AT") == step:
        raise MoveFailed(f"failure forced at « {step} » (BIPAGENTS_MOVE_FAIL_AT)")


def report_done(p: Plan, folder: Path) -> None:
    say(ui.finale(f"Your Hermes is on BipAgents' version ({p.target[:10]}) with all {len(p.fixes)} fixes."))
    say(ui.note("Your model, providers, platforms, skills, profiles, memory and conversations are as they were."))
    say(ui.note(f"Backup: {folder} ({gb(size_of([folder]))}). To go back: bipagents hermes-update --undo-move"))
    say(ui.note(f"Once you're happy (in a few days), free the space: rm -rf {folder}"))


def undo(args, agents: List[Dict[str, str]], check_agents: Callable, drain_module=None) -> int:
    """The last move undone by hand: code, environment and data back as they were before it."""
    from .family import confirm
    folder = last_move()
    if folder is None:
        say(ui.sad("No move of Hermes to undo here."))
        return 1
    journal = read_journal(folder)
    if journal.get("step") != "done":
        say(ui.sad(f"The last move ({folder.name}) is not one to undo ({journal.get('step')})."))
        return 1
    p = journal["plan"]
    say(ui.note(f"Back to Hermes {p['current'][:9]} as it was on {journal.get('started', '?')}, from {folder}."))
    say(ui.note("Conversations and memory written since then are lost (the backup is from that moment)."))
    if not args.yes and not confirm("Undo the move?"):
        say("Nothing was changed.")
        return 1

    def agents_answer() -> bool:
        from .update import report
        return report(agents, check_agents(agents, expect_clarify=False)) if agents else True

    if drain_module is None:
        from . import drain as drain_module
    say(ui.note(drain_module.until_quiet(what="gateways")))
    ok = roll_back(folder, hermes.Runner(), agents_answer)
    drain_module.done()
    say(ui.finale("Back as before the move.") if ok else ui.fail("Back at the old version, but an agent doesn't "
                                                                   "answer: hermes doctor."))
    return 0 if ok else 1


def advice(applied: List[Path], patches_dir: Path, say: Callable[[str], None] = say) -> bool:
    """For `hermes-update --check` (and before an update): whether this Hermes is on BipAgents' version, and what to
    run when it isn't. Asked of every Hermes, whatever existing-hermes.json says (a person's own Hermes can be there
    without it, measured on Alex's account 7/11). Silent when it is on it; True when it isn't. Changes nothing (no
    fetch: a version not fetched yet is offered too, --to-bipagents fetches it)."""
    checkout = hermes.hermes_checkout()
    current = git(checkout, "rev-parse", "HEAD").stdout.strip()
    target = full_target(checkout, hermes.HERMES_COMMIT, patches_dir / "BASE")
    if not current or current == target:
        return False
    rel, behind = relation(checkout, current, target)
    total = len(hermes.set_patches(patches_dir))
    when = git(checkout, "log", "-1", "--format=%cs", current).stdout.strip()
    say(ui.note(f"Your Hermes ({current[:9]}{', ' + when if when else ''}) is NOT the version BipAgents is built and "
                f"tested on ({target[:10]}): {len(applied)} of {total} BipAgents fixes are on it."))
    if rel in ("behind", "unknown"):
        gap = f"It is {behind:,} changes behind. " if rel == "behind" and behind else ""
        say(ui.note(f"{gap}To move it there with all {total} fixes:"))
        say("    bipagents hermes-update --to-bipagents")
        say(ui.note("It shows its plan and asks first. It backs up your Hermes (code, conversations, memory, "
                    "settings), keeps your model and every setting, and goes back as it was by itself if anything "
                    "fails (bipagents hermes-update --undo-move takes it back later)."))
        say(ui.note("(Plain `hermes update` goes to Hermes' latest instead, where BipAgents' fixes may not apply.)"))
    elif rel == "ahead":
        say(ui.note("It is newer than BipAgents' version: keep it, a BipAgents update will catch up."))
    else:
        say(ui.note("It isn't on the line of BipAgents' version (a fork, or commits of your own): it can't be moved "
                    "there by itself."))
    return True
