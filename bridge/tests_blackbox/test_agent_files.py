"""Files from one agent to another of the same person (`send_files_to_agent`, bridge-rs/src/agent_files.rs): only along
a link the person allowed (`/v1/agent-links`), never another household's agent, never a file of this machine; the
files land in the recipient's uploads as an app upload would (its isolated sandbox's copy too), and the sender is told
where the recipient reads them. Isolated sandboxes (`[sandbox] isolate = "on"`), agents whose tools run here, and an
agent set up by hand in a container of its own."""
import hashlib
import json
import os

from bbkit.common import AUTH
from conftest import managed_install, profile, wait_registered
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


def stored(folder):
    """The files stored in an uploads folder (or a sandbox's volume): {name without its id: bytes}."""
    found = {}
    for month in sorted(p for p in folder.iterdir() if p.is_dir() and p.name[:2] == "20"):
        for file in month.iterdir():
            found[file.name[13:]] = file.read_bytes()
    return found


def test_isolated_agents_give_files_along_the_links_the_person_allowed(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("vie", "wellness", "budget"))
    started(bridge, hub, ["vie", "wellness", "budget"])
    give_tokens(bridge, ["vie", "wellness", "budget"])
    vie, wellness, budget = (TOKENS[n] for n in ("vie", "wellness", "budget"))
    assert TOOL not in names(bridge, vie)                     # it may write to nobody yet
    link(bridge, ("vie", "wellness"))
    # The agents' tools registered again (vie's list changed): their tokens are new, the test's given again.
    assert any(args[:2] == ["-p", "vie"] and args[4].startswith("mcp_servers.bipagents.url")
               for args in bridge.commands.of("hermes") if len(args) > 4)
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
    assert failed and "hasn't let you write to Wellness" in text and "Qui peut écrire à qui" in text
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
    hub = managed_install(bridge, fakes, agents=("vie", "budget"))
    vie_uploads = profile(hub, "vie") / "uploads"
    (vie_uploads / "2026-10").mkdir(parents=True)
    (vie_uploads / "2026-10" / "0123456789ab-releve.pdf").write_bytes(b"%PDF releve")
    bridge.config["agents"]["vie"].update(upload_dir_host=str(vie_uploads), upload_dir_container=str(vie_uploads))
    kerfy_uploads = bridge.root / "kerfy-uploads"
    bridge.config["agents"]["kerfy"] = {
        "display_name": "Kerfy Studio", "hermes_url": fakes.down.url, "hermes_key": "q" * 32, "shares": "external",
        "upload_dir_host": str(kerfy_uploads), "upload_dir_container": "/home/hermes/.hermes/profiles/kerfy/uploads"}
    bridge.start()
    wait_registered(bridge, ["vie"])
    link(bridge, ("vie", "kerfy"), ("kerfy", "vie"), ("kerfy", "budget"))
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
    # Kerfy's administrator gives it the bridge's tools (its token printed once, never stored in clear).
    out = bridge.cli("agent-tools", "--agent", "kerfy")
    assert out.returncode == 0 and "mcp_servers.bipagents.url" in out.stdout
    token = out.stdout.split("Authorization 'Bearer ")[1].split("'")[0]
    assert token not in (bridge.state_dir / "agent-tools.json").read_text()
    assert TOOL in names(bridge, token)
    assert "by itself" in bridge.cli("agent-tools", "--agent", "vie").stdout
