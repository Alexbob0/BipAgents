"""Deck styles: each one is a visual identity (fonts, grid, shapes, title, image and chart treatments),
not a colour swap. Only fonts present on Windows + macOS + iOS/iPadOS + Google Slides/Docs are used, so a
deck looks the same in PowerPoint, Keynote, the iPhone preview and Google Slides (nothing to embed);
text is sized with their real advance widths (metrics.json).

The `fx` keys pick a treatment per element; deck.py implements each treatment once."""

STYLES = {
    "editorial": dict(
        label="Éditorial", hint="grands titres serif, photos pleine page, filets fins — culture, récit, portrait, gastronomie, magazine",
        head="Georgia", body="Georgia", lab="Arial", head_bold=False, head_size=40, body_size=22,
        bg="F5F1E8", alt="EAE3D4", text="141414", muted="6E685E", accent="B3261E", accent2="1F3A5F", accent3="B88A1B",
        on_accent="FFFFFF", line="141414", palette=["B3261E", "1F3A5F", "B88A1B", "6E685E", "141414", "7A9E7E"],
        fx=dict(title="rule", cover="bleed", section="numeral", quote="mark", closing="type", card="rules",
                table="rules", image="none", footer="running", decor="none", bullets="dash", stats="rules",
                timeline="line", people="square", gallery="mosaic", chart="light")),
    "suisse": dict(
        label="Suisse", hint="grille stricte, Arial gras, noir + un rouge, photos en noir et blanc — stratégie, architecture, design, sobre",
        head="Arial", body="Arial", lab="Arial", head_bold=True, head_size=40, body_size=22,
        bg="FFFFFF", alt="EFEFEF", text="000000", muted="6F6F6F", accent="E30613", accent2="000000", accent3="9B9B9B",
        on_accent="FFFFFF", line="000000", palette=["E30613", "000000", "8C8C8C", "C4C4C4", "5A5A5A", "E8888C"],
        fx=dict(title="grid", cover="poster", section="flood", quote="plain", closing="poster", card="toprule",
                table="rules", image="gray", footer="grid", decor="none", bullets="square", stats="huge",
                timeline="numbers", people="square", gallery="row", chart="highlight")),
    "terminal": dict(
        label="Terminal", hint="fond sombre, accents monospace, blocs de code — tech, logiciel, données, sécurité, équipes dev",
        head="Arial", body="Arial", lab="Courier New", head_bold=True, head_size=36, body_size=20,
        bg="0E1116", alt="161B22", text="E6EDF3", muted="8B949E", accent="3FB950", accent2="F0B429", accent3="58A6FF",
        on_accent="0E1116", line="30363D", palette=["3FB950", "58A6FF", "F0B429", "F778BA", "A371F7", "79C0FF"],
        fx=dict(title="prompt", cover="window", section="prompt", quote="code", closing="window", card="outline",
                table="mono", image="duotone", footer="path", decor="none", bullets="chevron", stats="mono",
                timeline="log", people="square", gallery="row", chart="dark")),
    "pop": dict(
        label="Pop", hint="blocs de couleurs vives, contours noirs, ombres franches — startup, lancement, événement, jeunes, marketing",
        head="Trebuchet MS", body="Trebuchet MS", lab="Trebuchet MS", head_bold=True, head_size=42, body_size=22,
        bg="FFF4E0", alt="FFFFFF", text="111111", muted="4A4A4A", accent="FF5A36", accent2="3D5AFE", accent3="FFC700",
        on_accent="111111", line="111111", palette=["FF5A36", "3D5AFE", "FFC700", "00C48C", "111111", "FF8FB1"],
        fx=dict(title="sticker", cover="blocks", section="badge", quote="block", closing="blocks", card="brutal",
                table="brutal", image="brutal", footer="pill", decor="none", bullets="block", stats="brutal",
                timeline="blocks", people="circle", gallery="row", chart="bold")),
    "maison": dict(
        label="Maison", hint="ivoire, capitales espacées, filets bronze, beaucoup d'air — hôtel, restaurant, mariage, immobilier, mode, vin",
        head="Times New Roman", body="Georgia", lab="Arial", head_bold=False, head_upper=True, head_track=4,
        head_size=32, body_size=19, center=True,
        bg="F3EEE6", alt="E8DFD2", text="2B2420", muted="8C7B6B", accent="8A6A3D", accent2="3E4A3D", accent3="B9A88F",
        on_accent="F3EEE6", line="8A6A3D", palette=["8A6A3D", "3E4A3D", "B9A88F", "2B2420", "C9B79C", "6B7F6A"],
        fx=dict(title="centered", cover="panel", section="roman", quote="centered", closing="monogram", card="hairline",
                table="hairline", image="frame", footer="centered", decor="none", bullets="tick", stats="hairline",
                timeline="line", people="circle", gallery="row", chart="light")),
    "rapport": dict(
        label="Rapport", hint="titres-phrases, chiffres et graphiques d'abord, sources en pied — budget, bilan, finances, conseil d'administration, KPI",
        head="Arial", body="Arial", lab="Arial", head_bold=True, head_size=28, body_size=18,
        bg="FFFFFF", alt="F1F4F8", text="1A1A1A", muted="6B7280", accent="0B3D91", accent2="00A3A1", accent3="F2A541",
        on_accent="FFFFFF", line="0B3D91", palette=["0B3D91", "00A3A1", "8DA9C4", "F2A541", "6B7280", "C0392B"],
        good="1E8E3E", bad="C0392B",
        fx=dict(title="action", cover="panel_left", section="tracker", quote="callout", closing="panel_left", card="tile",
                table="header", image="none", footer="source", decor="none", bullets="square", stats="kpi",
                timeline="chevrons", people="square", gallery="row", chart="highlight")),
    "ecole": dict(
        label="École", hint="gros caractères lisibles, surligneur, étapes numérotées, couleurs franches — cours, atelier, enfants, formation, tuto",
        head="Verdana", body="Verdana", lab="Verdana", head_bold=True, head_size=34, body_size=22,
        bg="FFFFFF", alt="EAF2FB", text="1B2A3A", muted="5B6B7B", accent="1565C0", accent2="E53935", accent3="FFD54F",
        on_accent="FFFFFF", line="C9DAEE", palette=["1565C0", "E53935", "2E7D32", "FB8C00", "6A1B9A", "00838F"],
        fx=dict(title="marker", cover="notebook", section="chapter", quote="bubble", closing="notebook", card="tab",
                table="zebra", image="none", footer="plain", decor="notebook", bullets="numbers", stats="circles",
                timeline="steps", people="circle", gallery="row", chart="big")),
    "carnet": dict(
        label="Carnet", hint="photos d'abord sur fond noir, légendes en italique, lieux en capitales — voyage, reportage, souvenirs, visite",
        head="Georgia", body="Georgia", lab="Arial", head_bold=False, head_size=40, body_size=20,
        bg="111111", alt="1E1E1E", text="F2F2F2", muted="A0A0A0", accent="F2B33D", accent2="6EC1E4", accent3="E86A5C",
        on_accent="111111", line="3A3A3A", palette=["F2B33D", "6EC1E4", "E86A5C", "9BC48A", "F2F2F2", "B49FDC"],
        fx=dict(title="place", cover="bleed", section="day", quote="mark", closing="bleed", card="rules",
                table="rules", image="none", footer="minimal", decor="none", bullets="dash", stats="rules",
                timeline="vertical", people="circle", gallery="mosaic", chart="dark")),
    "artisan": dict(
        label="Artisan", hint="papier kraft, capitales serif, doubles filets, tampons — boulangerie, commerce, ferme, artisanat, produits locaux",
        head="Georgia", body="Trebuchet MS", lab="Trebuchet MS", head_bold=True, head_size=36, body_size=21,
        bg="F4ECDC", alt="E9DCC3", text="2F2A24", muted="74685A", accent="2F5D50", accent2="C8752B", accent3="8C2F39",
        on_accent="F4ECDC", line="2F2A24", palette=["2F5D50", "C8752B", "8C2F39", "B79A5B", "2F2A24", "7D9A84"],
        fx=dict(title="label", cover="badge", section="badge_line", quote="centered", closing="badge", card="double",
                table="double", image="frame", footer="centered", decor="frame", bullets="diamond", stats="stamps",
                timeline="line", people="circle", gallery="row", chart="light")),
    "bauhaus": dict(
        label="Bauhaus", hint="formes géométriques, rouge-bleu-jaune-noir — culture, art, école de design, agence créative, affiche",
        head="Arial", body="Arial", lab="Arial", head_bold=True, head_size=40, body_size=22,
        bg="F2EBDD", alt="E6DCC8", text="1A1A1A", muted="5E584E", accent="D7262E", accent2="1F4E9E", accent3="F2B705",
        on_accent="FFFFFF", line="1A1A1A", palette=["D7262E", "1F4E9E", "F2B705", "1A1A1A", "8E8676", "E07B39"],
        fx=dict(title="geo", cover="geo", section="geo", quote="geo", closing="geo", card="solid",
                table="header", image="none", footer="grid", decor="geo", bullets="geo", stats="solid",
                timeline="numbers", people="circle", gallery="row", chart="bold")),
}

DEFAULT = "suisse"
