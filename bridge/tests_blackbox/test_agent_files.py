"""Files from one agent to another of the same person (`send_files_to_agent`, bridge-rs/src/agent_files.rs): only along
a link the person allowed (`/v1/agent-links`), never another household's agent, never a file of this machine; the
files land in the recipient's uploads as an app upload would (its isolated sandbox's copy too), and the sender is told
where the recipient reads them. Isolated sandboxes (`[sandbox] isolate = "on"`), agents whose tools run here, and an
agent set up by hand in a container of its own."""
import hashlib
import json
import os
import tomllib
from concurrent.futures import ThreadPoolExecutor

import yaml

from bbkit.common import AUTH
from bbkit.servers import wait_until
from conftest import managed_install, profile, servers, wait_registered
from test_sandbox import TOKENS, give_tokens, isolated, started

MCP = "/v1/agent-tools/mcp"
TOOL = "send_files_to_agent"


def rpc(bridge, token, method, params=None):
    body = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params or {}}
    return bridge.post(MCP, json=body, headers={"Authorization": f"Bearer {token}"}).json()


def send(bridge, token, **arguments):
    result = rpc(bridge, token, "tools/call", {"name": TOOL, "arguments": arguments})["result"]
    return result["content"][0]["text"], result["isError"]


def names(bridge, token):
    return [t["name"] for t in rpc(bridge, token, "tools/list")["result"]["tools"]]


def link(bridge, *pairs):
    links = [{"from": a, "to": b} for a, b in pairs]
    assert bridge.put("/v1/agent-links", headers=AUTH, json={"links": links}).status_code == 200


def own_tokens(bridge, tokens):
    """The agents' bridge tokens as the bridge keeps them (hashed), for tokens the test knows."""
    path = bridge.state_dir / "agent-tools.json"
    kept = json.loads(path.read_text())["tokens"] if path.exists() else {}
    kept.update({n: hashlib.sha256(t.encode()).hexdigest() for n, t in tokens.items()})
    path.write_text(json.dumps({"tokens": kept}))


def given_paths(text):
    return [line.split(": ", 1)[1] for line in text.splitlines() if line.startswith("- ")]


def stored(folder):
    """The files stored in an uploads folder (or a sandbox's volume): {name without its id: bytes}."""
    found = {}
    for month in sorted(p for p in folder.iterdir() if p.is_dir() and p.name[:2] == "20"):
        for file in month.iterdir():
            found[file.name[13:]] = file.read_bytes()
    return found


def test_isolated_agents_give_files_along_the_links_the_person_allowed(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness", "budget"))
    # each case of send_files_to_agent below, with no run going on: a routine past 10 calls of one tool for the brake
    bridge.config["brake"] = {"enabled": False}
    started(bridge, hub, ["vie", "wellness", "budget"])
    give_tokens(bridge, ["vie", "wellness", "budget"])
    vie, wellness, budget = (TOKENS[n] for n in ("vie", "wellness", "budget"))
    assert TOOL not in names(bridge, vie)                     # it may write to nobody yet
    before = servers(profile(hub, "vie"))["bipagents"]["url"]
    link(bridge, ("vie", "wellness"))
    # The agents' tools registered again (vie's list changed: a new tools URL), the test's tokens given again.
    assert servers(profile(hub, "vie"))["bipagents"]["url"] != before
    give_tokens(bridge, ["vie", "wellness", "budget"])
    assert TOOL in names(bridge, vie) and TOOL not in names(bridge, budget) and TOOL not in names(bridge, wellness)
    # A file it made (in /envoi), one it was sent, and a short text.
    (volumes[("envoi", "vie")] / "devis.pdf").write_bytes(b"%PDF devis")
    text, failed = send(bridge, vie, agent="bip/wellness",
                        files=["/envoi/devis.pdf", "/fichiers/2026-10/0123456789ab-facture.pdf"],
                        text="Trois points à voir.", text_name="resume.md")
    assert not failed, text
    assert text.startswith("Given to Wellness: 3 file(s).") and "message_agent (target bip/wellness)" in text
    given = [line.split(": ", 1)[1] for line in text.splitlines() if line.startswith("- ")]
    assert len(given) == 3 and all(p.startswith("/fichiers/20") for p in given)
    assert given[0].endswith("-devis.pdf") and given[1].endswith("-facture.pdf") and given[2].endswith("-resume.md")
    uploads = profile(hub, "wellness") / "uploads"
    expected = {"facture.pdf": b"%PDF facture", "devis.pdf": b"%PDF devis", "resume.md": "Trois points à voir.".encode()}
    assert stored(uploads) == expected
    assert {k: v for k, v in stored(volumes[("files", "wellness")]).items()} == expected   # its sandbox's copy
    for path in given:                                                                      # where it was told
        assert (volumes[("files", "wellness")] / path.removeprefix("/fichiers/")).exists()
    assert "files given to another agent" in bridge.log_tail(4000)
    # By its display name too; the person's link is checked each time.
    assert send(bridge, vie, agent="Wellness", text="ok")[1] is False
    text, failed = send(bridge, budget, agent="wellness", text="x")
    assert failed and "hasn't let you write to Wellness" in text and "« Who can write to whom »" in text  # the app's label in the agent's language (English: no language set)
    text, failed = send(bridge, wellness, agent="vie", text="x")
    assert failed and "hasn't let you" in text
    # Never a file of this machine: outside its folders, through a link, in a sub-folder of /envoi, up the tree.
    os.symlink("/etc/hosts", volumes[("envoi", "vie")] / "hosts")
    (volumes[("envoi", "vie")] / "sub").mkdir()
    (volumes[("envoi", "vie")] / "sub" / "x.md").write_text("x")
    os.mkfifo(volumes[("envoi", "vie")] / "pipe")
    for path in ("/envoi/hosts", "/etc/hosts", "/envoi/sub/x.md", "/envoi/pipe", "/envoi/../../sandbox.json",
                 "/fichiers/../envoi-vie/devis.pdf", "/partage/x", "/envoi/absent.pdf"):
        text, failed = send(bridge, vie, agent="wellness", files=[path])
        assert failed and "copy it right into /envoi" in text, (path, text)
    # All or nothing: one file it can't reach, none given.
    before = stored(uploads)
    assert send(bridge, vie, agent="wellness", files=["/envoi/devis.pdf", "/etc/hosts"])[1] is True
    assert stored(uploads) == before
    # Not to itself, nobody unknown, never another household's agent; something to give.
    assert send(bridge, vie, agent="vie", text="x") == ("That is you: name another agent.", True)
    text, failed = send(bridge, vie, agent="personne", text="x")
    assert failed and "You may give files to: bip/wellness." in text
    text, failed = send(bridge, vie, agent="bip/cuisine__lea", text="x")
    assert failed and "never to another household's" in text
    assert send(bridge, vie, agent="wellness")[1] is True


def test_agents_whose_tools_run_here_and_an_agent_in_its_own_container(bridges, fakes):
    """Vie in the hub, its tools on this machine (its uploads at the same path); Kerfy set up by hand in a container
    of its own: its uploads mounted where the bridge was told, its outbox listed by `shares-mounts`."""
    bridge = bridges.new()
    bridge.config["agents"].pop("Wellness", None)
    hub = managed_install(bridge, fakes, agents=("vie", "budget", "cuisine"))
    cuisine_uploads = profile(hub, "cuisine") / "uploads"
    (profile(hub, "cuisine") / "config.yaml").write_text(yaml.safe_dump({"terminal": {"backend": "local"}}))
    bridge.config["agents"]["cuisine"].update(upload_dir_host=str(cuisine_uploads), upload_dir_container="/fichiers")
    vie_uploads = profile(hub, "vie") / "uploads"
    (vie_uploads / "2026-10").mkdir(parents=True)
    (vie_uploads / "2026-10" / "0123456789ab-releve.pdf").write_bytes(b"%PDF releve")
    bridge.config["agents"]["vie"].update(upload_dir_host=str(vie_uploads), upload_dir_container=str(vie_uploads))
    kerfy_uploads = bridge.root / "kerfy-uploads"
    bridge.config["agents"]["kerfy"] = {
        "display_name": "Kerfy Studio", "hermes_url": fakes.down.url, "hermes_key": "q" * 32, "shares": "external",
        "upload_dir_host": str(kerfy_uploads), "upload_dir_container": "/home/hermes/.hermes/profiles/kerfy/uploads"}
    bridge.start()
    wait_registered(bridge, ["vie", "cuisine"])
    link(bridge, ("vie", "kerfy"), ("kerfy", "vie"), ("kerfy", "budget"), ("kerfy", "cuisine"))
    tokens = {"vie": "vie-token-0123456789abcdefghijklmnop", "kerfy": "kerfy-token-0123456789abcdefghijklm"}
    path = bridge.state_dir / "agent-tools.json"
    kept = json.loads(path.read_text())["tokens"]
    kept.update({n: hashlib.sha256(t.encode()).hexdigest() for n, t in tokens.items()})
    path.write_text(json.dumps({"tokens": kept}))
    # Vie gives a file it was sent: Kerfy reads it where its container mounts its uploads.
    text, failed = send(bridge, tokens["vie"], agent="bip/kerfy", files=[f"{vie_uploads}/2026-10/0123456789ab-releve.pdf"])
    assert not failed, text
    assert "Kerfy Studio finds them here" in text and "/home/hermes/.hermes/profiles/kerfy/uploads/20" in text
    assert stored(kerfy_uploads) == {"releve.pdf": b"%PDF releve"}
    # A file Vie made elsewhere on this machine is not given (only its received files, its audio, or a text).
    text, failed = send(bridge, tokens["vie"], agent="kerfy", files=["/etc/hosts"])
    assert failed and "Only files you were sent" in text
    # Kerfy gives what it put in its outbox (the folder `shares-mounts` lists, mounted at /envoi).
    out = bridge.cli("shares-mounts", "--agent", "kerfy")
    outbox = bridge.state_dir / "envoi" / "kerfy"
    assert out.returncode == 0 and f"{outbox}:/envoi:z" in out.stdout
    (outbox / "plan.pdf").write_bytes(b"%PDF plan")
    text, failed = send(bridge, tokens["kerfy"], agent="vie", files=["/envoi/plan.pdf"])
    assert not failed and f"{vie_uploads}/20" in text
    assert stored(vie_uploads)["plan.pdf"] == b"%PDF plan"
    text, failed = send(bridge, tokens["kerfy"], agent="vie", files=["/home/hermes/workspace/plan.pdf"])
    assert failed and "copy it right into /envoi" in text
    # An agent that can't receive files: said plainly, for the person.
    text, failed = send(bridge, tokens["kerfy"], agent="budget", text="x")
    assert failed and "Budget can't receive files" in text and "Tell your person simply" in text
    # Cuisine's tools on this machine (Hermes' local backend) though the bridge still names its files as a sandbox's
    # (/fichiers, left from when it had one): given where it really reads them, its folder here (token lab 2026-11-07).
    text, failed = send(bridge, tokens["kerfy"], agent="cuisine", text="Trois points.", text_name="b.md")
    assert not failed and given_paths(text)[0].startswith(f"{cuisine_uploads}/20"), text
    assert "/fichiers" not in text and stored(cuisine_uploads)["b.md"] == b"Trois points."
    # Its file tools turned off (the app's settings): nothing given, and the sender is told why.
    (profile(hub, "cuisine") / "config.yaml").write_text(yaml.safe_dump(
        {"terminal": {"backend": "local"}, "platform_toolsets": {"api_server": ["web", "memory", "vision"]}}))
    text, failed = send(bridge, tokens["kerfy"], agent="cuisine", text="x")
    assert failed and "its file tools are turned off" in text and "Nothing was given" in text, text
    assert list(stored(cuisine_uploads)) == ["b.md"]
    # Kerfy's administrator gives it the bridge's tools (its token printed once, never stored in clear).
    out = bridge.cli("agent-tools", "--agent", "kerfy")
    assert out.returncode == 0 and "mcp_servers.bipagents.url" in out.stdout
    token = out.stdout.split("Authorization 'Bearer ")[1].split("'")[0]
    assert token not in (bridge.state_dir / "agent-tools.json").read_text()
    assert TOOL in names(bridge, token)
    assert "by itself" in bridge.cli("agent-tools", "--agent", "vie").stdout


def sandbox_container(bridge, hub, name, container, mounts):
    """A running Hermes sandbox of `name`'s, as `podman ps` / `container inspect` show it: its home under its profile
    (so the bridge knows it as that agent's own) and the given mounts."""
    home = profile(hub, name) / "sandboxes" / "docker" / "task" / "home"
    home.mkdir(parents=True, exist_ok=True)
    inspected = [{"Id": container, "Config": {"Labels": {"hermes-agent": "1", "hermes-profile": name}},
                  "State": {"Running": True},
                  "Mounts": [{"Destination": "/root", "Source": str(home)}] + [{"Destination": m, "Source": "/x"}
                                                                               for m in mounts]}]
    bridge.commands.rule("podman", ["container", "inspect", container], stdout=json.dumps(inspected))
    return {"Id": container}


def test_a_running_sandbox_without_its_received_files_gets_the_file_at_once(bridges, fakes):
    """Wellness' sandbox started before its profile mounted /fichiers (token lab of 2026-11-07, the owner's report the
    same morning: « ni le PDF ni le dossier /fichiers ne sont visibles de mon côté »): Hermes keeps that sandbox while
    it is used, so the file is copied into it at the path the sender is told, and its gateway restarted once idle so
    its next sandbox mounts the folder. A sandbox that mounts it gets nothing more; a copy that fails is said plainly."""
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness"))
    listed = [sandbox_container(bridge, hub, "wellness", "c0ffee01", ["/envoi"]),
              sandbox_container(bridge, hub, "wellness", "c0ffee02", ["/fichiers", "/envoi"])]
    bridge.commands.rule("podman", ["ps", "-a", "--filter", "label=hermes-agent=1", "--filter",
                                    "label=hermes-profile=wellness"], stdout=json.dumps(listed))
    started(bridge, hub, ["vie", "wellness"])
    link(bridge, ("vie", "wellness"))
    give_tokens(bridge, ["vie", "wellness"])
    (volumes[("envoi", "vie")] / "analyses.pdf").write_bytes(b"%PDF analyses fictives")
    text, failed = send(bridge, TOKENS["vie"], agent="bip/wellness", files=["/envoi/analyses.pdf"])
    assert not failed, text
    [path] = given_paths(text)
    assert path.startswith("/fichiers/20") and path.endswith("-analyses.pdf")
    assert (volumes[("files", "wellness")] / path.removeprefix("/fichiers/")).read_bytes() == b"%PDF analyses fictives"
    month = path.rsplit("/", 1)[0]
    podman = bridge.commands.of("podman")
    assert ["exec", "-u", "0", "c0ffee01", "mkdir", "-p", month] in podman
    copies = [argv for argv in podman if argv[:1] == ["cp"]]
    host = profile(hub, "wellness") / "uploads" / path.removeprefix("/fichiers/")
    assert copies == [["cp", str(host), f"c0ffee01:{path}"]], copies            # never into the one that mounts it
    assert ["exec", "-u", "0", "c0ffee01", "chmod", "0644", path] in podman
    log = bridge.log_tail(6000)
    assert "doesn't mount its received files" in log and "restart requested" in log
    # The copy fails: the files stay in its « Fichiers », and the sender is told it can't open them yet.
    bridge.commands.rule("podman", ["cp"], exit=125, stderr="Error: no such container")
    text, failed = send(bridge, TOKENS["vie"], agent="wellness", text="Trois points.", text_name="resume.md")
    assert failed and "Wellness can't open them right now" in text and "/fichiers/" not in text, text
    assert "resume.md" in stored(profile(hub, "wellness") / "uploads")


def test_an_agent_that_moved_into_its_own_space_is_given_files_where_its_sandbox_reads_them(bridges, fakes):
    """Coach moves into its own space (its first secret, in a hub that also hosts a project agent): its sandbox there
    mounts its files' volume at /fichiers at once, and what Quotidien gives it is named by that path. Until
    2026-11-07 the bridge named them by its space's folder, which its sandbox doesn't mount, until its next start."""
    import test_own_space as own
    bridge, hub = own.hub_place(bridges, fakes, agents=("coach", "quotidien"), project="fete")
    volumes = {}
    for name in ("coach", "quotidien"):
        uploads = profile(hub, name) / "uploads"
        uploads.mkdir(parents=True, exist_ok=True)
        bridge.config["agents"][name].update(upload_dir_host=str(uploads), upload_dir_container=str(uploads))
        for kind in ("files", "envoi", "share"):
            volume = bridge.root / "volumes" / f"{kind}-{name}"
            volume.mkdir(parents=True)
            volumes[(kind, name)] = volume
            bridge.commands.rule("podman", ["volume", "inspect", "--format", "{{.Mountpoint}}",
                                            f"bipagents-{kind}-{name}"], stdout=f"{volume}\n")
    bridge.commands.rule("podman", ["network", "exists"], exit=1)
    bridge.start()
    wait_registered(bridge, ["coach", "quotidien", "fete"])
    own.use_token_of(hub)
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(own.tool, bridge, "ask_secret", own.ASK)
        assert wait_until(lambda: own.secret_cards(bridge), timeout=60), bridge.log_tail(4000)
        card = own.secret_cards(bridge)[0]
        assert bridge.post(f"/v1/vault/secrets/requests/{card['request_id']}", headers=AUTH,
                           json={"allow": False}).status_code == 200
        call.result(timeout=60)
    root = bridge.root / "spaces" / "coach"
    section = tomllib.loads(bridge.toml_text())["agents"]["coach"]
    assert section["space"] == str(root) and section["upload_dir_container"] == "/fichiers", section
    terminal = yaml.safe_load((root / "profiles" / "coach" / "config.yaml").read_text())["terminal"]
    assert terminal["backend"] == "docker" and "bipagents-files-coach:/fichiers:ro" in terminal["docker_volumes"]
    link(bridge, ("quotidien", "coach"))
    token = "quotidien-token-0123456789abcdefghij"
    own_tokens(bridge, {"quotidien": token})
    (volumes[("envoi", "quotidien")] / "analyses.pdf").write_bytes(b"%PDF analyses fictives")
    text, failed = send(bridge, token, agent="bip/coach", files=["/envoi/analyses.pdf"])
    assert not failed, text
    [path] = given_paths(text)
    assert path.startswith("/fichiers/20") and str(root) not in text, text
    assert (volumes[("files", "coach")] / path.removeprefix("/fichiers/")).read_bytes() == b"%PDF analyses fictives"
