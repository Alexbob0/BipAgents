---
name: slides-styles
description: Make a PowerPoint (.pptx) deck with a real visual style picked for the subject (10 styles), or in the person's own colours, logo or template. You write the content as JSON; the script does the design.
version: 0.3.0
metadata:
  hermes:
    tags: [pptx, powerpoint, presentation, slides, deck, diapos, diaporama, pitch, keynote]
    category: productivity
---

# Styled PowerPoint decks

Use this (not raw python-pptx, not the `powerpoint` skill) for any presentation, slides, deck or .pptx, in a
conversation or in a background task alike.
Scripts: `/root/.hermes/skills/slides-styles/scripts/` (always use this full path; S below).

1. **Style.** Pick one from the subject and audience, then say it in ONE line, e.g.
   « Style : Artisan (papier kraft, capitales serif). Dis-moi si tu en veux un autre. »
   When nothing in the subject calls for one: **{{DEFAULT_STYLE}}** (this agent's usual style).

   | style | for |
   |---|---|
   | editorial | culture, story, portrait, food, lifestyle |
   | suisse | strategy, architecture, design, sober business |
   | terminal | tech, software, data, security |
   | pop | startup pitch, launch, event, youth, marketing |
   | maison | hotel, restaurant, wedding, real estate, fashion, wine |
   | rapport | budget, financial review, board, KPIs, analysis |
   | ecole | lesson, workshop, kids, training, how-to |
   | carnet | trip, photo story, memories, visit |
   | artisan | bakery, shop, farm, crafts, local products |
   | bauhaus | art, culture, design school, creative agency |

   Another style asked → same deck.json, add `--style pop` to the build command (step 5).
   « mes couleurs » → `"brand": {"colors": ["#0A7D5A", "#F2C14E"]}`; « mon logo » → `"brand": {"logo": "logo.png"}`;
   « mon modèle » (they sent a .pptx) → `"template": "/fichiers/…/modele.pptx"` (their colours, fonts and master, our layouts).
2. **Plan** 6-12 slides (or the number asked), one idea each. Mix layouts: never two `bullets` in a row; prefer
   `big_number`, `stats`, `chart`, `image_text`, `timeline`, `quote`. Titles ≤ 60 characters, ≤ 5 bullets.
   Never invent contact details or figures: ask, or write « à compléter ».
3. **Photos**: when the subject has places, products or people, put a photo on the cover and on each `image_text`:
   `python3 S/photo.py "Cancale port" cancale.jpg` → path + credit (put it in the slide's `"credit"`).
   **Trip or walk map**: call the `map_image` tool (places by name, `line: true`); its result names the picture's
   path in your sandbox (« Picture in your sandbox: /fichiers/cartes/m-….png »): use that path as a `timeline`
   `"image"`. Never fetch map tiles yourself.
4. **Write `deck.json`** with write_file, in a folder of its own (e.g. `/envoi/fournil/`). Content only, no positions
   or colours. All layouts and fields: `python3 S/deck.py --help`. Example:
   `{"style": "artisan", "title": "Le Fournil", "slides": [{"layout": "cover", "title": "...", "subtitle": "...", "image": "pain.jpg"},
   {"layout": "big_number", "value": "+18 %", "label": "...", "context": "..."}, {"layout": "chart", "title": "...",
   "chart": {"type": "bar", "categories": ["2026", "2027"], "series": {"CA": [160, 240]}, "unit": "k€"}, "takeaway": "..."},
   {"layout": "closing", "title": "Merci", "contact": "..."}]}`
5. **Build**: `cd <that folder> && python3 S/deck.py deck.json <name>.pptx`. If it reports `missing_images`, fetch them
   again or drop them. Only if its answer has a `look` command: run it, then `vision_analyze` on `render/sheet.png`,
   fix cut text or wrong photos in deck.json and build again (at most 2 rounds).
6. **Send the .pptx** to the person with `give_file` (in a background task too: the file must reach the
   conversation). Charts and tables stay editable.
