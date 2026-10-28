"""Files a reply only cites (« c'est dans /envoi/notes.md ») made openable (bridge-rs/src/cited_files.rs, Alex
2026-10-28): when a turn ends, each cited path that is a file of the agent's own folders gets a `MEDIA:` line at the
end of the reply (the apps show a file card), the path's text kept; never a file outside its folders, never through a
link, never twice. A routine's reply too."""
import os
import time

import pytest

from bbkit.common import AUTH, read_sse, sse
from bbkit.servers import wait_until
from test_cronwatch import stamp, watching
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
    # cited inside: a card, the path's text kept, on the screen too (a last delta)
    reply = "C'est dans /envoi/notes.md, tu peux l'ouvrir."
    end, said = ended(bridge, fakes, "wellness", "run_in", reply)
    assert end["output"] == f"{reply}\nMEDIA:/envoi/notes.md" and end["bip_files_cited"] is True
    assert said == f"{reply}\n\nMEDIA:/envoi/notes.md"
    assert bridge.logged("a file the reply cites handed over (MEDIA line added)")
    # then kept, and served to the app
    got = bridge.get("/v1/media", params={"path": "/envoi/notes.md", "agent": "wellness"}, headers=AUTH)
    assert got.status_code == 200 and got.text.startswith("# Notes")
    # outside its folders, through a link, a secret, a missing file: nothing
    for i, path in enumerate((str(outside), "/envoi/lien.txt", "/envoi/dossier/secret.txt", "/envoi/auth.json",
                              "/envoi/absent.md", "/etc/hosts.txt")):
        reply = f"Regarde {path}."
        end, said = ended(bridge, fakes, "wellness", f"run_out{i}", reply)
        assert end["output"] == reply and "bip_files_cited" not in end, path
        assert "MEDIA:" not in said, path
    # already handed over with its MEDIA line: no second one
    reply = "Le devis est dans /envoi/deja.pdf\nMEDIA:/envoi/deja.pdf"
    end, said = ended(bridge, fakes, "wellness", "run_dup", reply)
    assert end["output"] == reply and "bip_files_cited" not in end
    assert said.count("MEDIA:") == 1


@pytest.mark.slow
def test_a_routines_reply_citing_a_file_gets_its_card(bridges, fakes):
    bridge = watching(bridges)
    uploads = bridge.root / "uploads" / "wellness" / "2026-10"
    uploads.mkdir(parents=True)
    (uploads / "0123456789ab-bilan.txt").write_text("Bilan de la semaine")
    seen = "/home/hermes/.hermes/profiles/wellness/uploads/2026-10/0123456789ab-bilan.txt"
    (bridge.root / "ailleurs.txt").write_text("non")
    reply = f"Ton bilan est prêt : `{seen}`. Pas {bridge.root / 'ailleurs.txt'}."
    session = f"cron_fc344ed09026_{stamp(600)}"
    fakes.hermes.add_session(session, [{"role": "user", "content": "Fais le bilan"},
                                       {"role": "assistant", "content": reply}], title="Bilan · Oct 28 07:00")
    bridge.start()
    assert wait_until(lambda: bridge.get("/v1/outbox", headers=AUTH).json()["items"], timeout=25)
    time.sleep(0.5)
    items = bridge.get("/v1/outbox", headers=AUTH).json()["items"]
    assert len(items) == 1
    # the path's text kept, one card at the end, none for the file outside its folders
    assert items[0]["text"] == f"{reply}\nMEDIA:{seen}"
    assert bridge.logged("a file the reply cites handed over (MEDIA line added)")
