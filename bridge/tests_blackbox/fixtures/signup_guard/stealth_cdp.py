"""A stand-in for stealth-browser-mcp (a7b9eea, src/dom_handler.py) over a real Chromium's DevTools, for the review of
fix-signup-hold: the same tools, output schemas and semantics where the guard of bip-browser-mcp depends on them.

- click_element: with `text_match`, the element whose text best matches it (nodriver `tab.find(text, best_match=True)`),
  the selector unused; else `tab.select(selector)` (waits for it); then a JS click (nodriver `Element.click`);
- type_text: waits for its element up to 10 s (nodriver `tab.select`), focuses it, empties it, types each character;
- press_key: focuses `selector`, one key; with Shift its text upper-cased (dom_handler.press_key);
- execute_script: `script`, or `(function() { script })(args…)` with `args` (dom_handler.execute_script);
- paste_text: the value set at once (card-guards, 14/11);
- get_active_tab: the tab's target id, which bip-browser-mcp's checks look at through DevTools (review of 14/11);
- select_option (added for select-option-direct, 13/11): as dom_handler.select_option, a `value` or an `index` set by a
  script declaring `const select` at the page's top level, its error never read (nodriver `Tab.evaluate` returns it),
  so True whatever happened: a page's second list is never set; a `text` typed into the list (`send_keys`: Chrome's
  type-ahead picks the first choice whose text starts with it, review of select-option-direct 14/11).

    python3 stealth_cdp.py <page's DevTools websocket URL>
"""
import json
import sys
import time

from websockets.sync.client import connect

WS = connect(sys.argv[1], max_size=None)
N = 0
WRAPPED_BOOL = {"properties": {"result": {"type": "boolean"}}, "required": ["result"], "type": "object",
                "x-fastmcp-wrap-result": True}
OBJECT = {"additionalProperties": True, "type": "object"}
TOOLS = [
    {"name": "navigate", "inputSchema": {"type": "object"}, "outputSchema": OBJECT},
    {"name": "click_element", "inputSchema": {"type": "object"}, "outputSchema": WRAPPED_BOOL},
    {"name": "type_text", "inputSchema": {"type": "object"}, "outputSchema": WRAPPED_BOOL},
    {"name": "press_key", "inputSchema": {"type": "object"}, "outputSchema": OBJECT},
    {"name": "execute_script", "inputSchema": {"type": "object"}, "outputSchema": OBJECT},
    {"name": "paste_text", "inputSchema": {"type": "object"}, "outputSchema": WRAPPED_BOOL},
    {"name": "get_active_tab", "inputSchema": {"type": "object"}, "outputSchema": OBJECT},
    {"name": "select_option", "inputSchema": {"type": "object"}, "outputSchema": WRAPPED_BOOL},
]


def cdp(method, params=None):
    global N
    N += 1
    WS.send(json.dumps({"id": N, "method": method, "params": params or {}}))
    while True:
        answer = json.loads(WS.recv(timeout=30))
        if answer.get("id") == N:
            if "error" in answer:
                raise Exception(answer["error"].get("message"))
            return answer.get("result", {})


def evaluate(expression):
    got = cdp("Runtime.evaluate", {"expression": expression, "returnByValue": True, "awaitPromise": True,
                                   "userGesture": True})
    if got.get("exceptionDetails"):
        raise Exception(got["exceptionDetails"].get("text", "script error"))
    return got.get("result", {}).get("value")


def select(selector, timeout=10.0):
    if selector.lstrip("(").startswith("/"):   # an XPath, as dom_handler takes one
        element = (f"document.evaluate({json.dumps(selector)}, document, null, "
                   "XPathResult.FIRST_ORDERED_NODE_TYPE, null).singleNodeValue")
    else:
        element = f"document.querySelector({json.dumps(selector)})"
    deadline = time.time() + timeout
    while not evaluate(f"!!{element}"):
        if time.time() > deadline:
            raise Exception(f"Element not found: {selector}")
        time.sleep(0.1)
    return element


FIND = """((t) => { let best = null, gap = Infinity;
  for (const e of document.body.querySelectorAll('*')) {
    const own = [...e.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join(' ').trim();
    if (!own.toLowerCase().includes(t.toLowerCase())) continue;
    const d = Math.abs(own.length - t.length); if (d < gap) { gap = d; best = e; } }
  if (!best) return false; best.scrollIntoView(); best.click(); return true; })"""


def call(name, args):
    if name == "navigate":
        cdp("Page.navigate", {"url": args["url"]})
        deadline = time.time() + 10
        time.sleep(0.2)
        while evaluate("document.readyState") != "complete" and time.time() < deadline:
            time.sleep(0.1)
        return {"success": True, "url": args["url"]}
    if name == "click_element":
        if args.get("text_match"):
            if not evaluate(f"{FIND}({json.dumps(args['text_match'])})"):
                raise Exception(f"Element not found: {args['text_match']}")
            return True
        element = select(args["selector"], float(args.get("timeout", 10000)) / 1000)
        evaluate(f"(() => {{ const e = {element}; e.scrollIntoView(); e.click(); return true; }})()")
        return True
    if name == "type_text":
        element = select(args["selector"])
        evaluate(f"(() => {{ const e = {element}; e.focus(); e.value = ''; return true; }})()")
        for char in args["text"]:
            cdp("Input.insertText", {"text": char})
        return True
    if name == "get_active_tab":   # server.get_active_tab: the tab's target id (the page this stand-in drives)
        return {"tab_id": sys.argv[1].rstrip("/").split("/")[-1], "url": evaluate("location.href"), "type": "page"}
    if name == "paste_text":   # dom_handler.paste_text: the value set at once, an input event
        element = select(args["selector"])
        evaluate(f"(() => {{ const e = {element}; e.focus(); e.value = {json.dumps(args['text'])}; "
                 "e.dispatchEvent(new Event('input', {bubbles: true})); return true; })()")
        return True
    if name == "press_key":
        if args.get("selector"):
            evaluate(f"{select(args['selector'])}.focus()")
        key = args["key"]
        text = key.upper() if "Shift" in (args.get("modifiers") or []) else key
        cdp("Input.dispatchKeyEvent", {"type": "keyDown", "key": text, "text": text})
        cdp("Input.dispatchKeyEvent", {"type": "keyUp", "key": text})
        return {"success": True, "key": key, "count": 1, "modifiers": args.get("modifiers") or []}
    if name == "execute_script":
        script, extra = args.get("script", ""), args.get("args")
        expression = f"(function() {{ {script} }})({','.join(json.dumps(a) for a in extra)})" if extra else script
        try:
            return {"success": True, "result": evaluate(expression), "error": None}
        except Exception as exc:
            return {"success": False, "result": None, "error": str(exc)}
    if name == "select_option":
        element = select(args["selector"])
        if args.get("text") is not None:
            # `send_keys` into the list: Chrome's type-ahead picks the first choice whose text starts with the keys
            # (the lab, 13/11: day « 14 », month « Mars », year « 1991 » all set this way); typed into anything else
            typed = json.dumps(str(args["text"]).lower())
            if evaluate(f"(() => {{ const s = {element}; s.focus(); if (s.tagName !== 'SELECT') return false; "
                        f"const o = [...s.options].find((x) => x.text.trim().toLowerCase().startsWith({typed})); "
                        "if (o) { s.value = o.value; s.dispatchEvent(new Event('input', {bubbles: true})); "
                        "s.dispatchEvent(new Event('change', {bubbles: true})); } return true; })()"):
                return True
            for char in str(args["text"]):
                cdp("Input.insertText", {"text": char})
            return True
        target = (f"select.value = {json.dumps(args['value'])};" if args.get("value") is not None else
                  f"select.selectedIndex = {int(args['index'])};" if args.get("index") is not None else None)
        if target is None:
            raise Exception("Failed to select option: No selection criteria provided (value, text, or index)")
        cdp("Runtime.evaluate", {"expression": f"const select = document.querySelector({json.dumps(args['selector'])});"
                                               f" if (select) {{ {target} select.dispatchEvent(new Event('change', "
                                               f"{{bubbles: true}})); }}", "userGesture": True})
        return True
    raise Exception(f"Unknown tool: {name}")


for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {"protocolVersion": message["params"]["protocolVersion"], "capabilities": {"tools": {}},
                  "serverInfo": {"name": "stealth", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    else:
        name, args = message["params"]["name"], message["params"].get("arguments") or {}
        try:
            value = call(name, args)
            structured = value if isinstance(value, dict) else {"result": value}
            result = {"content": [{"type": "text", "text": json.dumps(value)}], "structuredContent": structured}
        except Exception as exc:
            result = {"content": [{"type": "text", "text": f"Error calling tool '{name}': {exc}"}], "isError": True}
    print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}), flush=True)
