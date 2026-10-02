//! A venue's answer, read the way Rails reads it and within stated bounds, before anything is built from it.
//!
//! Rails parses an answer with `JSON.parse` (Faraday's `:json` middleware), which inside the app is Oj's, and stores
//! what it parsed, re-serialised by Oj's Rails encoder (ActiveRecord's JSON type). So what Rails holds, and what
//! `raw_data` gets, never has a key twice in one object (the last value wins, in the first one's place, at every
//! level) and is never nested more than 100 deep (`JSON::NestingError`: the fetch fails). This reader does the same
//! to the text itself. It keeps every scalar as the venue wrote it (`Node::text`: what a number is read from), and
//! prints the value as Rails stores it (`Node::stored`: a Float in Oj's sixteen digits, a string in its escapes).
//!
//! It also bounds what an answer may cost before any of it is allocated as a tree: a body inside its byte limit can
//! still hold millions of values (`[[],[],…]`) or keys. Every value and every key is counted against a budget while
//! the text is scanned, and a list is refused at its first item over the limit.
use std::collections::HashMap;

/// Ruby's `JSON.parse` default `max_nesting`: an array or object opened inside 100 others is refused.
pub const MAX_NESTING: usize = 100;

/// Why an answer was not read.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// Not JSON at all: what Clients::Alpaca#with_rescue reports as a failed request.
    NotJson,
    /// Nested deeper than Ruby's parser reads: Rails' fetch fails too ("Too deeply nested").
    TooDeep,
    /// More values and keys than the answer, or the run, may hold.
    OverBudget,
    /// A list with more items than the caller asked for.
    TooManyItems,
}

impl Refused {
    /// The text a sync fails with. It repeats nothing of the answer.
    pub fn text(&self, what: &str) -> String {
        match self {
            Self::NotJson | Self::TooDeep => format!("unreadable {what}"),
            Self::OverBudget => format!("{what} with more values than one answer may hold"),
            Self::TooManyItems => format!("{what} with more items than were asked for"),
        }
    }
}

/// How many values and keys may still be read. One budget can span several answers (a run's pages).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget(pub usize);

impl Budget {
    fn spend(&mut self) -> Result<(), Refused> {
        self.0 = self.0.checked_sub(1).ok_or(Refused::OverBudget)?;
        Ok(())
    }
}

/// One JSON value in Rails' canonical form. A scalar is its own text.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Scalar(String),
    Array(Vec<Node>),
    /// Members by decoded key, in the order of each key's first appearance.
    Object(Vec<(String, Node)>),
}

impl Node {
    /// A parsed value as a node: each scalar is its serialisation. (Tests, vectors, and the keys the port adds.)
    pub fn from_value(v: &serde_json::Value) -> Node {
        match v {
            serde_json::Value::Array(items) => Node::Array(items.iter().map(Node::from_value).collect()),
            serde_json::Value::Object(members) => Node::Object(members.iter().map(|(k, v)| (k.clone(), Node::from_value(v))).collect()),
            scalar => Node::Scalar(scalar.to_string()),
        }
    }

    /// The value as Rails stores it in a JSON column: `ActiveSupport::JSON.encode` of what `JSON.parse` held, which in
    /// the app is Oj's Rails encoder (config/initializers/oj.rb). Byte for byte what Rails writes to `raw_data`.
    pub fn stored(&self) -> String {
        let mut out = String::new();
        self.write_stored(&mut out);
        out
    }
    fn write_stored(&self, out: &mut String) {
        match self {
            Self::Scalar(s) if s.starts_with('"') => stored_string(&serde_json::from_str::<String>(s).unwrap_or_default(), out),
            Self::Scalar(s) if matches!(s.as_str(), "true" | "false" | "null") => out.push_str(s),
            Self::Scalar(s) => out.push_str(&stored_number(s)),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() { if i > 0 { out.push(','); } item.write_stored(out); }
                out.push(']');
            }
            Self::Object(members) => write_stored_members(members, out),
        }
    }

    /// The value as JSON text: scalars verbatim, keys written as JSON strings, no whitespace between tokens.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
    fn write(&self, out: &mut String) {
        match self {
            Self::Scalar(s) => out.push_str(s),
            Self::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() { if i > 0 { out.push(','); } item.write(out); }
                out.push(']');
            }
            Self::Object(members) => {
                out.push('{');
                for (i, (key, value)) in members.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    out.push_str(&serde_json::Value::String(key.clone()).to_string());
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }
}

fn write_stored_members(members: &[(String, Node)], out: &mut String) {
    out.push('{');
    for (i, (key, value)) in members.iter().enumerate() {
        if i > 0 { out.push(','); }
        stored_string(key, out);
        out.push(':');
        value.write_stored(out);
    }
    out.push('}');
}

/// An object's members as Rails stores them (`Node::stored` for an object held as its members).
pub fn stored_members(members: &[(String, Node)]) -> String {
    let mut out = String::new();
    write_stored_members(members, &mut out);
    out
}

/// A string as Oj's Rails encoder writes it with `escape_html_entities_in_json` on (Rails' default): the short
/// escapes for backspace, tab, newline, form feed and return, `\u00XX` for every other control character, `&`, `<`
/// and `>` as `\u0026`, `\u003c` and `\u003e`, U+2028 and U+2029 escaped, everything else as it is (DEL and all
/// non-ASCII raw, `/` unescaped).
fn stored_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{0c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 || matches!(c, '&' | '<' | '>' | '\u{2028}' | '\u{2029}') => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A JSON number as Rails stores it. Digits alone are an Integer, kept exactly whatever their length (`-0` is 0);
/// anything else is a Float, printed by `stored_float`.
///
/// Printed twice, as Rails prints it: assigning a value to a JSON attribute casts it through its own text
/// (`ActiveModel::Type::Helpers::Mutable#cast` is `deserialize(serialize(value))`), and saving serialises what that
/// read back. The second printing changes one thing: a double whose sixteen digits round up past the largest double
/// reads back as Infinity and is stored as `null`. (Sixteen digits otherwise read back as a double that prints the
/// same sixteen; and a Float printed without a point, `1234567890123457`, reads back as that Integer.)
fn stored_number(token: &str) -> String {
    fn printed(token: &str) -> String {
        if token == "null" { return token.to_string(); }
        if token.bytes().all(|b| b == b'-' || b.is_ascii_digit()) {
            return if token.bytes().all(|b| b == b'-' || b == b'0') { "0".into() } else { token.to_string() };
        }
        token.parse::<f64>().map_or_else(|_| "null".into(), stored_float)
    }
    printed(&printed(token))
}

/// A Float as Oj's Rails encoder writes it (oj 3.17.6, rails.c `dump_float`, with `Oj.optimize_rails`):
/// - zero, of either sign, is `0.0`; a value that is not finite is `null`;
/// - a whole number inside 64 bits is C's `%.1f` (`100.0`, `1000000000000000.0`);
/// - anything else is C's `%0.16g`: sixteen significant digits, trailing zeros dropped, an exponent (`1e-05`,
///   `1e+20`) below 10^-4 and from 10^16 on. So `0.30000000000000004` is stored as `0.3`;
/// - but when that text is seventeen characters or more and ends in `0001` or `9999` (Oj takes it for a rounding
///   artefact), it is Ruby's `Float#to_s` instead: the shortest digits that read back as the same double.
///
/// Not Ruby's own `Float#to_s`, which this crate has only as the digits behind `Float#to_d` (`ruby::BigDec`): that is
/// the shortest round-trip form (`0.30000000000000004`, `1.0e-05`), up to seventeen digits; Oj's is a fixed sixteen
/// with C's exponent, and loses the seventeenth.
pub fn stored_float(d: f64) -> String {
    if d == 0.0 { return "0.0".into(); }
    if !d.is_finite() { return "null".into(); }
    // `d == (double)(long long)d`. The cast of a double outside 64 bits is undefined in C: x86-64 gives the same
    // answer for every such value (so none compares equal), ARM saturates (so 2^63 alone does). This is x86-64's.
    if (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&d) && d == d.trunc() { return format!("{d:.1}"); }
    let (digits, exponent, sign) = decimal(&format!("{d:.15e}"));
    let g = if !(-4..16).contains(&exponent) {
        let mantissa = digits.trim_end_matches('0');
        let fraction = if mantissa.len() > 1 { format!(".{}", &mantissa[1..]) } else { String::new() };
        format!("{sign}{}{fraction}e{}{:02}", &mantissa[..1], if exponent < 0 { '-' } else { '+' }, exponent.abs())
    } else {
        let fixed = fixed(&digits, exponent);
        format!("{sign}{}", if fixed.contains('.') { fixed.trim_end_matches('0').trim_end_matches('.') } else { &fixed })
    };
    if g.len() >= 17 && (g.ends_with("0001") || g.ends_with("9999")) {
        // Float#to_s, for a value Ruby prints without an exponent (10^-4 up to 10^16: the only ones that get here).
        let (digits, exponent, sign) = decimal(&format!("{d:e}"));
        let fixed = fixed(&digits, exponent);
        return format!("{sign}{fixed}{}", if fixed.contains('.') { "" } else { ".0" });
    }
    g
}

/// Rust's `{:e}` output (`-1.2345e-7`) as its digits, its decimal exponent and its sign.
fn decimal(scientific: &str) -> (String, i32, &'static str) {
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((scientific, "0"));
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    (mantissa.chars().filter(char::is_ascii_digit).collect(), exponent.parse().unwrap_or(0), sign)
}

/// Digits `d.ddd × 10^exponent` written without an exponent.
fn fixed(digits: &str, exponent: i32) -> String {
    if exponent < 0 { return format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize)); }
    let whole = exponent as usize + 1;
    if digits.len() <= whole { return format!("{digits}{}", "0".repeat(whole - digits.len())); }
    format!("{}.{}", &digits[..whole], &digits[whole..])
}

struct Reader<'a> { bytes: &'a [u8], text: &'a str, at: usize }

impl<'a> Reader<'a> {
    fn space(&mut self) { while matches!(self.bytes.get(self.at), Some(b' ' | b'\t' | b'\n' | b'\r')) { self.at += 1; } }

    /// A string token, quotes included. Its escapes are checked by the caller, as soon as it is read.
    fn string(&mut self) -> Result<&'a str, Refused> {
        let start = self.at;
        self.at += 1;
        loop {
            match self.bytes.get(self.at) {
                None => return Err(Refused::NotJson),
                Some(b'\\') => self.at += 2,
                Some(b'"') => { self.at += 1; return self.text.get(start..self.at).ok_or(Refused::NotJson); }
                Some(_) => self.at += 1,
            }
        }
    }

    fn value(&mut self, depth: usize, budget: &mut Budget, max_items: Option<usize>) -> Result<Node, Refused> {
        self.space();
        budget.spend()?;
        match self.bytes.get(self.at) {
            Some(b'[') => {
                if depth >= MAX_NESTING { return Err(Refused::TooDeep); }
                self.at += 1;
                let mut items = vec![];
                self.space();
                if self.bytes.get(self.at) == Some(&b']') { self.at += 1; return Ok(Node::Array(items)); }
                loop {
                    if max_items.is_some_and(|max| items.len() >= max) { return Err(Refused::TooManyItems); }
                    items.push(self.value(depth + 1, budget, None)?);
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b']') => { self.at += 1; return Ok(Node::Array(items)); }
                        _ => return Err(Refused::NotJson),
                    }
                }
            }
            Some(b'{') => {
                if depth >= MAX_NESTING { return Err(Refused::TooDeep); }
                self.at += 1;
                let mut members: Vec<(String, Node)> = vec![];
                let mut place: HashMap<String, usize> = HashMap::new();
                self.space();
                if self.bytes.get(self.at) == Some(&b'}') { self.at += 1; return Ok(Node::Object(members)); }
                loop {
                    self.space();
                    if self.bytes.get(self.at) != Some(&b'"') { return Err(Refused::NotJson); }
                    budget.spend()?;
                    let key: String = serde_json::from_str(self.string()?).map_err(|_| Refused::NotJson)?;
                    self.space();
                    if self.bytes.get(self.at) != Some(&b':') { return Err(Refused::NotJson); }
                    self.at += 1;
                    let value = self.value(depth + 1, budget, None)?;
                    // A key met again: its last value, in its first place (a Ruby Hash).
                    match place.get(&key) {
                        Some(i) => members[*i].1 = value,
                        None => { place.insert(key.clone(), members.len()); members.push((key, value)); }
                    }
                    self.space();
                    match self.bytes.get(self.at) {
                        Some(b',') => self.at += 1,
                        Some(b'}') => { self.at += 1; return Ok(Node::Object(members)); }
                        _ => return Err(Refused::NotJson),
                    }
                }
            }
            // A scalar is checked where it stands, before anything can replace it: a value a later one of the same
            // key overwrites must still be JSON (Rails' parser refuses the whole body otherwise).
            Some(b'"') => {
                let token = self.string()?;
                serde_json::from_str::<String>(token).map_err(|_| Refused::NotJson)?;
                Ok(Node::Scalar(token.to_string()))
            }
            Some(_) => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(|b| !matches!(b, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r')) { self.at += 1; }
                let token = self.text.get(start..self.at).filter(|t| !t.is_empty()).ok_or(Refused::NotJson)?;
                if !serde_json::from_str::<serde_json::Value>(token).is_ok_and(|v| !v.is_array() && !v.is_object() && !v.is_string()) { return Err(Refused::NotJson); }
                Ok(Node::Scalar(token.to_string()))
            }
            None => Err(Refused::NotJson),
        }
    }
}

/// Reads one JSON document. `max_items` bounds a top-level list. The structure is this reader's; every scalar is
/// checked by serde_json as it is read (a number, a string's escapes, a literal), an overwritten one included, so
/// nothing that is not JSON gets through.
pub fn read(text: &str, budget: &mut Budget, max_items: Option<usize>) -> Result<Node, Refused> {
    let mut reader = Reader { bytes: text.as_bytes(), text, at: 0 };
    let node = reader.value(0, budget, max_items)?;
    reader.space();
    if reader.at != text.len() { return Err(Refused::NotJson); }
    Ok(node)
}

/// JSON read without losing an integer: one too large for 64 bits (which serde_json would round to a double) becomes
/// the string "<integer DIGITS>", so two texts that differ in such a number differ once parsed. Both halves of the
/// parity harness go through here: Rails' recorded output, and what Rust stored. Other numbers are compared as doubles, as
/// Ruby holds them. The text is scanned once; only what stands outside a string can be a number.
pub fn exact(text: &str) -> Result<serde_json::Value, serde_json::Error> {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut copied, mut in_string) = (0, 0, false);
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'-' | b'0'..=b'9' if !in_string => {
                let end = (i..bytes.len()).find(|&k| !matches!(bytes[k], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')).unwrap_or(bytes.len());
                let token = &text[i..end];
                let integer = token.bytes().all(|b| b == b'-' || b.is_ascii_digit());
                if integer && token.parse::<i64>().is_err() && token.parse::<u64>().is_err() {
                    out.push_str(&text[copied..i]);
                    out.push_str(&format!("\"<integer {token}>\""));
                    copied = end;
                }
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out.push_str(&text[copied..]);
    serde_json::from_str(&out)
}
