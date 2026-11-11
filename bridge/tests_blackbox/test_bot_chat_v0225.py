"""Each agent's main conversation (its « Bot Chat ») on Hermes v0.22.5 (release bench 2026-11-02).

v0.22.5 treats the conversation titled « Bot Chat » as Bot Mode's canonical chat: a local one whose text turns go to
Hermes' peer queue (the run's stream is only `run.completed`: no text, no tools, no usage, no clarify), and an
API-made one is moved aside as « Bot Chat (API <id>) ». Hermes patch 0021 keeps a Bot Chat made by a plain
`POST /api/sessions` an API conversation; the fake Hermes plays v0.22.5 with it (bbkit/fakes.py: a create with a
peer's source mints the local chat, whose runs go to `fifo`). Here: the bridge makes the main conversation that way,
a peer opening it too, and a thread an unpatched Hermes moved aside is put back at the bridge's start."""
import time
import tomllib

from bbkit.common import AUTH, read_sse, sse
from bbkit.fakes import CLOSE
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered


def test_a_new_agents_main_conversation_is_an_api_one_and_streams(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    managed_install(bridge, fakes, agents=(), public_hub_url="https://mini.ts.net:8642")
    bridge.start()
    made = bridge.post("/v1/agents", headers=AUTH, json={"name": "Léa", "kind": "wellness"})
    assert made.status_code == 201, made.text
    [bot_chat] = [sid for sid, s in fakes.hermes.sessions["lea"].items() if s["title"] == "Bot Chat"]
    assert [body for agent, body in fakes.hermes.created if agent == "lea"] == [{"title": "Bot Chat"}]
    assert not fakes.hermes.local_bot_chat("lea", bot_chat)
    # the app's turn in it, through the door: a run whose text streams, not a peer's message
    key = tomllib.loads(bridge.toml_text())["agents"]["lea"]["hermes_key"]
    auth = {"Authorization": f"Bearer {key}"}
    # the run's events ready before it starts, as Hermes has them once it answers 202: the bridge follows a relayed
    # run at once (runs.rs keep_started), and a script set after the start raced it (run_not_found 404)
    run_id = f"r{len(fakes.hermes.started) + 1}"
    fakes.hermes.runs[run_id] = [sse("message.delta", run_id=run_id, delta="Bonjour"),
                                 sse("run.completed", run_id=run_id, output="Bonjour"), CLOSE]
    started = bridge.post("/hermes/lea/v1/runs", headers=auth, json={"input": "Salut", "session_id": bot_chat})
    assert started.status_code == 202, started.text
    assert started.json()["run_id"] == run_id, started.text
    with bridge.stream("GET", f"/hermes/lea/v1/runs/{run_id}/events", headers=auth) as resp:
        events = read_sse(resp)
    assert [name for name, _ in events][:2] == ["message.delta", "run.completed"], events
    assert fakes.hermes.fifo == []


def test_a_thread_hermes_moved_aside_is_the_agents_bot_chat_again_at_start(bridges, fakes):
    """What an unpatched v0.22.5 left (the bench's lab place): the thread renamed « Bot Chat (API <id>) » with its
    history, a new local « Bot Chat » (hidden) holding the few turns sent since. At start the bridge archives and hides
    the new one and gives the thread its title back; at the next start nothing more."""
    bridge = bridges.new()
    hub = managed_install(bridge, fakes, agents=("coach",))
    profile(hub, "coach")
    fakes.hermes.add_session("local-9f", [{"role": "user", "content": "Salut"}] * 2, title="Bot Chat", agent="coach",
                             hidden=True, chat_id="local-9f", source="cli")
    fakes.hermes.add_session("api_1_a", [{"role": "user", "content": "Bonjour"}] * 40,
                             title="Bot Chat (API api_1_a)", agent="coach", last_active=time.time(), source="api_server")
    bridge.start()
    wait_registered(bridge, ("coach",))
    assert wait_until(lambda: {"id": "api_1_a", "title": "Bot Chat"} in fakes.hermes.patched, 15), fakes.hermes.patched
    assert fakes.hermes.patched[0] == {"id": "local-9f", "hidden": True, "archived": True}
    sessions = fakes.hermes.sessions["coach"]
    assert sessions["api_1_a"]["title"] == "Bot Chat" and sessions["local-9f"]["archived"] is True
    fakes.hermes.patched.clear()
    bridge.restart()
    wait_registered(bridge, ("coach",))
    time.sleep(1)
    assert fakes.hermes.patched == []
