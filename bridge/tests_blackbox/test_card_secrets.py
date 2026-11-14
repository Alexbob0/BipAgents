"""The secure-field card never asks for a payment card's details (card-guards, 14/11).

On 14/11 Alex's agent (Vie, his own test card, pharmaciedesdrakkars.com) could not get the vault's card fill to work on
the shop's payment page; it then asked him for the card's number through a secure-field card (`ask_secret`) and typed
it into the form itself, then asked for the holder, the expiry and the security code the same way. Alex's rule: through
a card the person gives only the security code, inside the payment's own flow (« Oui, payer »); never the number, the
expiry or the holder. So `ask_secret` refuses such a request, by its kind or its words (French, English, German,
Spanish), before any card is raised; and a card's number given in any secure-field card is never kept nor handed back.
"""
import json
from concurrent.futures import ThreadPoolExecutor

from bbkit.common import AUTH
from test_connectors import cards, rpc, setup, text, tool, wait_card, TOOLS

PANIER = "« Panier prêt »"


def test_a_secure_field_card_never_asks_for_a_cards_details(bridges, fakes):
    bridge, _, _ = setup(bridges, fakes)
    description = next(t for t in rpc(bridge, TOOLS, "tools/list")["tools"] if t["name"] == "ask_secret")["description"]
    assert "payment card" in description and "pay_with_card" in description and PANIER in description
    asked = [
        # by its kind
        {"name": "carte", "site": "pharmaciedesdrakkars.com", "kind": "card", "reason": "Pour payer ta commande"},
        {"name": "cvc", "site": "pharmaciedesdrakkars.com", "kind": "cvc", "reason": "Pour payer"},
        # by its words: what Vie asked on 14/11, and the same in four languages
        {"name": "drakkars", "site": "pharmaciedesdrakkars.com", "kind": "password", "use": "browser",
         "identifier_label": "Numéro de ta carte bancaire", "reason": "Pour payer ta commande"},
        {"name": "drakkars_exp", "site": "pharmaciedesdrakkars.com", "kind": "code",
         "identifier_label": "Date d'expiration (MM/AA)", "reason": "Le formulaire bancaire la demande"},
        {"name": "drakkars_titulaire", "site": "pharmaciedesdrakkars.com", "kind": "code",
         "reason": "Titulaire de la carte, pour le paiement"},
        {"name": "crypto", "site": "pharmaciedesdrakkars.com", "kind": "code", "reason": "Le cryptogramme au dos"},
        {"name": "card_number", "site": "shop.example", "kind": "token", "reason": "to pay"},
        {"name": "shop", "site": "shop.example", "kind": "code", "reason": "Kartennummer und Prüfnummer"},
        {"name": "tienda", "site": "shop.example", "kind": "code", "reason": "número de tarjeta de crédito"},
        {"name": "shop", "site": "shop.example", "kind": "password", "identifier": "4242 4242 4242 4242",
         "reason": "login"},
    ]
    for arguments in asked:
        result = tool(bridge, "ask_secret", arguments)
        said = text(result)
        assert result["isError"] and said.startswith("Refused: a card in the app never asks"), (arguments, said)
        assert "pay_with_card" in said and "hand_over_payment" in said and PANIER in said
    assert cards(bridge, "bip-secret-") == []   # no card was ever raised

    # a password for a site, a code, a loyalty card: still asked
    with ThreadPoolExecutor(1) as pool:
        call = pool.submit(tool, bridge, "ask_secret", {
            "name": "carrefour", "site": "carrefour.fr", "kind": "password", "use": "browser",
            "identifier_label": "Numéro de carte de fidélité", "reason": "Pour ton compte Carrefour"})
        card = wait_card(bridge, "bip-secret-")
        url = f"/v1/vault/secrets/requests/{card['request_id']}"
        # a card's number given in it, as the value or the identifier: never kept, never handed back
        for given in ({"value": "4242 4242 4242 4242", "identifier": "camille@example.org"},
                      {"value": "Fake-Pa55word", "identifier": "4242424242424242"}):
            refused = bridge.post(url, headers=AUTH, json={"allow": True, **given})
            assert refused.status_code == 400 and "never kept" in refused.text, refused.text
        kept = bridge.post(url, headers=AUTH, json={"allow": True, "value": "Fake-Pa55word",
                                                    "identifier": "camille@example.org"})
        assert kept.status_code == 200, kept.text
        result = call.result(timeout=60)
    assert not result["isError"] and "4242" not in json.dumps(result), result
