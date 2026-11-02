"""Opening a conversation, the apps' way before and after the bridge-owned timeline (docs/conversations.md §13): the
requests a screen needs, their bytes and their time, against the fake Hermes on this machine (counts and bytes are
exact; times say the order of magnitude). `-s` prints the table; the asserts keep « one request per screen »."""
import subprocess
import time

import httpx

from bbkit.common import AUTH, HERMES_KEY

MESSAGES = 400   # in the main conversation, over 3 segments (two compressions)


def thread(fakes):
    fakes.hermes.v0225 = True
    per = MESSAGES // 3
    rows = [{"role": "user" if i % 2 == 0 else "assistant",
             "content": f"Message {i} : " + "quelques mots de plus pour la taille d'un vrai message. " * 3,
             "timestamp": 1_000_000 + i} for i in range(MESSAGES)]
    fakes.hermes.add_session("s1", rows[:per], title="Bot Chat", last_active=1, started_at=1, end_reason="compression")
    fakes.hermes.add_session("s2", rows[per:2 * per], title="Bot Chat", last_active=2, started_at=2,
                             parent_session_id="s1", end_reason="compression")
    fakes.hermes.add_session("s3", rows[2 * per:], title="Bot Chat", last_active=3, started_at=3, parent_session_id="s2")
    for n in range(30):
        fakes.hermes.add_session(f"api_{n}", rows[:4], title=f"Conversation {n}", last_active=0.5, started_at=0.5)


ROUNDS = 20


def measure(calls):
    """(requests, bytes, seconds) of a screen's requests, sent one after the other (the mean of `ROUNDS`)."""
    started, size = time.perf_counter(), 0
    for _ in range(ROUNDS):
        size = 0
        for client, path, params in calls:
            resp = client.get(path, params=params)
            assert resp.status_code == 200, (path, resp.status_code, resp.text[:200])
            size += len(resp.content)
    return len(calls), size, (time.perf_counter() - started) / ROUNDS


def rss_kb(pid):
    return int(subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip() or 0)


def test_opening_a_conversation_old_and_new(bridges, fakes):
    thread(fakes)
    bridge = bridges.new().start()
    hermes = httpx.Client(base_url=fakes.hermes.url, headers={"Authorization": f"Bearer {HERMES_KEY}"}, timeout=30)
    app = httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30)
    # before: ConversationModel.load (iOS) / ChatModel (Android): find the Bot Chat, its session, its latest
    # messages, the Boîte, the approvals waiting and answered, the Live calls; then session_state every 5–8 s
    old_open = [
        (hermes, "/api/sessions", {"limit": 50}),
        (hermes, "/api/sessions/s3", {}),
        (hermes, "/api/sessions/s3/messages", {"limit": 300, "order": "latest", "inline_images": "false"}),
        (app, "/v1/outbox", {"agent": "wellness"}),
        (app, "/v1/approvals", {"agent": "wellness"}),
        (app, "/v1/approvals/answered", {"agent": "wellness", "session_id": "s3"}),
        (app, "/v1/live/calls", {"agent": "wellness", "session": "s3"}),
    ]
    # an old notification about a message of the first segment: the app had to fetch that segment whole
    old_around = old_open[:2] + [(hermes, "/api/sessions/s1/messages", {"inline_images": "false"})] + old_open[3:]
    new_open = [(app, "/v1/threads/main/timeline", {"agent": "wellness", "limit": 50})]
    # the same number of messages as the old screen's (the tip's 133), for bytes per message
    new_same = [(app, "/v1/threads/main/timeline", {"agent": "wellness", "limit": 133})]
    first = app.get("/v1/threads/main/timeline", params={"agent": "wellness", "limit": 500}).json()
    anchor = first["items"][5]["id"]
    new_around = [(app, "/v1/threads/main/timeline", {"agent": "wellness", "around": anchor, "before_count": 20,
                                                      "after_count": 50})]
    measure(old_open), measure(new_open)   # warm both
    rss_before = rss_kb(bridge.process.pid)
    hermes_before = len(fakes.hermes.requests)
    rows = {name: measure(calls) for name, calls in
            [("open, before", old_open), ("open, after", new_open), ("open, after (133 items)", new_same),
             ("around a message, before", old_around), ("around a message, after", new_around)]}
    hermes_calls_new = len(fakes.hermes.requests) - hermes_before - 6 * ROUNDS   # the old screens' own Hermes calls
    rss_after = rss_kb(bridge.process.pid)
    print("\n| Screen | Requests | Bytes | ms |\n|---|---:|---:|---:|")
    for name, (n, size, seconds) in rows.items():
        print(f"| {name} | {n} | {size} | {seconds * 1000:.1f} |")
    print(f"Hermes calls made by the bridge for the new screens ({3 * ROUNDS} opens): {hermes_calls_new}")
    print(f"Bridge RSS: {rss_before} kB before, {rss_after} kB after {5 * ROUNDS} screens")
    assert hermes_calls_new == 0
    assert rows["open, after"][0] == 1 and rows["around a message, after"][0] == 1
    assert rows["open, after"][1] < rows["open, before"][1]
    assert rows["around a message, after"][1] < rows["around a message, before"][1]
