"""The MCP Python SDK's own client (the one Hermes uses) gets `notifications/tools/list_changed` from the bridge and
re-lists. Needs a Python with that SDK: `MCP_SDK_PYTHON=/path/to/python` (e.g. a venv with `mcp==2.0.0`, Hermes
517b5e10f's pin); the client is bbkit/mcp_sdk_client.py (it prints LISTED, waits for the notification, re-lists and
prints RELISTED with the calendar tools). Skipped without it."""
import os
import subprocess
import time
from pathlib import Path

import pytest

from bbkit.common import AUTH
from bbkit.mcp import give_token
from conftest import managed_install, profile

PYTHON = os.environ.get("MCP_SDK_PYTHON")
# another client with the same output (e.g. one driving Hermes' own tools/mcp_tool code)
CLIENT = os.environ.get("MCP_SDK_CLIENT") or str(Path(__file__).parent / "bbkit" / "mcp_sdk_client.py")
TOKEN = "coach-token-" + "q" * 30

pytestmark = pytest.mark.skipif(not PYTHON, reason="no Python with the MCP SDK given (MCP_SDK_PYTHON)")


def test_the_sdk_client_hears_the_change_and_lists_again(bridges, fakes):
    bridge = bridges.new()
    folder = profile(managed_install(bridge, fakes, agents=("coach",)), "coach")
    # its profile already has its entry with this token: the bridge keeps it when it writes the entry again
    (folder / "config.yaml").write_text(
        f"mcp_servers:\n  bipagents:\n    headers:\n      Authorization: Bearer {TOKEN}\n")
    give_token(bridge, "coach", TOKEN)
    bridge.start()
    client = subprocess.Popen([PYTHON, "-I", CLIENT, bridge.url + "/v1/agent-tools/mcp", TOKEN],
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    out = []
    try:
        for line in client.stdout:
            out.append(line.rstrip())
            if line.startswith("LISTED"):
                time.sleep(1)
                assert bridge.put("/v1/calendar/agents", json={"agents": ["Coach"]}, headers=AUTH).status_code == 200
            if line.startswith("RELISTED"):
                break
        client.wait(timeout=30)
    finally:
        client.kill()
    text = "\n".join(out)
    assert "LISTCHANGED True" in text, text
    assert "ToolListChangedNotification" in text or "received tools/list_changed notification" in text, text
    relisted = [line for line in out if line.startswith("RELISTED ")]
    assert relisted and set(relisted[0].split()[1].split(",")) == {
        "calendar_list", "calendar_add", "calendar_update", "calendar_delete"}, text
