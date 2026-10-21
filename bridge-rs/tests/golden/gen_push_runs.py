"""Golden fixtures for the push-runs group's pure functions: push payloads and previews, FCM data messages, ntfy's
cron unwrapping and auth header, MEDIA lines, the cron watcher's helpers and the run hub's backlog merging. Run from
bridge/ with the venv:

    cd bridge && ../path/to/venv/bin/python ../bridge-rs/tests/golden/gen_push_runs.py

Writes tests/golden/push_runs.json: [{"fn": name, "input": [args], "output": result}]."""
import json
import os
import sys
from collections import deque

sys.path.insert(0, os.getcwd())
from bipbridge import cronwatch, fcm, media, ntfy, push, runs  # noqa: E402
from bipbridge.hermes import SSEEvent  # noqa: E402

CASES = []


def case(name, fn, *args):
    out = fn(*args)
    CASES.append({"fn": name, "input": list(args), "output": out})


# -- push ------------------------------------------------------------------------------------------
for args in [("vie", "Vie", "o1", None, None), ("vie", "Vie", "o1", "s1", "Prévu à 9 h"), ("vie", "Vie", "o1", "", ""),
             ("wellness", "Wellness", "abc", "sess_42", None)]:
    case("message_payload", push.message_payload, *args)
for args in [("vie", "Vie", "run_1", "req_9", ["once", "deny"], None, None),
             ("vie", "Vie", "run_1", None, None, None, None),
             ("vie", "Vie", "run_1", "", [], "", ""),
             ("vie", "Vie", "run_1", "req", ["once", "session", "deny"], "Approbation requise : rm -rf build/", "api_9"),
             ("vie", "Vie", "run_1", "req", [1, {"x": 2}], None, "s")]:
    case("approval_payload", push.approval_payload, *args)
for args in [("vie", "Vie", "run_2", None, None, None, push.REPLY_BODY),
             ("vie", "Vie", "run_2", "api_1", "Le train de 9h est à 19 €.", "abc", push.REPLY_BODY),
             ("vie", "Vie", "session:api_1", "api_1", None, None, push.SUBTASK_BODY),
             ("vie", "Vie", "session:scan", None, "", None, push.SUBTASK_BODY),
             ("vie", "Vie", "session:job", "", "Ligne", "", "Tâche terminée")]:
    case("reply_payload", push.reply_payload, *args)
for args in [("vie", "Vie", "run_q", "clr_1", "api_7", None, "rid"), ("vie", "Vie", "run_q", None, None, "Quel train ?", None),
             ("vie", "Vie", "run_q", "", "", "", "")]:
    case("question_payload", push.question_payload, *args)
CASES.append({"fn": "silent_payload", "input": ["vie", "run_finished", {"run_id": "r", "status": "run.completed", "gone": None}],
              "output": push.silent_payload("vie", "run_finished", run_id="r", status="run.completed", gone=None)})
CASES.append({"fn": "silent_payload", "input": ["vie", "test", {}], "output": push.silent_payload("vie", "test")})
for text, limit in [("Bonjour", 160), ("  a \n\t b  ", 160), ("x" * 200, 160), ("abcd efgh", 6), ("é" * 30, 10),
                    ("mot\u001cmot mot​mot", 160), ("a" * 159 + " " + "b", 160), ("", 160),
                    ("Une  phrase   assez longue pour être coupée quelque part", 20)]:
    case("preview", push._preview, text, limit)

# -- fcm ------------------------------------------------------------------------------------------
for payload in [push.approval_payload("wellness", "Wellness", "run_1", "req_1", ["once", "deny"], session_id="s1"),
                push.reply_payload("vie", "Vie", "run_2", reply_id="abc"),
                push.message_payload("vie", "Vie", "o1", "s1", "Aperçu é"),
                push.silent_payload("vie", "run_finished", run_id="r", status="run.completed"),
                {"aps": {"content-available": 1}, "agent": "a", "n": 5, "f": 1.5, "b": True, "l": [1, "é"],
                 "d": {"k": "v"}, "none": None},
                {"aps": {"alert": "string alert", "category": "X", "content-available": 1}},
                {"aps": {"alert": {"title": 5, "body": ""}, "category": ""}, "title": "override"},
                {"aps": {}}, {}, {"aps": None, "x": "y"}]:
    case("data_message", fcm.data_message, payload)

# -- ntfy ------------------------------------------------------------------------------------------
for text in ["Cronjob Response: test-push-bipagents (job_id: 0681ac73805e)\n-------------\n\nBonjour Alex, ceci est un test."
             "\n\nNote: The agent cannot see this message, and therefore cannot respond to it.",
             "Cronjob Response: Morning feeds\n-------------\n\nLe point du matin.",
             "  Un message normal.  ",
             "Cronjob Response: Test push propre (job_id: 1a2b3c4d5e6f)\n-------------\n\nCoucou, test réussi.\n\n"
             'To stop or manage this job, send me a new message (e.g. "stop reminder Test push propre").'
             "\n\nNote: The agent cannot see this message, and therefore cannot respond to it.",
             "Cronjob Response:  (job_id: abc)\n---\nCorps",
             "Cronjob Response: X\n--\nPas un en-tête",
             "Cronjob Response: Job\n-----\n\nLigne 1\nNote: The agent cannot see this message here\nLigne 3",
             "Cronjob Response: Job\n-----\n",
             ""]:
    case("unwrap_cron", ntfy.unwrap_cron, text)
for token in [None, "", "tk_abc", "tk_a:b", "user:pass", "plain-token", "üser:pässe"]:
    case("auth_header", ntfy.auth_header, token)

# -- media ------------------------------------------------------------------------------------------
for text in ["MEDIA:/home/hermes/simplex-files/point.mp3\n\n🎙️ Point du matin — lundi 5 octobre",
             "Voici :\n  MEDIA: /a/b.png  \n\n\n\nFin\nMEDIA:/c/d.mp3",
             "pas de média ici", "texte MEDIA:/x.mp3 au milieu", "MEDIA:/x.mp3\r\nSuite", "MEDIA:/only.mp3",
             "\n\nMEDIA:/a.mp3\n\n\n\nMEDIA:/b.mp3\n\n"]:
    case("extract_media", media.extract_media, text)

# -- cronwatch ------------------------------------------------------------------------------------------
SESSIONS = [{"id": "cron_fc344ed09026_20261005_075008"}, {"id": "api_1791112967_496e4649"}, {"session_id": "cron_x_1"},
            {"id": "x", "source": "Cron"}, {"id": "", "session_id": "cron_a_20261005_060000"}, {},
            {"id": "cron_a_b_20261005_060000"}, {"id": "cron_a_20261399_060000", "started_at": "2026-10-05T07:00:00"},
            {"id": "api_1", "started_at": "2026-10-05T07:50:08"}, {"id": "api_1", "created_at": "pas une date", "created": "2026-10-04"},
            {"id": "cron_x_20261005_075008\n"}]
for session in SESSIONS:
    case("is_cron_session", cronwatch.is_cron_session, session)
    started = cronwatch.session_started(session)
    CASES.append({"fn": "session_started", "input": [session], "output": started.isoformat() if started else None})
for sid in ["cron_fc344ed09026_20261005_075008", "api_123", "cron_a_b_c_20261005_075008", "cron__20261005_075008"]:
    case("job_id", cronwatch.job_id, sid)
for title in [None, "", "Podcast du matin · Oct 05 07:53", " · x", "Simple", "  Espaces  · y"]:
    case("job_name", cronwatch.job_name, title)
MESSAGES = [
    [{"role": "user", "content": "prompt"}, {"role": "assistant", "content": [{"type": "text", "text": "Voilà le point."}]}],
    [{"role": "assistant", "content": "…"}, {"role": "user", "content": "encore"}],
    [{"role": "user", "content": "p"}, {"role": "assistant", "content": "  Fin  "}, {"role": "assistant", "content": ""}],
    [{"role": "user", "content": "p"}, {"role": "assistant", "content": "c", "tool_calls": [{"x": 1}]}],
    [{"role": "user", "content": "p"}, {"role": "assistant", "content": "c", "tool_calls": []}],
    [{"role": "Assistant", "text": "Par text"}],
    [{"role": "assistant", "content": None, "text": "ignored"}],
    [{"role": "assistant", "content": [{"type": "output_text", "text": "A"}, {"type": "image", "text": "B"}, "x",
                                       {"type": "text", "text": "C"}]}],
    [{"role": "tool", "content": "ok"}],
    [],
    [{"role": "assistant", "content": "Relancé"}, {"role": "user", "content": "[ASYNC DELEGATION BATCH COMPLETE — d] ok"}],
    [{"role": "user", "content": "  [SUBAGENT done]"}],
    [{"role": "user", "content": "[DELEGATION x]"}, {"role": "assistant", "content": "Voilà"}],
    [{"role": "user", "content": "Message from 🤖 Vie (@vie): Comment était la nuit ?"}, {"role": "assistant", "content": "Correcte."}],
    [{"role": "user", "content": "Demande à Wellness"},
     {"role": "assistant", "content": "Je demande.", "tool_calls": [{"function": {"name": "message_agent", "arguments": "{\"target\": \"wellness\"}"}}]},
     {"role": "user", "content": "Message from 🤖 Wellness (@wellness): Nuit correcte."},
     {"role": "assistant", "content": "Wellness dit : nuit correcte."}],
    [{"role": "user", "content": "Message from Bob (@bob.b-1): hello"}, {"role": "assistant", "content": "hi"}],
    [{"role": "user", "content": "Message from (@x): sans nom"}],
]
for messages in MESSAGES:
    case("final_reply", cronwatch.final_reply, messages)
    case("subtask_report", cronwatch.subtask_report, messages)
    case("answers_another_agent", cronwatch.answers_another_agent, messages)
    for message in messages:
        case("message_text", cronwatch.message_text, message)
for report in ["[ASYNC DELEGATION BATCH COMPLETE — deleg_1194136c]\n\nTask 1 (completed): mini podcast generated, saved to "
               "/home/hermes/.hermes/media/podcast_jeudi0810.mp3 (58 s).",
               "[ASYNC DELEGATION BATCH COMPLETE — x] see /srv/media/chart-sleep.png",
               "[ASYNC DELEGATION BATCH COMPLETE — x] done, nothing saved",
               "see /a/media/b/clip__final--v2.MOV and /a/media/c.mp3", "/media/x.mp3", "/x/media/noext.mp3x",
               "/x/media/émission_spéciale.ogg", "/x/media/.mp3"]:
    case("report_line", cronwatch.report_line, report)
for session in [{"title": "Bot Chat"}, {"title": " bot chat "}, {"title": "Chat"}, {"title": None}, {}, {"title": 5}]:
    case("is_bot_chat", cronwatch.is_bot_chat, session)

# -- runs ------------------------------------------------------------------------------------------


def event(kind, data, raw=None):
    return SSEEvent(kind, data, raw if raw is not None else json.dumps({"event": kind, **data}))


SEQUENCES = [
    [event("message.delta", {"delta": "Bon"}), event("message.delta", {"delta": "jour é"}),
     event("tool.started", {"tool": "x"}), event("message.delta", {"delta": "a"}), event("assistant.delta", {"delta": "b"}),
     event("assistant.delta", {"delta": "c", "extra": 1})],
    [event("message.delta", {"delta": 5}), event("message.delta", {"delta": "x"}), event("message.delta", {"delta": "y"})],
    [event("response.output_text.delta", {"delta": "a", "run_id": "r"}, ""), event("response.output_text.delta", {"delta": "b"})],
    [event("run.completed", {"output": "x"}), event("run.completed", {"output": "y"})],
]
for sequence in SEQUENCES:
    backlog = deque(maxlen=runs.BACKLOG_EVENTS)
    for e in sequence:
        runs._append_to_backlog(backlog, e)
    CASES.append({"fn": "append_to_backlog", "input": [[[e.type, e.data, e.raw] for e in sequence]],
                  "output": [[e.type, e.data, e.raw] for e in backlog]})
for data in [{"request_id": "r", "clarify_id": "c"}, {"clarify_id": "c"}, {"request_id": 5, "clarify_id": "c"}, {}, {"request_id": ""}]:
    case("clarify_id", runs._clarify_id, data)

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "push_runs.json")
with open(path, "w", encoding="utf-8") as fh:
    json.dump(CASES, fh, ensure_ascii=False, indent=1)
print(f"{len(CASES)} cases -> {path}")
