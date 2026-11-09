"""Subscriptions kept once and read right (Alex, 2026-11-03): the same service from an invoice, a bank line and the
person is one record; periods in the Carnet's language and one price format; a past renewal moved on by its period
once at most, else shown as the last known one, never as « next ». The tables written before are put right at night."""
from datetime import date, timedelta

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from test_notebook import in_french, notebook, tidy


def iso(days):
    return (date.today() + timedelta(days=days)).isoformat()


def subscriptions(bridge):
    view = bridge.get("/v1/notebook/records", headers=AUTH).json()
    return next(t for t in view["types"] if t["type"] == "subscription")["items"]


def add(bridge, **fields):
    added = call(bridge, "notebook_record", {"action": "add", "type": "subscription", "fields": fields})
    assert added["isError"] is False, text_of(added)
    return text_of(added)


def test_one_service_under_three_names_is_one_record(bridge):
    give_token(bridge)
    in_french(bridge)
    add(bridge, service="Max plan - 20x", price="€196.57", period="monthly", renewal=iso(20), source="archive:a/b/1")
    second = add(bridge, service="Claude Max 20x (Anthropic, PBC)", price="196,57 € TTC")
    assert second.startswith("Updated ["), second
    add(bridge, service="Claude Max 20x (Ultra)")
    add(bridge, service="Anthropic Max plan - 5x", price="98 €", period="mensuel", renewal=iso(-38))
    items = subscriptions(bridge)
    assert [i["service"] for i in items] == ["Claude Max 20x (Anthropic, PBC)", "Anthropic Max plan - 5x"], items
    twenty, five = items
    assert (twenty["price"], twenty["period"], twenty["in_days"]) == ("196,57 €", "mensuel", 20)
    assert twenty["source"] == "archive:a/b/1"
    # 38 days ago, monthly: two renewals unheard of, only the last known one
    assert five["next_date"] is None and five["in_days"] is None
    assert (five["last_date"], five["last_title"]) == (iso(-38), "Dernier renouvellement connu")


def test_a_yearly_renewal_of_last_september_is_next_september(bridge):
    give_token(bridge)
    in_french(bridge)
    past = date.today() - timedelta(days=49)
    add(bridge, service="Le Monde", price="99 € par an", renewal=past.isoformat())
    item = subscriptions(bridge)[0]
    assert item["period"] == "annuel" and item["price"] == "99 €", item
    assert item["next_date"] == past.replace(year=past.year + 1).isoformat() and item["in_days"] > 300


def test_the_tables_written_before_are_put_right_at_night(bridge):
    give_token(bridge)
    in_french(bridge)
    folder = notebook(bridge) / "carnet" / "Fiches"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "Abonnements.md").write_text(
        "---\ntype: records\nrecords: subscription\n---\n# Abonnements\n\n"
        "| Service | Prix | Période | Prochain renouvellement | Comment résilier | Source | id |\n"
        "| --- | --- | --- | --- | --- | --- | --- |\n"
        f"| Max plan - 20x | €196.57 | monthly | {iso(20)} |  | archive:a/b/1 | r000001 |\n"
        "| Claude Max 20x (Anthropic, PBC) | 196,57 € TTC |  |  |  |  | r000002 |\n"
        "| Claude Max 20x (Ultra) |  |  |  |  |  | r000003 |\n"
        "| Spotify | 10,99 EUR | yearly |  |  |  | r000004 |\n")
    run = tidy(bridge)
    assert run, run
    items = subscriptions(bridge)
    assert [(i["id"], i["service"], i["price"], i["period"]) for i in items] == [
        ("r000001", "Claude Max 20x (Anthropic, PBC)", "196,57 €", "mensuel"),
        ("r000004", "Spotify", "10,99 €", "annuel")], items
    history = bridge.get("/v1/notebook/history", params={"path": "Fiches/Abonnements.md"}, headers=AUTH).json()
    assert history["changes"][0]["action"] == "tidy", "a change of the history, undone from there"
