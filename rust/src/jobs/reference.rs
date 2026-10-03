//! The reference-data jobs of config/recurring.yml, each a port of its Rails job on the deltabadger market-data provider.
//! HTTP is awaited on the runtime thread. Everything after it (the walk of the payload, the casts, the writes) runs in a
//! `Db::run` closure on the blocking pool, and the writes in units of CHUNK rows (import::publish).
use super::data_api::{ApiError, DataApi};
use super::import::{self, present, truthy};
use super::schedule::{Jitter, Schedule};
use super::{Cx, Job, JobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use crate::app_config;
use crate::codec::format_time;
use crate::venue::http::Transport;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::rc::Rc;

pub const STOCKS: &str = "sync_stocks_from_deltabadger_job";
pub const ALPACA_CRYPTO: &str = "sync_alpaca_crypto_from_deltabadger_job";
pub const INDICES: &str = "sync_indices_from_coingecko_job";
pub const TICKERS: &str = "sync_all_tickers_and_assets_job";
pub const ASSETS: &str = "fetch_all_assets_data_from_coingecko_job";
pub const PRUNE: &str = "prune_bot_activity_logs_job";

const NOT_PORTED: &str = "the market-data provider is not deltabadger (MARKET_DATA_URL): this build syncs reference data only from data-api";
const ALPACA_LISTINGS_LAST_GOOD: &str = "alpaca_listings_last_good_count";        // market_data.rb:398
const MIN_HEALTHY_ALPACA_LISTINGS: i64 = 1000;                                    // :401
const ALPACA_CRYPTO_LAST_GOOD: &str = "alpaca_crypto_listings_last_good_count";   // :589
const MIN_HEALTHY_ALPACA_CRYPTO: i64 = 30;                                        // :590
const BACKFILL_FLAG: &str = "stock_canonical_backfill_completed_at";              // :391

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Stocks, AlpacaCrypto, Indices, Tickers, Assets, Prune }
/// Registration order: ties in due time run in this order (the 10:00 → 10:15 → 10:30 chain, recurring.yml:25-35).
const KINDS: [Kind; 6] = [Kind::Stocks, Kind::AlpacaCrypto, Kind::Indices, Kind::Tickers, Kind::Assets, Kind::Prune];

/// config/recurring.yml's production entries, with each job's jitter and retry_on.
fn spec(kind: Kind) -> Spec {
    let daily = |hour, minute| Some(Schedule::Daily { hour, minute });
    match kind {
        // :53-56; jitter 1 s–15 min (sync_stocks_from_deltabadger_job.rb:28-36, :100-102); retry_on ×2, 5 attempts (:17-18).
        Kind::Stocks => Spec { name: STOCKS, schedule: daily(10, 0), jitter: Jitter { min_secs: 1, max_secs: 900 }, retry: Retry::Polynomial { attempts: 5 }, deadline: DEADLINE, scope: None },
        // :58-60; retry_on ×2, 5 attempts (sync_alpaca_crypto_from_deltabadger_job.rb:5-6).
        Kind::AlpacaCrypto => Spec { name: ALPACA_CRYPTO, schedule: daily(10, 15), jitter: Jitter::NONE, retry: Retry::Polynomial { attempts: 5 }, deadline: DEADLINE, scope: None },
        // :33-35; PullFailed, 15 minutes, 4 attempts (sync_from_coingecko_job.rb:15-17).
        Kind::Indices => Spec { name: INDICES, schedule: daily(10, 30), jitter: Jitter::NONE, retry: Retry::Fixed { wait_secs: 900, attempts: 4 }, deadline: DEADLINE, scope: None },
        // :13-15; `rand(60)` s on the deltabadger provider (sync_all_tickers_and_assets_job.rb:9, :14).
        Kind::Tickers => Spec { name: TICKERS, schedule: Some(Schedule::EveryHours { every: 4, minute: 15 }), jitter: Jitter { min_secs: 0, max_secs: 59 }, retry: Retry::None, deadline: DEADLINE, scope: None },
        // :20-23; 1 s–5 min (fetch_all_assets_data_from_coingecko_job.rb:17, :39).
        Kind::Assets => Spec { name: ASSETS, schedule: daily(0, 20), jitter: Jitter { min_secs: 1, max_secs: 300 }, retry: Retry::None, deadline: DEADLINE, scope: None },
        // :89-91.
        Kind::Prune => Spec { name: PRUNE, schedule: daily(4, 0), jitter: Jitter::NONE, retry: Retry::None, deadline: DEADLINE, scope: None },
    }
}

/// Every job's spec, in registration order.
pub fn specs() -> Vec<Spec> { KINDS.iter().map(|k| spec(*k)).collect() }

struct Reference<T: Transport> { kind: Kind, api: Rc<Option<DataApi<T>>> }

impl<T: Transport + 'static> Job for Reference<T> {
    fn spec(&self) -> Spec { spec(self.kind) }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> { Box::pin(run(self.kind, Option::as_ref(&self.api), cx)) }
}

/// The six jobs, sharing one client. `api` None: the install is not on the deltabadger provider.
pub fn jobs<T: Transport + 'static>(api: Option<DataApi<T>>) -> Vec<Box<dyn Job>> {
    let api = Rc::new(api);
    KINDS.iter().map(|k| Box::new(Reference { kind: *k, api: api.clone() }) as Box<dyn Job>).collect()
}

/// One run of the job named `name`, as the scheduler runs it (the parity harness).
pub async fn run_once<T: Transport>(name: &str, api: Option<DataApi<T>>, cx: Cx<'_>) -> Outcome {
    match KINDS.iter().find(|k| spec(**k).name == name) {
        Some(k) => run(*k, api.as_ref(), cx).await,
        None => Outcome::Failed(format!("no job named {name}")),
    }
}

async fn run<T: Transport>(kind: Kind, api: Option<&DataApi<T>>, cx: Cx<'_>) -> Outcome {
    if kind == Kind::Prune { return prune(&cx).await; }
    let Some(api) = api else { return Outcome::Failed(NOT_PORTED.into()) };
    match kind {
        Kind::Stocks => stocks(api, &cx).await,
        Kind::AlpacaCrypto => alpaca_crypto(api, &cx).await,
        Kind::Indices => indices(api, &cx).await,
        Kind::Tickers => tickers(api, &cx).await,
        Kind::Assets => assets(api, &cx).await,
        Kind::Prune => unreachable!("handled above"),
    }
}

/// What the stock and Alpaca syncs do with a failed request (market_data.rb:430-431, :456-459): a 429 raises
/// RateLimitedError and a network failure TransientNetworkError, which retry_on retries; any other is a logged Failure.
fn api_failure(e: ApiError) -> Outcome {
    if e.rate_limited() { return Outcome::RateLimited(e.message()); }
    match e { ApiError::Transient(m) => Outcome::Transient(m), other => Outcome::Failed(other.message()) }
}


/// `result.data['data']`: a body that is not an object raises NoMethodError, which every sync rescues into a Failure.
fn take_data(body: Value) -> Result<Value, String> {
    match body {
        Value::Object(mut m) => Ok(m.remove("data").unwrap_or(Value::Null)),
        other => Err(format!("undefined method '[]' for {other}")),
    }
}

/// `Array(result.data && result.data['data'])`. ponytail: a `data` that is neither a list nor nil reads as empty; data-api
/// always sends a list.
fn take_list(body: Value) -> Vec<Value> {
    match take_data(body) { Ok(Value::Array(a)) => a, _ => vec![] }
}

/// `Exchanges::Alpaca.first`.
fn alpaca_id(c: &Connection) -> Result<Option<i64>, String> {
    c.query_row("SELECT id FROM exchanges WHERE type = 'Exchanges::Alpaca' ORDER BY id LIMIT 1", [], |r| r.get(0)).optional().map_err(|e| e.to_string())
}

/// `String#to_i` of a stored count: leading digits, else 0.
fn ruby_to_i(s: Option<&str>) -> i64 {
    let t = s.unwrap_or("").trim_start();
    t[..t.chars().take_while(char::is_ascii_digit).count()].parse().unwrap_or(0)
}

/// alpaca_listings_degraded? (:411-417): empty is always degraded; with a baseline, under 90 % of it (integer division);
/// without one, under 1,000.
fn stocks_degraded(resolved: i64, last_good: Option<&str>) -> bool {
    if resolved <= 0 { return true; }
    let baseline = ruby_to_i(last_good);
    resolved < if baseline > 0 { baseline * 9 / 10 } else { MIN_HEALTHY_ALPACA_LISTINGS }
}

/// alpaca_crypto_listings_degraded? (:596-606): empty is always degraded; else under max(30, ⌈90 % of the baseline⌉).
fn crypto_degraded(resolved: i64, last_good: Option<&str>) -> bool {
    if resolved <= 0 { return true; }
    let baseline = ruby_to_i(last_good);
    resolved < if baseline > 0 { MIN_HEALTHY_ALPACA_CRYPTO.max((baseline as f64 * 9.0 / 10.0).ceil() as i64) } else { MIN_HEALTHY_ALPACA_CRYPTO }
}

fn degraded(resolved: i64, rows: usize, last_good: Option<&str>) -> Outcome {
    Outcome::Failed(format!("degraded listings payload: {resolved} resolved of {rows} rows (last good {}); nothing imported, availability kept",
                            last_good.unwrap_or("none")))
}

/// Asset::FetchAllAssetsDataFromCoingeckoJob → MarketData.sync_assets! → sync_assets_from_deltabadger!.
async fn assets<T: Transport>(api: &DataApi<T>, cx: &Cx<'_>) -> Outcome {
    let body = match api.assets().await { Ok(b) => b, Err(e) => return Outcome::Failed(e.message()) };
    let (url, now) = (api.public_url(), cx.clock.now());
    let plan = cx.db.run(move |_, _| {
        let rows = take_data(body)?;
        if !present(&rows) { return Ok(None); } // `return if assets_data.blank?`
        let rows = rows.as_array().ok_or_else(|| "the assets payload's data is not a list".to_string())?;
        import::plan_assets(rows, &url).map(Some)
    }).await;
    match plan {
        Ok(None) => Outcome::NothingNew, // Rails writes nothing; the catalogue keeps its age
        Ok(Some(plan)) => {
            if let Err(m) = incomplete(cx, ASSETS, now).await { return Outcome::Failed(m); }
            done(import::import_assets(&cx.db, plan, now).await)
        }
        Err(m) => Outcome::Failed(m),
    }
}

/// Ok: Done; Err: the failure Rails rescues into a logged Result::Failure.
fn done(r: Result<(), String>) -> Outcome { r.map_or_else(Outcome::Failed, |()| Outcome::Done) }

/// Before an import's first write unit: until the runner records this job's complete success, the
/// stamps this run leaves on the source's rows may be partial, and engine::staleness does not read them.
async fn incomplete(cx: &Cx<'_>, job: &'static str, now: DateTime<Utc>) -> Result<(), String> {
    cx.db.run(move |c, _| super::state::mark_incomplete(c, job, None, now)).await
}

/// `Exchange.available.where.not(type: Exchange::STOCK_TYPES)` (exchange.rb:23, :51), in id order.
fn venues(c: &Connection) -> Result<Vec<(i64, String)>, String> {
    let mut s = c.prepare("SELECT id, type FROM exchanges WHERE available = 1 AND type NOT IN ('Exchanges::Alpaca', 'Exchanges::Ibkr') ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok(rows)
}

/// Exchange#name_id (exchange.rb:77-79): the class name, demodulized and underscored ("Exchanges::BinanceUs" → "binance_us").
/// ponytail: one underscore per capital; Ruby groups acronyms ("ABCDef" → "abc_def"), which no exchange class has.
pub fn name_id(ty: &str) -> String {
    let short = ty.rsplit("::").next().unwrap_or(ty);
    let mut out = String::new();
    for (i, ch) in short.chars().enumerate() {
        if ch.is_ascii_uppercase() { if i > 0 { out.push('_'); } out.push(ch.to_ascii_lowercase()); } else { out.push(ch); }
    }
    out
}

/// Exchange::SyncAllTickersAndAssetsJob → Exchange::SyncTickersAndAssetsJob per venue → sync_tickers_from_deltabadger!.
async fn tickers<T: Transport>(api: &DataApi<T>, cx: &Cx<'_>) -> Outcome {
    let venues = match cx.db.run(|c, _| venues(c)).await { Ok(v) => v, Err(m) => return Outcome::Failed(m) };
    let (mut failed, mut refreshed) = (vec![], false);
    for (id, ty) in venues {
        let name = name_id(&ty);
        let result = match api.tickers(&name).await {
            Err(e) => Err(e.message()),
            Ok(body) => {
                let now = cx.clock.now();
                match cx.db.run(move |c, _| import::plan_tickers(c, id, take_data(body)?.as_array().map_or(&[][..], |a| a.as_slice()), None)).await {
                    Ok(plan) if plan.units.is_empty() && plan.exchange_assets.is_empty() => Ok(()),
                    Ok(plan) => {
                        refreshed |= !plan.units.is_empty();
                        match incomplete(cx, TICKERS, now).await {
                            Ok(()) => import::publish_tickers(&cx.db, id, plan, now).await.map(|_| ()),
                            Err(m) => Err(m),
                        }
                    }
                    Err(m) => Err(m),
                }
            }
        };
        if let Err(m) = result { failed.push(format!("{name}: {m}")); }
    }
    if failed.is_empty() && !refreshed { return Outcome::NothingNew; }
    if failed.is_empty() { Outcome::Done } else { Outcome::Failed(failed.join("; ")) }
}

/// Index::SyncFromCoingeckoJob's deltabadger branch → sync_indices_from_deltabadger!. Any failure is PullFailed, retried.
/// `recheck_index_bots` (:151-155) is left to the engine.
async fn indices<T: Transport>(api: &DataApi<T>, cx: &Cx<'_>) -> Outcome {
    let body = match api.indices().await { Ok(b) => b, Err(e) => return Outcome::Transient(e.message()) };
    let now = cx.clock.now();
    let plan = cx.db.run(move |_, _| {
        let rows = take_data(body)?;
        if !present(&rows) { return Ok(vec![]); } // `return if indices_data.blank?`
        import::plan_indices(rows.as_array().ok_or_else(|| "the indices payload's data is not a list".to_string())?)
    }).await;
    let result = match plan {
        Ok(rows) if rows.is_empty() => return Outcome::NothingNew, // Rails writes nothing; the indices keep their age
        Ok(rows) => match incomplete(cx, INDICES, now).await { Ok(()) => import::import_indices(&cx.db, rows, now).await, Err(m) => Err(m) },
        Err(m) => Err(m),
    };
    match result { Ok(()) => Outcome::Done, Err(m) => Outcome::Transient(m) }
}

/// ticker_data_from_listing_row (:677-704): the base from the row's public id ("crypto:bitcoin" → "bitcoin"), the pair from
/// its symbol, the quote always the local `usd`.
fn crypto_listing(row: &Value) -> Option<Value> {
    let raw = match &row["base_asset_id"] { Value::Null => String::new(), Value::String(s) => s.clone(), other => other.to_string() };
    let base_external_id = raw.splitn(2, ':').last().filter(|s| !s.trim().is_empty())?.to_string(); // unwrap_public_id
    let symbol = match &row["symbol"] { Value::Null => String::new(), Value::String(s) => s.clone(), other => other.to_string() };
    let (base, quote) = symbol.split_once('/')?;
    if base.trim().is_empty() || quote.trim().is_empty() { return None; }
    Some(json!({
        "base_external_id": base_external_id, "quote_external_id": "usd", "base": base, "quote": quote,
        "ticker": if truthy(&row["native_symbol"]) { row["native_symbol"].clone() } else { row["symbol"].clone() },
        "minimum_base_size": row["minimum_base_size"], "minimum_quote_size": row["minimum_quote_size"],
        "maximum_base_size": row["maximum_base_size"], "maximum_quote_size": row["maximum_quote_size"],
        "base_decimals": row["base_decimals"], "quote_decimals": row["quote_decimals"], "price_decimals": row["price_decimals"],
        "trading_enabled": row["trading_enabled"],
    }))
}

/// Asset::SyncAlpacaCryptoFromDeltabadgerJob → sync_alpaca_crypto_listings_from_deltabadger!.
async fn alpaca_crypto<T: Transport>(api: &DataApi<T>, cx: &Cx<'_>) -> Outcome {
    let body = match api.alpaca_crypto_listings().await { Ok(b) => b, Err(e) => return api_failure(e) };
    let now = cx.clock.now();
    // Computed first (reads, and the one-row `usd` write Rails also makes before its guard), published after.
    let step = cx.db.run(move |c, cipher| {
        let Some(alpaca) = alpaca_id(c)? else { return Ok(Err(Outcome::NothingNew)) };
        import::in_transaction(c, cipher, "usd", |c| import::ensure_usd(c, false, now))?;
        let rows = take_list(body);
        let tickers: Vec<Value> = rows.iter().filter_map(crypto_listing).collect();
        let mut bases: Vec<String> = vec![];
        for b in tickers.iter().filter_map(|t| t["base_external_id"].as_str()) { if !bases.iter().any(|x| x == b) { bases.push(b.into()); } }
        let resolved = import::count_assets(c, &bases)?;
        let last_good = app_config::get(c, cipher, ALPACA_CRYPTO_LAST_GOOD)?;
        if crypto_degraded(resolved, last_good.as_deref()) { return Ok(Err(degraded(resolved, rows.len(), last_good.as_deref()))); }
        Ok(Ok((alpaca, resolved, import::plan_tickers(c, alpaca, &tickers, Some("Cryptocurrency"))?)))
    }).await;
    let (alpaca, resolved, plan) = match step { Ok(Ok(x)) => x, Ok(Err(out)) => return out, Err(m) => return Outcome::Failed(m) };
    if let Err(m) = incomplete(cx, ALPACA_CRYPTO, now).await { return Outcome::Failed(m); }
    if let Err(m) = import::publish_tickers(&cx.db, alpaca, plan, now).await { return Outcome::Failed(m); }
    // Ratcheted only after the sweep.
    done(cx.db.run(move |c, cipher| app_config::set(c, cipher, ALPACA_CRYPTO_LAST_GOOD, &resolved.to_string(), now)).await)
}

/// Invariant B and STOCK_TICKER_DEFAULTS (:381-389, :500-504): every quote anchors to the local `usd`, and a missing or nil
/// trading parameter takes Alpaca's stock default (`l[k] ||= v`).
fn stock_listing(mut l: Value) -> Value {
    if let Value::Object(m) = &mut l {
        m.insert("quote_external_id".into(), json!("usd"));
        for (k, v) in [("minimum_base_size", json!(0.000000001)), ("maximum_base_size", json!(100_000)), ("minimum_quote_size", json!(1)),
                       ("maximum_quote_size", json!(10_000_000)), ("base_decimals", json!(9)), ("quote_decimals", json!(2)), ("price_decimals", json!(2))] {
            if !m.get(k).is_some_and(truthy) { m.insert(k.into(), v); }
        }
    }
    l
}

/// Asset::SyncStocksFromDeltabadgerJob#perform, the jitter split out to the scheduler (Spec.jitter): the switch, the
/// backfill's no-op case, the stock assets, then the Alpaca listings.
async fn stocks<T: Transport>(api: &DataApi<T>, cx: &Cx<'_>) -> Outcome {
    let now = cx.clock.now();
    let gate = cx.db.run(move |c, cipher| {
        // The per-container emergency switch: any value but "false" means on (:43-50).
        if app_config::get(c, cipher, "stock_sync_enabled")?.as_deref() == Some("false") {
            return Ok(Some("stock sync is switched off (app_configs stock_sync_enabled = false)".to_string()));
        }
        // backfill_canonical_stock_external_ids! (:723-813) rewrites legacy alpaca_<uuid> stock rows; without one it only sets
        // its flag (:731-734), which gates the sync (:64). The rewrite is not ported: refuse instead.
        let legacy: i64 = c.query_row("SELECT count(*) FROM assets WHERE category = 'Stock' AND external_id LIKE 'alpaca_%'", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if legacy > 0 {
            return Ok(Some(format!("{legacy} legacy alpaca_<uuid> stock asset(s): the canonical backfill is not ported; run the Rails app's stock sync once")));
        }
        if app_config::get(c, cipher, BACKFILL_FLAG)?.is_none_or(|v| v.trim().is_empty()) {
            app_config::set(c, cipher, BACKFILL_FLAG, &now.to_rfc3339_opts(SecondsFormat::Secs, true), now)?; // Time.current.iso8601
        }
        Ok(None)
    }).await;
    match gate { Ok(None) => {}, Ok(Some(m)) | Err(m) => return Outcome::Failed(m) }

    // sync_stocks_from_deltabadger! (:428-463). A failure skips the listings (sync_stocks_from_deltabadger_job.rb:66-73).
    let body = match api.stocks().await { Ok(b) => b, Err(e) => return api_failure(e) };
    let url = api.public_url();
    let rows = match cx.db.run(move |_, _| Ok(import::plan_stock_assets(&take_list(body), &url))).await { Ok(r) => r, Err(m) => return Outcome::Failed(m) };
    if let Err(m) = incomplete(cx, STOCKS, now).await { return Outcome::Failed(m); }
    if let Err(m) = import::import_stock_assets(&cx.db, rows, now).await { return Outcome::Failed(m); }

    // sync_alpaca_listings_from_deltabadger! (:475-582).
    let body = match api.alpaca_listings().await { Ok(b) => b, Err(e) => return api_failure(e) };
    let step = cx.db.run(move |c, cipher| {
        import::in_transaction(c, cipher, "usd", |c| import::ensure_usd(c, true, now))?;
        // fractionable: false is dropped (:495), then Invariant B and the defaults (:500-504), before the guard counts.
        let listings: Vec<Value> = take_list(body).into_iter().filter(|l| l["fractionable"] != Value::Bool(false)).map(stock_listing).collect();
        let Some(alpaca) = alpaca_id(c)? else { return Ok(Err(Outcome::NothingNew)) };
        let resolved = import::ticker_records_for(c, &listings)?.len() as i64; // import_tickers!' own builder, post-dedup (:514-520)
        let last_good = app_config::get(c, cipher, ALPACA_LISTINGS_LAST_GOOD)?;
        if stocks_degraded(resolved, last_good.as_deref()) { return Ok(Err(degraded(resolved, listings.len(), last_good.as_deref()))); }
        // The legacy collision guard (:539-551) and the sweep's legacy exclusion (:563) have nothing to act on: the gate
        // above refused any legacy row.
        Ok(Ok((alpaca, resolved, import::plan_tickers(c, alpaca, &listings, Some("Stock"))?)))
    }).await;
    let (alpaca, resolved, plan) = match step { Ok(Ok(x)) => x, Ok(Err(out)) => return out, Err(m) => return Outcome::Failed(m) };
    if let Err(m) = import::publish_tickers(&cx.db, alpaca, plan, now).await { return Outcome::Failed(m); }
    done(cx.db.run(move |c, cipher| app_config::set(c, cipher, ALPACA_LISTINGS_LAST_GOOD, &resolved.to_string(), now)).await)
}

/// BotActivityLog::PruneJob: `where('created_at < ?', 90.days.ago).delete_all`, in units of CHUNK rows (a backlog after
/// a long stop is bounded per unit). The rows deleted are Rails'.
async fn prune(cx: &Cx<'_>) -> Outcome {
    let cutoff = format_time(cx.clock.now() - Duration::days(90));
    loop {
        let cutoff = cutoff.clone();
        let deleted = cx.db.run(move |c, cipher| import::in_transaction(c, cipher, "prune", |c| {
            c.execute(&format!("DELETE FROM bot_activity_logs WHERE id IN (SELECT id FROM bot_activity_logs WHERE created_at < ?1 ORDER BY id LIMIT {})", super::CHUNK),
                      [cutoff]).map(|n| (n, import::Touched::None)).map_err(|e| e.to_string())
        })).await;
        match deleted {
            Ok(n) if n < super::CHUNK => return Outcome::Done,
            Ok(_) => {} // the next unit keeps CHUNK_GAP itself (import::in_transaction)
            Err(m) => return Outcome::Failed(m),
        }
    }
}
