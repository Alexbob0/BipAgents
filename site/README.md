# The websites

- **bipagents.com** sells the all-included offer, honestly. The home is short (reworked 2026-10-24 after a user
  review): the hero — on the left why, private and in Europe, with a « See more » that opens in place where your data
  is and who can read it (`#private` opens it, right under its button, so privacy comes first on the page as in the
  header); on the right « What they do » (`#results`): four things one asks and the card the agent answers with,
  taken from the app's demo by `hero_show()` in build.py (the booking's approval, the walk's map, the « Your runs »
  page, a reminder), appearing once one after another —, the Carnet, the family (with, in a card, sharing one
  conversation with friends), the comparison (`#compared`), the prices as two ways side by side — « All included » (the
  plans: monthly/yearly, Solo/Duo/Famille, web price against the stores' price, computed live) and « On your
  computer », free, whose install (what it means, what it needs, three steps with the command to copy, Windows, what
  one then does oneself) opens in place (`#install` opens it) —, six questions that settle a purchase, the footer
  (store badges, company, links). On a phone, a bar at the bottom keeps the beta's button. Their own pages: `/demo/` (the app's own demo
  playing in a chat: four scenarios, with the walk's map and the page kit's « Your runs » page inside the replies),
  `/faq/` (every question), `/beta/` (the public beta: what it is, what testers get, what we ask, TestFlight and
  Google Play badges, its terms on kerfystudio.com `/beta/`; « Join the beta » is the main button everywhere, the
  Carnet section after the cards shows what the agents keep); `/install/` only sends to the home's `#install`. English at `/`, French, German and
  Spanish at `/fr/`, `/de/`, `/es/` (and `/fr/demo/`, `/fr/faq/`…).
- **kerfystudio.com** — the publisher: its two apps (BipAgents, Tablée), how it works, the company's details (what Apple
  and Google check for an organization), contact, support, the privacy policy and terms in four languages and the
  mentions légales, rendered from [`docs/legal/`](../docs/legal/), and `/.well-known/security.txt`. French at `/`.

```bash
python3 site/build.py           # builds site/dist/bipagents.com and site/dist/kerfystudio.com
python3 site/build.py --serve   # and serves them: http://127.0.0.1:8770 (BipAgents), :8771 (Kerfy Studio)
```

Python 3.9+, Node for the Bips' still drawings (favicon, link previews), Chrome or Brave for the link previews
(`og-<lang>.png`, 1200×630), `sips` (macOS) to make the demo map a small JPEG. Without them, the build still works.

## What the owner fills in: `site/config.json`

One file, read by both sites:

- **`company`** — capital, SIREN, RCS city, registered office, phone, VAT number, president. Shown in both footers, on
  kerfystudio.com's company block and support page, and written into the legal texts' placeholders (`[SIREN]`,
  `[adresse du siège]`, `[Prénom Nom]`… in the four languages). Empty = a yellow `[TODO]` mark stays visible.
- **`contact`** — `general` (contact@kerfystudio.com: the publisher, authorities), `support` (support@bipagents.com:
  the app), `abuse`, `privacy`, `security` (all @bipagents.com; `security` goes into security.txt).
- **`stores`** — App Store and Google Play links for BipAgents and for Tablée. Empty = « Coming soon to » badges.
- **`beta`** — `testflight` (TestFlight public link) and `play` (Google Play testing link) for the `/beta/` page; both
  empty = « Coming soon on » badges and « Opening soon ». `cap` = the places line (« Places: … »), one text
  or one per language; empty = no line.
- **`checkout.url`** — the web checkout, one template for every plan and period:
  `https://…/v1/web/checkout?plan={plan}&period={period}&lang={lang}` (the page adds `&currency=`) (`plan` = solo | duo | famille, `period` = month |
  year). `checkout.links.<plan>.<period>` overrides one of them. Empty = no « Subscribe » button: the price block shows
  « the web subscription is coming very soon » and the store badges instead.
- **`prices`** — per currency (`prices.currencies`: EUR, USD, CAD, GBP, SGD, ILS; decided 2026-10-23, round prices of
  our own, not Stripe's conversion): web and store, monthly and yearly (10 months for 12), `tax` inclusive (VAT/GST
  inside) or exclusive (US, Canada: « + tax » shown, sales tax added at checkout), the `countries` whose visitors get
  it first, the selector's `symbol`. The one without countries (EUR) is the default the page is built in; SGD and ILS
  are proposals. `store: null` = the stores' price in that currency isn't known yet: the « in the app » line is hidden
  and the savings line compares the year with 12 months. Keep it equal to the guichet's guichet.toml (`[web.currencies]`
  and the plans' `web.<currency>`), which is what Stripe charges. The page picks the currency from the visitor's
  browser region (en-US → USD, en-GB → GBP, *-CA → CAD, *-SG → SGD, he/*-IL → ILS, else EUR), a selector changes it
  (remembered in localStorage), prices are written with `Intl.NumberFormat` in the page's language, and the checkout
  link carries `&currency=`.

## Where things come from

The pages are `src/bipagents/{index,demo,faq,beta}.html` (and `install.html`, the redirect to `#install`), sharing `partials/top.html` (head, header) and
`partials/foot.html` (footer, script) through `{{>top}}` / `{{>foot}}`; `PAGES` in `build.py` lists them, and
`FAQ_HOME` picks the home's questions. One `site.js` for all: each part runs where its elements are.

Nothing is copied twice: texts are in `src/<site>/i18n/*.json`, the demo's scenarios and pages from
`App/Resources/Demo/demo.json` (the « Your runs » page is the page kit's real output, written to
`assets/pages/runs-<lang>.html`; the walk's map is the OpenStreetMap drawing the demo embeds), the podcasts from the
apps' own files, the legal texts from `docs/legal/`. The Bips are the apps' pixel Bips, drawn by `src/shared/bips.js`
from `scripts/pixel/pixel.json` (the build writes it next to the engine as `assets/pixel-data.js`): the same squares,
colours per kind of agent, states (rest with a blink, think, talk, work…), tap reactions and costumes as the apps. During
an event (Halloween, Christmas, Easter…) the Bips one can tap wear its costumes; `?costumes=noel` (or `none`) previews
another. `prefers-reduced-motion` shows each state's still moment. The brand mark, the favicon (`favicon.svg`,
`icon-192.png`), the apple-touch-icon and the link previews use the app's icon « Lueur bleuet » (the SVG drawn by the
same engine as `scripts/pixel/icon.py`, the PNGs from its iOS 1024). `/play/` (`src/bipagents/play.html`) lets one play
with the eight Bips. Both sites use the engine. Tablée's colours are its « Table dressée » palette; its icon is redrawn
in SVG.

**Privacy**: no cookie, no tracker, no third-party request (the font, Nunito, is served from the site). So no consent
banner is needed.

**Accessibility and motion**: everything works with the keyboard (demo tabs and plan picker are ARIA tab/radio groups
with arrow keys), decorative animations are `aria-hidden`, and `prefers-reduced-motion` stops them. Light by default,
with a dark look that follows the system.

**Hosting**: `site/dist/<site>` is plain static files. Both sites live on OVH's free web hosting (Apache), so
`hosting.py` adds to each a `.htaccess` (one redirect to `https://<apex>/` from http and www; HSTS, short at first;
nosniff, Referrer-Policy, a Content-Security-Policy with no third party and the hashes of the pages' own inline scripts;
a year of cache for `?v=` stylesheets and scripts, a week for pictures and fonts, none for pages), a `404.html` and
`/.well-known/security.txt`. A private script of the publisher mirrors them to the hostings over SFTP (OVH offers no
FTP over TLS) and checks each home page. The pages already link to each other by their final addresses.

The Bips' drawings, their voices, the recordings and the screenshots are not under the MIT license
([LICENSE-ASSETS](../LICENSE-ASSETS)).
