"""Every read of a conversation: reading one never fails, whatever the cursor and whatever state it is in.

7/11: a catch-up with nothing new (`after=` the conversation's last item) while a turn went on was a 500 (thin-apps-7's
`layout.open` indexed the first item of an empty page). The iPhone catches up that way on every hint: it lost the
secret card placed with no item and the « Arrêter » of the answer going on (release journeys J13, J19). Neither the
unit tests nor this suite had asked for that page.

Each test puts a real conversation in one state: the release journeys' bridge and its scripted model
(bbkit/journeys.py, bbkit/model.py's markers), or rows stored in the fake Hermes, or a group discussion. Then it reads
it every way the apps do (HermesKit's `TimelineQuery`, Android's `ChatTimeline`): the latest page (the bridge's size
or a `limit`), `before=` and `after=` each item's cursor and the page's own `first` / `last`, a cursor whose item is
gone, one past the end, `around=` each item (20 before, all after) and an unknown one. Every page must be a 200 and
well-formed (`check`): its items the run of the conversation's that was asked for, `first` / `last` its own,
`more_before` / `more_after` what lies outside it, `layout.open` none or the item of the page where the turn going on
is (nothing decided from there on), the cards placed among the conversation's items, and the state unchanged.
The Rust side: bridge-rs/src/threads/timeline_guard.rs (every small thread, every `Want`)."""
import pytest

from bbkit.common import AUTH
from bbkit.journeys import AGENT, journey_bridge
from bbkit.servers import wait_until

# the bridge's own sizes (timeline.rs): the latest page, a catch-up at most, an older page; `around`'s 20 before
LATEST, CATCH_UP, OLDER, AROUND_BEFORE = 100, 200, 50, 20
WHOLE = 500   # LIMIT_MAX: the whole of these conversations in one page
KEYS = {"thread", "items", "first", "last", "more_before", "more_after", "anchor", "version", "pending", "reset",
        "joins"}


def cid(n: int) -> str:
    return f"T1MEL1NE-0000-4000-8000-{n:012d}"


class Reader:
    """A conversation's timeline, as an app asks for it."""

    def __init__(self, bridge, thread, **base):
        self.bridge, self.thread, self.base = bridge, thread, base

    def get(self, **params):
        return self.bridge.get(f"/v1/threads/{self.thread}/timeline", headers=AUTH, params={**self.base, **params})

    def whole(self):
        resp = self.get(limit=WHOLE)
        assert resp.status_code == 200, resp.text[:500]
        return resp.json()


def gone(cursor):
    """A cursor where an item was, the item gone (a rewind): its place, a sub-position no item has."""
    segment, order, sub = cursor.split(".")
    return f"{segment}.{order}.{int(sub) + 50}"


def expected(items, params):
    """The page `params` asks for, worked out from the whole conversation (`items`): its range, whether it is the
    window (`reset`), its anchor, the cursor it joins."""
    n = len(items)
    ids = [i["id"] for i in items]
    cursors = [i["cursor"] for i in items]
    # a message sent and not stored yet (« being sent », or « Pas envoyé » once nothing runs) comes in every catch-up
    sending = next((k for k, i in enumerate(items) if i.get("sending") or i.get("not_sent")), n)

    def latest(size):
        return max(0, n - size), n, True, None, None
    if "around" in params:
        found = [k for k, i in enumerate(ids) if i == params["around"] or i.startswith(params["around"] + ".")]
        if not found:
            return latest(LATEST)
        return max(0, found[0] - AROUND_BEFORE), n, True, ids[found[0]], None
    if "after" in params:
        if params["after"] not in cursors:
            return latest(LATEST)   # the phone's last item gone: the latest page, as the window
        k = cursors.index(params["after"])
        if n - (k + 1) > CATCH_UP:
            return latest(LATEST)
        start = min(k + 1, sending)
        return start, min(start + CATCH_UP, n), False, None, params["after"]
    if "before" in params:
        if params["before"] not in cursors:
            return latest(LATEST)
        k = cursors.index(params["before"])
        return max(0, k - OLDER), k, False, None, params["before"]
    return latest(params.get("limit", LATEST))


def check(items, params, resp, running):
    """What is wrong with the page read with `params` of the conversation whose items are `items` (none: well-formed);
    `running`: a turn goes on in it, started by its last message from the person."""
    if resp.status_code != 200:
        return f"{resp.status_code}: {resp.text[:300]}"
    page = resp.json()
    if not KEYS <= set(page):
        return f"keys missing: {KEYS - set(page)}"
    ids = [i["id"] for i in items]
    start, end, reset, anchor, joins = expected(items, params)
    got = [i["id"] for i in page["items"]]
    if got != ids[start:end]:
        return f"items {got}, expected {ids[start:end]} (of {ids})"
    if (page["reset"], page["anchor"], page["joins"]) != (reset, anchor, joins):
        return f"reset/anchor/joins {(page['reset'], page['anchor'], page['joins'])}, expected {(reset, anchor, joins)}"
    if (page["more_before"], page["more_after"]) != (start > 0, end < len(ids)):
        return f"more_before/more_after {(page['more_before'], page['more_after'])} for items {start}..{end} of {len(ids)}"
    cursors = {i["id"]: i["cursor"] for i in items}
    if any(i.get("cursor") != cursors[i["id"]] or not i.get("kind") for i in page["items"]):
        return "an item without its kind or with another cursor"
    edges = (page["items"][0]["cursor"], page["items"][-1]["cursor"]) if page["items"] else (None, None)
    if (page["first"], page["last"]) != edges:
        return f"first/last {(page['first'], page['last'])}, its items' {edges}"
    pending = page["pending"] or {}
    if running is not None and bool(pending.get("run")) != running:
        return f"pending.run {pending.get('run')}: the state changed while it was read"
    for card in (pending.get("approvals") or []) + (pending.get("questions") or []):
        if card.get("after") is not None and card["after"] not in ids:
            return f"a card placed after {card['after']}, not an item of the conversation"
    if "layout" in page:   # a conversation's (a group discussion's pages have none)
        open_ = page["layout"].get("open")
        # the turn going on starts at the last message from the person that starts one (words steered into the
        # answer and not stored yet do: they are the turn's last line, nothing of theirs is decided)
        turn = max((k for k, i in enumerate(items) if i["kind"] == "message" and i.get("role") == "person"
                    and not i.get("added_during_run")), default=None)
        want = ids[max(turn, start)] if running and turn is not None and start < end and turn < end else None
        if open_ != want:
            return f"layout.open {open_}, expected {want}"
        if open_ is not None and any("layout" in i for i in page["items"][got.index(open_):]):
            return "a line of the turn going on is decided"
    return None


def every_read(reader, running=None, label=""):
    """Reads the conversation every way the apps do and checks each page; the number of pages read."""
    whole = reader.whole()
    items = whole["items"]
    latest = reader.get()
    latest = latest.json() if latest.status_code == 200 else {}
    reads = [{}, {"limit": 1}, {"limit": 50}, {"limit": LATEST}]
    cursors = [i["cursor"] for i in items]
    # each item's cursor (first, middle, last…), the pages' own `first` / `last`, gone ones, one past the end
    page_edges = [c for c in (latest.get("first"), latest.get("last"), whole.get("first"), whole.get("last")) if c]
    for cursor in dict.fromkeys(cursors + page_edges + [gone(c) for c in cursors[:1] + cursors[-1:]] + ["9.999999.0"]):
        reads += [{"before": cursor}, {"after": cursor}]
    for id_ in [i["id"] for i in items] + ["m_unknown"]:
        reads.append({"around": id_, "before_count": AROUND_BEFORE, "after_count": "all"})
    failures = [f"{params}: {why}" for params in reads
                if (why := check(items, params, reader.get(**params), running))]
    assert not failures, f"{label}: {len(failures)} of {len(reads)} pages wrong:\n" + "\n".join(failures[:10])
    # the conversation stayed in the state it was read in
    assert [i["id"] for i in reader.whole()["items"]] == [i["id"] for i in items], f"{label}: it changed meanwhile"
    print(f"{label}: {len(items)} items, {len(reads)} pages read")
    return len(reads)


# -- the release journeys' conversations ------------------------------------------------------------------------

@pytest.fixture
def journey(bridges, fakes):
    return journey_bridge(bridges, fakes)


def conversation(journey):
    return Reader(journey.bridge, journey.thread, agent=AGENT, tz="Europe/Paris")


def sent(journey, n, words):
    resp = journey.send(cid(n), words)
    assert resp.status_code == 200 and resp.json()["state"] in ("started", "steered", "queued"), resp.text
    return resp.json()


def exchange(journey, n, words="Bonjour Vie, tu es là ?"):
    """One exchange, stored: the person's message and the agent's reply."""
    def replies():
        return sum(1 for i in journey.page()["items"] if i["kind"] == "message" and i.get("role") == "agent")
    before = replies()
    sent(journey, n, words)
    assert wait_until(lambda: replies() > before, timeout=60), journey.page()["items"]
    journey.idle()


def stored(journey, client_id):
    """The person's message is stored (Hermes took it), not « being sent » any more."""
    assert wait_until(lambda: [i for i in journey.page()["items"] if i.get("client_id") == client_id
                               and not i.get("sending")], timeout=30), journey.page()["items"]


def card(journey, prefix=""):
    def found():
        return [c for c in journey.page()["pending"].get("approvals") or [] if c.get("run_id", "").startswith(prefix)]
    assert wait_until(found, timeout=30), journey.page()["pending"]
    return found()[0]


def stop(journey):
    journey.bridge.post(f"/v1/threads/{journey.thread}/stop", headers=AUTH, json={"agent": AGENT})
    journey.idle()


def test_an_empty_conversation_reads_every_way(journey):
    reader = conversation(journey)
    assert reader.whole()["items"] == []
    every_read(reader, running=False, label="empty")


def test_one_stored_exchange_reads_every_way(journey):
    exchange(journey, 1)
    reader = conversation(journey)
    assert len(reader.whole()["items"]) == 2
    every_read(reader, running=False, label="one exchange")


def test_a_turn_going_on_reads_every_way(journey):
    """The page of 7/11 (J19): a phone reopened mid-turn catches up after its copy's last item, nothing new."""
    exchange(journey, 1)
    sent(journey, 2, "Raconte-moi tout #lent")
    stored(journey, cid(2))
    reader = conversation(journey)
    every_read(reader, running=True, label="a turn going on")
    # words steered into it: a message being sent, after every item, until the turn takes it
    assert sent(journey, 3, "Et aussi la météo ?")["state"] == "steered"
    assert wait_until(lambda: reader.whole()["items"][-1].get("client_id") == cid(3), timeout=10)
    every_read(reader, running=True, label="a turn going on, words steered into it")
    stop(journey)
    # nothing runs now: the words no turn took are « Pas envoyé », and still come in every catch-up
    assert wait_until(lambda: reader.whole()["items"][-1].get("not_sent"), timeout=30), reader.whole()["items"][-1]
    every_read(reader, running=False, label="words no turn took")


def test_a_secret_card_placed_with_no_item_reads_every_way(journey):
    """The page of 7/11 (J13): the phone catches up on the card's hint, nothing after its last item."""
    sent(journey, 1, "Connecte-toi à mon compte #secret")
    stored(journey, cid(1))
    secret = card(journey, "bip-secret-")
    every_read(conversation(journey), running=True, label="a secret card waiting")
    # refused: the agent's call returns and its turn ends (a Stop does not reach the scripted model inside the call)
    refused = journey.bridge.post(f"/v1/vault/secrets/requests/{secret['request_id']}", headers=AUTH,
                                  json={"allow": False})
    assert refused.status_code == 200, refused.text
    journey.idle()


def test_an_approval_waiting_reads_every_way(journey):
    exchange(journey, 1)
    sent(journey, 2, "Fais le ménage #approbation")
    stored(journey, cid(2))
    card(journey)
    every_read(conversation(journey), running=True, label="an approval waiting")
    stop(journey)


def test_a_question_waiting_reads_every_way(journey):
    sent(journey, 1, "Réserve un train #question")
    stored(journey, cid(1))
    assert wait_until(lambda: journey.page()["pending"].get("questions"), timeout=30), journey.page()["pending"]
    every_read(conversation(journey), running=True, label="a question waiting")
    stop(journey)


def test_a_turn_with_tools_and_a_file_reads_every_way(journey):
    exchange(journey, 1, "Les horaires de la piscine ? #outil")
    exchange(journey, 2, "Envoie-moi le rapport #fichier")
    every_read(conversation(journey), running=False, label="tools and a file")


# -- a conversation stored over several days, in two segments (Hermes compressed it) ------------------------------

def days_of_rows():
    """Six days of exchanges, 8 h apart, one reply in three after a tool call; the first half in a session Hermes
    compressed into the second."""
    rows = []
    for i in range(18):
        at = 1_790_000_000 + i * 8 * 3600
        rows.append({"role": "user", "content": f"Message {i} : on en parle ?", "timestamp": at})
        if i % 3 == 1:
            rows.append({"role": "assistant", "content": "", "timestamp": at + 60, "tool_calls": [
                {"id": f"call_{i}", "type": "function", "function": {"name": "web_search", "arguments": "{\"q\": \"x\"}"}}]})
            rows.append({"role": "tool", "content": "3 résultats", "tool_call_id": f"call_{i}", "timestamp": at + 61})
        rows.append({"role": "assistant", "content": f"Réponse {i} : bien sûr.", "timestamp": at + 120})
    return rows


def test_a_conversation_over_several_days_reads_every_way(bridges, fakes):
    fakes.hermes.v0225 = True
    rows = days_of_rows()
    fakes.hermes.add_session("s1", rows[:30], title="Bot Chat", last_active=1, started_at=1, end_reason="compression")
    fakes.hermes.add_session("s2", rows[30:], title="Bot Chat", last_active=2, started_at=2, parent_session_id="s1")
    bridge = bridges.new().start()
    main = bridge.get("/v1/threads/main", headers=AUTH, params={"agent": "wellness"})
    assert main.status_code == 200, main.text
    reader = Reader(bridge, main.json()["id"], agent="wellness", tz="Europe/Paris")
    items = reader.whole()["items"]
    assert len(items) == len(rows) - 6 and sum(1 for i in items if i.get("new_day")) >= 5, items
    every_read(reader, running=False, label="six days, two segments")


# -- a group discussion -------------------------------------------------------------------------------------------

def test_a_group_discussion_reads_every_way(bridges, fakes):
    from test_rooms import quiet, room_of, send, start
    bridge = start(bridges, fakes)
    fakes.hermes.rounds = 1
    room = room_of(bridge, "coach", "cuisine")
    reader = Reader(bridge, f"room:{room['id']}")
    every_read(reader, label="an empty group discussion")
    for n, words in enumerate(["@coach une idée pour ce soir ?", "@cuisine et pour le dîner ?"]):
        assert send(bridge, room["id"], words).status_code == 200
        # its answer stored, nobody answering any more
        assert wait_until(lambda: len(reader.whole()["items"]) >= 2 * (n + 1) and quiet(bridge, room["id"]),
                          timeout=30), reader.whole()["items"]
    every_read(reader, label="a group discussion")
