"""What the browser image is built with, read from its files (python3 -m pytest browser/test_image_rules.py): no
container engine or image needed. The rebuilt image itself is checked by test_browser_survives.py."""
import json
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))


def read(name: str) -> str:
    with open(os.path.join(HERE, name), encoding="utf-8") as file:
        return file.read()


def test_chrome_opens_no_file_source_internal_or_devtools_page():
    # Security audit 11/11 (side finding): navigate("file:///run/bipagents/vnc.pass") then get_page_content read the
    # container's secrets (the screen's password, the DevTools secret). Chrome's managed policy blocks those schemes.
    policy = json.loads(read("policies.json"))
    assert {"file://*", "view-source:*", "chrome://*", "devtools://*", "chrome-devtools://*"} <= set(policy["URLBlocklist"])
    assert set(policy["URLAllowlist"]) == {"chrome://newtab", "chrome://new-tab-page"}   # only the new tab page
    for kept in ("PasswordManagerEnabled", "AutofillAddressEnabled", "AutofillCreditCardEnabled",
                 "PasswordLeakDetectionEnabled"):
        assert policy[kept] is False, kept
    # Installed where Chrome and Chromium read their managed policies.
    containerfile = read("Containerfile")
    assert "COPY policies.json /opt/stealth/policies.json" in containerfile
    assert "/etc/opt/chrome/policies/managed /etc/chromium/policies/managed" in containerfile
    assert 'cp /opt/stealth/policies.json "$dir/bipagents.json"' in containerfile


def test_the_server_keeps_none_of_the_pages_traffic_in_the_container():
    # Fix review 11/11, R5: upstream's network interceptor keeps every request's body and every response's in the
    # server's memory, served as the resource browser://<id>/network (a card payment's own request among them).
    # In the agent's container (STEALTH_CDP_PORT set) its two recorders are replaced before it is set up.
    patch = read("stealth.patch")
    server = patch[patch.index("diff --git a/src/server.py"):]
    spawn = server[server.index("@@ -230,10"):server.index("@@ -277,17")]
    added = "\n".join(line[1:] for line in spawn.splitlines() if line.startswith("+"))
    assert 'if os.environ.get("STEALTH_CDP_PORT"):' in added
    assert "network_interceptor._on_request = _bip_nothing_kept" in added
    assert "network_interceptor._on_response = _bip_nothing_kept" in added
    assert spawn.index("_on_response = _bip_nothing_kept") < spawn.index("setup_interception(")   # before any event
    # the hunk's counts still match its lines (the patch applies)
    header = re.match(r"@@ -(\d+),(\d+) \+(\d+),(\d+) @@", spawn)
    body = spawn.splitlines()[1:]
    old = sum(1 for line in body if line[:1] in (" ", "-") or line == "")
    new = sum(1 for line in body if line[:1] in (" ", "+") or line == "")
    assert (old, new) == (int(header.group(2)), int(header.group(4)))
