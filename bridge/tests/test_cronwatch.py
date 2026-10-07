import asyncio
from datetime import datetime

from bipbridge.config import parse_config
from bipbridge.cronwatch import CronWatcher, final_reply, is_cron_session, session_started


class FakeHermes:
    def __init__(self):
        self.sessions = []
        self.messages = {}
        self.message_calls = 0

    async def list_sessions(self, agent, limit=20):
        return self.sessions

    async def session_messages(self, agent, session_id):
        self.message_calls += 1
        return self.messages.get(session_id, [])


class FakeOutbox:
    def __init__(self):
        self.ingested = []
        self.seen = set()

    async def ingest(self, agent, message, notify=True, replace=False):
        if message["id"] in self.seen:
            return None
        self.seen.add(message["id"])
        self.ingested.append((agent.name, message))
        self.notify = notify
        return {"id": "x"}


def watcher(config_dict):
    config = parse_config(config_dict)
    hermes, outbox = FakeHermes(), FakeOutbox()
    return CronWatcher(config, hermes, outbox), hermes, outbox, config.agent("wellness")


NOW = datetime(2026, 10, 5, 8, 30)


def test_cron_session_helpers():
    assert is_cron_session({"id": "cron_fc344ed09026_20261005_075008"})
    assert not is_cron_session({"id": "api_1791112967_496e4649"})
    assert session_started({"id": "cron_abc_20261005_075008"}) == datetime(2026, 10, 5, 7, 50, 8)
    assert final_reply([{"role": "user", "content": "prompt"},
                        {"role": "assistant", "content": [{"type": "text", "text": "Voilà le point."}]}]) == "Voilà le point."
    assert final_reply([{"role": "assistant", "content": "…"}, {"role": "user", "content": "encore"}]) is None


def test_finished_cron_reply_goes_to_the_outbox_once(config_dict):
    cron, hermes, outbox, agent = watcher(config_dict)
    sid = "cron_fc344ed09026_20261005_075008"
    hermes.sessions = [{"id": sid, "title": "Podcast du matin"}, {"id": "api_123", "title": "Chat"}]
    hermes.messages[sid] = [{"role": "user", "content": "Produis le point"}, {"role": "assistant", "content": "Bonjour Alex !"}]
    assert asyncio.run(cron.poll(agent, now=NOW)) == 0          # first sight: wait until it stops changing
    assert asyncio.run(cron.poll(agent, now=NOW)) == 1          # unchanged: stored
    name, message = outbox.ingested[0]
    assert message == {"message": "Bonjour Alex !", "title": "Podcast du matin", "id": f"hermes-session:{sid}",
                       "tags": [f"session:{sid}"]}
    calls = hermes.message_calls
    assert asyncio.run(cron.poll(agent, now=NOW)) == 0 and hermes.message_calls == calls  # done: not fetched again


def test_silent_old_and_running_sessions_are_skipped(config_dict):
    cron, hermes, outbox, agent = watcher(config_dict)
    hermes.sessions = [{"id": "cron_a_20261005_060000"}, {"id": "cron_b_20261001_060000"}, {"id": "cron_c_20261005_082900"}]
    hermes.messages = {
        "cron_a_20261005_060000": [{"role": "assistant", "content": "[SILENT]"}],
        "cron_b_20261001_060000": [{"role": "assistant", "content": "Vieux message"}],
        "cron_c_20261005_082900": [{"role": "user", "content": "Produis le point"}],
    }
    for _ in range(3):
        asyncio.run(cron.poll(agent, now=NOW))
    assert outbox.ingested == []


def test_job_helpers():
    from bipbridge.cronwatch import job_id, job_name
    assert job_id("cron_fc344ed09026_20261005_075008") == "fc344ed09026"
    assert job_id("api_123") is None
    assert job_name("Podcast du matin · Oct 05 07:53") == "Podcast du matin"


class FakePush:
    def __init__(self):
        self.replies = []

    async def notify_reply(self, agent, run_id, text, session_id=None):
        self.replies.append((agent, text, session_id))

    async def notify_subtask(self, agent, session_id, line=None):
        self.replies.append((agent, ("subtask", line), session_id))


class FakeHub:
    def __init__(self):
        self.followed = set()

    def followed_recently(self, agent, session_id, reply=None, within=600.0):
        return (agent, session_id) in self.followed


def bot_watcher(config_dict):
    from bipbridge.cronwatch import Presence
    config = parse_config(config_dict)
    hermes, push, hub, presence = FakeHermes(), FakePush(), FakeHub(), Presence()
    cron = CronWatcher(config, hermes, FakeOutbox(), hub=hub, push=push, presence=presence)
    return cron, hermes, push, hub, presence, config.agent("wellness")


def test_bot_chat_turn_taken_alone_is_pushed_once(config_dict):
    cron, hermes, push, hub, presence, agent = bot_watcher(config_dict)
    hermes.sessions = [{"id": "api_bot", "title": "Bot Chat", "message_count": 4}]
    hermes.messages["api_bot"] = [{"role": "user", "content": "Bonjour"}, {"role": "assistant", "content": "Ancienne réponse"}]
    asyncio.run(cron.poll(agent, now=NOW))                      # baseline: nothing old is pushed
    assert push.replies == []
    hermes.sessions[0]["message_count"] = 6
    hermes.messages["api_bot"] += [{"role": "user", "content": "[Routine] Point de midi"},
                                   {"role": "assistant", "content": "Wellness dit que ta nuit était bonne."}]
    asyncio.run(cron.poll(agent, now=NOW))                      # first sight: wait until it stops changing
    asyncio.run(cron.poll(agent, now=NOW))
    assert push.replies == [("wellness", "Wellness dit que ta nuit était bonne.", "api_bot")]
    asyncio.run(cron.poll(agent, now=NOW))
    assert len(push.replies) == 1


def test_bot_chat_reply_followed_or_on_screen_is_not_pushed(config_dict):
    cron, hermes, push, hub, presence, agent = bot_watcher(config_dict)
    hermes.sessions = [{"id": "api_bot", "title": "Bot Chat", "message_count": 2}]
    hermes.messages["api_bot"] = [{"role": "user", "content": "Salut"}, {"role": "assistant", "content": "Salut !"}]
    asyncio.run(cron.poll(agent, now=NOW))
    hermes.sessions[0]["message_count"] = 4
    hermes.messages["api_bot"] += [{"role": "user", "content": "Et demain ?"}, {"role": "assistant", "content": "Grand soleil."}]
    hub.followed.add(("wellness", "api_bot"))                   # the app sent it and followed the run
    for _ in range(2):
        asyncio.run(cron.poll(agent, now=NOW))
    hub.followed.clear()
    hermes.sessions[0]["message_count"] = 6
    hermes.messages["api_bot"] += [{"role": "user", "content": "Message from 🤖 Vie (@vie): ?"}, {"role": "assistant", "content": "Vu."}]
    presence.touch("wellness", "api_bot")                       # the conversation is open on the phone
    for _ in range(2):
        asyncio.run(cron.poll(agent, now=NOW))
    assert push.replies == []


def test_session_state_route_records_presence(client):
    from conftest import AUTH
    resp = client.get("/v1/sessions/api_bot/state?agent=wellness", headers=AUTH)
    assert resp.status_code == 200 and resp.json()["message_count"] == 7
    assert client.app.state.services.presence.viewing("wellness", "api_bot")
    assert client.get("/v1/sessions/bad%20id/state?agent=wellness", headers=AUTH).status_code == 400


def test_answer_to_another_agent_is_not_pushed(config_dict):
    cron, hermes, push, hub, presence, agent = bot_watcher(config_dict)
    hermes.sessions = [{"id": "api_bot", "title": "Bot Chat", "message_count": 2}]
    hermes.messages["api_bot"] = [{"role": "user", "content": "Salut"}, {"role": "assistant", "content": "Salut !"}]
    asyncio.run(cron.poll(agent, now=NOW))
    hermes.sessions[0]["message_count"] = 4
    hermes.messages["api_bot"] += [{"role": "user", "content": "Message from 🤖 Vie (@vie): Comment était la nuit d'Alex ?"},
                                   {"role": "assistant", "content": "Nuit correcte, 7 h 10, score 82."}]
    for _ in range(2):
        asyncio.run(cron.poll(agent, now=NOW))
    assert push.replies == []  # Vie reports back to the user: one notification, from Vie


def test_teammate_answer_to_our_own_request_is_pushed(config_dict):
    cron, hermes, push, hub, presence, agent = bot_watcher(config_dict)
    hermes.sessions = [{"id": "api_vie", "title": "Bot Chat", "message_count": 2}]
    hermes.messages["api_vie"] = [{"role": "user", "content": "Salut"}, {"role": "assistant", "content": "Salut !"}]
    asyncio.run(cron.poll(agent, now=NOW))
    hermes.sessions[0]["message_count"] = 6
    hermes.messages["api_vie"] += [
        {"role": "user", "content": "Demande à Wellness comment était ma nuit"},
        {"role": "assistant", "content": "Je demande à Wellness.",
         "tool_calls": [{"function": {"name": "message_agent", "arguments": "{\"target\": \"wellness\", \"message\": \"Nuit d'Alex ?\"}"}}]},
        {"role": "user", "content": "Message from 🤖 Wellness (@wellness): Nuit correcte, 7 h 10."},
        {"role": "assistant", "content": "Wellness dit : nuit correcte, 7 h 10."},
    ]
    for _ in range(2):
        asyncio.run(cron.poll(agent, now=NOW))
    assert push.replies == [("wellness", "Wellness dit : nuit correcte, 7 h 10.", "api_vie")]


def test_a_comment_before_a_running_tool_is_not_the_final_reply(config_dict):
    calling = {"role": "assistant", "content": "execute_code est bloqué en cron — je passe par terminal.",
               "tool_calls": [{"function": {"name": "terminal", "arguments": "{}"}}]}
    assert final_reply([{"role": "user", "content": "Produis le podcast"}, calling]) is None
    assert final_reply([{"role": "user", "content": "Produis le podcast"}, calling,
                        {"role": "tool", "content": "ok"}]) is None
    assert final_reply([{"role": "user", "content": "Produis le podcast"}, calling, {"role": "tool", "content": "ok"},
                        {"role": "assistant", "content": "MEDIA:/m/podcast.mp3\n🎙️ Point du matin"}]) == \
        "MEDIA:/m/podcast.mp3\n🎙️ Point du matin"
    cron, hermes, outbox, agent = watcher(config_dict)
    sid = "cron_fc344ed09026_20261006_075008"
    hermes.sessions = [{"id": sid, "title": "Podcast du matin"}]
    hermes.messages[sid] = [{"role": "user", "content": "Produis le podcast"}, calling]
    for _ in range(3):  # the synthesis takes minutes: the comment stays unchanged, it is not taken
        asyncio.run(cron.poll(agent, now=datetime(2026, 10, 6, 8, 0)))
    assert outbox.ingested == []


def test_a_later_delivery_after_a_followed_reply_is_pushed():
    import time
    from bipbridge.config import AgentConfig
    from bipbridge.runs import RunHub, RunSubscription

    agent = AgentConfig(name="vie", display_name="Vie", hermes_url="http://h", hermes_key="k" * 40)
    hub = RunHub(hermes=None, push=None)
    sub = RunSubscription(agent, "run_1")
    sub.session_id, sub.finished, sub.finished_at = "api_bot", True, time.time() - 120
    sub.output = "C'est lancé : un sous-agent prépare le podcast, je te le livre dès qu'il a fini."
    hub._subs[sub.key] = sub
    # The reply the app followed live: no push.
    assert hub.followed_recently("vie", "api_bot", "C'est lancé : un sous-agent prépare le podcast, je te le livre dès qu'il a fini.")
    # Two minutes later Vie delivers the sub-task's result on its own: a new reply, pushed.
    assert not hub.followed_recently("vie", "api_bot", "Voilà ton podcast ! MEDIA:/home/hermes/.hermes/media/podcast.mp3")
    # Another conversation is never concerned.
    assert not hub.followed_recently("vie", "api_other", "C'est lancé")
    # A run still going: its replies are on screen.
    sub.finished = False
    assert hub.followed_recently("vie", "api_bot", "anything")


REPORT = ("[ASYNC DELEGATION BATCH COMPLETE — deleg_1194136c]\n\nTask 1 (completed): mini podcast generated, "
          "saved to /home/hermes/.hermes/media/podcast_jeudi0810.mp3 (58 s).")


def test_report_helpers():
    from bipbridge.cronwatch import report_line, subtask_report
    assert subtask_report([{"role": "assistant", "content": "Relancé"}, {"role": "user", "content": REPORT}]) == REPORT
    assert subtask_report([{"role": "user", "content": REPORT}, {"role": "assistant", "content": "Voilà"}]) is None
    assert subtask_report([{"role": "user", "content": "Salut"}]) is None
    assert report_line(REPORT) == "🎧 Podcast jeudi0810"
    assert report_line("[ASYNC DELEGATION BATCH COMPLETE — x] see /srv/media/chart-sleep.png") == "🖼️ Chart sleep"
    assert report_line("[ASYNC DELEGATION BATCH COMPLETE — x] done, nothing saved") is None


def test_a_report_left_without_a_reply_is_pushed_once(config_dict):
    cron, hermes, push, hub, presence, agent = bot_watcher(config_dict)
    hermes.sessions = [{"id": "api_bot", "title": "Bot Chat", "message_count": 2}]
    hermes.messages["api_bot"] = [{"role": "user", "content": "Un podcast ?"}, {"role": "assistant", "content": "Relancé : le sous-agent régénère le podcast."}]
    asyncio.run(cron.poll(agent, now=NOW))
    hub.followed.add(("wellness", "api_bot"))                   # the app followed « Relancé… »
    hermes.sessions[0]["message_count"] = 3
    hermes.messages["api_bot"].append({"role": "user", "content": REPORT})  # Hermes: stored, no wake turn
    asyncio.run(cron.poll(agent, now=NOW))                      # first sight: maybe the agent wakes up
    assert push.replies == []
    asyncio.run(cron.poll(agent, now=NOW))                      # still last: nobody will reply
    assert push.replies == [("wellness", ("subtask", "🎧 Podcast jeudi0810"), "api_bot")]
    for _ in range(2):
        asyncio.run(cron.poll(agent, now=NOW))
    assert len(push.replies) == 1


def test_a_report_the_agent_answers_is_not_pushed_twice(config_dict):
    cron, hermes, push, hub, presence, agent = bot_watcher(config_dict)
    hermes.sessions = [{"id": "api_bot", "title": "Bot Chat", "message_count": 2}]
    hermes.messages["api_bot"] = [{"role": "user", "content": "Un podcast ?"}, {"role": "assistant", "content": "C'est lancé."}]
    asyncio.run(cron.poll(agent, now=NOW))
    hermes.sessions[0]["message_count"] = 3
    hermes.messages["api_bot"].append({"role": "user", "content": REPORT})
    asyncio.run(cron.poll(agent, now=NOW))
    hermes.sessions[0]["message_count"] = 4                     # this Hermes wakes the agent: it delivers
    hermes.messages["api_bot"].append({"role": "assistant", "content": "MEDIA:/m/podcast.mp3\nVoilà ton podcast !"})
    for _ in range(3):
        asyncio.run(cron.poll(agent, now=NOW))
    assert push.replies == [("wellness", "MEDIA:/m/podcast.mp3\nVoilà ton podcast !", "api_bot")]


def test_a_report_in_another_conversation_is_pushed_unless_on_screen(config_dict):
    cron, hermes, push, hub, presence, agent = bot_watcher(config_dict)
    hermes.sessions = [{"id": "api_x", "title": "Recherche", "message_count": 2},
                       {"id": "api_y", "title": "Autre", "message_count": 2}]
    hermes.messages = {"api_x": [{"role": "user", "content": "Cherche"}, {"role": "assistant", "content": "Lancé."}],
                       "api_y": [{"role": "user", "content": "Cherche"}, {"role": "assistant", "content": "Lancé."}]}
    asyncio.run(cron.poll(agent, now=NOW))                      # baseline, nothing fetched
    assert hermes.message_calls == 0
    for sid in ("api_x", "api_y"):
        hermes.sessions[[s["id"] for s in hermes.sessions].index(sid)]["message_count"] = 3
        hermes.messages[sid].append({"role": "user", "content": "[ASYNC DELEGATION BATCH COMPLETE — d] 3 sources found."})
    presence.touch("wellness", "api_y")
    for _ in range(3):
        asyncio.run(cron.poll(agent, now=NOW))
    assert push.replies == [("wellness", ("subtask", None), "api_x")]
