//! The SMTP side of an agent's own address (agent_email.rs): the guard Hermes' e-mail gateway sends through, and the
//! client that passes its mails on to the real server.
//!
//! The guard listens on 127.0.0.1 only (a port of this account's own, `guard_port`). Hermes logs in with the agent's
//! address and its mailbox password, as it would at the real server; then a mail goes on only when
//!
//! - it comes from that address,
//! - every recipient is one the person allowed (the same list as the senders the agent answers),
//! - the agent sent fewer than its daily limit today.
//!
//! A mail to an address not allowed yet is held, the person is told (a push), and Hermes gets « 550 held for the
//! person's approval »: the agent says so, it doesn't retry. The app's « Envoyer » sends it and allows the address.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context};
use base64::Engine;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::agent_email::{self as ae, Record};

/// A message's size at most (Hermes' attachments included).
pub const MAX_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_RECIPIENTS: usize = 20;
const TIMEOUT: Duration = Duration::from_secs(60);

/// What a held mail tells: (agent, first new recipient, held id).
pub type Notify = Arc<dyn Fn(String, String, String) + Send + Sync>;

/// The guard's world: where the agents' records are, and how the person is told.
#[derive(Clone)]
pub struct Guard {
    pub folder: PathBuf,
    pub notify: Notify,
}

/// This account's own port: each place of a server is an account of its own, so its uid tells its port apart.
/// `BIPAGENTS_MAIL_GUARD_PORT` sets another one.
pub fn guard_port() -> u16 {
    if let Some(port) = std::env::var("BIPAGENTS_MAIL_GUARD_PORT").ok().and_then(|p| p.trim().parse().ok()) {
        return port;
    }
    // SAFETY: getuid has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    25_000 + (uid % 5_000) as u16
}

/// Listens until the bridge stops (a port already taken is logged: the gateway's sends then fail, nothing else).
pub async fn serve(guard: Guard, port: u16) {
    let listener = match TcpListener::bind(("127.0.0.1", port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(port, error = %e, "the e-mail guard can't listen: agents' e-mails won't go");
            return;
        }
    };
    run(guard, listener).await
}

pub async fn run(guard: Guard, listener: TcpListener) {
    loop {
        let Ok((stream, _)) = listener.accept().await else { continue };
        let guard = guard.clone();
        tokio::spawn(async move {
            if let Err(e) = tokio::time::timeout(Duration::from_secs(600), session(guard, stream)).await {
                tracing::debug!(error = %e, "e-mail guard session timed out");
            }
        });
    }
}

async fn reply<W: AsyncWrite + Unpin>(w: &mut W, line: &str) -> std::io::Result<()> {
    w.write_all(format!("{line}\r\n").as_bytes()).await?;
    w.flush().await
}

fn b64(text: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(text.trim()).ok()?;
    String::from_utf8(bytes).ok()
}

/// `<a@b>` (with parameters after it) -> `a@b`, lowercased.
fn path_address(arg: &str) -> String {
    let arg = arg.trim();
    let inner = match (arg.find('<'), arg.find('>')) {
        (Some(a), Some(b)) if b > a => &arg[a + 1..b],
        _ => arg.split_whitespace().next().unwrap_or(""),
    };
    inner.trim().to_lowercase()
}

async fn session(guard: Guard, stream: TcpStream) -> anyhow::Result<()> {
    let (read, mut w) = stream.into_split();
    let mut r = BufReader::new(read);
    reply(&mut w, "220 bipagents e-mail guard ESMTP").await?;
    let mut who: Option<Record> = None;
    let mut from: Option<String> = None;
    let mut rcpts: Vec<String> = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = tokio::time::timeout(TIMEOUT, r.read_until(b'\n', &mut line)).await??;
        if n == 0 {
            return Ok(());
        }
        let text = String::from_utf8_lossy(&line).trim_end().to_string();
        let (verb, arg) = match text.split_once(' ') {
            Some((v, a)) => (v.to_ascii_uppercase(), a.to_string()),
            None => (text.to_ascii_uppercase(), String::new()),
        };
        match verb.as_str() {
            "EHLO" => {
                w.write_all(b"250-localhost\r\n250-8BITMIME\r\n250-AUTH PLAIN LOGIN\r\n250 SIZE 26214400\r\n").await?;
                w.flush().await?;
            }
            "HELO" => reply(&mut w, "250 localhost").await?,
            "NOOP" => reply(&mut w, "250 OK").await?,
            "RSET" => {
                from = None;
                rcpts.clear();
                reply(&mut w, "250 OK").await?;
            }
            "QUIT" => {
                reply(&mut w, "221 Bye").await?;
                return Ok(());
            }
            "AUTH" => {
                let mut parts = arg.split_whitespace();
                let method = parts.next().unwrap_or("").to_ascii_uppercase();
                let initial = parts.next().map(str::to_string);
                let (user, password) = match method.as_str() {
                    "PLAIN" => {
                        let given = match initial {
                            Some(i) => i,
                            None => {
                                reply(&mut w, "334 ").await?;
                                read_line(&mut r).await?
                            }
                        };
                        let decoded = b64(&given).unwrap_or_default();
                        let mut fields = decoded.split('\0');
                        let _authz = fields.next();
                        (fields.next().unwrap_or("").to_string(), fields.next().unwrap_or("").to_string())
                    }
                    "LOGIN" => {
                        let user = match initial {
                            Some(i) => b64(&i).unwrap_or_default(),
                            None => {
                                reply(&mut w, "334 VXNlcm5hbWU6").await?;
                                b64(&read_line(&mut r).await?).unwrap_or_default()
                            }
                        };
                        reply(&mut w, "334 UGFzc3dvcmQ6").await?;
                        (user, b64(&read_line(&mut r).await?).unwrap_or_default())
                    }
                    _ => {
                        reply(&mut w, "504 5.5.4 PLAIN or LOGIN").await?;
                        continue;
                    }
                };
                match ae::find_by_login(&guard.folder, &user, &password) {
                    Some(record) => {
                        who = Some(record);
                        reply(&mut w, "235 2.7.0 Authentication successful").await?;
                    }
                    None => {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        reply(&mut w, "535 5.7.8 Authentication credentials invalid").await?;
                    }
                }
            }
            "MAIL" => {
                let Some(record) = &who else {
                    reply(&mut w, "530 5.7.0 Authentication required").await?;
                    continue;
                };
                let address = path_address(arg.trim_start_matches(|c: char| c != ':').trim_start_matches(':'));
                if address != record.address.to_lowercase() {
                    reply(&mut w, "553 5.7.1 Only the agent's own address may send here").await?;
                    continue;
                }
                from = Some(address);
                rcpts.clear();
                reply(&mut w, "250 2.1.0 OK").await?;
            }
            "RCPT" => {
                if from.is_none() {
                    reply(&mut w, "503 5.5.1 MAIL first").await?;
                    continue;
                }
                let address = path_address(arg.trim_start_matches(|c: char| c != ':').trim_start_matches(':'));
                if !ae::looks_like_address(&address) {
                    reply(&mut w, "501 5.1.3 Bad recipient address").await?;
                } else if rcpts.len() >= MAX_RECIPIENTS {
                    reply(&mut w, "452 4.5.3 Too many recipients").await?;
                } else {
                    rcpts.push(address);
                    reply(&mut w, "250 2.1.5 OK").await?;
                }
            }
            "DATA" => {
                let (Some(record), Some(sender)) = (&who, &from) else {
                    reply(&mut w, "503 5.5.1 MAIL first").await?;
                    continue;
                };
                if rcpts.is_empty() {
                    reply(&mut w, "503 5.5.1 RCPT first").await?;
                    continue;
                }
                reply(&mut w, "354 End data with <CR><LF>.<CR><LF>").await?;
                let Some(message) = read_data(&mut r).await? else {
                    reply(&mut w, "552 5.3.4 Message too big").await?;
                    from = None;
                    rcpts.clear();
                    continue;
                };
                let answer = decide(&guard, &record.agent, sender, &rcpts, &message).await;
                reply(&mut w, &answer).await?;
                from = None;
                rcpts.clear();
            }
            _ => reply(&mut w, "502 5.5.2 Command not recognized").await?,
        }
    }
}

async fn read_line<R: AsyncBufReadExt + Unpin>(r: &mut R) -> anyhow::Result<String> {
    let mut line = Vec::new();
    tokio::time::timeout(TIMEOUT, r.read_until(b'\n', &mut line)).await??;
    Ok(String::from_utf8_lossy(&line).trim_end().to_string())
}

/// The message after DATA, dot-unstuffed, lines ending in CRLF; None when over `MAX_BYTES` (read to its end).
async fn read_data<R: AsyncBufReadExt + Unpin>(r: &mut R) -> anyhow::Result<Option<Vec<u8>>> {
    let mut message = Vec::new();
    let mut too_big = false;
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = tokio::time::timeout(TIMEOUT, r.read_until(b'\n', &mut line)).await??;
        if n == 0 {
            bail!("the connection closed during DATA");
        }
        let body = line.strip_suffix(b"\n").unwrap_or(&line);
        let body = body.strip_suffix(b"\r").unwrap_or(body);
        if body == b"." {
            return Ok(if too_big { None } else { Some(message) });
        }
        let body = body.strip_prefix(b".").filter(|_| body.starts_with(b"..")).unwrap_or(body);
        if message.len() + body.len() + 2 > MAX_BYTES {
            too_big = true;
            continue;
        }
        message.extend_from_slice(body);
        message.extend_from_slice(b"\r\n");
    }
}

/// The answer to a mail: sent, held for the person, or refused (the daily limit). The day's count is taken before
/// sending (two mails at once can't both pass the limit) and given back when the server didn't take it.
async fn decide(guard: &Guard, agent: &str, from: &str, rcpts: &[String], message: &[u8]) -> String {
    let record = {
        let _lock = ae::LOCK.lock().await;
        let Some(mut record) = ae::load(&guard.folder, agent) else {
            return "451 4.3.0 The agent's address was just turned off".into();
        };
        record.roll_day();
        let new: Vec<&String> = rcpts.iter().filter(|r| !record.allows(r)).collect();
        if let Some(first) = new.first() {
            return match ae::hold(&guard.folder, agent, from, rcpts, message) {
                Ok(id) => {
                    (guard.notify)(agent.to_string(), first.to_string(), id);
                    format!(
                        "550 5.7.1 Not sent yet: {first} isn't among the people allowed. The person was asked in \
                         the BipAgents app; it goes as soon as they approve"
                    )
                }
                Err(e) => {
                    tracing::warn!(agent, error = %e, "held mail not kept");
                    "451 4.3.0 Not sent: try again later".into()
                }
            };
        }
        if record.sent_today >= record.daily_limit {
            return format!(
                "550 5.7.1 Not sent: the daily limit of {} e-mails is reached. Tomorrow, or the person raises it \
                 in the app",
                record.daily_limit
            );
        }
        record.sent_today += 1;
        if ae::save(&guard.folder, &record).is_err() {
            return "451 4.3.0 Not sent: try again later".into();
        }
        record
    };
    let sent = deliver(&record, from, rcpts, message).await;
    let _lock = ae::LOCK.lock().await;
    let mut now = ae::load(&guard.folder, agent);
    match sent {
        Ok(()) => {
            if let Some(r) = now.as_mut().filter(|r| r.last_error.is_some()) {
                r.last_error = None;
                let _ = ae::save(&guard.folder, r);
            }
            "250 2.0.0 Sent".into()
        }
        Err(e) => {
            let why = format!("{e:#}");
            tracing::warn!(agent, error = %why, "agent e-mail not sent");
            if let Some(r) = now.as_mut() {
                r.sent_today = r.sent_today.saturating_sub(1);
                r.last_error = Some(crate::agents_admin::tail(&why, 200));
                let _ = ae::save(&guard.folder, r);
            }
            "451 4.4.1 The mail server didn't take it: try again later".into()
        }
    }
}

// -- the client to the real server ---------------------------------------------------------------------------------

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

fn tls() -> tokio_rustls::TlsConnector {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(config))
}

async fn wrap(stream: Box<dyn Io>, host: &str) -> anyhow::Result<Box<dyn Io>> {
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).context("the server's name")?;
    Ok(Box::new(tls().connect(name, stream).await.context("TLS")?))
}

/// A server's answer (its last line's code) and its text.
async fn answer(r: &mut BufReader<Box<dyn Io>>) -> anyhow::Result<(u16, String)> {
    let mut all = String::new();
    loop {
        let line = read_line(r).await?;
        if line.len() < 3 {
            bail!("the server answered {line:?}");
        }
        all.push_str(&line);
        all.push('\n');
        if line.as_bytes().get(3) != Some(&b'-') {
            let code = line[..3].parse().map_err(|_| anyhow!("the server answered {line:?}"))?;
            return Ok((code, all));
        }
    }
}

async fn command(r: &mut BufReader<Box<dyn Io>>, line: &str, want: u16) -> anyhow::Result<String> {
    r.get_mut().write_all(format!("{line}\r\n").as_bytes()).await?;
    r.get_mut().flush().await?;
    let (code, text) = answer(r).await?;
    if code / 100 != want / 100 {
        let shown = if line.starts_with("AUTH") { "AUTH" } else { line };
        bail!("{shown}: {}", text.trim());
    }
    Ok(text)
}

/// The mail passed on to the agent's real server (its own login), every recipient at once.
pub async fn deliver(record: &Record, from: &str, rcpts: &[String], message: &[u8]) -> anyhow::Result<()> {
    let host = record.smtp_host.as_str();
    let port = record.smtp_port;
    let tcp = tokio::time::timeout(TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| anyhow!("{host}:{port} doesn't answer"))?
        .with_context(|| format!("{host}:{port}"))?;
    let stream: Box<dyn Io> = Box::new(tcp);
    let mut r =
        if record.smtp_security == "tls" { BufReader::new(wrap(stream, host).await?) } else { BufReader::new(stream) };
    let (code, text) = answer(&mut r).await?;
    if code != 220 {
        bail!("the server greeted {}", text.trim());
    }
    command(&mut r, "EHLO localhost", 250).await?;
    if record.smtp_security == "starttls" {
        command(&mut r, "STARTTLS", 220).await?;
        let plain = r.into_inner();
        r = BufReader::new(wrap(plain, host).await?);
        command(&mut r, "EHLO localhost", 250).await?;
    }
    let login = base64::engine::general_purpose::STANDARD.encode(format!("\0{}\0{}", record.login(), record.password));
    command(&mut r, &format!("AUTH PLAIN {login}"), 235).await?;
    command(&mut r, &format!("MAIL FROM:<{from}>"), 250).await?;
    for rcpt in rcpts {
        command(&mut r, &format!("RCPT TO:<{rcpt}>"), 250).await?;
    }
    command(&mut r, "DATA", 354).await?;
    let mut stuffed = Vec::with_capacity(message.len() + 64);
    for line in message.split_inclusive(|b| *b == b'\n') {
        if line.starts_with(b".") {
            stuffed.push(b'.');
        }
        stuffed.extend_from_slice(line);
    }
    if !stuffed.ends_with(b"\r\n") {
        stuffed.extend_from_slice(b"\r\n");
    }
    stuffed.extend_from_slice(b".\r\n");
    r.get_mut().write_all(&stuffed).await?;
    r.get_mut().flush().await?;
    let (code, text) = answer(&mut r).await?;
    if code != 250 {
        bail!("the message: {}", text.trim());
    }
    let _ = command(&mut r, "QUIT", 221).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_in_commands() {
        assert_eq!(path_address("<Léa@X.test> SIZE=12"), "léa@x.test");
        assert_eq!(path_address(" a@b.test"), "a@b.test");
        assert_eq!(path_address("<>"), "");
    }
}
