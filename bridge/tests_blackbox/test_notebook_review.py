"""Carnet v2, B3 (internal/notes/carnet-v2/contract.md §3.6): the week's review comes only for someone who lives in
the Carnet (4 days a week for 3 weeks, 12 pages, 8 new facts: proved on a fake clock in bridge-rs review.rs); here, an
occasional use gets nothing, and the app's days are counted once a day, nothing else kept. Scripted, no model."""
import json

from bbkit.common import AUTH
from bbkit.mcp import call, give_token, text_of
from bbkit.servers import wait_until


def test_an_occasional_use_gets_no_review_and_the_days_are_counted(bridges, fakes):
    bridge = bridges.new().start()
    give_token(bridge)
    assert bridge.patch("/v1/notebook/settings", json={"language": "fr"}, headers=AUTH).status_code == 200
    done = call(bridge, "notebook_write", {"page": "Léa Martin", "kind": "person", "facts": ["Sœur"]})
    assert done["isError"] is False, text_of(done)
    assert bridge.get("/v1/notebook/review", params={"lang": "fr"}, headers=AUTH).status_code == 204
    home = bridge.get("/v1/notebook", params={"lang": "fr"}, headers=AUTH).json()
    assert home["review"] is None
    # the app's day (counted off the request, once the Carnet is there): one bit, no time, no screen
    def counted():
        return json.loads((bridge.state_dir / "notebook" / "state.json").read_text()).get("active_days")
    assert wait_until(counted, timeout=5), "the app's day is not counted"
    state = json.loads((bridge.state_dir / "notebook" / "state.json").read_text())
    assert state["active_days"]["bits"] & 1 == 1 and set(state["active_days"]) == {"day", "bits"}
    assert bridge.post("/v1/notebook/review/done", json={"off": True}, headers=AUTH).status_code == 204
    state = json.loads((bridge.state_dir / "notebook" / "state.json").read_text())
    assert state["review"]["off"] is True
