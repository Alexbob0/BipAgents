"""A very structured reply's hidden spoken version (Alex 10/11, spoken.rs): the agent ends it with `<!--bip:say …-->`,
read aloud when the reply is listened to; the person never sees it. The bridge takes it out of everything it passes
on (a run's deltas, even a mark cut across them, and its end's `output`; a Live turn's stream; the timeline; the
Boîte; a notification; the search) and gives it as `spoken` (the end event, the timeline's message item). The radio
rules: in a Live call's brief, and in the hidden note joined to a voice message."""
import json

import httpx

from bbkit.common import AUTH, boite_message, read_sse, sse
from bbkit.fakes import split_brief_now, taken_notes
from bbkit.servers import wait_until
from conftest import managed_install, profile
from test_message_search import store
from test_notifications_journal import with_device

SAY = "<!--bip:say J'ai trouvé quatre hôtels. Le moins cher a une terrasse : 95 euros.-->"
SPOKEN = "J'ai trouvé quatre hôtels. Le moins cher a une terrasse : 95 euros."
WORDS = "Quatre hôtels :\n- Ibis 95 €\n- Mercure 120 €\n- Novotel 140 €\n- Sofitel 210 €"
REPLY = f"{WORDS}\n\n{SAY}"


def pieces(text, step):
    return [text[i:i + step] for i in range(0, len(text), step)]


def test_a_runs_events_never_carry_it_and_its_end_gives_it_as_spoken(bridge, fakes):
    for n, step in enumerate([1, 3, 7, 40]):
        run = f"run_s{n}"
        fakes.hermes.runs[run] = [sse("message.delta", run_id=run, delta=p) for p in pieces(REPLY, step)] \
            + [sse("run.completed", run_id=run, output=REPLY)]
        with bridge.stream("GET", f"/v1/runs/{run}/events?agent=wellness", headers=AUTH) as resp:
            assert resp.status_code == 200
            events = read_sse(resp)
        assert all("bip:" not in json.dumps(data) for _, data in events), step
        said = "".join(d.get("delta", "") for name, d in events if name == "message.delta")
        assert said.rstrip() == WORDS, step
        name, end = events[-1]
        assert name == "run.completed" and end["output"] == WORDS and end["spoken"] == SPOKEN
        assert "bip:say" not in json.dumps(end) and "-->" not in json.dumps(end)


def test_a_live_turn_streams_without_it(bridge, fakes):
    hermes = fakes.hermes
    hermes.stream_script = [sse("assistant.delta", run_id="run_l", delta=p) for p in pieces(REPLY, 5)] \
        + [sse("assistant.completed", run_id="run_l", completed=True, text=REPLY),
           sse("run.completed", run_id="run_l", output=REPLY)]
    live = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"}).json()["live_session"]
    with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
        with client.stream("POST", "/v1/live/turns", json={"agent": "wellness", "live_session": live,
                                                             "text": "Des hôtels ?"}) as resp:
            assert resp.status_code == 200
            lines = list(resp.iter_lines())
    assert not any("bip:say" in line or "-->" in line for line in lines), lines
    data = [json.loads(line[6:]) for line in lines if line.startswith("data: ")]
    assert "".join(d["delta"] for d in data if d.get("event") == "assistant.delta").rstrip() == WORDS
    end = next(d for d in data if d.get("event") == "run.completed")
    assert end["output"] == WORDS and end["spoken"] == SPOKEN


def test_the_timeline_and_the_boite_show_the_words_only(bridges, fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Un hôtel ?"},
                                          {"role": "assistant", "content": REPLY}],
                             title="Bot Chat", last_active=10, started_at=1)
    bridge = with_device(bridges).start()
    main = bridge.get("/v1/threads/main", params={"agent": "wellness"}, headers=AUTH).json()
    items = bridge.get(f"/v1/threads/{main['id']}/timeline", headers=AUTH).json()["items"]
    reply = next(i for i in items if i["kind"] == "message" and i.get("role") == "agent")
    assert reply["text"] == WORDS and reply["spoken"] == SPOKEN
    # a routine's report in the Boîte: its words only (its reading made from the spoken version, outbox.rs)
    item = boite_message(bridge, fakes, f"Le point du jour.\n{SAY}")
    assert item["text"] == "Le point du jour." and "bip:" not in json.dumps(item)


def test_a_replys_notification_shows_its_words(bridges, fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Un hôtel ?"},
                                          {"role": "assistant", "content": REPLY}],
                             title="Bot Chat", last_active=10, started_at=1)
    bridge = with_device(bridges).start()
    fakes.hermes.runs["r1"] = [sse("run.completed", output=REPLY)]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "r1", "session_id": "api_main"})
    assert wait_until(lambda: fakes.apns.payloads())
    row = bridge.get(f"/v1/notifications/{fakes.apns.payloads()[0]['n']}", headers=AUTH).json()
    assert row["preview"].startswith("Quatre hôtels") and "bip:" not in json.dumps(row) and "terrasse" not in json.dumps(row)
    assert "bip:" not in json.dumps(fakes.apns.payloads())


def test_the_search_never_finds_its_words(bridges, fakes):
    bridge = bridges.new()
    home = managed_install(bridge, fakes, agents=("coach",))
    store(profile(home, "coach"), [("s1", None, None, "Hôtels", 0)], [
        ("s1", "user", "Un hôtel ?", 1_000_000.0, {}),
        ("s1", "assistant", REPLY, 1_000_100.0, {}),
        ("s1", "assistant", "Ok <!--bip:say jamais fermé terrasse", 1_000_200.0, {}),
    ])
    bridge.start()
    assert bridge.get("/v1/search/messages", params={"q": "terrasse"}, headers=AUTH).json()["results"] == []
    found = bridge.get("/v1/search/messages", params={"q": "Mercure"}, headers=AUTH).json()["results"]
    assert len(found) == 1 and "bip:" not in json.dumps(found)


def test_the_radio_rules_go_with_a_call_and_a_voice_message(bridge, fakes):
    _, rules = split_brief_now(bridge.get("/v1/live/brief", params={"agent": "wellness"}, headers=AUTH).json()["text"])
    assert "Spoken, for the ear: the essential first" in rules and "after_call" in rules   # after the Carnet line
    assert "« about 420 euros »" in rules   # its examples in the agent's language (English here)
    voice = taken_notes(bridge, kinds=("voice",))
    assert "<!--bip:voice\nYour person sent a voice message: your reply will be listened to." in voice
    # never the mark's name in a note (10/11: the Spark then echoed the whole note)
    assert "hidden spoken version" in voice and "bip:say" not in voice and voice.count("-->") == voice.count("<!--")
    assert "bip:voice" not in taken_notes(bridge, text="Et sinon ?")
