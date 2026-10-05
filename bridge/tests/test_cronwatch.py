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

    async def ingest(self, agent, message):
        if message["id"] in self.seen:
            return None
        self.seen.add(message["id"])
        self.ingested.append((agent.name, message))
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
