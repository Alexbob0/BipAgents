"""Golden fixtures for src/agents/pyyaml.rs: PyYAML's safe_load (as JSON, with dates as their text) and
safe_dump(allow_unicode=True, sort_keys=False) of YAML texts and of JSON values.

Run from bridge/: "<venv>/bin/python" ../bridge-rs/tests/golden/gen_pyyaml.py"""
import datetime
import json
import os

import yaml

HERE = os.path.dirname(os.path.abspath(__file__))


def jsonable(value):
    if isinstance(value, dict):
        return {str(k) if not isinstance(k, (bool, type(None))) else str(k): jsonable(v) for k, v in value.items()}
    if isinstance(value, list):
        return [jsonable(v) for v in value]
    if isinstance(value, datetime.datetime):
        return value.isoformat(" ")
    if isinstance(value, datetime.date):
        return value.isoformat()
    if isinstance(value, float) and (value != value or value in (float("inf"), float("-inf"))):
        return None
    return value


def dump(value):
    return yaml.safe_dump(value, allow_unicode=True, sort_keys=False)


TEXTS = [
    "model:\n  provider: custom\n  default: glm\n  base_url: http://gpu:8888/v1\nagent:\n  max_turns: 60\n",
    "a: yes\nb: off\nc: 0755\nd: 1:30\ne: 1_000\nf: 2026-10-21\ng: ~\nh: '0755'\ni: 1.5\nj: .inf\nk: 0x1F\nl: -.5\nm: 1e5\n",
    "when: 2026-10-21T08:15:00Z\nlater: 2026-10-21 08:15:00.25 +02:00\nlocal: 2026-1-2 3:04:05\n",
    "skills:\n  disabled: [plant-care, arxiv]\nmodel: {provider: custom}\n",
    "terminal:\n  backend: docker\n  docker_volumes:\n    - \"/home/x/photos:/photos:ro\"\n",
    "base: &b\n  x: 1\n  y: 2\nchild:\n  <<: *b\n  y: 3\n  z: 4\n",
    "list:\n- - 1\n  - 2\n- a: 1\n  b: [x]\n- {}\n- []\n",
    "description: Recettes et menus\nui_meta:\n  hermes-bots: {}\n",
    "1: one\ntrue: yes\nnull: x\n3.5: f\n",
    "text: |\n  line one\n  line two\nfolded: >\n  a\n  b\n",
    "quoted: \"tab\\there\"\nsingle: 'it''s'\nempty: ''\nspaces: '  lead'\n",
    "",
    "# only a comment\n",
    "- a\n- b\n",
    "just a string\n",
    "mcp_servers:\n  bipagents:\n    url: http://127.0.0.1:8643/mcp\n    headers:\n      Authorization: Bearer abc\n    timeout: 120\n",
]

LONG = ("You are Max, your user's money agent: spending, budget, subscriptions, savings. You are precise, careful "
        "and direct; you show the numbers and where they come from.")

VALUES = [
    {"ui_meta": {"hermes-bots": {}}},
    {"description": LONG},
    {"description": "Tu es Fougère, l'agent du quotidien — très « concret », avec des émojis 🌱 et des accents."},
    {"k": LONG.replace(" ", "  ")},
    {"k": "a" * 100 + " " + "b" * 10},
    {"k": "x " * 60},
    {"k": "line one\nline two"},
    {"k": "line one\n\nline two\n"},
    {"k": "trailing space "},
    {"k": " leading"},
    {"k": "tab\there and a \"quote\" and a \\ backslash " + "z" * 70},
    {"k": "it's 'quoted' " * 8},
    {"keys": {"yes": 1, "no": 2, "on": "off", "null": None, "~": "~", "": "", "-": "-", "- x": "?", "? x": ":",
              ": x": "a #b", "a#b": "#c", "x: y": "[a]", "{a}": "&x", "*x": "!x", "%x": "@x", "`x": "|x", ">x": "'x"}},
    {"numbers": ["1", "1.5", "0755", "0x1F", "1:30", "1e5", "1.0e+5", ".5", "-1", "+1", "1_000", "0b101", ".inf", ".nan",
                 "2026-10-21", "2026-10-21 08:00:00", "=", "<<", "---", "...", "--- x", "... x", "a,b", "a[b]", "a{b}"]},
    {"floats": [1.0, 0.1, 1e17, 1.5e-07, -2.25, 100.0, 1e16, 123456789.123]},
    {"ints": [0, -1, 2 ** 40, 7]},
    {"bools": [True, False, None]},
    {"nested": [[1, [2, []]], {"a": {"b": {"c": [{"d": 1, "e": [1, 2]}]}}}, [{"x": {}}]]},
    {"agent": {"max_turns": 60, "reasoning_effort": "xhigh"}, "providers": {"bipagents-thinking": {
        "api": "http://gpu:8888/v1", "extra_body": {"chat_template_kwargs": {"enable_thinking": False}}}}},
    {"skills": {"disabled": ["arxiv", "monthly-budget"]}},
    {"k" * 130: "a long key"},
    {"emoji 🎉": "value 🎉 " + "w" * 80},
    {"ctl": "a\u0007b\u001bc d\u0085e f﻿g"},
    {"k": "é" * 90 + " " + "è" * 5},
    {"k": "word " * 3 + "x" * 90 + " end"},
    {"url": "http://example.org/a#b", "comment": "a #b", "colon": "a:b", "colon_space": "a: b", "dash": "-a", "q": "?a"},
    ["top", "level"],
    "a root string",
    "",
    None,
    {"multi": "first line that is quite long, really quite long, longer than eighty columns for sure\nsecond"},
    {"spaced": "a  b   c    d" * 10},
    {"lead_break": "\nx", "trail_break": "x\n", "break_space": "a\n b", "space_break": "a \nb"},
]

out = []
for text in TEXTS:
    data = yaml.safe_load(text)
    out.append({"kind": "text", "input": text, "json": jsonable(data), "output": dump(data)})
for value in VALUES:
    out.append({"kind": "value", "input": value, "output": dump(value)})

with open(os.path.join(HERE, "pyyaml.json"), "w", encoding="utf-8") as fh:
    json.dump(out, fh, ensure_ascii=False, indent=1)
print(len(out), "cases")
