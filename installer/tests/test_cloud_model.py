"""The installer's model step, in the cloud (15/11, Alex: « On peut laisser les autres et mettre EUrouter »): EUrouter
first, recommended and European, with the models known to work well with BipAgents in a list; the other providers as
before (a key, then the model's exact ID)."""
import sys
from pathlib import Path

import pytest
import yaml

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bipinstall import __main__ as cli, hermes, models, routing  # noqa: E402
from bipinstall.detect import Machine  # noqa: E402

MAC = Machine(system="macos", arch="arm64", memory_gib=16, cores=10, cpu="Apple M4", gpu="apple",
              tailnet_name="mini.tail0000.ts.net", lan_addresses=["192.168.1.20"])
EUROUTER = "https://api.eurouter.ai/api/v1"
FAKE_KEY = "sk-fake-test-key"   # never a real one


def pick(monkeypatch, capsys, *answers, machine=MAC):
    """pick_model with these answers; the model check is faked (no network) and recorded."""
    given = iter(answers)

    def typed(prompt=""):   # as a terminal shows it: the question, then what was typed
        answer = next(given)
        print(f"{prompt}{answer}")
        return answer
    monkeypatch.setattr("builtins.input", typed)
    checked = []

    async def check(base_url, model, api_key=None, **kw):
        checked.append((base_url, model, api_key))
        return models.ModelCheck(ok=True, seconds=0.5, vision=True)
    monkeypatch.setattr(cli.models, "check", check)
    model, here = cli.pick_model(machine)
    assert next(given, None) is None, "every answer is used"
    assert checked == [(model.base_url, model.name, model.api_key)]   # the one chosen is tested
    return model, here, capsys.readouterr().out


def test_eurouter_comes_first_recommended_and_european_and_the_others_are_unchanged():
    assert [p.key for p in models.CLOUD] == ["eurouter", "openrouter", "openai", "anthropic", "mistral"]
    eu = models.CLOUD[0]
    assert eu.label == "EUrouter (recommended, European)"
    assert eu.base_url == EUROUTER == cli.CLOUD_MODEL_URL   # the all-included offer's address
    # its key page, as EUrouter's docs give it (docs/quickstart, docs/concepts/authentication: Dashboard › API Keys)
    assert eu.key_hint == "https://www.eurouter.ai/dashboard" and eu.key_where == "API Keys"
    assert [(p.label, p.base_url, p.key_hint, p.key_where) for p in models.CLOUD[1:]] == [
        ("OpenRouter (hundreds of models)", "https://openrouter.ai/api/v1", "https://openrouter.ai/keys", ""),
        ("OpenAI", "https://api.openai.com/v1", "https://platform.openai.com/api-keys", ""),
        ("Anthropic (Claude)", "https://api.anthropic.com/v1", "https://console.anthropic.com/settings/keys", ""),
        ("Mistral", "https://api.mistral.ai/v1", "https://console.mistral.ai/api-keys", ""),
    ]
    assert all(not p.known for p in models.CLOUD[1:])   # a typed ID for them, as before


def test_eurouter_s_models_are_the_offer_s_and_its_replacement():
    """The offer's model first (the default), then the one the offer proposes when it is withdrawn: the two the
    « real day » benches passed (internal/research/realday-bench-2026-10-30-release.md, -10-31.md)."""
    known = models.CLOUD[0].known
    assert [m.id for m in known] == ["glm-5.3-flash", "deepseek/deepseek-v4.1-flash"]
    assert known[0].id == cli.CLOUD_MODEL
    assert [m.label for m in known] == ["GLM-5.3 Flash (the all-included offer's model)",
                                        "DeepSeek V4.1 Flash (smarter, costs more)"]


def test_the_cloud_line_names_eurouter_first(monkeypatch, capsys):
    _, _, out = pick(monkeypatch, capsys, "2", "", FAKE_KEY, "")
    assert "  2. In the cloud, through an API (EUrouter, OpenRouter, OpenAI, Anthropic, Mistral)\n" in out
    assert ("  1. EUrouter (recommended, European)\n  2. OpenRouter (hundreds of models)\n  3. OpenAI\n"
            "  4. Anthropic (Claude)\n  5. Mistral\n") in out


def test_eurouter_by_default_with_the_offer_s_model(monkeypatch, capsys):
    """Enter at each question: EUrouter, then GLM-5.3 Flash, as the all-included offer."""
    model, here, out = pick(monkeypatch, capsys, "2", "", FAKE_KEY, "")
    assert (model.provider, model.name, model.base_url, model.api_key) == ("custom", "glm-5.3-flash", EUROUTER, FAKE_KEY)
    assert model.key_env == "CUSTOM_MODEL_API_KEY" and not here
    assert "Create your key here: https://www.eurouter.ai/dashboard (API Keys)\n" in out
    assert ("  1. GLM-5.3 Flash (the all-included offer's model)\n  2. DeepSeek V4.1 Flash (smarter, costs more)\n"
            "  3. Other (type its ID)\n") in out
    assert "exact ID" not in out   # no ID to type for a model of the list


def test_eurouter_s_second_model_from_the_list(monkeypatch, capsys):
    model, _, _ = pick(monkeypatch, capsys, "2", "1", FAKE_KEY, "2")
    assert (model.provider, model.name, model.base_url) == ("custom", "deepseek/deepseek-v4.1-flash", EUROUTER)


def test_eurouter_s_other_model_is_typed(monkeypatch, capsys):
    """« Other », last: its ID typed, asked again while empty."""
    model, _, out = pick(monkeypatch, capsys, "2", "1", FAKE_KEY, "3", "", "minimax-m3")
    assert (model.provider, model.name, model.base_url) == ("custom", "minimax-m3", EUROUTER)
    assert "exact ID" in out


@pytest.mark.parametrize("choice, provider, default, base_url", [
    ("2", "openrouter", "anthropic/claude-sonnet-5.5", "https://openrouter.ai/api/v1"),
    ("3", "openai-api", "gpt-5.1", "https://api.openai.com/v1"),
    ("4", "anthropic", "claude-sonnet-5-5", "https://api.anthropic.com/v1"),
    ("5", "custom", "mistral-large-latest", "https://api.mistral.ai/v1"),
])
def test_the_other_providers_still_ask_for_the_model_s_exact_id(monkeypatch, capsys, choice, provider, default, base_url):
    model, _, out = pick(monkeypatch, capsys, "2", choice, FAKE_KEY, "")
    assert (model.provider, model.name, model.base_url, model.api_key) == (provider, default, base_url, FAKE_KEY)
    assert f"Model (exact ID) [{default}]" in out and "Other (type its ID)" not in out
    typed, _, _ = pick(monkeypatch, capsys, "2", choice, FAKE_KEY, "my-own-model")
    assert typed.name == "my-own-model"


def test_a_rented_server_s_default_is_the_cloud_then_eurouter(monkeypatch, capsys):
    vps = Machine("linux", "arm64", 8, 4, "Neoverse-N1", None, tailnet_name="vps.tail0000.ts.net",
                  lan_addresses=["10.0.0.2"], cloud="Hetzner")
    model, _, _ = pick(monkeypatch, capsys, "", "", FAKE_KEY, "", machine=vps)
    assert (model.name, model.base_url) == ("glm-5.3-flash", EUROUTER)


def _hub_config(monkeypatch, tmp_path, model):
    """The hub's config.yaml once set_model has run (`hermes config set` recorded, not run: it writes the model
    section itself, put here as it would)."""
    monkeypatch.setenv("HERMES_HOME", str(tmp_path))
    (tmp_path / "config.yaml").write_text(yaml.safe_dump(
        {"model": {"provider": model.provider, "default": model.name, "base_url": model.base_url}}, sort_keys=False))

    class Recorder(hermes.Runner):
        def run(self, args, **kw):
            self.log.append(" ".join(args) if not isinstance(args, str) else args)
            return ""
    runner = Recorder()
    hermes.set_model(runner, hermes.HUB_PROFILE, model)
    assert FAKE_KEY not in "\n".join(runner.log)   # the key in the .env only
    return yaml.safe_load((tmp_path / "config.yaml").read_text())


def test_a_self_install_on_eurouter_is_routed_as_the_offer(monkeypatch, capsys, tmp_path):
    """The providers EUrouter tries first (their prompt cache) and the privacy settings, as the offer's places
    (routing.py, bridge-rs/src/model_route.rs)."""
    model, _, _ = pick(monkeypatch, capsys, "2", "", FAKE_KEY, "")
    data = _hub_config(monkeypatch, tmp_path, model)
    assert data["providers"] == {"bipagents-route": {"api": EUROUTER, "extra_body": {"provider": {
        "order": ["entrim", "greenpt", "inceptron"], "allow_fallbacks": True,
        "data_collection": "deny", "max_retention_days": 0}}}}
    assert (tmp_path / ".env").read_text() == f"CUSTOM_MODEL_API_KEY={FAKE_KEY}\n"


def test_deepseek_on_eurouter_gets_its_own_provider_as_on_the_offer(monkeypatch, capsys, tmp_path):
    """DeepSeek on the provider that caches it (GreenPT), as the bridge routes it (model_route.rs MODEL_ORDERS)."""
    model, _, _ = pick(monkeypatch, capsys, "2", "1", FAKE_KEY, "2")
    data = _hub_config(monkeypatch, tmp_path, model)
    assert data["providers"]["bipagents-route"]["extra_body"]["provider"]["order"] == ["greenpt"]


def test_the_other_providers_get_no_eurouter_routing(monkeypatch, capsys, tmp_path):
    model, _, _ = pick(monkeypatch, capsys, "2", "5", FAKE_KEY, "")   # Mistral: « custom » too, another address
    assert "providers" not in _hub_config(monkeypatch, tmp_path, model)


def test_the_check_runs_on_the_providers_the_agents_will_use():
    """The model's test message is routed as the agents' calls will be: DeepSeek to GreenPT, GLM to the place's order."""
    import asyncio
    import json

    import httpx
    sent = []

    def server(request):
        sent.append(json.loads(request.content))
        return httpx.Response(200, json={"choices": [{"message": {"content": "OK"}}]})
    for name, order in (("glm-5.3-flash", routing.DEFAULT_ORDER), ("deepseek/deepseek-v4.1-flash", ["greenpt"])):
        sent.clear()
        assert asyncio.run(models.check(EUROUTER, name, FAKE_KEY, transport=httpx.MockTransport(server))).ok
        assert [body["provider"]["order"] for body in sent] == [order, order]   # the text, then the image


def test_a_second_run_leaves_no_eurouter_key_behind(monkeypatch, tmp_path):
    """After a first run on EUrouter (review of installer-eurouter, N1): another provider's key pointer is its own,
    a server without a key gets no pointer, and EUrouter's key leaves the .env either way; the other lines stay."""
    from test_cloud_model_review import FakeHermes   # `hermes config set/unset` applied to the file, as Hermes does
    monkeypatch.setenv("HERMES_HOME", str(tmp_path))
    eu = hermes.Model("custom", "glm-5.3-flash", base_url=EUROUTER, api_key=FAKE_KEY)
    for second, pointer, kept in (
            (hermes.Model("openai-api", "gpt-5.1", base_url="https://api.openai.com/v1", api_key="sk-fake-openai"),
             "OPENAI_API_KEY", "OPENAI_API_KEY=sk-fake-openai"),
            (hermes.Model("custom", "glm", base_url="http://192.168.1.30:8888/v1"), None, None)):
        (tmp_path / "config.yaml").unlink(missing_ok=True)
        (tmp_path / ".env").write_text("API_SERVER_KEY=hub-key\n")
        runner = FakeHermes()
        hermes.set_model(runner, hermes.HUB_PROFILE, eu)
        assert "CUSTOM_MODEL_API_KEY" in (tmp_path / ".env").read_text()
        hermes.set_model(runner, hermes.HUB_PROFILE, second)
        model = yaml.safe_load((tmp_path / "config.yaml").read_text())["model"]
        assert model.get("key_env") == pointer and model["base_url"] == second.base_url
        env = (tmp_path / ".env").read_text()
        assert FAKE_KEY not in env and env.startswith("API_SERVER_KEY=hub-key\n") and (kept is None or kept in env)
        assert (tmp_path / ".env").stat().st_mode & 0o777 == 0o600
        assert not any(secret in line for line in runner.log for secret in (FAKE_KEY, "sk-fake-openai"))


def test_a_first_run_writes_the_key_pointer_as_before(monkeypatch, tmp_path):
    """Nothing left by an earlier run: the same commands as before the fix (no pointer for a provider of its own)."""
    monkeypatch.setenv("HERMES_HOME", str(tmp_path))
    openrouter = hermes.Model("openrouter", "x", base_url="https://openrouter.ai/api/v1", api_key="k")
    eurouter = hermes.Model("custom", "glm-5.3-flash", base_url=EUROUTER, api_key="k")
    for model, pointer in ((openrouter, None), (eurouter, "CUSTOM_MODEL_API_KEY")):
        runner = hermes.Runner(dry=True)
        hermes.set_model(runner, hermes.HUB_PROFILE, model)
        log = "\n".join(runner.log)
        assert ("config set model.key_env" in log) == bool(pointer) and "unset" not in log and "drop " not in log
