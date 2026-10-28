"""« Partager comme modèle » (bridge-rs/src/templates.rs, internal/next/agent-templates.md): an agent's model is made
from an allow-list — canaries planted in every place that must never travel (memory, the Carnet, conversations,
files, pages, the vault, the connector's secret and private state, the creator's identifier, the routine's delivery
target) are nowhere in it —, the person's review choices take parts out, the scrubber flags what looks personal, and
an import makes a new agent whose connector waits for THIS person's approval and whose routines stay off."""
import json

import yaml

from bbkit.common import AUTH
from test_connectors import EMAIL, PASSWORD, REGISTER, approve_next, ask, setup

CANARY = "CANARY-7f3a9e"
SKILL = "---\nname: plan-semaine\ndescription: Prépare la semaine\n---\n# Étapes\nÉcris à camille.durand@laposte.net\n"


def plant(bridge, hub, conn_id):
    """A canary in each excluded place of vie's."""
    folder = hub / "profiles" / "vie"
    (folder / "SOUL.md").write_text("Tu es Vie, une coach de course. Prépare une séance chaque matin.\n")
    (folder / "memories").mkdir(parents=True, exist_ok=True)
    (folder / "memories" / "MEMORY.md").write_text(f"Alex aime {CANARY}-memory\n")
    (folder / "memories" / "USER.md").write_text(f"{CANARY}-user\n")
    (folder / "knowledge").mkdir(parents=True, exist_ok=True)
    (folder / "knowledge" / "carnet.md").write_text(f"{CANARY}-carnet\n")
    (folder / "sessions").mkdir(parents=True, exist_ok=True)
    (folder / "sessions" / "s1.json").write_text(json.dumps({"messages": [f"{CANARY}-conversation"]}))
    (folder / "uploads" / "2026-10" / "note.txt").write_text(f"{CANARY}-file\n")
    (bridge.state_dir / "pages").mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "pages" / "p.html").write_text(f"{CANARY}-page")
    state = bridge.state_dir / "connectors" / "vie" / conn_id / "state"
    state.mkdir(parents=True, exist_ok=True)
    (state / "token.json").write_text(f"{CANARY}-state")
    # its own skill (travels) and a routine whose delivery and origin are the creator's (they don't)
    skill = folder / "skills" / "plan-semaine"
    skill.mkdir(parents=True, exist_ok=True)
    (skill / "SKILL.md").write_text(SKILL)
    (skill / ".secret-cache").write_text(f"{CANARY}-hidden")
    cron = folder / "cron"
    cron.mkdir(parents=True, exist_ok=True)
    (cron / "jobs.json").write_text(json.dumps({"jobs": [
        {"id": "aaaaaaaaaaaa", "name": "Séance du jour", "prompt": "Prépare la séance du jour.",
         "schedule": {"kind": "cron", "expr": "0 7 * * *", "display": "0 7 * * *"}, "schedule_display": "0 7 * * *",
         "enabled": True, "state": "scheduled", "deliver": f"telegram:{CANARY}", "origin": {"chat_id": CANARY},
         "last_output": f"{CANARY}-output", "skills": ["plan-semaine"]},
        {"id": "bbbbbbbbbbbb", "name": "Script", "prompt": "x", "script": f"/root/{CANARY}.py",
         "schedule": {"kind": "cron", "expr": "0 8 * * *"}}]}))


def test_a_model_carries_no_canary_and_lands_unapproved(bridges, fakes):
    bridge, hub, work = setup(bridges, fakes)
    (hub / "config.yaml").write_text(yaml.safe_dump({"terminal": {"backend": "docker"}}))   # new profiles isolated
    ask(bridge, {"name": "garmin", "site": "connect.garmin.example", "kind": "password",
                 "identifier_label": "E-mail de ton compte Garmin", "reason": "r"}, {"value": PASSWORD, "identifier": EMAIL})
    approve_next(bridge, "connector_register", REGISTER)
    conn = bridge.get("/v1/agents/vie/connectors", headers=AUTH).json()["items"][0]
    plant(bridge, hub, conn["id"])
    with (work / "server.py").open("a") as f:
        f.write(f"\n# {CANARY}-workspace-only\n")   # changed after approval: only the approved copy travels

    # the review: every part, the « never » list, the flags
    resp = bridge.post("/v1/agents/vie/template", headers=AUTH, json={"intro": "Coach de course"})
    assert resp.status_code == 200, resp.text
    review = resp.json()
    bundle = review["template"]
    text = json.dumps(bundle, ensure_ascii=False)
    for forbidden in (CANARY, PASSWORD, EMAIL, conn["id"]):
        assert forbidden not in text, forbidden
    ids = [p["id"] for p in review["parts"]]
    assert ids[0] == "look" and "instructions" in ids and "skill:plan-semaine" in ids
    assert "routine:aaaaaaaaaaaa" in ids and "routine:bbbbbbbbbbbb" not in ids   # a script never travels
    assert f"connector:{conn['id']}" in ids
    assert len(review["never"]) == 6
    assert bundle["format"] == "bipagents-agent-template" and bundle["version"] == 1
    assert bundle["routines"] == [{"name": "Séance du jour", "prompt": "Prépare la séance du jour.", "skills": ["plan-semaine"],
                                   "schedule": {"kind": "cron", "expr": "0 7 * * *", "display": "0 7 * * *"}}]
    shipped = bundle["connectors"][0]
    assert shipped["sites"] == REGISTER["sites"] and shipped["unofficial"] is True
    assert shipped["secrets"] == [{"name": "garmin", "kind": "password", "identifier_label": "E-mail de ton compte Garmin"}]
    assert [f["path"] for f in shipped["files"]] == ["README.md", "server.py"]
    assert not any(k in shipped for k in ("id", "fingerprint", "proxy_token", "status", "last_run", "folder"))
    assert [f["path"] for f in bundle["skills"][0]["files"]] == ["SKILL.md"]
    assert bundle["tools"]["toolsets"] == {"web": True}
    flags = review["flags"]
    assert any(f["part"] == "skill:plan-semaine" and f["kind"] == "email" and f["line"] == 6 for f in flags), flags
    assert all("durand" not in f["excerpt"] for f in flags)

    # parts taken out on the person's choice
    resp = bridge.post("/v1/agents/vie/template", headers=AUTH,
                       json={"leave_out": ["skill:plan-semaine", "routine:aaaaaaaaaaaa", "instructions"]})
    out = resp.json()
    assert out["template"]["skills"] == [] and out["template"]["routines"] == [] and "instructions" not in out["template"]
    assert not any(f["part"] == "skill:plan-semaine" for f in out["flags"])
    assert {p["id"]: p["included"] for p in out["parts"]}["skill:plan-semaine"] is False

    for kind in ("files", "envoi", "share"):   # the new agent's sandbox volumes, as isolated() makes vie's
        volume = bridge.root / "volumes" / f"{kind}-coach"
        volume.mkdir(parents=True)
        bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}", f"bipagents-{kind}-coach"],
                             stdout=f"{volume}\n")
    # import: a new agent, its connector pending with a card, its routine off, its skill marked imported
    resp = bridge.post("/v1/templates/import", headers=AUTH, json={"template": bundle, "name": "Coach"})
    assert resp.status_code == 201, resp.text
    made = resp.json()
    agent = made["agent"]["id"]
    assert agent != "vie" and made["skills"] == ["plan-semaine"]
    assert made["routines"][0]["on"] is False and made["routines"][0]["name"] == "Séance du jour"
    assert len(made["connectors"]) == 1, made
    new_conn = made["connectors"][0]
    listed = bridge.get(f"/v1/agents/{agent}/connectors", headers=AUTH).json()["items"][0]
    assert listed["status"] == "pending" and listed["runs"] is False and listed["from_template"] is True
    assert listed["secrets"][0]["saved"] is False   # the creator's password stayed home
    card = next(c for c in bridge.get("/v1/approvals", headers=AUTH, params={"agent": agent}).json()["items"]
                if c["run_id"] == new_conn["run_id"])
    assert card["bip_connector_change"] == "template" and "vient d'un modèle" in card["description"]
    # its secret card: THIS person's own password, with the model's label
    opened = bridge.post(f"/v1/agents/{agent}/connectors/{new_conn['id']}/secrets/garmin/edit", headers=AUTH)
    assert opened.status_code == 200, opened.text
    assert opened.json()["card"]["bip_secret_identifier"] == ""
    assert opened.json()["card"]["bip_secret_identifier_label"] == "E-mail de ton compte Garmin"
    jobs = json.loads((hub / "profiles" / agent / "cron" / "jobs.json").read_text())["jobs"]
    assert jobs[0]["enabled"] is False and jobs[0]["state"] == "paused" and jobs[0]["deliver"] == "local"
    settings = bridge.get(f"/v1/agents/{agent}/settings", headers=AUTH).json()
    assert {s["name"]: s.get("imported") for s in settings["skills"]}["plan-semaine"] is True
    everything = json.dumps(made) + (hub / "profiles" / agent / "cron" / "jobs.json").read_text()
    assert CANARY not in everything and PASSWORD not in everything and EMAIL not in everything

    # the routine turned on by the person, then off again
    job = made["routines"][0]["job"]
    on = bridge.put(f"/v1/agents/{agent}/template-routines/{job}", headers=AUTH, json={"on": True})
    assert on.status_code == 200, on.text
    jobs = json.loads((hub / "profiles" / agent / "cron" / "jobs.json").read_text())["jobs"]
    assert jobs[0]["enabled"] is True and jobs[0]["state"] == "scheduled" and "next_run_at" not in jobs[0]
    assert bridge.get(f"/v1/agents/{agent}/template-routines", headers=AUTH).json()["routines"][0]["on"] is True
    assert bridge.put(f"/v1/agents/{agent}/template-routines/aaaaaaaaaaaa", headers=AUTH,
                      json={"on": True}).status_code == 404   # not a routine a model brought here


def test_a_bad_bundle_is_refused_and_the_health_check_says_it_can(bridges, fakes):
    bridge, hub, work = setup(bridges, fakes)
    assert "agent_templates" in bridge.get("/health").json()["features"]
    good = {"format": "bipagents-agent-template", "version": 1, "name": "X", "look": {"category": "daily"}}
    for bad in ({**good, "version": 9},
                {**good, "connectors": [{"name": "Evil", "description": "d", "sites": ["a.example"], "entry": "python3 s.py",
                                         "files": [{"path": "../../etc/x", "text": "x"}]}]},
                {**good, "skills": [{"name": "../up", "files": [{"path": "SKILL.md", "text": "x"}]}]}):
        resp = bridge.post("/v1/templates/import", headers=AUTH, json={"template": bad})
        assert resp.status_code == 400, resp.text
    assert bridge.post("/v1/agents/nobody/template", headers=AUTH, json={}).status_code == 404
