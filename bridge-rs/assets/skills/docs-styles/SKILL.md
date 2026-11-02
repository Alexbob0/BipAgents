---
name: docs-styles
description: Make a Word document (.docx) — quote, invoice, letter, CV, meeting minutes, report, newsletter, product sheet — with a real visual style (8 styles) or the person's own colours, logo or Word template. You write the content as JSON; the script does the layout with real Word styles.
version: 0.3.0
metadata:
  hermes:
    tags: [docx, word, document, devis, facture, lettre, cv, compte rendu, rapport]
    category: productivity
---

# Styled Word documents

Use this (not raw python-docx, not the `docx` skill) to create any Word document, in a conversation or in a
background task alike.
Scripts: `/root/.hermes/skills/docs-styles/scripts/` (always use this full path; S below).

1. **Kind**: `devis`, `facture`, `lettre`, `cv`, `compte_rendu`, `rapport`, `newsletter`, `fiche` or `autre`.
2. **Style.** Pick one from the subject, then say it in ONE line, e.g. « Style : Artisan. Dis-moi si tu en veux un autre. »
   When nothing in the subject calls for one: **{{DEFAULT_STYLE}}** (this agent's usual style).

   | style | for |
   |---|---|
   | editorial | careful letter, article, story, food |
   | suisse | CV, sober report, technical sheet |
   | terminal | tech documentation, specification |
   | pop | newsletter, invitation, association, event |
   | maison | upmarket quote, menu, hotel, wedding |
   | rapport | report, review, minutes, budget |
   | ecole | school, lesson sheet, note to parents |
   | artisan | craft or shop quote, product sheet, local business |

   Another style asked → same doc.json, add `--style maison` to the build command (step 4). « mes couleurs » → `"brand": {"colors": ["#hex"]}`;
   « mon logo » → `"brand": {"logo": "logo.png"}`; « mon modèle » (they sent a .docx) → `"template": "/fichiers/…/modele.docx"`
   (their styles, header and footer are kept).
3. **Write `doc.json`** with write_file, in a folder of its own (e.g. `/envoi/devis-buffet/`): content only. Blocks and
   fields: `python3 S/doc.py --help`. Quotes and invoices: put the lines in an `items` block with `"vat"`; the script
   computes the totals HT, TVA and TTC to the cent (never compute them yourself).
   Example: `{"kind": "devis", "style": "artisan", "title": "Devis", "subtitle": "Buffet du 14 juin",
   "from": {"name": "...", "lines": ["..."]}, "to": {"name": "...", "lines": ["..."]}, "meta": [["N°", "D-014"], ["Date", "..."]],
   "blocks": [{"items": [["Désignation", "Qté", "Prix unitaire HT"], ["Pain au levain", 12, 6.5]], "vat": 5.5},
   {"heading": "Conditions", "level": 2}, {"bullets": ["..."]}], "footer": "SIRET ..."}`
   CV: skills, languages and diplomas go in `cv.sidebar`; experience as `entry` blocks.
   Never invent contact details, SIRET or prices: ask, or leave a visible « à compléter ».
4. **Build**: `cd <that folder> && python3 S/doc.py doc.json <name>.docx`. Only if its answer has a `look` command:
   run it, then `vision_analyze` on `render/sheet.png`; fix and build again (at most 2 rounds).
5. **Send the .docx** to the person with `give_file` (in a background task too: the file must reach the conversation).
   It stays editable: real headings, lists, tables, table of contents.
