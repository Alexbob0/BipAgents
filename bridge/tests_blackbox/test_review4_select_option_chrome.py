"""Fourth review of select-option-direct (head 594d15895, « prêt », 14/11): consent lists with ordinary choices (false
positives), the strong words in context, other consent wordings of the four languages. Each test says what should
happen; the « finding » ones failed on 594d15895 and int-release-next cb42ade24 (the reviewer's scratch tests, brought
in by consent-words). Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import pytest

import test_signup_guard_chrome as guard
from test_signup_guard_chrome import Session, pytestmark  # noqa: F401


def entry(ident, label, choices):
    options = "".join(f'<option value="{v}">{t}</option>' for v, t in choices)
    return f'<label for="{ident}">{label}</label><select id="{ident}" name="{ident}">{options}</select>\n'


PH = ("", "--")
LISTS = [
    # ordinary choices under consent words
    ("nl", "Consentement newsletter", [PH, ("h", "Hebdomadaire"), ("m", "Mensuelle")]),
    ("par", "Consentement parental", [PH, ("p", "Père"), ("m", "Mère"), ("t", "Tuteur")]),
    ("liv", "Conditions de livraison", [PH, ("s", "Standard"), ("e", "Express")]),
    ("lang", "Langue de la politique de confidentialité", [PH, ("fr", "Français"), ("en", "English")]),
    ("acct", "Datenschutz: Kontotyp", [PH, ("p", "Privat"), ("g", "Geschäftlich")]),
    # cookies: the box guard leaves a cookie choice alone unless it names the terms
    ("ck", "Préférences cookies", [PH, ("all", "Accepter tout"), ("ess", "Accepter les cookies essentiels seulement"),
                                   ("none", "Tout refuser")]),
    ("ck2", "Politique de confidentialité", [PH, ("all", "Accepter tout"),
                                             ("ess", "Accepter les cookies essentiels seulement"),
                                             ("none", "Tout refuser")]),
    # other consent wordings, each with its « no »
    ("fr", "Je consens au traitement de mes données (RGPD)", [PH, ("y", "Je consens"), ("n", "Je ne consens pas")]),
    ("es", "Protección de datos", [PH, ("y", "Autorizo"), ("n", "No autorizo")]),
    ("es2", "Consentimiento", [PH, ("y", "Consiento")]),
    ("de", "Einwilligung zur Datenverarbeitung", [PH, ("y", "Ich willige ein"), ("n", "Ich willige nicht ein")]),
    ("en", "Privacy consent", [PH, ("y", "Allow"), ("n", "Deny")]),
    ("en2", "Terms of service", [PH, ("y", "I authorize"), ("n", "I do not authorize")]),
    # declining in other words
    ("de2", "AGB", [PH, ("y", "Zustimmen"), ("n", "Nicht zustimmen")]),
    ("fr2", "Conditions générales", [PH, ("y", "Accepter"), ("n", "Ne pas accepter")]),
    ("fr3", "Politique de confidentialité", [PH, ("y", "J'autorise"), ("n", "Je n'autorise pas")]),
]
PAGE = ("""<!doctype html><html><head><meta charset="utf-8"><title>consents 4</title></head><body><form>\n"""
        + "".join(entry(*e) for e in LISTS) + "</form></body></html>")


@pytest.fixture(scope="module")
def browser():
    session = Session()
    (session.dir / "site" / "consents4.html").write_text(PAGE, encoding="utf-8")
    yield session
    session.close()


def choose(browser, ident, text):
    browser.tool("navigate", url=f"{browser.base}/consents4.html")
    said = browser.tool("select_option", selector=f"#{ident}", text=text)
    return said, browser.page(f"document.getElementById({ident!r}).value")


# --- checks: ordinary choices under consent words are chosen ------------------------------------------------------------

@pytest.mark.parametrize("ident,text,expected", [
    ("nl", "Hebdomadaire", "h"), ("par", "Mère", "m"), ("liv", "Express", "e"), ("lang", "English", "en"),
    ("acct", "Privat", "p"), ("ck", "Accepter les cookies essentiels seulement", "ess"), ("ck", "Tout refuser", "none"),
    ("ck2", "Tout refuser", "none"),
])
def test_an_ordinary_choice_is_made(browser, ident, text, expected):
    said, value = choose(browser, ident, text)
    assert value == expected and said.startswith("Chosen in that list"), (ident, value, said)


def test_essential_cookies_only_under_the_privacy_policy(browser):
    # not a finding: refused as an acceptance (the agent can « Tout refuser » or ask); printed for the report
    said, value = choose(browser, "ck2", "Accepter les cookies essentiels seulement")
    print(f"\nck2 essentials only: value={value!r} said={said[:90]!r}")


# --- findings (fail on 594d15895) -------------------------------------------------------------------------------------

@pytest.mark.parametrize("ident,text", [
    ("fr", "Je consens"), ("es", "Autorizo"), ("es2", "Consiento"), ("de", "Ich willige ein"), ("en", "Allow"),
    ("en2", "I authorize"),
])
def test_finding_an_accepting_choice_in_other_words_is_never_the_agents(browser, ident, text):
    said, value = choose(browser, ident, text)
    assert value == "" and said.startswith(guard.TERMS_REFUSED), (ident, value, said)


@pytest.mark.parametrize("ident,text,expected", [
    ("de2", "Nicht zustimmen", "n"), ("fr2", "Ne pas accepter", "n"), ("fr3", "Je n'autorise pas", "n"),
    ("fr", "Je ne consens pas", "n"), ("de", "Ich willige nicht ein", "n"), ("en2", "I do not authorize", "n"),
])
def test_finding_a_declining_choice_in_other_words_is_made(browser, ident, text, expected):
    said, value = choose(browser, ident, text)
    assert value == expected and said.startswith("Chosen in that list"), (ident, value, said)
