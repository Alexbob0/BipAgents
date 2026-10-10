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
    assert hermes.profile_name("Léa") == "lea"
    assert hermes.profile_name("Mon coach sportif !") == "mon-coach-sportif"
    assert hermes.profile_name("Léa", taken=["lea"]) == "lea-2"
    assert hermes.profile_name("Default") == "agent-default"
    assert hermes.profile_name("🙂") == "agent"
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
    assert tailscale_commands("tailscale", inst.ports)[0] == "tailscale serve --bg --https=9202 http://127.0.0.1:9202"
    label, path, content = bridge_service(inst, "macos")
    assert path.name == "io.github.bipagents.bridge.plist" and str(inst.bridge_config) in content
    label, path, content = bridge_service(inst, "linux")
    assert label == "bipagents-bridge" and "BRIDGE_CONFIG=" in content
