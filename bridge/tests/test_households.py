"""Two households (two bridges) linked by an invitation, their agents writing to each other along the links each
person turned on. Both bridges run in memory; their HTTP goes through a router transport to each other's app or to
a fake Hermes that echoes what it received."""
import asyncio
import copy
import json
import time

import httpx
import pytest
from fastapi.testclient import TestClient

from bipbridge.app import create_app
from bipbridge.config import parse_config
from bipbridge.households import Households, HouseholdError
from conftest import AUTH, BRIDGE_KEY

HERMES_RECEIVED = []


class Router(httpx.AsyncBaseTransport):
    """a.test → Alex's bridge, b.test → Léa's bridge, *.hermes → a fake Hermes that echoes the request."""

    def __init__(self):
        self.apps = {}

    async def handle_async_request(self, request: httpx.Request) -> httpx.Response:
        host = request.url.host
        if host in self.apps:
            return await httpx.ASGITransport(app=self.apps[host]).handle_async_request(request)
        body = (await request.aread()).decode() or "{}"
        HERMES_RECEIVED.append((str(request.url), request.headers.get("authorization"), json.loads(body)))
        return httpx.Response(200, json={"ok": True, "url": str(request.url)})


def make_bridge(config_dict, tmp_path, name, agents, router):
    data = copy.deepcopy(config_dict)
    data["public_url"] = f"http://{name}.test"
    data["outbox"] = {"db_path": str(tmp_path / name / "bridge.db")}
    data["agents"] = {a.title(): {"hermes_url": f"http://{name}.hermes/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                      for a in agents}
    app = create_app(parse_config(data))
    router.apps[f"{name}.test"] = app
    return app


@pytest.fixture
def pair(config_dict, tmp_path):
    HERMES_RECEIVED.clear()
    router = Router()
    alex = make_bridge(config_dict, tmp_path, "a", ["vie", "budget"], router)
    lea = make_bridge(config_dict, tmp_path, "b", ["cuisine"], router)
    with TestClient(alex) as a, TestClient(lea) as b:
        for client in (a, b):
            client.app.state.services.http = httpx.AsyncClient(transport=router)
        yield a, b


def test_invitations_are_one_time_and_expire(tmp_path):
    households = Households(str(tmp_path / "households.json"))
    with pytest.raises(HouseholdError):
        households.invite()                                    # name the household first
    households.set_name("Alex")
    code = households.invite()
    answer = households.accept(code, "Léa", "https://b.ts.net", "k" * 40)
    assert households.caller("k" * 40) == "lea" and len(answer["key"]) >= 32 and answer["name"] == "Alex"
    assert "k" * 40 not in households.path.read_text()        # only a digest of their key
    with pytest.raises(HouseholdError):
        households.accept(code, "Léa", "https://b.ts.net", "k" * 40)   # used once
    late = households.invite()
    data = households.load()
    data["invites"] = {k: time.time() - 1 for k in data["invites"]}
    households.save(data)
    with pytest.raises(HouseholdError):
        households.accept(late, "Paul", "https://c.ts.net", "p" * 40)


def test_two_households_write_to_each_other_only_where_both_allowed(pair):
    a, b = pair
    assert a.put("/v1/family/name", headers=AUTH, json={"name": "Alex"}).status_code == 200
    assert b.put("/v1/family/name", headers=AUTH, json={"name": "Léa"}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    assert invitation["kind"] == "family" and invitation["url"] == "http://a.test"
    joined = b.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]})
    assert joined.status_code == 200 and joined.json() == {"id": "alex", "name": "Alex"}
    assert a.get("/v1/family", headers=AUTH).json()["households"] == [{"id": "lea", "name": "Léa", "url": "http://b.test"}]
    # Each side lists the other's agents among its own.
    names = {x["id"]: x["name"] for x in a.get("/v1/agent-links", headers=AUTH).json()["agents"]}
    assert names["cuisine__lea"] == "Cuisine (Léa)" and "vie" in names

    # Alex lets Vie write to Cuisine; Léa has not allowed it yet: her bridge refuses.
    services = a.app.state.services
    assert a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie", "to": "cuisine__lea"}]}).status_code == 200
    token = "vie-gateway-token-" + "z" * 20
    data = services.links.load()
    data["tokens"]["manual:vie"] = __import__("hashlib").sha256(token.encode()).hexdigest()
    services.links.save(data)
    message = {"message": "Message from Vie (@vie): qu'est-ce qu'on mange ce soir ?"}
    send = lambda: a.post("/peer/p/cuisine__lea/v1/runs", headers={"Authorization": f"Bearer {token}"}, json=message)
    assert send().status_code == 403 and HERMES_RECEIVED == []
    # Léa allows « Vie (Alex) → Cuisine »: delivered to Cuisine, signed with its household and its answer address.
    assert b.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "vie__alex", "to": "cuisine"}]}).status_code == 200
    delivered = send()
    assert delivered.status_code == 200
    url, auth, body = HERMES_RECEIVED[-1]
    assert url == "http://b.hermes/p/cuisine/v1/runs" and auth == "Bearer cuisine-key-" + "x" * 24
    assert body["message"] == "Message from Vie (Alex) (@vie__alex): qu'est-ce qu'on mange ce soir ?"
    # Budget (its own gateway, not linked) cannot, nor can anything without the household's key.
    budget_token = "budget-gateway-token-" + "w" * 20
    data = services.links.load()
    data["tokens"]["manual:budget"] = __import__("hashlib").sha256(budget_token.encode()).hexdigest()
    services.links.save(data)
    assert a.post("/peer/p/cuisine__lea/v1/runs", headers={"Authorization": f"Bearer {budget_token}"},
                  json={"message": "Message from Budget (@budget): coucou"}).status_code == 403
    assert b.post("/family/p/cuisine/v1/runs", headers={"Authorization": "Bearer nope"}, json=message).status_code == 401

    # Unlinking the household drops its links.
    assert b.delete("/v1/family/alex", headers=AUTH).status_code == 204
    assert b.get("/v1/agent-links", headers=AUTH).json()["links"] == []
    assert send().status_code in (401, 403)
