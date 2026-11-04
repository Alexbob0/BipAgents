"""Files a reply only cites (« c'est dans /envoi/notes.md ») made openable (bridge-rs/src/cited_files.rs, Alex
2026-10-28): when a turn ends, each cited path that is a file of the agent's own folders is one of the end's
attachments (the apps show a file card; since 2026-11-03 the model's words are untouched); never a file outside its
folders, never through a link, never twice. A routine is a turn like any (routines.rs)."""
import os

from bbkit.common import AUTH, read_sse, sse
from bbkit.servers import wait_until
from test_sandbox import isolated, started


def follow(bridge, agent, run_id):
    with bridge.stream("GET", f"/v1/runs/{run_id}/events?agent={agent}&session_id=api_c", headers=AUTH) as resp:
        assert resp.status_code == 200
        return read_sse(resp)


def ended(bridge, fakes, agent, run_id, reply):
    fakes.hermes.runs[run_id] = [sse("message.delta", run_id=run_id, delta=reply),
                                 sse("run.completed", run_id=run_id, output=reply)]
    events = follow(bridge, agent, run_id)
    assert events[-1][0] == "run.completed"
    said = "".join(d.get("delta", "") for n, d in events if n == "message.delta")
    return events[-1][1], said


def test_a_cited_file_of_the_agents_own_folders_gets_a_card_and_nothing_else_does(bridges, fakes):
    bridge, hub, volumes = isolated(bridges, fakes, agents=("wellness",))
    started(bridge, hub, ["wellness"])
    envoi = volumes[("envoi", "wellness")]
    (envoi / "notes.md").write_text("# Notes\n- une\n- deux\n")
    (envoi / "deja.pdf").write_bytes(b"%PDF deja")
    (bridge.root / "secret.txt").write_text("secret")
    os.symlink(bridge.root / "secret.txt", envoi / "lien.txt")
    os.symlink(bridge.root, envoi / "dossier")
    (envoi / "auth.json").write_text("{}")
    outside = bridge.root / "secret.txt"
    # cited inside: an attachment of the end (a card), the model's words untouched (2026-11-03)
    reply = "C'est dans /envoi/notes.md, tu peux l'ouvrir."
    end, said = ended(bridge, fakes, "wellness", "run_in", reply)
    assert end["output"] == reply and [a["path"] for a in end["attachments"]] == ["/envoi/notes.md"]
    assert said == reply
    assert bridge.logged("files of the turn handed over as attachments")
    # then kept, and served to the app
    got = bridge.get("/v1/media", params={"path": "/envoi/notes.md", "agent": "wellness"}, headers=AUTH)
    assert got.status_code == 200 and got.text.startswith("# Notes")
    # outside its folders, through a link, a secret, a missing file: nothing
    for i, path in enumerate((str(outside), "/envoi/lien.txt", "/envoi/dossier/secret.txt", "/envoi/auth.json",
                              "/envoi/absent.md", "/etc/hosts.txt")):
        reply = f"Regarde {path}."
        end, said = ended(bridge, fakes, "wellness", f"run_out{i}", reply)
        assert end["output"] == reply and "attachments" not in end, path
    # already handed over with its MEDIA line: no second one
    reply = "Le devis est dans /envoi/deja.pdf\nMEDIA:/envoi/deja.pdf"
    end, said = ended(bridge, fakes, "wellness", "run_dup", reply)
    # its line the end's one attachment (4/11: no MEDIA line in what the apps get)
    assert end["output"] == "Le devis est dans /envoi/deja.pdf" and said == "Le devis est dans /envoi/deja.pdf\n"
    assert [a["path"] for a in end["attachments"]] == ["/envoi/deja.pdf"]
