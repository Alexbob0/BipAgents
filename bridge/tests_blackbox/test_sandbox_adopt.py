"""`bipbridge sandbox adopt` (bridge-rs/src/sandbox_adopt.rs), 2026-10-22: an agent set up by hand still mounts
folders of this machine in its sandbox; each one becomes a named volume at the same path (its code refreshed at each
start, its working folders copied once), Hermes' media folder and unused credentials go. podman is the fake one: its
volumes are folders of the test."""
import json
import time

import yaml

from bbkit.servers import wait_until
from conftest import managed_install, profile


def config_yaml(folder):
    return yaml.safe_load((folder / "config.yaml").read_text()) or {}


def volumes_of(folder):
    return config_yaml(folder).get("terminal", {}).get("docker_volumes", [])


def setup(bridges, fakes, agent="wellness"):
    """A sandboxed agent (`[sandbox] isolate = "on"`) with the hand-made mounts the owner's agents have on aibox."""
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    bridge.config["sandbox"] = {"isolate": "on"}
    hub = managed_install(bridge, fakes, agents=(agent,), isolated=True)
    folder = profile(hub, agent)
    (folder / "uploads").mkdir()
    bridge.config["agents"][agent]["upload_dir_host"] = str(folder / "uploads")
    bridge.config["agents"][agent]["upload_dir_container"] = "/fichiers"
    alex = bridge.root / "alex"
    for name, files in {"skills": {"garmin/SKILL.md": "# Garmin\nrun bin/sync"}, "bin": {"sync": "#!/bin/sh\necho"},
                        "scripts": {"report.py": "print('ok')", "oura.py": "open('/home/hermes/.oura-mcp/token')"},
                        "data": {"2026/sleep.json": "{}", "steps.csv": "a,b"}}.items():
        for relative, text in files.items():
            path = folder / name / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
    (folder / "bin" / "sync").chmod(0o755)
    for name in (".hermes/media", ".garmin-mcp", ".oura-mcp"):
        (alex / name).mkdir(parents=True, exist_ok=True)
    (alex / ".garmin-mcp" / "tokens.json").write_text("{}")
    mounts = [
        f"{folder}/skills:/home/hermes/.hermes/profiles/{agent}/skills:ro,z",
        f"{folder}/bin:/home/hermes/.hermes/profiles/{agent}/bin:ro,z",
        f"{folder}/scripts:/home/hermes/.hermes/profiles/{agent}/scripts:z",
        f"{alex}/.hermes/media:/home/hermes/.hermes/media:z",
        f"{folder}/data:/home/hermes/.hermes/profiles/{agent}/data:z",
        f"{alex}/.garmin-mcp:/home/hermes/.garmin-mcp:z",
        f"{alex}/.oura-mcp:/home/hermes/.oura-mcp:z",
        "bipagents-files-" + agent + ":/fichiers:ro",
        "bipagents-envoi-" + agent + ":/envoi",
    ]
    servers = {"garmin": {"command": "uvx", "args": ["garmin-mcp"], "env": {"GARMIN_TOKENS": f"{alex}/.garmin-mcp"}}}
    (folder / "config.yaml").write_text(yaml.safe_dump({"terminal": {"backend": "docker", "docker_volumes": mounts},
                                                        "mcp_servers": servers}))
    places = {}
    for name in ("skills", "bin", "scripts", "data", "files", "envoi", "oura-mcp", "garmin-mcp"):
        volume = f"bipagents-{agent}-{name}" if name not in ("files", "envoi") else f"bipagents-{name}-{agent}"
        place = bridge.root / "volumes" / volume
        place.mkdir(parents=True)
        places[name] = place
        bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}", volume], stdout=f"{place}\n")
    bridge.commands.rule("podman", ["ps", "-aq"], stdout="0ld5andb0x\n")
    bridge.commands.rule("podman", ["network", "exists"], exit=1)
    bridge.write_config()
    return bridge, hub, folder, alex, mounts, places


def test_adopt_plans_then_moves_each_mount_and_undoes(bridges, fakes):
    bridge, hub, folder, alex, mounts, places = setup(bridges, fakes)
    agent_path = "/home/hermes/.hermes/profiles/wellness"
    # The plan first: nothing done.
    plan = bridge.cli("sandbox", "adopt", "--agent", "wellness", "--dry-run")
    assert plan.returncode == 0, plan.stderr
    out = plan.stdout
    assert "wellness: 7 folder(s) of this machine in its sandbox" in out
    assert f"copy  {folder}/skills -> volume bipagents-wellness-skills at {agent_path}/skills (read-only" in out
    assert f"move  {folder}/data -> volume bipagents-wellness-data at {agent_path}/data (copied once, 2 files" in out
    assert f"drop  {alex}/.hermes/media (Hermes' media folder" in out
    assert f"drop  {alex}/.garmin-mcp (a connector's credentials, used by the MCP server « garmin »" in out
    # The credentials its own scripts use (kept before, the agent not isolated): linked, and the sites they need.
    assert (f"link  {alex}/.oura-mcp -> volume bipagents-wellness-oura-mcp at /home/hermes/.oura-mcp (programs outside "
            "the sandbox keep writing it: a connector's credentials that the agent's own files use (scripts/oura.py)"
            in out)
    assert f"the folder renamed {alex}/.oura-mcp.before-adopt as a backup" in out
    assert "it will need these sites allowed: api.ouraring.com, cloud.ouraring.com (--allow-sites" in out
    assert "then: isolated, no folder of this machine in its sandbox" in out
    assert "dry run: nothing changed" in out
    assert "token" not in out.replace(".oura-mcp/token", "")      # never a file's content
    assert volumes_of(folder) == mounts
    assert not any(a[:2] == ["volume", "create"] for a in bridge.commands.of("podman"))
    assert not any("config" in a for a in bridge.commands.of("hermes"))

    # The real run.
    done = bridge.cli("sandbox", "adopt", "--agent", "wellness")
    assert done.returncode == 0, done.stderr
    assert volumes_of(folder) == [
        f"bipagents-wellness-skills:{agent_path}/skills:ro",
        f"bipagents-wellness-bin:{agent_path}/bin:ro",
        f"bipagents-wellness-scripts:{agent_path}/scripts",
        f"bipagents-wellness-data:{agent_path}/data",
        "bipagents-wellness-oura-mcp:/home/hermes/.oura-mcp",
        "bipagents-files-wellness:/fichiers:ro",
        "bipagents-envoi-wellness:/envoi",
    ]
    # The credentials' folder: in its volume, a link at its path (the connector keeps writing there).
    assert (alex / ".oura-mcp").is_symlink() and (alex / ".oura-mcp").resolve() == places["oura-mcp"].resolve()
    assert (alex / ".oura-mcp.before-adopt").is_dir()
    (alex / ".oura-mcp" / "token.json").write_text("refreshed")
    assert (places["oura-mcp"] / "token.json").read_text() == "refreshed"
    assert "sites its programs will need, not allowed: api.ouraring.com, cloud.ouraring.com" in done.stdout
    assert (places["data"] / "2026" / "sleep.json").read_text() == "{}"
    assert (places["skills"] / "garmin" / "SKILL.md").exists()
    assert (places["bin"] / "sync").stat().st_mode & 0o111
    assert "copied and checked: 2 files" in done.stdout
    assert (folder / "data" / "steps.csv").read_text() == "a,b"            # the folder stays, as a backup
    assert f"the folders copied stay as they were, untouched" in done.stdout
    assert "wellness: isolated now" in done.stdout
    podman = bridge.commands.of("podman")
    assert ["volume", "create", "--ignore", "bipagents-wellness-data"] in podman
    assert ["rm", "-f", "0ld5andb0x"] in podman
    hermes = bridge.commands.entries("hermes")
    assert any(e["argv"] == ["gateway", "restart"] and e["hermes_home"] == str(hub) for e in hermes)
    state = json.loads((bridge.state_dir / "sandbox.json").read_text())["adopted"]["wellness"]
    assert state["before"] == mounts
    assert {v["name"]: v["kind"] for v in state["volumes"]} == {
        "bipagents-wellness-skills": "code", "bipagents-wellness-bin": "code",
        "bipagents-wellness-scripts": "work", "bipagents-wellness-data": "work",
        "bipagents-wellness-oura-mcp": "link"}

    # Again: nothing more to move.
    again = bridge.cli("sandbox", "adopt", "--agent", "wellness")
    assert again.returncode == 0 and "no folder of this machine in its sandbox, nothing to do" in again.stdout

    # Undone: the mounts as they were, the volumes kept.
    undone = bridge.cli("sandbox", "adopt", "--agent", "wellness", "--undo")
    assert undone.returncode == 0, undone.stderr
    assert volumes_of(folder) == mounts
    assert "volume bipagents-wellness-data kept" in undone.stdout
    assert "wellness" not in json.loads((bridge.state_dir / "sandbox.json").read_text()).get("adopted", {})
    assert (places["data"] / "steps.csv").exists()
    # The linked folder is a folder again, with what was written in the volume since.
    assert not (alex / ".oura-mcp").is_symlink() and (alex / ".oura-mcp" / "token.json").read_text() == "refreshed"
    assert not (alex / ".oura-mcp.before-adopt").exists()
    assert f"{alex}/.oura-mcp is a folder again" in undone.stdout


def test_the_code_volumes_follow_their_folders_at_each_start(bridges, fakes):
    bridge, hub, folder, alex, mounts, places = setup(bridges, fakes)
    (alex / ".oura-mcp").rename(alex / "oura-old")              # no script names it any more: dropped
    (folder / "scripts" / "oura.py").unlink()
    done = bridge.cli("sandbox", "adopt", "--agent", "wellness")
    assert done.returncode == 0, done.stderr
    assert "wellness: isolated now" in done.stdout
    assert not [v for v in volumes_of(folder) if v.startswith("/")]
    # The person adds a skill on the host: the volume has it after the bridge's next start.
    (folder / "skills" / "oura").mkdir()
    (folder / "skills" / "oura" / "SKILL.md").write_text("# Oura")
    (folder / "skills" / "garmin" / "SKILL.md").unlink()
    bridge.start()
    assert wait_until(lambda: (places["skills"] / "oura" / "SKILL.md").exists(), timeout=15)
    assert wait_until(lambda: not (places["skills"] / "garmin" / "SKILL.md").exists(), timeout=5)
    assert wait_until(lambda: "adopted code refreshed" in bridge.log_tail(4000), timeout=10)
    time.sleep(0.5)
    # The bridge's own start kept the adopted volumes.
    assert f"bipagents-wellness-skills:/home/hermes/.hermes/profiles/wellness/skills:ro" in volumes_of(folder)


def test_an_unrecognised_mount_is_kept_and_a_relative_one_too(bridges, fakes):
    bridge, hub, folder, alex, mounts, places = setup(bridges, fakes)
    data = config_yaml(folder)
    (bridge.root / "photos").mkdir()
    data["terminal"]["docker_volumes"] = [f"{bridge.root}/photos:/photos:ro", "./here:/here"]
    (folder / "config.yaml").write_text(yaml.safe_dump(data))
    plan = bridge.cli("sandbox", "adopt", "--agent", "wellness", "--dry-run")
    assert f"keep  {bridge.root}/photos (a folder outside the agent's own: not recognised)" in plan.stdout
    assert "keep  ./here (a relative path: not recognised)" in plan.stdout
    assert bridge.cli("sandbox", "adopt", "--agent", "nobody").returncode == 2


def test_an_agent_in_its_space_keeps_its_folder_of_notes_in_a_volume(bridges, fakes):
    """Vie (in a space of its own) writes a memory of e-mails in a folder of the person's home, which its cron reads:
    a working folder, moved into a volume at the same path; the host folder stays as a backup."""
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    bridge.config["sandbox"] = {"isolate": "on"}
    managed_install(bridge, fakes, agents=("vie",), isolated=True)
    space = bridge.root / "spaces" / "vie"
    bridge.config["agents"]["vie"]["space"] = str(space)
    folder = profile(space, "vie")
    notes = bridge.home / "Public" / "emails-memoire"
    (notes / "2025").mkdir(parents=True)
    (notes / "Marc.md").write_text("# Marc\n- dîner le 3 mars\n")
    (notes / "2025" / "Banque.md").write_text("- prêt signé\n")
    media = bridge.home / ".hermes" / "media"
    media.mkdir(parents=True)
    mounts = [f"{notes}:/workspace/emails-memoire:z", f"{media}:/home/hermes/.hermes/media:z"]
    (folder / "config.yaml").write_text(yaml.safe_dump({"terminal": {"backend": "docker", "docker_volumes": mounts}}))
    place = bridge.root / "volumes" / "bipagents-vie-emails-memoire"
    place.mkdir(parents=True)
    bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}", "bipagents-vie-emails-memoire"],
                         stdout=f"{place}\n")
    bridge.write_config()
    plan = bridge.cli("sandbox", "adopt", "vie", "--dry-run")
    assert plan.returncode == 0, plan.stderr
    assert (f"move  {notes} -> volume bipagents-vie-emails-memoire at /workspace/emails-memoire (copied once, 2 files"
            in plan.stdout)
    assert f"drop  {media} (Hermes' media folder" in plan.stdout
    done = bridge.cli("sandbox", "adopt", "--agent", "vie")
    assert done.returncode == 0, done.stderr
    assert "vie: isolated now" in done.stdout
    assert volumes_of(folder) == ["bipagents-vie-emails-memoire:/workspace/emails-memoire"]
    assert (place / "2025" / "Banque.md").read_text() == "- prêt signé\n"
    assert (notes / "Marc.md").exists()
    restart = [e for e in bridge.commands.entries("hermes") if e["argv"] == ["gateway", "restart"]]
    assert restart and restart[-1]["hermes_home"] == str(space)
    # Undone, then adopted again: the volume holds the copy already, left as it is (the agent's work since kept).
    (place / "new-by-vie.md").write_text("x")
    undone = bridge.cli("sandbox", "adopt", "--agent", "vie", "--undo")
    assert undone.returncode == 0 and volumes_of(folder) == mounts
    again = bridge.cli("sandbox", "adopt", "--agent", "vie")
    assert again.returncode == 0, again.stderr
    assert "holds 3 files" in again.stdout and (place / "new-by-vie.md").exists()


def test_a_folder_the_connectors_write_is_linked_not_copied(bridges, fakes):
    """The owner's case: the health connectors (MCP servers on Hermes' side) write SQLite databases in the agent's
    data/ every few hours, its sandbox's scripts read them; a copy made once would go stale. Linked: the volume at the
    folder's path; the credentials its scripts use too, and their sites allowed in the same run (--allow-sites)."""
    bridge, hub, folder, alex, mounts, places = setup(bridges, fakes)
    data = config_yaml(folder)
    data["mcp_servers"]["garmin"]["env"]["GARMIN_DB"] = f"{folder}/data/garmin.db"
    (folder / "config.yaml").write_text(yaml.safe_dump(data))
    # the sandbox last stopped long ago; scripts/ changed since (a program of this machine writes it)
    bridge.commands.rule("podman", ["ps", "-a", "--filter"], stdout="exited 1000\n")
    plan = bridge.cli("sandbox", "adopt", "--agent", "wellness", "--dry-run", "--link", f"{folder}/bin")
    assert plan.returncode == 0, plan.stderr
    out = plan.stdout
    assert f"link  {folder}/data -> volume bipagents-wellness-data" in out
    assert "the MCP server « garmin » uses it, outside the sandbox" in out
    assert f"link  {folder}/scripts -> volume bipagents-wellness-scripts" in out and "changed since its sandbox last ran" in out
    assert f"link  {folder}/bin -> volume bipagents-wellness-bin at /home/hermes/.hermes/profiles/wellness/bin (read-only)" in out
    assert "you asked for it (--link)" in out
    assert not (folder / "data").is_symlink()                                    # a dry run
    done = bridge.cli("sandbox", "adopt", "--agent", "wellness", "--link", f"{folder}/bin", "--allow-sites")
    assert done.returncode == 0, done.stderr
    agent_path = "/home/hermes/.hermes/profiles/wellness"
    assert f"bipagents-wellness-data:{agent_path}/data" in volumes_of(folder)
    assert f"bipagents-wellness-bin:{agent_path}/bin:ro" in volumes_of(folder)
    assert (folder / "data").is_symlink() and (folder / "data.before-adopt" / "steps.csv").read_text() == "a,b"
    assert (places["data"] / "2026" / "sleep.json").read_text() == "{}"
    # the connector writes its database through the link: the sandbox's volume has it
    (folder / "data" / "garmin.db").write_bytes(b"SQLite format 3")
    assert (places["data"] / "garmin.db").read_bytes() == b"SQLite format 3"
    assert oct((places["data"] / "steps.csv").stat().st_mode & 0o777) == "0o666"
    sites = json.loads((bridge.state_dir / "sandbox-sites.json").read_text())["agents"]["wellness"]["sites"]
    assert [(s["host"], s["by"]) for s in sites] == [("api.ouraring.com", "adopt"), ("cloud.ouraring.com", "adopt")]
    assert "sites allowed for its programs" in done.stdout
    state = json.loads((bridge.state_dir / "sandbox.json").read_text())["adopted"]["wellness"]
    kinds = {v["name"]: v["kind"] for v in state["volumes"]}
    assert kinds["bipagents-wellness-data"] == "link" and kinds["bipagents-wellness-skills"] == "code"
    assert next(v for v in state["volumes"] if v["name"] == "bipagents-wellness-data")["backup"] == f"{folder}/data.before-adopt"
    # Undone: the folders back, with the volume's latest content.
    undone = bridge.cli("sandbox", "adopt", "--agent", "wellness", "--undo")
    assert undone.returncode == 0, undone.stderr
    assert volumes_of(folder) == mounts
    assert (folder / "data").is_dir() and not (folder / "data").is_symlink()
    assert (folder / "data" / "garmin.db").read_bytes() == b"SQLite format 3"
    assert oct((folder / "data" / "steps.csv").stat().st_mode & 0o022) == "0o0"
