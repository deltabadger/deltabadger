//! The order-list recorder's logic (PLAN-A28-fixtures.md): the endpoint table and its allow-list, the permission reads,
//! the envelope and case checks, the fail-closed scrubber with its leak check, and the record flow over any Transport.
//! rust/tests/order_list_recorder.rs includes this file so CI runs its tests (CI never runs an example's own tests);
//! main.rs beside it only reads arguments and keys and builds the live transport.
#![allow(dead_code)] // the test crate that includes this file uses only part of it

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, SecondsFormat, Utc};
use deltabadger::venue::http::{HttpRequest, ScriptedTransport, Transport, TransportError};
use hmac::{Hmac, Mac};
use serde_json::{json, Map, Number, Value};
use sha2::Sha256;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Unit { Secs, Ms, Rfc3339 }

pub struct Endpoint {
    pub venue: &'static str,
    /// The fixture's file name, without the case suffix and `.json`.
    pub file: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    /// JSON pointer to the order list ("" is the root).
    pub list: &'static str,
    /// A history read (the full case must show ≥ 2 orders); an open-orders read is recorded once.
    pub history: bool,
    /// (start param, end param or "" for none, unit).
    pub window: Option<(&'static str, &'static str, Unit)>,
    /// The page-size param and its smallest value; None where the smallest page is above 1 (no page case).
    pub page: Option<(&'static str, &'static str)>,
    /// The symbol param's name, where the endpoint takes one.
    pub symbol: Option<&'static str>,
    pub fixed: &'static [(&'static str, &'static str)],
    /// Hyperliquid's `/info` read type.
    pub info: Option<&'static str>,
}

const fn ep(venue: &'static str, file: &'static str, method: &'static str, path: &'static str, list: &'static str) -> Endpoint {
    Endpoint { venue, file, method, path, list, history: false, window: None, page: None, symbol: None, fixed: &[], info: None }
}
impl Endpoint {
    const fn window(self, start: &'static str, end: &'static str, unit: Unit) -> Self { Self { window: Some((start, end, unit)), history: true, ..self } }
    const fn page(self, name: &'static str, value: &'static str) -> Self { Self { page: Some((name, value)), ..self } }
    const fn symbol(self, name: &'static str) -> Self { Self { symbol: Some(name), ..self } }
    const fn fixed(self, fixed: &'static [(&'static str, &'static str)]) -> Self { Self { fixed, ..self } }
    const fn info(self, info: &'static str, history: bool) -> Self { Self { info: Some(info), history, ..self } }
}

/// Every read the tool may send (with PERMISSION_READS, the allow-list): PLAN-A28-3 §8 Q1.
pub const ENDPOINTS: &[Endpoint] = &[
    ep("kraken", "closed_orders", "POST", "/0/private/ClosedOrders", "/result/closed").window("start", "end", Unit::Secs),
    ep("kraken", "open_orders", "POST", "/0/private/OpenOrders", "/result/open"),
    ep("kucoin", "orders_done", "GET", "/api/v1/orders", "/data/items").window("startAt", "endAt", Unit::Ms).symbol("symbol").fixed(&[("status", "done")]),
    ep("kucoin", "orders_active", "GET", "/api/v1/orders", "/data/items").symbol("symbol").fixed(&[("status", "active")]),
    ep("kucoin", "hf_orders_done", "GET", "/api/v1/hf/orders/done", "/data/items").window("startAt", "endAt", Unit::Ms).page("limit", "1").symbol("symbol"),
    ep("kucoin", "hf_orders_active", "GET", "/api/v1/hf/orders/active", "/data").symbol("symbol"),
    ep("coinbase", "historical_batch", "GET", "/api/v3/brokerage/orders/historical/batch", "/orders").window("start_date", "end_date", Unit::Rfc3339).page("limit", "1"),
    ep("alpaca", "orders", "GET", "/v2/orders", "").window("after", "until", Unit::Rfc3339).page("limit", "1").fixed(&[("status", "all")]),
    ep("bitget", "history_orders", "GET", "/api/v2/spot/trade/history-orders", "/data").window("startTime", "endTime", Unit::Ms).page("limit", "1").symbol("symbol"),
    ep("bitget", "unfilled_orders", "GET", "/api/v2/spot/trade/unfilled-orders", "/data").symbol("symbol"),
    ep("mexc", "all_orders", "GET", "/api/v3/allOrders", "").window("startTime", "endTime", Unit::Ms).page("limit", "1").symbol("symbol"),
    ep("mexc", "open_orders", "GET", "/api/v3/openOrders", "").symbol("symbol"),
    ep("gemini", "orders_history", "POST", "/v1/orders/history", "").window("timestamp", "", Unit::Secs).page("limit_orders", "1").symbol("symbol"),
    ep("gemini", "orders", "POST", "/v1/orders", ""),
    ep("bitvavo", "orders", "GET", "/v2/orders", "").window("start", "end", Unit::Ms).page("limit", "1").symbol("market"),
    ep("bitvavo", "orders_open", "GET", "/v2/ordersOpen", "").symbol("market"),
    ep("bingx", "history_orders", "GET", "/openApi/spot/v1/trade/historyOrders", "/data/orders").window("startTime", "endTime", Unit::Ms).page("pageSize", "1").symbol("symbol").fixed(&[("pageIndex", "1")]),
    ep("bingx", "open_orders", "GET", "/openApi/spot/v1/trade/openOrders", "/data/orders").symbol("symbol"),
    ep("bitrue", "all_orders", "GET", "/api/v1/allOrders", "").window("startTime", "endTime", Unit::Ms).page("limit", "1").symbol("symbol"),
    ep("bitrue", "open_orders", "GET", "/api/v1/openOrders", "").symbol("symbol"),
    ep("hyperliquid", "historical_orders", "POST", "/info", "").info("historicalOrders", true),
    ep("hyperliquid", "frontend_open_orders", "POST", "/info", "").info("frontendOpenOrders", false),
    ep("hyperliquid", "user_fills_by_time", "POST", "/info", "").info("userFillsByTime", true).window("startTime", "", Unit::Ms),
];

/// The permission reads, where the venue can tell whether a key trades or withdraws.
pub const PERMISSION_READS: &[(&str, &str, &str)] = &[
    ("coinbase", "GET", "/api/v3/brokerage/key_permissions"),
    ("gemini", "POST", "/v1/roles"),
    ("kucoin", "GET", "/api/v1/user/api-key"),
];

pub fn allowed(venue: &str, method: &str, path: &str) -> bool {
    ENDPOINTS.iter().any(|e| (e.venue, e.method, e.path) == (venue, method, path)) || PERMISSION_READS.contains(&(venue, method, path))
}

pub fn base(venue: &str, paper: bool) -> Option<&'static str> {
    use deltabadger::venue::*;
    Some(match venue {
        "kraken" => kraken::URL,
        "kucoin" => kucoin::URL,
        "coinbase" => coinbase::URL,
        "alpaca" if paper => alpaca::PAPER_TRADING_URL,
        "alpaca" => alpaca::TRADING_URL,
        "bitget" => bitget::URL,
        "mexc" => mexc::URL,
        "gemini" => gemini::URL,
        "bitvavo" => bitvavo::URL,
        "bingx" => bingx::URL,
        "bitrue" => bitrue::URL,
        "hyperliquid" => hyperliquid::URL,
        _ => return None,
    })
}

/// Keys are never an argument: refuse anything that looks like one.
pub fn refuse_key_args(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| ["key", "secret", "passphrase"].iter().any(|w| a.to_ascii_lowercase().contains(w))) {
        return Err("keys are read from RECORDER_KEY, RECORDER_SECRET and RECORDER_PASSPHRASE only, never from arguments".into());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Permission { ReadOnly, CanTrade, Unreadable }

/// The typed decision from a permission read's answer; nothing else of the body is kept. Missing, null or wrong-typed
/// fields are Unreadable, which refuses the run.
pub fn permission(venue: &str, status: u16, body: &str) -> Permission {
    let Some(v) = (200..300).contains(&status).then(|| serde_json::from_str::<Value>(body).ok()).flatten() else { return Permission::Unreadable };
    let flags = |names: &[&str]| {
        let mut any = false;
        for n in names {
            match v.get(*n) { Some(Value::Bool(b)) => any |= *b, _ => return Permission::Unreadable }
        }
        if any { Permission::CanTrade } else { Permission::ReadOnly }
    };
    match venue {
        "coinbase" => flags(&["can_trade", "can_transfer"]),
        "gemini" => flags(&["isTrader", "isFundManager"]),
        "kucoin" if v.get("code") == Some(&json!("200000")) => match v.pointer("/data/permission").and_then(Value::as_str) {
            Some("General") => Permission::ReadOnly,
            Some(_) => Permission::CanTrade,
            None => Permission::Unreadable,
        },
        _ => Permission::Unreadable,
    }
}

/// The success field a venue's envelope carries, checked on the raw body and kept verbatim through the scrub.
fn success(venue: &str) -> Option<(&'static str, Value)> {
    match venue {
        "kraken" => Some(("error", json!([]))),
        "kucoin" => Some(("code", json!("200000"))),
        "bitget" => Some(("code", json!("00000"))),
        "bingx" => Some(("code", json!(0))),
        _ => None,
    }
}

/// How many orders the answer lists, after its envelope is checked.
pub fn orders(e: &Endpoint, body: &Value) -> Result<usize, String> {
    if let Some((field, want)) = success(e.venue) {
        if body.get(field) != Some(&want) { return Err(format!("{} {}: the answer is not a success", e.venue, e.file)); }
    }
    match body.pointer(e.list) {
        Some(Value::Array(a)) => Ok(a.len()),
        Some(Value::Object(m)) if e.venue == "kraken" => Ok(m.len()),
        _ => Err(format!("{} {}: no order list at '{}'", e.venue, e.file, e.list)),
    }
}

/// An integer token of 20+ digits outside a string: serde_json would read it as a lossy f64.
pub fn long_integer(raw: &str) -> bool {
    let (b, mut i, mut run) = (raw.as_bytes(), 0, 0);
    while i < b.len() {
        match b[i] {
            b'"' => {
                run = 0;
                i += 1;
                while i < b.len() && b[i] != b'"' { i += if b[i] == b'\\' { 2 } else { 1 }; }
            }
            c if c.is_ascii_digit() => { run += 1; if run >= 20 { return true; } }
            _ => run = 0,
        }
        i += 1;
    }
    false
}

/// Field names whose values are kept (after a shape check): enums and envelope counters, never free text.
const KEEP: &[&str] = &[
    "status", "state", "side", "type", "ordertype", "ordtype", "symbol", "pair", "market", "productid", "producttype", "instid",
    "coin", "timeinforce", "tif", "force", "count", "total", "totalnum", "totalpage", "currentpage", "pagesize", "page",
    "pageindex", "limit", "hasnext", "exchange", "selftradeprevention", "stp", "feecurrency", "feeasset", "commissionasset",
    "ordersource", "enterpointsource", "source", "oflags", "triggertype", "tpsltype", "liquidity", "assetclass",
    "positionintent",
];
/// The exact time field names shifted (r1.1); a value that is not a time is replaced instead.
const TIME: &[&str] = &[
    "opentm", "closetm", "starttm", "expiretm", "time", "updatetime", "workingtime", "transacttime", "timestamp", "timestampms",
    "statustimestamp", "ctime", "utime", "created", "updated", "createdat", "updatedat", "submittedat", "filledat", "expiredat",
    "canceledat", "failedat", "replacedat", "expiresat", "createdtime", "updatedtime", "lastupdatetimestamp", "ordercreatetime",
    "orderupdatetime", "lastupdatedat", "createtime", "requesttime",
];

fn norm(k: &str) -> String { k.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect() }

fn kept_shape(s: &str) -> bool {
    let mut run = 0;
    s.len() <= 32
        && s.chars().all(|c| {
            run = if c.is_ascii_digit() { run + 1 } else { 0 };
            run < 5 && (c.is_ascii_alphanumeric() || "_/ .-@".contains(c))
        })
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Case { Full, Page, Empty }
impl Case {
    pub fn suffix(self) -> &'static str { match self { Case::Full => "", Case::Page => ".page", Case::Empty => ".empty" } }
}

pub struct Scrubber {
    key: Vec<u8>,
    days: i64,
    map: HashMap<String, String>,
    originals: HashSet<String>,
    fakes: HashSet<String>,
    /// Every value of every answer (`reserve`, before any fake): no fake may equal or contain one (≥ 4 characters), as
    /// the leak check reads it.
    reserved: Vec<String>,
    /// "file path" → kind → count, paths folded (`[]` for indices, `*` for Kraken's id keys).
    pub report: BTreeMap<String, BTreeMap<&'static str, usize>>,
}

impl Scrubber {
    pub fn new(key: Vec<u8>, days: i64) -> Self {
        Self { key, days, map: HashMap::new(), originals: HashSet::new(), fakes: HashSet::new(), reserved: vec![], report: BTreeMap::new() }
    }
    /// A salt that is never stored and an offset of 30–400 whole days back, both from the OS's randomness.
    pub fn random() -> Self {
        let salt = [*uuid::Uuid::new_v4().as_bytes(), *uuid::Uuid::new_v4().as_bytes()].concat();
        let days = 30 + i64::from(u16::from_be_bytes([salt[0], salt[1]]) % 371);
        Self::new(salt, days)
    }
    pub fn days(&self) -> i64 { self.days }

    fn note(&mut self, file: &str, path: &str, kind: &'static str) {
        *self.report.entry(format!("{file} {}", if path.is_empty() { "/" } else { path })).or_default().entry(kind).or_default() += 1;
    }

    /// Notes every key, string and number text of an answer, so that no fake made later collides with a value met after it.
    pub fn reserve(&mut self, v: &Value) {
        let mut seen: HashSet<String> = self.reserved.drain(..).collect();
        visit(v, "", &mut |text, _| { if text.chars().count() >= 4 { seen.insert(text.to_string()); } });
        self.reserved = seen.into_iter().collect();
    }

    /// The scrubbed body: every value replaced unless kept, zero, empty or a shifted time (PLAN §3, r1, r2).
    pub fn scrub(&mut self, venue: &str, file: &str, mut v: Value) -> Result<Value, String> {
        self.walk(venue, file, String::new(), None, &mut v)?;
        Ok(v)
    }

    fn walk(&mut self, venue: &str, file: &str, path: String, name: Option<&str>, v: &mut Value) -> Result<(), String> {
        match v {
            Value::Null | Value::Bool(_) => {}
            Value::Array(a) => {
                for x in a { self.walk(venue, file, format!("{path}/[]"), name, x)?; }
            }
            Value::Object(m) => {
                let dynamic = venue == "kraken" && (path == "/result/open" || path == "/result/closed");
                let success = success(venue).filter(|_| path.is_empty()).map(|(f, _)| f);
                for (k, mut x) in std::mem::take(m) {
                    if success == Some(k.as_str()) { m.insert(k, x); continue; }
                    if dynamic {
                        let child = format!("{path}/*");
                        let fake = self.fake(&k, false, &child)?;
                        self.note(file, &child, "key-replaced");
                        self.walk(venue, file, child, None, &mut x)?;
                        m.insert(fake, x);
                        continue;
                    }
                    if k.chars().filter(char::is_ascii_digit).count() >= 3 || k.len() > 40 {
                        return Err(format!("{file}: an unknown dynamic key under {}", if path.is_empty() { "/" } else { &path }));
                    }
                    self.walk(venue, file, format!("{path}/{k}"), Some(&k), &mut x)?;
                    m.insert(k, x);
                }
            }
            Value::String(s) => {
                if let Some(new) = self.string(venue, file, &path, name, s)? { *s = new; }
            }
            Value::Number(n) => {
                if let Some(new) = self.number(file, &path, name, n)? { *v = Value::Number(new); }
            }
        }
        Ok(())
    }

    fn string(&mut self, venue: &str, file: &str, path: &str, name: Option<&str>, s: &str) -> Result<Option<String>, String> {
        let n = name.map(norm).unwrap_or_default();
        if KEEP.contains(&n.as_str()) && kept_shape(s) { return Ok(None); }
        if TIME.contains(&n.as_str()) {
            if let Some(t) = self.shift_str(s) { self.note(file, path, "shifted"); return Ok(Some(t)); }
        }
        if !s.chars().any(|c| c.is_ascii_alphanumeric()) || zero(s) { return Ok(None); }
        if s.starts_with('{') || s.starts_with('[') {
            if let Ok(mut inner @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(s) {
                self.walk(venue, file, format!("{path}/(json)"), None, &mut inner)?;
                return Ok(Some(inner.to_string()));
            }
        }
        let fake = self.fake(s, false, path)?;
        self.note(file, path, "replaced");
        Ok(Some(fake))
    }

    fn number(&mut self, file: &str, path: &str, name: Option<&str>, n: &Number) -> Result<Option<Number>, String> {
        let name = name.map(norm).unwrap_or_default();
        if KEEP.contains(&name.as_str()) && n.as_u64().is_some_and(|x| x < 10_000) { return Ok(None); }
        let text = n.to_string();
        if TIME.contains(&name.as_str()) {
            if let Some(t) = self.shift_epoch(&text) {
                self.note(file, path, "shifted");
                return serde_json::from_str(&t).map(Some).map_err(|_| format!("{file}: a shifted time at {path} is no number"));
            }
        }
        if zero(&text) { return Ok(None); }
        let fake = self.fake(&text, true, path)?;
        self.note(file, path, "replaced");
        serde_json::from_str(&fake).map(Some).map_err(|_| format!("{file}: a replaced number at {path} is no number"))
    }

    /// A consistent, injective, character-class preserving fake that always differs from its original. In a number
    /// (`digits_only`) only the digits change, so `e`/`E` survive.
    pub fn fake(&mut self, s: &str, digits_only: bool, path: &str) -> Result<String, String> {
        let cache = if digits_only && s.chars().any(|c| c.is_ascii_alphabetic()) { format!("#{s}") } else { s.to_string() };
        if let Some(f) = self.map.get(&cache) { return Ok(f.clone()); }
        for attempt in 0u32..64 {
            let stream = self.stream(&cache, attempt, s.len());
            let mut exponent = false;
            let hex = s.starts_with("0x");
            // Leading zeros are kept only in a decimal's text (its magnitude); an id's digits are all free.
            let mut significant = !(decimal(s) || digits_only);
            let out: String = s.chars().zip(stream).enumerate().map(|(i, (c, b))| {
                let pick = |from: u8, n: u8| char::from(from + b % n);
                match c {
                    // A number's exponent is its magnitude, kept; so is a hex value's `0x`.
                    _ if exponent => c,
                    'e' | 'E' if digits_only => { exponent = true; c }
                    '0' | 'x' if hex && i < 2 => c,
                    '0' if !significant => '0',
                    '0'..='9' if significant => pick(b'0', 10),
                    '0'..='9' => { significant = true; pick(b'1', 9) }
                    'a'..='f' if !digits_only => pick(b'a', 6),
                    'g'..='z' if !digits_only => pick(b'g', 20),
                    'A'..='F' if !digits_only => pick(b'A', 6),
                    'G'..='Z' if !digits_only => pick(b'G', 20),
                    c => c,
                }
            }).collect();
            if out != s && !self.fakes.contains(&out) && !self.originals.contains(&out) && !self.map.contains_key(&out)
                && !self.reserved.iter().any(|r| out.contains(r.as_str())) {
                self.fakes.insert(out.clone());
                self.originals.insert(s.to_string());
                self.map.insert(cache, out.clone());
                return Ok(out);
            }
        }
        Err(format!("no distinct fake for a value at {}", if path.is_empty() { "/" } else { path }))
    }

    fn stream(&self, cache: &str, attempt: u32, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len + 32);
        let mut block = 0u32;
        while out.len() < len {
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&self.key).expect("HMAC takes a key of any length");
            mac.update(cache.as_bytes());
            mac.update(&attempt.to_be_bytes());
            mac.update(&block.to_be_bytes());
            out.extend_from_slice(&mac.finalize().into_bytes());
            block += 1;
        }
        out
    }

    /// An epoch in s, ms, µs or ns (as text, a fraction kept) moved back by the offset; None if it is no epoch.
    pub fn shift_epoch(&self, text: &str) -> Option<String> {
        let (int, frac) = match text.split_once('.') { Some((i, f)) => (i, Some(f)), None => (text, None) };
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(int) || frac.is_some_and(|f| !digits(f)) { return None; }
        let unit: i128 = match int.len() { 10 => 1, 13 => 1_000, 16 => 1_000_000, 19 => 1_000_000_000, _ => return None };
        let shifted = int.parse::<i128>().ok()? - i128::from(self.days) * 86_400 * unit;
        Some(match frac { Some(f) => format!("{shifted}.{f}"), None => shifted.to_string() })
    }

    /// A time string (an epoch, or a whole ISO date/datetime) moved back by the offset: only the date part changes.
    pub fn shift_str(&self, s: &str) -> Option<String> {
        if let Some(t) = self.shift_epoch(s) { return Some(t); }
        let iso = DateTime::parse_from_rfc3339(s).is_ok()
            || NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").is_ok()
            || NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").is_ok()
            || NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok_and(|_| s.len() == 10);
        if !iso || !s.is_char_boundary(10) { return None; }
        let date = NaiveDate::parse_from_str(&s[..10], "%Y-%m-%d").ok()? - Duration::days(self.days);
        Some(format!("{}{}", date.format("%Y-%m-%d"), &s[10..]))
    }

    /// No replaced original (≥ 4 characters) and no credential anywhere in `v`'s decoded keys and values (r1.4).
    pub fn leak_check(&self, name: &str, v: &Value, secrets: &[&str]) -> Result<(), String> {
        let needles: Vec<&str> = self.originals.iter().map(String::as_str).filter(|o| o.chars().count() >= 4)
            .chain(secrets.iter().copied().filter(|s| !s.is_empty())).collect();
        let mut hit = None;
        visit(v, "", &mut |text, path| {
            if hit.is_none() && needles.iter().any(|n| text.contains(n)) { hit = Some(path.to_string()); }
        });
        if let Some(path) = hit { return Err(format!("{name}: a recorded value or a credential survives at {path}; nothing written")); }
        let text = v.to_string();
        if deltabadger::crypto::scrub_known(&text, secrets) != text { return Err(format!("{name}: a credential survives; nothing written")); }
        Ok(())
    }

    pub fn report_text(&self) -> String {
        self.report.iter().map(|(field, kinds)| {
            let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{k} ×{n}")).collect();
            format!("  {field}: {}", kinds.join(", "))
        }).collect::<Vec<_>>().join("\n")
    }
}

/// A decimal number's text, `-?digits(.digits)?`: its leading zeros carry the magnitude.
fn decimal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match s.split_once('.') { Some((i, f)) => (i, Some(f)), None => (s, None) };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// An all-zero amount (`0`, `"0.00000000"`): "nothing filled" is what A28-3 reads, and it says nothing about anyone. Only
/// a decimal's text: an all-zero id like `"0000-0000"` is replaced.
fn zero(s: &str) -> bool { decimal(s) && !s.bytes().any(|b| (b'1'..=b'9').contains(&b)) }

/// Every key, string and number text in `v`, with its folded path; a key's own text reports its parent's path.
fn visit(v: &Value, path: &str, f: &mut dyn FnMut(&str, &str)) {
    match v {
        Value::Object(m) => for (k, x) in m {
            f(k, &format!("{} (a key)", if path.is_empty() { "/" } else { path }));
            visit(x, &format!("{path}/{k}"), f);
        },
        Value::Array(a) => for x in a { visit(x, &format!("{path}/[]"), f); },
        Value::String(s) => {
            f(s, path);
            if let Ok(inner @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(s) { visit(&inner, &format!("{path}/(json)"), f); }
        }
        Value::Number(n) => f(&n.to_string(), path),
        _ => {}
    }
}

pub struct Keys { pub key: String, pub secret: String, pub passphrase: String }
impl Keys {
    fn secrets(&self) -> Vec<&str> {
        vec![self.key.as_str(), self.secret.as_str(), self.passphrase.as_str()]
    }
}

pub struct Window { pub since: DateTime<Utc>, pub until: DateTime<Utc>, pub symbol: Option<String> }

fn stamp(t: DateTime<Utc>, unit: Unit) -> String {
    match unit {
        Unit::Secs => t.timestamp().to_string(),
        Unit::Ms => t.timestamp_millis().to_string(),
        Unit::Rfc3339 => t.to_rfc3339_opts(SecondsFormat::Secs, true),
    }
}

/// The request one case sends, signed where the signature is in the query (the client's own `signed`), and the params
/// the manifest names (no signature, timestamp or recvWindow).
pub fn request(e: &Endpoint, case: Case, w: &Window, keys: &Keys, base: &str, now: DateTime<Utc>) -> (HttpRequest, Vec<(&'static str, String)>) {
    use deltabadger::venue::{bingx::BingxVenue, bitrue::BitrueVenue, mexc::MexcVenue};
    let mut params: Vec<(&'static str, String)> = e.fixed.iter().map(|(k, v)| (*k, v.to_string())).collect();
    if let (Some(name), Some(s)) = (e.symbol, &w.symbol) { params.push((name, s.clone())); }
    if let Some((start, end, unit)) = e.window {
        let (from, to) = match case { Case::Empty => (w.since - Duration::days(2), w.since - Duration::days(1)), _ => (w.since, w.until) };
        params.push((start, stamp(from, unit)));
        if !end.is_empty() { params.push((end, stamp(to, unit))); }
    }
    if let (Case::Page, Some((name, value))) = (case, e.page) { params.push((name, value.into())); }
    if let Some(info) = e.info {
        // An /info body: its type and the address, then only the window's own params (userFillsByTime's startTime).
        params.retain(|(k, _)| e.window.is_some_and(|(start, end, _)| *k == start || *k == end));
        params.splice(0..0, [("type", info.to_string()), ("user", keys.key.clone())]);
    }
    let named = params.clone();
    let get = |query| HttpRequest { method: e.method, base: base.into(), path: e.path.into(), query, body: None, not_after: None };
    let post = |body| HttpRequest { method: e.method, base: base.into(), path: e.path.into(), query: vec![], body: Some(body), not_after: None };
    let r = match e.venue {
        "kraken" => post(Value::Object(params.into_iter().map(|(k, v)| (k.to_string(), Value::String(v))).collect())),
        "gemini" => post(Value::Object(params.into_iter().map(|(k, v)| (k.to_string(), v.parse::<i64>().map_or(Value::String(v), Value::from))).collect())),
        "hyperliquid" => post(Value::Object(params.into_iter().map(|(k, v)| (k.to_string(), v.parse::<i64>().map_or(Value::String(v), Value::from))).collect::<Map<_, _>>())),
        "mexc" | "bitrue" => {
            params.push(("recvWindow", "5000".into()));
            let t = ScriptedTransport::default();
            if e.venue == "mexc" { MexcVenue::new(t, base, &keys.secret).signed("GET", e.path, params, now) } else { BitrueVenue::new(t, base, &keys.secret).signed("GET", e.path, params, now) }
        }
        "bingx" => BingxVenue::new(ScriptedTransport::default(), base, &keys.secret).signed("GET", e.path, params, now),
        "coinbase" | "alpaca" => get(params),
        _ => { params.sort_by(|a, b| a.0.cmp(b.0)); get(params) }
    };
    (r, named)
}

fn cases(e: &Endpoint) -> Vec<Case> {
    let mut c = vec![Case::Full];
    if e.page.is_some() { c.push(Case::Page); }
    if e.window.is_some_and(|(_, end, _)| !end.is_empty()) { c.push(Case::Empty); }
    c
}

fn guard(venue: &str, r: &HttpRequest) -> Result<(), String> {
    if allowed(venue, r.method, &r.path) { Ok(()) } else { Err(format!("{venue}: {} {} is not on the read-only allow-list; refused", r.method, r.path)) }
}

fn failure(e: &TransportError) -> &'static str {
    match e { TransportError::NotSent(_) => "not sent", TransportError::MaybeSent(_) => "no answer", TransportError::Permanent(_) => "refused locally" }
}

struct Got { file: String, e: &'static Endpoint, params: Vec<(&'static str, String)>, status: u16, body: Value, orders: usize }

/// One venue's whole set: the permission read, every case of every endpoint, the case checks, the scrub, the leak
/// check, and only then the writes into `out/<venue>/`. Returns the report Jan reads. Prints nothing from a body.
pub async fn record<T: Transport>(t: &T, venue: &str, base: &str, w: &Window, keys: &Keys, sc: &mut Scrubber, out: &Path) -> Result<String, String> {
    let eps: Vec<&'static Endpoint> = ENDPOINTS.iter().filter(|e| e.venue == venue).collect();
    if eps.is_empty() { return Err(format!("unknown venue {venue}")); }
    if eps.iter().any(|e| e.symbol.is_some()) && w.symbol.is_none() { return Err(format!("{venue} needs --symbol (the venue's own spelling)")); }
    if let Some(&(_, method, path)) = PERMISSION_READS.iter().find(|p| p.0 == venue) {
        let r = HttpRequest { method, base: base.into(), path: path.into(), query: vec![], body: (method == "POST").then(|| json!({})), not_after: None };
        guard(venue, &r)?;
        match t.send_observed(&r, &|status, body| permission(venue, status, body)).await {
            Ok((_, Permission::ReadOnly)) => {}
            Ok((_, Permission::CanTrade)) => return Err(format!("{venue}: this key can trade or withdraw; use a read-only key")),
            Ok((_, Permission::Unreadable)) | Err(_) => return Err(format!("{venue}: this key's permissions could not be read; refused")),
        }
    } else if venue != "hyperliquid" {
        eprintln!("warning: {venue} cannot tell this key's permissions; use a read-only key");
    }
    let mut got = Vec::new();
    let mut counts: HashMap<(&str, Case), usize> = HashMap::new();
    for e in &eps {
        for case in cases(e) {
            let (r, params) = request(e, case, w, keys, base, t.now());
            guard(venue, &r)?;
            let file = format!("{}{}.json", e.file, case.suffix());
            let resp = t.send(&r).await.map_err(|x| format!("{venue} {file}: the request failed ({}); nothing written", failure(&x)))?;
            if !(200..300).contains(&resp.status) { return Err(format!("{venue} {file}: HTTP {}; nothing written", resp.status)); }
            if long_integer(&resp.body) { return Err(format!("{venue} {file}: an integer of 20+ digits would be lossy; nothing written")); }
            let body: Value = serde_json::from_str(&resp.body).map_err(|_| format!("{venue} {file}: the answer is not JSON; nothing written"))?;
            let n = orders(e, &body)?;
            counts.insert((e.file, case), n);
            got.push(Got { file, e, params, status: resp.status, body, orders: n });
        }
    }
    // The cases must show what they are for (r1.6). Where a full history lists < 2 orders (KuCoin classic on an HF
    // account lists none), its page/empty prove nothing and are dropped, named under `not_recorded`; the full file stays
    // as the envelope's shape, its order count in the manifest. At least one history read must list ≥ 2.
    let mut dropped = Vec::new();
    for e in eps.iter().filter(|e| e.history) {
        let full = counts[&(e.file, Case::Full)];
        if full < 2 {
            dropped.extend(cases(e).into_iter().filter(|c| *c != Case::Full).map(|c| format!("{}{}.json", e.file, c.suffix())));
            continue;
        }
        if counts.get(&(e.file, Case::Page)).is_some_and(|n| *n != 1) { return Err(format!("{venue} {}: the page case did not list exactly 1 order; nothing written", e.file)); }
        if counts.get(&(e.file, Case::Empty)).is_some_and(|n| *n != 0) { return Err(format!("{venue} {}: the empty window listed orders; nothing written", e.file)); }
    }
    if !eps.iter().any(|e| e.history && counts[&(e.file, Case::Full)] >= 2) {
        return Err(format!("{venue}: no history read lists 2 orders (place the test orders, check --since/--until); nothing written"));
    }
    got.retain(|g| !dropped.contains(&g.file));
    let mut files = Vec::new();
    let mut entries = Vec::new();
    for g in &got { sc.reserve(&g.body); }
    for g in got {
        let body = sc.scrub(venue, &g.file, g.body)?;
        let mut params = Map::new();
        for (k, v) in g.params {
            let window = g.e.window.filter(|(s, end, _)| k == *s || k == *end);
            let v = match (k, window) {
                (_, Some(_)) => sc.shift_str(&v).ok_or_else(|| format!("{venue}: a window param did not shift"))?,
                ("user", None) => sc.fake(&v, false, "params/user")?,
                _ => v,
            };
            params.insert(k.into(), Value::String(v));
        }
        entries.push(json!({ "file": g.file, "method": g.e.method, "path": g.e.path, "params": params, "status": g.status, "orders": g.orders }));
        files.push((g.file, body));
    }
    let report = sc.report_text();
    let recorded_at = sc.shift_str(&Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)).expect("an RFC 3339 time shifts");
    let scrubbed: Map<String, Value> = sc.report.iter().map(|(f, kinds)| (f.clone(), json!(kinds))).collect();
    let manifest = json!({ "exchange": venue, "recorded_at": recorded_at, "files": entries, "not_recorded": dropped, "scrubbed": scrubbed });
    let secrets = keys.secrets();
    let normalized = keys.secret.replace("\\n", "\n");
    let secrets: Vec<&str> = secrets.into_iter().chain([normalized.as_str()]).collect();
    for (name, v) in &files { sc.leak_check(name, v, &secrets)?; }
    sc.leak_check("manifest.json", &manifest, &secrets)?;
    sc.leak_check("report", &Value::String(report.clone()), &secrets)?;
    let dir = out.join(venue);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    // A rerun replaces the whole set: an earlier run's files (another salt and offset) must not linger beside it.
    for old in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let old = old.map_err(|e| e.to_string())?.path();
        if old.extension().is_some_and(|x| x == "json") { std::fs::remove_file(&old).map_err(|e| format!("{}: {e}", old.display()))?; }
    }
    files.push(("manifest.json".into(), manifest));
    for (name, v) in &files {
        let text = serde_json::to_string_pretty(v).expect("a Value serializes") + "\n";
        std::fs::write(dir.join(name), text).map_err(|e| format!("{name}: {e}"))?;
    }
    let dropped = if dropped.is_empty() { String::new() } else { format!("\nnot recorded (full history lists < 2 orders): {}", dropped.join(", ")) };
    Ok(format!("wrote {}\nchanged fields:\n{report}{dropped}", dir.display()))
}
