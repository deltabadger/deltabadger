//! D-KUCOIN-HF-LEDGER (plan HF-3 §3): one trade, one row on a KuCoin history whose account has HF trading. Everything
//! available is imported, from the API and from KuCoin's export, and each trade is counted once by an exact per-second
//! rule; what nothing covers is signalled on the existing "Import history" line. Classic-only histories (no
//! `hf_enabled` marker) never reach this module. A UTA history (D-KUCOIN-UTA-LEDGER) is managed the same way.
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
use crate::v2::BigDec;
use crate::venue_ids;
use crate::venue::kucoin_account::AccountMode;
use crate::tracker::import::kucoin::{self as file, Catalogue, Derived, SourceRow, Stored, API_ROW};
use chrono::{DateTime, Duration, Timelike, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub const ENABLED: &str = "hf_enabled";
pub const EXPORT: &str = "export";
pub const HF_API: &str = "hf_api";
pub const HF_ATTEMPT: &str = "hf_attempt";
pub const HF_ATTEMPT_OPEN: &str = "hf_attempt_open";
pub const UTA_API: &str = "uta_api";
pub const UTA_ATTEMPT: &str = "uta_attempt";
pub const UTA_ATTEMPT_OPEN: &str = "uta_attempt_open";
/// A UTA window whose walk ended on an explicit empty page (r4.1): read, whatever it certified. The backfill reads what
/// no such span covers.
pub const UTA_READ: &str = "uta_read";
/// A mode episode (UTA r2.1): `mode:<Mode>`, from the first detection of that mode after any other to the latest.
pub const MODE: &str = "mode:";
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
fn provenance_less(scope: &crate::venue_ids::Scope) -> String { format!("entry_type IN ({BUY}, {SELL}, {FEE}, {ADJUSTMENT}) AND NOT ({API_ROW}) AND NOT ({})", file::derived_sql(scope)) }

pub type Span = (DateTime<Utc>, DateTime<Utc>);
pub fn second(t: DateTime<Utc>) -> DateTime<Utc> { t.with_nanosecond(0).unwrap_or(t) }
fn time(s: &str) -> Result<DateTime<Utc>, SyncError> { parse_time(s).map_err(|e| SyncError(format!("kucoin_coverage time {s}: {e:?}"))) }

/// Whether the account (#710 slice 5: coverage is its account's) is an HF-enabled KuCoin history.
/// An install Rust never migrated has no table, so no history is enabled (a pure classic account reads as before).
pub fn enabled(c: &Connection, account: &venue_ids::Scope) -> Result<bool, SyncError> {
    Ok(migrated(c)? && c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM kucoin_coverage WHERE account_id = {} AND source = ?1)", account.account_sql()), [ENABLED], |r| r.get(0))?)
}

/// Whether any history is HF-enabled (the handback refuses then). An install Rust never migrated has no table, so none.
pub fn any_enabled(c: &Connection) -> Result<bool, SyncError> {
    Ok(migrated(c)? && c.query_row("SELECT EXISTS(SELECT 1 FROM kucoin_coverage WHERE source = ?1)", [ENABLED], |r| r.get(0))?)
}

fn migrated(c: &Connection) -> Result<bool, SyncError> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'kucoin_coverage')", [], |r| r.get(0))?)
}

/// A mode's episode source; the names stay whatever the enum is called.
pub fn mode_source(mode: AccountMode) -> &'static str {
    match mode { AccountMode::ClassicLf => "mode:ClassicLf", AccountMode::ClassicHf => "mode:ClassicHf", AccountMode::Uta => "mode:Uta" }
}

/// The latest episode: (source, span).
fn current_episode(c: &Connection, key: &Key) -> Result<Option<(String, Span)>, SyncError> {
    Ok(spans(c, key, MODE)?.into_iter().max_by_key(|(_, (a, b))| (*b, *a)))
}

/// UTA r2.1: the detected `mode` extends its episode to `now`, or a switch opens a new one at `now`. A history with no
/// episode yet gets one first over its HF units, if it has any (HF-3 ran before episodes existed); a history first seen
/// as ClassicHf starts its episode at its first unit's start, so HF-3's certification of that unit is unchanged; any
/// other first episode starts at its first detection (r1.5: nothing says what the account was before it).
pub fn observe_mode(c: &Connection, key: &Key, mode: AccountMode, now: DateTime<Utc>) -> Result<(), SyncError> {
    let now = second(now);
    if current_episode(c, key)?.is_none() {
        let units: Vec<Span> = spans(c, key, HF_ATTEMPT)?.into_iter().chain(spans(c, key, HF_ATTEMPT_OPEN)?).map(|(_, s)| s).collect();
        if let (Some(a), Some(b)) = (units.iter().map(|s| s.0).min(), units.iter().map(|s| s.1).max()) { add(c, key, mode_source(AccountMode::ClassicHf), (a, b), now)?; }
    }
    match current_episode(c, key)? {
        Some((source, (a, b))) if source == mode_source(mode) => {
            c.execute(&format!("UPDATE kucoin_coverage SET end_at = ?1, updated_at = ?1 WHERE account_id = {} AND source = ?2 AND start_at = ?3 AND end_at = ?4", key.scope.account_sql()),
                      params![sql_time(now.max(b)), source, sql_time(a), sql_time(b)])?;
        }
        Some(_) => {
            add(c, key, mode_source(mode), (now, now), now)?;
            // ClassicLf reopened after another mode: the rows placed before it are held once.
            if mode == AccountMode::ClassicLf { hold_switched(c, key, now)?; }
        }
        None => {
            let start = if mode == AccountMode::ClassicHf { now - super::kucoin_hf_ledger::RETENTION + super::kucoin_hf_ledger::MARGIN } else { now };
            add(c, key, mode_source(mode), (start, now), now)?;
        }
    }
    // On ClassicHf or UTA every record-less row is a classic one (each HF or UTA placement has its record), so each
    // observation holds them, not only the switch: a row materialized later (`resolve-placement --placed`) or placed in an
    // LF interval no observation recorded is held too.
    if mode != AccountMode::ClassicLf { hold_switched(c, key, now)?; }
    Ok(())
}

/// UTA r2.2, r12.1: a switch holds every unresolved order row of this history that has no record. Each gets a
/// `family: "Classic"` record (its symbol, side, type and exact executed totals), so it is read on the classic route as
/// before but protected as an HF or UTA row is: an unread state owes, nothing is written off, and only a terminal read or
/// `reconcile-order` settles it. A record is never removed. Adopted rows whose route is unproven are held already.
fn hold_switched(c: &Connection, key: &Key, now: DateTime<Utc>) -> Result<(), SyncError> {
    let record = crate::venue::kucoin::ORDER_RECORD;
    let origin = crate::engine::reconcile::ORIGIN;
    // #710: this history's account's orders, their venue ids wherever it keeps them.
    let (o, mine) = (venue_ids::order_id(c, "t")?, key.scope.orders("t"));
    let mut s = c.prepare(&format!(
        "SELECT t.id, (SELECT CASE WHEN count(*) = 1 THEN max(k.ticker) END FROM tickers k WHERE k.exchange_id = t.exchange_id AND k.base_asset_id = t.base_asset_id \
         AND k.quote_asset_id = t.quote_asset_id), coalesce(t.side, 0) = 1, coalesce(t.order_type, 0) = 1 \
         FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE b.user_id = ?1 AND t.exchange_id = ?2 AND t.status = 0 AND t.external_status IN (0, 1) \
         AND {o} IS NOT NULL AND {o} NOT LIKE 'imported_%' AND {mine} AND NOT EXISTS (SELECT 1 FROM app_configs k WHERE k.key = '{record}' || t.id) \
         AND NOT EXISTS (SELECT 1 FROM app_configs k WHERE k.key = '{origin}' || t.id AND json_extract(k.value, '$.origin') = 'unproven') ORDER BY t.id"))?;
    let rows: Vec<(i64, Option<String>, bool, bool)> =
        s.query_map(params![key.user_id, key.exchange_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<_, _>>()?;
    drop(s);
    for (id, symbol, sell, limit) in rows {
        // The row's executed totals: the floor no answer may go below (`advance_totals`).
        let row = crate::figures::fill::stored_order(c, id)?;
        let exact = |v: Option<crate::v2::BigDec>| v.unwrap_or_else(crate::v2::BigDec::zero).to_s_f();
        let value = serde_json::json!({ "family": "Classic", "symbol": symbol, "sell": sell, "limit": limit, "deal_size": exact(row.amount_exec), "deal_funds": exact(row.quote_amount_exec) });
        crate::app_config::set_plain(c, &format!("{record}{id}"), &value.to_string(), now).map_err(SyncError)?;
    }
    Ok(())
}

/// The families whose units run now (r1.8b): every one seen on this history, while its latest episode ended within
/// KuCoin's retention (an older one has nothing left to read). The current mode's first.
pub fn seen_modes(c: &Connection, key: &Key, now: DateTime<Utc>) -> Result<Vec<AccountMode>, SyncError> {
    let episodes = spans(c, key, MODE)?;
    let mut out: Vec<(DateTime<Utc>, AccountMode)> = [AccountMode::ClassicHf, AccountMode::Uta].into_iter().filter_map(|m| {
        episodes.iter().filter(|(s, _)| s == mode_source(m)).map(|(_, (_, b))| *b).max().filter(|b| now - super::kucoin_hf_ledger::RETENTION < *b).map(|b| (b, m))
    }).collect();
    out.sort_by_key(|(end, _)| std::cmp::Reverse(*end));
    Ok(out.into_iter().map(|(_, m)| m).collect())
}

/// What a family's complete unit `[s, e)` certifies (r2.1): only while `mode` is the current episode's, and only from
/// that episode's start. An inactive family certifies nothing, even when it read empty.
pub fn certifiable(c: &Connection, key: &Key, mode: AccountMode, (s, e): Span) -> Result<Option<Span>, SyncError> {
    Ok(current_episode(c, key)?.filter(|(source, _)| source == mode_source(mode)).map(|(_, (a, _))| (s.max(a), e)).filter(|(a, b)| a < b))
}

/// The spans of `source` (a `missing:` or `mode:` prefix matches every one), oldest first.
pub fn spans(c: &Connection, key: &Key, source: &str) -> Result<Vec<(String, Span)>, SyncError> {
    let (op, pattern) = if source == MISSING || source == MODE { ("LIKE", format!("{source}%")) } else { ("=", source.to_string()) };
    let mut s = c.prepare_cached(&format!("SELECT source, start_at, end_at FROM kucoin_coverage WHERE account_id = {} AND source {op} ?1 ORDER BY start_at, end_at", key.scope.account_sql()))?;
    let rows: Vec<(String, String, String)> = s.query_map(params![pattern], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    rows.into_iter().map(|(source, a, b)| Ok((source, (time(&a)?, time(&b)?)))).collect()
}

/// One coverage row of the key's account, unless the same one is held (its number 1's row made first where it has none).
pub fn add(c: &Connection, key: &Key, source: &str, (start, end): Span, now: DateTime<Utc>) -> Result<(), SyncError> {
    let (start, end) = (sql_time(start), sql_time(end));
    let account = key.scope.account_sql();
    let held: bool = c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM kucoin_coverage WHERE account_id = {account} AND source = ?1 AND start_at = ?2 AND end_at = ?3)"),
                                 params![source, start, end], |r| r.get(0))?;
    if !held {
        if key.scope.account.is_none() { crate::identity::touch(c, key.user_id, key.exchange_id, now)?; }
        c.execute(&format!("INSERT INTO kucoin_coverage (account_id, source, start_at, end_at, updated_at) VALUES ({account}, ?1, ?2, ?3, ?4)"),
                  params![source, start, end, sql_time(now)])?;
    }
    Ok(())
}

pub fn remove(c: &Connection, key: &Key, source: &str) -> Result<(), SyncError> {
    c.execute(&format!("DELETE FROM kucoin_coverage WHERE account_id = {} AND source = ?1", key.scope.account_sql()), params![source])?;
    Ok(())
}

// ---- exact fill keys ---------------------------------------------------------------------------------------------

/// A fill as the rule compares it: exact decimals, and a zero or absent fee read as no fee.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct FillKey { kind: i64, base: Option<String>, base_amount: BigDec, quote: Option<String>, quote_amount: Option<BigDec>, fee: Option<(Option<String>, BigDec)> }

fn fee(currency: Option<String>, amount: Option<BigDec>) -> Option<(Option<String>, BigDec)> { amount.filter(|a| !a.is_zero()).map(|a| (currency, a)) }

impl FillKey {
    fn of(e: &Entry) -> Self {
        Self { kind: e.entry_type, base: e.base_currency.as_deref().map(str::to_string), base_amount: e.base_amount.clone(), quote: e.quote_currency.as_deref().map(str::to_string), quote_amount: e.quote_amount.clone(),
               fee: fee(e.fee_currency.as_deref().map(str::to_string), e.fee_amount.clone()) }
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
    let at_ = "account_transactions";
    let mut s = c.prepare_cached(&format!("SELECT id, {}, entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, fee_amount, raw_data \
                                           FROM account_transactions {} WHERE user_id = ?1 AND exchange_id = ?2 AND {} AND transacted_at >= ?3 AND transacted_at < ?4 AND {} ORDER BY id",
                                          key.scope.row_id(at_), super::ledger::BY_TIME, key.scope.rows(at_), provenance_less(&key.scope)))?;
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
    let at_ = "account_transactions";
    let mut s = c.prepare_cached(&format!("SELECT id, COALESCE({}, ''), raw_data FROM account_transactions {} WHERE user_id = ?1 AND exchange_id = ?2 AND {} AND transacted_at >= ?3 \
                                           AND transacted_at < ?4 AND entry_type IN ({BUY}, {SELL}) AND {API_ROW} ORDER BY id", key.scope.row_id(at_), super::ledger::BY_TIME, key.scope.rows(at_)))?;
    let rows = s.query_map(params![key.user_id, key.exchange_id, from, to], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().map(|(id, tx, raw)| (id, tx, raw.and_then(|r| serde_json::from_str(&r).ok()).unwrap_or(Value::Null))).collect())
}

fn withheld_rows(c: &Connection, key: &Key, t: DateTime<Utc>) -> Result<Vec<Value>, SyncError> {
    let (from, to) = at_second(t);
    let mut s = c.prepare_cached(&format!("SELECT entry FROM kucoin_withheld_fills WHERE account_id = {} AND transacted_at >= ?1 AND transacted_at < ?2 ORDER BY tx_id", key.scope.account_sql()))?;
    let rows = s.query_map(params![from, to], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().filter_map(|r| serde_json::from_str(&r).ok()).collect())
}

/// The visible API fills at a second holding restored entries move to withheld: the restore is the user's record there.
/// Each keeps its venue id under its account (#710), and its membership goes with its row.
fn withhold(c: &Connection, key: &Key, t: DateTime<Utc>, now: DateTime<Utc>) -> Result<(), SyncError> {
    if restored_rows(c, key, t)?.is_empty() { return Ok(()); }
    for (id, tx_id, raw) in api_rows(c, key, t)? {
        let account = match key.scope.account { Some(account) => account, None => crate::identity::ensure_account(c, key.user_id, key.exchange_id, now)? };
        let at: String = c.query_row("SELECT transacted_at FROM account_transactions WHERE id = ?1", [id], |r| r.get(0))?;
        c.execute("INSERT INTO kucoin_withheld_fills (account_id, tx_id, transacted_at, entry, created_at) VALUES (?1, ?2, ?3, ?4, ?5) \
                   ON CONFLICT (account_id, tx_id) DO NOTHING", params![account, tx_id, at, raw.to_string(), sql_time(now)])?;
        // PLAN-710-confirm rule P3: only the owner's rows are unlinked.
        c.execute("UPDATE account_transactions SET linked_transaction_id = NULL WHERE linked_transaction_id = ?1 AND user_id = ?2", params![id, key.user_id])?;
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
    let mut writer = super::ledger::FileWriter::new(c, key, "kucoin")?;
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
                // UTA r1.6: an unknown export type in a currency an API fill of this second trades may be that fill (a
                // UTA export's types are unknown): it is withheld, the API wins, and the second is flagged.
                let traded: HashSet<&Option<String>> = j.reps.iter().filter(|r| r.api).filter_map(|r| r.key.as_ref()).chain(&j.withheld).flat_map(|k| [&k.base, &k.quote]).collect();
                let unknown = |d: &Derived| d.entry.raw.value["reason"] == "unmapped_type" && traded.contains(&d.entry.base_currency.as_deref().map(str::to_string));
                ambiguous |= j.other.iter().any(unknown);
                derived.extend(j.other.iter().filter(|d| !unknown(d)).filter(|d| d.entry.raw.value["class"].as_str().and_then(|class| cutoffs.get(class)).is_none_or(|cut| t < cut)).cloned());
            }
            let s = file::rewrite_second(c, key, *t, derived, &mut writer, now)?;
            (out.imported, out.duplicates, out.skipped, out.generic) = (out.imported + s.imported, out.duplicates + s.duplicates, out.skipped + s.skipped, out.generic + s.generic);
        }
        c.execute(&format!("DELETE FROM kucoin_coverage WHERE account_id = {} AND source = ?1 AND start_at = ?2", key.scope.account_sql()), params![AMBIGUOUS, sql_time(*t)])?;
        if ambiguous { add(c, key, AMBIGUOUS, (*t, *t + Duration::seconds(1)), now)?; }
    }
    Ok(out)
}

/// What every writer of an HF-enabled history calls before its transaction commits (r11): the rule over the seconds it
/// touched, then the gap row. A classic-only history is left as it is.
pub fn flush(c: &FencedTransaction<'_>, key: &Key, seconds: BTreeSet<DateTime<Utc>>, now: DateTime<Utc>) -> Result<Stored, SyncError> {
    if key.exchange_type != crate::venue::kucoin::KUCOIN || !enabled(c, &key.scope)? { return Ok(Stored::default()); }
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
    if enabled(c, &key.scope)? { return Ok(false); }
    add(c, key, ENABLED, (second(now), second(now)), now)?;
    let catalogue = Catalogue::load(c, key.exchange_id)?;
    // #710: the account's own imports.
    let account = key.scope.account_sql();
    let groups: Vec<String> = {
        let mut s = c.prepare(&format!("SELECT DISTINCT created_at FROM import_rows WHERE account_id = {account} ORDER BY created_at"))?;
        let groups = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        groups
    };
    let mut seconds = BTreeSet::new();
    for group in groups {
        let mut s = c.prepare_cached(&format!("SELECT DISTINCT transacted_at FROM import_rows WHERE account_id = {account} AND created_at = ?1"))?;
        let times: Vec<String> = s.query_map(params![group], |r| r.get(0))?.collect::<Result<_, _>>()?;
        // Only this group's rows: held_rows reads every row of the second.
        let ids: HashSet<String> = {
            let mut s = c.prepare_cached(&format!("SELECT row_id FROM import_rows WHERE account_id = {account} AND created_at = ?1"))?;
            let ids = s.query_map(params![group], |r| r.get(0))?.collect::<Result<_, _>>()?;
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
    let mut s = c.prepare(&format!("SELECT DISTINCT transacted_at FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND {} AND {}",
                                   key.scope.rows("account_transactions"), provenance_less(&key.scope)))?;
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
    (e.entry_type, e.base_currency.as_deref().map(str::to_string), f(&e.base_amount), e.quote_currency.as_deref().map(str::to_string), e.quote_amount.as_ref().map(f),
     fee(e.fee_currency.as_deref().map(str::to_string), e.fee_amount.clone()).map(|(c, a)| (c, f(&a))))
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
    let visible: Option<Option<String>> = c.query_row(&format!("SELECT raw_data FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND {} LIMIT 1",
                                                               key.scope.id_is("account_transactions", "?3")),
                                                      params![key.user_id, key.exchange_id, tx_id], |r| r.get(0)).optional()?;
    let stored = match visible {
        Some(raw) => raw,
        None => match c.query_row(&format!("SELECT entry FROM kucoin_withheld_fills WHERE account_id = {} AND tx_id = ?1", key.scope.account_sql()),
                                  [tx_id], |r| r.get::<_, String>(0)).optional()? {
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
        if key.exchange_type != crate::venue::kucoin::KUCOIN || !enabled(c, &key.scope)? { return Ok(None); }
        let mut out = HashMap::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut restores: BTreeMap<DateTime<Utc>, Vec<usize>> = BTreeMap::new();
        let present = |e: &Entry| e.tx_id.as_deref().filter(|t| !crate::v2::blank(t)).map(str::to_string);
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
    let present = |e: &Entry| e.tx_id.as_deref().filter(|t| !crate::v2::blank(t)).map(str::to_string);
    let known = |id: &str| -> Result<bool, SyncError> {
        Ok(venue_ids::known(c, &key.scope, id)?
           || c.query_row(&format!("SELECT EXISTS(SELECT 1 FROM kucoin_withheld_fills WHERE account_id = {} AND tx_id = ?1)", key.scope.account_sql()), [id], |r| r.get(0))?)
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
        if adopt_id && id.is_none() {
            venue_ids::set_id(c, &key.scope, row, present(e).as_deref(), venue_ids::Write::Adopt)?;
            c.execute("UPDATE account_transactions SET updated_at = ?1 WHERE id = ?2", params![sql_time(now), row])?;
        }
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
pub(super) fn subtract(from: Span, covered: &[Span]) -> Vec<Span> {
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
    let mut gap: Vec<Span> = spans(c, key, AMBIGUOUS)?.into_iter().map(|(_, s)| s).collect();
    gap.extend(missing(c, key)?);
    Ok(merge(gap))
}

/// What no certified unit or export covers within the horizon, without the ambiguous seconds: the periods an export
/// can close (D-GAP-EVIDENCE-ONLY), which the tracker's Import history line shows.
pub fn missing(c: &Connection, key: &Key) -> Result<Vec<Span>, SyncError> {
    let mut attempts: Vec<Span> = vec![];
    for source in [HF_ATTEMPT, HF_ATTEMPT_OPEN, UTA_ATTEMPT, UTA_ATTEMPT_OPEN] { attempts.extend(spans(c, key, source)?.into_iter().map(|(_, s)| s)); }
    let Some(end) = attempts.iter().map(|s| s.1).max() else { return Ok(vec![]) };
    let first = |sql: &str| -> Result<Option<DateTime<Utc>>, SyncError> {
        let t: Option<String> = c.query_row(sql, params![key.user_id, key.exchange_id], |r| r.get(0))?;
        t.map(|t| time(&t).map(second)).transpose()
    };
    // #710: the account's own history, its imports included.
    let imported: Option<String> = c.query_row(&format!("SELECT MIN(transacted_at) FROM import_rows WHERE account_id = {}", key.scope.account_sql()), [], |r| r.get(0))?;
    let imported = imported.map(|t| time(&t).map(second)).transpose()?;
    let starts = [first(&format!("SELECT MIN(a.transacted_at) FROM account_transactions a WHERE a.user_id = ?1 AND a.exchange_id = ?2 AND {}", key.scope.rows("a")))?,
                  imported, attempts.iter().map(|s| s.0).min()];
    let Some(start) = starts.into_iter().flatten().min() else { return Ok(vec![]) };
    let mut covered: Vec<Span> = vec![];
    for source in [HF_API, UTA_API, EXPORT] { covered.extend(spans(c, key, source)?.into_iter().map(|(_, s)| s)); }
    Ok(merge(subtract((start, end), &covered)))
}

/// The gap row's entry for this history, rebuilt from coverage: the reading key, the credential the writing sync is
/// fenced on (a file import, fenced on none, keeps the one stored for that key), the uncovered spans, and the missing
/// ones (no ambiguous second: those an import cannot close), which the tracker reads.
pub fn refresh_gap(c: &FencedTransaction<'_>, key: &Key, now: DateTime<Utc>) -> Result<(), SyncError> {
    let missing = missing(c, key)?;
    let mut uncovered: Vec<Span> = spans(c, key, AMBIGUOUS)?.into_iter().map(|(_, s)| s).collect();
    uncovered.extend(missing.iter().copied());
    let uncovered = merge(uncovered);
    // The run's own `identity::touch` may have made number 1 after the key was read: then its account is read again (a
    // keyless import, NO_KEY, always names its account).
    let account = match key.scope.account { Some(a) => Some(a), None => crate::identity::account_of_key(c, key.id)? };
    let reading = crate::tracker::import::run::reading(c, key.user_id)?.into_iter().find(|k| k.exchange_id == key.exchange_id && k.account_id == account);
    let raw: Option<String> = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [gap_key(key.user_id)], |r| r.get(0)).optional()?.flatten();
    let mut row = raw.and_then(|r| serde_json::from_str::<Value>(&r).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    // An account 2+'s entry names its account; a number 1's keeps its exchange's key.
    let id = match key.scope.member { None => key.exchange_id.to_string(), Some(n) => format!("{}:{n}", key.exchange_id) };
    let credential = match (&reading, c.producer_version()) {
        (Some(k), Some(v)) if v.id == k.id => Some(v.cache_stamp()),
        (Some(k), _) if row[&id]["key"] == k.id => Some(row[&id]["credential"].clone()),
        _ => None,
    };
    match (reading, credential) {
        // The row stays readable (gaps::MAX_ROW): the first spans, and how many there are.
        (Some(k), Some(credential)) => {
            let listed = |spans: &[Span]| spans.iter().take(MAX_SPANS).map(|(a, b)| json!([sql_time(*a), sql_time(*b)])).collect::<Vec<_>>();
            row[&id] = json!({ "key": k.id, "credential": credential, "spans": uncovered.len(), "uncovered": listed(&uncovered),
                               "missing_spans": missing.len(), "missing": listed(&missing) });
        }
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
