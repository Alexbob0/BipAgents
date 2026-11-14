"""Independent review of card-guards (14/11): the provider's own payment page reached by the agent's doing, through
the bridge's pay_with_card on a real Chrome (test_card_payments_chrome.py's harness).

The reviewer's case: a script of the agent's adds a link to ANOTHER merchant's page at the same provider, and the agent
clicks it: the tab's history says « link » from the card's site, and the card went to someone else's page. A script
reaches the page only through bip-browser-mcp, which now refuses scripts that add links or forms or send the tab
somewhere (card_guards_review.rs rv4), and writes down every other script it lets run on a page (`scripts.jsonl` beside
the browser's `busy` file: the page's origin and the time). The bridge refuses the provider's page after a script on
the card's site within the hour before (card_pay.rs `scripted_before`): the agent hands the payment over. Here the
script is played through DevTools (as the author's tests play the agent's clicks) and its record written as the proxy
writes it. Only Visa's published test number."""
import json
import time

from bbkit.common import AUTH
from test_card_payments_chrome import ARGS, CARD, Browser, pay, pay_bridge, site, test_providers, turn_on
import test_card_payments_chrome as c

pytestmark = c.pytestmark


def record_script(bridge, origin):
    """What bip-browser-mcp writes when it lets a script of the agent's run on a page of `origin`."""
    path = bridge.data / "browsers" / "vie" / "scripts.jsonl"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as out:
        out.write(json.dumps({"t": time.time(), "origin": origin}) + "\n")


def test_rv7_another_merchants_page_at_the_provider_reached_by_the_agents_link_is_refused(bridges, fakes):
    shop, provider = site(), site()
    hosted = f"http://psp.localhost:{provider.server_port}/hosted.html"
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/cart.html?psp={hosted}")
    try:
        bridge = pay_bridge(bridges, fakes, browser, change=test_providers("psp.localhost"))
        turn_on(bridge)
        assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]}).status_code == 201
        # the agent's script adds a link to ANOTHER merchant's page at the same provider, then clicks it (« link »)
        other = hosted + "?merchant=someone-else"
        browser.run("document.body.insertAdjacentHTML('beforeend', "
                    + json.dumps(f'<a id="z" href="{other}">Payer</a>') + "); document.getElementById('z').click()",
                    "cart.html")
        record_script(bridge, f"http://127.0.0.1:{shop.server_port}")
        deadline = time.time() + 10
        while time.time() < deadline and not any("someone-else" in u for u in browser.pages()):
            time.sleep(0.2)
        time.sleep(1)
        result, asked = pay(bridge, ARGS)
        filled = browser.values().get("hosted.html")
        assert asked is None and result["isError"] is True, (
            f"the person was asked {asked!r} (the shop shown), and the card went to {filled!r} on {browser.pages()}")
        said = result["content"][0]["text"]
        assert "script" in said and "hand_over_payment" in said and "« Panier prêt »" in said, said
        assert filled == {"number": "", "exp": "", "cvc": "", "holder": ""}
    finally:
        browser.stop()
        shop.shutdown()
        provider.shutdown()


def test_a_script_on_another_site_leaves_the_shops_provider_page_alone(bridges, fakes):
    # a script the agent ran on another site (a search, a comparison) is no reason to hand the payment over
    shop, provider = site(), site()
    hosted = f"http://psp.localhost:{provider.server_port}/hosted.html"
    browser = Browser(f"http://127.0.0.1:{shop.server_port}/cart.html?psp={hosted}")
    try:
        bridge = pay_bridge(bridges, fakes, browser, change=test_providers("psp.localhost"))
        turn_on(bridge)
        assert bridge.post("/v1/vault", headers=AUTH, json={**CARD, "sites": ["127.0.0.1"]}).status_code == 201
        record_script(bridge, "https://www.comparateur.example")
        browser.run("document.querySelector('#go button').click()", "cart.html")
        deadline = time.time() + 10
        while time.time() < deadline and not any("hosted.html" in u for u in browser.pages()):
            time.sleep(0.2)
        time.sleep(1)
        result, asked = pay(bridge, ARGS)
        assert result["isError"] is False and asked, result
        assert tool_cancel(bridge).startswith("The payment is given up")
    finally:
        browser.stop()
        shop.shutdown()
        provider.shutdown()


def tool_cancel(bridge):
    return c.tool(bridge, "cancel_payment")["content"][0]["text"]
