"""Review of turn-status (17/11): where a long turn's time went (`took`, turn_status.rs, kept in bridge.db `turn_times`
by the run hub, put on the turn's last reply by threads `with_took`) names the site it went to (« surtout sur
pharmacie-exemple.fr (1 min) »). The owner reads it; a family member (design D8 B: « jamais de site ») and a reader
(« Friends and feeds: none », docs/conversations.md §6.5b) never do. The hub keeps a turn of 60 s or more: here its
row is written as the hub writes it (`Store::save_turn_time`), for a reply already in the conversation."""
import json
import time

from bbkit.common import AUTH
from test_family_agent import make_family_agent
from test_households_and_peers import household
from test_shared_agents import SESSION, SHARED, alex_and_marie, link, share

SITE = "pharmacie-exemple.fr"


def kept(bridge, agent, started, ended):
    """A long turn's summary as the run hub keeps it (runs.rs `keep_took`)."""
    took = {"secs": round(ended - started), "model": 15, "tools": 60, "person": 0, "agent": 0, "actions": 3,
            "top": [{"what": "site", "site": SITE, "secs": 60}]}
    with bridge.db() as db:
        db.execute("INSERT INTO turn_times (agent, run_id, session_id, started, ended, took) VALUES (?, ?, ?, ?, ?, ?)",
                   (agent, "r_long", SESSION, started, ended, json.dumps(took)))


def timeline(bridge, agent):
    answer = bridge.get("/v1/threads/main/timeline", headers=AUTH, params={"agent": agent})
    assert answer.status_code == 200, answer.text
    return answer.json()


def replies(page):
    return [i for i in page["items"] if i.get("role") == "agent"]


def test_a_family_member_never_reads_a_site_in_a_long_turns_summary(bridges, fakes):
    fakes.hermes.v0225 = True
    now = time.time()
    fakes.hermes.add_session(SESSION, [
        {"id": 1, "role": "user", "content": "Trouve la pharmacie de garde", "timestamp": now - 80},
        {"id": 2, "role": "assistant", "content": "C'est celle de la place.", "timestamp": now - 5},
    ], title="Bot Chat", agent="famille", last_active=now - 5, started_at=now - 80)
    a = household(bridges, fakes, "alex", ["famille", "budget"])
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    make_family_agent(a)
    assert b.get("/v1/family-agent", headers=AUTH).status_code == 200
    kept(a, "famille", now - 80, now - 6)

    # the owner reads where the time went (the row is placed: this test says something)
    owner = timeline(a, "famille")
    took = [r.get("took") for r in replies(owner) if r.get("took")]
    assert took and took[0]["top"][0]["site"] == SITE, replies(owner)
    # the member reads the same reply, never the site
    member = timeline(b, "famille__alex")
    assert [r["text"] for r in replies(member)] == ["C'est celle de la place."], member["items"]
    assert SITE not in json.dumps(member, ensure_ascii=False), replies(member)


def test_a_reader_never_reads_a_site_in_a_long_turns_summary(bridges, fakes):
    a, b = alex_and_marie(bridges, fakes)
    share(a, b)
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    reply = next(m for m in fakes.hermes.messages[("cuisine", SESSION)] if m["role"] == "assistant" and m["id"] == 4)
    kept(a, "cuisine", reply["timestamp"] - 70, reply["timestamp"] - 1)

    owner = timeline(a, "cuisine")
    assert any((r.get("took") or {}).get("top", [{}])[0].get("site") == SITE for r in replies(owner)), replies(owner)
    page = b.get("/v1/threads/main/timeline", headers=AUTH, params=SHARED)
    assert page.status_code == 200, page.text
    assert [r["text"] for r in replies(page.json())] == ["Voici le menu."], page.json()["items"]
    assert SITE not in json.dumps(page.json(), ensure_ascii=False), replies(page.json())
