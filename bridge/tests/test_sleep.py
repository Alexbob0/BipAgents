import asyncio
import dataclasses
from datetime import datetime, timedelta, timezone

from bipbridge import sleep


class Clock:
    def __init__(self):
        self.t = 1_790_000_000.0

    def __call__(self):
        return self.t


class Systemd:
    def __init__(self):
        self.running = True
        self.calls = []

    async def __call__(self, args):
        self.calls.append(list(args))
        if args[0] == "list-units":
            return ("hermes-gateway.service loaded active running Hermes\n"
                    "bipagents-bridge.service loaded active running Bridge\n")
        if args[0] == "stop":
            self.running = False
        if args[0] == "start":
            self.running = True
        return ""


def a_place(config, tmp_path, routines=()):
    clock, systemd = Clock(), Systemd()

    async def ran():
        return list(routines)

    async def up():
        return systemd.running

    cloud = dataclasses.replace(config, cloud=True)
    return sleep.Sleep(cloud, ran, up, run=systemd, now=clock, home=tmp_path), clock, systemd


def test_an_idle_place_sleeps_then_the_next_request_wakes_it(config, tmp_path):
    place, clock, systemd = a_place(config, tmp_path)
    assert not asyncio.run(place.check())                                 # used today
    clock.t += 15 * 86400
    assert asyncio.run(place.check()) and not systemd.running
    assert ["stop", "hermes-gateway.service"] in systemd.calls
    assert ["stop", "bipagents-bridge.service"] not in systemd.calls        # the bridge keeps answering
    assert (tmp_path / sleep.MARKER).is_file()                             # the seat agent says « asleep »
    asyncio.run(place.awake())
    assert systemd.running and not place.asleep and not (tmp_path / sleep.MARKER).exists()
    # Remembered across restarts of the bridge.
    again = sleep.Sleep(place.config, place.routines, place.hermes_up, run=systemd, now=clock, home=tmp_path)
    assert again.last == clock.t and not again.asleep


def test_a_place_with_routines_or_at_home_never_sleeps(config, tmp_path):
    recent = (datetime.now(timezone.utc) - timedelta(days=2)).isoformat()
    place, clock, systemd = a_place(config, tmp_path, routines=[recent])
    clock.t += 30 * 86400
    assert not asyncio.run(place.check()) and systemd.running
    home = sleep.Sleep(config, place.routines, place.hermes_up, run=systemd, now=clock, home=tmp_path)
    assert not home.enabled and not asyncio.run(home.check())
