"""What the agent does now (turn_status.rs, docs/conversations.md §6.5b; Alex 16/11: « on attend 5-6 min et on sait
pas trop ce que l'agent essaye de faire »): an app that asks (`status=1`) gets one `turn.status` per change in the
run's stream, with a page's bare domain and never a call's arguments; an app that doesn't ask gets none, live or
replayed; off by default (`[status] enabled = false`)."""
import json

from bbkit.common import AUTH, read_sse, sse
from bbkit.servers import wait_until

from test_threads_send import SESSION, agent, send, user

CARD = "4970 1012 3456 7890"


def start(bridges, fakes, enabled):
    fakes.hermes.v0225 = True
    fakes.hermes.rounds = 1000
    fakes.hermes.add_session(SESSION, [user("Bonjour"), agent("Salut")], title="Bot Chat", last_active=10, started_at=1)
    change = (lambda c: c.update(status={"enabled": True})) if enabled else None
    bridge = bridges.new(change=change).start()
    thread = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "wellness"}).json()["id"]
    fakes.hermes.runs["r1"] = [
        sse("run.started", run_id="r1"),
        sse("tool.started", run_id="r1", tool="mcp__stealth__type_text", tool_call_id="a",
            preview=f"https://www.example-shop.fr/checkout/payment?card={CARD.replace(' ', '')} {CARD}"),
        sse("tool.completed", run_id="r1", tool="mcp__stealth__type_text", tool_call_id="a", duration=0.4, error=False),
        sse("message.delta", run_id="r1", delta="C'est fait."),
        sse("run.completed", run_id="r1", output="C'est fait.")]
    assert send(bridge, thread, "C5D00000-0000-4000-8000-0000000000F1", "Paie la commande").json()["state"] == "started"
    return bridge


def events(bridge, asked):
    params = {"agent": "wellness", **({"status": "1"} if asked else {})}
    with bridge.stream("GET", "/v1/runs/r1/events", params=params, headers=AUTH, timeout=30) as resp:
        assert resp.status_code == 200
        return read_sse(resp, timeout=60)


def test_an_app_that_asks_reads_what_the_agent_does_and_never_its_arguments(bridges, fakes):
    bridge = start(bridges, fakes, enabled=True)
    seen = events(bridge, asked=True)
    names = [name for name, _ in seen]
    statuses = [data for name, data in seen if name == "turn.status"]
    assert statuses, names
    first = statuses[0]
    assert names.index("turn.status") == names.index("tool.started") + 1, "right after the event that changed it"
    assert (first["state"], first["kind"], first["action"], first["site"]) == ("tool", "browser", "fill", "example-shop.fr")
    assert first["run_id"] == "r1" and first["v"] == 1 and first["since"] <= first["now"]
    # in the agent's language (this place's agent speaks English)
    assert first["text"] in ("On example-shop.fr: filling in a form", "Sur example-shop.fr : je remplis un formulaire")
    assert statuses[-1]["state"] == "model", "the model's turn again once the call ended"
    for status in statuses:
        raw = json.dumps(status, ensure_ascii=False)
        assert "4970" not in raw and "7890" not in raw and "checkout" not in raw, raw


def test_an_app_that_does_not_ask_gets_none_live_or_replayed(bridges, fakes):
    bridge = start(bridges, fakes, enabled=True)
    assert "turn.status" not in [name for name, _ in events(bridge, asked=False)]
    # the same run asked for again (its replay): only the app that asks gets them
    assert "turn.status" in [name for name, _ in events(bridge, asked=True)]
    assert "turn.status" not in [name for name, _ in events(bridge, asked=False)]


def test_off_by_default(bridges, fakes):
    bridge = start(bridges, fakes, enabled=False)
    assert "turn.status" not in [name for name, _ in events(bridge, asked=True)]


def test_a_background_tasks_activity_says_what_it_does_now(bridges, fakes):
    """The task's card: the same status as a conversation's working line, its `started` for « depuis 12 min »."""
    bridge = bridges.new(change=lambda c: c.update(status={"enabled": True})).start()
    fakes.hermes.rounds = 1000
    fakes.hermes.runs["r1"] = [sse("tool.started", tool="mcp__bipagents__mail_read", tool_call_id="m",
                                   preview="Objet : résultats d'analyses")]
    task = {"title": "Trier les e-mails", "prompt": "Trie mes e-mails de 2025.", "budget_eur": 0.5}
    job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=task).json()

    def status():
        jobs = bridge.get("/v1/jobs?agent=wellness", headers=AUTH).json()["jobs"]
        return ((jobs[0].get("activity") or {}).get("status") or {}) if jobs else {}

    assert wait_until(lambda: status().get("state") == "tool", timeout=15)
    found = status()
    assert found["kind"] == "mailRead" and found["site"] is None
    assert "analyses" not in json.dumps(found, ensure_ascii=False), "never a call's arguments"
    assert bridge.delete(f"/v1/jobs/{job['id']}", headers=AUTH).status_code == 200


def test_the_brake_route_answers_plainly(bridges, fakes):
    """« Continuer » / « Arrêter » on a loop (POST /v1/runs/{id}/brake): a run over or unknown is gone, an unknown
    action refused (the loop itself: runs.rs the_brakes_two_thirds_are_told_to_the_person_who_may_go_on_once_or_stop)."""
    bridge = start(bridges, fakes, enabled=True)
    events(bridge, asked=True)  # the run is over

    def post(run, action):
        return bridge.post(f"/v1/runs/{run}/brake", params={"agent": "wellness"}, headers=AUTH, json={"action": action})

    assert post("r1", "jump").status_code == 400
    assert post("r1", "continue").status_code == 404
    assert post("r9", "stop").status_code == 404
