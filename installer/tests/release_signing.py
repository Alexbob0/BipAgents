"""Throwaway minisign keys for the tests' fake releases: since BipAgents' own key is pinned
(installer/bipinstall/release_key.py), a release is used only once its SHA256SUMS are signed. The `signer` fixture
(conftest.py) pins one of these in its place."""
import base64
import hashlib
import os
from pathlib import Path


class Key:
    """A throwaway minisign key pair: its public line (as in a .pub) and a signer writing .minisig files."""

    def __init__(self):
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
        self.secret = Ed25519PrivateKey.generate()
        self.id = os.urandom(8)
        raw = self.secret.public_key().public_bytes_raw()
        self.public = base64.b64encode(b"Ed" + self.id + raw).decode()

    def sign(self, data: bytes, prehashed=True, comment="timestamp:1761600000\tfile:SHA256SUMS\thashed") -> bytes:
        message = hashlib.blake2b(data, digest_size=64).digest() if prehashed else data
        sig = self.secret.sign(message)
        whole = self.secret.sign(sig + comment.encode())
        line = base64.b64encode((b"ED" if prehashed else b"Ed") + self.id + sig).decode()
        return (f"untrusted comment: signature from minisign secret key\n{line}\n"
                f"trusted comment: {comment}\n{base64.b64encode(whole).decode()}\n").encode()

    def sign_release(self, folder: Path) -> Path:
        """<folder>/SHA256SUMS.minisig for <folder>/SHA256SUMS as it is now, as the release workflows publish it."""
        out = folder / "SHA256SUMS.minisig"
        out.write_bytes(self.sign((folder / "SHA256SUMS").read_bytes()))
        return out
