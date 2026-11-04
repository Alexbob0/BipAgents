"""The files a reply names reach the apps as attachments only (4/11): the bridge takes the model's `MEDIA:` lines out
of everything it passes on (a run's deltas, even a line cut across them, and its end's `output`; a Live turn's
stream; the timeline, the Boîte) and gives the files as structured items: a `message.file` event where the line was
while the reply streams, the end's `attachments`, the timeline's (media.rs `MediaFilter`)."""
import json

import httpx

from bbkit.common import AUTH, boite_message, read_sse, sse

PAGE = "/bipagents/pages/wellness/plan.html"
REPLY = f"Voici le plan :\nMEDIA:{PAGE}\n\nLa suite.\nMEDIA:/envoi/devis.pdf"


def test_a_runs_events_carry_no_media_line_and_its_end_the_files(bridge, fakes):
    fakes.hermes.runs["run_m"] = [sse("message.delta", run_id="run_m", delta="Voici le plan :\nMED"),
                                  sse("message.delta", run_id="run_m", delta=f"IA:{PAGE}\n\nLa "),
                                  sse("message.delta", run_id="run_m", delta="suite.\nMEDIA:/envoi/devis.pdf"),
                                  sse("run.completed", run_id="run_m", output=REPLY)]
    with bridge.stream("GET", "/v1/runs/run_m/events?agent=wellness", headers=AUTH) as resp:
        assert resp.status_code == 200
        events = read_sse(resp)
    assert all("MEDIA:" not in json.dumps(data) for _, data in events), events
    said = "".join(d.get("delta", "") for name, d in events if name == "message.delta")
    assert said == "Voici le plan :\n\nLa suite.\n"
    # each line a file event at once, in its place, before the run's end (the page shows while the reply is written)
    names = [name for name, _ in events]
    files = [(d["path"], d["kind"], d["offset"]) for name, d in events if name == "message.file"]
    assert files == [(PAGE, "page", 16), ("/envoi/devis.pdf", "file", 27)]
    assert names.index("message.file") < names.index("run.completed")
    assert names[names.index("message.file") - 1] == "message.delta", "after the words before it"
    name, end = events[-1]
    assert name == "run.completed" and end["output"] == "Voici le plan :\n\nLa suite."
    files = [(a["path"], a["kind"], a["offset"]) for a in end["attachments"]]
    assert files == [(PAGE, "page", 15), ("/envoi/devis.pdf", "file", 26)]
    # a follower that comes late gets the same words (the replayed backlog)
    with bridge.stream("GET", "/v1/runs/run_m/events?agent=wellness", headers=AUTH) as resp:
        again = read_sse(resp)
    assert "".join(d.get("delta", "") for name, d in again if name == "message.delta") == said


def test_a_live_turn_streams_without_media_lines(bridge, fakes):
    hermes = fakes.hermes
    hermes.stream_script = [sse("assistant.delta", run_id="run_l1", delta="Je te montre.\nMEDIA:/bipa"),
                            ": keepalive\n\n",
                            sse("assistant.delta", run_id="run_l1", delta=f"gents/pages/wellness/carte.html\nVoilà."),
                            sse("assistant.completed", run_id="run_l1", completed=True,
                                text="Je te montre.\nMEDIA:/bipagents/pages/wellness/carte.html\nVoilà."),
                            sse("run.completed", run_id="run_l1",
                                output="Je te montre.\nMEDIA:/bipagents/pages/wellness/carte.html\nVoilà.")]
    live = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"}).json()["live_session"]
    with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
        with client.stream("POST", "/v1/live/turns", json={"agent": "wellness", "live_session": live,
                                                             "text": "Montre-moi"}) as resp:
            assert resp.status_code == 200
            lines = list(resp.iter_lines())
    assert ": keepalive" in lines, "the keepalives go on"
    assert not any("MEDIA:" in line for line in lines), lines
    data = [json.loads(line[6:]) for line in lines if line.startswith("data: ")]
    said = "".join(d["delta"] for d in data if d.get("event") == "assistant.delta")
    assert said == "Je te montre.\nVoilà."
    events = [d.get("event") for d in data]
    file = next(d for d in data if d.get("event") == "message.file")
    assert file["path"] == "/bipagents/pages/wellness/carte.html" and file["offset"] == 14
    assert events.index("message.file") < events.index("run.completed")
    end = next(d for d in data if d.get("event") == "run.completed")
    assert end["output"] == "Je te montre.\nVoilà."
    assert [a["path"] for a in end["attachments"]] == ["/bipagents/pages/wellness/carte.html"]


def test_the_timeline_and_the_boite_give_files_never_lines(bridges, fakes):
    fakes.hermes.v0225 = True
    fakes.hermes.add_session("api_main", [{"role": "user", "content": "Un plan ?"},
                                          {"role": "assistant", "content": REPLY}],
                             title="Bot Chat", last_active=10, started_at=1)
    bridge = bridges.new().start()
    main = bridge.get("/v1/threads/main", params={"agent": "wellness"}, headers=AUTH).json()
    items = bridge.get(f"/v1/threads/{main['id']}/timeline", headers=AUTH).json()["items"]
    reply = next(i for i in items if i["kind"] == "message" and i.get("role") == "agent")
    assert reply["text"] == "Voici le plan :\n\nLa suite."
    assert [(a["path"], a["offset"]) for a in reply["attachments"]] == [(PAGE, 15), ("/envoi/devis.pdf", 26)]
    # a routine's report in the Boîte, as the apps read it (the journal's boite row, its message whole)
    item = boite_message(bridge, fakes, f"Le point du jour.\nMEDIA:{PAGE}")
    assert item["text"] == "Le point du jour." and [a["path"] for a in item["attachments"]] == [PAGE]
    assert "MEDIA:" not in json.dumps(items) + json.dumps(item)
