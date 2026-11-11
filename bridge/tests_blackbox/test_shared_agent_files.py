"""Files sent to a shared agent (shared_files.rs, Alex 11/11): Marie sends Alex's Cuisine a PDF and a photo. Her bridge
holds the document her app stored first, then takes both to Alex's bridge with her message: they are kept there, in
her conversation only (never in the agent's uploads). The agent sees her photo in her turn and reads her PDF with
`read_sent_file` — in her own turn only: not in Alex's, not in Paul's. Her timeline shows them, fetched through her
bridge; Paul can't. The same limits as files to one's own agents. When the share ends, or she stops using the agent,
her files go."""
import base64
import os
import threading

from bbkit.common import AUTH, sse
from bbkit.mcp import TOKEN, call, give_token, text_of
from test_shared_agents import link, wait

PHOTO = base64.b64decode("/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAP////////////////////////////////////////////////////////////"
                         "//////////////////////////2wBDAf//////////////////////////////////////////////////////////////"
                         "////////////////////////wAARCAABAAEDASIAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAAA//EABQQAQAAAAAAAAAA"
                         "AAAAAAAAAAD/xAAUAQEAAAAAAAAAAAAAAAAAAAAA/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AP//Z")


def tiny_pdf(text):
    """A one-page PDF whose words are `text` (Helvetica: any reader finds them)."""
    stream = f"BT /F1 14 Tf 72 760 Td ({text}) Tj ET"
    objs = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
            f"<< /Length {len(stream)} >>\nstream\n{stream}\nendstream",
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"]
    out, offsets = b"%PDF-1.4\n", []
    for i, o in enumerate(objs, 1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n{o}\nendobj\n".encode("latin-1")
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{off:010d} 00000 n \n".encode() for off in offsets)
    return out + f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()


REPORT = tiny_pdf("Bulletin du premier trimestre: Mathematiques 15,5")
SHARED = {"agent": "cuisine__alex"}


def join(a, c, name):
    assert c.put("/v1/family/name", headers=AUTH, json={"name": name}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    assert c.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]}).status_code == 200


def household(bridges, fakes, name, agents, limit_mb=None):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24}
                       for a in agents}
        if limit_mb is not None:
            c["limits"]["upload_max_mb"] = limit_mb
    bridge = bridges.new(name, change=change)
    bridge.config["public_url"] = bridge.url
    return bridge.start()


def family(bridges, fakes, host_limit_mb=None):
    """Alex shares Cuisine with Marie and Paul; both added it."""
    a = household(bridges, fakes, "alex", ["cuisine"], limit_mb=host_limit_mb)
    b = household(bridges, fakes, "marie", ["vie"])
    c = household(bridges, fakes, "paul", ["jardin"])
    link(a, b)
    join(a, c, "Paul")
    fakes.hermes.rounds = 1000
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["marie", "paul"]}).status_code == 200
    for x in (b, c):
        assert x.post("/v1/shared-agents/cuisine__alex/add", headers=AUTH).status_code == 200
    give_token(a, "cuisine", TOKEN)
    return a, b, c


def held(b, name="bulletin.pdf", data=REPORT, mime="application/pdf"):
    """Her app stores the document first, as for her own agents: her bridge holds it."""
    stored = b.post("/v1/files", headers=AUTH, data=SHARED, files={"file": (name, data, mime)})
    assert stored.status_code == 200, stored.text
    return stored.json()


def files_of(bridge, cid):
    folder = bridge.state_dir / "shared-files" / "cuisine" / cid
    return sorted(p.name for p in folder.iterdir()) if folder.is_dir() else []


def test_a_relatives_files_stay_in_her_conversation_and_are_read_in_her_turn_only(bridges, fakes):
    a, b, c = family(bridges, fakes)
    doc = held(b)
    assert doc["path"].startswith("/bipagents/held/") and doc["filename"] == "bulletin.pdf"
    holding = b.state_dir / "shared-held" / "cuisine__alex"
    assert [p.name.split("-", 1)[1] for p in holding.iterdir()] == ["bulletin.pdf"]

    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("message.delta", run_id="r1", delta="Je lis"),
                               sse("tool.started", run_id="r1", tool="mcp__bipagents__read_sent_file"), gate,
                               sse("run.completed", run_id="r1", output="Zoé a 15,5 en maths.")]
    message = {**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000B1", "text": "Voici le bulletin de Zoé",
               "attachments": [{"kind": "document", "name": "bulletin.pdf", "mime": "application/pdf",
                                "size": len(REPORT), "path": doc["path"]},
                               {"kind": "image", "name": "IMG_0042.jpg", "mime": "image/jpeg",
                                "data": base64.b64encode(PHOTO).decode()}]}
    sent = b.post("/v1/threads/new/messages", headers=AUTH, json=message)
    assert sent.status_code == 200, sent.text
    cid = sent.json()["thread"]
    assert sent.json()["run_id"] == "r1"

    # kept on Alex's place, in her conversation's folder (0700, files 0600), never in the agent's uploads
    kept = files_of(a, cid)
    assert sorted(n.split("-", 1)[1] for n in kept) == ["IMG_0042.jpg", "bulletin.pdf"]
    folder = a.state_dir / "shared-files" / "cuisine" / cid
    assert oct(os.stat(folder).st_mode & 0o777) == "0o700"
    assert {oct(os.stat(folder / n).st_mode & 0o777) for n in kept} == {"0o600"}
    assert (folder / next(n for n in kept if n.endswith("bulletin.pdf"))).read_bytes() == REPORT
    assert not list(holding.iterdir()), "the copy held on her side went once the message was taken"

    # her turn: her words framed, the PDF as one line the agent reads with the tool, the photo as an image part
    run = fakes.hermes.started["r1"]
    parts = run["input"][0]["content"]
    assert [p["type"] for p in parts] == ["text", "image_url"]
    assert parts[0]["text"].lstrip().startswith("[Marie, a relative of your person") or "[Marie, a relative" in parts[0]["text"]
    line = next(x for x in parts[0]["text"].splitlines() if x.startswith("[Pièce jointe : bulletin.pdf"))
    path = line.rsplit("→ ", 1)[1].rstrip("]")
    assert path.startswith(f"/proche/{cid}/") and path.endswith("-bulletin.pdf")
    assert parts[1]["image_url"]["url"] == "data:image/jpeg;base64," + base64.b64encode(PHOTO).decode()
    assert "read_sent_file" in run["instructions"]
    assert "mcp__bipagents__read_sent_file" in run["guest_turn"]["tools"]
    assert "read_sent_file" in {t["name"] for t in call_list(a)}

    # in her turn, the agent reads it
    read = call(a, "read_sent_file", {"path": path})
    assert read["isError"] is False, text_of(read)
    assert "Mathematiques 15,5" in text_of(read)
    photo_path = f"/proche/{cid}/" + next(n for n in kept if n.endswith("IMG_0042.jpg"))
    assert "came with their message" in text_of(call(a, "read_sent_file", {"path": photo_path}))
    for bad in ["/fichiers/x.pdf", f"/proche/{cid}/../agent-shares.json", f"/proche/{cid}/0123456789ab-autre.pdf"]:
        assert call(a, "read_sent_file", {"path": bad})["isError"] is True, bad
    # a second message while her turn goes on: refused, the file it carried taken back, still held on her side
    again = held(b, "fiche.pdf")
    refused = b.post(f"/v1/threads/{cid}/messages", headers=AUTH,
                     json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000B2", "text": "Et celle-ci",
                           "attachments": [{"kind": "document", "name": "fiche.pdf", "path": again["path"]}]})
    assert refused.status_code == 409, refused.text
    assert wait(lambda: files_of(a, cid) == kept)
    assert [p.name.split("-", 1)[1] for p in holding.iterdir()] == ["fiche.pdf"]

    # her turn ends with her message stored as Hermes keeps it: her photo committed as a file of the agent's profile
    # (`[Image attached at: …]`, the path as Hermes sees it), which the owner's own turns could reach
    session = run["session_id"]
    copy = a.home / ".hermes" / "profiles" / "cuisine" / "native-inputs" / "ab12" / "api_ab12.jpg"
    copy.parent.mkdir(parents=True)
    copy.write_bytes(PHOTO)
    seen = "/home/hermes/.hermes/profiles/cuisine/native-inputs/ab12/api_ab12.jpg"
    stored = [{"type": "text", "text": parts[0]["text"] + f"\n[Image attached at: {seen}]"}, parts[1]]
    fakes.hermes.messages[("cuisine", session)] = [{"role": "user", "content": stored},
                                                   {"role": "assistant", "content": "Zoé a 15,5 en maths."}]
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    page = lambda: b.get(f"/v1/threads/{cid}/timeline", headers=AUTH, params=SHARED).json()  # noqa: E731
    assert wait(lambda: page()["pending"]["run"] is None and page()["items"][-1]["role"] == "agent", timeout=15)
    assert wait(lambda: not copy.exists()), "Hermes' copy of her photo is still in the agent's profile"
    mine = page()["items"][0]
    assert mine["text"] == "Voici le bulletin de Zoé" and mine["client_id"] == message["client_id"]
    atts = {x["kind"]: x for x in mine["attachments"]}
    assert set(atts) == {"document", "image"}
    assert atts["document"]["name"] == "bulletin.pdf"
    for kind, data in [("document", REPORT), ("image", PHOTO)]:
        url = atts[kind]["url"]
        assert url.startswith(f"/v1/shared-agents/cuisine__alex/files/{cid}/"), url
        got = b.get(url, headers=AUTH)
        assert got.status_code == 200 and got.content == data, (kind, got.status_code)
        # Paul's bridge, his household's key: not his conversation
        assert c.get(url, headers=AUTH).status_code == 404

    # now Alex's own turn (no relative's going on): closed
    owner = call(a, "read_sent_file", {"path": path})
    assert owner["isError"] is True and "No relative is talking to you now" in text_of(owner)

    # Paul's own turn with Cuisine: Marie's file is not his to have read
    gate2 = threading.Event()
    fakes.hermes.runs["r2"] = [sse("tool.started", run_id="r2", tool="mcp__bipagents__read_sent_file"), gate2,
                               sse("run.completed", run_id="r2", output="Non.")]
    his = c.post("/v1/threads/new/messages", headers=AUTH,
                 json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000C1", "text": "Lis le bulletin de Zoé"})
    assert his.status_code == 200, his.text
    assert wait(lambda: "r2" in fakes.hermes.started)
    other = call(a, "read_sent_file", {"path": path})
    assert other["isError"] is True and "only the person who sent it" in text_of(other)
    gate2.set()
    fakes.hermes.started["r2"]["status"] = "completed"

    # the owner's app sees nothing of her files: who, when, how many messages
    view = a.get("/v1/agents/cuisine/shares", headers=AUTH).json()
    assert "files" not in str(view["conversations"])

    # Alex stops sharing with Marie: her conversation and its files go
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["paul"]}).status_code == 200
    assert files_of(a, cid) == [] and not folder.exists()


def call_list(bridge):
    from bbkit.mcp import rpc
    return rpc(bridge, "tools/list", token=TOKEN).json()["result"]["tools"]


def test_a_relatives_files_keep_the_hosts_limits_and_go_when_she_leaves(bridges, fakes):
    # Alex's place takes files of 20 KB at most here (`upload_max_mb`, as for his own agents)
    a, b, c = family(bridges, fakes, host_limit_mb=0.02)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Reçu.")]
    big = held(b, "scan.pdf", REPORT + b"%" + b"x" * 30_000)
    too_large = b.post("/v1/threads/new/messages", headers=AUTH,
                       json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D1", "text": "Le scan",
                             "attachments": [{"kind": "document", "name": "scan.pdf", "path": big["path"]}]})
    assert too_large.status_code == 413, too_large.text
    assert "r1" not in fakes.hermes.started
    # a held file the message did not take stays for a retry; one that isn't there any more is said plainly
    gone = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D2", "text": "Et ça",
                        "attachments": [{"kind": "document", "name": "x.pdf", "path": "/bipagents/held/0123456789ab-x.pdf"}]})
    assert gone.status_code == 422 and "no longer here" in gone.text
    # a file alone (no words) goes, its conversation titled with its name
    small = held(b, "certificat.pdf")
    sent = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D3", "text": "",
                        "attachments": [{"kind": "document", "name": "certificat.pdf", "path": small["path"]}]})
    assert sent.status_code == 200, sent.text
    cid = sent.json()["thread"]
    assert [n.split("-", 1)[1] for n in files_of(a, cid)] == ["certificat.pdf"]
    listed = b.get("/v1/threads", headers=AUTH, params=SHARED).json()["threads"]
    assert [t["title"] for t in listed if t["id"] == cid] == ["certificat.pdf"]
    # her household's key reaches only her conversations' files, not by guessing another one's
    assert c.get(f"/v1/shared-agents/cuisine__alex/files/{cid}/{files_of(a, cid)[0][:12]}", headers=AUTH).status_code == 404

    # « Ne plus l'utiliser »: her files with Cuisine go from Alex's place
    assert b.delete("/v1/shared-agents/cuisine__alex", headers=AUTH).status_code == 200
    assert wait(lambda: files_of(a, cid) == [])
    assert not (b.state_dir / "shared-held" / "cuisine__alex").exists()


def test_her_conversation_ended_takes_its_files(bridges, fakes):
    a, b, _ = family(bridges, fakes)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Reçu.")]
    sent = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000E1", "text": "La photo du frigo",
                        "attachments": [{"kind": "image", "name": "frigo.jpg", "mime": "image/jpeg",
                                         "data": base64.b64encode(PHOTO).decode()}]})
    assert sent.status_code == 200, sent.text
    cid = sent.json()["thread"]
    assert [n.split("-", 1)[1] for n in files_of(a, cid)] == ["frigo.jpg"]
    assert b.delete(f"/v1/threads/{cid}", headers=AUTH, params=SHARED).status_code == 200
    assert files_of(a, cid) == []
