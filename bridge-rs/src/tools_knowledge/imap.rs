//! An IMAP client shaped like Python's `imaplib.IMAP4` (what mailbox.py and mail_scan.py call on it): `select`,
//! `uid`, `list`, `append`, `logout`, with the same response data — a list of lines, a line with its literal as a
//! pair (`(b'1 (UID 5 BODY[] {12}', b'…')`), `None` when the server sent nothing — and the same errors (`IMAP4.error`
//! for a BAD answer, `abort` for a broken connection, `OSError` for the socket). Blocking, as imaplib is (the bridge
//! calls it from a thread); TLS by rustls with the Mozilla roots (Python's default context), or none checked for
//! Proton Mail Bridge on this machine.
//!
//! Not a Python module: the standard library part the group needs, behind [`Imap`] so tests give a fake.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use regex::bytes::Regex;

/// One item of a response's data.
#[derive(Debug, Clone, PartialEq)]
pub enum Data {
    /// Python's `None` (no untagged answer of that kind).
    None,
    Line(Vec<u8>),
    /// A line and the literal it announced (`{n}`).
    Literal(Vec<u8>, Vec<u8>),
}

impl Data {
    pub fn literal(&self) -> Option<&[u8]> {
        match self {
            Data::Literal(_, data) => Some(data),
            _ => None,
        }
    }
}

/// `(typ, data)`.
pub type Reply = (String, Vec<Data>);

/// imaplib's errors.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ImapError {
    /// `IMAP4.error` (and `IMAP4.abort`, its subclass): a refused command, a broken protocol.
    #[error("{0}")]
    Imap(String),
    /// `OSError` (timeouts, refused or reset connections, TLS): its class name and text.
    #[error("{message}")]
    Os { name: &'static str, message: String },
    /// `UnicodeEncodeError`: a non-ASCII argument while the connection encodes in ASCII.
    #[error("'ascii' codec can't encode characters: {0}")]
    Encode(String),
}

impl ImapError {
    /// `type(exc).__name__`.
    pub fn type_name(&self) -> &'static str {
        match self {
            ImapError::Imap(_) => "error",
            ImapError::Os { name, .. } => name,
            ImapError::Encode(_) => "UnicodeEncodeError",
        }
    }
}

/// What mailbox.py and mail_scan.py use of an `imaplib.IMAP4` (tests implement it with a fake).
pub trait Imap: Send {
    fn select(&mut self, mailbox: &str, readonly: bool) -> Result<Reply, ImapError>;
    fn uid(&mut self, command: &str, args: &[&str]) -> Result<Reply, ImapError>;
    fn list(&mut self) -> Result<Reply, ImapError>;
    fn append(&mut self, mailbox: &str, flags: &str, date_time: &str, message: &[u8]) -> Result<Reply, ImapError>;
    /// `CREATE mailbox` (mail_tidy.rs: the folder archived mail goes to). A fake without it refuses.
    fn create(&mut self, _mailbox: &str) -> Result<Reply, ImapError> {
        Ok(("NO".into(), Vec::new()))
    }
    /// `conn._encoding = "utf-8"`.
    fn use_utf8(&mut self) {}
    /// The selected folder's UIDVALIDITY (its `* OK [UIDVALIDITY n]` answer to the last `select`), when it gave one.
    fn uid_validity(&mut self) -> Option<u64> {
        None
    }
    fn logout(&mut self) -> Result<(), ImapError>;
}

/// An `std::io::Error` as Python names it.
pub fn os_error(err: &std::io::Error) -> ImapError {
    use std::io::ErrorKind::*;
    let name = match err.kind() {
        ConnectionRefused => "ConnectionRefusedError",
        ConnectionReset => "ConnectionResetError",
        ConnectionAborted => "ConnectionAbortedError",
        TimedOut | WouldBlock => "TimeoutError",
        BrokenPipe => "BrokenPipeError",
        NotFound => "gaierror",
        InvalidData => {
            if err.to_string().contains("certificate") {
                "SSLCertVerificationError"
            } else {
                "SSLError"
            }
        }
        _ => "OSError",
    };
    let message = match err.kind() {
        TimedOut | WouldBlock => "timed out".to_string(),
        _ => err.to_string(),
    };
    ImapError::Os { name, message }
}

trait Stream: Read + Write + Send {}
impl<T: Read + Write + Send> Stream for T {}

/// A connection (`IMAP4_SSL`, or `IMAP4` then `starttls`).
pub struct Client {
    reader: BufReader<Box<dyn Stream>>,
    tagpre: String,
    tagnum: u32,
    untagged: HashMap<String, Vec<Data>>,
    utf8: bool,
    capabilities: Vec<String>,
}

static UNTAGGED_RESPONSE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s-u)^\* (?P<type>[A-Z-]+)( (?P<data>.*))?$").expect("valid regex"));
static UNTAGGED_STATUS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s-u)^\* (?P<data>\d+) (?P<type>[A-Z-]+)( (?P<data2>.*))?$").expect("valid regex"));
static LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s-u).*\{(?P<size>\d+)\}$").expect("valid regex"));
static TAGGED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s-u)^(?P<tag>[A-Z]+\d+) (?P<type>[A-Z]+) (?P<data>.*)$").expect("valid regex"));

const MAXLINE: usize = 1_000_000;

/// How long a server on this machine has to greet before it is taken for a TLS one (`connect_local`).
const LOCAL_GREETING: Duration = Duration::from_secs(3);

fn tls_config(verify: bool) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions");
    let config = if verify {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(NoVerify)).with_no_client_auth()
    };
    Arc::new(config)
}

/// Proton Mail Bridge on this machine: its certificate is its own (Python: `CERT_NONE`).
#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

fn tcp(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, ImapError> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| ImapError::Os { name: "gaierror", message: e.to_string() })?
        .collect();
    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => {
                stream.set_read_timeout(Some(timeout)).map_err(|e| os_error(&e))?;
                stream.set_write_timeout(Some(timeout)).map_err(|e| os_error(&e))?;
                return Ok(stream);
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last
        .map(|e| os_error(&e))
        .unwrap_or(ImapError::Os { name: "gaierror", message: format!("no address for {host}") }))
}

fn wrap(stream: TcpStream, host: &str, verify: bool) -> Result<Box<dyn Stream>, ImapError> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| ImapError::Os { name: "SSLError", message: e.to_string() })?;
    let conn = rustls::ClientConnection::new(tls_config(verify), name)
        .map_err(|e| ImapError::Os { name: "SSLError", message: e.to_string() })?;
    let mut tls = rustls::StreamOwned::new(conn, stream);
    // the handshake now, as Python's wrap_socket does
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).map_err(|e| os_error(&e))?;
    }
    Ok(Box::new(tls))
}

impl Client {
    /// `IMAP4_SSL(host, port, timeout=…)`.
    pub fn connect_tls(host: &str, port: u16, timeout: Duration) -> Result<Client, ImapError> {
        let stream = wrap(tcp(host, port, timeout)?, host, true)?;
        Client::start(stream)
    }

    /// `IMAP4(host, port, timeout=…)` then `starttls(context)` (certificates not checked on this machine).
    pub fn connect_starttls(host: &str, port: u16, timeout: Duration, verify: bool) -> Result<Client, ImapError> {
        let plain = tcp(host, port, timeout)?;
        let clone = plain.try_clone().map_err(|e| os_error(&e))?;
        let client = Client::start(Box::new(plain))?;
        if !client.offers_starttls() {
            return Err(ImapError::Imap("TLS not supported by server".into()));
        }
        client.upgrade(clone, host, verify)
    }

    /// A server on this machine — Proton Mail Bridge or hydroxide (decided 2026-10-28) — whose certificate, when it
    /// has one, is its own (not checked):
    ///
    /// - STARTTLS when it offers it (Proton Mail Bridge; hydroxide never does);
    /// - implicit TLS when it says nothing until spoken to (hydroxide started with `-tls-cert`: `ListenAndServeTLS`);
    /// - else plain (hydroxide's default: `AllowInsecureAuth` without TLS options).
    ///
    /// The connected address must be a loopback one (whatever `host` resolved to): the password never goes in clear
    /// over a network. Err `Imap("not a server on this machine")` otherwise.
    pub fn connect_local(host: &str, port: u16, timeout: Duration) -> Result<Client, ImapError> {
        let plain = tcp(host, port, timeout)?;
        Client::loopback_only(&plain)?;
        let clone = plain.try_clone().map_err(|e| os_error(&e))?;
        clone.set_read_timeout(Some(LOCAL_GREETING)).map_err(|e| os_error(&e))?;
        let client = match Client::start(Box::new(plain)) {
            Ok(client) => client,
            // Silent: a TLS server waiting for our hello.
            Err(ImapError::Os { name: "TimeoutError", .. }) => {
                let stream = tcp(host, port, timeout)?;
                Client::loopback_only(&stream)?;
                return Client::start(wrap(stream, host, false)?);
            }
            Err(exc) => return Err(exc),
        };
        clone.set_read_timeout(Some(timeout)).map_err(|e| os_error(&e))?;
        if client.offers_starttls() {
            return client.upgrade(clone, host, false);
        }
        Ok(client)
    }

    /// Err unless the stream's other end is this machine.
    fn loopback_only(stream: &TcpStream) -> Result<(), ImapError> {
        match stream.peer_addr() {
            Ok(peer) if peer.ip().is_loopback() => Ok(()),
            _ => Err(ImapError::Imap("not a server on this machine".into())),
        }
    }

    fn offers_starttls(&self) -> bool {
        self.capabilities.iter().any(|c| c == "STARTTLS")
    }

    /// `starttls(context)` on a started connection (`clone`: its socket).
    fn upgrade(mut self, clone: TcpStream, host: &str, verify: bool) -> Result<Client, ImapError> {
        let (typ, _) = self.simple("STARTTLS", &[])?;
        if typ != "OK" {
            return Err(ImapError::Imap("Couldn't establish TLS session".into()));
        }
        self.reader = BufReader::new(wrap(clone, host, verify)?);
        self.capabilities = self.get_capabilities()?;
        Ok(self)
    }

    fn start(stream: Box<dyn Stream>) -> Result<Client, ImapError> {
        let mut client = Client {
            reader: BufReader::new(stream),
            tagpre: "BIPA".to_string(),
            tagnum: 0,
            untagged: HashMap::new(),
            utf8: false,
            capabilities: Vec::new(),
        };
        // the greeting
        let greeting = client.get_line()?;
        if !greeting.starts_with(b"* OK") && !greeting.starts_with(b"* PREAUTH") {
            return Err(ImapError::Imap(String::from_utf8_lossy(&greeting).into_owned()));
        }
        client.capabilities = client.get_capabilities()?;
        if !client.capabilities.iter().any(|c| c == "IMAP4REV1" || c == "IMAP4") {
            return Err(ImapError::Imap("server not IMAP4 compliant".into()));
        }
        Ok(client)
    }

    fn get_capabilities(&mut self) -> Result<Vec<String>, ImapError> {
        self.simple("CAPABILITY", &[])?;
        let data = self.untagged_of("CAPABILITY").unwrap_or_default();
        let last = match data.last() {
            Some(Data::Line(line)) => String::from_utf8_lossy(line).to_uppercase(),
            _ => return Err(ImapError::Imap("no CAPABILITY response from server".into())),
        };
        Ok(last.split_whitespace().map(str::to_string).collect())
    }

    fn untagged_of(&mut self, name: &str) -> Option<Vec<Data>> {
        self.untagged.remove(name)
    }

    /// `login(user, password)`: the user as it is, the password quoted (imaplib's way).
    pub fn login(&mut self, user: &str, password: &str) -> Result<(), ImapError> {
        let (typ, data) = self.simple("LOGIN", &[user, &quote(password)])?;
        if typ != "OK" {
            let last = match data.last() {
                Some(Data::Line(l)) => String::from_utf8_lossy(l).into_owned(),
                _ => String::new(),
            };
            return Err(ImapError::Imap(last));
        }
        Ok(())
    }

    fn get_line(&mut self) -> Result<Vec<u8>, ImapError> {
        let mut line = Vec::new();
        let read =
            (&mut self.reader).take(MAXLINE as u64 + 1).read_until(b'\n', &mut line).map_err(|e| os_error(&e))?;
        if read == 0 {
            return Err(ImapError::Imap("socket error: EOF".into()));
        }
        if line.len() > MAXLINE {
            return Err(ImapError::Imap(format!("got more than {MAXLINE} bytes")));
        }
        if !line.ends_with(b"\r\n") {
            return Err(ImapError::Imap(format!(
                "socket error: unterminated line: {:?}",
                String::from_utf8_lossy(&line)
            )));
        }
        line.truncate(line.len() - 2);
        Ok(line)
    }

    fn encode(&self, text: &str) -> Result<Vec<u8>, ImapError> {
        if !self.utf8 && !text.is_ascii() {
            return Err(ImapError::Encode(text.to_string()));
        }
        Ok(text.as_bytes().to_vec())
    }

    /// One response read: Some((tag, typ, data)) for a tagged one, None for an untagged or a continuation.
    fn get_response(&mut self) -> Result<Response, ImapError> {
        let resp = self.get_line()?;
        if let Some(caps) = TAGGED.captures(&resp) {
            let tag = String::from_utf8_lossy(&caps["tag"]).into_owned();
            if tag.starts_with(&self.tagpre) {
                let typ = String::from_utf8_lossy(&caps["type"]).into_owned();
                return Ok(Response::Tagged(tag, typ, caps["data"].to_vec()));
            }
        }
        let (typ, mut dat) = if let Some(caps) = UNTAGGED_RESPONSE.captures(&resp) {
            (
                String::from_utf8_lossy(&caps["type"]).into_owned(),
                caps.name("data").map(|m| m.as_bytes().to_vec()).unwrap_or_default(),
            )
        } else if let Some(caps) = UNTAGGED_STATUS.captures(&resp) {
            let mut dat = caps["data"].to_vec();
            if let Some(more) = caps.name("data2") {
                dat.push(b' ');
                dat.extend_from_slice(more.as_bytes());
            }
            (String::from_utf8_lossy(&caps["type"]).into_owned(), dat)
        } else if resp.starts_with(b"+") {
            return Ok(Response::Continuation);
        } else {
            return Err(ImapError::Imap(format!("unexpected response: {:?}", String::from_utf8_lossy(&resp))));
        };
        while let Some(caps) = LITERAL.captures(&dat) {
            let size: usize = String::from_utf8_lossy(&caps["size"]).parse().unwrap_or(0);
            let mut data = vec![0u8; size];
            self.reader.read_exact(&mut data).map_err(|e| os_error(&e))?;
            self.untagged.entry(typ.clone()).or_default().push(Data::Literal(dat.clone(), data));
            dat = self.get_line()?;
        }
        self.untagged.entry(typ).or_default().push(Data::Line(dat));
        Ok(Response::Untagged)
    }

    fn send(&mut self, data: &[u8]) -> Result<(), ImapError> {
        let stream = self.reader.get_mut();
        stream.write_all(data).and_then(|_| stream.flush()).map_err(|e| os_error(&e))
    }

    /// `_simple_command(name, *args)` (with `literal`: APPEND's message after the server's `+`).
    fn command(&mut self, name: &str, args: &[&str], literal: Option<&[u8]>) -> Result<Reply, ImapError> {
        self.tagnum += 1;
        let tag = format!("{}{}", self.tagpre, self.tagnum);
        let mut data = format!("{tag} {name}").into_bytes();
        for arg in args {
            data.push(b' ');
            data.extend(self.encode(arg)?);
        }
        if let Some(literal) = literal {
            data.extend(format!(" {{{}}}", literal.len()).into_bytes());
        }
        data.extend_from_slice(b"\r\n");
        self.send(&data)?;
        let mut result = None;
        if let Some(literal) = literal {
            loop {
                match self.get_response()? {
                    Response::Continuation => break,
                    Response::Tagged(t, typ, dat) if t == tag => {
                        result = Some((typ, vec![Data::Line(dat)]));
                        break;
                    }
                    _ => {}
                }
            }
            if result.is_none() {
                let mut out = literal.to_vec();
                out.extend_from_slice(b"\r\n");
                self.send(&out)?;
            }
        }
        let (typ, dat) = match result {
            Some(r) => r,
            None => loop {
                if let Response::Tagged(t, typ, dat) = self.get_response()? {
                    if t == tag {
                        break (typ, vec![Data::Line(dat)]);
                    }
                }
            },
        };
        if name != "LOGOUT" {
            if let Some(bye) = self.untagged.remove("BYE") {
                let last = match bye.last() {
                    Some(Data::Line(l)) => String::from_utf8_lossy(l).into_owned(),
                    _ => String::new(),
                };
                return Err(ImapError::Imap(last));
            }
        }
        if typ == "BAD" {
            return Err(ImapError::Imap(format!("{name} command error: {typ} {:?}", dat)));
        }
        Ok((typ, dat))
    }

    fn simple(&mut self, name: &str, args: &[&str]) -> Result<Reply, ImapError> {
        self.command(name, args, None)
    }

    /// `_untagged_response(typ, dat, name)`.
    fn untagged_response(&mut self, typ: String, dat: Vec<Data>, name: &str) -> Reply {
        if typ == "NO" {
            return (typ, dat);
        }
        match self.untagged.remove(name) {
            Some(data) => (typ, data),
            None => (typ, vec![Data::None]),
        }
    }
}

enum Response {
    Tagged(String, String, Vec<u8>),
    Untagged,
    Continuation,
}

/// `_quote`: an IMAP quoted string.
pub fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Imap for Client {
    fn select(&mut self, mailbox: &str, readonly: bool) -> Result<Reply, ImapError> {
        self.untagged.clear();
        let name = if readonly { "EXAMINE" } else { "SELECT" };
        let (typ, dat) = self.simple(name, &[mailbox])?;
        if typ != "OK" {
            return Ok((typ, dat));
        }
        let exists = self.untagged.get("EXISTS").cloned().unwrap_or_else(|| vec![Data::None]);
        Ok((typ, exists))
    }

    fn uid(&mut self, command: &str, args: &[&str]) -> Result<Reply, ImapError> {
        let mut all = vec![command];
        all.extend_from_slice(args);
        let (typ, dat) = self.simple("UID", &all)?;
        let name = if ["SEARCH", "SORT", "THREAD"].contains(&command.to_uppercase().as_str()) {
            command.to_uppercase()
        } else {
            "FETCH".into()
        };
        Ok(self.untagged_response(typ, dat, &name))
    }

    fn list(&mut self) -> Result<Reply, ImapError> {
        let (typ, dat) = self.simple("LIST", &["\"\"", "*"])?;
        Ok(self.untagged_response(typ, dat, "LIST"))
    }

    fn append(&mut self, mailbox: &str, flags: &str, date_time: &str, message: &[u8]) -> Result<Reply, ImapError> {
        static CRLF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n|\r|\n").expect("valid regex"));
        let flags = if flags.is_empty() {
            None
        } else if flags.starts_with('(') && flags.ends_with(')') {
            Some(flags.to_string())
        } else {
            Some(format!("({flags})"))
        };
        let literal = CRLF.replace_all(message, &b"\r\n"[..]).into_owned();
        let mut args = vec![if mailbox.is_empty() { "INBOX" } else { mailbox }];
        if let Some(f) = flags.as_deref() {
            args.push(f);
        }
        if !date_time.is_empty() {
            args.push(date_time);
        }
        self.command("APPEND", &args, Some(&literal))
    }

    fn use_utf8(&mut self) {
        self.utf8 = true;
    }

    fn create(&mut self, mailbox: &str) -> Result<Reply, ImapError> {
        self.simple("CREATE", &[mailbox])
    }

    fn uid_validity(&mut self) -> Option<u64> {
        static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\[UIDVALIDITY (\d+)\]").expect("valid regex"));
        self.untagged.get("OK")?.iter().find_map(|item| {
            let line = match item {
                Data::Line(line) | Data::Literal(line, _) => line,
                Data::None => return None,
            };
            CODE.captures(line).and_then(|c| std::str::from_utf8(&c[1]).ok()?.parse().ok())
        })
    }

    fn logout(&mut self) -> Result<(), ImapError> {
        let result = self.simple("LOGOUT", &[]);
        let _ = self.reader.get_mut().flush();
        result.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted server on a local socket: answers each command with the lines given for it.
    fn serve(script: Vec<(&'static str, &'static [u8])>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            sock.write_all(b"* OK IMAP4rev1 ready\r\n").unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            for (expect, answer) in script {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let tag = line.split(' ').next().unwrap().to_string();
                assert!(line.contains(expect), "{line} lacks {expect}");
                if expect == "APPEND" {
                    sock.write_all(b"+ go ahead\r\n").unwrap();
                    let size: usize = line.rsplit('{').next().unwrap().trim().trim_end_matches('}').parse().unwrap();
                    let mut literal = vec![0u8; size + 2];
                    reader.read_exact(&mut literal).unwrap();
                }
                sock.write_all(answer).unwrap();
                sock.write_all(format!("{tag} OK done\r\n").as_bytes()).unwrap();
            }
        });
        port
    }

    #[test]
    fn responses_as_imaplib_gives_them() {
        let port = serve(vec![
            ("CAPABILITY", b"* CAPABILITY IMAP4rev1 STARTTLS\r\n"),
            ("LOGIN alex@example.com \"p\\\"w\"", b""),
            ("EXAMINE \"INBOX\"", b"* 2 EXISTS\r\n* FLAGS (\\Seen)\r\n"),
            ("UID SEARCH ALL", b"* SEARCH 101 102\r\n"),
            ("UID FETCH 101 (BODY.PEEK[])", b"* 1 FETCH (UID 101 BODY[] {5}\r\nhello)\r\n"),
            ("LIST \"\" *", b"* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n"),
            ("APPEND \"Drafts\" (\\Draft)", b""),
            ("LOGOUT", b"* BYE\r\n"),
        ]);
        let stream = tcp("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        let mut client = Client::start(Box::new(stream)).unwrap();
        client.login("alex@example.com", "p\"w").unwrap();
        assert_eq!(client.select("\"INBOX\"", true).unwrap(), ("OK".into(), vec![Data::Line(b"2".to_vec())]));
        assert_eq!(client.uid("SEARCH", &["ALL"]).unwrap().1, vec![Data::Line(b"101 102".to_vec())]);
        let fetched = client.uid("FETCH", &["101", "(BODY.PEEK[])"]).unwrap().1;
        assert_eq!(
            fetched,
            vec![Data::Literal(b"1 (UID 101 BODY[] {5}".to_vec(), b"hello".to_vec()), Data::Line(b")".to_vec())]
        );
        assert_eq!(client.list().unwrap().1, vec![Data::Line(b"(\\HasNoChildren) \"/\" \"INBOX\"".to_vec())]);
        assert_eq!(
            client.append("\"Drafts\"", "(\\Draft)", "\"21-Oct-2026 10:00:00 +0200\"", b"a\nb").unwrap().0,
            "OK"
        );
        assert!(matches!(client.uid("SEARCH", &["TEXT", "\"relevé\""]), Err(ImapError::Encode(_))));
        client.logout().unwrap();
    }
}
