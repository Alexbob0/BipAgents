"""Independent security review of vault-domain-match (`vault_login`, 16/11). Tests written first: each one is red on
f5112b75e (origin/vault-domain-match-main) until its finding is fixed. Fake hosts, fake identifiers, fake passwords;
the author's fake DevTools (test_vault_login.py for a login page, test_vault_logins.py for a sign-up page).

- R1: a login is typed only on a host the person gave for it (its site's host, its login page's host, the www/bare
  twin), never alone on another host of its registrable domain: a tenant's pages on a platform the Public Suffix List
  doesn't split (zendesk.com, atlassian.net, wordpress.com, sharepoint.com… are not in it), a subdomain taken over.
  Decision for Alex (he asked for « tous les liens où amazon.fr est dedans »): this test is the recommended default.
- R2: a card's number never reaches a page through the vault: the agent gives it as the identifier of an account it
  creates (`vault_create_login` `username`, never checked), then `vault_login` types it, and writes it down so the
  browser's card checks leave it in the field.
- R3: the page's own address is checked again when the fields are looked for: a page that left the login's site
  between the two (a frame of the site inside another site's page) gets nothing.
- R4: an agent that other people talk to (shared with a relative, the family's, a project's) logs in only with a
  login the person confirmed for it, as for « Coordonnées » (vault_contacts.rs `readable_by`)."""
import json
import threading
import time
from concurrent.futures import ThreadPoolExecutor

from cryptography.fernet import Fernet

import test_vault_login as vl
import test_vault_logins as vls

# a Luhn-valid 16 digits with a card network's look (the author's « client code »)
CARD = "4970101234567893"


def typed_file(bridge):
    return bridge.data / "browsers" / "vie" / "vault-typed.json"


def write_records(bridge, records):
    folder = bridge.state_dir / "vault"
    key = Fernet((folder / "vault.key").read_bytes().strip())
    (folder / "vault.json.enc").write_bytes(key.encrypt(json.dumps(records).encode()))


def point_browser_at(bridge, chrome):
    (bridge.data / "browsers" / "vie" / "ports.json").write_text(json.dumps({"cdp": chrome.port, "vnc": chrome.port + 1}))


def test_r1_a_login_is_not_typed_alone_on_another_host_of_its_domain(bridges, fakes):
    """The person keeps their company's help desk login (a tenant of a platform the PSL doesn't split). Another
    tenant's pages on the same domain show a look-alike login form: the agent is sent there (a link in an e-mail, a
    redirect) and calls vault_login as its skill says on a login page. Nothing may be typed there without the person."""
    chrome = vl.FakeChrome("https://eve.helpdesk-example.com/access/login")
    try:
        bridge = vl.login_bridge(bridges, fakes, chrome)
        kept = vl.keep(bridge, "Support Kerfy", "https://kerfy.helpdesk-example.com/access/login",
                       "support@kerfy.example", "Fake-desk-pw-41!")
        refused = vl.vault_login(bridge)
        assert refused["isError"] is True, refused
        assert chrome.typed == [] and chrome.frames[0].values == {"login": "", "mdp": ""}
        # nor when the agent names the login
        named = vl.vault_login(bridge, {"id": kept["id"]})
        assert named["isError"] is True and chrome.typed == [], named
        # its own host: as today
        chrome.frames = [vl.Frame("https://kerfy.helpdesk-example.com/access/login")]
        assert vl.vault_login(bridge)["isError"] is False
        assert chrome.frames[0].values == {"login": "support@kerfy.example", "mdp": "Fake-desk-pw-41!"}
    finally:
        chrome.stop()


def test_r2_a_card_number_the_agent_gives_never_reaches_a_page_through_the_vault(bridges, fakes):
    """The card-guards' rule (Alex, 14/11): the agent never types a card's details, whoever gave them to it. Here the
    agent got the number in the conversation and launders it through the vault: an account « created » with it as
    the username (the person sees « identifiant 4970… » on a sign-up card and says yes), then vault_login on the
    site's login page types it, and writes it down so the card checks leave it there."""
    signup = vls.FakeChrome("https://shop.example/signup")
    login = vl.FakeChrome("https://shop.example/login")
    try:
        bridge = vls.login_bridge(bridges, fakes, signup)
        with ThreadPoolExecutor(1) as pool:
            call = pool.submit(vls.tool, bridge, "vault_create_login",
                               {**vls.ARGS, "username": CARD, "reason": "Pour finir la commande"})
            deadline, card = time.time() + 5, None
            while time.time() < deadline and card is None and not call.done():
                found = vls.cards(bridge)
                card = found[0] if found else None
                time.sleep(0.1)
            if card is not None:
                # the person, trusting their agent, says yes
                bridge.post(f"/v1/vault/logins/requests/{card['request_id']}", headers=vl.AUTH, json={"allow": True})
            created = call.result(timeout=120)
        point_browser_at(bridge, login)
        logged = vl.vault_login(bridge)
        assert all(CARD not in text for _, _, text in login.typed), (created, logged, login.typed)
        assert not typed_file(bridge).exists()
        # refused before any card reaches the person (the approval would show them the number as « identifiant »)
        assert card is None and created["isError"] is True, created
    finally:
        signup.stop()
        login.stop()


def test_r2_vault_login_never_types_a_card_like_identifier_an_agent_set(bridges, fakes):
    """Defence in depth for R2, whatever way an agent set it (vault_create_login before its fix, a tool to come): the
    exemption from the card checks is for a client code the person typed in the app, never for one an agent gave."""
    chrome = vl.FakeChrome("https://shop.example/login")
    try:
        bridge = vl.login_bridge(bridges, fakes, chrome)
        vl.keep(bridge, "shop.example", "https://shop.example", "alex@example.org", "Fake-shop-pw-2!")
        records = vls.vault_records(bridge)
        records[0].update({"identifier": CARD, "identifier_type": "username", "created_by": "vie", "pending": False})
        write_records(bridge, records)
        result = vl.vault_login(bridge)
        assert all(CARD not in text for _, _, text in chrome.typed), (result, chrome.typed)
        assert not typed_file(bridge).exists()
    finally:
        chrome.stop()


class LeavingChrome(vl.FakeChrome):
    """A page on the login's site when the bridge reads its address, then (a few milliseconds later, when the bridge
    looks for the fields) another site's page holding a frame of the login's site."""

    def __init__(self, url, elsewhere):
        super().__init__(url)
        self.elsewhere = elsewhere
        self.trees = 0
        self.move = self.into_a_frame

    def into_a_frame(self):
        self.frames = [vl.Frame(self.elsewhere, identifier=False, password=False), vl.Frame(self.frames[0].url, context=2)]

    def _answer(self, method, params):
        if method == "Page.getFrameTree":
            self.trees += 1
            if self.trees == 2:
                self.move()
        return super()._answer(method, params)


def test_r3_a_page_that_left_the_logins_site_before_the_fields_were_found_gets_nothing(bridges, fakes):
    chrome = LeavingChrome(vl.PAGE_A, "https://evil.example/landing")
    try:
        bridge = vl.login_bridge(bridges, fakes, chrome)
        vl.keep(bridge, "Extranet agence A", vl.PAGE_A, vl.IDENTIFIER_A, vl.PASSWORD_A)
        result = vl.vault_login(bridge)
        assert chrome.trees >= 2, "the fake's second look never came"
        assert result["isError"] is True and chrome.typed == [], (result, chrome.typed)
    finally:
        chrome.stop()


def test_r4_an_agent_other_people_talk_to_logs_in_only_with_a_login_confirmed_for_it(bridges, fakes):
    """Vie got the login while only its person talked to it; then the person shared Vie with a relative (Léa). Léa
    can now ask Vie to log in and act on the account: as for « Coordonnées », not without the person's « Continuer »
    for this login (`confirmed_for`)."""
    chrome = vl.FakeChrome(vl.PAGE_A)
    try:
        bridge = vl.login_bridge(bridges, fakes, chrome)
        vl.keep(bridge, "Extranet agence A", vl.PAGE_A, vl.IDENTIFIER_A, vl.PASSWORD_A)
        (bridge.state_dir / "households.json").write_text(json.dumps({"households": {"lea": {"name": "Léa"}}}))
        (bridge.state_dir / "agent-shares.json").write_text(json.dumps({"shared": {"vie": ["lea"]}}))
        agents = {a["id"]: a for a in bridge.get("/v1/vault", headers=vl.AUTH).json()["agents"]}
        assert agents["vie"]["others"] is True
        result = vl.vault_login(bridge)
        assert result["isError"] is True and chrome.typed == [], (result, chrome.typed)
    finally:
        chrome.stop()


# --- confirmation (16/11, on 404cb9e39): two more, found while checking R1 against `login_url` ---------------------


def test_r8_a_login_asked_for_one_site_is_kept_for_that_site_only(bridges, fakes):
    """ask_secret (kind password, use browser) with a `site` the two address parsers read apart: the bridge's
    `normalize_origin` (Python's urlsplit: the host after the last « @ ») gives the portal, which the person's card
    shows; the `url` crate (WHATWG, as Chrome: a backslash ends the host) gives another site, which `login_url_of`
    kept, and which secrets.rs `give` then sent as the login's `site`. The person types the portal's password into a
    card that names the portal: it must be kept for the portal, never for the other site."""
    from test_connectors import PASSWORD, ask, setup, vault_records  # noqa: PLC0415
    bridge, _, _ = setup(bridges, fakes)
    site = "https://evil.example\\@accounts.portail.example/login"
    result, card = ask(bridge, {"name": "portail", "site": site, "kind": "password", "use": "browser",
                                "reason": "Récupérer les factures du mois"},
                       {"value": PASSWORD, "identifier": "compta@example.org"})
    shown = json.dumps(card, ensure_ascii=False)
    assert "accounts.portail.example" in shown, shown
    login = next(r for r in vault_records(bridge) if r.get("kind") == "login")
    assert login["origin"] == "https://accounts.portail.example", (login["origin"], login.get("login_url"), shown)
    assert "evil.example" not in json.dumps(login.get("login_url")), login.get("login_url")


def test_r9_a_page_that_moved_to_another_host_of_the_site_before_the_fields_were_found_gets_nothing(bridges, fakes):
    """R3's check, with Alex's R1: the page read on the host the login was saved for, then (when the fields are
    looked for) on another host of the same site, which the person never gave: nothing typed there either."""
    kept = "https://kerfy.helpdesk-example.com/access/login"
    chrome = LeavingChrome(kept, "https://eve.helpdesk-example.com/access/login")
    try:
        # the page itself moves (its main frame on the other host, with the form), no frame of the saved host left
        def moved(self=chrome):
            self.frames = [vl.Frame(self.elsewhere)]
        chrome.move = moved
        bridge = vl.login_bridge(bridges, fakes, chrome)
        vl.keep(bridge, "Support Kerfy", kept, "support@kerfy.example", "Fake-desk-pw-41!")
        result = vl.vault_login(bridge)
        assert chrome.trees >= 2, "the fake's second look never came"
        assert result["isError"] is True and chrome.typed == [], (result, chrome.typed)
    finally:
        chrome.stop()
