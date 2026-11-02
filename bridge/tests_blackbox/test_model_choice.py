"""An agent's model chosen from the app on an install of the person's own (bridge-rs/src/model_choice.rs):
`model_choice` in its settings, `PUT /v1/agents/{id}/model-choice` (a connector of the whitelist with its key, a custom
OpenAI-compatible server, back to the install's default), `POST …/models`. The key goes to the profile's `.env` and
never comes back; subscription sign-ins set on Hermes are only « set on your Hermes »; nothing of it on the offer."""
import json

import yaml

from bbkit.common import AUTH, sse
from conftest import managed_install, profile, wait_registered

HUB_MODEL = ("model:\n  provider: custom\n  default: glm-5.3-flash\n  base_url: https://api.eurouter.ai/api/v1\n"
             "  key_env: CUSTOM_MODEL_API_KEY\n  supports_vision: true\n")
KEY = "sk-own-agent-key-9f3c2b71abcd"


def home_install(bridges, fakes, start=True):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    (home / "config.yaml").write_text(HUB_MODEL)
    (home / ".env").write_text("CUSTOM_MODEL_API_KEY=hub-router-key\n")
    folder = profile(home, "coach")
    (folder / "config.yaml").write_text(HUB_MODEL + "agent:\n  max_turns: 60\n")
    (folder / ".env").write_text("CUSTOM_MODEL_API_KEY=hub-router-key\nAPI_SERVER_KEY=kept\n")
    if start:
        bridge.start()
        wait_registered(bridge, ["coach"])
    return bridge, home, folder


def env(folder):
    return dict(line.split("=", 1) for line in (folder / ".env").read_text().splitlines() if "=" in line)


def test_a_custom_server_is_tried_then_applied_and_its_key_never_comes_back(bridges, fakes):
    bridge, home, folder = home_install(bridges, fakes)
    choice = bridge.get("/v1/agents/coach/settings", headers=AUTH).json()["model_choice"]
    assert choice["choice"] == "default" and choice["default_model"] == "glm-5.3-flash"
    assert {"openrouter", "openai", "anthropic", "mistral", "custom"} <= set(choice["providers"])
    put = bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={
        "provider": "custom", "base_url": f"{fakes.model.url}/v1/", "model": "qwen3-32b", "key": KEY})
    assert put.status_code == 200, put.text
    assert KEY not in put.text and put.json()["key_hint"] == "…abcd" and put.json()["choice"] == "own"
    # tried once with the key (a word, then a word about an image)
    assert fakes.model.chats and all(c["authorization"] == f"Bearer {KEY}" for c in fakes.model.chats)
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["model"] == {"provider": "custom", "default": "qwen3-32b", "base_url": f"{fakes.model.url}/v1",
                             "key_env": "BIPAGENTS_AGENT_MODEL_KEY", "supports_vision": True}
    assert data["agent"]["max_turns"] == 60 and "providers" not in data   # kept; no router routing on another address
    kept = env(folder)
    assert kept["BIPAGENTS_AGENT_MODEL_KEY"] == KEY and kept["API_SERVER_KEY"] == "kept"
    assert (folder / ".env").stat().st_mode & 0o777 == 0o600
    marker = (folder / ".bipagents-model.json").read_text()
    assert KEY not in marker and json.loads(marker)["provider"] == "custom"
    settings = bridge.get("/v1/agents/coach/settings", headers=AUTH).text
    got = bridge.get("/v1/agents/coach/model-choice", headers=AUTH).text
    assert KEY not in settings and KEY not in got and KEY not in bridge.log_tail(2000)
    # another model on the same server: the key it has is kept and used
    fakes.model.chats.clear()
    again = bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={
        "provider": "custom", "base_url": f"{fakes.model.url}/v1", "model": "qwen3-8b"})
    assert again.status_code == 200 and again.json()["key_hint"] == "…abcd"
    assert fakes.model.chats[0]["authorization"] == f"Bearer {KEY}"
    assert env(folder)["BIPAGENTS_AGENT_MODEL_KEY"] == KEY
    assert bridge.logged("agent model chosen")[-1]["model"] == "qwen3-8b"


def test_back_to_the_default_deletes_the_key_and_copies_the_install_s_model(bridges, fakes):
    bridge, home, folder = home_install(bridges, fakes)
    bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={
        "provider": "custom", "base_url": f"{fakes.model.url}/v1", "model": "qwen3-32b", "key": KEY})
    back = bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={"provider": "default"})
    assert back.status_code == 200 and back.json()["choice"] == "default" and back.json()["key_hint"] is None
    data = yaml.safe_load((folder / "config.yaml").read_text())
    assert data["model"]["default"] == "glm-5.3-flash" and data["model"]["base_url"] == "https://api.eurouter.ai/api/v1"
    assert data["model"]["key_env"] == "CUSTOM_MODEL_API_KEY"
    kept = env(folder)
    assert "BIPAGENTS_AGENT_MODEL_KEY" not in kept and kept["CUSTOM_MODEL_API_KEY"] == "hub-router-key"
    assert not (folder / ".bipagents-model.json").exists()


def test_a_refused_key_or_a_missing_one_changes_nothing(bridges, fakes):
    bridge, home, folder = home_install(bridges, fakes)
    before = (folder / "config.yaml").read_text()
    fakes.model.status = 401
    refused = bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={
        "provider": "custom", "base_url": f"{fakes.model.url}/v1", "model": "qwen3-32b", "key": "bad"})
    assert refused.status_code == 400 and refused.json()["detail"]["error"] == "key_refused"
    fakes.model.status = 200
    # a connector of the whitelist needs a key; one off the whitelist is refused
    assert bridge.put("/v1/agents/coach/model-choice", headers=AUTH,
                      json={"provider": "openrouter", "model": "x/y"}).status_code == 422
    assert bridge.put("/v1/agents/coach/model-choice", headers=AUTH,
                      json={"provider": "openai-codex", "model": "gpt-6.1-sol"}).status_code == 422
    assert (folder / "config.yaml").read_text() == before and "BIPAGENTS_AGENT_MODEL_KEY" not in env(folder)


def test_a_subscription_sign_in_set_on_hermes_is_only_shown_as_such(bridges, fakes):
    bridge, home, folder = home_install(bridges, fakes, start=False)
    (folder / "config.yaml").write_text("model:\n  provider: openai-codex\n  default: gpt-6.1-sol\n")
    bridge.start()
    wait_registered(bridge, ["coach"])
    assert bridge.get("/v1/agents/coach/settings", headers=AUTH).json()["model_choice"] == {"choice": "hermes"}
    refused = bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={"provider": "default"})
    assert refused.status_code == 409
    assert "openai-codex" in (folder / "config.yaml").read_text()


def test_the_models_a_server_lists(bridges, fakes):
    bridge, home, folder = home_install(bridges, fakes)
    listed = bridge.post("/v1/agents/coach/model-choice/models", headers=AUTH,
                         json={"provider": "custom", "base_url": f"{fakes.model.url}/v1"})
    assert listed.status_code == 200 and listed.json() == {"models": ["glm"]}


def test_nothing_of_it_on_the_offer(bridges, fakes):
    bridge, home, folder = home_install(bridges, fakes, start=False)
    settings = bridge.home / ".config" / "bipagents" / "backup.json"
    settings.parent.mkdir(parents=True, exist_ok=True)
    settings.write_text(json.dumps({"url": "https://guichet.example/v1/backup", "token": "place-token"}))
    bridge.start()
    wait_registered(bridge, ["coach"])
    assert bridge.get("/v1/agents/coach/settings", headers=AUTH).json().get("model_choice") is None
    assert bridge.get("/v1/agents/coach/model-choice", headers=AUTH).status_code == 404
    assert bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={"provider": "default"}).status_code == 404


def test_a_turn_that_fails_on_the_agent_s_own_model_ends_with_a_plain_sentence(bridges, fakes):
    bridge, home, folder = home_install(bridges, fakes)
    bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={
        "provider": "custom", "base_url": f"{fakes.model.url}/v1", "model": "qwen3-32b", "key": KEY})
    fakes.hermes.runs["run_f"] = [sse("run.failed", error="Error code: 401 - {'error': {'message': 'Incorrect API key'}}")]
    with bridge.stream("GET", "/v1/runs/run_f/events?agent=coach", headers=AUTH) as resp:
        text = b"".join(resp.iter_raw()).decode()
    data = json.loads(text.split("event: run.failed\ndata: ", 1)[1].split("\n", 1)[0])
    assert data["own_model"] is True and "401" not in data["error"] and "{" not in data["error"]
    assert data["error"].startswith("Coach's model didn't answer") and data["error"].endswith(".")  # the place speaks English here
    # an agent on the install's default keeps Hermes' own words (the apps say them plainly)
    bridge.put("/v1/agents/coach/model-choice", headers=AUTH, json={"provider": "default"})
    fakes.hermes.runs["run_g"] = [sse("run.failed", error="x")]
    with bridge.stream("GET", "/v1/runs/run_g/events?agent=coach", headers=AUTH) as resp:
        text = b"".join(resp.iter_raw()).decode()
    assert '"error": "x"' in text and "own_model" not in text
