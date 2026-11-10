"""Provider routing at EUrouter (routing.py): the Hermes configs name the providers EUrouter tries first."""
import sys
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bipinstall import routing  # noqa: E402

EU = {"model": {"provider": "custom", "default": "glm-5.3-flash", "base_url": "https://api.eurouter.ai/api/v1"}}
PRIVACY = {"data_collection": "deny", "max_retention_days": 0}
ROUTE = {"order": ["entrim", "greenpt", "inceptron"], "allow_fallbacks": True, **PRIVACY}


def test_only_eurouter_is_routed():
    assert routing.pins("https://api.eurouter.ai/api/v1") and routing.pins("https://api.eurouter.example/v1")
    assert not routing.pins("https://openrouter.ai/api/v1") and not routing.pins("http://gpu:8888/v1")
    assert routing.routing([], "https://api.eurouter.ai/api/v1") == PRIVACY   # pinning off: no training, 0 retention
    assert routing.routing(routing.DEFAULT_ORDER, "http://gpu:8888/v1") is None
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
    assert routing.with_routing(data, [])                       # pinning turned off: the privacy settings stay
    assert data["providers"]["bipagents-thinking"]["extra_body"]["provider"] == PRIVACY
    for other in ({"model": {"provider": "custom", "base_url": "http://gpu:8888/v1"}}, {"model": {"provider": "openrouter"}}, {}):
        assert not routing.with_routing(other, routing.DEFAULT_ORDER)


