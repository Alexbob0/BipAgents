"""The current time at the top of each turn (bridge-rs/src/dates/now.rs): ONE hidden line, computed when the turn goes,
in the person's zone (the phone's, `/v1/time/device`), never in the system prompt. The apps get it first in
`POST /v1/live/recaps/take` and put it before the message; a Live call's brief starts with it; the bridge's own turns
(a watcher waking its agent here) carry it. The apps hide every `<!--bip:…-->` note, so does the search."""
import re
from datetime import datetime
from zoneinfo import ZoneInfo

from bbkit.common import AUTH
from bbkit.fakes import split_now
from conftest import managed_install, profile
from test_message_search import store
from test_watchers import check, setup, turns

LINE = re.compile(r"^(?P<day>[^,]+(?:, [^,]+)*), (?P<hm>\d\d:\d\d), (?P<zone>[A-Za-z_/]+) \(UTC(?P<off>[+-]\d\d:\d\d)\)$")


def assert_now_in(line, zone):
    """The line says the time of `zone` now (within two minutes of the test's own clock), with its offset."""
    found = LINE.match(line or "")
    assert found, line
    assert found["zone"] == zone, line
    local = datetime.now(ZoneInfo(zone))
    offset = local.strftime("%z")
    assert found["off"] == f"{offset[:3]}:{offset[3:]}", line
    h, m = map(int, found["hm"].split(":"))
    apart = abs(local.hour * 60 + local.minute - (h * 60 + m)) % 1440
    assert min(apart, 1440 - apart) <= 2, (line, local.isoformat())
    assert str(local.year) in found["day"], line


def test_an_app_message_and_a_call_get_the_time_in_the_person_s_zone(bridge):
    assert bridge.put("/v1/time/device", headers=AUTH, json={"time_zone": "Asia/Tokyo"}).status_code == 200
    recap = bridge.post("/v1/live/recaps/take", headers=AUTH, json={"agent": "wellness", "session": "s1"}).json()["recap"]
    now, rest = split_now(recap)
    assert_now_in(now, "Asia/Tokyo")
    assert rest == ""   # nothing else waited
    # the person flies to New York: the next message has New York's time
    bridge.put("/v1/time/device", headers=AUTH, json={"time_zone": "America/New_York"})
    recap = bridge.post("/v1/live/recaps/take", headers=AUTH, json={"agent": "wellness", "session": "s1"}).json()["recap"]
    assert_now_in(split_now(recap)[0], "America/New_York")
    brief = bridge.get("/v1/live/brief", params={"agent": "wellness"}, headers=AUTH).json()
    assert_now_in(split_now(brief["text"])[0], "America/New_York")


def test_a_watcher_wakes_its_agent_with_the_time(bridges, fakes):
    bridge, value, conn, _ = setup(bridges, fakes)
    assert bridge.put("/v1/time/device", headers=AUTH, json={"time_zone": "Europe/Paris"}).status_code == 200
    check(bridge, conn)   # where comparisons start
    value.write_text("mercredi 09:00")
    assert check(bridge, conn)["woke"]
    [turn] = turns(fakes)
    assert_now_in(turn["now"], "Europe/Paris")
    assert turn["input"].startswith("<!--bip:watch\n")   # the note right after it
    assert not turn.get("instructions")   # never in the system prompt


def test_the_search_never_shows_the_line(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    line = "<!--bip:now jeudi 29 octobre 2026, 08:40, Europe/Paris (UTC+01:00)-->"
    store(profile(home, "coach"), [("s", None, None, "Voile", 0)], [
        ("s", "user", f"{line}\nOn réserve l'école de voile ?", 1_000_000.0, {}),
        ("s", "user", f"{line}\nEt la piscine ?", 1_000_100.0, {}),
    ])
    bridge.start()
    voile = bridge.get("/v1/search/messages", params={"q": "voile"}, headers=AUTH).json()["results"]
    assert [r["snippet"] for r in voile] == ["On réserve l'école de voile ?"]
    assert bridge.get("/v1/search/messages", params={"q": "octobre"}, headers=AUTH).json()["results"] == []
