//! The BullionStar recorder's logic (PLAN-bullionstar.md slice 0): the allow-list Guard inside the transport (an order
//! is only ever sent with `testOrder: true`, the PIN only in an order body), the three flows (probe, read, orders), the
//! scrub of account answers and the typed observations that answer F1-F13. rust/tests/bullionstar_recorder.rs includes
//! this file so CI runs its tests; main.rs beside it only reads arguments and keys and builds the live transport.
#![allow(dead_code)] // the test crate that includes this file uses only part of it

#[path = "../common/scrub.rs"]
mod scrub;
pub use scrub::*;

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;

pub const URL: &str = "https://api.bullionstar.com";
pub const PREFIX: &str = "/api-gateway/v1";
/// The currencies a price read asks for: the tracker's fiat list (D-BS-QUOTE) and the venue's own SGD, HKD and NZD.
pub const CURRENCIES: &[&str] = &["USD", "EUR", "GBP", "CHF", "AUD", "CAD", "JPY", "PLN", "CZK", "SGD", "HKD", "NZD"];
pub const PRICE_BATCH: usize = 50;
pub const MAX_PAGES: usize = 200;

/// Every call the tool may send (path after PREFIX; `{id}` is digits), and whether it is an order (orders mode only).
pub const ALLOWED: &[(&str, &str, bool)] = &[
    ("GET", "/stock-location/", false),
    ("GET", "/product/", false),
    ("GET", "/product/detail/{id}", false),
    ("GET", "/product/price/", false),
    ("GET", "/vault/holdings", false),
    ("GET", "/svf/holdings", false),
    ("GET", "/payment-method/list", false),
    ("GET", "/sell-payment-method/list", false),
    ("GET", "/shipping-method/list", false),
    ("GET", "/order/get-by-id", false),
    ("GET", "/order/get-by-order-ref", false),
    ("POST", "/checkout/buy/price-lock-token", true),
    ("POST", "/checkout/vault-sell/price-lock-token", true),
    ("POST", "/checkout/buy/order", true),
    ("POST", "/checkout/vault-sell/order", true),
];

/// Field names kept (enum-shaped, after the shape check) in account answers, in place of the shared KEEP (whose `total`
/// would keep an amount), and the time names shifted beside the shared TIME. A product id is not kept: an order names
/// it as `product.id`, which is replaced, and a kept `productId` of the same value would then fail the leak check.
pub const KEEP_NAMES: &[&str] = &["status", "statusString", "type", "locationId", "categoryId", "metal", "fineWeightUnit", "currency", "mandatory"];
pub const TIME_NAMES: &[&str] = &["orderDate", "paymentDate", "expireTime", "promoExpireAt"];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode { Probe, Read, Orders }
impl Mode {
    pub fn dir(self) -> &'static str { match self { Mode::Probe => "probe", Mode::Read => "read", Mode::Orders => "orders" } }
}

// ---- the command line

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Order {
    pub location: i64, pub currency: String, pub gram: i64, pub bar: i64, pub payment: i64, pub shipping: i64,
    pub sell_payment: i64, pub unavailable: Option<i64>, pub wrong_pin: bool, pub expiry: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Command { Probe { label: String, location: Option<i64>, product: Option<i64> }, Read { details: Option<usize> }, Orders(Order) }
impl Command {
    pub fn mode(&self) -> Mode { match self { Command::Probe { .. } => Mode::Probe, Command::Read { .. } => Mode::Read, Command::Orders(_) => Mode::Orders } }
}

pub const USAGE: &str = "usage: record_bullionstar probe [--label whitelisted|off_whitelist] [--location ID --product ID]\n       record_bullionstar read [--details N]\n       record_bullionstar orders --location ID --currency CUR --gram PRODUCT --bar PRODUCT --payment ID --shipping ID --sell-payment ID [--unavailable PRODUCT] [--wrong-pin] [--expiry]";

/// Credentials are never an argument: refuse anything that looks like one, or holds the PIN.
pub fn refuse_key_args(args: &[String], pin: &str) -> Result<(), String> {
    if args.iter().any(|a| ["key", "secret", "token"].iter().any(|w| a.to_ascii_lowercase().contains(w)) || (!pin.is_empty() && a == pin)) {
        return Err("the key and PIN are read from BULLIONSTAR_KEY and BULLIONSTAR_PIN only, never from arguments".into());
    }
    Ok(())
}

pub fn parse_args(args: Vec<String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let mode = args.next().ok_or(USAGE)?;
    let mut flags: BTreeMap<String, Option<String>> = BTreeMap::new();
    let rest: Vec<String> = args.collect();
    let mut i = 0;
    while i < rest.len() {
        let f = rest[i].clone();
        if !f.starts_with("--") { return Err(USAGE.into()); }
        if matches!(f.as_str(), "--wrong-pin" | "--expiry") { flags.insert(f, None); i += 1; continue; }
        let v = rest.get(i + 1).filter(|v| !v.starts_with("--")).ok_or(USAGE)?.clone();
        flags.insert(f, Some(v));
        i += 2;
    }
    let take = |flags: &mut BTreeMap<String, Option<String>>, f: &str| flags.remove(f).flatten();
    let int = |v: Option<String>, f: &str| -> Result<i64, String> {
        v.ok_or_else(|| format!("{f} is required\n{USAGE}"))?.parse::<i64>().ok().filter(|n| *n > 0).ok_or(format!("{f} takes a positive id"))
    };
    let cmd = match mode.as_str() {
        "probe" => Command::Probe {
            label: take(&mut flags, "--label").unwrap_or_else(|| "whitelisted".into()),
            location: take(&mut flags, "--location").map(|v| int(Some(v), "--location")).transpose()?,
            product: take(&mut flags, "--product").map(|v| int(Some(v), "--product")).transpose()?,
        },
        "read" => Command::Read { details: take(&mut flags, "--details").map(|v| v.parse::<usize>().map_err(|_| "--details takes a count")).transpose()? },
        "orders" => {
            let currency = take(&mut flags, "--currency").ok_or(USAGE)?.to_ascii_uppercase();
            if currency.len() != 3 || !currency.bytes().all(|b| b.is_ascii_uppercase()) { return Err("--currency takes a 3-letter code".into()); }
            let o = Order {
                location: int(take(&mut flags, "--location"), "--location")?, currency,
                gram: int(take(&mut flags, "--gram"), "--gram")?, bar: int(take(&mut flags, "--bar"), "--bar")?,
                payment: int(take(&mut flags, "--payment"), "--payment")?, shipping: int(take(&mut flags, "--shipping"), "--shipping")?,
                sell_payment: int(take(&mut flags, "--sell-payment"), "--sell-payment")?,
                unavailable: take(&mut flags, "--unavailable").map(|v| int(Some(v), "--unavailable")).transpose()?,
                wrong_pin: flags.remove("--wrong-pin").is_some(), expiry: flags.remove("--expiry").is_some(),
            };
            Command::Orders(o)
        }
        _ => return Err(USAGE.into()),
    };
    if let Command::Probe { label, .. } = &cmd {
        if !["whitelisted", "off_whitelist"].contains(&label.as_str()) { return Err("--label is whitelisted or off_whitelist".into()); }
    }
    if !flags.is_empty() { return Err(USAGE.into()); }
    Ok(cmd)
}

// ---- the Guard: what may be sent, checked inside the transport

/// Whether `r` may be sent in `mode` with `pin` as the account PIN: an allowed path, an order only in orders mode and
/// only with `testOrder: true` and nothing but the three fields, and the PIN in no other request.
pub fn check(r: &HttpRequest, mode: Mode, pin: &str) -> Result<(), String> {
    // The key goes to the dealer's host only.
    if r.base != URL { return Err(format!("{} is not {URL}; refused", r.base)); }
    let path = r.path.strip_prefix(PREFIX).ok_or_else(|| format!("{} is outside {PREFIX}; refused", r.path))?;
    let shape = |want: &str| match want.strip_suffix("{id}") {
        Some(head) => path.strip_prefix(head).is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())),
        None => path == want,
    };
    let Some(&(_, want, order)) = ALLOWED.iter().find(|(m, p, _)| *m == r.method && shape(p)) else {
        return Err(format!("{} {path} is not on the allow-list; refused", r.method));
    };
    if order && mode != Mode::Orders { return Err(format!("{} {path} is only sent in orders mode; refused", r.method)); }
    // The PIN as a whole value: a path segment, a query value, a body value (a product id or a price may hold its digits).
    let pin_in = |v: &Value| { let mut hit = false; visit(v, "", &mut |text, _| hit |= !pin.is_empty() && text == pin); hit };
    if !pin.is_empty() && path.split('/').any(|seg| seg == pin) { return Err("the PIN is in the path; refused".into()); }
    if r.query.iter().any(|(_, v)| !pin.is_empty() && v == pin) { return Err("the PIN is in a query; refused".into()); }
    if want.ends_with("/order") {
        let body = r.body.as_ref().and_then(Value::as_object).ok_or("an order without a body; refused")?;
        if body.get("testOrder") != Some(&Value::Bool(true)) { return Err("an order without testOrder: true; refused".into()); }
        if body.keys().any(|k| !["priceLockToken", "pin", "testOrder"].contains(&k.as_str())) { return Err("an order body with another field; refused".into()); }
    } else if r.body.as_ref().is_some_and(pin_in) {
        return Err(format!("the PIN is in the body of {path}; refused"));
    }
    Ok(())
}

pub struct Guard<T> { pub inner: T, pub mode: Mode, pin: String }
impl<T: Transport> Guard<T> {
    pub fn new(inner: T, mode: Mode, pin: String) -> Self { Self { inner, mode, pin } }
}
impl<T: Transport> Transport for Guard<T> {
    fn now(&self) -> DateTime<Utc> { self.inner.now() }
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        check(r, self.mode, &self.pin).map_err(TransportError::Permanent)?;
        self.inner.send(r).await
    }
}

// ---- the record flow

pub struct Opts {
    /// Between two calls.
    pub pause: Duration,
    /// After a 429, times the attempt (reads only; an order is never re-sent).
    pub backoff: Duration,
    /// The longest wait for a lock to expire (--expiry), and the margin past its expireTime.
    pub wait_cap: Duration,
    pub margin: Duration,
}
impl Opts {
    pub fn live() -> Self { Self { pause: Duration::from_millis(250), backoff: Duration::from_secs(5), wait_cap: Duration::from_secs(20 * 60), margin: Duration::from_secs(10) } }
}

struct Got { file: String, method: &'static str, path: String, status: u16, body: Value, account: bool }

pub struct Outcome { pub dir: String, pub observations: Vec<String> }

struct Rec<'a, T> { t: &'a T, opts: &'a Opts, got: Vec<Got>, obs: Vec<String>, throttled: usize, orders_sent: usize }

fn get(path: &str, query: Vec<(&'static str, String)>) -> HttpRequest {
    HttpRequest { method: "GET", base: URL.into(), path: format!("{PREFIX}{path}"), query, body: None, not_after: None }
}
fn post(path: &str, body: Value) -> HttpRequest {
    HttpRequest { method: "POST", base: URL.into(), path: format!("{PREFIX}{path}"), query: vec![], body: Some(body), not_after: None }
}

fn parse(raw: &str) -> Value {
    if raw.trim().is_empty() { Value::Null } else { serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.into())) }
}

/// The answer's error message, when it is one (`{message, requestId}`) and `plain_message`; else it is withheld.
pub fn message(status: u16, body: &Value) -> String {
    let m = body.get("message").and_then(Value::as_str).unwrap_or("");
    match ((200..300).contains(&status), plain_message(m)) {
        (true, _) => String::new(),
        (false, true) => format!(" \"{m}\""),
        (false, false) => " (message withheld: it holds digits, an address or a long token)".into(),
    }
}

impl<T: Transport> Rec<'_, T> {
    /// One call; a read answered 429 is sent again after a backoff (three times at most). Recorded under `file`.
    async fn call(&mut self, file: &str, r: HttpRequest, account: bool) -> Result<(u16, Value), String> {
        let read = r.method == "GET";
        let mut attempt = 0;
        let resp = loop {
            if !self.opts.pause.is_zero() { tokio::time::sleep(self.opts.pause).await; }
            let sent = self.t.send(&r).await;
            // An order counts as sent once it was answered or its answer was lost; never when it provably left nothing.
            if r.path.ends_with("/order") && matches!(sent, Ok(_) | Err(TransportError::MaybeSent(_))) { self.orders_sent += 1; }
            let resp = sent.map_err(|e| match e {
                TransportError::Permanent(m) => format!("{file}: {m}; nothing written"),
                TransportError::NotSent(_) => format!("{file}: not sent (network); nothing written"),
                TransportError::MaybeSent(_) => format!("{file}: no answer (it may have reached the dealer); nothing written"),
            })?;
            if resp.status == 429 { self.throttled += 1; }
            if resp.status == 429 && read && attempt < 3 {
                attempt += 1;
                tokio::time::sleep(self.opts.backoff * attempt).await;
                continue;
            }
            break resp;
        };
        if long_integer(&resp.body) { return Err(format!("{file}: an integer of 20+ digits would be lossy; nothing written")); }
        let body = parse(&resp.body);
        self.got.push(Got { file: format!("{file}.json"), method: r.method, path: r.path.trim_start_matches(PREFIX).into(), status: resp.status, body: body.clone(), account });
        Ok((resp.status, body))
    }
    fn note(&mut self, line: String) { self.obs.push(line); }
    async fn step(&mut self, file: &str, r: HttpRequest) -> Result<(u16, Value), String> {
        let (s, b) = self.call(file, r, true).await?;
        let extra = match (b.pointer("/order/statusString").and_then(Value::as_str), b.pointer("/order/status")) {
            (Some(text), Some(code)) => format!(" status {code} \"{text}\""),
            _ => String::new(),
        };
        self.note(format!("{file}: HTTP {s}{}{extra}", message(s, &b)));
        Ok((s, b))
    }
}

/// Runs `cmd` over `t` (the Guard over the live transport), then scrubs, leak-checks and writes `out/<dir>/` all or
/// nothing. `wrong_key`/`no_key` are the probe's other transports. Prints nothing from a body; the observations (typed
/// facts, the dealer's own catalogue values, error messages) are returned for Jan to read, never written.
#[allow(clippy::too_many_arguments)]
pub async fn record<T: Transport, U: Transport>(t: &T, probes: Option<(&U, &U)>, cmd: &Command, key: &str, pin: &str, sc: Scrubber, opts: &Opts, out: &Path) -> Result<Outcome, String> {
    let mut rec = Rec { t, opts, got: vec![], obs: vec![], throttled: 0, orders_sent: 0 };
    let result = run_cmd(&mut rec, probes, cmd, pin).await;
    if rec.throttled > 0 { rec.obs.push(format!("F11: {} answers were 429", rec.throttled)); }
    // Once an order was sent, no failure may hide it: the error says so, with every balance line seen.
    let notice = sent_notice(rec.orders_sent, &rec.obs);
    let dir = result.map_err(|e| format!("{e}{notice}"))?;
    let Rec { got, obs, .. } = rec;
    let kept = write(got, key, pin, sc, &out.join(&dir)).map_err(|e| format!("{e}{notice}"))?;
    let mut obs = obs;
    obs.push(format!("messages kept verbatim in the fixtures (check none names a person): {}",
        if kept.is_empty() { "none".into() } else { kept.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(", ") }));
    Ok(Outcome { dir: out.join(dir).display().to_string(), observations: obs })
}

fn sent_notice(sent: usize, obs: &[String]) -> String {
    if sent == 0 { return String::new(); }
    let moved: String = obs.iter().filter(|l| l.contains("moved a balance")).map(|l| format!("\n  {l}")).collect();
    format!("\n{sent} test order(s) were sent in this run: check the account's balances and orders before rerunning.{moved}")
}

async fn run_cmd<T: Transport, U: Transport>(rec: &mut Rec<'_, T>, probes: Option<(&U, &U)>, cmd: &Command, pin: &str) -> Result<String, String> {
    Ok(match cmd {
        Command::Probe { label, location, product } => { probe(rec, probes.ok_or("the probe needs its keyless transports")?, *location, *product).await?; format!("probe_{label}") }
        Command::Read { details } => { read(rec, *details).await?; "read".into() }
        Command::Orders(o) => { orders(rec, o, pin).await?; "orders".into() }
    })
}

/// The PIN is short and numeric: it is looked for as a whole value or a `pin` field, not as a substring (a price can hold
/// its digits). The key is looked for anywhere.
fn pin_leak(name: &str, v: &Value, pin: &str) -> Result<(), String> {
    let mut hit = None;
    visit(v, "", &mut |text, path| if hit.is_none() && ((!pin.is_empty() && text == pin) || norm(text) == "pin") { hit = Some(path.to_string()) });
    match hit { Some(p) => Err(format!("{name}: the PIN or a pin field at {p}; nothing written")), None => Ok(()) }
}

/// Every non-ASCII character of every string turned into `x`, before the scrub: the shared scrubber only replaces ASCII
/// letters and digits, so a name in another script would pass through it unseen.
fn ascii(v: &mut Value) {
    match v {
        Value::String(s) if !s.is_ascii() => *s = s.chars().map(|c| if c.is_ascii() { c } else { 'x' }).collect(),
        Value::Array(a) => a.iter_mut().for_each(ascii),
        Value::Object(m) => {
            *m = std::mem::take(m).into_iter().map(|(k, mut v)| {
                ascii(&mut v);
                (if k.is_ascii() { k } else { k.chars().map(|c| if c.is_ascii() { c } else { 'x' }).collect() }, v)
            }).collect();
        }
        _ => {}
    }
}

/// An error message kept as the dealer wrote it (F6), in the fixture and in the printed lines: short ASCII text with
/// no digit at all (a PIN, an id, an IP), no `@` and no token of 16+ letters and digits (a key). Any other is scrubbed
/// in the fixture and withheld from the printed lines. A kept message can still name a person: the report lists every
/// kept one for review before commit.
pub fn plain_message(m: &str) -> bool {
    m.len() <= 200 && m.is_ascii() && !m.contains('@') && !m.bytes().any(|b| b.is_ascii_digit())
        && m.split(|c: char| !c.is_ascii_alphanumeric()).all(|w| w.len() < 16)
}

fn write(got: Vec<Got>, key: &str, pin: &str, mut sc: Scrubber, dir: &Path) -> Result<BTreeSet<String>, String> {
    let mut kept_messages = BTreeSet::new();
    // A public read is kept as it came only when it answered 2xx; an error or an account answer is scrubbed.
    // A plain error message is taken out first, judged on the dealer's own text, and kept out of the scrub (so it is no
    // replaced original); it is leak-checked with the rest. Then every other string of a scrubbed body is made ASCII.
    let got: Vec<(Got, bool, Option<Value>)> = got.into_iter().map(|mut g| {
        // A public answer is kept as it came only when it is a 2xx JSON list or object (not an HTML page, not empty).
        let scrub = g.account || !(200..300).contains(&g.status) || !(g.body.is_object() || g.body.is_array());
        let kept = match (&mut g.body, scrub && !(200..300).contains(&g.status)) {
            (Value::Object(m), true) if m.get("message").and_then(Value::as_str).is_some_and(plain_message) => m.remove("message"),
            _ => None,
        };
        if scrub { ascii(&mut g.body); }
        (g, scrub, kept)
    }).collect();
    for (g, _, _) in got.iter().filter(|(_, scrub, _)| *scrub) { sc.reserve(&g.body); }
    let mut files = Vec::new();
    let mut entries = Vec::new();
    for (g, scrub, kept) in got {
        let body = if scrub {
            let mut b = sc.scrub("bullionstar", &g.file, g.body)?;
            if let (Some(m), Value::Object(o)) = (kept, &mut b) {
                kept_messages.insert(m.as_str().unwrap_or_default().to_string());
                o.insert("message".into(), m);
            }
            b
        } else { g.body };
        entries.push(json!({ "file": g.file, "method": g.method, "path": g.path, "status": g.status, "scrubbed": scrub }));
        files.push((g.file, body, scrub));
    }
    let scrubbed: serde_json::Map<String, Value> = sc.report.iter().map(|(f, k)| (f.clone(), json!(k))).collect();
    let recorded_at = sc.shift_str(&Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)).expect("an RFC 3339 time shifts");
    let manifest = json!({ "exchange": "bullionstar", "recorded_at": recorded_at, "files": entries, "scrubbed": scrubbed });
    for (name, v, scrub) in &files {
        if *scrub { sc.leak_check(name, v, &[key])?; }
        else if !key.is_empty() && v.to_string().contains(key) { return Err(format!("{name}: the key survives; nothing written")); }
        pin_leak(name, v, pin)?;
    }
    // The manifest holds only file names, public paths, statuses and the scrub's counts: a public product id in a path
    // may equal a replaced holdings value, so it is checked for the credentials only.
    if !key.is_empty() && manifest.to_string().contains(key) { return Err("manifest.json: the key survives; nothing written".into()); }
    pin_leak("manifest.json", &manifest, pin)?;
    let mut files: Vec<(String, Value)> = files.into_iter().map(|(n, v, _)| (n, v)).collect();
    files.push(("manifest.json".into(), manifest));
    write_set(dir, &files)?;
    Ok(kept_messages)
}

// ---- probe (F1)

/// With the key, without one and with a wrong one, the same reads: the locations, one location's product page, one
/// product's price, the vault (F1: which need a key?). The location and product are `--location`/`--product`, else the
/// first ones the keyed reads list; the off-whitelist run takes the ids the whitelisted run printed.
async fn probe<T: Transport, U: Transport>(rec: &mut Rec<'_, T>, (no_key, wrong_key): (&U, &U), location: Option<i64>, product: Option<i64>) -> Result<(), String> {
    let (s, b) = rec.call("stock_locations", get("/stock-location/", vec![]), false).await?;
    rec.note(format!("F1 key: GET /stock-location/ → HTTP {s}{}", message(s, &b)));
    let loc = location.or_else(|| b.as_array().and_then(|a| a.first()).and_then(|l| l.get("id")).and_then(Value::as_i64));
    let mut reads = vec![("stock_locations", get("/stock-location/", vec![]))];
    let mut product = product;
    if let Some(loc) = loc {
        let (s, b) = rec.call("products", get("/product/", vec![("locationId", loc.to_string())]), false).await?;
        rec.note(format!("F1 key: GET /product/ → HTTP {s}{}", message(s, &b)));
        reads.push(("products", get("/product/", vec![("locationId", loc.to_string())])));
        product = product.or_else(|| ids(&b, "/items").first().copied());
    }
    if let Some(p) = product {
        let q = vec![("productIds", p.to_string()), ("currency", "USD".to_string())];
        let (s, b) = rec.call("prices", get("/product/price/", q.clone()), false).await?;
        rec.note(format!("F1 key: GET /product/price/ → HTTP {s}{}", message(s, &b)));
        reads.push(("prices", get("/product/price/", q)));
    }
    rec.note(format!("F1 probed location {} and product {} (pass them as --location/--product off the whitelist)",
        loc.map_or("(none)".into(), |l| l.to_string()), product.map_or("(none)".into(), |p| p.to_string())));
    let (s, b) = rec.call("vault_holdings", get("/vault/holdings", vec![]), true).await?;
    rec.note(format!("F1 key: GET /vault/holdings → HTTP {s}{}", message(s, &b)));
    reads.push(("vault_holdings", get("/vault/holdings", vec![])));
    for (who, t) in [("no_key", no_key), ("wrong_key", wrong_key)] {
        for (file, r) in &reads {
            let mut attempt = 0;
            let resp = loop {
                if !rec.opts.pause.is_zero() { tokio::time::sleep(rec.opts.pause).await; }
                let resp = t.send(r).await.map_err(|_| format!("{who} {file}: the request failed; nothing written"))?;
                if resp.status == 429 { rec.throttled += 1; }
                if resp.status == 429 && attempt < 3 { attempt += 1; tokio::time::sleep(rec.opts.backoff * attempt).await; continue; }
                break resp;
            };
            if long_integer(&resp.body) { return Err(format!("{who} {file}: an integer of 20+ digits would be lossy; nothing written")); }
            let body = parse(&resp.body);
            rec.note(format!("F1 {who}: {} {} → HTTP {}{}", r.method, r.path.trim_start_matches(PREFIX), resp.status, message(resp.status, &body)));
            rec.got.push(Got { file: format!("{who}.{file}.json"), method: "GET", path: r.path.trim_start_matches(PREFIX).into(), status: resp.status, body, account: true });
        }
    }
    Ok(())
}

// ---- read (F2, F3, F4, F9, F11)

fn ids(v: &Value, list: &str) -> Vec<i64> {
    v.pointer(list).and_then(Value::as_array).into_iter().flatten().filter_map(|x| x.get("id").and_then(Value::as_i64)).collect()
}

async fn read<T: Transport>(rec: &mut Rec<'_, T>, details: Option<usize>) -> Result<(), String> {
    let (s, locations) = rec.call("stock_locations", get("/stock-location/", vec![]), false).await?;
    if s != 200 { return Err(format!("stock_locations: HTTP {s}{}; nothing written", message(s, &locations))); }
    // A malformed catalogue answer stops the run: an empty set must never replace a complete recording.
    let locs: Option<Vec<(i64, String)>> = locations.as_array().filter(|a| !a.is_empty())
        .and_then(|a| a.iter().map(|l| Some((l.get("id")?.as_i64()?, l.get("name")?.as_str()?.to_string()))).collect());
    let locs = locs.ok_or("stock_locations: not a list of {id, name}; nothing written")?;
    rec.note(format!("locations: {}", locs.iter().map(|(i, n)| format!("{i} {n}")).collect::<Vec<_>>().join(", ")));
    let mut all: Vec<(i64, i64)> = vec![]; // (location, product)
    let mut per_location: Vec<BTreeSet<i64>> = vec![];
    for (loc, _) in &locs {
        let mut cursor: Option<String> = None;
        let mut pages = Vec::new();
        let mut total = None;
        for page in 0.. {
            if page == MAX_PAGES { return Err(format!("products {loc}: more than {MAX_PAGES} pages; nothing written")); }
            let mut q = vec![("locationId", loc.to_string())];
            if let Some(c) = &cursor { q.push(("cursor", c.clone())); }
            let (s, b) = rec.call(&format!("products.{loc}.page{page}"), get("/product/", q), false).await?;
            if s != 200 { return Err(format!("products {loc}: HTTP {s}{}; nothing written", message(s, &b))); }
            let items = b.get("items").and_then(Value::as_array).ok_or(format!("products {loc}: no items list; nothing written"))?;
            if items.iter().any(|i| !i.get("id").is_some_and(Value::is_i64)) { return Err(format!("products {loc}: an item without an id; nothing written")); }
            let page_ids = ids(&b, "/items");
            total = b.get("totalCount").and_then(Value::as_i64).or(total);
            let next = b.get("cursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(str::to_string);
            pages.push(page_ids.len());
            all.extend(page_ids.iter().map(|p| (*loc, *p)));
            if page_ids.is_empty() || next.is_none() || next == cursor { break; }
            cursor = next;
        }
        let mine: Vec<i64> = all.iter().filter(|(l, _)| l == loc).map(|(_, p)| *p).collect();
        let distinct: BTreeSet<i64> = mine.iter().copied().collect();
        let complete = total.is_none_or(|t| t == distinct.len() as i64);
        rec.note(format!("F3 location {loc}: {} listings, {} distinct, over {} pages (page sizes {pages:?}), totalCount {total:?}{}",
            mine.len(), distinct.len(), pages.len(), if complete { "" } else { " — INCOMPLETE: the pages do not add up to totalCount" }));
        if !complete { return Err(format!("products {loc}: the pages list {} distinct products, totalCount says {total:?}; nothing written", distinct.len())); }
        per_location.push(distinct);
    }
    let unique: BTreeSet<i64> = per_location.iter().flatten().copied().collect();
    let shared = unique.iter().filter(|id| per_location.iter().filter(|l| l.contains(id)).count() > 1).count();
    rec.note(format!("F3 product ids shared between locations: {shared} ({} distinct ids)", unique.len()));

    let (mut no_metal, mut units, mut metals, mut read_n) = (0, BTreeMap::<String, usize>::new(), BTreeMap::<String, usize>::new(), 0);
    let mut seen = BTreeSet::new();
    for (loc, _) in &locs {
        let mine: Vec<i64> = all.iter().filter(|(l, _)| l == loc).map(|(_, p)| *p).filter(|p| seen.insert(*p)).take(details.unwrap_or(usize::MAX)).collect();
        for p in mine {
            let (s, b) = rec.call(&format!("detail.{p}"), get(&format!("/product/detail/{p}"), vec![]), false).await?;
            read_n += 1;
            if s != 200 { rec.note(format!("F3 detail {p}: HTTP {s}{}", message(s, &b))); continue; }
            let metal = b.get("metal").and_then(Value::as_str).unwrap_or("(none)").to_string();
            if metal == "(none)" || metal == "___" { no_metal += 1; }
            *metals.entry(metal).or_default() += 1;
            *units.entry(b.get("fineWeightUnit").and_then(Value::as_str).unwrap_or("(none)").to_string()).or_default() += 1;
            if b.get("locationId").and_then(Value::as_i64) != Some(*loc) { rec.note(format!("F3 detail {p}: locationId differs from its listing's {loc}")); }
        }
    }
    rec.note(format!("F3 details read: {read_n}; without metal: {no_metal}; metals {metals:?}; weight units {units:?}"));

    let products: Vec<String> = unique.iter().map(i64::to_string).collect();
    for cur in CURRENCIES {
        let (mut priced, mut buyable, mut empty) = (0, 0, 0);
        for (n, batch) in products.chunks(PRICE_BATCH).enumerate() {
            let mut q: Vec<(&'static str, String)> = batch.iter().map(|p| ("productIds", p.clone())).collect();
            q.push(("currency", cur.to_string()));
            let (s, b) = rec.call(&format!("prices.{cur}.{n}"), get("/product/price/", q), false).await?;
            if s == 204 { empty += 1; continue; }
            if s != 200 { rec.note(format!("F3 prices {cur} batch {n}: HTTP {s}{}", message(s, &b))); continue; }
            for p in b.as_array().into_iter().flatten() {
                if p.get("unitPrice").and_then(Value::as_str).is_some_and(|v| !v.is_empty()) { priced += 1; }
                if p.get("availableForPurchase") == Some(&Value::Bool(true)) { buyable += 1; }
            }
        }
        rec.note(format!("F3 prices {cur}: {priced} of {} priced, {buyable} available, {empty} batches answered 204", products.len()));
    }

    for (loc, _) in &locs {
        let (s, b) = rec.call(&format!("shipping_methods.{loc}"), get("/shipping-method/list", vec![("locationId", loc.to_string())]), false).await?;
        rec.note(format!("F4 shipping methods at {loc}: HTTP {s}{} {}", message(s, &b), titles(&b)));
        for cur in CURRENCIES {
            let (s, b) = rec.call(&format!("payment_methods.{loc}.{cur}"), get("/payment-method/list", vec![("locationId", loc.to_string()), ("currency", cur.to_string())]), false).await?;
            if s == 200 { rec.note(format!("F4 payment methods at {loc} in {cur}: {}", titles(&b))); }
            let (s, b) = rec.call(&format!("sell_payment_methods.{loc}.{cur}"), get("/sell-payment-method/list", vec![("locationId", loc.to_string()), ("currency", cur.to_string())]), false).await?;
            if s == 200 {
                let fields: Vec<String> = b.as_array().into_iter().flatten().map(|m| {
                    let f: Vec<String> = m.get("fields").and_then(Value::as_array).into_iter().flatten()
                        .map(|f| format!("{}{}", f.get("name").and_then(Value::as_str).unwrap_or("?"), if f.get("mandatory") == Some(&Value::Bool(true)) { "*" } else { "" })).collect();
                    format!("{} {} [{}]", m.get("id").map(Value::to_string).unwrap_or_default(), m.get("title").and_then(Value::as_str).unwrap_or(""), f.join(" "))
                }).collect();
                rec.note(format!("F4 sell-payment methods at {loc} in {cur}: {}", fields.join("; ")));
            }
        }
    }

    let (s, b) = rec.call("svf_holdings", get("/svf/holdings", vec![]), true).await?;
    let shape: Vec<String> = b.get("currencies").and_then(Value::as_array).into_iter().flatten()
        .map(|c| format!("{} ×{}", c.get("currency").and_then(Value::as_str).unwrap_or("?"), c.get("balance").and_then(Value::as_array).map_or(0, Vec::len))).collect();
    rec.note(format!("F2 svf/holdings: HTTP {s}{}; balance array lengths {}", message(s, &b), shape.join(", ")));
    let (s, b) = rec.call("vault_holdings", get("/vault/holdings", vec![]), true).await?;
    rec.note(format!("vault/holdings: HTTP {s}{}; {}", message(s, &b), vault_shape(&b)));
    Ok(())
}

fn titles(v: &Value) -> String {
    v.as_array().into_iter().flatten().map(|m| format!("{} {}", m.get("id").map(Value::to_string).unwrap_or_default(), m.get("title").and_then(Value::as_str).unwrap_or(""))).collect::<Vec<_>>().join("; ")
}

fn vault_rows(v: &Value) -> Vec<&Value> {
    v.get("locations").and_then(Value::as_array).into_iter().flatten()
        .flat_map(|l| l.get("products").and_then(Value::as_array).into_iter().flatten()).collect()
}

fn vault_shape(v: &Value) -> String {
    let rows = vault_rows(v);
    let paid = rows.iter().filter(|r| r.get("paid") == Some(&Value::Bool(true))).count();
    let serials = rows.iter().filter(|r| r.get("serialNumbers").and_then(Value::as_str).is_some_and(|s| !s.is_empty())).count();
    format!("{} rows ({paid} paid, {} unpaid, {serials} with serial numbers)", rows.len(), rows.len() - paid)
}

/// The pieces of `product` held, paid or not.
fn held(v: &Value, product: i64) -> i64 {
    vault_rows(v).iter().filter(|r| r.get("productId").and_then(Value::as_i64) == Some(product)).filter_map(|r| r.get("quantity").and_then(Value::as_i64)).sum()
}

// ---- orders (F5-F8, F12, F13), every order with testOrder: true

fn buy_lock(o: &Order, product: i64, quantity: i64) -> HttpRequest {
    post("/checkout/buy/price-lock-token", json!({
        "locationId": o.location, "currency": o.currency, "paymentMethodId": o.payment, "shippingMethodId": o.shipping,
        "products": [{ "productId": product.to_string(), "quantity": quantity }],
    }))
}
fn sell_lock(o: &Order, product: i64, quantity: i64) -> HttpRequest {
    post("/checkout/vault-sell/price-lock-token", json!({
        "locationId": o.location, "currency": o.currency, "sellPaymentMethodId": o.sell_payment, "sellPaymentMethodFields": [],
        "products": [{ "productId": product, "quantity": quantity }],
    }))
}
fn test_order(path: &str, token: &str, pin: &str) -> HttpRequest {
    post(path, json!({ "priceLockToken": token, "pin": pin, "testOrder": true }))
}
const BUY: &str = "/checkout/buy/order";
const SELL: &str = "/checkout/vault-sell/order";

/// The balances as a comparable set: (location, product, quantity, paid) rows and (currency, balance) rows. None when a
/// read failed: a comparison is then inconclusive.
fn balances(vault: &(u16, Value), svf: &(u16, Value)) -> Option<Vec<String>> {
    let rows = |(s, v): &(u16, Value), list: &str| -> Option<Vec<Value>> {
        match s { 204 => Some(vec![]), 200 => v.get(list)?.as_array().cloned(), _ => None }
    };
    let mut out = Vec::new();
    for l in rows(vault, "locations")? {
        let loc = l.pointer("/location/id")?.as_i64()?;
        for p in l.get("products")?.as_array()? {
            out.push(format!("vault {loc} {} {} {}", p.get("productId")?.as_i64()?, p.get("quantity")?.as_i64()?, p.get("paid")?.as_bool()?));
        }
    }
    for c in rows(svf, "currencies")? {
        let balance: Option<Vec<String>> = c.get("balance")?.as_array()?.iter().map(|b| b.as_f64().map(|_| b.to_string())).collect();
        out.push(format!("svf {} {}", c.get("currency")?.as_str()?, balance?.join(",")));
    }
    out.sort();
    Some(out)
}

async fn snapshot<T: Transport>(rec: &mut Rec<'_, T>, name: &str) -> Result<Option<Vec<String>>, String> {
    let vault = rec.step(&format!("{name}_vault"), get("/vault/holdings", vec![])).await?;
    let svf = rec.step(&format!("{name}_svf"), get("/svf/holdings", vec![])).await?;
    Ok(balances(&vault, &svf))
}

/// F7: whether a test order moved a balance, from the snapshots read just before and just after it.
fn moved(before: &Option<Vec<String>>, after: &Option<Vec<String>>) -> &'static str {
    match (before, after) { (Some(b), Some(a)) if a == b => "no", (Some(_), Some(_)) => "YES", _ => "inconclusive (a balance read failed)" }
}

fn token(v: &Value) -> Option<String> { v.get("priceLockToken").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string) }

/// expireTime as an instant: RFC 3339, else a naive time read as UTC (the report then names it naive).
pub fn expires(v: &Value) -> Option<(DateTime<Utc>, bool)> {
    let s = v.get("expireTime").and_then(Value::as_str)?;
    if let Ok(t) = DateTime::parse_from_rfc3339(s) { return Some((t.to_utc(), false)); }
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"].iter().find_map(|f| NaiveDateTime::parse_from_str(s, f).ok()).map(|t| (t.and_utc(), true))
}

/// The lock's lifetime and its non-zero costs (F5, F13): the dealer's own quote, no account data.
fn lock_facts<T: Transport>(rec: &mut Rec<'_, T>, file: &str, v: &Value) {
    if let Some((t, naive)) = expires(v) {
        rec.note(format!("F5 {file}: expires {}s after the answer{}", (t - rec.t.now()).num_seconds(), if naive { " (expireTime has no zone; read as UTC)" } else { "" }));
    }
    let costs: Vec<String> = v.get("costs").and_then(Value::as_object).into_iter().flatten()
        .filter(|(_, c)| c.as_str().is_some_and(|c| !zero(c)) || c.as_f64().is_some_and(|c| c != 0.0)).map(|(k, c)| format!("{k}={c}")).collect();
    let fee = v.get("storageFee").map(|f| format!(" storageFee={f}")).unwrap_or_default();
    let total = v.get("totalAmount").or_else(|| v.get("total")).map(|t| format!(" total={t}")).unwrap_or_default();
    let units: Vec<String> = v.get("products").and_then(Value::as_array).into_iter().flatten()
        .map(|p| format!("{}×{}", p.get("quantity").map(Value::to_string).unwrap_or_default(), p.get("unitPriceAmount").map(Value::to_string).unwrap_or_default())).collect();
    rec.note(format!("F13 {file}: products {} costs [{}]{fee}{total}; payment echo {}", units.join(" "), costs.join(" "),
        v.pointer("/paymentMethod/id").or_else(|| v.pointer("/sellPaymentMethodValue/id")).map(Value::to_string).unwrap_or("(none)".into())));
}

/// A test order on a fresh lock's token; the lock and the order are both recorded. Returns the order's answer.
async fn lock_and_order<T: Transport>(rec: &mut Rec<'_, T>, name: &str, lock: HttpRequest, path: &str, pin: &str) -> Result<Option<(u16, Value, String)>, String> {
    let (s, v) = rec.step(&format!("lock_{name}"), lock).await?;
    let Some(tok) = token(&v).filter(|_| s == 200) else { return Ok(None) };
    lock_facts(rec, &format!("lock_{name}"), &v);
    let (s, b) = rec.step(&format!("order_{name}"), test_order(path, &tok, pin)).await?;
    Ok(Some((s, b, tok)))
}

/// The order's (accountId, id), as read from an acknowledgement.
fn order_ids(v: &Value) -> (Option<i64>, Option<i64>) {
    (v.pointer("/order/accountId").and_then(Value::as_i64), v.pointer("/order/id").and_then(Value::as_i64))
}

/// A lookup's answer (an array of orders, per the spec; an object is read as one): whether it names the order, and its status.
fn lookup(v: &Value, same: impl Fn(&Value) -> bool) -> String {
    let list: Vec<&Value> = match v { Value::Array(a) => a.iter().collect(), Value::Object(_) => vec![v], _ => vec![] };
    let shape = if v.is_array() { "an array" } else { "not an array" };
    match list.iter().find(|o| same(o)) {
        Some(o) => format!("{shape} of {}, the order found, status {} \"{}\"", list.len(), o.get("status").map(Value::to_string).unwrap_or_default(), o.get("statusString").and_then(Value::as_str).unwrap_or("")),
        None => format!("{shape} of {}, the order not found", list.len()),
    }
}

async fn served<T: Transport>(rec: &mut Rec<'_, T>, name: &str, v: &Value) -> Result<Option<i64>, String> {
    let (account, id) = order_ids(v);
    rec.note(format!("F12 {name}: order.id {}, accountId {}, orderRef {}", if id.is_some() { "present" } else { "absent" },
        if account.is_some() { "present" } else { "absent" }, if v.pointer("/order/orderRef").is_some_and(|r| r.is_string()) { "present" } else { "absent" }));
    if let Some(id) = id {
        let (s, b) = rec.step(&format!("get_by_id_{name}"), get("/order/get-by-id", vec![("orderId", id.to_string())])).await?;
        rec.note(format!("F7 get-by-id {name}: HTTP {s}, {}", lookup(&b, |o| o.get("id").and_then(Value::as_i64) == Some(id))));
    }
    if let Some(r) = v.pointer("/order/orderRef").and_then(Value::as_str).map(str::to_string) {
        let (s, b) = rec.step(&format!("get_by_ref_{name}"), get("/order/get-by-order-ref", vec![("orderRef", r.clone())])).await?;
        rec.note(format!("F7 get-by-order-ref {name}: HTTP {s}, {}", lookup(&b, |o| o.get("orderRef").and_then(Value::as_str) == Some(r.as_str()))));
    }
    Ok(account)
}

/// After an order was sent: the balances read again and compared with the read before it (F7), and whether the run
/// may send another order. It stops on a moved or unreadable balance, and on an answer that is neither an
/// acknowledgement with an order id nor a definite refusal (a 4xx other than 429); `ack` requires the acknowledgement.
async fn settled<T: Transport>(rec: &mut Rec<'_, T>, name: &str, last: &mut Option<Vec<String>>, (s, b): (u16, &Value), ack: bool) -> Result<bool, String> {
    let now = snapshot(rec, &format!("after_{name}")).await?;
    let m = moved(last, &now);
    rec.note(format!("F7 {name} moved a balance: {m}"));
    *last = now;
    if m != "no" {
        rec.note("a balance moved or could not be read after a test order: no further order is sent; check the account".into());
        return Ok(false);
    }
    let acknowledged = s == 200 && order_ids(b).1.is_some();
    // A definite refusal: a status the dealer documents for one, with its `{message}` body. A 408, a 429 or any other
    // answer leaves the outcome open.
    let refused = [400, 401, 403, 404, 409, 422].contains(&s) && b.get("message").is_some_and(Value::is_string);
    if !(acknowledged || (refused && !ack)) {
        rec.note(format!("{name}: HTTP {s} is {}: no further order is sent", if ack { "no acknowledgement with an order id (check the PIN and the SVF balance)" } else { "neither an acknowledgement nor a definite refusal" }));
        return Ok(false);
    }
    Ok(true)
}

async fn orders<T: Transport>(rec: &mut Rec<'_, T>, o: &Order, pin: &str) -> Result<(), String> {
    if pin.is_empty() { return Err("set BULLIONSTAR_PIN for the orders mode".into()); }
    let before = snapshot(rec, "before").await?;
    if before.is_none() { return Err("the balances could not be read before any order; nothing written".into()); }
    let mut last = before.clone();
    let held_gram = held(&rec.got.iter().rev().find(|g| g.file == "before_vault.json").map(|g| g.body.clone()).unwrap_or_default(), o.gram);

    // F13: one piece and many of the gram, one bar (locks only).
    for (name, p, q) in [("buy_gram_5", o.gram, 5), ("buy_bar_1", o.bar, 1)] {
        let (s, v) = rec.step(&format!("lock_{name}"), buy_lock(o, p, q)).await?;
        if s == 200 { lock_facts(rec, &format!("lock_{name}"), &v); }
    }
    // F7: a test buy. Every later order sends the PIN again: unless this one is acknowledged, the run stops here.
    let Some((s, b, tok)) = lock_and_order(rec, "buy_gram_1", buy_lock(o, o.gram, 1), BUY, pin).await? else {
        rec.note("the 1-gram buy lock was refused: no order sent; the run stops here".into());
        return Ok(());
    };
    if !settled(rec, "buy", &mut last, (s, &b), true).await? { return Ok(()); }
    let mut accounts = vec![served(rec, "buy", &b).await?];
    // F6: the same token again.
    let (s, b) = rec.step("order_buy_repeat_token", test_order(BUY, &tok, pin)).await?;
    if !settled(rec, "repeat_token", &mut last, (s, &b), false).await? { return Ok(()); }
    // F7: a test vault-sell of one gram (refused by the dealer when none is held: that answer is recorded too).
    if let Some((s, b, _)) = lock_and_order(rec, "sell_gram_1", sell_lock(o, o.gram, 1), SELL, pin).await? {
        if !settled(rec, "sell", &mut last, (s, &b), false).await? { return Ok(()); }
        if s == 200 { accounts.push(served(rec, "sell", &b).await?); }
    }
    if accounts.len() == 2 { rec.note(format!("F12 buy and sell name one accountId: {}", accounts[0].is_some() && accounts[0] == accounts[1])); }
    // F6: insufficient funds, a sale over the holding, an unavailable product. File names carry no quantity: an echoed
    // one is a replaced original, and the manifest's leak check would find it.
    let mut probes = vec![("buy_insufficient_funds", buy_lock(o, o.bar, 1000), BUY), ("sell_over_holding", sell_lock(o, o.gram, held_gram + 1000), SELL)];
    if let Some(p) = o.unavailable { probes.push(("buy_unavailable", buy_lock(o, p, 1), BUY)); }
    for (name, lock, path) in probes {
        if let Some((s, b, _)) = lock_and_order(rec, name, lock, path, pin).await? {
            if !settled(rec, name, &mut last, (s, &b), false).await? { return Ok(()); }
        }
    }
    // F6: a wrong PIN, once, only when asked (a dealer may lock the account after several).
    if o.wrong_pin {
        let wrong = match pin.chars().last() {
            Some(c @ '0'..='9') => format!("{}{}", &pin[..pin.len() - 1], (c as u8 - b'0' + 1) % 10),
            _ => format!("{pin}0"),
        };
        let (s, v) = rec.step("lock_buy_wrong_pin", buy_lock(o, o.gram, 1)).await?;
        if let Some(tok) = token(&v).filter(|_| s == 200) {
            let (s, b) = rec.step("order_buy_wrong_pin", test_order(BUY, &tok, &wrong)).await?;
            if !settled(rec, "wrong_pin", &mut last, (s, &b), false).await? { return Ok(()); }
        }
    }
    // F5/F6: an expired token, when asked and when the lock expires within the cap.
    if o.expiry {
        let (s, v) = rec.step("lock_buy_expiry", buy_lock(o, o.gram, 1)).await?;
        match (token(&v).filter(|_| s == 200), expires(&v)) {
            (Some(tok), Some((t, _))) => {
                let left = (t - rec.t.now()).to_std().unwrap_or_default() + rec.opts.margin;
                if left > rec.opts.wait_cap { rec.note(format!("F5 expiry: the lock lives over {}s; not waited", rec.opts.wait_cap.as_secs())); }
                else {
                    tokio::time::sleep(left).await;
                    let (s, b) = rec.step("order_buy_expired_token", test_order(BUY, &tok, pin)).await?;
                    if !settled(rec, "expired_token", &mut last, (s, &b), false).await? { return Ok(()); }
                }
            }
            _ => rec.note("F5 expiry: no token or no readable expireTime; not waited".into()),
        }
    }
    rec.note(format!("F7 the whole run moved a balance: {}", moved(&before, &last)));
    Ok(())
}
