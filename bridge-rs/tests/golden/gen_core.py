"""Golden fixtures for the core's pure functions (hermes SSE parsing and line splitting, store timestamps, logs,
auth, Python's json.dumps / float repr / int() / str()). Run from bridge/ with the venv:

    cd bridge && "<venv>/bin/python" ../bridge-rs/tests/golden/gen_core.py
"""
import json
import logging
import os
import sys
from datetime import datetime, timedelta, timezone

sys.path.insert(0, os.getcwd())
from httpx._decoders import LineDecoder  # noqa: E402

from bipbridge.auth import _extract_bearer  # noqa: E402
from bipbridge.hermes import SSEParser  # noqa: E402
from bipbridge.logs import JsonFormatter, fields, redact  # noqa: E402
from bipbridge.store import iso, parse_since  # noqa: E402
from uvicorn.middleware.proxy_headers import _TrustedHosts  # noqa: E402

out = {}

# -- SSE ---------------------------------------------------------------------------------------------
SSE = [
    ': keepalive\n\nevent: message.delta\ndata: {"event": "message.delta", "run_id": "r1", "delta": "Bon"}\n\n'
    'data: {"type": "assistant.delta", "delta": "jour"}\n\nevent: run.completed\ndata: {"run_id": "r1"}\n\n',
    ": keepalive\n\n: keepalive\n\n",
    'data: {"type": "x",\ndata:  "delta": "a"}\n\ndata: [DONE]\n\n',
    '{"type": "message.delta", "delta": "hi"}\nevent: note\ndata: plain words\n\n',
    'data: {"type": "message.delta", "delta": {"content": "x"}}\n\ndata: {"type": "run.completed", "output": "Réponse finale."}\n\n',
    'data: {"type": "", "event": "e"}\n\ndata: {"type": 0, "event": false}\n\ndata: {"type": true}\n\n',
    'data: [1, 2]\n\ndata: "str"\n\ndata: 5\n\ndata: null\n\n',
    "event:\ndata: x\n\nevent:   spaced  \n\nid: 7\nretry: 100\nfoo: bar\n\n",
    'data:nospace\n\ndata:  two spaces\n\ndata\n\n',
    'event: run.failed\ndata: {"error": "boom", "text": "  "}',
    '   {"type": "indented"}  \ndata: {"type": "after"}\n\n',
    'event: a\n{"type": "not bare"}\n\n',
    'data: {"delta": {"text": "t", "content": ""}, "text": 5}\n\ndata: {"content": {"text": 1}, "result": "r"}\n\n',
    'data: {"final_output": "  ", "response": "resp"}\n\ndata:  [DONE] \n\n',
    "data: line1\r\ndata: line2\r\n\r\n",
]


def event(e):
    return {"type": e.type, "data": e.data, "raw": e.raw, "text": e.text, "final_text": e.final_text}


cases = []
for text in SSE:
    parser = SSEParser()
    events = []
    for line in text.split("\n"):
        ev = parser.feed_line(line)
        if ev:
            events.append(event(ev))
    last = parser.finish()
    if last:
        events.append(event(last))
    cases.append({"input": text, "output": events})
out["sse"] = cases

# -- httpx's aiter_lines -----------------------------------------------------------------------------
CHUNKS = [
    ["a\r", "\nb\n\nc"],
    ["x\r", "y"],
    ["one\ntwo\n"],
    ["no newline"],
    ["\n", "\n", "a"],
    ["a b\x0bc\x0cd\x1ce\x85f", "\r\n"],
    ["split ", "across ", "chunks\n", "end"],
    ["\r", "\r", "\n"],
    ["trailing\r"],
    [""],
]
cases = []
for chunks in CHUNKS:
    d = LineDecoder()
    lines = []
    for c in chunks:
        if c:
            lines.extend(d.decode(c))
    lines.extend(d.flush())
    cases.append({"input": chunks, "output": lines})
out["lines"] = cases

# -- store timestamps ----------------------------------------------------------------------------------
base = datetime(2026, 10, 4, 8, 15, 0, 123999, tzinfo=timezone.utc)
out["iso"] = [
    {"input": [2026, 10, 4, 8, 15, 0, 123999, 0], "output": iso(base)},
    {"input": [2026, 10, 4, 10, 15, 0, 5000, 120], "output": iso(datetime(2026, 10, 4, 10, 15, 0, 5000,
                                                                        tzinfo=timezone(timedelta(minutes=120))))},
]
SINCE = ["2026-10-04T08:15:00.123Z", "2026-10-04T10:15:00+02:00", "2026-10-04T08:15:00", "2026-10-04", " 0 ",
         "1.5", "1759565700", "2026-10-04 08:15:00.5-01:30", "20261004T081500", "2026-10-04T08:15Z", "2026-10-04T08",
         "yesterday", "", "2026-13-01", "-5", "1e3", "2026-10-04T08:15:00.123456789Z"]
cases = []
for value in SINCE:
    try:
        cases.append({"input": value, "output": iso(parse_since(value))})
    except (ValueError, OverflowError, OSError) as exc:
        cases.append({"input": value, "error": type(exc).__name__})
out["parse_since"] = cases

# -- logs and auth -----------------------------------------------------------------------------------
out["redact"] = [{"input": v, "output": redact(v)} for v in [None, "", "abc", "abcdef", "abcdefg", "éééééééé"]]
out["bearer"] = [{"input": v, "output": _extract_bearer(v)} for v in
                 [None, "", "Bearer abc", "bearer  abc ", "BEARER x y", "Basic abc", "abc", "Bearer ", "Bearer\tabc"]]
trusted = _TrustedHosts("127.0.0.1")
out["forwarded"] = [{"input": v, "output": list(trusted.get_trusted_client_address(v))} for v in
                    ["100.64.0.7", "100.64.0.7, 127.0.0.1", "127.0.0.1", "1.1.1.1, 2.2.2.2", " 3.3.3.3 ", "", "a,,b",
                     "100.64.0.7:5000", "[fd7a::1]:443", "[fd7a::1]", "fd7a::1", "127.0.0.1, 127.0.0.1", "x:y", "[bad"]]

formatter = JsonFormatter()
record = logging.LogRecord("bipbridge.guard", logging.WARNING, __file__, 1, "wrong key", None, None)
record.fields = fields(address="203.0.113.9", what="bridge key", count=3, agents=["a", "b"], ok=True, ratio=0.5)["fields"]
line = formatter.format(record)
payload = json.loads(line)
payload.pop("ts")
out["log_line"] = {"line_without_ts": json.dumps(payload, ensure_ascii=False, default=str), "keys": list(json.loads(line))}

# -- Python's json.dumps, repr(float), int(), str() -----------------------------------------------------
VALUES = [["b", "a"], {"a": 1, "b": [True, None], "c": "é"}, "😀", {"t": {"a": [1]}}, {}, [], "\x00\x1f\x7f \"\\",
          1.0, 0.1, 1e16, 1.5e-7, 123456.789, 0.0001, -2.5, 1e15, 3, -7, {"k": 1.25e-5}]
out["dumps"] = [{"input": v, "default": json.dumps(v), "indent1": json.dumps(v, indent=1),
                 "compact_unicode": json.dumps(v, ensure_ascii=False, separators=(",", ":")),
                 "sorted": json.dumps(v, sort_keys=True)} for v in VALUES]
FLOATS = [1.0, 0.1, 1e16, 1.5e-7, 123456.789, 0.0001, -2.5, 1e15, 2.0 ** 60, 5e-324, 1.7976931348623157e308, 100.0, 1e22, 12345678901234567.0]
out["float_repr"] = [{"input": f, "output": repr(f)} for f in FLOATS]
INTS = ["1_000", " 7 ", "+3", "-4", "x", "", "1__0", "_1", "1.5", 2.9, -2.9, True, None, [1]]
cases = []
for v in INTS:
    try:
        cases.append({"input": v, "output": int(v)})
    except (ValueError, TypeError):
        cases.append({"input": v, "error": True})
out["int"] = cases
out["str"] = [{"input": v, "output": str(v)} for v in [True, False, None, 1, 1.0, 2.5, "s", ["a", 1, None, True], {"k": "v", "n": 1.5}, "it's", 'say "hi"']]

path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "core.json")
with open(path, "w", encoding="utf-8") as fh:
    json.dump(out, fh, ensure_ascii=False, indent=1)
    fh.write("\n")
print({k: len(v) for k, v in out.items()}, "->", path)
