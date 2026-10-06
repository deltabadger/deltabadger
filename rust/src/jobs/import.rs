//! MarketData's import half (app/models/market_data.rb:233-338, :815-930) for the reference-data jobs: what a payload row
//! becomes in `assets`, `tickers`, `exchange_assets` and `indices`. Values are cast as ActiveRecord casts them before
//! upsert_all serializes them (activerecord 8.1 insert_all.rb:247). Decimals are bound as decimal text, which SQLite's
//! NUMERIC affinity stores exactly as it stores Rails' quoted BigDecimal.
//!
//! upsert_all on SQLite (sqlite3_adapter.rb:481-498) is `INSERT … ON CONFLICT (<unique_by>) DO UPDATE SET c = excluded.c` for
//! every given column but the conflict target. The timestamps are given, so they are plain columns: an update overwrites
//! created_at too, and no CASE keeps updated_at (insert_all.rb:57-58, :280-305). A payload with a conflict key twice is
//! applied in order, the last row winning, as Rails 8.1.4 on SQLite does: rows are upserted one by one, in order.
//!
//! Every import is computed first, off the write lock: the casts, the ticker reconcile and the final value of every row it
//! touches (a failure there writes nothing). Then it is published in phases, each a sequence of units (`publish`): one
//! `BEGIN IMMEDIATE` of at most CHUNK rows on the blocking pool, CHUNK_GAP apart. A unit writes every column of the rows it
//! carries, so a reader between two units sees each row either as it was or as it ends, never names without their trading
//! parameters. A stop or a deadline lands between two units, and no transaction holds the write lock for more than one
//! unit.
use super::{Db, CHUNK};
use crate::codec::format_time;
use crate::crypto::Cipher;
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use rusqlite::types::Value as Sql;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

type R<T> = Result<T, String>;
fn sql(e: rusqlite::Error) -> String { e.to_string() }

/// The pause between two units of a phase. Every other writer of this database waits on SQLite's default busy handler
/// (busy_timeout), which sleeps at most 100 ms between two tries: a pause longer than that lets a writer on another thread
/// (the web) in between two units. The engine, on the runtime thread, needs no pause: while it waits, no next unit starts.
pub const CHUNK_GAP: Duration = Duration::from_millis(110);

type Holds = BTreeMap<(String, &'static str), Duration>;
static HOLDS: Mutex<Holds> = Mutex::new(BTreeMap::new());
/// Per database file: when its last write unit committed, and the shortest pause seen before a unit (diagnostics).
static PACE: Mutex<BTreeMap<String, (Instant, Option<Duration>)>> = Mutex::new(BTreeMap::new());

/// The shortest pause between two write units on the database file `path` so far, phase boundaries included.
pub fn shortest_gap(path: &str) -> Option<Duration> { PACE.lock().unwrap_or_else(PoisonError::into_inner).get(path).and_then(|p| p.1) }

/// The longest write-lock hold of each phase on the database file `path` (as `Connection::path` reports it) in this
/// process so far: diagnostics, and what the write-lock bound tests assert.
pub fn holds(path: &str) -> BTreeMap<&'static str, Duration> {
    HOLDS.lock().unwrap_or_else(PoisonError::into_inner).iter().filter(|((p, _), _)| p == path).map(|((_, phase), d)| (*phase, *d)).collect()
}

/// Ruby truthiness: only nil and false are falsy (`||`, `&&`, `if`).
pub fn truthy(v: &Value) -> bool { !matches!(v, Value::Null | Value::Bool(false)) }

/// `present?`.
pub fn present(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// Rows actually written by a unit. These ids are checked inside its transaction, never cached across units.
pub enum Touched { None, Assets(Vec<i64>), Tickers(Vec<i64>) }

impl Touched {
    fn relevant(&self, c: &Connection) -> R<bool> {
        let (ids, predicate) = match self {
            Self::None => return Ok(false),
            Self::Assets(ids) => (ids, "t.base_asset_id IN (SELECT value FROM json_each(?1)) OR t.quote_asset_id IN (SELECT value FROM json_each(?1))"),
            Self::Tickers(ids) => (ids, "t.id IN (SELECT value FROM json_each(?1))"),
        };
        if ids.is_empty() { return Ok(false); }
        // Manual allocations include members not yet persisted; index membership (including leavers)
        // comes from bot_index_assets. Both participate in the same guard. The pair lookup
        // uses the exchange/base prefix of tickers' unique pair index. Every quote counts: the split-history check
        // reads all venue spellings of a member, including tickers outside the bot's own quote.
        // Pending intents and open orders count in every status: stopping a bot does not settle an order already sent.
        let query = format!("WITH members AS (SELECT b.id AS bot_id, CAST(a.key AS INTEGER) AS asset_id \
            FROM bots b, json_each(b.settings, '$.allocations') a UNION SELECT bot_id, asset_id FROM bot_index_assets) \
            SELECT EXISTS(SELECT 1 FROM bots b JOIN members m ON m.bot_id=b.id \
            JOIN tickers t ON t.exchange_id = b.exchange_id AND t.base_asset_id = m.asset_id \
            WHERE (b.status IN ({}) OR json_extract(b.transient_data, '$.rust_placement') IS NOT NULL \
                OR EXISTS(SELECT 1 FROM transactions orders WHERE orders.bot_id = b.id AND orders.status = 0 AND orders.external_status IN (0, 1))) \
                AND ({predicate}))",
            crate::engine::model::working_list());
        c.query_row(&query, [serde_json::to_string(ids).map_err(|e| e.to_string())?], |r| r.get(0)).map_err(sql)
    }
}

/// One `BEGIN IMMEDIATE` around `f`: its value and affected ids are returned together. A unit touching a working bot's
/// reference rows must pass the eligibility guard before COMMIT. Unrelated units skip the full install check. Both
/// commits and rollbacks record the write-lock hold and keep the gap before the next unit.
pub fn in_transaction<X>(c: &Connection, cipher: &Cipher, phase: &'static str, f: impl FnOnce(&Connection) -> R<(X, Touched)>) -> R<X> {
    // CHUNK_GAP since the last unit on this file, whatever its phase (phase boundaries and single-unit phases
    // too). On the blocking pool, so the runtime thread does not wait; a stop or a deadline lands at most this gap later.
    let path = c.path().unwrap_or_default().to_string();
    let last = PACE.lock().unwrap_or_else(PoisonError::into_inner).get(&path).map(|p| p.0);
    if let Some(wait) = last.map(|end| CHUNK_GAP.saturating_sub(end.elapsed())).filter(|w| !w.is_zero()) { std::thread::sleep(wait); }
    let gap = last.map(|end| end.elapsed());
    let started = Instant::now();
    let result = (|| {
        let tx = Transaction::new_unchecked(c, TransactionBehavior::Immediate).map_err(sql)?;
        let (x, touched) = f(&tx)?;
        if touched.relevant(&tx)? {
            crate::engine::eligibility::guard(&tx, cipher, None)
                .map_err(|refusal| format!("rolled back ({phase}): this write would stop a running bot: {}", refusal.reason()))?;
        }
        tx.commit().map_err(sql)?;
        Ok(x)
    })(); // committed, or rolled back as the transaction dropped: either way the write lock is released here
    // The release is recorded on every way out, a rollback included, so the next unit keeps the gap.
    let held = started.elapsed();
    let mut h = HOLDS.lock().unwrap_or_else(PoisonError::into_inner);
    let longest = h.entry((path.clone(), phase)).or_default();
    *longest = (*longest).max(held);
    let mut p = PACE.lock().unwrap_or_else(PoisonError::into_inner);
    let shortest = p.get(&path).and_then(|e| e.1);
    p.insert(path, (Instant::now(), match (shortest, gap) { (Some(a), Some(b)) => Some(a.min(b)), (a, b) => a.or(b) }));
    result
}

/// `items` in units of CHUNK.
pub fn chunks<T: Clone>(items: &[T]) -> Vec<Vec<T>> { items.chunks(CHUNK).map(<[T]>::to_vec).collect() }

/// Publishes a phase: each unit in its own transaction on the blocking pool (`in_transaction` keeps CHUNK_GAP since the
/// last unit on the file, across phases too). Between two units the job's future awaits, so the runner's deadline and the
/// stop signal take effect there, and the unit in hand is the only work a dropped run leaves behind.
pub async fn publish<U, F>(db: &Db, phase: &'static str, units: Vec<U>, write: F) -> R<()>
where U: Send + 'static, F: Fn(&Connection, U) -> R<Touched> + Clone + Send + 'static {
    for unit in units {
        let write = write.clone();
        db.run(move |c, cipher| in_transaction(c, cipher, phase, |c| write(c, unit).map(|touched| ((), touched)))).await?;
    }
    Ok(())
}

fn text(v: Option<String>) -> Sql { v.map_or(Sql::Null, Sql::Text) }
fn int(v: Option<i64>) -> Sql { v.map_or(Sql::Null, Sql::Integer) }

/// ActiveModel::Type::String#cast. ponytail: a JSON float prints as Rust prints it ("1.5"), not as Float#to_s ("1.0e+20");
/// data-api sends strings for every string column.
fn string(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(if *b { "t" } else { "f" }.into()),
        other => Some(other.to_string()),
    }
}

/// ActiveModel::Type::Integer#cast: an Integer as is, a Float truncated, a blank String nil, else its leading digits.
fn integer(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => {
            let t = s.trim_start();
            let end = t.char_indices().take_while(|(i, ch)| ch.is_ascii_digit() || (*i == 0 && (*ch == '-' || *ch == '+'))).count();
            Some(t[..end].parse().unwrap_or(0))
        }
        Value::Bool(b) => Some(*b as i64),
        _ => None,
    }
}

/// String#to_d's reading: after leading whitespace, the longest prefix of sign, digits, `.` digits and an exponent
/// (e, E, d or D), an underscore counting only between two digits; "0" when no digit leads. ponytail: "Infinity" and
/// "NaN", which Ruby reads as such, read as 0 here; data-api sends neither.
fn numeric_prefix(s: &str) -> String {
    let b = s.trim_start_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']).as_bytes();
    let mut i = 0;
    let digits = |i: &mut usize, out: &mut String| {
        let start = out.len();
        while *i < b.len() {
            if b[*i].is_ascii_digit() { out.push(b[*i] as char); *i += 1; }
            else if b[*i] == b'_' && out.len() > start && b.get(*i + 1).is_some_and(u8::is_ascii_digit) { *i += 1; }
            else { break; }
        }
        out.len() - start
    };
    let mut out = String::new();
    if let Some(&sign @ (b'+' | b'-')) = b.first() { out.push(sign as char); i = 1; }
    let mut n = digits(&mut i, &mut out);
    if b.get(i) == Some(&b'.') {
        let (mut j, mut frac) = (i + 1, String::new());
        let f = digits(&mut j, &mut frac);
        if n + f > 0 { if n == 0 { out.push('0'); } out.push('.'); out.push_str(&frac); if f == 0 { out.push('0'); } n += f; i = j; }
    }
    if n == 0 { return "0".into(); }
    if matches!(b.get(i), Some(b'e' | b'E' | b'd' | b'D')) {
        let (mut j, mut exp) = (i + 1, String::from("e"));
        if let Some(&sign @ (b'+' | b'-')) = b.get(j) { exp.push(sign as char); j += 1; }
        if digits(&mut j, &mut exp) > 0 { out.push_str(&exp); }
    }
    out
}

/// ActiveModel::Type::Decimal#cast with no precision: a Float by to_d (its shortest digits, 16 significant), an Integer exactly, a String by String#to_d
/// (its leading numeric prefix; garbage reads as 0).
fn decimal(v: &Value) -> R<Option<String>> {
    Ok(match v {
        Value::Null => None,
        Value::Number(n) if n.is_i64() || n.is_u64() => Some(n.to_string()),
        Value::Number(n) => Some(BigDec::from_f64(n.as_f64().unwrap_or(0.0)).map_err(|e| format!("{e:?}"))?.to_s_f()),
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => Some(BigDec::parse(&numeric_prefix(s)).map(|d| d.to_s_f()).unwrap_or_else(|_| "0".into())),
        other => return Err(format!("{other} is not a decimal")),
    })
}

/// The same with precision 30 and scale 8 (assets.circulating_supply): a Float goes through Float#round(8), then
/// BigDecimal(f, 16) (16 significant digits, half up), then round(8). Float#round(8) is taken on the float's shortest digits,
/// half up. ponytail: exact for every value the grid pins (assets-import); a tie in the 17th digit could differ by one unit.
fn supply(v: &Value) -> R<Option<String>> {
    if let Value::Number(n) = v {
        if !(n.is_i64() || n.is_u64()) {
            let d = BigDec::parse(&format!("{:e}", n.as_f64().unwrap_or(0.0))).map_err(|e| format!("{e:?}"))?;
            return Ok(Some(d.round(8).round_sig(16).round(8).to_s_f()));
        }
    }
    match decimal(v)? {
        Some(t) => Ok(Some(BigDec::parse(&t).map_err(|e| format!("{e:?}"))?.round(8).to_s_f())),
        None => Ok(None),
    }
}

/// ActiveRecord::Type::Json: nil stays NULL, anything else is stored as its JSON text.
fn json_text(v: &Value) -> Option<String> { (!v.is_null()).then(|| v.to_string()) }

/// `x || {}`.
fn or_empty(v: &Value) -> String { if truthy(v) { v.to_string() } else { "{}".into() } }

/// `image_url.presence || absolutize_logo_url(logo_url)` (:448, :879, :468-473).
fn image_url(a: &Value, public_url: &str) -> Option<String> {
    if present(&a["image_url"]) { return string(&a["image_url"]); }
    present(&a["logo_url"]).then(|| format!("{public_url}{}", string(&a["logo_url"]).unwrap_or_default()))
}

/// MarketData.import_assets! (:233-245, :871-888), computed: the upsert rows in payload order, and the instrument-type
/// groups of apply_instrument_types! (:251-264: only rows that carry the key, grouped by its presence).
pub struct AssetsPlan { pub rows: Vec<Vec<Sql>>, pub types: Vec<(Option<String>, Vec<String>)> }

pub fn plan_assets(rows: &[Value], public_url: &str) -> R<AssetsPlan> {
    let mut out = Vec::with_capacity(rows.len());
    for a in rows {
        out.push(vec![text(string(&a["external_id"])), text(string(&a["symbol"])), text(string(&a["name"])), text(string(&a["category"])),
                      text(image_url(a, public_url)), text(string(&a["color"])), int(integer(&a["market_cap_rank"])), int(integer(&a["market_cap"])),
                      text(supply(&a["circulating_supply"])?), text(string(&a["url"]))]);
    }
    let mut types: Vec<(Option<String>, Vec<String>)> = vec![];
    for a in rows.iter().filter(|a| a.as_object().is_some_and(|o| o.contains_key("instrument_type"))) {
        let ty = if present(&a["instrument_type"]) { string(&a["instrument_type"]) } else { None };
        let id = string(&a["external_id"]).unwrap_or_default();
        match types.iter_mut().find(|(t, _)| *t == ty) { Some((_, ids)) => ids.push(id), None => types.push((ty, vec![id])) }
    }
    Ok(AssetsPlan { rows: out, types })
}

/// Publishes an assets plan: the upserts, then the payload's instrument types, then the local tokenized registry
/// (Asset.mark_tokenized!, app/models/asset.rb:25-39), each phase in units of CHUNK. Callers return early on a blank
/// payload, as `return if assets_data.blank?` does.
pub async fn import_assets(db: &Db, plan: AssetsPlan, now: DateTime<Utc>) -> R<()> {
    let t = format_time(now);
    publish(db, "assets", chunks(&plan.rows), move |c, rows: Vec<Vec<Sql>>| {
        let mut ids = vec![];
        for r in rows {
            ids.push(c.prepare_cached("INSERT INTO assets (external_id, symbol, name, category, image_url, color, market_cap_rank, market_cap, circulating_supply, url, created_at, updated_at) \
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11) ON CONFLICT (external_id) DO UPDATE SET symbol = excluded.symbol, \
                       name = excluded.name, category = excluded.category, image_url = excluded.image_url, color = excluded.color, \
                       market_cap_rank = excluded.market_cap_rank, market_cap = excluded.market_cap, circulating_supply = excluded.circulating_supply, \
                       url = excluded.url, created_at = excluded.created_at, updated_at = excluded.updated_at RETURNING id").map_err(sql)?
                .query_row(params_from_iter(r.into_iter().chain([Sql::Text(t.clone())])), |r| r.get(0)).map_err(sql)?);
        }
        Ok(Touched::Assets(ids))
    }).await?;
    let units: Vec<(Option<String>, Vec<String>)> = plan.types.into_iter().flat_map(|(ty, ids)| chunks(&ids).into_iter().map(move |part| (ty.clone(), part))).collect();
    publish(db, "instrument types", units, |c, (ty, ids): (Option<String>, Vec<String>)| {
        let marks = vec!["?"; ids.len()].join(", ");
        match ty {
            // NULL-safe: `where.not(instrument_type: x)` would skip the NULL rows.
            Some(t) => {
                let binds: Vec<String> = std::iter::once(t.clone()).chain(ids).chain(std::iter::once(t)).collect();
                returning_assets(c, &format!("UPDATE assets SET instrument_type = ? WHERE external_id IN ({marks}) AND (instrument_type IS NULL OR instrument_type != ?) RETURNING id"), params_from_iter(&binds))
            }
            None => {
                returning_assets(c, &format!("UPDATE assets SET instrument_type = NULL WHERE external_id IN ({marks}) AND instrument_type IS NOT NULL RETURNING id"), params_from_iter(&ids))
            }
        }
    }).await?;
    mark_tokenized(db).await
}

fn returning_assets(c: &Connection, statement: &str, params: impl rusqlite::Params) -> R<Touched> {
    let mut s = c.prepare_cached(statement).map_err(sql)?;
    let ids = s.query_map(params, |r| r.get(0)).map_err(sql)?.collect::<Result<Vec<_>, _>>().map_err(sql)?;
    Ok(Touched::Assets(ids))
}

/// Asset.mark_tokenized! (app/models/asset.rb:25-39), the wrapper registry, update_all (updated_at stays): the rows to
/// mark are read first, then marked in units of CHUNK.
pub async fn mark_tokenized(db: &Db) -> R<()> {
    let ids: Vec<i64> = db.run(|c, _| {
        let mut s = c.prepare("SELECT id FROM assets WHERE (external_id IN ('pax-gold', 'tether-gold') OR external_id LIKE '%-xstock' \
                               OR external_id LIKE '%-ondo-tokenized%' OR external_id LIKE '%-bstocks') \
                               AND (instrument_type IS NULL OR instrument_type != 'tokenized') ORDER BY id").map_err(sql)?;
        let ids = s.query_map([], |r| r.get(0)).map_err(sql)?.collect::<Result<Vec<_>, _>>().map_err(sql)?;
        Ok(ids)
    }).await?;
    publish(db, "tokenized", chunks(&ids), |c, ids: Vec<i64>| {
        let marks = vec!["?"; ids.len()].join(", ");
        returning_assets(c, &format!("UPDATE assets SET instrument_type = 'tokenized' WHERE id IN ({marks}) RETURNING id"), params_from_iter(&ids))
    }).await
}

/// CATEGORY_BY_TYPE (:377): ETFs and stocks are both category Stock.
fn stock_category(v: &Value) -> Option<&'static str> { matches!(v.as_str(), Some("stock" | "etf")).then_some("Stock") }

/// sync_stocks_from_deltabadger!'s upsert (:433-454), computed: rows of an unknown type are skipped.
pub fn plan_stock_assets(rows: &[Value], public_url: &str) -> Vec<Vec<Sql>> {
    rows.iter().filter_map(|r| stock_category(&r["type"]).map(|category| vec![
        text(string(&r["external_id"])), text(string(&r["symbol"])), text(string(&r["name"])), Sql::Text(category.into()),
        text(string(&r["type"])), text(string(&r["color"])), text(image_url(r, public_url))])).collect()
}

/// Publishes the stock assets in units of CHUNK; nothing to write is no write.
pub async fn import_stock_assets(db: &Db, rows: Vec<Vec<Sql>>, now: DateTime<Utc>) -> R<()> {
    let t = format_time(now);
    publish(db, "stock assets", chunks(&rows), move |c, rows: Vec<Vec<Sql>>| {
        let mut ids = vec![];
        for r in rows {
            ids.push(c.prepare_cached("INSERT INTO assets (external_id, symbol, name, category, instrument_type, color, image_url, created_at, updated_at) \
                       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8) ON CONFLICT (external_id) DO UPDATE SET symbol = excluded.symbol, \
                       name = excluded.name, category = excluded.category, instrument_type = excluded.instrument_type, color = excluded.color, \
                       image_url = excluded.image_url, created_at = excluded.created_at, updated_at = excluded.updated_at RETURNING id").map_err(sql)?
                .query_row(params_from_iter(r.into_iter().chain([Sql::Text(t.clone())])), |r| r.get(0)).map_err(sql)?);
        }
        Ok(Touched::Assets(ids))
    }).await
}

/// Fiat.currencies' USD colour (app/models/fiat.rb:28-34).
const USD_COLOR: &str = "#355E3B";

/// The local `usd` row both Alpaca syncs anchor every quote to. `reassign`: the stock sync assigns symbol,
/// name and category on every run and saves a changed row with updated_at (:484-490); the crypto sync only creates it
/// (:630-636). A blank colour takes Fiat's USD colour in both. In the caller's transaction (`in_transaction`).
pub fn ensure_usd(c: &Connection, reassign: bool, now: DateTime<Utc>) -> R<((), Touched)> {
    let t = format_time(now);
    type Usd = (i64, Option<String>, Option<String>, Option<String>, Option<String>);
    let row: Option<Usd> = c.query_row("SELECT id, symbol, name, category, color FROM assets WHERE external_id = 'usd' LIMIT 1", [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional().map_err(sql)?;
    let mut ids = vec![];
    match row {
        None => {
            c.execute("INSERT INTO assets (external_id, symbol, name, category, color, created_at, updated_at) VALUES ('usd', 'USD', 'US Dollar', 'Fiat', ?1, ?2, ?2)",
                      params![USD_COLOR, t]).map_err(sql)?;
            ids.push(c.last_insert_rowid());
        }
        Some(_) if !reassign => {}
        Some((id, symbol, name, category, color)) => {
            let mut sets: Vec<(&str, &str)> = vec![];
            if symbol.as_deref() != Some("USD") { sets.push(("symbol", "USD")); }
            if name.as_deref() != Some("US Dollar") { sets.push(("name", "US Dollar")); }
            if category.as_deref() != Some("Fiat") { sets.push(("category", "Fiat")); }
            if color.as_deref().is_none_or(|s| s.trim().is_empty()) { sets.push(("color", USD_COLOR)); }
            if !sets.is_empty() {
                let cols = sets.iter().map(|(col, _)| format!("{col} = ?")).collect::<Vec<_>>().join(", ");
                let mut binds: Vec<String> = sets.iter().map(|(_, v)| v.to_string()).collect();
                binds.push(t);
                binds.push(id.to_string());
                c.execute(&format!("UPDATE assets SET {cols}, updated_at = ? WHERE id = ?"), params_from_iter(&binds)).map_err(sql)?;
                ids.push(id);
            }
        }
    }
    Ok(((), Touched::Assets(ids)))
}

/// `Asset.where(external_id: ids).pluck(:external_id, :id)`: the same query, so SQLite returns the same order.
fn asset_ids(c: &Connection, ids: &[String]) -> R<Vec<(String, i64)>> {
    if ids.is_empty() { return Ok(vec![]); }
    let marks = vec!["?"; ids.len()].join(", ");
    let mut s = c.prepare(&format!("SELECT external_id, id FROM assets WHERE external_id IN ({marks})")).map_err(sql)?;
    let rows = s.query_map(params_from_iter(ids), |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?.collect::<Result<Vec<_>, _>>().map_err(sql)?;
    Ok(rows)
}

/// `Asset.where(external_id: ids).count`.
pub fn count_assets(c: &Connection, ids: &[String]) -> R<i64> { Ok(asset_ids(c, ids)?.len() as i64) }

/// `tickers_data.flat_map { |t| [t['base_external_id'], t['quote_external_id']] }.uniq`.
fn external_ids(rows: &[Value]) -> Vec<String> {
    let mut seen = HashSet::new();
    rows.iter().flat_map(|t| [&t["base_external_id"], &t["quote_external_id"]])
        .filter_map(|v| v.as_str()).filter(|s| seen.insert(s.to_string())).map(String::from).collect()
}

#[derive(Clone, Debug)]
pub struct TickerRecord {
    pub base_asset_id: i64, pub quote_asset_id: i64, pub asset_class: String,
    pub base: Option<String>, pub quote: Option<String>, pub ticker: Option<String>,
    pub minimum_base_size: Option<String>, pub minimum_quote_size: Option<String>,
    pub maximum_base_size: Option<String>, pub maximum_quote_size: Option<String>,
    pub base_decimals: Option<i64>, pub quote_decimals: Option<i64>, pub price_decimals: Option<i64>,
    pub trading_enabled: bool,
}

/// `BigDecimal(x)` for a present value (:917-920), which raises on garbage (the sync then fails); else the default.
/// An Integer and a String are exact. A Float goes through `BigDec::from_f64`, Float#to_d's 16 significant digits, as
/// Ruby's `BigDecimal(float)` does: `BigDecimal(1.0000000000000002)` is `1.0`.
fn size(v: &Value, default: Option<&str>) -> R<Option<String>> {
    if !present(v) { return Ok(default.map(String::from)); }
    match v {
        Value::String(s) => BigDec::parse(s.trim()).map(|d| Some(d.to_s_f())).map_err(|_| format!("invalid value for BigDecimal(): {s:?}")),
        Value::Number(n) if n.is_i64() || n.is_u64() => Ok(Some(n.to_string())),
        Value::Number(n) => BigDec::from_f64(n.as_f64().unwrap_or(0.0)).map(|d| Some(d.to_s_f())).map_err(|e| format!("{e:?}")),
        other => Err(format!("can't convert {other} into BigDecimal")),
    }
}

/// MarketData.ticker_records_for (:286-314): the importable, deduped ticker rows. Pure: no writes.
pub fn ticker_records_for(c: &Connection, rows: &[Value], category: Option<&str>) -> R<Vec<TickerRecord>> {
    if rows.is_empty() { return Ok(vec![]); }
    let ids: HashMap<String, i64> = asset_ids(c, &external_ids(rows))?.into_iter().collect();
    let mut out = vec![];
    for t in rows {
        let (Some(&b), Some(&q)) = (t["base_external_id"].as_str().and_then(|k| ids.get(k)), t["quote_external_id"].as_str().and_then(|k| ids.get(k))) else { continue };
        // Pairs the exchange gave no trading params for are skipped (:296-304).
        if !(truthy(&t["base_decimals"]) && truthy(&t["quote_decimals"]) && truthy(&t["price_decimals"])) { continue; }
        let asset_class: String = c.query_row("SELECT coalesce(category, '') FROM assets WHERE id = ?1", [b], |r| r.get(0)).map_err(sql)?;
        if category.is_some_and(|category| category != asset_class) { continue; }
        out.push(TickerRecord { asset_class,
            base_asset_id: b, quote_asset_id: q, base: string(&t["base"]), quote: string(&t["quote"]), ticker: string(&t["ticker"]),
            minimum_base_size: size(&t["minimum_base_size"], Some("0"))?, minimum_quote_size: size(&t["minimum_quote_size"], Some("0"))?,
            maximum_base_size: size(&t["maximum_base_size"], None)?, maximum_quote_size: size(&t["maximum_quote_size"], None)?,
            base_decimals: integer(&t["base_decimals"]), quote_decimals: integer(&t["quote_decimals"]), price_decimals: integer(&t["price_decimals"]),
            // Older data-api versions omit the key: absent or nil is enabled (:924-926).
            trading_enabled: t["trading_enabled"] != Value::Bool(false),
        });
    }
    let mut classes = HashMap::new();
    for r in &out {
        if let Some(symbol) = &r.ticker {
            if classes.insert(symbol, &r.asset_class).is_some_and(|held| held != &r.asset_class) {
                return Err("Ticker symbol belongs to another asset class".into());
            }
        }
    }
    // `uniq!` keeps the first occurrence per key, in this order (:309-312).
    fn uniq_by<K: std::hash::Hash + Eq>(v: &mut Vec<TickerRecord>, key: impl Fn(&TickerRecord) -> K) {
        let mut seen = HashSet::new();
        v.retain(|r| seen.insert(key(r)));
    }
    uniq_by(&mut out, |r| (r.base_asset_id, r.quote_asset_id));
    uniq_by(&mut out, |r| (r.asset_class.clone(), r.base.clone(), r.quote.clone()));
    uniq_by(&mut out, |r| r.ticker.clone());
    Ok(out)
}

const TOMBSTONE: &str = "__stale_";

/// tombstone_value (:817-821): idempotent per namespace.
fn tombstone(id: i64, value: &str) -> String {
    if value.starts_with(TOMBSTONE) { value.into() } else { format!("{TOMBSTONE}{id}_{value}") }
}

struct Held { asset_class: String, id: i64, pair: (i64, i64), ticker: String, base: String, quote: String }


/// What one ticker unit writes, in order, in one transaction.
pub enum TickerWrite {
    /// upsert_ticker_attributes (:909-930) on the pair, every column at once: names, every trading parameter, available,
    /// trading_enabled and the stamps. `existing`: the pair's row (updated in place); else a new row under `id`, the id
    /// Rails' upsert_all gives it (one sequence value per payload row, conflicting or not).
    Upsert { existing: Option<i64>, id: i64, record: Box<TickerRecord> },
    /// reconcile_ticker_conflicts!' first pass (:823-869) for a row whose pair the payload does not carry: it gives up the
    /// names another pair needs, tombstoned in both namespaces, and becomes unavailable. Tickers are never deleted.
    Tombstone { id: i64, ticker: String, base: String },
}

/// One unit: `moving` are the rows of this unit whose names change; they first take a placeholder name, so that the
/// unit's rows may take each other's names in any order. All inside one transaction: no reader sees a placeholder.
/// `sequence`: in the last unit, the tickers sequence as Rails' upsert_all leaves it.
pub struct TickerUnit { pub moving: Vec<i64>, pub writes: Vec<TickerWrite>, pub sequence: Option<i64> }

/// MarketData.import_tickers! (:316-338), computed against the venue's current rows.
pub struct TickerPlan {
    /// exchange_assets for every resolved asset of the raw payload, even when no ticker is importable.
    pub exchange_assets: Vec<i64>,
    pub units: Vec<TickerUnit>,
    /// The availability sweep (:563-567, :661-664): the venue's tickers of the category that this import does not write.
    /// Empty when nothing is written, as Rails sweeps only `if written_base_asset_ids.any?`.
    pub sweep: Vec<i64>,
    /// The written base_asset_ids, first-seen order (import_tickers!' return value).
    pub written: Vec<i64>,
}

fn find(parent: &mut [usize], i: usize) -> usize {
    let mut r = i;
    while parent[r] != r { r = parent[r]; }
    let mut i = i;
    while parent[i] != r { let next = parent[i]; parent[i] = r; i = next; }
    r
}

/// Computes an import of `rows` on `exchange_id` (reads only). The final value of every row is Rails' end state:
/// - pass 1 of reconcile_ticker_conflicts! on in-memory copies, as Rails edits its in-memory objects (a row met twice is
///   tombstoned once): a row of another pair that holds the [exchange, ticker] or [exchange, base, quote] slot a record
///   needs is tombstoned;
/// - pass 2 (aligning a kept row of the same pair onto the record's names) and the upsert both end in the record's values,
///   so a row the payload carries ends as its record, and a tombstoned row the payload also carries does too.
///
/// Units: a record and the rows whose current names it takes form a rename group (union-find over those
/// dependencies only: a chain or a cycle of colliding names, wherever its rows sit in the payload). A group is never split
/// across units; groups are packed in payload order into units of at most CHUNK rows. A group larger than CHUNK fails the
/// import here, before anything is written. New rows get the ids Rails gives them whatever unit carries them: Rails'
/// upsert_all takes one sequence value per payload row, in order (a conflicting row too), so record `k`'s new row is
/// `base + k + 1`, and the last unit leaves the sequence at `base + records`.
pub fn plan_tickers(c: &Connection, exchange_id: i64, rows: &[Value], sweep_category: Option<&str>) -> R<TickerPlan> {
    let empty = |exchange_assets| TickerPlan { exchange_assets, units: vec![], sweep: vec![], written: vec![] };
    if rows.is_empty() { return Ok(empty(vec![])); }
    let exchange_assets: Vec<i64> = asset_ids(c, &external_ids(rows))?.into_iter().map(|(_, id)| id).collect();
    let records = ticker_records_for(c, rows, sweep_category)?;
    if records.is_empty() { return Ok(empty(exchange_assets)); }

    let mut s = c.prepare("SELECT t.id, t.base_asset_id, t.quote_asset_id, t.ticker, t.base, t.quote, coalesce(a.category, '') FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 ORDER BY t.id").map_err(sql)?;
    let held: Vec<Held> = s.query_map([exchange_id], |r| Ok(Held { asset_class: r.get(6)?, id: r.get(0)?, pair: (r.get(1)?, r.get(2)?), ticker: r.get(3)?, base: r.get(4)?, quote: r.get(5)? }))
        .map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
    let base: i64 = c.query_row("SELECT max(coalesce((SELECT seq FROM sqlite_sequence WHERE name = 'tickers'), 0), coalesce((SELECT max(id) FROM tickers), 0))",
                                [], |r| r.get(0)).map_err(sql)?;
    // index_by: the last row wins a key, though the unique indexes leave one per key.
    let by_pair: HashMap<(i64, i64), usize> = held.iter().enumerate().map(|(i, h)| (h.pair, i)).collect();
    let by_ticker: HashMap<&str, usize> = held.iter().enumerate().map(|(i, h)| (h.ticker.as_str(), i)).collect();
    let by_bq: HashMap<(&str, &str, &str), usize> = held.iter().enumerate().map(|(i, h)| ((h.asset_class.as_str(), h.base.as_str(), h.quote.as_str()), i)).collect();
    let record_of: HashMap<(i64, i64), usize> = records.iter().enumerate().map(|(i, r)| ((r.base_asset_id, r.quote_asset_id), i)).collect();

    // Pass 1, on copies of the names: who holds what each record needs, and the tombstones.
    let mut names: Vec<(String, String)> = held.iter().map(|h| (h.ticker.clone(), h.base.clone())).collect();
    let mut needs: Vec<Vec<usize>> = vec![vec![]; records.len()];
    for (ri, r) in records.iter().enumerate() {
        let by_t = r.ticker.as_deref().and_then(|t| by_ticker.get(t));
        let by_b = match (&r.base, &r.quote) { (Some(b), Some(q)) => by_bq.get(&(r.asset_class.as_str(), b.as_str(), q.as_str())), _ => None };
        for &i in [by_t, by_b].into_iter().flatten() {
            if held[i].asset_class != r.asset_class { return Err("Ticker symbol belongs to another asset class".into()); }
            if held[i].pair == (r.base_asset_id, r.quote_asset_id) || needs[ri].contains(&i) { continue; }
            needs[ri].push(i);
            names[i] = (tombstone(held[i].id, &names[i].0), tombstone(held[i].id, &names[i].1));
        }
    }

    // The writes, in payload order: a tombstoned row just before the first record that needs its names, then the record.
    let mut items: Vec<(TickerWrite, Option<i64>)> = vec![]; // the write, and the row it moves (whose names change)
    let mut position: HashMap<usize, usize> = HashMap::new(); // held index -> item
    let mut record_at: Vec<usize> = vec![0; records.len()];
    for (ri, r) in records.iter().enumerate() {
        for &i in &needs[ri] {
            if record_of.contains_key(&held[i].pair) || position.contains_key(&i) { continue; }
            position.insert(i, items.len());
            items.push((TickerWrite::Tombstone { id: held[i].id, ticker: names[i].0.clone(), base: names[i].1.clone() }, Some(held[i].id)));
        }
        let existing = by_pair.get(&(r.base_asset_id, r.quote_asset_id)).copied();
        let renamed = existing.filter(|&i| {
            let h = &held[i];
            r.ticker.as_deref() != Some(h.ticker.as_str()) || r.base.as_deref() != Some(h.base.as_str()) || r.quote.as_deref() != Some(h.quote.as_str())
        });
        if let Some(i) = existing { position.insert(i, items.len()); }
        record_at[ri] = items.len();
        let write = TickerWrite::Upsert { existing: existing.map(|i| held[i].id), id: base + ri as i64 + 1, record: Box::new(r.clone()) };
        items.push((write, renamed.map(|i| held[i].id)));
    }

    // Rename groups: a record and every row whose current names it takes.
    let mut parent: Vec<usize> = (0..items.len()).collect();
    for (ri, holders) in needs.iter().enumerate() {
        for &i in holders {
            let (a, b) = (find(&mut parent, record_at[ri]), find(&mut parent, position[&i]));
            if a != b { parent[a.max(b)] = a.min(b); }
        }
    }
    let mut groups: Vec<Vec<usize>> = vec![];
    let mut group_of: HashMap<usize, usize> = HashMap::new(); // root -> group, in order of first item
    for n in 0..items.len() {
        let root = find(&mut parent, n);
        let g = *group_of.entry(root).or_insert_with(|| { groups.push(vec![]); groups.len() - 1 });
        groups[g].push(n);
    }
    if let Some(g) = groups.iter().find(|g| g.len() > CHUNK) {
        return Err(format!("a rename chain of {} tickers is larger than one write unit ({CHUNK}); nothing was written", g.len()));
    }
    let mut slots: Vec<Option<(TickerWrite, Option<i64>)>> = items.into_iter().map(Some).collect();
    let mut units: Vec<TickerUnit> = vec![];
    let mut unit = TickerUnit { moving: vec![], writes: vec![], sequence: None };
    for g in groups {
        if unit.writes.len() + g.len() > CHUNK { units.push(std::mem::replace(&mut unit, TickerUnit { moving: vec![], writes: vec![], sequence: None })); }
        for n in g {
            let (w, moving) = slots[n].take().expect("each item in one group");
            unit.moving.extend(moving);
            unit.writes.push(w);
        }
    }
    unit.sequence = Some(base + records.len() as i64);
    units.push(unit);

    let mut seen = HashSet::new();
    let written: Vec<i64> = records.iter().map(|r| r.base_asset_id).filter(|id| seen.insert(*id)).collect();
    let sweep = match sweep_category {
        Some(category) => {
            let mut s = c.prepare("SELECT t.id, t.base_asset_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                                   WHERE t.exchange_id = ?1 AND a.category = ?2 ORDER BY t.id").map_err(sql)?;
            let all: Vec<(i64, i64)> = s.query_map(params![exchange_id, category], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?
                .collect::<Result<_, _>>().map_err(sql)?;
            all.into_iter().filter(|(_, base)| !seen.contains(base)).map(|(id, _)| id).collect()
        }
        None => vec![],
    };
    Ok(TickerPlan { exchange_assets, units, sweep, written })
}

/// Publishes a ticker plan: the exchange assets, the tickers, then the sweep (committed last: never a sweep without its
/// import), each phase in units. Returns the written base_asset_ids.
pub async fn publish_tickers(db: &Db, exchange_id: i64, plan: TickerPlan, now: DateTime<Utc>) -> R<Vec<i64>> {
    let t = format_time(now);
    let t2 = t.clone();
    publish(db, "exchange assets", chunks(&plan.exchange_assets), move |c, ids: Vec<i64>| {
        for id in ids {
            c.prepare_cached("INSERT INTO exchange_assets (asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?3) \
                       ON CONFLICT (asset_id, exchange_id) DO UPDATE SET available = excluded.available, created_at = excluded.created_at, \
                       updated_at = excluded.updated_at").map_err(sql)?.execute(params![id, exchange_id, t]).map_err(sql)?;
        }
        Ok(Touched::None)
    }).await?;
    publish(db, "tickers", plan.units, move |c, unit: TickerUnit| {
        let ids = unit.writes.iter().map(|w| match w {
            TickerWrite::Tombstone { id, .. } => *id,
            TickerWrite::Upsert { existing, id, .. } => existing.unwrap_or(*id),
        }).collect();
        for id in &unit.moving {
            c.execute("UPDATE tickers SET ticker = '__moving_' || id, base = '__moving_' || id WHERE id = ?1", [id]).map_err(sql)?;
        }
        for w in unit.writes {
            match w {
                TickerWrite::Tombstone { id, ticker, base } => {
                    c.execute("UPDATE tickers SET ticker = ?1, base = ?2, available = 0 WHERE id = ?3", params![ticker, base, id]).map_err(sql)?;
                }
                TickerWrite::Upsert { existing: Some(id), record: r, .. } => {
                    c.prepare_cached("UPDATE tickers SET base = ?1, quote = ?2, ticker = ?3, minimum_base_size = ?4, minimum_quote_size = ?5, \
                               maximum_base_size = ?6, maximum_quote_size = ?7, base_decimals = ?8, quote_decimals = ?9, price_decimals = ?10, \
                               available = 1, trading_enabled = ?11, created_at = ?12, updated_at = ?12 WHERE id = ?13").map_err(sql)?
                        .execute(params![r.base, r.quote, r.ticker, r.minimum_base_size, r.minimum_quote_size, r.maximum_base_size, r.maximum_quote_size,
                                         r.base_decimals, r.quote_decimals, r.price_decimals, r.trading_enabled, t2, id]).map_err(sql)?;
                }
                TickerWrite::Upsert { existing: None, id, record: r } => {
                    // `id` was planned from the table as it stood at plan time. If another writer inserted a ticker since,
                    // this INSERT can meet its id and fail on the primary key. That is refused, not overwritten: the unit
                    // rolls back whole, the job fails before the sweep, the units before it hold whole rows, and the next
                    // run plans again from the table as it then stands. Nothing else inserts tickers on an install the engine runs.
                    c.prepare_cached("INSERT INTO tickers (id, exchange_id, base, quote, ticker, base_asset_id, quote_asset_id, minimum_base_size, \
                               minimum_quote_size, maximum_base_size, maximum_quote_size, base_decimals, quote_decimals, price_decimals, available, \
                               trading_enabled, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1, ?15, ?16, ?16)").map_err(sql)?
                        .execute(params![id, exchange_id, r.base, r.quote, r.ticker, r.base_asset_id, r.quote_asset_id, r.minimum_base_size, r.minimum_quote_size,
                                         r.maximum_base_size, r.maximum_quote_size, r.base_decimals, r.quote_decimals, r.price_decimals, r.trading_enabled, t2]).map_err(sql)?;
                }
            }
        }
        if let Some(seq) = unit.sequence {
            // As Rails' upsert_all leaves it: one value per payload row (an explicit id moves it only to that id).
            if c.execute("UPDATE sqlite_sequence SET seq = max(seq, ?1) WHERE name = 'tickers'", [seq]).map_err(sql)? == 0 {
                c.execute("INSERT INTO sqlite_sequence (name, seq) VALUES ('tickers', ?1)", [seq]).map_err(sql)?;
            }
        }
        Ok(Touched::Tickers(ids))
    }).await?;
    // update_all: updated_at stays.
    publish(db, "sweep", chunks(&plan.sweep), |c, ids: Vec<i64>| {
        let marks = vec!["?"; ids.len()].join(", ");
        c.execute(&format!("UPDATE tickers SET available = 0 WHERE id IN ({marks})"), params_from_iter(&ids)).map_err(sql)?;
        Ok(Touched::Tickers(ids))
    }).await?;
    Ok(plan.written)
}

/// Index::WEIGHTED_CATEGORIES (app/models/index.rb:20-33).
const WEIGHTED_CATEGORIES: [(&str, i64); 12] = [
    ("layer-1", 12), ("layer-2", 11), ("meme-token", 10), ("privacy-coins", 9), ("yield-farming", 8), ("runes", 7),
    ("decentralized-finance-defi", 6), ("artificial-intelligence", 5), ("gaming", 4), ("real-world-assets-rwa", 3), ("ai-agents", 2),
    ("zero-knowledge-zk", 1),
];

/// MarketData.import_indices! (:266-273, :890-907), computed: one row per payload row, in order (a conflict key met twice
/// ends as its last row).
pub fn plan_indices(rows: &[Value]) -> R<Vec<Vec<Sql>>> {
    rows.iter().map(|i| {
        // `index_data['weight'] || WEIGHTED_CATEGORIES[external_id] || 0`
        let weight = if truthy(&i["weight"]) { integer(&i["weight"]) }
                     else { Some(i["external_id"].as_str().and_then(|e| WEIGHTED_CATEGORIES.iter().find(|(k, _)| *k == e)).map_or(0, |(_, w)| *w)) };
        Ok(vec![text(string(&i["external_id"])), text(string(&i["source"])), text(string(&i["name"])), text(string(&i["description"])),
                text(json_text(&i["top_coins"])), Sql::Text(or_empty(&i["top_coins_by_exchange"])), text(decimal(&i["market_cap"])?),
                Sql::Text(or_empty(&i["available_exchanges"])), Sql::Text(or_empty(&i["weights"])), int(weight)])
    }).collect()
}

/// Publishes the indices (tens of rows: one unit in practice).
pub async fn import_indices(db: &Db, rows: Vec<Vec<Sql>>, now: DateTime<Utc>) -> R<()> {
    let t = format_time(now);
    publish(db, "indices", chunks(&rows), move |c, rows: Vec<Vec<Sql>>| {
        for r in rows {
            c.prepare_cached("INSERT INTO indices (external_id, source, name, description, top_coins, top_coins_by_exchange, market_cap, available_exchanges, \
                       weights, weight, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11) \
                       ON CONFLICT (external_id, source) DO UPDATE SET name = excluded.name, description = excluded.description, \
                       top_coins = excluded.top_coins, top_coins_by_exchange = excluded.top_coins_by_exchange, market_cap = excluded.market_cap, \
                       available_exchanges = excluded.available_exchanges, weights = excluded.weights, weight = excluded.weight, \
                       created_at = excluded.created_at, updated_at = excluded.updated_at").map_err(sql)?
                .execute(params_from_iter(r.into_iter().chain([Sql::Text(t.clone())]))).map_err(sql)?;
        }
        Ok(Touched::None)
    }).await
}
