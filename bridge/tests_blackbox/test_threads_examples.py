"""The example answers the app agents build against (docs/examples/*.json): made from the real bridge's answers, and
checked against them at each run (same keys, same types, recursively). `UPDATE_EXAMPLES=1` writes them again."""
import json
import os
from pathlib import Path

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until

EXAMPLES = Path(__file__).resolve().parents[2] / "docs" / "examples"
NOW = "2026-11-02T20:00:00.000Z"


def shape(value):
    if isinstance(value, dict):
        if value and all(k.startswith("t_") for k in value):   # keyed by thread ids (minted at each run)
            return {"<thread>": shape(next(iter(value.values())))}
        return {k: shape(v) for k, v in value.items()}
    if isinstance(value, list):
        return [shape(value[0])] if value else []
    return type(value).__name__


def check(name, real):
    path = EXAMPLES / f"{name}.json"
    if os.environ.get("UPDATE_EXAMPLES") == "1" or not path.exists():
        EXAMPLES.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(real, ensure_ascii=False, indent=2) + "\n")
    kept = json.loads(path.read_text())
    assert shape(kept) == shape(real), f"{path.name} no longer matches the bridge's answer: UPDATE_EXAMPLES=1"


def test_the_examples_match_the_bridge(bridges, fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_old", [
        {"role": "user", "content": "<!--bip:now lundi 9 h-->Quel train pour Lyon demain ?"},
        {"role": "assistant", "content": "", "tool_calls": [
            {"id": "call_1", "function": {"name": "web_search", "arguments": "{\"q\": \"train Paris Lyon\"}"}}]},
        {"role": "tool", "content": "3 résultats", "tool_call_id": "call_1", "tool_name": "web_search"},
        {"role": "assistant", "content": "Le train de 9 h part de la gare de Lyon, voie 4.\nMEDIA:/bipagents/pages/wellness/horaires.html"},
    ], title="Bot Chat", last_active=10, started_at=1, end_reason="compression")
    fakes.hermes.add_session("api_new", [
        {"role": "user", "content": [{"type": "text", "text": "Et ce billet ?"},
                                     {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}}]},
        {"role": "assistant", "content": "C'est un billet pour jeudi."},
    ], title="Bot Chat", last_active=20, started_at=2, parent_session_id="api_old")
    fakes.hermes.add_session("api_side", [{"role": "user", "content": "Vacances ?"}], title="Vacances",
                             last_active=5, started_at=0.5)
    bridge = bridges.new()
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     ("a" * 64, "production", "[]", NOW, NOW))])
    bridge.start()
    get = lambda path, **p: bridge.get(path, params=p, headers=AUTH).json()   # noqa: E731
    check("threads-list", get("/v1/threads", agent="wellness"))
    main = get("/v1/threads/main", agent="wellness")
    check("thread", main)
    check("timeline-main", get(f"/v1/threads/{main['id']}/timeline", limit=50))
    first = get(f"/v1/threads/{main['id']}/timeline")["items"][0]["id"]
    check("timeline-around", get(f"/v1/threads/{main['id']}/timeline", around=first, before_count=0))
    check("resolve", get("/v1/threads/resolve", agent="wellness", session_id="api_old"))
    fakes.hermes.add_session("api_new", fakes.hermes.messages[("", "api_new")] +
                             [{"role": "assistant", "content": "Bon voyage !"}], title="Bot Chat", last_active=30,
                             started_at=2, parent_session_id="api_old")
    fakes.hermes.runs["r1"] = [sse("run.completed", output="Bon voyage !")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_new"})
    assert wait_until(lambda: fakes.apns.payloads())
    check("push-reply-v2", fakes.apns.payloads()[0])
    check("notifications", get("/v1/notifications", agent="wellness"))
    check("notifications-unread", get("/v1/notifications/unread"))
    check("notifications-rules", get("/v1/notifications/rules"))
