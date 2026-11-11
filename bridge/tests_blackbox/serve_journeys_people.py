"""Not a test of the suite: the bridge of the APPS' people and features journeys (J32+, docs/release-journeys.md):
BipAgentsUITests/ReleaseJourneysPeopleTests on the iPhone simulator, android/.../journeys/ReleaseJourneysPeopleTest
under Robolectric. Run by scripts/release-journeys.sh (or by hand, as serve_journeys.py):

    BRIDGE_CMD="<release bipbridge> serve" JOURNEYS_OUT=/tmp/people.json python -m pytest serve_journeys_people.py -q -s

The journeys' bridge (bbkit/journeys.py: agent « Vie ») answering this release's turns too (bbkit/model.py
`PeopleModel`: `#routine`, `#tache`…, the agent speaking French), a guichet for « Partager en lecture »
(bbkit/people.py `Guichet`), Alex's project « Pour les queens » shared there, its reply with a map, and Marie's place
with the family's agent « Maison » (J54). Writes:

    {"url", "door", "key", "agent": "vie", "thread", "pairing", "log",
     "guichet": "http://127.0.0.1:<port>", "project": {"agent", "thread", "share"},
     "family": {"agent": "maison__marie", "name", "owner"}}

then serves until `JOURNEYS_OUT + ".done"` appears (or `JOURNEYS_MINUTES`, default 60). A friend's journey makes its own
link (`POST /v1/read-shares/<share>/invites`, the bridge's key): a link opens once."""
import json
import os
import time
from pathlib import Path

from bbkit.common import AUTH, BRIDGE_KEY
from bbkit.journeys import AGENT, journey_bridge
from bbkit.model import PeopleModel
from bbkit.people import Guichet, give_agent_token, mcp_as, project_share


# J52's pages: (page, kind, section, facts)
JOURNEY_J52_PAGES = [
    ("BipAgents", "subject", "status", ["Bêta publique le 19 novembre"]),
    ("Famille", "area", "matters", ["Judo de Zoé : mercredi 14 h", "BipAgents : panorama concurrentiel livré le 25/10"]),
]


def family_agent(bridges, fakes, journey):
    """J54: Marie's place holds the family's agent « Maison » (family_agent.rs); the journeys' bridge (Alex's) is linked
    to it, so Alex's phone has it at once, « de Marie », opened in the conversation screen of his own agents. Its turns
    are answered by the journeys' model (one fake Hermes for every place)."""
    from test_family_agent import make_family_agent
    from test_households_and_peers import household

    bridge = journey.bridge
    marie = household(bridges, fakes, "marie", ["maison"])
    assert bridge.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert marie.put("/v1/family/name", headers=AUTH, json={"name": "Marie"}).status_code == 200
    invitation = marie.post("/v1/family/invite", headers=AUTH).json()
    joined = bridge.post("/v1/family/join", headers=AUTH,
                         json={k: invitation[k] for k in ("url", "code", "fingerprint") if invitation.get(k)})
    assert joined.status_code == 200, joined.text
    make_family_agent(marie, "maison")
    # asked as the apps ask (Famille's page, the home): the linked places say whether one holds the family's agent
    assert bridge.get("/v1/family-agent", headers=AUTH).json()["agent"]["id"] == "maison__marie"
    listed = bridge.get("/v1/shared-agents", headers=AUTH).json()["agents"]
    assert [(x["id"], x["family"]) for x in listed] == [("maison__marie", True)], listed
    # the link gave the bridge's agents their tokens again: Vie's, as the journeys' model calls its tools with it
    give_agent_token(bridge, AGENT, journey.token)
    assert bridge.post("/v1/agent-tools/mcp", json={"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
                       headers={"Authorization": f"Bearer {journey.token}", "Accept": "application/json"}).status_code == 200
    return {"agent": "maison__marie", "name": "Maison", "owner": "Marie"}


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
    # J47, the Carnet: Léa's page, the question of the day at any hour (quiet hours of one minute), Vie's calls
    assert bridge.patch("/v1/notebook/settings", headers=AUTH,
                        json={"quiet": {"from": "03:00", "to": "03:01"}}).status_code == 200
    mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "notebook_write",
           {"page": "Léa Martin", "kind": "person", "aliases": ["Léa"], "facts": ["Sœur, vit à Lyon"]})

    def as_vie(tool, arguments):
        return tool, arguments, lambda: mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", tool, arguments)
    journey.model.calls["carnet"] = lambda run: [
        as_vie("notebook_read", {"page": "Léa"}),
        as_vie("notebook_write", {"page": "Léa", "section": "who", "facts": ["Prof de musique"]}),
        as_vie("notebook_write", {"page": "Léa", "section": "tastes",
                                  "ask": {"question": "Léa chausse du combien ?", "chips": ["35", "36", "37"]}}),
    ]
    # J52, moving a point: « Famille » holds a work point of Vie's, « BipAgents » exists (proposed first);
    # `#rangement` makes Vie file one like it on « Famille » again (it goes where the person moved the first one)
    for page, kind, section, facts in JOURNEY_J52_PAGES:
        mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "notebook_write",
               {"page": page, "kind": kind, "section": section, "facts": facts})
    journey.model.calls["rangement"] = lambda run: [
        as_vie("notebook_write", {"page": "Famille", "kind": "area", "section": "matters",
                                  "facts": ["BipAgents : panorama concurrentiel mis à jour le 9/11"]}),
    ]
    family = family_agent(bridges, fakes, journey)
    guichet = Guichet().start()
    try:
        shared = project_share(journey, guichet)
        out = {"url": bridge.url, "door": bridge.config["public_url"], "key": BRIDGE_KEY, "agent": AGENT,
               "thread": journey.thread, "pairing": journey.pairing(), "log": str(bridge.log_path),
               "guichet": guichet.url, "project": {"agent": shared.agent, "thread": shared.thread,
                                                   "share": shared.share},
               "family": family}
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
