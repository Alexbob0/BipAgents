"""« Se connecter avec… » (`/v1/connections…`, `/oauth/callback`): a service's own sign-in (OAuth 2.1 with
discovery, dynamic client registration and PKCE), the keys kept by the bridge, the access token handed to the chosen
agents' Hermes files and renewed by the bridge. The service is a fake here (its sign-in server and its MCP address),
started by each test on a port of its own; `[connections] urls` points Notion at it."""
import base64
import hashlib
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlencode, urlsplit

import httpx

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered


class FakeService:
    """Protected resource metadata, authorization server metadata, /register, /authorize, /token, /revoke."""

    def __init__(self):
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), self._handler())
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        self.registered, self.authorized, self.tokens, self.revoked = [], [], [], []
        self.codes = {}
        self.expires_in = 3600
        self.refuse_refresh = False
        self.refresh_token = None
        self.generation = 0
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()

    def issue(self):
        self.generation += 1
        self.refresh_token = f"rt-{self.generation}"
        return {"access_token": f"at-{self.generation}", "token_type": "bearer", "expires_in": self.expires_in,
                "refresh_token": self.refresh_token, "workspace_name": "Maison Martin"}

    def _handler(self):
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def send(self, status, body=None, headers=()):
                data = json.dumps(body).encode() if body is not None else b""
                self.send_response(status)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(data)))
                for k, v in headers:
                    self.send_header(k, v)
                self.end_headers()
                self.wfile.write(data)

            def body(self):
                raw = self.rfile.read(int(self.headers.get("content-length") or 0)).decode()
                if "json" in (self.headers.get("content-type") or ""):
                    return json.loads(raw)
                return {k: v[0] for k, v in parse_qs(raw).items()}

            def do_GET(self):
                url = urlsplit(self.path)
                if url.path == "/.well-known/oauth-protected-resource/mcp":
                    return self.send(200, {"resource": f"{fake.base}/mcp", "authorization_servers": [fake.base]})
                if url.path == "/.well-known/oauth-authorization-server":
                    return self.send(200, {"issuer": fake.base, "authorization_endpoint": f"{fake.base}/authorize",
                                           "token_endpoint": f"{fake.base}/token",
                                           "registration_endpoint": f"{fake.base}/register",
                                           "revocation_endpoint": f"{fake.base}/revoke",
                                           "code_challenge_methods_supported": ["S256"]})
                if url.path == "/authorize":   # the person signs in and taps « Autoriser »
                    q = {k: v[0] for k, v in parse_qs(url.query).items()}
                    fake.authorized.append(q)
                    code = f"code-{len(fake.authorized)}"
                    fake.codes[code] = q
                    back = q["redirect_uri"] + "?" + urlencode({"code": code, "state": q["state"], "iss": fake.base})
                    return self.send(302, None, [("location", back)])
                self.send(404, {"error": "not_found"})

            def do_POST(self):
                body = self.body()
                if self.path == "/register":
                    fake.registered.append(body)
                    return self.send(201, {"client_id": f"client-{len(fake.registered)}",
                                           "redirect_uris": body["redirect_uris"],
                                           "token_endpoint_auth_method": "none"})
                if self.path == "/token":
                    fake.tokens.append(body)
                    if body.get("grant_type") == "authorization_code":
                        asked = fake.codes.pop(body.get("code"), None)
                        challenge = base64.urlsafe_b64encode(
                            hashlib.sha256(body.get("code_verifier", "").encode()).digest()).rstrip(b"=").decode()
                        if (not asked or asked["code_challenge"] != challenge
                                or asked["redirect_uri"] != body.get("redirect_uri")
                                or asked["client_id"] != body.get("client_id")):
                            return self.send(400, {"error": "invalid_grant"})
                        return self.send(200, fake.issue())
                    if body.get("grant_type") == "refresh_token":
                        if fake.refuse_refresh or body.get("refresh_token") != fake.refresh_token:
                            return self.send(400, {"error": "invalid_grant"})
                        return self.send(200, fake.issue())
                    return self.send(400, {"error": "unsupported_grant_type"})
                if self.path == "/revoke":
                    fake.revoked.append(body)
                    return self.send(200, {})
                self.send(404, {"error": "not_found"})

        return Handler


def install(bridges, fakes, service, refresh_seconds=60):
    """maison (its own protected space: may hold keys) and coach (the shared hub: may not)."""
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("maison", "coach"), isolated=True)
    folder = profile(hub, "maison")
    profile(hub, "coach")
    bridge.config["agents"]["maison"].update(isolated=True, python=["/opt/py"], hermes_home=str(folder))
    bridge.config["connections"] = {"urls": {"notion": f"{service.base}/mcp"}, "refresh_seconds": refresh_seconds}
    bridge.start()
    wait_registered(bridge, ["maison", "coach"])
    return bridge, folder


def sign_in(bridge, service, **body):
    started = bridge.post("/v1/connections/notion/start", headers=AUTH, json={"agents": ["maison"], **body})
    assert started.status_code == 200, started.text
    page = httpx.get(started.json()["authorize_url"], follow_redirects=False)   # the phone's browser
    assert page.status_code == 302
    return started.json(), page.headers["location"]


def test_the_catalog_lists_them_apart_from_the_services(bridge):
    catalog = bridge.get("/v1/tool-catalog", headers=AUTH).json()
    assert [s["id"] for s in catalog["services"]] == ["home_assistant", "github", "custom"]
    connections = catalog["connections"]
    assert [c["id"] for c in connections] == ["notion", "finary"]
    assert set(connections[0]) == {"id", "label", "description", "icon", "read_only", "texts"}
    assert set(connections[1]["texts"]) == {"fr", "en", "de", "es"}
    assert connections[1]["read_only"] is True and connections[0]["read_only"] is False
    listing = bridge.get("/v1/connections", headers=AUTH).json()["connections"]
    assert [(c["service"], c["state"], c["connected"], c["agents"]) for c in listing] == [
        ("notion", "not_connected", False, []), ("finary", "not_connected", False, [])]
    assert bridge.get("/v1/connections").status_code == 401


def test_notion_signs_in_on_the_phone_and_the_agent_gets_only_an_access_token(bridges, fakes):
    service = FakeService()
    try:
        bridge, folder = install(bridges, fakes, service)
        bridge.commands.clear()
        started, back = sign_in(bridge, service)
        assert started["redirect_uri"] == "bipagents://oauth/callback" and started["expires_in"] == 900
        asked = service.authorized[0]
        assert asked["code_challenge_method"] == "S256" and asked["resource"] == f"{service.base}/mcp"
        assert asked["state"] == started["state"] and asked["redirect_uri"] == "bipagents://oauth/callback"
        assert service.registered[0]["redirect_uris"] == ["bipagents://oauth/callback"]
        assert service.registered[0]["token_endpoint_auth_method"] == "none"
        assert back.startswith("bipagents://oauth/callback?code=")

        done = bridge.post("/v1/connections/callback", headers=AUTH, json={"url": back})
        assert done.status_code == 200, done.text
        connection = done.json()
        assert connection["service"] == "notion" and connection["state"] == "connected"
        assert connection["account"] == "Maison Martin" and connection["agents"] == ["maison"]
        assert "at-1" not in done.text and "rt-1" not in done.text and "client-1" not in done.text
        # The same answer twice: the sign-in it belonged to is over.
        again = bridge.post("/v1/connections/callback", headers=AUTH, json={"url": back})
        assert again.status_code == 400 and again.json()["detail"]["code"] == "unknown_state"

        # Hermes' own OAuth files, the access token only; the server entry asks before each write.
        tokens = json.loads((folder / "mcp-tokens" / "notion.json").read_text())
        assert tokens["access_token"] == "at-1" and "refresh_token" not in tokens and tokens["expires_at"] > 0
        assert json.loads((folder / "mcp-tokens" / "notion.client.json").read_text())["client_id"] == "client-1"
        assert oct((folder / "mcp-tokens" / "notion.json").stat().st_mode & 0o777) == "0o600"
        commands = bridge.commands.of("hermes")
        for key, value in (("url", f"{service.base}/mcp"), ("auth", "oauth"), ("trust", "untrusted")):
            assert ["-p", "maison", "config", "set", f"mcp_servers.notion.{key}", value] in commands
        assert not any(c[:2] == ["-p", "coach"] for c in commands)
        assert all("at-1" not in " ".join(c) and "rt-1" not in " ".join(c) for c in commands)
        # The bridge's copy: encrypted.
        kept = (bridge.state_dir / "connections" / "connections.json.enc").read_text()
        assert "rt-1" not in kept and "at-1" not in kept
        listing = bridge.get("/v1/connections", headers=AUTH).json()["connections"]
        assert listing[0]["connected"] is True and listing[0]["account"] == "Maison Martin"

        # Disconnected: revoked at the service, gone from the profile.
        bridge.commands.clear()
        gone = bridge.delete("/v1/connections/notion", headers=AUTH)
        assert gone.status_code == 200 and gone.json() == {"ok": True, "revoked": True}
        assert {r["token"] for r in service.revoked} == {"at-1", "rt-1"}
        assert bridge.commands.of("hermes") == [["-p", "maison", "config", "unset", "mcp_servers.notion"]]
        assert not (folder / "mcp-tokens" / "notion.json").exists()
        assert bridge.get("/v1/connections", headers=AUTH).json()["connections"][0]["state"] == "not_connected"
        assert bridge.delete("/v1/connections/notion", headers=AUTH).status_code == 404
    finally:
        service.close()


def test_the_bridge_renews_the_access_token_and_says_when_the_service_refuses(bridges, fakes):
    service = FakeService()
    service.expires_in = 30   # under the renewal margin: renewed at the next look (every second here)
    try:
        bridge, folder = install(bridges, fakes, service, refresh_seconds=1)
        _, back = sign_in(bridge, service)
        assert bridge.post("/v1/connections/callback", headers=AUTH, json={"url": back}).status_code == 200
        path = folder / "mcp-tokens" / "notion.json"
        assert wait_until(lambda: json.loads(path.read_text())["access_token"] == "at-2", timeout=10)
        renewal = next(t for t in service.tokens if t["grant_type"] == "refresh_token")
        assert renewal["refresh_token"] == "rt-1" and renewal["client_id"] == "client-1"
        assert "refresh_token" not in json.loads(path.read_text())
        service.refuse_refresh = True

        def expired():
            return bridge.get("/v1/connections", headers=AUTH).json()["connections"][0]["state"] == "expired"
        assert wait_until(expired, timeout=10)
    finally:
        service.close()


def test_what_is_refused(bridges, fakes):
    service = FakeService()
    try:
        bridge, folder = install(bridges, fakes, service)
        refused = bridge.post("/v1/connections/notion/start", headers=AUTH, json={"agents": ["coach"]})
        assert refused.status_code == 409
        assert refused.json() == {"detail": {"code": "cannot_hold", "agents": {"coach": "needs_own_space"}}}
        assert bridge.post("/v1/connections/drive/start", headers=AUTH, json={"agents": ["maison"]}).status_code == 404
        assert bridge.post("/v1/connections/notion/start", headers=AUTH, json={"agents": []}).status_code == 422
        assert bridge.post("/v1/connections/notion/start", headers=AUTH,
                           json={"agents": ["nobody"]}).status_code == 404
        # A server of that name set up by hand stays the person's.
        (folder / "config.yaml").write_text("mcp_servers:\n  notion:\n    url: https://example.org/mcp\n")
        taken = bridge.post("/v1/connections/notion/start", headers=AUTH, json={"agents": ["maison"]})
        assert taken.status_code == 409 and taken.json()["detail"] == {"code": "name_taken", "agents": ["maison"]}
        (folder / "config.yaml").write_text("")
        # The person said no at the service.
        started = bridge.post("/v1/connections/notion/start", headers=AUTH, json={"agents": ["maison"]}).json()
        denied = bridge.post("/v1/connections/callback", headers=AUTH, json={
            "url": "bipagents://oauth/callback?" + urlencode({"error": "access_denied", "state": started["state"]})})
        assert denied.status_code == 400 and denied.json()["detail"]["code"] == "denied"
        # An answer naming another sign-in server.
        _, back = sign_in(bridge, service)
        forged = back.replace(urlencode({"iss": service.base}), urlencode({"iss": "https://evil.example"}))
        wrong = bridge.post("/v1/connections/callback", headers=AUTH, json={"url": forged})
        assert wrong.status_code == 400 and wrong.json()["detail"]["code"] == "wrong_issuer"
        assert not (folder / "mcp-tokens" / "notion.json").exists()
        # One registration for all these sign-ins (same redirect).
        assert len(service.registered) == 1
    finally:
        service.close()


def test_the_way_back_through_the_bridge_and_choosing_the_agents_later(bridges, fakes):
    service = FakeService()
    try:
        bridge, folder = install(bridges, fakes, service)
        started, back = sign_in(bridge, service, return_to="bridge")
        assert started["redirect_uri"] == f"{bridge.url}/oauth/callback"
        assert back.startswith(f"{bridge.url}/oauth/callback?")
        answer = httpx.get(back, follow_redirects=False)                  # no bridge key: the state is the proof
        assert answer.status_code == 302
        assert answer.headers["location"] == "bipagents://oauth/done?service=notion&status=connected"
        assert (folder / "mcp-tokens" / "notion.json").exists()
        assert httpx.get(back, follow_redirects=False).status_code == 400   # once only
        # No agent any more: the connection stays, the profile is cleaned.
        bridge.commands.clear()
        none = bridge.put("/v1/connections/notion/agents", headers=AUTH, json={"agents": []})
        assert none.status_code == 200 and none.json()["agents"] == [] and none.json()["connected"] is True
        assert bridge.commands.of("hermes") == [["-p", "maison", "config", "unset", "mcp_servers.notion"]]
        assert not (folder / "mcp-tokens" / "notion.json").exists()
        back_in = bridge.put("/v1/connections/notion/agents", headers=AUTH, json={"agents": ["maison"]})
        assert back_in.json()["agents"] == ["maison"]
        assert json.loads((folder / "mcp-tokens" / "notion.json").read_text())["access_token"] == "at-1"
        assert bridge.put("/v1/connections/finary/agents", headers=AUTH, json={"agents": []}).status_code == 404
    finally:
        service.close()
