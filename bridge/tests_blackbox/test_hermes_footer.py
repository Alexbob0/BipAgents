"""Hermes' notes for a developer at a terminal never reach the person (bridge-rs/src/hermes_footer.rs, no_reply.rs;
Alex 11/11, Wellness' morning check-in): its file-mutation verifier, which Hermes adds under a reply when a write of
the turn failed, is out of a run's end (the apps' events), of a run's status (a routine's report: its Boîte message,
its audio, its push, the conversation) and of a Live turn; its explanation of a turn without an answer after a
fragment is said in plain words, and a routine whose turn ended on it says that it couldn't finish."""
import json

import httpx

from bbkit.common import AUTH, boite_items, boite_message, read_sse, sse
from bbkit.servers import wait_until

REPLY = ("9. Notes libres ?\n\nDès que tu réponds, je logge la nuit avant toute autre analyse.\n\n"
         "Séance du jour : Jambes (muscu 40') — voir Garmin Connect pour le workout du jour.")
# what Alex read on 11/11, as Hermes v0.22.5 writes it (agent/turn_explainers.py), its second error cut short
VERIFIER = ("⚠️ File-mutation verifier: 2 file edit(s) FAILED this turn despite any wording above that may suggest "
            "otherwise. Run `git status` or `read_file` to confirm what actually landed.\n"
            "  • `/workspace/checkin_read.py` — [write_file] Failed to write file: Permission denied: /workspace\n"
            "  • `/home/alex/.hermes/profiles/wellness/cache/scratch/checkin_read.py` — [write_file] Refusing to "
            "overwrite `/home/alex/.hermes/profiles/wellness/cache/scratch/checkin_read.py`: "
            "`/home/alex/.hermes/profiles/wellness/cache/scratch/checkin_read.py` exists but this task h…")
FRENCH = ("⚠️ Vérificateur de modifications de fichiers : 12 modification(s) de fichier ont ÉCHOUÉ ce tour, quoi "
          "qu'en dise le texte ci-dessus. Lancez `git status` ou `read_file` pour vérifier ce qui a réellement été "
          "appliqué.\n  • `/workspace/a.py` — [write_file] échec\n  • … et 2 de plus")
NO_REPLY_AFTER = ("\n\n⚠️ No reply: streaming stopped early and only a partial response was recovered. Send `continue` "
                  "to resume from where it stopped.")


def nothing_of_it(text: str) -> bool:
    return not any(w in text for w in ("verifier", "Vérificateur", "/workspace", "git status", "⚠", "No reply"))


def test_a_runs_end_reaches_the_apps_without_the_verifier(bridge, fakes):
    fakes.hermes.runs["run_v"] = [sse("message.delta", run_id="run_v", delta=REPLY),
                                  sse("run.completed", run_id="run_v", output=f"{REPLY}\n\n{VERIFIER}")]
    with bridge.stream("GET", "/v1/runs/run_v/events?agent=wellness", headers=AUTH) as resp:
        events = read_sse(resp)
    [end] = [data for name, data in events if name == "run.completed"]
    assert end["output"] == REPLY
    assert all(nothing_of_it(json.dumps(data, ensure_ascii=False)) for _, data in events), events
    # a follower that comes late reads the same end (the hub's backlog)
    with bridge.stream("GET", "/v1/runs/run_v/events?agent=wellness", headers=AUTH) as resp:
        again = [data for name, data in read_sse(resp) if name == "run.completed"]
    assert again and again[-1]["output"] == REPLY
    # the bridge's log keeps it, for whoever looks into the agent's files
    assert wait_until(lambda: "file-mutation verifier note left out" in bridge.log_tail(400), timeout=10)


def test_hermes_explanation_after_a_fragment_is_said_plainly(bridge, fakes):
    fakes.hermes.runs["run_p"] = [sse("run.completed", run_id="run_p",
                                      output=f"Je regarde ta nuit et\n\n{VERIFIER}{NO_REPLY_AFTER}")]
    with bridge.stream("GET", "/v1/runs/run_p/events?agent=wellness", headers=AUTH) as resp:
        [end] = [data for name, data in read_sse(resp) if name == "run.completed"]
    said = end["output"]
    assert said.startswith("Je regarde ta nuit et\n\n") and "continue" in said, said
    assert nothing_of_it(said) and "/model" not in said and "`" not in said, said


def test_a_routines_report_its_audio_and_the_conversation_never_carry_it(bridges, fakes):
    bridge = bridges.new().start()
    item = boite_message(bridge, fakes, f"{REPLY}\n\n{VERIFIER}", name="Check-in du matin")
    assert item["text"] == REPLY
    french = boite_message(bridge, fakes, f"Bonjour.\n\n{FRENCH}", name="Bonjour")
    assert french["text"] == "Bonjour."
    # its audio (« Écouter ») is read from its words alone
    assert wait_until(lambda: len([b for b in fakes.pocket.inputs if b["response_format"] == "mp3"]) >= 2, timeout=20)
    assert all(nothing_of_it(b["input"]) for b in fakes.pocket.inputs), fakes.pocket.inputs
    # the conversation shows the report the same
    items = bridge.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": "wellness"}).json()["items"]
    shown = [i["text"] for i in items if i["kind"] == "boite"]
    assert REPLY in shown and all(nothing_of_it(t) for t in shown), shown
    assert all(nothing_of_it(r.body.decode()) for r in fakes.apns.pushes())


def test_a_routine_whose_turn_ended_without_an_answer_says_it_couldnt_finish(bridges, fakes):
    bridge = bridges.new().start()
    job = bridge.post("/v1/agents/wellness/routines", headers=AUTH,
                      json={"name": "Courriels", "schedule": "every 2h", "prompt": "Regarde."}).json()["job"]
    fakes.hermes.run_output = ("⚠️ No reply: GLM didn't produce a reply this time, even after retries. Send `continue` "
                               "to try again, or switch models with /model.")
    assert bridge.post(f"/v1/routines/{job['id']}/run", headers=AUTH).status_code == 200
    assert wait_until(lambda: [i for i in boite_items(bridge) if i["title"] == "Courriels"], timeout=20)
    [told] = [i["text"] for i in boite_items(bridge) if i["title"] == "Courriels"]
    assert told.startswith("The routine “Courriels” couldn't finish"), told
    assert nothing_of_it(told) and "/model" not in told
    ran = bridge.get("/v1/agents/wellness/routines", headers=AUTH).json()["jobs"][0]
    assert ran["last_status"].startswith("ended failed"), ran


def test_a_live_turn_never_says_it(bridge, fakes):
    hermes = fakes.hermes
    hermes.stream_script = [sse("assistant.delta", run_id="run_l1", delta="C'est noté."),
                            sse("assistant.completed", run_id="run_l1", completed=True,
                                content=f"C'est noté.\n\n{VERIFIER}", text=f"C'est noté.\n\n{VERIFIER}"),
                            sse("run.completed", run_id="run_l1", output=f"C'est noté.\n\n{VERIFIER}")]
    live = bridge.post("/v1/live/calls/start", headers=AUTH, json={"agent": "wellness"}).json()["live_session"]
    with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
        with client.stream("POST", "/v1/live/turns", json={"agent": "wellness", "live_session": live,
                                                             "text": "Note ma nuit"}) as resp:
            assert resp.status_code == 200
            lines = list(resp.iter_lines())
    data = [json.loads(line[6:]) for line in lines if line.startswith("data: ")]
    done = next(d for d in data if d.get("event") == "assistant.completed")
    assert done["content"] == done["text"] == "C'est noté."
    assert next(d for d in data if d.get("event") == "run.completed")["output"] == "C'est noté."
    assert all(nothing_of_it(line) for line in lines), lines
