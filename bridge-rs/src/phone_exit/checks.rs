//! The phone exit's pure checks (docs/phone-exit.md §3, §5, §6): names, addresses, sites, the minimal route, the
//! ClientHello's server name and the tunnel's frames. The phone runs the same checks again and has the last word.

use std::net::IpAddr;

pub const T_HELLO: u8 = 1;
pub const T_OPEN: u8 = 2;
pub const T_OPEN_OK: u8 = 3;
pub const T_OPEN_ERR: u8 = 4;
pub const T_DATA: u8 = 5;
pub const T_WINDOW: u8 = 6;
pub const T_FIN: u8 = 7;
pub const T_RST: u8 = 8;
pub const T_PAUSE: u8 = 9;
pub const T_RESUME: u8 = 10;
pub const T_STATS: u8 = 11;
pub const T_BYE: u8 = 12;
pub const T_WELCOME: u8 = 13;
pub const T_SESSIONS: u8 = 14;
pub const T_END: u8 = 15;
pub const T_USAGE: u8 = 16;

/// The protocol's version (docs/phone-exit.md).
pub const VERSION: u64 = 1;
/// Each side may have this many bytes of a stream in flight before the other's `WINDOW`.
pub const WINDOW: u32 = 256 * 1024;
/// A `DATA` frame's payload at most.
pub const CHUNK: usize = 16 * 1024;
/// A frame's payload at most (anything bigger ends the tunnel).
pub const MAX_PAYLOAD: usize = 64 * 1024;

/// The first label under a site that marks a host of static files, sent direct by the minimal route.
pub const STATIC_LABELS: [&str; 18] = [
    "assets", "static", "statics", "cdn", "img", "images", "image", "media", "medias", "fonts", "font", "css", "js",
    "pics", "photos", "thumbs", "video", "videos",
];

const LOCAL_SUFFIXES: [&str; 9] =
    ["localhost", "local", "internal", "lan", "home", "arpa", "test", "invalid", "localdomain"];

/// Why a requested name is refused before anything (None: a public-looking name). Codes of docs/phone-exit.md §6.
pub fn name_refused(host: &str) -> Option<&'static str> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if bare.parse::<IpAddr>().is_ok() || looks_numeric(host) {
        return Some("literal_ip");
    }
    if host.len() > 253
        || host.is_empty()
        || !host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
        || host.starts_with('.')
        || host.contains("..")
    {
        return Some("bad_name");
    }
    let last = host.rsplit('.').next().unwrap_or("");
    if !host.contains('.')
        || LOCAL_SUFFIXES.contains(&last)
        || !(last.bytes().all(|b| b.is_ascii_alphabetic()) || last.starts_with("xn--"))
    {
        return Some("local_name");
    }
    None
}

/// `10.1`, `0x7f.1`, `2130706433`: forms some resolvers take as an IPv4 address.
fn looks_numeric(host: &str) -> bool {
    let last = host.rsplit('.').next().unwrap_or("");
    !last.is_empty() && (last.bytes().all(|b| b.is_ascii_digit()) || last.starts_with("0x"))
}

/// An address of the internet: not private, loopback, link-local, shared (100.64/10), multicast, reserved, nor an
/// IPv4 wrapped in IPv6 (NAT64, its local-use prefix, 6to4: refused whole), nor an IPv4-mapped address of these.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.is_documentation()
                || o[0] == 0
                || (o[0] == 100 && (o[1] & 0xc0) == 64)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                || o[0] >= 240)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(v4));
            }
            let s = v6.segments();
            let nat64 = s[0] == 0x64 && s[1] == 0xff9b && (s[2..6].iter().all(|&x| x == 0) || s[2] == 1);
            let six_to_four = s[0] == 0x2002;
            let unique_local = (s[0] & 0xfe00) == 0xfc00;
            let link_local = (s[0] & 0xffc0) == 0xfe80;
            let global_unicast = (s[0] & 0xe000) == 0x2000;
            let documentation = s[0] == 0x2001 && s[1] == 0xdb8;
            let teredo_orchid = s[0] == 0x2001 && s[1] < 0x200;
            global_unicast
                && !(nat64 || six_to_four || unique_local || link_local || documentation || teredo_orchid)
                && !v6.is_loopback()
                && !v6.is_unspecified()
        }
    }
}

/// A host as given (`https://www.ameli.fr/assure`, `WWW.Ameli.FR.`, `ameli.fr:443`) made plain.
pub fn host_of(raw: &str) -> String {
    let mut host = raw.trim().to_lowercase();
    if let Some((_, rest)) = host.split_once("://") {
        host = rest.to_string();
    }
    let host = host.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let host = match host.rsplit_once(':') {
        Some((h, port)) if !h.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    };
    host.trim_end_matches('.').to_string()
}

/// The site (registrable domain) a host or URL belongs to: `www.ameli.fr` → `ameli.fr`, `x.impots.gouv.fr` →
/// `impots.gouv.fr`. Err: the refusal code of a name that can never go through the phone.
pub fn site_of(raw: &str) -> Result<String, &'static str> {
    let host = host_of(raw);
    if let Some(why) = name_refused(&host) {
        return Err(why);
    }
    Ok(crate::mail_tidy::registrable(&host))
}

/// The host is the site or one of its subdomains.
pub fn covers(site: &str, host: &str) -> bool {
    host == site || (host.len() > site.len() + 1 && host.ends_with(site) && host.as_bytes()[host.len() - site.len() - 1] == b'.')
}

/// A host of static files under the site (`assets.doctolib.fr`, `x.img.leboncoin.fr`): the minimal route sends it
/// direct. The site itself and `www` never are.
pub fn is_static(site: &str, host: &str) -> bool {
    if !covers(site, host) || host == site {
        return false;
    }
    let under = &host[..host.len() - site.len() - 1];
    let label = under.rsplit('.').next().unwrap_or("");
    STATIC_LABELS.contains(&label)
}

/// What the first bytes of a stream say about its TLS server name.
#[derive(Debug, PartialEq, Eq)]
pub enum Sni {
    NeedMore,
    Name(String),
    NoName,
    NotTls,
    /// The client closed before sending anything (Chrome's preconnects): not a refusal.
    Closed,
}

/// The ClientHello's server name, from the stream's first bytes (one handshake record; a ClientHello split over
/// several records is not something browsers send, and is refused as `NotTls`).
pub fn client_hello_sni(buf: &[u8]) -> Sni {
    if buf.is_empty() {
        return Sni::NeedMore;
    }
    if buf[0] != 0x16 {
        return Sni::NotTls;
    }
    if buf.len() < 5 {
        return Sni::NeedMore;
    }
    let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
    if len > 16 * 1024 + 256 {
        return Sni::NotTls;
    }
    if buf.len() < 5 + len {
        return Sni::NeedMore;
    }
    let body = &buf[5..5 + len];
    parse_hello(body).unwrap_or(Sni::NotTls)
}

fn parse_hello(b: &[u8]) -> Option<Sni> {
    // handshake: type 1 (ClientHello), u24 length
    if *b.first()? != 1 || b.len() < 4 {
        return None;
    }
    let hs_len = ((b[1] as usize) << 16) | ((b[2] as usize) << 8) | b[3] as usize;
    let h = b.get(4..4 + hs_len)?;
    let mut at = 2 + 32; // version, random
    let sid = *h.get(at)? as usize;
    at += 1 + sid;
    let suites = u16::from_be_bytes([*h.get(at)?, *h.get(at + 1)?]) as usize;
    at += 2 + suites;
    let comp = *h.get(at)? as usize;
    at += 1 + comp;
    if at == h.len() {
        return Some(Sni::NoName);
    }
    let ext_len = u16::from_be_bytes([*h.get(at)?, *h.get(at + 1)?]) as usize;
    at += 2;
    let exts = h.get(at..at + ext_len)?;
    let mut e = 0;
    while e + 4 <= exts.len() {
        let kind = u16::from_be_bytes([exts[e], exts[e + 1]]);
        let size = u16::from_be_bytes([exts[e + 2], exts[e + 3]]) as usize;
        let data = exts.get(e + 4..e + 4 + size)?;
        if kind == 0 {
            // server_name: list length, then (type 0, u16 length, name)
            let mut n = 2;
            while n + 3 <= data.len() {
                let kind = data[n];
                let size = u16::from_be_bytes([data[n + 1], data[n + 2]]) as usize;
                let name = data.get(n + 3..n + 3 + size)?;
                if kind == 0 {
                    return Some(Sni::Name(String::from_utf8_lossy(name).to_lowercase()));
                }
                n += 3 + size;
            }
            return Some(Sni::NoName);
        }
        e += 4 + size;
    }
    Some(Sni::NoName)
}

pub fn frame(kind: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(9 + payload.len());
    out.push(kind);
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

pub fn parse_frame(data: &[u8]) -> Option<(u8, u32, &[u8])> {
    if data.len() < 9 {
        return None;
    }
    let stream = u32::from_be_bytes([data[1], data[2], data[3], data[4]]);
    let len = u32::from_be_bytes([data[5], data[6], data[7], data[8]]) as usize;
    if len > MAX_PAYLOAD || data.len() != 9 + len {
        return None;
    }
    Some((data[0], stream, &data[9..]))
}

/// A refusal code as a phone may send it (phase 0's `not-allowed`): snake_case ASCII, at most 40 characters.
pub fn code_of(raw: &str) -> String {
    let code: String = raw
        .chars()
        .take(40)
        .map(|c| if c == '-' { '_' } else { c.to_ascii_lowercase() })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if code.is_empty() {
        "phone".into()
    } else {
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn only_public_addresses() {
        for bad in [
            "10.0.0.1",
            "192.168.1.1",
            "172.16.5.4",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.1.1",
            "100.127.255.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:192.168.1.1",
            "::ffff:127.0.0.1",
            "64:ff9b::c0a8:101",
            "64:ff9b::808:808",
            "64:ff9b:1::a00:1",
            "2002:c0a8:101::1",
            "2002:808:808::1",
            "2001:db8::1",
            "2001::1",
        ] {
            assert!(!public_ip(ip(bad)), "{bad}");
        }
        for good in ["8.8.8.8", "151.101.0.223", "100.128.0.1", "::ffff:8.8.8.8", "2a04:4e42::223", "2003::1"] {
            assert!(public_ip(ip(good)), "{good}");
        }
    }

    #[test]
    fn names_never_addresses_or_local() {
        for bad in [
            "192.168.1.1",
            "[::1]",
            "::1",
            "64:ff9b::c0a8:101",
            "10.1",
            "2130706433",
            "0x7f.1",
            "localhost",
            "router",
            "nas.local",
            "box.lan",
            "printer.home",
            "a..b.fr",
            "x.y_z.fr",
            "",
        ] {
            assert!(name_refused(bad).is_some(), "{bad}");
        }
        assert_eq!(name_refused("192.168.1.1"), Some("literal_ip"));
        assert_eq!(name_refused("nas.local"), Some("local_name"));
        for good in ["doctolib.fr", "www.doctolib.fr", "xn--bcher-kva.example", "assets.doctolib.fr"] {
            assert_eq!(name_refused(good), None, "{good}");
        }
    }

    #[test]
    fn sites_hosts_and_the_minimal_route() {
        assert_eq!(site_of("https://www.Ameli.fr/assure?x=1"), Ok("ameli.fr".into()));
        assert_eq!(site_of("ameli.fr:443"), Ok("ameli.fr".into()));
        assert_eq!(site_of("www.impots.gouv.fr"), Ok("impots.gouv.fr".into()));
        assert_eq!(site_of("https://192.168.1.1/"), Err("literal_ip"));
        assert_eq!(site_of("nas.local"), Err("local_name"));
        assert!(covers("ameli.fr", "ameli.fr") && covers("ameli.fr", "assure.ameli.fr"));
        assert!(!covers("ameli.fr", "evil-ameli.fr") && !covers("ameli.fr", "ameli.fr.evil.com"));
        assert!(is_static("doctolib.fr", "assets.doctolib.fr") && is_static("leboncoin.fr", "img.leboncoin.fr"));
        assert!(is_static("leboncoin.fr", "x.img.leboncoin.fr"));
        assert!(!is_static("leboncoin.fr", "leboncoin.fr") && !is_static("leboncoin.fr", "www.leboncoin.fr"));
        assert!(!is_static("leboncoin.fr", "dd.leboncoin.fr") && !is_static("leboncoin.fr", "img.other.fr"));
        assert!(!is_static("leboncoin.fr", "img.x.leboncoin.fr")); // the label right under the site decides
    }

    /// A minimal TLS 1.2/1.3-shaped ClientHello with an optional server name.
    pub(crate) fn hello(name: Option<&str>) -> Vec<u8> {
        let mut exts = Vec::new();
        // an unrelated extension first (supported_versions)
        exts.extend_from_slice(&[0x00, 0x2b, 0x00, 0x03, 0x02, 0x03, 0x04]);
        if let Some(n) = name {
            let n = n.as_bytes();
            let mut list = vec![0u8];
            list.extend_from_slice(&(n.len() as u16).to_be_bytes());
            list.extend_from_slice(n);
            let mut data = (list.len() as u16).to_be_bytes().to_vec();
            data.extend_from_slice(&list);
            exts.extend_from_slice(&[0, 0]);
            exts.extend_from_slice(&(data.len() as u16).to_be_bytes());
            exts.extend_from_slice(&data);
        }
        let mut body = vec![3, 3];
        body.extend_from_slice(&[7u8; 32]);
        body.push(0); // session id
        body.extend_from_slice(&[0, 2, 0x13, 0x01]);
        body.extend_from_slice(&[1, 0]);
        body.extend_from_slice(&(exts.len() as u16).to_be_bytes());
        body.extend_from_slice(&exts);
        let mut hs = vec![1, 0, (body.len() >> 8) as u8, body.len() as u8];
        hs.extend_from_slice(&body);
        let mut rec = vec![0x16, 3, 1];
        rec.extend_from_slice(&(hs.len() as u16).to_be_bytes());
        rec.extend_from_slice(&hs);
        rec
    }

    #[test]
    fn the_client_hello_names_its_site() {
        let h = hello(Some("www.Doctolib.fr"));
        assert_eq!(client_hello_sni(&h), Sni::Name("www.doctolib.fr".into()));
        assert_eq!(client_hello_sni(&h[..10]), Sni::NeedMore);
        assert_eq!(client_hello_sni(&hello(None)), Sni::NoName);
        assert_eq!(client_hello_sni(b"GET / HTTP/1.1\r\n"), Sni::NotTls);
        let mut broken = hello(Some("a.fr"));
        broken[6] = 0xff; // a handshake length past the record
        assert_eq!(client_hello_sni(&broken), Sni::NotTls);
    }

    #[test]
    fn frames_round_trip_and_bound() {
        let f = frame(T_DATA, 7, b"abc");
        assert_eq!(parse_frame(&f), Some((T_DATA, 7, &b"abc"[..])));
        assert_eq!(parse_frame(&f[..8]), None);
        let mut long = frame(T_DATA, 1, b"");
        long[5..9].copy_from_slice(&((MAX_PAYLOAD + 1) as u32).to_be_bytes());
        assert_eq!(parse_frame(&long), None);
    }

    #[test]
    fn phone_codes_made_plain() {
        assert_eq!(code_of("not-allowed"), "not_allowed");
        assert_eq!(code_of("Wifi_Only\n"), "wifi_only");
        assert_eq!(code_of("€"), "phone");
    }
}
