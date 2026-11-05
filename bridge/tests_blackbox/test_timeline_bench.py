"""What a conversation's pages cost the bridge (thin apps, 5/11): a 2,000-message main conversation over 3 segments,
spread over 6 weeks, read the apps' ways (open: latest 100 / 50; catch-up after a cursor; around a notification's
message; an older page). Per request: bytes, latency (median), the bridge's CPU time (Linux /proc), and its RSS before
and after. `-s` prints the table; `BRIDGE_CMD` picks the binary (main's and a branch's compare). Runs only with
`BRIDGE_BENCH=1` (not in the regular run)."""
import os
import statistics
import time

import httpx
import pytest

from bbkit.common import AUTH

MESSAGES = int(os.environ.get("BENCH_MESSAGES", "2000"))
ROUNDS = int(os.environ.get("BENCH_ROUNDS", "50"))
TZ = "Europe/Paris"

pytestmark = pytest.mark.skipif(not os.environ.get("BRIDGE_BENCH"), reason="a measure: BRIDGE_BENCH=1")


def thread(fakes):
    """A Bot Chat of `MESSAGES` rows: person / agent in turn, one reply in four with a tool call, one every 30 min."""
    fakes.hermes.v0225 = True
    per = MESSAGES // 3
    rows = []
    for i in range(MESSAGES):
        at = 1_790_000_000 + i * 1800
        if i % 2 == 0:
            rows.append({"role": "user", "content": f"Message {i} : " + "quelques mots pour la taille d'un vrai message. " * 2,
                         "timestamp": at})
        elif i % 4 == 1:
            rows.append({"role": "assistant", "content": "", "timestamp": at,
                         "tool_calls": [{"id": f"call_{i}", "type": "function",
                                         "function": {"name": "web_search", "arguments": "{\"q\": \"train Lyon\"}"}}]})
            rows.append({"role": "tool", "content": "3 résultats…", "tool_call_id": f"call_{i}", "timestamp": at + 1})
            rows.append({"role": "assistant", "content": f"Réponse {i} : " + "une réponse de quelques lignes. " * 4,
                         "timestamp": at + 2})
        else:
            rows.append({"role": "assistant", "content": f"Réponse {i} : " + "une réponse de quelques lignes. " * 4,
                         "timestamp": at})
    fakes.hermes.add_session("s1", rows[:per], title="Bot Chat", last_active=1, started_at=1, end_reason="compression")
    fakes.hermes.add_session("s2", rows[per:2 * per], title="Bot Chat", last_active=2, started_at=2,
                             parent_session_id="s1", end_reason="compression")
    fakes.hermes.add_session("s3", rows[2 * per:], title="Bot Chat", last_active=3, started_at=3, parent_session_id="s2")


def cpu_seconds(pid):
    """The process' user + system CPU time (Linux), else 0."""
    try:
        fields = open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()
        return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
    except OSError:
        return 0.0


def rss_kb(pid):
    try:
        return next(int(line.split()[1]) for line in open(f"/proc/{pid}/status") if line.startswith("VmRSS:"))
    except (OSError, StopIteration):
        return 0


def test_timeline_pages_cost(bridges, fakes):
    thread(fakes)
    bridge = bridges.new().start()
    app = httpx.Client(base_url=bridge.url, headers=AUTH, timeout=60)
    base = {"agent": "wellness", "tz": TZ}
    whole = app.get("/v1/threads/main/timeline", params={**base, "limit": 500}).json()
    total = app.get("/v1/threads/main/timeline", params={**base, "limit": 1}).json()
    assert total["more_before"]
    middle = whole["items"][len(whole["items"]) // 2]
    screens = {
        "open (latest 100, iPhone)": {"limit": 100},
        "open (latest 50, Android)": {"limit": 50},
        "catch-up (after a cursor, 10 new)": {"after": whole["items"][-11]["cursor"], "limit": 500},
        "around a message (20 before, all after)": {"around": middle["id"], "before_count": 20, "after_count": "all"},
        "older page (50)": {"before": whole["items"][0]["cursor"], "limit": 50},
        # the bridge's own sizes (no `limit`: thin apps, brick 4; a bridge from before gives its default 50)
        "open (bridge's size)": {},
        "catch-up (bridge's size, 10 new)": {"after": whole["items"][-11]["cursor"]},
        "catch-up too long (bridge's size)": {"after": whole["items"][0]["cursor"]},
        "older page (bridge's size)": {"before": whole["items"][0]["cursor"]},
    }
    pid = bridge.process.pid
    for params in screens.values():   # warm
        app.get("/v1/threads/main/timeline", params={**base, **params})
    rss_before = rss_kb(pid)
    rows = {}
    for name, params in screens.items():
        times, size, days = [], 0, 0
        cpu = cpu_seconds(pid)
        for _ in range(ROUNDS):
            started = time.perf_counter()
            resp = app.get("/v1/threads/main/timeline", params={**base, **params})
            times.append(time.perf_counter() - started)
            assert resp.status_code == 200, resp.text[:200]
            size = len(resp.content)
            days = sum(1 for i in resp.json()["items"] if i.get("new_day"))
            items = len(resp.json()["items"])
        rows[name] = (items, size, statistics.median(times) * 1000, (cpu_seconds(pid) - cpu) / ROUNDS * 1000, days)
    rss_after = rss_kb(pid)
    print(f"\n{MESSAGES} messages, {ROUNDS} rounds each")
    print("| Request | items | bytes | ms (median) | CPU ms / request | day dividers |\n|---|---:|---:|---:|---:|---:|")
    for name, (items, size, ms, cpu, days) in rows.items():
        print(f"| {name} | {items} | {size} | {ms:.1f} | {cpu:.2f} | {days} |")
    print(f"Bridge RSS: {rss_before} kB before, {rss_after} kB after {ROUNDS * len(screens)} requests")
