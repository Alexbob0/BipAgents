"""Pages in shared conversations (shared_pages.rs, Alex 2026-11-09): a relative talking to a shared agent may have a
page made — her turn still reads nothing of the owner's —, the page is marked as made at her request, seen only by her
household (through her own bridge, never by a link), and limited per day; the owner's own turn, while she talks, keeps
everything open and its page unmarked."""
import threading
import time

from bbkit.common import AUTH, sse
from bbkit.mcp import TOKEN, call, give_token, media_of, text_of
from test_shared_agents import link, wait

SESSION = "api_main"


def household(bridges, fakes, name, agents, per_day=None):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in agents}
        if per_day is not None:
            c["shared_pages"] = {"friend_pages_per_day": per_day}
    bridge = bridges.new(name, change=change)
    bridge.config["public_url"] = bridge.url
    return bridge.start()


def marie_talks(a, b, fakes, gate, client_id="C0FFEE00-0000-4000-8000-0000000000A1"):
    """Alex shares Cuisine with Marie; her turn r1 goes on until `gate`."""
    fakes.hermes.rounds = 1000
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie"]}).status_code == 200
    assert b.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta="Je prépare"), gate,
                               sse("run.completed", run_id="r1", output="Voici le menu.")]
    sent = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={"agent": "cuisine__alex", "client_id": client_id, "text": "Un menu pour samedi, en page ?"})
    assert sent.status_code == 200, sent.text
    assert wait(lambda: "r1" in fakes.hermes.started)
    give_token(a, "cuisine", TOKEN)
    return sent.json()["thread"]


def test_a_relative_has_a_page_made_seen_only_by_her_household_and_marked(bridges, fakes):
    a = household(bridges, fakes, "alex", ["cuisine"])
    b = household(bridges, fakes, "marie", ["vie"])
    c = household(bridges, fakes, "paul", ["jardin"])
    link(a, b)
    assert c.put("/v1/family/name", headers=AUTH, json={"name": "Paul"}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    assert c.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]}).status_code == 200
    # Alex's own page, made before anyone talks: never Marie's to see or change
    give_token(a, "cuisine", TOKEN)
    own = call(a, "create_page", {"html": "<h1>Budget courses</h1><p>Compte joint : 1 240 €</p>", "title": "Budget"})
    assert own["isError"] is False, text_of(own)
    own_page = media_of(own).rsplit("/", 1)[1][:-5]

    gate = threading.Event()
    cid = marie_talks(a, b, fakes, gate)
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie", "paul"]}).status_code == 200
    assert c.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    # Hermes is told her turn may make a page (patch 0018's list), nothing more
    assert "mcp__bipagents__create_page" in fakes.hermes.started["r1"]["guest_turn"]["tools"]
    assert "create_page" in fakes.hermes.started["r1"]["instructions"]

    # during her turn: a page made, marked as hers; Alex's data stays closed, his page can't be read nor changed
    made = call(a, "create_page", {"html": "<h1>Menu de samedi</h1><p>Fondue, salade.</p>", "title": "Menu de samedi",
                                   "notebook": "Santé"})
    assert made["isError"] is False, text_of(made)
    path = media_of(made)
    page = path.rsplit("/", 1)[1][:-5]
    assert path == f"/bipagents/pages/cuisine/{page}.html" and "Carnet" not in text_of(made)
    for name, args in [("mail_search", {"query": "banque"}), ("notebook_search", {"query": "x"}),
                       ("read_page", {"page": own_page}), ("list_pages", {})]:
        refused = call(a, name, args)
        assert refused["isError"] is True and "closed during their turn" in text_of(refused), name
    changed = call(a, "create_page", {"html": "<h1>Volé</h1>", "page": own_page})
    assert changed["isError"] is True and "not a page made for Marie" in text_of(changed)
    listed = {p["page"]: p for p in a.get("/v1/pages", headers=AUTH, params={"agent": "cuisine"}).json()["pages"]}
    assert listed[page]["group"] is True and listed[page]["asked_by"] == "Marie"
    assert "group" not in listed[own_page] and "asked_by" not in listed[own_page]
    assert "Budget courses" in (a.state_dir / "agent-pages" / "cuisine" / f"{own_page}.html").read_text()

    # her turn ends with the page in its reply: her timeline shows it under her own id for the agent
    session = fakes.hermes.started["r1"]["session_id"]
    fakes.hermes.messages[("cuisine", session)] = [
        {"role": "user", "content": fakes.hermes.started["r1"]["input"]},
        {"role": "assistant", "content": f"Voici le menu.\nMEDIA:{path}\nBon appétit !\nMEDIA:/bipagents/pages/cuisine/{own_page}.html"}]
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    timeline = lambda: b.get(f"/v1/threads/{cid}/timeline", headers=AUTH, params={"agent": "cuisine__alex"}).json()  # noqa: E731
    assert wait(lambda: timeline()["pending"]["run"] is None and timeline()["items"][-1]["role"] == "agent", timeout=15)
    reply = timeline()["items"][-1]
    assert [x["path"] for x in reply.get("attachments", [])] == [f"/bipagents/pages/cuisine__alex/{page}.html"]
    assert reply["attachments"][0]["kind"] == "page" and "url" not in reply["attachments"][0]

    # through her bridge, the page opens, locked and marked; Alex's own page doesn't, even named
    shown = b.get("/v1/media", headers=AUTH, params={"path": f"/bipagents/pages/cuisine__alex/{page}.html",
                                                     "agent": "cuisine__alex"})
    assert shown.status_code == 200, shown.text
    assert shown.headers["content-type"].startswith("text/html")
    assert shown.headers["content-security-policy"].startswith("sandbox allow-scripts")
    assert "Menu de samedi" in shown.text and 'class="bip-asked"' in shown.text and "Marie" in shown.text
    assert '<meta name="bipagents-group" content="1">' in shown.text
    for path_ in (f"/bipagents/pages/cuisine__alex/{own_page}.html", "/bipagents/pages/cuisine__alex/x%2F..%2Fy.html"):
        assert b.get("/v1/media", headers=AUTH, params={"path": path_}).status_code == 404, path_
    # Paul shares the same agent: Marie's page is not his
    assert c.get("/v1/media", headers=AUTH, params={"path": f"/bipagents/pages/cuisine__alex/{page}.html"}).status_code == 404
    # nobody without a household's key
    assert a.get(f"/family/s/cuisine/pages/{page}", headers={"Authorization": "Bearer nope"}).status_code == 401
    assert "a page not made for this household asked for" in a.log_tail(20000)


def test_a_relatives_pages_are_limited_per_day_in_plain_words(bridges, fakes):
    a = household(bridges, fakes, "alex", ["cuisine"], per_day=2)
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    gate = threading.Event()
    marie_talks(a, b, fakes, gate)
    first = call(a, "create_page", {"html": "<h1>Liste</h1>", "title": "Liste"})
    second = call(a, "create_page", {"html": "<h1>Plan</h1>", "title": "Plan"})
    assert first["isError"] is False and second["isError"] is False
    third = call(a, "create_page", {"html": "<h1>Encore</h1>", "title": "Encore"})
    assert third["isError"] is True
    assert "reached today's limit of new pages (2)" in text_of(third)
    assert "you can ask for more tomorrow" in text_of(third)
    # one of her own pages changed: not a new one, still fine
    mine = media_of(first).rsplit("/", 1)[1][:-5]
    again = call(a, "create_page", {"html": "<h1>Liste v2</h1>", "page": mine})
    assert again["isError"] is False and "version 2" in text_of(again)
    gate.set()


def test_the_owners_turn_keeps_everything_while_a_relative_talks(bridges, fakes):
    """Alex talks to Cuisine in his own conversation while Marie's turn goes on: his call is his (the hub sees his run
    inside the tool), so his page is unmarked and uncounted; a call the hub can't place stays closed."""
    fakes.hermes.v0225 = True
    fakes.hermes.add_session(SESSION, [{"role": "user", "content": "Bonjour"}, {"role": "assistant", "content": "Salut"}],
                             title="Bot Chat", agent="cuisine", last_active=10, started_at=1)
    a = household(bridges, fakes, "alex", ["cuisine"], per_day=0)
    b = household(bridges, fakes, "marie", ["vie"])
    link(a, b)
    gate = threading.Event()
    marie_talks(a, b, fakes, gate)
    # no run of Alex's inside the tool: Marie's call (and today she may make none: the limit is 0)
    refused = call(a, "create_page", {"html": "<h1>x</h1>", "title": "x"})
    assert refused["isError"] is True and "today's limit" in text_of(refused)
    assert call(a, "mail_search", {"query": "x"})["isError"] is True

    mine = threading.Event()
    fakes.hermes.runs["r2"] = [sse("tool.started", run_id="r2", tool="mcp__bipagents__create_page"), mine,
                               sse("run.completed", run_id="r2", output="Fait.")]
    main = a.get("/v1/threads/main", headers=AUTH, params={"agent": "cuisine"}).json()["id"]
    sent = a.post(f"/v1/threads/{main}/messages", headers=AUTH,
                  json={"agent": "cuisine", "client_id": "C0FFEE00-0000-4000-8000-0000000000B1", "text": "Une page budget"})
    assert sent.status_code == 200 and sent.json()["run_id"] == "r2", sent.text
    time.sleep(1.0)   # the hub reads his run's tool.started
    own = call(a, "create_page", {"html": "<h1>Budget</h1>", "title": "Budget"})
    assert own["isError"] is False, text_of(own)
    listed = {p["page"]: p for p in a.get("/v1/pages", headers=AUTH, params={"agent": "cuisine"}).json()["pages"]}
    assert "asked_by" not in listed["budget"] and "group" not in listed["budget"]
    html = (a.state_dir / "agent-pages" / "cuisine" / "budget.html").read_text()
    assert "bip-asked" not in html and "bipagents-group" not in html
    mine.set()
    gate.set()
