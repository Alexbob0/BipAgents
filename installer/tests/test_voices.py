"""The light voices (voices.py): the phones' packages downloaded and checked, or nothing when they can't be had."""
import hashlib
import json

import httpx
import pytest

from bipinstall import hermes, voices


def packages_server(tamper=False):
    """index.json, a French package and the decoder, as voices/onnx/build.py publishes them."""
    files = {"mimi-1/mimi.onnx": b"decoder", "french-1/flowlm.onnx": b"model", "french-1/voices/colibri.kv.f16": b"voice"}

    def manifest(folder):
        return {"files": {path.split("/", 1)[1]: {"sha256": hashlib.sha256(data).hexdigest(), "bytes": len(data)}
                          for path, data in files.items() if path.startswith(folder + "/")}}

    index = {"languages": {"french": {"folder": "french-1", "bytes": 10}, "english": {"folder": "english-1", "bytes": 10}},
             "mimi": {"folder": "mimi-1", "bytes": 7}}

    def handle(request):
        path = request.url.path.removeprefix("/v1/").removeprefix("voices/")   # an address of its own, or the guichet
        if path == "index.json":
            return httpx.Response(200, json=index)
        folder, _, name = path.partition("/")
        if name == "manifest.json":
            return httpx.Response(200, json=manifest(folder))
        data = files.get(path)
        if data is None:
            return httpx.Response(404)
        return httpx.Response(200, content=b"tampered" if tamper and name == "flowlm.onnx" else data)

    return httpx.MockTransport(handle)


def test_the_packages_are_downloaded_checked_and_listed(tmp_path):
    said = []
    target = voices.install(["fr", "xx"], tmp_path, hermes.Runner(dry=False), said.append,
                            base="https://voices.test/v1/", transport=packages_server())
    assert target == tmp_path / ".local/share/bipagents/voices"
    assert (target / "french-1/voices/colibri.kv.f16").read_bytes() == b"voice"
    assert (target / "mimi-1/manifest.json").is_file()
    index = json.loads((target / "index.json").read_text())
    assert list(index["languages"]) == ["french"] and index["mimi"]["folder"] == "mimi-1"   # only what was asked
    assert not said


def test_a_bad_file_or_no_server_falls_back_to_pockets_models(tmp_path):
    said = []
    assert voices.install(["fr"], tmp_path, hermes.Runner(dry=False), said.append, base="https://voices.test/v1/",
                          transport=packages_server(tamper=True)) is None
    assert "Pocket's own models instead" in said[-1]
    assert not (tmp_path / ".local/share/bipagents/voices/french-1").exists()   # a checksum failed: not kept
    down = httpx.MockTransport(lambda request: httpx.Response(503))
    assert voices.install(["fr"], tmp_path, hermes.Runner(dry=False), said.append, base="https://voices.test/v1/",
                          transport=down) is None


def test_a_place_of_the_offer_fetches_them_from_its_guichet_with_its_token(tmp_path):
    from bipinstall import backup
    assert voices.source(tmp_path) == (None, None)                           # someone's own computer: no address
    said = []
    assert voices.install(["fr"], tmp_path, hermes.Runner(dry=False), said.append) is None
    assert "guichet only" in said[-1]
    backup.configure("https://guichet.test/v1/backup/", "t" * 24, base=tmp_path)
    assert voices.source(tmp_path) == ("https://guichet.test/v1/voices/", "t" * 24)
    seen = []
    served = packages_server()

    def guichet(request):
        seen.append((request.url.host, request.headers.get("authorization")))
        if request.headers.get("authorization") != "Bearer " + "t" * 24:
            return httpx.Response(401)
        return served.handle_request(request)

    assert voices.install(["fr"], tmp_path, hermes.Runner(dry=False), said.append, transport=httpx.MockTransport(guichet))
    assert seen and all(host == "guichet.test" and auth == "Bearer " + "t" * 24 for host, auth in seen)


def test_an_update_keeps_the_light_voices_light(monkeypatch, tmp_path):
    from bipinstall import upgrade
    monkeypatch.setenv("HOME", str(tmp_path))
    units = tmp_path / ".config/systemd/user"
    units.mkdir(parents=True)
    unit = units / "bipagents-pocket.service"
    unit.write_text("ExecStart=/x/python /x/pocket/pocket_server.py --voices /x/voices --port 8098\n")
    assert not upgrade.pocket_light()
    unit.write_text("ExecStart=/x/python /x/pocket/pocket_server.py --onnx /x/voices --keep none --port 8098\n")
    assert upgrade.pocket_light()


# -- Pocket 4.0.1 (2026-10-25): Pocket and its voices move together, then the bridge stops the respellings ----------

def write_packages(target, engines, build="1"):
    target.mkdir(parents=True, exist_ok=True)
    languages = {}
    for name, engine in engines.items():
        (target / f"{name}-{build}").mkdir()
        (target / f"{name}-{build}" / "manifest.json").write_text(json.dumps({"engine": f"pocket-tts {engine}"}))
        languages[name] = {"folder": f"{name}-{build}", "bytes": 1}
    (target / "index.json").write_text(json.dumps({"languages": languages, "mimi": {"folder": "mimi-1", "bytes": 1}}))


def test_the_packages_say_their_pocket_version(tmp_path):
    assert voices.engine_of(tmp_path) is None
    write_packages(tmp_path, {"french": "4.0.1", "german": "3.3.0"})
    assert voices.engine_of(tmp_path) == "3.3.0"                 # the lowest: one language still on the old voices
    assert voices.languages_of(tmp_path) == ["fr", "de"]
    assert voices.major("4.0.1") == 4 and voices.major(None) is None and voices.major("x") is None


def test_the_installer_pins_pocket_4_and_knows_which_runs(tmp_path):
    from pathlib import Path
    repo = Path(voices.__file__).resolve().parents[2]
    assert voices.pinned_pocket(repo) == "4.0.1"
    assert voices.installed_pocket(tmp_path) is None              # no pocket/.venv there
    answers = iter([httpx.Response(200, json={"status": "ok", "engine": "4.0.1"}),
                    httpx.Response(200, json={"status": "ok", "languages": ["fr"]})])
    transport = httpx.MockTransport(lambda request: next(answers))
    assert voices.engine_at("http://127.0.0.1:8098", transport=transport) == "4.0.1"
    assert voices.engine_at("http://127.0.0.1:8098", transport=transport) is None   # a Pocket from before: doesn't say
    down = httpx.MockTransport(lambda request: (_ for _ in ()).throw(httpx.ConnectError("refused")))
    assert voices.engine_at("http://127.0.0.1:8098", transport=down) is None


def test_the_light_voices_follow_the_guichets_new_packages(tmp_path, monkeypatch):
    target = voices.folder(tmp_path)
    write_packages(target, {"french": "3.3.0"}, build="0")
    said = []
    runner = hermes.Runner(dry=False)
    assert not voices.refresh(tmp_path, runner, said.append, transport=packages_server())   # no guichet: as they are
    monkeypatch.setattr(voices, "VOICES_URL", "https://voices.test/v1/")
    assert voices.refresh(tmp_path, runner, said.append, transport=packages_server())       # the new ones
    assert json.loads((target / "index.json").read_text())["languages"]["french"]["folder"] == "french-1"
    assert (target / "french-1" / "flowlm.onnx").read_bytes() == b"model" and not (target / "french-0").exists()
    assert not voices.refresh(tmp_path, runner, said.append, transport=packages_server())   # nothing new


def test_light_voices_with_every_language_ask_for_dutch_too(tmp_path, monkeypatch):
    # 2026-10-26: an account with the four older languages asks the guichet for nl as well (taken when it has it).
    target = voices.folder(tmp_path)
    monkeypatch.setattr(voices, "VOICES_URL", "https://voices.test/v1/")
    asked = []
    monkeypatch.setattr(voices, "install", lambda languages, *a, **k: asked.append(languages))
    write_packages(target, {"french": "4.0.1", "english": "4.0.1", "spanish": "4.0.1", "german": "4.0.1"})
    voices.refresh(tmp_path, hermes.Runner(dry=False), [].append)
    write_packages(target, {"french": "4.0.1"}, build="2")
    voices.refresh(tmp_path, hermes.Runner(dry=False), [].append)
    assert asked == [["fr", "en", "es", "de", "nl"], ["fr"]]


def pocket_unit(tmp_path, light=False, repo=None):
    units = tmp_path / ".config/systemd/user"
    units.mkdir(parents=True, exist_ok=True)
    flags = "--onnx /x/voices" if light else "--voices /x/voices"
    base = repo or "/x"
    python = f"{base}/pocket/.venv/bin/python" if repo else "/x/python"
    (units / "bipagents-pocket.service").write_text(f"ExecStart={python} {base}/pocket/pocket_server.py {flags}\n")


def test_an_update_moves_pockets_own_models_to_the_pinned_version(monkeypatch, tmp_path):
    from bipinstall import upgrade
    from bipinstall import pocketunit
    monkeypatch.setenv("HOME", str(tmp_path))
    repo = tmp_path / "BipAgents"
    pocket_unit(tmp_path, repo=repo)
    (repo / "pocket" / ".venv").mkdir(parents=True)
    monkeypatch.setattr(pocketunit, "engine_of", lambda unit: None)
    monkeypatch.setattr(upgrade, "REPO", repo)
    monkeypatch.setattr(voices, "pinned_pocket", lambda repo: "4.0.1")
    installed = {"version": "3.3.0"}
    monkeypatch.setattr(voices, "installed_pocket", lambda repo: installed["version"])
    made = []
    venv = lambda folder, runner, requirements="requirements.txt": made.append(requirements)  # noqa: E731
    runner = hermes.Runner(dry=False)
    nothing = lambda *prefixes: False  # noqa: E731
    # An update that didn't finish installing Pocket 4 (nothing touched this time): installed now, Pocket restarts.
    assert upgrade.pocket_step(runner, nothing, venv) and made == ["requirements.txt"]
    installed["version"] = "4.0.1"
    made.clear()
    assert not upgrade.pocket_step(runner, nothing, venv) and made == []          # up to date: nothing
    assert upgrade.pocket_step(runner, lambda *p: "voices/" in p, venv) and made == []   # new voice states: a restart

    def failing(folder, runner, requirements="requirements.txt"):
        raise RuntimeError("uv pip install failed (1): no network")

    installed["version"] = "3.3.0"
    # The install fails: Pocket keeps running as it was (its voices loaded), the next update tries again.
    assert not upgrade.pocket_step(runner, lambda *p: True, failing)


def test_an_update_brings_the_light_voices_new_packages(monkeypatch, tmp_path):
    from bipinstall import upgrade
    monkeypatch.setenv("HOME", str(tmp_path))
    pocket_unit(tmp_path, light=True)
    monkeypatch.setattr(upgrade, "REPO", tmp_path / "BipAgents")
    fetched = iter([True, False])
    monkeypatch.setattr(voices, "refresh", lambda home, runner, say: next(fetched))
    made = []
    venv = lambda folder, runner, requirements="requirements.txt": made.append(requirements)  # noqa: E731
    assert upgrade.pocket_step(hermes.Runner(dry=False), lambda *p: False, venv)       # new packages: a restart
    assert not upgrade.pocket_step(hermes.Runner(dry=False), lambda *p: False, venv) and made == []


def bridge_config(tmp_path, pocket):
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text('bridge_key = "k"\nport = 8643\n\n[kyutai]\nenabled = false\n\n' + pocket + "\n[cron]\nwatch = true\n")
    return config


def test_the_bridge_stops_the_respellings_once_its_pocket_runs_4(monkeypatch, tmp_path):
    import tomllib
    from bipinstall import family, migrations
    config = bridge_config(tmp_path, '[pocket]\nurl = "http://127.0.0.1:8098"\ndefault_voice = "pocket:colibri"\n')
    runner = hermes.Runner(dry=False)
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    monkeypatch.setattr(family, "bridge_restart", lambda home=None: ["systemctl", "--user", "restart", "bipagents-bridge"])
    asked = []
    engine = {"now": "3.3.0"}
    monkeypatch.setattr(voices, "engine_at", lambda url, wait=0: asked.append((url, wait)) or engine["now"])
    # The Pocket (here or in another account) not moved yet: nothing changes, the next update looks again.
    with pytest.raises(migrations.NotYet, match="still Pocket 3.3.0"):
        migrations.pocket_engine(runner, tmp_path)
    assert asked == [("http://127.0.0.1:8098", migrations.POCKET_WAIT)] and ran == []
    engine["now"] = None
    with pytest.raises(migrations.NotYet, match="no version"):
        migrations.pocket_engine(runner, tmp_path)
    engine["now"] = "4.0.1"
    said = migrations.pocket_engine(runner, tmp_path)
    assert "Pocket 4.0.1" in said and ran == [["systemctl", "--user", "restart", "bipagents-bridge"]]
    parsed = tomllib.loads(config.read_text())
    assert parsed["pocket"] == {"engine": "4.0.1", "url": "http://127.0.0.1:8098", "default_voice": "pocket:colibri"}
    assert parsed["cron"] == {"watch": True}
    ran.clear()
    assert "already" in migrations.pocket_engine(runner, tmp_path) and ran == []   # again: harmless
    assert migrations.MIGRATIONS[-3][0] == migrations.POCKET_ENGINE


def test_an_old_engine_line_is_replaced():
    from bipinstall import migrations
    text = '[pocket]\nurl = "u"\nengine = "3.3.0"\n\n[cron]\nwatch = true\n'
    assert migrations.with_pocket_engine(text, "4.0.1") == '[pocket]\nengine = "4.0.1"\nurl = "u"\n\n[cron]\nwatch = true\n'
    assert migrations.with_pocket_engine('port = 1\n', "4.0.1") == 'port = 1\n\n[pocket]\nengine = "4.0.1"\n'


def test_a_fresh_install_tells_its_bridge_which_pocket_speaks(tmp_path):
    import tomllib
    from pathlib import Path
    from bipinstall.ports import Ports
    from bipinstall.stack import Install, bridge_toml
    inst = Install(repo=Path("/r"), home=tmp_path, ports=Ports(8640), tailnet_name=None, lan_address=None)
    assert "engine" not in tomllib.loads(bridge_toml(inst))["pocket"]   # unknown: the bridge's default, then the update
    inst.pocket_engine = "4.0.1"
    assert tomllib.loads(bridge_toml(inst))["pocket"]["engine"] == "4.0.1"
    from bipinstall import migrations
    assert migrations.POCKET_ENGINE in migrations.AFTER_INSTALL
