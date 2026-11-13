"""Chrome's own UI pages are never the agent's tab (python3 -m pytest browser/test_chrome_ui.py).

13/11, seen by Alex on aibox and on an offer's place: an agent's browser failed every call for ten minutes with
« Browser window not found », and « Bureau » showed a tab « null » at chrome://omnibox-popup.top-chrome/. Chrome 157
(in the browser image since about 10/11) lists its address bar's suggestion popup as a `page` target with no window
of its own (Browser.getWindowForTarget: « Browser window not found »). nodriver's `main_tab` and `tabs[0]` are the
first page target: on a fresh Chrome its new tab, but once the agent opened a site in a new tab and closed the first
one, the popup. stealth.patch took it at spawn and reconnect (Hermes restarting its MCP servers) and as the fallback of
the navigation tab, and every call failed on it.

Three layers, each checked here: Chrome started without that popup page (`--disable-features=WebUIOmniboxPopup`,
merged into the one `--disable-features` Chrome reads: it keeps only the last), the patch's tab choices skipping
Chrome's own UI pages whatever their order, and (bridge-rs, desktop_page.rs and card_fill.rs) the bridge's. The first
tests need nothing; the last ones a container of the browser image (BIP_BROWSER_IMAGE, default `bipagents-browser`,
built from this folder; podman or docker), skipped without one.
"""
import asyncio
import json
import os
import re
import shutil
import socket
import subprocess
import time
import uuid
from typing import Optional

import pytest

HERE = os.path.dirname(os.path.abspath(__file__))
IMAGE = os.environ.get("BIP_BROWSER_IMAGE", "bipagents-browser")
ENGINE = next((e for e in ("podman", "docker") if shutil.which(e)), None)
POPUP = "chrome://omnibox-popup.top-chrome/"
# What the bridge gives the container (bridge-rs/src/browsers.rs `run_args`): its own --disable-features among them.
BRIDGE_ARGS = ("--start-maximized;--no-first-run;--no-default-browser-check;--disable-dev-shm-usage;"
               "--ignore-gpu-blocklist;--test-type;--hide-crash-restore-bubble;--disable-back-forward-cache;"
               "--accept-lang=fr-FR,fr;--force-webrtc-ip-handling-policy=disable_non_proxied_udp;"
               "--disable-features=IsolateOrigins,site-per-process,OptimizationGuideModelDownloading,"
               "OptimizationHintsFetching,OptimizationTargetPrediction,OptimizationHints")


def read(name: str) -> str:
    with open(os.path.join(HERE, name), encoding="utf-8") as file:
        return file.read()


# -- the patch's own helpers, run here without the server around them ------------------------------------------------

class _Logger:
    def __getattr__(self, name):
        return lambda *args, **kwargs: None


def patch_helpers(state_file: str) -> dict:
    """The module-level code stealth.patch adds to browser_manager.py (its first hunk), run on its own."""
    patch = read("stealth.patch")
    manager = patch[:patch.index("diff --git a/src/server.py")]
    hunk = manager[manager.index("\n@@ ") + 1:]
    hunk = hunk[:hunk.index("\n@@ ", 1)]
    code = "\n".join(line[1:] for line in hunk.splitlines()[1:] if line.startswith("+"))
    os.environ["STEALTH_STATE_FILE"] = state_file
    try:
        window_for = type("BrowserDomain", (), {"get_window_for_target": staticmethod(lambda t: ("window", t))})
        uc = type("Nodriver", (), {"cdp": type("Cdp", (), {"browser": window_for})})
        namespace = {"__name__": "stealth_patch", "asyncio": asyncio, "os": os, "json": json, "time": time,
                     "Optional": Optional, "uc": uc, "debug_logger": _Logger()}
        exec(compile(code, "stealth.patch", "exec"), namespace)
    finally:
        del os.environ["STEALTH_STATE_FILE"]
    return namespace


class Target:
    def __init__(self, target_id: str, url: str):
        self.target_id, self.url, self.type_ = target_id, url, "page"


class FakeTab:
    def __init__(self, target_id: str, url: str):
        self.target = Target(target_id, url)


class FakeConnection:
    """The browser's DevTools connection: `Browser.getWindowForTarget` fails for a tab with no window."""

    def __init__(self, windowless, stuck=()):
        self.windowless, self.stuck = windowless, stuck

    async def send(self, command):
        if command[0] == "window" and command[1] in self.stuck:
            await asyncio.sleep(3600)   # a DevTools that never answers
        if command[0] == "window" and command[1] in self.windowless:
            raise Exception("Browser window not found [code: -32000]")
        return {"windowId": 1}


class FakeBrowser:
    """nodriver's Browser as the patch sees it: `tabs` in Chrome's order, the popup first when it comes first.
    Chrome's UI pages (POPUP, SEARCH, READ) have no window."""

    def __init__(self, *tabs: FakeTab, port: int = 9222):
        self.targets = list(tabs)
        self._process = None
        self.config = type("Config", (), {"port": port})()
        self.opened = []
        self.connection = FakeConnection({"POPUP", "SEARCH", "READ"})

    @property
    def tabs(self):
        return [t for t in self.targets if t.target.type_ == "page"]

    @property
    def main_tab(self):
        return self.targets[0]

    async def update_targets(self):
        pass

    async def get(self, url, new_tab=False):
        tab = FakeTab(f"NEW{len(self.opened)}", url)
        self.opened.append(url)
        self.targets.append(tab)
        return tab


def chrome_ui_tabs():
    return [FakeTab("POPUP", POPUP), FakeTab("SEARCH", "chrome://tab-search.top-chrome/"),
            FakeTab("READ", "chrome-untrusted://read-anything-side-panel.top-chrome/?x=1")]


def test_chromes_own_ui_pages_are_told_apart(tmp_path):
    helpers = patch_helpers(str(tmp_path / "state.json"))
    ui = helpers["_chrome_ui"]
    for tab in chrome_ui_tabs():
        assert ui(tab), tab.target.url
    for url in ("chrome://newtab/", "about:blank", "https://www.lemonde.fr/", "http://127.0.0.1:8765/top-chrome/",
                "https://omnibox-popup.top-chrome.example/", "chrome://settings/", ""):
        assert not ui(FakeTab("T", url)), url


def test_every_tab_choice_skips_chromes_own_ui_even_first(tmp_path, monkeypatch):
    state = tmp_path / "state.json"
    helpers = patch_helpers(str(state))
    monkeypatch.setenv("STEALTH_CDP_PORT", "9222")
    site, new = FakeTab("SITE", "https://www.lemonde.fr/"), FakeTab("NEW", "chrome://newtab/")
    browser = FakeBrowser(*chrome_ui_tabs(), site, new)
    assert asyncio.run(helpers["_bip_tabs"](browser)) == [site, new]
    # Review of 13/11: a real tab on such an address (an error page, in a window) is still the agent's tab.
    lookalike = FakeTab("LOOK", "chrome-untrusted://foo.top-chrome/")
    assert asyncio.run(helpers["_bip_tabs"](FakeBrowser(lookalike, *chrome_ui_tabs(), site))) == [lookalike, site]
    # Second review of 13/11: a window question that never gets its answer does not hang the tab choice; that page
    # (a top-chrome address) counts as Chrome's UI, as when Chrome says it has no window.
    helpers["_WINDOW_SECONDS"] = 0.2
    stuck = FakeBrowser(lookalike, site)
    stuck.connection.stuck = {"LOOK"}
    assert asyncio.run(asyncio.wait_for(helpers["_bip_tabs"](stuck), timeout=3)) == [site]
    # spawn and reconnect with no tab of its own known: the first real one, never the popup listed before it
    assert asyncio.run(helpers["_bip_first_tab"](browser)) is site
    # only Chrome's UI left: a blank tab opened for the agent
    lonely = FakeBrowser(*chrome_ui_tabs())
    chosen = asyncio.run(helpers["_bip_first_tab"](lonely))
    assert chosen.target.url == "about:blank" and lonely.opened == ["about:blank"]
    # the tab the agent last used (an earlier server's state): never the popup, even when it was saved
    state.write_text(json.dumps({"tab": "POPUP"}))
    assert asyncio.run(helpers["_bip_last_tab"](browser)) is None
    state.write_text(json.dumps({"tab": "SITE"}))
    assert asyncio.run(helpers["_bip_last_tab"](browser)) is site


def test_one_disable_features_switch_with_the_popup_off(tmp_path):
    merge = patch_helpers(str(tmp_path / "state.json"))["_bip_features_off"]
    # nodriver's command line: its own switch, then the container's flags (the bridge's switch among them)
    args = ["--remote-allow-origins=*", "--user-data-dir=/home/bip/profile",
            "--disable-features=IsolateOrigins,site-per-process", "--start-maximized",
            "--disable-features=IsolateOrigins,site-per-process,OptimizationHints", "--no-sandbox"]
    merged = merge(args)
    switches = [a for a in merged if a.startswith("--disable-features=")]
    assert switches == ["--disable-features=IsolateOrigins,site-per-process,OptimizationHints,WebUIOmniboxPopup,"
                        "WebUIOmniboxAimPopup"]
    assert [a for a in merged if a not in switches] == [a for a in args if not a.startswith("--disable-features=")]
    # the caller's args alone (the server's own launch, after nodriver's switch): nodriver's two kept
    assert [a for a in merge(["--start-maximized"]) if a.startswith("--disable-features=")] == [
        "--disable-features=IsolateOrigins,site-per-process,WebUIOmniboxPopup,WebUIOmniboxAimPopup"]
    assert merge(merged) == merged


def test_the_patch_routes_every_tab_choice_through_its_filter():
    """The upstream lines the patch changes (spawn, the navigation tab's fallback, the tab tools, a released
    instance's pages): each goes through `_bip_tabs` / `_bip_first_tab`; and every hunk's counts match its lines."""
    patch = read("stealth.patch")
    removed = {line[1:].strip() for line in patch.splitlines() if line.startswith("-") and not line.startswith("---")}
    added = "\n".join(line[1:] for line in patch.splitlines() if line.startswith("+") and not line.startswith("+++"))
    assert "fallback_tab = browser.tabs[0]" in removed
    assert "tab = browser.main_tab" in removed
    assert "for tab in browser.tabs:" in removed               # list_tabs, switch_to_tab, close_tab
    assert "for candidate_tab in browser.tabs:" in removed     # the tracked tab, looked for again
    assert "browser.main_tab" not in added
    helper = "return [tab for tab in browser.tabs if not _chrome_ui(tab) or await _has_window(browser, tab)]"
    assert helper in added
    assert not re.search(r"browser\.tabs\b", added.replace("_bip_tabs(browser)", "").replace(helper, ""))
    assert added.count("await _bip_tabs(browser)") >= 6 and added.count("_bip_tabs(browser)") == added.count(
        "await _bip_tabs(browser)") + added.count("async def _bip_tabs(browser)")
    assert "_launch_detached(_bip_features_off(" in added
    assert "_bip_features_off(_without_dropped_args(merge_browser_args(caller_args)))" in added
    hunks = []
    for line in patch.splitlines():
        if line.startswith("@@ "):
            counts = re.match(r"@@ -\d+(?:,(\d+))? \+\d+(?:,(\d+))? @@", line)
            hunks.append([line, int(counts.group(1) or 1), int(counts.group(2) or 1), 0, 0])
        elif line.startswith("diff --git"):
            hunks.append(None)
        elif hunks and hunks[-1] is not None and not line.startswith("\\"):
            hunks[-1][3] += line[:1] in (" ", "-") or line == ""
            hunks[-1][4] += line[:1] in (" ", "+") or line == ""
    for header, old, new, seen_old, seen_new in filter(None, hunks):
        assert (seen_old, seen_new) == (old, new), header


# -- open_browser.sh, with a stand-in for Chrome ---------------------------------------------------------------------

def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def launched_with(tmp_path, *args, browser_args="") -> list:
    """The command line open_browser.sh gives Chrome (a script printing its arguments, one per line)."""
    bin_dir, profile, out = tmp_path / "bin", tmp_path / "profile", tmp_path / "args"
    bin_dir.mkdir(exist_ok=True)
    profile.mkdir(exist_ok=True)
    chrome = bin_dir / "google-chrome-stable"
    chrome.write_text('#!/bin/sh\nfor a in "$@"; do printf "%s\\n" "$a"; done > "$BIP_TEST_ARGS"\n')
    chrome.chmod(0o755)
    script = tmp_path / "open_browser.sh"
    script.write_text(read("open_browser.sh").replace("/tmp/open-browser.log", str(tmp_path / "chrome.log")))
    env = {"PATH": f"{bin_dir}:/usr/bin:/bin", "BIP_TEST_ARGS": str(out), "STEALTH_CDP_PORT": str(free_port()),
           "STEALTH_USER_DATA_DIR": str(profile), "STEALTH_BROWSER_ARGS": browser_args}
    subprocess.run(["sh", str(script), *args], env=env, check=True, timeout=30)
    return out.read_text().splitlines()


def features_off(args: list) -> list:
    return [a.split("=", 1)[1].split(",") for a in args if a.startswith("--disable-features=")]


def test_the_persons_chrome_starts_without_the_popup_page(tmp_path):
    # « Ouvrir le navigateur » in the app: open_browser.sh with no arguments, the bridge's flags in the container
    args = launched_with(tmp_path, browser_args=BRIDGE_ARGS)
    assert features_off(args) == [["IsolateOrigins", "site-per-process", "OptimizationGuideModelDownloading",
                                   "OptimizationHintsFetching", "OptimizationTargetPrediction", "OptimizationHints",
                                   "WebUIOmniboxPopup", "WebUIOmniboxAimPopup"]]
    assert "--start-maximized" in args and "--accept-lang=fr-FR,fr" in args and "--no-sandbox" in args


def test_the_agents_chrome_starts_without_the_popup_page(tmp_path):
    # spawn_browser: open_browser.sh with nodriver's command line, the container's switch after nodriver's
    nodriver = ["--remote-allow-origins=*", "--no-first-run", "--user-data-dir=/home/bip/profile",
                "--disable-features=IsolateOrigins,site-per-process", "--start-maximized",
                "--disable-features=IsolateOrigins,site-per-process,OptimizationHints", "--no-sandbox",
                "--remote-debugging-host=127.0.0.1", "--remote-debugging-port=9222"]
    args = launched_with(tmp_path, *nodriver)
    assert features_off(args) == [["IsolateOrigins", "site-per-process", "OptimizationHints", "WebUIOmniboxPopup",
                                   "WebUIOmniboxAimPopup"]]
    assert [a for a in args if not a.startswith("--disable-features=")] == [
        a for a in nodriver if not a.startswith("--disable-features=")]


# -- the browser image: a real Chrome 157 ----------------------------------------------------------------------------

def _image_present() -> bool:
    if ENGINE is None:
        return False
    return subprocess.run([ENGINE, "image", "inspect", IMAGE], capture_output=True).returncode == 0


needs_image = pytest.mark.skipif(not _image_present(), reason=f"no container engine or no {IMAGE} image")
SITE = "http://127.0.0.1:8765/"
PAGES = {"login.html": "<title>Connexion</title><h1>Se connecter</h1>",
         "account.html": "<title>Compte</title><h1>Mon compte</h1>"}


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


def in_container(container: str, *command: str) -> str:
    return subprocess.run([ENGINE, "exec", container, *command], capture_output=True, text=True).stdout


def chrome_targets(container: str) -> list:
    out = in_container(container, "python3.11", "-I", "-c",
                       "import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:9222/json/list').read().decode())")
    return [t for t in json.loads(out or "[]") if t["type"] == "page"]


def chrome_command_line(container: str) -> list:
    out = in_container(container, "pgrep", "-a", "-f", "--", "--remote-debugging-port=9222")
    main = [line for line in out.splitlines() if "--type=" not in line]
    assert len(main) == 1, out
    return main[0].split()


# Chrome started by hand WITHOUT the flag (as before the fix), found as open_browser.sh finds it.
BY_HAND = ("for c in google-chrome-stable google-chrome chromium-browser chromium; do"
           " command -v \"$c\" >/dev/null 2>&1 && exec \"$c\" --no-first-run --no-default-browser-check"
           " --start-maximized --test-type --no-sandbox --remote-debugging-host=127.0.0.1 --remote-debugging-port=9222"
           " --user-data-dir=/home/bip/profile >/tmp/chrome.log 2>&1; done; exit 1")


def chrome_with_its_popup(container: str) -> str:
    """Chrome started without the flag; its popup page's id (skipped where this Chrome has none: before 157)."""
    subprocess.run([ENGINE, "exec", "-d", container, "sh", "-c", BY_HAND], check=True)
    assert wait_for(lambda: chrome_targets(container))
    popup = wait_for(lambda: next((t["id"] for t in chrome_targets(container) if t["url"] == POPUP), None), 10)
    if not popup:
        pytest.skip("this Chrome lists no popup page (before Chrome 157)")
    return popup


def wait_for(check, seconds=30.0):
    deadline = time.time() + seconds
    while time.time() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.25)
    return check()


@pytest.fixture
def container():
    name = f"bip-chrome-ui-{uuid.uuid4().hex[:8]}"
    subprocess.run([ENGINE, "run", "-d", "--rm", "--name", name, "--shm-size", "1g",
                    "-e", f"STEALTH_BROWSER_ARGS={BRIDGE_ARGS}", IMAGE], check=True, capture_output=True)
    try:
        wait_for(lambda: subprocess.run([ENGINE, "exec", name, "test", "-e", "/tmp/.X11-unix/X99"]).returncode == 0)
        # the site the agent opens, served in the container
        files = " && ".join(f"printf '%s' '{html}' > /tmp/site/{page}" for page, html in PAGES.items())
        subprocess.run([ENGINE, "exec", name, "sh", "-c", f"mkdir -p /tmp/site && {files}"], check=True)
        subprocess.run([ENGINE, "exec", "-d", name, "python3.11", "-m", "http.server", "8765", "--bind", "127.0.0.1",
                        "--directory", "/tmp/site"], check=True)
        yield name
    finally:
        subprocess.run([ENGINE, "rm", "-f", name], capture_output=True)


def no_popup_page(container: str, started: str) -> None:
    assert wait_for(lambda: chrome_targets(container)), started
    time.sleep(2)   # the popup page, when there is one, comes a moment after the first tab
    # Chrome reads only the last --disable-features: ours, whatever a distribution's wrapper puts first (Fedora's
    # chromium-browser on ARM puts its own).
    switches = [a for a in chrome_command_line(container) if a.startswith("--disable-features=")]
    assert switches, started
    last = set(switches[-1].split("=", 1)[1].split(","))
    assert {"WebUIOmniboxPopup", "WebUIOmniboxAimPopup", "OptimizationHints", "IsolateOrigins"} <= last, (
        started, switches)
    assert not [t for t in chrome_targets(container) if ".top-chrome" in t["url"]], started


@needs_image
def test_chrome_runs_without_the_popup_page_however_it_starts(container):
    # the person's « Ouvrir le navigateur »: open_browser.sh with no arguments
    subprocess.run([ENGINE, "exec", "-d", container, "/opt/stealth/open_browser.sh"], check=True)
    no_popup_page(container, "open_browser.sh")
    subprocess.run([ENGINE, "exec", container, "pkill", "-f", "--", "--user-data-dir=/home/bip/profile"])
    assert wait_for(lambda: not in_container(container, "pgrep", "-f", "--", "--user-data-dir=/home/bip/profile"))
    # the agent's spawn_browser: open_browser.sh with nodriver's command line
    server = Server(container)
    server.call("spawn_browser")
    no_popup_page(container, "spawn_browser")
    server.end()


@needs_image
def test_the_agent_never_drives_chromes_popup_page(container):
    """Chrome started by hand WITHOUT the flag (as before this fix): its popup page is there, and the patch alone keeps
    the agent off it: the tab tools never show it, the navigation tab's fallback and a reconnect take the site."""
    popup = chrome_with_its_popup(container)

    first = Server(container)
    instance = first.call("spawn_browser")["instance_id"]
    start_tab = first.call("get_active_tab", instance_id=instance)["tab_id"]
    first.call("new_tab", instance_id=instance, url=SITE + "login.html")   # a login opened in a new tab…
    assert first.call("close_tab", instance_id=instance, tab_id=start_tab) is True   # …and the first tab closed
    assert [t["url"] for t in first.call("list_tabs", instance_id=instance)] == [SITE + "login.html"]
    assert first.call("switch_tab", instance_id=instance, tab_id=popup) is False
    assert first.call("close_tab", instance_id=instance, tab_id=popup) is False
    # its tracked tab is gone: the navigation goes to the site's tab, never to the popup listed before it
    assert first.call("navigate", instance_id=instance, url=SITE + "account.html")["title"] == "Compte"
    assert [t["url"] for t in chrome_targets(container) if ".top-chrome" not in t["url"]] == [SITE + "account.html"]
    first.end()   # Hermes restarts its MCP servers

    second = Server(container)
    assert second.call("spawn_browser")["instance_id"] == instance
    assert second.call("get_instance_state", instance_id=instance)["url"] == SITE + "account.html"
    assert second.call("navigate", instance_id=instance, url=SITE + "login.html")["title"] == "Connexion"
    second.end()
    # the person's view of the page (bridge-rs/src/desktop_page.rs, most recently active first) is tested there


# -- the review of 13/11 (its tests, kept) ---------------------------------------------------------------------------

LOOKALIKE = "chrome-untrusted://foo.top-chrome/"   # no page of Chrome behind it: an error page in a real tab


def window_of(container: str, target_id: str) -> str:
    """Browser.getWindowForTarget for one target, asked in the container: « window » or Chrome's error."""
    code = f"""
import asyncio, json, urllib.request, websockets
async def main():
    v = json.load(urllib.request.urlopen("http://127.0.0.1:9222/json/version"))
    async with websockets.connect(v["webSocketDebuggerUrl"], max_size=None) as ws:
        await ws.send(json.dumps({{"id": 1, "method": "Browser.getWindowForTarget",
                                   "params": {{"targetId": "{target_id}"}}}}))
        m = json.loads(await ws.recv())
        print("window" if "result" in m else m["error"]["message"])
asyncio.run(main())
"""
    return in_container(container, "/opt/stealth/venv/bin/python", "-I", "-c", code).strip()


@needs_image
def test_a_real_tab_on_an_address_like_chromes_ui_stays_the_agents_tab(container):
    """The bridge refuses chrome-untrusted: (browser_mcp.rs) and the policy blocks it, but the server here is asked
    directly: Chrome then shows an error page with that address in a real tab, on the screen the person watches.
    Only a page with no window of its own is Chrome's UI: that tab is listed and closed like any other."""
    server = Server(container)
    instance = server.call("spawn_browser")["instance_id"]
    server.call("navigate", instance_id=instance, url=SITE + "login.html")
    tab_id = server.call("new_tab", instance_id=instance, url=LOOKALIKE)["tab_id"]
    assert [t["url"] for t in chrome_targets(container) if t["id"] == tab_id] == [LOOKALIKE]
    assert window_of(container, tab_id) == "window"
    listed = [t["tab_id"] for t in server.call("list_tabs", instance_id=instance)]
    closed = server.call("close_tab", instance_id=instance, tab_id=tab_id)
    server.end()
    assert tab_id in listed, "the agent's own tab is not listed"
    assert closed is True, "the agent cannot close its own tab"


@needs_image
def test_release_and_spawn_again_with_the_popup_there(container):
    """Chrome without the flag (an old container's): the agent's close_instance (its pages closed, a blank one left)
    leaves the popup alone, and the next spawn_browser takes the blank tab, not the popup."""
    chrome_with_its_popup(container)
    server = Server(container)
    instance = server.call("spawn_browser")["instance_id"]
    server.call("new_tab", instance_id=instance, url=SITE + "login.html")
    server.call("close_instance", instance_id=instance)
    left = sorted(t["url"] for t in chrome_targets(container))
    again = server.call("spawn_browser")["instance_id"]
    state = server.call("get_active_tab", instance_id=again)
    server.end()
    # Chrome's UI left alone (the popup; Fedora's Chromium 156 has a second one, …/omnibox_popup_aim.html)
    assert POPUP in left and [u for u in left if ".top-chrome" not in u] == ["about:blank"], left
    assert state["url"] == "about:blank", state


# In the container: the first tab to argv[1], Alt+Shift+R sent to it (Chrome's shortcut for reading mode), then every
# target at a read-anything address: its type and its document's real address (chrome-error:// when blocked).
READ = """
import asyncio, json, sys, urllib.request
import websockets
async def main():
    v = json.load(urllib.request.urlopen("http://127.0.0.1:9222/json/version"))
    async with websockets.connect(v["webSocketDebuggerUrl"], max_size=None) as ws:
        n = 0
        async def call(method, params=None, session=None):
            nonlocal n
            n += 1
            await ws.send(json.dumps({"id": n, "method": method, "params": params or {},
                                      **({"sessionId": session} if session else {})}))
            while True:
                m = json.loads(await ws.recv())
                if m.get("id") == n:
                    return m.get("result", m.get("error"))
        tab = next(t for t in (await call("Target.getTargets"))["targetInfos"] if t["type"] == "page")
        s = (await call("Target.attachToTarget", {"targetId": tab["targetId"], "flatten": True}))["sessionId"]
        await call("Page.navigate", {"url": sys.argv[1]}, s)
        await asyncio.sleep(2)
        await call("Page.bringToFront", {}, s)
        key = {"modifiers": 9, "key": "R", "code": "KeyR", "windowsVirtualKeyCode": 82, "nativeVirtualKeyCode": 82}
        await call("Input.dispatchKeyEvent", {"type": "rawKeyDown", **key}, s)
        await call("Input.dispatchKeyEvent", {"type": "keyUp", **key}, s)
        await asyncio.sleep(4)
        out = []
        for t in (await call("Target.getTargets"))["targetInfos"]:
            if "read-anything" in t["url"]:
                ss = (await call("Target.attachToTarget", {"targetId": t["targetId"], "flatten": True}))["sessionId"]
                r = await call("Runtime.evaluate", {"expression": "location.href", "returnByValue": True}, ss)
                out.append([t["type"], r.get("result", {}).get("value")])
        print(json.dumps(out))
asyncio.run(main())
"""


@needs_image
def test_reading_mode_opens_for_the_person(container):
    """Chrome's reading mode (Alt+Shift+R, or the page's menu), which the person watching « Bureau » may open: a side
    panel of Chrome's own, chrome-untrusted://read-anything-side-panel.top-chrome/. Second review of 13/11: a policy
    blocking chrome-untrusted://* showed « This page is blocked » in it (and in Google Lens). The bridge's refusal of
    those addresses and the « address and no window » rule are enough against a tab made to look like Chrome's UI."""
    subprocess.run([ENGINE, "exec", "-d", container, "/opt/stealth/open_browser.sh"], check=True)
    assert wait_for(lambda: chrome_targets(container))
    time.sleep(2)
    out = in_container(container, "/opt/stealth/venv/bin/python", "-I", "-c", READ, SITE + "account.html")
    panels = json.loads(out)
    assert panels, "reading mode was not asked for (the shortcut did not reach Chrome)"
    assert all(href == "chrome-untrusted://read-anything-side-panel.top-chrome/" for _, href in panels), panels

