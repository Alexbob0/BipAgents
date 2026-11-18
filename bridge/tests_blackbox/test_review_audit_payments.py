"""Review of audit-payments-1117 (18/11): what the fixes still get wrong, each test stating the expected behaviour
(red = finding proved). Fake shop/DevTools (test_card_payments.FakeChrome) and real Chromium (BIP_CHROME), Visa's
published test number only. Plus a few green checks of the cases the brief asked about."""
import functools
import http.server
import threading

import pytest

from bbkit.common import AUTH
from test_card_payments import ARGS, CARD, FakeChrome, pay, pay_bridge, tool, turn_on


def card_of(bridge):
    return bridge.get("/v1/vault", headers=AUTH).json()["items"][0]["card"]


# -- R1: a payment never pressed (another amount on the button) still counts on the card -----------------------------

def test_r1_a_payment_whose_button_was_never_pressed_does_not_eat_the_cards_cap(bridges, fakes):
    chrome = FakeChrome()
    chrome.button = "payer 1 250,00 €"
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "cap_eur": "20"})
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer 12,50 €"})["content"][0]["text"]
        assert chrome.clicks == [] and "was not pressed" in confirmed, confirmed
        spent = card_of(bridge).get("spent", {})
        # nothing was paid (the bridge never pressed): the person's 20 EUR cap should still be whole
        assert spent.get("EUR", 0) == 0, f"not paid (amount_differs), yet the card counts spent={spent}"
        again, _ = pay(bridge, ARGS)
        assert again["isError"] is False, f"the next 12.50 EUR on the same card is refused: {again}"
    finally:
        chrome.stop()


# -- R2: Swiss writing of a number (the browser's own Intl, de-CH) read as another amount -----------------------------

def test_r2_a_swiss_written_total_is_not_taken_for_another_amount_fake(bridges, fakes):
    chrome = FakeChrome()
    chrome.button = "payer chf 1’250.00"   # Intl.NumberFormat('de-CH', CHF) writes U+2019 between thousands
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)   # no cap: CHF allowed
        result, asked = pay(bridge, {**ARGS, "amount": "1250", "currency": "CHF"})
        assert result["isError"] is False, (result, asked)
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
        assert chrome.clicks == ["https://shop.example/checkout"], (
            f"approved 1250.00 CHF, the button shows CHF 1’250.00, not pressed: {confirmed}")
    finally:
        chrome.stop()


SWISS = """<!doctype html><html lang="de-CH"><head><meta charset="utf-8"><title>Zahlung</title></head><body>
<form onsubmit="event.preventDefault(); location.href='done.html'">
  <p><label>Kartennummer <input id="nr" autocomplete="cc-number"></label></p>
  <p><label>Ablauf <input id="exp" autocomplete="cc-exp" placeholder="MM/JJ"></label></p>
  <p><label>CVC <input id="cvc" autocomplete="cc-csc"></label></p>
  <p><label>Name <input id="name" autocomplete="cc-name"></label></p>
  <p><button type="submit" id="pay">Bezahlen</button></p>
</form>
<script>document.getElementById('pay').textContent = 'Bezahlen ' +
  new Intl.NumberFormat('de-CH', {style: 'currency', currency: 'CHF'}).format(1250);</script>
</body></html>"""

SUP = """<!doctype html><html lang="fr"><head><meta charset="utf-8"><title>Paiement</title></head><body>
<form onsubmit="event.preventDefault(); location.href='done.html'">
  <p><label>Numéro de carte <input id="numero" autocomplete="cc-number"></label></p>
  <p><label>Expiration <input id="exp" autocomplete="cc-exp" placeholder="MM/AA"></label></p>
  <p><label>Cryptogramme <input id="crypto" autocomplete="cc-csc"></label></p>
  <p><label>Titulaire <input id="titulaire" autocomplete="cc-name"></label></p>
  <p><button type="submit">Payer 12<sup>50</sup> €</button></p>
</form></body></html>"""


def chromium_pay(bridges, fakes, tmp_path, page, arguments, button):
    from test_card_payments_chrome import CHROME, Browser
    if CHROME is None:
        pytest.skip("no Chromium")
    (tmp_path / "pay.html").write_text(page, encoding="utf-8")
    (tmp_path / "done.html").write_text("<!doctype html><title>Merci</title><p>Commande payée", encoding="utf-8")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(tmp_path))
    handler.log_message = lambda *a: None
    shop = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=shop.serve_forever, daemon=True).start()
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/pay.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]})
        result, asked = pay(bridge, arguments)
        assert result["isError"] is False, (result, asked)
        confirmed = tool(bridge, "confirm_payment", {"button": button})["content"][0]["text"]
        return confirmed, [u.split("/")[-1] for u in browser.pages()]
    finally:
        browser.stop()
        shop.shutdown()


def test_r2_a_swiss_written_total_is_not_taken_for_another_amount_chromium(bridges, fakes, tmp_path):
    confirmed, pages = chromium_pay(bridges, fakes, tmp_path, SWISS, {**ARGS, "amount": "1250", "currency": "CHF"},
                                    "Bezahlen")
    assert "moved on" in confirmed and pages == ["done.html"], (
        f"approved 1250.00 CHF, the page's own Intl de-CH button not pressed: {confirmed}; pages {pages}")


# -- R3: cents in superscript (« 12⁵⁰ € ») read as a hundred times the amount ---------------------------------------

def test_r3_cents_in_superscript_are_not_taken_for_another_amount_chromium(bridges, fakes, tmp_path):
    confirmed, pages = chromium_pay(bridges, fakes, tmp_path, SUP, ARGS, "Payer")
    assert "moved on" in confirmed and pages == ["done.html"], (
        f"approved 12.50 EUR, « Payer 12<sup>50</sup> € » not pressed: {confirmed}; pages {pages}")


# -- green checks: the writings the brief named, read right (pressed) -------------------------------------------------

@pytest.mark.parametrize("button", ["payer 12,50 €", "pay €12.50", "pay eur 12.50", "payer 12,50 € ttc",
                                    "payer 12,5 €", "payer 12,50 €", "payer 12.50 eur"])
def test_g1_the_approved_amount_in_any_writing_is_pressed(bridges, fakes, button):
    chrome = FakeChrome()
    chrome.button = button
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
        assert chrome.clicks == ["https://shop.example/checkout"], (button, confirmed)
    finally:
        chrome.stop()


def test_g2_a_thousands_total_in_german_writing_is_pressed(bridges, fakes):
    chrome = FakeChrome()
    chrome.button = "jetzt bezahlen 1.250,00 €"
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, _ = pay(bridge, {**ARGS, "amount": "1250,00"})
        assert result["isError"] is False, result
        tool(bridge, "confirm_payment", {"button": "Bezahlen"})
        assert chrome.clicks == ["https://shop.example/checkout"]
    finally:
        chrome.stop()


def test_g3_shipping_added_after_the_approval_is_handed_over_not_paid(bridges, fakes):
    chrome = FakeChrome()
    chrome.button = "payer 17,40 €"   # 12,50 + 4,90 shipping shown only at the last step
    try:
        bridge = pay_bridge(bridges, fakes, chrome)
        turn_on(bridge)
        bridge.post("/v1/vault", headers=AUTH, json=CARD)
        result, _ = pay(bridge, ARGS)
        assert result["isError"] is False, result
        confirmed = tool(bridge, "confirm_payment", {"button": "Payer"})["content"][0]["text"]
        assert chrome.clicks == [] and "17.40 EUR" in confirmed, confirmed
    finally:
        chrome.stop()


def test_g4_a_public_suffix_is_refused_in_the_words_the_apps_match(bridges, fakes):
    bridge = pay_bridge(bridges, fakes, None)
    r = bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["myshopify.com"]})
    assert r.status_code == 400 and r.json().get("detail", "").startswith("give the shop's own address"), r.text


# -- R4: the agent names another submit button of the same form, without an amount: the check is skipped --------------

TWO = """<!doctype html><html lang="fr"><head><meta charset="utf-8"><title>Paiement</title></head><body>
<form onsubmit="event.preventDefault(); location.href='done.html'">
  <p><label>Numéro de carte <input id="numero" autocomplete="cc-number"></label></p>
  <p><label>Expiration <input id="exp" autocomplete="cc-exp" placeholder="MM/AA"></label></p>
  <p><label>Cryptogramme <input id="crypto" autocomplete="cc-csc"></label></p>
  <p><label>Titulaire <input id="titulaire" autocomplete="cc-name"></label></p>
  <p><button type="submit">Payer 1 250,00 €</button> <button type="submit">Valider</button></p>
</form></body></html>"""


def test_r4_another_submit_of_the_card_form_never_pays_another_amount_chromium(bridges, fakes, tmp_path):
    confirmed, pages = chromium_pay(bridges, fakes, tmp_path, TWO, ARGS, "Valider")
    assert pages == ["pay.html"] and "moved on" not in confirmed, (
        f"approved 12.50 EUR, the form whose pay button shows 1 250,00 € was submitted by « Valider »: "
        f"{confirmed}; pages {pages}")


def test_p1_probe_what_this_chromium_writes_for_de_ch():
    """Not a finding: what the real Chromium's Intl gives for 1250 CHF in de-CH (code points), for the report."""
    from test_card_payments_chrome import CHROME, Browser
    if CHROME is None:
        pytest.skip("no Chromium")
    browser = Browser("about:blank")
    try:
        text = browser.run("new Intl.NumberFormat('de-CH', {style: 'currency', currency: 'CHF'}).format(1250) + ' | ' + "
                           "new Intl.NumberFormat('de-CH').format(1250) + ' | ' + "
                           "new Intl.NumberFormat('fr-CH', {style: 'currency', currency: 'CHF'}).format(1250) + ' | ' + "
                           "new Intl.NumberFormat('he-IL', {style: 'currency', currency: 'ILS'}).format(1250)",
                           "about:blank")
        print("PROBE", ascii(text))
    finally:
        browser.stop()


SWISS_HTML = SWISS.replace("""<script>document.getElementById('pay').textContent = 'Bezahlen ' +
  new Intl.NumberFormat('de-CH', {style: 'currency', currency: 'CHF'}).format(1250);</script>""", "").replace(
    ">Bezahlen</button>", ">Bezahlen CHF 1’250.00</button>")


def test_r2_a_swiss_written_total_is_not_taken_for_another_amount_html_chromium(bridges, fakes, tmp_path):
    confirmed, pages = chromium_pay(bridges, fakes, tmp_path, SWISS_HTML, {**ARGS, "amount": "1250", "currency": "CHF"},
                                    "Bezahlen")
    assert "moved on" in confirmed and pages == ["done.html"], (
        f"approved 1250.00 CHF, « Bezahlen CHF 1’250.00 » not pressed: {confirmed}; pages {pages}")
