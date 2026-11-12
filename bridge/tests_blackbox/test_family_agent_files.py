"""Files sent to a family agent (shared_files.rs, family_thread.rs; Alex 11/11-12/11): everyone's who is in it. Marie
sends Alex's « Famille » a PDF and a photo in the family's common conversation; Paul and Alex are in it too. Her bridge
holds the document her app stored first, then takes both to Alex's bridge with her message, into the agent's pool on
Alex's place (never its uploads). Everyone sees them, with who sent them (her app, Paul's through his bridge, Alex's),
the agent reads them in anyone's turn with `read_sent_file` — a document's words, a photo, the list. Alex's own files
to it join the pool. Only their sender deletes them; hers go when Alex takes her out of the agent's members. The same
limits as files to one's own agents; Word, Excel and a scan's text recognised on the phone read too (moved here from
test_shared_agent_files.py on 12/11: relatives no longer send files to a personal agent)."""
import base64
import os
import threading
import zipfile
from io import BytesIO

from bbkit.common import AUTH, sse
from bbkit.mcp import TOKEN, call, give_token, rpc, text_of
from test_family_agent import make_family_agent
from test_shared_agents import link, wait

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
SHARED = {"agent": "famille__alex"}


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
    """Alex's place holds « Famille », for the whole family: Marie and Paul have it."""
    a = household(bridges, fakes, "alex", ["famille"], limit_mb=host_limit_mb)
    b = household(bridges, fakes, "marie", ["vie"])
    c = household(bridges, fakes, "paul", ["jardin"])
    link(a, b)
    join(a, c, "Paul")
    fakes.hermes.rounds = 1000
    make_family_agent(a)
    for x in (b, c):
        assert any(v["id"] == "famille__alex" for v in x.get("/v1/family-agent", headers=AUTH).json()["agents"])
    give_token(a, "famille", TOKEN)
    return a, b, c


def held(b, name="bulletin.pdf", data=REPORT, mime="application/pdf"):
    """Her app stores the document first, as for her own agents: her bridge holds it."""
    stored = b.post("/v1/files", headers=AUTH, data=SHARED, files={"file": (name, data, mime)})
    assert stored.status_code == 200, stored.text
    return stored.json()


def pool(a):
    folder = a.state_dir / "shared-files" / "famille"
    return sorted(p.name for p in folder.iterdir()) if folder.is_dir() else []


def listed(bridge, path):
    answer = bridge.get(path, headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()["files"]


def test_a_family_agents_files_are_everyones_in_it(bridges, fakes):
    a, b, c = family(bridges, fakes)
    doc = held(b)
    assert doc["path"].startswith("/bipagents/held/") and doc["filename"] == "bulletin.pdf"
    holding = b.state_dir / "shared-held" / "famille__alex"
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
    assert sent.status_code == 200 and sent.json()["thread"] == "family", sent.text

    # in the agent's pool on Alex's place (0700, files 0600), never in its uploads; nothing left on her side
    kept = pool(a)
    assert sorted(n.split("-", 1)[1] for n in kept) == ["IMG_0042.jpg", "bulletin.pdf"]
    folder = a.state_dir / "shared-files" / "famille"
    assert oct(os.stat(folder).st_mode & 0o777) == "0o700"
    assert {oct(os.stat(folder / n).st_mode & 0o777) for n in kept} == {"0o600"}
    assert not list(holding.iterdir())

    # her turn: her words framed, the PDF as one line, the photo as an image part
    run = fakes.hermes.started["r1"]
    parts = run["input"][0]["content"]
    assert [p["type"] for p in parts] == ["text", "image_url"]
    assert "[Marie, a member of the family (household « marie »), writes to you:]\nVoici le bulletin de Zoé" in parts[0]["text"]
    line = next(x for x in parts[0]["text"].splitlines() if x.startswith("[Pièce jointe : bulletin.pdf"))
    path = line.rsplit("→ ", 1)[1].rstrip("]")
    assert path.startswith("/famille/") and path.endswith("-bulletin.pdf")
    assert "read_sent_file" in run["instructions"] and run["guest_turn"] is None
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

    # Paul writes with a file while her turn goes on: his message waits behind it (the family's one conversation),
    # his file already in the pool
    fakes.hermes.runs["r2"] = [sse("tool.started", run_id="r2", tool="mcp__bipagents__read_sent_file"),
                               sse("run.completed", run_id="r2", output="Oui.")]
    his_doc = c.post("/v1/files", headers=AUTH, data=SHARED, files={"file": ("fiche.pdf", tiny_pdf("Fiche sanitaire"), "application/pdf")})
    assert his_doc.status_code == 200, his_doc.text
    queued = c.post("/v1/threads/main/messages", headers=AUTH,
                    json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000C1", "text": "Lis aussi la fiche",
                          "attachments": [{"kind": "document", "name": "fiche.pdf", "path": his_doc.json()["path"]}]})
    assert queued.status_code == 200 and queued.json()["state"] == "queued", queued.text
    assert wait(lambda: len(pool(a)) == 3)

    session = run["session_id"]
    fakes.hermes.messages[("famille", session)] = [{"role": "user", "content": parts},
                                                   {"role": "assistant", "content": "Zoé a 15,5 en maths."}]
    gate.set()
    fakes.hermes.started["r1"]["status"] = "completed"
    # Paul's turn: the agent uses Marie's file in it
    assert wait(lambda: "r2" in fakes.hermes.started, timeout=15)
    in_his_turn = call(a, "read_sent_file", {"path": path})
    assert in_his_turn["isError"] is False and "Mathematiques 15,5" in text_of(in_his_turn)
    fakes.hermes.started["r2"]["status"] = "completed"
    page = lambda: b.get("/v1/threads/main/timeline", headers=AUTH, params=SHARED).json()  # noqa: E731
    assert wait(lambda: page()["pending"]["run"] is None, timeout=15)
    mine = next(i for i in page()["items"] if i.get("text") == "Voici le bulletin de Zoé")
    assert mine["by"] == {"name": "Marie", "mine": True} and mine["client_id"] == message["client_id"]
    atts = {x["kind"]: x for x in mine["attachments"]}
    for kind, data in [("document", REPORT), ("image", PHOTO)]:
        url = atts[kind]["url"]
        assert url.startswith("/v1/shared-agents/famille__alex/files/"), url
        got = b.get(url, headers=AUTH)
        assert got.status_code == 200 and got.content == data, (kind, got.status_code)

    # Paul sees them, from Marie, through his own bridge; he can't delete hers
    his = listed(c, "/v1/shared-agents/famille__alex/files")
    assert sorted((f["name"], f["from"], f["mine"]) for f in his) == [("IMG_0042.jpg", "Marie", False),
                                                                     ("bulletin.pdf", "Marie", False),
                                                                     ("fiche.pdf", "Paul", True)]
    report = next(f for f in his if f["name"] == "bulletin.pdf")
    assert c.get(report["url"], headers=AUTH).content == REPORT
    assert c.delete(report["url"], headers=AUTH).status_code == 404

    # Alex sees them in his app, and his own turns read them too
    own = listed(a, "/v1/agents/famille/shared-files")
    assert sorted(f["from"] for f in own) == ["Marie", "Marie", "Paul"] and not any(f["mine"] for f in own)
    assert a.get(next(f["url"] for f in own if f["name"] == "bulletin.pdf"), headers=AUTH).content == REPORT
    assert "Mathematiques 15,5" in text_of(call(a, "read_sent_file", {"path": path}))

    # Alex's own file to it, in the family's conversation: in its pool too, from Alex, read by its tool
    stored = a.post("/v1/files", headers=AUTH, data={"agent": "famille"},
                    files={"file": ("inscription.docx", DOCX, "application/octet-stream")})
    assert stored.status_code == 200, stored.text
    fakes.hermes.runs["r3"] = [sse("run.completed", run_id="r3", output="Noté.")]
    mine_sent = a.post("/v1/threads/new/messages", headers=AUTH,
                       json={"agent": "famille", "client_id": "C0FFEE00-0000-4000-8000-0000000000A1", "text": "L'inscription",
                             "attachments": [{"kind": "document", "name": "inscription.docx", "path": stored.json()["path"]}]})
    assert mine_sent.status_code == 200, mine_sent.text
    alex_file = next(f for f in listed(b, "/v1/shared-agents/famille__alex/files") if f["name"] == "inscription.docx")
    assert alex_file["from"] == "Alex" and alex_file["mine"] is False
    assert b.get(alex_file["url"], headers=AUTH).content == DOCX
    # by the path his own message gave it (the family's agent has no file tools: the tool reads it)
    assert "samedi 10 h" in text_of(call(a, "read_sent_file", {"path": stored.json()["path"]}))
    assert b.delete(alex_file["url"], headers=AUTH).status_code == 404   # his, not hers to delete

    # only its sender deletes a file
    photo_item = next(f for f in listed(b, "/v1/shared-agents/famille__alex/files") if f["name"] == "IMG_0042.jpg")
    assert photo_item["mine"] is True
    assert b.delete(photo_item["url"], headers=AUTH).status_code == 200
    assert "IMG_0042.jpg" not in {f["name"] for f in listed(c, "/v1/shared-agents/famille__alex/files")}
    assert not any(n.endswith("IMG_0042.jpg") for n in pool(a))

    # Alex takes Marie out of its members: her files go, the others' stay
    assert a.put("/v1/family-agent/famille/members", headers=AUTH, json={"members": ["paul"]}).status_code == 200
    assert {f["name"] for f in listed(a, "/v1/agents/famille/shared-files")} == {"inscription.docx", "fiche.pdf"}


def test_limits_office_files_and_a_scans_text(bridges, fakes):
    # Alex's place takes files of 20 KB at most here (`upload_max_mb`, as for his own agents)
    a, b, c = family(bridges, fakes, host_limit_mb=0.02)
    fakes.hermes.runs["r1"] = [sse("run.completed", run_id="r1", output="Reçu.")]
    big = held(b, "scan.pdf", REPORT + b"%" + b"x" * 30_000)
    too_large = b.post("/v1/threads/main/messages", headers=AUTH,
                       json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D1", "text": "Le scan",
                             "attachments": [{"kind": "document", "name": "scan.pdf", "path": big["path"]}]})
    assert too_large.status_code == 413, too_large.text
    assert "r1" not in fakes.hermes.started
    gone = b.post("/v1/threads/main/messages", headers=AUTH,
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
    sent = b.post("/v1/threads/main/messages", headers=AUTH,
                  json={**SHARED, "client_id": "C0FFEE00-0000-4000-8000-0000000000D3", "text": "", "attachments": attachments})
    assert sent.status_code == 200, sent.text
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
    assert len(listed(c, "/v1/shared-agents/famille__alex/files")) == 4
