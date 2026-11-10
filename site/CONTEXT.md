# The websites: context for a new designer and her agents

This is a hand-over of **context**, not a brief. The look, the layout, the order of the sections, the visuals, the tone
of the copy: all of it is open. `site/README.md` describes the site **as it is today** (the home was reworked on
9 Nov 2026); read it as a map of what exists, not as rules to keep. Different options are welcome.

What follows is only what the site has to stay true to, and how to work in this repository.

## What BipAgents is

- An iPhone and Android app to talk with your own AI agents, the « Bips » (one per part of life: daily life,
  wellness, budget, family, work…). They **act**: read and sort mail, calendar, reminders, browse the web, run
  routines, make short podcasts and pages, keep a shared notebook (the « Carnet ») of what matters about you and your
  people. Voice included (Live calls).
- **Built on Hermes Agent** (open-source agent by Nous Research). Say « built on Hermes Agent »; people who already
  run Hermes can plug their own in.
- Each person's agents live in **their own space**: either the all-included offer (a private server « place » in the
  EU, set up in a minute) or free on one's own computer (one command).
- Published by **Kerfy Studio** (SAS, Paris). Public beta planned around **18 November 2026** (TestFlight + Google Play
  testing), paid launch right after.
- People: individuals and families. Languages: **English, French, German, Spanish** (Dutch later). Sold in the EU,
  the US, the UK, Canada, Singapore and Israel; servers in the EU.

## The two sites

- **bipagents.com**: the product. Job: make people understand what the Bips do, trust where their data goes, try
  the demo, join the beta / subscribe; also the free install for people who run it themselves, the FAQ, `/status/`.
- **kerfystudio.com**: the publisher. Company details, contact and support, and the legal texts (privacy policy,
  terms, mentions légales) in four languages.

## Facts that must stay true (whatever the design)

1. **Privacy is the core promise, and every word of it must be true.** What we can say: Kerfy Studio doesn't read your
   space; no ads, no trackers, no selling data; your agents run in your own space in the EU; the AI model runs at a
   European host through EUrouter and only receives what your agent sends it; the model's makers never receive your
   messages. What we must NOT claim: that nobody could ever technically access the server (the host and Kerfy could, in
   principle: the site says so honestly, see `private.who3` in the i18n files), nor anything about « no training » by
   the model host until it is guaranteed by our own settings (planned).
2. **The site itself has no cookie, no tracker, no third-party request** (fonts are served from the site). The
   hosting sends a strict Content-Security-Policy, so an external script, font or analytics simply won't load. Keep it
   that way: it's part of the promise (and why there is no consent banner).
3. **Prices** come from `site/config.json` (per currency, web vs store, monthly/yearly) and must equal what the
   checkout charges. Don't type prices in pages; change them only there, and only when the owner says so.
4. **Legal texts** are rendered from `docs/legal/*.md`. Don't rewrite them in the site; if wording must change, it
   changes there (and those texts carry legal weight: ask first).
5. **The demo is the app's real demo** (`App/Resources/Demo/demo.json`, shared with the iPhone and Android apps).
   Don't edit it for the site's sake without telling the owner: the apps use the same file.
6. **The Bips** are drawn by the same pixel engine as the apps (`src/shared/bips.js`, data in `scripts/pixel/`). Use
   them as you like on the site; their drawings, voices and screenshots are not MIT (`LICENSE-ASSETS`).
7. **Four languages**: every text lives once, in `src/<site>/i18n/<lang>.json` (en, fr, de, es); a new text needs its
   four versions. Plain words, no technical jargon in the product's voice.

## How to work here

```bash
git clone git@github.com:Alexbob0/bipagents-private.git   # the private repository (ask the owner for access)
cd bipagents-private && git checkout main-next              # the branch with the latest site
git switch -c site-<your-topic>                             # your own branch
python3 site/build.py --serve                               # http://127.0.0.1:8770 (bipagents.com), :8771 (kerfystudio.com)
python3 scripts/test_deploy_sites.py                        # the sites' checks
```

- Python 3.9+ is enough to build; Node and Chrome/Brave only refresh the Bips' still pictures and the link previews.
- Pages: `site/src/bipagents/*.html` (+ `partials/`), one `site.css`, one `site.js`; kerfystudio.com in
  `site/src/kerfystudio/`. The build writes `site/dist/` (never edit `dist/`).
- Stay inside `site/` (and the i18n files). The rest of the repository is the apps and the server.
- **Delivering**: push your branch and tell the owner. Publishing to the live sites is done from the owner's Mac (the
  hosting credentials are not in GitHub). Don't try to deploy.

## Not in GitHub

- The hosting credentials (owner's Mac only).
- Some design mock-ups made along the way (living-room versions, the Bips' animation gallery): ask the owner if you
  want to see them.

## Open points the owner knows about

- kerfystudio.com's legal pages still show a few yellow `[TODO]` marks (a mediator, an arbitration body, the e-mail
  sending provider…): they are legal decisions, not design.
- The beta page reads « coming soon » until the TestFlight / Play links are put in `site/config.json` (`beta`) when the
  beta opens.
