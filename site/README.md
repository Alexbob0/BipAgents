# The websites

- **bipagents.com** — the app, in this order: what the agents automate (the app's own demo playing in a chat), one Bip
  per agent, private with nothing to do (a computer of one's own at a partner host, the AI of one's choice) and right
  below it the install command for one's own computer; then the morning podcast, Live, approvals, questions. English
  at `/`, French, German and Spanish at `/fr/`, `/de/`, `/es/`.
- **kerfystudio.com** — the publisher: its page, support (contact details the stores ask for), the privacy policy and
  terms in four languages and the mentions légales, rendered from [`docs/legal/`](../docs/legal/), and
  `/.well-known/security.txt`.

```bash
python3 site/build.py           # builds site/dist/bipagents.com and site/dist/kerfystudio.com
python3 site/build.py --serve   # and serves them: http://127.0.0.1:8770 (BipAgents), :8771 (Kerfy Studio)
```

Python 3.12 (or the bridge's venv), Node for the Bips' still drawings (icon, Kerfy Studio's page), and Chrome or Brave
for the link previews (`og-<lang>.png`, 1200×630). Without them, the build still works, without those images.

Nothing is copied twice: texts are in `src/<site>/i18n/*.json`, the demo's scenarios come from
`App/Resources/Demo/demo.json`, the podcasts from the apps' own files, the legal texts from
`docs/legal/`. The Bips are drawn by `src/shared/bips.js`, the same geometry as the apps.

**Privacy**: no cookie, no tracker, no third-party request (the font, Nunito, is served from the site). So no consent
banner is needed.

**Hosting**: `site/dist/<site>` is plain static files, for any static host (GitHub Pages, Cloudflare Pages, Netlify…).
Point the domains at it once bought. The pages already link to each other by their final addresses.

The Bips' drawings, their voices, the recordings and the screenshots are not under the MIT license
([LICENSE-ASSETS](../LICENSE-ASSETS)).
