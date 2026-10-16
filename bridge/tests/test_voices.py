"""The person's own voice server: which voice speaks for what, and the routes the app changes it with."""
import pytest

from bipbridge.config import parse_config
from bipbridge.voices import set_section_keys, voice_for
from conftest import AUTH


@pytest.fixture
def server_config(config_dict, tmp_path):
    path = tmp_path / "bridge.toml"
    path.write_text('bridge_key = "x"\n\n[voice_server]\nurl = "http://gpu.test:8880"\n\n'
                    '[agents.wellness]\ndisplay_name = "Wellness"\nntfy_topic = "t"\n', encoding="utf-8")
    config_dict["voice_server"] = {"url": "http://gpu.test:8880", "voices": ["af_bella", "am_adam"], "live": False}
    config_dict["agents"]["Wellness"]["server_voice"] = "server:fr/af_bella"
    return parse_config(config_dict, source_path=str(path))


@pytest.fixture
def server_client(server_config, backend, apns):
    from fastapi.testclient import TestClient

    from bipbridge.app import create_app

    with TestClient(create_app(server_config, transport=backend.transport(), apns_transport=apns.transport())) as client:
        yield client
        backend.release.set()


def test_each_use_speaks_with_the_voice_it_goes_to(server_config):
    agent = server_config.agent("wellness")
    assert voice_for(server_config, agent, "replies") == "server:fr/af_bella"
    assert voice_for(server_config, agent, "reading") == "server:fr/af_bella"
    assert voice_for(server_config, agent, "live") == "4193"        # the Live stays on its Bip
    assert voice_for(server_config, agent, None) == "4193"
    agent.server_voice = None
    assert voice_for(server_config, agent, "replies") == "4193"     # no voice picked on the server yet


def test_section_keys_are_replaced_added_or_removed_and_the_rest_stays(tmp_path):
    path = tmp_path / "bridge.toml"
    path.write_text('[voice_server]\nurl = "http://gpu:8880"\nlive = true\n# a comment\n\n[agents.vie]\nvoice = "a"\n',
                    encoding="utf-8")
    set_section_keys(str(path), "voice_server", {"live": False, "reading": True})
    set_section_keys(str(path), "agents.vie", {"voice": None, "server_voice": "server:fr/x"})
    set_section_keys(str(path), "pronunciations.fr", {"Garmin": "Garmine"})
    assert path.read_text(encoding="utf-8") == (
        '[voice_server]\nurl = "http://gpu:8880"\nlive = false\n# a comment\nreading = true\n\n'
        '[agents.vie]\nserver_voice = "server:fr/x"\n\n[pronunciations.fr]\nGarmin = "Garmine"\n')


def test_the_app_lists_the_server_voices_and_changes_the_uses(server_client, server_config):
    body = server_client.get("/v1/voices", headers=AUTH).json()
    assert body == {"pocket": True, "server": {"voices": ["af_bella", "am_adam"],
                                               "uses": {"live": False, "replies": True, "reading": True}}}
    resp = server_client.put("/v1/voices/uses", headers=AUTH, json={"live": True, "reading": False})
    assert resp.json() == {"live": True, "replies": True, "reading": False}
    text = open(server_config.source_path, encoding="utf-8").read()
    assert "live = true" in text and "reading = false" in text


def test_the_app_sets_an_agents_voices(server_client, server_config):
    resp = server_client.put("/v1/agents/wellness/voices", headers=AUTH,
                             json={"voice": "pocket:mousse", "server_voice": "server:fr/am_adam"})
    assert resp.status_code == 200
    agent = server_config.agent("wellness")
    assert agent.voice == "pocket:mousse" and agent.server_voice == "server:fr/am_adam"
    text = open(server_config.source_path, encoding="utf-8").read()
    assert 'voice = "pocket:mousse"' in text and 'server_voice = "server:fr/am_adam"' in text
    assert 'ntfy_topic = "t"' in text
    assert server_client.put("/v1/agents/nobody/voices", headers=AUTH, json={}).status_code == 404


def test_without_a_voice_server(client):
    assert client.get("/v1/voices", headers=AUTH).json() == {"pocket": True, "server": None}
    assert client.put("/v1/voices/uses", headers=AUTH, json={"live": False}).status_code == 409


def test_the_app_keeps_an_agents_look_for_the_other_apps(server_client, server_config):
    resp = server_client.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "creative", "color": "#f2649e"})
    assert resp.status_code == 200 and resp.json() == {"category": "creative", "color": "#F2649E"}
    listed = {a["id"]: a for a in server_client.get("/v1/agents", headers=AUTH).json()["agents"]}
    assert listed["wellness"]["category"] == "creative" and listed["wellness"]["color"] == "#F2649E"
    if server_config.source_path:
        assert 'category = "creative"' in open(server_config.source_path).read()
    assert server_client.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "dragon"}).status_code == 400
    assert server_client.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "tech", "color": "red"}).status_code == 422
    assert server_client.put("/v1/agents/nobody/appearance", headers=AUTH, json={"category": "tech"}).status_code == 404
