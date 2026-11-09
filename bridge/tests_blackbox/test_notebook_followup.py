"""Alex's answers of 2026-11-03 (carnet review): light person pages folded at night into their group's page (their
names kept, a fact never lost), and a trip whose page says it was called off archived at night."""
from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, notebook, tidy


def note(bridge, page, facts, section="who"):
    done = call(bridge, "notebook_write", {"page": page, "kind": "person", "section": section, "facts": facts})
    assert done["isError"] is False, text_of(done)


def test_light_pages_fold_into_their_group_and_keep_their_names(bridge):
    give_token(bridge)
    in_french(bridge)
    note(bridge, "Paul Lambert", ["Parrain d'Emma"])
    note(bridge, "Emma", ["Cousine, sœur de Julien", "Infirmière à Lille"])
    note(bridge, "Julien", ["Cousin", "Aime le vélo", "Cherche un appartement à Bordeaux"])
    note(bridge, "Françoise", ["Ma mère"])
    run = tidy(bridge)
    assert run, run
    people = {p["title"] for p in bridge.get("/v1/notebook/pages", params={"kind": "person"}, headers=AUTH).json()["pages"]}
    assert {"Julien", "Françoise"} <= people and not {"Paul Lambert", "Emma"} & people, people
    family = (notebook(bridge) / "carnet" / "Domaines" / "Famille élargie.md").read_text()
    assert "- Emma : Infirmière à Lille — *" in family and "  - Emma" in family, family
    others = (notebook(bridge) / "carnet" / "Domaines" / "Connaissances.md").read_text()
    assert "Paul Lambert : Parrain d'Emma" in others
    # the old name still opens something, and an agent's new fact about her lands on the group
    found = text_of(call(bridge, "notebook_read", {"page": "Emma"}))
    assert "Famille élargie" in found
    note(bridge, "Emma", ["De garde ce week-end"], section="")
    assert "Emma : De garde ce week-end" in (notebook(bridge) / "carnet" / "Domaines" / "Famille élargie.md").read_text()


def test_a_trip_called_off_is_archived_at_night(bridge):
    give_token(bridge)
    in_french(bridge)
    for page, facts in [("Lyon, nov. 2030", ["Week-end chez mes parents du 27 au 29 novembre 2030"]),
                        ("Rome, mai 2030", ["Voyage du 3 au 8 mai 2030", "Vol AZ333 annulé, remplacé par AZ335"])]:
        done = call(bridge, "notebook_write", {"page": page, "kind": "trip", "section": "dates", "facts": facts,
                                              "start": facts[0][-4:] + ("-11-27" if "Lyon" in page else "-05-03"),
                                              "end": facts[0][-4:] + ("-11-29" if "Lyon" in page else "-05-08")})
        assert done["isError"] is False, text_of(done)
    edit = call(bridge, "notebook_write", {"page": "Lyon, nov. 2030", "kind": "trip",
                                          "facts": ["Week-end annulé : mes parents ne seront pas là"]})
    assert edit["isError"] is False, text_of(edit)
    tidy(bridge)
    trips = bridge.get("/v1/notebook/pages", params={"kind": "trip"}, headers=AUTH).json()["pages"]
    by = {p["title"]: p for p in trips}
    assert by["Lyon, nov. 2030"]["path"].startswith("Archives/"), by["Lyon, nov. 2030"]
    assert by["Rome, mai 2030"]["path"].startswith("Voyages/"), "a cancelled flight is not a cancelled trip"
    found = text_of(call(bridge, "notebook_search", {"words": "Lyon"}))
    assert "Lyon, nov. 2030" in found
