//! One SMTP submission against a fake server in this process: the wire dialogue (STARTTLS, required when there is a user
//! name to protect; AUTH PLAIN; the envelope; one command at a time), the limits the client holds whatever the server
//! does (nothing read before TLS counts after it; a bounded reply; bounded time), what counts as delivered, and that a
//! failure names a stage and a code and nothing the server, the configuration or the message wrote. Rails-free.
mod common;
use common::smtp::{self, Behaviour};
use deltabadger::mail::smtp::{deliver, Env, Failure, Settings};
use std::time::Duration;

const MESSAGE: &str = "Subject: test\r\n\r\nfirst line\r\n.a line that starts with a dot\r\nthe-body-says-hello\r\n";
const PLAIN_TOKEN: &str = "AGFsaWNlAHMzY3JldC1wdw=="; // base64("\0alice\0s3cret-pw")

fn settings(port: u16, credentials: Option<(&str, &str)>) -> Settings {
    let env = Env::read(&|name| match name {
        "SMTP_ADDRESS" => Some("127.0.0.1".into()),
        "SMTP_PORT" => Some(port.to_string()),
        "SMTP_DOMAIN" => Some("bots.example.com".into()),
        "SMTP_USER_NAME" => credentials.map(|(user, _)| user.to_string()),
        "SMTP_PASSWORD" => credentials.map(|(_, password)| password.to_string()),
        _ => None,
    });
    let mut s = Settings::current(&env, &|_| None).expect("SMTP_ADDRESS is set");
    s.trust_root = Some(smtp::TLS_PEM.as_bytes().to_vec());
    s.read_timeout = Duration::from_secs(2);
    s
}
fn alice(port: u16) -> Settings { settings(port, Some(("alice", "s3cret-pw"))) }
async fn send(s: &Settings) -> Result<(), Failure> { deliver(s, "bots@example.com", "owner@example.com", MESSAGE).await }
/// What a log line would carry.
fn said(failure: &Failure) -> String { failure.to_string() }

#[tokio::test]
async fn starttls_then_auth_plain_then_the_envelope_and_the_message_one_command_at_a_time() {
    let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
    deliver(&alice(server.port), "My Bots <bots@example.com>", "owner@example.com", MESSAGE).await.unwrap();
    let session = &server.sessions()[0];
    // No `[pipelined]`: every command waited for the answer to the one before it.
    assert_eq!(session.lines, ["EHLO bots.example.com", "STARTTLS", "[tls]", "EHLO bots.example.com", &format!("AUTH PLAIN {PLAIN_TOKEN}"),
                               "MAIL FROM:<bots@example.com>", "RCPT TO:<owner@example.com>", "DATA", "QUIT"]);
    // Net::SMTP's bytes: the message, dot-stuffed on the wire, ended once.
    assert_eq!(session.message.as_deref(), Some(MESSAGE));
}

#[tokio::test]
async fn a_server_without_starttls_is_refused_when_a_user_name_is_set_and_nothing_secret_is_sent() {
    // The listed divergence: Rails (`enable_starttls_auto`) would go on and send AUTH PLAIN in the clear.
    let server = smtp::start(Behaviour { starttls: false, auth: true, ..Default::default() }).await;
    let failure = send(&alice(server.port)).await.unwrap_err();
    assert_eq!(said(&failure), "tls: 127.0.0.1 offers no STARTTLS, and a user name or password is never sent unencrypted");
    assert_eq!(server.sessions()[0].lines, ["EHLO bots.example.com"]);
    assert!(server.messages().is_empty());
    // A password alone counts too.
    let failure = send(&settings(server.port, Some(("", "s3cret-pw")))).await.unwrap_err();
    assert_eq!(failure.stage, "tls");
}

#[tokio::test]
async fn a_server_without_starttls_gets_the_mail_in_the_clear_when_there_is_no_user_name_as_rails_sends_it() {
    // An open relay configured by SMTP_ADDRESS alone. Rails still sends AUTH, with empty credentials (a Rails defect, ported),
    // and sends it whether or not the server offers AUTH.
    let server = smtp::start(Behaviour { starttls: false, auth: false, ..Default::default() }).await;
    deliver(&settings(server.port, None), "noreply@localhost", "owner@example.com", MESSAGE).await.unwrap();
    assert_eq!(server.sessions()[0].lines, ["EHLO bots.example.com", "AUTH PLAIN AAA=", "MAIL FROM:<noreply@localhost>", "RCPT TO:<owner@example.com>", "DATA", "QUIT"]);
    assert_eq!(server.messages(), [MESSAGE]);
}

#[tokio::test]
async fn a_server_that_offers_starttls_gets_it_without_a_user_name_too() {
    let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
    send(&settings(server.port, None)).await.unwrap();
    assert_eq!(server.sessions()[0].lines[..4], ["EHLO bots.example.com", "STARTTLS", "[tls]", "EHLO bots.example.com"]);
}

#[tokio::test]
async fn a_certificate_the_client_does_not_trust_ends_the_delivery_with_or_without_a_user_name() {
    // Never a downgrade: not to plain text, and not for the relay without credentials either.
    for credentials in [Some(("alice", "s3cret-pw")), None] {
        let server = smtp::start(Behaviour { starttls: true, auth: true, ..Default::default() }).await;
        let mut s = settings(server.port, credentials);
        s.trust_root = None; // the platform's roots: the test certificate is not among them
        let failure = send(&s).await.unwrap_err();
        assert_eq!(said(&failure), "tls: the handshake failed or the certificate did not verify");
        assert_eq!(server.sessions()[0].lines, ["EHLO bots.example.com", "STARTTLS"]);
        assert!(server.messages().is_empty());
    }
}

#[tokio::test]
async fn plaintext_sent_with_the_answer_to_starttls_fails_the_delivery_and_is_never_read_as_an_answer() {
    // The server (or someone on the path) sends, in the clear and together with the `220`, what looks like the answers
    // to everything the client is about to say over TLS, an acceptance of the message included. The server itself,
    // over TLS, would refuse the message. A client that kept the plaintext would report the mail as sent.
    let forged = "250-fake.test\r\n250 AUTH PLAIN LOGIN\r\n235 2.7.0 ok\r\n250 ok\r\n250 ok\r\n354 go\r\n250 2.0.0 forged\r\n221 bye\r\n";
    let server = smtp::start(Behaviour { starttls: true, auth: true, forge_with_starttls: Some(forged), reply: Some((".", "554 5.7.1 refused")), ..Default::default() }).await;
    let failure = send(&alice(server.port)).await.unwrap_err();
    assert_eq!(said(&failure), "tls: the server sent data before the TLS handshake");
    assert_eq!(server.sessions()[0].lines, ["EHLO bots.example.com", "STARTTLS"], "no handshake, no credential, no message");
}

#[tokio::test]
async fn a_reply_is_bounded_before_it_is_stored() {
    // 64 MiB with no line end: the client stops reading at one line's worth.
    let server = smtp::start(Behaviour { flood: Some(64 * 1024 * 1024), ..Default::default() }).await;
    let failure = send(&alice(server.port)).await.unwrap_err();
    assert_eq!(said(&failure), "greeting: the reply is too long");
    assert!(server.flooded() < 8 * 1024 * 1024, "the client took {} bytes of the flood (what is past 1 KiB sits in the kernel's buffers)", server.flooded());
    // A reply of too many lines.
    let endless = "220-more\r\n".repeat(100);
    let server = smtp::start(Behaviour { greeting: Some(endless), ..Default::default() }).await;
    assert_eq!(said(&send(&alice(server.port)).await.unwrap_err()), "greeting: the reply is too long");
    // A line of exactly the limit, with no end.
    let server = smtp::start(Behaviour { greeting: Some("2".repeat(1024)), ..Default::default() }).await;
    assert_eq!(said(&send(&alice(server.port)).await.unwrap_err()), "greeting: the reply is too long");
}

#[tokio::test]
async fn a_reply_is_parsed_strictly() {
    for greeting in ["220 bare line feed\n", "22 two digits\r\n", "220x no separator\r\n", "hello\r\n", "220-first\r\n250 another code\r\n", "199 too low\r\n",
                     "600 too high\r\n", " 220 leading space\r\n", "\r\n"] {
        let server = smtp::start(Behaviour { greeting: Some(greeting.into()), ..Default::default() }).await;
        assert_eq!(said(&send(&alice(server.port)).await.unwrap_err()), "greeting: the reply is not SMTP", "{greeting:?}");
        assert!(server.sessions()[0].lines.is_empty(), "{greeting:?}: nothing is said to a server that does not speak SMTP");
    }
    // What is allowed: several lines of one code, and a line that is only the code.
    let server = smtp::start(Behaviour { greeting: Some("220-fake.test\r\n220-two\r\n220\r\n".into()), ..Default::default() }).await;
    send(&settings(server.port, None)).await.unwrap();
    // A code the client did not expect is a failure with that code.
    let server = smtp::start(Behaviour { greeting: Some("554 5.3.2 not accepting mail from owner@example.com\r\n".into()), ..Default::default() }).await;
    assert_eq!(said(&send(&alice(server.port)).await.unwrap_err()), "greeting: 554 5.3.2");
}

#[tokio::test]
async fn a_failure_names_its_stage_and_code_and_nothing_the_server_wrote() {
    let failures: [(&str, &str, Behaviour); 7] = [
        ("the login refused, the server echoing what it was sent", "auth: 535 5.7.8",
         Behaviour { starttls: true, reply: Some(("AUTH", "535 5.7.8 bad credentials for alice: s3cret-pw AGFsaWNlAHMzY3JldC1wdw==")), ..Default::default() }),
        ("the sender refused", "mail: 550 5.7.1", Behaviour { starttls: true, reply: Some(("MAIL", "550 5.7.1 bots@example.com may not send")), ..Default::default() }),
        ("the recipient refused, the server echoing the address", "rcpt: 550 5.1.1",
         Behaviour { starttls: true, reply: Some(("RCPT", "550 5.1.1 <owner@example.com>: no such user owner@example.com")), ..Default::default() }),
        ("the message refused, the server echoing its text", "data: 554 5.7.1",
         Behaviour { starttls: true, reply: Some((".", "554 5.7.1 rejected: the-body-says-hello looks like spam")), ..Default::default() }),
        ("try later", "data: 451 4.3.0", Behaviour { starttls: true, reply: Some((".", "451 4.3.0 try again later")), ..Default::default() }),
        ("a code with no enhanced status, and text that only looks like one", "data: 552", Behaviour { starttls: true, reply: Some((".", "552 4.3.0 wrong class 5.3.4")), ..Default::default() }),
        ("the connection dies after the message, before the answer", "data: the connection closed", Behaviour { starttls: true, hang_up_after_message: true, ..Default::default() }),
    ];
    for (what, expected, behaviour) in failures {
        let server = smtp::start(behaviour).await;
        let s = alice(server.port);
        let failure = send(&s).await.expect_err(what);
        assert_eq!(said(&failure), expected, "{what}");
        for secret in ["s3cret-pw", "alice", PLAIN_TOKEN, "owner@example.com", "bots@example.com", "the-body-says-hello"] {
            assert!(!format!("{failure} {failure:?} {s:?}").contains(secret), "{what}: {failure:?}");
        }
    }
}

#[tokio::test]
async fn time_is_bounded_for_a_command_and_for_the_whole_delivery() {
    let server = smtp::start(Behaviour { silent: true, ..Default::default() }).await;
    let mut s = settings(server.port, None);
    s.read_timeout = Duration::from_millis(300);
    let started = std::time::Instant::now();
    assert_eq!(said(&send(&s).await.unwrap_err()), "greeting: no answer in time");
    assert!(started.elapsed() < Duration::from_secs(2));
    // Commands that each answer in time, and a delivery that still takes too long as a whole.
    s.read_timeout = Duration::from_secs(5);
    s.total_timeout = Duration::from_millis(300);
    assert_eq!(said(&send(&s).await.unwrap_err()), "delivery: not finished within 0 s");

    let closed = { let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap(); l.local_addr().unwrap().port() }; // bound, then dropped
    assert_eq!(said(&send(&settings(closed, None)).await.unwrap_err()), "connect: no connection");
    let mut bad_port = settings(closed, None);
    bad_port.port = "smtp".into();
    assert_eq!(said(&send(&bad_port).await.unwrap_err()), "connect: the port is not a number");
}

#[tokio::test]
async fn an_address_that_could_carry_a_command_is_refused_before_anything_is_connected() {
    let server = smtp::start(Behaviour::default()).await;
    let s = settings(server.port, None);
    for from in ["  ", "not an address", "bots@example.com\r\nRCPT TO:<evil@example.com>", "Ops\r\nBcc: evil@example.com <bots@example.com>", "a b@example.com", "bots@exa mple.com"] {
        assert_eq!(said(&deliver(&s, from, "owner@example.com", MESSAGE).await.unwrap_err()), "address: the sender is not an address", "{from:?}");
    }
    for to in ["", "owner@example.com\r\nDATA", "owner@example.com> SIZE=1", "<owner@example.com>", "żółw@example.com", "owner@", "a@b@c d"] {
        assert_eq!(said(&deliver(&s, "bots@example.com", to, MESSAGE).await.unwrap_err()), "address: the recipient is not an address", "{to:?}");
    }
    assert!(server.sessions().is_empty());
}

/// Once the server has answered the end of DATA with a 2xx, the mail is delivered. QUIT is a courtesy with a short
/// deadline of its own: unanswered, or cut off by the delivery's deadline, it never turns an accepted mail into a
/// failure (which the sender would answer by sending the mail again).
#[tokio::test]
async fn a_mail_the_server_accepted_is_delivered_whatever_becomes_of_quit() {
    // Accepted 1.5 s into a delivery that may take 3 s, and QUIT never answered: the deadline falls while QUIT is out.
    let server = smtp::start(Behaviour { starttls: true, auth: true, accept_after: Some(Duration::from_millis(1500)), silent_quit: true, ..Default::default() }).await;
    let mut s = alice(server.port);
    s.read_timeout = Duration::from_secs(5);
    s.total_timeout = Duration::from_secs(3);
    let started = std::time::Instant::now();
    assert_eq!(send(&s).await, Ok(()));
    assert!(started.elapsed() < Duration::from_secs(4), "the delivery's deadline still holds: {:?}", started.elapsed());
    assert_eq!((server.messages().len(), server.sessions()[0].lines.last().map(String::as_str)), (1, Some("QUIT")));

    // No deadline near: an unanswered QUIT is given up after its own short wait, not the delivery's 30 s.
    let server = smtp::start(Behaviour { starttls: true, auth: true, silent_quit: true, ..Default::default() }).await;
    let mut s = alice(server.port);
    s.read_timeout = Duration::from_millis(300);
    let started = std::time::Instant::now();
    assert_eq!(send(&s).await, Ok(()));
    assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    assert_eq!(server.messages().len(), 1);

    // What the server refused at the end of DATA is still refused.
    let server = smtp::start(Behaviour { starttls: true, auth: true, reply: Some((".", "451 4.3.0 try again later")), silent_quit: true, ..Default::default() }).await;
    assert_eq!(said(&send(&alice(server.port)).await.unwrap_err()), "data: 451 4.3.0");
}

/// One command, one reply. A reply the client did not ask for, already in its buffer when it is about to speak, is a
/// broken dialogue and ends the delivery: read later, it would pass for the answer to something else. Here the server
/// answers DATA with its 354 and a stale 250 in one write, and then refuses the message: the stale 250 must never be
/// taken for the message's acceptance.
#[tokio::test]
async fn a_reply_nobody_asked_for_ends_the_delivery_and_is_never_taken_for_an_acceptance() {
    let server = smtp::start(Behaviour { starttls: true, auth: true, data_go: Some("354 continue\r\n250 stale\r\n"), reply: Some((".", "554 rejected")), ..Default::default() }).await;
    assert_eq!(said(&send(&alice(server.port)).await.unwrap_err()), "data: the server sent a reply nobody asked for");
    assert!(server.messages().is_empty(), "the message was not sent after it");
    // The same before any other command: two greetings at once.
    let server = smtp::start(Behaviour { greeting: Some("220 fake.test ESMTP\r\n250 stale\r\n".into()), ..Default::default() }).await;
    assert_eq!(said(&send(&settings(server.port, None)).await.unwrap_err()), "ehlo: the server sent a reply nobody asked for");
    assert!(server.sessions()[0].lines.is_empty(), "nothing was said to it");
}

/// AUTH PLAIN in two steps (see the next test): the server's challenge to PLAIN is empty. One that is not (it does not
/// decode, or it decodes to something) is not PLAIN's: the exchange is cancelled with `*` and the token never sent.
#[tokio::test]
async fn a_challenge_to_auth_plain_that_is_not_empty_cancels_the_exchange_and_the_token_is_never_sent() {
    use base64::Engine as _;
    let (user, password) = ("u".repeat(116), "p".repeat(255)); // too long for one line
    let token = base64::engine::general_purpose::STANDARD.encode(format!("\0{user}\0{password}"));
    // The last two are white space after the separator: `334` and `334 ` are the empty challenge, `334  ` is not.
    for challenge in ["!!!", "VXNlcm5hbWU6", "=", " ", "\t"] {
        let server = smtp::start(Behaviour { starttls: true, auth: true, challenge: Some(challenge), ..Default::default() }).await;
        let failure = send(&settings(server.port, Some((&user, &password)))).await.unwrap_err();
        assert_eq!(said(&failure), "auth: the server's challenge to AUTH PLAIN is not empty", "{challenge:?}");
        let lines = server.sessions()[0].lines.clone();
        assert_eq!(lines.iter().skip_while(|line| !line.starts_with("AUTH")).collect::<Vec<_>>(), ["AUTH PLAIN", "*"], "{challenge}");
        assert!(!lines.contains(&token) && server.messages().is_empty(), "{challenge}");
    }
}

/// A command line is at most 512 bytes, CRLF included (RFC 5321), and the fake server here holds that. AUTH PLAIN
/// carries its response on the same line while that fits; when it would not, the command goes alone and the response
/// follows the server's 334 on a line of its own (RFC 4954, section 4). Credentials are bounded where they are
/// configured, so the response has a largest size.
#[tokio::test]
async fn auth_plain_goes_on_one_line_while_the_command_fits_in_512_bytes_and_on_two_when_it_does_not() {
    use base64::Engine as _;
    let password = "p".repeat(255);
    for (user_length, command_length) in [(115, 509), (116, 513)] {
        let server = smtp::start(Behaviour { starttls: true, auth: true, command_limit: true, ..Default::default() }).await;
        let user = "u".repeat(user_length);
        let token = base64::engine::general_purpose::STANDARD.encode(format!("\0{user}\0{password}"));
        assert_eq!(format!("AUTH PLAIN {token}\r\n").len(), command_length);
        send(&settings(server.port, Some((&user, &password)))).await.unwrap();
        let lines = server.sessions()[0].lines.clone();
        let auth: Vec<&String> = lines.iter().skip_while(|line| !line.starts_with("AUTH")).take_while(|line| !line.starts_with("MAIL")).collect();
        if command_length <= 512 { assert_eq!(auth, [&format!("AUTH PLAIN {token}")]); } else { assert_eq!(auth, [&"AUTH PLAIN".to_string(), &token]); }
        assert_eq!(server.messages().len(), 1, "delivered with a {user_length}-byte user name");
    }
    // The bound: 255 bytes each. More is "not configured", with a reason that names no credential.
    let current = |user: String, password: String| Settings::current(&Env::read(&move |name| match name {
        "SMTP_ADDRESS" => Some("mail.example.com".into()), "SMTP_USER_NAME" => Some(user.clone()), "SMTP_PASSWORD" => Some(password.clone()), _ => None }), &|_| None);
    assert!(current("u".repeat(255), "p".repeat(255)).is_ok());
    for (user, password) in [("u".repeat(256), "p".into()), ("u".into(), "p".repeat(256)), ("\u{17c}".repeat(128), "p".into())] {
        assert_eq!(current(user, password).unwrap_err(), "the SMTP user name or password is longer than 255 bytes");
    }
    // NUL separates the parts of a PLAIN response, so neither part may hold one (RFC 4616, section 2).
    for (user, password) in [("al\0ice".to_string(), "p".to_string()), ("alice".into(), "p\0".into()), ("\0".into(), "".into())] {
        assert_eq!(current(user, password).unwrap_err(), "the SMTP user name or password holds a NUL character");
    }
}

#[test]
fn an_ehlo_name_or_a_server_name_that_is_not_a_host_name_means_not_configured() {
    let current = |address: &str, domain: Option<&str>| {
        let (address, domain) = (address.to_string(), domain.map(str::to_string));
        Settings::current(&Env::read(&move |name| match name { "SMTP_ADDRESS" => Some(address.clone()), "SMTP_DOMAIN" => domain.clone(), _ => None }), &|_| None)
    };
    for domain in ["bots.example.com", "localhost", "[192.0.2.1]", "[IPv6:2001:db8::1]"] {
        assert_eq!(current("mail.example.com", Some(domain)).unwrap().domain, domain);
    }
    for domain in ["", " ", "bots.example.com\r\nMAIL FROM:<evil@example.com>", "two words", "-leading.example.com", "[not an address]", "[2001:db8::1]", "a..b", "ex\u{e4}mple.com"] {
        assert_eq!(current("mail.example.com", Some(domain)).unwrap_err(), "SMTP_DOMAIN is not a host name or an address literal", "{domain:?}");
    }
    for address in ["mail.example.com", "192.0.2.7", "2001:db8::7"] { assert!(current(address, None).is_ok(), "{address}"); }
    for address in ["mail.example.com\r\nevil", "mail example.com", "mail.example.com:587", "[192.0.2.7]"] {
        assert_eq!(current(address, None).unwrap_err(), "the SMTP server's name is not a host name or an IP address", "{address:?}");
    }
}
