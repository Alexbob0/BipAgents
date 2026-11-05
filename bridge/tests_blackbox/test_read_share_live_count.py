"""5/11 (« Pour les queens »): the owner's open conversation follows its « Visible par N personnes » by itself. While
the owner's app shows a shared project thread (its event stream open with `thread=`), the bridge looks at the
guichet's viewers every few seconds; a friend who opened their link is keyed at once and the stream carries a `share`
hint (no content: the app reads the share again). Nothing is asked of the app, no sync called."""
import json
import queue
import threading
import time

import httpx

from bbkit.common import AUTH
from test_project_agents import share_body
from test_read_shares import FakeGuichet, Phone

SHARE = "share0123456789abcdef"


def test_a_friend_opening_their_link_reaches_the_open_conversation_in_seconds(bridges, fakes):
    from test_agents_admin import managed

    bridge, _ = managed(bridges, fakes, isolated=True)
    made = bridge.post("/v1/project-agents", headers=AUTH, json={"name": "Pour les queens",
                                                                 "ends": time.time() + 10 * 86400, "lang": "fr"})
    assert made.status_code == 201, made.text
    agent = made.json()["agent"]
    fakes.hermes.add_session("bot-1", [
        {"id": 1, "role": "user", "content": "Un week-end à Barcelone ?"},
        {"id": 2, "role": "assistant", "content": "Voici le plan."},
    ], title="Bot Chat", agent=agent, started_at=100.0)

    guichet = FakeGuichet().start()
    stop = threading.Event()
    hints: "queue.Queue[tuple[str, dict]]" = queue.Queue()
    try:
        shared = bridge.post("/v1/read-shares", headers=AUTH, json=share_body(guichet, agent))
        assert shared.status_code == 200, shared.text
        assert shared.json()["count"] == 0
        link = bridge.post(f"/v1/read-shares/{SHARE}/invites", headers=AUTH, json={"name": "Léa"}).json()

        def watch():   # the owner's app shows the project's conversation
            with httpx.Client(base_url=bridge.url, headers=AUTH, timeout=30) as client:
                with client.stream("GET", "/v1/threads/events", params={"agent": agent, "thread": "main"}) as resp:
                    kind = None
                    for line in resp.iter_lines():
                        if stop.is_set():
                            return
                        if line.startswith("event: "):
                            kind = line[7:]
                        elif line.startswith("data: ") and kind:
                            hints.put((kind, json.loads(line[6:])))

        threading.Thread(target=watch, daemon=True).start()
        time.sleep(1.0)
        # the friend opens their link on their phone: the guichet knows them, nobody calls the owner's bridge
        lea = Phone(link["link"])
        guichet.redeem(lea.invite, lea.public, lea.proof(), "1")
        opened = time.time()
        deadline = opened + 10
        hint = None
        while time.time() < deadline:
            try:
                kind, data = hints.get(timeout=max(0.1, deadline - time.time()))
            except queue.Empty:
                break
            if kind == "share" and data.get("count") == 1:   # (the invitation itself may be hinted before: count 0)
                hint = data
                break
        assert hint is not None, "no share hint while the conversation is open"
        assert hint == {"agent": agent, "share": SHARE, "count": 1}
        assert time.time() - opened < 6   # within a couple of the bridge's looks (every 2 s)
        # keyed meanwhile: the friend can read, and the owner's next read says 1
        assert next(v for v in guichet.viewers if v["viewer"] == "1").get("wrapped")
        listed = bridge.get(f"/v1/read-shares?agent={agent}", headers=AUTH).json()["shares"]
        assert [s["count"] for s in listed] == [1]
    finally:
        stop.set()
        guichet.stop()
