//! The "import history" line (3.0, Rust only): a venue that holds more than the history Deltabadger could read from it
//! gets a line in the tracker's transactions table, which opens the steps to import what is missing.
//!
//! The judgement is a step of its own, after the `tracker_ledger` job has published and sent today's refresh
//! (`LedgerWalk::run`), never inside the publish: the ledger cache, its payload and its budget are untouched. The step
//! runs in one blocking unit with its own budget (`GAPS`, a fresh top-level scope) and its own SQL cap (a progress
//! handler, `SQL_INSTRUCTIONS`), and writes one row of its own, `rust_tracker_gaps.<owner>`, against the publication
//! it read. Any failure writes nothing. It reuses the cache's own validation (`cache::read_published`), which counts the
//! owner's history (`history_version`): that count is inside the cap, so a very large history can end the step before
//! it judges a venue, which leaves no row and loses nothing else.
//!
//! Pages only read stored evidence (`read`): one query on a clean account. Balance-derived gaps require the same
//! publication and balances. KuCoin HF periods no API unit or export covers (`kucoin_hf::refresh_gap`'s `missing`) carry
//! their credential fence and show without a monetary estimate or a warm publication; an export that covers them closes
//! them. The Binance repair's row is never read here (D-GAP-EVIDENCE-ONLY): its windows are open for every account and
//! no import closes them.
use super::cache::{self, Fingerprint, Published};
use super::figures::{self, Balance, Note};
use super::import::run::Reading;
use super::walk::Summary;
use crate::figures::{budget, dec::Dec, FiguresError};
use crate::identity::AccountRow;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering::SeqCst;

/// The step's arithmetic: a fresh scope, so nothing it does is charged to the ledger run's allowance.
pub const GAPS: budget::Limits = budget::Limits { steps: 50_000_000, held: 4_000_000 };
/// The step's SQL, all its statements together, in SQLite VM instructions.
pub const SQL_INSTRUCTIONS: u64 = 2_000_000;
/// The line shows from ten dollars of assets that arrived, or that the history needs, with no history behind them.
const GAP_FLOOR: i64 = 10;
const MAX_ROW: usize = 64 * 1024;
const MAX_DECIMAL: usize = 64;
const MAX_NOTES: usize = 20;
const VERSION: i64 = 1;

pub fn key(owner: i64) -> String { format!("rust_tracker_gaps.{owner}") }

/// Whether a venue's pending moves are empty: any row since its balances were taken (`figures::pending`'s own
/// `at >= taken`, borrowed rows included, so the probe errs towards "pending"). The time index bounds it to the rows
/// dated since the watermark; no composite index on (user, venue, time) exists, and the user's index would read their
/// whole history.
/// (#710 slice 5: a number 1's rows; `pending` reads an account 2+'s by its membership.)
#[doc(hidden)]
pub fn pending_sql() -> String { pending_sql_of("NOT EXISTS (SELECT 1 FROM exchange_account_rows m WHERE m.account_transaction_id = account_transactions.id)") }
fn pending_sql_of(rows: &str) -> String {
    format!("SELECT 1 FROM account_transactions {} WHERE transacted_at >= ?1 AND user_id = ?2 AND exchange_id = ?3 AND {rows} LIMIT 1", crate::sync::ledger::BY_TIME)
}

/// What the tests hold the step to: its step limit, a hook after its reads, how often it ran, and a page read that
/// reads nothing (the page as it was, for a statement trace to compare with).
#[doc(hidden)]
pub mod seam {
    use std::sync::{atomic::{AtomicBool, AtomicU64, AtomicUsize}, Mutex};
    pub static STEPS: AtomicU64 = AtomicU64::new(super::GAPS.steps);
    pub static SQL: AtomicU64 = AtomicU64::new(super::SQL_INSTRUCTIONS);
    pub type Hook = Box<dyn Fn() + Send>;
    pub static AFTER_READ: Mutex<Option<Hook>> = Mutex::new(None);
    pub static DERIVED: AtomicUsize = AtomicUsize::new(0);
    pub static SKIP_READ: AtomicBool = AtomicBool::new(false);
}

/// The KuCoin periods a page lists at most.
const MAX_UNREAD: usize = 20;

/// A gap as a page shows it: the venue, its reading key, the notes that are its evidence, and the periods of the
/// venue's history no API read or export covers (half-open, as stored; the first twenty, `more_unread` when there are
/// more).
/// `account` (#710 slice 5): the account the gap was judged on; a number 1's line keeps its exchange's markup.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredGap { pub exchange_id: i64, pub account: AccountRow, pub key: i64, pub name: String, pub kind: String, pub arrived: Dec, pub notes: Vec<Note>,
                       pub unread: Vec<(DateTime<Utc>, DateTime<Utc>)>, pub more_unread: bool }

/// `Figures#resolve` and `resolve_departed` for one venue whose pending moves are empty, without `compute_on`'s
/// lookups: per non-cash symbol, what the venue holds against what the history holds. More held arrived at the
/// venue's price (at cost while it has none); less left at cost; a position the venue no longer reports left at its
/// cost. Notes under a dollar are dropped as `compute_on` drops them; every note names `venue`, the venue judged.
pub fn resolve(summary: &Summary, balances: &[Balance], venue: &str) -> Result<Vec<Note>, FiguresError> {
    let labelled = figures::labelled(balances, &summary.labels);
    let balances = labelled.as_slice();
    let zero = Dec::zero();
    let mut symbols: Vec<&str> = vec![];
    let mut rows_of: HashMap<&str, Vec<&Balance>> = HashMap::new();
    for b in balances {
        budget::charge(1, 0)?;
        let rows = rows_of.entry(b.symbol.as_str()).or_default();
        if rows.is_empty() { symbols.push(b.symbol.as_str()); }
        rows.push(b);
    }
    budget::charge(summary.positions.len() as u64, 0)?;
    let position_of: HashMap<&str, &super::walk::Position> = summary.positions.iter().map(|p| (p.symbol.as_str(), p)).collect();
    let mut notes = vec![];
    let mut note = |kind: &'static str, symbol: &str, history: Dec, held: Dec, amount: Dec| {
        if figures::stated(&amount) { notes.push(Note { kind, symbol: symbol.into(), exchange: Some(venue.into()), history: Some(history), held: Some(held), amount }); }
    };
    let mut held_symbols: HashSet<&str> = HashSet::new();
    for symbol in symbols {
        budget::charge(1, 0)?;
        let mut units = Dec::zero();
        for r in &rows_of[symbol] { units = (&units + &(&r.free + &r.locked)?)?; }
        if !units.is_positive() { continue; }
        held_symbols.insert(symbol);
        if super::cash(symbol) { continue; }
        let (mut prices, mut values) = (Dec::zero(), Dec::zero());
        for r in &rows_of[symbol] {
            prices = (&prices + r.usd_price.as_ref().unwrap_or(&zero))?;
            values = (&values + r.usd_value.as_ref().unwrap_or(&zero))?;
        }
        let price = if prices.is_positive() { Some(values.div(&units)?) } else { None };
        let position = position_of.get(symbol).copied();
        let history = position.map_or_else(Dec::zero, |p| p.quantity.clone());
        let avg = position.map_or_else(Dec::zero, |p| p.avg_cost.clone());
        let delta = (&units - &history)?;
        if delta.is_negative() {
            let left = (&avg * &delta.neg())?;
            note("left", symbol, history, units, left);
        } else if delta.is_positive() {
            let arrived = (&delta * &price.unwrap_or(avg))?;
            note("arrived", symbol, history, units, arrived);
        }
    }
    for p in &summary.positions {
        budget::charge(1, 0)?;
        if held_symbols.contains(p.symbol.as_str()) || super::cash(&p.symbol) || !p.quantity.is_positive() { continue; }
        note("left", &p.symbol, p.quantity.clone(), Dec::zero(), p.cost.clone());
    }
    notes.sort_by(|a, b| (a.symbol.as_str(), a.kind).cmp(&(b.symbol.as_str(), b.kind)));
    Ok(notes)
}

/// One account's gap as the step found it, with the balances it was judged on.
struct Gap { exchange: i64, account: AccountRow, key: i64, origin: String, rows: i64, newest: Option<String>, arrived: Dec, notes: Vec<Note> }

/// The SQL cap for the closure's duration, cleared however it ends. SQLite calls the handler every 1000 instructions of
/// a statement, so a statement shorter than that goes uncounted: the cap is for the scans, which are what could grow.
struct Cap<'a>(&'a Connection);
impl<'a> Cap<'a> {
    fn on(c: &'a Connection, limit: u64) -> Self {
        let mut ran = 0u64;
        c.progress_handler(1000, Some(move || { ran += 1000; ran >= limit }));
        Cap(c)
    }
}
impl Drop for Cap<'_> {
    fn drop(&mut self) { self.0.progress_handler(0, None::<fn() -> bool>); }
}

fn data(what: &str) -> FiguresError { FiguresError::Data(format!("history gap: {what}")) }

/// The step: judge every venue against the entry just published and write the row (`[]` included), guarded on that
/// publication. Ok(true) when the row written has a gap to show; Ok(false) when it has none, when the entry was not
/// warm, or when another publication landed first. Nothing is written on an error.
pub fn derive(c: &Connection, owner: i64, now: DateTime<Utc>) -> Result<bool, FiguresError> {
    seam::DERIVED.fetch_add(1, SeqCst);
    let _cap = Cap::on(c, seam::SQL.load(SeqCst));
    let limits = budget::Limits { steps: seam::STEPS.load(SeqCst), held: GAPS.held };
    let (found, _) = budget::scope(limits, || judged(c, owner, now));
    let Some((from, gaps)) = found? else { return Ok(false) };
    if let Some(hook) = seam::AFTER_READ.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() { hook(); }
    let row = encode(&from, &gaps)?;
    // A late write (the job's deadline passed: this blocking unit runs on) or one that lost a race is a no-op.
    let tx = rusqlite::Transaction::new_unchecked(c, rusqlite::TransactionBehavior::Immediate)?;
    if cache::fingerprint(&tx, owner)?.as_ref() != Some(&from) { return Ok(false); }
    crate::app_config::set_plain(&tx, &key(owner), &row, now).map_err(FiguresError::Data)?;
    tx.commit()?;
    // A `[]` changes nothing a page shows (the publication's own refresh already drew it without a line): the page is
    // refreshed again only for a line to show, so a history with no gap completes exactly as it always has.
    Ok(!gaps.is_empty())
}

/// `derive` on the job's connection: whether a gap was written (the job then refreshes the page again). An error is one
/// log line and no row; it never fails the run.
pub async fn derive_logged(db: &crate::jobs::Db, owner: i64, wall: super::jobs::Wall) -> bool {
    match db.run(move |c, _| derive(c, owner, wall()).map_err(super::jobs::message)).await {
        Ok(wrote) => wrote,
        Err(e) => { crate::engine::log(&format!("[tracker] history gap for user {owner} not judged: {e}")); false }
    }
}

fn judged(c: &Connection, owner: i64, now: DateTime<Utc>) -> Result<Option<(Fingerprint, Vec<Gap>)>, FiguresError> {
    let tx = c.unchecked_transaction()?;
    let Published::Warm { venues, from, .. } = cache::read_published(&tx, owner, now)? else { return Ok(None) };
    let readings = super::import::run::reading(&tx, owner).map_err(|e| FiguresError::Data(e.0))?;
    let origin = crate::sync::cache::capture_read(&tx, owner,&crate::tracker::Scope::All).map_err(|_| data("reading provenance unavailable"))?;
    let mut gaps = vec![];
    let read = crate::sync::ledger_venues();
    for k in readings.iter().filter(|k| read.contains(&k.exchange_type.as_str())) {
        let account = account_of(&tx, owner, k.exchange_id, k.account_id)?;
        let summary = venues.iter().find(|(id, _)| *id == account.id).map(|(_, s)| s);
        if let Some(gap) = venue(&tx, owner, &origin, k, &account, summary)? { gaps.push(gap); }
    }
    tx.commit()?;
    Ok(Some((from, gaps)))
}

/// Account `id` of (owner, exchange) (None: the exchange's number 1, or its stand-in while it has no row).
fn account_of(c: &Connection, owner: i64, exchange: i64, id: Option<i64>) -> Result<AccountRow, FiguresError> {
    let found = match id { Some(id) => AccountRow::by_id(c, id)?.filter(|a| a.user_id == owner && a.exchange_id == exchange), None => AccountRow::first(c, owner, exchange)? };
    Ok(found.unwrap_or_else(|| AccountRow::unplaced(owner, exchange)))
}

/// The rules an account passes before its gap is stated: its ledger really read by the scheduler's key, the balances that
/// key produced and valued, nothing pending, and ten dollars or more arrived or opened.
fn venue(c: &Connection, owner: i64, origin: &crate::sync::cache::ReadOrigin, k: &Reading, account: &AccountRow, summary: Option<&Summary>) -> Result<Option<Gap>, FiguresError> {
    let scope = super::Scope::Account(*account);
    if !origin.key_read_complete(c, k.id, account).map_err(|_| data("reading provenance unavailable"))? { return Ok(None); }
    if figures::page::cold(c, owner, &scope, true)?.reason.is_some() { return Ok(None); }
    if pending(c, account)? { return Ok(None); }
    let balances = figures::balances(c, owner, &scope)?;
    let empty = Summary::empty();
    let summary = summary.unwrap_or(&empty);
    let mut notes = resolve(summary, &balances, &k.exchange_name)?;
    // D-GAP-OPENINGS: what the history needs but never shows arriving, at the opening's own price; unpriced, nothing.
    for ((symbol, quantity), (_, value)) in summary.openings.iter().zip(&summary.opening_values) {
        budget::charge(1, 0)?;
        if figures::stated(value) { notes.push(Note { kind: "opened", symbol: symbol.clone(), exchange: Some(k.exchange_name.clone()), history: Some(quantity.clone()), held: None, amount: value.clone() }); }
    }
    let (mut arrived, mut evidence) = (Dec::zero(), Dec::zero());
    for n in &notes {
        if n.kind == "arrived" { arrived = (&arrived + &n.amount)?; }
        if n.kind != "left" { evidence = (&evidence + &n.amount)?; }
    }
    if evidence < Dec::from_i64(GAP_FLOOR) { return Ok(None); }
    let Some(stamp) = self::origin(c, account)? else { return Ok(None) };
    let (rows, newest) = revision(c, account)?;
    Ok(Some(Gap { exchange: k.exchange_id, account: *account, key: k.id, origin: stamp, rows, newest, arrived, notes }))
}

/// Whether the account's pending moves are empty: any row of its own since its own balances were taken (#710 slice 5:
/// another account's later sync never hides them).
#[doc(hidden)]
pub fn pending(c: &Connection, account: &AccountRow) -> Result<bool, FiguresError> {
    let mark: Option<String> = c.query_row(&format!("SELECT MAX(k.balances_synced_at) FROM api_keys k WHERE {}", account.keys("k")), [], |r| r.get(0))?;
    // An account with no watermark is not brought forward (`figures::pending`).
    let Some(mark) = mark else { return Ok(false) };
    // To the second: a mark and a row of the same second compare in order whether or not either was written with its
    // microseconds (codec::format_time leaves whole seconds bare).
    let since: String = mark.chars().take(19).collect();
    let sql = if !account.placed() { pending_sql_of("1") } else if account.number == 1 { pending_sql() } else { pending_sql_of(&account.venue_ids().rows("account_transactions")) };
    Ok(c.query_row(&sql, rusqlite::params![since, account.user_id, account.exchange_id], |_| Ok(())).optional()?.is_some())
}

/// The balance origin row's `updated_at`: it moves with every balance sync of the account.
fn origin(c: &Connection, account: &AccountRow) -> Result<Option<String>, FiguresError> {
    Ok(c.query_row("SELECT updated_at FROM app_configs WHERE key = ?1", [crate::sync::cache::balance_origin_key_of(account)], |r| r.get::<_, Option<String>>(0)).optional()?.flatten())
}

/// The account `account` (None: (owner, exchange)'s number 1) and its balance rows' revision, in one read; none when it has
/// no row.
fn revision_of(c: &Connection, owner: i64, exchange: i64, account: Option<i64>) -> Result<Option<(AccountRow, i64, Option<String>)>, FiguresError> {
    let numbered = c.query_row("SELECT a.id, a.number, (SELECT COUNT(*) FROM exchange_account_balances b WHERE b.account_id = a.id), \
                    (SELECT MAX(b.updated_at) FROM exchange_account_balances b WHERE b.account_id = a.id) FROM exchange_accounts a \
                    WHERE a.user_id = ?1 AND a.exchange_id = ?2 AND CASE WHEN ?3 IS NULL THEN a.number = 1 ELSE a.id = ?3 END",
                   rusqlite::params![owner, exchange, account],
                   |r| Ok((AccountRow { id: r.get(0)?, user_id: owner, exchange_id: exchange, number: r.get(1)? }, r.get(2)?, r.get(3)?))).optional();
    match numbered {
        // An install without numbered accounts (no table, or no `number`): the exchange's one, its stand-in.
        Err(e) if e.to_string().contains("no such") => {
            let a = AccountRow::unplaced(owner, exchange);
            let (rows, newest) = c.query_row("SELECT COUNT(*), MAX(updated_at) FROM exchange_account_balances WHERE account_id = ?1 AND user_id = ?2", [a.id, owner], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(Some((a, rows, newest)))
        }
        other => Ok(other?),
    }
}

/// The account's balance rows: a sync writes `updated_at` on every row it keeps and deletes the rest.
fn revision(c: &Connection, account: &AccountRow) -> Result<(i64, Option<String>), FiguresError> {
    Ok(c.query_row("SELECT COUNT(*), MAX(updated_at) FROM exchange_account_balances WHERE account_id = ?1 AND user_id = ?2", [account.id, account.user_id], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

fn encode(from: &Fingerprint, gaps: &[Gap]) -> Result<String, FiguresError> {
    // Stored to the places a page could ever show and more (money to 8, quantities to 16; the line shows 2 and 8): a
    // price divided out of a balance carries 32 digits or more, which no note needs. The bound applies after that.
    let decimal = |d: &Dec, places: i64| -> Result<Value, FiguresError> {
        let text = d.round(places)?.to_s_f();
        if text.len() > MAX_DECIMAL { return Err(data("a decimal exceeds 64 characters")); }
        Ok(Value::String(text))
    };
    let money = |d: &Dec| decimal(d, 8);
    let optional = |d: &Option<Dec>| d.as_ref().map_or(Ok(Value::Null), |d| decimal(d, 16));
    let mut out = vec![];
    for g in gaps {
        let mut notes: Vec<&Note> = g.notes.iter().collect();
        // What opened the line (arrived, opened) first, so no number of `left` notes pushes it out of the twenty.
        notes.sort_by(|a, b| (a.kind == "left").cmp(&(b.kind == "left")).then_with(|| b.amount.cmp(&a.amount)));
        notes.truncate(MAX_NOTES);
        let notes = notes.into_iter().map(|n| Ok(json!({"kind": n.kind, "symbol": n.symbol, "exchange": n.exchange, "history": optional(&n.history)?, "held": optional(&n.held)?, "amount": money(&n.amount)?})))
            .collect::<Result<Vec<_>, FiguresError>>()?;
        let mut gap = json!({"exchange": g.exchange, "key": g.key, "balances": {"origin_updated_at": g.origin, "rows": g.rows, "max_updated_at": g.newest}, "arrived_usd": money(&g.arrived)?, "notes": notes});
        // An account 2+'s line names its account; a number 1's row is as it always was.
        if g.account.number != 1 { gap["account"] = json!(g.account.id); }
        out.push(gap);
    }
    let row = json!({"v": VERSION, "from": from.to_json(), "gaps": out}).to_string();
    if row.len() > MAX_ROW { return Err(data("the row exceeds 65536 bytes")); }
    Ok(row)
}

/// A stored gap before its revision is checked (`account` None: the exchange's number 1).
struct Decoded { exchange: i64, account: Option<i64>, key: i64, origin: String, rows: i64, newest: Option<String>, arrived: Dec, notes: Vec<Note> }

/// The row's gaps for `scope`, when it is this publication's; None for anything malformed.
fn decode(raw: &str, from: &Fingerprint, scope: &super::Scope) -> Option<Vec<Decoded>> {
    if raw.len() > MAX_ROW { return None; }
    let v: Value = serde_json::from_str(raw).ok()?;
    if v["v"].as_i64() != Some(VERSION) || !v["from"].is_object() || Fingerprint::from_json(&v["from"]) != *from { return None; }
    let decimal = |v: &Value| -> Option<Dec> { v.as_str().filter(|t| t.len() <= MAX_DECIMAL).and_then(|t| Dec::parse(t).ok()) };
    let optional = |v: &Value| -> Option<Option<Dec>> { if v.is_null() { Some(None) } else { decimal(v).map(Some) } };
    let mut out = vec![];
    for g in v["gaps"].as_array()? {
        let exchange = g["exchange"].as_i64()?;
        let account = if g["account"].is_null() { None } else { Some(g["account"].as_i64()?) };
        let notes = g["notes"].as_array()?.iter().map(|n| Some(Note {
            kind: match n["kind"].as_str()? { "arrived" => "arrived", "opened" => "opened", "left" => "left", _ => return None },
            symbol: n["symbol"].as_str()?.into(),
            exchange: if n["exchange"].is_null() { None } else { Some(n["exchange"].as_str()?.into()) },
            history: optional(&n["history"])?, held: optional(&n["held"])?, amount: decimal(&n["amount"])?,
        })).collect::<Option<Vec<_>>>()?;
        let balances = &g["balances"];
        let newest = if balances["max_updated_at"].is_null() { None } else { Some(balances["max_updated_at"].as_str()?.to_string()) };
        let gap = Decoded { exchange, account, key: g["key"].as_i64()?, origin: balances["origin_updated_at"].as_str()?.into(), rows: balances["rows"].as_i64()?, newest, arrived: decimal(&g["arrived_usd"])?, notes };
        if in_scope(scope, exchange, account) { out.push(gap); }
    }
    Some(out)
}

/// Whether a line of `exchange`'s account `account` (None: its number 1) is the scope's.
fn in_scope(scope: &super::Scope, exchange: i64, account: Option<i64>) -> bool {
    match scope {
        super::Scope::All => true,
        super::Scope::Account(a) => a.exchange_id == exchange && match account { Some(id) => id == a.id, None => a.number == 1 },
        super::Scope::EmptyLegacyExchange(_) => false,
    }
}

/// The gaps a page shows for `scope`, read against the entry the page itself read (`published`: None unless it is
/// warm, and then nothing is read). `reading` is the user's `ApiKey.reading`, when the caller holds it already.
/// Queries: the row; with an open gap stored, the reading keys once and two per derived gap venue (its balances'
/// revision), and the credential check per open KuCoin entry.
pub fn read(c: &Connection, owner: i64, scope: &super::Scope, published: Option<&Fingerprint>, reading: Option<&[Reading]>) -> Result<Vec<StoredGap>, FiguresError> {
    if seam::SKIP_READ.load(SeqCst) { return Ok(vec![]); }
    // Both sources share one indexed lookup, retaining the clean page's one-query budget.
    let kucoin_key = crate::sync::kucoin_hf::gap_key(owner);
    let mut statement = c.prepare("SELECT key,value FROM app_configs WHERE key IN (?1,?2)")?;
    let rows = statement.query_map([key(owner), kucoin_key.clone()], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?
        .collect::<Result<HashMap<_, _>, _>>()?;
    let derived = match (published, rows.get(&key(owner)).and_then(Option::as_deref)) {
        (Some(from), Some(raw)) => decode(raw, from, scope).unwrap_or_default(),
        _ => vec![],
    };
    // Which KuCoin entries are open, from the JSON alone: `missing` non-empty. An entry written before `missing` was
    // stored is closed until the next sync rewrites it.
    // #710 slice 5: an entry is keyed `<exchange>` for a number 1 and `<exchange>:<account>` for an account 2+.
    let kucoin: Vec<(i64, Option<i64>, Value)> = rows.get(&kucoin_key).and_then(Option::as_deref).filter(|s| s.len() <= MAX_ROW)
        .and_then(|s| serde_json::from_str::<Value>(s).ok()).and_then(|v| match v { Value::Object(m) => Some(m), _ => None }).into_iter().flatten()
        .filter_map(|(id, entry)| match id.split_once(':') {
            None => Some((id.parse::<i64>().ok()?, None, entry)),
            Some((e, a)) => Some((e.parse::<i64>().ok()?, Some(a.parse::<i64>().ok()?), entry)),
        })
        .filter(|(id, account, entry)| in_scope(scope, *id, *account) && entry["missing"].as_array().is_some_and(|m| !m.is_empty()))
        .collect();
    if derived.is_empty() && kucoin.is_empty() { return Ok(vec![]); }
    let loaded;
    let reading = match reading { Some(r) => r, None => { loaded = super::import::run::reading(c, owner).map_err(|e| FiguresError::Data(e.0))?; &loaded } };
    let mut out = read_derived(c, owner, derived, reading)?;
    for (exchange, account, entry) in kucoin {
        // The entry's account, as its reading key reads it (a stored account is an account 2+'s).
        let Some(k) = reading.iter().find(|k| k.exchange_id == exchange && entry["key"].as_i64().is_none_or(|key| key == k.id)
                                             && account.is_none_or(|a| k.account_id == Some(a))) else { continue };
        let first = match (account, k.account_id) {
            (Some(id), _) => AccountRow { id, user_id: owner, exchange_id: exchange, number: 2 },
            (None, Some(id)) => AccountRow { id, user_id: owner, exchange_id: exchange, number: 1 },
            (None, None) => AccountRow::unplaced(owner, exchange),
        };
        let Some(version) = crate::engine::model::CredentialVersion::from_stamp(&entry["credential"]) else { continue };
        if version.id != k.id || !crate::engine::model::credential_is_current(c, &version).map_err(|_| data("gap credential unavailable"))? { continue; }
        let listed = entry["missing"].as_array().map_or(&[][..], Vec::as_slice);
        let unread: Vec<(DateTime<Utc>, DateTime<Utc>)> = listed.iter().take(MAX_UNREAD).filter_map(|s| {
            let at = |v: &Value| v.as_str().and_then(|t| crate::codec::parse_time(t).ok());
            Some((at(&s[0])?, at(&s[1])?))
        }).collect();
        let total = entry["missing_spans"].as_u64().map_or(listed.len(), |n| n as usize).max(listed.len());
        let more_unread = total > unread.len();
        match out.iter_mut().find(|g| g.account.id == first.id) {
            Some(g) => { g.unread = unread; g.more_unread = more_unread; }
            None => out.push(StoredGap { exchange_id: k.exchange_id, account: first, key: k.id, name: k.exchange_name.clone(), kind: k.exchange_type.clone(), arrived: Dec::zero(), notes: vec![], unread, more_unread }),
        }
    }
    Ok(out)
}

fn read_derived(c: &Connection, owner: i64, stored: Vec<Decoded>, reading: &[Reading]) -> Result<Vec<StoredGap>, FiguresError> {
    let mut out = vec![];
    for g in stored {
        // The key the gap was judged with must still be the one the account is read with; its own balances unchanged.
        let Some(k) = reading.iter().find(|k| k.exchange_id == g.exchange && k.id == g.key) else { continue };
        // Two reads, as before #710: the origin by its key (a stored account is an account 2+'s), then the account and its
        // balances' revision in one.
        let key = match g.account { None => crate::sync::cache::balance_origin_key(owner, g.exchange), Some(id) => crate::sync::cache::balance_origin_key_of(&AccountRow { id, user_id: owner, exchange_id: g.exchange, number: 2 }) };
        if c.query_row("SELECT updated_at FROM app_configs WHERE key = ?1", [key], |r| r.get::<_, Option<String>>(0)).optional()?.flatten().as_deref() != Some(g.origin.as_str()) { continue; }
        let Some((account, rows, newest)) = revision_of(c, owner, g.exchange, g.account)? else { continue };
        if (rows, newest) != (g.rows, g.newest) { continue; }
        out.push(StoredGap { exchange_id: g.exchange, account, key: g.key, name: k.exchange_name.clone(), kind: k.exchange_type.clone(), arrived: g.arrived, notes: g.notes, unread: vec![], more_unread: false });
    }
    out.sort_by(|a, b| b.arrived.cmp(&a.arrived));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::walk::Position;
    fn d(s: &str) -> Dec { Dec::strict(s).unwrap() }
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges(id INTEGER PRIMARY KEY, name TEXT); INSERT INTO exchanges VALUES (1,'Binance'),(2,'KuCoin');
            CREATE TABLE assets(id INTEGER PRIMARY KEY, category TEXT, symbol TEXT);
            CREATE TABLE account_transactions(id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, base_currency TEXT, base_asset_id INTEGER);
            INSERT INTO account_transactions(user_id,exchange_id,base_currency) VALUES (1,1,'BTC'),(1,2,'BTC'),(1,2,'X'),(1,2,'ADA');").unwrap();
        c
    }
    fn position(symbol: &str, quantity: &str, cost: &str) -> Position {
        let (q, c) = (d(quantity), d(cost));
        Position { symbol: symbol.into(), avg_cost: c.div(&q).unwrap(), quantity: q, cost: c, opened_at: None, estimated: false, unpriced: Dec::zero() }
    }
    fn balance(symbol: &str, units: &str, price: Option<&str>) -> Balance {
        let value = price.map(|p| (&d(units) * &d(p)).unwrap());
        Balance { asset: 1, account: -1, category: Some("Cryptocurrency".into()), symbol: symbol.into(), exchange: "KuCoin".into(), free: d(units), locked: Dec::zero(), usd_price: price.map(d), usd_value: value, priced_at: None, synced_at: None }
    }
    fn key(n: &Note) -> (&'static str, String, Option<Dec>, Option<Dec>, Dec) { (n.kind, n.symbol.clone(), n.history.clone(), n.held.clone(), n.amount.clone()) }

    /// `resolve` against `compute_on` for one venue with nothing pending: the same arrived and left notes, exactly, in
    /// the same order; the venue named where `compute_on` names it, and where it names none (a symbol whose history
    /// spans venues), named still.
    #[test]
    fn resolve_agrees_with_compute_on_note_for_note() {
        let c = db();
        let mut summary = Summary::empty();
        summary.positions = vec![position("BTC", "1", "30000"), position("SOL", "1", "50"), position("X", "3", "1"), position("Y", "3", "3.5"), position("ADA", "10", "0.5"), position("USDT", "100", "100")];
        let balances = vec![balance("ETH", "7.4", Some("2000")), balance("SOL", "2", None), balance("BTC", "0.4", Some("60000")), balance("Y", "1", Some("1")), balance("USDT", "40", Some("1")), balance("DOT", "0.01", Some("5"))];
        let ours = resolve(&summary, &balances, "KuCoin").unwrap();
        let theirs = crate::tracker::figures::compute_on(&c, 1, &crate::tracker::Scope::All, &summary, &balances, &[], &crate::tracker::fx::FxTable::default(), chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()).unwrap();
        let theirs: Vec<&Note> = theirs.notes.iter().filter(|n| matches!(n.kind, "arrived" | "left")).collect();
        assert_eq!(ours.iter().map(key).collect::<Vec<_>>(), theirs.iter().map(|n| key(n)).collect::<Vec<_>>());
        let shown = |kind: &str, symbol: &str| ours.iter().find(|n| n.kind == kind && n.symbol == symbol).map(|n| n.amount.to_s_f());
        assert_eq!(shown("arrived", "ETH").as_deref(), Some("14800.0"), "at the venue's price");
        assert_eq!(shown("arrived", "SOL").as_deref(), Some("50.0"), "at cost while the venue has no price");
        assert_eq!(shown("left", "BTC").as_deref(), Some("18000.0"), "a partial leave at cost");
        // $1 over 3 units, all gone: its cost, never a third of a dollar times three rounded under the floor.
        assert_eq!(shown("left", "X").as_deref(), Some("1.0"));
        assert_eq!(shown("left", "Y"), Some((&d("3.5").div(&d("3")).unwrap() * &d("2")).unwrap().to_s_f()), "avg_cost × the units gone");
        assert_eq!(shown("arrived", "DOT"), None, "under a dollar, in silence");
        assert!(ours.iter().all(|n| n.symbol != "USDT"), "cash is never a gap");
        for (o, t) in ours.iter().zip(&theirs) {
            assert_eq!(o.exchange.as_deref(), Some("KuCoin"));
            match o.symbol.as_str() {
                // BTC's history spans Binance and KuCoin: compute_on names no venue for it; ADA's rows are KuCoin's.
                "BTC" => assert_eq!(t.exchange, Some("KuCoin".into()), "held: the balance row's venue"),
                "X" | "ADA" => assert_eq!(t.exchange.as_deref(), Some("KuCoin")),
                _ => assert!(t.exchange.is_none() || t.exchange == o.exchange, "{t:?}"),
            }
        }
        // Departed from KuCoin only, its history on both venues: compute_on names none, the step names the venue judged.
        let mut departed = Summary::empty();
        departed.positions = vec![position("BTC", "0.5", "20000")];
        let ours = resolve(&departed, &[], "KuCoin").unwrap();
        let theirs = crate::tracker::figures::compute_on(&c, 1, &crate::tracker::Scope::All, &departed, &[], &[], &crate::tracker::fx::FxTable::default(), chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()).unwrap();
        assert_eq!(ours.iter().map(key).collect::<Vec<_>>(), theirs.notes.iter().filter(|n| n.kind == "left").map(key).collect::<Vec<_>>());
        assert_eq!((ours[0].exchange.as_deref(), theirs.notes[0].exchange.as_deref()), (Some("KuCoin"), None));
    }

    fn from() -> Fingerprint { Fingerprint::from_json(&json!({"history":"1_2_x","prices":0,"expires":"2026-10-31T02:00:00+00:00","producers":[]})) }
    /// §6 test 10: a number 1's stored line is exactly as before; an account 2+'s names its account, and each scope reads its
    /// own account's line only.
    #[test]
    fn an_account_2s_line_names_its_account_and_a_number_1s_is_unchanged() {
        let raw = encode(&from(), &[gap(vec![note("ETH", d("20"))])]).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert!(v["gaps"][0].get("account").is_none(), "number 1: {raw}");
        let second = AccountRow { id: 77, user_id: 1, exchange_id: 2, number: 2 };
        let raw = encode(&from(), &[gap(vec![note("ETH", d("20"))]), Gap { account: second, ..gap(vec![note("BTC", d("30"))]) }]).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["gaps"][1]["account"], 77);
        let symbols = |scope: &crate::tracker::Scope| decode(&raw, &from(), scope).unwrap().iter().map(|g| g.notes[0].symbol.clone()).collect::<Vec<_>>();
        assert_eq!(symbols(&scope(2)), ["ETH"]);
        assert_eq!(symbols(&crate::tracker::Scope::Account(second)), ["BTC"]);
        assert_eq!(symbols(&crate::tracker::Scope::All), ["ETH", "BTC"]);
    }
    fn first(exchange: i64) -> AccountRow { AccountRow { id: 40 + exchange, user_id: 1, exchange_id: exchange, number: 1 } }
    fn scope(exchange: i64) -> crate::tracker::Scope { crate::tracker::Scope::Account(first(exchange)) }
    fn gap(notes: Vec<Note>) -> Gap { Gap { exchange: 2, account: first(2), key: 9, origin: "2026-10-01 02:00:00".into(), rows: 1, newest: Some("2026-10-01 02:00:00".into()), arrived: d("14800"), notes } }
    fn note(symbol: &str, amount: Dec) -> Note { Note { kind: "arrived", symbol: symbol.into(), exchange: Some("KuCoin".into()), history: Some(Dec::zero()), held: Some(d("7.4")), amount } }

    /// A row round-trips; its decimals are stored rounded (money to 8 places, units to 16), arrived first by amount,
    /// at most twenty a venue; over 64 characters, or a row over 64 KiB, is refused, writing nothing.
    #[test]
    fn the_row_is_bounded_and_round_trips() {
        let long = d("2000").div(&d("3")).unwrap();
        let mut notes: Vec<Note> = (0..25).map(|i| note(&format!("S{i:02}"), (&long + &Dec::from_i64(i)).unwrap())).collect();
        notes.push(Note { kind: "left", amount: d("99999"), ..note("BTC", Dec::zero()) });
        let raw = encode(&from(), &[gap(notes)]).unwrap();
        let v: Value = serde_json::from_str(&raw).unwrap();
        let stored = v["gaps"][0]["notes"].as_array().unwrap();
        assert_eq!(stored.len(), 20);
        assert_eq!((stored[0]["symbol"].as_str(), stored[0]["amount"].as_str()), (Some("S24"), Some("690.66666667")));
        assert!(stored.iter().all(|n| n["kind"] == "arrived"), "arrived first; the left note is past the twenty");
        let decoded = decode(&raw, &from(), &scope(2)).unwrap();
        assert_eq!((decoded.len(), decoded[0].notes.len(), decoded[0].arrived.to_s_f()), (1, 20, "14800.0".to_string()));
        assert!(decode(&raw, &from(), &scope(3)).unwrap().is_empty(), "another venue's scope");
        let other = Fingerprint::from_json(&json!({"history":"1_2_y","prices":0,"expires":"2026-10-31T02:00:00+00:00","producers":[]}));
        assert!(decode(&raw, &other, &crate::tracker::Scope::All).is_none(), "another publication");
        assert!(encode(&from(), &[gap(vec![note("BTC", d(&format!("1{}", "0".repeat(70))))])]).is_err(), "a decimal over 64 characters");
        // Twenty larger `left` notes never push out what opened the line.
        let mut many: Vec<Note> = (0..20).map(|i| Note { kind: "left", amount: d("1000"), ..note(&format!("L{i:02}"), Dec::zero()) }).collect();
        many.push(Note { kind: "opened", held: None, amount: d("15"), ..note("ETH", Dec::zero()) });
        let raw = encode(&from(), &[gap(many)]).unwrap();
        let decoded = decode(&raw, &from(), &crate::tracker::Scope::All).unwrap();
        assert_eq!((decoded[0].notes.len(), decoded[0].notes[0].kind, decoded[0].notes[0].held.clone()), (20, "opened", None));
        let wide = vec![note(&"X".repeat(70 * 1024), d("20"))];
        assert!(encode(&from(), &[gap(wide)]).is_err(), "a row over 64 KiB");
    }
}
