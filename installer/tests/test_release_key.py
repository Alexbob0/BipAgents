"""Release integrity (audit 2026-10-27, finding 22): a release's SHA256SUMS are trusted only once signed with minisign
by BipAgents' release key, pinned in the checkout. The keys here are throwaway ones made by the test itself."""
import base64
import hashlib
import os
import shutil

import httpx
import pytest

from bipinstall import hermes, release_key, rustbridge, shared_hermes, voices
from test_rustbridge import archive

ed25519 = pytest.importorskip("cryptography.hazmat.primitives.asymmetric.ed25519")


class Key:
    """A throwaway minisign key pair: its public line (as in a .pub) and a signer writing .minisig files."""

    def __init__(self):
        self.secret = ed25519.Ed25519PrivateKey.generate()
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


@pytest.fixture
def key(monkeypatch):
    made = Key()
    monkeypatch.setattr(release_key, "PUBLIC_KEY", made.public)
    return made


def test_rfc8032_vector():
    public = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    sig = bytes.fromhex("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46b"
                        "d25bf5f0595bbe24655141438e7a100b")
    assert release_key.ed25519_verify(public, b"", sig)
    assert not release_key.ed25519_verify(public, b"x", sig)


def test_a_signature_is_checked_against_the_pinned_key(key):
    sums = b"ab" * 32 + b"  bipbridge-linux-x86_64.tar.gz\n"
    release_key.check(sums, key.sign(sums))                      # minisign's default (prehashed)
    release_key.check(sums, key.sign(sums, prehashed=False))     # legacy
    with pytest.raises(release_key.ReleaseSignatureError, match="aren't signed"):
        release_key.check(sums.replace(b"ab", b"cd", 1), key.sign(sums))          # tampered sums
    with pytest.raises(release_key.ReleaseSignatureError, match="aren't signed"):
        release_key.check(sums, Key().sign(sums))                                  # another key
    forged = key.sign(sums).replace(b"hashed", b"hashez")
    with pytest.raises(release_key.ReleaseSignatureError, match="aren't signed"):
        release_key.check(sums, forged)                                            # trusted comment changed
    with pytest.raises(release_key.ReleaseSignatureError, match="no SHA256SUMS.minisig"):
        release_key.check(sums, None)
    with pytest.raises(release_key.ReleaseSignatureError, match="can't be read"):
        release_key.check(sums, b"<html>Not Found</html>")


def test_the_placeholder_key_keeps_today_s_behaviour_with_a_warning():
    assert not release_key.pinned()   # until Alex pastes the real key
    said = []
    release_key.check(b"anything", None, warn=said.append)
    assert "no release key pinned" in said[0]


# -- the bridge's binaries ------------------------------------------------------------------------------------------

def bridge_release(key, signed=True, sign_what=None):
    name = "bipbridge-linux-x86_64.tar.gz"
    good = archive({"bipbridge": b"\x7fELF bridge"})
    sums = f"{hashlib.sha256(good).hexdigest()}  {name}\n".encode()
    files = {"SHA256SUMS": sums, name: good}
    if signed:
        files["SHA256SUMS.minisig"] = key.sign(sign_what if sign_what is not None else sums)

    def opener(url):
        asset = url.rsplit("/", 1)[1]
        if asset not in files:
            raise OSError("HTTP Error 404: Not Found")
        return files[asset]
    return name, files, opener


def test_the_bridge_is_installed_only_from_signed_sums(tmp_path, key):
    name, _, opener = bridge_release(key)
    assert rustbridge.fetch(tmp_path, "000000000001", name, opener).read_bytes() == b"\x7fELF bridge"
    # Someone holding the workflow's token swaps the archive and its sum together: the old signature no longer fits.
    name, files, opener = bridge_release(key)
    evil = archive({"bipbridge": b"evil"})
    signature = files["SHA256SUMS.minisig"]
    files[name] = evil
    files["SHA256SUMS"] = f"{hashlib.sha256(evil).hexdigest()}  {name}\n".encode()
    files["SHA256SUMS.minisig"] = signature
    with pytest.raises(rustbridge.RustBridgeError, match="aren't signed"):
        rustbridge.fetch(tmp_path, "000000000002", name, opener)
    name, _, opener = bridge_release(key, signed=False)
    with pytest.raises(rustbridge.RustBridgeError, match="no SHA256SUMS.minisig"):
        rustbridge.fetch(tmp_path, "000000000003", name, opener)
    assert not rustbridge.binary(tmp_path, "000000000002").exists()
    assert not rustbridge.binary(tmp_path, "000000000003").exists()


def test_without_a_pinned_key_the_bridge_is_still_installed_from_its_sums(tmp_path):
    name, _, opener = bridge_release(None, signed=False)
    assert rustbridge.fetch(tmp_path, "000000000001", name, opener).exists()


# -- the shared Hermes ----------------------------------------------------------------------------------------------

@pytest.mark.skipif(not shutil.which("zstd"), reason="zstd packs and unpacks the release archive")
@pytest.mark.parametrize("signature", ["good", "missing", "other key"])
def test_the_shared_hermes_is_used_only_from_signed_sums(tmp_path, monkeypatch, key, signature):
    from test_shared_hermes import OnlyRemoves, released
    archive, fetch, _ = released(tmp_path)
    sums = (tmp_path / "release" / "SHA256SUMS").read_bytes()
    if signature != "missing":
        (tmp_path / "release" / "SHA256SUMS.minisig").write_bytes((key if signature == "good" else Key()).sign(sums))
    base = tmp_path / "opt"
    monkeypatch.setattr(shared_hermes, "DEFAULT_ROOT", base)
    monkeypatch.setattr(shared_hermes.platform, "machine", lambda: "x86_64")
    said = []
    monkeypatch.setattr(shared_hermes, "say", said.append)
    used = shared_hermes.prebuilt(OnlyRemoves(), base, "v1", fetch=fetch)
    assert used == (signature == "good")
    if signature != "good":
        assert "building it here" in said[-1] and list(base.iterdir()) == []


# -- the Bips' voices -----------------------------------------------------------------------------------------------

def test_the_voices_are_used_only_from_signed_sums(tmp_path, monkeypatch, key):
    import test_voices_release as tvr
    tvr.build(tmp_path / "voices")
    name = tvr.release.pack(tmp_path / "voices", tmp_path / "assets")
    github = tvr.GitHub([[tvr.release_entry(name, "2026-10-26T00:00:00Z")]], {name: tmp_path / "assets"})
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    said = []
    assert voices.install(["fr"], tmp_path / "a", hermes.Runner(dry=False), said.append,
                          transport=github.transport()) is None   # no signature published
    assert "no SHA256SUMS.minisig" in said[-1]
    (tmp_path / "assets" / "SHA256SUMS.minisig").write_bytes(key.sign((tmp_path / "assets" / "SHA256SUMS").read_bytes()))
    said = []
    assert voices.install(["fr"], tmp_path / "b", hermes.Runner(dry=False), said.append,
                          transport=github.transport()) is not None and not said
