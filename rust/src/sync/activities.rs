//! Exchanges::Alpaca#get_ledger's pure half: one account activity → one ledger entry (#normalize_activity and its
//! helpers), split legs merged into one signed adjustment (#merge_split_entries), and the ratio a restatement applied
//! (.split_ratio_label). No database and no network; pinned by Ruby vectors (rust/tests/sync_vectors.rs).
use super::number;
use super::wire::{self, Budget, Node};
use crate::ruby::BigDec;
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;

/// AccountTransaction.entry_types, as stored.
pub const BUY: i64 = 0;
pub const SELL: i64 = 1;
pub const DEPOSIT: i64 = 4;
pub const WITHDRAWAL: i64 = 5;
pub const FEE: i64 = 10;
pub const OTHER_INCOME: i64 = 11;
pub const WITHHOLDING_TAX: i64 = 13;
pub const RETURN_OF_CAPITAL: i64 = 14;
pub const ADJUSTMENT: i64 = 15;
pub const UNSUPPORTED_ACTIVITY: i64 = 16;

pub const DIVIDEND_INCOME_TYPES: [&str; 5] = ["DIV", "DIVCGL", "DIVCGS", "CGD", "DIVTXEX"];
pub const WITHHOLDING_TYPES: [&str; 5] = ["DIVNRA", "DIVFT", "DIVTW", "INTNRA", "INTTW"];
pub const CASH_FEE_TYPES: [&str; 3] = ["FEE", "DIVFEE", "PTC"];
pub const CASH_JOURNAL_TYPES: [&str; 3] = ["JNLC", "OCT", "ACATC"];
pub const SPLIT_TYPES: [&str; 2] = ["SPLIT", "SSP"];
/// CASH_ACTIVITY_TYPES: the activities that move cash and nothing else.
pub fn cash_activity(t: &str) -> bool {
    ["CSD", "CSW", "INT", "PTR"].contains(&t) || CASH_JOURNAL_TYPES.contains(&t) || DIVIDEND_INCOME_TYPES.contains(&t)
        || WITHHOLDING_TYPES.contains(&t) || CASH_FEE_TYPES.contains(&t)
}

/// One row of what #get_ledger returns. Fees are part of the shape every venue adapter returns; Alpaca never sets them.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub entry_type: i64,
    pub base_currency: Option<String>,
    pub base_amount: BigDec,
    pub quote_currency: Option<String>,
    pub quote_amount: Option<BigDec>,
    pub fee_currency: Option<String>,
    pub fee_amount: Option<BigDec>,
    /// The venue's id as it sent it; AccountTransactionSync reads a blank one as none.
    pub tx_id: Option<String>,
    pub group_id: Option<String>,
    pub description: Option<String>,
    pub transacted_at: Option<DateTime<Utc>>,
    pub raw: Raw,
}

/// One JSON object as Rails would hold it (`wire`): the parsed value the port reads, and each member as the venue
/// wrote it. A number is read from the venue's own text (`number`), so its size is judged before any Float; what is
/// stored as `raw_data` is the object as Rails stores it (`text`: Oj's serialisation of what Oj parsed, so an integer
/// of any length stays exact and a Float is printed in Oj's sixteen digits). Keys are in the venue's order; a key met
/// twice keeps its first place and its last value, at every level, as the app's parser leaves it.
#[derive(Clone, Debug, PartialEq)]
pub struct Raw { pub value: Value, members: Vec<(String, Node)> }

impl Raw {
    /// From a value `wire::read` read. `Err`: it is not an object.
    pub fn from_node(node: &Node) -> Result<Self, String> {
        let Node::Object(members) = node else { return Err("not a JSON object".into()) };
        let value = serde_json::from_str(&node.text()).map_err(|_| "not JSON".to_string())?;
        Ok(Self { value, members: members.clone() })
    }
    /// From text of this port's own keeping (a stored row, a test), read without a budget. `None`: not JSON, or
    /// nested past Rails' limit. `Some(Err)`: not an object.
    pub fn parse(text: &str) -> Option<Result<Self, String>> {
        wire::read(text, &mut Budget(usize::MAX), None).ok().map(|node| Self::from_node(&node))
    }
    /// For a value already parsed (tests, the vectors): each scalar's text is its serialisation.
    pub fn from_value(value: Value) -> Self {
        let members = match Node::from_value(&value) { Node::Object(members) => members, _ => vec![] };
        Self { value, members }
    }
    /// The member `key`, as the venue wrote it.
    pub fn member(&self, key: &str) -> Option<&Node> { self.members.iter().find(|(k, _)| k == key).map(|(_, node)| node) }
    /// `self[key]&.to_d`, within the venue caps. The error names the key and never the value.
    pub fn number(&self, key: &str) -> Result<Option<BigDec>, String> {
        match self.member(key) {
            None => Ok(None),
            Some(node) => number::json(&node.text()).map_err(|why| format!("unreadable {key}: {why}")),
        }
    }
    /// `self[key].to_d`: an absent or null value is 0 (`nil.to_d`).
    pub fn decimal(&self, key: &str) -> Result<BigDec, String> { Ok(self.number(key)?.unwrap_or_else(BigDec::zero)) }
    /// The member `key` as an object of its own; `None` when it is absent or null.
    pub fn object(&self, key: &str) -> Result<Option<Raw>, String> {
        match self.member(key) {
            None => Ok(None),
            Some(Node::Scalar(text)) if text == "null" => Ok(None),
            Some(node) => Raw::from_node(node).map(Some).map_err(|why| format!("unreadable {key}: {why}")),
        }
    }
    /// `hash[key] = node`: replaced in place, or added at the end.
    pub fn set_node(&mut self, key: &str, node: Node) {
        self.value[key] = serde_json::from_str(&node.text()).unwrap_or(Value::Null);
        match self.members.iter_mut().find(|(k, _)| k == key) {
            Some(member) => member.1 = node,
            None => self.members.push((key.to_string(), node)),
        }
    }
    /// `hash[key] = v`, for a value of the port's own (the keys a split row gains).
    pub fn set(&mut self, key: &str, v: Value) { self.set_node(key, Node::from_value(&v)); }
    /// The object as Rails stores it in `raw_data`, byte for byte (`wire::Node::stored`).
    pub fn text(&self) -> String { wire::stored_members(&self.members) }
}

/// The venue's available crypto pairs by their compact name ("ETHUSD"): #crypto_position_index.
#[derive(Clone, Debug, Default)]
pub struct CryptoPairs(pub HashMap<String, CryptoPair>);
#[derive(Clone, Debug)]
pub struct CryptoPair { pub base: String, pub quote: String, pub asset_symbol: Option<String>, pub base_asset_id: i64 }

fn text(v: &Value) -> Option<String> { v.as_str().map(str::to_string) }

/// What an activity whose time this port cannot read fails the sync with. It names no value of the answer.
const UNREADABLE_TIME: &str = "unreadable activity time";

/// `Time.parse(s).utc` for the shapes Alpaca sends: RFC 3339. Anything else is an error here (Ruby reads more).
fn trade_time(v: &Value) -> Result<DateTime<Utc>, String> {
    v.as_str().filter(|s| s.len() <= 64).and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|t| t.with_timezone(&Utc))
        .ok_or_else(|| UNREADABLE_TIME.to_string())
}

/// #non_trade_timestamp: `date` (midnight UTC) or `transaction_time`; nil when the activity carries neither.
fn non_trade_time(a: &Value) -> Result<Option<DateTime<Utc>>, String> {
    let v = if a["date"].is_null() { &a["transaction_time"] } else { &a["date"] };
    if v.is_null() { return Ok(None); }
    let s = v.as_str().ok_or_else(|| UNREADABLE_TIME.to_string())?;
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d.and_hms_opt(0, 0, 0).map(|t| t.and_utc()));
    }
    trade_time(v).map(Some)
}

fn abs(d: BigDec) -> BigDec { if d < BigDec::zero() { &BigDec::zero() - &d } else { d } }

fn non_trade(raw: &Raw, entry_type: i64, description: Option<String>, quote_currency: Option<String>) -> Result<Entry, String> {
    let a = &raw.value;
    Ok(Entry { entry_type, base_currency: Some("USD".into()), base_amount: abs(raw.decimal("net_amount")?), quote_currency, quote_amount: None,
               fee_currency: None, fee_amount: None, tx_id: text(&a["id"]), group_id: text(&a["group_id"]), description,
               transacted_at: non_trade_time(a)?, raw: raw.clone() })
}

/// #normalize_activity. `None` is a cancelled non-trade activity (dropped). `Err` is a time this port cannot read, or
/// a number outside the venue caps (`number::VENUE`): the sync fails, as Rails' does when Ruby raises on a value.
pub fn normalize(raw: &Raw, pairs: &CryptoPairs) -> Result<Option<Entry>, String> {
    let a = &raw.value;
    let kind = a["activity_type"].as_str().unwrap_or_default();
    let symbol = text(&a["symbol"]);
    if dropped(a) { return Ok(None); }
    if kind == "FILL" {
        let (qty, price) = (raw.decimal("qty")?, raw.decimal("price")?);
        let (base, quote) = match &symbol {
            Some(s) if s.contains('/') => { let (b, q) = s.split_once('/').unwrap_or((s, "")); (Some(b.to_string()), q.to_string()) }
            Some(s) => match pairs.0.get(s) { Some(p) => (Some(p.base.clone()), p.quote.clone()), None => (Some(s.clone()), "USD".into()) },
            None => (None, "USD".into()),
        };
        return Ok(Some(Entry { entry_type: if a["side"] == "buy" { BUY } else { SELL }, base_currency: base, quote_amount: Some(&qty * &price), base_amount: qty,
                               quote_currency: Some(quote), fee_currency: None, fee_amount: None, tx_id: text(&a["id"]), group_id: None, description: None,
                               transacted_at: Some(trade_time(&a["transaction_time"])?), raw: raw.clone() }));
    }
    let entry = match kind {
        "CSD" => non_trade(raw, DEPOSIT, None, None)?,
        "CSW" => non_trade(raw, WITHDRAWAL, None, None)?,
        k if CASH_JOURNAL_TYPES.contains(&k) => non_trade(raw, if raw.decimal("net_amount")? < BigDec::zero() { WITHDRAWAL } else { DEPOSIT }, None, None)?,
        k if DIVIDEND_INCOME_TYPES.contains(&k) => non_trade(raw, OTHER_INCOME, Some(format!("Dividend ({})", symbol.clone().unwrap_or_default())), symbol)?,
        k if WITHHOLDING_TYPES.contains(&k) => non_trade(raw, WITHHOLDING_TAX, Some(format!("Withholding ({})", symbol.clone().unwrap_or_else(|| "interest".into()))), symbol)?,
        "DIVROC" => Entry { entry_type: RETURN_OF_CAPITAL, base_amount: raw.decimal("qty")?, quote_currency: Some("USD".into()), quote_amount: Some(raw.decimal("net_amount")?),
                            description: Some(format!("Return of capital ({})", symbol.clone().unwrap_or_default())), base_currency: symbol, ..non_trade(raw, 0, None, None)? },
        k if CASH_FEE_TYPES.contains(&k) => non_trade(raw, FEE, None, None)?,
        // The fee is `qty`, charged in the coin; `symbol` is the compact pair ("ETHUSD").
        "CFEE" => Entry { base_currency: symbol.as_ref().and_then(|s| pairs.0.get(s)).and_then(|p| p.asset_symbol.clone()).or(symbol),
                          base_amount: abs(raw.decimal("qty")?), ..non_trade(raw, FEE, None, None)? },
        "INT" | "PTR" => non_trade(raw, OTHER_INCOME, None, None)?,
        k if SPLIT_TYPES.contains(&k) => {
            let mut marked = raw.clone();
            marked.set("corporate_action", json!("split")); // the marker everything that reacts to a split keys off
            Entry { entry_type: ADJUSTMENT, base_amount: raw.decimal("qty")?, description: Some(format!("Split ({})", symbol.clone().unwrap_or_default())),
                    base_currency: symbol, raw: marked, ..non_trade(raw, 0, None, None)? }
        }
        // Mergers, spin-offs, option events and future types: kept inert and verbatim.
        _ => Entry { entry_type: UNSUPPORTED_ACTIVITY, base_currency: Some(symbol.unwrap_or_else(|| "USD".into())), base_amount: raw.decimal("qty")?,
                     quote_amount: raw.number("net_amount")?, description: text(&a["activity_type"]), ..non_trade(raw, 0, None, None)? },
    };
    Ok(Some(entry))
}

/// A cancelled activity that is not a trade: #normalize_activity returns nil for it (exchanges/alpaca.rb:686), so it
/// is in no entry and stands between no two split legs. A `FILL` is never dropped.
pub fn dropped(activity: &Value) -> bool { activity["activity_type"] != "FILL" && activity["status"] == "canceled" }

fn split_leg(activity: &Value) -> bool { activity["activity_type"].as_str().is_some_and(|t| SPLIT_TYPES.contains(&t)) }

/// #merge_split_entries' test between two neighbours (exchanges/alpaca.rb:855-859): both split legs, the same symbol,
/// the same `date`.
fn same_split(previous: &Value, current: &Value) -> bool {
    split_leg(previous) && split_leg(current) && previous["symbol"].as_str() == current["symbol"].as_str() && previous["date"] == current["date"]
}

/// The group #merge_split_entries would be building at the end of these activities: where its first leg stands, and
/// how many legs it has so far. `None` when the read does not end in a split leg. Judged on the sequence Rails groups,
/// which is the activities without the dropped ones: a cancelled activity between two legs does not part them.
pub fn open_split(activities: &[Raw]) -> Option<(usize, usize)> {
    let mut kept = activities.iter().enumerate().rev().filter(|(_, a)| !dropped(&a.value));
    let (mut start, mut first) = kept.next()?;
    if !split_leg(&first.value) { return None; }
    let mut legs = 1;
    for (i, activity) in kept {
        if !same_split(&activity.value, &first.value) { break; }
        (start, first, legs) = (i, activity, legs + 1);
    }
    Some((start, legs))
}

/// How many legs the group #merge_split_entries would make starting at the first of these activities has (0 when
/// that is no split leg), the dropped activities aside: the split a run reads on for, counted as it grows.
pub fn leading_split(activities: &[Raw]) -> usize {
    let mut kept = activities.iter().filter(|a| !dropped(&a.value));
    let Some(mut previous) = kept.next().filter(|a| split_leg(&a.value)) else { return 0 };
    let mut legs = 1;
    for activity in kept {
        if !same_split(&previous.value, &activity.value) { break; }
        (previous, legs) = (activity, legs + 1);
    }
    legs
}

/// #merge_split_entries, exactly: split legs of one symbol and one `date` that are consecutive in the read become
/// the first leg, carrying the net share delta, every leg's id and the ratio. Legs that are not neighbours stay apart,
/// as in Rails (its defects with splits are listed in the plan and ported as they are). `Err`: a group whose ratio is
/// not a number this port can reduce (Ruby raises FloatDomainError on one that is not finite), which fails the sync.
pub fn merge_splits(entries: Vec<Entry>) -> Result<Vec<Entry>, String> {
    let mut groups: Vec<Vec<Entry>> = vec![];
    for e in entries {
        match groups.last_mut() {
            Some(g) if g.last().is_some_and(|p| same_split(&p.raw.value, &e.raw.value)) => g.push(e),
            _ => groups.push(vec![e]),
        }
    }
    let mut merged = Vec::with_capacity(groups.len());
    for mut g in groups {
        if g.len() == 1 { merged.extend(g.pop()); continue; }
        let ratio = split_ratio(&g)?;
        let mut first = g[0].clone();
        first.base_amount = g.iter().fold(BigDec::zero(), |sum, e| &sum + &e.base_amount);
        if let Some(r) = &ratio { first.description = Some(format!("{} {r}", first.description.clone().unwrap_or_default()).trim_start().to_string()); }
        first.raw.set_node("merged_activity_ids", Node::Array(g.iter().map(|e| e.raw.member("id").cloned().unwrap_or(Node::Scalar("null".into()))).collect()));
        if let Some(r) = ratio { first.raw.set("split_ratio", json!(r)); }
        merged.push(first);
    }
    Ok(merged)
}

/// #split_ratio: the removals are the old position, the additions the new one.
fn split_ratio(group: &[Entry]) -> Result<Option<String>, String> {
    let zero = BigDec::zero();
    let sum = |keep: &dyn Fn(&BigDec) -> bool| group.iter().map(|e| &e.base_amount).filter(|a| keep(a)).fold(BigDec::zero(), |s, a| &s + a);
    split_ratio_label(&(&zero - &sum(&|a| *a < zero)), &sum(&|a| *a > zero))
}

/// Exchanges::Alpaca.split_ratio_label: "10:1", from the position before and the position after. nil unless both
/// are positive and differ, and nil when they are closer than a per-mille (1000 becoming 1001 is not a split).
///
/// `Err`: the factor is not a finite positive double, or its fraction does not fit this port's integers. Ruby raises
/// FloatDomainError for the first (`Infinity.rationalize`) and prints a bignum for the second; both fail the sync here.
pub fn split_ratio_label(old: &BigDec, new: &BigDec) -> Result<Option<String>, String> {
    if !old.is_positive() || !new.is_positive() || old == new { return Ok(None); }
    let unusable = || "a split whose ratio is not a usable number".to_string();
    let factor = new.div(old).ok_or_else(unusable)?.to_f();
    if !factor.is_finite() || factor <= 0.0 { return Err(unusable()); }
    let (p, q) = rationalize(factor, factor * 0.001).ok_or_else(unusable)?;
    Ok((p != q).then(|| format!("{p}:{q}")))
}

/// The most steps `rationalize` walks. A per-mille window closes within a dozen (the denominators grow at least as
/// the Fibonacci numbers do); the cap is what makes the walk end whatever the doubles are.
pub const RATIONALIZE_STEPS: usize = 64;
/// No term of the walk may reach this (10^30 fits an i128 with room for one more product).
const TERM_LIMIT: f64 = 1e30;

/// Float#rationalize(eps) for a positive float (rational.c: rb_flt_rationalize_with_prec, nurat_rationalize_internal):
/// the simplest fraction within [f − |eps|, f + |eps|], walked in the same double arithmetic Ruby uses. `None`: an
/// input that is not finite, a walk that is not over after `RATIONALIZE_STEPS`, or a term that does not fit (checked
/// arithmetic throughout): nothing here loops or overflows on any double.
pub fn rationalize(f: f64, eps: f64) -> Option<(i128, i128)> {
    if !f.is_finite() || !eps.is_finite() { return None; }
    let (mut a, mut b) = (f - eps.abs(), f + eps.abs());
    if a == b { return exact(f); }
    let (mut p0, mut p1, mut q0, mut q1) = (0i128, 1i128, 1i128, 0i128);
    for _ in 0..RATIONALIZE_STEPS {
        let c = a.ceil();
        if !c.is_finite() || !b.is_finite() || c.abs() >= TERM_LIMIT { return None; }
        if c < b {
            let c = c as i128;
            return Some((c.checked_mul(p1)?.checked_add(p0)?, c.checked_mul(q1)?.checked_add(q0)?));
        }
        let k = c - 1.0;
        let (p2, q2) = ((k as i128).checked_mul(p1)?.checked_add(p0)?, (k as i128).checked_mul(q1)?.checked_add(q0)?);
        let t = 1.0 / (b - k);
        b = 1.0 / (a - k);
        a = t;
        (p0, q0, p1, q1) = (p1, q1, p2, q2);
    }
    None
}

/// Float#to_r, reduced: only reached when eps is too small to move f (never for a per-mille tolerance). `None` when
/// the fraction does not fit. The loop runs at most 1,074 times (a double's smallest exponent).
fn exact(f: f64) -> Option<(i128, i128)> {
    let bits = f.to_bits();
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = (bits & ((1u64 << 52) - 1)) as i128;
    let (mut m, mut e) = if exp == 0 { (frac, -1074) } else { (frac | (1i128 << 52), exp - 1075) };
    while m != 0 && m % 2 == 0 && e < 0 { m /= 2; e += 1; }
    if bits >> 63 == 1 { m = -m; }
    match e {
        0..=60 => Some((m << e, 1)),
        -120..=-1 => Some((m, 1i128 << -e)),
        _ => None,
    }
}
