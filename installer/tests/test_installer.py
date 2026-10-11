import asyncio
import json
import sys
from pathlib import Path

import httpx
import pytest

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "installer"))
sys.path.insert(0, str(ROOT / "bridge"))

from bipinstall import hermes, models  # noqa: E402
from bipbridge import souls  # noqa: E402
from bipinstall.detect import Machine  # noqa: E402
from bipinstall.plan import make_plan  # noqa: E402
from bipinstall.ports import Ports, pick_block  # noqa: E402
from bipinstall.stack import AgentEntry, Install, bridge_service, bridge_toml, tailscale_commands  # noqa: E402


def mac(memory, gpu="apple"):
    return Machine(system="macos", arch="arm64", memory_gib=memory, cores=10, cpu="Apple M4", gpu=gpu,
                   tailnet_name="mini.tail0000.ts.net", lan_addresses=["192.168.1.20"])


def test_plan_fits_the_machine():
    small = make_plan(mac(16), model_here=False)
    assert small.profile == "léger" and small.browser == "on-demand" and not small.kyutai
    assert 3 <= small.people <= 9   # a family on a 16 GB Mac mini when the model runs elsewhere
    local = make_plan(mac(16), model_here=True)
    assert local.profile == "local" and local.people < small.people
    assert any("modèle local" in n for n in local.notes)
    big = make_plan(Machine("linux", "x86_64", 125, 32, "Ryzen", "nvidia"), model_here=False)
    assert big.profile == "costaud" and big.browser == "permanent" and big.kyutai and big.people >= 10
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


def test_hermes_setup_commands(monkeypatch, tmp_path):
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    runner = hermes.Runner(dry=True)
    model = hermes.Model("custom", "glm", base_url="http://192.168.1.30:8888/v1", vision=True)
    hermes.setup_hub(runner, 8642, model)
    key = hermes.create_agent(runner, "finance", description="Budget", soul="Tu es Budget.", model=model)
    log = "\n".join(runner.log)
    assert "config set API_SERVER_PORT 8642" in log and "config set gateway.multiplex_profiles true" in log
    assert "profile create finance --no-alias" in log
    assert f"-p finance config set API_SERVER_KEY {key}" in log
    assert "-p finance config set API_SERVER_ENABLED" not in log   # only the hub listens
    assert "-p finance config set model.supports_vision true" in log
    assert hermes.agent_url("https://mini.tail0000.ts.net:8642", "finance") == "https://mini.tail0000.ts.net:8642/p/finance"


def test_names_are_the_persons():
    assert souls.profile_name("Léa") == "lea"
    assert souls.profile_name("Mon coach sportif !") == "mon-coach-sportif"
    assert souls.profile_name("Léa", taken=["lea"]) == "lea-2"
    assert souls.profile_name("Default") == "agent-default"
    assert souls.profile_name("🙂") == "agent"
    from bipinstall.__main__ import TEMPLATES
    assert all("{name}" in t["soul"] and "name" not in t for t in TEMPLATES if t["category"] != "custom")
    assert TEMPLATES[-1]["category"] == "custom"


def test_custom_agent(monkeypatch):
    from bipinstall import __main__ as cli
    answers = iter(["suit mes plantes et me dit quand les arroser", "drôle", "jamais de produits chimiques", "", "4"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    agent = cli.custom_agent("Fougère")
    assert agent["category"] == "home" and agent["description"].startswith("suit mes plantes")
    assert agent["soul"].startswith("Tu es Fougère") and "drôle" in agent["soul"] and "en français" in agent["soul"]
    assert "jamais de produits chimiques" in agent["soul"]


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
                                                            "0005-vault-fill-reattach-supervisor.patch"]


def test_bot_mode_marker(tmp_path):
    (tmp_path / "profile.yaml").write_text("description: Budget\n")
    hermes.mark_as_bot(tmp_path)
    hermes.mark_as_bot(tmp_path)  # idempotent
    import yaml
    assert yaml.safe_load((tmp_path / "profile.yaml").read_text()) == {"description": "Budget", "ui_meta": {"hermes-bots": {}}}


def test_bridge_config_is_valid_for_the_bridge(tmp_path):
    from bipbridge.config import parse_config
    try:
        import tomllib
    except ImportError:  # pragma: no cover
        import tomli as tomllib

    inst = Install(repo=ROOT, home=tmp_path, ports=Ports(9200), tailnet_name="mini.tail0000.ts.net",
                   lan_address="192.168.1.20", model=hermes.Model("custom", "glm", base_url="http://192.168.1.30:8888/v1"),
                   relay_url="https://aibox.tail0000.ts.net:8643", relay_key="r" * 40,
                   agents=[AgentEntry("finance", "Budget", "k" * 43)])
    config = parse_config(tomllib.loads(bridge_toml(inst)))
    agent = config.agent("finance")
    assert agent.hermes_url == "http://127.0.0.1:9202/p/finance"
    assert agent.public_url == "https://mini.tail0000.ts.net:9202/p/finance"
    assert config.public_url == "https://mini.tail0000.ts.net:9203" and config.port == 9203
    assert config.lan.enabled and config.lan.port == 9204 and config.lan.address == "192.168.1.20"
    assert config.push_relay_url == "https://aibox.tail0000.ts.net:8643"
    assert config.model_health_url == "http://192.168.1.30:8888/v1/models"
    assert config.kyutai_url is None and config.pocket_url == "http://127.0.0.1:8098"
    assert config.hermes.enabled and config.hermes.hub_url == "http://127.0.0.1:9202"
    assert config.hermes.public_hub_url == "https://mini.tail0000.ts.net:9202"
    assert config.hermes.isolated is False             # no sandbox: the bridge keeps no credentials
    assert config.browser.enabled is False
    inst.isolated = inst.browser = True
    config = parse_config(tomllib.loads(bridge_toml(inst)))
    assert config.hermes.isolated is True and config.browser.enabled is True
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
    answers = iter(["1", "1", "", "1,9", "Léa", "Fougère", "suit mes plantes", "", "", "", "4", "o"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "profile create lea" in out and "profile create fougere" in out
    assert "[agents.fougere]" in out and 'display_name = "Fougère"' in out
    assert "<clé>" in out and "API_SERVER_KEY <clé>" in out


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
    answers = iter(["1", "1", "", "o", "1", "Léa", "o"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "navigateur visible" in out and "[6/6]" in out
    assert "/usr/bin/podman build -t bipagents-browser -f " in out and "/browser/Containerfile" in out
    assert "-m bipbridge browser add --all --config" in out and "systemctl --user restart bipagents-bridge" in out
    assert "[browser]\nenabled = true" in out and "isolated = true" in out
    assert "bot_desktop.auto_start" not in out


def test_uninstall_removes_the_browsers(tmp_path, monkeypatch):
    from bipinstall.uninstall import uninstall
    monkeypatch.setenv("HERMES_HOME", str(tmp_path / ".hermes"))
    units = tmp_path / ".config" / "systemd" / "user"
    units.mkdir(parents=True)
    (units / "bipagents-browser-lea.service").write_text("[Unit]\n")
    monkeypatch.setattr("shutil.which", lambda name: "/usr/bin/podman" if name == "podman" else None)
    runner = hermes.Runner(dry=True)
    uninstall(runner, everything=False, home=tmp_path, system="linux", tailscale=None)
    log = "\n".join(runner.log)
    assert "systemctl --user disable --now bipagents-browser-lea" in log
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
    answers = iter(["", "Marc", "", "o"])   # every agent, the main one renamed « Marc », go
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "Tu as déjà Hermes" in out and "Où tourne le modèle" not in out
    assert "config set API_SERVER_ENABLED true" in out and "config set API_SERVER_PORT 8642" in out
    assert "-p cuisine config set API_SERVER_KEY <clé>" in out
    assert "model." not in out and "terminal.backend" not in out and "tools disable" not in out
    assert "profile create" not in out and "pas de coffre" in out
    assert '[agents.default]\ndisplay_name = "Marc"\nhermes_url = "http://127.0.0.1:8642"' in out
    assert 'hermes_url = "http://127.0.0.1:8642/p/cuisine"' in out and "isolated = true" not in out


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
    answers = iter(["", "1", "sk-or-test", "", "1", "Léa", "o"])
    monkeypatch.setattr("builtins.input", lambda prompt="": next(answers))
    assert cli.main(["--dry-run"]) == 0
    out = capsys.readouterr().out
    assert "Serveur loué chez Hetzner" in out and "autre machine de mon tailnet" in out
    assert "[lan]\nenabled = false" in out


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
    # The bridge reads the relay keys back as `name key` lines.
    from bipbridge.config import parse_config
    try:
        import tomllib
    except ImportError:  # pragma: no cover
        import tomli as tomllib
    data = tomllib.loads(config.read_text())
    data.update({"bridge_key": "x" * 40, "pocket": {"url": "http://p"}})
    assert set(parse_config(data).relay.clients.values()) == {"lea", "chloe"}
