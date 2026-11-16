"""Review of vault-no-readback (16/11): the stand-in for stealth-browser-mcp (fixtures/signup_guard/stealth_cdp.py) with
click_element's `text_match` as the real server does it. dom_handler.click_element calls nodriver 0.47's
`tab.find(text_match, best_match=True)` (the image's /opt/stealth/venv/.../nodriver/core/tab.py): `DOM.getDocument`
(depth -1, pierce), then `DOM.performSearch(text, includeUserAgentShadowDOM=True)`, which matches the text in every
attribute's value and text node of the page (case ignored), and also takes it as a CSS selector and as an XPath; the
first result's element is clicked (`Element.click`: a JS click), else it tries again every 0.5 s until its timeout
(10 s; 3 s here, to keep the test short) and dom_handler raises « Element not found ». The other tools are the
stand-in's own.

    python3 stealth_find.py <page's DevTools websocket URL>
"""
import json
import sys
import time
from pathlib import Path

ORIGINAL = Path(__file__).resolve().parent.parent.parent / "signup_guard" / "stealth_cdp.py"
SOURCE = ORIGINAL.read_text()
HEAD, LOOP = SOURCE.split("\nfor line in sys.stdin:", 1)
G = {"__name__": "__main__", "__file__": str(ORIGINAL)}
exec(compile(HEAD, str(ORIGINAL), "exec"), G)
CDP, STAND_IN_CALL = G["cdp"], G["call"]
FIND_TIMEOUT = 3.0


def performed(text):
    """nodriver's find_element_by_text: the node ids DOM.performSearch gives for `text`."""
    CDP("DOM.getDocument", {"depth": -1, "pierce": True})
    search = CDP("DOM.performSearch", {"query": text.strip(), "includeUserAgentShadowDOM": True})
    ids = []
    if search["resultCount"]:
        ids = CDP("DOM.getSearchResults", {"searchId": search["searchId"], "fromIndex": 0,
                                           "toIndex": search["resultCount"]})["nodeIds"]
    CDP("DOM.discardSearchResults", {"searchId": search["searchId"]})
    return ids


def call(name, args):
    if name == "click_element" and args.get("text_match"):
        deadline = time.time() + FIND_TIMEOUT
        ids = performed(args["text_match"])
        while not ids and time.time() < deadline:
            time.sleep(0.5)
            ids = performed(args["text_match"])
        if not ids:
            raise Exception(f"Failed to click element: Element not found: {args.get('selector')}")
        target = CDP("DOM.resolveNode", {"nodeId": ids[0]})["object"]["objectId"]   # a text node: its element
        CDP("Runtime.callFunctionOn", {"functionDeclaration": "function () { (this.nodeType === 3 ? this.parentElement "
                                                              ": this).click(); }", "objectId": target,
                                       "userGesture": True})
        return True
    return STAND_IN_CALL(name, args)


G["call"] = call
exec(compile("for line in sys.stdin:" + LOOP, str(ORIGINAL), "exec"), G)
