"""Third review of fix-signup-hold (765a2f85c): `accept_terms` with the wider reading of a box's text (the short text
around a box without a <label>), in a real Chromium the bridge reaches over DevTools (test_terms_consent_chrome's
harness). The person's answer is waited for 3 s here: « did not answer in time » means a card was asked.

Needs a Chromium (`BIP_CHROME`)."""
import functools
import http.server
import threading
from pathlib import Path

from test_card_payments import pay_bridge
from test_card_payments_chrome import Browser, pytestmark  # noqa: F401  (skipped without a Chromium)
from test_terms_consent_chrome import accept

PAGES = Path(__file__).parent / "fixtures" / "review3"
ASKED = "did not answer in time"


def pages():
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(PAGES))
    handler.log_message = lambda *a: None
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def test_the_terms_box_beside_the_newsletters_in_one_paragraph_gets_its_card(bridges, fakes):
    # two boxes without <label> in one short paragraph: each has its own text, the terms box is one
    shop = pages()
    base = f"http://127.0.0.1:{shop.server_port}"
    browser = Browser(f"{base}/two_boxes.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser,
                            change=lambda config: config.setdefault("browser", {}).update({"consent_seconds": 3}))
        result = accept(bridge, {"site": base, "box": "J'accepte les conditions générales de vente"})
        assert ASKED in result["content"][0]["text"], result
    finally:
        browser.stop()
        shop.shutdown()
