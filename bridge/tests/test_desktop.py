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
