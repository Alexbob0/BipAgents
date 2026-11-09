"""A self-install at home (its address on a tailnet: the internet can't reach it) linked to a place of the offer (a
public address), family_relay.rs: the place can't call the home, so the home's bridge fetches the place's calls for it
and answers them. Two bridge processes; the home's announced address (`public_url`) is a tailnet one nothing answers
at, as from a place of the offer.

- The place scanning the home's invitation is refused at once, saying why (`their_side_unreachable`, `private`): the
  apps then say who should scan whose invitation.
- The home scanning the place's invitation links them; the place lists the home's agents and its agents write to
  them through the relay, the home's write to the place directly.
- The home switched off: the place's family page and its agent links still open at once, the home's call failing as a
  household that doesn't answer (502)."""
import json
import time

from starlette.responses import JSONResponse

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_households_and_peers import give_peer_token

RECEIVED = []
HOME_ADDRESS = "http://100.100.100.1:9"   # a tailnet address: nothing answers there from the place


def echo(seen):
    """A Hermes whose Bot Chat is « bot-1 » (the pair's conversation « pair-1 »), answering a run with what it got."""
    if seen.method == "GET" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"data": [{"id": "bot-1", "title": "Bot Chat"}]})
    if seen.method == "POST" and seen.path.endswith("/api/sessions"):
        return JSONResponse({"session": {"id": "pair-1"}})
    if seen.method == "PATCH":
        return JSONResponse({})
    if seen.method == "GET" and "/api/sessions/pair-1/messages" in seen.path:
        return JSONResponse({"messages": [{"role": "assistant", "content": "D'accord."}]})
    if seen.method == "GET" and seen.path.endswith("/api/jobs"):
        return JSONResponse({"jobs": []})
    if seen.method == "GET" and "/v1/runs/" in seen.path:
        return JSONResponse({"status": "running", "session_id": "bot-1"})
    RECEIVED.append((seen.path, seen.json()))
    return JSONResponse({"ok": True, "path": seen.path})


def install(bridges, fakes, name, agents, place):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in agents}
    bridge = bridges.new(name, change=change)
    if place:
        bridge.config["cloud"] = True
        bridge.config["public_url"] = bridge.url
    else:
        bridge.config["public_url"] = HOME_ADDRESS
    return bridge.start()


def named(bridge, name):
    assert bridge.put("/v1/family/name", headers=AUTH, json={"name": name}).status_code == 200


def households(bridge):
    return json.loads((bridge.state_dir / "households.json").read_text())["households"]


def link_agents(bridge):
    return {a["id"] for a in bridge.get("/v1/agent-links", headers=AUTH).json()["agents"]}


def test_a_place_scanning_a_homes_invitation_is_told_why_at_once(bridges, fakes):
    fakes.hermes.override = echo
    home = install(bridges, fakes, "alex", ["vie"], place=False)
    place = install(bridges, fakes, "chi", ["cuisine"], place=True)
    named(home, "Alex")
    named(place, "Chi")
    invitation = home.post("/v1/family/invite", headers=AUTH).json()
    assert invitation["url"] == HOME_ADDRESS
    started = time.monotonic()
    refused = place.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert time.monotonic() - started < 3
    assert refused.status_code == 502
    assert refused.json()["detail"]["code"] == "their_side_unreachable" and refused.json()["detail"]["private"] is True
    # nothing is linked on either side, and the place's family page opens as before
    assert place.get("/v1/family", headers=AUTH).json()["households"] == []
    assert home.get("/v1/family", headers=AUTH).json()["households"] == []


def test_a_home_links_to_a_place_which_reaches_it_through_the_relay(bridges, fakes):
    RECEIVED.clear()
    fakes.hermes.override = echo
    home = install(bridges, fakes, "alex", ["vie"], place=False)
    place = install(bridges, fakes, "chi", ["cuisine"], place=True)
    named(home, "Alex")
    named(place, "Chi")
    invitation = place.post("/v1/family/invite", headers=AUTH).json()
    joined = home.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200 and joined.json() == {"id": "chi", "name": "Chi"}
    # the place keeps its calls for the home (a relay token), the home fetches them
    assert households(home)["chi"]["pull"] is True and "relay" not in households(home)["chi"]
    kept = households(place)["alex"]
    assert kept["url"] == HOME_ADDRESS and len(kept["relay"]) >= 16 and "pull" not in kept
    assert place.get("/v1/family", headers=AUTH).json()["households"] == [
        {"id": "alex", "name": "Alex", "url": HOME_ADDRESS, "relink": False}]

    # The place lists the home's agents: its call went through the home's long poll.
    assert wait_until(lambda: "vie__alex" in link_agents(place), timeout=20), place.log_tail()
    assert "cuisine__chi" in link_agents(home)
    # A relay is only for a household that pulls: nobody else gets the place's calls
    assert place.get("/family/relay/next", headers={"Authorization": "Bearer nope"}).status_code == 401

    # Cuisine (Chi) writes to Vie (Alex): both allowed it, delivered at the home through the relay.
    assert place.put("/v1/agent-links", headers=AUTH,
                     json={"links": [{"from": "cuisine", "to": "vie__alex"}]}).status_code == 200
    assert home.put("/v1/agent-links", headers=AUTH,
                    json={"links": [{"from": "cuisine__chi", "to": "vie"}]}).status_code == 200
    token = "cuisine-gateway-token-" + "z" * 20
    give_peer_token(place, "manual:cuisine", token)
    message = {"session_id": "bot-1", "message": "Message from Cuisine (@cuisine): des pâtes ce soir ?"}
    delivered = place.post("/peer/p/vie__alex/v1/runs", headers={"Authorization": f"Bearer {token}"}, json=message)
    assert delivered.status_code == 200 and delivered.json() == {"ok": True, "path": "/p/vie/v1/runs"}
    path, body = RECEIVED[-1]
    assert path == "/p/vie/v1/runs" and "Message from Cuisine (Chi) (@cuisine__chi): des pâtes ce soir ?" in body["message"]

    # Vie (Alex) answers Cuisine (Chi): the home calls the place directly.
    assert home.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "cuisine__chi", "to": "vie"},
                                                                     {"from": "vie", "to": "cuisine__chi"}]}).status_code == 200
    assert place.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "cuisine", "to": "vie__alex"},
                                                                      {"from": "vie__alex", "to": "cuisine"}]}).status_code == 200
    vie = "vie-gateway-token-" + "y" * 20
    give_peer_token(home, "manual:vie", vie)
    back = home.post("/peer/p/cuisine__chi/v1/runs", headers={"Authorization": f"Bearer {vie}"},
                     json={"session_id": "bot-1", "message": "Message from Vie (@vie): oui !"})
    assert back.status_code == 200 and back.json() == {"ok": True, "path": "/p/cuisine/v1/runs"}

    # The home switched off: the place's pages open at once all the same, the home's call failing plainly.
    home.stop()
    started = time.monotonic()
    assert place.get("/v1/family", headers=AUTH).status_code == 200
    assert "vie__alex" not in link_agents(place)   # its 4 s wait, not a hang
    assert time.monotonic() - started < 10
    gone = place.post("/peer/p/vie__alex/v1/runs", headers={"Authorization": f"Bearer {token}"}, json=message)
    assert gone.status_code == 502
