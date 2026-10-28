"""The agents' sandboxes isolated (bridge-rs/src/sandbox.rs, `[sandbox] isolate = "on"` since these tests run where
`auto` would leave them as they are): no folder of this machine mounted, the uploads copied into a podman volume, the
shared folders in podman volumes of their own (shares.rs) and through the file tools (`/v1/agent-tools/files`) with
their permission checks, the registries-only network, and the way back (`isolate = "off"`). podman is the fake one:
its volumes are folders of the test."""
import base64
import hashlib
import ipaddress
import json
import os
import socket
import subprocess
import sys
import threading
import time
import tomllib
from pathlib import Path

import yaml

from bbkit.common import AUTH
from bbkit.servers import free_port, wait_until
from conftest import managed_install, profile, wait_registered

FILES_PATH = "/v1/agent-tools/files"
TOKENS = {"vie": "vie-token-0123456789abcdefghijklmnop", "wellness": "wellness-token-0123456789abcdefghij",
          "budget": "budget-token-0123456789abcdefghijklm"}


def isolated(bridges, fakes, agents=("vie", "wellness"), proxy_ok=True):
    """`agents` run their tools in Hermes' docker sandbox, with their uploads; vie mounts photos of the person's and
    still has the mounts an earlier bridge set."""
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    bridge.config["sandbox"] = {"isolate": "on"}
    hub = managed_install(bridge, fakes, agents=agents, isolated=True)
    root = bridge.state_dir / "shares"
    volumes = {}
    for name in agents:
        folder = profile(hub, name)
        uploads = folder / "uploads"
        (uploads / "2026-10").mkdir(parents=True)
        (uploads / "2026-10" / "0123456789ab-facture.pdf").write_bytes(b"%PDF facture")
        bridge.config["agents"][name]["upload_dir_host"] = str(uploads)
        bridge.config["agents"][name]["upload_dir_container"] = str(uploads)
        mounts = [f"{uploads}:/fichiers:ro,z", f"{root}/{name}:/partage:z"]
        if name == "vie":
            mounts = ["/home/x/photos:/photos:ro"] + mounts + [f"{root}/wellness:/lecture/wellness:ro,z"]
        (folder / "config.yaml").write_text(yaml.safe_dump({"terminal": {"backend": "docker", "docker_volumes": mounts}}))
        for kind in ("files", "envoi", "share"):
            volume = bridge.root / "volumes" / f"{kind}-{name}"
            volume.mkdir(parents=True)
            volumes[(kind, name)] = volume
            bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}",
                                            f"bipagents-{kind}-{name}"], stdout=f"{volume}\n")
    bridge.commands.rule("podman", ["network", "exists"], exit=1)
    if not proxy_ok:
        bridge.commands.rule("podman", ["run"], exit=125, stderr="Error: image not known")
    return bridge, hub, volumes


def linked(bridge):
    """Vie reads what Wellness shares (the person's link, already there)."""
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    (bridge.state_dir / "shares.json").write_text(json.dumps({"reads": [["vie", "wellness"]]}) + "\n")


def terminal(hub, name):
    """The profile's terminal settings, read whole: the fake `hermes` rewrites the file in place, and the bridge's
    other start-up work (its tools, its soul) may be writing it at that moment."""
    for _ in range(400):    # 20 s: a loaded CI runner starts the bridge slower than a laptop
        data = yaml.safe_load((hub / "profiles" / name / "config.yaml").read_text())
        if isinstance(data, dict) and "mcp_servers" in data:
            return data.get("terminal", {})
        time.sleep(0.05)
    raise AssertionError(f"{name}'s config.yaml never read whole")


CODE_KEYS = ["execute_code", "script execution via -e/-c flag", "script execution via heredoc",
             "shell command via -c/-lc flag"]


def allowlist(hub, name):
    """The profile's `command_allowlist` (Hermes' permanent approvals), or None while the file is being rewritten."""
    try:
        data = yaml.safe_load((hub / "profiles" / name / "config.yaml").read_text())
    except (OSError, yaml.YAMLError):
        return None
    return data.get("command_allowlist", []) if isinstance(data, dict) else None


def give_tokens(bridge, names):
    import hashlib
    path = bridge.state_dir / "agent-tools.json"
    tokens = json.loads(path.read_text())["tokens"] if path.exists() else {}
    for name in names:
        tokens[name] = hashlib.sha256(TOKENS[name].encode()).hexdigest()
    path.write_text(json.dumps({"tokens": tokens}, indent=1) + "\n")


def rpc(bridge, who, method, params=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    return bridge.post(FILES_PATH, json=body, headers={"Authorization": f"Bearer {TOKENS[who]}"})


def call(bridge, who, name, **arguments):
    result = rpc(bridge, who, "tools/call", {"name": name, "arguments": arguments}).json()["result"]
    return result["content"][0]["text"], result["isError"]


def started(bridge, hub, names):
    bridge.start()
    wait_registered(bridge, list(names))
    assert wait_until(lambda: all("bipagents-envoi-" + n + ":/envoi" in terminal(hub, n).get("docker_volumes", [])
                                  for n in names), timeout=15)
    assert wait_until(lambda: all(tomllib.loads(bridge.toml_text())["agents"][n].get("upload_dir_container") == "/fichiers"
                                  for n in names), timeout=10)
    assert wait_until(lambda: all(
        n in json.loads((bridge.state_dir / "sandbox.json").read_text()).get("agents", {}) for n in names), timeout=10)


def test_an_isolated_sandbox_mounts_no_folder_and_reaches_only_the_registries(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes)
    started(bridge, hub, ["vie", "wellness"])
    vie = terminal(hub, "vie")
    # The person's own mount stays (Hermes keeps asking because of it); the bridge's folders are gone.
    assert vie["docker_volumes"] == ["/home/x/photos:/photos:ro", "bipagents-files-vie:/fichiers:ro",
                                     "bipagents-envoi-vie:/envoi"]
    assert terminal(hub, "wellness")["docker_volumes"] == ["bipagents-files-wellness:/fichiers:ro",
                                                           "bipagents-envoi-wellness:/envoi"]
    assert vie["docker_extra_args"] == ["--network=bipagents-sandbox", "--dns=127.0.0.1"]
    assert vie.get("docker_network", True) is True     # Hermes' default: not written
    # The code it writes runs without asking (Hermes' own permanent approvals), its risky commands still ask.
    assert wait_until(lambda: allowlist(hub, "vie") == CODE_KEYS and allowlist(hub, "wellness") == CODE_KEYS, timeout=10)
    # Each sandbox its own credentials on the proxy (sandbox_sites.rs): the proxy knows which agent asks.
    sites = json.loads((bridge.state_dir / "sandbox-sites.json").read_text())["agents"]
    token = sites["vie"]["token"]
    assert len(token) >= 24 and token != sites["wellness"]["token"]
    proxy = f"http://vie:{token}@10.233.80.2:3128"
    assert vie["docker_env"]["HTTPS_PROXY"] == proxy == vie["docker_env"]["https_proxy"]
    assert vie["docker_env"]["npm_config_https_proxy"] == proxy
    assert oct(os.stat(bridge.state_dir / "sandbox-sites.json").st_mode & 0o777) == "0o600"
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    # reading open to every agent, sending guarded: no site validated yet, the guard's limits; a self-install has
    # neither the download cap nor the connections' limit, nor a share of the processors
    assert policy["agents"]["vie"] == {"key": hashlib.sha256(token.encode()).hexdigest(), "hosts": []}
    assert policy["limits"] == {"send_site_hour": 65536, "send_day": 262144, "download_day": 0,
                                "connections_minute": 0}
    assert policy["browsers"] == {}
    assert not any(a.startswith("--cpus") for a in vie["docker_extra_args"])
    assert not (bridge.state_dir / "sandbox-proxy" / "policy" / "blocklist.txt").exists()
    assert token not in json.dumps(policy) and token not in bridge.log_tail(20000)
    assert vie["docker_env"]["NO_PROXY"] == "localhost,127.0.0.1"
    podman = bridge.commands.of("podman")
    assert ["network", "create", "--internal", "--disable-dns", "--subnet", "10.233.80.0/24", "bipagents-sandbox"] in podman
    run = next(args for args in podman if args[:1] == ["run"])
    assert run[run.index("--name") + 1] == "bipagents-egress" and "--replace" in run
    assert "bipagents-sandbox:ip=10.233.80.2" in run and "podman" in run and "--read-only" in run
    allow = next(a for a in run if a.startswith("BIP_ALLOW="))
    assert "pypi.org" in allow and "files.pythonhosted.org" in allow and "registry.npmjs.org" in allow
    assert "github.com" not in allow
    assert run[run.index("--entrypoint") + 1] == "python3" and "docker.io/nousresearch/hermes-sandbox:desktop" in run
    assert run[run.index("--user") + 1] == "0:0"
    # this machine's resolvers, never a loopback stub nor the internal network's own (aibox 2026-10-28)
    resolvers = [a.removeprefix("--dns=") for a in run if a.startswith("--dns=")]
    assert all(not ipaddress.ip_address(r).is_loopback for r in resolvers)
    assert f"{bridge.state_dir}/sandbox-proxy/policy:/bip/policy:ro,z" in run
    assert f"{bridge.state_dir}/sandbox-proxy/reports:/bip/reports:z" in run
    assert "BIP_POLICY=/bip/policy/sites.json" in run and "BIP_REPORTS=/bip/reports/sends.jsonl" in run
    assert "BIP_USAGE=/bip/reports/usage.json" in run and "BIP_BLOCKLIST=/bip/policy/blocklist.txt" in run
    assert not any("bipagents-browsers" in a for a in run)              # no visible browser on this install
    assert sum(1 for args in podman if args[:1] == ["run"]) == 1       # the network set up once for both agents
    # The uploads are copied into the volume, at the same paths as before.
    assert (volumes[("files", "vie")] / "2026-10" / "0123456789ab-facture.pdf").read_bytes() == b"%PDF facture"
    sent = bridge.post("/v1/files", headers=AUTH, data={"agent": "vie"}, files={"file": ("b.txt", b"hello", "text/plain")})
    path = sent.json()["path"]
    assert path.startswith("/fichiers/")
    assert (volumes[("files", "vie")] / path.removeprefix("/fichiers/")).read_bytes() == b"hello"
    assert oct(os.stat(volumes[("files", "vie")] / path.removeprefix("/fichiers/")).st_mode & 0o777) == "0o644"
    # The file tools: a second MCP entry, « untrusted » (the person approves writes), the same token.
    servers = yaml.safe_load((hub / "profiles" / "vie" / "config.yaml").read_text())["mcp_servers"]
    assert servers["bipagents_files"]["trust"] == "untrusted"
    assert servers["bipagents_files"]["url"].split("?")[0].endswith(FILES_PATH)
    assert servers["bipagents_files"]["headers"] == servers["bipagents"]["headers"]
    state = json.loads((bridge.state_dir / "sandbox.json").read_text())
    assert state["agents"]["vie"] == {"files": str(volumes[("files", "vie")]), "outbox": str(volumes[("envoi", "vie")])}
    assert wait_until(lambda: "sandbox isolated" in bridge.log_tail(2000), timeout=10)
    # Started again: nothing to change, nothing rewritten.
    bridge.stop()
    bridge.commands.clear()
    bridge.start()
    assert wait_until(lambda: sum(a[:2] == ["volume", "inspect"] for a in bridge.commands.of("podman")) == 4, timeout=10)
    time.sleep(1)
    assert not any("terminal." in " ".join(a) for a in bridge.commands.of("hermes"))


def test_without_the_registries_proxy_the_sandbox_gets_no_network_at_all(bridges, fakes):
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie",), proxy_ok=False)
    started(bridge, hub, ["vie"])
    vie = terminal(hub, "vie")
    assert vie["docker_network"] is False
    assert "docker_extra_args" not in vie or vie["docker_extra_args"] == []
    assert not any(k.lower().endswith("proxy") for k in vie.get("docker_env", {}))
    assert vie["docker_volumes"][-2:] == ["bipagents-files-vie:/fichiers:ro", "bipagents-envoi-vie:/envoi"]
    assert wait_until(lambda: "sandboxes without network" in bridge.log_tail(2000), timeout=10)


def test_the_shared_folders_are_volumes_and_the_file_tools_follow_the_links(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness", "budget"))
    started(bridge, hub, ["vie", "wellness", "budget"])
    bridge.commands.clear()
    reads = bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "vie", "source": "wellness"}]})
    assert reads.json() == {"reads": [{"reader": "vie", "source": "wellness"}]}
    # Named volumes (the sandbox stays isolated): Wellness writes in /partage, Vie reads it at /lecture/wellness.
    assert ["volume", "create", "--ignore", "bipagents-share-wellness"] in bridge.commands.of("podman")
    assert terminal(hub, "wellness")["docker_volumes"] == ["bipagents-files-wellness:/fichiers:ro",
                                                           "bipagents-envoi-wellness:/envoi",
                                                           "bipagents-share-wellness:/partage"]
    assert terminal(hub, "vie")["docker_volumes"] == ["/home/x/photos:/photos:ro", "bipagents-files-vie:/fichiers:ro",
                                                      "bipagents-envoi-vie:/envoi",
                                                      "bipagents-share-wellness:/lecture/wellness:ro"]
    assert "bipagents-share" not in " ".join(terminal(hub, "budget")["docker_volumes"])
    assert oct(os.stat(volumes[("share", "wellness")]).st_mode & 0o777) == "0o777"   # its agent writes there
    skill = (hub / "profiles" / "vie" / "skills" / "shared-data" / "SKILL.md").read_text()
    assert "/lecture/<agent>" in skill and "shared_files_get" in skill and "nobody needs to approve" in skill
    # A night routine writes straight into the volume (the reader mounts the same one).
    (volumes[("share", "wellness")] / "sommeil.csv").write_text("nuit;7h")
    share = bridge.state_dir / "shares" / "wellness"
    give_tokens(bridge, ["vie", "wellness", "budget"])
    listed = rpc(bridge, "vie", "tools/list").json()["result"]["tools"]
    hints = {t["name"]: t["annotations"]["readOnlyHint"] for t in listed}
    # Writing in its own shared folder is never asked (it writes it directly anyway); deleting is.
    assert hints == {"shared_files_list": True, "shared_files_get": True, "shared_files_write": True,
                     "shared_files_delete": False, "sandbox_sites": True, "sandbox_sites_request": True}
    assert bridge.post(FILES_PATH, json={"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
                       headers={"Authorization": "Bearer nope"}).status_code == 401
    # Wellness shares a text and a file it made (from /envoi).
    assert call(bridge, "wellness", "shared_files_write", path="resume/semaine.md", content="7 h de sommeil") == (
        "In your shared folder: resume/semaine.md", False)
    (volumes[("envoi", "wellness")] / "graphe.png").write_bytes(b"PNG")
    assert call(bridge, "wellness", "shared_files_write", path="graphe.png", **{"from": "/envoi/graphe.png"})[1] is False
    volume = volumes[("share", "wellness")]
    assert (volume / "resume" / "semaine.md").read_text() == "7 h de sommeil" and (volume / "graphe.png").read_bytes() == b"PNG"
    assert oct(os.stat(volume / "graphe.png").st_mode & 0o777) == "0o666"           # its agent may change it
    # The folder backups take follows the volume (every minute, and at once after the tools' writes).
    assert (share / "resume" / "semaine.md").read_text() == "7 h de sommeil"
    assert (share / "sommeil.csv").read_text() == "nuit;7h"
    # Never a file of this machine: outside /envoi, through a link, or up the tree.
    os.symlink("/etc/hosts", volumes[("envoi", "wellness")] / "hosts")
    assert call(bridge, "wellness", "shared_files_write", path="h", **{"from": "/envoi/hosts"})[1] is True
    assert call(bridge, "wellness", "shared_files_write", path="h", **{"from": "/etc/hosts"})[1] is True
    (volumes[("envoi", "wellness")] / "sub").mkdir()
    (volumes[("envoi", "wellness")] / "sub" / "x.md").write_text("x")
    assert call(bridge, "wellness", "shared_files_write", path="h", **{"from": "/envoi/sub/x.md"})[1] is True
    os.mkfifo(volumes[("envoi", "wellness")] / "pipe")
    assert call(bridge, "wellness", "shared_files_write", path="h", **{"from": "/envoi/pipe"})[1] is True
    assert call(bridge, "wellness", "shared_files_write", path="../vie/x.md", content="x")[1] is True
    assert call(bridge, "wellness", "shared_files_get", agent="wellness", path="../../sandbox.json")[1] is True
    assert not (volume / "h").exists()
    # Vie reads it (a copy in its own volume), budget can't.
    text, failed = call(bridge, "vie", "shared_files_list")
    assert not failed and "Shared with you by Wellness (agent `wellness`, at /lecture/wellness" in text
    assert "resume/semaine.md" in text and "sommeil.csv" in text
    assert call(bridge, "vie", "shared_files_get", agent="wellness", path="resume/semaine.md") == (
        "Copied to /fichiers/partages/wellness/resume/semaine.md (read-only).", False)
    copy = volumes[("files", "vie")] / "partages" / "wellness" / "resume" / "semaine.md"
    assert copy.read_text() == "7 h de sommeil"
    text, failed = call(bridge, "budget", "shared_files_get", agent="wellness", path="resume/semaine.md")
    assert failed and "shares nothing with you" in text
    assert "wellness" not in call(bridge, "budget", "shared_files_list")[0].split("Your shared folder")[1]
    # Writes and deletes only in one's own folder.
    assert call(bridge, "vie", "shared_files_delete", path="resume/semaine.md")[1] is True     # not in vie's folder
    assert (volume / "resume" / "semaine.md").exists()
    assert call(bridge, "wellness", "shared_files_delete", path="resume") == (
        "Deleted from your shared folder: resume", False)
    assert not (volume / "resume").exists() and (volume / "graphe.png").exists()
    assert not (share / "resume").exists()
    # A file of its shared folder, or of one it reads, handed to the person (MEDIA:).
    got = bridge.get("/v1/media", params={"path": "/lecture/wellness/graphe.png", "agent": "vie"}, headers=AUTH)
    assert got.status_code == 200 and got.content == b"PNG"
    got = bridge.get("/v1/media", params={"path": "/partage/sommeil.csv", "agent": "wellness"}, headers=AUTH)
    assert got.status_code == 200 and got.content == b"nuit;7h"
    assert bridge.get("/v1/media", params={"path": "/lecture/wellness/graphe.png", "agent": "budget"},
                      headers=AUTH).status_code == 404
    # The link taken back: vie's mount and copies go, and it can't fetch any more; the volume stays its source's.
    bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": []})
    assert terminal(hub, "vie")["docker_volumes"][-1] == "bipagents-envoi-vie:/envoi"
    assert terminal(hub, "wellness")["docker_volumes"][-1] == "bipagents-envoi-wellness:/envoi"
    assert not (volumes[("files", "vie")] / "partages" / "wellness").exists()
    assert call(bridge, "vie", "shared_files_get", agent="wellness", path="graphe.png")[1] is True
    assert not (hub / "profiles" / "vie" / "skills" / "shared-data").exists()
    assert (volume / "graphe.png").exists()


def test_an_update_moves_the_shared_folders_into_their_volumes(bridges, fakes):
    """An install whose agents shared through folders of this machine (or the tools of 2026-10-22): at start, each
    source's folder is copied into its volume once, the folder kept as a backup, the sandboxes mount the volumes."""
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness"))
    linked(bridge)
    old = bridge.state_dir / "shares" / "wellness"
    (old / "export").mkdir(parents=True)
    (old / "export" / "sommeil.csv").write_text("nuit;7h")
    started(bridge, hub, ["vie", "wellness"])
    assert wait_until(lambda: "bipagents-share-wellness:/lecture/wellness:ro" in terminal(hub, "vie")["docker_volumes"],
                      timeout=10)
    assert (volumes[("share", "wellness")] / "export" / "sommeil.csv").read_text() == "nuit;7h"
    backup = bridge.state_dir / "shares" / "wellness.before-volume"
    assert (backup / "export" / "sommeil.csv").read_text() == "nuit;7h"
    assert (old / "export" / "sommeil.csv").read_text() == "nuit;7h"         # the folder kept in step with the volume
    assert wait_until(lambda: "old folder is kept as a backup" in bridge.log_tail(4000), timeout=10)
    state = json.loads((bridge.state_dir / "sandbox.json").read_text())
    assert state["shares"] == {"wellness": {"folder": str(volumes[("share", "wellness")])}}
    # Started again: nothing copied twice, no second backup; the agent's changes in the volume win.
    (volumes[("share", "wellness")] / "export" / "sommeil.csv").write_text("nuit;8h")
    bridge.stop()
    bridge.start()
    assert wait_until(lambda: (old / "export" / "sommeil.csv").read_text() == "nuit;8h", timeout=15)
    assert sorted(p.name for p in (bridge.state_dir / "shares").iterdir()) == ["wellness", "wellness.before-volume"]
    # The command line of the update says it too.
    bridge.stop()
    out = bridge.cli("sandbox")
    assert out.returncode == 0, out.stderr
    assert "wellness's shared folder: in bipagents-share-wellness" in out.stdout


def test_turning_isolation_off_gives_the_sandboxes_their_folders_back(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness"))
    linked(bridge)
    started(bridge, hub, ["vie", "wellness"])
    assert wait_until(lambda: "bipagents-share-wellness:/partage" in terminal(hub, "wellness")["docker_volumes"],
                      timeout=10)
    (volumes[("share", "wellness")] / "semaine.md").write_text("written while isolated")
    bridge.stop()
    bridge.commands.clear()
    bridge.config["sandbox"] = {"isolate": "off"}
    bridge.start()
    root = bridge.state_dir / "shares"
    uploads = profile(hub, "vie") / "uploads"
    assert wait_until(lambda: terminal(hub, "vie")["docker_volumes"] == [
        "/home/x/photos:/photos:ro", f"{uploads}:/fichiers:ro,z", f"{root}/wellness:/lecture/wellness:ro,z"],
        timeout=15), terminal(hub, "vie")
    assert wait_until(lambda: terminal(hub, "wellness")["docker_volumes"] == [
        f"{profile(hub, 'wellness') / 'uploads'}:/fichiers:ro,z", f"{root}/wellness:/partage:z"], timeout=15)
    for name in ("vie", "wellness"):
        settings = terminal(hub, name)
        assert settings.get("docker_extra_args", []) == []
        assert not any(k.lower().endswith("proxy") for k in settings.get("docker_env", {}))
        assert settings.get("docker_network", True) is True
    # Its code asks again before it runs (the network is no longer the bridge's).
    assert wait_until(lambda: allowlist(hub, "vie") == [] and allowlist(hub, "wellness") == [], timeout=10)
    # What the agent wrote while isolated is in the folder it now mounts; the proxy is stopped, the volumes kept.
    assert (root / "wellness" / "semaine.md").read_text() == "written while isolated"
    assert wait_until(lambda: ["rm", "-f", "--ignore", "bipagents-egress"] in bridge.commands.of("podman"), timeout=10)
    assert not any(a[:2] == ["volume", "rm"] for a in bridge.commands.of("podman"))
    assert not any(a[:1] == ["run"] for a in bridge.commands.of("podman"))
    state = json.loads((bridge.state_dir / "sandbox.json").read_text())
    assert not state.get("agents") and not state.get("shares")
    assert wait_until(lambda: "sandbox not isolated any more" in bridge.log_tail(4000), timeout=10)
    skill = (hub / "profiles" / "vie" / "skills" / "shared-data" / "SKILL.md").read_text()
    assert "shared_files_get" not in skill
    # The sites' list is kept, with no effect, and the app says so.
    view = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert view["effective"] is False and view["why"] == "isolation"
    # Started again: nothing more to change.
    bridge.stop()
    bridge.commands.clear()
    bridge.start()
    time.sleep(2)
    assert not any("terminal." in " ".join(a) for a in bridge.commands.of("hermes"))
    assert not any(a[:1] == ["rm"] for a in bridge.commands.of("podman"))


def site_cards(bridge, agent):
    items = bridge.get("/v1/approvals", params={"agent": agent}, headers=AUTH).json()["items"]
    return [i for i in items if i["run_id"].startswith("bip-site-")]


def send_report(bridge, *rows):
    """What the proxy writes when it cuts a send (egress_proxy.py)."""
    reports = bridge.state_dir / "sandbox-proxy" / "reports" / "sends.jsonl"
    with reports.open("a") as f:
        for agent, host, size in rows:
            f.write(json.dumps({"kind": "send", "agent": agent, "host": host, "bytes": size, "day": False,
                                "at": int(time.time())}) + "\n")
    return reports


def test_a_send_cut_asks_the_person_once_and_autoriser_validates_the_site(bridges, fakes):
    """Reading is open; the proxy cut a 2 MB upload of vie's to a site nobody validated (its report): ONE card for the
    person, « Vie veut envoyer des données à upload.example.net (environ 2 Mo) — Autoriser / Refuser », among the
    approvals; the agent can ask first (on the same card, a moment later); an older app's answer; the app's list."""
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    view = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert view["reading_open"] is True and view["all_sites"] is True and view["sites"] == []
    # an older app's « Tous les sites » switch: accepted, nothing to change (reading is open)
    off = bridge.patch("/v1/agents/vie/sandbox/sites", json={"all_sites": False}, headers=AUTH)
    assert off.status_code == 200 and off.json()["all_sites"] is True, off.text
    reports = bridge.state_dir / "sandbox-proxy" / "reports" / "sends.jsonl"
    assert reports.parent.is_dir()
    send_report(bridge, ("vie", "upload.example.net", 2 * 1024 * 1024), ("vie", "upload.example.net", 2 * 1024 * 1024),
                ("vie", "localhost", 99999), ("nobody", "x.fr", 99999))
    assert wait_until(lambda: len(site_cards(bridge, "vie")) == 1, timeout=10)
    assert wait_until(lambda: not reports.exists(), timeout=5)
    card = site_cards(bridge, "vie")[0]
    assert card["bip_site"] == "upload.example.net" and card["choices"] == ["always", "deny"]
    assert card["description"] == "Vie veut envoyer des données à upload.example.net (environ 2 Mo)"
    assert card["bip_site_kind"] == "send" and card["bip_site_send_bytes"] == 2 * 1024 * 1024
    assert card["bip_site_via"] == "proxy" and card["session_id"] is None
    assert "a send to a site not validated was cut" in bridge.log_tail(20000)
    # The agent asks first for another one, with its reason; and lists what it has.
    give_tokens(bridge, ["vie", "wellness"])
    text, error = call(bridge, "vie", "sandbox_sites_request", host="https://api.meteo.fr/v1", reason="la pluie demain")
    assert not error and "api.meteo.fr" in text
    assert call(bridge, "vie", "sandbox_sites_request", host="pypi.org")[0] == "You may send to it already."
    assert call(bridge, "vie", "sandbox_sites_request", host="10.0.0.1")[1] is True
    # one card, not two: the sites asked a moment apart gather
    [card] = site_cards(bridge, "vie")
    assert card["bip_sites"] == ["upload.example.net", "api.meteo.fr"] and card["bip_site"] == "upload.example.net"
    assert card["description"] == ("Vie veut envoyer des données à 2 sites (environ 2 Mo) : upload.example.net, "
                                   "api.meteo.fr")
    assert card["bip_site_reason"] == "la pluie demain" and card["command"] == "upload.example.net, api.meteo.fr"
    assert site_cards(bridge, "wellness") == []
    # An older app (« Autoriser toujours », the first site shown): that one; the other waits on a card of its own.
    answered = bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{card['request_id']}", json={"allow": True},
                           headers=AUTH)
    assert answered.status_code == 200, answered.text
    assert [s["host"] for s in answered.json()["sites"]] == ["upload.example.net"]
    assert answered.json()["sites"][0]["by"] == "request"
    [left] = site_cards(bridge, "vie")
    assert left["bip_sites"] == ["api.meteo.fr"] and left["request_id"] != card["request_id"]
    # « Refuser » from a notification
    denied = bridge.post("/v1/approve", json={"agent": "vie", "run_id": left["run_id"], "choice": "deny"}, headers=AUTH)
    assert denied.status_code == 200 and denied.json()["choice"] == "deny", denied.text
    assert site_cards(bridge, "vie") == []
    assert bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{left['request_id']}",
                       json={"allow": True}, headers=AUTH).status_code == 404
    # validated for vie only, its counts reset in the proxy
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    assert policy["agents"]["vie"]["hosts"] == ["upload.example.net"] and policy["agents"]["vie"]["reset"] > 0
    assert policy["agents"]["wellness"]["hosts"] == [] and "any" not in policy["agents"]["wellness"]
    kept = bridge.get("/v1/approvals/answered", params={"agent": "vie"}, headers=AUTH).json()["items"]
    assert [(k["command"], k["choice"], k["topic"]) for k in kept] == [
        ("upload.example.net", "always", "site"), ("api.meteo.fr", "deny", "site")]
    # Refused: the proxy's next cut asks nothing; the agent's own request neither, today. A validated site is never
    # asked about.
    send_report(bridge, ("vie", "api.meteo.fr", 80000), ("vie", "upload.example.net", 5 * 1024 * 1024))
    assert wait_until(lambda: not reports.exists(), timeout=10)
    time.sleep(0.5)
    assert site_cards(bridge, "vie") == []
    assert call(bridge, "vie", "sandbox_sites_request", host="api.meteo.fr")[1] is True
    # The app's list: the registries always validated, the sites with their date, one added by hand, one removed.
    view = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert "pypi.org" in view["registries"] and view["effective"] is True and view["why"] is None
    assert bridge.post("/v1/agents/vie/sandbox/sites", json={"host": "*.ouraring.com"}, headers=AUTH).status_code == 200
    for bad in ("localhost", "*.com", "192.168.1.1", "nas.local"):
        assert bridge.post("/v1/agents/vie/sandbox/sites", json={"host": bad}, headers=AUTH).status_code == 422, bad
    assert bridge.delete("/v1/agents/vie/sandbox/sites/upload.example.net", headers=AUTH).status_code == 200
    assert bridge.delete("/v1/agents/vie/sandbox/sites/upload.example.net", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents/nobody/sandbox/sites", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents/vie/sandbox/sites").status_code == 401
    listed = call(bridge, "vie", "sandbox_sites")[0]
    assert "*.ouraring.com" in listed and "upload.example.net" not in listed and "pypi.org" in listed
    assert "every public HTTPS site is open" in listed
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    assert policy["agents"]["vie"]["hosts"] == ["*.ouraring.com"]
    # the proxy's counts of the day, shown with the list
    usage = {"day": time.strftime("%Y-%m-%d", time.gmtime()), "at": int(time.time()),
             "agents": {"vie": {"sent": 1234, "received": 5678, "unvalidated": 1000, "connections": 3, "held": False}}}
    (bridge.state_dir / "sandbox-proxy" / "reports" / "usage.json").write_text(json.dumps(usage))
    today = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()["today"]
    assert today["received"] == 5678 and today["sent"] == 1234


def test_the_sites_kept_before_reading_was_open_stay_validated(bridges, fakes):
    """An install from before 2026-10-28: its « Tous les sites » choices and visited sites are dropped, the sites the
    person allowed are the agent's validated sites, an old reading card still answers."""
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    bridge.state_dir.mkdir(parents=True, exist_ok=True)
    old = {"agents": {"vie": {"token": "t" * 32, "all_sites": False, "refused": {},
                              "sites": [{"host": "data.gouv.fr", "at": "2026-10-24T10:00:00+00:00", "by": "request"}],
                              "visited": {"www.lemonde.fr": time.time()}}},
           "pending": [{"id": "oldcard1", "agent": "vie", "host": "a.fr", "hosts": ["a.fr"], "reason": "",
                        "via": "proxy", "at": time.time()}]}
    (bridge.state_dir / "sandbox-sites.json").write_text(json.dumps(old))
    started(bridge, hub, ["vie", "wellness"])
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    assert policy["agents"]["vie"]["hosts"] == ["data.gouv.fr"] and "any" not in policy["agents"]["vie"]
    [card] = site_cards(bridge, "vie")
    assert card["description"] == "Vie veut envoyer des données à a.fr" and card["bip_site_send_bytes"] == 0
    # an older app's « Tous les sites pour Vie » on it: the sites on the card, not every site
    opened = bridge.post("/v1/approve", json={"agent": "vie", "run_id": card["run_id"], "choice": "all_sites"},
                         headers=AUTH)
    assert opened.status_code == 200 and opened.json()["choice"] == "always", opened.text
    view = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert [s["host"] for s in view["sites"]] == ["data.gouv.fr", "a.fr"]
    assert "visited" not in json.loads((bridge.state_dir / "sandbox-sites.json").read_text())["agents"]["vie"]


def test_the_offer_caps_downloads_connections_processors_and_refuses_piracy(bridges, fakes):
    """On an offer place (`cloud`): each sandbox's share of the processors (`--cpus`), the proxy's daily download
    limit and connections per minute, and the piracy/tracker blocklist fetched by the bridge (here from a local
    server) for the proxy; the agents' visible browsers go through the proxy, known by their address."""
    import http.server
    import threading

    class Lists(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            body = (b"# Title: piracy\n0.0.0.0 thepiratebay.example\n||rarbg.example^\n" if self.path == "/piracy.txt"
                    else b"tracker.opentrackr.example\n")
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Lists)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
        port = server.server_address[1]
        bridge.config["cloud"] = True
        bridge.config["sandbox"]["blocklist_urls"] = [f"http://127.0.0.1:{port}/piracy.txt",
                                                     f"http://127.0.0.1:{port}/torrent.txt"]
        bridge.config.setdefault("browser", {})["enabled"] = True
        bridge.config["browser"]["runtime"] = "podman"
        (bridge.data / "browsers" / "vie").mkdir(parents=True)
        (bridge.data / "browsers" / "vie" / "ports.json").write_text('{"cdp": 9602, "vnc": 9603}')
        started(bridge, hub, ["vie", "wellness"])
        vie = terminal(hub, "vie")
        assert vie["docker_extra_args"] == ["--network=bipagents-sandbox", "--dns=127.0.0.1", "--cpus=1.5"]
        policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
        assert policy["limits"] == {"send_site_hour": 65536, "send_day": 262144,
                                    "download_day": 2048 * 1024 * 1024, "connections_minute": 300}
        # vie's browser: 10.233.82.11 (its DevTools port is the second pair of the range)
        assert policy["browsers"] == {"10.233.82.11": "vie"}
        podman = bridge.commands.of("podman")
        assert ["network", "create", "--subnet", "10.233.82.0/24", "bipagents-browsers"] in podman
        run = next(args for args in podman if args[:1] == ["run"])
        assert "bipagents-browsers:ip=10.233.82.2" in run and "BIP_BROWSERS=10.233.82.2" in run
        blocklist = bridge.state_dir / "sandbox-proxy" / "policy" / "blocklist.txt"
        assert wait_until(blocklist.exists, timeout=10)
        names = [l for l in blocklist.read_text().splitlines() if l and not l.startswith("#")]
        assert names == ["rarbg.example", "thepiratebay.example", "tracker.opentrackr.example"]
    finally:
        server.shutdown()


def test_a_deleted_agent_loses_its_volumes(bridges, fakes):
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    linked(bridge)
    started(bridge, hub, ["vie", "wellness"])
    assert wait_until(lambda: "wellness" in json.loads((bridge.state_dir / "sandbox.json").read_text()).get("shares", {}),
                      timeout=10)
    bridge.commands.clear()
    assert bridge.delete("/v1/agents/wellness", headers=AUTH).status_code == 204
    podman = bridge.commands.of("podman")
    assert ["volume", "rm", "-f", "bipagents-files-wellness"] in podman
    assert ["volume", "rm", "-f", "bipagents-envoi-wellness"] in podman
    assert ["volume", "rm", "-f", "bipagents-share-wellness"] in podman
    assert not (bridge.state_dir / "shares" / "wellness").exists()
    assert "bipagents-share-wellness:/lecture/wellness:ro" not in terminal(hub, "vie")["docker_volumes"]
    assert "wellness" not in json.loads((bridge.state_dir / "sandbox.json").read_text())["agents"]
    assert "wellness" not in json.loads((bridge.state_dir / "sandbox-sites.json").read_text())["agents"]


def test_the_command_line_isolates_and_says_what_each_agent_got(bridges, fakes):
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie",))
    bridge.write_config()
    out = bridge.cli("sandbox")
    assert out.returncode == 0, out.stderr
    assert "network: bipagents-sandbox (no route out), registries through http://10.233.80.2:3128" in out.stdout
    assert ("vie: isolated (volumes: /home/x/photos:/photos:ro bipagents-files-vie:/fichiers:ro "
            "bipagents-envoi-vie:/envoi); still mounted by hand: /home/x/photos:/photos:ro") in out.stdout
    assert tomllib.loads(bridge.toml_text())["agents"]["vie"]["upload_dir_container"] == "/fichiers"


def test_a_sandbox_holding_its_desktop_browser_is_left_as_it_was(bridges, fakes):
    """An install without the visible browser's containers: Hermes' Bot Desktop, and the browser the agent drives,
    run in the sandbox, which needs the internet. That sandbox is not isolated at all: its folders stay mounted, so
    Hermes keeps asking the person before risky commands (never code with the open internet and no question asked)."""
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie",))
    (hub / "config.yaml").write_text(yaml.safe_dump({"bot_desktop": {"auto_start": True}}))
    bridge.start()
    wait_registered(bridge, ["vie"])
    time.sleep(1)
    vie = terminal(hub, "vie")
    assert any(v.startswith("/") and v.endswith(":/fichiers:ro,z") for v in vie["docker_volumes"])
    assert not any(v.startswith("bipagents-") for v in vie["docker_volumes"])
    assert vie.get("docker_network", True) is True and not vie.get("docker_extra_args")
    assert not any(k.lower().endswith("proxy") for k in vie.get("docker_env", {}))
    assert not any(a[:1] == ["run"] or a[:2] == ["network", "create"] for a in bridge.commands.of("podman"))
    servers = yaml.safe_load((hub / "profiles" / "vie" / "config.yaml").read_text())["mcp_servers"]
    assert "bipagents_files" not in servers


PROXY_SCRIPT = Path(__file__).resolve().parents[2] / "bridge-rs" / "assets" / "sandbox" / "egress_proxy.py"
# The proxy as the bridge runs it, with one change: every site is a server of this test (its checks of addresses and
# names are its own unit tests'; here no name is looked up and nothing leaves this machine).
LOCAL_SITES = """
import asyncio, importlib.util, os, sys
spec = importlib.util.spec_from_file_location("egress", sys.argv[1])
proxy = importlib.util.module_from_spec(spec)
spec.loader.exec_module(proxy)
proxy.SINK_IDLE = 0.5
site = int(os.environ["TEST_SITE_PORT"])
async def resolve(host, port):
    return ["127.0.0.1"]
async def connect(addresses, port):
    return await asyncio.open_connection("127.0.0.1", site)
proxy.resolve, proxy.connect = resolve, connect
asyncio.run(proxy.main())
"""


class Site(threading.Thread):
    """A site: reads what comes (counted), answers a GET with `serve` bytes."""

    def __init__(self, serve=0):
        super().__init__(daemon=True)
        self.serve, self.received = serve, 0
        self.sock = socket.socket()
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(16)
        self.port = self.sock.getsockname()[1]

    def run(self):
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            threading.Thread(target=self.one, args=(conn,), daemon=True).start()

    def one(self, conn):
        try:
            first = conn.recv(65536)
            self.received += len(first)
            if self.serve:
                conn.sendall(b"y" * self.serve)
            else:
                while True:
                    data = conn.recv(65536)
                    if not data:
                        break
                    self.received += len(data)
        except OSError:
            pass
        conn.close()


def record(kind, size):
    return bytes([kind, 3, 3]) + size.to_bytes(2, "big") + b"x" * size


def through(port, host, token, payload, wait_answer=0):
    """A CONNECT as `vie`, then `payload` (a TLS-looking stream); returns the status line and what came back."""
    sock = socket.create_connection(("127.0.0.1", port), timeout=10)
    auth = base64.b64encode(f"vie:{token}".encode()).decode()
    sock.sendall(f"CONNECT {host}:443 HTTP/1.1\r\nProxy-Authorization: Basic {auth}\r\n\r\n".encode())
    head = b""
    while b"\r\n\r\n" not in head:
        head += sock.recv(1)
    got = 0
    try:
        sock.sendall(payload)
        while got < wait_answer:
            data = sock.recv(1 << 20)
            if not data:
                break
            got += len(data)
    except OSError:
        pass
    sock.close()
    return head.split(b"\r\n")[0].decode(), got


def test_the_real_proxy_cuts_a_big_upload_and_the_person_gets_one_card(bridges, fakes):
    """The proxy, run with the bridge's own policy and reports folder: a 2 MB upload to a site vie's person never
    validated is cut past the hour's budget and becomes ONE card; a page read and a 5 MB download pass; once the person
    validated the site, the same upload goes whole."""
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    token = json.loads((bridge.state_dir / "sandbox-sites.json").read_text())["agents"]["vie"]["token"]
    proxy_dir = bridge.state_dir / "sandbox-proxy"
    site = Site()
    site.start()
    reader = Site(serve=5 * 1024 * 1024)
    reader.start()
    port = free_port()
    env = {"BIP_POLICY": str(proxy_dir / "policy" / "sites.json"), "BIP_REPORTS": str(proxy_dir / "reports" / "sends.jsonl"),
           "BIP_PORT": str(port), "BIP_ALLOW": "pypi.org", "TEST_SITE_PORT": str(site.port), "PATH": "/usr/bin:/bin"}
    proc = subprocess.Popen([sys.executable, "-I", "-u", "-c", LOCAL_SITES, str(PROXY_SCRIPT)], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        def up():
            try:
                socket.create_connection(("127.0.0.1", port), timeout=0.2).close()
                return True
            except OSError:
                return False
        assert wait_until(up, timeout=10)
        hello = record(22, 517)
        # a page read: a request's worth
        status, _ = through(port, "www.example-news.fr", token, hello + record(23, 400))
        assert status == "HTTP/1.1 200 Connection established"
        # the upload: cut, measured, reported
        upload = hello + b"".join(record(23, 16384) for _ in range(128))
        status, _ = through(port, "upload.example.net", token, upload)
        assert status == "HTTP/1.1 200 Connection established"
        assert wait_until(lambda: len(site_cards(bridge, "vie")) == 1, timeout=15)
        [card] = site_cards(bridge, "vie")
        assert card["bip_site"] == "upload.example.net" and card["bip_site_kind"] == "send"
        assert 1.5 * 1024 * 1024 < card["bip_site_send_bytes"] <= 2.2 * 1024 * 1024
        assert card["description"].startswith("Vie veut envoyer des données à upload.example.net (environ 2")
        assert site.received < 64 * 1024 + 2000 + 600       # the read, at most the hour's budget, the handshakes
        # a download of any size passes (the site answering is another server: the proxy counts what it gets)
        proc.kill()
        proc.wait()
        env["TEST_SITE_PORT"] = str(reader.port)
        proc = subprocess.Popen([sys.executable, "-I", "-u", "-c", LOCAL_SITES, str(PROXY_SCRIPT)], env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        assert wait_until(up, timeout=10)
        status, got = through(port, "files.example.net", token, hello + record(23, 300), wait_answer=5 * 1024 * 1024)
        assert status == "HTTP/1.1 200 Connection established" and got == 5 * 1024 * 1024
        # « Autoriser »: validated, the same upload goes whole
        assert bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{card['request_id']}",
                           json={"allow": True, "hosts": ["upload.example.net"]}, headers=AUTH).status_code == 200
        proc.kill()
        proc.wait()
        env["TEST_SITE_PORT"] = str(site.port)
        proc = subprocess.Popen([sys.executable, "-I", "-u", "-c", LOCAL_SITES, str(PROXY_SCRIPT)], env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
        assert wait_until(up, timeout=10)
        before = site.received
        through(port, "upload.example.net", token, upload)
        assert wait_until(lambda: site.received - before >= len(upload), timeout=10)
        time.sleep(4)
        assert site_cards(bridge, "vie") == []
    finally:
        proc.kill()
        site.sock.close()
        reader.sock.close()
