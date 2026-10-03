//! JSON as Rails writes it. Rails' encoder here is Oj in Rails mode (config/initializers/oj.rb): a BigDecimal is
//! a string (`to_s`, plain notation), a Float is `num::oj_float`, a Time is ISO 8601 with a fixed number of
//! fractional digits, cut not rounded, and `<`, `>`, `&`, U+2028 and U+2029 are escaped in strings.
use super::num::{oj_float, Num};
use chrono::{DateTime, Offset, TimeZone, Utc};
use chrono_tz::{OffsetName, Tz};

#[derive(Clone, Debug, PartialEq)]
pub enum J { Null, Bool(bool), Int(i64), Float(f64), Str(String), Arr(Vec<J>), Obj(Vec<(String, J)>) }

impl J {
    pub fn num(n: &Num) -> J {
        match n { Num::Int(i) => J::Int(*i), Num::Dec(d) => J::Str(d.to_s_f()), Num::Float(f) => J::Float(*f) }
    }
    pub fn opt<T>(v: &Option<T>, f: impl Fn(&T) -> J) -> J { v.as_ref().map_or(J::Null, f) }
    pub fn arr<T>(items: &[T], f: impl Fn(&T) -> J) -> J { J::Arr(items.iter().map(f).collect()) }
    pub fn obj<T>(pairs: &[(String, T)], f: impl Fn(&T) -> J) -> J { J::Obj(pairs.iter().map(|(k, v)| (k.clone(), f(v))).collect()) }

    pub fn write(&self) -> String {
        let mut out = String::new();
        self.put(&mut out);
        out
    }

    fn put(&self, out: &mut String) {
        match self {
            J::Null => out.push_str("null"),
            J::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            J::Int(i) => out.push_str(&i.to_string()),
            J::Float(f) => out.push_str(&oj_float(*f)),
            J::Str(s) => string(s, out),
            J::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    item.put(out);
                }
                out.push(']');
            }
            J::Obj(pairs) => {
                out.push('{');
                for (i, (key, value)) in pairs.iter().enumerate() {
                    if i > 0 { out.push(','); }
                    string(key, out);
                    out.push(':');
                    value.put(out);
                }
                out.push('}');
            }
        }
    }
}

fn string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn written<Z: TimeZone>(t: &DateTime<Z>, digits: usize, utc: bool) -> String {
    let local = t.naive_local();
    let nanos = format!("{:09}", t.timestamp_subsec_nanos().min(999_999_999));
    let fraction = if digits == 0 { String::new() } else { format!(".{}", &nanos[..digits.min(9)]) };
    let offset = t.offset().fix().local_minus_utc();
    let zone = if utc { "Z".to_string() } else { format!("{}{:02}:{:02}", if offset < 0 { '-' } else { '+' }, offset.abs() / 3600, offset.abs() % 3600 / 60) };
    format!("{}{fraction}{zone}", local.format("%Y-%m-%dT%H:%M:%S"))
}

/// A Time in UTC: `2026-03-02T14:30:00.250Z`, `digits` fractional digits (Rails: 3).
pub fn time_utc(t: &DateTime<Utc>, digits: usize) -> String { written(t, digits, true) }

/// A time in a user's zone (ActiveSupport::TimeWithZone): `2026-03-02T15:30:00.250+01:00`. `Z` only where the
/// zone itself is UTC (its abbreviation is `UTC`): London in winter is `+00:00`.
pub fn time(t: &DateTime<Tz>, digits: usize) -> String {
    written(t, digits, matches!(t.offset().abbreviation(), Some("UTC" | "UCT")))
}
