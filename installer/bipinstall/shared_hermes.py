"""One Hermes for all the places of a server (`bipinstall shared-hermes`), read-only, instead of a copy in each account.

Each place of the all-included offer used to get its own Hermes in ~/.hermes: its code, its Python and Node, its
browser and its Python environment, about 3 GB each (measured 2026-10-26: 2.9 GB in ~/.hermes, 0.2 GB around it). Here
the server keeps one copy per version, built once by root:

    /opt/bipagents/hermes/<hermes commit>-<patches>/     owned by root, read-only for the places
        hermes-agent/       Hermes' code with BipAgents' patches (hermes/patches/), no .git
        tools/              its Python, Node, uv, ffmpeg, Chromium (Hermes' pm store)
        installs/…/venv     its Python environment
        pm-runtime/         Hermes' package manager, resident
        manifest.json       says the above is a sealed payload, the layout Hermes' own Docker image and desktop app use
        bin/python3         a stable name for its Python (the bridge's vault and spaces)
        .complete           written last: a version without it is unfinished and built again

and each place points at the version it runs: ~/.hermes/shared -> /opt/bipagents/hermes/<version>, with
~/.hermes/hermes-agent -> shared/hermes-agent and ~/.local/bin/hermes calling shared/hermes-agent/.hermes/bin/hermes.
Everything a person's agents make stays in their own account: memory, sessions, skills (~/.hermes/skills), MCP servers'
settings (config.yaml) and their packages (npx's ~/.npm, uv's ~/.cache/uv), whatever Hermes installs later for one of
them (~/.hermes/tools and ~/.hermes/installs, its « writable store »), pip's ~/.local. They count in the person's 5 GB
(backup.py: personal_runtime_bytes).

Hermes treats such a tree as sealed (hermes_cli/steward.py: no .git, an install stamp naming its distribution):
`hermes update` and its lazy dependency syncs leave it alone, and an agent with a terminal on the host cannot change it.

Updates stay place by place (the guichet's waves): root builds the version a BipAgents commit pins (`shared-hermes
build`, run by the seat agent before it updates a place), the place's own update points at it and restarts its gateway
(`follow`), and the way back is the same pointer moved back. A version no place points at and no process still runs
is deleted (`shared-hermes gc`).

Off unless root turned it on (/etc/bipagents/shared-hermes.json, written by `shared-hermes enable`): a self-hosted
install keeps its own Hermes; a family machine can turn it on too (several accounts on one machine).

Ready-made versions (since 2026-10-27): building one takes minutes on each new server (Hermes' installer, its Python,
Node, Chromium, its environment), the same work every time. GitHub Actions does it once
(.github/workflows/hermes-prebuilt.yml, the same `build` in /opt/bipagents/hermes) and publishes the tree, packed with
zstd, as the release `hermes-<version>` (`hermes-<version>-linux-<x86_64|aarch64>.tar.zst` and SHA256SUMS). `build`
fetches it first: checked against SHA256SUMS, unpacked beside its place, moved in, given to root, `.complete` last. No
release for this version or this processor, a checksum that differs, no zstd, another folder than /opt/bipagents/hermes
(the tree holds its own absolute paths): it is built here as before.

A server of the offer installed just after a public push (2026-10-30: bootstrapped 4 minutes after the push, the
workflow published 3 minutes later) asks before the workflow is done: a 404 there meant building everything on the
server, past the guichet's install time. While a server of the offer installs (`cloud_install`), a missing release
is waited for (every `RELEASE_EVERY` s, up to `RELEASE_WAIT_MINUTES`) before building here. A machine whose owner
turned the shared Hermes on builds at once, as before.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import shutil
import platform
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import Callable, Dict, Iterable, List, Optional

from . import bridgeconf, hermes, release_key

REPO = Path(__file__).resolve().parents[2]
PATCHES = REPO / "hermes" / "patches"
DEFAULT_ROOT = Path("/opt/bipagents/hermes")
SETTING = Path(os.environ.get("BIPAGENTS_SHARED_HERMES_SETTING", "/etc/bipagents/shared-hermes.json"))
BUILD_USER = "bipagents-hermes"
COMPLETE = ".complete"
POINTER = "shared"          # in a HERMES_HOME
LAUNCHERS = ("hermes", "hermes-acp")
# What a place's own Hermes was made of (removed once it runs the shared one): in ~/.hermes and in each space.
# (and the caches its installer filled: the package manager's uv cache, npm's download cache).
OWN_COPY = ("hermes-agent", "tools", "installs", "cache/uv")
OWN_CACHES = (".npm/_cacache",)
PATH = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
SLUG = "Alexbob0/BipAgents"
RELEASES = f"https://github.com/{SLUG}/releases/download"
SUMS = "SHA256SUMS"
ZSTD_LEVEL = "-15"   # the pack: several times smaller than the tree, unpacked in seconds whatever the level
# A missing release waited for on a server of the offer: the bridge's workflow takes ~29 min after a push, Hermes'
# ~7 (2026-10-30), both started by the same push; past this, it is built here.
RELEASE_WAIT_MINUTES = 35
RELEASE_EVERY = 60
CLOUD_SETUPS = Path(os.environ.get("BIPAGENTS_CLOUD_SETUPS", "/etc/bipagents"))

Fetch = Callable[[str, Path], None]   # (url, file): the file written, or an exception


def cloud_install(setups: Optional[Path] = None) -> bool:
    """A server of the offer installing places: its first-boot script wrote their setups (/etc/bipagents/cloud.json,
    cloud-<n>.json from add-seat.sh), deleted once the place is installed. Never on a machine of someone's own."""
    folder = setups or CLOUD_SETUPS
    try:
        return any(folder.glob("cloud*.json"))
    except OSError:
        return False


def wait_minutes() -> float:
    """How long a missing release is waited for here: `RELEASE_WAIT_MINUTES` on a server of the offer, else 0."""
    return RELEASE_WAIT_MINUTES if cloud_install() else 0


def _published(version: str, fetch: Fetch, minutes: float, sleep: Optional[Callable[[float], None]] = None,
               clock: Optional[Callable[[], float]] = None) -> bool:
    """Whether the release of `version` is out (its SHA256SUMS answers), asked every `RELEASE_EVERY` s for up to
    `minutes` (0: once). Any refusal counts as not yet: a 404 while the workflow publishes it, a network blip."""
    sleep, clock = sleep or time.sleep, clock or time.monotonic
    until = clock() + minutes * 60
    told = False
    while True:
        with tempfile.TemporaryDirectory() as folder:
            try:
                fetch(f"{release_url(version)}/{SUMS}", Path(folder) / SUMS)
                return True
            except Exception as exc:   # noqa: BLE001 - not out yet, or no network: asked again below
                why = f"{type(exc).__name__}: {str(exc)[:120]}"
        if clock() + RELEASE_EVERY > until:
            if told:
                say(f"Ready-made Hermes {version} still not out after {minutes:.0f} min ({why}).")
            return False
        if not told:
            say(f"Ready-made Hermes {version} not out yet ({why}): GitHub Actions is most likely publishing it, "
                f"waiting up to {minutes:.0f} min.")
            told = True
        sleep(RELEASE_EVERY)


def say(text: str = "") -> None:
    print(text, flush=True)


# -- what is on ---------------------------------------------------------------------------------------------------

def root() -> Optional[Path]:
    """The folder of the shared versions when root turned it on for this machine, else None."""
    try:
        data = json.loads(SETTING.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict) or data.get("enabled") is False:
        return None
    return Path(data.get("root") or DEFAULT_ROOT)


def version_id(commit: str = hermes.HERMES_COMMIT, patches: Path = PATCHES) -> str:
    """A version: Hermes' commit and BipAgents' patches (two BipAgents commits with the same pair share a build)."""
    digest = hashlib.sha256()
    for patch in sorted(p for p in patches.glob("*.patch") if not p.name.startswith(".")):
        digest.update(patch.name.encode() + b"\0" + patch.read_bytes() + b"\0")
    return f"{commit or 'default'}-{digest.hexdigest()[:10]}"


def ready(base: Path, version: str) -> bool:
    return (base / version / COMPLETE).is_file()


def pointed(hermes_home: Path) -> Optional[str]:
    """The version this HERMES_HOME runs, None when it runs its own Hermes."""
    link = hermes_home / POINTER
    return Path(os.readlink(link)).name if link.is_symlink() else None


# -- a ready-made version (GitHub Actions' release) ----------------------------------------------------------------

def arch(machine: Optional[str] = None) -> Optional[str]:
    machine = (machine or platform.machine()).lower()
    return {"x86_64": "x86_64", "amd64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}.get(machine)


def asset(version: str, machine: Optional[str] = None) -> Optional[str]:
    """The release's archive for this processor (None: none is built for it)."""
    found = arch(machine)
    return f"hermes-{version}-linux-{found}.tar.zst" if found else None


def release_url(version: str) -> str:
    return f"{RELEASES}/hermes-{version}"


def _download(url: str, path: Path) -> None:
    """`url` into `path`, a block at a time (the archive is hundreds of MB); GitHub answers with a redirect."""
    with urllib.request.urlopen(url, timeout=60) as resp, open(path, "wb") as out:
        shutil.copyfileobj(resp, out, 1 << 20)


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def prebuilt(runner: hermes.Runner, base: Path, version: str, *, fetch: Fetch = _download,
             machine: Optional[str] = None, wait: float = 0,
             sleep: Optional[Callable[[float], None]] = None) -> bool:
    """The version from its release instead of a build: True once it is in place (sealed by the workflow, given to
    root here, `.complete` last); False, and why said, when it is to be built here instead. With `wait` (minutes, a
    server of the offer), a release not out yet is waited for that long first. Never raises."""
    name = asset(version, machine)
    if name is None:
        say(f"No ready-made Hermes for this processor ({platform.machine()}): building it here.")
        return False
    if base != DEFAULT_ROOT:
        say(f"Ready-made Hermes are made for {DEFAULT_ROOT} (their paths are inside them), not {base}: building it here.")
        return False
    if not runner.dry and not shutil.which("zstd"):
        say("No zstd on this server to unpack a ready-made Hermes (apt install zstd): building it here.")
        return False
    url = f"{release_url(version)}/{name}"
    target = base / version
    runner.log.append(f"fetch {url} (checked against its {SUMS}) and unpack it into {target}")
    if runner.dry:
        return True
    if wait and not _published(version, fetch, wait, sleep):
        say(f"Ready-made Hermes {version} not used (no release): building it here.")
        return False
    started = time.monotonic()
    fetched = started
    work: Optional[Path] = None
    try:
        base.mkdir(parents=True, exist_ok=True)
        work = Path(tempfile.mkdtemp(prefix=f".{version}-", dir=base))   # the same disk: the tree moves in at once
        fetch(f"{release_url(version)}/{SUMS}", work / SUMS)
        signature = None
        if release_key.pinned():   # the sums are trusted only once signed by BipAgents' release key
            try:
                fetch(f"{release_url(version)}/{release_key.SIGNATURE}", work / release_key.SIGNATURE)
                signature = (work / release_key.SIGNATURE).read_bytes()
            except Exception:   # noqa: BLE001 - no signature published: refused just below
                signature = None
        release_key.check((work / SUMS).read_bytes(), signature, warn=say)
        sums ={line.split()[-1].lstrip("*"): line.split()[0].lower()
                for line in (work / SUMS).read_text(encoding="utf-8").splitlines() if len(line.split()) == 2}
        if name not in sums:
            raise ValueError(f"{name} is not in the release's {SUMS}")
        archive = work / name
        fetch(url, archive)
        fetched = time.monotonic()
        if _sha256(archive) != sums[name]:
            raise ValueError(f"{name} doesn't match the release's {SUMS}")
        unpacked = work / "tree"
        unpacked.mkdir()
        # GNU tar refuses names with « .. » and strips a leading « / »; the files become root's, not the workflow's.
        done = subprocess.run(["tar", "--use-compress-program=zstd -d", "-xf", str(archive), "-C", str(unpacked),
                               "--no-same-owner"], capture_output=True, text=True)
        if done.returncode != 0:
            raise ValueError(f"unpacking {name}: {(done.stderr or done.stdout).strip()[-300:]}")
        archive.unlink()
        inside = sorted(p.name for p in unpacked.iterdir())
        if inside != [version] or not (unpacked / version / "manifest.json").is_file():
            raise ValueError(f"{name} holds {inside[:3]}, not the sealed tree of {version}")
        runner.run(["rm", "-rf", str(target)])   # an unfinished build or download starts over
        os.replace(unpacked / version, target)
        runner.run(["chown", "-R", "root:root", str(target)])
        runner.run(["chmod", "-R", "a+rX,go-w", str(target)])
        (target / COMPLETE).write_text(json.dumps({
            "version": version, "hermes_commit": hermes.HERMES_COMMIT, "browser": True, "source": url,
            "fetched": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")}) + "\n")
        (target / COMPLETE).chmod(0o644)
    except Exception as exc:   # noqa: BLE001 - 404 before the workflow published it, no network, a full disk...
        say(f"Ready-made Hermes {version} not used ({type(exc).__name__}: {str(exc)[:200]}): building it here.")
        return False
    finally:
        if work is not None:
            shutil.rmtree(work, ignore_errors=True)
    end = time.monotonic()
    say(f"Hermes {version}: ready-made, downloaded in {fetched - started:.0f} s, unpacked in {end - fetched:.0f} s.")
    return True


def pack(base: Path, version: str, out: Path, machine: Optional[str] = None) -> Path:
    """(The release workflow) a built version as its release archive: the tree as it is, without `.complete` (a
    server writes its own once the tree is in place)."""
    name = asset(version, machine)
    if name is None or not ready(base, version):
        raise RuntimeError(f"no built Hermes {version} in {base} for this processor")
    out.mkdir(parents=True, exist_ok=True)
    archive = out / name
    subprocess.run(["tar", "-C", str(base), f"--exclude={version}/{COMPLETE}",
                    f"--use-compress-program=zstd {ZSTD_LEVEL} -T0", "-cf", str(archive), version], check=True)
    return archive


# -- building a version (as root) ---------------------------------------------------------------------------------

def build(runner: hermes.Runner, base: Path, version: str, *, browser: bool = True, patches: Path = PATCHES,
          commit: str = hermes.HERMES_COMMIT, ready_made: bool = False, fetch: Fetch = _download,
          wait: float = 0) -> Path:
    """The version made once for this server: Hermes' official installer run by an account of its own (not root)
    into its folder, BipAgents' patches put on, then sealed and handed to root, read-only for everyone else. With
    `ready_made`, its release is tried first (the workflow builds it with Hermes' browser, at the pinned commit),
    waited for `wait` minutes when it isn't out yet (`wait_minutes`)."""
    target = base / version
    if ready(base, version):
        return target
    if ready_made and browser and commit == hermes.HERMES_COMMIT and prebuilt(runner, base, version, fetch=fetch, wait=wait):
        return target
    started = time.monotonic()
    home = target / ".build-home"
    as_builder = ["sudo", "-u", BUILD_USER, "env", "-i", f"HOME={home}", f"HERMES_HOME={target}", f"PATH={PATH}",
                  f"USER={BUILD_USER}", "LANG=C.UTF-8", "TERM=dumb"]
    runner.run(f"id -u {BUILD_USER} >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin "
               f"--home-dir /nonexistent {BUILD_USER}", shell=True)
    runner.run(["rm", "-rf", str(target)])   # an unfinished build starts over
    runner.run(["install", "-d", "-m", "0755", "-o", "root", "-g", "root", str(base)])
    runner.run(["install", "-d", "-m", "0755", "-o", BUILD_USER, "-g", BUILD_USER, str(target), str(home)])
    script = home / "install.sh"
    runner.run(as_builder + ["curl", "-fsSL", hermes.INSTALL_URL, "-o", str(script)])
    flags = ["--non-interactive", "--skip-computer-use"] + ([] if browser else ["--skip-browser"])
    if commit:
        flags += ["--commit", commit]
    runner.run(as_builder + ["bash", str(script), *flags], cwd=target)
    checkout = target / "hermes-agent"
    for patch in sorted(p for p in patches.glob("*.patch") if not p.name.startswith(".")):
        runner.run(as_builder + ["git", "-C", str(checkout), "apply", str(patch)])
    python = store_python(target) if not runner.dry else target / "tools" / "python" / "bin" / "python3"
    # Hermes' package manager, resident in the payload (as its Dockerfile stages it): a sealed tree has no
    # per-home copy of it.
    stage = (f"import sys; sys.path.insert(0, {str(checkout)!r}); from pathlib import Path; "
             "from pm import stage_manager_runtime; from scripts.bundles.payload import seal_pm_runtime; "
             f"root = Path({str(target)!r}); python = Path({str(python)!r}).resolve(); "
             "stage_manager_runtime(python=python, destination=root / 'pm-runtime', project=root / 'hermes-agent' / 'pm'); "
             "seal_pm_runtime(root, python)")
    runner.run(as_builder + [f"HERMES_RUNTIME_DIR={target / 'tools'}", str(python), "-I", "-c", stage], cwd=checkout)
    runner.log.append(f"seal {target}")
    if not runner.dry:
        seal(target, version=version, commit=commit)
    runner.run(["chown", "-R", "root:root", str(target)])
    runner.run(["chmod", "-R", "a+rX,go-w", str(target)])
    runner.log.append(f"write {target / COMPLETE}")
    if not runner.dry:
        (target / COMPLETE).write_text(json.dumps({
            "version": version, "hermes_commit": commit, "browser": browser,
            "patches": sorted(p.name for p in patches.glob("*.patch") if not p.name.startswith(".")),
            "built": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")}) + "\n")
        (target / COMPLETE).chmod(0o644)
        say(f"Hermes {version}: built here in {(time.monotonic() - started) / 60:.1f} min.")
    return target


def store_python(target: Path) -> Path:
    found = sorted((target / "tools").glob("python-*/bin/python3"))
    if not found:
        raise RuntimeError(f"no Python in {target / 'tools'}: Hermes' installer did not finish")
    return found[-1]


def venv_of(target: Path) -> Path:
    """The Python environment Hermes' installer committed for this tree."""
    for facts in sorted((target / "installs").glob("*/facts.json")):
        environment = json.loads(facts.read_text(encoding="utf-8")).get("packages", {}).get("venv", {}).get("environment")
        if environment and Path(environment).is_dir():
            return Path(environment)
    raise RuntimeError(f"no committed Python environment under {target / 'installs'}")


def seal(target: Path, *, version: str, commit: str) -> None:
    """The finished install turned into a sealed payload (pm/environments.py's manifest beside the tree): every
    place's Hermes then runs on this tree's environment and store instead of building its own, writes what it adds
    later in its own HERMES_HOME (pm/paths.py: writable_store_root), and never updates the tree."""
    checkout = target / "hermes-agent"
    venv = venv_of(target)
    shutil.rmtree(checkout / ".git", ignore_errors=True)   # no git: Hermes' updater and lazy syncs leave it be
    (target / "manifest.json").write_text(json.dumps({
        "repo": "hermes-agent", "venv": str(venv.relative_to(target)), "store": "tools",
        "runtime": {"commands": {"hermes": "hermes-agent/.hermes/bin/hermes"}}}, indent=2) + "\n")
    stamp_path = checkout / "install-stamp.json"
    try:
        stamp = json.loads(stamp_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        stamp = {"schemaVersion": 2, "commit": commit}
    stamp.update(distribution="bipagents", updateMechanism="external", source="bipagents-shared",
                 bipagentsVersion=version)
    stamp_path.write_text(json.dumps(stamp, indent=2) + "\n")
    # Generations with leases are collected by their owner; nobody owns this one but root, and a place's process
    # could not write its lease here.
    (venv.parent / ".lease-managed").unlink(missing_ok=True)
    shutil.rmtree(venv.parent / ".leases", ignore_errors=True)
    stable = target / "bin" / "python3"
    stable.parent.mkdir(exist_ok=True)
    stable.unlink(missing_ok=True)
    stable.symlink_to(os.path.relpath(store_python(target), stable.parent))
    for cache in (".npm", ".cache"):   # the build's own downloads
        shutil.rmtree(target / ".build-home" / cache, ignore_errors=True)
    # Bytecode once, here: a place cannot write __pycache__ in a read-only tree (Python then compiles in memory at
    # each start). Includes the files the patches changed.
    subprocess.run([str(store_python(target)), "-m", "compileall", "-q", "-j0", str(checkout),
                    str(store_python(target).parents[1] / "lib"), str(venv)], capture_output=True)


# -- a place pointing at a version (as the person) ----------------------------------------------------------------

def _symlink(link: Path, target: str) -> None:
    """`link` -> `target`, replaced in one step (a running Hermes never sees it missing)."""
    tmp = link.with_name(f".{link.name}.new")
    tmp.unlink(missing_ok=True)
    tmp.symlink_to(target)
    os.replace(tmp, link)


def point(runner: hermes.Runner, base: Path, version: str, home: Optional[Path] = None,
          hermes_home: Optional[Path] = None) -> None:
    """This account's Hermes is now `version`: its HERMES_HOME's pointer and its commands. Its own copy, if any,
    stays until `remove_own_copy` (the gateway must run on the shared one first)."""
    home = home or Path.home()
    hermes_home = hermes_home or hermes.hermes_home()
    runner.log.append(f"link {hermes_home / POINTER} -> {base / version}")
    if runner.dry:
        return
    hermes_home.mkdir(parents=True, exist_ok=True)
    _symlink(hermes_home / POINTER, str(base / version))
    checkout = hermes_home / "hermes-agent"
    if checkout.is_symlink() or not checkout.exists():
        _symlink(checkout, f"{POINTER}/hermes-agent")
    bindir = home / ".local" / "bin"
    bindir.mkdir(parents=True, exist_ok=True)
    for name in LAUNCHERS:   # the shape Hermes' installer writes there itself
        path = bindir / name
        tmp = path.with_name(f".{name}.new")
        tmp.write_text(f'#!/bin/sh\nexec "{hermes_home / POINTER}/hermes-agent/.hermes/bin/{name}" "$@"\n',
                       encoding="utf-8")
        tmp.chmod(0o755)
        os.replace(tmp, path)


def bridge_lines(hermes_home: Path) -> List[str]:
    """The bridge's [hermes] python and root for a shared Hermes (its own guess looks in ~/.hermes/tools)."""
    python = json.dumps(str(hermes_home / POINTER / "bin" / "python3"))
    return [f"python = [{python}, \"-I\"]", f"root = {json.dumps(str(hermes_home / POINTER / 'hermes-agent'))}"]


def bridge_config(config: Path, hermes_home: Path) -> bool:
    """Those lines put in an existing bridge.toml's [hermes] table (once). Returns whether it changed."""
    if not config.is_file():
        raise FileNotFoundError(config)

    def with_lines(text: str) -> str:
        lines = text.splitlines()
        try:
            start = lines.index("[hermes]")
        except ValueError:
            return text
        end = next((i for i in range(start + 1, len(lines)) if lines[i].startswith("[")), len(lines))
        table = lines[start + 1:end]
        kept = [line for line in table if line.split("=")[0].strip() not in ("python", "root")]
        while kept and not kept[-1].strip():   # the blank line before the next table comes back below
            kept.pop()
        new = kept + bridge_lines(hermes_home) + ([""] if end < len(lines) else [])
        if new == table:
            return text
        return "\n".join(lines[:start + 1] + new + lines[end:]) + "\n"
    return bridgeconf.update(config, with_lines)


def own_copies(home: Path, hermes_home: Path, spaces: Iterable[Path] = ()) -> List[Path]:
    """What this account's own Hermes was made of, in its HERMES_HOME and its agents' spaces (which borrowed its
    code and kept a Python environment each)."""
    found = []
    for folder in [hermes_home, *spaces]:
        for name in OWN_COPY:
            path = folder / name
            if path.exists() and not path.is_symlink():
                found.append(path)
    found += [home / rel for rel in OWN_CACHES if (home / rel).is_dir()]
    return found


def size(paths: Iterable[Path]) -> int:
    total = 0
    for path in paths:
        if path.is_file():
            total += path.stat().st_size
            continue
        for folder, _, names in os.walk(path):
            for name in names:
                try:
                    total += os.lstat(os.path.join(folder, name)).st_size
                except OSError:
                    pass
    return total


def remove_own_copy(runner: hermes.Runner, paths: List[Path]) -> int:
    freed = size(paths) if not runner.dry else 0
    for path in paths:
        runner.run(["rm", "-rf", str(path)])
    return freed


def gateway_again(runner: hermes.Runner) -> None:
    """The gateway's service names the version's own files (Hermes writes ExecStart with them): written again by
    the version now pointed at, then restarted on it. Our own unit (stack.hermes_service) calls ~/.local/bin/hermes
    and only needs the restart."""
    from . import update
    for kind, _ in update.gateway_units():
        if kind == "hermes":
            runner.run([hermes.hermes_bin(), "gateway", "install", "--start-on-login"], check=False)
    update.restart(runner)


def spaces_of(home: Path) -> List[Path]:
    folder = home / ".local" / "share" / "bipagents" / "spaces"
    return sorted(p for p in folder.glob("*") if p.is_dir()) if folder.is_dir() else []


def follow(runner: hermes.Runner, home: Optional[Path] = None) -> Optional[str]:
    """At an update: this place moved to the version its BipAgents now pins, when the server has built it (the seat
    agent builds it before it updates the place). Its own copy goes once the shared one runs. None: nothing to do."""
    home = home or Path.home()
    base = root()
    if base is None:
        return None
    hermes_home = hermes.hermes_home()
    wanted = version_id()
    current = pointed(hermes_home)
    if current == wanted:
        return None
    if not ready(base, wanted):
        return (f"Hermes {wanted} is not ready on this server yet: this place keeps "
                f"{current or 'its own copy'} (the administrator builds it: bipagents shared-hermes build).")
    point(runner, base, wanted, home, hermes_home)
    from .family import find_config
    try:
        config: Optional[Path] = find_config(None)
    except SystemExit:
        config = None
    changed = bool(config) and not runner.dry and bridge_config(config, hermes_home)
    gateway_again(runner)
    if changed:
        from .family import bridge_restart
        runner.run(bridge_restart(), check=False)
    freed = remove_own_copy(runner, own_copies(home, hermes_home, spaces_of(home)))
    if not runner.dry and not (hermes_home / "hermes-agent").exists():
        _symlink(hermes_home / "hermes-agent", f"{POINTER}/hermes-agent")   # where its own copy was
    note = f", {freed / 1e9:.1f} GB of its own copy freed" if freed else ""
    return f"Hermes {wanted}, shared on this server{note}."


# -- the server's versions (as root) ------------------------------------------------------------------------------

def users_of(base: Path, homes: Iterable[Path]) -> Dict[str, List[str]]:
    """Each version and the HERMES_HOMEs pointing at it (the accounts' and their spaces')."""
    found: Dict[str, List[str]] = {}
    for home in homes:
        for hermes_home in [home / ".hermes", *spaces_of(home)]:
            link = hermes_home / POINTER
            if link.is_symlink():
                target = Path(os.readlink(link))
                if target.parent == base:
                    found.setdefault(target.name, []).append(str(hermes_home))
    return found


def running_versions(base: Path, proc: Path = Path("/proc")) -> Dict[str, int]:
    """Versions some process still runs (its program or a file it mapped is in it): a gateway not restarted yet."""
    found: Dict[str, int] = {}
    prefix = str(base) + "/"
    for entry in proc.glob("[0-9]*"):
        text = ""
        for read in (lambda: (entry / "maps").read_text(errors="ignore"), lambda: os.readlink(entry / "exe"),
                     lambda: os.readlink(entry / "cwd")):   # cwd: a build under way (its installer runs there)
            try:
                text += read() + "\n"
            except OSError:
                pass   # gone meanwhile, or not ours to read (root reads them all)
        for line in text.splitlines():
            at = line.find(prefix)
            if at >= 0:
                name = line[at + len(prefix):].split("/", 1)[0]
                found[name] = found.get(name, 0) + 1
                break
    return found


def gc(runner: hermes.Runner, base: Path, homes: Iterable[Path], keep: Iterable[str] = (),
       proc: Path = Path("/proc")) -> List[str]:
    """The versions nothing uses any more, deleted: no place points at them, no process runs them, and not the one
    this BipAgents pins (the next place to update needs it). Unfinished builds too, unless one is under way."""
    if not base.is_dir():
        return []
    used = set(users_of(base, homes)) | set(running_versions(base, proc)) | set(keep)
    removed = []
    for folder in sorted(p for p in base.iterdir() if p.is_dir()):
        if folder.name in used:
            continue
        runner.run(["rm", "-rf", str(folder)])
        removed.append(folder.name)
    return removed


def homes() -> List[Path]:
    import pwd
    return [Path(e.pw_dir) for e in pwd.getpwall() if e.pw_uid >= 1000 and e.pw_dir.startswith("/home/")]


def status(base: Path) -> List[str]:
    lines = []
    if not base.is_dir():
        return [f"No shared Hermes in {base}."]
    users = users_of(base, homes())
    running = running_versions(base)
    for folder in sorted(p for p in base.iterdir() if p.is_dir()):
        state = "ready" if (folder / COMPLETE).is_file() else "unfinished"
        lines.append(f"{folder.name}: {state}, {len(users.get(folder.name, []))} place(s), "
                     f"{running.get(folder.name, 0)} process(es), {size([folder]) / 1e9:.1f} GB")
    return lines


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser(prog="bipinstall shared-hermes", description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    on = sub.add_parser("enable", help="(root) turn it on for this machine and build this version")
    on.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    on.add_argument("--no-browser", action="store_true", help="without Hermes' own Chromium")
    made = sub.add_parser("build", help="(root) build the version this BipAgents pins, if missing")
    made.add_argument("--no-browser", action="store_true")
    packed = sub.add_parser("pack", help="(the release workflow) the built version as its release archive")
    packed.add_argument("out", type=Path)
    packed.add_argument("--root", type=Path, default=DEFAULT_ROOT)
    sub.add_parser("gc", help="(root) delete the versions nothing uses")
    sub.add_parser("status", help="the versions, the places on each")
    sub.add_parser("version", help="the version this BipAgents pins")
    sub.add_parser("follow", help="(as a place) move to the version this BipAgents pins, when it is built")
    for p in (on, made):
        p.add_argument("--dry-run", action="store_true")
        p.add_argument("--no-prebuilt", action="store_true",
                       help="build it here even when its release has it (the release workflow itself)")
    args = parser.parse_args(argv)
    if args.command == "version":
        say(version_id())
        return 0
    if args.command == "pack":
        say(str(pack(args.root, version_id(), args.out)))
        return 0
    if args.command == "follow":
        said = follow(hermes.Runner())
        say(said or "Nothing to do.")
        return 0
    base = root()
    if args.command == "status":
        for line in status(base or DEFAULT_ROOT):
            say(line)
        return 0
    if os.geteuid() != 0:
        say("Run it as root (sudo): the shared Hermes belongs to root, read-only for the places.")
        return 1
    if args.command == "gc":
        if base is None:
            return 0
        removed = gc(hermes.Runner(), base, homes(), keep=[version_id()])
        say(f"Removed: {', '.join(removed)}." if removed else "Nothing to remove.")
        return 0
    runner = hermes.Runner(dry=args.dry_run)
    if args.command == "enable":
        base = args.root
        runner.log.append(f"write {SETTING}")
        if not runner.dry:
            SETTING.parent.mkdir(parents=True, exist_ok=True)
            SETTING.write_text(json.dumps({"enabled": True, "root": str(base)}) + "\n", encoding="utf-8")
            SETTING.chmod(0o644)
    if base is None:
        say(f"Not turned on here ({SETTING}): `bipinstall shared-hermes enable` first.")
        return 1
    target = build(runner, base, version_id(), browser=not args.no_browser, ready_made=not args.no_prebuilt,
                   wait=wait_minutes())
    if runner.dry:
        for line in runner.log:
            say(f"  $ {line}")
    else:
        say(f"Hermes {target.name} ready in {target}.")
    return 0


if __name__ == "__main__":
    import sys
    sys.exit(main(sys.argv[1:]))
