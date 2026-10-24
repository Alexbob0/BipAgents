"""The agent's Chrome outlives its MCP server (python3 -m pytest browser/test_browser_survives.py).

Hermes restarts its MCP servers (a gateway restart, a stdio reconnect, a `hermes chat` that ends); before
stealth.patch started Chrome apart, the Chrome a server launched died with it, and the agent had to start its browser
again at every turn, its page lost. Here, in a real container of the browser image (BIP_BROWSER_IMAGE, default
`bipagents-browser`, built from this folder; podman or docker): a page opened through one server is still there, under
the same instance id, through the next one, whether the first ended cleanly or was killed; `close_instance` closes the
pages and leaves Chrome; only `quit_browser` quits it. Skipped where no engine or image is at hand.
"""
import json
import os
import shutil
import subprocess
import time
import uuid

import pytest

IMAGE = os.environ.get("BIP_BROWSER_IMAGE", "bipagents-browser")
ENGINE = next((e for e in ("podman", "docker") if shutil.which(e)), None)


def _image_present() -> bool:
    if ENGINE is None:
        return False
    return subprocess.run([ENGINE, "image", "inspect", IMAGE], capture_output=True).returncode == 0


pytestmark = pytest.mark.skipif(not _image_present(), reason=f"no container engine or no {IMAGE} image")

PAGE = "data:text/html,<title>Still here</title><p>the agent's page</p>"


class Server:
    """One MCP server over stdio, as Hermes starts it (`<engine> exec -i <container> /opt/stealth/mcp.sh`)."""

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

    def kill(self) -> None:
        self.process.kill()
        self.process.wait(timeout=10)


def chromes(container: str) -> int:
    out = subprocess.run([ENGINE, "exec", container, "pgrep", "-c", "-f", "--", "--user-data-dir=/home/bip/profile"],
                         capture_output=True, text=True).stdout.strip()
    return int(out or 0)


@pytest.fixture
def container():
    name = f"bip-survives-{uuid.uuid4().hex[:8]}"
    subprocess.run([ENGINE, "run", "-d", "--rm", "--name", name, "--shm-size", "1g", IMAGE], check=True,
                   capture_output=True)
    try:
        for _ in range(100):
            if subprocess.run([ENGINE, "exec", name, "test", "-e", "/tmp/.X11-unix/X99"]).returncode == 0:
                break
            time.sleep(0.2)
        yield name
    finally:
        subprocess.run([ENGINE, "rm", "-f", name], capture_output=True)


def test_the_page_and_its_instance_outlive_the_server(container):
    first = Server(container)
    instance = first.call("spawn_browser")["instance_id"]
    first.call("navigate", instance_id=instance, url=PAGE)
    # Asked again within the same server: the same browser, not a second one.
    assert first.call("spawn_browser")["instance_id"] == instance
    first.end()   # Hermes restarts its MCP server
    assert chromes(container) > 0

    second = Server(container)
    state = second.call("get_instance_state", instance_id=instance)   # the agent's old id, straight away
    assert state["title"] == "Still here"
    assert [i["instance_id"] for i in second.call("list_instances") if i["source"] == "active"] == [instance]
    assert second.call("spawn_browser")["instance_id"] == instance
    second.kill()   # killed mid-browse: Chrome stays all the same

    third = Server(container)
    assert third.call("get_instance_state", instance_id=instance)["title"] == "Still here"
    # Done with the task: its pages close, Chrome stays for the next one.
    assert third.call("close_instance", instance_id=instance) is True
    assert chromes(container) > 0
    assert third.call("spawn_browser")["instance_id"] == instance
    assert third.call("get_instance_state", instance_id=instance)["url"] == "about:blank"
    # Only the person's « quit » quits it; the next spawn_browser starts it again, under the same id.
    assert third.call("close_instance", instance_id=instance, quit_browser=True) is True
    for _ in range(50):
        if chromes(container) == 0:
            break
        time.sleep(0.2)
    assert chromes(container) == 0
    assert third.call("spawn_browser")["instance_id"] == instance
    assert chromes(container) > 0
    third.end()


def test_a_chrome_that_went_away_comes_back_under_the_same_id(container):
    server = Server(container)
    instance = server.call("spawn_browser")["instance_id"]
    # The person closed its window in « Bureau », or it crashed.
    subprocess.run([ENGINE, "exec", container, "pkill", "-f", "--", "--user-data-dir=/home/bip/profile"])
    for _ in range(50):
        if chromes(container) == 0:
            break
        time.sleep(0.2)
    assert server.call("navigate", instance_id=instance, url=PAGE)["title"] == "Still here"
    assert chromes(container) > 0
    server.end()


def test_the_persons_browser_and_two_servers_share_one_chrome(container):
    # The person opened the browser from the app first (no flags: open_browser.sh gives nodriver's own).
    subprocess.run([ENGINE, "exec", "-d", container, "/opt/stealth/open_browser.sh"], check=True)
    for _ in range(100):
        if chromes(container) > 0:
            break
        time.sleep(0.2)
    gateway, chat = Server(container), Server(container)   # the agent's gateway and a `hermes chat` beside it
    instance = gateway.call("spawn_browser")["instance_id"]
    assert chat.call("spawn_browser")["instance_id"] == instance
    chat.call("navigate", instance_id=instance, url=PAGE)
    chat.end()   # the `hermes chat` ends: the gateway's browser goes on
    assert gateway.call("get_instance_state", instance_id=instance)["title"] == "Still here"
    gateway.end()
    out = subprocess.run([ENGINE, "exec", container, "pgrep", "-a", "-f", "--", "--remote-debugging-port=9222"],
                         capture_output=True, text=True).stdout
    assert len([line for line in out.splitlines() if "--type=" not in line]) == 1   # one Chrome, never two
