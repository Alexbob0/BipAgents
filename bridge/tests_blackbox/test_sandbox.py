"""The agents' sandboxes isolated (bridge-rs/src/sandbox.rs, `[sandbox] isolate = "on"` since these tests run where
`auto` would leave them as they are): no folder of this machine mounted, the uploads copied into a podman volume, the
shared folders in podman volumes of their own (shares.rs) and through the file tools (`/v1/agent-tools/files`) with
their permission checks, the registries-only network, and the way back (`isolate = "off"`). podman is the fake one:
its volumes are folders of the test."""
import hashlib
import json
import os
import time
import tomllib

import yaml

from bbkit.common import AUTH
from bbkit.servers import wait_until
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
    for _ in range(100):
        data = yaml.safe_load((hub / "profiles" / name / "config.yaml").read_text())
        if isinstance(data, dict) and "mcp_servers" in data:
            return data.get("terminal", {})
        time.sleep(0.05)
    raise AssertionError(f"{name}'s config.yaml never read whole")


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
    # Each sandbox its own credentials on the proxy (sandbox_sites.rs): the proxy knows which agent asks.
    sites = json.loads((bridge.state_dir / "sandbox-sites.json").read_text())["agents"]
    token = sites["vie"]["token"]
    assert len(token) >= 24 and token != sites["wellness"]["token"]
    proxy = f"http://vie:{token}@10.233.80.2:3128"
    assert vie["docker_env"]["HTTPS_PROXY"] == proxy == vie["docker_env"]["https_proxy"]
    assert vie["docker_env"]["npm_config_https_proxy"] == proxy
    assert oct(os.stat(bridge.state_dir / "sandbox-sites.json").st_mode & 0o777) == "0o600"
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    # « Tous les sites » on by default: vie holds nothing private (no vault item, mailbox, health data, money)
    assert policy["agents"]["vie"] == {"key": hashlib.sha256(token.encode()).hexdigest(), "hosts": [], "any": True}
    assert token not in json.dumps(policy) and token not in bridge.log_tail(20000)
    assert vie["docker_env"]["NO_PROXY"] == "localhost,127.0.0.1"
    podman = bridge.commands.of("podman")
    assert ["network", "create", "--internal", "--subnet", "10.233.80.0/24", "bipagents-sandbox"] in podman
    run = next(args for args in podman if args[:1] == ["run"])
    assert run[run.index("--name") + 1] == "bipagents-egress" and "--replace" in run
    assert "bipagents-sandbox:ip=10.233.80.2" in run and "podman" in run and "--read-only" in run
    allow = next(a for a in run if a.startswith("BIP_ALLOW="))
    assert "pypi.org" in allow and "files.pythonhosted.org" in allow and "registry.npmjs.org" in allow
    assert "github.com" not in allow
    assert run[run.index("--entrypoint") + 1] == "python3" and "docker.io/nousresearch/hermes-sandbox:desktop" in run
    assert run[run.index("--user") + 1] == "0:0"
    assert f"{bridge.state_dir}/sandbox-proxy/policy:/bip/policy:ro,z" in run
    assert f"{bridge.state_dir}/sandbox-proxy/reports:/bip/reports:z" in run
    assert "BIP_POLICY=/bip/policy/sites.json" in run and "BIP_REPORTS=/bip/reports/blocked.jsonl" in run
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


def test_a_site_refused_to_an_agents_programs_asks_the_person(bridges, fakes):
    """The proxy refused data.gouv.fr to vie's programs (its report, as the proxy writes it): one card for the person,
    « Autoriser ces sites / Tous les sites pour Vie / Refuser », among the approvals; the agent can ask first (on the
    same card, a moment later); an older app's answer; the app's list."""
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    # « Tous les sites » is on by default for an agent with nothing private: vie's person turns it off.
    view = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert (view["all_sites"], view["all_sites_default"], view["sensitive"]) == (True, True, None)
    off = bridge.patch("/v1/agents/vie/sandbox/sites", json={"all_sites": False}, headers=AUTH)
    assert off.status_code == 200 and off.json()["all_sites"] is False, off.text
    reports = bridge.state_dir / "sandbox-proxy" / "reports" / "blocked.jsonl"
    assert reports.parent.is_dir()
    lines = [{"agent": "vie", "host": "data.gouv.fr"}, {"agent": "vie", "host": "data.gouv.fr"},
             {"agent": "vie", "host": "localhost"}, {"agent": "nobody", "host": "x.fr"}]
    reports.write_text("".join(json.dumps(l) + "\n" for l in lines))
    assert wait_until(lambda: len(site_cards(bridge, "vie")) == 1, timeout=10)
    assert wait_until(lambda: not reports.exists(), timeout=5)
    card = site_cards(bridge, "vie")[0]
    assert card["bip_site"] == "data.gouv.fr" and card["choices"] == ["always", "all_sites", "deny"]
    assert card["description"] == "Vie veut joindre data.gouv.fr depuis ses programmes"
    assert card["bip_site_via"] == "proxy" and card["session_id"] is None
    # The agent asks first for another one, with its reason; and lists what it has.
    give_tokens(bridge, ["vie", "wellness"])
    text, error = call(bridge, "vie", "sandbox_sites_request", host="https://api.meteo.fr/v1", reason="la pluie demain")
    assert not error and "api.meteo.fr" in text
    assert call(bridge, "vie", "sandbox_sites_request", host="pypi.org")[0] == "Your programs can reach it already."
    assert call(bridge, "vie", "sandbox_sites_request", host="10.0.0.1")[1] is True
    # one card, not two: the sites asked a moment apart gather
    [card] = site_cards(bridge, "vie")
    assert card["bip_sites"] == ["data.gouv.fr", "api.meteo.fr"] and card["bip_site"] == "data.gouv.fr"
    assert card["description"] == "Vie veut joindre 2 sites depuis ses programmes : data.gouv.fr, api.meteo.fr"
    assert card["bip_site_reason"] == "la pluie demain" and card["command"] == "data.gouv.fr, api.meteo.fr"
    assert site_cards(bridge, "wellness") == []
    # An older app (« Autoriser toujours », the first site shown): that one; the other waits on a card of its own.
    answered = bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{card['request_id']}", json={"allow": True},
                           headers=AUTH)
    assert answered.status_code == 200, answered.text
    assert [s["host"] for s in answered.json()["sites"]] == ["data.gouv.fr"]
    assert answered.json()["sites"][0]["by"] == "request"
    [left] = site_cards(bridge, "vie")
    assert left["bip_sites"] == ["api.meteo.fr"] and left["request_id"] != card["request_id"]
    # « Refuser » from a notification
    denied = bridge.post("/v1/approve", json={"agent": "vie", "run_id": left["run_id"], "choice": "deny"}, headers=AUTH)
    assert denied.status_code == 200, denied.text
    assert site_cards(bridge, "vie") == []
    assert bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{left['request_id']}",
                       json={"allow": True}, headers=AUTH).status_code == 404
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    assert policy["agents"]["vie"]["hosts"] == ["data.gouv.fr"] and "any" not in policy["agents"]["vie"]
    assert policy["agents"]["wellness"]["hosts"] == [] and policy["agents"]["wellness"]["any"] is True
    kept = bridge.get("/v1/approvals/answered", params={"agent": "vie"}, headers=AUTH).json()["items"]
    assert [(k["command"], k["choice"], k["topic"]) for k in kept] == [
        ("data.gouv.fr", "always", "site"), ("api.meteo.fr", "deny", "site")]
    # Refused: the proxy's next refusal asks nothing; the agent's own request neither, today.
    reports.write_text(json.dumps({"agent": "vie", "host": "api.meteo.fr"}) + "\n")
    assert wait_until(lambda: not reports.exists(), timeout=10)
    assert site_cards(bridge, "vie") == []
    assert call(bridge, "vie", "sandbox_sites_request", host="api.meteo.fr")[1] is True
    # The app's list: the registries always open, the sites with their date, one added by hand, one removed.
    view = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert "pypi.org" in view["registries"] and view["effective"] is True and view["why"] is None
    assert bridge.post("/v1/agents/vie/sandbox/sites", json={"host": "*.ouraring.com"}, headers=AUTH).status_code == 200
    for bad in ("localhost", "*.com", "192.168.1.1", "nas.local"):
        assert bridge.post("/v1/agents/vie/sandbox/sites", json={"host": bad}, headers=AUTH).status_code == 422, bad
    assert bridge.delete("/v1/agents/vie/sandbox/sites/data.gouv.fr", headers=AUTH).status_code == 200
    assert bridge.delete("/v1/agents/vie/sandbox/sites/data.gouv.fr", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents/nobody/sandbox/sites", headers=AUTH).status_code == 404
    assert bridge.get("/v1/agents/vie/sandbox/sites").status_code == 401
    listed = call(bridge, "vie", "sandbox_sites")[0]
    assert "*.ouraring.com" in listed and "data.gouv.fr" not in listed and "pypi.org" in listed
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    assert policy["agents"]["vie"]["hosts"] == ["*.ouraring.com"]


def test_sites_asked_together_one_card_three_choices_and_all_sites(bridges, fakes):
    """Three sites refused together: one card; « Autoriser ces sites » allows the three; « Tous les sites pour Vie »
    (from a notification) opens every public site with no card any more. An agent with private access (its health
    switch) starts with « Tous les sites » off, and its programs ask."""
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    assert bridge.patch("/v1/agents/vie/sandbox/sites", json={"all_sites": False}, headers=AUTH).status_code == 200
    reports = bridge.state_dir / "sandbox-proxy" / "reports" / "blocked.jsonl"
    reports.write_text("".join(json.dumps({"agent": "vie", "host": h}) + "\n" for h in ("a.fr", "b.fr", "c.fr")))
    assert wait_until(lambda: len(site_cards(bridge, "vie")) == 1 and len(site_cards(bridge, "vie")[0]["bip_sites"]) == 3,
                      timeout=10), site_cards(bridge, "vie")
    [card] = site_cards(bridge, "vie")
    view = bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()
    assert [p["hosts"] for p in view["pending"]] == [["a.fr", "b.fr", "c.fr"]]
    answered = bridge.post(f"/v1/agents/vie/sandbox/sites/requests/{card['request_id']}",
                           json={"allow": True, "hosts": ["a.fr", "b.fr", "c.fr"]}, headers=AUTH)
    assert answered.status_code == 200, answered.text
    assert [s["host"] for s in answered.json()["sites"]] == ["a.fr", "b.fr", "c.fr"]
    assert site_cards(bridge, "vie") == []
    kept = bridge.get("/v1/approvals/answered", params={"agent": "vie"}, headers=AUTH).json()["items"]
    assert [(k["command"], k["choice"]) for k in kept] == [("a.fr, b.fr, c.fr", "always")]
    # « Tous les sites pour Vie »
    reports.write_text(json.dumps({"agent": "vie", "host": "d.fr"}) + "\n")
    assert wait_until(lambda: len(site_cards(bridge, "vie")) == 1, timeout=10)
    run_id = site_cards(bridge, "vie")[0]["run_id"]
    opened = bridge.post("/v1/approve", json={"agent": "vie", "run_id": run_id, "choice": "all_sites"}, headers=AUTH)
    assert opened.status_code == 200 and opened.json()["choice"] == "all_sites", opened.text
    assert bridge.get("/v1/agents/vie/sandbox/sites", headers=AUTH).json()["all_sites"] is True
    policy = json.loads((bridge.state_dir / "sandbox-proxy" / "policy" / "sites.json").read_text())
    assert policy["agents"]["vie"]["any"] is True
    reports.write_text(json.dumps({"agent": "vie", "host": "e.fr"}) + "\n")
    assert wait_until(lambda: not reports.exists(), timeout=10)
    assert site_cards(bridge, "vie") == []
    give_tokens(bridge, ["vie", "wellness"])
    assert call(bridge, "vie", "sandbox_sites_request", host="f.fr")[0] == "Your programs can reach it already."
    assert "every public HTTPS site" in call(bridge, "vie", "sandbox_sites")[0]
    # wellness reads health data: off by default, its programs ask
    assert bridge.patch("/v1/agents/wellness/settings", json={"health": True}, headers=AUTH).status_code == 200
    view = bridge.get("/v1/agents/wellness/sandbox/sites", headers=AUTH).json()
    assert (view["all_sites"], view["all_sites_default"], view["sensitive"]) == (False, False, "health")
    reports.write_text(json.dumps({"agent": "wellness", "host": "g.fr"}) + "\n")
    assert wait_until(lambda: len(site_cards(bridge, "wellness")) == 1, timeout=10)
    # its person may still turn it on
    on = bridge.patch("/v1/agents/wellness/sandbox/sites", json={"all_sites": True}, headers=AUTH).json()
    assert on["all_sites"] is True and on["pending"] == [] and site_cards(bridge, "wellness") == []


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
