"""Fixtures shared by the installer's tests."""
import sys

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



# Tests marked `linux` need Linux itself (openat2, /proc/self/fdinfo and mountinfo, mount namespaces, fusectl, GNU
# rsync 3): skipped elsewhere (the release battery's Mac lane), run on the build server by the battery's
# « Installeur (Linux, espaces de montage) » suite (scripts/release-battery/server.sh: `pytest -m linux` under
# `unshare -rm`, Python 3.12).
LINUX_ONLY = ("Linux only (openat2, /proc, mountinfo, mount namespaces): run by the release battery's « Installeur "
              "(Linux, espaces de montage) » suite on the build server")


def pytest_configure(config):
    config.addinivalue_line("markers", "linux: needs Linux itself; skipped elsewhere, run by the battery's Linux "
                                       "installer suite (pytest -m linux, under unshare -rm)")
    config.addinivalue_line("markers", "bridge_build: needs the bridge's release build (skipped without one): run by "
                                       "the battery's Linux installer suite with the one the build server made")


def pytest_collection_modifyitems(config, items):
    if sys.platform.startswith("linux"):
        return
    skip = pytest.mark.skip(reason=LINUX_ONLY)
    for item in items:
        if item.get_closest_marker("linux") is not None:
            item.add_marker(skip)

