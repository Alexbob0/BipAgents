"""Re-check of `page-store` after the second review (15/11, 8d3b18fb6): black-box test for what is still wrong.
It FAILS on 8d3b18fb6 (`pytest tests_blackbox/test_zz_review3_page_store.py`)."""
from bbkit.mcp import call, give_token
from test_page_store import store_get
from test_zz_review_page_store import project_share


def test_review3_a_friend_s_line_not_written_while_the_disk_refuses_is_never_lost_silently(bridges, fakes):
    """FINDING R3-1. A friend's write that can't be written (`retry_later`: the store's file not written, « disque
    plein… » says the design note) is now kept in friends-cards.json (`retry`) and the guichet is told it was taken
    (`through: last`). But when the disk refuses writes, that file can't be written either (`update` only logs it): the
    line is gone from the guichet, from the bridge, and the store never notes it refused — Léa's page shows it « En
    attente » for as long as her app runs, then it vanishes. Here the place's disk refuses writes for one round (the two
    files the bridge would replace are made folders), then comes back."""
    s = project_share(bridges, fakes)
    try:
        state = s["bridge"].state_dir
        store = state / "agent-pages" / s["agent"] / f"{s['page']}.store.json"
        cards = state / "friends-cards.json"
        kept_store = store.read_bytes()
        kept_cards = cards.read_bytes() if cards.exists() else None
        store.unlink()
        store.mkdir()
        if kept_cards is not None:
            cards.unlink()
        cards.mkdir()
        act = s["write"]({"op": "add", "key": "depenses", "id": "f1", "value": {"what": "Glaces", "cents": 900}})
        s["sync"]()                                   # the round while the disk refuses
        store.rmdir()
        store.write_bytes(kept_store)
        cards.rmdir()
        if kept_cards is not None:
            cards.write_bytes(kept_cards)
        s["sync"]()                                   # the disk is back
        lines = [i["value"]["what"] for i in store_get(s["bridge"], s["path"]).json()["keys"]["depenses"]["items"]]
        waiting_here = cards.exists() and act in cards.read_text()
        still_at_guichet = bool(s["guichet"].actions)
        refused = act in str(store_get(s["bridge"], s["path"]).json().get("refused"))
        assert "Glaces" in lines or waiting_here or still_at_guichet or refused, \
            f"Léa's line is nowhere (store {lines}, not kept to try again, taken at the guichet, never refused)"
    finally:
        s["guichet"].stop()


ALIASED_WRITE = """<h1>Notes</h1><div id="t"></div><script>var doc = document;
Bip.store.onChange(function(){ doc.write("<p>" + Bip.store.get("note") + "</p>"); });</script>"""
TYPED_LINK = """<h1>Restos à tester</h1><ul id="l"></ul><script>Bip.store.onChange(function(){ l.textContent = "";
Bip.store.items("restos").forEach(function(r){ var a = document.createElement("a"); a.href = r.value.lien;
a.textContent = r.value.nom; var li = document.createElement("li"); li.appendChild(a); l.appendChild(li); }); });</script>"""


def test_review3_the_page_guard_sees_document_write_through_an_alias_and_a_typed_link(bridge):
    """FINDING R3-2 (low). The create_page guard (pages.rs `draws_typed_html`) reads `document.write` by its name only:
    `var doc = document; doc.write(…)` — the kit's own style — goes through. And a page that only uses textContent can
    still run a friend's text: a link whose `href` is what was typed (`javascript:…` runs in the page when Alex taps
    it, the policy's 'unsafe-inline' allows it; Android's web view never asks the app about a javascript: link)."""
    give_token(bridge)
    for source in (ALIASED_WRITE, TYPED_LINK):
        made = call(bridge, "create_page", {"html": source, "title": "Notes"})
        assert made["isError"] is True, f"kept: {source[:60]}…"
