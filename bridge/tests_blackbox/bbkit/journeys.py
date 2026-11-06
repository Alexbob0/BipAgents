"""The release journeys' bridge (docs/release-journeys.md): ONE real bridge (the release build, `BRIDGE_CMD`) over the
fake Hermes, whose runs are answered by the scripted model (`bbkit/model.py`), with what a person's install has: agent
« Vie » (`vie`) served by the hub, its tools isolated in Hermes' sandbox (so its sites and connectors cards exist), its
own files served (`[media] roots`), its tools to the bridge open (pages, maps, secrets, sites, connectors).

Used by the black-box journeys (`test_journeys.py`, the apps' HTTP calls) and by `serve_journeys.py` (the same bridge
for the iPhone's and Android's journeys, which drive the real apps against it).

    journey = journey_bridge(bridges, fakes)
    journey.bridge, journey.model, journey.thread (vie's main thread)
"""
from __future__ import annotations

import base64
import hashlib
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional

from .common import AUTH
from .model import ScriptedModel, picture
from .servers import free_port, wait_until

AGENT = "vie"
FILES_ROOT = "/home/hermes/.hermes/fichiers"


@dataclass
class Journey:
    bridge: Any
    fakes: Any
    model: ScriptedModel
    thread: str
    token: str

    # -- as the apps do it ---------------------------------------------------------------------------------------
    def page(self, thread: Optional[str] = None, **params: Any) -> Dict[str, Any]:
        resp = self.bridge.get(f"/v1/threads/{thread or self.thread}/timeline", headers=AUTH,
                               params={"agent": AGENT, "tz": "Europe/Paris", **params})
        assert resp.status_code == 200, resp.text
        return resp.json()

    def upload(self, name: str, data: bytes, mime: str) -> Dict[str, Any]:
        """`POST /v1/files` as the apps store a document before sending it: its path, name, mime and size."""
        resp = self.bridge.post("/v1/files", headers=AUTH, data={"agent": AGENT},
                                files={"file": (name, data, mime)})
        assert resp.status_code in (200, 201), resp.text
        return resp.json()

    def send(self, client_id: str, text: str = "", photos: List[bytes] = (), documents: List[Dict[str, Any]] = (),
             thread: Optional[str] = None, **extra: Any):
        attachments = [{"kind": "image", "name": f"photo-{n + 1}.png", "mime": "image/png",
                        "data": base64.b64encode(p).decode()} for n, p in enumerate(photos)]
        attachments += [{"kind": "document", **d} for d in documents]
        body = {"agent": AGENT, "client_id": client_id, "text": text, **extra}
        if attachments:
            body["attachments"] = attachments
        return self.bridge.post(f"/v1/threads/{thread or self.thread}/messages", headers=AUTH, json=body)

    def run_events(self, run_id: str, timeout: float = 120) -> List[tuple]:
        from .common import read_sse
        with self.bridge.stream("GET", f"/v1/runs/{run_id}/events", headers=AUTH, params={"agent": AGENT},
                                timeout=timeout) as resp:
            assert resp.status_code == 200, resp.read()
            return read_sse(resp, timeout=timeout)

    def idle(self, timeout: float = 60) -> Dict[str, Any]:
        """The thread's page once no turn runs in it (the run's end seen by the bridge)."""
        assert wait_until(lambda: self.page()["pending"].get("run") is None, timeout=timeout), self.page()["pending"]
        return self.page()

    def pairing(self) -> Dict[str, Any]:
        """The install's QR code (`bipbridge qr --json`): what a phone scans to add every agent."""
        made = self.bridge.cli("qr", "--json")
        assert made.returncode == 0, made.stderr
        return json.loads(made.stdout)

    def mcp(self, path: str, name: str, arguments: Dict[str, Any], timeout: float = 1800) -> str:
        body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": name, "arguments": arguments}}
        resp = self.bridge.post(path, json=body, headers={"Authorization": f"Bearer {self.token}"}, timeout=timeout)
        assert resp.status_code == 200, resp.text
        result = resp.json()["result"]
        text = "\n".join(p.get("text", "") for p in result.get("content", []))
        if result.get("isError"):
            raise RuntimeError(text)
        return text


def photo(seed: int = 1) -> bytes:
    """A photo as the apps send one: a real picture (PNG, 256 × 256), a different colour for each seed."""
    from .fakes import tiny_png
    return tiny_png(256, 256, ((seed * 70) % 256, (120 + seed * 40) % 256, (200 - seed * 30) % 256))


def journey_bridge(bridges, fakes, word_gap: float = 0.06, name: str = "journeys") -> Journey:
    from test_connectors import IMAGE, SERVER
    from test_sandbox import TOKENS, isolated, started
    from conftest import profile

    fakes.hermes.v0225 = True
    bridge, hub, _ = isolated(bridges, fakes, agents=(AGENT,))
    bridge.name = name
    agent = bridge.config["agents"][AGENT]
    agent.update({"display_name": "Vie", "isolated": True, "hermes_home": "/home/hermes/.hermes",
                  "python": f"{bridge.fakebin}/podman exec -i hermes-vie python"})
    bridge.config["hermes"]["isolated"] = True
    bridge.commands.rule("podman", ["exec", "-i", "hermes-vie"], stdout=json.dumps({"id": "x"}) + "\n")
    bridge.commands.rule("podman", ["run", "--rm", "-i"], exec_image=IMAGE)
    work = profile(hub, AGENT) / "sandboxes" / "docker" / "default" / "workspace" / "connecteurs" / "garmin"
    work.mkdir(parents=True, exist_ok=True)
    (work / "server.py").write_text(SERVER)
    files = bridge.root / "fichiers"
    files.mkdir(parents=True, exist_ok=True)
    bridge.config["media"] = {"roots": {FILES_ROOT: str(files)}}
    bridge.config["limits"] = {"upload_max_mb": 20, "purge_interval_hours": 1000}
    # its door (HTTPS, a certificate the apps pin), as an install's QR code gives it: the iPhone pairs there only
    door = free_port()
    bridge.config["lan"] = {"enabled": True, "address": "127.0.0.1", "host": "127.0.0.1", "port": door}
    bridge.config["public_url"] = f"https://127.0.0.1:{door}"
    started(bridge, hub, [AGENT])
    token = TOKENS[AGENT]
    path = bridge.state_dir / "agent-tools.json"
    tokens = json.loads(path.read_text())["tokens"]
    tokens[AGENT] = hashlib.sha256(token.encode()).hexdigest()
    path.write_text(json.dumps({"tokens": tokens}))
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": AGENT})
    assert main.status_code == 200, main.text
    journey = Journey(bridge, fakes, None, main.json()["id"], token)   # type: ignore[arg-type]

    def page(run: str) -> str:
        html = (f"<h1>Résumé de la journée</h1><p>Trois rendez-vous, deux trajets, une course.</p>"
                f"<p>Page {run}.</p>")
        return journey.mcp("/v1/agent-tools/mcp", "create_page", {"html": html, "title": f"Résumé {run}"})

    def map_(run: str) -> str:
        try:
            return journey.mcp("/v1/agent-tools/mcp", "map_image",
                               {"markers": [{"place": "Tour Eiffel"}, {"place": "Louvre"}], "area": "Paris",
                                "title": "Paris en deux étapes"})
        except Exception:  # noqa: BLE001 - no geocoder: a picture in the agent's maps, as the tool keeps one
            shown = picture(bridge.data / "agent-maps" / AGENT / f"carte-{run}.png")
            return f"MEDIA:/bipagents/maps/{AGENT}/{shown.name}"

    def secret(words: str) -> str:
        return journey.mcp("/v1/agent-tools/connectors", "ask_secret",
                           {"name": "exemple", "site": "exemple.com", "kind": "password",
                            "reason": "Pour te connecter à ton compte"})

    def sites(words: str) -> str:
        return journey.mcp("/v1/agent-tools/files", "sandbox_sites_request",
                           {"host": "connect.garmin.example", "reason": "le connecteur Garmin"})

    def connector(words: str) -> str:
        from test_connectors import REGISTER
        return journey.mcp("/v1/agent-tools/connectors", "connector_register", REGISTER)

    journey.model = ScriptedModel(fakes.hermes, files, FILES_ROOT, word_gap=word_gap,
                                  calls={"page": page, "map": map_, "secret": secret, "sites": sites,
                                         "connector": connector}).install()
    return journey


def media_dir(journey: Journey) -> Path:
    return Path(journey.bridge.config["media"]["roots"][FILES_ROOT])
