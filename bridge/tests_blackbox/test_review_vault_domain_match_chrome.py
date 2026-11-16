"""Review of vault-domain-match (16/11), point 4: the card checks' exemption for an identifier `vault_login` typed
(`vault-typed.json`), in a real Chromium behind bip-browser-mcp (the card guards' harness, test_card_guard_chrome.py).
Green on f5112b75e: the exemption keeps exactly that value, on exactly that origin, outside a card's field, for 30
minutes, and nothing else. A client code that passes Luhn, never a real card.

Needs a Chromium (`BIP_CHROME`) and the proxy (`BIP_BROWSER_MCP`): skipped without."""
import json
import os
import time

import pytest

from test_card_guard_chrome import CHROME, PROXY, Session, value

pytestmark = pytest.mark.skipif(not (CHROME and os.path.exists(CHROME) and PROXY and os.path.exists(PROXY)),
                                reason="no Chromium or no bip-browser-mcp")

CODE = "4970101234567893"        # a client code that looks like a card's number (the author's)
OTHER = "4242424242424242"       # Visa's published test number


def guard_hash(text):
    """GUARD_JS `h` / browser_mcp.rs `guard_hash`: FNV-1a over UTF-16 code units."""
    data, x = text.encode("utf-16-le"), 0x811C9DC5
    for i in range(0, len(data), 2):
        x ^= data[i] | (data[i + 1] << 8)
        x = (x * 0x01000193) & 0xFFFFFFFF
    return format(x, "x")


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def mark(browser, origin, text, age=0.0):
    (browser.dir / "vault-typed.json").write_text(json.dumps([{"o": origin, "h": guard_hash(text), "t": time.time() - age}]))


def put(browser, field, text):
    """As the bridge's DevTools typing would leave it: the field holds it, the agent didn't type it."""
    browser.page(f"document.getElementById({json.dumps(field)}).value = {json.dumps(text)}")


def a_call(browser):
    """Any call of the agent's that the guard surrounds."""
    browser.tool("type_text", selector="#address", text="1 rue de l'Essai")


def test_the_exemption_keeps_exactly_that_value_on_exactly_that_origin_outside_card_fields(browser):
    assert guard_hash(CODE) == "f445ad40"            # browser_mcp.rs `the_guards_hash_is_the_pages`
    browser.fresh()
    shop = f"http://127.0.0.1:{browser.shop.server_address[1]}"
    # control: not written down, the guard empties it
    (browser.dir / "vault-typed.json").unlink(missing_ok=True)
    put(browser, "notes", CODE)
    a_call(browser)
    assert value(browser, "notes") == ""
    # written down for this origin: left in the delivery notes
    mark(browser, shop, CODE)
    put(browser, "notes", CODE)
    a_call(browser)
    assert value(browser, "notes") == CODE
    # the same value in the card's own field: emptied
    put(browser, "numero", CODE)
    a_call(browser)
    assert value(browser, "numero") == ""
    # another card-like value beside it: emptied
    put(browser, "notes", OTHER)
    a_call(browser)
    assert value(browser, "notes") == ""
    # written down for another origin (the provider's): emptied here
    mark(browser, f"http://localhost:{browser.psp.server_address[1]}", CODE)
    put(browser, "notes", CODE)
    a_call(browser)
    assert value(browser, "notes") == ""
    # 30 minutes later: emptied; a row dated in the future: emptied
    for age in (1801.0, -600.0):
        mark(browser, shop, CODE, age=age)
        put(browser, "notes", CODE)
        a_call(browser)
        assert value(browser, "notes") == "", age
