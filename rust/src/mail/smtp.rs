//! Where mail goes and how: SmtpSettings.current, AppConfig.notifications_sender, and one SMTP submission with those
//! settings, as the mail gem's Mail::SMTP makes it: EHLO, STARTTLS, EHLO, AUTH PLAIN, MAIL, RCPT, DATA, QUIT.
//! Once the server has answered the end of DATA with a 2xx the mail is delivered, whatever becomes of QUIT.
//!
//! The client is this crate's own, over tokio and rustls (the TLS stack reqwest already links), because it has to hold
//! rules no SMTP crate tried here holds:
//! - **Nothing read before TLS is trusted after it.** At the moment of the upgrade the read buffer must be empty: a byte
//!   received after the server's `220` and before the handshake fails the delivery. The capabilities are read again.
//! - **Replies are bounded before they are stored**: 1024 bytes a line, 64 lines a reply, read into a buffer that never
//!   grows past one line. A reply is parsed strictly (three digits, then a space, a hyphen or the end; CRLF; one code).
//! - **Time is bounded**: `open_timeout` to connect, `read_timeout` for each command's reply, `total_timeout` for the
//!   whole delivery.
//! - **No pipelining**: one command, one reply. A reply the client did not ask for, already in its buffer when it is
//!   about to send a command or the message, fails the delivery: read later, it would pass for another answer.
//! - **A failure says where and with what code, and nothing the server wrote**: the stage, the reply code, the
//!   enhanced status code. Never the reply's text (it can echo an address or the message), never a credential.
//!
//! - **A user name or a password is never sent in the clear.** With either configured, STARTTLS is required: a server
//!   that does not offer it is a failed delivery, before AUTH, as Rails' `enable_starttls: :always` does. With neither
//!   configured (an open relay), plain text stays allowed (`enable_starttls_auto`), as in Rails. A certificate that does
//!   not verify is a failure in both cases, never a downgrade.
//!
//! One rule differs from Rails (a listed divergence):
//! - **Nothing configured means nothing is sent.** `Settings::current` is then `Err` with the reason. Rails delivers
//!   to localhost:25. An EHLO name or a server name that is not a host name counts as not configured.
use super::{address_ok, host_ok, mailbox};
use base64::Engine as _;
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{self, ClientConfig, RootCertStore};

/// The environment's part of the configuration, read once at start. Values are kept as written: Rails tests
/// SMTP_ADDRESS with `blank?`, and reads the rest with `ENV.fetch`, which returns an empty value as it is.
/// It holds the password, so it has no `Debug`.
#[derive(Clone, Default)]
pub struct Env {
    pub address: Option<String>,
    pub port: Option<String>,
    pub domain: Option<String>,
    pub user_name: Option<String>,
    password: Option<String>,
    pub notifications_sender: Option<String>,
}

impl Env {
    pub fn read(env: &dyn Fn(&str) -> Option<String>) -> Self {
        Self { address: env("SMTP_ADDRESS"), port: env("SMTP_PORT"), domain: env("SMTP_DOMAIN"), user_name: env("SMTP_USER_NAME"),
               password: env("SMTP_PASSWORD"), notifications_sender: env("NOTIFICATIONS_SENDER") }
    }
}

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub address: String,
    /// As configured (`"587"`, `""`): read as a number only when connecting.
    pub port: String,
    /// The EHLO name: a host name or an address literal.
    pub domain: String,
    /// User name and password. AUTH PLAIN is always sent, with empty ones too: Net::SMTP authenticates whenever a user
    /// name is given, and the environment's settings always give one (a Rails defect, ported).
    pub credentials: (String, String),
    /// The mail gem's `open_timeout`: how long the TCP connection may take.
    pub open_timeout: Duration,
    /// The mail gem's `read_timeout`, here for each command's whole reply (the TLS handshake counts as one).
    pub read_timeout: Duration,
    /// The whole delivery, connect to acceptance.
    pub total_timeout: Duration,
    /// Tests only: a PEM certificate to trust instead of the platform's roots.
    pub trust_root: Option<Vec<u8>>,
}

/// Never a credential.
impl std::fmt::Debug for Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Settings {{ address: {:?}, port: {:?}, domain: {:?} }}", self.address, self.port, self.domain)
    }
}

fn present(v: Option<String>) -> Option<String> { v.filter(|s| !s.trim().is_empty()) }

/// An EHLO argument: a host name, or an address literal (`[192.0.2.1]`, `[IPv6:2001:db8::1]`).
fn ehlo_ok(name: &str) -> bool {
    match name.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
        Some(literal) => literal.strip_prefix("IPv6:").map_or_else(|| literal.parse::<std::net::Ipv4Addr>().is_ok(), |v6| v6.parse::<std::net::Ipv6Addr>().is_ok()),
        None => host_ok(name),
    }
}

impl Settings {
    /// SmtpSettings.current: what a delivery uses, or why mail is not configured. `config` reads a decrypted
    /// `app_configs` value (AppConfig.get).
    /// - SMTP_ADDRESS set: the environment wins over anything saved in Settings (port 587, EHLO `localhost` by default).
    /// - else `smtp_provider` = `custom_smtp` with a user name and a password: that server (smtp.gmail.com:587 by default).
    /// - else not configured. (Rails then delivers to production.rb's localhost:25; this crate sends nothing.)
    ///
    /// A server name that is not a host name or an IP address, and an EHLO name that is not a host name or an address
    /// literal, are "not configured" too: neither may carry a space or a line break onto the wire.
    pub fn current(env: &Env, config: &dyn Fn(&str) -> Option<String>) -> Result<Self, String> {
        let settings = |address: String, port: String, domain: &str, credentials| -> Result<Self, String> {
            if !host_ok(&address) && address.parse::<std::net::IpAddr>().is_err() { return Err("the SMTP server's name is not a host name or an IP address".into()); }
            if !ehlo_ok(domain) { return Err("SMTP_DOMAIN is not a host name or an address literal".into()); }
            let (user, password): &(String, String) = &credentials;
            if user.len() > CREDENTIAL_LIMIT || password.len() > CREDENTIAL_LIMIT { return Err(format!("the SMTP user name or password is longer than {CREDENTIAL_LIMIT} bytes")); }
            // NUL separates the parts of an AUTH PLAIN response, so no part may hold one (RFC 4616, section 2).
            if user.contains('\0') || password.contains('\0') { return Err("the SMTP user name or password holds a NUL character".into()); }
            Ok(Self { address, port, domain: domain.to_string(), credentials, open_timeout: Duration::from_secs(5), read_timeout: Duration::from_secs(5),
                      total_timeout: Duration::from_secs(30), trust_root: None })
        };
        if let Some(address) = present(env.address.clone()) {
            return settings(address, env.port.clone().unwrap_or_else(|| "587".into()), &env.domain.clone().unwrap_or_else(|| "localhost".into()),
                            (env.user_name.clone().unwrap_or_default(), env.password.clone().unwrap_or_default()));
        }
        if config("smtp_provider").as_deref() == Some("custom_smtp") {
            if let (Some(user), Some(password)) = (present(config("smtp_username")), present(config("smtp_password"))) {
                // `(AppConfig.smtp_port.presence || '587').to_i`
                let port = crate::ruby::to_i(&present(config("smtp_port")).unwrap_or_else(|| "587".into()));
                return settings(present(config("smtp_host")).unwrap_or_else(|| "smtp.gmail.com".into()), port.to_string(), "localhost.localdomain", (user, password));
            }
        }
        Err("no SMTP_ADDRESS, and no SMTP settings saved".into())
    }

    /// Whether there is anything to keep from the wire: then the connection must be encrypted before AUTH.
    pub fn has_secret(&self) -> bool { !self.credentials.0.is_empty() || !self.credentials.1.is_empty() }
}

/// AppConfig.notifications_sender: NOTIFICATIONS_SENDER when it is set (an empty value too, which then sends nothing),
/// else the saved SMTP user name, else `noreply@localhost`.
pub fn notifications_sender(env: &Env, config: &dyn Fn(&str) -> Option<String>) -> String {
    env.notifications_sender.clone().or_else(|| present(config("smtp_username"))).unwrap_or_else(|| "noreply@localhost".into())
}

/// Why a delivery failed, in a form that is safe to log: where in the dialogue, and either the server's reply code
/// (with its enhanced status code when it sent one) or a fixed phrase of this crate's. Never the server's own text.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    /// `address`, `connect`, `greeting`, `ehlo`, `tls`, `auth`, `mail`, `rcpt`, `data`.
    pub stage: &'static str,
    pub code: Option<u16>,
    pub enhanced: Option<String>,
    pub what: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code, &self.enhanced) {
            (Some(code), Some(enhanced)) => write!(f, "{}: {code} {enhanced}", self.stage),
            (Some(code), None) => write!(f, "{}: {code}", self.stage),
            (None, _) => write!(f, "{}: {}", self.stage, self.what),
        }
    }
}

fn failed(stage: &'static str, what: impl Into<String>) -> Failure { Failure { stage, code: None, enhanced: None, what: what.into() } }

/// The enhanced status code (RFC 3463) a reply starts with, when it has one of the reply's own class (`5.7.8` in a
/// 535): the only part of the server's text that is kept.
fn enhanced_status(text: &str, code: u16) -> Option<String> {
    let first = text.split(' ').next()?;
    let parts: Vec<&str> = first.split('.').collect();
    let digits = |p: &&str, max: usize| (1..=max).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit());
    (parts.len() == 3 && parts[0] == (code / 100).to_string() && digits(&parts[1], 3) && digits(&parts[2], 3)).then(|| first.to_string())
}

const LINE_LIMIT: usize = 1024;
/// RFC 5321's limit for a command line, CRLF included.
const COMMAND_LIMIT: usize = 512;
/// The longest user name and the longest password, in bytes: more is "not configured". Together they are a 684-byte
/// AUTH PLAIN response, far above any real credential (an SES password is 44 characters).
pub const CREDENTIAL_LIMIT: usize = 255;
/// How long QUIT may take, at most: the mail is delivered by then.
const QUIT_WAIT: Duration = Duration::from_secs(2);
const REPLY_LINES: usize = 64;

struct Reply { code: u16, lines: Vec<String> }

/// One connection, plain or encrypted, with the bytes read from it and not yet used. The buffer never holds more than
/// one line's worth: a read asks only for what still fits.
struct Wire<S> { io: S, unread: Vec<u8>, read_timeout: Duration }

impl<S: AsyncRead + AsyncWrite + Unpin> Wire<S> {
    async fn line(&mut self, stage: &'static str) -> Result<String, Failure> {
        loop {
            if let Some(end) = self.unread.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.unread.drain(..=end).collect();
                let Some(text) = line.strip_suffix(b"\r\n") else { return Err(failed(stage, "the reply is not SMTP")) };
                return Ok(String::from_utf8_lossy(text).into_owned());
            }
            if self.unread.len() >= LINE_LIMIT { return Err(failed(stage, "the reply is too long")); }
            let mut chunk = [0u8; LINE_LIMIT];
            let room = LINE_LIMIT - self.unread.len();
            match self.io.read(&mut chunk[..room]).await {
                Ok(0) | Err(_) => return Err(failed(stage, "the connection closed")),
                Ok(n) => self.unread.extend_from_slice(&chunk[..n]),
            }
        }
    }

    /// One whole reply: `250-…` lines, then `250 …` (or a bare `250`). Every line carries the same code.
    async fn read_reply(&mut self, stage: &'static str) -> Result<Reply, Failure> {
        let mut reply = Reply { code: 0, lines: vec![] };
        loop {
            let line = self.line(stage).await?;
            let code = line.get(..3).filter(|d| d.bytes().all(|b| b.is_ascii_digit())).and_then(|d| d.parse::<u16>().ok()).filter(|c| (200..600).contains(c));
            let (Some(code), more) = (code, match line.as_bytes().get(3) { None | Some(b' ') => Some(false), Some(b'-') => Some(true), _ => None }) else {
                return Err(failed(stage, "the reply is not SMTP"));
            };
            let Some(more) = more else { return Err(failed(stage, "the reply is not SMTP")) };
            if !reply.lines.is_empty() && code != reply.code { return Err(failed(stage, "the reply is not SMTP")); }
            reply.code = code;
            reply.lines.push(line.get(4..).unwrap_or_default().to_string());
            if !more { return Ok(reply); }
            if reply.lines.len() >= REPLY_LINES { return Err(failed(stage, "the reply is too long")); }
        }
    }

    async fn reply(&mut self, stage: &'static str, expect: fn(u16) -> bool) -> Result<Reply, Failure> {
        let reply = tokio::time::timeout(self.read_timeout, self.read_reply(stage)).await.map_err(|_| failed(stage, "no answer in time"))??;
        if expect(reply.code) { return Ok(reply); }
        let enhanced = reply.lines.first().and_then(|text| enhanced_status(text, reply.code));
        Err(Failure { stage, code: Some(reply.code), enhanced, what: String::new() })
    }

    /// Writes a command or the message. Whatever the server said has been read and used by now, one reply per command:
    /// bytes still in the buffer are a reply nobody asked for, and would be taken for the answer to what is sent next.
    async fn send(&mut self, stage: &'static str, bytes: &[u8]) -> Result<(), Failure> {
        if !self.unread.is_empty() { return Err(failed(stage, "the server sent a reply nobody asked for")); }
        let write = async { self.io.write_all(bytes).await?; self.io.flush().await };
        match tokio::time::timeout(self.read_timeout, write).await {
            Ok(Ok(())) => Ok(()),
            _ => Err(failed(stage, "the connection closed")),
        }
    }

    async fn command(&mut self, stage: &'static str, line: &str, expect: fn(u16) -> bool) -> Result<Reply, Failure> {
        self.send(stage, format!("{line}\r\n").as_bytes()).await?;
        self.reply(stage, expect).await
    }

    /// EHLO, and whether the server offers STARTTLS. (Rails falls back to HELO when EHLO is refused; here that is a failure.)
    async fn ehlo(&mut self, domain: &str) -> Result<bool, Failure> {
        let reply = self.command("ehlo", &format!("EHLO {domain}"), |c| c == 250).await?;
        Ok(reply.lines.iter().skip(1).any(|line| line.split(' ').next().is_some_and(|keyword| keyword.eq_ignore_ascii_case("STARTTLS"))))
    }

    /// AUTH PLAIN, the envelope, the message, QUIT: the same on a plain and on an encrypted connection.
    /// `accepted` turns true the moment the server has answered the end of DATA with a 2xx: from then on the mail is
    /// delivered, and QUIT is a courtesy with a short deadline of its own.
    async fn submit(&mut self, s: &Settings, from: &str, to: &str, message: &str, accepted: &std::cell::Cell<bool>) -> Result<(), Failure> {
        let ok = |code: u16| code / 100 == 2;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("\0{}\0{}", s.credentials.0, s.credentials.1));
        let auth = format!("AUTH PLAIN {token}");
        if auth.len() + 2 <= COMMAND_LIMIT {
            self.command("auth", &auth, ok).await?;
        } else {
            // RFC 4954 section 4: an initial response that would make the command too long is not used; the response
            // goes on a line of its own after the server's 334.
            let challenge = self.command("auth", "AUTH PLAIN", |c| c == 334).await?;
            // PLAIN's challenge is empty: `334` or `334 ` (the separator is not part of it). One that is not (white
            // space, something undecodable, something said) belongs to another exchange: cancel with `*`, as RFC 4954
            // section 4 says, and never send the token after it.
            if challenge.lines.iter().any(|text| !text.is_empty()) {
                let _ = self.command("auth", "*", |_| true).await;
                return Err(failed("auth", "the server's challenge to AUTH PLAIN is not empty"));
            }
            self.command("auth", &token, ok).await?;
        }
        self.command("mail", &format!("MAIL FROM:<{from}>"), ok).await?;
        self.command("rcpt", &format!("RCPT TO:<{to}>"), ok).await?;
        self.command("data", "DATA", |c| c == 354).await?;
        // A line that starts with a dot gets a second one; the message ends with CRLF "." CRLF, as Net::SMTP ends it.
        let mut body = message.replace("\r\n.", "\r\n..");
        if body.starts_with('.') { body.insert(0, '.'); }
        if !body.ends_with("\r\n") { body.push_str("\r\n"); }
        body.push_str(".\r\n");
        self.send("data", body.as_bytes()).await?;
        self.reply("data", ok).await?;
        accepted.set(true);
        let _ = tokio::time::timeout(QUIT_WAIT.min(self.read_timeout), self.command("quit", "QUIT", ok)).await; // a lost QUIT changes nothing
        Ok(())
    }
}

fn tls_config(s: &Settings) -> Result<Arc<ClientConfig>, Failure> {
    let no = |_| failed("tls", "the TLS client could not be set up");
    let builder = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions().map_err(no)?;
    let builder = match &s.trust_root {
        Some(pem) => {
            let mut roots = RootCertStore::empty();
            for cert in CertificateDer::pem_slice_iter(pem).flatten() { roots.add(cert).map_err(no)?; }
            builder.with_root_certificates(roots)
        }
        None => builder.with_platform_verifier().map_err(no)?,
    };
    Ok(Arc::new(builder.with_no_client_auth()))
}

/// Sends `message` (the encoded mail) from `from` (the configured sender; a display name is allowed) to `to`.
/// Ok only when the server accepted the message.
pub async fn deliver(s: &Settings, from: &str, to: &str, message: &str) -> Result<(), Failure> {
    let sender = mailbox(from).filter(|m| address_ok(&m.address)).ok_or_else(|| failed("address", "the sender is not an address"))?;
    let to = to.trim();
    if !address_ok(to) { return Err(failed("address", "the recipient is not an address")); }
    // Net::SMTP raises SocketError for a port it cannot resolve.
    let port: u16 = s.port.trim().parse().map_err(|_| failed("connect", "the port is not a number"))?;
    let accepted = std::cell::Cell::new(false);
    let dialogue = async {
        let tcp = match tokio::time::timeout(s.open_timeout, tokio::net::TcpStream::connect((s.address.as_str(), port))).await {
            Ok(Ok(tcp)) => tcp,
            Ok(Err(_)) => return Err(failed("connect", "no connection")),
            Err(_) => return Err(failed("connect", "no answer in time")),
        };
        let mut plain = Wire { io: tcp, unread: Vec::with_capacity(LINE_LIMIT), read_timeout: s.read_timeout };
        plain.reply("greeting", |c| c == 220).await?;
        if !plain.ehlo(&s.domain).await? {
            // Required with a secret to protect (Rails' `:always`); otherwise `enable_starttls_auto`: plain when not offered.
            if s.has_secret() {
                return Err(failed("tls", format!("{} offers no STARTTLS, and a user name or password is never sent unencrypted", s.address)));
            }
            return plain.submit(s, &sender.address, to, message, &accepted).await;
        }
        plain.command("tls", "STARTTLS", |c| c == 220).await?;
        // Whatever arrived with the 220 was not protected by the handshake that follows, and must not be read as an
        // answer to anything said after it.
        if !plain.unread.is_empty() { return Err(failed("tls", "the server sent data before the TLS handshake")); }
        let name = ServerName::try_from(s.address.clone()).map_err(|_| failed("tls", "the server's name is not a host name"))?;
        let connector = tokio_rustls::TlsConnector::from(tls_config(s)?);
        let encrypted = match tokio::time::timeout(s.read_timeout, connector.connect(name, plain.io)).await {
            Ok(Ok(stream)) => stream,
            Ok(Err(_)) => return Err(failed("tls", "the handshake failed or the certificate did not verify")),
            Err(_) => return Err(failed("tls", "no answer in time")),
        };
        let mut wire = Wire { io: encrypted, unread: Vec::with_capacity(LINE_LIMIT), read_timeout: s.read_timeout };
        wire.ehlo(&s.domain).await?; // the capabilities read in the clear are forgotten
        wire.submit(s, &sender.address, to, message, &accepted).await
    };
    match tokio::time::timeout(s.total_timeout, dialogue).await {
        Ok(outcome) => outcome,
        // The deadline fell while QUIT was out: the server has the mail, and saying otherwise would send it twice.
        Err(_) if accepted.get() => Ok(()),
        Err(_) => Err(failed("delivery", format!("not finished within {} s", s.total_timeout.as_secs()))),
    }
}
