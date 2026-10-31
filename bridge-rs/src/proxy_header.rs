//! The PROXY protocol (version 2, binary) on the door, from this machine only.
//!
//! On a server of the all-included offer, every place answers on port 443 (2026-10-31: many networks let out only
//! 443, so doors 444 and up looked offline from a strict home box or an office). Root's router (HAProxy, set up by
//! installer/cloud/seat-agent.py) takes 443, reads the name the client asks for in its TLS hello, and passes the
//! connection as it is (nothing decrypted) to that place's door on 127.0.0.1. Seen from here, that connection comes
//! from 127.0.0.1, which the bridge trusts (no wrong-key count: guard.rs; `X-Forwarded-For` believed: auth.rs). So
//! the router first sends a PROXY v2 header with the client's real address, and the door takes that address as the
//! connection's peer.
//!
//! Only a connection from a loopback address may carry the header: anyone else's bytes go to TLS as they are (a
//! header sent from outside is refused by the TLS handshake). A local process could claim any address with it, but a
//! local process is trusted already. A connection from 127.0.0.1 without the header (the bridge's own calls, the
//! seat agent's look at the door) keeps its loopback peer. The `LOCAL` command (the seat agent's check that this
//! door understands the header) keeps it too.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// The 12 bytes every v2 header starts with.
pub const SIGNATURE: [u8; 12] = [0x0D, 0x0A, 0x0D, 0x0A, 0x00, 0x0D, 0x0A, 0x51, 0x55, 0x49, 0x54, 0x0A];

/// How long a loopback connection may take to show whether it carries a header.
const WAIT: Duration = Duration::from_secs(5);

/// The longest address block we read (a TCP over IPv6 one is 36 bytes; TLVs after it are skipped, up to this).
const MOST: usize = 512;

/// What a header said.
#[derive(Debug, PartialEq, Eq)]
pub enum Header {
    /// `PROXY`: the client's address and port.
    Proxy(SocketAddr),
    /// `LOCAL`, or an address family we don't take (UNIX, UDP, unspecified): the connection's own peer stays.
    Local,
}

/// The 16 first bytes of a v2 header: its command and the length of what follows. None when they aren't one.
pub fn head(bytes: &[u8; 16]) -> Option<(u8, u8, usize)> {
    if bytes[..12] != SIGNATURE || bytes[12] >> 4 != 2 {
        return None;
    }
    let command = bytes[12] & 0x0F;
    if command > 1 {
        return None;
    }
    let length = u16::from_be_bytes([bytes[14], bytes[15]]) as usize;
    if length > MOST {
        return None;
    }
    Some((command, bytes[13], length))
}

/// The header once its address block is read.
pub fn parse(command: u8, family: u8, block: &[u8]) -> Option<Header> {
    if command == 0 {
        return Some(Header::Local);
    }
    match family {
        // TCP over IPv4: source, destination, source port, destination port
        0x11 => {
            let b = block.get(..12)?;
            let ip = Ipv4Addr::new(b[0], b[1], b[2], b[3]);
            Some(Header::Proxy(SocketAddr::new(IpAddr::V4(ip), u16::from_be_bytes([b[8], b[9]]))))
        }
        // TCP over IPv6
        0x21 => {
            let b = block.get(..36)?;
            let mut octets = [0u8; 16];
            octets.copy_from_slice(&b[..16]);
            let ip = Ipv6Addr::from(octets);
            // an IPv4 client through a dual-stack listener: shown as its IPv4 address, as a direct connection is
            let ip = ip.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(ip));
            Some(Header::Proxy(SocketAddr::new(ip, u16::from_be_bytes([b[32], b[33]]))))
        }
        _ => Some(Header::Local),
    }
}

/// Whether the first bytes seen so far could still be a header's signature.
fn may_be_header(seen: &[u8]) -> bool {
    let n = seen.len().min(SIGNATURE.len());
    seen[..n] == SIGNATURE[..n]
}

/// The connection's peer: `peer` itself, or the client's address when a loopback peer sent a header first (the header
/// then consumed, the stream left at the TLS hello). Err: a loopback connection whose header is broken or late (the
/// connection is then dropped).
pub async fn peer_of(stream: &mut TcpStream, peer: SocketAddr) -> std::io::Result<SocketAddr> {
    if !peer.ip().is_loopback() {
        return Ok(peer);
    }
    let broken = || std::io::Error::new(std::io::ErrorKind::InvalidData, "broken PROXY header");
    let mut first = [0u8; 16];
    // Peeked first: without a header the bytes stay for TLS. A TLS hello starts with 0x16, so one byte tells.
    let seen = tokio::time::timeout(WAIT, async {
        loop {
            let n = stream.peek(&mut first).await?;
            if n == 0 || !may_be_header(&first[..n]) || n == first.len() {
                return Ok::<usize, std::io::Error>(n);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "no TLS hello nor PROXY header"))??;
    if seen < first.len() || !may_be_header(&first[..seen]) {
        return Ok(peer);
    }
    let (command, family, length) = head(&first).ok_or_else(broken)?;
    stream.read_exact(&mut first).await?;
    let mut block = vec![0u8; length];
    tokio::time::timeout(WAIT, stream.read_exact(&mut block))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "PROXY header cut short"))??;
    match parse(command, family, &block).ok_or_else(broken)? {
        Header::Proxy(client) => Ok(client),
        Header::Local => Ok(peer),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    fn v4_header(ip: [u8; 4], port: u16) -> Vec<u8> {
        let mut h = SIGNATURE.to_vec();
        h.extend([0x21, 0x11, 0x00, 12]);
        h.extend(ip);
        h.extend([127, 0, 0, 1]);
        h.extend(port.to_be_bytes());
        h.extend(443u16.to_be_bytes());
        h
    }

    async fn pair() -> (TcpStream, TcpStream, SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let (server, peer) = listener.accept().await.unwrap();
        (client, server, peer)
    }

    #[tokio::test]
    async fn a_header_from_loopback_gives_the_clients_address_and_leaves_the_hello() {
        let (mut client, mut server, peer) = pair().await;
        let mut sent = v4_header([203, 0, 113, 9], 51000);
        sent.extend([0x16, 0x03, 0x01]);
        client.write_all(&sent).await.unwrap();
        let got = peer_of(&mut server, peer).await.unwrap();
        assert_eq!(got, "203.0.113.9:51000".parse::<SocketAddr>().unwrap());
        let mut rest = [0u8; 3];
        server.read_exact(&mut rest).await.unwrap();
        assert_eq!(rest, [0x16, 0x03, 0x01]);
    }

    #[tokio::test]
    async fn a_tls_hello_from_loopback_keeps_the_peer_and_its_bytes() {
        let (mut client, mut server, peer) = pair().await;
        client.write_all(&[0x16, 0x03, 0x01, 0x00]).await.unwrap();
        assert_eq!(peer_of(&mut server, peer).await.unwrap(), peer);
        let mut rest = [0u8; 4];
        server.read_exact(&mut rest).await.unwrap();
        assert_eq!(rest, [0x16, 0x03, 0x01, 0x00]);
    }

    #[tokio::test]
    async fn a_header_sent_in_pieces_is_waited_for() {
        let (mut client, mut server, peer) = pair().await;
        let sent = v4_header([198, 51, 100, 7], 4242);
        let task = tokio::spawn(async move {
            client.write_all(&sent[..5]).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            client.write_all(&sent[5..]).await.unwrap();
            client
        });
        let got = peer_of(&mut server, peer).await.unwrap();
        assert_eq!(got.ip(), "198.51.100.7".parse::<IpAddr>().unwrap());
        drop(task.await.unwrap());
    }

    #[tokio::test]
    async fn local_keeps_the_peer() {
        let (mut client, mut server, peer) = pair().await;
        let mut h = SIGNATURE.to_vec();
        h.extend([0x20, 0x00, 0x00, 0x00, 0x16]);
        client.write_all(&h).await.unwrap();
        assert_eq!(peer_of(&mut server, peer).await.unwrap(), peer);
        let mut rest = [0u8; 1];
        server.read_exact(&mut rest).await.unwrap();
        assert_eq!(rest, [0x16]);
    }

    #[tokio::test]
    async fn a_remote_peer_is_never_read() {
        // not loopback: returned at once, nothing peeked (a header from outside goes to TLS and fails there)
        let (_client, mut server, _) = pair().await;
        let remote: SocketAddr = "203.0.113.1:9".parse().unwrap();
        assert_eq!(peer_of(&mut server, remote).await.unwrap(), remote);
    }

    #[tokio::test]
    async fn a_broken_header_is_refused() {
        let (mut client, mut server, peer) = pair().await;
        let mut h = SIGNATURE.to_vec();
        h.extend([0x13, 0x11, 0x00, 0x0C]); // version 1 in the high nibble
        client.write_all(&h).await.unwrap();
        assert!(peer_of(&mut server, peer).await.is_err());
    }

    #[test]
    fn ipv6_and_mapped_ipv4() {
        let mut block = vec![0u8; 36];
        block[..16].copy_from_slice(&"2001:db8::5".parse::<Ipv6Addr>().unwrap().octets());
        block[32..34].copy_from_slice(&7000u16.to_be_bytes());
        assert_eq!(parse(1, 0x21, &block), Some(Header::Proxy("[2001:db8::5]:7000".parse().unwrap())));
        block[..16].copy_from_slice(&"::ffff:192.0.2.4".parse::<Ipv6Addr>().unwrap().octets());
        assert_eq!(parse(1, 0x21, &block), Some(Header::Proxy("192.0.2.4:7000".parse().unwrap())));
        assert_eq!(parse(1, 0x11, &block[..8]), None);
        assert_eq!(parse(1, 0x31, &[]), Some(Header::Local));
    }
}
