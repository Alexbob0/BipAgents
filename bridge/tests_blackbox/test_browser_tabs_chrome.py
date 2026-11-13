"""The agent's browser keeps its site when its first tab is closed: a real container of the browser image (Chrome 157,
its MCP server, its DevTools relay), started with the flags the bridge gives it, and the bridge's page view of it.

13/11, seen by Alex on aibox and on an offer's place: an agent opened a site in a new tab (a login, a link opening a
tab), its first tab was closed, Hermes restarted its MCP servers, and every browser call failed for ten minutes with
« Browser window not found »; « Bureau » showed a tab « null » at chrome://omnibox-popup.top-chrome/. Chrome 157 lists
its address bar's suggestion popup as a page with no window, and the browser's server took it for the agent's tab.

Needs podman or docker and the browser image (BIP_BROWSER_IMAGE, default `bipagents-browser`, built from browser/):
skipped without them. Nothing leaves the container: the site is served in it, on its own loopback."""
import json
import os
import shutil
import subprocess
import time
import uuid

import pytest

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from test_desktop_page import browser_bridge

IMAGE = os.environ.get("BIP_BROWSER_IMAGE", "bipagents-browser")
ENGINE = next((e for e in ("podman", "docker") if shutil.which(e)), None)


def _image_present() -> bool:
    if ENGINE is None:
        return False
    return subprocess.run([ENGINE, "image", "inspect", IMAGE], capture_output=True).returncode == 0


pytestmark = pytest.mark.skipif(not _image_present(), reason=f"no container engine or no {IMAGE} image")

SITE = "http://127.0.0.1:8765/"
PAGES = {"login.html": "<title>Connexion</title><h1>Se connecter</h1>",
         "account.html": "<title>Compte</title><h1>Mon compte</h1>"}
# The flags the bridge gives the container (bridge-rs/src/browsers.rs `run_args`): its own --disable-features last.
BRIDGE_ARGS = ("--start-maximized;--no-first-run;--no-default-browser-check;--disable-dev-shm-usage;"
               "--ignore-gpu-blocklist;--test-type;--hide-crash-restore-bubble;--disable-back-forward-cache;"
               "--accept-lang=fr-FR,fr;--force-webrtc-ip-handling-policy=disable_non_proxied_udp;"
               "--disable-features=IsolateOrigins,site-per-process,OptimizationGuideModelDownloading,"
               "OptimizationHintsFetching,OptimizationTargetPrediction,OptimizationHints")


class Server:
    """The browser's MCP server over stdio, as Hermes starts it (`<engine> exec -i <container> /opt/stealth/mcp.sh`)."""

    def __init__(self, container: str):
        self.process = subprocess.Popen([ENGINE, "exec", "-i", container, "/opt/stealth/mcp.sh"],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        self.next = 0
        self.request("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                    "clientInfo": {"name": "test", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message: dict) -> None:
        self.process.stdin.write((json.dumps(message) + "\n").encode())
        self.process.stdin.flush()

    def request(self, method: str, params: dict) -> dict:
        self.next += 1
        self.send({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params})
        while True:
            line = self.process.stdout.readline()
            assert line, "the server ended"
            message = json.loads(line)
            if message.get("id") == self.next:
                return message

    def call(self, tool: str, **arguments):
        answer = self.request("tools/call", {"name": tool, "arguments": arguments})
        result = answer["result"]
        assert not result.get("isError"), result
        if "structuredContent" in result:
            value = result["structuredContent"]
            return value.get("result", value) if isinstance(value, dict) and set(value) == {"result"} else value
        return json.loads(result["content"][0]["text"])

    def end(self) -> None:
        self.process.stdin.close()
        self.process.wait(timeout=30)


def in_container(container: str, *command: str) -> str:
    return subprocess.run([ENGINE, "exec", container, *command], capture_output=True, text=True).stdout


def chrome_pages(container: str) -> list:
    out = in_container(container, "python3.11", "-I", "-c",
                       "import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:9222/json/list').read().decode())")
    return [t for t in json.loads(out or "[]") if t["type"] == "page"]


@pytest.fixture
def browser_container():
    """The agent's container as the bridge runs it: its flags, the relay published on a port of 127.0.0.1."""
    name, cdp = f"bip-tabs-{uuid.uuid4().hex[:8]}", free_port()
    subprocess.run([ENGINE, "run", "-d", "--rm", "--name", name, "--shm-size", "1g", "-p", f"127.0.0.1:{cdp}:9223",
                    "-e", f"STEALTH_BROWSER_ARGS={BRIDGE_ARGS}", IMAGE], check=True, capture_output=True)
    try:
        assert wait_until(lambda: subprocess.run([ENGINE, "exec", name, "test", "-e", "/tmp/.X11-unix/X99"]).returncode == 0,
                          timeout=30)
        files = " && ".join(f"printf '%s' '{html}' > /tmp/site/{page}" for page, html in PAGES.items())
        subprocess.run([ENGINE, "exec", name, "sh", "-c", f"mkdir -p /tmp/site && {files}"], check=True)
        subprocess.run([ENGINE, "exec", "-d", name, "python3.11", "-m", "http.server", "8765", "--bind", "127.0.0.1",
                        "--directory", "/tmp/site"], check=True)
        yield name, cdp
    finally:
        subprocess.run([ENGINE, "rm", "-f", name], capture_output=True)


def test_the_agent_keeps_its_site_when_its_first_tab_is_closed(bridges, fakes, browser_container):
    container, cdp = browser_container
    first = Server(container)
    instance = first.call("spawn_browser")["instance_id"]
    start_tab = first.call("get_active_tab", instance_id=instance)["tab_id"]
    first.call("new_tab", instance_id=instance, url=SITE + "login.html")   # a login opened in a new tab…
    assert first.call("close_tab", instance_id=instance, tab_id=start_tab) is True   # …and the first tab closed
    first.end()   # Hermes restarts its MCP servers (a gateway restart, a reconnect)

    second = Server(container)
    assert second.call("spawn_browser")["instance_id"] == instance
    assert second.call("get_instance_state", instance_id=instance)["url"] == SITE + "login.html"
    assert second.call("navigate", instance_id=instance, url=SITE + "account.html")["title"] == "Compte"
    content = second.call("get_page_content", instance_id=instance)
    assert "Mon compte" in json.dumps(content, ensure_ascii=False)
    second.end()

    # Chrome as it runs: one --disable-features (Chrome reads only the last), its popup page off, the bridge's kept
    out = in_container(container, "pgrep", "-a", "-f", "--", "--remote-debugging-port=9222")
    main = [line.split() for line in out.splitlines() if "--type=" not in line]
    assert len(main) == 1, out
    switches = [a for a in main[0] if a.startswith("--disable-features=")]
    assert len(switches) == 1, switches
    assert {"WebUIOmniboxPopup", "OptimizationHints", "IsolateOrigins"} <= set(switches[0].split("=", 1)[1].split(","))
    assert [t["url"] for t in chrome_pages(container)] == [SITE + "account.html"]

    # « Bureau »: the bridge's view of that browser shows the site, never Chrome's own UI
    bridge, _ = browser_bridge(bridges, fakes, cdp)
    state = bridge.get("/v1/agents/vie/desktop/state", headers=AUTH).json()
    assert (state["state"], state["site"], state["title"]) == ("active", "127.0.0.1", "Compte"), state
    deadline = time.time() + 5
    while time.time() < deadline:   # still so a moment later (the popup page, when there is one, comes late)
        time.sleep(1)
        assert bridge.get("/v1/agents/vie/desktop/state", headers=AUTH).json()["title"] == "Compte"
