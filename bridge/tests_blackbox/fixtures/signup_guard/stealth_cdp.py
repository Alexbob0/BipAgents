"""A stand-in for stealth-browser-mcp (a7b9eea, src/dom_handler.py) over a real Chromium's DevTools, for the review of
fix-signup-hold: the same tools, output schemas and semantics where the guard of bip-browser-mcp depends on them.

- click_element: with `text_match`, the element whose text best matches it (nodriver `tab.find(text, best_match=True)`),
  the selector unused; else `tab.select(selector)` (waits for it); then a JS click (nodriver `Element.click`);
- type_text: waits for its element up to 10 s (nodriver `tab.select`), focuses it, empties it, types each character;
- press_key: focuses `selector`, one key; with Shift its text upper-cased (dom_handler.press_key);
- execute_script: `script`, or `(function() { script })(args…)` with `args` (dom_handler.execute_script).

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
    deadline = time.time() + timeout
    while not evaluate(f"!!document.querySelector({json.dumps(selector)})"):
        if time.time() > deadline:
            raise Exception(f"Element not found: {selector}")
        time.sleep(0.1)
    return f"document.querySelector({json.dumps(selector)})"


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
