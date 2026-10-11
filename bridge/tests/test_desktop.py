import struct

from bipbridge.desktop import InputGate, app_holds, set_lease


def test_watching_sends_no_input_and_taking_over_does():
    allowed = {"on": False}
    gate = InputGate(lambda: allowed["on"])
    handshake = b"RFB 003.008\n" + b"\x01" + b"\x00"          # version, security None, ClientInit not shared
    assert gate.feed(handshake) == b"RFB 003.008\n\x01\x01"   # always shared
    update = b"\x03" + b"\x00" + struct.pack(">HHHH", 0, 0, 100, 100)
    key = b"\x04\x01\x00\x00" + struct.pack(">I", 0x61)
    pointer = b"\x05\x01" + struct.pack(">HH", 10, 10)
    encodings = b"\x02\x00" + struct.pack(">H", 2) + struct.pack(">ii", 0, -223)
    assert gate.feed(update + key + pointer + encodings) == update + encodings
    allowed["on"] = True
    assert gate.feed(key[:3]) == b"" and gate.feed(key[3:] + pointer) == key + pointer   # split across frames


def test_the_lease_is_hermes_own_file(tmp_path):
    folder = tmp_path / "bot-desktop"
    assert not app_holds(folder)
    lease = set_lease(folder, True, reason="test")
    assert lease["holder"] == "human" and lease["epoch"] == 1 and app_holds(folder)
    assert set_lease(folder, True)["epoch"] == 1                   # already ours: no change
    assert set_lease(folder, False)["holder"] == "agent" and not app_holds(folder)
    (folder / "lease.json").write_text("{broken")
    assert not app_holds(folder)                                    # unreadable: a human holds (Hermes' fail-safe)


def test_routes(client):
    from conftest import AUTH
    assert client.post("/v1/agents/wellness/desktop", headers=AUTH).status_code == 409   # set up by hand, no viewer page
    assert client.post("/v1/agents/nobody/desktop", headers=AUTH).status_code == 404
    assert client.post("/v1/agents/wellness/desktop").status_code == 401
    assert client.get("/desktop/nope/").status_code == 404
    assert client.get("/desktop/static/novnc/core/rfb.js").status_code == 200


def test_the_bridge_logs_in_to_a_password_protected_vnc():
    import asyncio
    from bipbridge.desktop import Upstream, vnc_login, vnc_response

    challenge = bytes(range(16))

    async def scenario(password_ok: bool):
        received = bytearray()

        async def server(reader, writer):
            writer.write(b"RFB 003.008\n")
            await writer.drain()
            received.extend(await reader.readexactly(12))
            writer.write(b"\x01\x02")                       # one type: VNC Authentication
            await writer.drain()
            received.extend(await reader.readexactly(1))
            writer.write(challenge)
            await writer.drain()
            answer = await reader.readexactly(16)
            ok = answer == vnc_response("MotVNC-42", challenge)
            writer.write(b"\x00\x00\x00\x00" if ok else b"\x00\x00\x00\x01")
            await writer.drain()

        srv = await asyncio.start_server(server, "127.0.0.1", 0)
        port = srv.sockets[0].getsockname()[1]
        reader, writer = await asyncio.open_connection("127.0.0.1", port)
        try:
            await vnc_login(Upstream(reader, writer, writer.close), "MotVNC-42" if password_ok else "faux")
            result = True
        except ConnectionError:
            result = False
        writer.close()
        srv.close()
        return result, bytes(received)

    ok, sent = asyncio.run(scenario(True))
    assert ok and sent == b"RFB 003.008\n\x02"
    assert asyncio.run(scenario(False))[0] is False
    # The DES quirk itself (bit-reversed key bytes) is checked against a real TigerVNC server, not here.
    assert len(vnc_response("MotVNC-42", challenge)) == 16


def test_the_vnc_password_file_may_be_clear_or_vncpasswd(tmp_path):
    from bipbridge.desktop import read_vnc_password

    clear = tmp_path / "clear.txt"
    clear.write_text("MotVNC-42\n")
    assert read_vnc_password(str(clear)) == "MotVNC-42"
    # Written by TigerVNC's own `vncpasswd` for « MotVNC-42 » (VNC keeps the first 8 characters).
    obscured = tmp_path / "passwd"
    obscured.write_bytes(bytes.fromhex("5bb576d0733bc57b"))
    assert read_vnc_password(str(obscured)) == "MotVNC-4"
