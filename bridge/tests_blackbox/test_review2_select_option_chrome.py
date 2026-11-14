"""Second review of select-option-direct (head 35fb3be1f, « à corriger », 14/11): the consent-list check, the one-list
rule and the password guard of the proxy's select_option, in a real Chromium behind the branch's stand-in. Each test
says what should happen; the « finding » ones failed on 35fb3be1f (the reviewer's scratch tests, brought into the
branch). Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import time

import pytest

import test_signup_guard_chrome as guard
from test_signup_guard_chrome import Session, pytestmark  # noqa: F401

PAGE = """<!doctype html><html><head><meta charset="utf-8"><title>lists 2</title></head><body>
<form id="f2" action="/sent.html" method="get">
  <label for="optin">J'accepte la politique de confidentialité</label>
  <select id="optin" name="optin"><option value="-1">Choisir</option><option value="0">J'accepte</option>
    <option value="1">Je refuse</option></select>
  <label for="ds">Einwilligung zum Datenschutz</label>
  <select id="ds" name="ds"><option value="">Bitte wählen</option><option value="a">Zustimmen</option>
    <option value="b">Ablehnen</option></select>
  <label for="es">Acepto los términos y condiciones</label>
  <select id="es" name="es"><option value="">Elegir</option><option value="si">Acepto</option>
    <option value="re">Rechazar</option></select>
  <label for="es2">Acepto la política de privacidad</label>
  <select id="es2" name="es2"><option value="">Elegir</option><option value="si">Acepto</option>
    <option value="re">Rechazar</option></select>
  <label for="pays">Pays</label>
  <select id="pays" name="pays"><option value="">--</option><option value="FR">France</option>
    <option value="BE">Belgique</option></select>
  <label for="acc">Accepté par :</label>
  <select id="acc" name="acc"><option value="">--</option><option value="visa">Visa</option>
    <option value="mc">Mastercard</option></select>
  <div class="row"><select id="livraison" name="livraison"><option value="">Mode de livraison</option>
    <option value="colissimo">Colissimo</option><option value="relais">Point relais</option></select>
    <small>Délais selon nos conditions générales de vente</small></div>
  <label for="vis">Visibilité de votre profil</label>
  <select id="vis" name="privacy"><option value="">--</option><option value="public">Public</option>
    <option value="friends">Amis</option></select>
  <label for="pt">Payment terms</label>
  <select id="pt" name="pt"><option value="">--</option><option value="30">Net 30</option>
    <option value="60">Net 60</option></select>
  <div class="mobile" style="display:none"><select id="titre" name="titre"><option value="">--</option>
    <option value="M">Monsieur</option><option value="Mme">Madame</option></select></div>
  <div class="desktop"><label for="titre">Civilité</label><select id="titre" name="titre"><option value="">--</option>
    <option value="M">Monsieur</option><option value="Mme">Madame</option></select></div>
  <div id="country-block"><input type="hidden" name="country_iso" value="">
    <select id="c2" name="c2"><option value="">--</option><option value="FR">France</option></select></div>
  <label for="pw2">Mot de passe</label><input id="pw2" name="pw2" type="password">
  <div id="pw-block"><input id="pw3" name="pw3" type="password"></div>
  <button type="submit">Envoyer</button>
</form></body></html>"""

VISIBLE_TITRE = "document.querySelector('.desktop select').value"


@pytest.fixture(scope="module")
def browser():
    session = Session()
    (session.dir / "site" / "lists2.html").write_text(PAGE, encoding="utf-8")
    yield session
    session.close()


def open_page(browser):
    browser.tool("navigate", url=f"{browser.base}/lists2.html")


def value(browser, element_id):
    return browser.page(f"document.getElementById({element_id!r}).value")


# --- findings (fail on 35fb3be1f) -------------------------------------------------------------------------------------

def test_finding_an_accepting_choice_valued_0_is_never_given(browser):
    # « J'accepte » valued « 0 » (a list built from an array, values its places): `declines` reads the value first, so
    # « 0 » passes for « Non » and the agent gives the consent itself
    open_page(browser)
    said = browser.tool("select_option", selector="#optin", text="J'accepte")
    assert value(browser, "optin") == "-1" and said.startswith(guard.TERMS_REFUSED), said


@pytest.mark.parametrize("selector,text,expected", [("#ds", "Ablehnen", "b"), ("#es", "Rechazar", "re")])
def test_finding_a_consent_is_declined_in_german_and_spanish_too(browser, selector, text, expected):
    # declining gives nothing: « Non », « No », « Nein », « Je refuse » go on; « Ablehnen » and « Rechazar » (the
    # German and Spanish « decline ») are refused as consents
    open_page(browser)
    said = browser.tool("select_option", selector=selector, text=text)
    assert value(browser, selector[1:]) == expected, said


@pytest.mark.parametrize("selector,choice,expected", [
    ("#livraison", "Colissimo", "colissimo"),   # no label, a hint after it naming the shop's conditions of sale
    ("#vis", "Public", "public"),               # profile visibility, its name « privacy »
    ("#pt", "Net 30", "30"),                    # B2B payment terms
])
def test_finding_an_ordinary_list_is_not_taken_for_a_consent(browser, selector, choice, expected):
    open_page(browser)
    said = browser.tool("select_option", selector=selector, text=choice)
    assert value(browser, selector[1:]) == expected, said


def test_finding_a_spanish_privacy_consent_is_a_consent(browser):
    # « política de privacidad » (and « protección de datos ») are not among the consent words (the box guard's
    # LABEL_JS, shared: the box of the same label is ticked too); « privacy », « confidentialité », « Datenschutz » are
    open_page(browser)
    said = browser.tool("select_option", selector="#es2", text="Acepto")
    assert value(browser, "es2") == "" and said.startswith(guard.TERMS_REFUSED), said


def test_finding_a_list_with_a_hidden_copy_is_chosen_in_the_one_shown(browser):
    # the same form twice, a mobile copy hidden (same id): « several lists, nothing chosen », and the agent has no other
    # selector to give
    open_page(browser)
    said = browser.tool("select_option", selector="#titre", value="M")
    assert browser.page(VISIBLE_TITRE) == "M", said


def test_finding_a_guessed_list_name_costs_no_more_than_before(browser):
    # a list looked for under a guessed name (GLM, every bench): a4c364653 answered after ~3.3 s; now the password
    # guard waits for the selector (LATE_FIELD_TRIES) before the list's own wait
    open_page(browser)
    started = time.time()
    said = browser.tool("select_option", selector='select[name="civilite"]', text="Madame")
    took = time.time() - started
    assert said.startswith("Not done: nothing on the page") and took < 4.5, (round(took, 1), said)


# --- checks that hold --------------------------------------------------------------------------------------------------

@pytest.mark.parametrize("selector,choice,expected", [
    ("#pays", {"text": "France"}, "FR"), ("#acc", {"text": "Visa"}, "visa"), ("#c2", {"value": "FR"}, "FR"),
    ("#country-block", {"value": "FR"}, "FR"),
])
def test_ordinary_lists_and_a_one_list_block_are_chosen(browser, selector, choice, expected):
    open_page(browser)
    said = browser.tool("select_option", selector=selector, **choice)
    target = "c2" if selector == "#country-block" else selector[1:]
    assert value(browser, target) == expected and said.startswith("Chosen in that list"), said


@pytest.mark.parametrize("selector,choice", [
    ("//input[@id='pw2']", {"text": "Secret123!"}), ("//input[@type='password']", {"value": "Secret123!"}),
    ("label[for=pw2]", {"text": "Secret123!"}), ("#pw-block", {"text": "Secret123!"}),
    ("#pw2, #pays", {"text": "Secret123!"}), ("input[type=password]", {"index": 0}),
])
def test_no_password_field_gets_a_value_through_select_option(browser, selector, choice):
    open_page(browser)
    said = browser.tool("select_option", selector=selector, **choice)
    # (« #pw2, #pays »: the list « Pays » is the one named, its answer echoes the agent's own text: nothing typed)
    assert value(browser, "pw2") == "" and value(browser, "pw3") == "", (selector, said)


def test_the_focused_password_field_gets_nothing_from_a_list_without_selector(browser):
    open_page(browser)
    browser.page("document.getElementById('pw2').focus(); true")
    for arguments in ({"selector": "", "text": "Secret123!"}, {"text": "Secret123!"}):
        said = browser.tool("select_option", **arguments)
        assert value(browser, "pw2") == "", (arguments, said)
