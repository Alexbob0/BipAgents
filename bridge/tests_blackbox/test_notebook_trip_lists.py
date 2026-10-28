"""A page gathering all the trips (« Voyages ») is never a trip, and a trip's dates are those of its main cluster
(2026-10-28, from a real Carnet: one « Voyages » page shown « 31 déc. 2021 – 20 déc. 2026, jour 1 763 sur 1 816 »,
its bookings carrying their raw « archive:… » source and a « Me le rappeler » years too late)."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, notebook


def test_a_list_of_trips_is_refused_and_read_as_a_subject(bridge):
    give_token(bridge)
    in_french(bridge)
    refused = call(bridge, "notebook_note", {"page": "Voyages", "kind": "trip", "section": "transport",
                                             "facts": ["Parking aéroport le 22/07/2022"]})
    assert refused["isError"] is True and "one trip page per journey" in text_of(refused)
    # A page already of kind trip with such a name (made before the rule) is shown as a subject: no countdown.
    folder = notebook(bridge) / "carnet" / "Voyages"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "Voyages.md").write_text(
        "---\ntype: trip\nid: v1\n---\n# Voyages\n\n## Transports\n- Remboursement Air France le 31/12/2021\n"
        "- Parking aéroport le 22/07/2022\n- Location voiture le 01/10/2022\n- Vol Paris → Lisbonne le 20/12/2026\n")
    view = bridge.get("/v1/notebook/page", params={"path": "Voyages/Voyages.md"}, headers=AUTH).json()
    assert view["kind"] == "subject" and view["trip"] is None
    assert "Parking aéroport" in view["text"]
    home = bridge.get("/v1/notebook", headers=AUTH).json()
    assert home["trips"] == []
    assert next(s for s in home["sections"] if s["kind"] == "trip")["count"] == 0


def test_a_trip_s_dates_are_its_bookings_not_the_refund_months_later(bridge):
    give_token(bridge)
    in_french(bridge)
    made = call(bridge, "notebook_note", {"page": "Amsterdam, déc. 2021", "kind": "trip", "section": "transport",
                                          "facts": ["Vol AF1240 Paris → Amsterdam le 18/12/2021 à 07:15",
                                                    "Vol retour le 22/12/2021",
                                                    "Remboursement Air France le 15/03/2022 : 120 €"]})
    assert made["isError"] is False
    call(bridge, "notebook_record", {"action": "add", "type": "booking",
                                     "fields": {"date": "2021-12-18 15:00", "what": "Hôtel Pulitzer",
                                                "reference": "PUL123", "trip": "[[Amsterdam, déc. 2021]]",
                                                "source": "archive:vie/mails/77d17aaf351443f8"}})
    listed = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    assert [(p["start"], p["end"]) for p in listed] == [("2021-12-18", "2021-12-22")]
    trip = bridge.get("/v1/notebook/page", params={"path": listed[0]["path"]}, headers=AUTH).json()["trip"]
    assert (trip["start"], trip["end"], trip["days_count"]) == ("2021-12-18", "2021-12-22", 5)
    hotel = next(b for b in trip["bookings"] if b["kind"] == "booking")
    # Never the raw source: the apps open the mail; nothing to remind years later.
    assert hotel["source"] is None and hotel["source_ref"] == "vie/mails/77d17aaf351443f8"
    assert all(b["past"] is True for b in trip["bookings"])
    assert bridge.get("/v1/notebook", headers=AUTH).json()["trips"] == []
