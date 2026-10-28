"""Watchers, « veilleurs » (bridge-rs/src/watchers.rs): a connector with `watch: {every, notify}` whose program the
bridge runs by itself (the fake podman plays the container here), with NO model call, and compares what it printed
with the state the agent last knew: the first check notes where comparisons start, no change gives no turn, a change
gives exactly ONE turn of the agent in the watcher's own conversation (a hidden note with the before and after), whose
message goes to the Boîte (pushed only when it starts with URGENT, kept to itself with NOTHING); wake-ups are
rate-limited; failures in a row make it « en panne » and the agent is told once; the list « Ce que tes agents font
tout seuls » shows it in « Quand quelque chose change »."""
import hashlib
import json
import time

import pytest

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import profile, wait_registered
from test_connectors import IMAGE, TOKEN, approve_next, rpc, text
from test_sandbox import isolated

NOW = "2026-10-28T08:00:00+00:00"
DEVICE = "a" * 64

CHECK = r'''
import json, sys
mode = open(__VALUE__).read().strip()
if mode.startswith("fail"):
    sys.stderr.write("the site answered 503\n")
    sys.exit(1)
print("looking at the doctor's agenda…")
print(json.dumps({"state": {"slots": [mode], "checked_at": None}, "summary": "Prochain créneau : " + mode,
                  "condition": mode.startswith("low")}))
'''

REGISTER = {"name": "Créneau médecin", "sites": ["www.doctolib.example"],
            "description": "Les créneaux libres du Dr Martin", "icon": "health",
            "folder": "/workspace/connecteurs/creneau", "entry": "python3 check.py",
            "watch": {"every": "15m", "notify": "on_change"}}


def setup(bridges, fakes, watchers=None, notify="on_change"):
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie",))
    bridge.config["agents"]["vie"].update({"isolated": True, "python": f"{bridge.fakebin}/podman exec -i hermes-vie python",
                                           "hermes_home": "/home/hermes/.hermes"})
    bridge.config["hermes"]["isolated"] = True
    # the scheduler out of the way: the tests check « now »
    bridge.config["watchers"] = {"tick_seconds": 3600, "wake_gap_minutes": 0, "failures_max": 3,
                                 "run_now_gap_seconds": 0, **(watchers or {})}
    bridge.commands.rule("podman", ["exec", "-i", "hermes-vie"], stdout=json.dumps({"id": "x"}) + "\n")
    bridge.commands.rule("podman", ["run", "--rm", "-i"], exec_image=IMAGE)
    value = bridge.root / "value.txt"
    value.write_text("mardi 10:00")
    work = profile(hub, "vie") / "sandboxes" / "docker" / "default" / "workspace" / "connecteurs" / "creneau"
    work.mkdir(parents=True)
    (work / "check.py").write_text(CHECK.replace("__VALUE__", repr(str(value))))
    bridge.seed_db([("INSERT INTO devices (token, environment, agent_ids, created_at, updated_at) VALUES (?,?,?,?,?)",
                     (DEVICE, "sandbox", "[]", NOW, NOW))])
    bridge.start()
    wait_registered(bridge, ["vie"])
    path = bridge.state_dir / "agent-tools.json"
    tokens = json.loads(path.read_text())["tokens"]
    tokens["vie"] = hashlib.sha256(TOKEN.encode()).hexdigest()
    path.write_text(json.dumps({"tokens": tokens}))
    result, card = approve_next(bridge, "connector_register", {**REGISTER, "watch": {"every": "15m", "notify": notify}})
    assert "approved" in text(result), text(result)
    conn = bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"][0]
    return bridge, value, conn, card


def check(bridge, conn):
    resp = bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/run", headers=AUTH, params={"wait": "true"}, timeout=120)
    assert resp.status_code == 200, resp.text
    return resp.json()["outcome"]


def turns(fakes):
    return [r for r in fakes.hermes.started.values() if "<!--bip:watch" in (r.get("input") or "")]


def row(bridge, conn):
    items = bridge.get("/v1/autonomous", headers=AUTH, params={"agent": "vie"}).json()["items"]
    return next(i for i in items if i["id"] == f"watcher:vie:{conn['id']}")


def boite(bridge):
    return [i for i in bridge.get("/v1/outbox", headers=AUTH).json()["items"] if i["title"] == "Créneau médecin"]


def podman_runs(bridge):
    return [a for a in bridge.commands.of("podman") if a[:3] == ["run", "--rm", "-i"] and "python3" in a
            and "check.py" in a]


def test_a_change_wakes_the_agent_once_and_no_change_never(bridges, fakes):
    bridge, value, conn, card = setup(bridges, fakes)
    # the card says how often and when it wakes the agent
    assert card["description"] == "Vie a créé le veilleur Créneau médecin"
    assert card["bip_connector_schedule"] == "Vérifie toutes les 15 min, ne réveille l'agent que si ça change"
    assert card["bip_connector_watch"]["minutes"] == 15 and card["bip_connector_watch"]["notify"] == "on_change"
    assert conn["watch"]["every"] == "15m" and conn["watch"]["last_check"] is None and conn["schedule"] is None

    # 1. the first check: where comparisons start, no turn
    first = check(bridge, conn)
    assert (first["ok"], first["step"], first["woke"]) == (True, "baseline", False), first
    # 2. the same state again (keys printed in another order would be the same too): no turn, no model call
    again = check(bridge, conn)
    assert again["step"] == "same" and not again["woke"]
    assert turns(fakes) == [] and not fakes.model.requests
    assert len(podman_runs(bridge)) == 2

    # 3. a change: exactly ONE turn, in the watcher's own conversation, with the before and after
    fakes.hermes.run_output = "Un créneau s'est libéré mercredi à 9 h chez le Dr Martin."
    value.write_text("mercredi 09:00")
    woke = check(bridge, conn)
    assert (woke["step"], woke["woke"], woke["sent"], woke["urgent"]) == ("woke", True, True, False), woke
    [turn] = turns(fakes)
    note = turn["input"]
    assert note.startswith("<!--bip:watch\nYour watcher « Créneau médecin » (Les créneaux libres du Dr Martin)")
    assert "Before (what you last knew): Prochain créneau : mardi 10:00" in note
    assert "Now: Prochain créneau : mercredi 09:00" in note and "exactly NOTHING" in note and note.endswith("-->")
    assert not turn.get("instructions")
    created = [body for agent, body in fakes.hermes.created if agent == "vie"]
    assert created[-1]["title"] == "Watcher: Créneau médecin"   # in the agent's language (« Veilleur : … » in French)
    # its message in the Boîte, tagged with its conversation; not urgent: no notification
    assert wait_until(lambda: boite(bridge), timeout=20)
    item = boite(bridge)[0]
    assert item["text"] == "Un créneau s'est libéré mercredi à 9 h chez le Dr Martin."
    assert item["session_id"] == turn["session_id"]
    time.sleep(1)
    assert not [p for p in fakes.apns.payloads() if p.get("outbox_id") == item["id"]]

    # 4. nothing new since: no other turn
    assert check(bridge, conn)["step"] == "same"
    assert len(turns(fakes)) == 1

    # 5. urgent: the message is pushed; the same conversation goes on
    fakes.hermes.run_output = "URGENT\nUn créneau ce soir à 18 h : réserve vite."
    value.write_text("ce soir 18:00")
    urgent = check(bridge, conn)
    assert urgent["urgent"] is True and len(turns(fakes)) == 2
    assert turns(fakes)[1]["session_id"] == turn["session_id"]
    assert wait_until(lambda: [i for i in boite(bridge) if i["text"] == "Un créneau ce soir à 18 h : réserve vite."])
    pushed = next(i for i in boite(bridge) if i["text"].startswith("Un créneau ce soir"))
    assert wait_until(lambda: [p for p in fakes.apns.payloads() if p.get("outbox_id") == pushed["id"]], timeout=20)

    # 6. NOTHING: the agent keeps it to itself
    fakes.hermes.run_output = "NOTHING"
    value.write_text("jeudi 11:00")
    quiet = check(bridge, conn)
    assert quiet["woke"] is True and quiet["sent"] is False
    assert len(boite(bridge)) == 2

    # the list: « Quand quelque chose change », how often, the last check and change, no AI cost
    shown = row(bridge, conn)
    assert (shown["kind"], shown["group"], shown["state"]) == ("watcher", "on_change", "active")
    assert shown["when"]["text"] == "Toutes les 15 minutes" and shown["cost"] == {"ai": False}
    assert shown["actions"] == ["pause", "run", "delete", "open"]
    assert shown["last_run"]["ok"] is True and shown["detail"]["last_change"] and shown["detail"]["wakes"] == 3
    assert shown["detail"]["summary"] == "Prochain créneau : jeudi 11:00" and shown["detail"]["broken"] is False
    assert shown["next_run"] > time.time()
    # the runtime is the bridge's, out of the program's reach and private
    runtime = bridge.state_dir / "connectors" / "vie" / conn["id"] / "watch.json"
    assert oct(runtime.stat().st_mode & 0o777) == "0o600"
    # paused: no « run now »; resumed: it runs
    bridge.patch(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH, json={"enabled": False})
    paused = bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/run", headers=AUTH)
    assert paused.status_code == 409 and paused.json()["detail"]["code"] == "not_running"
    assert row(bridge, conn)["main_action"] == "resume"
    # deleted: gone from the list
    assert bridge.delete(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH).status_code == 204
    items = bridge.get("/v1/autonomous", headers=AUTH, params={"agent": "vie"}).json()["items"]
    assert not [i for i in items if i["kind"] == "watcher"]


def test_wake_ups_are_rate_limited_and_a_change_that_waits_is_told_after(bridges, fakes):
    bridge, value, conn, _ = setup(bridges, fakes, watchers={"wake_gap_minutes": 30})
    assert check(bridge, conn)["step"] == "baseline"
    value.write_text("mercredi 09:00")
    assert check(bridge, conn)["step"] == "woke"
    value.write_text("jeudi 11:00")
    held = check(bridge, conn)
    assert held["step"] == "held" and not held["woke"]
    assert len(turns(fakes)) == 1
    assert row(bridge, conn)["detail"]["pending"] is True
    # « run now » too soon is refused when the gap is set
    bridge.config["watchers"]["run_now_gap_seconds"] = 600
    bridge.restart()
    soon = bridge.post(f"/v1/agents/vie/connectors/{conn['id']}/run", headers=AUTH)
    assert soon.status_code == 409 and soon.json()["detail"]["code"] == "too_soon"


def test_a_used_credit_holds_the_wake_up_until_it_is_back(bridges, fakes):
    bridge, value, conn, _ = setup(bridges, fakes)
    assert check(bridge, conn)["step"] == "baseline"
    credit = bridge.home / ".local" / "state" / "bipagents" / "credit.json"
    credit.parent.mkdir(parents=True, exist_ok=True)
    credit.write_text(json.dumps({"state": "full", "resumes": time.time() + 86400}))
    value.write_text("mercredi 09:00")
    held = check(bridge, conn)
    assert held["step"] == "held" and not held["woke"] and not turns(fakes)
    credit.write_text(json.dumps({"state": "ok"}))
    assert check(bridge, conn)["step"] == "woke"
    assert len(turns(fakes)) == 1


def test_failures_in_a_row_mark_it_broken_and_the_agent_is_told_once(bridges, fakes):
    bridge, value, conn, _ = setup(bridges, fakes)
    assert check(bridge, conn)["step"] == "baseline"
    fakes.hermes.run_output = "Le veilleur du Dr Martin ne marche plus : je regarde ce qui a changé sur le site."
    value.write_text("fail")
    one, two = check(bridge, conn), check(bridge, conn)
    assert (one["step"], two["step"]) == ("failed", "failed") and turns(fakes) == []
    assert "503" in one["message"]
    broken = check(bridge, conn)
    assert broken["step"] == "broken" and broken["woke"] is True
    [turn] = turns(fakes)
    assert "failed 3 times in a row" in turn["input"] and "connector_update" in turn["input"]
    shown = row(bridge, conn)
    assert shown["detail"]["broken"] is True and shown["last_run"]["ok"] is False and "503" in shown["last_run"]["error"]
    # told once: the next failure wakes nobody
    assert check(bridge, conn)["step"] == "failed" and len(turns(fakes)) == 1
    # working again: no longer broken, and the same state as before is no change
    value.write_text("mardi 10:00")
    back = check(bridge, conn)
    assert back["step"] == "same" and row(bridge, conn)["detail"]["broken"] is False
    detail = bridge.get(f"/v1/agents/vie/connectors/{conn['id']}", headers=AUTH).json()["watch"]
    assert detail["failures"] == 0 and [r["ok"] for r in detail["runs"]][:2] == [True, False]


def test_on_condition_wakes_when_it_holds(bridges, fakes):
    bridge, value, conn, card = setup(bridges, fakes, notify="on_condition")
    assert card["bip_connector_schedule"] == "Vérifie toutes les 15 min, ne réveille l'agent que si sa condition est remplie"
    assert check(bridge, conn)["step"] == "baseline"      # condition false
    value.write_text("low 80")
    assert check(bridge, conn)["step"] == "woke"
    assert check(bridge, conn)["step"] == "same"           # still holding, nothing new
    value.write_text("high 300")
    assert check(bridge, conn)["step"] == "same"
    value.write_text("low 60")
    assert check(bridge, conn)["step"] == "woke"
    assert len(turns(fakes)) == 2 and "its condition holds now" in turns(fakes)[1]["input"]


@pytest.mark.slow
def test_the_bridge_checks_on_its_own(bridges, fakes):
    bridge, value, conn, _ = setup(bridges, fakes, watchers={"tick_seconds": 0.5})
    # approved: its first check comes by itself, soon, without any model call
    assert wait_until(lambda: row(bridge, conn)["detail"]["last_check"], timeout=20)
    assert podman_runs(bridge) and turns(fakes) == []
    # not again before its 15 minutes
    count = len(podman_runs(bridge))
    time.sleep(2)
    assert len(podman_runs(bridge)) == count
