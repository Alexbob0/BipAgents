"""The person's time zone and standard dates (`/v1/time`, `/v1/dates`, the `upcoming_dates` tool, the Carnet's
computed « Dates à venir »): the phone's zone followed, the settings checked and kept, holidays, school holidays and
clock changes computed for the person."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of


def offers(bridge):
    assert "time_zone" in bridge.get("/health").json().get("features", []), "this bridge has no time_zone"


def test_the_phone_zone_followed_and_the_settings_kept(bridge):
    offers(bridge)
    assert bridge.get("/v1/time").status_code in (401, 403)
    first = bridge.put("/v1/time/device?lang=fr", headers=AUTH, json={"time_zone": "Europe/Paris"})
    assert first.status_code == 200, first.text
    view = first.json()
    assert view["time_zone"] == "Europe/Paris" and view["routines_zone"] == "Europe/Paris" and view["auto"] is True
    assert view["question"] is None   # no routine: nothing to ask
    assert view["country"] == "FR" and view["country_guessed"] is True
    assert view["school_zones"] == ["A", "B", "C", "Corse"]
    assert {"code": "FR", "name": "France"} in view["countries"] and len(view["countries"]) == 32
    assert bridge.put("/v1/time/device", headers=AUTH, json={"time_zone": "Mars/Olympus"}).status_code == 422
    moved = bridge.put("/v1/time/device", headers=AUTH, json={"time_zone": "America/New_York"}).json()
    assert moved["time_zone"] == "America/New_York" and moved["routines_zone"] == "America/New_York"
    assert moved["country"] == "US" and moved["school_zones"] == []
    set_ = bridge.patch("/v1/time", headers=AUTH, json={"auto": False, "time_zone": "Europe/Paris", "country": "FR",
                                                         "postal_code": "67000"})
    assert set_.status_code == 200, set_.text
    assert (set_.json()["time_zone"], set_.json()["school_zone"], set_.json()["region"]) == ("Europe/Paris", "B", "6AE")
    assert bridge.put("/v1/time/device", headers=AUTH, json={"time_zone": "Asia/Tokyo"}).json()["time_zone"] == "Europe/Paris"
    assert bridge.patch("/v1/time", headers=AUTH, json={"country": "JP"}).status_code == 422
    assert bridge.patch("/v1/time", headers=AUTH, json={"school_zone": "D"}).status_code == 422
    assert bridge.post("/v1/time/routines", headers=AUTH, json={"choice": "move"}).status_code == 404
    bridge.restart()
    kept = bridge.get("/v1/time?lang=en", headers=AUTH).json()
    assert (kept["auto"], kept["time_zone"], kept["device"], kept["region"]) == (False, "Europe/Paris", "Asia/Tokyo", "6AE")
    assert {"code": "FR", "name": "France"} in kept["countries"]


def test_the_standard_dates_route_the_carnet_and_the_tool(bridge):
    offers(bridge)
    bridge.patch("/v1/time", headers=AUTH, json={"auto": False, "time_zone": "Europe/Paris", "country": "FR",
                                                 "school_zone": "C"})
    dates = bridge.get("/v1/dates?days=366&lang=fr", headers=AUTH).json()
    assert dates["time_zone"] == "Europe/Paris"
    kinds = {d["kind"] for d in dates["dates"]}
    assert kinds == {"holiday", "school", "clock_change"}, dates
    assert any(d["text"] == "jour férié : Fête nationale" for d in dates["dates"])
    assert any(d["text"].startswith("passage à l'heure d'") for d in dates["dates"])
    carnet = bridge.get("/v1/notebook/dates?days=366", headers=AUTH).json()["dates"]
    computed = [d for d in carnet if d.get("computed")]
    assert computed and all(d["fact"] == d["text"] for d in computed)
    plain = bridge.get("/v1/notebook/dates?days=366&standard=0", headers=AUTH).json()["dates"]
    assert not [d for d in plain if d.get("computed")]
    give_token(bridge)
    result = call(bridge, "upcoming_dates", {"days": 366})
    assert result["isError"] is False
    text = text_of(result)
    assert text.startswith("Time zone: Europe/Paris (now ") and "French school zone: C" in text, text
    assert "Fête nationale" in text or "National Day" in text, text
