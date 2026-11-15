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


def test_deepseek_has_its_own_order_and_glm_keeps_the_places():
    """As the bridge (model_route.rs `order_for`): DeepSeek on GreenPT, the provider that caches it; pinning off stays
    off; the other models keep the place's order."""
    eu = "https://api.eurouter.ai/api/v1"
    assert routing.routing(routing.DEFAULT_ORDER, eu, "deepseek/deepseek-v4.1-flash")["order"] == ["greenpt"]
    assert routing.routing(routing.DEFAULT_ORDER, eu, "DeepSeek-V4-Flash-0731")["order"] == ["greenpt"]
    assert routing.routing(routing.DEFAULT_ORDER, eu, "glm-5.3-flash") == ROUTE
    assert routing.routing([], eu, "deepseek/deepseek-v4.1-flash") == PRIVACY
    data = {"model": {**EU["model"], "default": "deepseek/deepseek-v4.1-flash"}}
    assert routing.with_routing(data, routing.DEFAULT_ORDER)
    assert data["providers"]["bipagents-route"]["extra_body"]["provider"]["order"] == ["greenpt"]


# routing.py's copy of model_route.rs `order_for`, as written there (whitespace aside): a change of the bridge's rule
# is a change to make in routing.py `order_for` too, then here.
ORDER_FOR_RS = ("let model = model.to_lowercase(); match MODEL_ORDERS.iter().find(|(part, _)| model.contains(part)) { "
                "Some((_, own)) if !order.is_empty() => own.iter().map(|p| p.to_string()).collect(), "
                "_ => order.to_vec(), }")


def test_the_orders_are_the_bridge_s():
    """The installer writes the hub's routing and routes its model check, the bridge every agent's at each start: the
    same rule, read from model_route.rs (every entry of its constants, the matching rule, the privacy settings) and
    tried on the cases both suites share (bridge-rs/tests/golden/model_orders.json, model_route.rs
    `the_shared_cases_hold`). Review of installer-eurouter (15/11, N2): an order through a named constant, another
    matching rule or another privacy setting must not pass."""
    import json
    import re
    root = Path(__file__).resolve().parents[2]
    source = (root / "bridge-rs" / "src" / "model_route.rs").read_text(encoding="utf-8")
    size, names = re.search(r"pub const DEFAULT_ORDER: \[&str; (\d+)\] = \[([^\]]*)\];", source).groups()
    default = re.findall(r'"([^"]+)"', names)
    assert len(default) == int(size) and default == routing.DEFAULT_ORDER
    size, own = re.search(r"pub const MODEL_ORDERS: \[\(&str, &\[&str\]\); (\d+)\] = \[(.*?)\];\n", source,
                          re.S).groups()
    pairs = re.findall(r'\("([^"]+)", &\[([^\]]*)\]\)', own)
    assert len(pairs) == int(size), 'an entry of MODEL_ORDERS not written ("part", &["provider", …]): read it here too'
    assert {part: re.findall(r'"([^"]+)"', names) for part, names in pairs} == routing.MODEL_ORDERS
    rule = re.search(r"pub fn order_for\(order: &\[String\], model: &str\) -> Vec<String> \{\n(.*?)\n\}\n", source,
                     re.S)
    assert rule and " ".join(rule.group(1).split()) == ORDER_FOR_RS, "model_route.rs order_for changed: routing.py too"
    privacy = re.search(r"pub fn privacy\(\) -> Value \{\s*json!\((\{[^{}]*\})\)\s*\}", source)
    assert privacy and json.loads(privacy.group(1)) == routing.PRIVACY
    golden = json.loads((root / "bridge-rs" / "tests" / "golden" / "model_orders.json").read_text(encoding="utf-8"))
    assert golden["place_order"] == routing.DEFAULT_ORDER and golden["privacy"] == routing.PRIVACY
    eu = "https://api.eurouter.ai/api/v1"
    assert len(golden["cases"]) >= 5
    for case in golden["cases"]:
        assert routing.order_for(golden["place_order"], case["model"]) == case["order"], case["model"]
        assert routing.routing(golden["place_order"], eu, case["model"]) == \
            {"order": case["order"], "allow_fallbacks": True, **routing.PRIVACY}, case["model"]
    for model in golden["pinning_off"]:
        assert routing.routing([], eu, model) == routing.PRIVACY
