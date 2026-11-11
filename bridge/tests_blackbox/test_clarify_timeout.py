"""A question waits an hour for the person's answer (Alex, 10/11), on Hermes 0.22.5 as on 0.23.x (bridge-rs/src/tool_diet.rs
`with_clarify`). Hermes 0.23 waits 90 s (#150916) and its config migration v53 rewrites an `agent.clarify_timeout` of
3600 to 90, in each profile, at that profile's first start on the new code: the bridge writes the hour in both keys
Hermes reads (`clarify.timeout` first, which no migration touches), puts back what v53 lowered, and a second start
changes nothing."""
import yaml

from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered

HOUR = 3600


def waits(folder):
    data = yaml.safe_load((folder / "config.yaml").read_text()) or {}
    return (data.get("agent") or {}).get("clarify_timeout"), (data.get("clarify") or {}).get("timeout")


def test_every_agent_waits_an_hour_for_an_answer_whatever_hermes_migrated(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    hub = managed_install(bridge, fakes, agents=("coach", "budget"))
    # Coach: a profile of Hermes 0.22.5 (no key, its default an hour); Budget: what 0.23.1's migration v53 left
    (profile(hub, "coach") / "config.yaml").write_text("_config_version: 51\nmodel:\n  default: glm\n")
    (profile(hub, "budget") / "config.yaml").write_text(
        "_config_version: 53\nmodel:\n  default: glm\nagent:\n  clarify_timeout: 90\n  max_turns: 40\n")
    bridge.start()
    wait_registered(bridge, ["coach", "budget"])
    for name in ("coach", "budget"):
        assert wait_until(lambda: waits(profile(hub, name)) == (HOUR, HOUR)), (name, waits(profile(hub, name)))
    budget = yaml.safe_load((profile(hub, "budget") / "config.yaml").read_text())
    assert budget["agent"]["max_turns"] == 40 and budget["_config_version"] == 53   # its own settings and stamp kept
    # idempotent: a second start writes nothing
    written = {name: (profile(hub, name) / "config.yaml").read_text() for name in ("coach", "budget")}
    bridge.restart()
    wait_registered(bridge, ["coach", "budget"])
    assert {name: (profile(hub, name) / "config.yaml").read_text() for name in ("coach", "budget")} == written
