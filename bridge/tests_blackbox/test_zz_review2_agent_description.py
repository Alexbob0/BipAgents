"""Second review of fix-created-agent-description (15/11), black-box: what it found, as failing tests.

R2-2: the branch tells an agent's ongoing conversations to rebuild their prompt (`hermes sessions refresh-prompts`)
after a personality, an answer style, a language, a name or a card: Hermes v0.23.1 replays a continuing
conversation's stored system prompt until then. The person's « Seulement le critique » (and « Tout valider ») for
one agent is another setting of theirs written in its SOUL.md (approval_level.rs `with_block`: « Don't ask them
« shall I…? » before what is not critical… do it »), and its ongoing conversation keeps the old rule: the agent goes
on asking what the person just said it may do, or acting without the « shall I…? » they just asked back.
"""
from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_agents_admin import hermes_args, managed

REFRESH = ["-p", "max", "sessions", "refresh-prompts", "--json"]
RULE = "<!-- bipagents:approval-level -->"


def test_review2_an_approval_level_changed_reaches_its_ongoing_conversation(bridges, fakes):
    bridge, home = managed(bridges, fakes)
    assert bridge.post("/v1/agents", headers=AUTH, json={"name": "Max", "kind": "daily", "lang": "fr"}).status_code == 201
    soul_path = home / "profiles" / "max" / "SOUL.md"
    assert RULE not in soul_path.read_text()
    bridge.commands.clear()
    put = bridge.put("/v1/agents/max/approval-level", headers=AUTH, json={"level": "critical"})
    assert put.status_code == 200, put.text
    assert wait_until(lambda: RULE in soul_path.read_text()), soul_path.read_text()   # its prompt source changed…
    assert wait_until(lambda: REFRESH in hermes_args(bridge), timeout=5), hermes_args(bridge)   # …its conversations told
    # and back: the rule leaves SOUL.md, the conversation must lose it too
    bridge.commands.clear()
    assert bridge.put("/v1/agents/max/approval-level", headers=AUTH, json={"level": "all"}).status_code == 200
    assert wait_until(lambda: RULE not in soul_path.read_text()), soul_path.read_text()
    assert wait_until(lambda: REFRESH in hermes_args(bridge), timeout=5), hermes_args(bridge)

