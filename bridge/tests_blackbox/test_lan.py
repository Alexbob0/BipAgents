"""The local-network door (HTTPS with a self-signed certificate the app pins), the pairing payload, and the agents'
Hermes through the bridge under /hermes/<agent>/ (tests/test_lan.py, over HTTP and HTTPS)."""
import hashlib
import json
import os
import ssl
import stat

import httpx

from bbkit.common import AUTH, BRIDGE_KEY, HERMES_KEY, REMOTE, sse
from bbkit.fakes import CLOSE
from bbkit.servers import free_port


def with_lan(bridges, fakes, **lan):
    def change(c):
        c["lan"] = {"enabled": True, "address": "192.168.1.177", "host": "127.0.0.1", "port": free_port(), **lan}
        c["public_url"] = "https://box.example.ts.net:8643"
        c["agents"]["Wellness"]["public_url"] = "https://box.example.ts.net:8642"
    return bridges.new(change=change)


def served_fingerprint(port):
    pem = ssl.get_server_certificate(("127.0.0.1", port))
    return hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest()


def test_the_door_serves_https_with_the_pinned_certificate(bridges, fakes):
    bridge = with_lan(bridges, fakes).start()
    port = bridge.config["lan"]["port"]
    cert_dir = bridge.data / "lan"
    key_path, cert_path = cert_dir / "bridge-lan.key", cert_dir / "bridge-lan.pem"
    assert stat.S_IMODE(os.stat(key_path).st_mode) == 0o600 and cert_path.exists()
    fingerprint = served_fingerprint(port)
    pairing = bridge.get("/v1/pairing", headers=AUTH)
    assert pairing.json() == {"lan": {"url": f"https://192.168.1.177:{port}", "fingerprint": fingerprint}}
    assert bridge.get("/v1/pairing").status_code == 401
    pem = cert_path.read_text()
    assert hashlib.sha256(ssl.PEM_cert_to_DER_cert(pem)).hexdigest() == fingerprint
    with httpx.Client(base_url=f"https://127.0.0.1:{port}", verify=False) as door:
        assert door.get("/health").json()["ok"] is True
        assert door.get("/v1/agents", headers=AUTH).json()["bridge"]["lan"]["fingerprint"] == fingerprint
    # Kept, not remade: the same certificate after a restart.
    bridge.restart()
    assert served_fingerprint(port) == fingerprint and cert_path.read_text() == pem
    listed = bridge.get("/v1/agents", headers=AUTH).json()
    assert listed["bridge"] == {"url": "https://box.example.ts.net:8643",
                                "lan": {"url": f"https://192.168.1.177:{port}", "fingerprint": fingerprint}}


def test_pairing_payload_carries_tailnet_addresses_keys_and_lan(bridges, fakes):
    bridge = with_lan(bridges, fakes).start()
    port = bridge.config["lan"]["port"]
    payload = json.loads(bridge.cli("qr", "--json", "--agent", "wellness").stdout)
    assert payload["baseURL"] == "https://box.example.ts.net:8642" and payload["agent"] == "wellness"
    assert payload["apiKey"] == HERMES_KEY and payload["bridgeKey"] == BRIDGE_KEY
    assert payload["bridgeURL"] == "https://box.example.ts.net:8643"
    assert payload["lan"] == {"url": f"https://192.168.1.177:{port}", "fingerprint": served_fingerprint(port)}
    install = json.loads(bridge.cli("qr", "--json").stdout)
    assert install == {"v": 2, "bridgeURL": "https://box.example.ts.net:8643", "bridgeKey": BRIDGE_KEY,
                       "lan": payload["lan"]}
    bridge.config["agents"]["Wellness"].pop("public_url")
    bridge.write_config()
    missing = bridge.cli("qr", "--json", "--agent", "wellness")
    assert missing.returncode == 2 and "public_url" in missing.stderr


def test_lan_off_by_default(bridge):
    assert bridge.get("/v1/pairing", headers=AUTH).json() == {"lan": None}


def test_pairing_route_and_hermes_relay(bridges, fakes):
    bridge = with_lan(bridges, fakes).start()
    hermes_auth = {"Authorization": f"Bearer {HERMES_KEY}"}
    posted = bridge.post("/hermes/wellness/v1/runs?x=1", json={"input": "Salut"}, headers=hermes_auth)
    assert posted.status_code == 202 and posted.json() == {"run_id": "r1", "status": "started"}
    seen = fakes.hermes.seen("/v1/runs", "POST")[-1]
    assert seen.path == "/v1/runs" and seen.query == "x=1" and seen.json() == {"input": "Salut"}
    assert seen.header("authorization") == f"Bearer {HERMES_KEY}"  # Hermes checks its own key
    fakes.hermes.runs["run_1"] = [sse("message.delta", delta="Hi"), CLOSE]
    events = bridge.get("/hermes/wellness/v1/runs/run_1/events", headers=hermes_auth)
    assert events.headers["content-type"].startswith("text/event-stream") and b"message.delta" in events.content
    assert events.content == sse("message.delta", delta="Hi").encode()
    wrong = bridge.get("/hermes/wellness/api/sessions", headers={"Authorization": "Bearer nope"})
    assert wrong.status_code == 401 and wrong.json() == {"error": "bad key"}   # Hermes' own answer, passed on
    assert bridge.get("/hermes/wellness/health").status_code == 200
    assert bridge.get("/hermes/nobody/v1/runs").status_code == 404
    assert bridge.get("/hermes/wellness/admin").status_code == 404  # only Hermes' API paths
    with httpx.Client(base_url=f"https://127.0.0.1:{bridge.config['lan']['port']}", verify=False) as door:
        through = door.get("/hermes/wellness/v1/capabilities", headers=hermes_auth)
        assert through.status_code == 200 and through.json() == {"runs": True}


def test_relay_only_reaches_known_agents_and_hermes_routes(bridge, fakes):
    # Always there (agents in their own space are reached through it), but nothing else than an agent's Hermes API.
    nobody = bridge.get("/hermes/nobody/v1/capabilities")
    assert nobody.status_code == 404 and nobody.json() == {"detail": "not found"}
    assert bridge.get("/hermes/wellness/etc/passwd").status_code == 404
    for method in ("PUT", "PATCH", "DELETE"):
        assert bridge.request(method, "/hermes/wellness/api/sessions/s1",
                              headers={"Authorization": f"Bearer {HERMES_KEY}"}).status_code in (200, 404)
    assert [r.method for r in fakes.hermes.seen("/api/sessions/s1")] == ["PUT", "PATCH", "DELETE"]


def test_hermes_down_through_the_door(bridges, fakes):
    bridge = bridges.new(change=lambda c: c["agents"]["Wellness"].update(hermes_url=f"http://127.0.0.1:{free_port()}"))
    bridge.start()
    resp = bridge.get("/hermes/wellness/v1/capabilities", headers={"Authorization": f"Bearer {HERMES_KEY}"})
    assert resp.status_code == 502 and resp.json() == {"detail": "Hermes unreachable"}


def test_wrong_agent_keys_through_the_door_get_the_address_blocked(bridge, fakes):
    remote = {"X-Forwarded-For": REMOTE, "Authorization": "Bearer guess"}
    for _ in range(20):
        assert bridge.get("/hermes/wellness/v1/capabilities", headers=remote).status_code == 401
    blocked = bridge.get("/hermes/wellness/v1/capabilities", headers={**remote, "Authorization": f"Bearer {HERMES_KEY}"})
    assert blocked.status_code == 429 and blocked.json() == {"detail": "too many wrong keys, try again later"}
    assert bridge.get("/v1/agents", headers={"X-Forwarded-For": REMOTE, **AUTH}).status_code == 429
    assert bridge.logged("wrong key")[0]["what"] == "agent key"


def test_the_relay_passes_headers_both_ways_but_not_the_connections_own(bridge, fakes):
    from starlette.responses import Response

    def custom(seen):
        if seen.path == "/api/sessions/s1/messages":
            return Response(b'{"messages": []}', media_type="application/json",
                            headers={"X-Hermes-Trace": "t-1", "Cache-Control": "no-cache"})
        return None

    fakes.hermes.override = custom
    resp = bridge.get("/hermes/wellness/api/sessions/s1/messages?inline_images=false",
                      headers={"Authorization": f"Bearer {HERMES_KEY}", "X-Client": "ios", "Accept-Language": "fr"})
    assert resp.status_code == 200 and resp.json() == {"messages": []}
    assert resp.headers["x-hermes-trace"] == "t-1" and resp.headers["cache-control"] == "no-cache"
    seen = fakes.hermes.seen("/api/sessions/s1/messages")[-1]
    assert seen.query == "inline_images=false" and seen.header("x-client") == "ios" and seen.header("accept-language") == "fr"
    assert seen.header("host") == f"127.0.0.1:{fakes.hermes.port}"          # its own, not the bridge's
    assert seen.header("authorization") == f"Bearer {HERMES_KEY}"          # the app's key, as it sent it
