"""The `weather` tool (an agent's « Météo » switch, MET Norway's forecast, Nominatim's places) and the `route` tool
(a link to open the maps app, Google's travel time with `[maps] google_key`), over HTTP against bbkit's FakeMaps."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from conftest import managed_install, profile, wait_registered


def names(bridge):
    answer = rpc(bridge, "tools/list").json()
    assert "result" in answer, answer
    return [t["name"] for t in answer["result"]["tools"]]


def test_weather_follows_the_agents_switch(bridges, fakes):
    bridge = bridges.new("managed")
    profile(managed_install(bridge, fakes), "coach")   # its profile: the tools are registered at start
    bridge.start()
    wait_registered(bridge, ["coach"])
    give_token(bridge, "coach")
    assert "weather" not in names(bridge) and "route" in names(bridge)
    off = call(bridge, "weather", {"place": "Lyon"})
    assert off["isError"] is True and "« Météo »" in text_of(off)
    assert fakes.maps.requests == []

    settings = bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"weather": True})
    assert settings.status_code == 200 and settings.json()["weather"] == {"on": True}
    # Its tools' entry changed (a new version): registered again, with a token of its own; ours given back.
    assert sum(1 for a in bridge.commands.of("hermes") if len(a) > 4 and a[4] == "mcp_servers.bipagents.url") == 2
    give_token(bridge, "coach")
    assert names(bridge)[-1] == "weather"
    result = call(bridge, "weather", {"place": "Lyon", "days": 2})
    text = text_of(result)
    assert result["isError"] is False, text
    assert text.startswith("Weather for Lyon, Auvergne-Rhône-Alpes, France (45.76, 4.83).")
    assert "Rain: from " in text and "(3.0 mm over the next 24 hours)" in text
    assert text.endswith("Source: MET Norway (Norwegian Meteorological Institute), CC BY 4.0. "
                         "Place search © OpenStreetMap contributors.")
    met = fakes.maps.seen("/weatherapi/")
    assert len(met) == 1 and met[0].params == {"lat": "45.76", "lon": "4.83"}
    assert met[0].header("user-agent").startswith("BipAgents/")
    assert met[0].header("user-agent").endswith(" https://bipagents.com")
    assert fakes.maps.seen("/search")[0].params == {"q": "Lyon", "format": "jsonv2", "limit": "1", "addressdetails": "1"}
    # Again: the place and the forecast are kept (MET's Expires is an hour away).
    call(bridge, "weather", {"place": "lyon"})
    assert len(fakes.maps.requests) == 2
    nowhere = call(bridge, "weather", {"place": "Nowhere"})
    assert nowhere["isError"] is True and text_of(nowhere).startswith("no place found")

    assert bridge.patch("/v1/agents/coach/settings", headers=AUTH, json={"weather": False}).json()["weather"] == {"on": False}
    give_token(bridge, "coach")
    assert "weather" not in names(bridge)


def test_a_route_is_a_maps_link_and_with_a_key_a_travel_time(bridge, bridges, fakes):
    give_token(bridge)
    plain = call(bridge, "route", {"destination": "Musée des Confluences, Lyon", "mode": "walking"})
    assert plain["isError"] is False
    assert text_of(plain).endswith("\nhttps://www.google.com/maps/dir/?api=1&destination=Mus%C3%A9e%20des%20Confluences%2C%20Lyon"
                                   "&travelmode=walking")
    assert fakes.maps.requests == []

    keyed = bridges.new("keyed", change=lambda c: c["maps"].update(google_key="test-maps-key")).start()
    give_token(keyed)
    result = call(keyed, "route", {"destination": "Musée des Confluences, Lyon", "origin": "Place Bellecour, Lyon",
                                   "mode": "walking"})
    assert "Travel time: 45 min, 3.4 km (Google Maps)." in text_of(result)
    assert "test-maps-key" not in text_of(result)
    sent = fakes.maps.seen("/directions/")[0]
    assert sent.json()["travelMode"] == "WALK" and sent.header("x-goog-api-key") == "test-maps-key"
    assert call(keyed, "route", {"destination": "Lyon", "mode": "rocket"})["isError"] is True
