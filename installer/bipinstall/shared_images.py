"""The agents' container images, once per server (`bipinstall shared-images`), instead of in each account.

Each place of the all-included offer kept its own copy of two images in its rootless podman storage, about 3.5 GB an
account: the agents' browser (bipagents-browser, built from browser/Containerfile in each account, several minutes)
and Hermes' sandbox (docker.io/nousresearch/hermes-sandbox:desktop, pulled by each account at its bridge's first
start, which the registries' proxy runs too). Since 2026-10-27:

- GitHub Actions builds the browser (.github/workflows/server-images.yml) and pushes it to
  ghcr.io/alexbob0/bipagents-browser:<key>, the key a hash of browser/ (made like the bridge's release key), for
  x86_64 and ARM. Each night it asks dnf in that image whether Chrome or Fedora have something new (the same check
  as browser_refresh.py) and if so builds it again under the same key. It also mirrors Hermes' sandbox to
  ghcr.io/alexbob0/hermes-sandbox:desktop (no Docker Hub limits for the servers).
- root pulls them once into a store of the server, /var/lib/bipagents/images (`shared-images pull`: at first boot,
  each night after the workflow's build, and when the seat agent prepares a BipAgents version), under the names the
  places use (localhost/bipagents-browser:latest, docker.io/nousresearch/hermes-sandbox:desktop). The image before is
  kept as :previous (a running browser may still be on it), the one before that removed.
- each account reads that store as an additional image store (~/.config/containers/storage.conf,
  additionalimagestores): it pulls and builds nothing. Before it relies on it, the installer runs both images once as
  the account (`check`): a rootless account sees a store root filled with ids of its own, and the browser's user
  must still own its home and write there. A failed check takes the setting back; the account then gets its own
  copy of the browser from GHCR (`podman pull`, a minute), else builds it as before.

Self-hosted and family installs (no /etc/bipagents/shared-images.json) also pull the browser from GHCR rather than
build it, when the registry has this checkout's key; else they build it as before. Where this account's browser came
from is kept in ~/.config/bipagents/browser-image.json, which the nightly refresh (browser_refresh.py) follows.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import Callable, Dict, List, Optional, Sequence, Tuple

from . import browser_refresh

REPO = Path(__file__).resolve().parents[2]
SETTING = Path(os.environ.get("BIPAGENTS_SHARED_IMAGES_SETTING", "/etc/bipagents/shared-images.json"))
STORE = Path("/var/lib/bipagents/images")
RUNROOT = Path("/run/bipagents-images")
REGISTRY = "ghcr.io/alexbob0"
BROWSER = "localhost/bipagents-browser:latest"     # what `bipagents-browser` means to podman (browser_refresh.IMAGE)
SANDBOX = "docker.io/nousresearch/hermes-sandbox:desktop"   # Hermes' default sandbox (bridge-rs/src/sandbox.rs)
SANDBOX_SOURCES = (f"{REGISTRY}/hermes-sandbox:desktop", SANDBOX)   # the mirror first, Docker Hub when it fails
PREVIOUS = "previous"
STORAGE_CONF = Path(".config") / "containers" / "storage.conf"
MARK = "# BipAgents: the server's shared images"
SOURCE = Path(".config") / "bipagents" / "browser-image.json"
UNIT = "bipagents-images"

Run = Callable[[Sequence[str], int], Tuple[int, str]]

# Run as the account in each image: the ids it sees are its own (a store root filled with other ids would show them
# as « nobody »), and it can write where the image's user writes.
BROWSER_CHECK = ('test "$(stat -c %u /home/bip)" = "$(id -u)" && touch /home/bip/.bipagents-check && '
                 + browser_refresh.SMOKE)
SANDBOX_CHECK = 'test "$(stat -c %u /)" = 0 && touch /tmp/.bipagents-check'


def say(text: str = "") -> None:
    print(text, flush=True)


def browser_key(repo: Path = REPO) -> Optional[str]:
    """The browser image's key, the same hash the workflow computes: sha256 of the tree id of browser/ (the image is
    built from that folder alone), 12 characters. None outside a git checkout."""
    done = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD:browser"], capture_output=True, text=True)
    if done.returncode != 0:
        return None
    return hashlib.sha256(done.stdout.strip().encode()).hexdigest()[:12]


def browser_ref(key: str) -> str:
    return f"{REGISTRY}/bipagents-browser:{key}"


def setting() -> Optional[dict]:
    """What root pulled into the server's store, when it did: {"store", "browser": {"key", "id"}, "sandbox": {"id"}}."""
    try:
        data = json.loads(SETTING.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) and data.get("store") else None


# -- the server's store (as root) ------------------------------------------------------------------------------------

def store_podman(store: Path = STORE) -> List[str]:
    return ["podman", "--root", str(store), "--runroot", str(RUNROOT), "--storage-driver", "overlay"]


def image_id(podman: List[str], name: str, run: Run) -> str:
    code, out = run(podman + ["image", "inspect", "--format", "{{.Id}}", name], 60)
    return out.strip().removeprefix("sha256:") if code == 0 else ""


def pull_one(podman: List[str], sources: Sequence[str], name: str, run: Run) -> Tuple[str, str]:
    """The first of `sources` that pulls, named `name` in the store; the image `name` had before kept as :previous.
    Returns (its id, what was said), id '' when none pulled (the store keeps what it had)."""
    said = ""
    for source in sources:
        code, out = run(podman + ["pull", "--quiet", source], 3600)
        if code != 0:
            said = f"{source}: {out.strip()[-200:]}"
            continue
        new = image_id(podman, source, run)
        old = image_id(podman, name, run)
        previous = name.rsplit(":", 1)[0] + ":" + PREVIOUS
        if old and old != new:
            run(podman + ["rmi", previous], 120)          # the one before the one before (untagged, then pruned)
            run(podman + ["tag", old, previous], 60)
        run(podman + ["tag", new, name], 60)
        if source != name:
            run(podman + ["untag", new, source], 60)     # only the place's name holds it
        return new, f"{name} from {source}" + (" (new)" if old != new else " (unchanged)")
    return "", said


def open_store(store: Path, run: Run) -> None:
    """The store readable by every account (podman makes some of its folders root's only); the layers' own files keep
    the modes their image gave them."""
    folders = [store] + [store / name for name in ("overlay", "overlay-images", "overlay-layers", "overlay/l")]
    run(["chmod", "0755", *[str(f) for f in folders if f.is_dir()]], 60)
    for meta in ("overlay-images", "overlay-layers"):
        if (store / meta).is_dir():
            run(["chmod", "-R", "a+rX", str(store / meta)], 120)
    if (store / "overlay").is_dir():
        layers = [str(p) for p in (store / "overlay").iterdir() if p.is_dir() and p.name != "l"]
        if layers:
            run(["chmod", "a+rx", *layers], 120)
            run(["chmod", "-f", "a+r", *[f"{layer}/link" for layer in layers], *[f"{layer}/lower" for layer in layers]],
                120)


def pull(repo: Path = REPO, store: Path = STORE, run: Run = browser_refresh.run) -> int:
    """(root) The browser of this checkout's key and Hermes' sandbox into the server's store, the setting written for
    the accounts. 0 when both are there (new or kept), 1 when one is missing."""
    podman = store_podman(store)
    found: Dict[str, dict] = {}
    key = browser_key(repo)
    if key:
        browser, said = pull_one(podman, [browser_ref(key)], BROWSER, run)
        say(said or "no browser image for this checkout")
        if browser:
            found["browser"] = {"key": key, "id": browser}
    sandbox, said = pull_one(podman, SANDBOX_SOURCES, SANDBOX, run)
    say(said or "no sandbox image")
    if sandbox:
        found["sandbox"] = {"id": sandbox}
    run(podman + ["image", "prune", "--force"], 600)   # the untagged ones (two nights old)
    open_store(store, run)
    before = setting() or {}
    for name in ("browser", "sandbox"):   # a failed pull keeps what the store has
        if name not in found and name in before:
            found[name] = before[name]
    if found:
        SETTING.parent.mkdir(parents=True, exist_ok=True)
        SETTING.write_text(json.dumps({"store": str(store), **found, "pulled": datetime.datetime.now(
            datetime.timezone.utc).isoformat(timespec="seconds")}) + "\n", encoding="utf-8")
        SETTING.chmod(0o644)
    return 0 if {"browser", "sandbox"} <= set(found) or (not key and "sandbox" in found) else 1


def units(repo: Path) -> List[Tuple[str, str]]:
    """root's nightly pull: after the workflow's build (21:00 UTC) and the night's reboot (00:00-00:44), before the
    places' own browser refresh (01:00-03:00), which then restarts the idle browsers on the new image."""
    service = ("[Unit]\nDescription=BipAgents: the server's shared images (browser, sandbox) pulled again\n"
               "After=network-online.target\nWants=network-online.target\n\n"
               "[Service]\nType=oneshot\nNice=10\nIOSchedulingClass=idle\n"
               f"Environment=PYTHONPATH={repo / 'installer'}\nWorkingDirectory={repo}\n"
               "ExecStart=/usr/bin/python3 -m bipinstall.shared_images pull\n")
    timer = ("[Unit]\nDescription=BipAgents: pull the server's shared images each night\n\n[Timer]\n"
             "OnCalendar=*-*-* 00:50:00\nRandomizedDelaySec=10min\nPersistent=true\n\n"
             "[Install]\nWantedBy=timers.target\n")
    return [(f"{UNIT}.service", service), (f"{UNIT}.timer", timer)]


def install_timer(repo: Path = REPO, folder: Path = Path("/etc/systemd/system"), run: Run = browser_refresh.run) -> None:
    for name, text in units(repo):
        (folder / name).write_text(text, encoding="utf-8")
    run(["systemctl", "daemon-reload"], 60)
    run(["systemctl", "enable", f"{UNIT}.timer"], 60)


# -- an account reading it ---------------------------------------------------------------------------------------------

def storage_conf(store: Path) -> str:
    return (f"{MARK} (installer/bipinstall/shared_images.py), read-only for this account.\n"
            "[storage]\ndriver = \"overlay\"\n\n[storage.options]\n"
            f"additionalimagestores = [\"{store}\"]\n")


def wire(home: Path, store: Path) -> bool:
    """This account's podman reads the server's store too. False when the account has a storage.conf of its own
    (left as it is)."""
    path = home / STORAGE_CONF
    if path.exists() and MARK not in path.read_text(encoding="utf-8", errors="ignore"):
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(storage_conf(store), encoding="utf-8")
    return True


def unwire(home: Path) -> None:
    path = home / STORAGE_CONF
    if path.exists() and MARK in path.read_text(encoding="utf-8", errors="ignore"):
        path.unlink()


def check(engine: str, data: dict, run: Run) -> Tuple[bool, str]:
    """Both images run as this account from the server's store, with the right owners."""
    tests = [(label, image, script) for label, image, script in (("sandbox", SANDBOX, SANDBOX_CHECK),
                                                                  ("browser", BROWSER, BROWSER_CHECK))
             if data.get(label)]
    for label, image, script in tests:
        code, out = run([engine, "run", "--rm", "--pull=never", "--network=none", "--entrypoint", "/bin/sh", image, "-c",
                         script], 300)
        if code != 0:
            return False, f"the {label} image doesn't run right from the server's store ({out.strip()[-200:]})"
    return True, ""


def remember(home: Path, source: str, **extra: str) -> None:
    path = home / SOURCE
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"source": source, **extra}) + "\n", encoding="utf-8")


def source_of(home: Path) -> dict:
    try:
        data = json.loads((home / SOURCE).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return data if isinstance(data, dict) else {}


def use_shared(runner, engine: Optional[str], home: Optional[Path] = None, run: Run = browser_refresh.run,
               say: Callable[[str], None] = say) -> bool:
    """At an install: this account reads the server's images when root pulled them and they run right here. Never
    fails the install: without them, the account has its own as before."""
    home = home or Path.home()
    data = setting()
    if not data or not engine or not browser_refresh.is_podman(engine):
        return False
    store = Path(data["store"])
    runner.log.append(f"write {home / STORAGE_CONF} (additionalimagestores = {store})")
    if runner.dry:
        return True
    if not wire(home, store):
        say(f"  This account has a podman storage setting of its own ({home / STORAGE_CONF}): its own images.")
        return False
    ok, why = check(engine, data, run)
    if not ok:
        unwire(home)
        say(f"  The server's shared images can't be used here, {why}: this account gets its own.")
        return False
    return True


def browser(runner, engine: str, repo: Path = REPO, home: Optional[Path] = None, run: Run = browser_refresh.run,
            say: Callable[[str], None] = say) -> str:
    """The agents' browser image for this account, the quickest way there is: the server's store (nothing to do),
    else this checkout's image from GHCR, else built here (hermes.build_browser's way). Returns where it came from:
    « shared », « registry » or « built ». Raises RuntimeError when even the build fails."""
    home = home or Path.home()
    key = browser_key(repo)
    data = setting()
    wired = (home / STORAGE_CONF).is_file() and MARK in (home / STORAGE_CONF).read_text(encoding="utf-8", errors="ignore")
    if data and wired and key and (data.get("browser") or {}).get("key") == key:
        runner.log.append(f"browser image: the server's ({data['store']})")
        if not runner.dry:
            remember(home, "shared", key=key)
        return "shared"
    if key:
        ref = browser_ref(key)
        try:
            runner.run([engine, "pull", "--quiet", ref])
            runner.run([engine, "tag", ref, browser_refresh.IMAGE])
            runner.run([engine, "rmi", ref])    # only the name: the image is bipagents-browser now
            if not runner.dry:
                code, out = run(browser_refresh.smoke_command(engine, browser_refresh.IMAGE), 300)
                if code != 0:
                    raise RuntimeError(f"the pulled browser doesn't start: {out.strip()[-200:]}")
                remember(home, "registry", key=key)
            else:
                runner.log.append("when the registry has none: "
                                  + " ".join(browser_refresh.build_command(engine, repo, browser_refresh.IMAGE)))
            return "registry"
        except RuntimeError as exc:
            say(f"  The browser's ready-made image isn't here ({str(exc)[-160:]}): building it (a few minutes).")
    runner.run(browser_refresh.build_command(engine, repo, browser_refresh.IMAGE))
    if not runner.dry:
        remember(home, "built")
    return "built"


def main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser(prog="bipinstall shared-images", description=__doc__.split("\n\n")[0])
    sub = parser.add_subparsers(dest="command", required=True)
    pulled = sub.add_parser("pull", help="(root) the browser and the sandbox into the server's store")
    pulled.add_argument("--timer", action="store_true", help="and pull them again each night")
    sub.add_parser("key", help="the browser image's key for this checkout")
    sub.add_parser("status", help="what the server's store has")
    args = parser.parse_args(argv)
    if args.command == "key":
        say(browser_key() or "unknown")
        return 0
    if args.command == "status":
        say(json.dumps(setting() or {"store": None}, indent=2))
        return 0
    if os.geteuid() != 0:
        say("Run it as root (sudo): the server's store belongs to root, read-only for the places.")
        return 1
    if args.timer:
        install_timer()
    return pull()


if __name__ == "__main__":
    import sys
    sys.exit(main(sys.argv[1:]))
