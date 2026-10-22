"""Files an agent hands the person with a `MEDIA:` line from its own places (bridge-rs/src/sent_files.rs): an isolated
sandbox's /envoi, its persistent workspace; served by `GET /v1/media?path=…&agent=…` from the bridge's copy, never
from another agent's places (owner's report 2026-10-22: « Fichier indisponible » for Wellness' infographic)."""
import os

from bbkit.common import AUTH, sse
from bbkit.servers import wait_until
from test_sandbox import isolated, started

XLSX = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"


def media(bridge, path, agent=None):
    params = {"path": path} | ({"agent": agent} if agent else {})
    return bridge.get("/v1/media", params=params, headers=AUTH)


def test_an_isolated_agents_files_reach_the_person_and_no_other_agents(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes)
    started(bridge, hub, ["vie", "wellness"])
    envoi = volumes[("envoi", "wellness")]
    (envoi / "courses.xlsx").write_bytes(b"PK\x03\x04courses")
    got = media(bridge, "/envoi/courses.xlsx", "wellness")
    assert got.status_code == 200 and got.content == b"PK\x03\x04courses"
    assert got.headers["content-type"] == XLSX and got.headers["x-content-type-options"] == "nosniff"
    assert got.headers["content-disposition"] == 'attachment; filename="courses.xlsx"'
    assert "sandbox" in got.headers["content-security-policy"]
    # Another agent's message naming that path gets nothing of Wellness'.
    assert media(bridge, "/envoi/courses.xlsx", "vie").status_code == 404
    assert media(bridge, "/envoi/courses.xlsx", "nobody").status_code == 404
    # An app that names no agent: the copy, as only one agent has that path.
    assert media(bridge, "/envoi/courses.xlsx").content == b"PK\x03\x04courses"
    # The sandbox reset: the bridge's copy stays.
    (envoi / "courses.xlsx").unlink()
    assert media(bridge, "/envoi/courses.xlsx", "wellness").content == b"PK\x03\x04courses"
    # Never a file of this machine: up the tree, through a link, a link on the way, a pipe, another type.
    (bridge.root / "secret.txt").write_text("secret")
    os.symlink(bridge.root / "secret.txt", envoi / "lien.txt")
    os.symlink(bridge.root, envoi / "dossier")
    os.mkfifo(envoi / "tube.txt")
    (envoi / "script.sh").write_text("x")
    for path in ("/envoi/../secret.txt", "/envoi/lien.txt", "/envoi/dossier/secret.txt", "/envoi/tube.txt",
                 "/envoi/script.sh", "/etc/hosts", "/tmp/x.png"):
        missing = media(bridge, path, "wellness")
        assert missing.status_code == 404 and missing.json() == {"detail": "not found"}, path
    assert wait_until(lambda: "media file asked for and not found" in bridge.log_tail(4000), timeout=5)


def test_an_html_file_from_a_sandbox_is_served_locked_like_a_page(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("wellness",))
    started(bridge, hub, ["wellness"])
    (volumes[("envoi", "wellness")] / "infographie.html").write_text("<h1>Courses</h1><script>fetch('x')</script>")
    got = media(bridge, "/envoi/infographie.html", "wellness")
    assert got.status_code == 200 and got.headers["content-type"].startswith("text/html")
    policy = got.headers["content-security-policy"]
    assert policy.startswith("sandbox allow-scripts;") and "connect-src 'none'" in policy
    assert got.headers["cache-control"] == "no-store"


def test_the_workspace_and_a_finished_runs_files(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("wellness",))
    workspace = hub / "profiles" / "wellness" / "sandboxes" / "docker" / "profile_wellness" / "workspace"
    (workspace / "out").mkdir(parents=True)
    (workspace / "out" / "graph.png").write_bytes(b"\x89PNG graph")
    started(bridge, hub, ["wellness"])
    assert media(bridge, "/workspace/out/graph.png", "wellness").content == b"\x89PNG graph"
    # A reply naming a file: kept when the run ends, before the app asks (the sandbox may be reset meanwhile).
    (volumes[("envoi", "wellness")] / "semaine.csv").write_text("jour;total\nlundi;12\n")
    fakes.hermes.runs["run_m"] = [sse("run.completed", output="Voilà !\nMEDIA:/envoi/semaine.csv")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_m"})
    assert wait_until(lambda: "file the agent sent kept" in bridge.log_tail(4000), timeout=10)
    (volumes[("envoi", "wellness")] / "semaine.csv").unlink()
    got = media(bridge, "/envoi/semaine.csv", "wellness")
    assert got.status_code == 200 and got.text == "jour;total\nlundi;12\n"
    assert got.headers["content-type"] == "text/csv; charset=utf-8"
