"""One's own Hermes into a place of the all-included offer: the agents a person already has on a computer (with
BipAgents, or a Hermes alone) replace the place's, with their memory, conversations, skills, routines and files. The
place keeps what makes it a place: its door and certificate, its bridge key (the phones paired with it keep working),
its backups, and the offer's model (its AI credit) and sandbox for every agent.

The app asks the place's bridge for a one-time code (bridge-rs/src/place_import.rs: an hour, one use) and shows

    bipagents send <address> <code>                                     a computer with BipAgents
    curl -fsSL …/install.sh | bash -s -- send <address> <code>          a Hermes alone

The computer and the place prove to each other they know the code (HMACs over a nonce, the place's certificate
fingerprint and its challenge), then the export (moving.py's, sealed with the code) goes over a connection pinned to
that certificate. Nothing goes through Kerfy Studio. The bridge then runs `bipinstall import-place` in a unit of its
own (restarting the bridge must not stop it): the place's agents set aside, the export's put in their place and
adjusted to this server, the place's own settings put back; on a failure, the place as it was.

    ~/.local/state/bipagents/import/        the archive, its passphrase, progress.json (what the app shows), and
                                            while it runs the export unpacked (in/) and the place's agents (aside/)
"""
from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import secrets
import shutil
import ssl
import tempfile
import time
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional, Tuple

from . import hermes, moving, ui
from .existing import profile_label, read_env, read_yaml
from .sealed import SealedError

IMPORT_CONTEXT = "bipagents-import-v1"
UPLOAD_CONTEXT = "bipagents-import-upload-v1"
ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"   # Crockford's: no I, L, O or U to misread
CODE_LENGTH = 16                                # 80 bits: no guessing, even offline from a proof
ROOT_AGENT = "hermes"                           # the main profile of a Hermes alone, as one of the place's agents
FOLDER = Path(".local") / "state" / "bipagents" / "import"
STEPS = ("Opening your export", "Putting your agents in", "Adjusting them to this server", "Restarting your agents")
MODEL_KEYS = ("OPENROUTER_API_KEY", "CUSTOM_MODEL_API_KEY")   # the offer's model key (bridge-rs/src/cloud_setup.rs)
HUB_SERVER = ("API_SERVER_ENABLED", "API_SERVER_HOST", "API_SERVER_PORT")
PLACE_DATA = {"sleep.json"}   # the place's own in its data folder (with its door's certificate)


class PlaceError(Exception):
    """Safe to show."""


def say(text: str = "") -> None:
    print(text, flush=True)


def normalize_code(code: str) -> str:
    table = str.maketrans({"O": "0", "I": "1", "L": "1"})
    return "".join(c for c in code.upper().translate(table) if c in ALPHABET)


def proof(code: str, *parts: str) -> str:
    """What proves the code without showing it (the bridge's place_import.proof, the same)."""
    return hmac.new(normalize_code(code).encode(), "\n".join(parts).encode(), hashlib.sha256).hexdigest()


# -- the computer: what it sends, and how ------------------------------------------------------------------------------

class ForPlace(moving.Inventory):
    """A BipAgents install as a place takes it: the agents its hub serves. One in a space of its own, or set up by
    hand (its own container), stays here (a place has neither)."""

    def skipped(self, path: Path) -> bool:
        return moving._relative(path, self.spaces_root) is not None or super().skipped(path)

    def roots(self) -> Dict[str, Path]:
        return {name: path for name, path in super().roots().items() if path != self.spaces_root}

    def brought(self) -> Tuple[List[str], Optional[str]]:
        return brought(self.toml)

    def left_out(self) -> List[str]:
        keep, _ = self.brought()
        return [str(t.get("display_name") or n) for n, t in self.agents().items() if n not in keep]

    def names(self) -> List[str]:
        keep, _ = self.brought()
        return [str(t.get("display_name") or n) for n, t in self.agents().items() if n in keep]


def brought(toml: Dict[str, Any]) -> Tuple[List[str], Optional[str]]:
    """The agents of a bridge config a place takes (those its hub serves, not in a space of their own), and the one
    served at the hub's root (an existing Hermes' main profile), if any."""
    hub = str((toml.get("hermes") or {}).get("hub_url") or "").rstrip("/")
    keep, main = [], None
    for name, table in (toml.get("agents") or {}).items():
        if not isinstance(table, dict) or table.get("space") or not hub:
            continue
        url = str(table.get("hermes_url") or "").rstrip("/")
        if url == hub and main is None:
            keep.append(name)
            main = name
        elif url.startswith(hub + "/p/"):
            keep.append(name)
    return keep, main


class LoneHermes(moving.Inventory):
    """A Hermes without BipAgents: its home only (every profile, their memory, conversations, skills, routines)."""

    def __init__(self, hermes_home: Path, home: Path):
        super().__init__(hermes_home / "config.yaml", {"hermes": {"home": str(hermes_home)}}, home)

    def agents(self) -> Dict[str, Dict[str, Any]]:
        return {}

    def hermes_homes(self) -> List[Path]:
        return [self.hermes_home]

    def roots(self) -> Dict[str, Path]:
        return {"hermes": self.hermes_home}

    def ports(self) -> Dict[str, int]:
        port = read_env(self.hermes_home / ".env").get("API_SERVER_PORT", "")
        return {"hub": int(port) if port.isdigit() else 8642, "bridge": 0, "lan": 0, "desk": 0}

    def profiles(self) -> List[str]:
        root = self.hermes_home / "profiles"
        return sorted(p.name for p in root.iterdir() if (p / "config.yaml").is_file()) if root.is_dir() else []

    def names(self) -> List[str]:
        return [profile_label(hermes.HUB_PROFILE)] + [profile_label(p) for p in self.profiles()]


def what_to_send(config: Optional[str], home: Path) -> moving.Inventory:
    from .family import CONFIG_CANDIDATES
    for raw in ([config] if config else []) + [os.environ.get("BRIDGE_CONFIG", "")] + list(CONFIG_CANDIDATES):
        if raw and Path(raw).expanduser().is_file():
            inv = moving.read_inventory(Path(raw).expanduser(), home)
            return ForPlace(inv.config, inv.toml, inv.home)
    hub = hermes.hermes_home()
    if (hub / "config.yaml").is_file():
        return LoneHermes(hub, home)
    raise PlaceError("there is no Hermes here to send (neither BipAgents nor ~/.hermes)")


def parse_address(text: str) -> Tuple[str, int]:
    text = text.strip().removeprefix("https://").split("/", 1)[0]
    host, _, port = text.rpartition(":") if text.count(":") == 1 else (text, "", "443")
    if not host or not port.isdigit():
        raise PlaceError(f"{text}: an address as the app shows it (e.g. 203.0.113.7:8701)")
    return host, int(port)


def pinned(pem: str) -> ssl.SSLContext:
    """A TLS context trusting this one certificate, whatever names it carries (the code proved it is the place's)."""
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    context.check_hostname = False
    context.load_verify_locations(cadata=pem)
    context.verify_flags |= ssl.VERIFY_X509_PARTIAL_CHAIN
    context.verify_flags &= ~ssl.VERIFY_X509_STRICT
    return context


def meet(host: str, port: int, code: str, client: Any = None,
         certificate: Optional[Callable[[str, int], str]] = None) -> Tuple[str, str, str]:
    """The place proves it holds the code and which certificate is its own. Returns (fingerprint, a one-time
    challenge for the upload, the certificate)."""
    import httpx
    nonce = secrets.token_hex(16)
    try:
        if client is None:
            with httpx.Client(verify=False, timeout=20) as plain:   # nothing secret goes on it; the answer is proved
                resp = plain.get(f"https://{host}:{port}/v1/import/hello", params={"nonce": nonce})
        else:
            resp = client.get(f"https://{host}:{port}/v1/import/hello", params={"nonce": nonce})
    except httpx.HTTPError as exc:
        raise PlaceError(f"the place doesn't answer at {host}:{port}") from exc
    if resp.status_code == 404:
        raise PlaceError("this place waits for nothing: ask the app for a new code (each lasts an hour)")
    if resp.status_code == 429:
        raise PlaceError("too many wrong tries: wait a few minutes")
    if resp.status_code != 200:
        raise PlaceError(f"the place answered {resp.status_code}")
    data = resp.json()
    fingerprint, challenge = str(data.get("fingerprint") or ""), str(data.get("challenge") or "")
    if not hmac.compare_digest(proof(code, IMPORT_CONTEXT, nonce, fingerprint, challenge), str(data.get("proof") or "")):
        raise PlaceError("this code isn't this place's: check both, or ask the app for a new one")
    pem = (certificate or (lambda h, p: ssl.get_server_certificate((h, p), timeout=20)))(host, port)
    if hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest() != fingerprint.lower():
        raise PlaceError("the place's certificate isn't the one it proved: nothing sent")
    return fingerprint, challenge, pem


def upload(host: str, port: int, pem: str, code: str, challenge: str, archive: Path, client: Any = None) -> Dict:
    import httpx
    size = archive.stat().st_size

    def chunks():
        sent, shown = 0, -1
        with open(archive, "rb") as handle:
            while chunk := handle.read(1 << 20):
                sent += len(chunk)
                if sent * 20 // max(size, 1) != shown:
                    shown = sent * 20 // max(size, 1)
                    print(f"\r  {sent * 100 // max(size, 1)} % ({sent / 2**20:.0f} MB)", end="", flush=True)
                yield chunk
        print(flush=True)

    headers = {"Content-Type": "application/octet-stream", "Content-Length": str(size),
               "X-Import-Challenge": challenge, "X-Import-Proof": proof(code, UPLOAD_CONTEXT, challenge, str(size))}
    url = f"https://{host}:{port}/v1/import/archive"
    try:
        if client is None:
            with httpx.Client(verify=pinned(pem), timeout=httpx.Timeout(60, write=900, read=600)) as tls:
                resp = tls.put(url, content=chunks(), headers=headers)
        else:
            resp = client.put(url, content=chunks(), headers=headers)
    except httpx.HTTPError as exc:
        raise PlaceError("the sending stopped half-way: run the same command again (the code still works)") from exc
    if resp.status_code in (401, 403):
        raise PlaceError("the place refused the code (an hour went by?): ask the app for a new one")
    if resp.status_code == 409:
        raise PlaceError("the place is already receiving or putting in an export")
    if resp.status_code == 413:
        raise PlaceError("this export is too big for a place (10 GB at most)")
    if resp.status_code == 507:
        raise PlaceError("not enough room left on the place's server for this export")
    if resp.status_code >= 400:
        raise PlaceError(f"the place answered {resp.status_code}")
    return resp.json() if resp.content else {}


def send_main(argv: List[str]) -> int:
    from .__main__ import ask, yes
    from .detect import detect

    parser = argparse.ArgumentParser(prog="bipinstall send", description="Send this computer's agents (BipAgents, or "
                                     "a Hermes alone) to your place of the all-included offer, with the code its app "
                                     "shows: they replace the place's agents")
    parser.add_argument("address", help="the place's address, as the app shows it (e.g. 203.0.113.7:8701)")
    parser.add_argument("code", help="the one-time code the app shows")
    parser.add_argument("--keep", action="store_true", help="keep the agents running here too (by default they stop "
                        "here once sent: no routine runs twice, no bot answers twice; `bipagents resume` restarts them)")
    parser.add_argument("--yes", action="store_true", help="no questions")
    parser.add_argument("--config", help="the bridge's config (default: found)")
    args = parser.parse_args(argv)
    code = normalize_code(args.code)
    if len(code) != CODE_LENGTH:
        say(ui.fail("This code isn't whole: 16 letters and digits, as the app shows it."))
        return 1
    home = Path.home()
    try:
        host, port = parse_address(args.address)
        inv = what_to_send(args.config, home)
        say("Checking the place…")
        meet(host, port, code)
    except PlaceError as exc:
        say(ui.fail(str(exc)[0].upper() + str(exc)[1:] + "."))
        return 1
    lone = isinstance(inv, LoneHermes)
    say(ui.done(f"Your place answers. To send: {', '.join(inv.names()) or 'no agent'}, with their memory, "  # type: ignore[attr-defined]
                "conversations, skills, routines and files."))
    if not lone and inv.left_out():   # type: ignore[attr-defined]
        say(ui.note(f"Agents in a space of their own stay here: {', '.join(inv.left_out())} (a place has none yet)."))  # type: ignore[attr-defined]
    say(ui.note("They replace your place's agents. The place keeps the offer's model (your AI credit) for all of them."))
    if not args.yes and not yes(ask("Send them? (y/n)", "y")):
        return 1
    machine = detect()
    runner = hermes.Runner()
    found: List[Tuple[str, str]] = []
    if not args.keep:
        found = [("hermes", "gateway")] if lone else moving.services(home, machine.system)
        say("Stopping them here first (nothing changes meanwhile)…")
        moving.stop(runner, found, home)
    scratch = Path(tempfile.mkdtemp(prefix=".bipagents-send-", dir=home))   # next to the files: the same disk
    try:
        say("Copying your agents (encrypted with the code)…")
        out = scratch / "export.bipagents"
        _, tally = moving.export(inv, out, code, machine, extra={"for_place": True, "lone": lone,
                                                                 "left_out": [] if lone else inv.left_out()})  # type: ignore[attr-defined]
        say(ui.done(f"{tally.files} files, {out.stat().st_size / 2**20:.0f} MB to send."))
        _, challenge, pem = meet(host, port, code)   # a fresh challenge: the copy may have taken a while
        say("Sending them to your place…")
        upload(host, port, pem, code, challenge, out)
    except (PlaceError, SealedError, OSError) as exc:
        if found:
            moving.resume(runner, home)
            say(ui.note("Nothing was sent: your agents run here again."))
        say(ui.fail(str(exc)[0].upper() + str(exc)[1:] + "."))
        return 1
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    say(ui.finale("Sent! Your place is putting your agents in: the app shows how it goes (a few minutes)."))
    if found:
        say("They are stopped here. `bipagents resume` starts them again (they would then run in both places).")
    else:
        say(ui.note("They still run here too: their routines run twice and their bots answer from both places."))
    return 0


# -- the place: putting an export in ------------------------------------------------------------------------------------

def progress(base: Path, step: int, done: bool = False, error: Optional[str] = None) -> None:
    """What the app shows (the bridge reads it: GET /v1/import)."""
    from .backup import write_private
    label = error or (STEPS[step - 1] if 1 <= step <= len(STEPS) else "Ready")
    write_private(base / FOLDER / "progress.json", {"step": step, "total": len(STEPS), "label": label, "done": done,
                                                    "error": error, "at": time.time()})


def _remove(path: Path) -> None:
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


class Aside:
    """The place's agents moved out of the way (renamed, on the same disk), back as they were if the import fails."""

    def __init__(self, folder: Path):
        self.folder = folder
        self.moved: List[Tuple[Path, Path]] = []
        self.cleared: List[Tuple[Path, set]] = []
        _remove(folder)

    def take(self, path: Path) -> None:
        if not (path.exists() or path.is_symlink()):
            return
        dest = self.folder / f"{len(self.moved)}-{path.name}"
        dest.parent.mkdir(parents=True, exist_ok=True)
        path.rename(dest)
        self.moved.append((path, dest))

    def clear(self, folder: Path, keep: set) -> None:
        """Everything in `folder` but `keep` (and, if the import fails, whatever it put there removed first)."""
        for entry in sorted(folder.iterdir()) if folder.is_dir() else []:
            if entry.name not in keep:
                self.take(entry)
        self.cleared.append((folder, keep))

    def put_back(self) -> None:
        for folder, keep in self.cleared:
            for entry in sorted(folder.iterdir()) if folder.is_dir() else []:
                if entry.name not in keep:
                    _remove(entry)
        for path, dest in reversed(self.moved):
            _remove(path)
            dest.rename(path)
        self.moved, self.cleared = [], []

    def drop(self) -> None:
        _remove(self.folder)
        self.moved, self.cleared = [], []


def _move_children(source: Path, target: Path, skip: Tuple[str, ...] = ()) -> None:
    """`source`'s entries into `target` (merged into a folder already there)."""
    if not source.is_dir():
        return
    target.mkdir(parents=True, exist_ok=True)
    for entry in sorted(source.iterdir()):
        if entry.name in skip:
            continue
        dest = target / entry.name
        if dest.is_dir() and entry.is_dir() and not entry.is_symlink():
            _move_children(entry, dest)
            continue
        if dest.exists() or dest.is_symlink():
            _remove(dest)
        entry.rename(dest)


def _sections(text: str) -> List[Tuple[str, List[str]]]:
    """A TOML file as (table, lines): what comes before any table under "", then each table with its lines."""
    pieces: List[Tuple[str, List[str]]] = [("", [])]
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            pieces.append((stripped.strip("[]").strip(), [line]))
        else:
            pieces[-1][1].append(line)
    return pieces


def _join(pieces: List[Tuple[str, List[str]]]) -> str:
    return "\n".join(line for _, lines in pieces for line in lines).rstrip("\n") + "\n"


def _set_line(lines: List[str], key: str, value: Optional[str]) -> List[str]:
    """`key = value` in a table's lines (replaced, added after its header, or removed with None)."""
    pattern = re.compile(rf"\s*{re.escape(key)}\s*=")
    kept = [line for line in lines if not pattern.match(line)]
    if value is not None:
        kept.insert(1, f"{key} = {json.dumps(value, ensure_ascii=False)}")
    return kept


def _person(table: str) -> bool:
    """The person's own tables of a bridge config (the rest describes the machine: the place's stay)."""
    return table.split(".", 1)[0] in ("agents", "pronunciations")


def merged_config(place_text: str, imported_text: str, keep: List[str], main: Optional[str], root: Optional[str],
                  hub_local: str, root_uploads: Optional[Path], media: Dict[str, str]) -> str:
    """The place's bridge config with the export's agents (those in `keep`) and pronunciations instead of its own,
    and `media`'s roots added. The agent `main` (served at its hub's root) becomes the profile `root`; no agent keeps
    a tailnet address."""
    pieces = [piece for piece in _sections(place_text) if not _person(piece[0])]
    for table, lines in _sections(imported_text)[1:] if imported_text else []:
        if not _person(table):
            continue
        parts = table.split(".")
        if parts[0] == "agents":
            name = parts[1] if len(parts) > 1 else ""
            if name not in keep:
                continue
            lines = _set_line(lines, "public_url", None)
            if name == main and root:
                table = ".".join(["agents", root] + parts[2:])
                lines = [f"[{table}]"] + lines[1:]
                if len(parts) == 2:
                    lines = _set_line(lines, "hermes_url", f"{hub_local.rstrip('/')}/p/{root}")
                    if root_uploads is not None:
                        lines = _set_line(lines, "upload_dir_host", str(root_uploads))
                        lines = _set_line(lines, "upload_dir_container", str(root_uploads))
        pieces.append((table, [""] + lines))
    roots_at = next((i for i, (table, _) in enumerate(pieces) if table == "media.roots"), None)
    if roots_at is None and media:
        pieces.append(("media.roots", ["", "[media.roots]"]))
        roots_at = len(pieces) - 1
    if roots_at is not None:
        lines = pieces[roots_at][1]
        header = next(i for i, line in enumerate(lines) if line.strip() == "[media.roots]")
        for prefix, folder in media.items():
            key = json.dumps(prefix, ensure_ascii=False)
            if not any(line.strip().startswith(f"{key} =") for line in lines):
                lines.insert(header + 1, f"{key} = {json.dumps(folder, ensure_ascii=False)}")
    return _join(pieces)


def _write_env(path: Path, values: Dict[str, Optional[str]]) -> None:
    """A Hermes .env with these keys set (None: removed), the others as they were."""
    lines = path.read_text(encoding="utf-8").splitlines() if path.exists() else []
    kept = [line for line in lines if line.split("=", 1)[0].strip().removeprefix("export ").strip() not in values]
    kept += [f"{key}={value}" for key, value in values.items() if value is not None]
    path.write_text("\n".join(kept) + "\n", encoding="utf-8")
    os.chmod(path, 0o600)


def place_profile(folder: Path, hub: Dict[str, Any], hub_env: Dict[str, str]) -> None:
    """One profile brought in, set as the place's agents are: the offer's model (and nothing else answering for it),
    the place's sandbox, risky commands asked."""
    import yaml
    path = folder / "config.yaml"
    data = read_yaml(path)
    for key in ("model", "terminal"):
        if isinstance(hub.get(key), dict):
            data[key] = hub[key]
    data.setdefault("approvals", {})
    if isinstance(data["approvals"], dict):
        data["approvals"]["mode"] = "manual"
    for key in ("providers", "fallback_model", "fallback_providers", "smart_model_routing"):
        data.pop(key, None)
    for task in (data.get("auxiliary") or {}).values() if isinstance(data.get("auxiliary"), dict) else []:
        if isinstance(task, dict):
            task.update({"provider": "auto", "model": ""})
            for key in ("base_url", "api_key"):
                task.pop(key, None)
    if isinstance(data.get("delegation"), dict):
        for key in ("model", "provider", "base_url", "api_key", "fallback_providers"):
            data["delegation"].pop(key, None)
    extra = ((data.get("platforms") or {}).get("api_server") or {}).get("extra") if isinstance(data.get("platforms"), dict) else None
    if isinstance(extra, dict):
        extra.pop("model_routes", None)
    from . import routing
    routing.with_routing(data, routing.DEFAULT_ORDER)   # the providers EUrouter tries first (its cache)
    path.write_text(yaml.safe_dump(data, allow_unicode=True, sort_keys=False), encoding="utf-8")
    values: Dict[str, Optional[str]] = {key: hub_env[key] for key in MODEL_KEYS if hub_env.get(key)}
    if hub_env.get("HERMES_DOCKER_BINARY"):
        values["HERMES_DOCKER_BINARY"] = hub_env["HERMES_DOCKER_BINARY"]
    values.update({key: None for key in HUB_SERVER})   # served by the place's hub, under /p/<profile>/
    _write_env(folder / ".env", values)


def _free_name(profiles: Path, wanted: str) -> str:
    name, n = wanted, 2
    while (profiles / name).exists():
        name, n = f"{wanted}-{n}", n + 1
    return name


def bring_in(archive: Path, passphrase: str, runner: hermes.Runner, base: Optional[Path] = None,
             repo: Optional[Path] = None) -> Dict[str, Any]:
    """The whole import, as the bridge starts it. Returns the export's manifest."""
    import tomllib
    repo = repo or moving.REPO
    base = base or Path(os.environ.get("HOME") or Path.home())
    work = base / FOLDER
    config = base / ".config" / "bipagents" / "bridge.toml"
    place = moving.read_inventory(config, base)
    hub_home, profiles = place.hermes_home, place.hermes_home / "profiles"
    hub_local = str((place.toml.get("hermes") or {}).get("hub_url") or "")
    place_text = config.read_text(encoding="utf-8")
    hub_yaml, hub_env = read_yaml(hub_home / "config.yaml"), read_env(hub_home / ".env")
    from .update import gateway_units

    from .rustbridge import cli
    bridge = cli(base, repo)
    browsers = bool((place.toml.get("browser") or {}).get("enabled"))

    progress(base, 1)
    tar, info, handle = moving.open_archive(archive, passphrase)
    aside = Aside(work / "aside")
    scratch = work / "in"
    try:
        if not info.get("for_place"):
            raise PlaceError("this export wasn't made for a place (`bipagents send`)")
        lone = bool(info.get("lone"))
        progress(base, 2)
        for how, name in gateway_units():
            if how == "systemd":
                runner.run(["systemctl", "--user", "stop", name], check=False)
        if browsers:   # the place's agents' browsers (services, containers) go with them
            runner.run(bridge + ["browser", "remove", "--all", "--config", str(config)], cwd=repo / "bridge",
                       check=False)
            place_text = config.read_text(encoding="utf-8")
        _remove(scratch)
        unpacked = {**info, "roots": {name: str(scratch / name) for name in info["roots"]}}
        target = moving.Target(home=base, repo=repo, tailnet_name=None, lan_address=None)
        moving.restore(tar, unpacked, target)
        imported_config = scratch / "config" / "bridge.toml"
        imported_before = imported_config.read_text(encoding="utf-8") if imported_config.is_file() else ""
        if not lone and not imported_before:
            raise PlaceError("the export holds no bridge config")
        # Its paths and ports as this place's (each port once: the old and new blocks may overlap).
        new_ports = place.ports()
        mapping = {int(info["ports"][k]): int(new_ports[k]) for k in ("hub", "bridge", "desk")
                   if int(info["ports"].get(k) or 0) and int(info["ports"][k]) != int(new_ports[k])}
        marks = {old: f"BIPAGENTSPORT{i}X" for i, old in enumerate(mapping)}
        roots = [scratch / name for name in info["roots"]]
        moving.rewrite(moving.settings_files(roots), moving.replacements(info, moving.Target(
            home=base, repo=repo, tailnet_name=None, lan_address=None, ports=marks)))
        moving.rewrite(moving.settings_files(roots), [(re.compile(re.escape(mark)), str(mapping[old]))
                                                      for old, mark in marks.items()])

        # The place's agents out of the way; the export's in.
        aside.take(profiles)
        data_skip: Tuple[str, ...] = ()
        if not lone:
            cert_dir = Path((place.toml.get("lan") or {}).get("cert_dir") or place.data_dir / "lan")
            own = PLACE_DATA | ({cert_dir.name} if cert_dir.parent == place.data_dir else set())
            aside.clear(place.data_dir, own)
            imported_lan = Path(((tomllib.loads(imported_before).get("lan") or {}).get("cert_dir")) or "lan").name
            data_skip = tuple(own | {imported_lan, "lan"})
        progress(base, 3)
        source_hub = scratch / "hermes"
        profiles.mkdir(parents=True, exist_ok=True)
        _move_children(source_hub / "profiles", profiles)
        source_hermes = info["roots"].get("hermes", "")
        media: Dict[str, str] = {f"{source_hermes}/profiles": str(profiles)} if source_hermes else {}
        for old, new in ((tomllib.loads(imported_before).get("media") or {}).get("roots") or {}).items() \
                if imported_before else []:
            media[str(old)] = str(moving.rehome(str(new), info, target))
        keep, main = brought(tomllib.loads(imported_before)) if imported_before else ([], None)
        root = _free_name(profiles, ROOT_AGENT) if lone or main else None
        if root:
            # The main profile's own (its memory, conversations, skills, routines) becomes a profile of its own,
            # served by the place's hub under /p/<root>/ with its key.
            _move_children(source_hub, profiles / root, skip=tuple(moving.HERMES_SKIP_ROOT | {"profiles"}))
            if source_hermes:
                media[f"{source_hermes}/media"] = str(profiles / root / "media")
        elif (source_hub / "media").is_dir():
            _move_children(source_hub / "media", hub_home / "media")
        if not lone:
            _move_children(scratch / "data", place.data_dir, skip=data_skip)
        for folder in sorted(profiles.iterdir()):
            if folder.is_dir() and (folder / "config.yaml").is_file():
                place_profile(folder, hub_yaml, hub_env)
            browser_ports = place.browsers_root / folder.name / "ports.json"
            browser_ports.unlink(missing_ok=True)   # picked again from the place's own ports
        text = merged_config(place_text, scratch_config_text(scratch), keep, main, root, hub_local,
                             (profiles / root / "uploads") if root else None, media)
        config.write_text(text, encoding="utf-8")
        os.chmod(config, 0o600)

        progress(base, 4)
        from . import migrations
        for line in migrations.run(runner, base):
            runner.log.append(line)
        for how, name in gateway_units():
            if how == "systemd":
                runner.run(["systemctl", "--user", "start", name], check=False)
        # Every profile served to the app as the place's agents are (its key, uploads in the sandbox, the bridge's
        # tools, Bot Mode; those of a Hermes alone claimed), then their browsers from the place's own ports.
        runner.run(bridge + ["adopt", "--config", str(config)], cwd=repo / "bridge", check=False)
        if browsers:
            runner.run(bridge + ["browser", "add", "--all", "--config", str(config)], cwd=repo / "bridge", check=False)
    except BaseException:
        aside.put_back()
        if config.read_text(encoding="utf-8") != place_text:
            config.write_text(place_text, encoding="utf-8")
            os.chmod(config, 0o600)
        for how, name in gateway_units():
            if how == "systemd":
                runner.run(["systemctl", "--user", "start", name], check=False)
        if browsers:
            runner.run(bridge + ["browser", "add", "--all", "--config", str(config)], cwd=repo / "bridge", check=False)
        raise
    finally:
        tar.close()
        handle.close()
        _remove(scratch)
    aside.drop()
    return info


def scratch_config_text(scratch: Path) -> str:
    path = scratch / "config" / "bridge.toml"
    return path.read_text(encoding="utf-8") if path.is_file() else ""


def import_place_main(argv: List[str]) -> int:
    parser = argparse.ArgumentParser(prog="bipinstall import-place", description="Put an export sent from a computer "
                                     "(`bipagents send`) into this place, in place of its agents; the bridge runs it")
    parser.add_argument("--archive", required=True, help="the export the bridge received")
    parser.add_argument("--passphrase-file", required=True, help="a file holding its code (deleted once read)")
    args = parser.parse_args(argv)
    base = Path(os.environ.get("HOME") or Path.home())
    secret = Path(args.passphrase_file)
    passphrase = secret.read_text(encoding="utf-8").strip()
    secret.unlink(missing_ok=True)
    archive = Path(args.archive)
    runner = hermes.Runner()
    try:
        bring_in(archive, passphrase, runner, base)
    except (PlaceError, SealedError, OSError, ValueError, KeyError) as exc:
        message = str(exc) if isinstance(exc, (PlaceError, SealedError)) else "the export couldn't be put in"
        progress(base, 0, error=message)
        print(f"Import failed: {exc}", flush=True)
        archive.unlink(missing_ok=True)
        from .family import bridge_restart
        runner.run(bridge_restart(base), check=False)
        return 1
    archive.unlink(missing_ok=True)
    progress(base, len(STEPS), done=True)
    from .family import bridge_restart
    runner.run(bridge_restart(base), check=False)   # last: it comes back with its agents
    print("Imported.", flush=True)
    return 0
