//! The recorders' shared scrub and write (rust/examples/record_order_lists, rust/examples/record_ibkr): the fail-closed
//! Scrubber (consistent class-preserving fakes, whole-day time shifts, the leak check, the change report), A28's own walk
//! over its field lists, and the all-or-nothing `write_set`. Each recorder includes this file by `#[path]`; it is no
//! example of its own (no main.rs).
#![allow(dead_code)] // each including crate uses only part of it

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime};
use hmac::{Hmac, Mac};
use serde_json::{json, Number, Value};
use sha2::Sha256;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

/// The success field a venue's envelope carries, checked on the raw body and kept verbatim through the scrub.
/// The success field a venue's envelope carries, checked on the raw body and kept verbatim through the scrub.
pub fn success(venue: &str) -> Option<(&'static str, Value)> {
    match venue {
        "kraken" => Some(("error", json!([]))),
        "kucoin" => Some(("code", json!("200000"))),
        "bitget" => Some(("code", json!("00000"))),
        "bingx" => Some(("code", json!(0))),
        _ => None,
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

pub fn norm(k: &str) -> String { k.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect() }

pub fn kept_shape(s: &str) -> bool {
    let mut run = 0;
    s.len() <= 32
        && s.chars().all(|c| {
            run = if c.is_ascii_digit() { run + 1 } else { 0 };
            run < 5 && (c.is_ascii_alphanumeric() || "_/ .-@".contains(c))
        })
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
        // A zero is kept, never a leak (`zero`): reserving it would bar every fake sharing its leading zeros.
        visit(v, "", &mut |text, _| { if text.chars().count() >= 4 && !zero(text) { seen.insert(text.to_string()); } });
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
pub fn decimal(s: &str) -> bool {
    let s = s.strip_prefix('-').unwrap_or(s);
    let (int, frac) = match s.split_once('.') { Some((i, f)) => (i, Some(f)), None => (s, None) };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    digits(int) && frac.is_none_or(digits)
}

/// An all-zero amount (`0`, `"0.00000000"`): "nothing filled" is what A28-3 reads, and it says nothing about anyone. Only
/// a decimal's text: an all-zero id like `"0000-0000"` is replaced.
pub fn zero(s: &str) -> bool { decimal(s) && !s.bytes().any(|b| (b'1'..=b'9').contains(&b)) }

/// Every key, string and number text in `v`, with its folded path; a key's own text reports its parent's path.
pub fn visit(v: &Value, path: &str, f: &mut dyn FnMut(&str, &str)) {
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

impl Scrubber {
    /// `random`, with an offset of a whole number of weeks (35–399 days): a stock's trading day stays a weekday.
    pub fn random_weeks() -> Self {
        let salt = [*uuid::Uuid::new_v4().as_bytes(), *uuid::Uuid::new_v4().as_bytes()].concat();
        let days = 7 * (5 + i64::from(u16::from_be_bytes([salt[0], salt[1]]) % 53));
        Self::new(salt, days)
    }
    /// `original`'s fake set by the caller (an IBKR account keeps its prefix): every later sight of it maps there.
    pub fn pin(&mut self, original: &str, fake: &str) {
        self.map.insert(original.to_string(), fake.to_string());
        self.originals.insert(original.to_string());
        self.fakes.insert(fake.to_string());
    }
    /// The fake `s` already maps to, if any.
    pub fn mapped(&self, s: &str) -> Option<String> { self.map.get(s).cloned() }
    /// `s` counted as replaced by a constant (no fake), so the leak check still looks for it.
    pub fn dropped(&mut self, s: &str) { self.originals.insert(s.to_string()); }
    /// Counts one change of `kind` at `path` in `file`, for the report.
    pub fn count(&mut self, file: &str, path: &str, kind: &'static str) { self.note(file, path, kind); }
    /// A compact IBKR time (`%Y%m%d`, `%Y%m%d-%H:%M:%S`, `%y%m%d%H%M%S`) moved back by the offset, its format kept.
    pub fn shift_compact(&self, s: &str) -> Option<String> {
        let fmts: &[(&str, usize)] = &[("%Y%m%d-%H:%M:%S", 17), ("%y%m%d%H%M%S", 12)];
        for (f, n) in fmts {
            if s.len() == *n {
                if let Ok(t) = NaiveDateTime::parse_from_str(s, f) { return Some((t - Duration::days(self.days)).format(f).to_string()); }
            }
        }
        if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(d) = NaiveDate::parse_from_str(s, "%Y%m%d") { return Some((d - Duration::days(self.days)).format("%Y%m%d").to_string()); }
        }
        None
    }
}

/// Writes `files` (name, body) as `dir`'s whole set, all or nothing: staged beside `dir` (its non-JSON files copied over),
/// then swapped in by two renames. A failure before the swap leaves `dir` as it was; a failed second rename moves it back.
pub fn write_set(dir: &Path, files: &[(String, Value)]) -> Result<(), String> {
    let parent = dir.parent().ok_or("the output dir has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let name = dir.file_name().ok_or("the output dir has no name")?.to_string_lossy().to_string();
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let stage = parent.join(format!(".{name}.staging-{tag}"));
    let old = parent.join(format!(".{name}.old-{tag}"));
    let staged = (|| -> Result<(), String> {
        std::fs::create_dir(&stage).map_err(|e| format!("{}: {e}", stage.display()))?;
        if dir.exists() {
            for f in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                let f = f.map_err(|e| e.to_string())?.path();
                if f.is_file() && !f.extension().is_some_and(|x| x == "json") {
                    std::fs::copy(&f, stage.join(f.file_name().expect("a listed file has a name"))).map_err(|e| format!("{}: {e}", f.display()))?;
                }
            }
        }
        for (file, v) in files {
            let text = serde_json::to_string_pretty(v).expect("a Value serializes") + "\n";
            std::fs::write(stage.join(file), text).map_err(|e| format!("{file}: {e}"))?;
        }
        Ok(())
    })();
    if let Err(e) = staged { let _ = std::fs::remove_dir_all(&stage); return Err(format!("{e}; nothing written")); }
    let had = dir.exists();
    if had { std::fs::rename(dir, &old).map_err(|e| { let _ = std::fs::remove_dir_all(&stage); format!("{}: {e}; nothing written", dir.display()) })?; }
    if let Err(e) = std::fs::rename(&stage, dir) {
        if had { let _ = std::fs::rename(&old, dir); }
        let _ = std::fs::remove_dir_all(&stage);
        return Err(format!("{}: {e}; nothing written", dir.display()));
    }
    if had { let _ = std::fs::remove_dir_all(&old); }
    Ok(())
}
