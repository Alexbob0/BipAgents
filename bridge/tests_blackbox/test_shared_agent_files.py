"""Files sent to a shared agent (shared_files.rs, Alex 11/11-12/11): ONE rule for every agent more than one person
uses. Marie sends Alex's Cuisine a PDF and a photo; Paul and Alex use it too. Her bridge holds the document her app
stored first, then takes both to Alex's bridge with her message, into Cuisine's pool on Alex's place (never the agent's
uploads). Everyone who uses Cuisine sees them, with who sent them (her app, Paul's through his bridge, Alex's), the
agent reads them in anyone's turn (Marie's, Paul's, Alex's own) with `read_sent_file` — a document's words, a photo,
the list —, never another household's; Alex's own files to it join the pool. Only their sender deletes them; they go
when the agent stops being shared with her (or anyone), when she stops using it; a conversation deleted keeps them.
The same limits as files to one's own agents; Word, Excel and a scan's text recognised on the phone read too."""
import base64
import os
import threading
import zipfile
from io import BytesIO

import pytest

from bbkit.common import AUTH, sse
from bbkit.mcp import TOKEN, call, give_token, rpc, text_of
from test_shared_agents import link, wait

# A personal agent shared with relatives is read-only since 12/11 (agent_feed.rs): relatives send it no file, and the
# pool is the family agent's only. These journeys move to the family agent with its common conversation
# (family-common-thread); `tiny_pdf` stays for serve_journeys_people.py.
pytestmark = pytest.mark.skip(reason="personal shares read-only (12/11): to port to the family agent")

PHOTO = base64.b64decode("/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAP////////////////////////////////////////////////////////////"
                         "//////////////////////////2wBDAf//////////////////////////////////////////////////////////////"
                         "////////////////////////wAARCAABAAEDASIAAhEBAxEB/8QAFAABAAAAAAAAAAAAAAAAAAAAA//EABQQAQAAAAAAAAAA"
                         "AAAAAAAAAAD/xAAUAQEAAAAAAAAAAAAAAAAAAAAA/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AP//Z")


def tiny_pdf(text):
    """A one-page PDF whose words are `text` (Helvetica: any reader finds them); empty: a « scan » with no words."""
    stream = f"BT /F1 14 Tf 72 760 Td ({text}) Tj ET" if text else "q Q"
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


def office(parts):
    buffer = BytesIO()
    with zipfile.ZipFile(buffer, "w") as z:
        for name, content in parts.items():
            z.writestr(name, content)
    return buffer.getvalue()


DOCX = office({"word/document.xml": '<w:document xmlns:w="w"><w:body><w:p><w:r><w:t>Inscription au foot : '
                                    'samedi 10 h</w:t></w:r></w:p></w:body></w:document>'})
REL = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
XLSX = office({
    "[Content_Types].xml": '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="xml" '
                           'ContentType="application/xml"/><Default Extension="rels" ContentType="application/vnd.openxmlformats-'
                           'package.relationships+xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.'
                           'openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/'
                           'sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>',
    "_rels/.rels": f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" '
                   f'Type="{REL}/officeDocument" Target="xl/workbook.xml"/></Relationships>',
    "xl/workbook.xml": f'<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="{REL}"><sheets>'
                       '<sheet name="Budget" sheetId="1" r:id="rId1"/></sheets></workbook>',
    "xl/_rels/workbook.xml.rels": f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship '
                                  f'Id="rId1" Type="{REL}/worksheet" Target="worksheets/sheet1.xml"/></Relationships>',
    "xl/worksheets/sheet1.xml": '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1">'
                                '<c r="A1" t="inlineStr"><is><t>Cotisation</t></is></c><c r="B1"><v>120</v></c></row></sheetData></worksheet>',
})
REPORT = tiny_pdf("Bulletin du premier trimestre: Mathematiques 15,5")
SHARED = {"agent": "cuisine__alex"}


def household(bridges, fakes, name, agents, limit_mb=None):
    def change(c):
        c["agents"] = {a.title(): {"hermes_url": f"{fakes.hermes.url}/p/{a}", "hermes_key": f"{a}-key-" + "x" * 24,
                                   "upload_dir_host": c["data_dir"] + f"/../uploads/{a}",
                                   "upload_dir_container": f"/home/hermes/.hermes/profiles/{a}/uploads"}
                       for a in agents}
        if limit_mb is not None:
            c["limits"]["upload_max_mb"] = limit_mb
    bridge = bridges.new(name, change=change)
    bridge.config["public_url"] = bridge.url
    return bridge.start()


def join(a, c, name):
    assert c.put("/v1/family/name", headers=AUTH, json={"name": name}).status_code == 200
    invitation = a.post("/v1/family/invite", headers=AUTH).json()
    assert c.post("/v1/family/join", headers=AUTH, json={"url": invitation["url"], "code": invitation["code"]}).status_code == 200


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


def pool(a):
    folder = a.state_dir / "shared-files" / "cuisine"
    return sorted(p.name for p in folder.iterdir()) if folder.is_dir() else []


def listed(bridge, path):
    answer = bridge.get(path, headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()["files"]


def test_a_shared_agents_files_are_everyones_who_uses_it(bridges, fakes):
    a, b, c = family(bridges, fakes)
    # shared for the first time: its Hermes restarted once idle, its new tool in it
    assert "its services changed" in a.log_tail(400)
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

    # in Cuisine's pool on Alex's place (0700, files 0600), never in the agent's uploads; nothing left on her side
    kept = pool(a)
    assert sorted(n.split("-", 1)[1] for n in kept) == ["IMG_0042.jpg", "bulletin.pdf"]
    folder = a.state_dir / "shared-files" / "cuisine"
    assert oct(os.stat(folder).st_mode & 0o777) == "0o700"
    assert {oct(os.stat(folder / n).st_mode & 0o777) for n in kept} == {"0o600"}
    assert not list(holding.iterdir())

    # her turn: her words framed, the PDF as one line, the photo as an image part
    run = fakes.hermes.started["r1"]
    parts = run["input"][0]["content"]
    assert [p["type"] for p in parts] == ["text", "image_url"]
    line = next(x for x in parts[0]["text"].splitlines() if x.startswith("[Pièce jointe : bulletin.pdf"))
    path = line.rsplit("→ ", 1)[1].rstrip("]")
    assert path.startswith("/famille/") and path.endswith("-bulletin.pdf")
    assert "read_sent_file" in run["instructions"] and "mcp__bipagents__read_sent_file" in run["guest_turn"]["tools"]
    assert "read_sent_file" in {t["name"] for t in rpc(a, "tools/list", token=TOKEN).json()["result"]["tools"]}

    # in her turn, the agent reads it, sees the photo, lists the pool
    read = call(a, "read_sent_file", {"path": path})
    assert read["isError"] is False and "Mathematiques 15,5" in text_of(read) and "from Marie" in text_of(read)
    photo_path = "/famille/" + next(n for n in kept if n.endswith("IMG_0042.jpg"))
    photo = call(a, "read_sent_file", {"path": photo_path})
    assert photo["content"][0] == {"type": "image", "data": base64.b64encode(PHOTO).decode(), "mimeType": "image/jpeg"}
    assert "bulletin.pdf" in text_of(call(a, "read_sent_file", {}))
    for bad in ["/fichiers/x.pdf", "/famille/../agent-shares.json", "/famille/0123456789ab-autre.pdf"]:
        assert call(a, "read_sent_file", {"path": bad})["isError"] is True, bad
    # a second message while her turn goes on: refused, the file it carried taken back, still held on her side
    again = held(b, "fiche.pdf")
    refused = b.post(f"/v1/threads/{cid}/messages", headers=AUTH,
                     json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000B2", "text": "Et celle-ci",
                           "attachments": [{"kind": "document", "name": "fiche.pdf", "path": again["path"]}]})
    assert refused.status_code == 409, refused.text
    assert wait(lambda: pool(a) == kept)
    assert [p.name.split("-", 1)[1] for p in holding.iterdir()] == ["fiche.pdf"]

    session = run["session_id"]
    fakes.hermes.messages[("cuisine", session)] = [{"role": "user", "content": parts},
                                                   {"role": "assistant", "content": "Zoé a 15,5 en maths."}]
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    page = lambda: b.get(f"/v1/threads/{cid}/timeline", headers=AUTH, params=SHARED).json()  # noqa: E731
    assert wait(lambda: page()["pending"]["run"] is None and page()["items"][-1]["role"] == "agent", timeout=15)
    mine = page()["items"][0]
    assert mine["text"] == "Voici le bulletin de Zoé" and mine["client_id"] == message["client_id"]
    atts = {x["kind"]: x for x in mine["attachments"]}
    for kind, data in [("document", REPORT), ("image", PHOTO)]:
        url = atts[kind]["url"]
        assert url.startswith("/v1/shared-agents/cuisine__alex/files/"), url
        got = b.get(url, headers=AUTH)
        assert got.status_code == 200 and got.content == data, (kind, got.status_code)

    # Paul sees them, from Marie, through his own bridge; he can't delete hers
    his = listed(c, "/v1/shared-agents/cuisine__alex/files")
    assert sorted((f["name"], f["from"], f["mine"]) for f in his) == [("IMG_0042.jpg", "Marie", False),
                                                                     ("bulletin.pdf", "Marie", False)]
    report = next(f for f in his if f["name"] == "bulletin.pdf")
    assert c.get(report["url"], headers=AUTH).content == REPORT
    assert c.delete(report["url"], headers=AUTH).status_code == 404
    # ...and the agent uses Marie's file in Paul's turn
    gate2 = threading.Event()
    fakes.hermes.runs["r2"] = [sse("tool.started", run_id="r2", tool="mcp__bipagents__read_sent_file"), gate2,
                               sse("run.completed", run_id="r2", output="Oui.")]
    asked = c.post("/v1/threads/new/messages", headers=AUTH,
                   json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000C1", "text": "Lis le bulletin de Zoé"})
    assert asked.status_code == 200, asked.text
    assert wait(lambda: "r2" in fakes.hermes.started)
    in_his_turn = call(a, "read_sent_file", {"path": path})
    assert in_his_turn["isError"] is False and "Mathematiques 15,5" in text_of(in_his_turn)
    gate2.set()
    fakes.hermes.started["r2"]["status"] = "completed"
    assert wait(lambda: c.get(f"/v1/threads/{asked.json()['thread']}/timeline", headers=AUTH,
                              params=SHARED).json()["pending"]["run"] is None, timeout=15)

    # Alex sees them in his app, and his own agent's turns read them too
    own = listed(a, "/v1/agents/cuisine/shared-files")
    assert sorted(f["from"] for f in own) == ["Marie", "Marie"] and not any(f["mine"] for f in own)
    assert a.get(next(f["url"] for f in own if f["name"] == "bulletin.pdf"), headers=AUTH).content == REPORT
    assert "Mathematiques 15,5" in text_of(call(a, "read_sent_file", {"path": path}))

    # Alex's own file to Cuisine, sent in his own conversation: in its pool too, from Alex, read by its tool
    stored = a.post("/v1/files", headers=AUTH, data={"agent": "cuisine"},
                    files={"file": ("inscription.docx", DOCX, "application/octet-stream")})
    assert stored.status_code == 200, stored.text
    fakes.hermes.runs["r3"] = [sse("run.completed", run_id="r3", output="Noté.")]
    mine_sent = a.post("/v1/threads/new/messages", headers=AUTH,
                       json={"agent": "cuisine", "client_id": "C0FFEE00-0000-4000-8000-0000000000A1", "text": "L'inscription",
                             "attachments": [{"kind": "document", "name": "inscription.docx", "path": stored.json()["path"]}]})
    assert mine_sent.status_code == 200, mine_sent.text
    alex_file = next(f for f in listed(b, "/v1/shared-agents/cuisine__alex/files") if f["name"] == "inscription.docx")
    assert alex_file["from"] == "Alex" and alex_file["mine"] is False
    assert b.get(alex_file["url"], headers=AUTH).content == DOCX
    # by the path his own message gave it (the family's agent has no file tools: the tool reads it)
    assert "samedi 10 h" in text_of(call(a, "read_sent_file", {"path": stored.json()["path"]}))
    assert b.delete(alex_file["url"], headers=AUTH).status_code == 404   # his, not hers to delete

    # only its sender deletes a file; a conversation deleted keeps the files (everyone's)
    photo_item = next(f for f in listed(b, "/v1/shared-agents/cuisine__alex/files") if f["name"] == "IMG_0042.jpg")
    assert photo_item["mine"] is True
    assert b.delete(photo_item["url"], headers=AUTH).status_code == 200
    assert "IMG_0042.jpg" not in {f["name"] for f in listed(c, "/v1/shared-agents/cuisine__alex/files")}
    assert not any(n.endswith("IMG_0042.jpg") for n in pool(a))
    assert b.delete(f"/v1/threads/{cid}", headers=AUTH, params=SHARED).status_code == 200
    assert "bulletin.pdf" in {f["name"] for f in listed(c, "/v1/shared-agents/cuisine__alex/files")}

    # Alex stops sharing with Marie: her files go, his and the agent's other users' stay; then with nobody: all go
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": ["paul"]}).status_code == 200
    assert {f["name"] for f in listed(a, "/v1/agents/cuisine/shared-files")} == {"inscription.docx"}
    assert a.put("/v1/agents/cuisine/shares", headers=AUTH, json={"households": []}).status_code == 200
    assert listed(a, "/v1/agents/cuisine/shared-files") == [] and pool(a) == []


def test_limits_office_files_a_scans_text_and_leaving(bridges, fakes):
    # Alex's place takes files of 20 KB at most here (`upload_max_mb`, as for his own agents)
    a, b, c = family(bridges, fakes, host_limit_mb=0.02)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Reçu.")]
    big = held(b, "scan.pdf", REPORT + b"%" + b"x" * 30_000)
    too_large = b.post("/v1/threads/new/messages", headers=AUTH,
                       json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D1", "text": "Le scan",
                             "attachments": [{"kind": "document", "name": "scan.pdf", "path": big["path"]}]})
    assert too_large.status_code == 413, too_large.text
    assert "r1" not in fakes.hermes.started
    gone = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D2", "text": "Et ça",
                        "attachments": [{"kind": "document", "name": "x.pdf", "path": "/bipagents/held/0123456789ab-x.pdf"}]})
    assert gone.status_code == 422 and "no longer here" in gone.text

    # files alone (no words): Word, Excel, a scan with its text recognised on her phone, a scan without
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("tool.started", run_id="r1", tool="mcp__bipagents__read_sent_file"), gate,
                               sse("run.completed", run_id="r1", output="Reçu.")]
    docs = [("inscription.docx", DOCX, None), ("budget.xlsx", XLSX, None),
            ("certificat.pdf", tiny_pdf(""), "Certificat medical : apte a la pratique du football"),
            ("vieux.pdf", tiny_pdf(""), None)]
    attachments = []
    for name, data, text in docs:
        h = held(b, name, data, "application/octet-stream" if not name.endswith(".pdf") else "application/pdf")
        attachments.append({"kind": "document", "name": name, "path": h["path"], **({"text": text} if text else {})})
    sent = b.post("/v1/threads/new/messages", headers=AUTH,
                  json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D3", "text": "", "attachments": attachments})
    assert sent.status_code == 200, sent.text
    listed_threads = b.get("/v1/threads", headers=AUTH, params=SHARED).json()["threads"]
    assert [t["title"] for t in listed_threads if t["id"] == sent.json()["thread"]] == ["inscription.docx"]
    assert wait(lambda: "r1" in fakes.hermes.started)
    words = fakes.hermes.started["r1"]["input"]
    paths = {line.split(" : ", 1)[1].split(" (", 1)[0]: line.rsplit("→ ", 1)[1].rstrip("]")
             for line in words.splitlines() if line.startswith("[Pièce jointe")}
    read = lambda name: text_of(call(a, "read_sent_file", {"path": paths[name]}))  # noqa: E731
    assert "Inscription au foot : samedi 10 h" in read("inscription.docx")
    assert "Cotisation\t120" in read("budget.xlsx")
    certificate = read("certificat.pdf")
    assert "apte a la pratique du football" in certificate and "recognised on the phone" in certificate
    assert "say so plainly" in read("vieux.pdf")
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"

    # « Ne plus l'utiliser »: her files go from Alex's place
    assert len(listed(c, "/v1/shared-agents/cuisine__alex/files")) == 4
    assert b.delete("/v1/shared-agents/cuisine__alex", headers=AUTH).status_code == 200
    assert wait(lambda: listed(c, "/v1/shared-agents/cuisine__alex/files") == [])
    assert not (b.state_dir / "shared-held" / "cuisine__alex").exists()


def test_another_households_agent_never_reads_the_pool(bridges, fakes):
    """A discussion or another household's agent writing to Cuisine (a guest turn) has no read_sent_file."""
    from test_shared_agents import RELATIVE_RUN_TOOLS
    assert "mcp__bipagents__read_sent_file" not in RELATIVE_RUN_TOOLS
