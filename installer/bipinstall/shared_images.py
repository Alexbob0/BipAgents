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
- the store itself stays root's (0700): a place's rootless podman could not use its layers as they are, since files
  owned by the real root are « nobody » in the place's user namespace and its containers could write nothing over them
  (« creating /etc/mtab symlink: permission denied », load test of 2026-10-27). So root gives each account a view of
  it (`views`, and at every boot before the places start, bipagents-image-views.service): the store bind-mounted
  read-only at /var/lib/bipagents/views/<account> as an idmapped mount, the store's root shown as the account itself
  and the store's ids 1… as the account's own subordinate ids, i.e. exactly what its podman's user namespace maps.
  One copy on disk (and in the page cache) for all the places; each place's view is read-only (the mount, locked in
  its namespaces) and its containers write only in its own storage, as before. When a view isn't mounted, an empty
  store's skeleton under it keeps the account's podman working with its own images.
- each account reads its view as an additional image store (~/.config/containers/storage.conf,
  additionalimagestores) through fuse-overlayfs: the browser runs with --userns=keep-id, a mapping of its own, which
  fuse-overlayfs shifts as it reads, where the kernel's overlay would copy and chown the whole image (2 GB) at each
  start of the browser. Before it relies on it, the installer runs both images once as the account (`check`), the
  browser with keep-id as the bridge runs it, its user owning its home and writing there. A failed check takes the
  setting back; the account then gets its own copy of the browser from GHCR (`podman pull`, a minute), else builds it
  as before.

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
import pwd
import re
import subprocess
from pathlib import Path
from typing import Callable, Dict, List, Optional, Sequence, Tuple

from . import browser_refresh

REPO = Path(__file__).resolve().parents[2]
SETTING = Path(os.environ.get("BIPAGENTS_SHARED_IMAGES_SETTING", "/etc/bipagents/shared-images.json"))
STORE = Path("/var/lib/bipagents/images")
VIEWS = Path("/var/lib/bipagents/views")           # one idmapped read-only view of STORE per account (`views`)
SUBIDS = (Path("/etc/subuid"), Path("/etc/subgid"))
FUSE_OVERLAYFS = "/usr/bin/fuse-overlayfs"
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
VIEWS_UNIT = "bipagents-image-views"

Run = Callable[[Sequence[str], int], Tuple[int, str]]

# Run as the account in each image: the ids it sees are its own (a store root filled with other ids would show them
# as « nobody »), and it can write where the image's user writes. The browser as the bridge runs it (browsers.rs).
BROWSER_CHECK = ('test "$(stat -c %u /home/bip)" = "$(id -u)" && touch /home/bip/.bipagents-check && '
                 + browser_refresh.SMOKE)
SANDBOX_CHECK = 'test "$(stat -c %u /)" = 0 && touch /tmp/.bipagents-check'
BROWSER_USERNS = "--userns=keep-id:uid=1000,gid=1000"


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


def subids(files: Sequence[Path] = SUBIDS) -> Dict[str, Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]]:
    """Each account's subordinate id ranges, in the file's order (podman maps them one after the other): name ->
    (uid ranges, gid ranges), each range (start, count)."""
    found: Dict[str, Tuple[List[Tuple[int, int]], List[Tuple[int, int]]]] = {}
    for index, path in enumerate(files):
        try:
            lines = path.read_text(encoding="utf-8").splitlines()
        except OSError:
            continue
        for line in lines:
            parts = line.strip().split(":")
            if len(parts) != 3 or not parts[1].isdigit() or not parts[2].isdigit() or int(parts[2]) < 1:
                continue
            found.setdefault(parts[0], ([], []))[index].append((int(parts[1]), int(parts[2])))
    return found


def idmap(uid: int, gid: int, uids: Sequence[Tuple[int, int]], gids: Sequence[Tuple[int, int]]) -> str:
    """The view's mapping (mount's X-mount.idmap, util-linux 2.39): the store's id 0 is the account, its ids 1, 2…
    the account's subordinate ids in order: what the account's rootless podman maps its containers' ids to. Each
    entry is « type:id in the store:id shown:count » (checked on Ubuntu 24.04, 2026-10-27)."""
    entries = []
    for kind, own, ranges in (("u", uid, uids), ("g", gid, gids)):
        entries.append(f"{kind}:0:{own}:1")
        inside = 1
        for start, count in ranges:
            entries.append(f"{kind}:{inside}:{start}:{count}")
            inside += count
    return " ".join(entries)


def is_mounted(path: Path, mountinfo: Path = Path("/proc/self/mountinfo")) -> bool:
    """Something is mounted right there (os.path.ismount can't tell a bind mount of the same disk)."""
    try:
        lines = mountinfo.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return False
    wanted = os.path.normpath(str(path))
    for line in lines:
        fields = line.split(" ")
        if len(fields) > 4:
            point = re.sub(r"\\([0-7]{3})", lambda m: chr(int(m.group(1), 8)), fields[4])
            if point == wanted:
                return True
    return False


SKELETON = ("overlay-images/images.lock", "overlay-layers/layers.lock", "overlay/.lock")


def skeleton(view: Path) -> None:
    """Under a view's mount point, an empty store: when the view isn't mounted (an older kernel, a failed boot unit),
    the account's podman sees an image store with nothing in it instead of failing at every command."""
    for name in ("", "overlay", "overlay/l", "overlay-images", "overlay-layers"):
        (view / name).mkdir(mode=0o755, parents=True, exist_ok=True)
    for name in SKELETON:
        lock = view / name
        if not lock.exists():
            lock.touch(mode=0o644)


def views(store: Path = STORE, folder: Path = VIEWS, run: Run = browser_refresh.run,
          files: Sequence[Path] = SUBIDS, account: Callable[[str], "pwd.struct_passwd"] = pwd.getpwnam,
          mounted_at: Optional[Callable[[Path], bool]] = None) -> List[str]:
    """(root) Each account that has subordinate ids (a place, or anyone set up for rootless podman) gets its
    read-only idmapped view of the store, once (a mounted view is left as it is: running containers read it).
    Returns the accounts whose view is mounted."""
    if not (store / "overlay-images").is_dir():
        return []
    store.chmod(0o700)                    # root's own: the places go through their views
    store.parent.chmod(0o711)             # /var/lib/bipagents: only crossed, never listed
    folder.mkdir(mode=0o711, parents=True, exist_ok=True)
    folder.chmod(0o711)
    mounted_at = mounted_at or is_mounted
    mounted = []
    for name, (uids, gids) in sorted(subids(files).items()):
        try:
            found = account(name)
        except KeyError:
            continue
        if found.pw_uid < 1000 or not uids or not gids:
            continue
        view = folder / name
        if mounted_at(view):
            mounted.append(name)
            continue
        skeleton(view)
        code, out = run(["mount", "--bind", "-o", "ro,X-mount.idmap=" + idmap(found.pw_uid, found.pw_gid, uids, gids),
                          str(store), str(view)], 60)
        if code == 0:
            mounted.append(name)
        else:
            say(f"no view of the shared images for {name}: {out.strip()[-200:]}")
    return mounted


def pull(repo: Path = REPO, store: Path = STORE, run: Run = browser_refresh.run, folder: Path = VIEWS) -> int:
    """(root) The browser of this checkout's key and Hermes' sandbox into the server's store, each account's view of it
    mounted, the setting written for the accounts. 0 when both are there (new or kept), 1 when one is missing."""
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
    views(store, folder, run)
    before = setting() or {}
    for name in ("browser", "sandbox"):   # a failed pull keeps what the store has
        if name not in found and name in before:
            found[name] = before[name]
    if found:
        SETTING.parent.mkdir(parents=True, exist_ok=True)
        pulled = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")
        SETTING.write_text(json.dumps({"store": str(store), "views": str(folder), **found, "pulled": pulled}) + "\n",
                           encoding="utf-8")
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
    # Mounts don't outlive a reboot: the views again before the places' own services (user@.service comes after
    # systemd-user-sessions.service).
    mounts = ("[Unit]\nDescription=BipAgents: each place's read-only view of the server's shared images\n"
              "After=local-fs.target\nBefore=systemd-user-sessions.service\n\n"
              "[Service]\nType=oneshot\nRemainAfterExit=yes\n"
              f"Environment=PYTHONPATH={repo / 'installer'}\nWorkingDirectory={repo}\n"
              "ExecStart=/usr/bin/python3 -m bipinstall.shared_images views\n\n"
              "[Install]\nWantedBy=multi-user.target\n")
    return [(f"{UNIT}.service", service), (f"{UNIT}.timer", timer), (f"{VIEWS_UNIT}.service", mounts)]


def install_timer(repo: Path = REPO, folder: Path = Path("/etc/systemd/system"), run: Run = browser_refresh.run) -> None:
    for name, text in units(repo):
        (folder / name).write_text(text, encoding="utf-8")
    run(["systemctl", "daemon-reload"], 60)
    run(["systemctl", "enable", f"{UNIT}.timer", f"{VIEWS_UNIT}.service"], 60)


# -- an account reading it ---------------------------------------------------------------------------------------------

def storage_conf(store: Path) -> str:
    return (f"{MARK} (installer/bipinstall/shared_images.py), read-only for this account.\n"
            "[storage]\ndriver = \"overlay\"\n\n[storage.options]\n"
            f"additionalimagestores = [\"{store}\"]\n\n"
            "# The browser's keep-id mapping shifted as it is read, not the whole image copied at each start.\n"
            f"[storage.options.overlay]\nmount_program = \"{FUSE_OVERLAYFS}\"\n")


def view_of(data: dict, user: str) -> Optional[Path]:
    """This account's view of the server's store, when root mounted one for it."""
    if not data.get("views"):
        return None             # a store pulled by an older BipAgents: no views, not usable rootless
    view = Path(data["views"]) / user
    return view if is_mounted(view) else None


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
    images = (("sandbox", SANDBOX, SANDBOX_CHECK, []), ("browser", BROWSER, BROWSER_CHECK, [BROWSER_USERNS]))
    tests = [test for test in images if data.get(test[0])]
    for label, image, script, userns in tests:
        code, out = run([engine, "run", "--rm", "--pull=never", "--network=none", *userns, "--entrypoint", "/bin/sh",
                         image, "-c", script], 300)
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
               say: Callable[[str], None] = say, user: Optional[str] = None) -> bool:
    """At an install: this account reads the server's images, through the view root mounted for it, when they run
    right here. Never fails the install: without them, the account has its own as before."""
    home = home or Path.home()
    data = setting()
    if not data or not engine or not browser_refresh.is_podman(engine):
        return False
    user = user or pwd.getpwuid(os.getuid()).pw_name
    if runner.dry:
        view = Path(data.get("views") or "?") / user
        runner.log.append(f"write {home / STORAGE_CONF} (additionalimagestores = {view})")
        return True
    store = view_of(data, user)
    if store is None:
        say("  The server's shared images have no view for this account (an older kernel or BipAgents?): its own.")
        return False
    if not os.path.exists(FUSE_OVERLAYFS):
        say("  fuse-overlayfs isn't installed: this account gets its own images.")
        return False
    runner.log.append(f"write {home / STORAGE_CONF} (additionalimagestores = {store})")
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
    pulled.add_argument("--timer", action="store_true",
                        help="and pull them again each night, the accounts' views mounted again at each boot")
    sub.add_parser("views", help="(root) each account's read-only view of the server's store, mounted")
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
    if args.command == "views":
        mounted = views()
        say("views: " + (", ".join(mounted) or "none"))
        return 0
    if args.timer:
        install_timer()
    # root's checkout at a new version: the seat agent and its scripts follow it (server_tools.py)
    try:
        from bipinstall import server_tools
        replaced = server_tools.refresh_here()
        if replaced:
            say("server tools refreshed: " + ", ".join(replaced))
    except OSError as exc:
        say("server tools not refreshed: %s" % exc)
    return pull()


if __name__ == "__main__":
    import sys
    sys.exit(main(sys.argv[1:]))
