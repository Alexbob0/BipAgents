"""The Carnet's trips (« Mode voyage ») and records (2026-10-23): a trip page made by an agent or turned from a
subject (by the agent, or at night by its words) with its content kept, the trip's layout for the apps (days, bookings,
packing ticks, places), the home's upcoming trip; records as Markdown tables with stable ids, merged on their key, the
legal warranty computed, and their reminders proposed at night and answered in the app."""
import re
from datetime import date, timedelta

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, nightly, notebook, tidy


def iso(days):
    return (date.today() + timedelta(days=days)).isoformat()


def test_an_agent_makes_a_trip_page_the_apps_lay_out(bridge):
    give_token(bridge)
    in_french(bridge)
    start, end = iso(12), iso(15)
    made = call(bridge, "notebook_note", {"page": "Week-end à Lisbonne", "kind": "trip", "start": start, "end": end,
                                          "destination": "Lisbonne, Portugal", "section": "transport",
                                          "facts": [f"Vol TP433 le {start} à 07:15, réf X7K2PQ"]})
    assert made["isError"] is False and "new page, Voyages/Week-end à Lisbonne.md" in text_of(made)
    for section, facts in [("itinerary", ["J2 : Belém, la tour et le monastère", "J1 : Alfama, arrivée"]),
                           ("lodging", ["Hôtel Alfama, Rua X 12, check-in 15:00, réservation 88123"]),
                           ("packing", ["Passeport", "Adaptateur"]),
                           ("onsite", ["Restaurant Ramiro : fruits de mer"])]:
        assert call(bridge, "notebook_note", {"page": "Week-end à Lisbonne", "section": section, "facts": facts})["isError"] is False
    page = (notebook(bridge) / "carnet" / "Voyages" / "Week-end à Lisbonne.md").read_text()
    assert "type: trip" in page and f"start: {start}" in page and 'destination: "Lisbonne, Portugal"' in page
    assert "## À emporter\n- [ ] Passeport" in page
    # The same thing to pack is not added twice, ticked or not.
    ticked = bridge.post("/v1/notebook/check", json={"path": "Voyages/Week-end à Lisbonne.md", "item": "Passeport",
                                                     "done": True}, headers=AUTH)
    assert ticked.status_code == 200
    again = call(bridge, "notebook_note", {"page": "Week-end à Lisbonne", "section": "packing", "facts": ["Passeport"]})
    assert text_of(again).startswith("Already in the Carnet")
    view = bridge.get("/v1/notebook/page", params={"path": "Voyages/Week-end à Lisbonne.md"}, headers=AUTH).json()
    trip = view["trip"]
    assert (trip["start"], trip["end"], trip["days_count"], trip["destination"]) == (start, end, 4, "Lisbonne, Portugal")
    assert [(d["day"], d["text"]) for d in trip["days"]] == [(1, "Alfama, arrivée"), (2, "Belém, la tour et le monastère")]
    assert trip["days"][1]["date"] == iso(13)
    flight = trip["bookings"][0]
    assert (flight["kind"], flight["date"], flight["time"], flight["number"], flight["reference"]) == (
        "transport", start, "07:15", "TP433", "X7K2PQ")
    assert trip["packing"] == [{"text": "Passeport", "done": True}, {"text": "Adaptateur", "done": False}]
    assert {"Alfama", "Hôtel Alfama", "Restaurant Ramiro"} <= {p["name"] for p in trip["places"]}
    assert trip["map_page"] is None
    assert bridge.post("/v1/notebook/check", json={"path": "Voyages/Week-end à Lisbonne.md", "item": "Crème",
                                                   "done": True}, headers=AUTH).status_code == 404
    # The home shows it while it is within 30 days; trips have their own section, apart from the subjects.
    home = bridge.get("/v1/notebook", headers=AUTH).json()
    assert [(t["title"], t["in_days"], t["day"]) for t in home["trips"]] == [("Week-end à Lisbonne", 12, None)]
    assert next(s for s in home["sections"] if s["kind"] == "subject")["count"] == 0
    assert next(s for s in home["sections"] if s["kind"] == "trip") == {"kind": "trip", "title": "Voyages", "count": 1,
                                                                        "archived": 0}
    assert bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"] == []
    listed = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    assert [(p["kind"], p["start"], p["end"], p["archived"]) for p in listed] == [("trip", start, end, False)]
    # Its reminders are tasks like any other (no duplicate).
    for _ in range(2):
        added = bridge.post("/v1/notebook/tasks", json={"title": "Check-in en ligne", "due": iso(11),
                                                        "page": "Week-end à Lisbonne"}, headers=AUTH)
        assert added.status_code == 200
    assert len(bridge.get("/v1/notebook/tasks", headers=AUTH).json()["open"]) == 1


def test_a_subject_becomes_a_trip_with_everything_kept(bridges, fakes):
    bridge = nightly(bridges, fakes)
    give_token(bridge)
    in_french(bridge)
    folder = notebook(bridge) / "carnet" / "Sujets"
    folder.mkdir(parents=True)
    (folder / "Japon.md").write_text(
        "---\nid: p00000000aa\ntype: subject\naliases: []\n---\n# Japon\n\n## Où on en est\n- Voyage tout réservé\n\n"
        "## Décisions\n- Vol AF276 le 12/11/2030 à 10:35, réf Q7K2PQ\n- Hôtel Gion à Kyoto, 3 nuits\n\n"
        "## Historique\n- 2026-10-19 : parlé avec Marc\n")
    (folder / "Cuisine.md").write_text("---\nid: p00000000bb\ntype: subject\naliases: []\n---\n# Cuisine\n\n"
                                       "## Décisions\n- Devis le 12/11 : 480 €\n")
    run = tidy(bridge)
    assert run["trips"] == 1
    # a trip leaves the subjects for « Voyages »
    assert not (folder / "Japon.md").exists()
    folder = notebook(bridge) / "carnet" / "Voyages"
    page = (folder / "Japon.md").read_text()
    assert "type: trip" in page and "start: 2030-11-12" in page
    for fact in ["Vol AF276", "Hôtel Gion", "Voyage tout réservé", "parlé avec Marc"]:
        assert fact in page
    assert "## Transports\n- Vol AF276" in page and "## Hébergements\n- Hôtel Gion" in page
    assert "type: subject" in (notebook(bridge) / "carnet" / "Sujets" / "Cuisine.md").read_text()
    changes = bridge.get("/v1/notebook/history", params={"path": "Sujets/Japon.md"}, headers=AUTH).json()["changes"]
    assert [c["action"] for c in changes] == ["move", "trip"] and changes[1]["name"] == "rangement"
    # Turned back into a subject by the person: left so.
    (folder / "Japon.md").write_text(page.replace("type: trip", "type: subject"))
    assert "trips" not in tidy(bridge)
    # The agent's judgement: notebook_note with kind trip on the subject.
    turned = call(bridge, "notebook_note", {"page": "Japon", "kind": "trip"})
    assert turned["isError"] is False and "trip page is up to date" in text_of(turned)
    assert "type: trip" in (folder / "Japon.md").read_text()


def test_records_are_tables_merged_on_their_key(bridge):
    give_token(bridge)
    in_french(bridge)
    added = call(bridge, "notebook_record", {"action": "add", "type": "purchase",
                                             "fields": {"date": "2025-03-12", "item": "Lave-linge Bosch", "store": "Darty",
                                                        "price": "499 €", "source": "arch:42"}})
    assert added["isError"] is False
    rid = re.search(r"\(id (r[0-9a-f]{6})\)", text_of(added)).group(1)
    assert "2027-03-12 (légale)" in text_of(added)
    same = call(bridge, "notebook_record", {"action": "add", "type": "purchase",
                                            "fields": {"date": "12/03/2025", "item": "lave linge bosch", "price": "479 €"}})
    assert text_of(same).startswith(f"Updated in the Carnet: purchase (id {rid})")
    page = (notebook(bridge) / "carnet" / "Fiches" / "Achats.md").read_text()
    assert "type: records\nrecords: purchase" in page
    assert ("| Date | Objet | Magasin | Prix | Catégorie | Fin de garantie | Source | id |\n"
            "| --- | --- | --- | --- | --- | --- | --- | --- |\n"
            f"| 2025-03-12 | Lave-linge Bosch | Darty | 479 € | bien durable | 2027-03-12 (légale) | arch:42 | {rid} |\n") in page
    # A real warranty replaces the legal one; the person's text around the table stays.
    path = notebook(bridge) / "carnet" / "Fiches" / "Achats.md"
    path.write_text(path.read_text() + "\nÀ garder : les factures.\n")
    call(bridge, "notebook_record", {"action": "update", "type": "purchase", "id": rid, "fields": {"warranty_end": "2028-03-12"}})
    page = path.read_text()
    assert "| 2028-03-12 | arch:42 |" in page and page.endswith("À garder : les factures.\n")
    unknown = call(bridge, "notebook_record", {"action": "update", "type": "purchase", "id": "r000000", "fields": {"price": "1 €"}})
    assert unknown["isError"]
    call(bridge, "notebook_record", {"action": "add", "type": "subscription",
                                     "fields": {"service": "Musique+", "price": "10,99 €", "period": "mois",
                                                "renewal": iso(5), "cancel": "dans l'appli, Réglages > Abonnement"}})
    call(bridge, "notebook_record", {"action": "add", "type": "paper", "fields": {"document": "Passeport", "expires": iso(40)}})
    call(bridge, "notebook_record", {"action": "add", "type": "paper", "fields": {"document": "Permis", "expires": iso(400)}})
    listed = text_of(call(bridge, "notebook_record", {"action": "list"}))
    assert "subscription id r" in listed and "Passeport" in listed
    view = bridge.get("/v1/notebook/records", headers=AUTH).json()
    papers = next(t for t in view["types"] if t["type"] == "paper")
    assert [i["document"] for i in papers["items"]] == ["Passeport", "Permis"] and papers["items"][0]["in_days"] == 40
    purchase = next(t for t in view["types"] if t["type"] == "purchase")["items"][0]
    assert purchase["legal"] is False and purchase["next_date"] == "2028-03-12"
    home = bridge.get("/v1/notebook", headers=AUTH).json()["records"]
    assert [(r["type"], r["count"]) for r in home] == [("purchase", 1), ("subscription", 1), ("paper", 2)]
    shown = bridge.get("/v1/notebook/page", params={"path": "Fiches/Papiers.md"}, headers=AUTH).json()
    assert shown["records"]["type"] == "paper"
    removed = call(bridge, "notebook_record", {"action": "remove", "type": "paper", "id": papers["items"][1]["id"]})
    assert text_of(removed).startswith("Removed from the Carnet: Permis")


def test_bookings_feed_their_trip(bridge):
    give_token(bridge)
    in_french(bridge)
    call(bridge, "notebook_note", {"page": "Rome", "kind": "trip", "start": iso(3), "end": iso(6), "facts": ["Colisée"]})
    call(bridge, "notebook_record", {"action": "add", "type": "booking",
                                     "fields": {"date": f"{iso(4)} 09:30", "what": "Visite du Colisée", "provider": "Coop",
                                                "reference": "COL123", "trip": "[[Rome]]", "source": "arch:7"}})
    call(bridge, "notebook_record", {"action": "add", "type": "booking",
                                     "fields": {"date": iso(60), "what": "Dîner", "reference": "D1"}})
    trip = bridge.get("/v1/notebook/page", params={"path": "Voyages/Rome.md"}, headers=AUTH).json()["trip"]
    booked = [b for b in trip["bookings"] if b["kind"] == "booking"]
    assert len(booked) == 1
    assert (booked[0]["reference"], booked[0]["time"], booked[0]["date"], booked[0]["source"]) == ("COL123", "09:30", iso(4), "arch:7")


def test_reminders_are_proposed_at_night_and_answered_in_the_app(bridges, fakes):
    bridge = nightly(bridges, fakes)
    give_token(bridge)
    in_french(bridge)
    bought = (date.today() - timedelta(days=2 * 365 - 20)).isoformat()
    call(bridge, "notebook_record", {"action": "add", "type": "purchase", "fields": {"date": bought, "item": "Aspirateur", "price": "189 €"}})
    call(bridge, "notebook_record", {"action": "add", "type": "subscription", "fields": {"service": "Cloud", "renewal": iso(3)}})
    call(bridge, "notebook_record", {"action": "add", "type": "paper", "fields": {"document": "Passeport", "expires": iso(200)}})
    run = tidy(bridge)
    assert run["proposed"] == 2
    proposals = bridge.get("/v1/notebook", headers=AUTH).json()["proposals"]
    assert sorted(p["kind"] for p in proposals) == ["renewal", "warranty"]
    assert all(p["agent"] == "" and p["state"] == "open" for p in proposals)
    warranty = next(p for p in proposals if p["kind"] == "warranty")
    assert warranty["title"].startswith("Garantie de Aspirateur : se termine le") and warranty["page"] == "Achats"
    # Each date once: not proposed again.
    assert "proposed" not in tidy(bridge)
    # The agents see them waiting.
    assert "Proposed by the Carnet" in text_of(call(bridge, "notebook_task", {"action": "list"}))
    yes = bridge.post(f"/v1/notebook/proposals/{warranty['id']}", json={"accept": True}, headers=AUTH)
    assert yes.status_code == 200 and yes.json()["task"]["origin"] == "accepted"
    assert "[[Achats]]" in yes.json()["task"]["title"]
    renewal = next(p for p in proposals if p["kind"] == "renewal")
    no = bridge.post(f"/v1/notebook/proposals/{renewal['id']}", json={"accept": False, "never": True}, headers=AUTH)
    assert no.status_code == 200 and "renouvellements" in no.json()["preference"]
    assert bridge.get("/v1/notebook", headers=AUTH).json()["proposals"] == []
    assert bridge.post(f"/v1/notebook/proposals/{warranty['id']}", json={"accept": True}, headers=AUTH).status_code == 400
