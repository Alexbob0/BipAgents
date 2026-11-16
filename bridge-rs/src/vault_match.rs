//! Where a saved login may be filled, and which one fits the agent's page (`vault_login`, vault_logins.rs).
//!
//! Alex, 16/11: « je veux que l'on puisse se connecter à Amazon sur tous les liens où amazon.fr est dedans ». A login
//! is kept for an origin (`https://www.amazon.fr`, vault.rs `normalize_origin`) and fills on any page of that origin's
//! **registrable domain**: the host and its subdomains (`amazon.fr`, `www.amazon.fr`, `smile.amazon.fr`), never
//! another domain (`amazon.com`, `amazon.fr.evil.com`, `amaz0n.fr`). The registrable domain (eTLD+1) comes from the
//! Public Suffix List, private section included (`psl` crate, the list compiled in): `alice.github.io` and
//! `bob.github.io`, two shops on `myshopify.com`, stay two sites. Hosts are compared in their ASCII form (IDNA: what
//! Chrome reports), so a look-alike in another script (`аmazon.fr`, Cyrillic а) is another domain.
//!
//! Exact host only: an IP address, a one-label name (`localhost`), a host that is itself a public suffix
//! (`github.io`), a suffix the list doesn't know (`extranet.corp`). A port other than the scheme's default, on either
//! side: exactly that host and port (another port can be another service).
//!
//! Scheme: a login kept for `https://` fills on https pages only, never on plain http, where anyone on the network
//! can answer for the site. One kept for `http://` (the person typed it so) fills on http and on https pages of its
//! domain: the same site, better protected.
//!
//! Several logins on one domain (an extranet several agencies share, 16/11: each its own login page under
//! `extranet2.ics.fr/V5/…`): the one whose login page (`login_url`) is the page's own path, else the longest path
//! prefix of it, else the one kept for the page's exact origin. Still several: never a guess, the agent names one
//! (`id`), the person telling it which when the names don't.

use crate::vault::Item;

/// A web address's site, as the vault compares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub scheme: String,
    /// ASCII (IDNA), lower case, no trailing dot; an IPv6 address in brackets.
    pub host: String,
    /// A port other than the scheme's default.
    pub port: Option<u16>,
    /// Its registrable domain (`registrable`), None: exact host only.
    pub domain: Option<String>,
    /// Its path's segments (empty ones dropped).
    pub path: Vec<String>,
}

/// The site of an http(s) address with its scheme (an origin, a page, a frame); None for anything else.
pub fn site_of(url: &str) -> Option<Site> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    let scheme = parsed.scheme().to_string();
    if scheme != "https" && scheme != "http" {
        return None;
    }
    let (host, ip) = match parsed.host()? {
        url::Host::Domain(d) => (d.trim_end_matches('.').to_ascii_lowercase(), false),
        url::Host::Ipv4(a) => (a.to_string(), true),
        url::Host::Ipv6(a) => (format!("[{a}]"), true),
    };
    if host.is_empty() {
        return None;
    }
    let domain = if ip { None } else { registrable(&host) };
    let path = parsed.path().split('/').filter(|s| !s.is_empty()).map(str::to_string).collect();
    Some(Site { scheme, host, port: parsed.port(), domain, path })
}

/// The registrable domain of an ASCII host name (eTLD+1, Public Suffix List with its private section): None for a
/// one-label name, a host that is a public suffix itself, or one under a suffix the list doesn't know.
pub fn registrable(host: &str) -> Option<String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if !host.contains('.') || host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let domain = psl::domain(host.as_bytes())?;
    if !domain.suffix().is_known() {
        return None;
    }
    std::str::from_utf8(domain.as_bytes()).ok().map(str::to_string)
}

/// Whether a login kept for `origin` may be filled on `page` (a page's or a frame's address, Chrome's own).
pub fn fills_on(origin: &str, page: &str) -> bool {
    let (Some(kept), Some(here)) = (site_of(origin), site_of(page)) else { return false };
    same_site(&kept, &here)
}

fn same_site(kept: &Site, here: &Site) -> bool {
    let scheme_ok = here.scheme == kept.scheme || (kept.scheme == "http" && here.scheme == "https");
    if !scheme_ok {
        return false;
    }
    // a port, or a login kept for plain http (review R5): exactly that host
    if kept.port.is_some() || here.port.is_some() || kept.scheme == "http" {
        return kept.host == here.host && kept.port == here.port;
    }
    match (&kept.domain, &here.domain) {
        (Some(a), Some(b)) => a == b,
        _ => kept.host == here.host,
    }
}

/// What `choose` found for the agent's page.
#[derive(Debug, PartialEq)]
pub enum Choice<'a> {
    /// The page is not a web page.
    NotWeb,
    /// No login of the agent's fills on this page.
    Nothing,
    /// The one to fill.
    One(&'a Item),
    /// Several fit as well as each other: the agent names one (`id`).
    Several(Vec<&'a Item>),
    /// The `id` asked for is a login of the agent's kept for another site.
    NotHere(&'a Item),
    /// No login of the agent's has the `id` asked for.
    Unknown,
    /// The logins fit the page's domain, but the page is on a host the person never gave for any of them (review
    /// R1, Alex's choice of 16/11): nothing typed there; the person saves this page for a login, or logs in.
    OtherHost(Vec<&'a Item>),
}

/// Whether the page is on a host the person gave for the login (review R1): its site's, its login page's, or the
/// www/bare twin of either, with the same port.
fn own_host(item: &Item, here: &Site) -> bool {
    let bare = |h: &str| h.strip_prefix("www.").unwrap_or(h).to_string();
    [item.origin.as_deref(), item.login_url.as_deref()]
        .into_iter()
        .flatten()
        .filter_map(site_of)
        .any(|s| s.port == here.port && bare(&s.host) == bare(&here.host))
}

/// How well a login fits the page: its login page's path (the page's own: above any prefix; a longer prefix above a
/// shorter one; the site's root says nothing), then whether it was kept for the page's exact origin.
fn rank(item: &Item, here: &Site) -> (usize, bool) {
    let exact_origin = |url: &str| {
        site_of(url).is_some_and(|s| s.scheme == here.scheme && s.host == here.host && s.port == here.port)
    };
    let path = item
        .login_url
        .as_deref()
        .filter(|u| exact_origin(u))
        .and_then(site_of)
        .map(|login| {
            let n = login.path.len();
            if n > 0 && login.path == here.path {
                usize::MAX
            } else if n > 0 && here.path.starts_with(&login.path) {
                n
            } else {
                0
            }
        })
        .unwrap_or(0);
    (path, item.origin.as_deref().is_some_and(exact_origin))
}

/// The login of `items` (the agent's: only logins are looked at) to fill on `page`, or `id`'s.
pub fn choose<'a>(items: &'a [Item], page: &str, id: Option<&str>) -> Choice<'a> {
    let Some(here) = site_of(page) else { return Choice::NotWeb };
    let logins: Vec<&Item> = items.iter().filter(|i| i.kind == "login").collect();
    let fits = |i: &Item| i.origin.as_deref().and_then(site_of).is_some_and(|kept| same_site(&kept, &here));
    if let Some(id) = id.map(str::trim).filter(|s| !s.is_empty()) {
        return match logins.iter().find(|i| i.id == id) {
            Some(item) if fits(item) && own_host(item, &here) => Choice::One(item),
            Some(item) if fits(item) => Choice::OtherHost(vec![item]),
            Some(item) => Choice::NotHere(item),
            None => Choice::Unknown,
        };
    }
    let fitting: Vec<&Item> = logins.into_iter().filter(|i| fits(i)).collect();
    if fitting.is_empty() {
        return Choice::Nothing;
    }
    // only the logins the person gave this host for are ever typed here: the choice by login page is among them
    let given: Vec<&Item> = fitting.iter().copied().filter(|i| own_host(i, &here)).collect();
    match given.len() {
        0 => return Choice::OtherHost(fitting),
        1 => return Choice::One(given[0]),
        _ => {}
    }
    let best = given.iter().map(|i| rank(i, &here)).max().unwrap_or((0, false));
    let top: Vec<&Item> = given.iter().copied().filter(|i| rank(i, &here) == best).collect();
    if top.len() == 1 && best != (0, false) {
        return Choice::One(top[0]);
    }
    Choice::Several(if best == (0, false) { given } else { top })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(id: &str, origin: &str, login_url: Option<&str>) -> Item {
        Item {
            id: id.into(),
            kind: "login".into(),
            label: id.into(),
            origin: Some(origin.into()),
            login_url: login_url.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn a_login_fills_on_its_domain_and_its_subdomains() {
        for kept in ["https://amazon.fr", "https://www.amazon.fr", "https://smile.amazon.fr"] {
            for page in [
                "https://amazon.fr/",
                "https://www.amazon.fr/ap/signin?openid.return_to=x",
                "https://smile.amazon.fr/gp/cart",
                "https://a.b.amazon.fr/x",
                "https://WWW.Amazon.FR./x",
            ] {
                assert!(fills_on(kept, page), "{kept} on {page}");
            }
        }
    }

    #[test]
    fn never_on_another_domain_nor_a_look_alike() {
        let kept = "https://www.amazon.fr";
        for page in [
            "https://amazon.com/",
            "https://www.amazon.de/",
            "https://amazon.fr.evil.com/",
            "https://www.amazon.fr.evil.com/ap/signin",
            "https://amazon-fr.com/",
            "https://amaz0n.fr/",
            "https://evil.com/amazon.fr",
            "https://evil.com/?u=https://www.amazon.fr",
            "https://www.amazon.fr@evil.com/",
            "https://fr/",
            "www.amazon.fr",
            "mailto:x@www.amazon.fr",
            "about:blank",
            "data:text/html,amazon.fr",
            "javascript:alert(1)",
            "",
        ] {
            assert!(!fills_on(kept, page), "{page}");
        }
    }

    #[test]
    fn private_suffixes_keep_their_tenants_apart() {
        assert!(!fills_on("https://alice.github.io", "https://bob.github.io/login"));
        assert!(fills_on("https://alice.github.io", "https://alice.github.io/login"));
        assert!(!fills_on("https://shop-a.myshopify.com", "https://shop-b.myshopify.com/account/login"));
        assert!(!fills_on("https://a.herokuapp.com", "https://b.herokuapp.com/"));
        assert!(!fills_on("https://x.blogspot.com", "https://y.blogspot.com/"));
        // the suffix itself is no one's site: exactly that host
        assert!(fills_on("https://github.io", "https://github.io/"));
        assert!(!fills_on("https://github.io", "https://alice.github.io/"));
        // two-label public suffixes (ICANN)
        assert!(!fills_on("https://bank.co.uk", "https://evil.co.uk/"));
        assert!(fills_on("https://bank.co.uk", "https://online.bank.co.uk/"));
        assert!(!fills_on("https://mairie.gouv.fr", "https://autre.gouv.fr/"));
    }

    #[test]
    fn the_shared_extranet_is_one_domain() {
        assert_eq!(registrable("extranet2.ics.fr").as_deref(), Some("ics.fr"));
        assert!(fills_on("https://extranet2.ics.fr", "https://extranet2.ics.fr/V5/connexion-agence-a.html"));
        assert!(fills_on("https://extranet2.ics.fr", "https://extranet1.ics.fr/V5/x.html"));
        assert!(!fills_on("https://extranet2.ics.fr", "https://extranet2.ics.fr.example/V5/x.html"));
    }

    #[test]
    fn hosts_are_compared_in_their_ascii_form() {
        // what Chrome reports for bücher.de
        assert!(fills_on("https://bücher.de", "https://www.xn--bcher-kva.de/login"));
        assert!(fills_on("https://xn--bcher-kva.de", "https://bücher.de/login"));
        // a Cyrillic « а » in amazon: another domain
        assert!(!fills_on("https://amazon.fr", "https://xn--mazon-3ve.fr/"));
        assert!(!fills_on("https://amazon.fr", "https://аmazon.fr/"));
        assert_eq!(registrable("www.xn--bcher-kva.de").as_deref(), Some("xn--bcher-kva.de"));
    }

    #[test]
    fn a_port_means_exactly_that_host_and_port() {
        assert!(fills_on("https://intranet.example.com:8443", "https://intranet.example.com:8443/login"));
        assert!(!fills_on("https://intranet.example.com:8443", "https://intranet.example.com/login"));
        assert!(!fills_on("https://intranet.example.com:8443", "https://www.example.com:8443/login"));
        assert!(!fills_on("https://example.com", "https://www.example.com:8443/login"));
        assert!(fills_on("https://example.com:443", "https://www.example.com/login"));
        assert!(fills_on("http://127.0.0.1:8080", "http://127.0.0.1:8080/V5/x.html"));
        assert!(!fills_on("http://127.0.0.1:8080", "http://127.0.0.1:8081/V5/x.html"));
    }

    #[test]
    fn ip_addresses_and_local_names_are_exact_hosts() {
        assert_eq!(registrable("127.0.0.1"), None);
        assert_eq!(registrable("localhost"), None);
        assert!(fills_on("http://10.0.0.1", "http://10.0.0.1/login"));
        assert!(!fills_on("http://10.0.0.1", "http://10.0.0.2/login"));
        assert!(fills_on("http://localhost", "http://localhost/login"));
        assert!(!fills_on("http://localhost", "http://a.localhost/login"));
        // a suffix the list doesn't know: no domain guessed
        assert_eq!(registrable("extranet.corp-internal-tld"), None);
        assert!(!fills_on("https://a.extranet.corp-internal-tld", "https://b.extranet.corp-internal-tld/"));
    }

    #[test]
    fn https_never_falls_back_to_http_and_http_moves_up_to_https() {
        assert!(!fills_on("https://www.amazon.fr", "http://www.amazon.fr/ap/signin"));
        assert!(fills_on("http://old.example.com", "http://old.example.com/login"));
        assert!(fills_on("http://old.example.com", "https://old.example.com/login"));
        // review R5: an http login, exactly its host
        assert!(!fills_on("http://old.example.com", "https://www.example.com/login"));
        assert!(!fills_on("http://old.example.com", "ftp://old.example.com/"));
    }

    #[test]
    fn an_old_login_kept_as_an_origin_fills_on_its_domain() {
        let items = [login("item_old", "https://www.amazon.fr", None)];
        assert_eq!(choose(&items, "https://www.amazon.fr/ap/signin?x=1", None), Choice::One(&items[0]));
        assert_eq!(choose(&items, "https://amazon.fr/", None), Choice::One(&items[0]));
        assert_eq!(choose(&items, "https://amazon.com/", None), Choice::Nothing);
    }

    #[test]
    fn two_agencies_on_one_extranet_are_told_apart_by_their_login_page() {
        let items = [
            login("item_a", "https://extranet2.ics.fr", Some("https://extranet2.ics.fr/V5/connexion-agence-a.html")),
            login("item_b", "https://extranet2.ics.fr", Some("https://extranet2.ics.fr/V5/connexion-agence-b.html")),
            login("item_shop", "https://www.amazon.fr", None),
        ];
        let page_a = "https://extranet2.ics.fr/V5/connexion-agence-a.html";
        assert_eq!(choose(&items, page_a, None), Choice::One(&items[0]));
        assert_eq!(choose(&items, &format!("{page_a}?retour=1#haut"), None), Choice::One(&items[0]));
        assert_eq!(choose(&items, "https://extranet2.ics.fr/V5/connexion-agence-b.html", None), Choice::One(&items[1]));
        // a page that is neither's login page: never a guess
        match choose(&items, "https://extranet2.ics.fr/V5/accueil.html", None) {
            Choice::Several(found) => assert_eq!(found.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["item_a", "item_b"]),
            other => panic!("{other:?}"),
        }
        // the agent names one
        assert_eq!(choose(&items, "https://extranet2.ics.fr/V5/accueil.html", Some("item_b")), Choice::One(&items[1]));
        assert_eq!(choose(&items, page_a, Some("item_shop")), Choice::NotHere(&items[2]));
        assert_eq!(choose(&items, page_a, Some("item_nope")), Choice::Unknown);
    }

    #[test]
    fn the_longest_path_prefix_wins_and_the_root_says_nothing() {
        let items = [
            login("root", "https://portal.example.com", Some("https://portal.example.com/")),
            login("team", "https://portal.example.com", Some("https://portal.example.com/teams/blue/")),
            login("deep", "https://portal.example.com", Some("https://portal.example.com/teams/blue/admin/")),
        ];
        assert_eq!(choose(&items, "https://portal.example.com/teams/blue/admin/login", None), Choice::One(&items[2]));
        assert_eq!(choose(&items, "https://portal.example.com/teams/blue/login", None), Choice::One(&items[1]));
        // a path that merely starts with the same letters is no prefix
        assert!(matches!(choose(&items, "https://portal.example.com/teams/blueberry/login", None), Choice::Several(_)));
        assert!(matches!(choose(&items, "https://portal.example.com/", None), Choice::Several(_)));
    }

    #[test]
    fn a_login_page_on_another_host_of_the_domain_says_nothing_of_the_path() {
        let items = [
            login("a", "https://extranet1.example.fr", Some("https://extranet1.example.fr/V5/login.html")),
            login("b", "https://extranet2.example.fr", Some("https://extranet2.example.fr/V5/other.html")),
        ];
        // same path, other host: not a's login page; b is kept for this exact origin
        assert_eq!(choose(&items, "https://extranet2.example.fr/V5/login.html", None), Choice::One(&items[1]));
    }

    #[test]
    fn the_choice_is_only_among_the_logins_given_for_this_host() {
        // Alex's choice of 16/11 (review R1): typed without the person only on the hosts it was saved for
        let items = [
            login("a", "https://extranet2.ics-example.fr", Some("https://extranet2.ics-example.fr/V5/a.html")),
            login("b", "https://extranet2.ics-example.fr", Some("https://extranet2.ics-example.fr/V5/b.html")),
            login("other", "https://extranet1.ics-example.fr", Some("https://extranet1.ics-example.fr/V5/a.html")),
        ];
        assert_eq!(choose(&items, "https://extranet2.ics-example.fr/V5/a.html", None), Choice::One(&items[0]));
        // a tie on this host names only this host's logins, never the one kept for another host
        match choose(&items, "https://extranet2.ics-example.fr/V5/accueil.html", None) {
            Choice::Several(found) => assert_eq!(found.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), ["a", "b"]),
            other => panic!("{other:?}"),
        }
        // the only login given for this host, whatever the others on the domain
        assert_eq!(choose(&items, "https://extranet1.ics-example.fr/V5/z.html", None), Choice::One(&items[2]));
        // a host of the domain none was given for: each one named, none typed, even by its id
        match choose(&items, "https://extranet3.ics-example.fr/V5/a.html", None) {
            Choice::OtherHost(found) => assert_eq!(found.len(), 3),
            other => panic!("{other:?}"),
        }
        assert_eq!(choose(&items, "https://extranet3.ics-example.fr/V5/a.html", Some("a")), Choice::OtherHost(vec![&items[0]]));
        // the www twin of a host given counts as given
        let shop = [login("shop", "https://www.amazon.fr", Some("https://www.amazon.fr/ap/signin"))];
        assert_eq!(choose(&shop, "https://amazon.fr/ap/signin", None), Choice::One(&shop[0]));
        assert_eq!(choose(&shop, "https://smile.amazon.fr/ap/signin", None), Choice::OtherHost(vec![&shop[0]]));
    }

    #[test]
    fn the_exact_origin_breaks_a_tie_on_one_domain() {
        let items = [
            login("www", "https://www.example.com", None),
            login("business", "https://business.example.com", None),
        ];
        assert_eq!(choose(&items, "https://business.example.com/login", None), Choice::One(&items[1]));
        // the bare host is www.'s twin: given for « www » only (R1)
        assert_eq!(choose(&items, "https://example.com/login", None), Choice::One(&items[0]));
        assert!(matches!(choose(&items, "https://shop.example.com/login", None), Choice::OtherHost(v) if v.len() == 2));
    }

    #[test]
    fn two_accounts_on_one_page_are_never_guessed() {
        let items = [
            login("perso", "https://www.amazon.fr", Some("https://www.amazon.fr/ap/signin")),
            login("pro", "https://www.amazon.fr", Some("https://www.amazon.fr/ap/signin")),
        ];
        assert!(matches!(choose(&items, "https://www.amazon.fr/ap/signin", None), Choice::Several(v) if v.len() == 2));
        assert_eq!(choose(&items, "https://www.amazon.fr/ap/signin", Some("pro")), Choice::One(&items[1]));
    }

    #[test]
    fn only_logins_are_chosen_and_only_web_pages() {
        let mut card = login("card", "https://www.amazon.fr", None);
        card.kind = "card".into();
        let items = [card];
        assert_eq!(choose(&items, "https://www.amazon.fr/", None), Choice::Nothing);
        assert_eq!(choose(&items, "about:blank", None), Choice::NotWeb);
        assert_eq!(choose(&items, "chrome://settings", None), Choice::NotWeb);
    }
}
