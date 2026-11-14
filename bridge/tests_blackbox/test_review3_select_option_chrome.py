"""Third review of select-option-direct (head b3780795b, « à corriger », 14/11): consent lists told by their own label
and their yes / no choices, in the four languages, in a real Chromium behind the branch's stand-in. Each test says what
should happen; the « finding » ones failed on b3780795b (the reviewer's scratch tests, brought into the branch). Needs a
Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import pytest

import test_signup_guard_chrome as guard
from test_signup_guard_chrome import Session, pytestmark  # noqa: F401


def consent(ident, label, choices):
    options = "".join(f'<option value="{v}">{t}</option>' for v, t in choices)
    return f'<label for="{ident}">{label}</label><select id="{ident}" name="{ident}">{options}</select>\n'


PH = ("", "--")
LISTS = [
    # a required consent: one « J'accepte » and the empty placeholder, no explicit « no »
    ("single", "J'accepte les conditions générales de vente", [PH, ("1", "J'accepte")]),
    ("acc", "J'accepte les CGU", [PH, ("a", "Accepter"), ("r", "Refuser")]),
    ("on", "Conditions générales", [PH, ("1", "Oui"), ("0", "Non")]),
    ("de", "AGB", [PH, ("1", "Ja"), ("0", "Nein")]),
    ("de2", "Ich akzeptiere die AGB", [PH, ("a", "Akzeptieren"), ("b", "Ablehnen")]),
    ("es", "Acepto los términos y condiciones", [PH, ("s", "Sí"), ("n", "No")]),
    ("en", "Terms of service", [PH, ("a", "Accept"), ("r", "Reject")]),
    ("en2", "Marketing consent", [PH, ("i", "Opt in"), ("o", "Opt out")]),
    ("fr2", "Politique de confidentialité", [PH, ("d", "D'accord"), ("p", "Pas d'accord")]),
    ("es2", "Política de privacidad", [PH, ("d", "De acuerdo"), ("e", "En desacuerdo")]),
    ("en3", "I agree to the privacy policy", [PH, ("y", "Yes"), ("n", "No")]),
    ("es3", "Protección de datos", [PH, ("s", "Sí, acepto"), ("n", "No acepto")]),
]
PAGE = ("""<!doctype html><html><head><meta charset="utf-8"><title>consents</title></head><body><form>\n"""
        + "".join(consent(*entry) for entry in LISTS)
        # a consent list without a <label>: the shop's text in a cell before it, its name « cgv »
        + """<table><tr><td>J'accepte les conditions générales de vente</td><td><select name="cgv" id="nolabel">
<option value="">--</option><option value="1">Oui</option><option value="0">Non</option></select></td></tr></table>
</form></body></html>""")


@pytest.fixture(scope="module")
def browser():
    session = Session()
    (session.dir / "site" / "consents.html").write_text(PAGE, encoding="utf-8")
    yield session
    session.close()


def choose(browser, ident, text):
    browser.tool("navigate", url=f"{browser.base}/consents.html")
    said = browser.tool("select_option", selector=f"#{ident}", text=text)
    return said, browser.page(f"document.getElementById({ident!r}).value")


# --- findings (fail on b3780795b) -------------------------------------------------------------------------------------

@pytest.mark.parametrize("ident,text", [
    ("single", "J'accepte"),      # no explicit « no »: the coordinator's question
    ("de2", "Akzeptieren"),       # German « accept » (akzeptieren) not an accepting word
    ("es", "Sí"),                 # « s[ií]\\b »: no word boundary after « í » in a JavaScript regex without /u
    ("en", "Accept"),             # « Reject » not a declining word
    ("en2", "Opt in"),            # « Opt in » not an accepting word
    ("fr2", "D'accord"),          # « Pas d'accord » not a declining word
    ("es2", "De acuerdo"),        # neither « De acuerdo » nor « En desacuerdo »
    ("nolabel", "Oui"),           # no <label>: its own label empty (the cell's text, its name « cgv » not read)
])
def test_finding_an_accepting_choice_in_a_consent_list_is_never_the_agents(browser, ident, text):
    said, value = choose(browser, ident, text)
    assert value == "" and said.startswith(guard.TERMS_REFUSED), (ident, value, said)


# --- checks that hold --------------------------------------------------------------------------------------------------

@pytest.mark.parametrize("ident,text", [("acc", "Accepter"), ("on", "Oui"), ("de", "Ja"), ("en3", "Yes"),
                                        ("es3", "Sí, acepto")])
def test_an_accepting_choice_is_refused(browser, ident, text):
    said, value = choose(browser, ident, text)
    assert value == "" and said.startswith(guard.TERMS_REFUSED), (ident, value, said)


@pytest.mark.parametrize("ident,text,expected", [
    ("acc", "Refuser", "r"), ("on", "Non", "0"), ("de", "Nein", "0"), ("de2", "Ablehnen", "b"), ("es", "No", "n"),
    ("en3", "No", "n"), ("es3", "No acepto", "n"), ("en2", "Opt out", "o"), ("single", "--", ""),
])
def test_a_declining_choice_is_made(browser, ident, text, expected):
    said, value = choose(browser, ident, text)
    assert value == expected and said.startswith("Chosen in that list"), (ident, value, said)
