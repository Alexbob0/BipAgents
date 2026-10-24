# Contributing

Thanks for wanting to help! BipAgents is a small project: an issue before a big change saves everyone time.

## Where things are

This repository holds BipAgents' server side, built on [Hermes Agent](https://github.com/NousResearch/hermes-agent).
The apps (iPhone, iPad, Android) are Kerfy Studio's and their source is private; bugs and ideas about them are welcome
as issues all the same.

| Part | Folder | Run its tests |
|---|---|---|
| Bridge (Rust) and web console | `bridge-rs/` | `cargo test` and `cargo clippy --all-targets -- -D warnings` in `bridge-rs/` |
| Bridge's HTTP contract (black-box suite) | `bridge/tests_blackbox/` | `python -m pytest tests_blackbox -q` in `bridge/`, after `cargo build --release` in `bridge-rs/` |
| Installer and `bipagents` | `installer/`, `install.sh`, `install.ps1` | `PYTHONPATH=installer python -m pytest -q installer/tests` |
| Push relay (Rust) | `relay/` | `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` in `relay/` |
| Bips' voices (Pocket TTS server) | `pocket/`, `voices/` | `python -m pytest pocket/tests` |
| Agents' visible browser | `browser/` | see its README |
| Hermes patches | `hermes/patches/` | see its README |
| Websites | `site/` | `python3 site/build.py --serve` |

The Python tests' dependencies: `pip install -r installer/requirements.txt -r bridge/tests_blackbox/requirements.txt`
(Python 3.12). The bridge and the relay need stable Rust. CI (`.github/workflows/tests.yml`) runs the bridge, the
installer, the relay and a parse check of `install.ps1` on every pull request.

To try a change for real: install BipAgents on a machine of yours (see the [README](README.md)), then build the
bridge there and point the installer at it with `BIPAGENTS_BRIDGE_BUILD=<folder of bipbridge and bip-browser-mcp>`
([installer](installer/README.md)).

## A few habits

- **Languages**: BipAgents speaks French, English, Spanish and German. READMEs and docs exist in English and French
  (`*.fr.md`).
- **Plain words for people**: what an agent or the app says to the person avoids technical terms.
- **No secrets, no personal data**: keys, addresses, names and real conversations never go in the repo, tests included.
- **Security first**: a new agent-facing feature must not widen what an agent can reach (links between agents,
  approvals, sandboxes, the vault). See [SECURITY.md](SECURITY.md).
- **Tests** come with a change to the bridge, the installer or the relay.
- **Commits** in English, saying what changes for the person using it.

## Contribution terms

BipAgents' code published here is under the [MIT license](LICENSE); the Bips' drawings, the app's icon, the Bips'
babble, their voices, the recordings and the screenshots are not ([LICENSE-ASSETS](LICENSE-ASSETS)). By submitting a
contribution (a pull request, a patch, a file in an issue), you agree that:

1. **it is released under the same terms**: the MIT license for code, LICENSE-ASSETS for drawings, icons, sounds,
   voices and recordings;
2. **Kerfy Studio**, the publisher of BipAgents (a French SAS), and its successors may use, modify and distribute it
   under any license, including another license than MIT for later versions. This right is worldwide, free, perpetual
   and irrevocable;
3. **you wrote it, or have the right to contribute it**, and it holds no secret, no personal data and nothing under a
   license that forbids this.

Your contribution stays yours: you keep every right to use it elsewhere. The pull request template asks you to
confirm these terms.
