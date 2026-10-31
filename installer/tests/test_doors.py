"""Every place's door on 443 (installer/cloud/seat-agent.py: doors): root's router by the names the clients ask for,
and root's tools following root's checkout forward only (installer/bipinstall/server_tools.py)."""
import json
import socket
import ssl
import subprocess
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def _seat_agent():
    import importlib.util
    spec = importlib.util.spec_from_file_location("seat_agent_doors", ROOT / "installer" / "cloud" / "seat-agent.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _paths(agent, tmp_path, monkeypatch):
    for name, file in (("DOORS_CONF", "doors.cfg"), ("DOORS_NFT", "doors.nft"), ("DOORS_UNIT", "doors.service")):
        monkeypatch.setattr(agent, name, str(tmp_path / file))
    return str(tmp_path / "doors.json")


class Runner:
    def __init__(self, active=False, table=False):
        self.calls, self.active, self.table = [], active, table

    def __call__(self, *args):
        self.calls.append(args)
        if args[:3] == ("systemctl", "is-active", "--quiet"):
            return 0 if self.active else 3
        if args[:2] == ("systemctl", "start"):
            self.active = True
        if args[:3] == ("nft", "list", "table"):
            return 0 if self.table else 1
        return 0


def test_hosts_are_valid_names_and_never_shared():
    agent = _seat_agent()
    answer = {"hosts": {"1": "abcdefghij.p.bipagents.com", "2": "klmnopqrst.p.bipagents.com",
                        "3": "bad name.p.bipagents.com", "4": "x; rm -rf /", "5": "UPPER.p.bipagents.com",
                        "6": "same.p.bipagents.com", "7": "same.p.bipagents.com", "41": "far.p.bipagents.com",
                        "z": "zz.p.bipagents.com"}}
    assert agent.door_hosts(answer) == {1: "abcdefghij.p.bipagents.com", 2: "klmnopqrst.p.bipagents.com"}
    assert agent.door_hosts({}) == {} and agent.door_hosts({"hosts": ["a"]}) == {}


def test_the_router_text_sends_each_name_to_its_place_with_the_clients_address():
    agent = _seat_agent()
    text = agent.doors_config({"aaaa.p.bipagents.com": 1, "bbbb.p.bipagents.com": 3}, 1, True)
    assert "bind 0.0.0.0:440" in text and "bind [::]:440 v6only" in text
    assert "use_backend place3 if { req.ssl_sni -i bbbb.p.bipagents.com }" in text
    assert "use_backend place1 if !{ req.ssl_sni -m found }" in text
    assert "server door 127.0.0.1:445 send-proxy-v2" in text and "server door 127.0.0.1:443 send-proxy-v2" in text
    assert "default_backend" not in text   # a name nobody has: closed, never another place's
    assert "log" not in text.replace("global", "")   # nothing of anyone kept
    assert "[::]" not in agent.doors_config({}, 1, False)
    text.encode("ascii")


def test_the_redirect_file_replaces_its_table_and_only_removes_it_when_off():
    agent = _seat_agent()
    on, off = agent.doors_nft(True), agent.doors_nft(False)
    assert "tcp dport 443 redirect to :440" in on and "type nat hook prerouting priority dstnat" in on
    assert off == "table inet bipagents_doors\ndelete table inet bipagents_doors\n"
    assert on.startswith(off)


def test_nothing_goes_through_the_router_until_place_1_takes_the_header(tmp_path, monkeypatch):
    agent = _seat_agent()
    state = _paths(agent, tmp_path, monkeypatch)
    runner = Runner()
    answer = {"hosts": {"1": "one.p.bipagents.com", "2": "two.p.bipagents.com"}}
    probes = {443: False, 444: True}
    reached = agent.doors(answer, state, runner, probe=lambda port: probes[port], exists=lambda n: n <= 2,
                          ready=lambda runner: True)
    assert reached == [] and not (tmp_path / "doors.cfg").exists()
    assert not [c for c in runner.calls if c[:2] == ("systemctl", "start")]
    assert json.loads(Path(state).read_text())["proxy"] == {"1": False, "2": True}


def test_once_place_1_takes_it_the_router_and_the_redirect_go_on(tmp_path, monkeypatch):
    agent = _seat_agent()
    state = _paths(agent, tmp_path, monkeypatch)
    runner = Runner()
    answer = {"hosts": {"1": "one.p.bipagents.com", "2": "two.p.bipagents.com", "3": "three.p.bipagents.com"}}
    probes = {443: True, 444: True, 445: False}
    reached = agent.doors(answer, state, runner, probe=lambda port: probes[port], exists=lambda n: n <= 3,
                          ready=lambda runner: True)
    assert reached == [1, 2]   # place 3's older bridge keeps its own door only
    text = (tmp_path / "doors.cfg").read_text()
    assert "two.p.bipagents.com" in text and "three.p.bipagents.com" not in text
    assert ("systemctl", "start", "bipagents-doors.service") in runner.calls
    assert ("nft", "-f", str(tmp_path / "doors.nft")) in runner.calls
    assert ("ufw", "allow", "440/tcp") in runner.calls
    assert agent.doors_reached(state) == {1, 2}
    # next round, nothing changed: no reload, no new redirect
    runner2 = Runner(active=True, table=True)
    assert agent.doors(answer, state, runner2, probe=lambda port: probes[port], exists=lambda n: n <= 3,
                       ready=lambda runner: True) == [1, 2]
    assert not [c for c in runner2.calls if c[:2] in (("systemctl", "reload"), ("nft", "-f"))]


def test_a_door_that_does_not_answer_keeps_what_it_said_last(tmp_path, monkeypatch):
    agent = _seat_agent()
    state = _paths(agent, tmp_path, monkeypatch)
    answer = {"hosts": {"2": "two.p.bipagents.com"}}
    ok = {443: True, 444: True}
    agent.doors(answer, state, Runner(), probe=lambda port: ok[port], exists=lambda n: n <= 2, ready=lambda runner: True)
    # place 1 restarting (no answer), place 2 too: still routed, the redirect stays
    reached = agent.doors(answer, state, Runner(active=True, table=True), probe=lambda port: None,
                          exists=lambda n: n <= 2, ready=lambda runner: True)
    assert reached == [1, 2]
    # place 1 back to an older bridge (an update's way back): everything off, the redirect removed
    runner = Runner(active=True, table=True)
    off = {443: False, 444: True}
    assert agent.doors(answer, state, runner, probe=lambda port: off[port], exists=lambda n: n <= 2,
                       ready=lambda runner: True) == []
    assert ("systemctl", "disable", "--now", "bipagents-doors.service") in runner.calls
    assert (tmp_path / "doors.nft").read_text() == agent.doors_nft(False)


def test_the_beat_names_the_places_443_reaches(tmp_path, monkeypatch):
    agent = _seat_agent()
    state = tmp_path / "doors.json"
    state.write_text(json.dumps({"reached": [1, 3, "x"]}))
    assert agent.doors_reached(str(state)) == {1, 3}
    assert agent.doors_reached(str(tmp_path / "none.json")) == set()


def _tls_server(header_ok):
    """A door on 127.0.0.1: TLS, after a PROXY header when `header_ok` (else the header breaks its handshake)."""
    import tempfile
    folder = Path(tempfile.mkdtemp())
    key, cert = folder / "k.pem", folder / "c.pem"
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", str(key), "-out",
                    str(cert), "-days", "1", "-subj", "/CN=door"], check=True, capture_output=True)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(str(cert), str(key))
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(8)

    def serve():
        while True:
            try:
                conn, _ = listener.accept()
            except OSError:
                return
            try:
                if header_ok:
                    first = conn.recv(12, socket.MSG_PEEK)
                    if first == b"\r\n\r\n\x00\r\nQUIT\n":
                        conn.recv(16)
                with context.wrap_socket(conn, server_side=True) as tls:
                    tls.recv(1)
            except (OSError, ssl.SSLError):
                pass
            finally:
                conn.close()
    threading.Thread(target=serve, daemon=True).start()
    return listener


def test_the_probe_tells_a_bridge_that_takes_the_header_from_an_older_one_and_a_closed_door():
    agent = _seat_agent()
    new, old = _tls_server(True), _tls_server(False)
    try:
        assert agent.proxy_probe(new.getsockname()[1], timeout=3) is True
        assert agent.proxy_probe(old.getsockname()[1], timeout=3) is False
        closed = socket.socket()
        closed.bind(("127.0.0.1", 0))
        port = closed.getsockname()[1]
        closed.close()
        assert agent.proxy_probe(port, timeout=1) is None
    finally:
        new.close()
        old.close()


def _repo(tmp_path):
    repo = tmp_path / "repo"
    (repo / "installer" / "cloud").mkdir(parents=True)
    git = lambda *a: subprocess.run(["git", "-C", str(repo), *a], check=True, capture_output=True)
    git("init", "-q")
    git("config", "user.email", "t@example.com")
    git("config", "user.name", "t")
    return repo, git


def test_root_tools_follow_the_checkout_forward_never_back(tmp_path):
    import sys
    sys.path.insert(0, str(ROOT / "installer"))
    from bipinstall import server_tools
    repo, git = _repo(tmp_path)
    lib = tmp_path / "lib"
    lib.mkdir()
    (lib / "place-move.py").write_text("old move\n")
    cloud = repo / "installer" / "cloud"
    for name in server_tools.FILES:
        (cloud / name).write_text("v1 %s\n" % name)
    git("add", "-A")
    git("commit", "-qm", "v1")
    first = server_tools.refresh(repo, lib, root_only=False)
    # seat-agent.py and add-seat.sh always; the others only where the server had them
    assert sorted(first) == ["add-seat.sh", "place-move.py", "seat-agent.py"]
    assert not (lib / "nightly-reboot.sh").exists()
    v1 = subprocess.run(["git", "-C", str(repo), "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    assert (lib / "tools-commit").read_text().strip() == v1
    assert server_tools.refresh(repo, lib, root_only=False) == []   # same commit: nothing
    (cloud / "seat-agent.py").write_text("v2\n")
    git("commit", "-qam", "v2")
    assert server_tools.refresh(repo, lib, root_only=False) == ["seat-agent.py"]
    assert (lib / "seat-agent.py").read_text() == "v2\n"
    git("checkout", "-q", v1)   # a wave's way back: the newer tools stay
    assert server_tools.refresh(repo, lib, root_only=False) == []
    assert (lib / "seat-agent.py").read_text() == "v2\n"


def test_seat_agent_stays_ascii():
    (ROOT / "installer" / "cloud" / "seat-agent.py").read_bytes().decode("ascii")
    (ROOT / "installer" / "bipinstall" / "server_tools.py").read_bytes().decode("ascii")
