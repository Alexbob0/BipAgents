"""Carnet v2, B1 (internal/notes/carnet-v2/contract.md): only the useful pages reach the model — 6 different pages a
turn, each read in the journal (notebook/history/reads.jsonl, never the content) — and the line under the answer that
the bridge makes from it (`carnet_line`: « A consulté 6 pages · Noté dans Sujet 0 »), the app's new page and its
section written over the revision it read, reminders_list narrowed by words. Scripted, no model."""
import json
import threading
import time

from bbkit.common import AUTH, sse
from bbkit.mcp import call as mcp_call, give_token, text_of

from test_threads_send import page, send, setup, stored, user, agent


def in_french(bridge):
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200


def tool_call(call_id, name, arguments):
    return {"id": call_id, "type": "function", "function": {"name": name, "arguments": json.dumps(arguments)}}


def test_a_turn_reads_six_pages_and_its_line_is_under_the_answer(bridges, fakes):
    bridge, thread = setup(bridges, fakes)
    give_token(bridge)
    in_french(bridge)
    for n in range(7):
        done = mcp_call(bridge, "notebook_write", {"page": f"Sujet {n}", "kind": "subject", "section": "decisions",
                                                   "facts": [f"Décision {n}"]})
        assert done["isError"] is False, text_of(done)
    gate = threading.Event()
    fakes.hermes.runs["r1"] = [sse("run.started", run_id="r1"), gate, sse("run.completed", run_id="r1", output="Voilà.")]
    t0 = time.time() - 1
    assert send(bridge, thread, "C5D00000-0000-4000-8000-0000000000C1", "Où en sont mes sujets ?").json()["state"] == "started"
    # the turn's calls, while the hub follows its run: six different pages, the 7th refused
    for n in range(6):
        assert text_of(mcp_call(bridge, "notebook_read", {"page": f"Sujet {n}"})).startswith(f"# Sujet {n}\n")
    assert text_of(mcp_call(bridge, "notebook_read", {"page": "Sujet 0", "section": "decisions"})).startswith("# Sujet 0")
    assert text_of(mcp_call(bridge, "notebook_read", {"page": "Sujet 6"})) == "Limit reached: ask the person which page to read."
    wrote = mcp_call(bridge, "notebook_write", {"page": "Sujet 0", "section": "decisions", "facts": ["Devis signé"]})
    assert text_of(wrote) == "Saved: Sujet 0 › decisions (1 fact)."
    gate.set()
    # the journal: one line per page and per turn, its run and session, never the content
    reads = [json.loads(l) for l in (bridge.state_dir / "notebook" / "history" / "reads.jsonl").read_text().splitlines()]
    turn = [r for r in reads if r["run"] == "r1"]
    assert len(turn) == 6 and {r["how"] for r in turn} == {"read"} and all(r["session"] for r in turn), turn
    assert "Décision" not in json.dumps(reads)
    assert oct((bridge.state_dir / "notebook" / "history" / "reads.jsonl").stat().st_mode & 0o777) == "0o600"
    # the stored turn: the line right after its answer, the Carnet's calls backstage (no « Étapes » card for them)
    now = time.time() + 1
    stored(fakes,
           dict(user("Où en sont mes sujets ?"), timestamp=t0 + 0.5),
           {"role": "assistant", "content": "", "timestamp": t0 + 0.6,
            "tool_calls": [tool_call(f"n{n}", "mcp__bipagents__notebook_read", {"page": f"Sujet {n}"}) for n in range(3)]},
           *[{"role": "tool", "tool_call_id": f"n{n}", "tool_name": "mcp__bipagents__notebook_read",
              "content": f"# Sujet {n}", "timestamp": t0 + 0.7} for n in range(3)],
           dict(agent("Voilà où en sont tes sujets."), timestamp=now))
    items = page(bridge, thread)["items"]
    lines = [i for i in items if i["kind"] == "carnet_line"]
    assert len(lines) == 1, [(i["kind"], i.get("turn", i["id"])[-8:], i["at"], i.get("text", "")[:20]) for i in items]
    line = lines[0]
    assert line["text"] == "A consulté 6 pages · Noté dans Sujet 0" and line["all"] == "Tout voir"
    assert line["pages"][0] == {"title": "Sujet 0", "path": "Sujets/Sujet 0.md"}
    assert line["noted"][0]["title"] == "Sujet 0" and line["noted"][0]["change_id"].startswith("c")
    assert items[items.index(line) - 1].get("text") == "Voilà où en sont tes sujets."
    assert not [i for i in items if i["kind"] == "steps"]
    # « Lignes Noté » off: only what was read
    bridge.patch("/v1/notebook/settings", json={"mentions": False}, headers=AUTH)
    line = next(i for i in page(bridge, thread)["items"] if i["kind"] == "carnet_line")
    assert line["text"] == "A consulté 6 pages" and line["noted"] == []


def test_the_app_makes_a_page_and_writes_over_what_it_read(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    in_french(bridge)
    made = bridge.post("/v1/notebook/page", json={"kind": "person", "title": "Léa Martin"}, headers=AUTH)
    assert made.status_code == 201, made.text
    assert made.json()["path"] == "Personnes/Léa Martin.md" and len(made.json()["rev"]) == 16
    again = bridge.post("/v1/notebook/page", json={"kind": "person", "title": "Léa Martin"}, headers=AUTH)
    assert again.status_code == 409 and again.json()["detail"] == {
        "code": "page_exists", "text": "Cette page existe déjà.", "path": "Personnes/Léa Martin.md"}
    english = bridge.post("/v1/notebook/page", params={"lang": "en"}, json={"kind": "person", "title": "Léa Martin"},
                          headers=AUTH)
    assert english.json()["detail"]["text"] == "This page already exists."
    # an agent writes between the app's read and its save: 409, nothing lost, the app saves over the new revision
    rev = made.json()["rev"]
    mcp_call(bridge, "notebook_write", {"page": "Léa", "section": "tastes", "facts": ["Pointure : 36"]})
    body = {"path": "Personnes/Léa Martin.md", "section": "tastes", "text": "- Aime le jazz", "rev": rev}
    clash = bridge.put("/v1/notebook/section", json=body, headers=AUTH)
    assert clash.status_code == 409 and clash.json()["detail"]["section"]["facts"][0]["text"] == "Pointure : 36"
    saved = bridge.put("/v1/notebook/section", json={**body, "text": "- Pointure : 36\n- Aime le jazz",
                                                      "rev": clash.json()["detail"]["rev"]}, headers=AUTH)
    assert saved.status_code == 200, saved.text
    text = (bridge.state_dir / "notebook" / "carnet" / "Personnes" / "Léa Martin.md").read_text()
    assert "Pointure : 36 — *Wellness" in text and "Aime le jazz — *toi" in text
    # ID numbers never kept, from the app either
    refused = bridge.put("/v1/notebook/section", json={**body, "text": "- Passeport : 12AB34567",
                                                       "rev": saved.json()["rev"]}, headers=AUTH)
    assert refused.status_code == 422 and refused.json()["detail"]["code"] == "number_refused"


def test_reminders_list_is_narrowed_by_words(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.put("/v1/reminders/agents", json={"agents": ["Wellness"]}, headers=AUTH).status_code == 200
    items = [{"id": f"r{n}", "title": f"Courses {n}"} for n in range(35)]
    items.append({"id": "rp", "title": "Appeler le plombier", "notes": "fuite cuisine"})
    assert bridge.put("/v1/reminders", json={"lists": [], "items": items}, headers=AUTH).status_code == 200
    every = text_of(mcp_call(bridge, "reminders_list", {}))
    assert every.count("\n- id ") == 30 and "… and 6 more: narrow with words." in every, every
    found = text_of(mcp_call(bridge, "reminders_list", {"words": "plombier"}))
    assert "Appeler le plombier" in found and "Courses" not in found, found
