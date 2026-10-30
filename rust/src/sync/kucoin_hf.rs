//! D-KUCOIN-HF-LEDGER (plan HF-3 §3): one trade, one row on a KuCoin history whose account has HF trading. Everything
//! available is imported, from the API and from KuCoin's export, and each trade is counted once by an exact per-second
//! rule; what nothing covers is signalled on the existing "Import history" line. Classic-only histories (no
//! `hf_enabled` marker) never reach this module.
//!
//! - **Coverage** (`kucoin_coverage`): the marker, each export's period, certified HF units, every HF unit, pairs KuCoin
//!   no longer lists, and the seconds the rule could not pair.
//! - **The rule** (`rederive`): a second's export `fills` derivations against the trades that represent it (the visible
//!   API BUY/SELL rows, or the user's restored entries where there are any): same effect, else one for one, else the
//!   leftover is ambiguous (the API wins and the second is flagged) when an unpaired API trade is within a second of it.
//! - **Allocation** (`Allocation`): which incoming fill-class entries of a store are rows already held, by exact key.
//! - **The gap** (`refresh_gap`): what no certified unit or export covers, and every ambiguous second.
use super::activities::{Entry, Raw, ADJUSTMENT, BUY, FEE, SELL};
use super::{sql_time, Key, SyncError};
use crate::codec::parse_time;
use crate::engine::model::FencedTransaction;
use crate::ruby::BigDec;
use crate::tracker::import::kucoin::{self as file, Catalogue, Derived, SourceRow, Stored, API_ROW, DERIVED};
use chrono::{DateTime, Duration, Timelike, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const ENABLED: &str = "hf_enabled";
pub const EXPORT: &str = "export";
pub const HF_API: &str = "hf_api";
pub const HF_ATTEMPT: &str = "hf_attempt";
pub const HF_ATTEMPT_OPEN: &str = "hf_attempt_open";
pub const AMBIGUOUS: &str = "ambiguous";
pub const MISSING: &str = "missing:";
/// What a page that meets a stored fill with another reading of it fails with: nothing of it is written.
pub const CONFLICT: &str = "KuCoin fill conflict: ";
/// The fill-class entries the rule and the allocation cover.
pub const FILL_TYPES: [i64; 4] = [BUY, SELL, FEE, ADJUSTMENT];

/// The derived gap row: `{exchange_id: {key, credential, uncovered: [[start, end], …]}}`, rebuilt if lost.
/// The spans the gap row lists (about 50 bytes each); `spans` counts them all.
const MAX_SPANS: usize = 500;
pub fn gap_key(user_id: i64) -> String { format!("rust_kucoin_hf_gap.{user_id}") }

/// A KuCoin fill-class row with neither this importer's provenance nor the API's: restored, or entered by the user.
fn provenance_less() -> String { format!("entry_type IN ({BUY}, {SELL}, {FEE}, {ADJUSTMENT}) AND NOT ({API_ROW}) AND NOT ({DERIVED})") }

pub type Span = (DateTime<Utc>, DateTime<Utc>);
pub fn second(t: DateTime<Utc>) -> DateTime<Utc> { t.with_nanosecond(0).unwrap_or(t) }
fn time(s: &str) -> Result<DateTime<Utc>, SyncError> { parse_time(s).map_err(|e| SyncError(format!("kucoin_coverage time {s}: {e:?}"))) }

/// Whether (user, exchange) is an HF-enabled KuCoin history.
pub fn enabled(c: &Connection, user_id: i64, exchange_id: i64) -> Result<bool, SyncError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM kucoin_coverage WHERE user_id = ?1 AND exchange_id = ?2 AND source = ?3)", params![user_id, exchange_id, ENABLED], |r| r.get(0))?)
}

/// Whether any history is HF-enabled (the handback refuses then).
pub fn any_enabled(c: &Connection) -> Result<bool, SyncError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM kucoin_coverage WHERE source = ?1)", [ENABLED], |r| r.get(0))?)
}

/// The spans of `source` (a `missing:` prefix matches every pair), oldest first.
pub fn spans(c: &Connection, key: &Key, source: &str) -> Result<Vec<(String, Span)>, SyncError> {
    let (op, pattern) = if source == MISSING { ("LIKE", format!("{MISSING}%")) } else { ("=", source.to_string()) };
    let mut s = c.prepare_cached(&format!("SELECT source, start_at, end_at FROM kucoin_coverage WHERE user_id = ?1 AND exchange_id = ?2 AND source {op} ?3 ORDER BY start_at, end_at"))?;
    let rows: Vec<(String, String, String)> = s.query_map(params![key.user_id, key.exchange_id, pattern], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    rows.into_iter().map(|(source, a, b)| Ok((source, (time(&a)?, time(&b)?)))).collect()
}

/// One coverage row, unless the same one is held.
pub fn add(c: &Connection, key: &Key, source: &str, (start, end): Span, now: DateTime<Utc>) -> Result<(), SyncError> {
    let (start, end) = (sql_time(start), sql_time(end));
    let held: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM kucoin_coverage WHERE user_id = ?1 AND exchange_id = ?2 AND source = ?3 AND start_at = ?4 AND end_at = ?5)",
                                 params![key.user_id, key.exchange_id, source, start, end], |r| r.get(0))?;
    if !held {
        c.execute("INSERT INTO kucoin_coverage (user_id, exchange_id, source, start_at, end_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                  params![key.user_id, key.exchange_id, source, start, end, sql_time(now)])?;
    }
    Ok(())
}

pub fn remove(c: &Connection, key: &Key, source: &str) -> Result<(), SyncError> {
    c.execute("DELETE FROM kucoin_coverage WHERE user_id = ?1 AND exchange_id = ?2 AND source = ?3", params![key.user_id, key.exchange_id, source])?;
    Ok(())
}

// ---- exact fill keys ---------------------------------------------------------------------------------------------

/// A fill as the rule compares it: exact decimals, and a zero or absent fee read as no fee.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FillKey { kind: i64, base: Option<String>, base_amount: BigDec, quote: Option<String>, quote_amount: Option<BigDec>, fee: Option<(Option<String>, BigDec)> }

fn fee(currency: Option<String>, amount: Option<BigDec>) -> Option<(Option<String>, BigDec)> { amount.filter(|a| !a.is_zero()).map(|a| (currency, a)) }

impl FillKey {
    fn of(e: &Entry) -> Self {
        Self { kind: e.entry_type, base: e.base_currency.clone(), base_amount: e.base_amount.clone(), quote: e.quote_currency.clone(), quote_amount: e.quote_amount.clone(),
               fee: fee(e.fee_currency.clone(), e.fee_amount.clone()) }
    }
    /// An API fill's raw (`kucoin_ledger::fill`'s reading of it).
    fn api(raw: &Value) -> Result<Self, SyncError> {
        super::kucoin_ledger::fill(Raw::from_value(raw.clone())).map(|e| Self::of(&e)).map_err(|(why, _)| SyncError(format!("stored KuCoin fill: {why}")))
    }
}

/// The net change per currency, and per (pair, side) the summed base and quote.
type Effect = (BTreeMap<String, BigDec>, BTreeMap<(Option<String>, Option<String>, i64), (BigDec, BigDec)>);
fn effect<'a>(keys: impl IntoIterator<Item = &'a FillKey>) -> Effect {
    let (mut net, mut pairs): Effect = Default::default();
    let mut add = |currency: &Option<String>, amount: BigDec| {
        let n = net.entry(currency.clone().unwrap_or_default()).or_insert_with(BigDec::zero);
        *n = &*n + &amount;
    };
    for k in keys {
        let neg = |d: &BigDec| &BigDec::zero() - d;
        match k.kind {
            BUY | SELL => {
                let (sign_base, sign_quote) = if k.kind == BUY { (1, -1) } else { (-1, 1) };
                let q = k.quote_amount.clone().unwrap_or_else(BigDec::zero);
                add(&k.base, if sign_base > 0 { k.base_amount.clone() } else { neg(&k.base_amount) });
                add(&k.quote, if sign_quote > 0 { q.clone() } else { neg(&q) });
                let p = pairs.entry((k.base.clone(), k.quote.clone(), k.kind)).or_insert_with(|| (BigDec::zero(), BigDec::zero()));
                *p = (&p.0 + &k.base_amount, &p.1 + &q);
            }
            FEE => add(&k.base, neg(&k.base_amount)),
            _ => add(&k.base, k.base_amount.clone()),
        }
        if let Some((currency, amount)) = &k.fee { add(currency, neg(amount)); }
    }
    net.retain(|_, v| !v.is_zero());
    (net, pairs)
}

// ---- the rule ----------------------------------------------------------------------------------------------------

/// A row that represents trades at its second: an API fill, or a restored entry (`key` None: no exact amounts, never
/// matched).
struct Rep { order: String, key: Option<FillKey>, api: bool, inexact: bool }

/// One second's rules 1–2.
struct Judged { csv: Vec<Derived>, other: Vec<Derived>, represented: Vec<bool>, reps: Vec<Rep>, used: Vec<bool>, restored: bool, withheld: Vec<FillKey>, held: bool }

fn at_second(t: DateTime<Utc>) -> (String, String) { (sql_time(t), sql_time(t + Duration::seconds(1))) }

/// The provenance-less fill rows at second `t`: (id, tx_id, type, base, base amount, quote, quote amount, fee currency, fee amount, raw).
type Restored = (i64, Option<String>, i64, Option<String>, f64, Option<String>, Option<f64>, Option<String>, Option<f64>, Value);
fn restored_rows(c: &Connection, key: &Key, t: DateTime<Utc>) -> Result<Vec<Restored>, SyncError> {
    let (from, to) = at_second(t);
    let mut s = c.prepare_cached(&format!("SELECT id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, raw_data \
                                           FROM account_transactions {} WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at >= ?3 AND transacted_at < ?4 AND {} ORDER BY id",
                                          super::ledger::BY_TIME, provenance_less()))?;
    let rows = s.query_map(params![key.user_id, key.exchange_id, from, to], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get::<_, Option<f64>>(4)?.unwrap_or_default(),
        r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get::<_, Option<String>>(9)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().map(|(a, b, cc, d, e, f, g, h, i, raw)| (a, b, cc, d, e, f, g, h, i, raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null))).collect())
}

/// A restored row's exact key, from the `amounts` strings its writer kept; None without them.
fn restored_key(r: &Restored) -> Option<FillKey> {
    let a = r.9.get("amounts")?;
    let d = |k: &str| -> Option<Option<BigDec>> { match &a[k] { Value::Null => Some(None), Value::String(s) => BigDec::parse(s).ok().map(Some), _ => None } };
    Some(FillKey { kind: r.2, base: r.3.clone(), base_amount: d("base")??, quote: r.5.clone(), quote_amount: d("quote")?, fee: fee(r.7.clone(), d("fee")?) })
}

/// The visible API BUY/SELL rows at `t`: (id, tx_id, raw).
fn api_rows(c: &Connection, key: &Key, t: DateTime<Utc>) -> Result<Vec<(i64, String, Value)>, SyncError> {
    let (from, to) = at_second(t);
    let mut s = c.prepare_cached(&format!("SELECT id, COALESCE(tx_id, ''), raw_data FROM account_transactions {} WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at >= ?3 \
                                           AND transacted_at < ?4 AND entry_type IN ({BUY}, {SELL}) AND {API_ROW} ORDER BY id", super::ledger::BY_TIME))?;
    let rows = s.query_map(params![key.user_id, key.exchange_id, from, to], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().map(|(id, tx, raw)| (id, tx, raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null))).collect())
}

fn withheld_rows(c: &Connection, key: &Key, t: DateTime<Utc>) -> Result<Vec<Value>, SyncError> {
    let (from, to) = at_second(t);
    let mut s = c.prepare_cached("SELECT entry FROM kucoin_withheld_fills WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at >= ?3 AND transacted_at < ?4 ORDER BY tx_id")?;
    let rows = s.query_map(params![key.user_id, key.exchange_id, from, to], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter_map(|r| serde_json::from_str(&r).ok()).collect())
}

/// The visible API fills at a second holding restored entries move to withheld: the restore is the user's record there.
fn withhold(c: &Connection, key: &Key, t: DateTime<Utc>, now: DateTime<Utc>) -> Result<(), SyncError> {
    if restored_rows(c, key, t)?.is_empty() { return Ok(()); }
    for (id, tx_id, raw) in api_rows(c, key, t)? {
        let at: String = c.query_row("SELECT transacted_at FROM account_transactions WHERE id = ?1", [id], |r| r.get(0))?;
        c.execute("INSERT INTO kucoin_withheld_fills (user_id, exchange_id, tx_id, transacted_at, entry, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                   ON CONFLICT (user_id, exchange_id, tx_id) DO NOTHING", params![key.user_id, key.exchange_id, tx_id, at, raw.to_string(), sql_time(now)])?;
        c.execute("UPDATE account_transactions SET linked_transaction_id = NULL WHERE linked_transaction_id = ?1", [id])?;
        c.execute("DELETE FROM account_transactions WHERE id = ?1", [id])?;
    }
    Ok(())
}

fn judge(c: &Connection, key: &Key, t: DateTime<Utc>, catalogue: &Catalogue) -> Result<Judged, SyncError> {
    let held = file::held_rows(c, key, t)?;
    let (csv, other): (Vec<Derived>, Vec<Derived>) = file::derive(&held, catalogue).into_iter().partition(|d| d.entry.raw.value["class"] == "fills");
    let restored = restored_rows(c, key, t)?;
    let (reps, withheld) = if restored.is_empty() {
        let reps = api_rows(c, key, t)?.into_iter().map(|(_, tx, raw)| Ok(Rep { order: tx, key: Some(FillKey::api(&raw)?), api: true, inexact: false })).collect::<Result<Vec<_>, SyncError>>()?;
        (reps, vec![])
    } else {
        let reps = restored.iter().map(|r| Rep { order: format!("{:020}", r.0), key: restored_key(r), api: false, inexact: r.9["amounts_inexact"] == true }).collect();
        (reps, withheld_rows(c, key, t)?.iter().map(FillKey::api).collect::<Result<Vec<_>, _>>()?)
    };
    let mut represented = vec![false; csv.len()];
    let mut used = vec![false; reps.len()];
    let csv_keys: Vec<FillKey> = csv.iter().map(|d| FillKey::of(&d.entry)).collect();
    // Rule 1: the same effect.
    if !csv.is_empty() && !reps.is_empty() && reps.iter().all(|r| r.key.is_some()) && effect(&csv_keys) == effect(reps.iter().filter_map(|r| r.key.as_ref())) {
        represented.iter_mut().for_each(|r| *r = true);
        used.iter_mut().for_each(|u| *u = true);
    } else {
        // Rule 2: one for one, in a fixed order, never across seconds.
        let mut order: Vec<usize> = (0..csv.len()).collect();
        order.sort_by(|a, b| csv[*a].entry.tx_id.cmp(&csv[*b].entry.tx_id));
        let mut rep_order: Vec<usize> = (0..reps.len()).collect();
        rep_order.sort_by(|a, b| reps[*a].order.cmp(&reps[*b].order));
        for i in order {
            if let Some(&j) = rep_order.iter().find(|&&j| !used[j] && reps[j].key.as_ref() == Some(&csv_keys[i])) { used[j] = true; represented[i] = true; }
        }
    }
    Ok(Judged { csv, other, represented, reps, used, restored: !restored.is_empty(), withheld, held: !held.is_empty() })
}

/// The seconds within one of `seconds`, `by` either side.
fn widen(seconds: &BTreeSet<DateTime<Utc>>, by: i64) -> BTreeSet<DateTime<Utc>> {
    seconds.iter().flat_map(|t| (-by..=by).map(move |d| second(*t) + Duration::seconds(d))).collect()
}

/// The rule over `seconds` (r1.1: every second within one of them is judged, from rules 1–2 within two), in the caller's
/// transaction: what each judged second materializes of the export's entries, and its `ambiguous` row.
fn rederive(c: &FencedTransaction<'_>, key: &Key, seconds: &BTreeSet<DateTime<Utc>>, now: DateTime<Utc>) -> Result<Stored, SyncError> {
    let mut out = Stored::default();
    if seconds.is_empty() { return Ok(out); }
    let (judged_at, ruled_at) = (widen(seconds, 1), widen(seconds, 2));
    for t in &ruled_at { withhold(c, key, *t, now)?; }
    let catalogue = Catalogue::load(c, key.exchange_id)?;
    let cutoffs = file::cutoffs(c, key)?;
    let mut ruled: HashMap<DateTime<Utc>, Judged> = HashMap::new();
    for t in &ruled_at { ruled.insert(*t, judge(c, key, *t, &catalogue)?); }
    let unpaired_api = |t: DateTime<Utc>| ruled.get(&t).is_some_and(|j| j.reps.iter().zip(&j.used).any(|(r, u)| r.api && !u));
    let mut writer = super::ledger::FileWriter::new(c, key)?;
    for t in &judged_at {
        let j = &ruled[t];
        let leftover: Vec<usize> = (0..j.csv.len()).filter(|i| !j.represented[*i]).collect();
        let mut ambiguous = j.reps.iter().any(|r| r.inexact);
        let materialize = if j.restored {
            let unmatched = j.used.iter().any(|u| !u);
            if !j.withheld.is_empty() {
                let restored: Option<Vec<&FillKey>> = j.reps.iter().map(|r| r.key.as_ref()).collect();
                ambiguous |= restored.is_none_or(|r| effect(r) != effect(&j.withheld));
            }
            ambiguous |= !leftover.is_empty() && unmatched;
            !unmatched
        } else {
            let near = (-1..=1).any(|d| unpaired_api(*t + Duration::seconds(d)));
            let flagged = !leftover.is_empty() && near;
            ambiguous |= flagged;
            !flagged
        };
        if j.held {
            let mut derived: Vec<Derived> = if materialize { leftover.iter().map(|i| j.csv[*i].clone()).collect() } else { vec![] };
            if !file::restored_at(c, key, *t)? {
                derived.extend(j.other.iter().filter(|d| d.entry.raw.value["class"].as_str().and_then(|class| cutoffs.get(class)).is_none_or(|cut| t < cut)).cloned());
            }
            let s = file::rewrite_second(c, key, *t, derived, &mut writer, now)?;
            (out.imported, out.duplicates, out.skipped, out.generic) = (out.imported + s.imported, out.duplicates + s.duplicates, out.skipped + s.skipped, out.generic + s.generic);
        }
        c.execute("DELETE FROM kucoin_coverage WHERE user_id = ?1 AND exchange_id = ?2 AND source = ?3 AND start_at = ?4", params![key.user_id, key.exchange_id, AMBIGUOUS, sql_time(*t)])?;
        if ambiguous { add(c, key, AMBIGUOUS, (*t, *t + Duration::seconds(1)), now)?; }
    }
    Ok(out)
}

/// What every writer of an HF-enabled history calls before its transaction commits (r11): the rule over the seconds it
/// touched, then the gap row. A classic-only history is left as it is.
pub fn flush(c: &FencedTransaction<'_>, key: &Key, seconds: BTreeSet<DateTime<Utc>>, now: DateTime<Utc>) -> Result<Stored, SyncError> {
    if key.exchange_type != crate::venue::kucoin::KUCOIN || !enabled(c, key.user_id, key.exchange_id)? { return Ok(Stored::default()); }
    let seconds = seconds.into_iter().map(second).collect();
    let out = rederive(c, key, &seconds, now)?;
    refresh_gap(c, key, now)?;
    Ok(out)
}

/// The seconds of `entries`' fill-class rows.
pub fn seconds_of<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> BTreeSet<DateTime<Utc>> {
    entries.into_iter().filter(|e| FILL_TYPES.contains(&e.entry_type)).filter_map(|e| e.transacted_at).map(second).collect()
}

// ---- export coverage and the migration ------------------------------------------------------------------------------

/// An export's period, `[first, last + 1 s)`, when it covers trades: it has Spot rows and no unpaired Spot leg (a
/// currency-filtered file leaves one; a funding-only file has none).
fn period_of(rows: &[SourceRow], catalogue: &Catalogue) -> Option<Span> {
    if !rows.iter().any(|r| r.kind == "Spot") { return None; }
    let mut by: BTreeMap<DateTime<Utc>, Vec<SourceRow>> = BTreeMap::new();
    for r in rows { by.entry(r.at).or_default().push(r.clone()); }
    let unpaired = by.values().flat_map(|rows| file::derive(rows, catalogue)).any(|d| d.entry.raw.value["class"] == "fills" && d.entry.raw.value["reason"] == "unpaired");
    if unpaired { return None; }
    Some((*by.keys().next()?, *by.keys().next_back()? + Duration::seconds(1)))
}

/// The file's `export` period, in the import's transaction.
pub fn export_period(c: &Connection, key: &Key, rows: &[SourceRow], now: DateTime<Utc>) -> Result<(), SyncError> {
    if let Some(span) = period_of(rows, &Catalogue::load(c, key.exchange_id)?) { add(c, key, EXPORT, span, now)?; }
    Ok(())
}

/// The first ClassicHf detection, ONE fenced transaction, idempotent through the marker: the marker; an `export` period
/// for each earlier upload (each `import_rows.created_at` group passing the Spot test: a group can be narrower than its
/// file, so the gap shows more, never less); every import_rows second and every restored second judged by the rule, which
/// rebuilds what the classic cutoff hid (L08) and withholds API fills at restored seconds.
pub fn migrate(c: &FencedTransaction<'_>, key: &Key, now: DateTime<Utc>) -> Result<bool, SyncError> {
    if enabled(c, key.user_id, key.exchange_id)? { return Ok(false); }
    add(c, key, ENABLED, (second(now), second(now)), now)?;
    let catalogue = Catalogue::load(c, key.exchange_id)?;
    let mut s = c.prepare("SELECT DISTINCT created_at FROM import_rows WHERE user_id = ?1 AND exchange_id = ?2 ORDER BY created_at")?;
    let groups: Vec<String> = s.query_map(params![key.user_id, key.exchange_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let mut seconds = BTreeSet::new();
    for group in groups {
        let mut s = c.prepare_cached("SELECT DISTINCT transacted_at FROM import_rows WHERE user_id = ?1 AND exchange_id = ?2 AND created_at = ?3")?;
        let times: Vec<String> = s.query_map(params![key.user_id, key.exchange_id, group], |r| r.get(0))?.collect::<Result<_, _>>()?;
        // Only this group's rows: held_rows reads every row of the second.
        let ids: HashSet<String> = {
            let mut s = c.prepare_cached("SELECT row_id FROM import_rows WHERE user_id = ?1 AND exchange_id = ?2 AND created_at = ?3")?;
            let ids = s.query_map(params![key.user_id, key.exchange_id, group], |r| r.get(0))?.collect::<Result<_, _>>()?;
            ids
        };
        let mut rows = vec![];
        for at in times {
            let at = time(&at)?;
            seconds.insert(at);
            rows.extend(file::held_rows(c, key, at)?.into_iter().filter(|r| ids.contains(&r.row_id)));
        }
        if let Some(span) = period_of(&rows, &catalogue) { add(c, key, EXPORT, span, now)?; }
    }
    let mut s = c.prepare(&format!("SELECT DISTINCT transacted_at FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND {}", provenance_less()))?;
    let restored: Vec<String> = s.query_map(params![key.user_id, key.exchange_id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for at in restored { seconds.insert(second(time(&at)?)); }
    flush(c, key, seconds, now)?;
    Ok(true)
}

// ---- allocation (r9–r13) ------------------------------------------------------------------------------------------

/// What a store does with each covered entry of its batch on an HF-enabled history: `true` a row already held, `false`
/// a new row (no inherited duplicate heuristic). Entries absent from the map keep the inherited rules.
pub(crate) struct Allocation(pub(crate) HashMap<usize, bool>);

/// The legacy key: the values as the writer stores them (their doubles).
type Legacy = (i64, Option<String>, f64, Option<String>, Option<f64>, Option<(Option<String>, f64)>);
fn legacy_of(e: &Entry) -> Legacy {
    let f = |d: &BigDec| d.to_f();
    (e.entry_type, e.base_currency.clone(), f(&e.base_amount), e.quote_currency.clone(), e.quote_amount.as_ref().map(f),
     fee(e.fee_currency.clone(), e.fee_amount.clone()).map(|(c, a)| (c, f(&a))))
}
fn legacy_stored(r: &Restored) -> Legacy {
    (r.2, r.3.clone(), r.4, r.5.clone(), r.6, r.8.filter(|a| *a != 0.0).map(|a| (r.7.clone(), a)))
}
/// The stored double's own decimal (its shortest reading).
fn decimal(f: f64) -> Option<BigDec> { BigDec::parse(&format!("{f}")).ok() }

fn amounts_of(e: &Entry) -> Value {
    json!({ "base": e.base_amount.to_s_f(), "quote": e.quote_amount.as_ref().map(BigDec::to_s_f), "fee": e.fee_amount.as_ref().map(BigDec::to_s_f) })
}

/// The `amounts` a Deltabadger CSV row carries into the ledger (r2.3): its exact decimal strings.
pub fn with_amounts(mut e: Entry) -> Entry {
    let mut raw = if e.raw.value.is_object() { e.raw.value.clone() } else { json!({}) };
    raw["amounts"] = amounts_of(&e);
    e.raw = Raw::from_value(raw);
    e
}

fn api_entry(e: &Entry) -> bool { e.raw.value.get("createdAt").is_some() }
fn file_entry(e: &Entry) -> bool { e.raw.value["source"] == "kucoin_csv" }

/// A KuCoin API fill already stored (visible or withheld) under `tx_id`: compared by its raw side, size, funds, fee,
/// feeCurrency and createdAt second; any difference is a conflict (the page fails, nothing written). A row without the
/// API's raw (a restore keeping the id) is the trade: a duplicate.
pub fn conflict(c: &Connection, key: &Key, e: &Entry) -> Result<Option<Result<(), String>>, SyncError> {
    let Some(tx_id) = e.tx_id.as_deref() else { return Ok(None) };
    let visible: Option<Option<String>> = c.query_row("SELECT raw_data FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3 LIMIT 1",
                                                      params![key.user_id, key.exchange_id, tx_id], |r| r.get(0)).optional()?;
    let stored = match visible {
        Some(raw) => raw,
        None => match c.query_row("SELECT entry FROM kucoin_withheld_fills WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3",
                                  params![key.user_id, key.exchange_id, tx_id], |r| r.get::<_, String>(0)).optional()? {
            Some(entry) => Some(entry),
            None => return Ok(None),
        },
    };
    let raw: Value = stored.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    if raw.get("createdAt").is_none() || !api_entry(e) { return Ok(Some(Ok(()))); }
    Ok(Some(if same_fill(&raw, &e.raw.value) { Ok(()) } else { Err(format!("{CONFLICT}{tx_id} differs from the one already stored")) }))
}

/// Two API reads of one fill agree: side, size, funds, fee, feeCurrency (decimals by value) and the createdAt second.
fn same_fill(a: &Value, b: &Value) -> bool {
    let ms = |v: &Value| v["createdAt"].as_i64().map(|m| m.div_euclid(1000));
    let text = |v: &Value, k: &str| match &v[k] { Value::String(s) => BigDec::parse(s).map(|d| d.to_s_f()).unwrap_or_else(|_| s.clone()), other => other.to_string() };
    ["side", "size", "funds", "fee", "feeCurrency"].iter().all(|k| text(a, k) == text(b, k)) && ms(a) == ms(b)
}

impl Allocation {
    /// The allocation of `batch[range]`, or None on a classic-only history or another venue. Phase 1 (ids already
    /// stored) for every source; for the provenance-less class (restores, the user's rows) phases 2 and 3, each exact
    /// first and then legacy (r13), sharing one set of reserved rows. Adoptions and enrichments are written here.
    pub(crate) fn new(c: &Connection, key: &Key, batch: &[Entry], range: std::ops::Range<usize>, now: DateTime<Utc>) -> Result<Option<Self>, SyncError> {
        if key.exchange_type != crate::venue::kucoin::KUCOIN || !enabled(c, key.user_id, key.exchange_id)? { return Ok(None); }
        let mut out = HashMap::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut restores: BTreeMap<DateTime<Utc>, Vec<usize>> = BTreeMap::new();
        let present = |e: &Entry| e.tx_id.as_deref().filter(|t| !crate::ruby::blank(t)).map(str::to_string);
        for i in range {
            let e = &batch[i];
            if !FILL_TYPES.contains(&e.entry_type) { continue; }
            let id = present(e);
            if let Some(id) = &id {
                // The same id twice in one batch: one fill, unless the API read it two ways (the page fails).
                if let Some(&first) = seen.get(id) {
                    if api_entry(e) && api_entry(&batch[first]) && !same_fill(&batch[first].raw.value, &e.raw.value) {
                        return Err(SyncError(format!("{CONFLICT}{id} read two ways in one page")));
                    }
                    out.insert(i, true);
                    continue;
                }
                seen.insert(id.clone(), i);
            }
            if api_entry(e) || file_entry(e) {
                let held = match &id { Some(_) => match conflict(c, key, e)? { Some(Ok(())) => true, Some(Err(why)) => return Err(SyncError(why)), None => false }, None => false };
                out.insert(i, held);
                continue;
            }
            let Some(at) = e.transacted_at else { continue };
            restores.entry(second(at)).or_default().push(i);
        }
        for (t, entries) in restores { allocate_second(c, key, batch, t, &entries, &mut out, now)?; }
        Ok(Some(Self(out)))
    }
}

/// r9–r13 at one second, for provenance-less entries.
fn allocate_second(c: &Connection, key: &Key, batch: &[Entry], t: DateTime<Utc>, entries: &[usize], out: &mut HashMap<usize, bool>, now: DateTime<Utc>) -> Result<(), SyncError> {
    let stored = restored_rows(c, key, t)?;
    let mut reserved = vec![false; stored.len()];
    let present = |e: &Entry| e.tx_id.as_deref().filter(|t| !crate::ruby::blank(t)).map(str::to_string);
    let known = |id: &str| -> Result<bool, SyncError> {
        Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3) \
                        OR EXISTS(SELECT 1 FROM kucoin_withheld_fills WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3)", params![key.user_id, key.exchange_id, id], |r| r.get(0))?)
    };
    // Phase 1: ids already stored.
    let mut new_ids = vec![];
    let mut idless = vec![];
    for &i in entries {
        match present(&batch[i]) {
            Some(id) if known(&id)? => {
                if let Some(j) = stored.iter().position(|r| r.1.as_deref() == Some(id.as_str())) { reserved[j] = true; }
                out.insert(i, true);
            }
            Some(_) => new_ids.push(i),
            None => idless.push(i),
        }
    }
    // Deterministic, whatever the order the batch holds them in.
    let order = |v: &mut Vec<usize>| v.sort_by(|a, b| (FillKey::of(&batch[*a]), &batch[*a].tx_id).cmp(&(FillKey::of(&batch[*b]), &batch[*b].tx_id)));
    order(&mut new_ids);
    order(&mut idless);
    let exact: Vec<Option<FillKey>> = stored.iter().map(restored_key).collect();
    let adopt = |i: usize, j: usize, adopt_id: bool, flag: bool| -> Result<(), SyncError> {
        let e = &batch[i];
        let (id, row) = (&stored[j].1, stored[j].0);
        if adopt_id && id.is_none() { c.execute("UPDATE account_transactions SET tx_id = ?1, updated_at = ?2 WHERE id = ?3", params![present(e), sql_time(now), row])?; }
        if exact[j].is_none() {
            c.execute("UPDATE account_transactions SET raw_data = json_set(CASE WHEN json_valid(raw_data) AND json_type(raw_data) = 'object' THEN raw_data ELSE '{}' END, \
                       '$.amounts', json(?1), '$.amounts_inexact', json(?2)), updated_at = ?3 WHERE id = ?4", params![amounts_of(e).to_string(), if flag { "true" } else { "false" }, sql_time(now), row])?;
        }
        Ok(())
    };
    for (incoming, idless_rows_only) in [(new_ids, true), (idless, false)] {
        let mut left = vec![];
        // (a) exact: rows that carry `amounts`.
        for i in incoming {
            let k = FillKey::of(&batch[i]);
            match (0..stored.len()).find(|&j| !reserved[j] && (!idless_rows_only || stored[j].1.is_none()) && exact[j].as_ref() == Some(&k)) {
                Some(j) => { reserved[j] = true; out.insert(i, true); adopt(i, j, idless_rows_only, false)?; }
                None => left.push(i),
            }
        }
        // (b) legacy: rows without them, by the values as stored; an incoming value the double does not hold flags.
        for i in left {
            let e = &batch[i];
            let l = legacy_of(e);
            match (0..stored.len()).find(|&j| !reserved[j] && (!idless_rows_only || stored[j].1.is_none()) && exact[j].is_none() && legacy_stored(&stored[j]) == l) {
                Some(j) => {
                    reserved[j] = true;
                    out.insert(i, true);
                    let r = &stored[j];
                    let held = |f: Option<f64>, d: Option<&BigDec>| match (f, d) { (Some(f), Some(d)) => decimal(f).as_ref() == Some(d), (None, None) => true, (Some(f), None) => f == 0.0, (None, Some(d)) => d.is_zero() };
                    let inexact = !held(Some(r.4), Some(&e.base_amount)) || !held(r.6, e.quote_amount.as_ref()) || !held(r.8.filter(|f| *f != 0.0), e.fee_amount.as_ref().filter(|d| !d.is_zero()));
                    adopt(i, j, idless_rows_only, inexact)?;
                }
                None => { out.insert(i, false); }
            }
        }
    }
    Ok(())
}

// ---- the gap -----------------------------------------------------------------------------------------------------

/// `spans` merged into disjoint, sorted spans.
fn merge(mut spans: Vec<Span>) -> Vec<Span> {
    spans.sort();
    let mut out: Vec<Span> = vec![];
    for (a, b) in spans {
        if b <= a { continue; }
        match out.last_mut() { Some(last) if a <= last.1 => last.1 = last.1.max(b), _ => out.push((a, b)) }
    }
    out
}

/// `from` less `covered`.
fn subtract(from: Span, covered: &[Span]) -> Vec<Span> {
    let mut out = vec![];
    let mut at = from.0;
    for (a, b) in merge(covered.to_vec()) {
        if b <= at || a >= from.1 { continue; }
        if a > at { out.push((at, a)); }
        at = at.max(b);
    }
    if at < from.1 { out.push((at, from.1)); }
    out
}

/// What no certified unit or export covers within the horizon (the history's first row or unit to the last unit's end),
/// with every ambiguous second.
pub fn uncovered(c: &Connection, key: &Key) -> Result<Vec<Span>, SyncError> {
    let attempts: Vec<Span> = spans(c, key, HF_ATTEMPT)?.into_iter().chain(spans(c, key, HF_ATTEMPT_OPEN)?).map(|(_, s)| s).collect();
    let mut gap: Vec<Span> = spans(c, key, AMBIGUOUS)?.into_iter().map(|(_, s)| s).collect();
    if let Some(end) = attempts.iter().map(|s| s.1).max() {
        let first = |sql: &str| -> Result<Option<DateTime<Utc>>, SyncError> {
            let t: Option<String> = c.query_row(sql, params![key.user_id, key.exchange_id], |r| r.get(0))?;
            t.map(|t| time(&t).map(second)).transpose()
        };
        let starts = [first("SELECT MIN(transacted_at) FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2")?,
                      first("SELECT MIN(transacted_at) FROM import_rows WHERE user_id = ?1 AND exchange_id = ?2")?, attempts.iter().map(|s| s.0).min()];
        if let Some(start) = starts.into_iter().flatten().min() {
            let covered: Vec<Span> = spans(c, key, HF_API)?.into_iter().chain(spans(c, key, EXPORT)?).map(|(_, s)| s).collect();
            gap.extend(subtract((start, end), &covered));
        }
    }
    Ok(merge(gap))
}

/// The gap row's entry for this history, rebuilt from coverage: the reading key, the credential the writing sync is
/// fenced on (a file import, fenced on none, keeps the one stored for that key), and the uncovered spans.
pub fn refresh_gap(c: &FencedTransaction<'_>, key: &Key, now: DateTime<Utc>) -> Result<(), SyncError> {
    let uncovered = uncovered(c, key)?;
    let reading = crate::tracker::import::run::reading(c, key.user_id)?.into_iter().find(|k| k.exchange_id == key.exchange_id);
    let raw: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [gap_key(key.user_id)], |r| r.get(0)).optional()?.flatten();
    let mut row = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let id = key.exchange_id.to_string();
    let credential = match (&reading, c.producer_version()) {
        (Some(k), Some(v)) if v.id == k.id => Some(v.cache_stamp()),
        (Some(k), _) if row[&id]["key"] == k.id => Some(row[&id]["credential"].clone()),
        _ => None,
    };
    match (reading, credential) {
        // The row stays readable (gaps::MAX_ROW): the first spans, and how many there are.
        (Some(k), Some(credential)) => row[&id] = json!({ "key": k.id, "credential": credential, "spans": uncovered.len(),
                                                          "uncovered": uncovered.iter().take(MAX_SPANS).map(|(a, b)| json!([sql_time(*a), sql_time(*b)])).collect::<Vec<_>>() }),
        (None, _) => { row.as_object_mut().expect("an object").remove(&id); }
        // A file import before any sync wrote the row: the next sync writes it.
        (Some(_), None) => return Ok(()),
    }
    c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
              params![gap_key(key.user_id), row.to_string(), sql_time(now)])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(s: &str) -> DateTime<Utc> { format!("2026-01-01T{s}Z").parse().unwrap() }
    #[test]
    fn subtract_and_merge_are_interval_arithmetic() {
        assert_eq!(merge(vec![(t("00:00:05"), t("00:00:07")), (t("00:00:01"), t("00:00:03")), (t("00:00:03"), t("00:00:04"))]),
                   vec![(t("00:00:01"), t("00:00:04")), (t("00:00:05"), t("00:00:07"))]);
        assert_eq!(subtract((t("00:00:00"), t("00:00:10")), &[(t("00:00:02"), t("00:00:04")), (t("00:00:08"), t("00:00:12"))]),
                   vec![(t("00:00:00"), t("00:00:02")), (t("00:00:04"), t("00:00:08"))]);
    }
    #[test]
    fn a_shared_fee_and_an_aggregate_have_one_effect() {
        let k = |kind, base: &str, b: &str, q: &str, fee: Option<(&str, &str)>| FillKey { kind, base: Some(base.into()), base_amount: BigDec::parse(b).unwrap(), quote: Some("USDT".into()),
            quote_amount: Some(BigDec::parse(q).unwrap()), fee: fee.map(|(c, a)| (Some(c.to_string()), BigDec::parse(a).unwrap())) };
        let two = [k(BUY, "BTC", "1", "100", Some(("KCS", "0.1"))), k(BUY, "BTC", "2", "200", None)];
        let one = [k(BUY, "BTC", "3", "300", Some(("KCS", "0.1")))];
        assert_eq!(effect(&two), effect(&one));
        assert_ne!(effect(&two), effect(&[k(BUY, "BTC", "3", "300", None)]));
    }
}
