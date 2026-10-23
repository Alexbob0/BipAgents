"""The Carnet put in order (the owner, 2026-10-23: « Sujets is a mess », « garantie légale » on a 3 € registration) and
its « Archives »: the trips the extraction made among the subjects moved to « Voyages », renamed by city and month
(the old name an alias, the links pointed to the new one), the same trip merged, a commute train turned back into a
booking with its lines kept; a trip the person wrote moved, never renamed; past trips and records archived, never
deleted, all of it in the history and undone there; purchases given their class, the legal warranty only on durable
goods, « important » purchases only reminded of. Synthetic data only."""
from datetime import date, timedelta

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, nightly, notebook, tidy

MONTHS = ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."]


def day(n):
    return date.today() + timedelta(days=n)


def ref(n):
    return f"archive:wellness/mails/{n:016x}"


def link(n):
    return f"[[{ref(n)}|voir le document]]"


def page(kind, title, body, extra=""):
    return f"---\nid: p{abs(hash(title)) % 10**10:010d}\ntype: {kind}\naliases: []\n{extra}---\n# {title}\n\n{body}"


def history(bridge, path):
    return bridge.get("/v1/notebook/history", params={"path": path}, headers=AUTH).json()["changes"]


def test_the_extractions_trips_are_put_right_and_the_past_archived(bridges, fakes):
    bridge = nightly(bridges, fakes)
    give_token(bridge)
    in_french(bridge)
    carnet = notebook(bridge) / "carnet"
    out, back, commute = day(-200), day(-196), day(-120)
    future = day(90)
    (carnet / "Sujets").mkdir(parents=True)
    (carnet / "Fiches").mkdir()
    # what the extraction wrote before: the bookings, and a « trip » per name (a city, an airport, a commute)
    (carnet / "Fiches" / "Réservations.md").write_text(
        "---\nid: p0000000001\ntype: records\nrecords: booking\n---\n# Réservations\n\n"
        "| Date | Quoi | Chez | Référence | Voyage | Source | id |\n| --- | --- | --- | --- | --- | --- | --- |\n"
        f"| {out} 07:15 | Vol Paris → Lisbonne | TAP | LIS7Q2 | Lisbonne | {ref(1)} | r000001 |\n"
        f"| {back} 18:00 | Vol LIS → CDG | TAP | LIS9Z8 | LIS | {ref(2)} | r000002 |\n"
        f"| {commute} 08:04 | Train Paris → Lyon | SNCF | QWERTY | Lyon | {ref(3)} | r000003 |\n"
        f"| {future} | Dîner chez l’Ami Jean | L'Ami Jean | T42 |  | {ref(4)} | r000004 |\n")
    (carnet / "Sujets" / "Lisbonne.md").write_text(page("trip", "Lisbonne",
        f"## Transports\n- {out} 07:15 — Vol Paris → Lisbonne — TAP — LIS7Q2 {link(1)} — *Wellness, 2026-10-23*\n"))
    (carnet / "Sujets" / "LIS.md").write_text(page("trip", "LIS",
        f"## Transports\n- {back} 18:00 — Vol LIS → CDG — TAP — LIS9Z8 {link(2)} — *Wellness, 2026-10-23*\n"))
    (carnet / "Sujets" / "Lyon.md").write_text(page("trip", "Lyon",
        f"## Transports\n- {commute} 08:04 — Train Paris → Lyon — SNCF — QWERTY {link(3)} — *Wellness, 2026-10-23*\n"))
    # a trip the person wrote (no document behind it), and a subject that links the city's page
    (carnet / "Sujets" / "Japon.md").write_text(page("trip", "Japon", "## Itinéraire\n- J1 : Tokyo\n",
                                                     f"start: {future}\nend: {future + timedelta(days=10)}\n"))
    (carnet / "Sujets" / "Cuisine.md").write_text(page("subject", "Cuisine", "## Notes\n- Recette vue à [[Lisbonne]]\n"))
    # purchases from before the classes: a registration and a TV, both with the computed legal warranty
    bought = day(-30)
    legal = bought.replace(year=bought.year + 2)
    (carnet / "Fiches" / "Achats.md").write_text(
        "---\nid: p0000000002\ntype: records\nrecords: purchase\n---\n# Achats\n\n"
        "| Date | Objet | Magasin | Prix | Fin de garantie | Source | id |\n| --- | --- | --- | --- | --- | --- | --- |\n"
        f"| {bought} | Inscription au trail | Njuko | 3 € | {legal} (légale) |  | r000010 |\n"
        f"| {bought} | Téléviseur | Darty | 499 € | {legal} (légale) |  | r000011 |\n"
        f"| 2020-01-05 | Grille-pain | Darty | 39 € | 2022-01-05 (légale) |  | r000012 |\n")

    run = tidy(bridge)
    assert (run["trips_renamed"], run["trips_merged"], run["trips_to_records"]) == (1, 1, 1), run
    title = f"Lisbonne — {MONTHS[out.month - 1]} {out.year}"
    # the two legs are one trip, named by its city and month, past: in the archives
    archived = carnet / "Archives" / "Voyages" / f"{title}.md"
    text = archived.read_text()
    assert "LIS7Q2" in text and "LIS9Z8" in text and "origin: archive" in text
    assert "  - LIS\n" in text and "  - Lisbonne\n" in text, text   # the old names: aliases
    assert not (carnet / "Sujets" / "Lisbonne.md").exists() and not (carnet / "Sujets" / "LIS.md").exists()
    # the links to it point to the new name, shown as before
    assert f"[[{title}|Lisbonne]]" in (carnet / "Sujets" / "Cuisine.md").read_text()
    # the commute train is a booking again: its line kept under the bookings' table, the page gone
    assert not (carnet / "Sujets" / "Lyon.md").exists()
    bookings = (carnet / "Fiches" / "Réservations.md").read_text()
    assert "## Lyon\n- Transports : " in bookings and "QWERTY" in bookings
    # the person's trip: moved to « Voyages », its name kept
    assert "# Japon\n" in (carnet / "Voyages" / "Japon.md").read_text()
    assert sorted(p.name for p in (carnet / "Sujets").iterdir()) == ["Cuisine.md"]
    # the past bookings archived (the same table in the archives), the coming dinner stays; its apostrophes too
    past = (carnet / "Archives" / "Fiches" / "Réservations.md").read_text()
    assert "r000001" in past and "r000002" in past and "r000003" in past and "r000004" not in past
    assert "Dîner chez l’Ami Jean | L'Ami Jean" in (carnet / "Fiches" / "Réservations.md").read_text()
    # the bookings name their trip
    assert f"| {title} |" in past
    # purchases: classes, the legal warranty only on goods, the toaster out of warranty archived
    achats = (carnet / "Fiches" / "Achats.md").read_text()
    assert "| Catégorie |" in achats
    assert f"| Inscription au trail | Njuko | 3 € | billet ou inscription |  |" in achats
    assert f"| Téléviseur | Darty | 499 € | bien durable | {legal} (légale) |" in achats
    assert "Grille-pain" in (carnet / "Archives" / "Fiches" / "Achats.md").read_text()

    # the app: trips apart, the archives' tile, the important purchases
    home = bridge.get("/v1/notebook", headers=AUTH).json()
    sections = {s["kind"]: s for s in home["sections"]}
    assert sections["subject"]["count"] == 1 and (sections["trip"]["count"], sections["trip"]["archived"]) == (1, 1)
    assert home["archives"]["counts"]["trip"] == 1 and home["archives"]["counts"]["booking"] == 3
    assert home["archives"]["counts"]["purchase"] == 1 and home["archives"]["title"] == "Archives"
    trips = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    assert [(t["title"], t["archived"]) for t in trips] == [("Japon", False), (title, True)]
    records = {t["type"]: t for t in bridge.get("/v1/notebook/records", headers=AUTH).json()["types"]}
    purchases = records["purchase"]
    assert [(i["item"], i["class_id"], i["important"], i["legal"]) for i in purchases["items"]] == [
        ("Téléviseur", "goods", True, True), ("Inscription au trail", "ticket", False, False)]
    assert purchases["important_count"] == 1 and purchases["warranty_min_eur"] == 50.0
    assert [i["item"] for i in purchases["archived"]] == ["Grille-pain"] and purchases["archived"][0]["archived"]
    shown = bridge.get("/v1/notebook/page", params={"path": f"Archives/Voyages/{title}.md"}, headers=AUTH).json()
    assert shown["archived"] is True and shown["kind"] == "trip"
    assert {b["reference"] for b in shown["trip"]["bookings"] if b["kind"] == "booking"} == {"LIS7Q2", "LIS9Z8"}
    # the agents still find what is archived, marked so
    found = text_of(call(bridge, "notebook_search", {"words": "Lisbonne"}))
    assert f"{title} (Archives/Voyages/{title}.md) [archived: past]" in found
    listed = text_of(call(bridge, "notebook_record", {"action": "list", "type": "booking"}))
    assert "r000001 [archived]" in listed and "r000004 ·" in listed

    # again: nothing more to do
    before = len(history(bridge, f"Archives/Voyages/{title}.md"))
    run = tidy(bridge)
    assert not any(k in run for k in ("trips_renamed", "trips_merged", "trips_to_records", "archived_trips",
                                      "archived_records", "classes", "trips_placed")), run
    assert len(history(bridge, f"Archives/Voyages/{title}.md")) == before

    # undone from the history: the rename moves the page back under its old name, and it is left so
    rename = next(c for c in history(bridge, f"Archives/Voyages/{title}.md") if c["action"] == "rename")
    assert bridge.post("/v1/notebook/restore", json={"change": rename["id"]}, headers=AUTH).status_code == 200
    assert "# LIS\n" in (carnet / "Sujets" / "LIS.md").read_text() and not archived.exists()
    tidy(bridge)
    assert [p.name for p in carnet.rglob("LIS.md")] == ["LIS.md"]
    # the commute page restored: back, and left as it is
    gone = next(c for c in history(bridge, "Sujets/Lyon.md") if c["action"] == "demote")
    assert bridge.post("/v1/notebook/restore", json={"change": gone["id"]}, headers=AUTH).status_code == 200
    tidy(bridge)
    assert [p.name for p in carnet.rglob("Lyon.md")] == ["Lyon.md"]
    # a record brought back by the person stays live
    back_again = bridge.post("/v1/notebook/records/archive", json={"type": "purchase", "id": "r000012", "archived": False},
                             headers=AUTH)
    assert back_again.status_code == 200 and back_again.json()["record"]["item"] == "Grille-pain"
    tidy(bridge)
    assert "Grille-pain" in (carnet / "Fiches" / "Achats.md").read_text()
    assert bridge.post("/v1/notebook/records/archive", json={"type": "purchase", "id": "r999999", "archived": True},
                       headers=AUTH).status_code == 404
    assert text_of(call(bridge, "notebook_record", {"action": "archive", "type": "purchase", "id": "r000012"})).startswith(
        "Archived: ")


def test_trips_from_bookings_follow_the_rule(bridges, fakes):
    """Only time away makes a trip: a commute train, a dinner, a lone flight leg stay bookings; a past trip goes
    straight to the archives; the same trip seen in two bookings is one page."""
    bridge = nightly(bridges, fakes)
    give_token(bridge)
    in_french(bridge)
    carnet = notebook(bridge) / "carnet"
    (carnet / "Fiches").mkdir(parents=True)
    hotel, leave, alone, dinner = day(20), day(23), day(40), day(50)
    old, old_back = day(-60), day(-57)
    (carnet / "Fiches" / "Réservations.md").write_text(
        "---\nid: p0000000001\ntype: records\nrecords: booking\n---\n# Réservations\n\n"
        "| Date | Quoi | Chez | Référence | Voyage | Source | id |\n| --- | --- | --- | --- | --- | --- | --- |\n"
        f"| {hotel} | Hôtel Gion à Kyoto | Hotel Gion | HG1 | Kyoto | {ref(11)} | r000011 |\n"
        f"| {leave} | Vol KIX → CDG | Air France | AF2 | KIX | {ref(12)} | r000012 |\n"
        f"| {alone} | Vol Paris → Nice | easyJet | EZ3 | Nice | {ref(13)} | r000013 |\n"
        f"| {dinner} | Table au restaurant Septime | Septime | S4 | Paris | {ref(14)} | r000014 |\n"
        f"| {old} | Train Paris → Bordeaux | SNCF | B5 | Bordeaux | {ref(15)} | r000015 |\n"
        f"| {old_back} | Train Bordeaux → Paris | SNCF | B6 |  | {ref(16)} | r000016 |\n")
    run = tidy(bridge)
    assert run["trips_created"] == 2, run
    kyoto = list((carnet / "Voyages").glob("*Kyoto — *.md"))
    assert [p.name.split("Kyoto — ")[1] for p in kyoto] == [f"{MONTHS[hotel.month - 1]} {hotel.year}.md"]
    assert "HG1" in kyoto[0].read_text() and "AF2" in kyoto[0].read_text()
    bordeaux = list((carnet / "Archives" / "Voyages").glob("*Bordeaux*.md"))
    assert len(bordeaux) == 1 and "B5" in bordeaux[0].read_text() and "B6" in bordeaux[0].read_text()
    assert not list(carnet.rglob("*Nice*.md")) and not list(carnet.rglob("*Septime*.md"))
    # a trip the person wrote, over: archived; brought back from the history, it stays where they put it
    (carnet / "Voyages" / "Ski.md").write_text(page("trip", "Ski", "## Hébergements\n- Chalet\n",
                                                    f"start: {day(-12)}\nend: {day(-5)}\n"))
    assert tidy(bridge)["archived_trips"] == 1
    move = next(c for c in history(bridge, "Archives/Voyages/Ski.md") if c["action"] == "archive")
    assert bridge.post("/v1/notebook/restore", json={"change": move["id"]}, headers=AUTH).status_code == 200
    assert (carnet / "Voyages" / "Ski.md").exists()
    tidy(bridge)
    assert (carnet / "Voyages" / "Ski.md").exists() and not (carnet / "Archives" / "Voyages" / "Ski.md").exists()
    # deleted by the person: not made again
    path = f"Voyages/{kyoto[0].name}"
    assert bridge.delete("/v1/notebook/page", params={"path": path}, headers=AUTH).status_code == 200
    assert "trips_created" not in tidy(bridge)


def test_a_purchase_s_class_comes_from_the_agent_or_its_words(bridge):
    give_token(bridge)
    in_french(bridge)
    added = call(bridge, "notebook_record", {"action": "add", "type": "purchase",
                                             "fields": {"date": "2026-09-01", "item": "Place de concert", "price": "45 €"}})
    assert "billet ou inscription" in text_of(added) and "légale" not in text_of(added)
    added = call(bridge, "notebook_record", {"action": "add", "type": "purchase",
                                             "fields": {"date": "2026-09-01", "item": "Lampe", "price": "20 €",
                                                        "class": "goods"}})
    assert "bien durable" in text_of(added) and "2028-09-01 (légale)" in text_of(added)
    view = next(t for t in bridge.get("/v1/notebook/records", headers=AUTH).json()["types"] if t["type"] == "purchase")
    assert [(i["item"], i["important"]) for i in view["items"]] == [("Lampe", False), ("Place de concert", False)]
