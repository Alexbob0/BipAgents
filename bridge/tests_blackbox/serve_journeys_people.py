"""Not a test of the suite: the bridge of the APPS' people and features journeys (J32+, docs/release-journeys.md):
BipAgentsUITests/ReleaseJourneysPeopleTests on the iPhone simulator, android/.../journeys/ReleaseJourneysPeopleTest
under Robolectric. Run by scripts/release-journeys.sh (or by hand, as serve_journeys.py):

    BRIDGE_CMD="<release bipbridge> serve" JOURNEYS_OUT=/tmp/people.json python -m pytest serve_journeys_people.py -q -s

The journeys' bridge (bbkit/journeys.py: agent « Vie ») answering this release's turns too (bbkit/model.py
`PeopleModel`: `#routine`, `#tache`…, the agent speaking French), a guichet for « Partager en lecture »
(bbkit/people.py `Guichet`), and Alex's project « Pour les queens » shared there, its reply with a map. Writes:

    {"url", "door", "key", "agent": "vie", "thread", "pairing", "log",
     "guichet": "http://127.0.0.1:<port>", "project": {"agent", "thread", "share"}}

then serves until `JOURNEYS_OUT + ".done"` appears (or `JOURNEYS_MINUTES`, default 60). A friend's journey makes its own
link (`POST /v1/read-shares/<share>/invites`, the bridge's key): a link opens once."""
import json
import os
import time
from pathlib import Path

from bbkit.common import AUTH, BRIDGE_KEY
from bbkit.journeys import AGENT, journey_bridge
from bbkit.model import PeopleModel
from bbkit.people import Guichet, mcp_as, project_share


def test_serve_people(bridges, fakes):
    journey = journey_bridge(bridges, fakes, word_gap=float(os.environ.get("JOURNEYS_WORD_GAP", "0.12")),
                             name="journeys-people")
    journey.model = PeopleModel.upgrade(journey.model)
    bridge = journey.bridge
    assert bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200
    journey.model.calls["routine"] = lambda run: mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "routine_schedule", {
        "action": "create", "name": "Le point du soir", "schedule": "every day at 19:00",
        "prompt": "Fais le point de la journée.", "silent": True})
    journey.model.calls["task"] = lambda run: mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "propose_task", {
        "title": "Dossier location", "prompt": "Prépare le dossier de location #lent", "budget": "small"})
    guichet = Guichet().start()
    try:
        shared = project_share(journey, guichet)
        out = {"url": bridge.url, "door": bridge.config["public_url"], "key": BRIDGE_KEY, "agent": AGENT,
               "thread": journey.thread, "pairing": journey.pairing(), "log": str(bridge.log_path),
               "guichet": guichet.url, "project": {"agent": shared.agent, "thread": shared.thread,
                                                   "share": shared.share}}
        target = Path(os.environ.get("JOURNEYS_OUT", "/tmp/people.json"))
        target.write_text(json.dumps(out))
        print(f"\n[journeys-people] {json.dumps(out)}", flush=True)
        done = Path(str(target) + ".done")
        deadline = time.time() + 60 * float(os.environ.get("JOURNEYS_MINUTES", "60"))
        while not done.exists() and time.time() < deadline:
            time.sleep(0.5)
        assert bridge.running, bridge.log_tail(80)
    finally:
        guichet.stop()
