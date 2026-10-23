"""Records filled from an archive (bridge-rs/src/archives/records_fill.rs, 2026-10-23): a free pre-filter on the
archive's index, a model reading the likely documents with a strict JSON answer checked by the bridge, a sample that
writes nothing and estimates the whole archive, the full pass as a capped background job (« Tâches de fond »), the
records' sources linked to their documents, idempotent passes, the agent's `archive_records` tool and the command
line. Synthetic documents only, a fake model answering canned JSON."""
import json
from datetime import date, timedelta

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, rpc, text_of
from bbkit.servers import wait_until
from test_archives import wait_ready

AGENT = "wellness"
LATER = (date.today() + timedelta(days=40)).isoformat()


def mails(root):
    """A folder of e-mail notes: four likely records, an advert, a diary page, an answer the model spoils."""
    folder = root / "mails"
    folder.mkdir()

    def note(name, title, day, body):
        (folder / name).write_text(f"---\ntitle: {title}\nfrom: Shop <s@shop.example>\ndate: {day}\n---\n{body}\n")
    note("velo.md", "Votre commande Vélo", "2024-05-02",
         "Merci pour votre achat chez Boutique Cycles : vélo électrique, 1 299 €. Numéro de commande 778.")
    note("netflix.md", "Votre abonnement Netflix", "2024-02-01", "Abonnement mensuel Netflix : 13,49 €.")
    note("vol.md", "Confirmation de réservation Lisbonne", "2024-04-01",
         f"Vol Paris → Lisbonne le {LATER} à 07:15, TAP, référence de réservation LIS7Q2. "
         "Hôtel Alfama à Lisbonne, 2 nuits, réservation HTL889.")
    note("faux.md", "Facture Inventé", "2024-03-01", "Facture : objet Inventé, contrat auto.")
    note("promo.md", "Newsletter soldes", "2024-03-02", "Tout à -50 %.")
    note("journal.md", "Journal", "2024-03-03", "Une belle journée à la plage.")
    return folder


ANSWERS = {
    "Vélo": [{"type": "purchase", "fields": {"date": "2024-05-02", "item": "Vélo électrique", "store": "Boutique Cycles",
                                             "price": "1 299 €"}}],
    "Netflix": [{"type": "subscription", "fields": {"service": "Netflix", "price": "13,49 €", "period": "mensuel"}}],
    "Lisbonne": [{"type": "booking", "fields": {"date": f"{LATER} 07:15", "what": "Vol Paris → Lisbonne",
                                                "provider": "TAP", "reference": "LIS7Q2", "trip": "Lisbonne"}},
                 {"type": "booking", "fields": {"date": LATER, "what": "Hôtel Alfama à Lisbonne", "provider": "Hôtel Alfama",
                                                "reference": "HTL889", "trip": "Lisbonne"}}],
    # an invented field, and a contract number that isn't in the document: both refused
    "Inventé": [{"type": "purchase", "fields": {"item": "Inventé", "warranty_years": "5"}},
                {"type": "contract", "fields": {"what": "auto", "number": "NOPE-000"}}],
}


def started(bridges, fakes, **extra):
    bridge = bridges.new(change=lambda c: c.update(notebook={"base_url": fakes.model.url + "/v1", "key": "sk-nb",
                                                             "model": "small-model"}, **extra))
    folder = mails(bridge.root)
    bridge.config["archives"] = [{"agent": AGENT, "name": "Mails", "folder": str(folder)}]
    bridge.start()
    wait_ready(bridge, AGENT, "mails", documents=6)
    fakes.model.records_by_word = dict(ANSWERS)
    return bridge, folder


def records_state(bridge):
    answer = bridge.get(f"/v1/agents/{AGENT}/archives/mails/records", headers=AUTH)
    assert answer.status_code == 200, answer.text
    return answer.json()


def extraction_chats(fakes):
    return [c for c in fakes.model.chats if c["body"]["messages"][0]["content"].startswith("You extract precise records")]


def carnet_records(bridge):
    view = bridge.get("/v1/notebook/records", headers=AUTH).json()
    return {t["type"]: t["items"] for t in view["types"]}


def job_of(bridge, job_id):
    return [j for j in bridge.get("/v1/jobs", headers=AUTH).json()["jobs"] if j["id"] == job_id][0]


def test_a_sample_reports_without_writing_then_the_full_pass_fills_the_records(bridges, fakes):
    bridge, folder = started(bridges, fakes)
    state = records_state(bridge)
    assert state["candidates"] == 4 and state["read"] == 0 and state["sample"] is None and state["nightly"] is False
    assert state["models"][0]["id"] == "small-model" and state["models"][0]["local"] is True

    sampled = bridge.post(f"/v1/agents/{AGENT}/archives/mails/records/sample", json={"size": 50}, headers=AUTH)
    assert sampled.status_code == 202 and sampled.json()["state"] == "sampling"
    assert wait_until(lambda: records_state(bridge)["sample"] is not None, timeout=30)
    sample = records_state(bridge)["sample"]
    assert (sample["sent"], sample["new"], sample["rejected"]) == (4, 4, 2)
    assert sample["records"]["booking"]["new"] == 2 and sample["estimate"]["documents"] == 4
    titles = {e["title"] for e in sample["examples"]}
    assert titles == {"Votre commande Vélo", "Votre abonnement Netflix", "Confirmation de réservation Lisbonne"}
    sent = "\n".join(c["body"]["messages"][1]["content"] for c in extraction_chats(fakes))
    assert "soldes" not in sent and "plage" not in sent          # the advert and the diary never sent
    assert extraction_chats(fakes)[0]["body"]["response_format"] == {"type": "json_object"}
    assert all(not items for items in carnet_records(bridge).values())   # a sample writes nothing

    run = bridge.post(f"/v1/agents/{AGENT}/archives/mails/records/run", json={"budget_eur": 0}, headers=AUTH)
    assert run.status_code == 200, run.text
    job_id = run.json()["id"]
    assert run.json()["kind"] == "archive_extract"
    assert wait_until(lambda: job_of(bridge, job_id)["state"] == "done", timeout=30), job_of(bridge, job_id)
    job = job_of(bridge, job_id)
    assert job["archive"] == "mails" and job["progress"] == {"done": 4, "total": 4}
    assert job["report"].startswith("4 new records from « Mails »")
    found = carnet_records(bridge)
    assert [i["item"] for i in found["purchase"]] == ["Vélo électrique"]
    assert found["purchase"][0]["warranty_end"].startswith("2026-05-02")
    reference = found["purchase"][0]["source_ref"]
    assert reference.startswith(f"{AGENT}/mails/") and found["purchase"][0]["source"] == f"archive:{reference}"
    document = bridge.get("/v1/archives/document", params={"ref": reference}, headers=AUTH).json()
    assert document["title"] == "Votre commande Vélo"
    assert [b["reference"] for b in found["booking"]] == ["LIS7Q2", "HTL889"]
    # the flight and the hotel make one trip (time away), named by its city and month, linked to its document
    trips = list((bridge.state_dir / "notebook" / "carnet" / "Trips").glob("Lisbon — *.md"))
    assert len(trips) == 1, trips
    page = trips[0].read_text()
    assert "type: trip" in page and "LIS7Q2" in page and "HTL889" in page and "[[archive:" in page
    assert "origin: archive" in page
    state = records_state(bridge)
    assert state["read"] == 4 and state["nightly"] is True and state["job"]["id"] == job_id

    # again: nothing new to read, no model call; a changed document is read again and merged (no duplicate)
    calls = len(extraction_chats(fakes))
    again = bridge.post(f"/v1/agents/{AGENT}/archives/mails/records/run", json={"budget_eur": 0}, headers=AUTH).json()
    assert wait_until(lambda: job_of(bridge, again["id"])["state"] == "done", timeout=30)
    assert len(extraction_chats(fakes)) == calls and job_of(bridge, again["id"])["progress"]["total"] == 0
    (folder / "velo.md").write_text((folder / "velo.md").read_text() + "Livré.\n")
    (folder / "casse.md").write_text("---\ntitle: Facture BROKEN\ndate: 2024-06-01\n---\nFacture 12 €\n")
    assert bridge.post(f"/v1/agents/{AGENT}/archives/mails/refresh", headers=AUTH).status_code == 200
    wait_ready(bridge, AGENT, "mails", documents=7)
    third = bridge.post(f"/v1/agents/{AGENT}/archives/mails/records/run", json={"budget_eur": 0}, headers=AUTH).json()
    assert wait_until(lambda: job_of(bridge, third["id"])["state"] == "done", timeout=30)
    assert len(carnet_records(bridge)["purchase"]) == 1
    # the spoiled batch was asked again one document at a time
    singles = [c for c in extraction_chats(fakes)[calls:] if c["body"]["messages"][1]["content"].count("Document ") == 1]
    assert len(singles) == 2


def test_the_cap_pauses_the_job_and_a_bigger_one_finishes_it(bridges, fakes):
    bridge, _ = started(bridges, fakes)
    # a dear model: one call may cost up to ≈ 0.09 € (its prompt, its longest answer): 0.05 € stops before any call
    priced = {"model": "big-model", "price_in": 10, "price_out": 50}
    run = bridge.post(f"/v1/agents/{AGENT}/archives/mails/records/run", json={"budget_eur": 0.05, **priced}, headers=AUTH)
    assert run.status_code == 200, run.text
    job_id = run.json()["id"]
    assert wait_until(lambda: job_of(bridge, job_id)["state"] == "paused", timeout=30), job_of(bridge, job_id)
    job = job_of(bridge, job_id)
    assert job["reason"] == "budget" and job["eur"] == 0 and extraction_chats(fakes) == []
    refused = bridge.post(f"/v1/jobs/{job_id}/resume", headers=AUTH)
    assert refused.status_code == 409 and "cap" in refused.json()["detail"]
    resumed = bridge.post(f"/v1/jobs/{job_id}/resume", json={"budget_eur": 2}, headers=AUTH)
    assert resumed.status_code == 200, resumed.text
    assert wait_until(lambda: job_of(bridge, job_id)["state"] == "done", timeout=30), job_of(bridge, job_id)
    job = job_of(bridge, job_id)
    assert job["budget_eur"] == 2 and 0 < job["eur"] <= 2 and job["model"] == "big-model"
    assert {c["body"]["model"] for c in extraction_chats(fakes)} == {"big-model"}
    assert bridge.post(f"/v1/agents/{AGENT}/archives/mails/records/run", json={"budget_eur": 0, **priced},
                       headers=AUTH).status_code == 400            # a priced model needs a cap
    switched = bridge.patch(f"/v1/agents/{AGENT}/archives/mails/records", json={"nightly": False}, headers=AUTH)
    assert switched.status_code == 200 and switched.json()["nightly"] is False


def test_the_agent_samples_and_proposes_and_the_command_line_reports(bridges, fakes):
    bridge, _ = started(bridges, fakes)
    give_token(bridge)
    tools = {t["name"]: t for t in rpc(bridge, "tools/list").json()["result"]["tools"]}
    assert "archive_records" in tools and "« Mails »" in tools["archive_records"]["description"]
    assert "Run a sample first" in text_of(call(bridge, "archive_records", {"action": "propose"}))
    sampling = call(bridge, "archive_records", {"action": "sample"})
    assert sampling["isError"] is False and "Sample started" in text_of(sampling)
    assert wait_until(lambda: records_state(bridge)["sample"] is not None, timeout=30)
    status = text_of(call(bridge, "archive_records", {"action": "status"}))
    assert "4 new records would be made" in status and "Vélo électrique" in status
    proposed = call(bridge, "archive_records", {"action": "propose"})
    assert proposed["isError"] is False and "Tâches de fond" in text_of(proposed)
    job = [j for j in bridge.get("/v1/jobs", headers=AUTH).json()["jobs"] if j["kind"] == "archive_extract"][0]
    assert job["state"] == "proposed" and job["proposed_by"] == "agent"
    assert bridge.post(f"/v1/jobs/{job['id']}/start", headers=AUTH).status_code == 200
    assert wait_until(lambda: job_of(bridge, job["id"])["state"] == "done", timeout=30), job_of(bridge, job["id"])

    out = bridge.cli("archive", "extract", "--agent", AGENT, "--sample", "3")
    assert out.returncode == 0, out.stderr
    assert "sample run: 3 documents looked at" in out.stdout and "whole archive:" in out.stdout
    assert "(would be)" in out.stdout
    status = bridge.cli("archive", "extract", "--agent", AGENT, "--status")
    assert "4 already read" in status.stdout and "nightly pass on new documents: on" in status.stdout
    assert bridge.cli("archive", "extract", "--agent", AGENT, "--run").returncode == 2   # no cap given
    assert json.loads(json.dumps(records_state(bridge)["sample"]))["mode"] == "sample"
