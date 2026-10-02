//! Bot and account mail. A message is built here exactly as Action Mailer 8.1 and the mail gem 2.9.1 build it for this
//! app's mailers: one text/html part, UTF-8, the headers in the gem's order, the subject folded and Q-encoded word by
//! word, the body sent as 7bit, quoted-printable or base64, whichever the gem would pick. Only Date and Message-ID are
//! this process's own. Pinned by rust/tests/mail.rs (vectors from script/rust/mail_vectors.rb) and, for whole mails
//! against Rails, by rust/tests/mail_parity.rs.
//!
//! One rule is stricter than the gem's: no value that becomes a header may hold a control character (a line break in
//! a display name or a label would otherwise write a header of its own). Such a message is refused, not repaired.
use base64::Engine as _;
use chrono::{DateTime, Utc};

/// One mail as a mailer action leaves it: `mail(from:, reply_to:, to:, subject:)` and one HTML body.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// AppConfig.notifications_sender, as configured: `noreply@example.com` or `My App <noreply@example.com>`.
    pub from: String,
    pub reply_to: Option<String>,
    /// The recipient's address, bare.
    pub to: String,
    pub subject: String,
    pub html: String,
}

/// A sender as the header shows it and as the SMTP envelope names it.
#[derive(Debug, Clone, PartialEq)]
pub struct Mailbox { pub header: String, pub address: String }

/// Mail::Constants::PHRASE_UNSAFE: a display name holding one of these is written in double quotes.
fn phrase_unsafe(name: &str) -> bool { name.chars().any(|c| c.is_ascii_control() || "()<>[]:;@\\,.\"".contains(c)) }

/// Whether a value may become (part of) a header: no control character, a line break least of all.
pub fn clean(value: &str) -> bool { !value.chars().any(char::is_control) }

/// A host name: ASCII labels of letters, digits and hyphens, joined by dots, 253 characters at most.
pub fn host_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 253
        && name.split('.').all(|label| !label.is_empty() && label.len() <= 63 && !label.starts_with('-') && !label.ends_with('-')
                                       && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
}

/// An address as it may stand in MAIL FROM, RCPT TO and a header: `local@host`, the local part of RFC 5321's
/// dot-string characters. Quoted local parts and anything outside ASCII are refused (the mail gem would write them).
pub fn address_ok(address: &str) -> bool {
    let Some((local, host)) = address.rsplit_once('@') else { return false };
    !local.is_empty() && address.len() <= 254 && !local.starts_with('.') && !local.ends_with('.') && !local.contains("..")
        && local.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+/=?^_`{|}~.-".contains(&b))
        && host_ok(host)
}

/// The sender value as the mail gem writes an address field: a bare address, or `Name <address>` with the name quoted
/// when it needs it. None for a blank value (the gem then writes no header and refuses to deliver) and for one that
/// holds a control character. A display name outside ASCII is dropped (a listed divergence: the gem B-encodes it).
pub fn mailbox(value: &str) -> Option<Mailbox> {
    let value = value.trim();
    if value.is_empty() || !clean(value) { return None; }
    let Some((name, address)) = value.strip_suffix('>').and_then(|v| v.rsplit_once('<')) else {
        return Some(Mailbox { header: value.to_string(), address: value.to_string() });
    };
    let (name, address) = (name.trim(), address.trim());
    // Mail::Utilities.unquote: a quoted name loses its quotes and its backslash escapes before it is judged again.
    let name = match name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(inner) => { let mut out = String::new(); let mut chars = inner.chars(); while let Some(c) = chars.next() { out.push(if c == '\\' { chars.next().unwrap_or('\\') } else { c }); } out }
        None => name.to_string(),
    };
    let header = if name.is_empty() || !name.is_ascii() {
        address.to_string()
    } else if phrase_unsafe(&name) {
        format!("\"{}\" <{address}>", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("{name} <{address}>")
    };
    Some(Mailbox { header, address: address.to_string() })
}

fn hex(out: &mut String, byte: u8) { out.push_str(&format!("={byte:02X}")); }

/// Mail::UnstructuredField#encode for one word of a subject that is not plain ASCII (no control character gets here).
fn q_word(word: &str) -> String {
    let mut out = String::new();
    for &b in word.as_bytes() {
        match b {
            b' ' => out.push('_'),
            b'"' | b'(' | b')' | b'?' | b'_' | b'=' => hex(&mut out, b),
            b if b > 126 => hex(&mut out, b),
            b => out.push(b as char),
        }
    }
    out
}

/// Mail::UnstructuredField#wrapped_value for `Subject`: the whole field, folded at 78 columns; a subject outside ASCII
/// becomes one encoded-word per line (`=?UTF-8?Q?…?=`), each word encoded on its own so no character is split.
fn subject_field(subject: &str) -> String {
    let encode = !subject.is_ascii();
    // String#split(/[ \t]/): empty strings at the end are dropped, those in front and in the middle are kept.
    let mut split: Vec<&str> = subject.split(' ').collect();
    while split.last() == Some(&"") { split.pop(); }
    let words: Vec<String> = if !encode {
        split.iter().map(|w| w.to_string()).collect()
    } else {
        split.iter().enumerate().flat_map(|(i, w)| {
            let word = if i == 0 { w.to_string() } else { format!(" {w}") };
            if !word.is_ascii() { return vec![word]; }
            // word.scan(/.{7}|.+$/): pieces of seven characters.
            word.as_bytes().chunks(7).map(|c| String::from_utf8_lossy(c).into_owned()).collect()
        }).collect()
    };
    let mut lines: Vec<String> = vec![];
    let mut prepend = "Subject: ".len();
    let mut queue = words.into_iter().peekable();
    while queue.peek().is_some() {
        let limit = 78 - prepend - if encode { 7 + "UTF-8".len() } else { 0 };
        let (mut line, mut first) = (String::new(), true);
        while let Some(next) = queue.peek() {
            let word = if encode { q_word(next) } else { next.clone() };
            if !line.is_empty() && line.len() + word.len() + 1 > limit { break; }
            queue.next();
            if first { first = false } else if !encode { line.push(' ') }
            line.push_str(&word);
        }
        lines.push(if encode { format!("=?UTF-8?Q?{line}?=") } else { line });
        prepend = 0;
    }
    format!("Subject: {}", lines.join("\r\n "))
}

/// Ruby's `[text].pack("M")` (pack.c, qpencode): `=XX` for bytes above 126, below 32 (but not line feed or tab) and `=`;
/// a soft break after 73 columns; a space or tab before a line feed is protected by a soft break; a last line without a
/// line feed ends in one.
fn pack_m(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    let (mut n, mut prev) = (0usize, 0u8);
    for &b in text.as_bytes() {
        if b > 126 || (b < 32 && b != b'\n' && b != b'\t') || b == b'=' {
            hex(&mut out, b);
            n += 3;
            prev = 0;
        } else if b == b'\n' {
            if prev == b' ' || prev == b'\t' { out.push_str("=\n"); }
            out.push('\n');
            n = 0;
            prev = b;
        } else {
            out.push(b as char);
            n += 1;
            prev = b;
        }
        if n > 72 {
            out.push_str("=\n");
            n = 0;
            prev = b'\n';
        }
    }
    if n > 0 { out.push_str("=\n"); }
    out
}

/// Mail::Utilities.binary_unsafe_to_lf and .binary_unsafe_to_crlf.
fn to_lf(text: &str) -> String { text.replace("\r\n", "\n").replace('\r', "\n") }
fn to_crlf(text: &str) -> String { to_lf(text).replace('\n', "\r\n") }

/// Mail::Body#negotiate_best_encoding for a 7bit transport, then Mail::Body#encoded: the Content-Transfer-Encoding and
/// the body as sent.
/// - ASCII with no line over 998 bytes: 7bit.
/// - Otherwise the cheaper of quoted-printable (three bytes for each byte it must escape) and base64 (4/3);
///   quoted-printable on a tie (its PRIORITY is lower).
fn transfer_encode(html: &str) -> (&'static str, String) {
    let short_lines = html.split_inclusive('\n').all(|line| line.len() <= 998);
    if html.is_ascii() && short_lines { return ("7bit", to_crlf(html)); }
    let safe = html.bytes().filter(|b| matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x3C | 0x3E..=0x7E)).count();
    let quoted_cost = ((html.len() - safe) * 3 + safe) as f64 / html.len() as f64;
    if quoted_cost <= 4.0 / 3.0 {
        // An ASCII body is decoded as 7bit first (line endings to LF); quoted-printable does the same to any body.
        return ("quoted-printable", to_crlf(&pack_m(&to_lf(html))));
    }
    // `[text].pack("m")`: lines of 60 characters, each ended. The source bytes go in as they are: a body outside ASCII
    // is "8bit" to the gem, which decodes nothing.
    let b64 = base64::engine::general_purpose::STANDARD.encode(html.as_bytes());
    let mut out = String::with_capacity(b64.len() + b64.len() / 30 + 2);
    for line in b64.as_bytes().chunks(60) {
        out.push_str(&String::from_utf8_lossy(line));
        out.push_str("\r\n");
    }
    ("base64", out)
}

impl Message {
    /// The message on the wire (CRLF line ends, ASCII only). `date` and `message_id` are the two fields that are this
    /// process's own; everything else is what Rails would send. Err names the first value that may not be sent: a
    /// sender or recipient that is not an address, or a header value with a control character in it.
    pub fn encode(&self, date: DateTime<Utc>, message_id: &str) -> Result<String, &'static str> {
        let from = mailbox(&self.from).filter(|m| address_ok(&m.address)).ok_or("the sender is blank, not an address, or holds a control character")?;
        let reply_to = match &self.reply_to {
            Some(value) => Some(mailbox(value).filter(|m| address_ok(&m.address)).ok_or("the reply address is not an address")?),
            None => None,
        };
        let to = self.to.trim();
        if !address_ok(to) { return Err("the recipient is not an address this crate sends to"); }
        if !clean(&self.subject) { return Err("the subject holds a control character"); }
        if !clean(message_id) || message_id.contains(' ') { return Err("the message id holds a space or a control character"); }
        let mut out = format!("Date: {}\r\nFrom: {}\r\n", date.format("%a, %d %b %Y %H:%M:%S +0000"), from.header);
        if let Some(reply_to) = reply_to { out.push_str(&format!("Reply-To: {}\r\n", reply_to.header)); }
        out.push_str(&format!("To: {to}\r\nMessage-ID: {message_id}\r\n"));
        if !self.subject.is_empty() { out.push_str(&subject_field(&self.subject)); out.push_str("\r\n"); }
        let (encoding, body) = transfer_encode(&self.html);
        out.push_str(&format!("MIME-Version: 1.0\r\nContent-Type: text/html;\r\n charset=UTF-8\r\nContent-Transfer-Encoding: {encoding}\r\n\r\n"));
        out.push_str(&body);
        Ok(out)
    }
}
