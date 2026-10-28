"""The person's phone calendar shared with the agents they pick (`/v1/calendar…`), a mailbox's calendars by CalDAV
(`PATCH /v1/mail/accounts/{id}` with `calendar`), and the calendar tools over MCP, listed only for those agents. Rust
only: skipped when `/health` does not list the feature (the Python bridge)."""
import base64
import html
import http.server
import os
import threading

import pytest

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from conftest import managed_install, profile, wait_registered

PASSWORD = "bridge-password-1234"

TOKEN = "coach-token-" + "k" * 30
COPY = {"from": "2026-10-14", "to": "2026-12-20",
        "calendars": [{"id": "C1", "title": "Perso", "color": "#3CC37A"}, {"id": "C2", "title": "Travail"}],
        "events": [
            {"id": "e1", "calendar": "C1", "title": "Dentiste", "start": "2026-10-22T10:00:00+02:00",
             "end": "2026-10-22T10:30:00+02:00", "all_day": False, "location": "12 rue Haute"},
            {"id": "e2", "calendar": "C2", "title": "Salon", "start": "2026-10-23", "end": "2026-10-25",
             "all_day": True, "notes": "Stand B4"},
            {"id": "e3", "calendar": "C2", "title": "Point équipe", "start": "2026-10-21T09:00:00+02:00",
             "end": "2026-10-21T09:30:00+02:00", "recurring": True}]}


def coach(bridges, fakes):
    bridge = bridges.new()
    profile(managed_install(bridge, fakes, agents=("coach",)), "coach")
    bridge.start()
    if "calendar" not in bridge.get("/health").json().get("features", []):
        pytest.skip("this bridge has no calendar")
    wait_registered(bridge, ["coach"])
    give_token(bridge, "coach", TOKEN)
    return bridge


def names(bridge):
    return [t["name"] for t in rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]]


def test_the_phone_shares_its_calendar_and_does_what_the_agents_ask(bridges, fakes):
    bridge = coach(bridges, fakes)
    assert bridge.get("/v1/calendar").status_code == 401
    assert not any(n.startswith("calendar_") for n in names(bridge))
    not_yet = call(bridge, "calendar_list", {}, token=TOKEN)
    assert not_yet["isError"] and "hasn't shared their phone calendar" in text_of(not_yet)
    unknown = bridge.put("/v1/calendar/agents", json={"agents": ["ghost"]}, headers=AUTH)
    assert unknown.status_code == 400 and unknown.json() == {"detail": "unknown agent"}
    assert bridge.put("/v1/calendar/agents", json={"agents": ["Coach"]}, headers=AUTH).json() == {"agents": ["coach"]}
    give_token(bridge, "coach", TOKEN)   # its tools' entry was written again (a new version), with a token of its own
    assert [n for n in names(bridge) if n.startswith("calendar_")] == [
        "calendar_list", "calendar_add", "calendar_update", "calendar_delete"]

    assert bridge.put("/v1/calendar", json=COPY, headers=AUTH).json() == {"events": 3}
    listed = text_of(call(bridge, "calendar_list", {"from": "2026-10-21", "to": "2026-10-24"}, token=TOKEN))
    assert "- id e1 · Perso (phone) · 2026-10-22 10:00–10:30 · Dentiste · at 12 rue Haute" in listed
    assert "- id e2 · Travail (phone) · 2026-10-23 → 2026-10-25 all day · Salon" in listed and "Stand B4" in listed
    assert "· repeats" in listed and "for 2026-10-14 to 2026-12-20" in listed
    work = text_of(call(bridge, "calendar_list", {"from": "2026-10-21", "to": "2026-10-30", "calendar": "perso"},
                        token=TOKEN))
    assert "Dentiste" in work and "Salon" not in work

    asked = call(bridge, "calendar_add", {"title": "Banque", "start": "2026-10-30T10:00", "duration_minutes": 30,
                                          "calendar": "Perso"}, token=TOKEN)
    assert not asked["isError"] and "next sync" in text_of(asked)
    assert call(bridge, "calendar_add", {"title": "x", "start": "plus tard"}, token=TOKEN)["isError"]
    assert call(bridge, "calendar_delete", {"id": "zz"}, token=TOKEN)["isError"]
    assert "repeating" in text_of(call(bridge, "calendar_delete", {"id": "e3"}, token=TOKEN))
    assert not call(bridge, "calendar_update", {"id": "e1", "title": "Dentiste (Dr Roy)"}, token=TOKEN)["isError"]
    queue = bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"]
    assert [op["op"] for op in queue] == ["add", "update"]
    assert set(queue[0]) == {"id", "op", "args", "agent", "created"} and queue[0]["agent"] == "coach"
    assert queue[0]["args"] == {"title": "Banque", "start": "2026-10-30T10:00:00", "end": "2026-10-30T10:30:00",
                                "all_day": False, "calendar": "Perso", "location": "", "notes": ""}
    assert queue[1]["args"] == {"id": "e1", "title": "Dentiste (Dr Roy)"}
    assert bridge.post(f"/v1/calendar/queue/{queue[0]['id']}", json={"done": True}, headers=AUTH).json() == {"ok": True}
    assert bridge.post("/v1/calendar/queue/nope", json={"done": True}, headers=AUTH).status_code == 404
    seen = bridge.get("/v1/calendar", headers=AUTH).json()
    assert seen["agents"] == ["coach"] and seen["events"] == 3 and seen["calendars"] == 2 and seen["pending"] == 1
    assert isinstance(seen["at"], float) and seen["from"] == "2026-10-14"
    assert oct(os.stat(bridge.state_dir / "calendar.json").st_mode & 0o777) == "0o600"

    assert bridge.put("/v1/calendar/agents", json={"agents": []}, headers=AUTH).json() == {"agents": []}
    give_token(bridge, "coach", TOKEN)
    assert not any(n.startswith("calendar_") for n in names(bridge))
    assert call(bridge, "calendar_list", {}, token=TOKEN)["isError"]


def test_what_the_agent_asked_shows_until_the_phone_shares_again(bridges, fakes):
    """2026-10-27, token lab: an agent added an event, listed the calendar to check, didn't find it (the phone hadn't
    synced) and added it again, 255 times in one turn. Its own adds are listed now, marked; the same one isn't asked
    again; one still waiting can be cancelled or changed by its id; the same write twice in a turn is refused."""
    bridge = coach(bridges, fakes)
    assert bridge.put("/v1/calendar/agents", json={"agents": ["coach"]}, headers=AUTH).status_code == 200
    give_token(bridge, "coach", TOKEN)
    assert bridge.put("/v1/calendar", json=COPY, headers=AUTH).json() == {"events": 3}
    asked = text_of(call(bridge, "calendar_add", {"title": "Test", "start": "2026-10-28T10:00"}, token=TOKEN))
    assert "next sync" in asked and "calendar_list shows it as asked" in asked
    op_id = bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"][0]["id"]
    assert f"with the id {op_id}" in asked
    listed = text_of(call(bridge, "calendar_list", {"from": "2026-10-27", "to": "2026-10-29"}, token=TOKEN))
    assert f"- id {op_id} · default calendar (phone) · 2026-10-28 10:00–11:00 · Test · asked by you, not on the phone yet" \
        in listed
    # the same event again (other words for the same moment): not asked again, its id given
    again = call(bridge, "calendar_add", {"title": " test", "start": "2026-10-28T10:00:00", "duration_minutes": 30},
                 token=TOKEN)
    assert not again["isError"] and text_of(again).startswith(f"Already asked: id {op_id}")
    # exactly the same call again
    same = call(bridge, "calendar_add", {"title": "Test", "start": "2026-10-28T10:00"}, token=TOKEN)
    assert not same["isError"] and text_of(same).startswith(f"Already asked: id {op_id}")
    # an event already in the phone's copy
    known = text_of(call(bridge, "calendar_add", {"title": "Dentiste", "start": "2026-10-22T10:00"}, token=TOKEN))
    assert known.startswith("Already in the calendar: id e1")
    assert len(bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"]) == 1
    # changed, then cancelled, before the phone does it
    assert text_of(call(bridge, "calendar_update", {"id": op_id, "title": "Banque"}, token=TOKEN)).startswith("Changed")
    assert bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"][0]["args"]["title"] == "Banque"
    assert text_of(call(bridge, "calendar_delete", {"id": op_id}, token=TOKEN)).startswith("Cancelled")
    assert bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"] == []
    assert "Banque" not in text_of(call(bridge, "calendar_list", {"from": "2026-10-27", "to": "2026-10-29"}, token=TOKEN))
    # done by the phone: listed as added until its next copy, which has it
    call(bridge, "calendar_add", {"title": "Coiffeur", "start": "2026-10-28T15:00"}, token=TOKEN)
    op_id = bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"][0]["id"]
    assert bridge.post(f"/v1/calendar/queue/{op_id}", json={"done": True}, headers=AUTH).json() == {"ok": True}
    listed = text_of(call(bridge, "calendar_list", {"from": "2026-10-27", "to": "2026-10-29"}, token=TOKEN))
    assert "Coiffeur · added by the phone just now" in listed
    assert call(bridge, "calendar_delete", {"id": op_id}, token=TOKEN)["isError"]
    shared = {**COPY, "events": COPY["events"] + [
        {"id": "e9", "calendar": "C1", "title": "Coiffeur", "start": "2026-10-28T15:00:00+01:00",
         "end": "2026-10-28T16:00:00+01:00", "all_day": False}]}
    assert bridge.put("/v1/calendar", json=shared, headers=AUTH).json() == {"events": 4}
    listed = text_of(call(bridge, "calendar_list", {"from": "2026-10-27", "to": "2026-10-29"}, token=TOKEN))
    assert "- id e9 · Perso (phone)" in listed and op_id not in listed


def test_the_copy_is_checked(bridges, fakes):
    bridge = coach(bridges, fakes)
    assert bridge.put("/v1/calendar", json={"events": [{"id": "x" * 301, "start": "2026-10-22"}]},
                      headers=AUTH).status_code == 422
    assert bridge.put("/v1/calendar", json={"events": [{"id": "e"}]}, headers=AUTH).status_code == 422   # no start
    assert bridge.put("/v1/calendar", json={"events": [{}] * 5001}, headers=AUTH).status_code == 422


# -- a mailbox's calendars, by CalDAV -----------------------------------------------------------------------------------

class FakeDav(http.server.BaseHTTPRequestHandler):
    """A CalDAV server answering the requests the bridge makes (one calendar, « Travail »), checking the credentials."""
    events = {}

    def log_message(self, *args):
        pass

    def answer(self, status, body=b"", headers=()):
        self.send_response(status)
        for k, v in headers:
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def multistatus(self, responses):
        body = ('<?xml version="1.0"?><d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">'
                + responses + "</d:multistatus>").encode()
        self.answer(207, body, [("Content-Type", "application/xml")])

    def handle_dav(self):
        length = int(self.headers.get("Content-Length") or 0)
        body = self.rfile.read(length).decode() if length else ""
        if self.headers.get("Authorization") != "Basic " + base64.b64encode(f"alex@proton.me:{PASSWORD}".encode()).decode():
            return self.answer(401)
        if self.command == "PROPFIND" and self.path == "/dav/":
            return self.multistatus("<d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:current-user-principal>"
                                    "<d:href>/dav/home/</d:href></d:current-user-principal></d:prop></d:propstat></d:response>")
        if self.command == "PROPFIND" and self.path == "/dav/home/":
            if "calendar-home-set" in body:
                return self.multistatus("<d:response><d:href>/dav/home/</d:href><d:propstat><d:prop><c:calendar-home-set>"
                                        "<d:href>/dav/home/</d:href></c:calendar-home-set></d:prop></d:propstat></d:response>")
            return self.multistatus(
                "<d:response><d:href>/dav/home/work/</d:href><d:propstat><d:prop><d:displayname>Travail</d:displayname>"
                "<d:resourcetype><d:collection/><c:calendar/></d:resourcetype></d:prop></d:propstat></d:response>")
        if self.command == "REPORT" and self.path == "/dav/home/work/":
            return self.multistatus("".join(
                f"<d:response><d:href>{href}</d:href><d:propstat><d:prop><c:calendar-data>{html.escape(ics)}"
                "</c:calendar-data></d:prop></d:propstat></d:response>" for href, ics in FakeDav.events.items()))
        if self.command == "PUT" and self.path.startswith("/dav/home/work/"):
            FakeDav.events[self.path] = body
            return self.answer(201)
        return self.answer(405)

    do_PROPFIND = do_REPORT = do_PUT = do_GET = do_DELETE = handle_dav


@pytest.fixture
def fake_dav():
    FakeDav.events = {"/dav/home/work/e1.ics": "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:e1\r\nDTSTART;VALUE=DATE:20261023\r\n"
                                                "DTEND;VALUE=DATE:20261024\r\nSUMMARY:Salon\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n"}
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeDav)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


def test_a_mailboxs_calendar_switched_on_and_off(bridges, fakes, fake_dav):
    bridge = coach(bridges, fakes)
    fakes.imap.users["alex@proton.me"] = PASSWORD
    added = bridge.post("/v1/mail/accounts", headers=AUTH, json={
        "provider": "proton", "username": "alex@proton.me", "password": PASSWORD, "port": fakes.imap.port,
        "label": "Mail perso", "agents": ["Coach"]})
    assert added.status_code == 201, added.text
    account = added.json()["id"]
    assert "calendar" not in added.json()
    plain = bridge.patch(f"/v1/mail/accounts/{account}", headers=AUTH,
                         json={"calendar": True, "calendar_url": "http://dav.example.org/"})
    assert plain.status_code == 400
    on = bridge.patch(f"/v1/mail/accounts/{account}", headers=AUTH,
                      json={"calendar": True, "calendar_url": f"{fake_dav}/dav/"})
    assert on.status_code == 200, on.text
    assert on.json()["calendar"] == {"calendars": ["Travail"]} and PASSWORD not in on.text
    assert bridge.get("/v1/mail/accounts", headers=AUTH).json()["accounts"][0]["calendar"] == {"calendars": ["Travail"]}
    give_token(bridge, "coach", TOKEN)   # its tools' entry was written again
    assert "calendar_list" in names(bridge)
    listed = text_of(call(bridge, "calendar_list", {"from": "2026-10-21", "to": "2026-10-31"}, token=TOKEN))
    assert "· Travail (Mail perso) · 2026-10-23 all day · Salon" in listed and "read now from Mail perso" in listed
    asked = call(bridge, "calendar_add", {"title": "Banque", "start": "2026-10-30T10:00:00+01:00", "calendar": "Travail"},
                 token=TOKEN)
    assert not asked["isError"] and "Added to « Travail » (Mail perso)" in text_of(asked)
    put = [ics for href, ics in FakeDav.events.items() if href.endswith("@bipagents.ics")]
    assert len(put) == 1 and "SUMMARY:Banque" in put[0] and "DTSTART:20261030T090000Z" in put[0]
    assert bridge.get("/v1/calendar/queue", headers=AUTH).json()["queue"] == []   # not the phone's

    off = bridge.patch(f"/v1/mail/accounts/{account}", headers=AUTH, json={"calendar": False})
    assert off.status_code == 200 and "calendar" not in off.json()
    give_token(bridge, "coach", TOKEN)
    assert "calendar_list" not in names(bridge)
