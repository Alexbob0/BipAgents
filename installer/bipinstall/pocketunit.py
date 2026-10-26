"""The Bips' voices server (Pocket) this account runs, as its systemd user service says, for `bipagents update`.

A voice state only fits the Pocket it was computed with (voices/<language>/voices.json `engine`): the update never
lets a Pocket load states made for another one.

- `serve_matching`: the service's `--voices` follows its engine. When its Pocket can't move to the repository's
  (a failed install, a service set up by hand that couldn't be adopted), it serves the states of its own Pocket,
  taken from git (the commit the update came from, the reflog, the history) into
  ~/.local/share/bipagents/voices/pocket-<version>/<language>/: the clones are shallow, but the objects of the
  commit just left are still in the clone. Once its Pocket matches the repository's voices again, back to them.
- `adopt`: a Pocket service set up by hand (its own pocket_server.py or Python environment) becomes the installer's
  bipagents-pocket (pocket/.venv with the pinned requirements, same port, languages and voices), the old unit kept as
  `<unit>.service.disabled` and put back if the new one doesn't answer with the new Pocket.
"""
from __future__ import annotations

import json
import os
import shlex
import shutil
import subprocess
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, List, Optional

from . import hermes, voices

LANGUAGE_DIRS = {"fr": "french", "en": "english", "es": "spanish", "de": "german", "nl": "dutch"}
STANDARD = "bipagents-pocket"
# The first start of a new Pocket downloads its weights: waited for longer than a restart.
ADOPT_WAIT = 300.0
# pocket_server.py's options a hand-made service may carry, kept by the adoption (with their value or not).
KEPT_FLAGS = {"--quantize": False, "--threads": True, "--keep": True, "--idle-unload": True, "--host": True}


def units_folder(home: Path) -> Path:
    return home / ".config" / "systemd" / "user"


def old_voices_root(home: Path) -> Path:
    return home / ".local" / "share" / "bipagents" / "voices"


@dataclass
class Unit:
    path: Path
    text: str
    home: Path
    program: List[str] = field(default_factory=list)
    workdir: Optional[Path] = None

    @property
    def name(self) -> str:
        return self.path.stem

    def _expand(self, value: str) -> str:
        return value.replace("%h", str(self.home))

    @property
    def python(self) -> Path:
        return Path(self.program[0])

    @property
    def script(self) -> Optional[Path]:
        found = next((arg for arg in self.program[1:] if arg.endswith("pocket_server.py")), None)
        if found is None:
            return None
        path = Path(found)
        return path if path.is_absolute() or self.workdir is None else self.workdir / path

    def flag(self, name: str) -> Optional[str]:
        for index, arg in enumerate(self.program):
            if arg == name and index + 1 < len(self.program):
                return self.program[index + 1]
            if arg.startswith(name + "="):
                return arg.split("=", 1)[1]
        return None

    @property
    def light(self) -> bool:
        return "--onnx" in self.program or any(arg.startswith("--onnx=") for arg in self.program)

    @property
    def port(self) -> int:
        value = self.flag("--port")
        return int(value) if value and value.isdigit() else 8098

    @property
    def languages(self) -> str:
        return self.flag("--languages") or ",".join(LANGUAGE_DIRS)   # pocket_server.py's default: all of them

    @property
    def voices(self) -> Optional[Path]:
        value = self.flag("--voices")
        if value is None:   # pocket_server.py's default: the voices/ next to its folder
            return self.script.resolve().parent.parent / "voices" if self.script else None
        path = Path(value)
        return path if path.is_absolute() or self.workdir is None else self.workdir / path

    @property
    def url(self) -> str:
        host = self.flag("--host") or "127.0.0.1"
        return f"http://{'127.0.0.1' if host in ('0.0.0.0', '::') else host}:{self.port}"

    def standard(self, repo: Path) -> bool:
        """Run the way a fresh install sets it up: the repository's pocket_server.py with pocket/.venv."""
        return (self.python == repo / "pocket" / ".venv" / "bin" / "python"
                and self.script is not None and self.script.resolve() == (repo / "pocket" / "pocket_server.py").resolve())

    def extra_flags(self) -> List[str]:
        kept = []
        for index, arg in enumerate(self.program):
            name = arg.split("=", 1)[0]
            if name in KEPT_FLAGS:
                kept.append(arg)
                if KEPT_FLAGS[name] and "=" not in arg and index + 1 < len(self.program):
                    kept.append(self.program[index + 1])
        return kept

    def with_voices(self, folder: Path) -> str:
        """The unit's text with `--voices <folder>` (added when it had none)."""
        return self.with_flag("--voices", str(folder))

    def with_languages(self, codes: str) -> str:
        """The unit's text with `--languages <codes>` (added when it had none)."""
        return self.with_flag("--languages", codes)

    def with_flag(self, name: str, value: str) -> str:
        program = list(self.program)
        for index, arg in enumerate(program):
            if arg == name and index + 1 < len(program):
                program[index + 1] = value
                break
            if arg.startswith(name + "="):
                program[index] = f"{name}={value}"
                break
        else:
            program += [name, value]
        lines = [f"ExecStart={shlex.join(program)}" if line.strip().startswith("ExecStart=") else line
                 for line in self.text.splitlines()]
        return "\n".join(lines) + "\n"


def parse(path: Path, home: Path) -> Optional[Unit]:
    try:
        text = path.read_text(errors="ignore")
    except OSError:
        return None
    unit = Unit(path=path, text=text, home=home)
    for line in text.splitlines():
        key, _, value = line.strip().partition("=")
        if key == "ExecStart" and "pocket_server.py" in value:
            try:
                unit.program = [unit._expand(arg) for arg in shlex.split(value.lstrip("-@+!:"))]
            except ValueError:
                return None
        elif key == "WorkingDirectory":
            unit.workdir = Path(unit._expand(value.strip().lstrip("-")))
    return unit if unit.program else None


def find(home: Path) -> Optional[Unit]:
    """This account's Pocket service (the installer's first, else one set up by hand), None without one."""
    folder = units_folder(home)
    paths = sorted(folder.glob("*.service"), key=lambda p: p.stem != STANDARD) if folder.is_dir() else []
    for path in paths:
        unit = parse(path, home)
        if unit is not None:
            return unit
    return None


LAUNCHD = "io.github.bipagents.pocket"


def find_launchd(home: Path) -> Optional[Unit]:
    """The Pocket service of a Mac install (the LaunchAgent stack.pocket_service writes), as a Unit (its `path` a
    .plist), None without one."""
    import plistlib
    path = home / "Library" / "LaunchAgents" / f"{LAUNCHD}.plist"
    try:
        text = path.read_text(errors="ignore")
        data = plistlib.loads(text.encode())
    except (OSError, ValueError, plistlib.InvalidFileException):
        return None
    program = [str(arg) for arg in data.get("ProgramArguments") or []]
    if not any(arg.endswith("pocket_server.py") for arg in program):
        return None
    workdir = data.get("WorkingDirectory")
    return Unit(path=path, text=text, home=home, program=program, workdir=Path(workdir) if workdir else None)


def find_any(home: Path) -> Optional[Unit]:
    """This account's Pocket service, systemd's or a Mac's LaunchAgent."""
    return find(home) or find_launchd(home)


def engine_of(unit: Unit) -> Optional[str]:
    """The pocket-tts version the service's Python has (« 3.3.0 »), else the one its /health says."""
    return voices.python_pocket(unit.python) or voices.engine_at(unit.url)


def made_for(folder: Path, code: str) -> Optional[str]:
    """The Pocket version a voices folder's language was computed with (its voices.json `engine`: « 4.0.1 »)."""
    try:
        engine = json.loads((folder / LANGUAGE_DIRS[code] / "voices.json").read_text(encoding="utf-8")).get("engine")
    except (OSError, ValueError, KeyError):
        return None
    return str(engine).split()[-1] if engine else None


def fits(folder: Path, languages: List[str], engine: str, strict: bool = False) -> bool:
    """Whether the folder's states were made for `engine` (its major version); a language that doesn't say (no
    voices.json: voices of one's own) fits unless `strict`."""
    made = [made_for(folder, code) for code in languages if code in LANGUAGE_DIRS]
    return all(voices.major(version) == voices.major(engine) if version else not strict for version in made)


def _git(repo: Path, *args: str, binary: bool = False) -> subprocess.CompletedProcess:
    return subprocess.run(["git", "-C", str(repo), *args], capture_output=True, text=not binary)


def _commits(repo: Path) -> List[str]:
    """Where older voice states may still be: the commit the update came from, the reflog, the history."""
    found = [os.environ.get("BIPAGENTS_BEFORE", "")]
    found += _git(repo, "log", "-g", "--format=%H", "-n", "50", "HEAD").stdout.split()
    found += _git(repo, "log", "--format=%H", "-n", "50", "--", "voices/").stdout.split()
    seen, ordered = set(), []
    for sha in found:
        if sha and sha not in seen:
            seen.add(sha)
            ordered.append(sha)
    return ordered


def old_states(repo: Path, home: Path, engine: str, languages: List[str]) -> Optional[Path]:
    """The voice states made for `engine` (its major version), in ~/.local/share/bipagents/voices/pocket-<engine>/,
    taken from git when they aren't there yet: None when git no longer has them."""
    target = old_voices_root(home) / f"pocket-{engine}"
    codes = [code for code in languages if code in LANGUAGE_DIRS]
    missing = [code for code in codes if not fits(target, [code], engine, strict=True)]
    for code in missing:
        language = LANGUAGE_DIRS[code]
        for sha in _commits(repo):
            shown = _git(repo, "show", f"{sha}:voices/{language}/voices.json")
            if shown.returncode != 0:
                continue
            try:
                made = str(json.loads(shown.stdout).get("engine", "")).split()[-1:]
            except ValueError:
                continue
            if not made or voices.major(made[0]) != voices.major(engine):
                continue
            names = _git(repo, "ls-tree", "--name-only", f"{sha}:voices/{language}/").stdout.split()
            states = [name for name in names if name.endswith(".safetensors")]
            staging = target / f".{language}.part"
            shutil.rmtree(staging, ignore_errors=True)
            staging.mkdir(parents=True)
            ok = bool(states)
            for name in states + ["voices.json"]:
                blob = _git(repo, "cat-file", "blob", f"{sha}:voices/{language}/{name}", binary=True)
                if blob.returncode != 0:
                    ok = False
                    break
                (staging / name).write_bytes(blob.stdout)
            if not ok:
                shutil.rmtree(staging, ignore_errors=True)
                continue
            shutil.rmtree(target / language, ignore_errors=True)
            staging.rename(target / language)
            break
        else:
            return None
    return target if codes else None


def _systemctl(runner: hermes.Runner, *args: str, check: bool = False) -> None:
    runner.run(["systemctl", "--user", *args], check=check)


def serve_matching(unit: Unit, repo: Path, home: Path, runner: hermes.Runner, say: Callable[[str], None],
                   engine: Optional[str]) -> Optional[bool]:
    """The service's `--voices` made to fit its Pocket (`engine`): True when its unit changed (Pocket must restart on
    it), False when it already fits, None when no fitting states can be had (Pocket must not restart on others).
    A service whose voices are neither the repository's nor kept old ones is left as it is (False)."""
    current = unit.voices
    root = old_voices_root(home)
    managed = current is not None and (current.resolve() == (repo / "voices").resolve()
                                       or root.resolve() in current.resolve().parents)
    if not managed or not engine:
        return False
    languages = unit.languages.split(",")
    if fits(repo / "voices", languages, engine):
        target: Optional[Path] = repo / "voices"
    else:
        target = old_states(repo, home, engine, languages)
        if target is None:
            say(f"  ! The Bips' voices run Pocket {engine}, but this BipAgents' voices are made for Pocket "
                f"{made_for(repo / 'voices', languages[0])} and git no longer has Pocket {engine}'s: Pocket is not "
                f"restarted on them. Move it to Pocket {voices.pinned_pocket(repo)}: `bipagents update` again (it "
                f"installs it in {repo / 'pocket' / '.venv'}), or see pocket/README.md.")
            return None
    if current is not None and current.resolve() == target.resolve():
        return False
    runner.log.append(f"write {unit.path}")
    if not runner.dry:
        unit.path.write_text(unit.with_voices(target), encoding="utf-8")
    _systemctl(runner, "daemon-reload")
    if target == repo / "voices":
        say(f"  ✓ The Bips' voices back on this BipAgents' voices (Pocket {engine}).")
    else:
        say(f"  ! The Bips' voices still run Pocket {engine}: they keep its own voice states ({target}), not the new "
            f"ones made for Pocket {made_for(repo / 'voices', languages[0])}.")
    return True


def serve_new_languages(unit: Unit, runner: hermes.Runner, say: Callable[[str], None], engine: Optional[str]) -> bool:
    """A Pocket set up with every language of its time (`--languages fr,en,es,de` before Dutch, 2026-10-26) gets the
    newer ones once its voices folder has their states made for its Pocket (`engine`): True when its unit changed
    (Pocket must restart). One that chose fewer languages keeps its choice; without `--languages` it already serves
    them all (pocket_server.py's default)."""
    flag = unit.flag("--languages")
    folder = unit.voices
    if unit.light or flag is None or folder is None or not engine:
        return False
    codes = [code.strip() for code in flag.split(",") if code.strip()]
    added = [code for code in voices.with_new_languages(codes)[len(codes):]
             if code in LANGUAGE_DIRS and made_for(folder, code) and fits(folder, [code], engine, strict=True)]
    if not added:
        return False
    runner.log.append(f"write {unit.path}")
    if not runner.dry:
        unit.path.write_text(unit.with_languages(",".join(codes + added)), encoding="utf-8")
    _systemctl(runner, "daemon-reload")
    names = ", ".join(LANGUAGE_DIRS[code].capitalize() for code in added)
    say(f"  ✓ The Bips now speak {names} too (Pocket restarts with it).")
    return True


def adopt(unit: Unit, repo: Path, home: Path, runner: hermes.Runner, venv, say: Callable[[str], None],
          wait: float = ADOPT_WAIT) -> bool:
    """The hand-made Pocket service `unit` replaced by the installer's (bipagents-pocket, the pinned Pocket in
    pocket/.venv, same port, languages and voices): True once the new one answers with the pinned Pocket. Anything
    failing puts the old one back as it was (False)."""
    from .stack import systemd_unit
    pinned = voices.pinned_pocket(repo) or "?"
    python = repo / "pocket" / ".venv" / "bin" / "python"
    backup = unit.path.with_name(unit.path.name + ".disabled")
    current = unit.voices
    voices_dir = (repo / "voices" if current is None or old_voices_root(home).resolve() in current.resolve().parents
                  else current)
    say(f"  The Bips' voices run from a service set up by hand ({unit.path.name}, with {unit.python}): moving them "
        f"to BipAgents' own ({STANDARD}.service, port {unit.port}, languages {unit.languages}).")
    say(f"  Installing Pocket {pinned} with PyTorch in {python.parent.parent} (about 5 GB the first time, several "
        "minutes)…")
    try:
        venv(repo / "pocket", runner, "requirements.txt")
    except RuntimeError as exc:
        say(f"  ✗ Pocket {pinned} couldn't be installed ({str(exc)[-200:]}): the Bips' voices stay on "
            f"{unit.path.name}; `bipagents update` tries again.")
        return False
    program = [str(python), str(repo / "pocket" / "pocket_server.py"), "--voices", str(voices_dir),
               "--languages", unit.languages, "--port", str(unit.port), *unit.extra_flags()]
    content = systemd_unit("BipAgents voices (Pocket TTS)", program, repo / "pocket", {"PYTHONUNBUFFERED": "1"})
    new = units_folder(home) / f"{STANDARD}.service"
    say(f"  Stopping {unit.path.name} (kept as {backup.name} to go back) and starting {new.name}…")
    _systemctl(runner, "stop", unit.name)
    _systemctl(runner, "disable", unit.name)
    runner.log.append(f"mv {unit.path} {backup}")
    runner.log.append(f"write {new}")
    if not runner.dry:
        unit.path.rename(backup)
        new.write_text(content, encoding="utf-8")
    _systemctl(runner, "daemon-reload")
    _systemctl(runner, "enable", "--now", STANDARD)
    if runner.dry:
        return True
    say(f"  Waiting for Pocket {pinned} to answer (its first start downloads its weights)…")
    engine = voices.engine_at(f"http://127.0.0.1:{unit.port}", wait=wait)
    if engine and voices.major(engine) == voices.major(pinned):
        say(f"  ✓ The Bips' voices run {STANDARD} with Pocket {engine} (the old service: {backup}).")
        return True
    say(f"  ✗ {STANDARD} didn't answer with Pocket {pinned} ({engine or 'no answer'}): {unit.path.name} is put back "
        f"(`journalctl --user -u {STANDARD}` says why).")
    _systemctl(runner, "disable", "--now", STANDARD)
    new.unlink(missing_ok=True)
    backup.rename(unit.path)
    _systemctl(runner, "daemon-reload")
    _systemctl(runner, "enable", unit.name)
    _systemctl(runner, "restart", unit.name)
    return False

