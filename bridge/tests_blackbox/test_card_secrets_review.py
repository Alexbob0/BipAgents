"""Independent review of card-guards (14/11): ask_secret asked for a card's details in ordinary words. Alex's rule:
through a card of the app, never the number, the expiry or the holder (the security code only in « Oui, payer »).
Failed on the branch: the card was raised, and what the person gives for the expiry, the holder and the security code
was kept. No real card data (an expiry, a name and a code made up)."""
import concurrent.futures

from bbkit.common import AUTH
from test_connectors import cards, setup, text, tool, wait_card

ASKED = [
    # (arguments, what the person types)
    ({"name": "drakkars_date", "site": "pharmaciedesdrakkars.com", "kind": "code",
      "reason": "Jusqu'à quand ta carte est valable ?"}, "12/30"),
    ({"name": "drakkars_nom", "site": "pharmaciedesdrakkars.com", "kind": "code",
      "reason": "Le nom imprimé sur ta carte"}, "ALEX MARTIN"),
    ({"name": "drakkars_code", "site": "pharmaciedesdrakkars.com", "kind": "code",
      "reason": "Les 3 chiffres derrière ta carte"}, "737"),
    ({"name": "visa", "site": "pharmaciedesdrakkars.com", "kind": "password", "use": "browser",
      "identifier_label": "Les 16 chiffres de ta carte", "reason": "Pour payer ta commande"}, None),
]


def test_rv_ask_secret_never_raises_a_card_for_a_cards_details_in_other_words(bridges, fakes):
    bridge, _, _ = setup(bridges, fakes)
    raised = []
    for arguments, given in ASKED:
        seen = {c["request_id"] for c in cards(bridge, "bip-secret-")}
        with concurrent.futures.ThreadPoolExecutor(1) as pool:
            call = pool.submit(tool, bridge, "ask_secret", arguments)
            try:
                result = call.result(timeout=4)
            except concurrent.futures.TimeoutError:
                result = None
            if result is not None and result.get("isError") and text(result).startswith("Refused"):
                continue   # refused at once, as it should
            card = wait_card(bridge, "bip-secret-", seen)
            url = f"/v1/vault/secrets/requests/{card['request_id']}"
            if given is None:
                sent = bridge.post(url, headers=AUTH, json={"allow": False})
                raised.append(f"{arguments['identifier_label']!r}: card raised (the person refused it here)")
            else:
                sent = bridge.post(url, headers=AUTH, json={"allow": True, "value": given})
                raised.append(f"{arguments['reason']!r}: card raised, {given!r} -> HTTP {sent.status_code} "
                              f"({'KEPT' if sent.status_code == 200 else sent.text[:80]})")
            call.result(timeout=60)
    assert not raised, "ask_secret asked for a card's details:\n" + "\n".join(raised)
