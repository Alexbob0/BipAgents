"""The Bips' voices for every install (2026-10-26): build/voices packed as a GitHub release of the public repository
(voices/onnx/release.py, scripts/publish-voices.sh), fetched and checked by installs without a guichet, and the
migration voices-everywhere that moves their Pocket off PyTorch."""
import hashlib
import importlib.util
import io
import json
import tarfile
from pathlib import Path

import httpx
import pytest

from bipinstall import checkout, hermes, migrations, pocketunit, voices

REPO = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("voices_release", REPO / "voices" / "onnx" / "release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


def build(out: Path, engine="4.0.1", languages=("french", "dutch", "english"), bump=""):
    """A small build/voices as voices/onnx/build.py leaves it, with an older folder and samples it no longer lists."""
    index = {"format": 1, "package": 2, "engine": f"pocket-tts {engine}", "languages": {}}

    def package(name, files, credit="Pocket TTS by Kyutai, CC-BY-4.0"):
        folder = out / name
        for path, data in files.items():
            (folder / path).parent.mkdir(parents=True, exist_ok=True)
            (folder / path).write_bytes(data)
        manifest = {"engine": f"pocket-tts {engine}", "credit": credit,
                    "files": {path: {"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()}
                              for path, data in files.items()}}
        (folder / "manifest.json").write_text(json.dumps(manifest))
        return {"folder": name, "manifest_sha256": hashlib.sha256((folder / "manifest.json").read_bytes()).hexdigest(),
                "bytes": 1}

    index["mimi"] = package("mimi-aaa", {"mimi.onnx": b"decoder"})
    for language in languages:
        index["languages"][language] = package(
            f"{language}-{language[:3]}{bump}",
            {"flowlm.onnx": b"model " + language.encode() + bump.encode(), "flowlm.onnx.data": b"w" * 300,
             "voices/colibri.kv.f16": b"voice " + language.encode()})
    (out / "english-old").mkdir(parents=True)          # an older build, not in the index: never published
    (out / "samples").mkdir()
    (out / "samples" / "french.wav").write_bytes(b"RIFF")
    (out / "index.json").write_text(json.dumps(index, indent=1) + "\n")
    return index


def test_the_release_holds_one_archive_per_package_and_their_checksums(tmp_path):
    index = build(tmp_path / "voices")
    name = release.pack(tmp_path / "voices", tmp_path / "out")
    digest = hashlib.sha256((tmp_path / "voices" / "index.json").read_bytes()).hexdigest()[:12]
    assert name == f"voices-4.0.1-{digest}" == release.tag(tmp_path / "voices")
    assets = sorted(p.name for p in (tmp_path / "out").iterdir())
    assert assets == ["CREDITS.txt", "LICENSE-ASSETS", "SHA256SUMS", "dutch-dut.tar.gz", "english-eng.tar.gz",
                      "french-fre.tar.gz", "index.json", "mimi-aaa.tar.gz"]
    sums = voices.sums_of((tmp_path / "out" / "SHA256SUMS").read_text())
    assert set(sums) == set(assets) - {"SHA256SUMS"}
    assert all(hashlib.sha256((tmp_path / "out" / a).read_bytes()).hexdigest() == s for a, s in sums.items())
    with tarfile.open(tmp_path / "out" / "french-fre.tar.gz") as tar:
        members = tar.getmembers()
    assert {m.name for m in members} >= {"french-fre/manifest.json", "french-fre/voices/colibri.kv.f16"}
    assert all(m.mtime == 0 and m.uid == 0 and m.uname == "" for m in members)
    assert "Kyutai" in (tmp_path / "out" / "CREDITS.txt").read_text()
    assert "not under the MIT license" in (tmp_path / "out" / "LICENSE-ASSETS").read_text()
    # The same build gives the same bytes: a second run finds the release already out under the same name.
    assert release.pack(tmp_path / "voices", tmp_path / "again") == name
    assert (tmp_path / "again" / "SHA256SUMS").read_text() == (tmp_path / "out" / "SHA256SUMS").read_text()
    assert index["mimi"]["folder"] == "mimi-aaa"


def release_entry(tag, published, draft=False):
    return {"tag_name": tag, "draft": draft, "prerelease": True, "published_at": published}


class GitHub:
    """GitHub's releases API and download store, with the releases `releases` (tag -> assets folder)."""

    def __init__(self, listing, releases, tamper=None):
        self.listing, self.releases, self.tamper, self.asked = listing, releases, tamper, []

    def handle(self, request):
        self.asked.append(f"{request.url.host}{request.url.path}" + (f"?page={request.url.params.get('page')}"
                                                                     if request.url.host == "api.github.com" else ""))
        if request.url.host == "api.github.com":
            page = int(request.url.params.get("page", "1"))
            return httpx.Response(200, json=self.listing[page - 1] if page <= len(self.listing) else [])
        prefix = "/Alexbob0/BipAgents/releases/download/"
        if request.url.host != "github.com" or not request.url.path.startswith(prefix):
            return httpx.Response(404)
        tag, _, asset = request.url.path[len(prefix):].partition("/")
        path = self.releases.get(tag, Path("/nonexistent")) / asset
        if not path.is_file():
            return httpx.Response(404)
        data = path.read_bytes()
        if self.tamper == asset:
            data = data[:-1] + bytes([data[-1] ^ 1])
        return httpx.Response(200, content=data)

    def transport(self):
        return httpx.MockTransport(self.handle)


@pytest.fixture
def published(tmp_path):
    build(tmp_path / "voices")
    name = release.pack(tmp_path / "voices", tmp_path / "assets")
    bridges = [release_entry(f"bridge-{i:012x}", f"2026-10-2{i % 10}T00:00:00Z") for i in range(100)]
    listing = [bridges, [release_entry("voices-3.3.0-old", "2026-10-30T00:00:00Z"),       # another Pocket: not this one
                         release_entry("voices-4.0.1-draft", "2026-10-31T00:00:00Z", draft=True),
                         release_entry(name, "2026-10-26T00:00:00Z"),
                         release_entry("voices-4.0.1-older", "2026-10-20T00:00:00Z"),
                         release_entry("v0.1.0", "2026-09-01T00:00:00Z")]]
    return name, GitHub(listing, {name: tmp_path / "assets"})


def test_an_own_install_fetches_its_languages_from_the_latest_release(tmp_path, published, monkeypatch):
    name, github = published
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    home = tmp_path / "home"
    said = []
    target = voices.install(["fr", "nl"], home, hermes.Runner(dry=False), said.append, transport=github.transport())
    assert target == voices.folder(home) and not said
    assert sorted(p.name for p in target.iterdir()) == ["dutch-dut", "french-fre", "index.json", "mimi-aaa"]
    assert (target / "french-fre" / "voices" / "colibri.kv.f16").read_bytes() == b"voice french"
    assert json.loads((target / "index.json").read_text())["languages"].keys() == {"french", "dutch"}
    assert voices.engine_of(target) == "4.0.1" and voices.languages_of(target) == ["fr", "nl"]
    # The newest release of this checkout's Pocket, looked for past a page of the bridge's releases; English and the
    # other assets never fetched.
    assert github.asked[:2] == ["api.github.com/repos/Alexbob0/BipAgents/releases?page=1",
                                "api.github.com/repos/Alexbob0/BipAgents/releases?page=2"]
    fetched = sorted(a.rsplit("/", 1)[1] for a in github.asked[2:])
    assert fetched == ["SHA256SUMS", "dutch-dut.tar.gz", "french-fre.tar.gz", "index.json", "mimi-aaa.tar.gz"]


def test_a_given_release_or_pocket_decides_which(published):
    name, github = published
    with httpx.Client(transport=github.transport()) as client:
        assert voices.latest_release(client, pinned="4.0.1") == name
        assert voices.latest_release(client, pinned="3.3.0") == "voices-3.3.0-old"
        assert voices.latest_release(client, pinned="5.0.0") is None


@pytest.mark.parametrize("tamper", ["french-fre.tar.gz", "index.json"])
def test_a_tampered_asset_is_refused_and_nothing_kept(tmp_path, published, monkeypatch, tamper):
    name, github = published
    github.tamper = tamper
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    said = []
    assert voices.install(["fr"], tmp_path / "home", hermes.Runner(dry=False), said.append,
                          transport=github.transport()) is None
    assert "checksum" in said[-1] and "Pocket's own models instead" in said[-1]
    target = voices.folder(tmp_path / "home")
    assert not (target / "french-fre").exists() and not list(target.glob("*.part")) \
        and not list(target.glob(".*.part")) and not (target / "index.json").exists()


def evil_release(tmp_path, member: tarfile.TarInfo, data: bytes = b"x"):
    """A release whose French archive carries `member` (its SHA256SUMS right: a release made by someone else)."""
    build(tmp_path / "voices", languages=("french",))
    out = tmp_path / "evil"
    release.pack(tmp_path / "voices", out)
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as tar:
        member.size = len(data) if member.isfile() else 0
        tar.addfile(member, io.BytesIO(data) if member.isfile() else None)
    (out / "french-fre.tar.gz").write_bytes(buffer.getvalue())
    lines = [line for line in (out / "SHA256SUMS").read_text().splitlines() if not line.endswith("french-fre.tar.gz")]
    lines.append(f"{hashlib.sha256(buffer.getvalue()).hexdigest()}  french-fre.tar.gz")
    (out / "SHA256SUMS").write_text("\n".join(lines) + "\n")
    return GitHub([[release_entry("voices-4.0.1-evil", "2026-10-26T00:00:00Z")]], {"voices-4.0.1-evil": out})


@pytest.mark.parametrize("member", [tarfile.TarInfo("french-fre/../../escaped"), tarfile.TarInfo("other/manifest.json"),
                                    tarfile.TarInfo("/etc/passwd")])
def test_an_archive_writing_outside_its_folder_is_refused(tmp_path, monkeypatch, member):
    github = evil_release(tmp_path, member)
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    said = []
    assert voices.install(["fr"], tmp_path / "home", hermes.Runner(dry=False), said.append,
                          transport=github.transport()) is None
    assert "outside its folder" in said[-1]
    assert not (tmp_path / "home" / ".local" / "escaped").exists() and not (tmp_path / "escaped").exists()


def test_a_link_in_an_archive_is_refused(tmp_path, monkeypatch):
    link = tarfile.TarInfo("french-fre/manifest.json")
    link.type, link.linkname = tarfile.SYMTYPE, "/etc/passwd"
    github = evil_release(tmp_path, link)
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    said = []
    assert voices.install(["fr"], tmp_path / "home", hermes.Runner(dry=False), said.append,
                          transport=github.transport()) is None
    assert "isn't a file" in said[-1]


def test_the_light_voices_follow_a_newer_release(tmp_path, published, monkeypatch):
    name, github = published
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    home = tmp_path / "home"
    runner = hermes.Runner(dry=False)
    assert voices.install(["fr"], home, runner, [].append, transport=github.transport())
    assert not voices.refresh(home, runner, [].append, transport=github.transport())          # nothing new
    build(tmp_path / "newer", bump="2")                                                         # new Bips' voices
    newer = release.pack(tmp_path / "newer", tmp_path / "newer-assets")
    github.listing[1].insert(0, release_entry(newer, "2026-11-02T00:00:00Z"))
    github.releases[newer] = tmp_path / "newer-assets"
    assert voices.refresh(home, runner, [].append, transport=github.transport())
    target = voices.folder(home)
    assert sorted(p.name for p in target.iterdir()) == ["french-fre2", "index.json", "mimi-aaa"]   # the old one gone
    down = httpx.MockTransport(lambda request: httpx.Response(403, json={"message": "rate limit"}))
    said = []
    assert not voices.refresh(home, runner, said.append, transport=down)
    assert "keep the ones they have" in said[-1] and (target / "french-fre2").is_dir()


# -- the migration: every install's Pocket off PyTorch ----------------------------------------------------------------

TORCH_UNIT = """[Service]
WorkingDirectory={repo}/pocket
ExecStart={repo}/pocket/.venv/bin/python {repo}/pocket/pocket_server.py --voices {voices} --languages fr,en,es,de --port 8098
"""

TORCH_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>io.github.bipagents.pocket</string>
  <key>ProgramArguments</key><array><string>{repo}/pocket/.venv/bin/python</string><string>{repo}/pocket/pocket_server.py</string><string>--voices</string><string>{voices}</string><string>--languages</string><string>fr,en</string><string>--port</string><string>8098</string></array>
  <key>WorkingDirectory</key><string>{repo}/pocket</string>
</dict>
</plist>
"""


def own_install(tmp_path, monkeypatch, answers, voices_dir=None, mac=False):
    """An install of one's own (no guichet) whose Pocket runs its own models with PyTorch."""
    repo = tmp_path / "BipAgents"
    (repo / "pocket" / ".venv" / "bin").mkdir(parents=True)
    (repo / "pocket" / ".venv" / "torch").write_text("5.5 GB")
    (repo / "voices").mkdir()
    if mac:
        unit = tmp_path / "Library" / "LaunchAgents" / "io.github.bipagents.pocket.plist"
        text = TORCH_PLIST
    else:
        unit = tmp_path / ".config" / "systemd" / "user" / "bipagents-pocket.service"
        text = TORCH_UNIT
    unit.parent.mkdir(parents=True)
    unit.write_text(text.format(repo=repo, voices=voices_dir or repo / "voices"))
    weights = tmp_path / ".cache/huggingface/hub/models--kyutai--pocket-tts-without-voice-cloning"
    weights.mkdir(parents=True)
    monkeypatch.setattr(migrations, "REPO", repo)
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    asked = []

    def install(languages, home, runner, say, **kw):
        asked.append(languages)
        if not answers:
            say("  the light voices can't be had (ValueError: no voices release for this Pocket yet): ...")
            return None
        return tmp_path / "packages"

    monkeypatch.setattr(voices, "install", install)
    monkeypatch.setattr(voices, "engine_at", lambda url, wait=0: answers.pop(0))
    trimmed = []
    monkeypatch.setattr(checkout, "lighten", lambda repo, runner, voice_states: None)
    monkeypatch.setattr(checkout, "narrow", lambda repo, runner, voice_states: trimmed.append(voice_states) or True)
    monkeypatch.setattr(migrations, "own_packages", lambda runner: ["torch"])

    def run(args, **kw):
        ran.append(" ".join(args))
        if args[:2] == ["uv", "venv"]:
            (repo / "pocket" / ".venv-new" / "bin").mkdir(parents=True)
        return ""

    runner, ran = hermes.Runner(dry=False), []
    runner.run = run
    return repo, unit, weights, runner, ran, asked, trimmed


def test_an_own_install_moves_off_pytorch_with_the_release(tmp_path, monkeypatch):
    repo, unit, weights, runner, ran, asked, trimmed = own_install(tmp_path, monkeypatch, ["4.0.1"])
    said = migrations.voices_everywhere(runner, tmp_path)
    assert "without PyTorch" in said and asked == [["fr", "en", "es", "de", "nl"]]   # Dutch too: it had all the others
    assert f"--onnx {tmp_path}/packages --keep none" in unit.read_text()
    assert not (repo / "pocket" / ".venv" / "torch").exists() and not weights.exists()
    assert any("requirements-onnx.txt" in line for line in ran) and "systemctl --user restart bipagents-pocket" in ran
    assert trimmed == [False]                                         # Pocket's voice states leave the checkout
    assert any(line.endswith("cache clean torch") for line in ran)
    assert "uv's cache emptied" in said
    assert "already" in migrations.voices_everywhere(runner, tmp_path)   # again: harmless
    assert migrations.MIGRATIONS[-1][0] == "2026-10-26-voices-everywhere"


def test_without_a_release_yet_pytorch_stays_and_the_next_update_tries_again(tmp_path, monkeypatch):
    repo, unit, weights, runner, ran, asked, trimmed = own_install(tmp_path, monkeypatch, [])
    with pytest.raises(migrations.NotYet, match="can't be had yet"):
        migrations.voices_everywhere(runner, tmp_path)
    assert ran == [] and trimmed == [] and (repo / "pocket" / ".venv" / "torch").exists() and weights.exists()


def test_light_voices_that_dont_answer_put_pytorch_back(tmp_path, monkeypatch):
    repo, unit, weights, runner, ran, asked, trimmed = own_install(tmp_path, monkeypatch, [None])
    before = unit.read_text()
    with pytest.raises(migrations.NotYet, match="didn't answer"):
        migrations.voices_everywhere(runner, tmp_path)
    assert unit.read_text() == before and (repo / "pocket" / ".venv" / "torch").exists() and weights.exists()
    assert trimmed == []


def test_voices_of_ones_own_keep_pytorch(tmp_path, monkeypatch):
    mine = tmp_path / "my-voices"
    repo, unit, weights, runner, ran, asked, trimmed = own_install(tmp_path, monkeypatch, [], voices_dir=mine)
    said = migrations.voices_everywhere(runner, tmp_path)
    assert f"voices of your own ({mine})" in said and asked == [] and ran == []
    assert (repo / "pocket" / ".venv" / "torch").exists() and trimmed == []


def test_a_mac_install_moves_too(tmp_path, monkeypatch):
    repo, plist, weights, runner, ran, asked, trimmed = own_install(tmp_path, monkeypatch, ["4.0.1"], mac=True)
    assert pocketunit.find(tmp_path) is None and pocketunit.find_any(tmp_path).languages == "fr,en"
    said = migrations.voices_everywhere(runner, tmp_path)
    assert "without PyTorch" in said and asked == [["fr", "en"]]
    text = plist.read_text()
    assert "<string>--onnx</string>" in text and "<key>Label</key><string>io.github.bipagents.pocket</string>" in text
    assert ran[-3:-1] == [f"launchctl unload {plist}", f"launchctl load {plist}"]
    assert not weights.exists() and pocketunit.find_any(tmp_path).light
