"""Second independent review of `page-store` (15/11, after the fixes of 3ee0bdcc8): black-box tests for what is still
wrong. Each test FAILS on 3ee0bdcc8 and shows one finding (`pytest tests_blackbox/test_zz_review2_page_store.py`)."""
from bbkit.mcp import call, give_token, text_of
from test_page_store import store_get
from test_zz_review_page_store import project_share


def test_review2_a_page_whose_reply_hermes_no_longer_lists_stays_with_the_friends(bridges, fakes):
    """FINDING R2-1 (regression of the fix for finding 5). `withdrawn_pages` withdraws every page sent before that is
    not among this sync's items — not only a deleted page. But a thread share's items come from Hermes'
    `GET /api/sessions/{id}/messages` WITHOUT `include_compacted` (read_shares.rs `messages_of`): the active rows only,
    500 at most (internal/notes/history-compaction.md). Once Hermes compacts the project's conversation (or it passes
    500 rows), the reply that holds the expense sheet is no longer listed: the page, alive at the owner's, is sent
    `gone` and erased from every friend's phone with everything they typed in it. Before this branch a page sent once
    stayed with the friends."""
    s = project_share(bridges, fakes)
    try:
        s["write"]({"op": "add", "key": "depenses", "id": "f1", "value": {"what": "Glaces", "cents": 900}})
        s["sync"]()
        assert "Glaces" in str(s["item"]()["store"])
        # Hermes compacts the conversation in place: its API now gives the summary and the kept tail only
        fakes.hermes.messages[(s["agent"], "bot-1")] = [
            {"id": 7, "role": "user", "content": "Et dimanche, on fait quoi ?"},
            {"id": 8, "role": "assistant", "content": "Une balade au bord du lac, puis le retour."},
        ]
        s["sync"]()
        # the page is still the owner's, with its data
        assert [i["value"]["what"] for i in store_get(s["bridge"], s["path"]).json()["keys"]["depenses"]["items"]] \
            == ["Essence", "Glaces"]
        left = s["item"]()
        assert left is not None and not left.get("gone"), \
            "the page was withdrawn from the friends (sent `gone`) because Hermes no longer lists its reply"
        assert "Glaces" in str(left.get("store"))
    finally:
        s["guichet"].stop()



def test_review2_a_page_whose_reply_hermes_no_longer_lists_still_brings_the_friends_lines_back(bridges, fakes):
    """FINDING R2-1, its second half: not withdrawing such a page is not enough. Its item is built only from the
    replies Hermes lists, so after the compaction it is never sent again: a friend's new line is applied at the owner's
    but never reaches the friends' sealed copy (their page shows it « En attente » for ever, the others never see it).
    A page sent before and still alive must keep going out from disk, whatever Hermes lists."""
    s = project_share(bridges, fakes)
    try:
        s["sync"]()
        fakes.hermes.messages[(s["agent"], "bot-1")] = [
            {"id": 7, "role": "user", "content": "Et dimanche, on fait quoi ?"},
            {"id": 8, "role": "assistant", "content": "Une balade au bord du lac, puis le retour."},
        ]
        s["sync"]()
        s["write"]({"op": "add", "key": "depenses", "id": "f2", "value": {"what": "Glaces", "cents": 900}})
        s["sync"]()
        assert "Glaces" in str(store_get(s["bridge"], s["path"]).json()["keys"])   # applied at the owner's
        left = s["item"]()
        assert left is not None and not left.get("gone"), "withdrawn from the friends"
        assert "Glaces" in str(left.get("store")), "the friends' copy no longer follows the sheet: Léa's line never comes back"
    finally:
        s["guichet"].stop()

SLOPPY_SHEET = """<h1>Dépenses</h1><div id="t"></div><input id="w"><button id="go">Ajouter</button>
<script>Bip.store.onChange(function(){ document.getElementById("t").innerHTML = Bip.store.items("depenses")
  .map(function(r){ return "<p>" + r.value.what + " · " + r.by + "</p>"; }).join(""); });
go.onclick = function(){ Bip.store.add("depenses", {what: w.value, cents: 0}); };</script>"""


def test_review2_a_page_that_remembers_never_draws_what_people_typed_as_html(bridge):
    """FINDING R2-5 (finding 4's residual risk, judged not acceptable for the friends). Only the description stands
    between a friend and a script in everyone's page: a page that remembers and draws the lines with innerHTML (the
    habit of most models, and of the small local ones the places run) is kept as it is, without a word. Léa then types
    `<img src=x onerror=…>` as a line: in Alex's app it writes and deletes lines AS ALEX through the door (a direct
    fetch is not held by the kit's « after a gesture »), and can draw a fake « reconnecte-toi » whose answer it writes
    in the store — that Léa reads in her own copy. The CSP stops none of it (the door is in connect-src). One guard
    that does not rely on the model is wanted: here create_page refusing such a page (or at least telling the model,
    as `no_page_in` does) — or the kit handing typed strings to the page already inert."""
    give_token(bridge)
    made = call(bridge, "create_page", {"html": SLOPPY_SHEET, "title": "Dépenses"})
    said = text_of(made)
    assert made["isError"] is True or "Bip.rows" in said or "textContent" in said, \
        f"a page drawing typed lines with innerHTML was kept silently: {said[:200]}"
