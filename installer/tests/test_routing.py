"""Provider routing at EUrouter (routing.py): the Hermes configs name the providers EUrouter tries first."""
import sys
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bipinstall import hermes, migrations, routing  # noqa: E402

EU = {"model": {"provider": "custom", "default": "glm-5.3-flash", "base_url": "https://api.eurouter.ai/api/v1"}}
ROUTE = {"order": ["entrim", "melious"], "allow_fallbacks": True}


def test_only_eurouter_is_routed():
    assert routing.pins("https://api.eurouter.ai/api/v1") and routing.pins("https://api.eurouter.example/v1")
    assert not routing.pins("https://openrouter.ai/api/v1") and not routing.pins("http://gpu:8888/v1")
    assert routing.routing([], "https://api.eurouter.ai/api/v1") is None
    assert routing.routing(routing.DEFAULT_ORDER, "https://api.eurouter.ai/api/v1") == ROUTE


def test_one_entry_for_the_router_and_the_thinking_entry_carries_it():
    data = yaml.safe_load(yaml.safe_dump(EU))
    assert routing.with_routing(data, routing.DEFAULT_ORDER)
    assert data["providers"] == {"bipagents-route": {"api": "https://api.eurouter.ai/api/v1",
                                                     "extra_body": {"provider": ROUTE}}}
    assert not routing.with_routing(data, routing.DEFAULT_ORDER)   # nothing more to do
    data["providers"]["bipagents-thinking"] = {"api": "https://api.eurouter.ai/api/v1",
                                               "extra_body": {"chat_template_kwargs": {"enable_thinking": False}}}
    assert routing.with_routing(data, routing.DEFAULT_ORDER)
    assert list(data["providers"]) == ["bipagents-thinking"]   # Hermes takes the first entry for an address
    assert data["providers"]["bipagents-thinking"]["extra_body"]["provider"] == ROUTE
    assert routing.with_routing(data, [])                       # turned off
    assert "provider" not in data["providers"]["bipagents-thinking"]["extra_body"]
    for other in ({"model": {"provider": "custom", "base_url": "http://gpu:8888/v1"}}, {"model": {"provider": "openrouter"}}, {}):
        assert not routing.with_routing(other, routing.DEFAULT_ORDER)


def test_the_migration_routes_every_config_with_the_bridge_s_order(tmp_path):
    hub = tmp_path / ".hermes"
    (hub / "profiles" / "vie").mkdir(parents=True)
    (hub / "config.yaml").write_text(yaml.safe_dump(EU))
    (hub / "profiles" / "vie" / "config.yaml").write_text(yaml.safe_dump(EU))
    local = tmp_path / ".hermes" / "profiles" / "gpu"
    local.mkdir()
    (local / "config.yaml").write_text(yaml.safe_dump({"model": {"provider": "custom", "base_url": "http://gpu:8888/v1"}}))
    bridge = tmp_path / ".config" / "bipagents" / "bridge.toml"
    bridge.parent.mkdir(parents=True)
    bridge.write_text('bridge_key = "k"\n\n[model]\nprovider_order = ["melious", "entrim"]\n')
    runner = hermes.Runner()
    said = migrations.provider_routing(runner, tmp_path)
    assert said.endswith("(2 config(s))") and "melious, entrim" in said
    vie = yaml.safe_load((hub / "profiles" / "vie" / "config.yaml").read_text())
    assert vie["providers"]["bipagents-route"]["extra_body"]["provider"]["order"] == ["melious", "entrim"]
    assert "providers" not in yaml.safe_load((local / "config.yaml").read_text())
    assert migrations.provider_routing(runner, tmp_path).startswith("no agent on EUrouter")   # again: nothing to do
    assert "2026-10-26-provider-routing" in [mid for mid, _ in migrations.MIGRATIONS]
    assert routing.order_from(None) == routing.DEFAULT_ORDER
