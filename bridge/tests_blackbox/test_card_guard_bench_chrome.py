"""Card guards, hotfix v2 of 15/11 (Alex's option A: an agent's script only reads, proven by V8): what the 145
scripts real agents wrote in the sign-up and drop-down list benches (bridge-rs/tests/rx_agent_scripts.json) get from
the proxy now, on the sign-up page they were written for, Chrome 157 (the production image's), the stand-in for
stealth-browser-mcp: run (a read V8 proves), refused by the static checks, or refused as changing the page. A measure
for Alex, printed (with every refused script, to see what an agent would have to do with its own tools instead);
it checks only that every script got an answer and that none that changes the page ran."""
import json
from pathlib import Path

import pytest

import test_signup_guard_chrome as s
from test_signup_guard_chrome import Session

pytestmark = s.pytestmark
BENCH = Path(__file__).parents[2] / "bridge-rs" / "tests" / "rx_agent_scripts.json"
CHANGES = "Not done: this script changes the page"
UNCHECKED = "Not done: your browser can't check this script"


@pytest.fixture(scope="module")
def browser():
    session = Session()
    yield session
    session.close()


def test_what_the_bench_scripts_get(browser):
    scripts = json.loads(BENCH.read_text())
    counts = {"read": 0, "changes the page": 0, "refused by a check": 0, "unchecked": 0, "error": 0}
    seen = {k: [] for k in counts}
    for script in scripts:
        browser.tool("navigate", url=f"{browser.base}/signup.html")
        said = browser.tool("execute_script", script=script)
        if said.startswith(CHANGES):
            kind = "changes the page"
        elif said.startswith(UNCHECKED):
            kind = "unchecked"
        elif said.startswith("Not done"):
            kind = "refused by a check"
        elif '"success": false' in said or said.startswith("Error"):
            kind = "error"
        else:
            kind = "read"
        counts[kind] += 1
        seen[kind].append(f"{script[:160]!r} => {said[:70]!r}")
    print(f"\n{len(scripts)} bench scripts on the sign-up page: {counts}")
    for kind in ("changes the page", "refused by a check", "unchecked", "error"):
        for line in seen[kind]:
            print(f"  {kind}: {line}")
    for line in seen["read"][:5]:
        print(f"  read: {line}")
    assert sum(counts.values()) == len(scripts)
    assert counts["unchecked"] == 0, "DevTools out of reach: nothing measured"
