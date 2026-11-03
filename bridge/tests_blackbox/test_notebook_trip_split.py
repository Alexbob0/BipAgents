"""A page mixing several journeys is proposed to be split into real trips, with the person's yes (2026-10-29, from a
real Carnet: one « Voyages » page holding an Amsterdam flight of December 2021, an airport parking of July 2022 and a
car rental of October 2022, filed by an agent from archived mails)."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, notebook, tidy

VOYAGES = (
    "---\ntype: trip\nid: pvoyages001\n---\n# Voyages\n\n## Transports\n"
    "- Vol AF1240 Paris → Amsterdam le 18/12/2021 à 07:15\n- Vol retour Amsterdam → Paris le 22/12/2021\n"
    "- Remboursement Air France le 31/12/2021\n- Parking aéroport le 22/07/2022\n"
    "- Location voiture le 01/10/2022\n- Location voiture retour le 08/10/2022\n\n## Notes\n- Garder les factures\n")
QUESTION = ("Séparer « Voyages » en 3 voyages : Amsterdam, déc. 2021 · Lisbonne, juil. 2022 · Voyage d'oct. 2022 ?")


def the_page(bridge):
    give_token(bridge)
    in_french(bridge)
    folder = notebook(bridge) / "carnet" / "Voyages"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "Voyages.md").write_text(VOYAGES)
    added = call(bridge, "notebook_record", {"action": "add", "type": "booking",
                                             "fields": {"date": "2022-07-23", "what": "Hôtel à Lisbonne",
                                                        "reference": "LIS777", "trip": "[[Voyages]]"}})
    assert added["isError"] is False, text_of(added)


def splits(bridge):
    listed = bridge.get("/v1/notebook/proposals", headers=AUTH).json()["proposals"]
    return [p for p in listed if p["kind"] == "split"]


def test_one_card_then_yes_makes_the_trips_and_keeps_the_rest(bridge):
    the_page(bridge)
    run = tidy(bridge)
    assert run.get("splits") == 1, run
    assert "splits" not in tidy(bridge), "one card per page"
    cards = splits(bridge)
    assert len(cards) == 1 and cards[0]["state"] == "open" and cards[0]["title"] == QUESTION
    card = cards[0]
    assert [t["title"] for t in card["trips"]] == ["Amsterdam, déc. 2021", "Lisbonne, juil. 2022", "Voyage d'oct. 2022"]
    home = bridge.get("/v1/notebook", headers=AUTH).json()
    assert [p["id"] for p in home["proposals"]] == [card["id"]]
    view = bridge.get("/v1/notebook/page", params={"path": "Voyages/Voyages.md"}, headers=AUTH).json()
    assert view["split"]["id"] == card["id"]
    # « Séparer »
    answer = bridge.post(f"/v1/notebook/proposals/{card['id']}", json={"accept": True}, headers=AUTH)
    assert answer.status_code == 200, answer.text
    made = answer.json()["split"]
    assert made["already"] is False and len(made["trips"]) == 3
    again = bridge.post(f"/v1/notebook/proposals/{card['id']}", json={"accept": True}, headers=AUTH).json()["split"]
    assert again["already"] is True and again["trips"] == made["trips"]
    trips = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    assert sorted(p["title"] for p in trips) == ["Amsterdam, déc. 2021", "Lisbonne, juil. 2022", "Voyage d'oct. 2022"]
    amsterdam = next(p for p in trips if p["title"] == "Amsterdam, déc. 2021")
    assert (amsterdam["start"], amsterdam["end"]) == ("2021-12-18", "2021-12-22")
    page = bridge.get("/v1/notebook/page", params={"path": amsterdam["path"]}, headers=AUTH).json()
    for line in ["Vol AF1240", "Vol retour", "Remboursement Air France"]:
        assert line in page["text"], page["text"]
    lisbon = next(p for p in trips if p["title"] == "Lisbonne, juil. 2022")
    lisbon_view = bridge.get("/v1/notebook/page", params={"path": lisbon["path"]}, headers=AUTH).json()
    assert "Parking aéroport" in lisbon_view["text"]
    assert any(b.get("reference") == "LIS777" for b in lisbon_view["trip"]["bookings"]), "its record follows it"
    # The page: a subject linking to its trips, what has no date kept, nothing left that moved.
    rest = bridge.get("/v1/notebook/page", params={"path": "Voyages/Voyages.md"}, headers=AUTH).json()
    assert rest["kind"] == "subject" and rest["split"] is None
    assert "Garder les factures" in rest["text"] and "Parking" not in rest["text"]
    # its trips point back to it (no line per trip on it: they read as its status, 2026-11-03)
    assert {link["title"] for link in rest["backlinks"]} >= {"Amsterdam, déc. 2021", "Lisbonne, juil. 2022",
                                                             "Voyage d'oct. 2022"}
    assert all(link["path"] for link in rest["backlinks"])
    assert not [s for s in rest["sections"] if s["id"] == "status"], rest["sections"]
    card = next(p for p in bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"]
                if p["path"] == "Voyages/Voyages.md")
    assert card["status"] is None, card
    # Every move in the history, and undone from there.
    history = bridge.get("/v1/notebook/history", params={"path": "Voyages/Voyages.md"}, headers=AUTH).json()["changes"]
    assert history[0]["action"] == "split"  # the change before it: the page as it was (refiled, or as found)
    restore = bridge.post("/v1/notebook/restore", json={"change": history[1]["id"]}, headers=AUTH)
    assert restore.status_code == 200, restore.text
    assert "Parking aéroport" in (notebook(bridge) / "carnet" / "Voyages" / "Voyages.md").read_text()
    # undone by the person: not proposed again
    assert "splits" not in tidy(bridge)
    assert [p["state"] for p in splits(bridge)] == ["accepted"]


def test_left_as_it_is_no_card_again(bridge):
    the_page(bridge)
    assert tidy(bridge).get("splits") == 1
    card = splits(bridge)[0]
    answer = bridge.post(f"/v1/notebook/proposals/{card['id']}", json={"accept": False}, headers=AUTH)
    assert answer.status_code == 200, answer.text
    assert "splits" not in tidy(bridge)
    assert [p["state"] for p in splits(bridge)] == ["declined"]
    assert bridge.get("/v1/notebook", headers=AUTH).json()["proposals"] == []
    assert bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"] == []
    # The person asks their agent: proposed again.
    asked = call(bridge, "notebook_edit", {"page": "Voyages", "split_trips": True})
    assert asked["isError"] is False and "Proposed to the person" in text_of(asked) and QUESTION in text_of(asked)
    assert [p["state"] for p in splits(bridge)] == ["open", "declined"]


ALEX = ("---\ntype: subject\n---\n# Voyages\n\n## Où on en est\n"
        "- Vol Paris → Amsterdam le 18/12/2021 à 07:15 — *toi, 2026-10-30*\n"
        "- Vol Amsterdam → Paris le 22/12/2021 — *toi, 2026-10-30*\n"
        "- Vol Paris → Amsterdam le 16/12/2026 - et retour vendredi 18/12/2026 — *toi, 2026-10-30*\n")


def test_alex_s_page_is_split_by_places_and_leaves_no_status_lines(bridge):
    """What Alex saw on 2026-11-03: « Où on en est » holding « et retour vendredi, déc. 2026 : 20 déc. 2026 » by « toi »."""
    give_token(bridge)
    in_french(bridge)
    put = bridge.put("/v1/notebook/page", json={"path": "Sujets/Voyages.md", "text": ALEX}, headers=AUTH)
    assert put.status_code == 200, put.text
    assert tidy(bridge).get("splits") == 1
    card = splits(bridge)[0]
    assert [t["title"] for t in card["trips"]] == ["Amsterdam, déc. 2021", "Amsterdam, déc. 2026"], card
    made = bridge.post(f"/v1/notebook/proposals/{card['id']}", json={"accept": True}, headers=AUTH).json()["split"]
    assert len(made["trips"]) == 2
    trips = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    new = next(p for p in trips if p["title"] == "Amsterdam, déc. 2026")
    assert (new["start"], new["end"]) == ("2026-12-16", "2026-12-18"), "the return said in the same line"
    subjects = bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"]
    assert not [p for p in subjects if p["title"] == "Voyages"], "nothing left on it: it goes (its trips in Voyages)"


def test_the_lines_older_splits_wrote_are_put_right_at_night(bridge):
    give_token(bridge)
    in_french(bridge)
    root = notebook(bridge) / "carnet"
    for path, text in [
        ("Sujets/Voyages.md", "---\ntype: subject\n---\n# Voyages\n\n## Où on en est\n"
                              "- [[Amsterdam, déc. 2021]] : 18 – 22 déc. 2021 — *toi, 2026-11-01*\n"
                              "- [[et retour vendredi, déc. 2026]] : 20 déc. 2026 — *toi, 2026-11-01*\n"),
        ("Archives/Voyages/Amsterdam, déc. 2021.md",
         "---\ntype: trip\norigin: split\nsplit_from: Voyages\nstart: 2021-12-18\nend: 2021-12-22\n---\n"
         "# Amsterdam, déc. 2021\n\n## Transports\n- Vol Paris → Amsterdam le 18/12/2021\n"),
        ("Voyages/et retour vendredi, déc. 2026.md",
         "---\ntype: trip\norigin: split\nsplit_from: Voyages\nstart: 2026-12-16\nend: 2026-12-20\n---\n"
         "# et retour vendredi, déc. 2026\n\n## Transports\n"
         "- Vol Paris → Amsterdam le 16/12/2026 - et retour vendredi 20/12/2026\n"),
    ]:
        (root / path).parent.mkdir(parents=True, exist_ok=True)
        (root / path).write_text(text)
    tidy(bridge)
    subjects = bridge.get("/v1/notebook/pages", params={"kind": "subject"}, headers=AUTH).json()["pages"]
    assert not [p for p in subjects if p["title"] == "Voyages"], subjects
    trips = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    assert sorted(p["title"] for p in trips) == ["Amsterdam, déc. 2021", "Amsterdam, déc. 2026"], trips
