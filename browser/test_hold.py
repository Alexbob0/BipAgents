"""The hold flag as the browser container's two doors read it (python3 -m pytest browser/test_hold.py): the person's
hold stops everything; a card payment's hold (« card <token> ») lets only the bridge's own DevTools connections
through, and tells the agent's tools why they wait."""
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import devtools_relay  # noqa: E402
import hold_gate  # noqa: E402

HEAD = b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9223\r\n"


def use_flag(monkeypatch, tmp_path, text):
    flag = tmp_path / "hold"
    if text is not None:
        flag.write_bytes(text)
    monkeypatch.setattr(hold_gate, "HOLD", str(flag))
    monkeypatch.setattr(devtools_relay, "HOLD", str(flag))
    return flag


def test_the_relay_tells_its_version_even_while_held(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"human 0123abcd")
    assert devtools_relay.VERSION_PATH.match(b"GET /bipagents/relay HTTP/1.1\r\nHost: x\r\n\r\n")
    assert not devtools_relay.VERSION_PATH.match(HEAD + b"\r\n")
    answer = devtools_relay.version_answer()
    head, _, body = answer.partition(b"\r\n\r\n")
    assert head.startswith(b"HTTP/1.1 200 OK") and json.loads(body)["relay"] == 3 and json.loads(body)["hold_token"] is True
    assert f"Content-Length: {len(body)}".encode() in head


def test_nothing_held_lets_everything_through(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, None)
    assert devtools_relay.let_through(HEAD + b"\r\n")
    assert not hold_gate.held()


def test_the_persons_hold_stops_everything(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"")
    assert hold_gate.held() and not hold_gate.paying()
    assert devtools_relay.payment_token() is None
    assert not devtools_relay.let_through(HEAD + b"X-Bip-Card: abc\r\n\r\n")
    refusal = json.loads(hold_gate.refusal({"id": 3}))
    assert refusal["result"]["content"][0]["text"] == hold_gate.WAIT


def test_a_payment_lets_only_the_bridge_through(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"card 0123abcd")
    assert hold_gate.held() and hold_gate.paying()
    assert devtools_relay.let_through(HEAD + b"x-bip-card: 0123abcd\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"X-Bip-Card: 0123abce\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"\r\n")
    refusal = json.loads(hold_gate.refusal({"id": 3}))
    assert "card payment" in refusal["result"]["content"][0]["text"]


def test_the_persons_hold_lets_only_the_bridges_view_through(monkeypatch, tmp_path):
    use_flag(monkeypatch, tmp_path, b"human 77aa88bb")
    assert hold_gate.held() and not hold_gate.paying()
    assert devtools_relay.payment_token() is None and devtools_relay.hold_token() == b"77aa88bb"
    assert devtools_relay.let_through(HEAD + b"X-Bip-Hold: 77aa88bb\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"X-Bip-Hold: 77aa88bc\r\n\r\n")
    assert not devtools_relay.let_through(HEAD + b"\r\n")
    assert json.loads(hold_gate.refusal({"id": 3}))["result"]["content"][0]["text"] == hold_gate.WAIT


def test_an_old_hold_lapses(monkeypatch, tmp_path):
    flag = use_flag(monkeypatch, tmp_path, b"card 0123abcd")
    old = time.time() - hold_gate.LIMIT - 5
    os.utime(flag, (old, old))
    assert not hold_gate.held() and devtools_relay.let_through(HEAD + b"\r\n")
