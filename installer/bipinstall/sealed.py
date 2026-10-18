"""Encrypted files for `bipagents export` / `import` (moving.py): a passphrase, scrypt, then AES-256-GCM in 1 MiB
chunks. Each chunk is authenticated and the last one is marked, so a file cut short, reordered or altered is refused,
as is a wrong passphrase, before anything is written. Streamed both ways: an install of several gigabytes never sits
in memory.

    MAGIC · scrypt log2(N) · salt (16) · nonce prefix (7) · then per chunk: length (4, big-endian) · ciphertext
    nonce = prefix · chunk counter (4, big-endian) · 1 for the last chunk else 0; the header is the associated data.
"""
from __future__ import annotations

import io
import os
import secrets
import struct
from typing import BinaryIO, Optional

MAGIC = b"BIPAGENTS-SEALED\x01"
CHUNK = 1 << 20
SCRYPT_LOG_N = 15        # 32 MiB of memory: fine on a 1 GB server; the passphrase itself carries the strength
ALPHABET = "abcdefghjkmnpqrstuvwxyz23456789"   # no 0/o, 1/l/i: read aloud or copied by hand without doubt


class SealedError(Exception):
    """Safe to show."""


def new_passphrase(groups: int = 5, size: int = 5) -> str:
    """About 124 bits, in groups easy to copy: « k7qm2-rt9vw-dp4hn-3cjzx-a8bfe »."""
    return "-".join("".join(secrets.choice(ALPHABET) for _ in range(size)) for _ in range(groups))


def normalize(passphrase: str) -> bytes:
    return "".join(c for c in passphrase.lower() if c.isalnum()).encode()


def _key(passphrase: str, salt: bytes, log_n: int) -> bytes:
    from cryptography.hazmat.primitives.kdf.scrypt import Scrypt
    return Scrypt(salt=salt, length=32, n=1 << log_n, r=8, p=1).derive(normalize(passphrase))


def _nonce(prefix: bytes, counter: int, last: bool) -> bytes:
    return prefix + struct.pack(">I", counter) + (b"\x01" if last else b"\x00")


class SealedWriter(io.RawIOBase):
    """A write-only stream: what is written goes out encrypted to `out`; `close()` writes the last chunk."""

    def __init__(self, out: BinaryIO, passphrase: str, log_n: int = SCRYPT_LOG_N) -> None:
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
        salt, self.prefix = os.urandom(16), os.urandom(7)
        self.header = MAGIC + bytes([log_n]) + salt + self.prefix
        self.aead = AESGCM(_key(passphrase, salt, log_n))
        self.out = out
        self.buffer = bytearray()
        self.counter = 0
        out.write(self.header)

    def writable(self) -> bool:
        return True

    def write(self, data) -> int:   # type: ignore[override]
        self.buffer += data
        while len(self.buffer) > CHUNK:   # a full chunk is only sealed once more follows: the last one stays here
            self._seal(bytes(self.buffer[:CHUNK]), last=False)
            del self.buffer[:CHUNK]
        return len(data)

    def _seal(self, plain: bytes, last: bool) -> None:
        if self.counter >= 0xFFFFFFFF:
            raise SealedError("file too large")
        sealed = self.aead.encrypt(_nonce(self.prefix, self.counter, last), plain, self.header)
        self.out.write(struct.pack(">I", len(sealed)) + sealed)
        self.counter += 1

    def close(self) -> None:
        if not self.closed:
            self._seal(bytes(self.buffer), last=True)
            self.buffer.clear()
            self.out.flush()
        super().close()


class SealedReader(io.RawIOBase):
    """A read-only stream of what a SealedWriter wrote. Raises SealedError on a wrong passphrase or a damaged file."""

    def __init__(self, source: BinaryIO, passphrase: str) -> None:
        from cryptography.hazmat.primitives.ciphers.aead import AESGCM
        head = source.read(len(MAGIC) + 1 + 16 + 7)
        if len(head) < len(MAGIC) + 24 or not head.startswith(MAGIC):
            raise SealedError("this is not a BipAgents export")
        log_n = head[len(MAGIC)]
        salt, self.prefix = head[len(MAGIC) + 1:len(MAGIC) + 17], head[len(MAGIC) + 17:]
        if not 10 <= log_n <= 22:
            raise SealedError("this is not a BipAgents export")
        self.header = head
        self.aead = AESGCM(_key(passphrase, salt, log_n))
        self.source = source
        self.counter = 0
        self.pending: Optional[bytes] = self._record()
        if self.pending is None:
            raise SealedError("the file is cut short")
        self.plain = bytearray()
        self.done = False

    def _record(self) -> Optional[bytes]:
        size = self.source.read(4)
        if not size:
            return None
        if len(size) < 4:
            raise SealedError("the file is cut short")
        (length,) = struct.unpack(">I", size)
        if length > CHUNK + 16:
            raise SealedError("the file is damaged")
        data = self.source.read(length)
        if len(data) < length:
            raise SealedError("the file is cut short")
        return data

    def _open_next(self) -> None:
        from cryptography.exceptions import InvalidTag
        assert self.pending is not None
        following = self._record()
        last = following is None
        try:
            self.plain += self.aead.decrypt(_nonce(self.prefix, self.counter, last), self.pending, self.header)
        except InvalidTag:
            if self.counter == 0:
                raise SealedError("wrong passphrase (or a damaged file)") from None
            raise SealedError("the file is damaged or cut short") from None
        self.counter += 1
        self.pending = following
        self.done = last

    def readable(self) -> bool:
        return True

    def readinto(self, target) -> int:   # type: ignore[override]
        while not self.plain and not self.done:
            self._open_next()
        size = min(len(target), len(self.plain))
        target[:size] = self.plain[:size]
        del self.plain[:size]
        return size
