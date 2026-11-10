"""Carnet v2, B3 (internal/notes/carnet-v2/contract.md §3.7): a paper photographed. The phone reads the text and sends
it with the photo; the bridge files it by rules — never a number kept (MRZ and long codes taken out first), the holder
matched to their page, two reminder tasks — and keeps the photo apart (notebook/papers/, 0600), given back only by its
own route. Scripted, no model."""
import base64
import json

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of

# a synthetic passport (ICAO's test MRZ, a made-up person), as the phone reads it
PASSPORT = """RÉPUBLIQUE FRANÇAISE
PASSEPORT / PASSPORT
Type P  Code FRA  Passeport n° 19AB12345
Nom / Surname
MARTIN
Prénoms / Given names
LÉA, MARIE
Date de naissance 03.04.1990
Date d'expiration / Date of expiry
12 MARS/MAR 2031
P<FRAMARTIN<<LEA<MARIE<<<<<<<<<<<<<<<<<<<<<<
19AB123458FRA9004035F3103128<<<<<<<<<<<<<<04
"""
JPEG = b"\xff\xd8\xff\xe0" + b"synthetic photo" * 4


def test_a_paper_is_filed_without_its_number(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200
    assert call(bridge, "notebook_write", {"page": "Léa Martin", "kind": "person", "facts": ["Sœur"]})["isError"] is False
    filed = bridge.post("/v1/notebook/paper", params={"lang": "fr"}, headers=AUTH,
                        json={"text": PASSPORT, "image": base64.b64encode(JPEG).decode()})
    assert filed.status_code == 200, filed.text
    out = filed.json()
    assert {k: out["record"][k] for k in ("type", "document", "holder", "expires")} == {
        "type": "paper", "document": "Passeport", "holder": "Léa Martin", "expires": "2031-03-12"}
    assert out["card"] == {"title": "Passeport · Léa", "lines": ["Fin : 12 mars 2031", "Rappels : 12 déc. 2030 et 12 févr. 2031"],
                           "buttons": [{"id": "fix", "text": "Corriger"}, {"id": "ok", "text": "OK"}]}
    assert out["photo"] == "Voir la photo" and len(out["tasks"]) == 2
    tasks = bridge.get("/v1/notebook/tasks", headers=AUTH).json()["open"]
    assert sorted(t["due"] for t in tasks) == ["2030-12-12", "2031-02-12"]
    assert all(t["origin"] == "asked" for t in tasks)
    photo = bridge.get(f"/v1/notebook/papers/{out['record']['id']}/photo", headers=AUTH)
    assert photo.status_code == 200 and photo.content == JPEG and photo.headers["content-type"] == "image/jpeg"
    kept = bridge.state_dir / "notebook" / "papers" / f"{out['record']['id']}.jpg"
    assert oct(kept.stat().st_mode & 0o777) == "0o600"
    # never the number, anywhere the bridge keeps the Carnet; never in the offline copy
    for f in (bridge.state_dir / "notebook").rglob("*"):
        if f.is_file() and f.suffix != ".jpg":
            text = f.read_text(errors="ignore")
            assert "19AB12345" not in text and "123458" not in text and "<<" not in text, f
    offline = bridge.get("/v1/notebook/offline", params={"lang": "fr"}, headers=AUTH).text
    assert "Passeport" not in offline
    # again: the same record, no new tasks; nothing readable: 422
    again = bridge.post("/v1/notebook/paper", params={"lang": "fr"}, headers=AUTH, json={"text": PASSPORT}).json()
    assert again["record"]["id"] == out["record"]["id"] and again["tasks"] == out["tasks"]
    bad = bridge.post("/v1/notebook/paper", params={"lang": "fr"}, headers=AUTH, json={"text": "bonjour"})
    assert bad.status_code == 422 and bad.json()["detail"]["code"] == "paper_unreadable"
    assert bridge.get("/v1/notebook/papers/r000000/photo", headers=AUTH).status_code == 404


def test_an_agents_paper_is_cleaned_the_same_way(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200
    done = call(bridge, "notebook_record", {"action": "add", "type": "paper",
                                            "fields": {"document": "Permis de conduire", "holder": "Marc", "expires": "2034-05-01"}})
    assert done["isError"] is False, text_of(done)
    tasks = bridge.get("/v1/notebook/tasks", headers=AUTH).json()["open"]
    assert sorted(t["due"] for t in tasks) == ["2034-02-01", "2034-04-01"]
    refused = call(bridge, "notebook_record", {"action": "add", "type": "paper",
                                               "fields": {"document": "Passeport", "number": "19AB12345"}})
    assert text_of(refused) == "Not saved: ID numbers are never kept."
    assert "19AB12345" not in json.dumps(bridge.get("/v1/notebook/records", headers=AUTH).json())
