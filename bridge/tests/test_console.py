"""The web console: logging in by a one-time link or a code approved in the app, its session standing for the bridge
key, and Hermes keys never reaching the browser."""
from conftest import AUTH


def login(client):
    code = client.app.state.console.new_link()
    resp = client.post("/console/api/login", json={"code": code, "name": "PC du salon"})
    assert resp.status_code == 200 and "bip_console" in resp.cookies
    return code


def test_a_one_time_link_opens_a_session_once(client):
    client.base_url = "https://testserver"   # the cookie is Secure
    code = login(client)
    assert client.post("/console/api/login", json={"code": code}).status_code == 401   # used up
    assert client.post("/console/api/login", json={"code": "made-up"}).status_code == 401
    console = {"X-Bip-Console": "1"}
    assert client.get("/console/api/me", headers=console).json() == {"logged_in": True}
    agents = client.get("/v1/agents", headers=console)
    assert agents.status_code == 200 and all(a["key"] is None for a in agents.json()["agents"])
    # Without the header (a page from elsewhere), the cookie alone is not enough.
    assert client.get("/v1/agents").status_code == 401
    # The app lists it and can revoke it.
    devices = client.get("/v1/console/devices", headers=AUTH).json()["devices"]
    assert [d["name"] for d in devices] == ["PC du salon"]
    assert client.delete(f"/v1/console/devices/{devices[0]['id']}", headers=AUTH).status_code == 204
    assert client.get("/v1/agents", headers=console).status_code == 401


def test_a_code_approved_in_the_app_opens_a_session(client):
    client.base_url = "https://testserver"
    pairing = client.post("/console/api/pairing").json()
    assert len(pairing["code"]) == 6
    assert client.post("/console/api/pairing/wait", json=pairing).json() == {"ok": False}   # not yet
    assert client.post(f"/v1/console/pairings/{pairing['code']}", json={"name": "Mac"}).status_code == 401
    assert client.post(f"/v1/console/pairings/{pairing['code']}", headers=AUTH, json={"name": "Mac"}).status_code == 200
    wrong = dict(pairing, wait="guess")
    assert client.post("/console/api/pairing/wait", json=wrong).json() == {"ok": False}
    assert client.post("/console/api/pairing/wait", json=pairing).json() == {"ok": True}
    assert client.get("/v1/agents", headers={"X-Bip-Console": "1"}).status_code == 200


def test_the_console_reaches_hermes_without_its_key(client):
    client.base_url = "https://testserver"
    login(client)
    # The fake Hermes answers 401 to a wrong key: the bridge put the agent's own.
    reached = client.get("/hermes/wellness/api/sessions", headers={"X-Bip-Console": "1"})
    assert reached.status_code != 401
    assert client.get("/hermes/wellness/api/sessions").status_code == 401   # no session, no key: Hermes refuses


def test_the_page_runs_only_its_own_scripts(client):
    page = client.get("/console/")
    assert page.status_code == 200 and 'src="/console/static/app.js"' in page.text
    csp = page.headers["content-security-policy"]
    assert "script-src 'self'" in csp and "frame-ancestors 'none'" in csp and "unsafe" not in csp
    for name in ("app.js", "api.js", "markdown.js", "bips.js", "text.js", "style.css", "vendor/preact-htm.js"):
        assert client.get(f"/console/static/{name}").status_code == 200, name
    assert client.get("/console", follow_redirects=False).headers["location"] == "/console/"


def test_the_link_command_prints_a_working_link(tmp_path, capsys):
    import os
    import re
    from bipbridge.__main__ import main
    from bipbridge.console import ConsoleStore
    config = tmp_path / "bridge.toml"
    config.write_text(f"""bridge_key = "{'k' * 40}"
public_url = "https://box.tailnet.ts.net:8643"
[pocket]
url = "http://127.0.0.1:1"
[outbox]
db_path = "{tmp_path}/bridge.db"
audio_dir = "{tmp_path}/audio"
[agents.wellness]
hermes_url = "http://127.0.0.1:2"
hermes_key = "x"
""")
    os.chmod(config, 0o600)
    assert main(["console", "--config", str(config)]) == 0
    code = re.search(r"https://box\.tailnet\.ts\.net:8643/console/#([\w-]+)", capsys.readouterr().out).group(1)
    assert ConsoleStore(str(tmp_path / "console.json")).use_link(code)
