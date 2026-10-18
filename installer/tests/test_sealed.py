import io
import os

import pytest

from bipinstall import sealed


def seal(data: bytes, passphrase: str, writes: int = 7) -> bytes:
    out = io.BytesIO()
    writer = sealed.SealedWriter(out, passphrase, log_n=10)
    step = max(1, len(data) // writes)
    for i in range(0, len(data), step):
        writer.write(data[i:i + step])
    writer.close()
    return out.getvalue()


def unseal(blob: bytes, passphrase: str) -> bytes:
    return sealed.SealedReader(io.BytesIO(blob), passphrase).read()


@pytest.mark.parametrize("size", [0, 1, sealed.CHUNK - 1, sealed.CHUNK, sealed.CHUNK + 1, 3 * sealed.CHUNK + 5])
def test_what_is_sealed_comes_back_whole(size):
    data = os.urandom(size)
    phrase = sealed.new_passphrase()
    blob = seal(data, phrase)
    assert unseal(blob, phrase) == data
    # Typed by hand: case, spaces and dashes do not matter.
    assert unseal(blob, " " + phrase.upper().replace("-", " ") + " ") == data


def test_a_wrong_passphrase_or_a_damaged_or_cut_file_is_refused():
    data = os.urandom(2 * sealed.CHUNK + 10)
    blob = seal(data, "good")
    with pytest.raises(sealed.SealedError, match="wrong passphrase"):
        unseal(blob, "bad")
    damaged = bytearray(blob)
    damaged[-40] ^= 1
    with pytest.raises(sealed.SealedError, match="damaged"):
        unseal(bytes(damaged), "good")
    # The last chunk dropped: the one before is not marked last, refused.
    last = len(blob) - (4 + 10 + 16)
    with pytest.raises(sealed.SealedError):
        unseal(blob[:last], "good")
    with pytest.raises(sealed.SealedError, match="cut short"):
        unseal(blob[:-3], "good")
    with pytest.raises(sealed.SealedError, match="not a BipAgents export"):
        unseal(b"PK\x03\x04 a zip file", "good")


def test_passphrases_are_easy_to_copy():
    phrase = sealed.new_passphrase()
    assert len(phrase) == 29 and phrase.count("-") == 4
    assert not set(phrase.replace("-", "")) & set("01ilo")
