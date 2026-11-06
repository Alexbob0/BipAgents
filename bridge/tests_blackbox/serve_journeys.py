"""Not a test of the suite: the release journeys' bridge (bbkit/journeys.py) served for the APPS' journeys
(BipAgentsUITests/ReleaseJourneysTests on the iPhone simulator, android/.../journeys/ReleaseJourneysTest under
Robolectric), which drive the real apps against it. Run by scripts/release-journeys.sh (or by hand):

    BRIDGE_CMD="<release bipbridge> serve" JOURNEYS_OUT=/tmp/journeys.json python -m pytest serve_journeys.py -q -s

Writes `JOURNEYS_OUT` once the bridge answers:

    {"url": "http://127.0.0.1:<port>", "door": "https://127.0.0.1:<port>", "key": "<bridge key>", "agent": "vie",
     "thread": "t_…", "pairing": {<the install's QR code: what the iPhone pairs with>}}

then serves until the file `JOURNEYS_OUT + ".done"` appears (or `JOURNEYS_MINUTES`, default 60). The phones' journeys
use their own words each time: they share the one conversation, its earlier messages stay."""
import json
import os
import time
from pathlib import Path

from bbkit.common import BRIDGE_KEY
from bbkit.journeys import AGENT, journey_bridge


def test_serve(bridges, fakes):
    journey = journey_bridge(bridges, fakes, word_gap=float(os.environ.get("JOURNEYS_WORD_GAP", "0.12")))
    pairing = journey.pairing()
    out = {"url": journey.bridge.url, "door": journey.bridge.config["public_url"], "key": BRIDGE_KEY, "agent": AGENT,
           "thread": journey.thread, "pairing": pairing, "log": str(journey.bridge.log_path)}
    target = Path(os.environ.get("JOURNEYS_OUT", "/tmp/journeys.json"))
    target.write_text(json.dumps(out))
    print(f"\n[journeys] {json.dumps(out)}", flush=True)
    done = Path(str(target) + ".done")
    deadline = time.time() + 60 * float(os.environ.get("JOURNEYS_MINUTES", "60"))
    while not done.exists() and time.time() < deadline:
        time.sleep(0.5)
    assert journey.bridge.running, journey.bridge.log_tail(80)
