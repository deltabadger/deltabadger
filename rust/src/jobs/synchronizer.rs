//! Exchange::Synchronizer (app/models/exchange/synchronizer.rb) on the coingecko provider (PLAN-A17 A17-2): per venue, its
//! own public catalogue (get_tickers_info) applied unconditionally; when the catalogue holds a pair no ticker of the venue
//! has, and the venue's once-a-day crawl claim is taken (or the run is forced), CoinGecko's `exchanges/{id}/tickers` crawl
//! builds the symbol → coin map that resolves new pairs and mints the assets it names. Nothing is ever deleted.
//!
//! Each venue is planned in one read (`plan`), Rails' row-by-row loop simulated over the rows it reads, then published in
//! bounded units (`import::publish`): new assets first, then each catalogue row's writes (its exchange assets with its
//! ticker, never split), then the sweep. A venue's units commit on their own, so a run the deadline cancels keeps every
//! finished venue and the next run redoes the rest.
//!
//! Named divergences:
//! - D-CG-SYMBOL-AMBIGUOUS: a symbol CoinGecko gives two ids, or an id two symbols claim, leaves every new pair using it
//!   unresolved (logged), instead of Rails' "first wins" (synchronizer.rb:87-90, :120-128), which can bind a new pair to
//!   the wrong coin. Kraken's DOGE → XDG and the blacklists apply first, as Rails applies them.
//! - D-CG-429-NO-MORE-CRAWLS: a crawl's 429 ends the run's further crawls (every catalogue still applies); Rails goes on
//!   asking the next venue into the same closed quota.
//! - D-CG-RETIRED-NOT-ASKED: a retired venue (Exchange::RETIRED_TYPES) is not synced; Rails queues it and logs its
//!   retired_failure every 4 hours.
//! - D-CG-ROW-REFUSED: a catalogue row the database refuses (a ticker name another of the venue's tickers holds, a
//!   required column missing) is skipped and logged, and the rows after it still apply; Rails raises RecordNotUnique or
//!   NotNullViolation out of the venue's sync there. Like Rails, the refused row confirms nothing and the venue is not
//!   swept that run.
use super::coingecko::{self, Clients, CrawlError};
use super::import::{self, decimal, integer, string, Touched};
use super::{reference, state, Cx, Outcome, CHUNK};
use crate::codec::format_time;
use crate::venue::http::{self, HttpRequest, HttpResponse, Transport, TransportError};
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::types::Value as Sql;
use rusqlite::{params, params_from_iter, Connection};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

type R<T> = Result<T, String>;
fn sql(e: rusqlite::Error) -> String { e.to_string() }

/// Fiat.currencies (app/models/fiat.rb): (eodhd external id, symbol, name, color), category 'Currency'.
pub const FIAT: [(&str, &str, &str, &str); 17] = [
    ("USD.FOREX", "USD", "US Dollar", "#355E3B"), ("EUR.FOREX", "EUR", "Euro", "#003087"), ("GBP.FOREX", "GBP", "British Pound", "#4B0082"),
    ("JPY.FOREX", "JPY", "Japanese Yen", "#C1A36F"), ("CHF.FOREX", "CHF", "Swiss Franc", "#D52B1E"), ("CAD.FOREX", "CAD", "Canadian Dollar", "#D80621"),
    ("AUD.FOREX", "AUD", "Australian Dollar", "#3A9C9F"), ("PLN.FOREX", "PLN", "Polish Zloty", "#C0A98E"), ("ARS.FOREX", "ARS", "Argentine Peso", "#75AADB"),
    ("BRL.FOREX", "BRL", "Brazilian Real", "#009C3B"), ("TRY.FOREX", "TRY", "Turkish Lira", "#D45D5D"), ("MXN.FOREX", "MXN", "Mexican Peso", "#6C9A74"),
    ("ZAR.FOREX", "ZAR", "South African Rand", "#F9A825"), ("UAHUSD.FOREX", "UAH", "Ukrainian Hryvnia", "#B4A7D6"),
    ("CZK.FOREX", "CZK", "Czech Koruna", "#D36C6C"), ("RUB.FOREX", "RUB", "Russian Ruble", "#9C9CDE"), ("IDR.FOREX", "IDR", "Indonesian Rupiah", "#E74C3C"),
];

/// Each crypto venue's COINGECKO_ID (app/models/exchanges/*.rb).
pub fn coingecko_id(venue_type: &str) -> Option<&'static str> {
    Some(match venue_type {
        "Exchanges::Binance" => "binance", "Exchanges::BinanceUs" => "binance_us", "Exchanges::Bingx" => "bingx", "Exchanges::Bitget" => "bitget",
        "Exchanges::Bitrue" => "bitrue", "Exchanges::Bitvavo" => "bitvavo", "Exchanges::Bybit" => "bybit_spot", "Exchanges::Coinbase" => "gdax",
        "Exchanges::Gemini" => "gemini", "Exchanges::Hyperliquid" => "hyperliquid-spot", "Exchanges::Kraken" => "kraken", "Exchanges::Kucoin" => "kucoin",
        "Exchanges::Mexc" => "mxc", "Exchanges::Bitmart" => "bitmart",
        _ => return None,
    })
}

/// Exchange::RETIRED_TYPES (exchange.rb:18).
pub const RETIRED: [&str; 1] = ["Exchanges::Bitmart"];
/// Exchanges::Coinbase::ASSET_BLACKLIST and Exchanges::Kraken::ASSET_BLACKLIST.
const COINBASE_BLACKLIST: [&str; 3] = ["RNDR", "ZETACHAIN", "WAXL"];
const KRAKEN_BLACKLIST: [&str; 1] = ["COPM"];

pub const CRAWL_CLAIM: &str = "rust_job.crawl_claim.";
pub const BACKFILL_CLAIM: &str = "rust_job.asset_backfill_claim";

/// Rails' `Rails.cache.write(key, true, expires_in: 1.day, unless_exist: true)`, persisted: taken iff no claim of the last
/// day holds, in one statement (two racing claimers get one claim).
pub fn claim(c: &Connection, key: &str, now: DateTime<Utc>) -> R<bool> {
    let stamp = |t: DateTime<Utc>| t.to_rfc3339_opts(SecondsFormat::Micros, true);
    let n = c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, ?3, ?3) \
                       ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at \
                       WHERE app_configs.value IS NULL OR app_configs.value <= ?4",
                      params![key, stamp(now), format_time(now), stamp(now - Duration::days(1))]).map_err(sql)?;
    Ok(n > 0)
}

/// The symbol → coin id map of set_symbol_to_external_id_hash (synchronizer.rb:78-133), in Rails' hash order (the order
/// new assets are minted in), with D-CG-SYMBOL-AMBIGUOUS in place of first-wins. Also the ambiguous symbols, for the log.
pub fn symbol_map(coingecko_id: &str, crawl: &[Value]) -> (Vec<(String, String)>, Vec<String>) {
    // Every id each symbol was given, symbols in order of first appearance.
    let mut claims: Vec<(String, Vec<String>)> = vec![];
    for t in crawl {
        for (sym_key, id_key) in [("base", "coin_id"), ("target", "target_coin_id")] {
            let symbol = string(&t[sym_key]).unwrap_or_default().to_uppercase();
            let id = if import::truthy(&t[id_key]) { string(&t[id_key]) } else { FIAT.iter().find(|f| f.1 == symbol).map(|f| f.0.to_string()) };
            let Some(id) = id.filter(|i| !i.trim().is_empty()) else { continue };
            match claims.iter_mut().find(|(s, _)| *s == symbol) {
                Some((_, ids)) => if !ids.contains(&id) { ids.push(id) },
                None => claims.push((symbol, vec![id])),
            }
        }
    }
    // translate_coingecko_symbols_to_exchange_symbols: `hash['XDG'] = hash.delete('DOGE')` (an XDG already there keeps its
    // place; else it goes last; no DOGE leaves XDG nil, unresolvable), then the venue's blacklist.
    let blacklist: &[&str] = match coingecko_id {
        "gdax" => &COINBASE_BLACKLIST,
        "kraken" => {
            let doge = claims.iter().position(|(s, _)| s == "DOGE").map(|i| claims.remove(i).1);
            match (claims.iter_mut().find(|(s, _)| s == "XDG"), doge) {
                (Some(xdg), Some(ids)) => xdg.1 = ids,
                (None, Some(ids)) => claims.push(("XDG".into(), ids)),
                (Some(_), None) => claims.retain(|(s, _)| s != "XDG"),
                (None, None) => {}
            }
            &KRAKEN_BLACKLIST
        }
        _ => &[],
    };
    claims.retain(|(s, _)| !blacklist.contains(&s.as_str()));
    let mut symbols_of: HashMap<&str, usize> = HashMap::new();
    for (_, ids) in &claims { for id in ids { *symbols_of.entry(id).or_default() += 1; } }
    let (mut map, mut ambiguous) = (vec![], vec![]);
    for (symbol, ids) in &claims {
        if ids.len() > 1 { ambiguous.push(format!("{symbol} has ids {}", ids.join(", "))); }
        else if symbols_of[ids[0].as_str()] > 1 { ambiguous.push(format!("{symbol}'s id {} is claimed by another symbol", ids[0])); }
        else { map.push((symbol.clone(), ids[0].clone())); }
    }
    (map, ambiguous)
}

/// One write of a catalogue row: its exchange assets made available or created, then its ticker updated or created.
#[derive(Clone, Debug)]
enum Write {
    ExchangeAssetAvailable(i64),
    ExchangeAssetCreate { id: i64, asset: i64 },
    Update { id: i64, cols: Vec<(&'static str, Sql)> },
    Create { id: i64, base_asset: i64, quote_asset: i64, cols: Vec<(&'static str, Sql)> },
}

/// A new asset: (planned id, external id, its Fiat.currencies row when it is one).
type NewAsset = (i64, String, Option<(&'static str, &'static str, &'static str)>);

/// A venue's writes, planned in one read.
#[derive(Default)]
pub struct Plan {
    assets: Vec<NewAsset>,
    rows: Vec<Vec<Write>>,
    sweep_tickers: Vec<i64>,
    sweep_exchange_assets: Vec<(i64, i64)>,
    /// A crypto asset was minted: the backfill wake's condition (create_missing_assets!'s new_crypto_assets).
    pub crypto_created: bool,
}

impl Plan {
    pub fn is_empty(&self) -> bool { self.assets.is_empty() && self.rows.is_empty() && self.sweep_tickers.is_empty() && self.sweep_exchange_assets.is_empty() }
    pub fn units(&self) -> usize {
        let writes: usize = self.rows.iter().map(Vec::len).sum();
        self.assets.len().div_ceil(CHUNK) + writes.div_ceil(CHUNK) + self.sweep_tickers.len().div_ceil(CHUNK) + self.sweep_exchange_assets.len().div_ceil(CHUNK)
    }
}

/// A ticker of the venue as the loop sees it.
struct Held { id: i64, base: String, quote: String, ticker: String, pair: (i64, i64), available: bool }

/// The catalogue's columns, cast as the attributes cast them, in the order the row gives them (`update` assigns only the
/// keys the hash holds).
const STRINGS: [&str; 3] = ["ticker", "base", "quote"];
const DECIMALS: [&str; 4] = ["minimum_base_size", "minimum_quote_size", "maximum_base_size", "maximum_quote_size"];
const INTEGERS: [&str; 3] = ["base_decimals", "quote_decimals", "price_decimals"];
const BOOLEANS: [&str; 2] = ["trading_enabled", "available"];
const NOT_NULL: [&str; 9] = ["ticker", "base", "quote", "minimum_base_size", "minimum_quote_size", "base_decimals", "quote_decimals", "price_decimals", "trading_enabled"];

/// ActiveModel::Type::Boolean#cast: nil and "" are nil; false, 0 and the false strings are false; anything else true.
fn boolean(v: &Value) -> Option<bool> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(*b),
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(!matches!(s.as_str(), "0" | "f" | "F" | "false" | "FALSE" | "off" | "OFF")),
        Value::Number(n) => Some(n.as_f64() != Some(0.0)),
        _ => Some(true),
    }
}

fn columns(row: &Value) -> R<Vec<(&'static str, Sql)>> {
    let Some(obj) = row.as_object() else { return Err(format!("catalogue row {row} is not a hash")) };
    let mut out = vec![];
    for k in STRINGS.iter().chain(&DECIMALS).chain(&INTEGERS).chain(&BOOLEANS) {
        let Some(v) = obj.get(*k) else { continue };
        let cast = if STRINGS.contains(k) { string(v).map(Sql::Text) }
            else if DECIMALS.contains(k) { decimal(v)?.map(Sql::Text) }
            else if INTEGERS.contains(k) { integer(v).map(Sql::Integer) }
            else { boolean(v).map(|b| Sql::Integer(b as i64)) };
        out.push((*k, cast.unwrap_or(Sql::Null)));
    }
    Ok(out)
}

fn col<'a>(cols: &'a [(&'static str, Sql)], name: &str) -> Option<&'a Sql> { cols.iter().find(|(k, _)| *k == name).map(|(_, v)| v) }
fn text_col(cols: &[(&'static str, Sql)], name: &str) -> Option<String> { match col(cols, name) { Some(Sql::Text(s)) => Some(s.clone()), _ => None } }

/// What the venue's catalogue row means for the loop's own view of a ticker it writes.
fn apply(h: &mut Held, cols: &[(&'static str, Sql)]) {
    if let Some(t) = text_col(cols, "ticker") { h.ticker = t; }
    if let Some(b) = text_col(cols, "base") { h.base = b; }
    if let Some(q) = text_col(cols, "quote") { h.quote = q; }
    if let Some(Sql::Integer(a)) = col(cols, "available") { h.available = *a != 0; }
}

/// A NOT NULL column the row would leave null (`create` needs them all; `update` only those it assigns).
fn null_violation(cols: &[(&'static str, Sql)], creating: bool) -> Option<&'static str> {
    NOT_NULL.iter().copied().find(|k| match col(cols, k) { Some(Sql::Null) => true, None => creating && *k != "trading_enabled", _ => false })
}

/// `new_pairs?` (synchronizer.rb:34-37): a catalogue (base, quote) no ticker of the venue has.
pub fn new_pairs(c: &Connection, exchange_id: i64, rows: &[Value]) -> R<bool> {
    let mut s = c.prepare("SELECT base, quote FROM tickers WHERE exchange_id = ?1").map_err(sql)?;
    let known: HashSet<(String, String)> = s.query_map([exchange_id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
    Ok(rows.iter().any(|r| !known.contains(&(string(&r["base"]).unwrap_or_default(), string(&r["quote"]).unwrap_or_default()))))
}

fn next_id(c: &Connection, table: &str) -> R<i64> {
    c.query_row(&format!("SELECT max(coalesce((SELECT seq FROM sqlite_sequence WHERE name = '{table}'), 0), coalesce((SELECT max(id) FROM {table}), 0))"),
                [], |r| r.get(0)).map_err(sql)
}

/// create_missing_assets! then sync_existing_exchange_assets_and_tickers! (synchronizer.rb:135-232), simulated over one read:
/// every write in Rails' order, with the rows the loop's later steps see as Rails' loop sees them.
pub fn plan(c: &Connection, exchange_id: i64, venue: &str, rows: &[Value], map: Option<&[(String, String)]>) -> R<Plan> {
    let mut plan = Plan::default();
    let e = sql;
    // Assets by external id, with the new ones in the map's order.
    let mut assets: HashMap<String, i64> = {
        let mut s = c.prepare("SELECT external_id, id FROM assets").map_err(e)?;
        let all = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(e)?.collect::<Result<_, _>>().map_err(e)?;
        all
    };
    let resolve: HashMap<&str, &str> = map.unwrap_or_default().iter().map(|(s, i)| (s.as_str(), i.as_str())).collect();
    let mut next_asset = next_id(c, "assets")?;
    for (_, ext) in map.unwrap_or_default() {
        if assets.contains_key(ext) { continue; }
        next_asset += 1;
        let fiat = FIAT.iter().find(|f| f.0 == ext).map(|f| (f.1, f.2, f.3));
        plan.crypto_created |= fiat.is_none();
        assets.insert(ext.clone(), next_asset);
        plan.assets.push((next_asset, ext.clone(), fiat));
    }

    let mut held: Vec<Held> = {
        let mut s = c.prepare("SELECT id, base, quote, ticker, base_asset_id, quote_asset_id, available FROM tickers WHERE exchange_id = ?1 ORDER BY id").map_err(e)?;
        let all = s.query_map([exchange_id], |r| Ok(Held { id: r.get(0)?, base: r.get(1)?, quote: r.get(2)?, ticker: r.get(3)?, pair: (r.get(4)?, r.get(5)?),
                                                           available: r.get::<_, Option<bool>>(6)?.unwrap_or(false) }))
            .map_err(e)?.collect::<Result<_, _>>().map_err(e)?;
        all
    };
    // exchange_assets of the venue: asset id → (row id, available).
    let mut listed: HashMap<i64, (i64, bool)> = {
        let mut s = c.prepare("SELECT asset_id, id, available FROM exchange_assets WHERE exchange_id = ?1 ORDER BY id").map_err(e)?;
        let all = s.query_map([exchange_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get::<_, Option<bool>>(2)?.unwrap_or(false))))).map_err(e)?.collect::<Result<_, _>>().map_err(e)?;
        all
    };
    let mut next_ticker = next_id(c, "tickers")?;
    let mut next_listing = next_id(c, "exchange_assets")?;

    let current: Vec<String> = held.iter().filter(|h| h.available).map(|h| h.ticker.clone()).collect();
    let reported: HashSet<(String, String)> = rows.iter().map(|r| (string(&r["base"]).unwrap_or_default(), string(&r["quote"]).unwrap_or_default())).collect();
    let mut confirmed: Vec<String> = vec![];
    let (mut unresolved, mut refused) = (vec![], vec![]);
    // A row the database refuses: Rails' sync raises out of the venue there and never reaches the sweep (D-CG-ROW-REFUSED).
    let mut aborted = false;
    for row in rows {
        let (base, quote) = (string(&row["base"]).unwrap_or_default(), string(&row["quote"]).unwrap_or_default());
        let cols = match columns(row) { Ok(c) => c, Err(m) => { refused.push(m); aborted = true; continue } };
        // A ticker name another ticker of the venue holds: the unique index refuses it (D-CG-ROW-REFUSED).
        let name_taken = |held: &[Held], id: Option<i64>| text_col(&cols, "ticker").is_some_and(|t| held.iter().any(|h| h.ticker == t && Some(h.id) != id));
        if let Some(i) = held.iter().position(|h| h.base == base && h.quote == quote) {
            let id = held[i].id;
            // exchange_matches_assets: a ticker whose assets the venue does not list fails validation; `update` returns
            // false and the name it was assigned still counts as confirmed.
            if !(listed.contains_key(&held[i].pair.0) && listed.contains_key(&held[i].pair.1)) {
                confirmed.push(text_col(&cols, "ticker").unwrap_or_else(|| held[i].ticker.clone()));
                continue;
            }
            if name_taken(&held, Some(id)) || null_violation(&cols, false).is_some() {
                refused.push(format!("{base}/{quote}"));
                aborted = true;
                continue;
            }
            apply(&mut held[i], &cols);
            // `updated_tickers << ticker.ticker`: the name as assigned, written or not.
            confirmed.push(held[i].ticker.clone());
            plan.rows.push(vec![Write::Update { id, cols }]);
            continue;
        }
        let ids = (resolve.get(base.to_uppercase().as_str()), resolve.get(quote.to_uppercase().as_str()));
        let (Some(b), Some(q)) = ids else { unresolved.push(format!("{base}/{quote}")); continue };
        let (Some(&base_asset), Some(&quote_asset)) = (assets.get(*b), assets.get(*q)) else { continue };
        let mut writes = vec![];
        for asset in [base_asset, quote_asset] {
            match listed.get_mut(&asset) {
                Some((id, available)) => { if !*available { *available = true; writes.push(Write::ExchangeAssetAvailable(*id)); } }
                None => { next_listing += 1; listed.insert(asset, (next_listing, true)); writes.push(Write::ExchangeAssetCreate { id: next_listing, asset }); }
            }
        }
        let renamed = held.iter().position(|h| h.pair == (base_asset, quote_asset));
        if let Some(i) = renamed.filter(|&i| !reported.contains(&(held[i].base.clone(), held[i].quote.clone()))) {
            // Coinbase RNDR -> RENDER: the ticker holding these assets under a spelling the venue no longer reports takes
            // the new one in place.
            let id = held[i].id;
            if name_taken(&held, Some(id)) || null_violation(&cols, false).is_some() {
                refused.push(format!("{base}/{quote}"));
                aborted = true;
            } else {
                apply(&mut held[i], &cols);
                confirmed.push(held[i].ticker.clone());
                writes.push(Write::Update { id, cols });
            }
        } else if renamed.is_some() {
            // Both spellings live: the (exchange, base asset, quote asset) validation refuses it, and the sync goes on.
            refused.push(format!("{base}/{quote}"));
        } else if name_taken(&held, None) || null_violation(&cols, true).is_some() {
            refused.push(format!("{base}/{quote}"));
            aborted = true;
        } else {
            next_ticker += 1;
            let mut h = Held { id: next_ticker, base: String::new(), quote: String::new(), ticker: String::new(), pair: (base_asset, quote_asset), available: true };
            apply(&mut h, &cols);
            confirmed.push(h.ticker.clone());
            held.push(h);
            writes.push(Write::Create { id: next_ticker, base_asset, quote_asset, cols });
        }
        if !writes.is_empty() { plan.rows.push(writes); }
    }
    let sample = |v: &[String]| format!("{} ({}{})", v.len(), v.iter().take(10).cloned().collect::<Vec<_>>().join(", "), if v.len() > 10 { ", …" } else { "" });
    if !unresolved.is_empty() { crate::engine::log(&format!("[Sync] {venue}: {} pairs unresolved, deferred to the next market-data pull", sample(&unresolved))); }
    if !refused.is_empty() { crate::engine::log(&format!("[Sync] {venue}: skipped {} refused rows", sample(&refused))); }

    // The sweep, only when the catalogue confirmed some tickers and not every current one went missing, and no row was
    // refused by the database.
    let confirmed: HashSet<String> = confirmed.into_iter().collect();
    let stale: HashSet<&String> = current.iter().filter(|t| !confirmed.contains(*t)).collect();
    if !aborted && !confirmed.is_empty() && stale.len() < current.len() {
        for h in held.iter_mut().filter(|h| h.available && stale.contains(&h.ticker)) { h.available = false; plan.sweep_tickers.push(h.id); }
        let used: HashSet<i64> = held.iter().filter(|h| h.available).flat_map(|h| [h.pair.0, h.pair.1]).collect();
        let mut gone: Vec<(i64, i64)> = listed.iter().filter_map(|(asset, (id, available))| (*available && !used.contains(asset)).then_some((*id, *asset))).collect();
        gone.sort();
        plan.sweep_exchange_assets = gone;
    }
    Ok(plan)
}

/// Publishes a venue's plan: assets (`publish_assets`), then the rows (a row's writes never split), then the sweep, each in
/// units of CHUNK.
pub async fn publish(db: &super::Db, exchange_id: i64, plan: Plan, now: DateTime<Utc>) -> R<()> {
    let plan = publish_assets(db, plan, now).await?;
    publish_rows(db, exchange_id, plan, now).await
}

/// The plan's new assets, committed first (create_missing_assets! runs before the loop); the plan without them.
pub async fn publish_assets(db: &super::Db, mut plan: Plan, now: DateTime<Utc>) -> R<Plan> {
    let t1 = format_time(now);
    import::publish(db, "venue assets", import::chunks(&std::mem::take(&mut plan.assets)), move |c, assets: Vec<NewAsset>| {
        for (id, ext, fiat) in &assets {
            match fiat {
                Some((symbol, name, color)) => c.prepare_cached("INSERT INTO assets (id, external_id, symbol, name, category, color, created_at, updated_at) \
                                                                 VALUES (?1, ?2, ?3, ?4, 'Currency', ?5, ?6, ?6)").map_err(sql)?
                    .execute(params![id, ext, symbol, name, color, t1]).map_err(sql)?,
                None => c.prepare_cached("INSERT INTO assets (id, external_id, category, created_at, updated_at) VALUES (?1, ?2, 'Cryptocurrency', ?3, ?3)").map_err(sql)?
                    .execute(params![id, ext, t1]).map_err(sql)?,
            };
        }
        Ok(Touched::None)
    }).await?;
    Ok(plan)
}

/// The rows, then the sweep.
pub async fn publish_rows(db: &super::Db, exchange_id: i64, plan: Plan, now: DateTime<Utc>) -> R<()> {
    let t = format_time(now);
    let mut units: Vec<Vec<Write>> = vec![];
    for row in plan.rows {
        if units.last().is_none_or(|u| u.len() + row.len() > CHUNK) { units.push(vec![]); }
        units.last_mut().expect("pushed").extend(row);
    }
    let t2 = t.clone();
    import::publish(db, "venue tickers", units, move |c, writes: Vec<Write>| {
        let mut touched = vec![];
        for w in writes {
            match w {
                Write::ExchangeAssetAvailable(id) => { c.execute("UPDATE exchange_assets SET available = 1, updated_at = ?2 WHERE id = ?1", params![id, t2]).map_err(sql)?; }
                Write::ExchangeAssetCreate { id, asset } => {
                    c.execute("INSERT INTO exchange_assets (id, asset_id, exchange_id, available, created_at, updated_at) VALUES (?1, ?2, ?3, 1, ?4, ?4)",
                              params![id, asset, exchange_id, t2]).map_err(sql)?;
                }
                Write::Update { cols, .. } if cols.is_empty() => {}
                Write::Update { id, cols } => {
                    // `update`: the changed attributes, and updated_at only when one changed.
                    let set: Vec<String> = cols.iter().enumerate().map(|(i, (k, _))| format!("{k} = ?{}", i + 3)).collect();
                    let same: Vec<String> = cols.iter().enumerate().map(|(i, (k, _))| format!("{k} IS ?{}", i + 3)).collect();
                    let statement = format!("UPDATE tickers SET {}, updated_at = ?2 WHERE id = ?1 AND NOT ({})", set.join(", "), same.join(" AND "));
                    let binds = [Sql::Integer(id), Sql::Text(t2.clone())].into_iter().chain(cols.into_iter().map(|(_, v)| v));
                    if c.execute(&statement, params_from_iter(binds)).map_err(sql)? > 0 { touched.push(id); }
                }
                Write::Create { id, base_asset, quote_asset, cols } => {
                    let names: Vec<&str> = cols.iter().map(|(k, _)| *k).collect();
                    let marks: Vec<String> = (0..cols.len()).map(|i| format!("?{}", i + 6)).collect();
                    let statement = format!("INSERT INTO tickers (id, exchange_id, base_asset_id, quote_asset_id, created_at, updated_at, {}) VALUES (?1, ?2, ?3, ?4, ?5, ?5, {})",
                                            names.join(", "), marks.join(", "));
                    let binds = [Sql::Integer(id), Sql::Integer(exchange_id), Sql::Integer(base_asset), Sql::Integer(quote_asset), Sql::Text(t2.clone())]
                        .into_iter().chain(cols.into_iter().map(|(_, v)| v));
                    c.execute(&statement, params_from_iter(binds)).map_err(sql)?;
                    touched.push(id);
                }
            }
        }
        Ok(Touched::Tickers(touched))
    }).await?;
    // update_all: updated_at stays.
    import::publish(db, "venue sweep", import::chunks(&plan.sweep_tickers), |c, ids: Vec<i64>| {
        c.execute(&format!("UPDATE tickers SET available = 0 WHERE id IN ({})", vec!["?"; ids.len()].join(", ")), params_from_iter(&ids)).map_err(sql)?;
        Ok(Touched::Tickers(ids))
    }).await?;
    import::publish(db, "venue sweep", import::chunks(&plan.sweep_exchange_assets), |c, rows: Vec<(i64, i64)>| {
        let ids: Vec<i64> = rows.iter().map(|r| r.0).collect();
        c.execute(&format!("UPDATE exchange_assets SET available = 0 WHERE id IN ({})", vec!["?"; ids.len()].join(", ")), params_from_iter(&ids)).map_err(sql)?;
        Ok(Touched::Assets(rows.into_iter().map(|r| r.1).collect()))
    }).await
}

/// A venue's catalogue (Exchange#get_tickers_info(force: true)): Rails' rows, or why it failed.
pub type CatalogueFuture = Pin<Box<dyn Future<Output = R<Vec<Value>>>>>;
/// The catalogues a run reads, by venue type and the venue's exchange proxy: live in production; a test hands scripted ones.
pub type Catalogues = Rc<dyn Fn(&str, Option<String>) -> CatalogueFuture>;

/// The live catalogues: each venue's public endpoints over an unsigned transport (venue::catalogue), through its proxy.
pub fn live_catalogues() -> Catalogues {
    Rc::new(|ty: &str, proxy: Option<String>| {
        let ty = ty.to_string();
        Box::pin(async move { crate::venue::catalogue::read(&ty, &PublicTransport::new(proxy.as_deref())?).await })
    })
}

/// ExchangeProxy.for(name_id), as the venue's own client takes it: AppConfig `proxy_<name>`, else ENV `PROXY_<NAME>`.
pub fn catalogue_proxy(c: &Connection, cipher: &crate::crypto::Cipher, venue_type: &str, env: &dyn Fn(&str) -> Option<String>) -> R<Option<String>> {
    let name = reference::name_id(venue_type);
    let stored = crate::app_config::get(c, cipher, &format!("proxy_{name}"))?;
    Ok(http::proxy_for(&name, stored, env))
}

/// Public venue endpoints: no key, nothing signed; JSON bodies; bounded answers; through the venue's proxy when it has one
/// (an unusable proxy URL sends nothing).
pub struct PublicTransport { client: reqwest::Client }

impl PublicTransport {
    pub fn new(proxy: Option<&str>) -> R<Self> {
        Ok(Self { client: match proxy { Some(p) => http::client_via(p)?, None => http::client() } })
    }
}

impl Transport for PublicTransport {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> { self.send_limited(r, super::data_api::MAX_REFERENCE_BODY).await }
    async fn send_limited(&self, r: &HttpRequest, limit: usize) -> Result<HttpResponse, TransportError> {
        let mut url = reqwest::Url::parse(&format!("{}{}", r.base, r.path)).map_err(|e| TransportError::Permanent(e.to_string()))?;
        if !r.query.is_empty() { url.query_pairs_mut().extend_pairs(&r.query); }
        let mut b = if r.method == "POST" { self.client.post(url) } else { self.client.get(url) };
        if let Some(body) = &r.body { b = b.header("Content-Type", "application/json").body(body.to_string()); }
        let resp = b.header("Accept", "application/json").send().await.map_err(http::classify)?;
        http::read_limited(resp, limit.min(super::data_api::MAX_REFERENCE_BODY)).await
    }
}

/// The run's venues: `Exchange.available.where.not(type: STOCK_TYPES)` in id order, without the retired ones
/// (D-CG-RETIRED-NOT-ASKED).
fn venues(c: &Connection) -> R<Vec<(i64, String)>> {
    let mut s = c.prepare("SELECT id, type FROM exchanges WHERE available = 1 AND type NOT IN ('Exchanges::Alpaca', 'Exchanges::Ibkr', 'Exchanges::Bitmart') ORDER BY id")
        .map_err(sql)?;
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?.collect::<Result<Vec<_>, _>>().map_err(sql);
    rows
}

/// The coingecko provider's key as the ticker sync reads it: None when not configured or not on coingecko (`return
/// unless MarketData.configured?`); a blank key Some("").
fn provider_key(c: &Connection, cipher: &crate::crypto::Cipher) -> R<Option<String>> {
    crate::engine::provider::bind_cipher(c, cipher).map_err(|e| format!("{e:?}"))?;
    let s = crate::engine::provider::settings(c).map_err(|e| format!("{e:?}"))?;
    Ok(match (s.configured, &s.provider) {
        (true, crate::market_data::Provider::CoinGecko { key }) => Some(key.clone().unwrap_or_default()),
        _ => None,
    })
}

/// Exchange::SyncAllTickersAndAssetsJob on the coingecko provider: the synchronizer for each venue, in id order.
/// `setup` is Setup::SeedAndSyncJob's call (force: true, skip_async_jobs: true): no claim, no backfill wake; and a blank key
/// still applies the catalogues, where the scheduled job (sync_tickers_from_coingecko!) fails before them.
/// Outcome: RateLimited if a crawl's 429 ended discovery; else Failed listing the venues whose catalogue or write failed;
/// else Done if any venue's rows were written; else NothingNew.
pub async fn run(cg: &Clients, catalogues: &Catalogues, cx: &Cx<'_>, setup: bool) -> Outcome {
    let read = cx.db.run(|c, cipher| Ok((provider_key(c, cipher)?, venues(c)?))).await;
    let (key, venues) = match read { Ok((Some(k), v)) => (k, v), Ok((None, _)) => return Outcome::NothingNew, Err(m) => return Outcome::Failed(m) };
    if key.trim().is_empty() && !setup { return Outcome::Failed("CoinGecko API key not configured".into()); }
    let client = cg(&key);
    let (mut limited, mut failed, mut written, mut marked) = (None::<String>, vec![], false, false);
    for (id, ty) in venues {
        let name = reference::name_id(&ty);
        let t = ty.clone();
        let proxy = match cx.db.run(move |c, cipher| catalogue_proxy(c, cipher, &t, &|k| std::env::var(k).ok())).await { Ok(p) => p, Err(m) => { failed.push(format!("{name}: {m}")); continue } };
        let rows = match catalogues(&ty, proxy).await { Ok(r) => r, Err(m) => { failed.push(format!("{name}: {m}")); continue } };
        let Some(cg_id) = coingecko_id(&ty) else { failed.push(format!("{name}: no CoinGecko id")); continue };
        let now = cx.clock.now();
        let rows = std::sync::Arc::new(rows);
        let (r2, claim_key) = (rows.clone(), format!("{CRAWL_CLAIM}{cg_id}"));
        // new_pairs?, then (force || claim): the claim only when a crawl would follow, and none once a 429 closed the quota.
        let crawl_now = limited.is_none();
        let wanted = cx.db.run(move |c, _| Ok(new_pairs(c, id, &r2)? && crawl_now && (setup || claim(c, &claim_key, now)?))).await;
        let map = match wanted {
            Err(m) => { failed.push(format!("{name}: {m}")); continue }
            Ok(false) => None,
            Ok(true) => match coingecko::crawl(&*client, &key, cg_id).await {
                Ok(tickers) => {
                    let (map, ambiguous) = symbol_map(cg_id, &tickers);
                    if !ambiguous.is_empty() { crate::engine::log(&format!("[Sync] {name}: ambiguous CoinGecko symbols left unresolved (D-CG-SYMBOL-AMBIGUOUS): {}", ambiguous.join("; "))); }
                    Some(map)
                }
                Err(CrawlError::RateLimited(m)) => {
                    crate::engine::log(&format!("[Sync] {name}: market data rate limited, no further crawls this run: {m}"));
                    limited = Some(format!("{name}: {m}"));
                    None
                }
                Err(CrawlError::Failed(m)) => { crate::engine::log(&format!("[Sync] {name}: market data unavailable, new pairs deferred: {m}")); None }
            },
        };
        let label = name.clone();
        let planned = cx.db.run(move |c, _| plan(c, id, &label, &rows, map.as_deref())).await;
        let plan = match planned { Ok(p) => p, Err(m) => { failed.push(format!("{name}: {m}")); continue } };
        if plan.is_empty() { continue; }
        let crypto = plan.crypto_created;
        if !marked {
            if let Err(m) = cx.db.run(move |c, _| state::mark_incomplete(c, reference::TICKERS, None, now)).await { return Outcome::Failed(m); }
            marked = true;
        }
        let plan = match publish_assets(&cx.db, plan, now).await { Ok(p) => p, Err(m) => { failed.push(format!("{name}: {m}")); continue } };
        // The bulk asset backfill, at most once a day however many venues minted coins (synchronizer.rb:156-162): claimed
        // and woken as soon as the coins are committed, so a ticker unit failing after them never strands them unfilled.
        if crypto && !setup {
            match cx.db.run(move |c, _| claim(c, BACKFILL_CLAIM, now)).await {
                Ok(true) => cx.wakers.wake(reference::ASSETS, None, None),
                Ok(false) => {}
                Err(m) => crate::engine::log(&format!("[Sync] the asset backfill claim failed: {m}")),
            }
        }
        if let Err(m) = publish_rows(&cx.db, id, plan, now).await { failed.push(format!("{name}: {m}")); continue; }
        written = true;
    }
    match (limited, failed.is_empty()) {
        (Some(m), _) => Outcome::RateLimited(std::iter::once(m).chain(failed).collect::<Vec<_>>().join("; ")),
        (None, false) => Outcome::Failed(failed.join("; ")),
        (None, true) if written => Outcome::Done,
        (None, true) => Outcome::NothingNew,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn t(base: &str, coin: Option<&str>, target: &str, target_coin: Option<&str>) -> Value {
        json!({ "base": base, "coin_id": coin, "target": target, "target_coin_id": target_coin })
    }

    #[test]
    fn the_map_keeps_rails_order_maps_fiat_and_refuses_ambiguity() {
        let crawl = [t("btc", Some("bitcoin"), "USD", None), t("ETH", Some("ethereum"), "EUR", None), t("doge", Some("dogecoin"), "usd", None),
                     t("XDG", Some("other-xdg"), "USD", None), t("COPM", Some("ecomi"), "USD", None),
                     t("SAME", Some("a"), "USD", None), t("SAME", Some("b"), "USD", None),
                     t("ONE", Some("shared"), "USD", None), t("TWO", Some("shared"), "USD", None)];
        let (map, ambiguous) = symbol_map("kraken", &crawl);
        let syms: Vec<(&str, &str)> = map.iter().map(|(s, i)| (s.as_str(), i.as_str())).collect();
        assert_eq!(syms, [("BTC", "bitcoin"), ("USD", "USD.FOREX"), ("ETH", "ethereum"), ("EUR", "EUR.FOREX"), ("XDG", "dogecoin")]);
        assert_eq!(ambiguous.len(), 3, "{ambiguous:?}");
        // Coinbase's blacklist: RNDR is dropped before RENDER and RNDR could collide on one id.
        let (map, ambiguous) = symbol_map("gdax", &[t("RNDR", Some("render-token"), "USD", None), t("RENDER", Some("render-token"), "USD", None)]);
        assert!(ambiguous.is_empty());
        assert!(map.contains(&("RENDER".to_string(), "render-token".to_string())) && !map.iter().any(|(s, _)| s == "RNDR"));
        // No DOGE on Kraken: XDG is unresolvable, as Rails' nil.
        assert!(symbol_map("kraken", &[t("XDG", Some("x"), "USD", None)]).0.iter().all(|(s, _)| s != "XDG"));
    }

    #[test]
    fn a_claim_holds_a_day_and_one_of_two_claimers_takes_it() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE app_configs (id INTEGER PRIMARY KEY AUTOINCREMENT, key TEXT NOT NULL UNIQUE, value TEXT, created_at TEXT NOT NULL, updated_at TEXT NOT NULL)").unwrap();
        let now: DateTime<Utc> = "2026-10-02T10:00:00Z".parse().unwrap();
        assert!(claim(&c, "k", now).unwrap());
        assert!(!claim(&c, "k", now).unwrap());
        assert!(!claim(&c, "k", now + Duration::hours(23)).unwrap());
        assert!(claim(&c, "k", now + Duration::days(1)).unwrap());
    }
}
