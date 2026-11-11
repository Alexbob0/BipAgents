"""The local-network door (HTTPS with a self-signed certificate the app pins) and the pairing payload (tests/test_lan.py,
over HTTP and HTTPS). No relay to the agents' Hermes any more: the apps talk to the bridge only."""
import hashlib
import json
import os
import ssl
import stat

import httpx

from bbkit.common import AUTH, BRIDGE_KEY, HERMES_KEY
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
        hermes = door.get("/hermes/wellness/v1/capabilities", headers={"Authorization": f"Bearer {HERMES_KEY}"})
        assert hermes.status_code == 404
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
    assert payload["agent"] == "wellness" and "apiKey" not in payload and "baseURL" not in payload
    assert payload["bridgeKey"] == BRIDGE_KEY
    assert payload["bridgeURL"] == "https://box.example.ts.net:8643"
    assert payload["lan"] == {"url": f"https://192.168.1.177:{port}", "fingerprint": served_fingerprint(port)}
    install = json.loads(bridge.cli("qr", "--json").stdout)
    assert install == {"v": 2, "bridgeURL": "https://box.example.ts.net:8643", "bridgeKey": BRIDGE_KEY,
                       "lan": payload["lan"]}
    bridge.config["agents"]["Wellness"].pop("public_url")   # the agent's own Hermes address is not needed any more
    bridge.write_config()
    assert bridge.cli("qr", "--json", "--agent", "wellness").returncode == 0


def test_lan_off_by_default(bridge):
    assert bridge.get("/v1/pairing", headers=AUTH).json() == {"lan": None}
