"""An install switches implementation and back without losing anything: a bridge builds up state (devices, outbox
and audio, an agent created in the app, console session, device key, vault item, mailbox, household, reminders,
links, a proposed job, a call's brief, a page, a watched run waiting for an approval, the agents' tool tokens), stops;
the other implementation starts on the same config and data and must show the same; then the first one again.

The other implementation is `BRIDGE_SWITCH_CMD` (e.g. an earlier release's binary); without it, the same command is
restarted (persistence across a restart)."""
import json
import os
import shlex

from bbkit.bridge import bridge_command
from bbkit.common import AUTH, sse
from bbkit.mcp import call, give_token, rpc, text_of
from bbkit.servers import wait_until
from conftest import managed_install, profile, wait_registered

ANSWER = json.dumps({"id": "vault_1", "removed": True}) + "\n"
TOKEN = "f" * 64
FCM = "eXaMpLe:APA91bH_" + "y" * 60


def other_command():
    raw = os.environ.get("BRIDGE_SWITCH_CMD", "").strip()
    return shlex.split(raw) if raw else bridge_command()


def build(bridge, fakes):
    """State through the routes, as the apps and the agents make it."""
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": TOKEN, "environment": "sandbox",
                                                           "agent_ids": ["wellness"]}).status_code == 200
    assert bridge.post("/v1/devices", headers=AUTH, json={"token": FCM, "environment": "fcm"}).status_code == 200
    assert wait_until(lambda: bridge.get("/v1/outbox", headers=AUTH).json()["items"])
    item = bridge.get("/v1/outbox", headers=AUTH).json()["items"][0]
    assert wait_until(lambda: bridge.get(f"/v1/outbox/{item['id']}/audio", headers=AUTH).status_code == 200)
    assert bridge.put("/v1/cron-jobs/wellness/fc344ed09026", headers=AUTH, json={"notify": False}).status_code == 200
    code = bridge.cli("console").stdout.split("/console/#", 1)[1].split()[0]
    login = bridge.post("/console/api/login", json={"code": code, "name": "PC"})
    cookie = login.headers["set-cookie"].split(";", 1)[0]
    device = bridge.post("/v1/device-keys", headers=AUTH, json={"name": "iPad"}).json()
    created = bridge.post("/v1/agents", headers=AUTH, json={"name": "Max", "kind": "finance", "lang": "en"})
    assert created.status_code == 201, created.text   # its [agents.max] table and profile, read by the other one
    assert bridge.post("/v1/vault", headers=AUTH, json={"label": "SNCF", "site": "sncf-connect.com", "identifier": "a@b.c",
                                                         "password": "s3cret", "agents": ["vie"]}).status_code == 201
    added = bridge.post("/v1/mail/accounts", headers=AUTH, json={
        "provider": "proton", "username": "alex@proton.me", "password": "pw-1234", "port": fakes.imap.port, "agents": ["coach"]})
    assert added.status_code == 201, added.text
    bridge.put("/v1/family/name", headers=AUTH, json={"name": "Alex"})
    bridge.put("/v1/reminders/agents", headers=AUTH, json={"agents": ["coach"]})
    bridge.put("/v1/reminders", headers=AUTH, json={"lists": [{"id": "L1", "title": "Courses"}],
                                                     "items": [{"id": "r1", "list": "L1", "title": "Lait"}]})
    call(bridge, "reminders_add", {"title": "Pain"}, token=keys_of(bridge)["coach"])
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": [{"from": "coach", "to": "wellness"}]}).status_code == 200
    give_token(bridge)
    assert not call(bridge, "propose_task", {"title": "Dossier", "prompt": "Prépare-le.", "budget": "small"})["isError"]
    assert not call(bridge, "create_page", {"html": "<p>Suivi</p>", "title": "Suivi"})["isError"]
    bridge.post("/v1/live/calls", headers=AUTH, json={"agent": "wellness", "session": "s1", "live_session": "live1"})
    bridge.post("/v1/live/recaps", headers=AUTH, json={"agent": "wellness", "session": "s1", "recap": "Appel de 10:05"})
    bridge.put("/v1/agents/wellness/appearance", headers=AUTH, json={"category": "home", "color": "#112233"})
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_wait", "session_id": "api_bot"})
    assert wait_until(lambda: bridge.db().execute("SELECT count(*) FROM watched_runs").fetchone()[0] == 1)
    return {"cookie": cookie, "device_key": device["key"], "outbox_id": item["id"]}


def keys_of(bridge):
    """The agents' tool tokens as their Hermes holds them (the bridge wrote them in each profile)."""
    import yaml
    home = bridge.config["hermes"]["home"]
    entry = yaml.safe_load(open(f"{home}/profiles/coach/config.yaml"))["mcp_servers"]["bipagents"]
    return {"coach": entry["headers"]["Authorization"].removeprefix("Bearer ")}


def snapshot(bridge, keys):
    """What the apps and the agents can read back (nothing that changes it)."""
    def get(path, **kw):
        resp = bridge.get(path, **kw)
        assert resp.status_code == 200, (path, resp.status_code, resp.text)
        return resp.json()
    console = {"Cookie": keys["cookie"], "X-Bip-Console": "1"}
    with bridge.db() as db:
        devices = sorted(db.execute("SELECT token, environment, agent_ids, created_at FROM devices").fetchall())
    assert wait_until(lambda: bridge.get("/v1/approvals", headers=AUTH).json()["items"])
    approvals = [{k: v for k, v in a.items() if k != "created_at"} for a in get("/v1/approvals", headers=AUTH)["items"]]
    pages = get("/v1/pages", params={"agent": "wellness"}, headers=AUTH)["pages"]
    coach = keys_of(bridge)["coach"]
    tools = rpc(bridge, "tools/list", token=coach).json()["result"]["tools"]
    return {
        "agents": get("/v1/agents", headers=AUTH),
        "max": get("/v1/agents/max/settings", headers=AUTH),
        "devices": devices,
        "outbox": get("/v1/outbox", headers=AUTH)["items"],
        "audio": bridge.get(f"/v1/outbox/{keys['outbox_id']}/audio", headers=AUTH).content,
        "cron": get("/v1/cron-jobs", headers=AUTH)["jobs"],
        "console": (get("/console/api/me", headers=console), [d["name"] for d in get("/v1/console/devices", headers=AUTH)["devices"]]),
        "device_keys": [(d["name"], d["me"]) for d in get("/v1/device-keys", headers={"Authorization": f"Bearer {keys['device_key']}"})["devices"]],
        "vault": get("/v1/vault", headers=AUTH),
        "mail": get("/v1/mail/accounts", headers=AUTH)["accounts"],
        "mail_search": text_of(call(bridge, "mail_search", {}, token=coach)),
        "family": get("/v1/family", headers=AUTH),
        "reminders": ({k: v for k, v in get("/v1/reminders", headers=AUTH).items() if k != "at"},
                      [(op["op"], op["args"]) for op in get("/v1/reminders/queue", headers=AUTH)["queue"]]),
        "links": get("/v1/agent-links", headers=AUTH),
        "jobs": get("/v1/jobs", headers=AUTH),
        "brief": get("/v1/live/brief", params={"agent": "wellness", "session": "s1"}, headers=AUTH)["text"],
        "recap": (bridge.state_dir / "call-recaps" / "wellness" / "s1.txt").read_text(),
        "pages": pages,
        "page": bridge.get("/v1/media", params={"path": pages[0]["path"]}, headers=AUTH).text,
        "approvals": approvals,
        "tools": [t["name"] for t in tools],
    }


def test_an_install_switches_implementation_and_back(bridges, fakes):
    bridge = bridges.new()
    # Its table named as the bridge rewrites it (README, « Found while writing this suite », 2).
    bridge.config["agents"]["wellness"] = bridge.config["agents"].pop("Wellness")
    home = managed_install(bridge, fakes, agents=("coach",), isolated=True)
    folder = profile(home, "coach")
    (folder / "config.yaml").write_text("model:\n  provider: custom\n  default: glm\n")
    bridge.config["agents"]["vie"] = {"hermes_url": fakes.down.url, "hermes_key": "v" * 32, "isolated": True,
                                      "python": f"{bridge.fakebin}/podman exec -i hermes-vie /opt/py",
                                      "hermes_home": "/home/hermes/.hermes"}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-vie"], stdout=ANSWER)
    bridge.seed_db([("INSERT INTO cron_jobs (agent, job, name, notify, last_seen) VALUES (?,?,?,?,?)",
                     ("wellness", "fc344ed09026", "Scan des e-mails", 1, "2026-10-21T08:00:00.000Z"))])
    fakes.ntfy.lines["hermes-wellness-out"] = [json.dumps({"id": "m1", "event": "message", "message": "Bonjour **Sam**",
                                                           "tags": ["session:s1"]})]
    fakes.imap.users["alex@proton.me"] = "pw-1234"
    fakes.imap.add("INBOX", 7, b"From: Sam <sam@example.com>\r\nSubject: Repas\r\nDate: Mon, 19 Oct 2026 10:00:00 +0200\r\n"
                               b"\r\nOn mange ?")
    fakes.hermes.runs["run_wait"] = [__import__("threading").Event()]
    fakes.hermes.run_info["run_wait"] = {"status": "waiting_for_approval", "session_id": "api_bot",
                                         "approval": {"request_id": "req_x", "command": "ls", "choices": ["once", "deny"]}}
    fakes.hermes.add_session("s1", [{"role": "user", "content": "Bonjour"}, {"role": "assistant", "content": "Salut !"}])
    fakes.hermes.add_session("live1", [{"role": "user", "content": "Je pars."}, {"role": "assistant", "content": "Bon voyage."}])
    bridge.start()
    wait_registered(bridge, ["coach"])
    keys = build(bridge, fakes)
    bridge.restart(write=False)     # the first implementation, again: the approval rebuilt from Hermes' run status
    before = snapshot(bridge, keys)
    for command in (other_command(), bridge_command()):
        bridge.stop()
        fakes.ntfy.subscriptions.clear()
        bridge.command = command
        bridge.start(write=False)
        after = snapshot(bridge, keys)
        assert after == before, [k for k in before if before[k] != after.get(k)]
        assert wait_until(lambda: fakes.ntfy.subscriptions)
        assert fakes.ntfy.subscriptions[0].params.get("since") == "m1"            # the ntfy cursor kept
