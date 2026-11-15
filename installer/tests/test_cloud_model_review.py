"""Review of installer-eurouter (15/11): EUrouter, now the default (Enter, Enter), is Hermes' `custom` provider, so the
first run writes `model.key_env: CUSTOM_MODEL_API_KEY` and the EUrouter key in `.env`. A run that stops after the
hub is set up (hermes.UNFINISHED) is taken up again on the same files; if the person then picks OpenRouter, OpenAI
or Anthropic, `set_model` leaves that pointer in place:

- Hermes (pinned 3b42157858) keeps `model.key_env` when `config set model.provider` changes the provider
  (hermes_cli/route_identity.py `drop_stale_model_route` pops `base_url` and `api_mode` only) and reads the key from
  it first for the provider the model names (hermes_cli/auth.py `_model_level_key_env`, no prefix check but
  OpenRouter's « sk-or- »): the EUrouter key goes to OpenAI or Anthropic, every turn answers 401 although the
  installer's test passed with the right key;
- the bridge's own calls (Carnet, mail scan, rooms, records: bridge-rs/src/mail_scan.rs `model_of`) read
  `model.key_env` for OpenRouter too: the EUrouter key goes to openrouter.ai.

The bridge already drops the pointer when it switches an agent to a provider of its own (model_choice.rs, `m.remove(
"key_env")`); the installer doesn't. Fake keys only, nothing run (`hermes config set` applied to the file here)."""
import sys
from pathlib import Path

import pytest
import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bipinstall import hermes  # noqa: E402

EUROUTER = "https://api.eurouter.ai/api/v1"
FAKE_EU_KEY = "sk-fake-eurouter-key"     # never a real one
FAKE_NEW_KEY = "sk-fake-new-provider"


class FakeHermes(hermes.Runner):
    """`hermes [-p name] config set key value` applied to the profile's config.yaml as Hermes 3b42157858 does for the
    model section: a provider change drops the old route (`base_url`, `api_mode`), nothing else."""

    def run(self, args, **kw):
        self.log.append(" ".join(args) if not isinstance(args, str) else args)
        args = list(args)
        if args[1:2] == ["-p"]:
            profile, rest = args[2], args[3:]
        else:
            profile, rest = hermes.HUB_PROFILE, args[1:]
        if rest[:2] == ["config", "set"]:
            key, value = rest[2], rest[3]
            path = hermes.profile_dir(profile) / "config.yaml"
            data = yaml.safe_load(path.read_text()) if path.exists() else {}
            data = data or {}
            parts = key.split(".")
            node = data
            for part in parts[:-1]:
                node = node.setdefault(part, {})
            if key == "model.provider" and node.get("provider") not in (None, value):
                for stale in ("base_url", "api_mode"):
                    node.pop(stale, None)
            node[parts[-1]] = value
            path.write_text(yaml.safe_dump(data, sort_keys=False))
        if rest[:2] == ["config", "unset"]:
            path = hermes.profile_dir(profile) / "config.yaml"
            data = yaml.safe_load(path.read_text()) or {}
            parts = rest[2].split(".")
            node = data
            for part in parts[:-1]:
                node = node.get(part, {})
            node.pop(parts[-1], None)
            path.write_text(yaml.safe_dump(data, sort_keys=False))
        return ""


def _env(path: Path) -> dict:
    return dict(line.split("=", 1) for line in path.read_text().splitlines() if "=" in line)


@pytest.mark.parametrize("provider, name, base_url", [
    ("openrouter", "anthropic/claude-sonnet-5.5", "https://openrouter.ai/api/v1"),
    ("openai-api", "gpt-5.1", "https://api.openai.com/v1"),
    ("anthropic", "claude-sonnet-5-5", "https://api.anthropic.com/v1"),
])
def test_a_second_run_on_another_provider_never_sends_it_the_eurouter_key(monkeypatch, tmp_path, provider, name,
                                                                          base_url):
    monkeypatch.setenv("HERMES_HOME", str(tmp_path))
    runner = FakeHermes()
    # first run: Enter, Enter (EUrouter, GLM-5.3 Flash); the install then stops (bridge, voices…): UNFINISHED
    hermes.set_model(runner, hermes.HUB_PROFILE, hermes.Model("custom", "glm-5.3-flash", base_url=EUROUTER,
                                                              api_key=FAKE_EU_KEY))
    # second run, taken up on the same files: another provider of the list
    new = hermes.Model(provider, name, base_url=base_url, api_key=FAKE_NEW_KEY)
    hermes.set_model(runner, hermes.HUB_PROFILE, new)
    model = yaml.safe_load((tmp_path / "config.yaml").read_text())["model"]
    assert (model["provider"], model["default"], model["base_url"]) == (provider, name, base_url)
    # the variable Hermes (auth._model_level_key_env, then the provider's own) and the bridge (mail_scan::model_of)
    # read the key from
    variable = model.get("key_env") or new.key_env
    assert _env(tmp_path / ".env").get(variable) == FAKE_NEW_KEY, \
        f"model.key_env still {model.get('key_env')!r}: the EUrouter key would go to {base_url}"


# -- the pin test (test_routing.py `test_the_orders_are_the_bridge_s`) and what it lets drift --------------------------
# The installer copies the bridge's routing rule (routing.py DEFAULT_ORDER, MODEL_ORDERS, order_for, PRIVACY). The pin
# test parses model_route.rs and catches a literal entry added or changed, and a changed DEFAULT_ORDER (checked in
# this review by editing the file). These three changes of model_route.rs pass it silently: the hub's config and the
# installer's model test would then route otherwise than the bridge does, with nothing red.
_ORIGINAL = 'pub const MODEL_ORDERS: [(&str, &[&str]); 1] = [("deepseek", &["greenpt"])];'
DRIFTS = {
    # rustfmt or a longer list: an order written once, as a named constant
    "an order through a named constant": (
        _ORIGINAL,
        'const KIMI: &[&str] = &["entrim"];\n'
        'pub const MODEL_ORDERS: [(&str, &[&str]); 2] = [("deepseek", &["greenpt"]), ("kimi", KIMI)];'),
    # the rule itself (routing.py order_for copies « a part of its name »)
    "the matching rule": (
        "MODEL_ORDERS.iter().find(|(part, _)| model.contains(part))",
        "MODEL_ORDERS.iter().find(|(part, _)| model.starts_with(part))"),
    # what every call asks (routing.py PRIVACY copies model_route.rs privacy())
    "the privacy settings": (
        'json!({"data_collection": "deny", "max_retention_days": 0})',
        'json!({"data_collection": "deny", "max_retention_days": 0, "zdr": true})'),
}


@pytest.mark.parametrize("drift", sorted(DRIFTS))
def test_the_pin_test_fails_when_the_bridge_s_rule_drifts(monkeypatch, drift):
    import importlib.util
    import pathlib
    source = Path(__file__).resolve().parents[2] / "bridge-rs" / "src" / "model_route.rs"
    real = source.read_text(encoding="utf-8")
    before, after = DRIFTS[drift]
    assert before in real, "model_route.rs changed: update this review test"
    drifted = real.replace(before, after)
    reader = pathlib.Path.read_text

    def read_text(self, *a, **k):
        return drifted if self.resolve() == source else reader(self, *a, **k)
    monkeypatch.setattr(pathlib.Path, "read_text", read_text)
    spec = importlib.util.spec_from_file_location("pin_test", Path(__file__).with_name("test_routing.py"))
    pin = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(pin)
    with pytest.raises(Exception):
        pin.test_the_orders_are_the_bridge_s()
