"""The web console: logging in by a one-time link (`bipbridge console`) or a code approved in the app, its session
cookie standing for the bridge key, Hermes keys never reaching the browser (tests/test_console.py, over HTTP)."""
import json
import os
import re
import stat

from bbkit.common import AUTH, HERMES_KEY

CONSOLE = {"X-Bip-Console": "1"}


def set_cookie(resp):
    """The `bip_console` cookie the response sets: (value, {attribute: value}) with lower-case attribute names."""
    for header in resp.headers.get_list("set-cookie"):
        name, _, rest = header.partition("=")
        if name.strip() != "bip_console":
            continue
        value, *attrs = [part.strip() for part in rest.split(";")]
        parsed = {}
        for attr in attrs:
            key, _, val = attr.partition("=")
            parsed[key.strip().lower()] = val.strip()
        return value.strip('"'), parsed
    return None, {}


def link_code(bridge):
    out = bridge.cli("console")
    assert out.returncode == 0, out.stderr
    found = re.search(r"/console/#([\w-]+)", out.stdout)
    assert found, out.stdout
    return found.group(1), out.stdout


def login(bridge, name="PC du salon"):
    code, _ = link_code(bridge)
    resp = bridge.post("/console/api/login", json={"code": code, "name": name})
    assert resp.status_code == 200 and resp.json() == {"ok": True}
    token, attrs = set_cookie(resp)
    assert token
    return code, {"Cookie": f"bip_console={token}", **CONSOLE}, attrs


def console_bridge(bridges):
    return bridges.new(change=lambda c: c["agents"]["Wellness"].update(public_url="https://box.ts.net:8642")).start()


def test_a_one_time_link_opens_a_session_once(bridges):
    bridge = console_bridge(bridges)
    code, session, attrs = login(bridge)
    assert {"httponly", "secure", "path", "max-age", "samesite"} <= set(attrs)
    assert attrs["max-age"] == str(30 * 86400) and attrs["path"] == "/" and attrs["samesite"].lower() == "strict"
    again = bridge.post("/console/api/login", json={"code": code})
    assert again.status_code == 401
    assert again.json() == {"detail": "this link was used or has expired: ask for a new one"}
    assert bridge.post("/console/api/login", json={"code": "made-up"}).status_code == 401
    assert bridge.get("/console/api/me", headers=session).json() == {"logged_in": True}
    assert bridge.get("/console/api/me").json() == {"logged_in": False}
    agents = bridge.get("/v1/agents", headers=session)
    assert agents.status_code == 200 and all(a["key"] is None for a in agents.json()["agents"])
    assert bridge.get("/v1/agents", headers=AUTH).json()["agents"][0]["key"] == HERMES_KEY   # the app gets it
    # Without the header (a page from elsewhere), the cookie alone is not enough.
    assert bridge.get("/v1/agents", headers={"Cookie": session["Cookie"]}).status_code == 401
    # The app lists it and can revoke it.
    devices = bridge.get("/v1/console/devices", headers=AUTH).json()["devices"]
    assert [d["name"] for d in devices] == ["PC du salon"] and set(devices[0]) == {"id", "name", "created", "seen"}
    assert bridge.get("/v1/console/devices", headers=session).status_code == 200   # the console itself may look
    assert bridge.delete(f"/v1/console/devices/{devices[0]['id']}", headers=AUTH).status_code == 204
    assert bridge.delete(f"/v1/console/devices/{devices[0]['id']}", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents", headers=session).status_code == 401
    # Its file keeps digests only, readable by this account alone.
    path = bridge.state_dir / "console.json"
    assert stat.S_IMODE(os.stat(path).st_mode) == 0o600
    assert code not in path.read_text() and session["Cookie"].split("=", 1)[1] not in path.read_text()


def test_the_link_command_prints_a_working_link(bridges):
    bridge = bridges.new(change=lambda c: c.update(public_url="https://box.tailnet.ts.net:8643"))
    bridge.write_config()
    code, printed = link_code(bridge)
    assert f"https://box.tailnet.ts.net:8643/console/#{code}" in printed and "10 minutes" in printed
    data = json.loads((bridge.state_dir / "console.json").read_text())
    assert len(data["links"]) == 1                      # made before the bridge runs: it reads the file
    bridge.start()
    assert bridge.post("/console/api/login", json={"code": code}).status_code == 200
    local = bridges.new("local")
    local.write_config()
    assert f"http://127.0.0.1:{local.port}/console/#" in link_code(local)[1]


def test_a_code_approved_in_the_app_opens_a_session(bridge):
    pairing = bridge.post("/console/api/pairing").json()
    assert len(pairing["code"]) == 6 and pairing["code"].isdigit() and set(pairing) == {"code", "wait"}
    assert bridge.post("/console/api/pairing/wait", json=pairing).json() == {"ok": False}   # not yet
    assert bridge.post(f"/v1/console/pairings/{pairing['code']}", json={"name": "Mac"}).status_code == 401
    approved = bridge.post(f"/v1/console/pairings/{pairing['code']}", headers=AUTH, json={"name": "Mac"})
    assert approved.status_code == 200 and approved.json() == {"ok": True}
    twice = bridge.post(f"/v1/console/pairings/{pairing['code']}", headers=AUTH, json={"name": "Mac"})
    assert twice.status_code == 404 and twice.json() == {"detail": "no console waits with this code"}
    wrong = dict(pairing, wait="guess")
    assert bridge.post("/console/api/pairing/wait", json=wrong).json() == {"ok": False}
    done = bridge.post("/console/api/pairing/wait", json=pairing)
    assert done.json() == {"ok": True}
    token, _ = set_cookie(done)
    session = {"Cookie": f"bip_console={token}", **CONSOLE}
    assert bridge.get("/v1/agents", headers=session).status_code == 200
    assert [d["name"] for d in bridge.get("/v1/console/devices", headers=AUTH).json()["devices"]] == ["Mac"]
    assert bridge.post("/console/api/pairing/wait", json=pairing).json() == {"ok": False}    # used up
    assert bridge.post("/console/api/pairing/wait", json={"code": "1234567", "wait": "x"}).status_code == 422


def test_logging_out_ends_the_session(bridge):
    _, session, _ = login(bridge)
    out = bridge.post("/console/api/logout", headers=session)
    assert out.status_code == 200 and out.json() == {"ok": True}
    value, attrs = set_cookie(out)
    assert value == "" and attrs.get("max-age") == "0" and attrs.get("path") == "/"
    assert bridge.get("/v1/agents", headers=session).status_code == 401
    assert bridge.get("/v1/console/devices", headers=AUTH).json() == {"devices": []}


def test_the_console_reaches_hermes_without_its_key(bridge, fakes):
    _, session, _ = login(bridge)
    # The fake Hermes answers 401 to a wrong key: the bridge put the agent's own.
    reached = bridge.get("/hermes/wellness/api/sessions", headers=session)
    assert reached.status_code == 200
    seen = fakes.hermes.seen("/api/sessions")[-1]
    assert seen.header("authorization") == f"Bearer {HERMES_KEY}"
    assert seen.header("cookie") is None and seen.header("x-bip-console") is None
    assert bridge.get("/hermes/wellness/api/sessions").status_code == 401   # no session, no key: Hermes refuses


def test_the_page_runs_only_its_own_scripts(bridge):
    page = bridge.get("/console/")
    assert page.status_code == 200 and 'src="/console/static/app.js"' in page.text
    assert page.headers["content-type"].startswith("text/html")
    csp = page.headers["content-security-policy"]
    assert "script-src 'self'" in csp and "frame-ancestors 'none'" in csp and "unsafe" not in csp
    assert page.headers["x-content-type-options"] == "nosniff" and page.headers["referrer-policy"] == "no-referrer"
    assert page.headers["cache-control"] == "no-store"
    for name in ("app.js", "api.js", "markdown.js", "bips.js", "text.js", "style.css", "vendor/preact-htm.js"):
        assert bridge.get(f"/console/static/{name}").status_code == 200, name
    assert bridge.get("/console/static/app.js").headers["content-type"].startswith(("text/javascript",
                                                                                     "application/javascript"))
    assert bridge.get("/console/static/style.css").headers["content-type"].startswith("text/css")
    assert bridge.get("/console/static/nothing.js").status_code == 404
    assert bridge.get("/console/static/../console.py").status_code == 404
    redirect = bridge.get("/console")
    assert redirect.status_code == 307 and redirect.headers["location"] == "/console/"
