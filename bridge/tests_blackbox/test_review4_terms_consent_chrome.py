"""Fourth review of fix-signup-hold (81c8cbdbf): `accept_terms` for a terms box followed by its text in a <label> tied
to nothing, in a real Chromium the bridge reaches over DevTools (test_terms_consent_chrome's harness). The person's
answer is waited for 3 s here: « did not answer in time » means a card was asked.

Needs a Chromium (`BIP_CHROME`)."""
import functools
import http.server
import threading
from pathlib import Path

from test_card_payments import pay_bridge
from test_card_payments_chrome import Browser, pytestmark  # noqa: F401  (skipped without a Chromium)
from test_terms_consent_chrome import accept

PAGES = Path(__file__).parent / "fixtures" / "review4"
ASKED = "did not answer in time"


def test_a_terms_box_followed_by_its_untied_label_gets_its_card(bridges, fakes):
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(PAGES))
    handler.log_message = lambda *a: None
    shop = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=shop.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{shop.server_port}"
    browser = Browser(f"{base}/unassociated_labels.html")
    try:
        bridge = pay_bridge(bridges, fakes, browser,
                            change=lambda config: config.setdefault("browser", {}).update({"consent_seconds": 3}))
        result = accept(bridge, {"site": base, "box": "J'accepte les conditions générales de vente"})
        assert ASKED in result["content"][0]["text"], result
    finally:
        browser.stop()
        shop.shutdown()
