"""Second review of card-guards (083b7b12f), 14/11: the guard's look in a world of its own needs Chrome's DevTools
secret (`cdp_secret_file`), which bip-browser-mcp reads from its settings (the agent's mcp.json). Only a browser set up
by this bridge gets it (browsers.rs `proxy_config`); one set up before (every agent of today's places and aibox) keeps
its mcp.json, its look can't reach DevTools through the relay, and the guard falls back to the page's own world (the
earlier script that changes what it sees). The phone exit brings old mcp.json up to date at the bridge's start
(test_phone_exit.py::test_browsers_set_up_before_learn_where_the_hub_is); this does the same check for the secret.
FAILS on the branch. Never pushed."""
import json
import time

from bbkit.servers import wait_until


def test_rw4_a_browser_set_up_before_gets_its_devtools_secret_for_the_guard(bridges):
    bridge = bridges.new()
    folder = bridge.state_dir / "browsers" / "wellness"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "cdp.token").write_text("a" * 32)
    (folder / "ports.json").write_text(json.dumps({"cdp": 49222, "vnc": 49223}))
    config = folder / "mcp.json"
    config.write_text(json.dumps({"server": ["true"], "busy": str(folder / "busy"), "cdp_port": 49222}))
    bridge.start()
    found = wait_until(lambda: json.loads(config.read_text()).get("cdp_secret_file"), timeout=15)
    assert found, f"mcp.json after the bridge's start: {json.loads(config.read_text())}"
