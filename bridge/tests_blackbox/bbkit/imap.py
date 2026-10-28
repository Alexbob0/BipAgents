"""A small IMAP4rev1 server for the mailbox tests: plain connection then STARTTLS (as Proton Mail Bridge on this machine,
whose certificate the bridge does not check) — or plain only with `starttls = False`, as hydroxide without TLS
options —, LOGIN, LIST, SELECT/EXAMINE, UID SEARCH (ALL, SINCE, BEFORE, FROM, TEXT,
CHARSET), UID FETCH (RFC822.SIZE, BODY.PEEK[], BODY.PEEK[HEADER.FIELDS (…)], BODY.PEEK[TEXT], partials), APPEND, LOGOUT. It records
every command it gets."""
from __future__ import annotations

import datetime
import email
import email.policy
import re
import socket
import ssl
import tempfile
import threading
from email.utils import parsedate_to_datetime
from typing import Dict, List, Optional, Tuple

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.x509.oid import NameOID


def _self_signed() -> ssl.SSLContext:
    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "fake-imap")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key())
            .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(days=1))
            .not_valid_after(now + datetime.timedelta(days=30)).sign(key, hashes.SHA256()))
    folder = tempfile.mkdtemp(prefix="bb-imap-")
    cert_path, key_path = f"{folder}/cert.pem", f"{folder}/key.pem"
    with open(cert_path, "wb") as fh:
        fh.write(cert.public_bytes(serialization.Encoding.PEM))
    with open(key_path, "wb") as fh:
        fh.write(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                   serialization.NoEncryption()))
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert_path, key_path)
    return context


def _tokens(line: str) -> List[str]:
    """IMAP arguments: atoms, "quoted strings" (unquoted here), (parenthesised lists) kept whole."""
    out, i = [], 0
    while i < len(line):
        c = line[i]
        if c == " ":
            i += 1
        elif c == '"':
            j, value = i + 1, ""
            while j < len(line) and line[j] != '"':
                if line[j] == "\\" and j + 1 < len(line):
                    j += 1
                value += line[j]
                j += 1
            out.append(value)
            i = j + 1
        elif c == "(":
            depth, j = 0, i
            while j < len(line):
                depth += line[j] == "("
                depth -= line[j] == ")"
                j += 1
                if depth == 0:
                    break
            out.append(line[i:j])
            i = j
        else:
            j = line.find(" ", i)
            j = len(line) if j < 0 else j
            out.append(line[i:j])
            i = j
    return out


class FakeImap:
    FOLDERS = [("\\HasNoChildren", "INBOX"), ("\\HasNoChildren \\Drafts", "Brouillons"), ("\\HasNoChildren \\Sent", "Sent")]

    def __init__(self) -> None:
        self.context = _self_signed()
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.listen(16)
        self.port = self.sock.getsockname()[1]
        self.reset()
        threading.Thread(target=self._accept, name="fake-imap", daemon=True).start()

    def reset(self) -> None:
        self.users: Dict[str, str] = {}
        self.folders: Dict[str, List[Tuple[int, bytes]]] = {"INBOX": [], "Brouillons": [], "Sent": []}
        self.commands: List[str] = []
        self.appended: List[Tuple[str, str, bytes]] = []
        self.logins: List[Tuple[str, bool]] = []
        self.tls_seen = 0
        self.starttls = True

    def add(self, folder: str, uid: int, raw: bytes) -> None:
        self.folders.setdefault(folder, []).append((uid, raw))

    def _accept(self) -> None:
        while True:
            try:
                conn, _ = self.sock.accept()
            except OSError:
                return
            threading.Thread(target=self._session, args=(conn,), daemon=True).start()

    # -- one connection ---------------------------------------------------------------------------------------------
    def _session(self, conn: socket.socket) -> None:
        stream = conn.makefile("rwb")
        state = {"user": None, "folder": None, "conn": conn, "stream": stream}
        try:
            self._send(state, b"* OK [CAPABILITY IMAP4rev1" + (b" STARTTLS" if self.starttls else b"")
                       + b"] fake IMAP ready\r\n")
            while True:
                line = state["stream"].readline()
                if not line:
                    return
                text = line.decode("utf-8", "replace").rstrip("\r\n")
                literal = re.search(r"\{(\d+)\}$", text)
                payload = b""
                if literal:
                    self._send(state, b"+ Ready for literal data\r\n")
                    payload = state["stream"].read(int(literal.group(1)))
                    state["stream"].readline()
                tag, _, rest = text.partition(" ")
                command, _, args = rest.partition(" ")
                self.commands.append(f"{command.upper()} {args}".strip() if command.upper() != "LOGIN" else "LOGIN")
                if not self._dispatch(state, tag, command.upper(), args, payload):
                    return
        except (OSError, ssl.SSLError, ValueError):
            return
        finally:
            try:
                conn.close()
            except OSError:
                pass

    def _send(self, state, data: bytes) -> None:
        state["stream"].write(data)
        state["stream"].flush()

    def _ok(self, state, tag: str, text: str = "done") -> None:
        self._send(state, f"{tag} OK {text}\r\n".encode())

    def _dispatch(self, state, tag: str, command: str, args: str, payload: bytes) -> bool:
        if command == "CAPABILITY":
            caps = "IMAP4rev1 AUTH=PLAIN" + ("" if state.get("tls") or not self.starttls else " STARTTLS")
            self._send(state, f"* CAPABILITY {caps}\r\n".encode())
            self._ok(state, tag)
        elif command == "STARTTLS" and self.starttls:
            self._ok(state, tag, "begin TLS now")
            state["stream"].flush()
            tls = self.context.wrap_socket(state["conn"], server_side=True)
            state.update(conn=tls, stream=tls.makefile("rwb"), tls=True)
            self.tls_seen += 1
        elif command == "LOGIN":
            user, password = (_tokens(args) + ["", ""])[:2]
            good = self.users.get(user) == password
            self.logins.append((user, good))
            if good:
                state["user"] = user
                self._ok(state, tag, "LOGIN completed")
            else:
                self._send(state, f"{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n".encode())
        elif command == "LOGOUT":
            self._send(state, b"* BYE logging out\r\n")
            self._ok(state, tag)
            return False
        elif command == "NOOP":
            self._ok(state, tag)
        elif state["user"] is None:
            self._send(state, f"{tag} NO not logged in\r\n".encode())
        elif command == "LIST":
            for flags, name in self.FOLDERS:
                self._send(state, f'* LIST ({flags}) "/" "{name}"\r\n'.encode())
            self._ok(state, tag)
        elif command in ("SELECT", "EXAMINE"):
            name = (_tokens(args) or [""])[0]
            if name not in self.folders:
                self._send(state, f"{tag} NO no such folder\r\n".encode())
            else:
                state["folder"] = name
                self._send(state, f"* {len(self.folders[name])} EXISTS\r\n* OK [UIDVALIDITY 1] ok\r\n".encode())
                self._ok(state, tag, f"[READ-{'ONLY' if command == 'EXAMINE' else 'WRITE'}] {command} completed")
        elif command == "UID":
            sub, _, rest = args.partition(" ")
            if sub.upper() == "SEARCH":
                uids = self._search(state["folder"], _tokens(rest))
                self._send(state, ("* SEARCH" + "".join(f" {u}" for u in uids) + "\r\n").encode())
                self._ok(state, tag)
            elif sub.upper() == "FETCH":
                uidset, _, items = rest.partition(" ")
                self._fetch(state, uidset, items)
                self._ok(state, tag)
            else:
                self._send(state, f"{tag} BAD unknown UID command\r\n".encode())
        elif command == "APPEND":
            parts = _tokens(args)
            folder = parts[0]
            flags = next((p for p in parts[1:] if p.startswith("(")), "")
            self.appended.append((folder, flags, payload))
            self._ok(state, tag, "APPEND completed")
        else:
            self._send(state, f"{tag} BAD unknown command\r\n".encode())
        return True

    # -- searching and fetching -------------------------------------------------------------------------------------
    @staticmethod
    def _header(raw: bytes) -> email.message.Message:
        return email.message_from_bytes(raw.partition(b"\r\n\r\n")[0] + b"\r\n\r\n", policy=email.policy.default)

    def _search(self, folder: Optional[str], words: List[str]) -> List[int]:
        mails = self.folders.get(folder or "", [])
        keep = []
        for uid, raw in mails:
            header, ok, i = self._header(raw), True, 0
            while i < len(words):
                word = words[i].upper()
                value = words[i + 1] if i + 1 < len(words) else ""
                if word == "CHARSET":
                    i += 2
                    continue
                if word in ("SINCE", "BEFORE"):
                    day = datetime.datetime.strptime(value, "%d-%b-%Y").date()
                    sent = parsedate_to_datetime(header.get("Date")).date()
                    ok &= sent >= day if word == "SINCE" else sent < day
                    i += 2
                elif word == "FROM":
                    ok &= value.lower() in str(header.get("From") or "").lower()
                    i += 2
                elif word == "TEXT":
                    ok &= value.lower() in raw.decode("utf-8", "replace").lower()
                    i += 2
                else:
                    i += 1
            if ok:
                keep.append(uid)
        return sorted(keep)

    def _fetch(self, state, uidset: str, items: str) -> None:
        wanted = set()
        for piece in uidset.split(","):
            if ":" in piece:
                low, high = piece.split(":")
                wanted.update(range(int(low), 10**9 if high == "*" else int(high) + 1))
            else:
                wanted.add(int(piece))
        mails = self.folders.get(state["folder"] or "", [])
        for seq, (uid, raw) in enumerate(mails, start=1):
            if uid not in wanted:
                continue
            chunks: List[Tuple[str, bytes]] = []
            for match in re.finditer(r"BODY(?:\.PEEK)?\[([^\]]*)\](?:<(\d+)\.(\d+)>)?", items, re.I):
                section, start, size = match.group(1), match.group(2), match.group(3)
                head, _, body = raw.partition(b"\r\n\r\n")
                if section.upper().startswith("HEADER.FIELDS"):
                    names = {n.upper() for n in re.findall(r"[\w-]+", section.split("(", 1)[1])}
                    lines, keep_line = [], False
                    for line in head.split(b"\r\n"):
                        if line[:1] in (b" ", b"\t"):
                            if keep_line:
                                lines.append(line)
                            continue
                        keep_line = line.split(b":", 1)[0].decode().upper() in names
                        if keep_line:
                            lines.append(line)
                    data = b"\r\n".join(lines) + b"\r\n\r\n"
                elif section.upper() == "TEXT":
                    data = body
                else:
                    data = raw
                label = f"BODY[{section}]"
                if start is not None:
                    data = data[int(start):int(start) + int(size)]
                    label += f"<{start}>"
                chunks.append((label, data))
            out = f"* {seq} FETCH (UID {uid}".encode()
            if "RFC822.SIZE" in items.upper():
                out += f" RFC822.SIZE {len(raw)}".encode()
            for label, data in chunks:
                out += f" {label} {{{len(data)}}}\r\n".encode() + data
            self._send(state, out + b")\r\n")

    def stop(self) -> None:
        self.sock.close()
