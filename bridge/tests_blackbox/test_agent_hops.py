"""The hop cap between agents (bridge-rs/src/agent_hops.rs, Hermes 0.23 study §4.4-3, 6/11): agents writing to each
other with `message_agent` stop after 4 messages since their person last spoke, refused at once with a clear error;
the person gets a « Continuer » card (a bridge card, `bip-hops-…`) that starts the count over. A message back to an
agent waiting for this very exchange never waits on the pair's turn (until 6/11: the pair's lock, up to 600 s): A → B
→ A answers at once in A's conversation of the pair, B → A → B is told at once."""
import threading
import time

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_agent_messages import ask
from test_agent_pairs import linked

BOTH_WAYS = [{"from": "quotidien", "to": "coach"}, {"from": "coach", "to": "quotidien"}]
CAPPED = "Limite d'échanges entre agents atteinte : réponds à la personne ou demande-lui de continuer."


def hop_cards(bridge, agent="quotidien"):
    items = bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-hops-")]


def test_a_ping_pong_stops_at_the_cap_and_continuer_starts_it_over(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    linked(bridge, fakes, BOTH_WAYS)
    fakes.hermes.rounds = 1
    fakes.hermes.run_output = "Sept heures."
    # a single exchange, as before
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="Combien d'heures ?")
    assert not failed and text == "Coach (bip/coach) answered:\nSept heures.", text
    # A → B → A → B…: the next three go, the fifth is refused at once with the cap's error
    for who, target in (("coach", "bip/quotidien"), ("quotidien", "bip/coach"), ("coach", "bip/quotidien")):
        text, failed = ask(bridge, who, target=target, message="Et ensuite ?")
        assert not failed and "answered:" in text, text
    assert not hop_cards(bridge)
    # (an untracked turn of each agent counts per agent: Quotidien's chain has 2 of its own, Coach's 2)
    for _ in range(2):
        text, failed = ask(bridge, "quotidien", target="bip/coach", message="Encore ?")
        assert not failed, text
    runs = len(fakes.hermes.started)
    since = time.monotonic()
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="Et encore ?")
    assert time.monotonic() - since < 3
    assert failed and text.startswith(CAPPED), text
    assert "« Continuer »" in text and "Tell your person simply" in text
    assert len(fakes.hermes.started) == runs, "nothing sent"
    # the person's card, once, in Quotidien's conversation
    cards = hop_cards(bridge)
    assert len(cards) == 1, cards
    card = cards[0]
    assert card["choices"] == ["once", "deny"]
    assert card["bip_summary"] == "Quotidien et Coach ont échangé 4 messages entre eux sans toi. Continuer l'échange ?"
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="Toujours ?")
    assert failed and text.startswith(CAPPED)
    assert len(hop_cards(bridge)) == 1
    # « Continuer »: the count starts over
    answered = bridge.post("/v1/approve", headers=AUTH, json={"agent": "quotidien", "run_id": card["run_id"],
                                                               "choice": "once"})
    assert answered.status_code == 200, answered.text
    assert not hop_cards(bridge)
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="On continue ?")
    assert not failed and "answered:" in text, text
    # and Quotidien goes on, told the person said yes (card_place.rs `settle`)
    assert wait_until(lambda: [r for r in list(fakes.hermes.started.values())
                               if r["agent"] == "quotidien" and "<!--bip:card" in str(r["input"])
                               and "Continuer l'échange ?" in str(r["input"]) and "ALLOWED" in str(r["input"])],
                      timeout=20)
    # answered again: gone
    again = bridge.post("/v1/approve", headers=AUTH, json={"agent": "quotidien", "run_id": card["run_id"],
                                                           "choice": "once"})
    assert again.status_code == 404


def test_a_message_back_never_waits_on_the_pairs_turn(bridges, fakes):
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    linked(bridge, fakes, BOTH_WAYS)
    # the turns run until the test ends them (`run_info`), their event streams open (the hub follows them)
    fakes.hermes.rounds = 10 ** 6
    for n in range(1, 11):
        fakes.hermes.runs[f"r{n}"] = []
    out = {}

    def asking(key, who, target, message):
        started = time.monotonic()
        out[key] = (*ask(bridge, who, target=target, message=message), time.monotonic() - started)

    def run_of(agent):
        return [rid for rid, r in list(fakes.hermes.started.items()) if r["agent"] == agent]

    # Quotidien asks Coach: Coach's turn goes on
    first = threading.Thread(target=asking, args=("first", "quotidien", "bip/coach", "Combien d'heures ?"))
    first.start()
    assert wait_until(lambda: run_of("coach"), timeout=10)
    coach_run = run_of("coach")[0]
    # from that turn, Coach writes back to Quotidien: Quotidien's turn starts at once, in its own conversation
    back = threading.Thread(target=asking, args=("back", "coach", "bip/quotidien", "Tu as le menu ?"))
    since = time.monotonic()
    back.start()
    assert wait_until(lambda: run_of("quotidien"), timeout=5)
    assert time.monotonic() - since < 5
    quotidien_run = run_of("quotidien")[0]
    assert fakes.hermes.started[quotidien_run]["session_id"] != fakes.hermes.started[coach_run]["session_id"]
    # from Quotidien's turn, to Coach again (B → A → B): Coach waits for this exchange, told at once, nothing sent
    since = time.monotonic()
    text, failed = ask(bridge, "quotidien", target="bip/coach", message="Et hier ?")
    assert time.monotonic() - since < 3
    assert failed and "is itself waiting for your answer in this same exchange" in text, text
    assert len(run_of("coach")) == 1
    # the turns end, inner first: each ask gets its answer, well under the old 600 s
    fakes.hermes.run_info[quotidien_run] = {"status": "completed", "output": "Oui, le menu est prêt."}
    back.join(timeout=15)
    assert not back.is_alive()
    assert out["back"][:2] == ("Quotidien (bip/quotidien) answered:\nOui, le menu est prêt.", False), out["back"]
    fakes.hermes.run_info[coach_run] = {"status": "completed", "output": "Sept heures."}
    first.join(timeout=15)
    assert not first.is_alive()
    assert out["first"][:2] == ("Coach (bip/coach) answered:\nSept heures.", False), out["first"]
    assert out["first"][2] < 30
    # the thread: both asks and both answers, not the refused one
    pair = bridge.get("/v1/agent-pairs", params={"agent": "coach"}, headers=AUTH).json()["pairs"][0]
    thread = bridge.get(f"/v1/agent-pairs/{pair['id']}", headers=AUTH).json()["thread"]
    assert [(m["from"], m["kind"]) for m in thread] == [
        ("quotidien", "ask"), ("coach", "ask"), ("quotidien", "answer"), ("coach", "answer")]
