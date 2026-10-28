"""BipAgents' release key: the SHA256SUMS of a release are trusted only once their minisign signature checks out.

The release workflows (.github/workflows/bridge-release.yml, hermes-prebuilt.yml) and scripts/sign-release-sums.sh
sign each release's SHA256SUMS with minisign (Ed25519) and publish ``SHA256SUMS.minisig`` beside it. The installer
checks that signature against the public key pinned below, in the checkout it runs from, BEFORE it trusts any sum:
whoever can change a release's files (a leaked workflow token) can't swap an archive and its sum together any more.

Checked here in plain Python (RFC 8032), on purpose: the shared Hermes is fetched by root's system Python on a fresh
server, where neither the minisign program nor the ``cryptography`` package can be assumed. Only public data is
verified, so constant time doesn't matter.

While PUBLIC_KEY is the placeholder (no key generated yet), nothing is checked and a warning says so: today's
behaviour. Once the real key is pasted, a missing or bad signature means the download is refused."""
from __future__ import annotations

import base64
import hashlib
import sys
from typing import Callable, Optional

SIGNATURE = "SHA256SUMS.minisig"
PLACEHOLDER = "PLACEHOLDER"
# The second line of bipagents-release.pub (minisign -G), « RW… ». Until then: the placeholder, checks skipped.
PUBLIC_KEY = "PLACEHOLDER-no-release-key-yet-paste-the-second-line-of-bipagents-release.pub-here"


class ReleaseSignatureError(ValueError):
    """A release whose SHA256SUMS aren't signed by BipAgents' key: nothing of it is used."""


def pinned(key: Optional[str] = None) -> bool:
    """A real key is pinned in this checkout (signatures are then mandatory)."""
    return not (PUBLIC_KEY if key is None else key).strip().startswith(PLACEHOLDER)


def _warn(text: str) -> None:
    print(text, file=sys.stderr)


def check(sums: bytes, signature: Optional[bytes], *, key: Optional[str] = None,
          warn: Callable[[str], None] = _warn) -> None:
    """Raises ReleaseSignatureError unless `signature` (a SHA256SUMS.minisig, None when the release has none) is the
    pinned key's signature of `sums`. With the placeholder key: a warning only."""
    key = PUBLIC_KEY if key is None else key
    if not pinned(key):
        warn("Warning: no release key pinned in this checkout yet (installer/bipinstall/release_key.py): "
             "the release's SHA256SUMS are used without a signature check.")
        return
    if not signature:
        raise ReleaseSignatureError(f"the release has no {SIGNATURE}: not installed")
    try:
        ok = minisign_verify(key, sums, signature)
    except ValueError as exc:
        raise ReleaseSignatureError(f"the release's {SIGNATURE} can't be read ({exc}): not installed") from exc
    if not ok:
        raise ReleaseSignatureError("the release's SHA256SUMS aren't signed by BipAgents' release key: not installed")


# --- minisign ------------------------------------------------------------------------------------------------------

def _b64(text: str, size: int, what: str) -> bytes:
    try:
        raw = base64.b64decode(text.strip(), validate=True)
    except (ValueError, TypeError) as exc:
        raise ValueError(f"{what}: not base64") from exc
    if len(raw) != size:
        raise ValueError(f"{what}: {len(raw)} bytes, not {size}")
    return raw


def minisign_verify(public_key: str, message: bytes, minisig: bytes) -> bool:
    """`minisig` (a .minisig file) is `public_key`'s (the base64 line of a minisign .pub) signature of `message`,
    trusted comment included. Legacy (« Ed ») and prehashed (« ED », minisign's default) signatures both."""
    key = _b64(public_key.strip().splitlines()[-1], 42, "public key")
    if key[:2] != b"Ed":
        raise ValueError("public key: not an Ed25519 minisign key")
    key_id, pk = key[2:10], key[10:]
    lines = minisig.decode("utf-8", "strict").splitlines()
    if len(lines) < 4 or not lines[0].startswith("untrusted comment:") or not lines[2].startswith("trusted comment: "):
        raise ValueError("not a minisign signature")
    sig = _b64(lines[1], 74, "signature")
    algorithm, sig_id, sig = sig[:2], sig[2:10], sig[10:]
    if sig_id != key_id:
        return False   # signed by another key
    if algorithm == b"ED":
        signed = hashlib.blake2b(message, digest_size=64).digest()
    elif algorithm == b"Ed":
        signed = message
    else:
        raise ValueError("signature: unknown algorithm")
    if not ed25519_verify(pk, signed, sig):
        return False
    trusted = lines[2][len("trusted comment: "):].encode("utf-8")
    return ed25519_verify(pk, sig + trusted, _b64(lines[3], 64, "global signature"))


# --- Ed25519 verification (RFC 8032, section 6) --------------------------------------------------------------------

_P = 2 ** 255 - 19
_L = 2 ** 252 + 27742317777372353535851937790883648493
_D = -121665 * pow(121666, _P - 2, _P) % _P
_SQRT_M1 = pow(2, (_P - 1) // 4, _P)


def _add(a, b):
    x1, y1, z1, t1 = a
    x2, y2, z2, t2 = b
    aa, bb = (y1 - x1) * (y2 - x2) % _P, (y1 + x1) * (y2 + x2) % _P
    cc, dd = 2 * t1 * t2 * _D % _P, 2 * z1 * z2 % _P
    e, f, g, h = bb - aa, dd - cc, dd + cc, bb + aa
    return e * f % _P, g * h % _P, f * g % _P, e * h % _P


def _mul(s: int, point):
    result = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            result = _add(result, point)
        point = _add(point, point)
        s >>= 1
    return result


def _x(y: int, sign: int) -> Optional[int]:
    if y >= _P:
        return None
    x2 = (y * y - 1) * pow(_D * y * y + 1, _P - 2, _P) % _P
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (_P + 3) // 8, _P)
    if (x * x - x2) % _P:
        x = x * _SQRT_M1 % _P
    if (x * x - x2) % _P:
        return None
    if (x & 1) != sign:
        x = _P - x
    return x


def _decode(raw: bytes):
    if len(raw) != 32:
        return None
    y = int.from_bytes(raw, "little")
    sign, y = y >> 255, y & ((1 << 255) - 1)
    x = _x(y, sign)
    return None if x is None else (x, y, 1, x * y % _P)


_GY = 4 * pow(5, _P - 2, _P) % _P
_G = (_x(_GY, 0), _GY, 1, _x(_GY, 0) * _GY % _P)


def _equal(a, b) -> bool:
    return (a[0] * b[2] - b[0] * a[2]) % _P == 0 and (a[1] * b[2] - b[1] * a[2]) % _P == 0


def ed25519_verify(public: bytes, message: bytes, signature: bytes) -> bool:
    if len(public) != 32 or len(signature) != 64:
        return False
    a, r = _decode(public), _decode(signature[:32])
    if a is None or r is None:
        return False
    s = int.from_bytes(signature[32:], "little")
    if s >= _L:
        return False
    h = int.from_bytes(hashlib.sha512(signature[:32] + public + message).digest(), "little") % _L
    return _equal(_mul(s, _G), _add(r, _mul(h, a)))
