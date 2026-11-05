"""A reply starting with an emoji (Alex 5/11): its words reach the apps whole, whatever way the emoji was cut.

Hermes writes its events with Python's `json.dumps` (ASCII escapes: 😊 = `\\ud83d\\ude0a`). A router that cuts a
JavaScript string between two tokens hands over half an emoji per delta (a lone surrogate escape each): no JSON
reader takes such a half, and the bridge passed it on as an unparsed `message` that both apps dropped, the emoji
lost. The bridge joins the halves across the deltas (hermes.rs `SSEParser`); a half alone becomes U+FFFD.
"""
import json

from bbkit.common import AUTH, read_sse

HALF = '"\\ud83d"', '"\\ude0a Bonjour"'


def raw_delta(run, quoted_text):
    """A delta as Hermes writes it (no `event:` line), its text already JSON (escapes as they are)."""
    return f'data: {{"event": "message.delta", "run_id": "{run}", "delta": {quoted_text}}}\n\n'


def test_an_emoji_in_two_halves_reaches_the_apps_whole(bridge, fakes):
    fakes.hermes.runs["run_e"] = [raw_delta("run_e", HALF[0]), raw_delta("run_e", HALF[1]),
                                  raw_delta("run_e", '" !"'),
                                  'data: {"event": "run.completed", "run_id": "run_e", "output": "\\ud83d\\ude0a Bonjour !"}\n\n']
    with bridge.stream("GET", "/v1/runs/run_e/events?agent=wellness", headers=AUTH) as resp:
        assert resp.status_code == 200
        events = read_sse(resp)
    names = [name for name, _ in events]
    assert "message" not in names, events            # never an unparsed event
    said = "".join(d.get("delta", "") for name, d in events if name == "message.delta")
    assert said == "😊 Bonjour !"
    assert events[-1][0] == "run.completed" and events[-1][1]["output"] == "😊 Bonjour !"
    # a follower that comes late gets the same words (the bridge's backlog)
    with bridge.stream("GET", "/v1/runs/run_e/events?agent=wellness", headers=AUTH) as resp:
        again = read_sse(resp)
    assert "".join(d.get("delta", "") for name, d in again if name == "message.delta") == said


def test_a_half_alone_is_a_replacement_character_not_a_lost_word(bridge, fakes):
    fakes.hermes.runs["run_h"] = [raw_delta("run_h", '"Oui \\ud83d"'), raw_delta("run_h", '" merci"'),
                                  'data: {"event": "run.completed", "run_id": "run_h", "output": "Oui merci"}\n\n']
    with bridge.stream("GET", "/v1/runs/run_h/events?agent=wellness", headers=AUTH) as resp:
        events = read_sse(resp)
    said = "".join(d.get("delta", "") for name, d in events if name == "message.delta")
    assert said == "Oui � merci"
    assert all(json.dumps(d) for _, d in events)
