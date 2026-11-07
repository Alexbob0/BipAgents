"""Each agent's working space tidied by itself (bridge-rs/src/space_tidy.rs, Alex 2026-11-07: « il faut une règle pour
nos agents dans BipAgents »): a background task gets a drafts folder of its own in its sandbox's workspace, named in
its rules; once the task is over (`[tidy] task_hours` after its end, 0 here) the bridge removes that folder, and only
that: the task's result, the person's files, the agent's other files and its recent drafts stay."""
import json
import time

import pytest

from bbkit.common import AUTH
from bbkit.servers import wait_until
from test_jobs import TASK, view
from test_sandbox import isolated, started


@pytest.mark.slow
def test_a_tasks_drafts_are_gone_after_it_ends_and_the_persons_files_remain(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("wellness",))
    bridge.config["tidy"] = {"every_hours": 1 / 3600, "task_hours": 0}   # a pass each second, no grace after the end
    sandbox = hub / "profiles" / "wellness" / "sandboxes" / "docker" / "profile_wellness"
    workspace, home = sandbox / "workspace", sandbox / "home"
    (workspace / "brouillons").mkdir(parents=True)
    (home / ".cache" / "pip").mkdir(parents=True)
    (home / ".cache" / "pip" / "wheel.whl").write_bytes(b"PK wheel")   # a recent cache: kept (two weeks)
    started(bridge, hub, ["wellness"])
    fakes.hermes.rounds = 1
    started_job = bridge.post("/v1/agents/wellness/jobs", headers=AUTH, json=TASK)
    assert started_job.status_code == 200, started_job.text
    job = started_job.json()
    # the task's rules name its own drafts folder
    run = fakes.hermes.seen("/p/wellness/v1/runs", "POST")[0].json()
    assert f"go in /workspace/brouillons/{job['id']}: it is emptied a day after the task ends" in run["input"]

    # what the agent writes while it works: its drafts in the task's folder, its result given to the person
    drafts = workspace / "brouillons" / job["id"]
    (drafts / "pages").mkdir(parents=True)
    (drafts / "pages" / "ecole-1.html").write_text("<html>école 1</html>")
    (drafts / "calcul.py").write_text("print(3 * 4)")
    (workspace / "brouillons" / "notes.md").write_text("une note d'hier")    # a draft of its own, recent: kept
    (workspace / "rapport.xlsx").write_bytes(b"PK\x03\x04rapport")            # its result, outside the drafts
    (volumes[("envoi", "wellness")] / "comparatif.pdf").write_bytes(b"%PDF comparatif")
    uploads = hub / "profiles" / "wellness" / "uploads" / "2026-10" / "0123456789ab-facture.pdf"   # the person's
    weight = sum(f.stat().st_size for f in drafts.rglob("*") if f.is_file())
    # never while the task runs (passes each second; the bridge reads the run's end every 10 s)
    time.sleep(3)
    if view(bridge, job["id"])["state"] == "running":
        assert (drafts / "calcul.py").exists()

    assert wait_until(lambda: view(bridge, job["id"])["state"] == "done", timeout=25)
    assert wait_until(lambda: not drafts.exists(), timeout=10), bridge.log_tail(4000)
    assert wait_until(lambda: "space tidied" in bridge.log_tail(20000), timeout=5)

    # everything else stays
    assert (workspace / "brouillons").is_dir() and (workspace / "brouillons" / "notes.md").exists()
    assert (workspace / "rapport.xlsx").read_bytes() == b"PK\x03\x04rapport"
    assert (volumes[("envoi", "wellness")] / "comparatif.pdf").read_bytes() == b"%PDF comparatif"
    assert uploads.read_bytes() == b"%PDF facture"
    assert (home / ".cache" / "pip" / "wheel.whl").exists()
    record = json.loads((bridge.state_dir / "space-tidy.json").read_text())
    assert [(f["agent"], f["kinds"]) for f in record["history"]] == [("wellness", {"task": [1, weight]})]
    # a small pass: no line in the Boîte
    assert not [r for r in bridge.db().execute("SELECT ntfy_id FROM outbox") if str(r[0]).startswith("space-tidy:")]


@pytest.mark.slow
def test_a_drafts_folder_swapped_for_a_link_leads_nowhere(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("wellness",))
    bridge.config["tidy"] = {"every_hours": 1 / 3600, "task_hours": 0}
    workspace = hub / "profiles" / "wellness" / "sandboxes" / "docker" / "profile_wellness" / "workspace"
    theirs = bridge.root / "photos"
    (theirs / "t0123456789ab").mkdir(parents=True)
    (theirs / "t0123456789ab" / "vacances.jpg").write_bytes(b"jpg")   # named like a forgotten task's folder
    workspace.mkdir(parents=True)
    (workspace / "brouillons").symlink_to(theirs)
    started(bridge, hub, ["wellness"])
    time.sleep(3)   # a few passes
    assert (theirs / "t0123456789ab" / "vacances.jpg").read_bytes() == b"jpg"
    assert not (bridge.state_dir / "space-tidy.json").exists()
