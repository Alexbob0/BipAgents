"""The shared Hermes gc reads the places' pointers as root without entering anything of theirs (14/11, the disk
breakdown's tenth review): no mount is ever crossed (openat2, RESOLVE_NO_XDEV), except a place's own open encrypted
space (a gocryptfs FUSE of its uid at its view), only from a disposable child the gc never waits for past its time.
The FUSE tests need a mount namespace of their own: run under `unshare -rm`."""
import ctypes
import os
import select
import signal
import struct
import subprocess
import sys
import time
from pathlib import Path

import pytest

from bipinstall import hermes, shared_hermes

in_namespace = pytest.mark.skipif(os.geteuid() != 0, reason="a mount namespace of its own: run under `unshare -rm`")
DATA = Path(".local") / "share" / "bipagents"
VIEW = DATA / "space" / "open"
CIPHER = DATA / "space" / "cipher"
ENOENT, ENOSYS = 2, 38


def _server(tmp_path, *places):
    """Versions old and new; place 2 on new (a plain ~/.hermes); `old` used by nobody; the other places' homes."""
    base = tmp_path / "opt"
    for name in ("old", "new"):
        (base / name).mkdir(parents=True)
    two = tmp_path / "home" / "bip2"
    (two / ".hermes").mkdir(parents=True)
    (two / ".hermes" / "shared").symlink_to(base / "new")
    homes = [two]
    for number in places:
        home = tmp_path / "home" / ("bip%d" % number)
        home.mkdir(parents=True)
        homes.append(home)
    proc = tmp_path / "proc"
    proc.mkdir()
    return base, homes, proc


def _gc(base, homes, proc):
    return shared_hermes.gc(hermes.Runner(), base, homes, keep=["new"], proc=proc)


def _within(seconds, run, release=lambda: None):
    """run() in a child: (its exit code or None when still running after `seconds`, then killed; what it printed).
    Held past its time (in D on a FUSE request: the gc of before), `release()` (the daemon killed) frees it first."""
    reader, writer = os.pipe()
    pid = os.fork()
    if pid == 0:
        os.close(reader)
        os.dup2(writer, 1)
        sys.stdout = open(1, "w", closefd=False)            # past pytest's own capture
        try:
            run()
            sys.stdout.flush()
            os._exit(0)
        except BaseException:
            os._exit(2)
    os.close(writer)
    out, end, code = b"", time.monotonic() + seconds, None
    while time.monotonic() < end:
        ready, _, _ = select.select([reader], [], [], 0.2)
        if ready:
            chunk = os.read(reader, 65536)
            out += chunk
            if chunk:
                continue
        done, status = os.waitpid(pid, os.WNOHANG)
        if done:
            code = os.waitstatus_to_exitcode(status)
            break
    else:
        os.kill(pid, signal.SIGKILL)
        release()
        os.waitpid(pid, 0)
    os.close(reader)
    return code, out.decode(errors="replace")


def _held():
    return [line for line in subprocess.run(["ps", "-u", str(os.getuid()), "-o", "pid=,stat=,wchan:32="],
                                            capture_output=True, text=True).stdout.splitlines()
            if "request_wait_answer" in line]


def _attr(ino, mode, size=0):
    # fuse_attr: ino size blocks atime mtime ctime (u64), atimensec mtimensec ctimensec mode nlink uid gid rdev blksize
    # flags (u32)
    return struct.pack("<QQQQQQIIIIIIIIII", ino, size, 0, 0, 0, 0, 0, 0, 0, mode, 1, 0, 0, 0, 4096, 0)


def _space(target, how="answer", fstype=b"fuse.gocryptfs", source=None, delay=0.0, pointer=b""):
    """A FUSE at `target`, standing for a place's open space (gocryptfs: its type, its cipher folder for source):
    `answer` serves hermes/shared -> `pointer` (after `delay` s for the first request), `stuck` reads every request
    and answers none. (pid, a pipe telling « r » for each request it read, its connection's number)."""
    ready_r, ready_w = os.pipe()
    seen_r, seen_w = os.pipe()
    pid = os.fork()
    if pid == 0:
        try:
            os.close(ready_r)
            os.close(seen_r)
            libc = ctypes.CDLL(None, use_errno=True)
            fd = os.open("/dev/fuse", os.O_RDWR)
            options = "fd=%d,rootmode=40000,user_id=0,group_id=0,allow_other" % fd
            if libc.mount(source or b"a-place-s-fuse", str(target).encode(), fstype, 0, options.encode()) != 0:
                os._exit(3)
            waited = False
            nodes = {1: (0o40755, b""), 2: (0o40755, b""), 3: (0o120777, pointer)}
            names = {(1, b"hermes"): 2, (2, b"shared"): 3}
            while True:
                try:
                    data = os.read(fd, 1 << 20)
                except OSError:
                    os._exit(5)                                       # aborted
                _, opcode, unique, node = struct.unpack_from("<IIQQ", data)

                def reply(body=b"", error=0):
                    os.write(fd, struct.pack("<IiQ", 16 + len(body), -error, unique) + body)
                if opcode == 26:                                      # INIT
                    _, minor = struct.unpack_from("<II", data, 40)
                    reply(struct.pack("<IIIIHHIIHHII24x", 7, minor, 0, 0, 16, 12, 4096, 1, 0, 0, 0, 0))
                    os.write(ready_w, b"x")
                    continue
                if opcode in (2, 42, 36):                             # FORGET, BATCH_FORGET, INTERRUPT: no answer
                    continue
                os.write(seen_w, b"r")
                if how == "stuck":
                    continue
                if delay and not waited:
                    time.sleep(delay)
                    waited = True
                if opcode == 1:                                       # LOOKUP
                    child = names.get((node, data[40:].split(b"\0", 1)[0]))
                    if child is None:
                        reply(error=ENOENT)
                    else:
                        mode, text = nodes[child]
                        reply(struct.pack("<QQQQII", child, 0, 0, 0, 0, 0) + _attr(child, mode, len(text)))
                elif opcode == 3:                                     # GETATTR
                    mode, text = nodes.get(node, nodes[1])
                    reply(struct.pack("<QII", 0, 0, 0) + _attr(node, mode, len(text)))
                elif opcode == 5:                                     # READLINK
                    reply(nodes.get(node, (0, b""))[1])
                elif opcode in (27, 14):                              # OPENDIR, OPEN
                    reply(struct.pack("<QII", 0, 0, 0))
                elif opcode in (29, 18, 25):                          # RELEASEDIR, RELEASE, FLUSH
                    reply()
                else:
                    reply(error=ENOSYS)
        finally:
            os._exit(4)
    os.close(ready_w)
    os.close(seen_w)
    ready, _, _ = select.select([ready_r], [], [], 10)
    assert ready and os.read(ready_r, 1) == b"x", "the test's FUSE daemon did not start"
    os.close(ready_r)
    minor = None
    for line in open("/proc/self/mountinfo"):
        fields = line.split()
        if fields[4] == str(target):
            minor = fields[2].split(":")[1]
    return pid, seen_r, minor


def _requests(seen):
    count = 0
    while select.select([seen], [], [], 0)[0]:
        data = os.read(seen, 4096)
        if not data:
            break
        count += len(data)
    return count


def _release(daemon, *points):
    """The daemon gone (its /dev/fuse closed: every request it held ends), its mounts lazily unmounted; again: nothing."""
    try:
        os.kill(daemon, signal.SIGKILL)
        os.waitpid(daemon, 0)
    except (ProcessLookupError, ChildProcessError):
        pass
    for point in points:
        subprocess.run(["umount", "-l", str(point)], check=False, capture_output=True)


def _open_space(home, base, version="new"):
    """A place with its encrypted space on, as space.rs makes it: ~/.hermes and its agents' spaces links into the
    view; the view a mount point (the caller mounts it)."""
    view = home / VIEW
    view.mkdir(parents=True)
    (home / CIPHER).mkdir(parents=True)
    (home / ".hermes").symlink_to(view / "hermes")
    return view, str(base / version).encode()


@pytest.fixture
def quick(monkeypatch):
    """The gc's waits for a place's open space, shortened: killed at 2 s, its connection aborted at 8 s."""
    monkeypatch.setattr(shared_hermes, "VIEW_KILL_S", 2.0, raising=False)
    monkeypatch.setattr(shared_hermes, "VIEW_ABORT_S", 8.0, raising=False)


# -- nothing of a place's is entered ---------------------------------------------------------------------------------

def test_a_link_out_of_the_home_is_never_followed_by_root(tmp_path):
    """~/.hermes a link out of the place's home (another place's, anywhere): never followed as root; the place points
    at nothing root reads (its own business: it can only break itself)."""
    base, (two, three), proc = _server(tmp_path, 3)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "shared").symlink_to(base / "old")
    (three / ".hermes").symlink_to(elsewhere)
    assert _gc(base, [two, three], proc) == ["old"]


@in_namespace
def test_fuse_a_place_s_link_into_its_own_fuse_never_holds_the_gc(tmp_path):
    """(The disk breakdown's fourth review.) ~/.hermes a link into a FUSE of the place's whose daemon reads root's
    request and never answers: never entered (no request), the place unknown, everything kept, at once."""
    base, (two, three), proc = _server(tmp_path, 3)
    mnt = three / "mnt"
    mnt.mkdir()
    (three / ".hermes").symlink_to(mnt)
    daemon, seen, _ = _space(mnt, how="stuck", fstype=b"fuse")
    try:
        code, said = _within(15, lambda: print(_gc(base, [two, three], proc)), lambda: _release(daemon))
        requests, left = _requests(seen), _held()
    finally:
        _release(daemon, mnt)
    assert code == 0 and requests == 0 and left == [], (code, requests, left, said)
    assert "All kept: 1 place folder(s)" in said and "(place 3)" in said


@in_namespace
def test_fuse_a_fuse_over_a_place_s_home_never_holds_the_gc(tmp_path):
    base, (two, three), proc = _server(tmp_path, 3)
    daemon, seen, _ = _space(three, how="stuck", fstype=b"fuse")
    try:
        code, said = _within(15, lambda: print(_gc(base, [two, three], proc)), lambda: _release(daemon))
        requests = _requests(seen)
    finally:
        _release(daemon, three)
    assert code == 0 and requests == 0 and "(place 3)" in said, (code, requests, said)


@in_namespace
@pytest.mark.parametrize("how", ["not gocryptfs", "another cipher folder", "tmpfs"])
def test_fuse_a_foreign_mount_at_the_view_is_never_entered(tmp_path, quick, how):
    """Only a gocryptfs FUSE of the place's uid, its source the place's cipher folder, mounted at its view, is its open
    space: anything else there is never entered, the place unknown, everything kept."""
    base, (two, three), proc = _server(tmp_path, 3)
    view, pointer = _open_space(three, base)
    if how == "tmpfs":
        subprocess.run(["mount", "-t", "tmpfs", "none", str(view)], check=True)
        (view / "hermes").mkdir()
        (view / "hermes" / "shared").symlink_to(base / "new")
        daemon, seen = None, None
    else:
        source = str(tmp_path / "cipher").encode() if how == "another cipher folder" else str(three / CIPHER).encode()
        fstype = b"fuse.sshfs" if how == "not gocryptfs" else b"fuse.gocryptfs"
        daemon, seen, _ = _space(view, fstype=fstype, source=source, pointer=pointer)
    try:
        code, said = _within(15, lambda: print(_gc(base, [two, three], proc)),
                             lambda: daemon and _release(daemon))
        requests = _requests(seen) if seen is not None else 0
    finally:
        if daemon:
            _release(daemon, view)
        else:
            subprocess.run(["umount", "-l", str(view)], check=False)
    assert code == 0 and requests == 0 and "(place 3)" in said, (code, requests, said)
    assert sorted(p.name for p in base.iterdir()) == ["new", "old"]


# -- a place's own open space: read from a disposable child --------------------------------------------------------

@in_namespace
def test_fuse_a_place_s_open_space_is_read_and_an_unused_version_goes(tmp_path, quick):
    base, (two, three), proc = _server(tmp_path, 3)
    view, pointer = _open_space(three, base)
    daemon, seen, _ = _space(view, source=str(three / CIPHER).encode(), pointer=pointer)
    try:
        code, said = _within(15, lambda: print(_gc(base, [two, three], proc)), lambda: _release(daemon))
        requests = _requests(seen)
    finally:
        _release(daemon, view)
    assert code == 0 and "['old']" in said and requests > 0, (code, requests, said)


@in_namespace
def test_fuse_a_place_s_space_that_never_answers_is_aborted_and_the_next_gc_reads_again(tmp_path, quick):
    """Its daemon reads root's request and never answers: the gc's child is killed at VIEW_KILL_S, stays (the request
    read), and only at VIEW_ABORT_S is that connection aborted (said: a broken space shows); everything kept that
    time. Nothing sticks: the next gc reads the place again."""
    base, (two, three), proc = _server(tmp_path, 3)
    view, pointer = _open_space(three, base)
    daemon, seen, _ = _space(view, how="stuck", source=str(three / CIPHER).encode())
    try:
        started = time.monotonic()
        code, said = _within(30, lambda: print(_gc(base, [two, three], proc)), lambda: _release(daemon))
        took = time.monotonic() - started
        time.sleep(1)
        left = _held()
    finally:
        _release(daemon, view)
    assert code == 0 and left == [], (code, left, said)
    assert 7 <= took < 20 and "Space stuck: place 3" in said and "(place 3)" in said, (took, said)
    assert sorted(p.name for p in base.iterdir()) == ["new", "old"]
    (view / "hermes").mkdir()
    (view / "hermes" / "shared").symlink_to(base / "new")            # its space as a plain folder again: read
    assert _gc(base, [two, three], proc) == ["old"]


@in_namespace
def test_fuse_a_slow_space_that_answers_before_the_abort_is_not_aborted(tmp_path, quick, monkeypatch):
    """Its daemon answers at 4 s (killed at 2 s, aborted at 8 s): the request answered, the child goes, the person's
    space is never aborted; the place unknown that time only."""
    base, (two, three), proc = _server(tmp_path, 3)
    view, pointer = _open_space(three, base)
    daemon, seen, minor = _space(view, source=str(three / CIPHER).encode(), delay=4.0, pointer=pointer)
    ctl = tmp_path / "fusectl"
    (ctl / minor).mkdir(parents=True)
    (ctl / minor / "abort").write_text("")
    monkeypatch.setattr(shared_hermes, "FUSECTL", ctl, raising=False)
    try:
        code, said = _within(30, lambda: print(_gc(base, [two, three], proc)), lambda: _release(daemon))
        time.sleep(1)
        left = _held()
    finally:
        _release(daemon, view)
    assert code == 0 and left == [] and (ctl / minor / "abort").read_text() == "", (code, left, said)
    assert "Space stuck" not in said and "(place 3)" in said, said


@in_namespace
@pytest.mark.parametrize("then", ["lazily unmounted", "stacked over"])
def test_fuse_a_stuck_space_unmounted_or_hidden_once_it_holds_the_child_is_aborted_all_the_same(tmp_path, quick, then):
    """Once its daemon holds the gc's child, the place lazily unmounts its view (gone from the mounts, its connection
    alive) or stacks another FUSE over it: the connection the child entered (its number taken before) is aborted."""
    base, (two, three), proc = _server(tmp_path, 3)
    view, pointer = _open_space(three, base)
    daemon, seen, _ = _space(view, how="stuck", source=str(three / CIPHER).encode())
    other = []

    def act():
        if then == "lazily unmounted":
            subprocess.run(["umount", "-l", str(view)], check=True)
        else:
            other.append(_space(view, how="stuck", fstype=b"fuse")[0])
    pid = os.fork()
    if pid == 0:
        try:
            _gc(base, [two, three], proc)
            os._exit(0)
        except BaseException:
            os._exit(2)
    try:
        end, acted, code = time.monotonic() + 30, False, None
        while time.monotonic() < end:
            if not acted and _held():
                act()
                acted = True
            done, status = os.waitpid(pid, os.WNOHANG)
            if done:
                code = os.waitstatus_to_exitcode(status)
                break
            time.sleep(0.1)
        else:
            os.kill(pid, signal.SIGKILL)
            _release(daemon)
            for d in other:
                _release(d)
            os.waitpid(pid, 0)
        time.sleep(1)
        left = _held()
    finally:
        for d in other:
            _release(d)
        _release(daemon, view, view)
    assert acted and code == 0 and left == [], (acted, code, left)


# -- the « root-kept last pointer » (not done here) -----------------------------------------------------------------

@pytest.mark.xfail(strict=True, reason="needs a source of truth root reads without entering the place: the version "
                                       "the seat agent's update() moved each place to (a record of root's); the "
                                       "running programs alone are wrong when a gateway is down after a move")
def test_gc_a_place_locked_for_good_keeps_only_what_it_may_run(tmp_path):
    """(The disk breakdown's fourth review.) A place locked for good keeps every version made since: it can only run
    the one it pointed at when root last read it. Place 2 seen on v1, then locked; two waves later v2 (made after,
    used by nobody) is still kept."""
    base = tmp_path / "opt"
    two, three = tmp_path / "home" / "bip2", tmp_path / "home" / "bip3"
    view = two / VIEW
    (view / "hermes").mkdir(parents=True)
    (two / ".hermes").symlink_to(view / "hermes")
    (three / ".hermes").mkdir(parents=True)
    proc = tmp_path / "proc"
    proc.mkdir()
    removed = []
    for wave, version in enumerate(("v1", "v2", "v3")):
        (base / version).mkdir(parents=True)
        if wave == 0:
            (view / "hermes" / "shared").symlink_to(base / "v1")
        elif wave == 1:
            (view / "hermes").rename(two / "space-cipher")
        target = three / ".hermes" / "shared"
        if target.is_symlink():
            target.unlink()
        target.symlink_to(base / version)
        removed += shared_hermes.gc(hermes.Runner(), base, [two, three], keep=[version], proc=proc)
    assert "v2" in removed, removed
