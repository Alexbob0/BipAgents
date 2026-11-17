"""The vault list's shared cases (contracts/vault-list/cases.json; passation-pro briefs/design-coffre-rassurer.md §7,
Alex's D6 A and D7 A, 16/11), read by the HermesKit and Android tests: this file is the rule written once, in plain
Python, and the check that every case's expected list follows it. The bridge's own part (`domain`, `last_used_*`) is
checked by bridge-rs (vault_used.rs `the_shared_cases_carry_the_bridges_domains`) and test_vault_login.py.

`VAULT_LIST_WRITE=1 python -m pytest tests_blackbox/test_vault_list_cases.py` rewrites the expected lists from the
rule (after a deliberate change of it: review the diff, then the apps' tests follow). Nothing here needs a bridge."""
import json
import os
import unicodedata
from datetime import datetime
from pathlib import Path
from urllib.parse import urlsplit

FILE = Path(__file__).resolve().parents[2] / "contracts" / "vault-list" / "cases.json"

CATEGORY = {"login": "logins", "card": "cards", "payment": "cards", "address": "contacts", "secret": "connectors"}
SECTIONS = ["cards", "contacts", "connectors"]   # after the letters, in this order
CHIPS = ["all", "logins", "cards", "contacts", "connectors"]
RECENT = 3
PALETTE = 7


def fold(text):
    """Lower case, accents off: Unicode NFD, every non-spacing mark (Mn) dropped, then lower case (no locale)."""
    decomposed = unicodedata.normalize("NFD", text or "")
    return "".join(c for c in decomposed if unicodedata.category(c) != "Mn").lower()


def host_of(url):
    return (urlsplit(url).hostname or "") if url else ""


def without_www(host):
    return host[4:] if host.startswith("www.") else host


def site_of(item):
    """The site a login is grouped under: the bridge's `domain`, else (an older bridge) its host without « www. »."""
    return item.get("domain") or without_www(host_of(item.get("origin"))) or None


def no_scheme(url):
    return url.split("://", 1)[1] if "://" in url else url


def page_of(item, grouped):
    """« Page : … »: only when the login page is not the site's root; in a group, the host left out (« …/V5/x »)."""
    url = item.get("login_url")
    if not url:
        return None
    parts = urlsplit(url)
    rest = parts.path + (f"?{parts.query}" if parts.query else "")
    if rest in ("", "/"):
        return None
    return f"…{rest}" if grouped else f"{parts.hostname}{rest}"


def color_of(key):
    """The square's colour, 0-6 (the palette's index): the sum of the folded key's code points, modulo 7."""
    return sum(ord(c) for c in fold(key)) % PALETTE


def initial_of(text):
    return text[:1].upper() if text else "?"


def fields_of(item):
    """What the search looks at, by kind."""
    kind = item.get("kind")
    if kind == "login":
        return [item.get("label"), host_of(item.get("origin")), item.get("identifier"),
                no_scheme(item.get("login_url") or "")]
    if kind in ("card", "payment"):
        return [item.get("label"), (item.get("card") or {}).get("last4")]
    if kind == "address":
        return [item.get("label"), (item.get("contact") or {}).get("city")]
    if kind == "secret":
        return [item.get("label"), host_of(item.get("origin")), item.get("identifier")]
    return []


def matches(item, words):
    folded = [fold(f) for f in fields_of(item) if f]
    return all(any(w in f for f in folded) for w in words)


def sort_key(item):
    return (fold(item.get("label")), fold(item.get("identifier") or ""), item["id"])


def letter_of(key):
    first = key[:1]
    return first.upper() if "a" <= first <= "z" else "#"


def logins_entries(logins, grouped_only_from):
    """Rows of these logins, a site's logins together from two (`{"group", "count", "rows"}`), sorted by the folded
    name (a group by its site)."""
    by_site = {}
    for item in logins:
        site = site_of(item)
        if site:
            by_site.setdefault(site, []).append(item)
    groups = {site: items for site, items in by_site.items() if len(items) >= grouped_only_from}
    entries = []
    for site, items in groups.items():
        rows = [{"id": i["id"]} for i in sorted(items, key=sort_key)]
        entries.append(((fold(site), "", ""), {"group": site, "count": len(rows), "rows": rows}))
    for item in logins:
        if site_of(item) not in groups:
            entries.append((sort_key(item), {"id": item["id"]}))
    entries.sort(key=lambda e: e[0])
    return entries


def view(vault, kind="all", agent=None, query=""):
    """The list as the app draws it: its chips, its sections, « rien trouvé »."""
    items = [i for i in vault["items"] if i.get("kind") in CATEGORY]
    if agent:
        items = [i for i in items if agent in (i.get("agents") or [])]
    counts = {c: 0 for c in CHIPS}
    for item in items:
        counts["all"] += 1
        counts[CATEGORY[item["kind"]]] += 1
    chips = [{"key": c, "count": counts[c]} for c in CHIPS if c == "all" or counts[c] > 0]
    if kind != "all":
        items = [i for i in items if CATEGORY[i["kind"]] == kind]
    words = fold(query).split()
    logins = [i for i in items if i["kind"] == "login"]
    others = {s: sorted([i for i in items if CATEGORY[i["kind"]] == s], key=sort_key) for s in SECTIONS}
    if words:
        logins = [i for i in logins if matches(i, words)]
        others = {s: [i for i in found if matches(i, words)] for s, found in others.items()}
        rows = [row for _, row in logins_entries(logins, 2)]
        rows += [{"id": i["id"]} for s in SECTIONS for i in others[s]]
        count = len(logins) + sum(len(v) for v in others.values())
        sections = [{"key": "results", "count": count, "rows": rows}] if count else []
        return {"chips": chips, "sections": sections, "nothing_found": count == 0}
    sections = []
    used = [i for i in logins if i.get("last_used_at")]
    used.sort(key=lambda i: (datetime.fromisoformat(i["last_used_at"]), i["id"]), reverse=True)
    if used[:RECENT]:
        sections.append({"key": "recent", "rows": [{"id": i["id"], "used_by": i.get("last_used_by"),
                                                   "used_at": i["last_used_at"]} for i in used[:RECENT]]})
    letters = {}
    for key, row in logins_entries(logins, 2):
        letters.setdefault(letter_of(key[0]), []).append(row)
    for letter in sorted(letters, key=lambda l: (l == "#", l)):
        sections.append({"key": "letter", "letter": letter, "rows": letters[letter]})
    for section in SECTIONS:
        if others[section]:
            sections.append({"key": section, "rows": [{"id": i["id"]} for i in others[section]]})
    return {"chips": chips, "sections": sections, "nothing_found": False}


def for_menu(vault):
    """« Pour : tous ▾ »'s agents: those that hold at least one item, by their folded name."""
    holding = {a for i in vault["items"] for a in (i.get("agents") or [])}
    agents = [a for a in vault["agents"] if a["id"] in holding]
    return [a["id"] for a in sorted(agents, key=lambda a: (fold(a.get("name") or a["id"]), a["id"]))]


def looks(vault):
    """Each login's square (initial of its name, colour of its site) and « Page : … » (alone, in a group); each
    site's group's square."""
    out, groups = {}, {}
    for item in vault["items"]:
        if item.get("kind") != "login":
            continue
        site = site_of(item)
        out[item["id"]] = {"initial": initial_of(item.get("label")), "color": color_of(site or item.get("label") or ""),
                           "page": page_of(item, False), "page_grouped": page_of(item, True)}
        if site:
            groups[site] = {"initial": initial_of(site), "color": color_of(site)}
    return out, groups


def expected(doc):
    out = {"fold": [[text, fold(text)] for text, _ in doc["fold"]], "looks": {}, "group_looks": {}, "for_menu": {},
           "cases": []}
    for name, vault in doc["vaults"].items():
        out["looks"][name], out["group_looks"][name] = looks(vault)
        out["for_menu"][name] = for_menu(vault)
    for case in doc["cases"]:
        f = case["filter"]
        out["cases"].append({**case, "expect": view(doc["vaults"][case["vault"]], f["kind"], f["for"], f["query"])})
    return out


def test_every_case_follows_the_rule():
    doc = json.loads(FILE.read_text(encoding="utf-8"))
    want = expected(doc)
    if os.environ.get("VAULT_LIST_WRITE") == "1":
        doc.update(want)
        FILE.write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    for key in ("fold", "looks", "group_looks", "for_menu"):
        assert doc[key] == want[key], key
    assert len(doc["cases"]) == len(want["cases"])
    for got, rule in zip(doc["cases"], want["cases"]):
        assert got == rule, got["name"]


def test_the_cases_cover_what_the_design_asks():
    """Sorting with accents and capitals, letters and groups, search on every field, « Pour », chips, « rien
    trouvé », an older bridge."""
    doc = json.loads(FILE.read_text(encoding="utf-8"))
    by_name = {c["name"]: c["expect"] for c in doc["cases"]}
    first = by_name["everything, as the vault opens"]
    assert [s.get("letter") for s in first["sections"] if s["key"] == "letter"] == list("ABCEFGMNRTZ#")
    assert first["sections"][0]["key"] == "recent" and len(first["sections"][0]["rows"]) == RECENT
    assert [s["key"] for s in first["sections"][-3:]] == ["cards", "contacts", "connectors"]
    assert any("group" in r for s in first["sections"] for r in s["rows"])
    assert [c["key"] for c in by_name["Pour : Maison (no card, no connector: their chips go)"]["chips"]] == ["all", "logins", "contacts"]
    assert by_name["nothing found"]["nothing_found"] is True and by_name["nothing found"]["sections"] == []
    assert by_name["search without accents finds « école »"] == by_name["search in capitals with accents"]
    older = by_name["an older bridge: grouped by host without www., no « Récents »"]
    assert all(s["key"] != "recent" for s in older["sections"])
    assert "fournisseur.example" not in json.dumps(older)   # one login on its site: no group
    assert doc["palette"] and len(doc["palette"]) == PALETTE


def test_the_edges_the_two_apps_must_agree_on():
    """Review of 17/11: what Swift (graphemes, `first`), Kotlin (UTF-16 `Char`, `compareTo`, `\\s`) and Python would
    each do their own way, pinned by hand: the initial is the first letter or digit by code point, the name trimmed and
    in NFC; keys compare by code point; equal « Récents » by id; « Page : » as written with its port; a no-break space
    splits a search; a chosen chip gone with « Pour » falls back to « Tout »; an empty name is « ? »."""
    doc = json.loads(FILE.read_text(encoding="utf-8"))
    by_name = {c["name"]: c["expect"] for c in doc["cases"]}
    vault = doc["vaults"]["edges"]
    label = {i["id"]: i["label"] for i in vault["items"]}
    look = doc["looks"]["edges"]
    initials = {label[i]: v["initial"] for i, v in look.items()}
    assert initials["🎵 Musique Plus"] == "M" and initials["E\u0301cole des arts"] == "É"
    assert initials["Ｚone Libre"] == "Ｚ" and initials["𝒜rt Studio"] == "𝒜" and initials["  Zèbre Café "] == "Z"
    assert initials[""] == "?" and initials["***"] == "?"
    first = by_name["edges: first characters, code points, empty names, ties"]
    assert first["kind"] == "all"
    recent = [s for s in first["sections"] if s["key"] == "recent"][0]["rows"]
    assert [label[r["id"]] for r in recent] == ["Banque du temps A", "Banque du temps B"], "equal times: by id"
    letters = {s["letter"]: [label[r["id"]] for r in s["rows"]] for s in first["sections"] if s["key"] == "letter"}
    assert letters["E"] == ["E\u0301cole des arts"] and letters["Z"] == ["  Zèbre Café "]
    # code points: the fullwidth Ｚ (U+FF3A) before the script 𝒜 (U+1D49C) before the emoji (U+1F3B5); in UTF-16
    # units the order would be 𝒜, 🎵, Ｚ
    assert letters["#"] == ["", "***", "Ｚone Libre", "𝒜rt Studio", "🎵 Musique Plus"], letters["#"]
    page = [v for i, v in look.items() if label[i] == "Intranet"][0]
    assert page["page"] == "intra.exemple-port.example:8443/Mon%20Espace/connexion?retour=%2Faccueil"
    assert page["page_grouped"] == "…/Mon%20Espace/connexion?retour=%2Faccueil"
    found = by_name["edges: a search split on a no-break space"]["sections"][0]["rows"]
    assert [label[r["id"]] for r in found] == ["🎵 Musique Plus"]
    gone = by_name["edges: a chosen chip gone with « Pour » falls back to « Tout »"]
    assert gone["kind"] == "all" and [c["key"] for c in gone["chips"]] == ["all", "logins"]
    assert [label[r["id"]] for s in gone["sections"] for r in s["rows"] if "id" in r] == ["Banque du temps A", "Banque du temps A"]
    assert by_name["edges: a login page with a port and an encoded path"]["sections"][0]["count"] == 1
