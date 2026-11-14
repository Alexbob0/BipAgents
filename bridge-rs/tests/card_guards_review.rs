//! Independent review of card-guards (97121f670 + int-release-next cf04b8217), 14/11: each test states what the
//! branch should do; all failed on it (the reviewer's run), kept here as they were written, before the fixes.
//! Only published test numbers (Visa 4242…) and synthetic IBAN / NIR values with valid check digits.

use bipbridge::browser_mcp::{refused_address, script_card_refusal, PROVIDER_SCRIPT_REFUSED};
use bipbridge::card_details::{asks_for_card, hide_card_numbers, holds_card_number};
use serde_json::{json, Value};

fn dict(v: Value) -> serde_json::Map<String, Value> {
    v.as_object().cloned().unwrap()
}

/// R1. ask_secret: Alex's rule is « never number / expiry / holder through a card ». Plain French (and other
/// languages a model may use) gets through the word lists: the card is raised; for the number, only the value check
/// at give() saves it; the expiry, the holder and the security code are kept as a secret.
#[test]
fn rv1_ask_secret_refuses_a_cards_details_however_worded() {
    // (kind, [name, identifier_label, identifier, reason]) as connectors.rs passes them
    let asked: &[(&str, [&str; 4])] = &[
        ("password", ["visa", "", "", "Le numéro de ta Visa, pour finir la commande"]),
        ("password", ["pharmacie", "Les 16 chiffres de ta carte", "", "Pour payer ta commande"]),
        ("code", ["pharmacie_date", "", "", "Jusqu'à quand ta carte est valable ?"]),
        ("code", ["pharmacie_nom", "", "", "Le nom imprimé sur ta carte"]),
        ("code", ["pharmacie_code", "", "", "Les 3 chiffres derrière ta carte"]),
        ("code", ["carte_fin", "", "", "Date de fin de ta carte"]),
        ("password", ["mastercard", "", "", "Le numéro de ta Mastercard"]),
        ("password", ["shop", "", "", "The long number on the front of your Visa"]),
        ("password", ["negozio", "", "", "Numero della carta di credito"]),
        ("password", ["loja", "", "", "Número do cartão de crédito"]),
        ("password", ["winkel", "", "", "Kaartnummer van je bankpas"]),
        // the meaning split over two fields, each harmless alone
        ("password", ["carte", "Numéro", "", "Pour la pharmacie"]),
    ];
    let through: Vec<String> = asked
        .iter()
        .filter(|(kind, texts)| !asks_for_card(kind, texts))
        .map(|(kind, texts)| format!("{kind} {texts:?}"))
        .collect();
    assert!(through.is_empty(), "ask_secret would raise a card for these ({}):\n{}", through.len(), through.join("\n"));
}

/// R2. False positives: a French IBAN written as usual (groups of 4) holds a « card number » in ~15 % of cases (a
/// Diners/Mir/Maestro-looking run of whole groups); a man's NIR (French social security number, 15 digits starting
/// with 1) in ~10 % (UATP). Both are masked in what the agent reads and refused or erased when typed (the IBAN even
/// in a field labelled « IBAN »: guard_after has no « other number » exception; see the Chrome test).
#[test]
fn rv2_an_iban_or_a_social_security_number_is_no_card_number() {
    for text in [
        "FR98 0378 3840 1165 3551 5752 774", // synthetic FR IBAN, valid mod-97 check digits
        "IBAN : FR98 0378 3840 1165 3551 5752 774 — BIC AGRIFRPP",
        "1 48 10 09 262 121 10",              // synthetic NIR, valid key (97 - n mod 97)
        "148100926212110",
    ] {
        assert!(!holds_card_number(text), "taken for a card number: {text:?} -> {:?}", hide_card_numbers(text));
    }
}

/// R3. navigate / new_tab with a javascript: address runs a script in the page with none of the guard's checks
/// (navigate is not in GUARDED: no look before, no catch-up after, no script_card_refusal): see the Chrome test, where
/// it writes a card's number into the shop's card field.
#[test]
fn rv3_a_javascript_address_is_refused_like_a_script() {
    for url in [
        "javascript:void(document.getElementById('numero').value='4242424242424242')",
        " JavaScript:void(0)",
        "java\tscript:void(0)",
    ] {
        for tool in ["navigate", "new_tab"] {
            assert!(refused_address(tool, &dict(json!({"url": url}))).is_some(), "{tool} {url:?} not refused");
        }
    }
}

/// R4. The author's residual risk, shown: a script of the agent's that sends the tab to a payment service's page is
/// refused only when the provider's host is written out; built or encoded, it goes through, and the page reached
/// that way counts as « the shop's own link » (card_fill.rs reached_from: transition « link »).
#[test]
fn rv4_a_script_sending_the_tab_to_a_provider_is_refused_however_written() {
    let empty = json!({"pw": {}, "known": [], "boxes": [], "cards": {}, "cardtext": false});
    for script in [
        "location.href = 'https://payment.cawl-' + 'solutions.fr/hostedcheckout/PaymentMethods/Selection/x'",
        // https://payment.cawl-solutions.fr/hostedcheckout/x
        "location.href = atob('aHR0cHM6Ly9wYXltZW50LmNhd2wtc29sdXRpb25zLmZyL2hvc3RlZGNoZWNrb3V0L3g=')",
        "const a = document.createElement('a'); a.href = ['https://secure', 'payzen', 'eu/vads-payment/'].join('.'); \
         document.body.append(a); a.click()",
        // the host written out in full, as a link the agent then clicks with click_element (« link »)
        "document.body.insertAdjacentHTML('beforeend', '<a id=\"z\" href=\"https://payment.cawl-solutions.fr/hostedcheckout/x\">Payer</a>')",
    ] {
        assert_eq!(script_card_refusal(script, Some(&empty)), Some(PROVIDER_SCRIPT_REFUSED), "{script}");
    }
}
