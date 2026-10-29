"""Two agents' own conversation « A ⇄ B » (bridge-rs/src/agent_pairs.rs, 2026-10-29): a message between two linked
agents lands in the pair's conversation (a hidden one in the target's Hermes), not in the target's Bot Chat, the
person's main conversation with it; the asker gets the answer in the same call; the pair, its exchanges (the lines
the apps place in both agents' main conversations) and its thread are listed for both agents (their « Historique »);
a relative never sees them."""
import yaml

from bbkit.common import AUTH
from conftest import managed_install, profile, wait_registered
from test_agent_messages import TOKENS, ask, give_tokens
from test_shared_agents import link
from test_households_and_peers import household


def linked(bridge, fakes, links):
    hub = managed_install(bridge, fakes, agents=("quotidien", "coach", "budget"))
    for name in ("quotidien", "coach", "budget"):
        (profile(hub, name) / "config.yaml").write_text("model:\n  provider: custom\n  base_url: http://gpu:8888/v1\n")
    bridge.start()
    wait_registered(bridge, ["quotidien", "coach", "budget"])
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": links}).status_code == 200
    give_tokens(bridge, TOKENS)
    return hub


def test_a_message_between_two_agents_lands_in_their_own_conversation(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    hub = linked(bridge, fakes, [{"from": "quotidien", "to": "coach"}, {"from": "coach", "to": "quotidien"}])
    assert yaml.safe_load((profile(hub, "quotidien") / "config.yaml").read_text())   # still readable
    # Coach's main conversation is there already: it is left alone
    fakes.hermes.add_session("botchat-coach", [], title="Bot Chat", agent="coach", last_active=1.0)
    fakes.hermes.run_output = "Tu as dormi 7 h 10 en moyenne cette semaine."
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="Combien d'heures a-t-il dormi ?")
    assert not failed and "Coach (bip/coach) answered:\nTu as dormi 7 h 10" in text, text

    # the target answered in the pair's own conversation, hidden in its Hermes, never in its Bot Chat
    created = [(agent, body["title"]) for agent, body in fakes.hermes.created]
    assert len(created) == 1 and created[0][0] == "coach" and created[0][1].startswith("BipAgents · ⇄ p")
    pair_session = next(sid for sid, s in fakes.hermes.sessions["coach"].items() if s["title"] == created[0][1])
    assert {"id": pair_session, "title": created[0][1], "hidden": True} in fakes.hermes.patched
    run = [r for r in fakes.hermes.started.values() if r["agent"] == "coach"][-1]
    assert run["session_id"] == pair_session and run["session_id"] != "botchat-coach"
    assert run["input"].startswith("<!--bip:pair\nThis is your own conversation with Quotidien (@quotidien)")
    assert run["input"].endswith("\n-->\nMessage from Quotidien (@quotidien): Combien d'heures a-t-il dormi ?")
    assert not fakes.hermes.chats

    # both agents list the pair (their « Historique »), with its exchange: the line of their main conversations
    pairs = bridge.get("/v1/agent-pairs", params={"agent": "coach"}, headers=AUTH).json()["pairs"]
    assert len(pairs) == 1
    pair = pairs[0]
    assert [a["id"] for a in pair["agents"]] == ["coach", "quotidien"]
    assert [a["name"] for a in pair["agents"]] == ["Coach", "Quotidien"]
    assert pair["messages"] == 2 and pair["last"]["kind"] == "answer" and pair["last"]["from"] == "coach"
    assert [(e["from"], e["count"]) for e in pair["exchanges"]] == [("quotidien", 2)]
    assert bridge.get("/v1/agent-pairs", params={"agent": "quotidien"}, headers=AUTH).json()["pairs"][0]["id"] == pair["id"]
    assert bridge.get("/v1/agent-pairs", params={"agent": "budget"}, headers=AUTH).json() == {"pairs": []}
    thread = bridge.get(f"/v1/agent-pairs/{pair['id']}", headers=AUTH).json()["thread"]
    assert [(m["from"], m["to"], m["kind"], m["text"]) for m in thread] == [
        ("quotidien", "coach", "ask", "Combien d'heures a-t-il dormi ?"),
        ("coach", "quotidien", "answer", "Tu as dormi 7 h 10 en moyenne cette semaine.")]
    assert {m["exchange"] for m in thread} == {pair["exchanges"][0]["id"]}

    # the other way round: the same pair, Quotidien answering in a hidden conversation of its own
    fakes.hermes.run_output = "Le menu de la semaine est prêt."
    text, failed = ask(bridge, "coach", target="bip/quotidien", message="Tu as le menu ?")
    assert not failed and "Le menu de la semaine" in text
    assert [a for a, _ in fakes.hermes.created] == ["coach", "quotidien"]
    pair = bridge.get("/v1/agent-pairs", params={"agent": "quotidien"}, headers=AUTH).json()["pairs"][0]
    assert pair["messages"] == 4 and [(e["from"], e["count"]) for e in pair["exchanges"]] == [("quotidien", 4)]
    # and its conversation is used again
    text, failed = ask(bridge, "quotidien", target="Coach", message="Et hier ?")
    assert not failed and len(fakes.hermes.created) == 2
    runs = [r for r in fakes.hermes.started.values() if r["agent"] == "coach"]
    assert runs[-1]["session_id"] == runs[0]["session_id"] == pair_session

    # the person's app only
    assert bridge.get("/v1/agent-pairs", params={"agent": "coach"}).status_code == 401
    assert bridge.get(f"/v1/agent-pairs/{pair['id']}").status_code == 401
    assert bridge.get("/v1/agent-pairs", params={"agent": "ghost"}, headers=AUTH).status_code == 404
    assert bridge.get("/v1/agent-pairs/p000000000000", headers=AUTH).status_code == 404


def test_a_relative_never_sees_the_pair_conversation(bridges, fakes):
    a = household(bridges, fakes, "alex", ["quotidien", "coach"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    assert a.put("/v1/agents/coach/shares", headers=AUTH, json={"households": ["marie"]}).status_code == 200
    assert b.post("/v1/shared-agents/coach__alex/add", headers=AUTH).status_code == 200
    assert a.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "quotidien", "to": "coach"}]}).status_code == 200
    give_tokens(a, TOKENS)
    fakes.hermes.run_output = "Sept heures."
    text, failed = ask(a, "quotidien", target="bip/coach", message="Combien d'heures ?")
    assert not failed and "Sept heures." in text
    assert len(a.get("/v1/agent-pairs", params={"agent": "coach"}, headers=AUTH).json()["pairs"]) == 1
    # Marie's view of Coach: her own conversations only, never the pair's
    mine = b.get("/v1/shared-agents/coach__alex/conversations", headers=AUTH)
    assert mine.status_code == 200, mine.text
    assert mine.json()["conversations"] == []
    # her bridge has no way to Alex's pairs
    assert b.get("/v1/agent-pairs", params={"agent": "coach__alex"}, headers=AUTH).status_code == 404
