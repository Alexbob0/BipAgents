"""Review of fix-created-agent-description (15/11), black-box: what the review found, as failing tests.

Finding 5: the agent's language, changed in the same « Modifier » sheet as its personality (PUT /language), rewrites
SOUL.md's language block, and its ongoing conversation keeps the old one: Hermes v0.23.1 replays a continuing
conversation's stored prompt until `sessions refresh-prompts` (the branch asks it after a personality, an answer style,
a name or a card, not after a language)."""
import time

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_agents_admin import hermes_args, managed

REFRESH = ["-p", "max", "sessions", "refresh-prompts", "--json"]


def test_review_a_language_changed_reaches_its_ongoing_conversation(bridges, fakes):
    bridge, home = managed(bridges, fakes)
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Max", "kind": "daily", "lang": "en"}).status_code == 201
    soul_path = home / "profiles" / "max" / "SOUL.md"
    assert "write in English." in soul_path.read_text()
    bridge.commands.clear()
    changed = bridge.put("/v1/agents/max/language", headers=AUTH, json={"lang": "fr"})
    assert changed.status_code == 200, changed.text
    assert "write in French." in soul_path.read_text()   # its prompt source changed…
    assert wait_until(lambda: REFRESH in hermes_args(bridge)), hermes_args(bridge)   # …and its conversations are told
    # the same language again changes nothing: nothing asked
    bridge.commands.clear()
    assert bridge.put("/v1/agents/max/language", headers=AUTH, json={"lang": "fr"}).status_code == 200
    time.sleep(1.0)
    assert REFRESH not in hermes_args(bridge)
