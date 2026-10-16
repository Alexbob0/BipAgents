# Contributing

Thanks for wanting to help! BipAgents is a small project: an issue before a big change saves everyone time.

## Where things are

| Part | Folder | Run its tests |
|---|---|---|
| iPhone app (SwiftUI, iOS 18+, Swift 6) | `App/`, `NotificationService/`, `Widgets/` | Xcode, scheme BipAgents |
| Hermes client | `Packages/HermesKit` | `swift test` |
| Voice (capture, transcription, playback) | `Packages/VoiceKit` | `swift test` |
| Bridge (Python, FastAPI) and web console | `bridge/` | `python -m pytest -q` in `bridge/` |
| Installer and `bipagents` | `installer/`, `install.sh` | `PYTHONPATH=installer:bridge python -m pytest -q installer/tests` |
| Agents' visible browser | `browser/` | see its README |
| Hermes patches | `hermes/patches/` | see its README |

The bridge's dependencies: `python3.12 -m venv bridge/.venv && bridge/.venv/bin/pip install -r bridge/requirements-dev.txt`.

## Trying the app without a server

Open `BipAgents.xcodeproj`, scheme BipAgents › Run › Arguments: `-demo`. Add `-screen conversation`, `-screen call`,
`-tab inbox` or `-demoAgents 5` to land on a screen. To build on your iPhone, put your team in `Config/Local.xcconfig`
(`DEVELOPMENT_TEAM = <your Team ID>`, and `BIP_BUNDLE_ID = com.example.bipagents` if you use your own identifier).

## A few habits

- **Languages**: the app speaks French, English, Spanish and German. A new string in the app goes into
  `App/Resources/Localizable.xcstrings` in all four; READMEs and docs exist in English and French (`*.fr.md`).
- **No secrets, no personal data**: keys, addresses, names and real conversations never go in the repo, tests included.
- **Security first**: a new agent-facing feature must not widen what an agent can reach (links between agents,
  approvals, the vault). See [SECURITY.md](SECURITY.md).
- **Tests** come with a change to the bridge or the installer; CI runs them on every pull request.
- **Commits** in English, saying what changes for the person using it.

## License

By contributing, you agree that your contribution is released under the [MIT license](LICENSE).
