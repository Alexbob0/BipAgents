"""The agents' sandboxes isolated (bridge-rs/src/sandbox.rs, `[sandbox] isolate = "on"` since these tests run where
`auto` would leave them as they are): no folder of this machine mounted, the uploads copied into a podman volume, the
shared folders through the file tools (`/v1/agent-tools/files`) and their permission checks, the registries-only
network. podman is the fake one: its volumes are folders of the test."""
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
        for kind in ("files", "envoi"):
            volume = bridge.root / "volumes" / f"{kind}-{name}"
            volume.mkdir(parents=True)
            volumes[(kind, name)] = volume
            bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}",
                                            f"bipagents-{kind}-{name}"], stdout=f"{volume}\n")
    bridge.commands.rule("podman", ["network", "exists"], exit=1)
    if not proxy_ok:
        bridge.commands.rule("podman", ["run"], exit=125, stderr="Error: image not known")
    return bridge, hub, volumes


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
    assert vie["docker_env"]["HTTPS_PROXY"] == "http://10.233.80.2:3128" == vie["docker_env"]["https_proxy"]
    assert vie["docker_env"]["npm_config_https_proxy"] == "http://10.233.80.2:3128"
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


def test_the_file_tools_follow_the_share_links(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness", "budget"))
    started(bridge, hub, ["vie", "wellness", "budget"])
    bridge.commands.clear()
    reads = bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": [{"reader": "vie", "source": "wellness"}]})
    assert reads.json() == {"reads": [{"reader": "vie", "source": "wellness"}]}
    # Nothing mounted: the links are checked by the tools.
    assert not any(a[4:5] == ["terminal.docker_volumes"] for a in bridge.commands.of("hermes"))
    skill = (hub / "profiles" / "vie" / "skills" / "shared-data" / "SKILL.md").read_text()
    assert "shared_files_get" in skill and "/lecture" not in skill
    give_tokens(bridge, ["vie", "wellness", "budget"])
    listed = rpc(bridge, "vie", "tools/list").json()["result"]["tools"]
    hints = {t["name"]: t["annotations"]["readOnlyHint"] for t in listed}
    assert hints == {"shared_files_list": True, "shared_files_get": True, "shared_files_write": False,
                     "shared_files_delete": False}
    assert bridge.post(FILES_PATH, json={"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
                       headers={"Authorization": "Bearer nope"}).status_code == 401
    # Wellness shares a text and a file it made (from /envoi).
    assert call(bridge, "wellness", "shared_files_write", path="resume/semaine.md", content="7 h de sommeil") == (
        "In your shared folder: resume/semaine.md", False)
    (volumes[("envoi", "wellness")] / "graphe.png").write_bytes(b"PNG")
    assert call(bridge, "wellness", "shared_files_write", path="graphe.png", **{"from": "/envoi/graphe.png"})[1] is False
    share = bridge.state_dir / "shares" / "wellness"
    assert (share / "resume" / "semaine.md").read_text() == "7 h de sommeil" and (share / "graphe.png").read_bytes() == b"PNG"
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
    assert not (share / "h").exists()
    # Vie reads it (a copy in its own volume), budget can't.
    text, failed = call(bridge, "vie", "shared_files_list")
    assert not failed and "Shared with you by Wellness (agent `wellness`)" in text and "resume/semaine.md" in text
    assert call(bridge, "vie", "shared_files_get", agent="wellness", path="resume/semaine.md") == (
        "Copied to /fichiers/partages/wellness/resume/semaine.md (read-only).", False)
    copy = volumes[("files", "vie")] / "partages" / "wellness" / "resume" / "semaine.md"
    assert copy.read_text() == "7 h de sommeil"
    text, failed = call(bridge, "budget", "shared_files_get", agent="wellness", path="resume/semaine.md")
    assert failed and "shares nothing with you" in text
    assert "wellness" not in call(bridge, "budget", "shared_files_list")[0].split("Your shared folder")[1]
    # Writes and deletes only in one's own folder.
    assert call(bridge, "vie", "shared_files_delete", path="resume/semaine.md")[1] is True     # not in vie's folder
    assert (share / "resume" / "semaine.md").exists()
    assert call(bridge, "wellness", "shared_files_delete", path="resume") == (
        "Deleted from your shared folder: resume", False)
    assert not (share / "resume").exists() and (share / "graphe.png").exists()
    # The link taken back: vie's copies go, and it can't fetch any more.
    bridge.put("/v1/agent-shares", headers=AUTH, json={"reads": []})
    assert not (volumes[("files", "vie")] / "partages" / "wellness").exists()
    assert call(bridge, "vie", "shared_files_get", agent="wellness", path="graphe.png")[1] is True
    assert not (hub / "profiles" / "vie" / "skills" / "shared-data").exists()


def test_a_deleted_agent_loses_its_volumes(bridges, fakes):
    bridge, hub, _ = isolated(bridges, fakes, agents=("vie", "wellness"))
    started(bridge, hub, ["vie", "wellness"])
    bridge.commands.clear()
    assert bridge.delete("/v1/agents/wellness", headers=AUTH).status_code == 204
    podman = bridge.commands.of("podman")
    assert ["volume", "rm", "-f", "bipagents-files-wellness"] in podman
    assert ["volume", "rm", "-f", "bipagents-envoi-wellness"] in podman
    assert "wellness" not in json.loads((bridge.state_dir / "sandbox.json").read_text())["agents"]


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
