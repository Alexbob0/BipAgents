//! Independent security review of vault-domain-match (`vault_login`, vault_match.rs), 16/11. Tests written first,
//! each red on f5112b75e (origin/vault-domain-match-main) until its finding is fixed. Never merged as such: the fix
//! takes them over. Fake hosts only.
//!
//! R1 (decision for Alex): without the person, a login is typed only on a host the person gave for it: the host of its
//! site, the host of its login page, and the www/bare twin of the site's host; another host of the same registrable
//! domain (a tenant's pages on a domain the Public Suffix List doesn't split, a subdomain taken over) is never chosen
//! on its own.
//! R5: a login kept for plain http fills on exactly its host (on http, or on https for that same host), never across
//! the domain.
//! Verified (green, kept as a guard): a public suffix written with non-ASCII labels (公司.cn, Chrome reports
//! `xn--55qx5d.cn`) keeps its tenants apart; the psl crate knows the list's IDN rules in their ASCII form.

use bipbridge::vault::Item;
use bipbridge::vault_match::{choose, fills_on, registrable, Choice};

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

/// R1: the university's CAS login, kept with its login page; a student's pages on another host of the same
/// domain (not in the PSL), with a look-alike login form. Never typed there without the person.
#[test]
fn r1_a_login_is_never_chosen_alone_on_another_host_of_its_domain() {
    let items = [login("cas", "https://cas.univ-example.fr", Some("https://cas.univ-example.fr/cas/login"))];
    assert_eq!(registrable("perso.univ-example.fr").as_deref(), Some("univ-example.fr"));
    // its own login page, and its site's host: as today
    assert_eq!(choose(&items, "https://cas.univ-example.fr/cas/login?service=x", None), Choice::One(&items[0]));
    // another host of the domain (user pages, a subdomain taken over): not chosen alone
    for page in [
        "https://perso.univ-example.fr/~eve/cas/login",
        "https://old-blog.univ-example.fr/login",
        "https://univ-example.fr/cas/login",
    ] {
        assert!(!matches!(choose(&items, page, None), Choice::One(_)), "{page}");
        // nor when the agent names it
        assert!(!matches!(choose(&items, page, Some("cas")), Choice::One(_)), "{page} by id");
    }
    // Alex's Amazon case stays: kept for one of www./bare, typed on the other
    let shop = [login("shop", "https://shop-example.fr", None)];
    assert_eq!(choose(&shop, "https://www.shop-example.fr/ap/signin", None), Choice::One(&shop[0]));
    let shop = [login("shop", "https://www.shop-example.fr", None)];
    assert_eq!(choose(&shop, "https://shop-example.fr/ap/signin", None), Choice::One(&shop[0]));
}

/// R5: a plain-http login (the person typed http://) is a legacy or local site: exactly its host.
#[test]
fn r5_an_http_login_fills_only_on_its_own_host() {
    assert!(fills_on("http://intranet.example.com", "http://intranet.example.com/login"));
    assert!(fills_on("http://intranet.example.com", "https://intranet.example.com/login"));
    for page in [
        "http://wiki.example.com/login",
        "http://anything.example.com/login",
        "https://www.example.com/login",
        "http://example.com/login",
    ] {
        assert!(!fills_on("http://intranet.example.com", page), "{page}");
    }
}

/// Verified: 公司.cn, 网络.cn, 個人.hk-style suffixes are public suffixes (ICANN section, written in Unicode in the list).
/// Chrome hands the bridge their ASCII form: two companies under one of them are two sites.
#[test]
fn verified_idn_public_suffixes_keep_their_tenants_apart_in_ascii() {
    // 公司.cn = xn--55qx5d.cn ; 网络.cn = xn--io0a7i.cn
    assert_eq!(registrable("alice.xn--55qx5d.cn").as_deref(), Some("alice.xn--55qx5d.cn"));
    assert!(!fills_on("https://alice.xn--55qx5d.cn", "https://bob.xn--55qx5d.cn/login"));
    assert!(!fills_on("https://alice.公司.cn", "https://bob.公司.cn/login"));
    assert!(!fills_on("https://shop-a.xn--io0a7i.cn", "https://shop-b.xn--io0a7i.cn/login"));
    // the same tenant still matches its own subdomains
    assert!(fills_on("https://alice.xn--55qx5d.cn", "https://www.alice.xn--55qx5d.cn/login"));
}
