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
     "family": {"agent": "maison__marie", "name", "owner", "other", "other_file", "other_said"},
     "relay_family": {"invitation": "<Léo's QR code>", "name": "Léo"}}

(`other_said`: Paul's message, already in the family's common conversation when the apps open it, from « Paul »:
family_thread.rs. `relay_family`, J58: Léo's home, an address nobody reaches, and its invitation with its mailbox at
Kerfy Studio's relay, the real binary served beside them: the app scans it, the journeys' bridge links through the relay.)

then serves until `JOURNEYS_OUT + ".done"` appears (or `JOURNEYS_MINUTES`, default 60). A friend's journey makes its own
link (`POST /v1/read-shares/<share>/invites`, the bridge's key): a link opens once."""
import json
import os
import re
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


def journey_tag(run) -> str:
    """The journey's tag right after `#tache` in the person's message (iPhone « J46-12345 », Android « [J46-k3x9a] »),
    else the turn's own id: never the same twice."""
    found = re.search(r"#tache\s+\[?(J\d+-[0-9A-Za-z]+)", run.words)
    return found.group(1) if found else run.id


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
    family_files(bridges, fakes, journey, marie)
    return {"agent": "maison__marie", "name": "Maison", "owner": "Marie", "other": "Paul", "other_file": "menu-de-noel.pdf",
            "other_said": "Le menu de Noël"}


def family_files(bridges, fakes, journey, marie):
    """J55: Paul, another member of Marie's family, sent « Maison » a PDF (kept on Marie's place, in its pool:
    everyone's who uses it); `#fichiers` makes the agent list its files with `read_sent_file` (as Maison, Marie's
    place's token), in whoever's turn."""
    from test_households_and_peers import household
    from test_family_agent_files import tiny_pdf
    paul = household(bridges, fakes, "paul", ["jardin"])
    assert paul.put("/v1/family/name", headers=AUTH, json={"name": "Paul"}).status_code == 200
    invitation = marie.post("/v1/family/invite", headers=AUTH).json()
    joined = paul.post("/v1/family/join", headers=AUTH,
                       json={k: invitation[k] for k in ("url", "code", "fingerprint") if invitation.get(k)})
    assert joined.status_code == 200, joined.text
    assert paul.get("/v1/family-agent", headers=AUTH).json()["agent"]["id"] == "maison__marie"
    shared = {"agent": "maison__marie"}
    stored = paul.post("/v1/files", headers=AUTH, data=shared,
                       files={"file": ("menu-de-noel.pdf", tiny_pdf("Menu de Noel : dinde aux marrons"), "application/pdf")})
    assert stored.status_code == 200, stored.text
    sent = paul.post("/v1/threads/new/messages", headers=AUTH,
                     json={**shared, "client_id": "C0FFEE00-0000-4000-8000-0000000055A1", "text": "Le menu de Noël",
                           "attachments": [{"kind": "document", "name": "menu-de-noel.pdf", "mime": "application/pdf",
                                            "path": stored.json()["path"]}]})
    assert sent.status_code == 200, sent.text
    # his turn over before the apps come (the family's one conversation, family_thread.rs): their first message starts a
    # turn of their own, it never waits behind his
    end = time.time() + 60
    while time.time() < end:
        page = paul.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": "maison__marie"}).json()
        if page.get("pending", {}).get("run") is None:
            break
        time.sleep(0.2)
    token = "maison-token-for-the-journeys-0123456789"
    give_agent_token(marie, "maison", token)
    journey.model.calls["files"] = lambda run: mcp_as(marie, token, "/v1/agent-tools/mcp", "read_sent_file", {})


def relay_home(bridges, fakes, relay):
    """J58: Léo's home (no address anyone reaches) and its invitation, its mailbox at Kerfy Studio's relay in it."""
    def change(c):
        c["agents"] = {"Atelier": {"hermes_url": f"{fakes.hermes.url}/p/atelier", "hermes_key": "atelier-key-" + "x" * 24}}
        c["family"] = {"relay_url": relay.url}
    leo = bridges.new("leo", change=change)
    leo.config["public_url"] = "http://leo.home.arpa:8642"
    leo.start()
    assert leo.put("/v1/family/name", headers=AUTH, json={"name": "Léo"}).status_code == 200
    invitation = leo.post("/v1/family/invite", headers=AUTH, json={"days": 1}).json()
    assert invitation.get("relay"), invitation
    return {"invitation": json.dumps(invitation), "name": "Léo"}


def test_serve_people(bridges, fakes, tmp_path):
    from bbkit.kerfy_relay import KerfyRelay
    relay = KerfyRelay(tmp_path).start()
    journey = journey_bridge(bridges, fakes, word_gap=float(os.environ.get("JOURNEYS_WORD_GAP", "0.12")),
                             name="journeys-people", family_relay=relay.url)
    journey.model = PeopleModel.upgrade(journey.model)
    bridge = journey.bridge
    assert bridge.patch("/v1/notebook/settings", headers=AUTH, json={"language": "fr"}).status_code == 200
    journey.model.calls["routine"] = lambda run: mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "routine_schedule", {
        "action": "create", "name": "Le point du soir", "schedule": "every day at 19:00",
        "prompt": "Fais le point de la journée."})
    # each journey's task its own (the journey's tag in its prompt): one bridge serves every journey on both apps, and a
    # task like one already there (J39's, paused by « Annuler ») is not proposed again (jobs.rs `same_task`)
    journey.model.calls["task"] = lambda run: mcp_as(bridge, journey.token, "/v1/agent-tools/mcp", "propose_task", {
        "title": "Dossier location", "prompt": f"Prépare le dossier de location {journey_tag(run)} #lent",
        "budget": "small"})
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
    relayed = relay_home(bridges, fakes, relay)
    guichet = Guichet().start()
    try:
        shared = project_share(journey, guichet)
        out = {"url": bridge.url, "door": bridge.config["public_url"], "key": BRIDGE_KEY, "agent": AGENT,
               "thread": journey.thread, "pairing": journey.pairing(), "log": str(bridge.log_path),
               "guichet": guichet.url, "project": {"agent": shared.agent, "thread": shared.thread,
                                                   "share": shared.share},
               "family": family, "relay_family": relayed}
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
        relay.stop()
