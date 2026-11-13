"""Second review of fix-signup-hold (9415fdb13): `accept_terms` against terms boxes worded or built as many real sign-up
forms are, in a real Chromium the bridge reaches over DevTools (test_terms_consent_chrome's harness). The agent's
browser refuses to tick these boxes itself (test_review2_guard_chrome), so the card is the only way. The person's answer
is waited for 3 s here: « did not answer in time » means a card was asked.

Needs a Chromium (`BIP_CHROME`)."""
import functools
import http.server
import threading
from pathlib import Path

from test_card_payments import pay_bridge
from test_card_payments_chrome import Browser, pytestmark  # noqa: F401  (skipped without a Chromium)
from test_terms_consent_chrome import accept

PAGES = Path(__file__).parent / "fixtures" / "review2"
ASKED = "did not answer in time"


def pages():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(PAGES))
    handler.log_message = lambda *a: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def quick(config):
    config.setdefault("browser", {}).update({"consent_seconds": 3})


def test_a_terms_box_naming_cookies_gets_its_card(bridges, fakes):
    # « … et la politique de confidentialité et de cookies »: the site's terms; never « tick it yourself »
    shop = pages()
    base = f"http://127.0.0.1:{shop.server_port}"
    browser = Browser(f"{base}/terms_cookies.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser, change=quick)
        result = accept(bridge, {"site": base, "box": "J'ai lu et j'accepte les conditions générales d'utilisation"})
        assert ASKED in result["content"][0]["text"], result
    finally:
        browser.stop()
        shop.shutdown()


def test_a_terms_box_whose_text_is_beside_it_gets_its_card(bridges, fakes):
    # the text in a span next to the box, or tied to it by aria-labelledby: no <label>
    shop = pages()
    base = f"http://127.0.0.1:{shop.server_port}"
    browser = Browser(f"{base}/terms_unlabelled.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser, change=quick)
        said = {}
        for box in ("J'accepte les conditions générales de vente", "J'accepte la politique de confidentialité"):
            result = accept(bridge, {"site": base, "box": box})
            if ASKED not in result["content"][0]["text"]:
                said[box] = result["content"][0]["text"]
        assert not said, said
    finally:
        browser.stop()
        shop.shutdown()


def test_the_visible_terms_box_gets_its_card_beside_a_hidden_copy_of_the_form(bridges, fakes):
    # a responsive page: the same form twice, one copy hidden on this screen; the box the person sees is one
    shop = pages()
    base = f"http://127.0.0.1:{shop.server_port}"
    browser = Browser(f"{base}/terms_twice.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser, change=quick)
        result = accept(bridge, {"site": base, "box": "J'accepte les conditions générales de vente"})
        assert ASKED in result["content"][0]["text"], result
    finally:
        browser.stop()
        shop.shutdown()
