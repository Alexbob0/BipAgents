"""The person's own voice server: which voice speaks for what, and the routes the app changes voices and looks with
(`/v1/voices`, `/v1/voices/uses`, `/v1/agents/{id}/voices`, `/v1/agents/{id}/appearance`) — tests/test_voices.py,
over HTTP; the changes are written in the bridge's own config file."""
import tomllib

from bbkit.common import AUTH


def server_bridge(bridges, fakes):
    def change(c):
        c["voice_server"] = {"url": fakes.voices.url, "voices": ["af_bella", "am_adam"], "live": False}
        # Its table named as the bridge names it: the Python bridge writes `[agents.<id>]` and would add a second
        # table for `[agents.Wellness]` (see README, « Found while writing this suite »).
        c["agents"]["wellness"] = c["agents"].pop("Wellness")
        c["agents"]["wellness"]["server_voice"] = "server:fr/af_bella"
    fakes.voices.voices_route = "none"           # it cannot list them: the config's
    return bridges.new(change=change).start()


def prepared(bridge, use):
    return bridge.post("/v1/tts/prepare", headers=AUTH, json={"text": "Salut.", "agent": "wellness", "use": use}).json()


def test_each_use_speaks_with_the_voice_it_goes_to(bridges, fakes):
    bridge = server_bridge(bridges, fakes)
    assert prepared(bridge, "replies") == {"local": False, "voice": "server:fr/af_bella"}
    assert prepared(bridge, "reading") == {"local": False, "voice": "server:fr/af_bella"}
    assert prepared(bridge, "live") == {"local": False, "voice": "4193"}        # the Live stays on its Bip
    assert prepared(bridge, None) == {"local": False, "voice": "4193"}
    bridge.put("/v1/agents/wellness/voices", headers=AUTH, json={"voice": "4193"})
    assert prepared(bridge, "replies") == {"local": False, "voice": "4193"}     # no voice picked on the server yet


def test_the_app_lists_the_server_voices_and_changes_the_uses(bridges, fakes):
    bridge = server_bridge(bridges, fakes)
    body = bridge.get("/v1/voices", headers=AUTH).json()
    assert body == {"pocket": True, "server": {"voices": ["af_bella", "am_adam"],
                                               "uses": {"live": False, "replies": True, "reading": True}}}
    resp = bridge.put("/v1/voices/uses", headers=AUTH, json={"live": True, "reading": False})
    assert resp.json() == {"live": True, "replies": True, "reading": False}
    text = bridge.toml_text()
    assert "live = true" in text and "reading = false" in text
    assert tomllib.loads(text)["voice_server"]["voices"] == ["af_bella", "am_adam"]
    bridge.restart(write=False)
    assert bridge.get("/v1/voices", headers=AUTH).json()["server"]["uses"] == {"live": True, "replies": True,
                                                                              "reading": False}


def test_the_server_voices_in_its_order_from_whichever_route_it_has(bridges, fakes):
    bridge = bridges.new(change=lambda c: c.update(voice_server={"url": fakes.voices.url, "voices": ["alba", "marius"]}))
    bridge.start()
    fakes.voices.voices_route = "audio"
    assert bridge.get("/v1/voices", headers=AUTH).json()["server"]["voices"] == ["fr-naturelle-s15", "fr-mascotte-s17"]
    for name, route, voices in (("kyutai", "kyutai", ["5476", "posee"]), ("bare", "none", ["alba", "marius"])):
        other = bridges.new(name, change=lambda c: c.update(voice_server={"url": fakes.voices.url,
                                                                          "voices": ["alba", "marius"]})).start()
        fakes.voices.voices_route, fakes.voices.voices = route, ["5476", "posee"]
        assert other.get("/v1/voices", headers=AUTH).json()["server"]["voices"] == voices


def test_the_app_sets_an_agents_voices(bridges, fakes):
    bridge = server_bridge(bridges, fakes)
    resp = bridge.put("/v1/agents/wellness/voices", headers=AUTH,
                      json={"voice": "pocket:mousse", "server_voice": "server:fr/am_adam"})
    assert resp.status_code == 200 and resp.json() == {"voice": "pocket:mousse", "server_voice": "server:fr/am_adam"}
    agent = tomllib.loads(bridge.toml_text())["agents"]["wellness"]
    assert agent["voice"] == "pocket:mousse" and agent["server_voice"] == "server:fr/am_adam"
    assert agent["ntfy_topic"] == "hermes-wellness-out"
    listed = bridge.get("/v1/agents", headers=AUTH).json()["agents"][0]
    assert listed["voice"] == "pocket:mousse" and listed["server_voice"] == "server:fr/am_adam"
    cleared = bridge.put("/v1/agents/wellness/voices", headers=AUTH, json={"voice": "", "server_voice": None})
    assert cleared.json() == {"voice": None, "server_voice": None}
    assert "server_voice" not in tomllib.loads(bridge.toml_text())["agents"]["wellness"]
    assert bridge.get("/v1/agents", headers=AUTH).json()["agents"][0]["voice"] == "5476"   # the default
    assert bridge.put("/v1/agents/nobody/voices", headers=AUTH, json={}).status_code == 404


def test_without_a_voice_server(bridge):
    assert bridge.get("/v1/voices", headers=AUTH).json() == {"pocket": True, "server": None}
    refused = bridge.put("/v1/voices/uses", headers=AUTH, json={"live": False})
    assert refused.status_code == 409 and refused.json() == {"detail": "no voice server"}


def test_the_app_keeps_an_agents_look_for_the_other_apps(bridges, fakes):
    bridge = server_bridge(bridges, fakes)
    resp = bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "creative", "color": "#f2649e"})
    assert resp.status_code == 200 and resp.json() == {"category": "creative", "color": "#F2649E", "outfit": "", "costumes": True}
    listed = {a["id"]: a for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]}
    assert listed["wellness"]["category"] == "creative" and listed["wellness"]["color"] == "#F2649E"
    assert 'category = "creative"' in bridge.toml_text() and 'color = "#F2649E"' in bridge.toml_text()
    dragon = bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "dragon"})
    assert dragon.status_code == 400 and dragon.json() == {"detail": "unknown category"}
    assert bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "tech", "color": "red"}).status_code == 422
    assert bridge.put("/v1/agents/nobody/appearance", headers=AUTH, json={"category": "tech"}).status_code == 404
    no_color = bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "tech"})
    assert no_color.json() == {"category": "tech", "color": None, "outfit": "", "costumes": True}
    assert "color" not in tomllib.loads(bridge.toml_text())["agents"]["wellness"]
    # The Bip's costume (« Tenue ») and its event costumes: kept when an older app leaves them out
    dressed = bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "tech", "outfit": "beret", "costumes": False})
    assert dressed.json() == {"category": "tech", "color": None, "outfit": "beret", "costumes": False}
    kept = bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "home"})
    assert kept.json() == {"category": "home", "color": None, "outfit": "beret", "costumes": False}
    table = tomllib.loads(bridge.toml_text())["agents"]["wellness"]
    assert table["outfit"] == "beret" and table["costumes"] is False
    listed = {a["id"]: a for a in bridge.get("/v1/agents", headers=AUTH).json()["agents"]}
    assert listed["wellness"]["outfit"] == "beret" and listed["wellness"]["costumes"] is False
    cleared = bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "home", "outfit": "", "costumes": True})
    assert cleared.json() == {"category": "home", "color": None, "outfit": "", "costumes": True}
    assert "outfit" not in tomllib.loads(bridge.toml_text())["agents"]["wellness"]


def test_section_keys_are_replaced_added_or_removed_and_the_rest_stays(bridges, fakes):
    """voices.set_section_keys, seen in the file: comments and other lines kept, a missing table added."""
    bridge = bridges.new()
    bridge.config["voice_server"] = {"url": fakes.voices.url}
    bridge.extra_toml = '[agents.vie]\nhermes_url = "http://127.0.0.1:9"\nhermes_key = "' + "v" * 32 + '"\n# her voice\nvoice = "a"\n'
    bridge.start()
    bridge.put("/v1/voices/uses", headers=AUTH, json={"live": False, "reading": True})
    bridge.put("/v1/agents/vie/voices", headers=AUTH, json={"server_voice": "server:fr/x"})
    text = bridge.toml_text()
    assert "# her voice" in text and 'voice = "a"' not in text and 'server_voice = "server:fr/x"' in text
    vs = text.split("[voice_server]")[1].split("[")[0]
    assert "live = false" in vs and "reading = true" in vs
