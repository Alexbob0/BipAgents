"""The DevTools relay's secret (python3 -m pytest browser/test_devtools_relay.py; security audit 2026-10-27, 11 and 21):
nothing reaches Chrome without the browser's secret, shown as `X-Bip-Cdp` or as a `/t/<secret>/` path prefix, and a
web page's request (it carries an Origin) never does. Against a fake Chrome on this machine's loopback."""
import asyncio
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import devtools_relay  # noqa: E402

SECRET = "0123456789abcdef0123456789abcdef"
HEAD = b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9223\r\n"


def use_secret(monkeypatch, tmp_path, text):
    path = tmp_path / "cdp.token"
    if text is not None:
        path.write_text(text)
    monkeypatch.setattr(devtools_relay, "SECRET_FILE", str(path), raising=False)
    monkeypatch.setattr(devtools_relay, "HOLD", str(tmp_path / "hold"))   # nothing held
    import hold_gate
    monkeypatch.setattr(hold_gate, "HOLD", str(tmp_path / "hold"))


def test_a_request_must_show_the_secret(monkeypatch, tmp_path):
    use_secret(monkeypatch, tmp_path, SECRET + "\n")
    assert devtools_relay.authenticate(HEAD + b"\r\n") == (None, None)
    assert devtools_relay.authenticate(HEAD + b"X-Bip-Cdp: " + b"f" * 32 + b"\r\n\r\n") == (None, None)
    head = HEAD + b"x-bip-cdp: " + SECRET.encode() + b"\r\n\r\n"
    assert devtools_relay.authenticate(head) == (head, None)
    # in front of the path: taken off before Chrome sees it, kept to put back in Chrome's answer
    sent, prefix = devtools_relay.authenticate(
        b"GET /t/" + SECRET.encode() + b"/devtools/browser/old HTTP/1.1\r\nHost: x\r\n\r\n")
    assert sent == b"GET /devtools/browser/old HTTP/1.1\r\nHost: x\r\n\r\n" and prefix == b"/t/" + SECRET.encode()
    sent, _ = devtools_relay.authenticate(b"GET /t/" + SECRET.encode() + b" HTTP/1.1\r\nHost: x\r\n\r\n")
    assert sent.startswith(b"GET / HTTP/1.1\r\n")
    assert devtools_relay.authenticate(b"GET /t/abc/json/version HTTP/1.1\r\nHost: x\r\n\r\n") == (None, None)


def test_a_web_pages_request_never_gets_through(monkeypatch, tmp_path):
    use_secret(monkeypatch, tmp_path, SECRET)
    shown = HEAD + b"X-Bip-Cdp: " + SECRET.encode() + b"\r\nOrigin: https://evil.example\r\n\r\n"
    assert devtools_relay.authenticate(shown) == (None, None)
    use_secret(monkeypatch, tmp_path, None)   # even in a container without the secret
    assert devtools_relay.authenticate(HEAD + b"Origin: null\r\n\r\n") == (None, None)


def test_a_mounted_but_unusable_secret_lets_nothing_through(monkeypatch, tmp_path):
    use_secret(monkeypatch, tmp_path, "short")
    assert devtools_relay.secret() == b""
    assert devtools_relay.authenticate(HEAD + b"X-Bip-Cdp: short\r\n\r\n") == (None, None)


def test_a_container_from_an_older_unit_works_as_before(monkeypatch, tmp_path):
    use_secret(monkeypatch, tmp_path, None)
    assert devtools_relay.secret() is None
    assert devtools_relay.authenticate(HEAD + b"\r\n") == (HEAD + b"\r\n", None)
    sent, prefix = devtools_relay.authenticate(b"GET /t/whatever/json/version HTTP/1.1\r\nHost: x\r\n\r\n")
    assert sent.startswith(b"GET /json/version ") and prefix == b"/t/whatever"


def test_chromes_addresses_come_back_through_the_prefix():
    body = json.dumps({"webSocketDebuggerUrl": "ws://127.0.0.1:19600/devtools/browser/abc",
                       "devtoolsFrontendUrl": "/devtools/inspector.html?ws=127.0.0.1:19600/devtools/page/X1"}).encode()
    out = json.loads(devtools_relay.with_prefix(body, b"/t/s3cret"))
    assert out["webSocketDebuggerUrl"] == "ws://127.0.0.1:19600/t/s3cret/devtools/browser/abc"
    assert out["devtoolsFrontendUrl"] == "/devtools/inspector.html?ws=127.0.0.1:19600/t/s3cret/devtools/page/X1"


async def fake_chrome(reader, writer):
    head = await reader.readuntil(b"\r\n\r\n")
    path = head.split(b" ", 2)[1].decode()
    body = json.dumps({"path": path, "webSocketDebuggerUrl": "ws://127.0.0.1:19600/devtools/browser/abc"}).encode()
    writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=UTF-8\r\nContent-Length: "
                 + str(len(body)).encode() + b"\r\n\r\n" + body)
    await writer.drain()
    writer.close()


async def ask(port, request):
    reader, writer = await asyncio.open_connection("127.0.0.1", port)
    writer.write(request)
    await writer.drain()
    answer = await asyncio.wait_for(reader.read(), 5)
    writer.close()
    return answer


def through_the_relay(requests):
    async def main():
        chrome = await asyncio.start_server(fake_chrome, "127.0.0.1", 0)
        devtools_relay.CHROME = ("127.0.0.1", chrome.sockets[0].getsockname()[1])
        relay = await asyncio.start_server(devtools_relay.relay, "127.0.0.1", 0)
        port = relay.sockets[0].getsockname()[1]
        try:
            return [await ask(port, r) for r in requests]
        finally:
            relay.close()
            chrome.close()
    return asyncio.run(main())


def test_end_to_end_only_the_secret_reaches_chrome(monkeypatch, tmp_path):
    use_secret(monkeypatch, tmp_path, SECRET)
    monkeypatch.setattr(devtools_relay, "CHROME", devtools_relay.CHROME)
    host = b"Host: 127.0.0.1:19600\r\n"
    bare, with_header, with_prefix, page = through_the_relay([
        b"GET /json/version HTTP/1.1\r\n" + host + b"\r\n",
        b"GET /json/version HTTP/1.1\r\n" + host + b"X-Bip-Cdp: " + SECRET.encode() + b"\r\n\r\n",
        b"GET /t/" + SECRET.encode() + b"/json/version HTTP/1.1\r\n" + host + b"Connection: keep-alive\r\n\r\n",
        b"GET /json/version HTTP/1.1\r\n" + host + b"X-Bip-Cdp: " + SECRET.encode()
        + b"\r\nOrigin: https://evil.example\r\n\r\n",
    ])
    assert bare == b"" and page == b""   # closed, nothing of Chrome's
    assert json.loads(with_header.partition(b"\r\n\r\n")[2])["path"] == "/json/version"
    head, _, body = with_prefix.partition(b"\r\n\r\n")
    answer = json.loads(body)
    assert answer["path"] == "/json/version"   # Chrome never sees the secret
    assert answer["webSocketDebuggerUrl"] == f"ws://127.0.0.1:19600/t/{SECRET}/devtools/browser/abc"
    assert f"Content-Length: {len(body)}".encode() in head and b"Connection: close" in head
