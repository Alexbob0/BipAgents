"""An agent moving into its own space (bridge-rs/src/spaces.rs, own_space.rs) keeps what it had going, and a move that
doesn't finish is taken up again (token lab of 2026-11-07):

- its conversations made in the hub go on in its space: Hermes refuses a run in a conversation another profile home
  owns (409 `profile_mismatch`), so the move makes the copied ones its space's (`rebind_api_sessions`); a pair
  conversation with another agent that still can't be used is replaced by a new one, and a refusal of ours is never
  told to the asker as the other agent's silence (agent_pairs.rs);
- the hub's copy deleted later when Hermes won't delete it yet (« a running gateway still owns this profile », said
  in the log, not an empty « failed: »): the move stands, and a space a move left unfinished is removed and the move
  made again, never « a space with this name already exists » forever.

The space's gateway is the fake one (bbkit/fakebin.py `fake-gateway`): it answers runs at once and checks a
conversation's owner record in the profile's state.db as Hermes v0.22.5 does."""
import hashlib
import json
import sqlite3
import tomllib
from concurrent.futures import ThreadPoolExecutor

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import wait_registered
import test_own_space as own

MCP = "/v1/agent-tools/mcp"
TOKEN = "quotidien-token-0123456789abcdefghij"
BINDING = "gateway.api.binding.v1."


def own_tokens(bridge):
    """Quotidien's bridge token as the bridge keeps it (hashed), given again before each call: the bridge rewrites
    the file when it registers its agents' tools (a move does)."""
    path = bridge.state_dir / "agent-tools.json"
    kept = json.loads(path.read_text())["tokens"] if path.exists() else {}
    kept["quotidien"] = hashlib.sha256(TOKEN.encode()).hexdigest()
    path.write_text(json.dumps({"tokens": kept}))


def ask(bridge, message):
    own_tokens(bridge)
    body = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "message_agent", "arguments": {"target": "bip/coach", "message": message}}}
    result = bridge.post(MCP, json=body, headers={"Authorization": f"Bearer {TOKEN}"}, timeout=120).json()["result"]
    return result["content"][0]["text"], result["isError"]


def owner_record(database, session_id, home):
    """Hermes' record of who owns an API conversation (gateway/session_api.py), in a profile's state.db."""
    with sqlite3.connect(database) as db:
        db.execute("CREATE TABLE IF NOT EXISTS state_meta (key TEXT PRIMARY KEY, value TEXT)")
        db.execute("INSERT OR REPLACE INTO state_meta VALUES (?, ?)", (BINDING + session_id, json.dumps(
            {"profile_id": str(home), "session_id": session_id, "route": "agent:main:api_server:dm:" + session_id,
             "entry": {"session_id": session_id}})))


def book(bridge):
    return json.loads((bridge.state_dir / "agent-pairs.json").read_text())


def pair_session(bridge):
    return next(iter(book(bridge)["pairs"].values()))["sessions"]["coach"]


def runs_at(root):
    path = root / "fake-gateway-runs.jsonl"
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def test_a_pair_conversation_made_in_the_hub_goes_on_after_the_move(bridges, fakes):
    bridge, hub = own.hub_place(bridges, fakes, agents=("coach", "quotidien"), project="fete")
    bridge.start()
    wait_registered(bridge, ["coach", "quotidien", "fete"])
    links = [{"from": "quotidien", "to": "coach"}]
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": links}).status_code == 200
    fakes.hermes.run_output = "7 h 10 en moyenne."
    text, failed = ask(bridge, "Combien d'heures a-t-il dormi ?")
    assert not failed and "7 h 10" in text, text
    old = pair_session(bridge)
    # Hermes noted that conversation as the hub profile's
    owner_record(hub / "profiles" / "coach" / "state.db", old, (hub / "profiles" / "coach").resolve())
    # Coach moves into its own space (its first secret, in a hub that hosts a project agent)
    own.use_token_of(hub)
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(own.tool, bridge, "ask_secret", own.ASK)
        assert wait_until(lambda: own.secret_cards(bridge), timeout=60), bridge.log_tail(4000)
        card = own.secret_cards(bridge)[0]
        assert bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": False}).status_code == 200
        call.result(timeout=60)
    root = bridge.root / "spaces" / "coach"
    assert tomllib.loads(bridge.toml_text())["agents"]["coach"]["space"] == str(root)
    # the same conversation goes on there
    (root / "fake-gateway.json").write_text(json.dumps({"output": "Toujours 7 h 10, vu depuis mon espace."}))
    text, failed = ask(bridge, "Et cette nuit ?")
    assert not failed and "Coach (bip/coach) answered:" in text and "vu depuis mon espace" in text, text
    assert runs_at(root)[-1] == {"agent": "coach", "session_id": old, "refused": None}
    assert pair_session(bridge) == old
    assert "its API conversations made its space's" in bridge.log_tail(6000)
    # a conversation the move didn't carry (another home's): a new one, and the message goes through
    owner_record(root / "profiles" / "coach" / "state.db", "api-elsewhere", "/elsewhere/profiles/coach")
    edited = book(bridge)
    next(iter(edited["pairs"].values()))["sessions"]["coach"] = "api-elsewhere"
    (bridge.state_dir / "agent-pairs.json").write_text(json.dumps(edited))
    text, failed = ask(bridge, "Et demain ?")
    assert not failed and "vu depuis mon espace" in text, text
    assert [r["session_id"] for r in runs_at(root) if r["refused"]] == ["api-elsewhere"]
    assert pair_session(bridge).startswith("space-")
    # Hermes refuses whatever the bridge asks: said as a problem of ours, never as Coach's silence
    (root / "fake-gateway.json").write_text(json.dumps({"refuse": "admission_conflict"}))
    text, failed = ask(bridge, "Tu es là ?")
    assert failed and "did not reach Coach: a problem on the app's side" in text, text
    assert "didn't answer" not in text and "isn't answering" not in text


def test_a_move_whose_hub_delete_fails_stands_and_a_left_space_is_taken_up(bridges, fakes):
    """A project agent leaves the hub at start (own_space.rs `tidy_at_start`). A space an earlier move left
    unfinished is there: removed, the move made again. Hermes won't delete the hub's copy at once: the move stands,
    the log says why, and the delete is made again a moment later."""
    bridge, hub = own.hub_place(bridges, fakes, agents=("coach",), project="fete", tidy_after=1)
    bridge.config["spaces"]["forget_again"] = 0.5
    left = bridge.root / "spaces" / "fete"
    (left / "profiles" / "fete").mkdir(parents=True)
    (left / ".env").write_text("API_SERVER_PORT=1\n")
    (left / "LEFT-BY-AN-EARLIER-MOVE").write_text("x")
    cause = "Error: a running gateway still owns this profile; retry the delete once it settles"
    bridge.commands.rule("hermes", ["profile", "delete", "fete", "--yes"], exit=1, stdout=f"Deleting fete\n{cause}\n",
                         times=1)
    bridge.start()
    wait_registered(bridge, ["coach", "fete"])

    def moved():
        section = tomllib.loads(bridge.toml_text())["agents"].get("fete", {})
        return section.get("isolated") is True and section.get("space") == str(left)
    assert wait_until(moved, timeout=120), bridge.log_tail(4000)
    assert not (left / "LEFT-BY-AN-EARLIER-MOVE").exists()
    assert (left / "profiles" / "fete").is_dir()
    log = bridge.log_tail(6000)
    assert "a space left by a move that didn't finish" in log
    assert "the hub's copy of the moved agent not deleted yet" in log and cause in log
    deletes = lambda: [e for e in bridge.commands.of("hermes") if e == ["profile", "delete", "fete", "--yes"]]
    assert wait_until(lambda: len(deletes()) >= 2 and not (hub / "profiles" / "fete").is_dir(), timeout=30), \
        bridge.log_tail(4000)
    assert wait_until(lambda: "the hub forgot the moved agent" in bridge.log_tail(6000), timeout=10)
