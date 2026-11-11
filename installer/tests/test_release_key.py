"""Release integrity (audit 2026-10-27, finding 22): a release's SHA256SUMS are trusted only once signed with minisign
by BipAgents' release key, pinned in the checkout. The keys here are throwaway ones made by the test itself
(release_signing.py, the `signer` fixture)."""
import base64
import hashlib
import shutil
import subprocess

import pytest

from bipinstall import hermes, release_key, rustbridge, shared_hermes, voices
from release_signing import Key
from test_rustbridge import archive


def test_rfc8032_vector():
    public = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
    sig = bytes.fromhex("e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46b"
                        "d25bf5f0595bbe24655141438e7a100b")
    assert release_key.ed25519_verify(public, b"", sig)
    assert not release_key.ed25519_verify(public, b"x", sig)


def test_a_signature_is_checked_against_the_pinned_key(signer):
    sums = b"ab" * 32 + b"  bipbridge-linux-x86_64.tar.gz\n"
    release_key.check(sums, signer.sign(sums))                   # minisign's default (prehashed)
    release_key.check(sums, signer.sign(sums, prehashed=False))  # legacy
    with pytest.raises(release_key.ReleaseSignatureError, match="aren't signed"):
        release_key.check(sums.replace(b"ab", b"cd", 1), signer.sign(sums))       # tampered sums
    with pytest.raises(release_key.ReleaseSignatureError, match="aren't signed"):
        release_key.check(sums, Key().sign(sums))                                 # another key
    forged = signer.sign(sums).replace(b"hashed", b"hashez")
    with pytest.raises(release_key.ReleaseSignatureError, match="aren't signed"):
        release_key.check(sums, forged)                                           # trusted comment changed
    with pytest.raises(release_key.ReleaseSignatureError, match="no SHA256SUMS.minisig"):
        release_key.check(sums, None)
    with pytest.raises(release_key.ReleaseSignatureError, match="can't be read"):
        release_key.check(sums, b"<html>Not Found</html>")


@pytest.mark.skipif(not shutil.which("minisign"), reason="the minisign program signs here as the workflows do")
def test_what_the_minisign_program_signs_is_accepted(tmp_path):
    """The release workflows and scripts/sign-release-sums.sh sign with the minisign program itself, not this file's
    throwaway signer: its output must pass the installer's own check."""
    run = lambda *args: subprocess.run(["minisign", *args], cwd=tmp_path, stdin=subprocess.DEVNULL,
                                       capture_output=True, check=True)
    run("-G", "-W", "-p", "release.pub", "-s", "release.key")
    sums = b"ab" * 32 + b"  bipbridge-linux-x86_64.tar.gz\n"
    (tmp_path / "SHA256SUMS").write_bytes(sums)
    run("-S", "-s", "release.key", "-m", "SHA256SUMS", "-t", "BipAgents bridge-000000000001 built from 64d7755e0346")
    public = (tmp_path / "release.pub").read_text()
    release_key.check(sums, (tmp_path / "SHA256SUMS.minisig").read_bytes(), key=public)
    with pytest.raises(release_key.ReleaseSignatureError, match="aren't signed"):
        release_key.check(sums.replace(b"ab", b"cd", 1), (tmp_path / "SHA256SUMS.minisig").read_bytes(), key=public)


def test_the_placeholder_key_keeps_the_old_behaviour_with_a_warning():
    placeholder = release_key.PLACEHOLDER + "-a-checkout-without-a-key"
    assert not release_key.pinned(placeholder)
    said = []
    release_key.check(b"anything", None, key=placeholder, warn=said.append)
    assert "no release key pinned" in said[0]


def test_bipagents_release_key_is_pinned():
    # Alex's key, made 2026-11-11 (minisign -G): « Ed », the key id, then the 32-byte Ed25519 public key
    assert release_key.pinned()
    raw = base64.b64decode(release_key.PUBLIC_KEY)
    assert len(raw) == 42 and raw[:2] == b"Ed" and raw[2:10].hex() == "f054eb0f5518d52e"


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


def test_the_bridge_is_installed_only_from_signed_sums(tmp_path, signer):
    name, _, opener = bridge_release(signer)
    assert rustbridge.fetch(tmp_path, "000000000001", name, opener).read_bytes() == b"\x7fELF bridge"
    # Someone holding the workflow's token swaps the archive and its sum together: the old signature no longer fits.
    name, files, opener = bridge_release(signer)
    evil = archive({"bipbridge": b"evil"})
    signature = files["SHA256SUMS.minisig"]
    files[name] = evil
    files["SHA256SUMS"] = f"{hashlib.sha256(evil).hexdigest()}  {name}\n".encode()
    files["SHA256SUMS.minisig"] = signature
    with pytest.raises(rustbridge.RustBridgeError, match="aren't signed"):
        rustbridge.fetch(tmp_path, "000000000002", name, opener)
    name, _, opener = bridge_release(signer, signed=False)
    with pytest.raises(rustbridge.RustBridgeError, match="no SHA256SUMS.minisig"):
        rustbridge.fetch(tmp_path, "000000000003", name, opener)
    assert not rustbridge.binary(tmp_path, "000000000002").exists()
    assert not rustbridge.binary(tmp_path, "000000000003").exists()


def test_without_a_pinned_key_the_bridge_is_still_installed_from_its_sums(tmp_path, monkeypatch, capsys):
    monkeypatch.setattr(release_key, "PUBLIC_KEY", release_key.PLACEHOLDER)   # a checkout from before the key
    name, _, opener = bridge_release(None, signed=False)
    assert rustbridge.fetch(tmp_path, "000000000001", name, opener).exists()
    assert "no release key pinned" in capsys.readouterr().err


# -- the shared Hermes ----------------------------------------------------------------------------------------------

@pytest.mark.skipif(not shutil.which("zstd"), reason="zstd packs and unpacks the release archive")
@pytest.mark.parametrize("signature", ["good", "missing", "other key"])
def test_the_shared_hermes_is_used_only_from_signed_sums(tmp_path, monkeypatch, signer, signature):
    from test_shared_hermes import OnlyRemoves, released
    archive, fetch, _ = released(tmp_path)
    sums = (tmp_path / "release" / "SHA256SUMS").read_bytes()
    if signature != "missing":
        (tmp_path / "release" / "SHA256SUMS.minisig").write_bytes((signer if signature == "good" else Key()).sign(sums))
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

def test_the_voices_are_used_only_from_signed_sums(tmp_path, monkeypatch, signer):
    import test_voices_release as tvr
    tvr.build(tmp_path / "voices")
    name = tvr.release.pack(tmp_path / "voices", tmp_path / "assets")
    github = tvr.GitHub([[tvr.release_entry(name, "2026-10-26T00:00:00Z")]], {name: tmp_path / "assets"})
    monkeypatch.setattr(voices, "source", lambda home: (None, None))
    said = []
    assert voices.install(["fr"], tmp_path / "a", hermes.Runner(dry=False), said.append,
                          transport=github.transport()) is None   # no signature published
    assert "no SHA256SUMS.minisig" in said[-1]
    signer.sign_release(tmp_path / "assets")
    said = []
    assert voices.install(["fr"], tmp_path / "b", hermes.Runner(dry=False), said.append,
                          transport=github.transport()) is not None and not said
