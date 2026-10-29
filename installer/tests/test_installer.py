import asyncio
import json
import os
import subprocess
import sys
from pathlib import Path

import httpx
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "installer"))

from bipinstall import hermes, models, souls, ui  # noqa: E402
from bipinstall.detect import Machine  # noqa: E402
from bipinstall.plan import make_plan  # noqa: E402
from bipinstall.ports import Ports, pick_block  # noqa: E402
from bipinstall.stack import AgentEntry, Install, bridge_service, bridge_toml, tailscale_commands  # noqa: E402
from bipinstall.voice_server import discover as DISCOVER  # noqa: E402  (the real one: the tests' fixture replaces it)


@pytest.fixture(autouse=True)
def no_voice_server_on_the_network(monkeypatch):
    """The install flows meet no voice server (none is looked for on the real network)."""
    from bipinstall import voice_server

    async def nothing(addresses, *args, **kwargs):
        return []
    monkeypatch.setattr(voice_server, "discover", nothing)


@pytest.fixture(autouse=True)
def no_sandbox_unless_a_test_says_so(monkeypatch):
    """The flows don't depend on podman being installed where the tests run (CI machines have it)."""
    from bipinstall import hermes as hermes_module
    monkeypatch.setattr(hermes_module, "sandbox_runtime", lambda: None)


@pytest.fixture(autouse=True)
def this_version_s_bridge_is_here(monkeypatch):
    """The updates' migrations find this checkout's bridge binary fetched (a test says otherwise when it means to)."""
    from bipinstall import rustbridge
    monkeypatch.setattr(rustbridge, "missed", lambda home, repo=None: False)


def mac(memory, gpu="apple"):
    return Machine(system="macos", arch="arm64", memory_gib=memory, cores=10, cpu="Apple M4", gpu=gpu,
                   tailnet_name="mini.tail0000.ts.net", lan_addresses=["192.168.1.20"])


def test_plan_fits_the_machine():
    small = make_plan(mac(16), model_here=False)
    assert small.profile == "léger" and small.browser == "on-demand"
    assert 3 <= small.people <= 9   # a family on a 16 GB Mac mini when the model runs elsewhere
    local = make_plan(mac(16), model_here=True)
    assert local.profile == "local" and local.people < small.people
    assert any("local model" in n for n in local.notes)
    big = make_plan(Machine("linux", "x86_64", 125, 32, "Ryzen", "nvidia"), model_here=False)
    assert big.profile == "costaud" and big.browser == "permanent" and big.people >= 10
    tiny = make_plan(mac(8), model_here=True)
    assert tiny.browser == "none"


def test_port_blocks():
    taken = {8642, 8643}
    first = pick_block(lambda p: p not in taken)
    assert first.base == 9200 and (first.hermes, first.bridge, first.lan) == (9202, 9203, 9204)
    assert pick_block(lambda p: True).base == 8640
    with pytest.raises(RuntimeError):
        pick_block(lambda p: p != 9212, wanted=9210)
    assert list(Ports(9200).tailnet) == [9202, 9203, 9205]
    from bipinstall.stack import tailscale_commands
    assert [c.split("=")[1].split()[0] for c in tailscale_commands("tailscale", Ports(9200))] == ["9202", "9203"]
    assert len(tailscale_commands("tailscale", Ports(9200), desk=True)) == 3


def test_discovery_finds_openai_compatible_servers():
    def network(request: httpx.Request) -> httpx.Response:
        if request.url.host == "192.168.1.30" and request.url.port == 8888:
            return httpx.Response(200, json={"data": [{"id": "glm-5.3-flash"}]})
        if request.url.host == "127.0.0.1" and request.url.port == 11434:
            return httpx.Response(200, json={"data": [{"id": "qwen3:8b"}, {"id": "llama3.2"}]})
        if request.url.host == "192.168.1.40":
            return httpx.Response(200, text="<html>router</html>")  # not a model server
        raise httpx.ConnectError("closed")

    found = asyncio.run(models.discover(["192.168.1.20"], transport=httpx.MockTransport(network)))
    assert models.summary(found) == {"http://192.168.1.30:8888/v1": ["glm-5.3-flash"],
                                     "http://127.0.0.1:11434/v1": ["qwen3:8b", "llama3.2"]}
    assert {s.kind for s in found} == {"OpenAI-compatible", "Ollama"}


def test_model_check_tells_vision():
    def server(request: httpx.Request) -> httpx.Response:
        body = json.loads(request.content)
        if isinstance(body["messages"][0]["content"], list):
            return httpx.Response(400, json={"error": "image input not supported"})
        return httpx.Response(200, json={"choices": [{"message": {"content": "OK"}}]})

    result = asyncio.run(models.check("http://spark:8888/v1", "glm", transport=httpx.MockTransport(server)))
    assert result.ok and result.vision is False
    down = asyncio.run(models.check("http://spark:8888/v1", "glm", transport=httpx.MockTransport(
        lambda r: (_ for _ in ()).throw(httpx.ConnectError("off")))))
    assert not down.ok and down.error == "ConnectError"


def test_a_key_never_starts_with_a_dash(monkeypatch):
    """`hermes config set API_SERVER_KEY <key>`: a key starting with « - » would be read as an option."""
    drawn = iter(["-xPdIb1o6", "-abc", "Zk3-_q"])
    monkeypatch.setattr(hermes.secrets, "token_urlsafe", lambda n: next(drawn))
    assert hermes.new_key() == "Zk3-_q"


def test_hermes_setup_commands(monkeypatch, tmp_path):
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    runner = hermes.Runner(dry=True)
    model = hermes.Model("custom", "glm", base_url="http://192.168.1.30:8888/v1", vision=True)
    hermes.setup_hub(runner, 8642, model)
    key = hermes.create_agent(runner, "finance", description="Budget", soul="Tu es Budget.", model=model)
    log = "\n".join(runner.log)
    assert "config set API_SERVER_PORT 8642" in log and "config set gateway.multiplex_profiles true" in log
    assert "config set approvals.mode manual" in log   # a risky command always asks the person
    assert "profile create finance --clone-from default --no-alias" in log   # the hub's sandbox, as the app's agents
    assert f"-p finance config set API_SERVER_KEY {key}" in log
    assert "-p finance config set API_SERVER_ENABLED" not in log   # only the hub listens
    assert "-p finance config set model.supports_vision true" in log
    assert "-p finance config set compression.threshold_tokens 100000" in log   # a local server: compress early
    cloud = hermes.Runner(dry=True)
    hermes.set_model(cloud, "finance", hermes.Model("openrouter", "x", base_url="https://openrouter.ai/api/v1"))
    assert "threshold_tokens" not in "\n".join(cloud.log)
    assert hermes.agent_url("https://mini.tail0000.ts.net:8642", "finance") == "https://mini.tail0000.ts.net:8642/p/finance"


def test_names_are_the_persons():
    assert souls.profile_name("Léa") == "lea"
    assert souls.profile_name("Mon coach sportif !") == "mon-coach-sportif"
    assert souls.profile_name("Léa", taken=["lea"]) == "lea-2"
    assert souls.profile_name("Default") == "default-agent"
    assert souls.profile_name("Test") == "test-agent"   # Hermes refuses its reserved names
    assert souls.profile_name("🙂") == "agent"
    from bipinstall.__main__ import templates
    assert all("{name}" in t["soul"] and "name" not in t for t in templates() if t["category"] != "custom")
    assert templates()[-1]["category"] == "custom"


def test_custom_agent(monkeypatch):
    from bipinstall import __main__ as cli
    answers = iter(["keeps an eye on my plants and tells me when to water them", "funny", "never chemicals", "4"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    agent = cli.custom_agent("Fern")
    assert agent["category"] == "home" and agent["description"].startswith("keeps an eye on my plants")
    assert agent["soul"].startswith("You are Fern") and "funny" in agent["soul"] and "never chemicals" in agent["soul"]
    assert "write in English." in agent["soul"]   # its language block, until the app sets another


def test_uninstall_undoes_the_install(tmp_path, monkeypatch):
    from bipinstall.uninstall import install_ports, uninstall
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    (tmp_path / ".config" / "bipagents").mkdir(parents=True)
    (tmp_path / ".config" / "bipagents" / "bridge.toml").write_text('bridge_key = "x"\nport = 9203\n')
    agents = tmp_path / "Library" / "LaunchAgents"
    agents.mkdir(parents=True)
    (agents / "io.github.bipagents.bridge.plist").write_text("<plist/>")
    (tmp_path / ".hermes").mkdir()
    assert install_ports(tmp_path / ".config" / "bipagents" / "bridge.toml") == [9202, 9203, 9205]
    runner = hermes.Runner(dry=True)
    uninstall(runner, everything=False, home=tmp_path, system="macos", tailscale="tailscale")
    log = "\n".join(runner.log)
    assert "tailscale serve --https=9202 off" in log and "launchctl bootout" in log
    assert f"remove {tmp_path}/.config/bipagents" in log and ".hermes" not in log   # the agents' data stays
    runner = hermes.Runner(dry=True)
    uninstall(runner, everything=True, home=tmp_path, system="macos", tailscale=None)
    assert f"remove {tmp_path}/.hermes" in "\n".join(runner.log)


def test_hermes_is_pinned_and_patched(tmp_path, monkeypatch):
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    monkeypatch.setenv("HOME", str(tmp_path))   # no Hermes installed there
    runner = hermes.Runner(dry=True)
    hermes.install(runner, browser=False)
    assert "--commit 517b5e10f" in runner.log[0] and "--skip-browser" in runner.log[0]
    hermes.apply_patches(runner, ROOT / "hermes" / "patches")
    applied = [line for line in runner.log if " apply " in line and "--check" not in line]
    assert [line.rsplit("/", 1)[-1].strip("'") for line in applied] == ["0001-api-server-clarify.patch",
                                                            "0004-image-store-replay-recent-images.patch",
                                                            "0005-vault-fill-reattach-supervisor.patch",
                                                            "0006-bot-mode-sync-peer-reply.patch",
                                                            "0007-browser-vault-only.patch",
                                                            "0008-timezone-routines-wall-clock.patch",
                                                                "0009-tool-search-keep-direct-compact-skills.patch",
                                                                "0010-empty-replies-changed-retries-leaked-tool-calls.patch",
                                                                "0011-titles-reasoning-floor-glm.patch",
                                                                "0012-bot-chat-one-message-agent.patch",
                                                                "0013-mcp-results-vault-scrub.patch",
                                                                "0014-cron-results-plain-utf8.patch",
                                                                "0015-time-anchor.patch",
                                                                "0016-message-agent-the-bridges-everywhere.patch",
                                                                "0017-vault-address-contact-fields.patch",
                                                                "0018-guest-turns-closed-tools-no-user-profile.patch"]


def test_bot_mode_marker(tmp_path):
    (tmp_path / "profile.yaml").write_text("description: Budget\n")
    hermes.mark_as_bot(tmp_path)
    hermes.mark_as_bot(tmp_path)  # idempotent
    import yaml
    assert yaml.safe_load((tmp_path / "profile.yaml").read_text()) == {"description": "Budget", "ui_meta": {"hermes-bots": {}}}


def bridge_check(tmp_path: Path, text: str) -> str:
    """The bridge's own reading of a config (`bipbridge check`, the Rust release build: $CARGO_TARGET_DIR or
    bridge-rs/target); skipped without one."""
    binary = Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "bridge-rs" / "target") / "release" / "bipbridge"
    if not binary.is_file():
        pytest.skip(f"no bridge build at {binary} (cargo build --release in bridge-rs/)")
    path = tmp_path / "check.toml"
    path.write_text(text)
    path.chmod(0o600)
    done = subprocess.run([str(binary), "--config", str(path), "check"], capture_output=True, text=True,
                          env={"HOME": str(tmp_path), "PATH": "/usr/bin:/bin"})
    assert done.returncode == 0, done.stderr
    return done.stdout


def test_bridge_config_is_valid_for_the_bridge(tmp_path):
    inst = Install(repo=ROOT, home=tmp_path, ports=Ports(9200), tailnet_name="mini.tail0000.ts.net",
                   lan_address="192.168.1.20", model=hermes.Model("custom", "glm", base_url="http://192.168.1.30:8888/v1"),
                   relay_url="https://admin.tail0000.ts.net:8643", relay_key="r" * 40,
                   agents=[AgentEntry("finance", "Budget", "k" * 43)])
    shown = bridge_check(tmp_path, bridge_toml(inst))
    assert "agent finance: hermes=http://127.0.0.1:9202/p/finance " in shown
    assert "listen      127.0.0.1:9203" in shown and "lan         off" not in shown
    assert "pocket      http://127.0.0.1:8098" in shown
    import tomllib
    config = tomllib.loads(bridge_toml(inst))
    assert config["push"]["relay_url"] == "https://admin.tail0000.ts.net:8643"
    assert config["model"]["health_url"] == "http://192.168.1.30:8888/v1/models"
    assert config["hermes"]["hub_url"] == "http://127.0.0.1:9202"
    assert config["hermes"]["public_hub_url"] == "https://mini.tail0000.ts.net:9202"
    assert not config["hermes"].get("isolated")             # no sandbox: the bridge keeps no credentials
    assert not config.get("browser", {}).get("enabled")
    inst.isolated = inst.browser = True
    config = tomllib.loads(bridge_check(tmp_path, bridge_toml(inst)) and bridge_toml(inst))
    assert config["hermes"]["isolated"] is True and config["browser"]["enabled"] is True
    assert tailscale_commands("tailscale", inst.ports)[0] == "tailscale serve --bg --https=9202 http://127.0.0.1:9202"
    label, path, content = bridge_service(inst, "macos")
    assert path.name == "io.github.bipagents.bridge.plist" and str(inst.bridge_config) in content
    label, path, content = bridge_service(inst, "linux")
    assert label == "bipagents-bridge" and "BRIDGE_CONFIG=" in content


def test_whole_dry_run(monkeypatch, tmp_path, capsys):
    """The installer from the first question to the end, without network or changes."""
    from bipinstall import __main__ as cli
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(cli, "detect", lambda: mac(16))
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli.models, "discover", lambda addresses: _async([models.Server("http://192.168.1.30:8888/v1", "vLLM", ["glm"])]))
    monkeypatch.setattr(cli.models, "check", lambda *a, **k: _async(models.ModelCheck(ok=True, seconds=0.2, vision=True)))
    monkeypatch.setattr(cli, "pocket_running", lambda: True)
    monkeypatch.setattr(cli, "voice_engine_answers", lambda url: False)
    # The machine is a Mac whatever runs the test (CI: Linux), and its binaries come from the release.
    from bipinstall import rustbridge
    monkeypatch.setattr(rustbridge, "asset", lambda system=None, machine=None: "bipbridge-macos-universal.tar.gz")
    monkeypatch.delenv(rustbridge.BUILD, raising=False)
    answers = iter(["1", "1", "", "1,9", "Léa", "Fougère", "suit mes plantes", "", "", "4", "y"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "profile create lea" in out and "profile create fougere" in out
    assert "[agents.fougere]" in out and 'display_name = "Fougère"' in out
    assert "API_SERVER_KEY <key>" in out
    assert "[kyutai]\nenabled = false" in out
    # The bridge: this version's Rust binaries, nothing of Python's (no venv, no `-m bipbridge`).
    assert "fetch the bridge " in out and "bipbridge-macos-universal.tar.gz" in out
    assert "bridge/.venv" not in out and "-m bipbridge" not in out and "bridge/requirements" not in out


def test_the_agents_get_their_language_from_the_app(monkeypatch, tmp_path, capsys):
    """No language asked: the agents are created in English, answer in the language they are written to, and their
    language block (the one they write first in) is set later by the app."""
    from bipinstall import __main__ as cli
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(cli, "detect", lambda: mac(16))
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli.models, "discover", lambda addresses: _async([models.Server("http://192.168.1.30:8888/v1", "vLLM", ["glm"])]))
    monkeypatch.setattr(cli.models, "check", lambda *a, **k: _async(models.ModelCheck(ok=True, seconds=0.2, vision=True)))
    monkeypatch.setattr(cli, "pocket_running", lambda: True)
    monkeypatch.setattr(cli, "voice_engine_answers", lambda url: False)
    souls_written = {}
    create_agent = cli.hermes.create_agent

    def remembering(runner, name, **kw):   # the dry run writes no SOUL.md: keep what would be written
        souls_written[name] = kw["soul"]
        return create_agent(runner, name, **kw)

    monkeypatch.setattr(cli.hermes, "create_agent", remembering)
    answers = iter(["1", "1", "", "1,9", "Lea", "Fern", "watches my plants", "", "", "4", "y"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "Hello" in out and "Where does the model" in out and "Hermes, the agents' brain" in out
    assert "The Bips' voices" in out and "Dry run: here is what would have been done." in out
    assert "language" not in out.lower().split("the bips' voices")[0]   # nothing asked about it
    assert "API_SERVER_KEY <key>" in out and "Où tourne" not in out
    assert souls_written["lea"].startswith("You are Lea, your user's everyday agent.")
    assert souls_written["fern"].startswith("You are Fern") and "watches my plants" in souls_written["fern"]
    assert "answer in the language your person writes in" in souls_written["fern"] and "Tu " not in souls_written["fern"]


def test_a_kyutai_server_is_a_voice_model_like_any_other(monkeypatch, tmp_path, capsys):
    """Kyutai is no engine of its own: one already running (an administrator's, on any GPU) is the person's voice model,
    given here by its old flag."""
    from bipinstall import __main__ as cli
    from bipinstall import voice_server
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setattr(cli, "detect", lambda: mac(16))
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli.models, "discover", lambda addresses: _async([models.Server("http://192.168.1.30:8888/v1", "vLLM", ["glm"])]))
    monkeypatch.setattr(cli.models, "check", lambda *a, **k: _async(models.ModelCheck(ok=True, seconds=0.2, vision=True)))
    monkeypatch.setattr(cli, "pocket_running", lambda: True)
    monkeypatch.setattr(voice_server, "check", lambda url: voice_server.VoiceServer(url.rstrip("/"), ["5476"], "kyutai-tts",
                                                                                    kind="Kyutai"))
    answers = iter(["1", "1", "", "1,9", "Léa", "Fougère", "suit mes plantes", "", "", "4", "y"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run", "--kyutai-url", "http://127.0.0.1:8097", "--voice-uses", "reading"]) == 0
    out = capsys.readouterr().out
    assert "Your voice model: Kyutai — http://127.0.0.1:8097" in out
    assert '[kyutai]\nenabled = false' in out
    assert '[voice_server]\nurl = "http://127.0.0.1:8097"\nmodel = "kyutai-tts"\nlive = false\nreplies = false\nreading = true' in out


def test_linux_with_podman_offers_the_visible_browser(monkeypatch, tmp_path, capsys):
    """A sandbox on Linux: the agents' visible browser is offered, its image built from the repository, each agent
    given its container by the bridge, and Hermes' own desktop left out (the browser brings its own)."""
    from bipinstall import __main__ as cli
    monkeypatch.setenv("HOME", str(tmp_path))
    linux = Machine("linux", "x86_64", 32, 8, "Ryzen", "none", tailnet_name="box.tail0000.ts.net",
                    lan_addresses=["192.168.1.21"])
    monkeypatch.setattr(cli, "detect", lambda: linux)
    monkeypatch.setattr(cli.hermes, "sandbox_runtime", lambda: "/usr/bin/podman")
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli.models, "discover", lambda addresses: _async([models.Server("http://192.168.1.30:8888/v1", "vLLM", ["glm"])]))
    monkeypatch.setattr(cli.models, "check", lambda *a, **k: _async(models.ModelCheck(ok=True, seconds=0.2, vision=True)))
    monkeypatch.setattr(cli, "pocket_running", lambda: True)
    answers = iter(["1", "1", "", "y", "1", "Léa", "y"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "visible browser" in out and "[6/6]" in out
    assert "/usr/bin/podman build --pull=newer --build-arg BROWSER_REFRESH=" in out and "/browser/Containerfile" in out
    assert " -t bipagents-browser -f " in out                          # today's Chrome, never a cached layer
    assert "bipagents-refresh-browser.timer" in out                     # and its security fixes each night
    assert "/bipbridge browser add --all --config" in out and "systemctl --user restart bipagents-bridge" in out
    assert "fetch the bridge " in out and "bridge/.venv" not in out and "bridge/requirements" not in out
    assert "[browser]\nenabled = true" in out and "isolated = true" in out
    assert "config set HERMES_DOCKER_BINARY /usr/bin/podman" in out   # not a root docker found first
    assert "registries.conf.d/50-bipagents.conf" in out                # its sandbox image by its full name
    assert "bot_desktop.auto_start" not in out


def test_uninstall_removes_the_browsers(tmp_path, monkeypatch):
    from bipinstall.uninstall import uninstall
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    (units / "bipagents-browser-lea.service").write_text("[Unit]\n")
    (units / "bipagents-refresh-browser.timer").write_text("[Timer]\n")
    monkeypatch.setattr("shutil.which", lambda name: "/usr/bin/podman" if name == "podman" else None)
    runner = hermes.Runner(dry=True)
    uninstall(runner, everything=False, home=tmp_path, system="linux", tailscale=None)
    log = "\n".join(runner.log)
    assert "systemctl --user disable --now bipagents-browser-lea" in log
    assert "systemctl --user disable --now bipagents-refresh-browser.timer" in log      # its nightly refresh too
    assert "/usr/bin/podman image rm -f bipagents-browser" in log


def _existing_hermes(home: Path, api: bool = False, sandboxed: bool = False):
    """A Hermes installed before BipAgents: a model, a named profile, its own settings."""
    (home / ".local" / "bin").mkdir(parents=True)
    (home / ".local" / "bin" / "hermes").write_text("#!/bin/sh\n")
    hermes_home = home / ".hermes"
    checkout = hermes_home / "hermes-agent" / "hermes_cli"
    checkout.mkdir(parents=True)
    (checkout / "config_defaults.py").write_text('"multiplex_profiles": True,\n')
    terminal = "terminal:\n  backend: docker\n" if sandboxed else ""
    (hermes_home / "config.yaml").write_text(f"model:\n  default: glm-5.3\n  provider: custom\n{terminal}")
    (hermes_home / ".env").write_text("TELEGRAM_BOT_TOKEN=tg\n" + ("API_SERVER_ENABLED=true\nAPI_SERVER_PORT=8642\n"
                                                                   "API_SERVER_KEY=main-key\n" if api else ""))
    cuisine = hermes_home / "profiles" / "cuisine"
    cuisine.mkdir(parents=True)
    (cuisine / "config.yaml").write_text(f"model:\n  default: glm-5.3\n{terminal}")
    (cuisine / "profile.yaml").write_text("description: Recettes et menus\n")
    return hermes_home


def test_an_existing_hermes_keeps_its_settings(monkeypatch, tmp_path, capsys):
    """Someone who already uses Hermes: no model question, nothing of theirs changed; the local API and the shown
    agents' keys are added, and the app reaches the main profile at the gateway's root."""
    from bipinstall import __main__ as cli
    from bipinstall import existing
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    _existing_hermes(tmp_path)
    found = existing.detect_existing(run=lambda args: "Hermes Agent v0.21.6\n" if "--version" in args else "")
    assert found and found.multiplex and not found.api_enabled and not found.protected and found.gateway == "stopped"
    assert [p.name for p in found.profiles] == ["default", "cuisine"] and found.model == "glm-5.3"
    monkeypatch.setattr(cli, "detect", lambda: mac(16))
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli.hermes, "sandbox_runtime", lambda: None)
    monkeypatch.setattr(existing, "_run", lambda args: "")
    monkeypatch.setattr(cli, "pocket_running", lambda: True)
    answers = iter(["", "Marc", "", "y"])   # every agent, the main one renamed « Marc », go
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "You already have Hermes" in out and "Where does the model" not in out
    assert "config set API_SERVER_ENABLED true" in out and "config set API_SERVER_PORT 8642" in out
    assert "-p cuisine config set API_SERVER_KEY <key>" in out
    assert "model." not in out and "terminal.backend" not in out and "tools disable" not in out
    assert "profile create" not in out and "no vault" in out
    assert '[agents.default]\ndisplay_name = "Marc"\nhermes_url = "http://127.0.0.1:8642"' in out
    assert 'hermes_url = "http://127.0.0.1:8642/p/cuisine"' in out and "isolated = true" not in out


def test_a_hermes_our_install_left_unfinished_is_taken_up_again(tmp_path, monkeypatch):
    from bipinstall import existing, hermes
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    _existing_hermes(tmp_path, api=True, sandboxed=False)
    (tmp_path / ".hermes" / hermes.UNFINISHED).touch()
    assert existing.detect_existing(run=lambda args: "") is None

def test_an_existing_hermes_with_its_api_on_keeps_its_port_and_key(tmp_path, monkeypatch):
    from bipinstall import existing
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    _existing_hermes(tmp_path, api=True, sandboxed=True)
    found = existing.detect_existing(run=lambda args: "")
    assert found.api_enabled and found.api_port == 8642 and found.protected
    changes = existing.plan_changes(found, ["default", "cuisine"], 9202)
    assert [(p, k) for p, k, _, _ in changes] == [("cuisine", "API_SERVER_KEY")]
    assert existing.key_of(found, changes, "default") == "main-key"


def test_uninstall_puts_an_existing_hermes_back(tmp_path, monkeypatch):
    from bipinstall import existing
    from bipinstall.uninstall import uninstall
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    _existing_hermes(tmp_path)
    config = tmp_path / ".config" / "bipagents"
    config.mkdir(parents=True)
    (config / "bridge.toml").write_text('bridge_key = "x"\nport = 9203\n')
    runner = hermes.Runner()
    monkeypatch.setattr(runner, "run", lambda args, **k: runner.log.append(" ".join(args)) or "")
    changes = [("default", "API_SERVER_ENABLED", "true", None), ("default", "API_SERVER_PORT", "9202", "8642")]
    existing.apply_changes(runner, changes, config / existing.RECORD, "service", ["/repo/hermes/patches/0004.patch"])
    runner = hermes.Runner(dry=True)
    uninstall(runner, everything=True, home=tmp_path, system="linux", tailscale=None)
    log = "\n".join(runner.log)
    assert "config unset API_SERVER_ENABLED" in log and "config set API_SERVER_PORT 8642" in log
    assert "gateway restart" in log and "gateway uninstall" not in log
    assert "apply --reverse /repo/hermes/patches/0004.patch" in log   # our patches come back off its code
    assert f"keep {tmp_path}/.hermes" in log and f"remove {tmp_path}/.hermes\n" not in log + "\n"


def test_a_rented_server_is_recognised(tmp_path):
    from bipinstall.detect import cloud_vendor
    (tmp_path / "sys_vendor").write_text("Hetzner\n")
    assert cloud_vendor(str(tmp_path)) == "Hetzner"
    (tmp_path / "sys_vendor").write_text("Microsoft Corporation\n")
    (tmp_path / "product_name").write_text("Surface Laptop 7\n")
    assert cloud_vendor(str(tmp_path)) is None                  # a Surface, not Azure
    (tmp_path / "product_name").write_text("Virtual Machine\n")
    assert cloud_vendor(str(tmp_path)) == "Azure"
    assert cloud_vendor(str(tmp_path / "missing")) is None      # a Mac, a home server


def test_on_a_rented_server_nothing_listens_on_a_local_network(monkeypatch, tmp_path, capsys):
    from bipinstall import __main__ as cli
    monkeypatch.setenv("HOME", str(tmp_path))
    vps = Machine("linux", "arm64", 8, 4, "Neoverse-N1", None, tailnet_name="vps.tail0000.ts.net",
                  lan_addresses=["10.0.0.2"], cloud="Hetzner")
    monkeypatch.setattr(cli, "detect", lambda: vps)
    monkeypatch.setattr(cli.hermes, "sandbox_runtime", lambda: None)
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli.models, "check", lambda *a, **k: _async(models.ModelCheck(ok=True, seconds=0.4, vision=False)))
    monkeypatch.setattr(cli, "pocket_running", lambda: True)
    # The model in the cloud is the default choice (Enter), OpenRouter, a key, the model; one agent; go.
    answers = iter(["", "1", "sk-or-test", "", "1", "Léa", "y"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "Server rented from Hetzner" in out and "another machine on my tailnet" in out
    assert "[lan]\nenabled = false" in out


def test_hermes_update_checks(tmp_path, monkeypatch):
    from bipinstall import update
    assert update.behind("⚕ Update available: 590 commits behind origin/main.") == 590
    assert update.behind("✓ Already up to date") == 0
    assert update.channel("→ Update channel: main\n") == "main"
    agents = [{"id": "vie", "name": "Vie", "url": "http://h/p/vie", "key": "k"},
              {"id": "jardin", "name": "Jardin", "url": "http://h/p/jardin", "key": "k"},
              {"id": "old", "name": "Old", "url": "http://h/p/old", "key": "bad"}]

    def gateway(request):
        if request.headers["authorization"] == "Bearer bad":
            return httpx.Response(401)
        if "/vie/" in str(request.url):
            return httpx.Response(200, json={"run_submission": True, "run_clarify": True})
        return httpx.Response(200, json={"run_submission": True})

    client = httpx.Client(transport=httpx.MockTransport(gateway))
    problems = update.check_agents(agents, expect_clarify=True, client=client, attempts=1)
    assert problems["vie"] == "" and "0001" in problems["jardin"] and problems["old"] == "refuses its key"
    assert update.check_agents(agents[1:2], expect_clarify=False, client=client, attempts=1) == {"jardin": ""}
    monkeypatch.setattr(update.Path, "home", lambda: tmp_path)
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    (units / "bipagents-hermes.service").write_text("")
    (units / "bipagents-space-jardin.service").write_text("")
    monkeypatch.setattr(update.platform, "system", lambda: "Linux")
    assert update.gateway_units() == [("systemd", "bipagents-hermes"), ("systemd", "bipagents-space-jardin")]


async def _async_value(value):
    return value


def _async(value):
    return _async_value(value)


def test_family_member_gets_a_relay_key_and_a_block_of_ports(tmp_path):
    from bipinstall import family

    config = tmp_path / "bridge.toml"
    config.write_text('bridge_key = "k"\npublic_url = "https://box.ts.net:8643"\n\n[apns]\nteam_id = "T"\n')
    result = family.add("Léa", str(config), is_free=lambda port: True)
    assert result["member"] == "lea" and result["relay_url"] == "https://box.ts.net:8643"
    assert result["ports"].base == 9200                     # never 8640: the administrator's own block
    assert 'clients_file = "' in config.read_text() and "[relay]" in config.read_text()
    clients = tmp_path / "relay-clients"
    assert clients.read_text() == f"lea {result['key']}\n" and oct(clients.stat().st_mode & 0o777) == "0o600"
    second = family.add("Chloé", str(config), is_free=lambda port: True)
    assert second["ports"].base == 9210                     # reserved blocks are not handed out twice
    with pytest.raises(SystemExit):
        family.add("Léa", str(config), is_free=lambda port: True)
    # The bridge reads the relay keys back as `name key` lines (its check fails on a bad clients file).
    text = config.read_text().replace('bridge_key = "k"', 'bridge_key = "' + "x" * 40 + '"')
    bridge_check(tmp_path, text + '\n[pocket]\nurl = "http://p"\n' if "[pocket]" not in text else text)



def test_family_add_restarts_the_bridge_by_its_own_name(tmp_path, monkeypatch):
    from bipinstall import family
    monkeypatch.setattr(family.sys, "platform", "linux")
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    assert family.bridge_restart(tmp_path) == ["systemctl", "--user", "restart", "bipagents-bridge"]
    (units / "hermes-pocket.service").write_text("ExecStart=/x/python pocket_server.py\n")
    (units / "hermes-ios-bridge.service").write_text("ExecStart=/x/python -m bipbridge serve\n")
    assert family.bridge_restart(tmp_path) == ["systemctl", "--user", "restart", "hermes-ios-bridge"]


def test_family_add_tells_their_iphone_to_join_your_tailnet():
    from bipinstall import family
    text = "\n".join(family.iphone_steps("Léa", "https://box.tail1234.ts.net:8643"))
    assert "YOUR tailnet (tail1234.ts.net)" in text and "box" in text and "Invite users" in text

# -- a family member's account -----------------------------------------------------------------------------------

from bipinstall import account  # noqa: E402

SUBUID = "admin:100000:65536\nlea:165536:65536\n"


def test_next_subid_range_follows_every_given_range():
    assert account.next_subid_start(SUBUID, "admin:100000:65536\n") == 231072
    assert account.next_subid_start("", "") == account.SUBID_FIRST


def test_a_missing_account_is_created_first_and_alone():
    commands = account.prepare_commands("marc", account_exists=False, subuid=SUBUID, subgid=SUBUID, linger=False)
    assert commands == [["sudo", "useradd", "--create-home", "--shell", "/bin/bash", "marc"]]


def test_an_existing_account_gets_only_what_it_lacks():
    commands = account.prepare_commands("marc", account_exists=True, subuid=SUBUID, subgid=SUBUID + "marc:231072:65536\n",
                                        linger=False)
    assert commands == [["sudo", "usermod", "--add-subuids", "296608-362143", "marc"],
                        ["sudo", "loginctl", "enable-linger", "marc"]]
    assert account.prepare_commands("lea", account_exists=True, subuid=SUBUID, subgid=SUBUID, linger=True) == []


def test_linger_and_session_env(tmp_path):
    (tmp_path / "linger").mkdir()
    (tmp_path / "linger" / "lea").touch()
    assert account.linger_enabled("lea", tmp_path / "linger") and not account.linger_enabled("marc", tmp_path / "linger")
    assert account.session_env(1001, tmp_path) == {}
    (tmp_path / "1001").mkdir()
    (tmp_path / "1001" / "bus").touch()
    assert account.session_env(1001, tmp_path)["DBUS_SESSION_BUS_ADDRESS"] == f"unix:path={tmp_path}/1001/bus"


def test_install_runs_in_their_account_with_their_systemd():
    command = account.install_command("lea", 1001, "curl -fsSL https://x/install.sh | bash -s -- --ports 8650")
    assert command[:3] == ["sudo", "-iu", "lea"]
    assert "XDG_RUNTIME_DIR=/run/user/1001" in command
    assert command[-1].endswith("--ports 8650")


def test_an_account_without_its_own_systemd_stops_the_install():
    said = []
    assert not account.ensure_user_services(said.append, login="lea", check=lambda: False)
    assert any("enable-linger lea" in line for line in said)
    assert account.ensure_user_services(said.append, login="lea", check=lambda: True)


def test_another_accounts_runtime_folder_is_private(tmp_path):
    """The administrator cannot look inside a member's /run/user/<uid>: no crash, the system manager is asked."""
    folder = tmp_path / "1002"
    folder.mkdir()
    (folder / "bus").touch()
    folder.chmod(0o000)
    try:
        assert account.session_env(1002, tmp_path) == {}
    finally:
        folder.chmod(0o700)
    asked = []
    assert account.user_manager_running(1002, run=lambda c: asked.append(c) or 0)
    assert asked == [["systemctl", "is-active", "--quiet", "user@1002.service"]]


def test_podman_is_told_where_the_sandbox_image_comes_from(tmp_path):
    """Hermes asks for « nousresearch/hermes-sandbox:desktop »: without an alias, podman in a service stops at the short
    name (exit 125) and the agents have no sandbox."""
    import tomllib
    path = hermes.podman_short_names(hermes.Runner(), home=tmp_path)
    aliases = tomllib.loads(path.read_text())["aliases"]
    assert aliases["nousresearch/hermes-sandbox"] == "docker.io/nousresearch/hermes-sandbox"




# -- The person's own voice model ---------------------------------------------------------------------------------

def test_a_voice_server_is_found_with_its_voices_and_the_model_it_expects():
    from bipinstall import voice_server

    def server(request: httpx.Request) -> httpx.Response:
        if request.url.host == "192.168.1.20" and request.url.port == 8099:
            if request.url.path == "/v1/audio/voices":
                return httpx.Response(200, json={"voices": ["fr-mascotte-s17", "fr-naturelle-s15"]})
            if request.url.path == "/v1/models":
                return httpx.Response(200, json={"data": [{"id": "drama-3"}]})
        if request.url.host == "192.168.1.30" and request.url.port == 8097 and request.url.path == "/v1/voices":
            return httpx.Response(200, json={"default": "5476", "aliases": {"5476": "a.wav"}})
        raise httpx.ConnectError("refused", request=request)

    found = asyncio.run(DISCOVER(["192.168.1.5"], transport=httpx.MockTransport(server)))
    by_url = {s.url: s for s in found}
    drama = by_url["http://192.168.1.20:8099"]
    assert drama.voices == ["fr-mascotte-s17", "fr-naturelle-s15"] and drama.model == "drama-3"
    assert drama.stream_rate == 44100 and drama.kind == "Fish Audio Drama 3"
    kyutai = by_url["http://192.168.1.30:8097"]
    assert kyutai.voices == ["5476"] and kyutai.kind == "Kyutai" and kyutai.stream_rate is None
    assert len(found) == 2


def test_the_bridge_gets_its_voice_server_and_what_it_speaks_for(tmp_path):
    from bipinstall import voice_server
    server = voice_server.VoiceServer("http://192.168.1.20:8099", ["a"], "drama-3", 44100)
    lines = voice_server.toml_lines(server, voice_server.parse_uses("live,reading"))
    assert lines == ['[voice_server]', 'url = "http://192.168.1.20:8099"', 'model = "drama-3"', 'stream_rate = 44100',
                     'live = true', 'replies = false', 'reading = true']
    with pytest.raises(ValueError):
        voice_server.parse_uses("live,podcasts")
    config = tmp_path / "bridge.toml"
    config.write_text('bridge_key = "k"\n\n[drama]\nurl = "http://127.0.0.1:8099"\n\n[cron]\nwatch = true\n')
    assert voice_server.connect(config, lines, hermes.Runner())
    text = config.read_text()
    assert "[drama]" not in text and "[cron]\nwatch = true" in text and text.endswith("reading = true\n")
    voice_server.connect(config, lines, hermes.Runner())
    assert config.read_text().count("[voice_server]") == 1     # replaced, not added twice
    import tomllib
    assert tomllib.loads(config.read_text())["voice_server"]["replies"] is False


def test_picking_a_voice_server_or_none():
    from bipinstall import voice_server
    found = [voice_server.VoiceServer("http://a:8880", ["af_bella"], kind="Kokoro")]
    assert voice_server.choose(found, lambda q, d="": "", lambda q, o, d=1: 1, print) is found[0]
    assert voice_server.choose(found, lambda q, d="": "", lambda q, o, d=1: 3, print) is None
    defaults = []
    voice_server.choose([], lambda q, d="": "", lambda q, o, d=1: defaults.append(d) or d, print)
    assert defaults == [2]                                      # nothing found: « None » by default


def test_the_voice_server_lines_go_into_the_bridge_config(tmp_path):
    import tomllib
    from bipinstall import voice_server
    inst = Install(repo=tmp_path, home=tmp_path, ports=Ports(8640), tailnet_name=None, lan_address=None,
                   voice_server=voice_server.toml_lines(voice_server.VoiceServer("http://gpu:8880", ["af_bella"]),
                                                        {"live": False}))
    config = tomllib.loads(bridge_toml(inst))
    assert config["voice_server"] == {"url": "http://gpu:8880", "model": "tts-1", "live": False, "replies": True,
                                      "reading": True}


# -- Updates --------------------------------------------------------------------------------------------------------

def test_an_update_applies_the_settings_a_fresh_install_now_makes_once(tmp_path, monkeypatch):
    from bipinstall import migrations
    monkeypatch.setattr(migrations, "STATE", tmp_path / "state" / "migrations.json")
    hub = tmp_path / ".hermes"
    (hub / "profiles" / "vie").mkdir(parents=True)
    (hub / "config.yaml").write_text("model:\n  base_url: http://192.168.1.30:8888/v1\n")
    (hub / "profiles" / "vie" / "config.yaml").write_text("model:\n  base_url: https://openrouter.ai/api/v1\n")
    space = tmp_path / ".local" / "share" / "bipagents" / "spaces" / "wellness"
    (space / "profiles" / "wellness").mkdir(parents=True)
    (space / "profiles" / "wellness" / "config.yaml").write_text("model:\n  base_url: 'http://gpu:8888/v1'\n")
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append((dict(runner.env).get("HERMES_HOME"), list(args[1:]))) or ""
    monkeypatch.setattr(migrations, "REPO", tmp_path / "BipAgents")
    said = migrations.run(runner, home=tmp_path)
    assert said[0] == "the bridge already ran in Rust"
    assert "2 agent(s)" in said[1] and "approvals.mode manual" in said[2] and said[3] == "no agent browser here"
    assert (str(hub), ["config", "set", "compression.threshold_tokens", "100000"]) in ran
    assert (str(space), ["-p", "wellness", "config", "set", "compression.threshold_tokens", "100000"]) in ran
    assert (str(hub), ["-p", "vie", "config", "set", "compression.threshold_tokens", "100000"]) not in ran   # an API model
    assert (str(hub), ["-p", "vie", "config", "set", "approvals.mode", "manual"]) in ran
    ran.clear()
    assert migrations.run(runner, home=tmp_path) == [] and ran == []        # once
    monkeypatch.setattr(migrations, "STATE", tmp_path / "other.json")
    said = migrations.run(runner, home=tmp_path, own_hermes=False)
    assert ran == [] and all("not applied to your own Hermes" in line for line in said)


def test_an_update_makes_the_browsers_start_when_used(tmp_path, monkeypatch):
    from bipinstall import migrations, update
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    (units / "bipagents-browser-vie.service").write_text("[Service]\n")
    (tmp_path / ".config" / "bipagents").mkdir()
    (tmp_path / ".config" / "bipagents" / "bridge.toml").write_text("")
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes")])
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    said = migrations.browsers_on_demand(runner, tmp_path)
    assert ran[0][0].endswith("/bipbridge") and ran[0][1:] == ["browser", "add", "--all", "--config",
                                                               str(tmp_path / ".config" / "bipagents" / "bridge.toml")]
    assert ran[1] == ["systemctl", "--user", "restart", "bipagents-hermes"] and "(1)" in said


def test_an_update_isolates_the_agents_sandboxes_on_linux_with_podman(tmp_path, monkeypatch):
    """The bridge's own command does it and prints what each agent got; the gateways restart so their sandboxes are
    made again without the old mounts. Elsewhere nothing runs."""
    import platform
    import shutil
    from bipinstall import migrations, update
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text('bridge_key = "k"\n')
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes")])
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    report = "network: bipagents-sandbox (no route out)\nvie: isolated (volumes: bipagents-files-vie:/fichiers:ro)\n"
    runner.run = lambda args, **kw: ran.append(list(args)) or (report if "sandbox" in args else "")
    monkeypatch.setattr(platform, "system", lambda: "Darwin")
    assert "left as they are" in migrations.isolated_sandboxes(runner, tmp_path) and ran == []
    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/podman" if name == "podman" else None)
    said = migrations.isolated_sandboxes(runner, tmp_path)
    assert ran[0][0].endswith("/bipbridge") and ran[0][1:] == ["sandbox", "--config", str(config)]
    assert ran[1] == ["systemctl", "--user", "restart", "bipagents-hermes"]
    assert said.startswith("agents' sandboxes isolated") and "\n  vie: isolated" in said
    assert "2026-10-22-isolated-sandboxes" in [mid for mid, _ in migrations.MIGRATIONS]


def test_a_migration_needing_the_bridge_waits_for_its_release(tmp_path, monkeypatch):
    """2026-10-26 on aibox: the update ran before this version's bridge was out, and the migration ran a binary that
    wasn't there. It now waits (NotYet): the next `bipagents update` applies it, the previous bridge meanwhile."""
    import platform
    import shutil
    from bipinstall import migrations, rustbridge
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text('bridge_key = "k"\n')
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/podman" if name == "podman" else None)
    monkeypatch.setattr(rustbridge, "missed", lambda home, repo=None: True)
    with pytest.raises(migrations.NotYet, match="isn't out yet"):
        migrations.code_without_asking(runner, tmp_path)
    assert ran == []


def test_an_update_puts_the_shared_folders_in_podman_volumes(tmp_path, monkeypatch):
    """2026-10-23: the same command of the bridge makes the shared folders' volumes (the old folders copied in and
    kept as a backup) and mounts them; the gateways restart. Elsewhere nothing runs."""
    import platform
    import shutil
    from bipinstall import migrations, update
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text('bridge_key = "k"\n')
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes")])
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    report = ("network: bipagents-sandbox (no route out)\nwellness's shared folder: 3 file(s) copied into "
              "bipagents-share-wellness; the old folder kept as a backup at /d/shares/wellness.before-volume\n")
    runner.run = lambda args, **kw: ran.append(list(args)) or (report if "sandbox" in args else "")
    monkeypatch.setattr(platform, "system", lambda: "Darwin")
    assert migrations.share_volumes(runner, tmp_path).startswith("shared folders left as they are") and ran == []
    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/podman" if name == "podman" else None)
    said = migrations.share_volumes(runner, tmp_path)
    assert ran[0][1:] == ["sandbox", "--config", str(config)]
    assert ran[1] == ["systemctl", "--user", "restart", "bipagents-hermes"]
    assert said.startswith("shared folders in podman volumes, gateways restarted")
    assert "\n  wellness's shared folder: 3 file(s) copied" in said
    assert "2026-10-23-share-volumes" in [mid for mid, _ in migrations.MIGRATIONS]


def test_an_update_lets_agents_run_their_code_in_their_isolated_sandbox(tmp_path, monkeypatch):
    """2026-10-26: the bridge's `sandbox` command puts the code keys in each isolated agent's `command_allowlist`;
    Hermes reads it when its gateway starts, so the gateways restart. Elsewhere nothing runs."""
    import platform
    import shutil
    from bipinstall import migrations, update
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text('bridge_key = "k"\n')
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes")])
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ("vie: isolated\n" if "sandbox" in args else "")
    monkeypatch.setattr(platform, "system", lambda: "Darwin")
    assert migrations.code_without_asking(runner, tmp_path).startswith("agents keep asking") and ran == []
    monkeypatch.setattr(platform, "system", lambda: "Linux")
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/podman" if name == "podman" else None)
    said = migrations.code_without_asking(runner, tmp_path)
    assert ran[0][1:] == ["sandbox", "--config", str(config)]
    assert ran[1] == ["systemctl", "--user", "restart", "bipagents-hermes"]
    assert said.startswith("agents run the code they write in their isolated sandbox without asking")
    assert "2026-10-26-code-without-asking" in [mid for mid, _ in migrations.MIGRATIONS]


def test_the_approvals_command_lists_what_the_agents_asked(tmp_path, monkeypatch):
    """`bipagents approvals [agent] [--last N]`: the bridge's `approvals` with this account's config (read only)."""
    from bipinstall import __main__ as main, rustbridge
    home = tmp_path / "home"
    monkeypatch.setattr(main.Path, "home", staticmethod(lambda: home))
    assert main.approvals_main(["wellness"]) == 1   # no install in this account
    config = home / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text('bridge_key = "k"\n')
    monkeypatch.setattr(rustbridge, "ensure", lambda home: None)
    monkeypatch.setattr(rustbridge, "cli", lambda home, repo=None: ["/r/bridge-rs/bin/bipbridge"])
    seen = []

    class Done:
        returncode = 0

    monkeypatch.setattr(main.subprocess, "run", lambda args, **kw: seen.append(list(args)) or Done())
    assert main.approvals_main(["wellness", "--last", "5"]) == 0
    assert seen[0][1:] == ["approvals", "--config", str(config), "--agent", "wellness", "--last", "5"]


def test_each_install_of_a_machine_hands_out_browser_ports_of_its_own():
    from bipinstall.ports import browser_ports
    assert browser_ports(8640) == (9600, 400)       # the first install keeps its ports
    assert browser_ports(9200) == (20000, 40) and browser_ports(9210) == (20040, 40) and browser_ports(9230) == (20120, 40)
    inst = Install(repo=Path("/r"), home=Path("/home/bip2"), ports=Ports(9210), tailnet_name=None, lan_address=None,
                   browser=True)
    assert "[browser]\nenabled = true\nfirst_port = 20040\nports = 40" in bridge_toml(inst)


def test_forty_places_on_one_server_get_port_blocks_clear_of_the_first_install_s_browsers():
    """A shared server holds up to 40 places (seat-agent.py's MAX_SEATS): the 40th block used to be 9600, where place
    1's browsers listen. The blocks now skip that range, and every place's browsers get a range of their own."""
    from bipinstall.ports import BROWSERS_FIRST, BROWSERS_FIRST_COUNT, browser_ports
    taken = set()
    blocks = []
    for _ in range(42):
        ports = pick_block(lambda port: port not in taken)
        blocks.append(ports.base)
        taken.update(range(ports.base, ports.base + 10))
    assert blocks[:2] == [8640, 9200] and blocks[39:] == [9580, 9590, 10000]
    first_browsers = set(range(BROWSERS_FIRST, BROWSERS_FIRST + BROWSERS_FIRST_COUNT))
    assert not taken & first_browsers
    browsers = [set(range(first, first + count)) for first, count in map(browser_ports, blocks)]
    assert sum(len(b) for b in browsers) == len(set().union(*browsers))   # no two places share a browser port


def test_an_update_moves_the_browsers_into_the_install_s_own_ports(tmp_path, monkeypatch):
    """Two accounts on one machine gave their first browsers 9600 and 9601: only one could run. The update writes
    this install's range and sets the browsers up again in it (their profiles kept)."""
    from bipinstall import migrations, update
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    (units / "bipagents-browser-daily.service").write_text("[Service]\n")
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir()
    config.write_text('bridge_key = "k"\nport = 9213\n\n[browser]\nenabled = true\nidle_minutes = 10\n\n'
                      '[agents.daily]\nport = 1\n')
    monkeypatch.setattr(update, "gateway_units", lambda: [("systemd", "bipagents-hermes")])
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    said = migrations.browser_ports(runner, tmp_path)
    text = config.read_text()
    assert "[browser]\nfirst_port = 20040\nports = 40\nenabled = true\nidle_minutes = 10\n" in text
    assert "[agents.daily]\nport = 1" in text and text.count("first_port") == 1
    assert ran[0][0].endswith("/bipbridge") and ran[0][1:3] == ["browser", "add"] and "20040 to 20079" in said
    assert ran[1] == ["systemctl", "--user", "restart", "bipagents-hermes"]
    assert migrations.with_browser_range(text, 20040, 40) == text     # again: nothing changes


def test_the_bipagents_command_runs_install_sh_and_shows_its_help_alone(tmp_path):
    import subprocess
    from bipinstall import upgrade
    path = upgrade.install_command(hermes.Runner(), home=tmp_path)
    assert path == tmp_path / ".local" / "bin" / "bipagents" and path.stat().st_mode & 0o111
    shown = subprocess.run([str(path)], capture_output=True, text=True).stdout
    assert "bipagents update" in shown and "bipagents uninstall" in shown   # no new install by mistake
    assert f'exec bash "{upgrade.REPO}/install.sh" "$@"' in path.read_text()


def test_a_blocked_update_says_how_to_put_local_changes_aside(monkeypatch):
    from bipinstall import upgrade
    porcelain = " M bridge-rs/src/tts.rs\n?? voices/english/galet.safetensors\n"
    monkeypatch.setattr(upgrade, "git", lambda *args: __import__("subprocess").CompletedProcess(args, 0, porcelain, ""))
    lines = upgrade.blocked_pull()
    assert any("bridge-rs/src/tts.rs" in line for line in lines) and any("git stash" in line for line in lines)
    assert any("voices/english/galet.safetensors" in line for line in lines)
    assert upgrade.as_account("chi", "bash BipAgents/install.sh update --auto").startswith("sudo -u chi bash -c 'cd /home/chi")


def test_the_other_installs_of_the_machine_are_found_and_updated_after_one_sudo(monkeypatch, capsys):
    import pwd
    import subprocess
    from bipinstall import upgrade
    Entry = __import__("collections").namedtuple("Entry", "pw_name pw_uid pw_dir")
    monkeypatch.setattr(pwd, "getpwall", lambda: [Entry("root", 0, "/root"), Entry("admin", 1000, "/home/admin"),
                                                   Entry("chi", 1002, "/home/chi"), Entry("alex", 1003, "/home/alex"),
                                                   Entry("guest", 1004, "/home/guest"), Entry("aibox", 1005, "/home/aibox")])
    monkeypatch.setenv("USER", "admin")
    ran = []

    def fake_run(args, **kwargs):
        ran.append(args)
        ok = args[:2] == ["sudo", "-v"] or (args[:3] == ["sudo", "-n", "test"] and "guest" not in args[-1])
        ok = ok or (args[0] == "pgrep" and args[2] != "aibox")   # aibox's old install: stopped, left there
        return subprocess.CompletedProcess(args, 0 if ok or args[0] == "bash" else 1)

    monkeypatch.setattr(subprocess, "run", fake_run)
    upgrade.update_others(hermes.Runner())
    updates = [a[2] for a in ran if a[0] == "bash"]
    assert len(updates) == 2 and updates[0].startswith("sudo -n -u chi bash -c") and "update --auto" in updates[1]
    assert not any("aibox" in update for update in updates)
    assert "aibox: its BipAgents isn't running" in capsys.readouterr().out
    assert ran[0] == ["sudo", "-v"] and not any("admin/.config" in " ".join(a) for a in ran)   # not itself


def test_in_wsl_windows_tailscale_publishes_the_install(monkeypatch):
    from bipinstall import detect
    monkeypatch.setattr(detect.shutil, "which", lambda name: None)
    monkeypatch.setenv("WSL_DISTRO_NAME", "Ubuntu-24.04")
    monkeypatch.setattr(detect.os.path, "exists", lambda path: path == detect.TAILSCALE_WINDOWS)
    assert detect.tailscale_cli() == detect.TAILSCALE_WINDOWS
    monkeypatch.delenv("WSL_DISTRO_NAME")
    monkeypatch.setattr(detect.os.path, "exists", lambda path: path == detect.TAILSCALE_WINDOWS)
    assert detect.tailscale_cli() is None   # elsewhere, a Windows path means nothing


def test_new_installs_send_through_the_publishers_relay_when_it_is_set(tmp_path):
    inst = Install(repo=tmp_path, home=tmp_path, ports=Ports(base=9200), tailnet_name=None, lan_address=None,
                   push_service_url="https://push.example")
    toml = bridge_toml(inst)
    assert '[push]\nservice_url = "https://push.example"' in toml
    assert "[push]" not in bridge_toml(Install(repo=tmp_path, home=tmp_path, ports=Ports(base=9200), tailnet_name=None,
                                               lan_address=None))


def test_bip_languages_put_this_machine_first():
    from bipinstall.stack import bip_languages

    assert bip_languages("fr_FR.UTF-8") == "fr,en,es,de,nl"
    assert bip_languages("de_DE.UTF-8") == "de,fr,en,es,nl"
    assert bip_languages("nl_NL.UTF-8") == "nl,fr,en,es,de"
    assert bip_languages("ja_JP.UTF-8") == "fr,en,es,de,nl"
    assert bip_languages("") == "fr,en,es,de,nl"


def test_zram_for_small_rented_servers(tmp_path):
    from bipinstall.detect import zram_active
    from bipinstall.stack import zram_commands

    swaps = tmp_path / "swaps"
    swaps.write_text("Filename Type Size Used Priority\n/swapfile file 2097148 0 -2\n")
    assert not zram_active(str(swaps))
    swaps.write_text("Filename Type Size Used Priority\n/dev/zram0 partition 1990652 0 100\n")
    assert zram_active(str(swaps))
    assert not zram_active(str(tmp_path / "missing"))
    commands = zram_commands("apt")
    assert commands[0] == "apt-get install -y systemd-zram-generator" and zram_commands("dnf")[0].startswith("dnf ")
    assert "zram-size = ram / 2" in commands[1] and commands[1].endswith("> /etc/systemd/zram-generator.conf")


def _cloud_file(tmp_path, **values):
    import base64
    data = {"v": 1, "pair": base64.urlsafe_b64encode(bytes(32)).decode().rstrip("="), "language": "de", "profile": "solo"}
    data.update(values)
    path = tmp_path / "cloud.json"
    path.write_text(json.dumps(data))
    return path


def test_a_server_the_app_created_installs_without_a_question(monkeypatch, tmp_path, capsys):
    """`--cloud`: no question, no Tailscale, the bridge's door on the Internet (443, its address), the pairing secret
    and the progress for the app, the Bips in the person's language first, the model left for the app to send."""
    from bipinstall import __main__ as cli, cloud
    monkeypatch.setenv("HOME", str(tmp_path))
    droplet = Machine("linux", "x86_64", 4, 2, "DO-Premium-Intel", None, lan_addresses=["10.110.0.2"], cloud="DigitalOcean")
    monkeypatch.setattr(cli, "detect", lambda: droplet)
    monkeypatch.setattr(cli.hermes, "sandbox_runtime", lambda: "/usr/bin/podman")
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli, "pocket_running", lambda: False)
    monkeypatch.setattr(cloud, "public_address", lambda: "164.90.10.20")
    monkeypatch.setattr("builtins.input", lambda prompt="": pytest.fail(f"asked: {prompt}"))
    languages = []
    real_pocket = cli.pocket_service
    units = []
    monkeypatch.setattr(cli, "pocket_service", lambda inst, system, langs, packages=None:
                        languages.append((langs, packages)) or units.append(real_pocket(inst, system, langs, packages))
                        or units[-1])
    assert cli.main(["--dry-run", "--cloud", str(_cloud_file(tmp_path))]) == 0
    out = capsys.readouterr().out
    assert "164.90.10.20: the app pairs itself" in out and "[5/5]" in out and "Tailscale" not in out
    assert 'cloud = true\npublic_url = "https://164.90.10.20"' in out
    assert '[lan]\nenabled = true\nport = 443\naddress = "164.90.10.20"' in out
    assert f'[setup]\nfile = "{tmp_path}/.config/bipagents/setup.json"' in out
    assert "config set model.provider openrouter" in out and "OPENROUTER_API_KEY" not in out
    assert "profile create" not in out   # the agents come with the app's claim
    # The phones' 8-bit voices, without PyTorch, loaded only while they speak.
    assert languages == [("de,fr,en,es,nl", tmp_path / ".local/share/bipagents/voices")]
    assert "would download the voices of de, fr, en, es" in out and "requirements-onnx.txt" in out
    assert "--onnx" in units[0][2] and "--keep none" in units[0][2]
    assert "[browser]\nenabled = true" in out and "Scan this code" not in out
    # Installed: the download caches and the images' leftover layers go.
    assert "uv cache clean" in out and ".npm/_cacache" in out and "/usr/bin/podman image prune --force" in out
    assert ".hermes/cache/uv" in out


def test_a_mini_server_has_no_browser_and_one_language(monkeypatch, tmp_path, capsys):
    from bipinstall import __main__ as cli, cloud
    monkeypatch.setenv("HOME", str(tmp_path))
    droplet = Machine("linux", "x86_64", 2, 1, "DO-Regular", None, lan_addresses=[], cloud="DigitalOcean")
    monkeypatch.setattr(cli, "detect", lambda: droplet)
    monkeypatch.setattr(cli.hermes, "sandbox_runtime", lambda: "/usr/bin/podman")
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(8640))
    monkeypatch.setattr(cli, "pocket_running", lambda: False)
    monkeypatch.setattr(cloud, "public_address", lambda: "164.90.10.21")
    assert cli.main(["--dry-run", "--cloud", str(_cloud_file(tmp_path, profile="mini", language="fr"))]) == 0
    out = capsys.readouterr().out
    assert "[4/4]" in out and "[browser]" not in out and "fr,en" not in out


def test_the_second_seat_of_a_shared_server_has_its_door_on_444(monkeypatch, tmp_path, capsys):
    """Two people on one server, each in an account of their own: the second one's door is on 444, and the voices
    already running there are shared."""
    from bipinstall import __main__ as cli, cloud
    monkeypatch.setenv("HOME", str(tmp_path))
    server = Machine("linux", "x86_64", 4, 2, "VPS-1", None, lan_addresses=[], cloud="OVHcloud")
    monkeypatch.setattr(cli, "detect", lambda: server)
    monkeypatch.setattr(cli.hermes, "sandbox_runtime", lambda: "/usr/bin/podman")
    monkeypatch.setattr(cli, "pick_block", lambda wanted=None: Ports(9200))
    monkeypatch.setattr(cli, "pocket_running", lambda: True)
    monkeypatch.setattr(cloud, "public_address", lambda: "51.75.1.2")
    assert cli.main(["--dry-run", "--cloud", str(_cloud_file(tmp_path, port=444))]) == 0
    out = capsys.readouterr().out
    assert "51.75.1.2:444: the app pairs itself" in out
    assert 'public_url = "https://51.75.1.2:444"' in out and "[lan]\nenabled = true\nport = 444" in out
    assert "already set up on this machine: I'll share it" in out


def test_the_cloud_file_is_checked(tmp_path):
    from bipinstall import cloud
    with pytest.raises(cloud.CloudError):
        cloud.read(_cloud_file(tmp_path, pair="c2hvcnQ"))   # 5 bytes
    with pytest.raises(cloud.CloudError):
        cloud.read(_cloud_file(tmp_path, v=2))
    setup = cloud.read(_cloud_file(tmp_path, language="it", profile="huge"))
    assert (setup.language, setup.profile, setup.port) == ("en", "solo", 443)
    with pytest.raises(cloud.CloudError):
        cloud.read(_cloud_file(tmp_path, port=80))
    assert cloud.read(_cloud_file(tmp_path, port="444")).port == 444
    assert cloud.public_address(metadata=lambda: "10.0.0.5") in (None,) or cloud._global(cloud.public_address() or "")
    progress = cloud.Progress(tmp_path / "p.json")
    progress(2, 5, "Voices")
    assert json.loads((tmp_path / "p.json").read_text()) == {"step": 2, "total": 5, "label": "Voices", "done": False}


def test_the_bridge_s_service_stops_within_15_seconds(tmp_path, monkeypatch):
    """2026-10-27: systemd waited its 90 s for a bridge hanging on its way out (every update, every move): a fresh
    install's unit and plist say 15 s, and an update adds it to the units installed before (once)."""
    import sys
    from bipinstall import migrations, stack
    assert "2026-10-27-bridge-stop-timeout" == migrations.MIGRATIONS[-1][0]
    inst = Install(repo=tmp_path, home=tmp_path, ports=Ports(8640), tailnet_name=None, lan_address=None)
    _, _, unit = stack.bridge_service(inst, "linux")
    assert "\nTimeoutStopSec=15\n" in unit
    _, _, plist = stack.bridge_service(inst, "macos")
    assert "<key>ExitTimeOut</key><integer>15</integer>" in plist
    assert "TimeoutStopSec" not in stack.systemd_unit("x", ["/bin/x"], tmp_path, {})   # the other services as before
    # an install from before: its unit gets the line, systemd reads it again
    folder = tmp_path / ".config" / "systemd" / "user"
    folder.mkdir(parents=True)
    old = unit.replace("TimeoutStopSec=15\n", "")
    (folder / "bipagents-bridge.service").write_text(old)
    (folder / "bipagents-pocket.service").write_text(stack.systemd_unit("p", ["/bin/pocket"], tmp_path, {}))
    monkeypatch.setattr(sys, "platform", "linux")
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    assert "15 s" in migrations.bridge_stop_timeout(runner, tmp_path)
    text = (folder / "bipagents-bridge.service").read_text()
    assert text.count("TimeoutStopSec=15") == 1 and text.index("[Service]") < text.index("TimeoutStopSec")
    assert "TimeoutStopSec" not in (folder / "bipagents-pocket.service").read_text()
    assert ran == [["systemctl", "--user", "daemon-reload"]]
    assert "already" in migrations.bridge_stop_timeout(runner, tmp_path)
    # launchd: the plist gets ExitTimeOut
    monkeypatch.setattr(sys, "platform", "darwin")
    agents = tmp_path / "Library" / "LaunchAgents"
    agents.mkdir(parents=True)
    (agents / "io.github.bipagents.bridge.plist").write_text(plist.replace("\n  <key>ExitTimeOut</key><integer>15</integer>", ""))
    assert "15 s" in migrations.bridge_stop_timeout(runner, tmp_path)
    assert (agents / "io.github.bipagents.bridge.plist").read_text() == plist
    assert "already" in migrations.bridge_stop_timeout(runner, tmp_path)


def test_the_bridge_s_notifications_follow_the_new_app_identifier(tmp_path, monkeypatch):
    """2026-10-26: the app became com.kerfystudio.bipagents; a bridge pushing to Apple itself names it from the
    update on (and restarts); an own build's identifier stays."""
    from bipinstall import family, migrations
    config = tmp_path / ".config" / "bipagents" / "bridge.toml"
    config.parent.mkdir(parents=True)
    config.write_text('[apns]\nteam_id = "T"\nbundle_id = "io.github.bipagents"\n')
    monkeypatch.setattr(family, "bridge_restart", lambda home=None: ["systemctl", "--user", "restart", "bipagents-bridge"])
    runner = hermes.Runner()
    runner.dry = False
    ran = []
    runner.run = lambda args, **kw: ran.append(list(args)) or ""
    assert "com.kerfystudio.bipagents" in migrations.app_identifier(runner, tmp_path)
    assert 'bundle_id = "com.kerfystudio.bipagents"' in config.read_text() and ran[-1][-1] == "bipagents-bridge"
    assert "already" in migrations.app_identifier(runner, tmp_path)
    config.write_text('[apns]\nbundle_id = "com.example.mine"\n')
    assert "already" in migrations.app_identifier(runner, tmp_path) and "com.example.mine" in config.read_text()
