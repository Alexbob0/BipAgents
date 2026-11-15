"""The public repository BipAgents installs from, and its channels: the one setting.

Every file that names the public repository (the installers, the seat scripts, the workflows, the docs, the apps'
links, the site) says the same as this: `scripts/public-repo.py check` (its test) finds any that doesn't, and
`scripts/public-repo.py set <owner>/<name> --branch <default branch>` rewrites them all at once (the switch to the
BipAgents organisation's repository: one commit). Files that can't import it (install.sh, seat-agent.py, add-seat.sh…)
carry the value itself, checked the same way.

Channels (2026-11-15, Alex): the organisation's repository has two branches an install can follow, `pre-test` (each
publication lands there first: Alex's aibox and his places) and `stable` (what Kerfy Studio promoted after the
pre-test: the default branch, what a self-hosted install follows). The first repository has only `main`, which its
installs follow as before. The guichet's places follow no branch: each update order names a commit (and its
repository, among KNOWN only)."""
from __future__ import annotations

SLUG = "Alexbob0/BipAgents"
URL = f"https://github.com/{SLUG}.git"
# The default branch: what the install line and the docs' links name, what an install follows unless told otherwise.
BRANCH = "main"
# The servers' container images (shared_images.py, .github/workflows/server-images.yml): GHCR wants it in lower case.
REGISTRY = f"ghcr.io/{SLUG.split('/')[0].lower()}"

# Every public repository an install or a place may be moved to (the guichet's orders name one of these, never
# another): the first one, then the organisation's. Only code review adds one.  (known-public-repos)
KNOWN = ("Alexbob0/BipAgents", "BipAgents/BipAgents")
# The channels an install may follow; `main` on a repository that has no channels.
CHANNELS = ("stable", "pre-test")
FALLBACK = "main"


def url_of(slug: str) -> str:
    return f"https://github.com/{slug}.git"


def known(url: str) -> bool:
    """An https or ssh address of one of the KNOWN public repositories (with or without .git)."""
    import re
    found = re.fullmatch(r"(?:https://github\.com/|git@github\.com:|ssh://git@github\.com/)([^/]+/[^/]+?)(?:\.git)?/?",
                         url.strip())
    return bool(found) and found.group(1) in KNOWN
