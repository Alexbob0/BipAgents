import logging
import os

import pytest

from bipbridge.config import ConfigError, load_config, parse_config

EXAMPLE = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "bridge.example.toml")


def test_example_config_parses_but_refuses_placeholders(tmp_path):
    with pytest.raises(ConfigError, match="placeholder"):
        load_config(EXAMPLE)


def test_example_config_with_real_values(tmp_path, monkeypatch):
    text = open(EXAMPLE, encoding="utf-8").read()
    text = text.replace('"CHANGE-ME-run-python-m-bipbridge-genkey"', '"k' + "x" * 40 + '"')
    for name in ("wellness", "vie"):
        text = text.replace(f'"CHANGE-ME-API_SERVER_KEY-of-{name}"', f'"key-{name}"')
    text = text.replace('"CHANGE-ME-tk_token-with-read-access"', '"tk_abc"')
    path = tmp_path / "bridge.toml"
    path.write_text(text, encoding="utf-8")
    os.chmod(path, 0o600)
    monkeypatch.setenv("BRIDGE_CONFIG", str(path))
    cfg = load_config()
    assert cfg.port == 8643 and cfg.host == "127.0.0.1"
    assert cfg.kyutai_url == "http://127.0.0.1:8097" and cfg.ntfy_url == "http://127.0.0.1:8645"
    assert cfg.apns.bundle_id == "io.github.bipagents" and cfg.apns.enabled is False
    assert set(cfg.agents) == {"wellness", "vie"}
    assert cfg.agents["vie"].hermes_url == "http://127.0.0.1:8644"
    assert cfg.agents["wellness"].upload_dir_container == "/home/hermes/.hermes/profiles/wellness/uploads"
    assert cfg.limits.upload_max_bytes == 50 * 1024 * 1024 and cfg.limits.upload_file_mode == 0o640


def test_loose_permissions_warn(tmp_path, caplog):
    path = tmp_path / "bridge.toml"
    path.write_text('bridge_key = "' + "z" * 40 + '"\n', encoding="utf-8")
    os.chmod(path, 0o644)
    with caplog.at_level(logging.WARNING):
        load_config(str(path))
    assert any("chmod 600" in r.getMessage() for r in caplog.records)


def test_secret_files_and_lowercase_agents(tmp_path):
    key_file = tmp_path / "hermes.key"
    key_file.write_text("from-file\n", encoding="utf-8")
    cfg = parse_config({"bridge_key": "b" * 40,
                        "agents": {"Vie": {"hermes_url": "http://h/", "hermes_key_file": str(key_file)}}})
    assert cfg.agent("VIE").hermes_key == "from-file" and cfg.agent("vie").hermes_url == "http://h"
    assert cfg.agent("vie").display_name == "Vie"


def test_invalid_configs():
    with pytest.raises(ConfigError):
        parse_config({})
    with pytest.raises(ConfigError):
        parse_config({"bridge_key": "b" * 40, "agents": {"a": {"hermes_key": "k"}}})
    with pytest.raises(ConfigError):
        parse_config({"bridge_key": "b" * 40, "agents": {"a": {"hermes_url": "http://h", "hermes_key": "k",
                                                               "upload_dir_host": "/tmp/x"}}})
    with pytest.raises(ConfigError):
        parse_config({"bridge_key": "b" * 40, "apns": {"environment": "dev"}})


def test_kyutai_can_be_turned_off_for_pocket_only():
    from bipbridge.config import ConfigError, parse_config
    base = {"bridge_key": "k" * 40}
    config = parse_config({**base, "kyutai": {"enabled": False}, "pocket": {"url": "http://127.0.0.1:8098"}})
    assert config.kyutai_url is None and config.default_voice == "pocket:colibri"
    config = parse_config({**base, "kyutai": {"url": ""}, "pocket": {"url": "http://p", "default_voice": "pocket:ours"}})
    assert config.kyutai_url is None and config.default_voice == "pocket:ours"
    with pytest.raises(ConfigError):
        parse_config({**base, "kyutai": {"enabled": False}})


def test_the_voice_server_is_optional_and_an_earlier_drama_section_is_one():
    base = {"bridge_key": "k" * 40, "pocket": {"url": "http://127.0.0.1:8098"}}
    assert parse_config(base).voice_server is None
    drama = parse_config({**base, "drama": {"url": "http://127.0.0.1:8099/"}}).voice_server
    assert drama.url == "http://127.0.0.1:8099" and drama.model == "drama-3" and drama.stream_rate == 44100
    assert drama.uses == {"live": True, "replies": True, "reading": True}
    assert parse_config({**base, "drama": {"url": "http://127.0.0.1:8099", "enabled": False}}).voice_server is None
    server = parse_config({**base, "voice_server": {"url": "http://gpu:8880", "model": "kokoro", "live": False,
                                                    "voices": ["af_bella"]}}).voice_server
    assert server.model == "kokoro" and server.stream_rate is None and server.voices == ["af_bella"]
    assert server.uses == {"live": False, "replies": True, "reading": True}


def test_push_hosts_can_be_replaced_for_tests(config_dict):
    """`[apns] base_url` (both environments) and `sandbox_url`, `[fcm] base_url` and `token_url`: Apple's and
    Google's by default, a local fake in the black-box suite (tests_blackbox/)."""
    import asyncio

    import httpx

    from bipbridge.apns import ApnsClient

    default = parse_config(config_dict)
    assert (default.apns.base_url, default.apns.sandbox_url) == ("", "")
    assert default.fcm_base_url == "https://fcm.googleapis.com" and default.fcm_token_url is None
    config_dict["apns"].update(base_url="http://127.0.0.1:9/prod/", sandbox_url="http://127.0.0.1:9/sandbox")
    config_dict["fcm"] = {"base_url": "http://127.0.0.1:9/", "token_url": "http://127.0.0.1:9/token"}
    config = parse_config(config_dict)
    assert config.apns.base_url == "http://127.0.0.1:9/prod" and config.fcm_base_url == "http://127.0.0.1:9"
    seen = []

    def handler(request):
        seen.append(str(request.url))
        return httpx.Response(200)

    async def go():
        async with httpx.AsyncClient(transport=httpx.MockTransport(handler)) as http:
            apns = ApnsClient(config.apns, http)
            await apns.send("ab" * 32, {"aps": {}}, environment="sandbox")
            await apns.send("ab" * 32, {"aps": {}}, environment="production")

    asyncio.run(go())
    assert seen == [f"http://127.0.0.1:9/sandbox/3/device/{'ab' * 32}", f"http://127.0.0.1:9/prod/3/device/{'ab' * 32}"]
