"""A file of the agent's own space handed to one of its services (bridge-rs/src/service_files.rs): what its visible
browser downloaded (`/telechargements`) or a file of `/envoi`, POSTed to `<the service's MCP address>/uploads` with
the service's own token, never through a link, never a hidden or secret-looking file, never another agent's, at most
10 MB, no redirect followed; the agent gets the upload id, the log a line without the token. And the login an agent
asks for its browser on a supplier's portal (`ask_secret` with `use: browser`): the same card, kept as a vault login
for that site only. Fictitious services, files and credentials."""
import hashlib
import json
import os
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import yaml

from bbkit.servers import wait_until
from conftest import profile
from test_sandbox import call, give_tokens, isolated, rpc, started

SERVICE_TOKEN = "fake-compta-token-0123456789abcdef"
PDF = b"%PDF-1.4\n% facture fictive 2026-10\n%%EOF\n"


class Service:
    """A fake MCP service that takes files: /mcp/uploads answers an upload id, /old/mcp/uploads does not exist,
    /moved/mcp/uploads redirects elsewhere (never followed)."""

    def __init__(self):
        self.requests = []
        outer = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("content-length", "0")))
                outer.requests.append({"path": self.path, "headers": {k.lower(): v for k, v in self.headers.items()},
                                       "body": body})
                if self.path == "/mcp/uploads":
                    data, status = {"upload": f"upl_{len(outer.requests):024x}"}, 201
                elif self.path == "/moved/mcp/uploads":
                    self.send_response(307)
                    self.send_header("location", f"http://127.0.0.1:{outer.port}/elsewhere")
                    self.end_headers()
                    return
                else:
                    data, status = {"error": "not found"}, 404
                raw = json.dumps(data).encode()
                self.send_response(status)
                self.send_header("content-type", "application/json")
                self.send_header("content-length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def url(self, prefix=""):
        return f"http://127.0.0.1:{self.port}{prefix}/mcp"

    def close(self):
        self.server.shutdown()


def with_services(hub, name, service):
    """The person added services to the agent (services.rs writes them so): compta takes files, ancien doesn't,
    demenage redirects; the token in the profile's .env only."""
    folder = profile(hub, name)
    path = folder / "config.yaml"
    data = yaml.safe_load(path.read_text()) or {}
    bearer = {"Authorization": "Bearer ${MCP_COMPTA_API_KEY}"}
    data["mcp_servers"] = {
        "compta": {"url": service.url(), "headers": bearer, "trust": "untrusted"},
        "ancien": {"url": service.url("/old"), "trust": "untrusted"},
        "demenage": {"url": service.url("/moved"), "headers": bearer, "trust": "untrusted"},
    }
    path.write_text(yaml.safe_dump(data))
    (folder / ".env").write_text(f"MCP_COMPTA_API_KEY={SERVICE_TOKEN}\n")
    os.chmod(folder / ".env", 0o600)


def test_a_downloaded_invoice_goes_to_the_service_and_nothing_else_can(bridges, fakes):
    service = Service()
    try:
        bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness"))
        browsers = bridge.data / "browsers"
        with_services(hub, "vie", service)
        downloads = browsers / "vie" / "profile" / "Downloads"
        downloads.mkdir(parents=True)
        (downloads / "facture-2026-10.pdf").write_bytes(PDF)
        (downloads / "suite.pdf.crdownload").write_bytes(b"%PDF partial")
        os.symlink("/etc/hosts", downloads / "hosts.pdf")
        (downloads / "gros.pdf").write_bytes(b"%PDF" + b"0" * (10 * 1024 * 1024))
        (browsers / "vie" / "profile" / "Cookies").write_bytes(b"session cookies")
        other = browsers / "wellness" / "profile" / "Downloads"
        other.mkdir(parents=True)
        (other / "bilan.pdf").write_bytes(b"%PDF wellness")
        (volumes[("envoi", "vie")] / "releve.csv").write_text("date;montant\n")
        (volumes[("envoi", "vie")] / ".env").write_text("SECRET=1\n")
        started(bridge, hub, ["vie", "wellness"])
        give_tokens(bridge, ["vie", "wellness"])

        listed = rpc(bridge, "vie", "tools/list").json()["result"]["tools"]
        tool = next(t for t in listed if t["name"] == "send_file_to_service")
        assert tool["annotations"]["readOnlyHint"] is False      # on the « untrusted » entry: the person approves

        text, failed = call(bridge, "vie", "send_file_to_service", service="compta",
                            path="/telechargements/facture-2026-10.pdf")
        assert not failed, text
        assert f"upload upl_{1:024x}" in text and "piece_add" in text
        sent = service.requests[0]
        assert sent["path"] == "/mcp/uploads" and sent["body"] == PDF
        assert sent["headers"]["authorization"] == f"Bearer {SERVICE_TOKEN}"
        assert sent["headers"]["x-filename"] == "facture%2D2026%2D10%2Epdf"
        assert sent["headers"]["x-sha256"] == hashlib.sha256(PDF).hexdigest()
        # A file it made, from /envoi.
        # Its downloads listed (its terminal doesn't see them): finished files only, never a link.
        text, failed = call(bridge, "vie", "send_file_to_service", service="compta", path="/telechargements/")
        assert not failed and "/telechargements/facture-2026-10.pdf" in text and "/telechargements/gros.pdf" in text
        assert "crdownload" not in text and "hosts.pdf" not in text
        text, failed = call(bridge, "vie", "send_file_to_service", service="compta", path="/envoi/releve.csv")
        assert not failed and service.requests[-1]["body"] == b"date;montant\n"

        before = len(service.requests)
        refused = [
            ("compta", "/telechargements/hosts.pdf"),                       # a link
            ("compta", "/telechargements/../profile/Cookies"),             # up the tree
            ("compta", "/telechargements/suite.pdf.crdownload/../../Cookies"),
            ("compta", "/etc/hosts"),                                       # not one of its places
            ("compta", "/envoi/.env"),                                      # secret-looking
            ("compta", "/telechargements/gros.pdf"),                        # over 10 MB
            ("compta", "/telechargements/inconnu.pdf"),
            ("bipagents", "/telechargements/facture-2026-10.pdf"),          # the bridge's own entry
            ("bipagents_files", "/telechargements/facture-2026-10.pdf"),
            ("nope", "/telechargements/facture-2026-10.pdf"),
        ]
        for name, path in refused:
            text, failed = call(bridge, "vie", "send_file_to_service", service=name, path=path)
            assert failed, (name, path, text)
        assert len(service.requests) == before
        text, _ = call(bridge, "vie", "send_file_to_service", service="compta", path="/telechargements/inconnu.pdf")
        assert "/telechargements/facture-2026-10.pdf" in text            # its latest downloads, to pick from
        text, _ = call(bridge, "vie", "send_file_to_service", service="nope", path="/envoi/releve.csv")
        assert "compta" in text and "bipagents" not in text
        # Another agent never reaches vie's downloads nor its services.
        text, failed = call(bridge, "wellness", "send_file_to_service", service="compta",
                            path="/telechargements/facture-2026-10.pdf")
        assert failed
        # A service that takes no files, one that redirects: said plainly, the token never follows a redirect.
        text, failed = call(bridge, "vie", "send_file_to_service", service="ancien",
                            path="/telechargements/facture-2026-10.pdf")
        assert failed and "does not take files" in text
        assert "authorization" not in service.requests[-1]["headers"]  # ancien has no token
        text, failed = call(bridge, "vie", "send_file_to_service", service="demenage",
                            path="/telechargements/facture-2026-10.pdf")
        assert failed and not any(r["path"] == "/elsewhere" for r in service.requests)

        # The audit line: who, which service, which file, never the token nor the content.
        log = bridge.log_tail(200000)
        assert "file sent to a service" in log and "facture-2026-10.pdf" in log
        assert SERVICE_TOKEN not in log and "facture fictive" not in log
    finally:
        service.close()


def test_a_portal_login_for_the_browser_is_asked_once_in_the_card_and_kept_for_that_site(bridges, fakes):
    from test_connectors import PASSWORD, ask, setup, text as text_of, vault_records  # noqa: PLC0415
    bridge, hub, _ = setup(bridges, fakes)
    result, card = ask(bridge, {"name": "hetzner", "site": "accounts.portail.example", "kind": "password",
                                "use": "browser", "reason": "Récupérer les factures du mois"},
                       {"value": PASSWORD, "identifier": "compta@example.org"})
    assert result["isError"] is False, result
    assert "browser_vault" in text_of(result) and PASSWORD not in json.dumps(result)
    records = vault_records(bridge)
    login = next(r for r in records if r.get("kind") == "login")
    assert login["origin"] == "https://accounts.portail.example" and login["identifier"] == "compta@example.org"
    assert list(login["agents"]) == ["vie"] and login["secret"]["password"] == PASSWORD
    assert not any(r.get("kind") == "secret" and r.get("label") == "hetzner" for r in records)
    assert PASSWORD not in bridge.log_tail(200000)
