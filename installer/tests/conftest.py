"""Fixtures shared by the installer's tests."""
import pytest

from release_signing import Key


@pytest.fixture
def signer(monkeypatch):
    """A throwaway release key pinned in place of BipAgents' own (installer/bipinstall/release_key.py): the fake
    releases a test makes are signed with it, as the release workflows sign the real ones."""
    from bipinstall import release_key
    made = Key()
    monkeypatch.setattr(release_key, "PUBLIC_KEY", made.public)
    return made
