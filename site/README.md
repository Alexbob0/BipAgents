# The websites

- **bipagents.com** sells the all-included offer, honestly. In order: the hero (price from, the plan button), the app's
  own demo playing in a chat (four scenarios, with the walk's map and the page kit's « Your runs » page inside the
  replies), « a day with your Bips » (a sticky phone whose screen follows the story: podcast, mail, the Carnet, the
  visible browser, approvals, background tasks, family), the morning podcast player, Live, the Bips, why it's private
  (the apps' diagram, animated), the prices (monthly/yearly, Solo/Duo/Famille, web price against the stores' price,
  computed live), the free self-install, questions, the final call. On a phone, a bar at the bottom keeps the chosen
  plan and its button. English at `/`, French, German and Spanish at `/fr/`, `/de/`, `/es/`.
- **kerfystudio.com** — the publisher: its two apps (BipAgents, Tablée), how it works, the company's details (what Apple
  and Google check for an organization), contact, support, the privacy policy and terms in four languages and the
  mentions légales, rendered from [`docs/legal/`](../docs/legal/), and `/.well-known/security.txt`. French at `/`.

```bash
python3 site/build.py           # builds site/dist/bipagents.com and site/dist/kerfystudio.com
python3 site/build.py --serve   # and serves them: http://127.0.0.1:8770 (BipAgents), :8771 (Kerfy Studio)
```

Python 3.9+, Node for the Bips' still drawings (icon, link previews), Chrome or Brave for the link previews
(`og-<lang>.png`, 1200×630), `sips` (macOS) to make the demo map a small JPEG. Without them, the build still works.

## What the owner fills in: `site/config.json`

One file, read by both sites:

- **`company`** — capital, SIREN, RCS city, registered office, phone, VAT number, president. Shown in both footers, on
  kerfystudio.com's company block and support page, and written into the legal texts' placeholders (`[SIREN]`,
  `[adresse du siège]`, `[Prénom Nom]`… in the four languages). Empty = a yellow `[TODO]` mark stays visible.
- **`contact`** — `contact@` (general), `support@`, `abuse@`.
- **`stores`** — App Store and Google Play links for BipAgents and for Tablée. Empty = « Coming soon to » badges.
- **`checkout.url`** — the web checkout, one template for every plan and period:
  `https://…/v1/web/checkout?plan={plan}&period={period}&lang={lang}` (`plan` = solo | duo | famille, `period` = month |
  year). `checkout.links.<plan>.<period>` overrides one of them. Empty = no « Subscribe » button: the price block shows
  « the web subscription is coming very soon » and the store badges instead.
- **`prices`** — euros TTC, web and store, monthly and yearly (10 months for 12). The page, its live picker and the
  savings line are computed from these.

## Where things come from

Nothing is copied twice: texts are in `src/<site>/i18n/*.json`, the demo's scenarios and pages from
`App/Resources/Demo/demo.json` (the « Your runs » page is the page kit's real output, written to
`assets/pages/runs-<lang>.html`; the walk's map is the OpenStreetMap drawing the demo embeds), the podcasts from the
apps' own files, the legal texts from `docs/legal/`. The Bips are drawn by `src/shared/bips.js`, the same geometry as
the apps (both sites use it). Tablée's colours are its « Table dressée » palette; its icon is redrawn in SVG.

**Privacy**: no cookie, no tracker, no third-party request (the font, Nunito, is served from the site). So no consent
banner is needed.

**Accessibility and motion**: everything works with the keyboard (demo tabs and plan picker are ARIA tab/radio groups
with arrow keys), decorative animations are `aria-hidden`, and `prefers-reduced-motion` stops them. Light by default,
with a dark look that follows the system.

**Hosting**: `site/dist/<site>` is plain static files, for any static host (GitHub Pages, Cloudflare Pages, Netlify…).
Point the domains at it once bought. The pages already link to each other by their final addresses.

The Bips' drawings, their voices, the recordings and the screenshots are not under the MIT license
([LICENSE-ASSETS](../LICENSE-ASSETS)).
