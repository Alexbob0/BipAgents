"""Not a test of the suite: two real bridges for the iPhone's timeline UI tests (BipAgentsUITests/RealBridgeTimelineTests),
over this suite's fake Hermes. Run by scripts/ui-real-bridge.sh (or by hand):

    BRIDGE_CMD="<debug bipbridge> serve" UI_BRIDGE_OUT=/tmp/ui-bridge.json python -m pytest serve_ui_bridge.py -q -s

- bridge « compressed »: agent `vie`, a main thread of 120 messages Hermes compressed once (two sessions);
- bridge « photo-yes »: agent `vie`, a photo sent with an older message, a long reply, « Yes », the reply to it.

Writes `UI_BRIDGE_OUT` (URLs, key, thread ids, message ids) once both answer, then serves until the file
`UI_BRIDGE_OUT + ".done"` appears (or 20 minutes)."""
import hashlib
import json
import os
import time
from pathlib import Path

from bbkit.common import AUTH, BRIDGE_KEY

PHOTO = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="


def uid(agent, session, i):
    return "m_" + hashlib.md5(f"{agent}/{session}/{i}".encode()).hexdigest()


def words(n):
    """FakeTimelineScenarios.words: the same texts as the in-app fake."""
    return f"Message n°{n}. Une question de la personne." if n % 2 == 0 else f"Message n°{n}. La réponse de l’agent."


def row(n):
    return {"role": "user" if n % 2 == 0 else "assistant", "content": words(n)}


def as_vie(hub, url):
    def change(config):
        agent = dict(config["agents"].pop("Wellness"))
        agent.update(display_name="Vie", hermes_url=url)
        config["agents"]["vie"] = agent
    return change


def main_thread(bridge):
    resp = bridge.get("/v1/threads/main", params={"agent": "vie"}, headers=AUTH)
    assert resp.status_code == 200, resp.text
    return resp.json()["id"]


def test_serve(bridges, fakes):
    fakes.hermes.v0225 = True
    # compressed: hub root (agent ""), sessions api_seg0 (messages 0-59, compressed) and api_seg1 (60-119)
    fakes.hermes.add_session("api_seg0", [row(n) for n in range(60)], title="Bot Chat", last_active=10, started_at=1,
                             end_reason="compression")
    fakes.hermes.add_session("api_seg1", [row(n) for n in range(60, 120)], title="Bot Chat", last_active=20,
                             started_at=2, parent_session_id="api_seg0", end_reason=None)
    # photo-yes: the hub's /p/photo
    long = "\n\n".join(f"Point {i} : le relevé du compteur montre une consommation stable, rien d’anormal cette semaine."
                       for i in range(1, 15))
    fakes.hermes.add_session("api_photo", [
        {"role": "user", "content": [{"type": "text", "text": "Bonjour, voici la photo du compteur."},
                                     {"type": "image_url", "image_url": {"url": PHOTO}}]},
        {"role": "assistant", "content": "Merci, je regarde le relevé.\n\n" + long},
        {"role": "user", "content": "Yes"},
        {"role": "assistant", "content": "Parfait, je programme le rappel pour lundi."},
        {"role": "user", "content": "Et pour la facture ?"},
        {"role": "assistant", "content": "Je la vérifie dès qu’elle arrive."},
    ], title="Bot Chat", last_active=30, started_at=3, agent="photo")
    compressed = bridges.new("compressed", change=as_vie("", fakes.hermes.url)).start()
    photo = bridges.new("photo", change=as_vie("photo", f"{fakes.hermes.url}/p/photo")).start()
    out = {
        "key": BRIDGE_KEY,
        "compressed": {"url": compressed.url, "thread": main_thread(compressed),
                       "recent": uid("", "api_seg1", 41), "first": uid("", "api_seg0", 11)},
        "photo": {"url": photo.url, "thread": main_thread(photo), "yes_reply": uid("photo", "api_photo", 3)},
    }
    # the ids are the timeline's
    for name, bridge, ids in (("compressed", compressed, ("recent", "first")), ("photo", photo, ("yes_reply",))):
        for key in ids:
            got = bridge.get(f"/v1/threads/{out[name]['thread']}/items/{out[name][key]}", headers=AUTH)
            assert got.status_code == 200, (name, key, got.text)
    target = Path(os.environ.get("UI_BRIDGE_OUT", "/tmp/ui-bridge.json"))
    target.write_text(json.dumps(out))
    print(f"\n[ui-bridge] {json.dumps(out)}", flush=True)
    done = Path(str(target) + ".done")
    deadline = time.time() + 1200
    while not done.exists() and time.time() < deadline:
        time.sleep(1)
