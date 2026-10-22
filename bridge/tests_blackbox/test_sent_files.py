"""Files an agent hands the person with a `MEDIA:` line from its own places (bridge-rs/src/sent_files.rs): an isolated
sandbox's /envoi, its persistent workspace; served by `GET /v1/media?path=…&agent=…` from the bridge's copy, never
from another agent's places (owner's report 2026-10-22: « Fichier indisponible » for Wellness' infographic)."""
import io
import json
import os
import tarfile

import yaml

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


def add_mounts(hub, name, mounts):
    """Folders mounted by hand into `name`'s sandbox (its profile's terminal.docker_volumes)."""
    path = hub / "profiles" / name / "config.yaml"
    data = yaml.safe_load(path.read_text())
    data["terminal"]["docker_volumes"] += mounts
    path.write_text(yaml.safe_dump(data))


def test_the_profile_and_the_hand_mounted_folders_by_either_path_but_never_a_secret(bridges, fakes):
    """Owner's report 2026-10-22: Wellness' scripts write its infographics in its own profile
    (`…/.hermes/profiles/wellness/data/progress_photos/…png`), mounted by hand into its sandbox."""
    bridge, hub, volumes = isolated(bridges, fakes)
    wellness = hub / "profiles" / "wellness"
    photos = wellness / "data" / "progress_photos"
    photos.mkdir(parents=True)
    (photos / "runs_infographic_23.png").write_bytes(b"\x89PNG runs")
    (wellness / "output").mkdir()
    (wellness / "output" / "plan.pdf").write_bytes(b"%PDF plan")
    (wellness / "data" / "auth.json").write_text('{"token": "secret"}')
    (wellness / "auth.json").write_text('{"token": "secret"}')
    (wellness / "data" / "tokens.json").write_text("{}")
    (wellness / "data" / ".cache").mkdir()
    (wellness / "data" / ".cache" / "x.png").write_bytes(b"\x89PNG hidden")
    (wellness / "data" / "cle.pem").write_text("-----BEGIN")
    add_mounts(hub, "wellness", [f"{wellness}/data:/home/hermes/.hermes/profiles/wellness/data:z"])
    add_mounts(hub, "vie", [f"{wellness}/data:/lecture/data:z"])    # never: another agent's profile
    # The same as podman volumes named for the agent; Vie naming Wellness' volume gets nothing.
    plans = bridge.root / "volumes" / "wellness-plans"
    plans.mkdir(parents=True)
    (plans / "semaine.png").write_bytes(b"\x89PNG plan")
    bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}", "bipagents-wellness-plans"],
                         stdout=f"{plans}\n")
    add_mounts(hub, "wellness", ["bipagents-wellness-plans:/home/hermes/.hermes/profiles/wellness/plans:z"])
    add_mounts(hub, "vie", ["bipagents-wellness-plans:/plans:z"])
    started(bridge, hub, ["vie", "wellness"])
    assert media(bridge, "/home/hermes/.hermes/profiles/wellness/plans/semaine.png", "wellness").content == b"\x89PNG plan"
    assert media(bridge, "/plans/semaine.png", "vie").status_code == 404
    on_host = f"{wellness}/data/progress_photos/runs_infographic_23.png"
    got = media(bridge, on_host, "wellness")
    assert got.status_code == 200 and got.content == b"\x89PNG runs"
    assert got.headers["content-type"] == "image/png" and got.headers["x-content-type-options"] == "nosniff"
    seen = "/home/hermes/.hermes/profiles/wellness/data/progress_photos/runs_infographic_23.png"
    assert media(bridge, seen, "wellness").content == b"\x89PNG runs"
    assert media(bridge, f"{wellness}/output/plan.pdf", "wellness").content == b"%PDF plan"
    for path in (on_host, "/lecture/data/progress_photos/runs_infographic_23.png"):
        assert media(bridge, path, "vie").status_code == 404, path
    for path in (f"{wellness}/data/auth.json", f"{wellness}/auth.json", f"{wellness}/data/tokens.json",
                 f"{wellness}/data/.cache/x.png", f"{wellness}/data/cle.pem",
                 "/home/hermes/.hermes/profiles/wellness/data/auth.json", f"{wellness}/output/../auth.json"):
        assert media(bridge, path, "wellness").status_code == 404, path
    assert wait_until(lambda: "secret-looking" in bridge.log_tail(8000), timeout=5)
    assert wait_until(lambda: '"logger": "bipbridge.media"' in bridge.log_tail(8000), timeout=5)
    # An app naming an agent the bridge doesn't know: logged too.
    assert media(bridge, on_host, "Wellnesss").status_code == 404
    assert wait_until(lambda: "unknown agent" in bridge.log_tail(8000), timeout=5)


def tar_of(folder, name, data=None, link=None):
    """What `podman cp <container>:<path> -` prints: a tar stream of one entry."""
    out = folder / f"{name}.tar"
    with tarfile.open(out, "w") as tar:
        info = tarfile.TarInfo(name)
        if link:
            info.type, info.linkname = tarfile.SYMTYPE, link
            tar.addfile(info)
        else:
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
    return str(out)


def sandbox_container(bridge, hub, cid, profile, mounted_from):
    """A Hermes sandbox container, as `podman container inspect` shows it: labelled for `profile`, its /root the
    persistent folder of `mounted_from`'s profile."""
    home = hub / "profiles" / mounted_from / "sandboxes" / "docker" / f"profile_{mounted_from}" / "home"
    home.mkdir(parents=True, exist_ok=True)
    inspected = [{"Id": cid, "State": {"Running": True},
                  "Config": {"Labels": {"hermes-agent": "1", "hermes-profile": profile, "hermes-task-id": "x"}},
                  "Mounts": [{"Type": "bind", "Source": str(home), "Destination": "/root"}]}]
    bridge.commands.rule("podman", ["container", "inspect", cid], stdout=json.dumps(inspected))


def test_a_file_left_in_the_sandboxs_tmp_comes_out_of_its_own_container_only(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes)
    tars = bridge.root / "tars"
    tars.mkdir()
    listed = ["ps", "-a", "--filter", "label=hermes-agent=1", "--filter"]
    # Wellness' sandbox, and a container labelled for Wellness that holds Vie's folders (never used).
    bridge.commands.rule("podman", listed + ["label=hermes-profile=wellness"],
                         stdout=json.dumps([{"Id": "cccc3333", "State": "running"},
                                            {"Id": "aaaa1111", "State": "running"}]))
    bridge.commands.rule("podman", listed + ["label=hermes-profile=vie"],
                         stdout=json.dumps([{"Id": "bbbb2222", "State": "running"}]))
    sandbox_container(bridge, hub, "aaaa1111", "wellness", "wellness")
    sandbox_container(bridge, hub, "bbbb2222", "vie", "vie")
    sandbox_container(bridge, hub, "cccc3333", "wellness", "vie")
    png = b"\x89PNG\r\n\x1a\n\x00infographie\xff"
    bridge.commands.rule("podman", ["cp", "aaaa1111:/tmp/infographie.png", "-"],
                         stdout_file=tar_of(tars, "infographie.png", png))
    bridge.commands.rule("podman", ["cp", "cccc3333:/tmp/infographie.png", "-"],
                         stdout_file=tar_of(tars, "autre.png", b"\x89PNG not wellness'"))
    bridge.commands.rule("podman", ["cp", "aaaa1111:/tmp/lien.png", "-"],
                         stdout_file=tar_of(tars, "lien.png", link="/etc/passwd"))
    bridge.commands.rule("podman", ["cp", "aaaa1111:/root/graph.svg", "-"],
                         stdout_file=tar_of(tars, "graph.svg", b"<svg><script>x()</script></svg>"))
    started(bridge, hub, ["vie", "wellness"])
    bridge.commands.clear()
    got = media(bridge, "/tmp/infographie.png", "wellness")
    assert got.status_code == 200 and got.content == png and got.headers["content-type"] == "image/png"
    copies = [a for a in bridge.commands.of("podman") if a[:1] == ["cp"]]
    assert copies == [["cp", "aaaa1111:/tmp/infographie.png", "-"]]
    assert wait_until(lambda: "file copied out of the agent's sandbox" in bridge.log_tail(8000), timeout=5)
    # Vie asking for that path: only Vie's container is asked, which has no such file.
    bridge.commands.clear()
    assert media(bridge, "/tmp/infographie.png", "vie").status_code == 404
    copies = [a for a in bridge.commands.of("podman") if a[:1] == ["cp"]]
    assert copies and all(a[1].startswith("bbbb2222:") for a in copies), copies
    # A link in the container is never followed; a relative path is in its working folder (an SVG locked down).
    assert media(bridge, "/tmp/lien.png", "wellness").status_code == 404
    assert wait_until(lambda: "a link (never followed)" in bridge.log_tail(8000), timeout=5)
    svg = media(bridge, "graph.svg", "wellness")
    assert svg.status_code == 200 and svg.headers["content-type"].startswith("image/svg+xml")
    assert svg.headers["content-security-policy"] == "sandbox; default-src 'none'"
    assert not any(a[1].startswith("cccc3333:") for a in bridge.commands.of("podman") if a[:1] == ["cp"])
    # At the end of a run naming it, before the app asks (the sandbox may be gone by then).
    bridge.commands.rule("podman", ["cp", "aaaa1111:/tmp/semaine.pdf", "-"], times=1,
                         stdout_file=tar_of(tars, "semaine.pdf", b"%PDF semaine"))
    fakes.hermes.runs["run_p"] = [sse("run.completed", output="Voilà !\nMEDIA:/tmp/semaine.pdf")]
    bridge.post("/v1/watch", headers=AUTH, json={"agent": "wellness", "run_id": "run_p"})
    assert wait_until(lambda: ["cp", "aaaa1111:/tmp/semaine.pdf", "-"] in bridge.commands.of("podman"), timeout=10)
    assert wait_until(lambda: media(bridge, "/tmp/semaine.pdf", "wellness").content == b"%PDF semaine", timeout=10)
