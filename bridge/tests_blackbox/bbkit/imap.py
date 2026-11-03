"""A small IMAP4rev1 server for the mailbox tests: plain connection then STARTTLS (as Proton Mail Bridge on this machine,
whose certificate the bridge does not check) — or plain only with `starttls = False`, as hydroxide without TLS
options —, LOGIN, LIST, SELECT/EXAMINE, UID SEARCH (ALL, SINCE, BEFORE, FROM, TEXT,
CHARSET, UID n:*), UID FETCH (RFC822.SIZE, BODY.PEEK[], BODY.PEEK[HEADER.FIELDS (…)], BODY.PEEK[TEXT], partials, `*`),
APPEND, IDLE (new mail told as `* n EXISTS` while idling; off with `idle = False`), NOOP, LOGOUT; for the mail rules
(mail_rules.rs): FLAGS (`flags`), UID STORE, UID MOVE, UID COPY, CREATE (listed after), SEARCH HEADER Message-ID. It
records every command it gets."""
from __future__ import annotations

import datetime
import email
import email.policy
import re
import select
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
        self.idle = True
        self.idling = 0        # connections in IDLE now
        self.idles = 0         # IDLE commands seen
        self.flags: Dict[Tuple[str, int], set] = {}   # (folder, uid) → flags
        self.created: List[str] = []
        self.next_uid = 10000  # a moved or copied message's uid in its new folder

    def add(self, folder: str, uid: int, raw: bytes, flags=()) -> None:
        self.folders.setdefault(folder, []).append((uid, raw))
        self.flags[(folder, uid)] = set(flags)

    def message_ids(self, folder: str) -> List[str]:
        return [str(self._header(raw).get("Message-ID") or "") for _, raw in self.folders.get(folder, [])]

    def flags_of(self, folder: str, message_id: str) -> set:
        for uid, raw in self.folders.get(folder, []):
            if str(self._header(raw).get("Message-ID") or "") == message_id:
                return self.flags.get((folder, uid), set())
        return set()

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
            caps += " IDLE" if self.idle else ""
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
        elif command == "IDLE" and self.idle:
            folder = state["folder"] or "INBOX"
            start = len(self.folders.get(folder, []))
            self._send(state, b"+ idling\r\n")
            self.idles += 1
            self.idling += 1
            told = False
            conn = state["conn"]
            try:
                while True:
                    count = len(self.folders.get(folder, []))
                    if not told and count > start:
                        self._send(state, f"* {count} EXISTS\r\n".encode())
                        told = True
                    # the client says nothing until DONE: nothing waits in the file's buffer meanwhile
                    readable, _, _ = select.select([conn], [], [], 0.05)
                    if readable or (hasattr(conn, "pending") and conn.pending()):
                        if not state["stream"].readline():
                            return False
                        break
            finally:
                self.idling -= 1
            self._ok(state, tag, "IDLE terminated")
        elif command == "LIST":
            for flags, name in self.FOLDERS + [("\\HasNoChildren", n) for n in self.created]:
                self._send(state, f'* LIST ({flags}) "/" "{name}"\r\n'.encode())
            self._ok(state, tag)
        elif command == "CREATE":
            name = (_tokens(args) or [""])[0]
            if name in self.folders:
                self._send(state, f"{tag} NO already there\r\n".encode())
            else:
                self.folders[name] = []
                self.created.append(name)
                self._ok(state, tag, "CREATE completed")
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
            elif sub.upper() == "STORE":
                uidset, how, flags = (_tokens(rest) + ["", "", ""])[:3]
                folder = state["folder"] or ""
                wanted = set(flags.strip("()").split())
                for uid in self._uids(folder, uidset):
                    have = self.flags.setdefault((folder, uid), set())
                    if how.startswith("+"):
                        have |= wanted
                    elif how.startswith("-"):
                        have -= wanted
                self._ok(state, tag)
            elif sub.upper() in ("MOVE", "COPY"):
                uidset, dest = (_tokens(rest) + ["", ""])[:2]
                folder = state["folder"] or ""
                if dest not in self.folders:
                    self._send(state, f"{tag} NO [TRYCREATE] no such folder\r\n".encode())
                    return True
                chosen = [(u, r) for u, r in self.folders.get(folder, []) if u in set(self._uids(folder, uidset))]
                for uid, raw in chosen:
                    self.next_uid += 1
                    self.folders[dest].append((self.next_uid, raw))
                    self.flags[(dest, self.next_uid)] = set(self.flags.get((folder, uid), set()))
                if sub.upper() == "MOVE":
                    gone = {u for u, _ in chosen}
                    self.folders[folder] = [(u, r) for u, r in self.folders[folder] if u not in gone]
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

    def _uids(self, folder: str, uidset: str) -> List[int]:
        mails = self.folders.get(folder, [])
        top = max((u for u, _ in mails), default=0)
        wanted = set()
        for piece in uidset.split(","):
            if piece == "*":
                wanted.add(top)
            elif ":" in piece:
                low, high = piece.split(":")
                wanted.update(range(int(low), (top if high == "*" else int(high)) + 1))
            elif piece:
                wanted.add(int(piece))
        return [u for u, _ in mails if u in wanted]

    def _search(self, folder: Optional[str], words: List[str]) -> List[int]:
        mails = self.folders.get(folder or "", [])
        # HEADER Message-ID <id> (any of several, ORed): the undo's way to find messages again
        ids = [words[i + 2] for i in range(len(words) - 2) if words[i].upper() == "HEADER"
               and words[i + 1].lower() == "message-id"]
        if ids:
            return sorted(u for u, raw in mails if str(self._header(raw).get("Message-ID") or "") in ids)
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
                elif word == "UID":
                    low, _, high = value.partition(":")
                    top = max((u for u, _ in mails), default=0)
                    high_n = top if high in ("*", "") else int(high)
                    # `n:*` always includes the highest UID, even below n (RFC 3501)
                    ok &= (int(low) <= uid <= high_n) or (high == "*" and uid == top)
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
        mails_all = self.folders.get(state["folder"] or "", [])
        top = max((u for u, _ in mails_all), default=0)
        for piece in uidset.split(","):
            if piece == "*":
                wanted.add(top)
            elif ":" in piece:
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
            if re.search(r"\bFLAGS\b", items.upper()):
                out += f" FLAGS ({' '.join(sorted(self.flags.get((state['folder'], uid), set())))})".encode()
            if "RFC822.SIZE" in items.upper():
                out += f" RFC822.SIZE {len(raw)}".encode()
            for label, data in chunks:
                out += f" {label} {{{len(data)}}}\r\n".encode() + data
            self._send(state, out + b")\r\n")

    def stop(self) -> None:
        self.sock.close()
