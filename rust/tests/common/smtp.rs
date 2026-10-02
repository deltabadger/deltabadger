//! A fake SMTP server in the test's own runtime: it speaks enough of RFC 5321 for one delivery, records every line the
//! client sends (and whether TLS was on), and can be told to misbehave at one point of the dialogue.
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};

/// The certificate the server presents when STARTTLS is used, and the root a test hands the client to trust it.
pub const TLS_PEM: &str = include_str!("../fixtures/smtp_test_tls.pem");

#[derive(Clone, Default)]
pub struct Behaviour {
    /// Offer STARTTLS in EHLO, and upgrade when asked.
    pub starttls: bool,
    /// Offer `AUTH PLAIN LOGIN` in EHLO (after the upgrade when STARTTLS is offered, as real servers do).
    pub auth: bool,
    /// Answer this command (its first word, upper case; `.` for the end of the message) with this line instead of success.
    pub reply: Option<(&'static str, &'static str)>,
    /// Accept the connection and never say a word.
    pub silent: bool,
    /// Read the whole message, then close the connection without answering.
    pub hang_up_after_message: bool,
    /// Sent in the same write as the `220` that answers STARTTLS, before the handshake: plaintext a client must never
    /// take for an answer to anything it says after the upgrade.
    pub forge_with_starttls: Option<&'static str>,
    /// Instead of a greeting: this many bytes with no line end in them.
    pub flood: Option<usize>,
    /// Instead of `220 fake.test ESMTP` and its line end: these bytes, as they are.
    pub greeting: Option<String>,
    /// Hold RFC 5321's limit: a command line longer than 512 bytes, CRLF included, is answered `500`.
    pub command_limit: bool,
    /// Wait this long before answering the end of the message.
    pub accept_after: Option<std::time::Duration>,
    /// Never answer QUIT (and keep the connection open).
    pub silent_quit: bool,
    /// Instead of `354 end with a dot` and its line end: these bytes, in one write (a reply nobody asked for can ride along).
    pub data_go: Option<&'static str>,
    /// What follows `334 ` when AUTH PLAIN comes without its response: the challenge. PLAIN's is empty.
    pub challenge: Option<&'static str>,
}

/// What one connection sent: its lines in order (`[tls]` marks the upgrade, `[pipelined]` a command that arrived before
/// the answer to the one before it), and the message between DATA and the dot.
#[derive(Clone, Debug, Default)]
pub struct Session { pub lines: Vec<String>, pub message: Option<String> }

pub struct Fake { pub port: u16, sessions: Arc<Mutex<Vec<Session>>>, flooded: Arc<std::sync::atomic::AtomicUsize>, _task: tokio::task::JoinHandle<()> }

impl Fake {
    pub fn sessions(&self) -> Vec<Session> { self.sessions.lock().unwrap().clone() }
    pub fn messages(&self) -> Vec<String> { self.sessions().into_iter().filter_map(|s| s.message).collect() }
    /// How many bytes of a flood the clients took before they stopped reading.
    pub fn flooded(&self) -> usize { self.flooded.load(std::sync::atomic::Ordering::SeqCst) }
}
impl Drop for Fake { fn drop(&mut self) { self._task.abort(); } }

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

fn acceptor() -> tokio_rustls::TlsAcceptor {
    use tokio_rustls::rustls;
    let cert = CertificateDer::from_pem_slice(TLS_PEM.as_bytes()).expect("the test certificate");
    let key = PrivateKeyDer::from_pem_slice(TLS_PEM.as_bytes()).expect("the test key");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::aws_lc_rs::default_provider()))
        .with_safe_default_protocol_versions().expect("protocol versions")
        .with_no_client_auth().with_single_cert(vec![cert], key).expect("a server config");
    tokio_rustls::TlsAcceptor::from(Arc::new(config))
}

pub async fn start(behaviour: Behaviour) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let sessions = Arc::new(Mutex::new(vec![]));
    let log = sessions.clone();
    let flooded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let taken = flooded.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let (behaviour, log, taken) = (behaviour.clone(), log.clone(), taken.clone());
            tokio::spawn(async move {
                let index = { let mut all = log.lock().unwrap(); all.push(Session::default()); all.len() - 1 };
                if let Some(total) = behaviour.flood {
                    let chunk = [b'a'; 16 * 1024];
                    while taken.load(std::sync::atomic::Ordering::SeqCst) < total {
                        if socket.write_all(&chunk).await.is_err() { return; }
                        taken.fetch_add(chunk.len(), std::sync::atomic::Ordering::SeqCst);
                    }
                    return;
                }
                serve(Box::new(socket), behaviour, &log, index).await;
            });
        }
    });
    Fake { port, sessions, flooded, _task: task }
}

async fn serve(socket: Box<dyn Stream>, b: Behaviour, log: &Mutex<Vec<Session>>, index: usize) {
    if b.silent { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; return; }
    let mut io = BufReader::new(socket);
    let mut tls = false;
    let record = |line: &str| log.lock().unwrap()[index].lines.push(line.to_string());
    let greeting = b.greeting.clone().unwrap_or_else(|| "220 fake.test ESMTP\r\n".into());
    if io.get_mut().write_all(greeting.as_bytes()).await.is_err() { return; }
    loop {
        let mut line = String::new();
        if io.read_line(&mut line).await.unwrap_or(0) == 0 { return; }
        let too_long = b.command_limit && line.len() > 512;
        let line = line.trim_end().to_string();
        record(&line);
        if too_long {
            if io.get_mut().write_all(b"500 5.5.2 line too long\r\n").await.is_err() { return; }
            continue;
        }
        // A client that waits for each answer has sent nothing more by now.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        if !io.buffer().is_empty() { record("[pipelined]"); }
        let verb = line.split(' ').next().unwrap_or("").to_ascii_uppercase();
        if let Some((_, reply)) = b.reply.filter(|(at, _)| *at == verb) {
            if io.get_mut().write_all(format!("{reply}\r\n").as_bytes()).await.is_err() { return; }
            continue;
        }
        let answer = match verb.as_str() {
            "EHLO" => {
                let mut offers = vec!["250-fake.test", "250-8BITMIME"];
                if b.starttls && !tls { offers.push("250-STARTTLS"); }
                if b.auth && (tls || !b.starttls) { offers.push("250-AUTH PLAIN LOGIN"); }
                format!("{}\r\n250 SIZE 10485760\r\n", offers.join("\r\n"))
            }
            "STARTTLS" => {
                let go = format!("220 go ahead\r\n{}", b.forge_with_starttls.unwrap_or(""));
                if io.get_mut().write_all(go.as_bytes()).await.is_err() { return; }
                let Ok(upgraded) = acceptor().accept(io.into_inner()).await else { return };
                io = BufReader::new(Box::new(upgraded) as Box<dyn Stream>);
                tls = true;
                record("[tls]");
                continue;
            }
            // Without an initial response: 334, then the response on a line of its own (RFC 4954), of any length.
            "AUTH" if line == "AUTH PLAIN" => {
                if io.get_mut().write_all(format!("334 {}\r\n", b.challenge.unwrap_or("")).as_bytes()).await.is_err() { return; }
                let mut response = String::new();
                if io.read_line(&mut response).await.unwrap_or(0) == 0 { return; }
                record(response.trim_end());
                // `*` cancels the exchange (RFC 4954, section 4).
                if response.trim_end() == "*" { "501 5.7.0 cancelled\r\n".to_string() } else { "235 2.7.0 accepted\r\n".to_string() }
            }
            "AUTH" => "235 2.7.0 accepted\r\n".to_string(),
            "MAIL" | "RCPT" => "250 ok\r\n".to_string(),
            "DATA" => {
                if io.get_mut().write_all(b.data_go.unwrap_or("354 end with a dot\r\n").as_bytes()).await.is_err() { return; }
                let mut message = String::new();
                loop {
                    let mut data = String::new();
                    if io.read_line(&mut data).await.unwrap_or(0) == 0 { return; }
                    if data == ".\r\n" { break; }
                    message.push_str(data.strip_prefix('.').unwrap_or(&data)); // undo dot-stuffing
                }
                log.lock().unwrap()[index].message = Some(message);
                if b.hang_up_after_message { return; }
                if let Some(wait) = b.accept_after { tokio::time::sleep(wait).await; }
                match b.reply.filter(|(at, _)| *at == ".") { Some((_, reply)) => format!("{reply}\r\n"), None => "250 2.0.0 queued\r\n".to_string() }
            }
            "QUIT" if b.silent_quit => { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; return; }
            "QUIT" => { let _ = io.get_mut().write_all(b"221 bye\r\n").await; return; }
            _ => "250 ok\r\n".to_string(),
        };
        if io.get_mut().write_all(answer.as_bytes()).await.is_err() { return; }
    }
}
