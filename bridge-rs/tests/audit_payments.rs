//! Audit Mistral 17/11, section 2.4 (verification, then the fix): a card's allowed sites compared by raw suffix. A
//! public suffix typed as the card's site (« myshopify.com », « github.io », « co.uk ») would let the card pay on
//! every shop hosted under it: it is refused when the card is saved, in plain words; the test names the black-box
//! suites use (no known suffix: shop.example, 127.0.0.1, localhost) stay accepted.

use bipbridge::{cards, vault_match};

/// The card's site `text`, as saved: refused, or (if accepted) never covering `other`.
fn never_covers(text: &str, other: &str) {
    match cards::site_host(text) {
        Err(why) => {
            let why = why.to_string();
            assert!(why.contains("the shop's own address"), "refused in plain words: {why}");
        }
        Ok(site) => assert!(
            !cards::site_allowed(&[site], other),
            "a card allowed on « {text} » pays on « {other} », another site hosted there"
        ),
    }
}

#[test]
fn a_card_for_a_hosting_platform_pays_on_another_merchants_shop() {
    // The person types the platform's name (what they see at the end of the shop's address).
    never_covers("myshopify.com", "another-merchant.myshopify.com");
}

#[test]
fn a_card_for_github_io_pays_on_anyones_page() {
    never_covers("https://github.io", "attacker.github.io");
    never_covers("co.uk", "anyone.co.uk");
}

#[test]
fn a_shops_own_address_and_the_test_names_stay_accepted() {
    for site in ["shop-a.myshopify.com", "www.sncf-connect.com", "sncf-connect.com", "shop.example", "127.0.0.1",
                 "localhost", "http://127.0.0.1:8080"]
    {
        assert!(cards::site_host(site).is_ok(), "« {site} » refused as a card's site");
    }
    assert!(!cards::site_allowed(&[cards::site_host("shop-a.myshopify.com").unwrap()], "shop-b.myshopify.com"));
}

#[test]
fn the_vault_logins_psl_helper_tells_a_public_suffix_from_a_site() {
    // vault_match::registrable (psl crate, private section): None for a public suffix itself, the eTLD+1 otherwise.
    assert_eq!(vault_match::registrable("myshopify.com"), None);
    assert_eq!(vault_match::registrable("github.io"), None);
    assert_eq!(vault_match::registrable("co.uk"), None);
    assert_eq!(vault_match::registrable("shop-a.myshopify.com").as_deref(), Some("shop-a.myshopify.com"));
    assert_eq!(vault_match::registrable("www.sncf-connect.com").as_deref(), Some("sncf-connect.com"));
    // the test names the black-box suite uses (no known suffix) are None as well: a refusal keyed on « None » alone
    // would refuse them; the fix keys on « the host IS a known public suffix »
    assert_eq!(vault_match::registrable("shop.example"), None);
}
