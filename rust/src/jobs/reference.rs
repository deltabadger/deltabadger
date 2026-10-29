//! The reference-data jobs of config/recurring.yml, each a port of its Rails job on the deltabadger market-data provider.
//! HTTP is awaited on the runtime thread. Everything after it (the walk of the payload, the casts, the writes) runs in a
//! `Db::run` closure on the blocking pool, and the writes in units of CHUNK rows (import::publish).
use super::data_api::{self, ApiError, DataApi, MAX_REFERENCE_BODY};
use super::import::{self, present, truthy};
use super::schedule::{Jitter, Schedule};
use super::{Cx, Job, JobFuture, Outcome, Retry, Spec, Wake, DEADLINE};
use crate::app_config;
use crate::codec::format_time;
use crate::venue::http::Transport;
use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;

pub const STOCKS: &str = "sync_stocks_from_deltabadger_job";
pub const ALPACA_CRYPTO: &str = "sync_alpaca_crypto_from_deltabadger_job";
pub const INDICES: &str = "sync_indices_from_coingecko_job";
pub const TICKERS: &str = "sync_all_tickers_and_assets_job";
pub const ASSETS: &str = "fetch_all_assets_data_from_coingecko_job";
pub const PRUNE: &str = "prune_bot_activity_logs_job";
/// The IBKR catalogue sync (Ibkr plan R1, D-IBKR-CATALOGUE): Rails has no such job; IBKR listings come from our catalogue
/// through the same importer as every venue's, behind a revision fence.
pub const IBKR: &str = "sync_ibkr_listings_job";

const NOT_PORTED: &str = "the market-data provider is not deltabadger (MARKET_DATA_URL): this build syncs reference data only from data-api";
const ALPACA_LISTINGS_LAST_GOOD: &str = "alpaca_listings_last_good_count";        // market_data.rb:398
const MIN_HEALTHY_ALPACA_LISTINGS: i64 = 1000;                                    // :401
const ALPACA_CRYPTO_LAST_GOOD: &str = "alpaca_crypto_listings_last_good_count";   // :589
const MIN_HEALTHY_ALPACA_CRYPTO: i64 = 30;                                        // :590
const BACKFILL_FLAG: &str = "stock_canonical_backfill_completed_at";              // :391

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Stocks, AlpacaCrypto, Indices, Tickers, Assets, Prune, Ibkr }
/// Registration order: ties in due time run in this order (the 10:00 → 10:15 → 10:30 chain, recurring.yml:25-35).
const KINDS: [Kind; 7] = [Kind::Stocks, Kind::AlpacaCrypto, Kind::Indices, Kind::Tickers, Kind::Assets, Kind::Prune, Kind::Ibkr];

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
        // No Rails entry: daily after the 10:00-10:30 chain, retried as the other catalogue syncs are.
        Kind::Ibkr => Spec { name: IBKR, schedule: daily(10, 45), jitter: Jitter::NONE, retry: Retry::Polynomial { attempts: 5 }, deadline: DEADLINE, scope: None },
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
    shared_jobs(Rc::new(api))
}

pub fn shared_jobs<T: Transport + 'static>(api: Rc<Option<DataApi<T>>>) -> Vec<Box<dyn Job>> {
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
    if kind == Kind::Indices && api.is_none() { return coingecko_indices(&LiveCoinGecko, &cx).await; }
    let Some(api) = api else { return Outcome::Failed(NOT_PORTED.into()) };
    let feeds = Feeds::Api(api);
    match kind {
        Kind::Stocks => stocks(&feeds, &cx).await,
        Kind::AlpacaCrypto => alpaca_crypto(&feeds, &cx).await,
        Kind::Indices => indices(api, &cx).await,
        Kind::Tickers => tickers(api, &cx).await,
        Kind::Assets => assets(api, &cx).await,
        Kind::Ibkr => ibkr(&feeds, &cx).await,
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
/// A pull that did not fail, an empty one included, ends in `recheck_index_bots` (:22-27, :151-155): every index bot is
/// marked for the engine's composition resync (engine::index::mark_resync_all), in the job's last unit.
async fn indices<T: Transport>(api: &DataApi<T>, cx: &Cx<'_>) -> Outcome {
    let config = api.configuration().clone();
    let captured = cx.db.run(move |c,_| {
        if crate::engine::provider::config(c).map_err(|_|"index configuration unreadable")?.as_ref()!=Some(&config) {return Err("index client configuration changed; restart with current configuration".into());}
        crate::engine::provider::fingerprint(c).map_err(|_|"index configuration unreadable".to_string())?.ok_or_else(||"index provider not configured".into())
    }).await;
    let captured = match captured {Ok(v)=>v,Err(e)=>return Outcome::Failed(e)};
    let body = match api.indices().await { Ok(b) => b, Err(e) => return Outcome::Transient(e.message()) };
    let now = cx.clock.now();
    let plan = cx.db.run(move |_, _| {
        let rows = take_data(body)?;
        if !present(&rows) { return Ok(vec![]); } // `return if indices_data.blank?`
        import::plan_indices(rows.as_array().ok_or_else(|| "the indices payload's data is not a list".to_string())?)
    }).await;
    let result = match plan {
        Ok(rows) if rows.is_empty() => { // Rails writes nothing; the indices keep their age
            let now = cx.clock.now();
            return match cx.db.run(move |c, _| crate::engine::model::locked(c, |tx| crate::engine::index::mark_resync_all(tx, now)).map_err(|e| format!("{e:?}"))).await {
                Ok(_) => { cx.wakers.wake_engine(); Outcome::NothingNew }
                Err(e) => Outcome::Failed(e),
            };
        }
        Ok(rows) => match incomplete(cx, INDICES, now).await { Ok(()) => import::import_indices(&cx.db, rows, now).await, Err(m) => Err(m) },
        Err(m) => Err(m),
    };
    match result {
        Ok(()) => match cx.db.run(move |c,_| {
            crate::engine::model::locked(c,|tx| {
                if crate::engine::provider::fingerprint(tx)?.as_deref()!=Some(&captured) {return Err(crate::engine::EngineError::Data("index configuration changed during refresh".into()));}
                crate::engine::provider::record(tx,&captured,now)?;
                crate::engine::index::mark_resync_all(tx,now).map(|_|())
            }).map_err(|_|"index configuration changed during refresh; refresh again".to_string())
        }).await {Ok(())=>{cx.wakers.wake_engine();Outcome::Done},Err(e)=>Outcome::Failed(e)},
        Err(m)=>Outcome::Transient(m)
    }
}

/// D-INDEX-TOP-N: the direct provider refreshes Top Coins through the same daily indices job,
/// retry policy, provider fence and composition resync path as the data-api provider.
pub trait CoinGecko {
    fn markets<'a>(&'a self, key: &'a str) -> Pin<Box<dyn Future<Output = Result<Value, ApiError>> + 'a>>;
}

struct LiveCoinGecko;
impl CoinGecko for LiveCoinGecko {
    fn markets<'a>(&'a self, key: &'a str) -> Pin<Box<dyn Future<Output = Result<Value, ApiError>> + 'a>> {
        Box::pin(async move {
            let client = crate::venue::http::client();
            // Clients::Coingecko.detect_plan: the pro ping selects the URL and key header.
            let ping = client.get("https://pro-api.coingecko.com/api/v3/ping").header("x-cg-pro-api-key", key).send().await
                .map_err(|_| ApiError::Transient("CoinGecko plan check failed".into()))?;
            let (base, header) = if ping.status().is_success() { ("https://pro-api.coingecko.com", "x-cg-pro-api-key") }
                else { ("https://api.coingecko.com", "x-cg-demo-api-key") };
            let reply = client.get(format!("{base}/api/v3/coins/markets?vs_currency=usd&order=market_cap_desc&per_page=250&page=1&sparkline=false")).header(header, key)
                .send().await.map_err(|_| ApiError::Transient("CoinGecko ranking request failed".into()))?;
            let reply = crate::venue::http::read_limited(reply, MAX_REFERENCE_BODY).await
                .map_err(|_| ApiError::Transient("CoinGecko ranking response unavailable".into()))?;
            if reply.status != 200 { return Err(ApiError::Failed{status:Some(reply.status),message:format!("CoinGecko ranking HTTP {}",reply.status)}); }
            serde_json::from_str(&reply.body).map_err(|_| ApiError::Failed{status:Some(200),message:"Unreadable CoinGecko ranking".into()})
        })
    }
}

pub async fn coingecko_indices<F: CoinGecko>(fetch: &F, cx: &Cx<'_>) -> Outcome {
    let captured = cx.db.run(|c, cipher| {
        crate::engine::provider::bind_cipher(c,cipher).map_err(|e|format!("{e:?}"))?;
        let key = crate::engine::provider::coingecko_key(c).map_err(|_|"index configuration unreadable")?
            .ok_or_else(||NOT_PORTED.to_string())?;
        let fingerprint = crate::engine::provider::fingerprint(c).map_err(|_|"index configuration unreadable")?
            .ok_or("index provider not configured")?;
        Ok((key,fingerprint))
    }).await;
    let (key,captured) = match captured { Ok(v)=>v, Err(e)=>return Outcome::Failed(e) };
    let body = match fetch.markets(&key).await { Ok(body)=>body, Err(e)=>return api_failure(e) };
    let Some(rows) = body.as_array().filter(|rows|!rows.is_empty() && rows.len()<=250) else {
        return Outcome::Failed("CoinGecko ranking is empty or malformed; previous ranking kept".into());
    };
    let mut ids = vec![];
    let mut weights = serde_json::Map::new();
    for row in rows {
        let (Some(id),Some(cap)) = (row["id"].as_str().filter(|s|!s.is_empty()),row["market_cap"].as_f64().filter(|n|n.is_finite() && *n>0.0)) else {
            return Outcome::Failed("CoinGecko ranking has an invalid id or market cap; previous ranking kept".into());
        };
        if weights.insert(id.to_string(),json!(cap)).is_some() { return Outcome::Failed("CoinGecko ranking has duplicate ids; previous ranking kept".into()); }
        ids.push(id.to_string());
    }
    let now = cx.clock.now();
    let result = cx.db.run(move |c,_| crate::engine::model::locked(c,|tx| {
        use crate::engine::{index,provider,EngineError};
        if provider::fingerprint(tx)?.as_deref()!=Some(&captured) { return Err(EngineError::Data("index configuration changed during refresh".into())); }
        let stamp = format_time(now);
        // Keep existing asset identities and taxonomy. Fresh market caps are also used by the next composition step.
        for (id,cap) in &weights { tx.execute("UPDATE assets SET market_cap=?1,updated_at=?2 WHERE external_id=?3",rusqlite::params![cap.as_f64(),stamp,id])?; }
        let id: Option<i64> = tx.query_row("SELECT id FROM indices WHERE external_id='top-coins' ORDER BY id LIMIT 1",[],|r|r.get(0)).optional()?;
        match id {
            Some(id) => { tx.execute("UPDATE indices SET top_coins=?1,weights=?2,updated_at=?3 WHERE id=?4",rusqlite::params![json!(ids).to_string(),json!(weights).to_string(),stamp,id])?; }
            None => { tx.execute("INSERT INTO indices (external_id,source,name,top_coins,weights,created_at,updated_at) VALUES ('top-coins','internal','Top Coins',?1,?2,?3,?3)",rusqlite::params![json!(ids).to_string(),json!(weights).to_string(),stamp])?; }
        }
        provider::record(tx,&captured,now)?;
        // Only Top Coins was refreshed; direct-provider category indices remain unsupported.
        let mut s=tx.prepare("SELECT id FROM bots WHERE type='Bots::DcaIndex' AND status NOT IN (3,7) AND exchange_id IS NOT NULL")?;
        let bots=s.query_map([],|r|r.get::<_,i64>(0))?.collect::<Result<Vec<_>,_>>()?;
        for id in bots { if crate::engine::model::load_bot(tx,id)?.index_external_id()=="top-coins" { index::mark_resync(tx,id,now)?; } }
        Ok(())
    }).map_err(|e|format!("{e:?}"))).await;
    match result { Ok(())=>{cx.wakers.wake_engine();Outcome::Done},Err(e)=>Outcome::Failed(e) }
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
async fn alpaca_crypto<T: Transport>(feeds: &Feeds<'_, T>, cx: &Cx<'_>) -> Outcome {
    let body = match feeds.get(ALPACA_CRYPTO_LISTINGS).await { Ok(b) => b, Err(e) => return api_failure(e) };
    let rev = match admit(feeds, cx, ALPACA_CRYPTO, &body).await { Ok(r) => r, Err(out) => return out };
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
        let mut plan = import::plan_tickers(c, alpaca, &tickers, Some("Cryptocurrency"))?;
        // A catalogue revision with a listing whose asset is not here yet is partial: no sweep, imported again.
        let partial = rev.is_some() && (resolved as usize) < bases.len();
        if rev.is_some_and(|r| !r.complete) || partial { plan.sweep.clear(); } // an incomplete revision is no delisting
        let written = plan.written.len() as i64;
        let insufficient = crypto_degraded(written, last_good.as_deref())
            .then(|| format!("degraded ticker import: {written} written from {resolved} resolved assets (last good {}); ticker freshness kept", last_good.as_deref().unwrap_or("none")));
        Ok(Ok((alpaca, resolved, plan, insufficient, partial)))
    }).await;
    let (alpaca, resolved, plan, insufficient, partial) = match step { Ok(Ok(x)) => x, Ok(Err(out)) => return out, Err(m) => return Outcome::Failed(m) };
    if let Err(m) = incomplete(cx, ALPACA_CRYPTO, now).await { return Outcome::Failed(m); }
    if let Err(m) = import::publish_tickers_fenced(&cx.db, alpaca, plan, now, rev.map(Rev::fence)).await { return fenced(m); }
    // Exchange assets may have refreshed, but only enough actually written tickers can renew ticker freshness.
    if let Some(m) = insufficient { return Outcome::Failed(m); }
    // Ratcheted only after the sweep.
    if let Err(m) = cx.db.run(move |c, cipher| app_config::set(c, cipher, ALPACA_CRYPTO_LAST_GOOD, &resolved.to_string(), now)).await { return Outcome::Failed(m); }
    finished(cx, rev, partial).await
}

/// One IBKR feed body (data-api C2): its metadata and rows, checked before anything is written. A row must be an
/// `ibkr_conid` listing whose ticker is its conid (`listing_id` "ibkr:<conid>") with an explicit `trading_enabled`, and
/// `count` must match the rows: anything else refuses the payload.
struct Feed { revision: i64, generated_at: DateTime<Utc>, complete: bool, rows: Vec<Value> }

fn ibkr_feed(body: Value) -> Result<Feed, String> {
    let bad = |what: &str| format!("IBKR feed: {what}; nothing imported");
    let meta = body.get("metadata").cloned().unwrap_or(Value::Null);
    let revision = meta["revision"].as_i64().filter(|r| *r > 0).ok_or_else(|| bad("metadata.revision is not a positive integer"))?;
    let generated_at: DateTime<Utc> = meta["generated_at"].as_str().and_then(|t| t.parse().ok()).ok_or_else(|| bad("metadata.generated_at is not a time"))?;
    let complete = meta["complete"].as_bool().ok_or_else(|| bad("metadata.complete is not a boolean"))?;
    let rows = match take_data(body)? { Value::Array(a) => a, _ => return Err(bad("data is not a list")) };
    if meta["count"].as_u64() != Some(rows.len() as u64) { return Err(bad("metadata.count does not match the rows")); }
    let text = |v: &Value| v.as_str().is_some_and(|s| !s.trim().is_empty());
    let decimals = |v: &Value| v.as_u64().is_some();
    let size = |v: &Value| match v {
        Value::String(s) => crate::ruby::BigDec::parse(s.trim()).is_ok(),
        Value::Number(_) => true,
        _ => false,
    };
    for r in &rows {
        // Every field the conid check and ticker_records_for read: a row they would drop or fail on refuses the payload
        // here, before the takeover, so a malformed complete feed never sweeps.
        let fields = ["base", "quote", "base_external_id", "quote_external_id"].iter().all(|k| text(&r[*k]))
            && ["base_decimals", "quote_decimals", "price_decimals"].iter().all(|k| decimals(&r[*k]))
            && size(&r["minimum_base_size"])
            && ["minimum_quote_size", "maximum_base_size", "maximum_quote_size"].iter().all(|k| r[*k].is_null() || size(&r[*k]));
        if !fields { return Err(bad(&format!("listing {} lacks a required field or carries an unreadable one", r["listing_id"]))); }
        let conid = r["ticker"].as_str().filter(|t| !t.is_empty() && !t.starts_with('0') && t.bytes().all(|b| b.is_ascii_digit()) && t.parse::<i64>().is_ok());
        let ok = r["venue_scheme"] == "ibkr_conid" && r["trading_enabled"].is_boolean()
            && conid.is_some_and(|c| r["listing_id"].as_str() == Some(format!("ibkr:{c}").as_str()));
        if !ok { return Err(bad(&format!("listing {} is not an ibkr_conid row with its conid as ticker", r["listing_id"]))); }
    }
    Ok(Feed { revision, generated_at, complete, rows })
}

/// D-IBKR-CONIDS: the rows to import and the identities they append. A row whose conid `ibkr_conids` (or an earlier row)
/// holds for another asset is skipped and counted; a row whose base or quote asset this install lacks yet is dropped and
/// counted apart (`unresolved`: the import is partial).
/// The kept rows, the (conid, asset) appends, how many rows were skipped as moved, and how many as unresolved.
type ConidRows = (Vec<Value>, Vec<(i64, i64)>, usize, usize);

fn conid_rows(c: &Connection, rows: Vec<Value>) -> Result<ConidRows, String> {
    let e = |e: rusqlite::Error| e.to_string();
    let (mut keep, mut appends, mut moved, mut unresolved) = (vec![], vec![], 0, 0);
    let asset = |ext: &Value| -> Result<Option<i64>, String> {
        c.prepare_cached("SELECT id FROM assets WHERE external_id = ?1").map_err(e)?
            .query_row([ext.as_str().unwrap_or_default()], |r| r.get(0)).optional().map_err(e)
    };
    let mut seen: std::collections::HashMap<i64, i64> = Default::default();
    for row in rows {
        let conid: i64 = row["ticker"].as_str().and_then(|t| t.parse().ok()).ok_or("IBKR feed: unreadable conid")?;
        let (Some(asset), Some(_)) = (asset(&row["base_external_id"])?, asset(&row["quote_external_id"])?) else { unresolved += 1; continue };
        let held = match seen.get(&conid) {
            Some(a) => Some(*a),
            None => c.prepare_cached("SELECT asset_id FROM ibkr_conids WHERE conid = ?1").map_err(e)?.query_row([conid], |r| r.get(0)).optional().map_err(e)?,
        };
        match held {
            Some(a) if a != asset => { moved += 1; continue; }
            Some(_) => {}
            None => { appends.push((conid, asset)); }
        }
        seen.insert(conid, asset);
        keep.push(row);
    }
    Ok((keep, appends, moved, unresolved))
}

#[derive(Clone, Copy, PartialEq)]
enum Gate { Takeover, Evidence, Unchanged }

/// One revisioned payload (data-api C2) as Ibkr plan R1's protocol reads it, and the job record that holds its source.
#[derive(Clone, Copy, Debug)]
struct Rev { job: &'static str, revision: i64, generated_at: DateTime<Utc>, complete: bool, catalogue: bool }

impl Rev {
    fn fence(self) -> import::Fence { import::Fence { job: self.job, revision: self.revision } }
}

/// `generated_at: null` is no evidence yet (a source with no upstream success, e.g. the IBKR feed before any conid
/// resolved): nothing to import, not an error. Rows without evidence are refused.
fn no_evidence(body: &Value) -> Result<(), String> {
    match &body["data"] {
        Value::Null => Ok(()),
        Value::Array(a) if a.is_empty() => Ok(()),
        _ => Err("rows without evidence (metadata.generated_at is null); nothing imported".into()),
    }
}

/// A payload's revision metadata. Ok(None): no evidence yet (`generated_at` null, as data-api publishes a source that never
/// synced upstream). A payload more than 5 minutes ahead (step 0), or one whose `count` is not its rows, is refused.
fn revision(job: &'static str, body: &Value, catalogue: bool, now: DateTime<Utc>) -> Result<Option<Rev>, String> {
    let bad = |what: &str| format!("{job}: {what}; nothing imported");
    let meta = &body["metadata"];
    if meta["generated_at"].is_null() { return no_evidence(body).map(|()| None).map_err(|m| format!("{job}: {m}")); }
    let revision = meta["revision"].as_i64().filter(|r| *r > 0).ok_or_else(|| bad("metadata.revision is not a positive integer"))?;
    let generated_at: DateTime<Utc> = meta["generated_at"].as_str().and_then(|t| t.parse().ok()).ok_or_else(|| bad("metadata.generated_at is not a time"))?;
    let complete = meta["complete"].as_bool().ok_or_else(|| bad("metadata.complete is not a boolean"))?;
    if meta["count"].as_u64() != body["data"].as_array().map(|a| a.len() as u64) { return Err(bad("metadata.count does not match the rows")); }
    if generated_at > now + Duration::minutes(5) {
        return Err(bad(&format!("generated_at {} is in the future", generated_at.to_rfc3339_opts(SecondsFormat::Secs, true))));
    }
    Ok(Some(Rev { job, revision, generated_at, complete, catalogue }))
}

/// Ibkr plan R1's import protocol for one payload P (D-JOBS-REVISION, D-JOBS-SOURCE-AT), steps 1-2 in one transaction:
/// 1. evidence-only: P.revision == revision with no import in flight moves source_at forward, nothing else (unless the
///    revision is partial: then it is taken over again);
/// 2. takeover: P.revision > revision and >= any importing_revision sets importing_revision, else nothing is written.
///
/// Then (the caller): 3. the plan, only after the takeover committed; 4. every unit first checks the fence; 5. `finish`.
/// The runner's records leave these fields as they are; a crash leaves importing_revision, which the next payload at
/// that revision or newer takes over.
fn take_over(c: &Connection, cipher: &crate::crypto::Cipher, r: Rev, now: DateTime<Utc>) -> Result<Gate, String> {
    use super::state;
    import::in_transaction(c, cipher, "takeover", |c| {
        let mut s = state::read(c, r.job, None)?;
        // A partial revision imports again: listings whose assets were missing may resolve now.
        let reimport = s.partial && s.revision == Some(r.revision);
        let gate = if s.importing_revision.is_none() && s.revision == Some(r.revision) && !reimport {
            if s.source_at.is_some_and(|at| at >= r.generated_at) { return Ok((Gate::Unchanged, import::Touched::None)); }
            (s.source_at, s.catalogue) = (Some(r.generated_at), r.catalogue);
            Gate::Evidence
        } else if (Some(r.revision) > s.revision || reimport) && s.importing_revision.is_none_or(|x| r.revision >= x) {
            s.importing_revision = Some(r.revision);
            s.incomplete_since.get_or_insert(now);
            Gate::Takeover
        } else { return Ok((Gate::Unchanged, import::Touched::None)) };
        state::write(c, r.job, None, &s, now).map(|()| (gate, import::Touched::None))
    })
}

/// Step 5, fenced: sets revision, clears importing_revision, moves source_at forward and marks the revision partial when
/// rows were skipped for missing assets (no sweep then).
fn finish(c: &Connection, cipher: &crate::crypto::Cipher, r: Rev, partial: bool, now: DateTime<Utc>) -> Result<(), String> {
    use super::state;
    import::in_transaction(c, cipher, "revision", |c| {
        r.fence().check(c)?;
        let mut s = state::read(c, r.job, None)?;
        (s.revision, s.importing_revision, s.incomplete_since, s.partial) = (Some(r.revision), None, None, partial);
        if s.source_at.is_none_or(|at| r.generated_at >= at) { (s.source_at, s.catalogue) = (Some(r.generated_at), r.catalogue); }
        state::write(c, r.job, None, &s, now).map(|()| ((), import::Touched::None))
    })
}

/// Steps 1-2 for `r`: Ok(r) imports behind its fence; Err is the run's outcome when there is nothing to import (moved
/// evidence is a refresh: Done).
async fn gate(cx: &Cx<'_>, r: Rev) -> Result<Rev, Outcome> {
    let now = cx.clock.now();
    match cx.db.run(move |c, cipher| take_over(c, cipher, r, now)).await {
        Ok(Gate::Takeover) => Ok(r),
        Ok(Gate::Evidence) => Err(Outcome::Done),
        Ok(Gate::Unchanged) => Err(Outcome::NothingNew),
        Err(m) => Err(Outcome::Failed(m)),
    }
}

/// D-BROKER-SEED: a catalogue source imports through R1's protocol under `job`'s record. Ok(None): data-api's payload,
/// imported as Rails imports it; Ok(Some(rev)): import behind rev's fence, then `finished`; Err: nothing to import.
async fn admit<T: Transport>(feeds: &Feeds<'_, T>, cx: &Cx<'_>, job: &'static str, body: &Value) -> Result<Option<Rev>, Outcome> {
    if !feeds.catalogue() { return Ok(None); }
    match revision(job, body, true, cx.clock.now()) {
        Ok(Some(r)) => gate(cx, r).await.map(Some),
        Ok(None) => Err(Outcome::NothingNew),
        Err(m) => Err(Outcome::Failed(m)),
    }
}

/// A unit's error: a superseded import's plan is discarded (not an error), anything else fails the run.
fn fenced(m: String) -> Outcome {
    if m.starts_with(import::SUPERSEDED) { crate::engine::log(&format!("[jobs] {m}; this run's plan is discarded")); return Outcome::NothingNew; }
    Outcome::Failed(m)
}

/// After an import's last unit: step 5 for a revisioned one.
async fn finished(cx: &Cx<'_>, rev: Option<Rev>, partial: bool) -> Outcome {
    let Some(r) = rev else { return Outcome::Done };
    let now = cx.clock.now();
    cx.db.run(move |c, cipher| finish(c, cipher, r, partial, now)).await.map_or_else(fenced, |()| Outcome::Done)
}

/// The IBKR catalogue sync: R1's protocol (`take_over`, `finish`) on the IBKR feed, the plan computed after the takeover
/// with a pair-level sweep when P is complete, plus the conid appends.
async fn ibkr<T: Transport>(feeds: &Feeds<'_, T>, cx: &Cx<'_>) -> Outcome {
    let body = match feeds.get(IBKR_LISTINGS).await { Ok(b) => b, Err(e) => return api_failure(e) };
    // No evidence yet, from data-api or the snapshot: nothing new (rows without evidence are refused).
    if body["metadata"]["generated_at"].is_null() {
        return no_evidence(&body).map_or_else(|m| Outcome::Failed(format!("IBKR feed: {m}")), |()| Outcome::NothingNew);
    }
    let now = cx.clock.now();
    let feed = match cx.db.run(move |_, _| ibkr_feed(body)).await { Ok(f) => f, Err(m) => return Outcome::Failed(m) };
    let r = Rev { job: IBKR, revision: feed.revision, generated_at: feed.generated_at, complete: feed.complete, catalogue: feeds.catalogue() };
    if r.generated_at > now + Duration::minutes(5) {
        return Outcome::Failed(format!("IBKR feed: generated_at {} is in the future; nothing imported", r.generated_at.to_rfc3339_opts(SecondsFormat::Secs, true)));
    }
    let venue = cx.db.run(|c, _| c.query_row("SELECT id FROM exchanges WHERE type = 'Exchanges::Ibkr' ORDER BY id LIMIT 1", [], |r| r.get::<_, i64>(0))
        .optional().map_err(|e| e.to_string())).await;
    let ibkr = match venue { Ok(Some(id)) => id, Ok(None) => return Outcome::NothingNew, Err(m) => return Outcome::Failed(m) };
    let r = match gate(cx, r).await { Ok(r) => r, Err(out) => return out };
    let step = cx.db.run(move |c, cipher| {
        import::in_transaction(c, cipher, "usd", |c| import::ensure_usd(c, false, now))?;
        let (rows, conids, moved, unresolved) = conid_rows(c, feed.rows)?;
        // No sweep from a partial import: a listing it could not import is not a delisting.
        let plan = import::plan_tickers_with(c, ibkr, &rows, None, r.complete && unresolved == 0)?;
        Ok((plan, conids, moved, unresolved))
    }).await;
    let (plan, conids, moved, unresolved) = match step { Ok(x) => x, Err(m) => return Outcome::Failed(m) };
    let revision = r.revision;
    if unresolved > 0 { crate::engine::log(&format!("[jobs] {IBKR}: revision {revision}: {unresolved} listing(s) skipped, their assets are not on this install yet; partial, no sweep")); }
    if moved > 0 { crate::engine::log(&format!("[jobs] {IBKR}: revision {revision}: {moved} listing(s) skipped, their conid belongs to another asset (ibkr_conids)")); }
    let t = format_time(now);
    let published = async {
        import::publish_fenced(&cx.db, "ibkr conids", import::chunks(&conids), Some(r.fence()), move |c, rows: Vec<(i64, i64)>| {
            for (conid, asset) in rows {
                c.prepare_cached("INSERT INTO ibkr_conids (conid, asset_id, created_at) VALUES (?1, ?2, ?3) ON CONFLICT (conid) DO NOTHING")
                    .and_then(|mut s| s.execute(rusqlite::params![conid, asset, t])).map_err(|e| e.to_string())?;
            }
            Ok(import::Touched::None)
        }).await?;
        import::publish_tickers_fenced(&cx.db, ibkr, plan, now, Some(r.fence())).await.map(|_| ())
    };
    match published.await {
        Ok(()) => finished(cx, Some(r), unresolved > 0).await,
        Err(m) => fenced(m),
    }
}

/// The catalogue snapshot's sources (data-api's StockCatalogCache::SOURCES), as `metadata.sources` and `data` key them.
const STOCK_ASSETS: &str = "stock_assets";
const ALPACA_LISTINGS: &str = "alpaca_listings";
const ALPACA_CRYPTO_LISTINGS: &str = "alpaca_crypto";
const IBKR_LISTINGS: &str = "ibkr_listings";
/// The stock assets source's revision record (D-BROKER-SEED): the other sources keep theirs on their sync job's record
/// (the Alpaca listings on the stock job's, which the stock tickers' freshness reads); the stock assets, the stock job's
/// first half, need one of their own.
pub const STOCK_ASSETS_FEED: &str = "catalogue_stock_assets";

/// Where a sync reads its payloads: data-api, or one install's copy of the catalogue snapshot (the bundle or the public
/// snapshot), whose sources import through R1's protocol.
enum Feeds<'a, T: Transport> { Api(&'a DataApi<T>), Catalogue(&'a Snapshot) }

impl<T: Transport> Feeds<'_, T> {
    async fn get(&self, source: &str) -> Result<Value, ApiError> {
        match self {
            Self::Api(api) => match source {
                STOCK_ASSETS => api.stocks().await,
                ALPACA_LISTINGS => api.alpaca_listings().await,
                ALPACA_CRYPTO_LISTINGS => api.alpaca_crypto_listings().await,
                _ => api.ibkr_listings().await,
            },
            Self::Catalogue(s) => Ok(s.body(source)),
        }
    }
    fn catalogue(&self) -> bool { matches!(self, Self::Catalogue(_)) }
    /// Logo paths are served from data-api's public host.
    fn public_url(&self) -> String { match self { Self::Api(api) => api.public_url(), Self::Catalogue(_) => "https://data.deltabadger.com".into() } }
}

/// The decompressed snapshot's bound: the gzip itself is held to MAX_REFERENCE_BODY. ponytail: 4x, far above today's
/// 14 MB of JSON; widen it when the catalogue grows past it.
const MAX_CATALOGUE_JSON: usize = 4 * MAX_REFERENCE_BODY;

/// data-api's PublishCatalogueSnapshotJob: `{metadata: {built_at, sources: {<source>: {revision, generated_at, complete,
/// count}}}, data: {<source>: [rows]}}`, gzipped.
pub struct Snapshot { sources: serde_json::Map<String, Value>, data: serde_json::Map<String, Value> }

impl Snapshot {
    /// Read whole before anything is written: a corrupt or oversized snapshot is refused.
    pub fn parse(gz: &[u8]) -> Result<Self, String> {
        let bad = |what: String| format!("catalogue snapshot: {what}; nothing imported");
        let mut json = Vec::new();
        flate2::read::GzDecoder::new(gz).take(MAX_CATALOGUE_JSON as u64 + 1).read_to_end(&mut json).map_err(|e| bad(format!("unreadable gzip ({e})")))?;
        if json.len() > MAX_CATALOGUE_JSON { return Err(bad(format!("over {MAX_CATALOGUE_JSON} bytes decompressed"))); }
        let Value::Object(mut top) = serde_json::from_slice(&json).map_err(|e| bad(format!("unreadable JSON ({e})")))? else { return Err(bad("not an object".into())) };
        let sources = top.get_mut("metadata").and_then(|m| m.get_mut("sources")).map(Value::take);
        match (sources, top.remove("data")) {
            (Some(Value::Object(sources)), Some(Value::Object(data))) => Ok(Self { sources, data }),
            _ => Err(bad("no metadata.sources or data object".into())),
        }
    }
    /// One source as its feed carries it; an absent source has no evidence (skipped).
    fn body(&self, source: &str) -> Value {
        json!({ "metadata": self.sources.get(source).cloned().unwrap_or(Value::Null), "data": self.data.get(source).cloned().unwrap_or(Value::Null) })
    }
}

/// The self-hosted catalogue (Ibkr plan R2, D-BROKER-SEED): the bundled snapshot, once per start, and the public one,
/// daily. Both only where data-api is not configured (there the syncs above keep the catalogue).
pub const BOOTSTRAP: &str = "catalogue_bootstrap";
pub const REFRESH: &str = "catalogue_refresh";
/// The bundle a release ships (the release-catalogue job), relative to the app's directory.
pub const BUNDLE: &str = "db/seed_data/catalogue.json.gz";
/// The refresh's default source; `IBKR_CATALOGUE_URL` names another, or `off`.
pub const SNAPSHOT_URL: &str = "https://data.deltabadger.com/public/v1/catalogue.json.gz";

pub type FetchFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<u8>, ApiError>> + 'a>>;

/// The refresh's one GET: the snapshot's bytes (a gzip, so not a Transport's text body), at most MAX_REFERENCE_BODY.
pub trait Fetch { fn fetch<'a>(&'a self, url: &'a str) -> FetchFuture<'a>; }

#[derive(Clone)]
pub struct LiveFetch(reqwest::Client);

impl Default for LiveFetch {
    /// The bulk pulls' timeouts.
    fn default() -> Self { Self(crate::venue::http::client_with(crate::venue::http::CONNECT_TIMEOUT, data_api::BULK_READ_TIMEOUT, std::time::Duration::from_secs(600))) }
}

impl Fetch for LiveFetch {
    fn fetch<'a>(&'a self, url: &'a str) -> FetchFuture<'a> {
        use crate::venue::http::TransportError;
        let failed = |status, message: String| ApiError::Failed { status, message };
        Box::pin(async move {
            let url = reqwest::Url::parse(url).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())
                .ok_or_else(|| failed(None, "IBKR_CATALOGUE_URL is not an HTTP(S) URL".into()))?;
            let mut resp = self.0.get(url).send().await.map_err(|e| match crate::venue::http::classify(e) {
                TransportError::Permanent(m) => failed(None, m),
                TransportError::NotSent(m) | TransportError::MaybeSent(m) => ApiError::Transient(m),
            })?;
            let status = resp.status().as_u16();
            if status >= 400 { return Err(failed(Some(status), format!("catalogue snapshot: HTTP {status}"))); }
            let mut body = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|e| ApiError::Transient(e.to_string()))? {
                if body.len() + chunk.len() > MAX_REFERENCE_BODY { return Err(failed(Some(status), format!("catalogue snapshot: over {MAX_REFERENCE_BODY} bytes; nothing imported"))); }
                body.extend_from_slice(&chunk);
            }
            Ok(body)
        })
    }
}

struct Catalogue<F: Fetch> { refresh: bool, env: Vec<(&'static str, String)>, bundle: PathBuf, fetch: F }

/// The two catalogue jobs, reading `bundle` and fetching with `fetch`, with the environment as the process started.
pub fn catalogue_jobs<F: Fetch + Clone + 'static>(env: &dyn Fn(&str) -> Option<String>, bundle: PathBuf, fetch: F) -> Vec<Box<dyn Job>> {
    let env: Vec<_> = ["MARKET_DATA_URL", "MARKET_DATA_TOKEN", "IBKR_CATALOGUE_URL"].into_iter().filter_map(|n| env(n).map(|v| (n, v))).collect();
    [false, true].into_iter().map(|refresh| Box::new(Catalogue { refresh, env: env.clone(), bundle: bundle.clone(), fetch: fetch.clone() }) as Box<dyn Job>).collect()
}

impl<F: Fetch + 'static> Job for Catalogue<F> {
    fn spec(&self) -> Spec {
        // One run imports what the stock, Alpaca crypto and IBKR syncs import.
        let deadline = DEADLINE * 3;
        if !self.refresh { return Spec { name: BOOTSTRAP, schedule: None, jitter: Jitter::NONE, retry: Retry::None, deadline, scope: None }; }
        // Daily after the snapshot's morning sources, spread over 30 minutes across installs; retried as the syncs are.
        Spec { name: REFRESH, schedule: Some(Schedule::Daily { hour: 11, minute: 0 }), jitter: Jitter { min_secs: 1, max_secs: 1800 },
               retry: Retry::Polynomial { attempts: 5 }, deadline, scope: None }
    }
    fn run<'a>(&'a self, cx: Cx<'a>, _wakes: Vec<Wake>) -> JobFuture<'a> { Box::pin(catalogue(self, cx)) }
}

/// The bundle's bytes; None when this build ships none.
fn read_bundle(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("catalogue bundle {}: {e}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_REFERENCE_BODY as u64 + 1).read_to_end(&mut bytes).map_err(|e| format!("catalogue bundle {}: {e}; nothing imported", path.display()))?;
    if bytes.len() > MAX_REFERENCE_BODY { return Err(format!("catalogue bundle {}: over {MAX_REFERENCE_BODY} bytes; nothing imported", path.display())); }
    Ok(Some(bytes))
}

async fn catalogue<F: Fetch>(job: &Catalogue<F>, cx: Cx<'_>) -> Outcome {
    let env = job.env.clone();
    let hosted = cx.db.run(move |c, cipher| data_api::config(&|k| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone()), c, cipher)).await;
    match hosted { Ok(Some(_)) => return Outcome::NothingNew, Ok(None) => {} Err(m) => return Outcome::Failed(m) }
    let fetched = if job.refresh {
        let url = job.env.iter().find(|(n, _)| *n == "IBKR_CATALOGUE_URL").map(|(_, v)| v.trim()).filter(|v| !v.is_empty()).unwrap_or(SNAPSHOT_URL);
        if url.eq_ignore_ascii_case("off") { return Outcome::NothingNew; }
        match job.fetch.fetch(url).await { Ok(b) => Some(b), Err(e) => return api_failure(e) }
    } else { None };
    let bundle = job.bundle.clone();
    let parsed = tokio::task::spawn_blocking(move || match fetched {
        Some(bytes) => Snapshot::parse(&bytes).map(Some),
        None => read_bundle(&bundle)?.map(|b| Snapshot::parse(&b)).transpose(),
    }).await.unwrap_or_else(|e| Err(format!("the blocking pool lost the snapshot: {e}")));
    let snapshot = match parsed {
        Ok(Some(s)) => s,
        Ok(None) => { crate::engine::log(&format!("[jobs] {BOOTSTRAP}: no catalogue bundle at {}", job.bundle.display())); return Outcome::NothingNew; }
        Err(m) => return Outcome::Failed(m),
    };
    let feeds = Feeds::<data_api::ApiTransport>::Catalogue(&snapshot);
    let outcomes = [stocks(&feeds, &cx).await, alpaca_crypto(&feeds, &cx).await, ibkr(&feeds, &cx).await];
    let failed: Vec<String> = outcomes.iter().filter_map(|o| match o {
        Outcome::Failed(m) | Outcome::Transient(m) | Outcome::RateLimited(m) => Some(m.clone()),
        Outcome::Done | Outcome::NothingNew => None,
    }).collect();
    if !failed.is_empty() { return Outcome::Failed(failed.join("; ")); }
    if outcomes.contains(&Outcome::Done) { Outcome::Done } else { Outcome::NothingNew }
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

/// `Asset.where(category: 'Stock').where("external_id LIKE 'alpaca_%'")`, as SQL: LIKE's case-insensitive match and its `_`
/// wildcard included.
const LEGACY_STOCK: &str = "category = 'Stock' AND external_id LIKE 'alpaca_%'";

fn flag_unset(c: &Connection, cipher: &crate::crypto::Cipher) -> Result<bool, String> {
    Ok(app_config::get(c, cipher, BACKFILL_FLAG)?.is_none_or(|v| v.trim().is_empty()))
}

/// The legacy stock tickers on Alpaca: (id, ticker).
fn legacy_tickers(c: &Connection, alpaca: i64) -> Result<Vec<(i64, String)>, String> {
    let mut s = c.prepare("SELECT t.id, t.ticker FROM tickers t JOIN assets a ON a.id = t.base_asset_id \
                           WHERE t.exchange_id = ?1 AND a.category = 'Stock' AND a.external_id LIKE 'alpaca_%'").map_err(|e| e.to_string())?;
    let rows = s.query_map([alpaca], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string());
    rows
}

/// The backfill's rows, as Ruby walks `Array(result.data && result.data['data'])`: what Ruby would raise on is an error here,
/// before anything is written; a String row or identifier is indexed as a substring and never matches, so it is skipped.
fn backfill_rows(body: &Value) -> Result<Vec<&serde_json::Map<String, Value>>, String> {
    let malformed = |what: &str, v: &Value| Err(format!("stock backfill: malformed payload ({what} is {v}); nothing rewritten"));
    let data = match body { Value::Object(m) => m.get("data").unwrap_or(&Value::Null), Value::Null | Value::String(_) => &Value::Null, v => return malformed("the body", v) };
    let rows: &[Value] = match data { Value::Null | Value::String(_) => &[], Value::Array(a) => a, v => return malformed("data", v) };
    let mut out = vec![];
    for row in rows {
        let row = match row { Value::Object(m) => m, Value::String(_) => continue, v => return malformed("a row", v) };
        // Float#to_s ("1.0e+20") is not serde's spelling ("1e+20"): a float id would miss the existing-canonical check.
        if let Some(Value::Number(n)) = row.get("external_id") {
            if !(n.is_i64() || n.is_u64()) { return Err(format!("stock backfill: external_id {n} is not an integer or a string; nothing rewritten")); }
        }
        let ids: &[Value] = match row.get("identifiers").unwrap_or(&Value::Null) {
            Value::Null | Value::String(_) => &[],
            Value::Object(m) if m.is_empty() => &[],
            Value::Array(a) => a,
            v => return malformed("identifiers", v),
        };
        for i in ids { if !matches!(i, Value::Object(_) | Value::String(_)) { return malformed("an identifier", i); } }
        out.push(row);
    }
    Ok(out)
}

/// MarketData.backfill_canonical_stock_external_ids! (:733-823) on a fetched payload: each legacy alpaca_<uuid> stock row
/// whose symbol maps to exactly one canonical external_id takes it, keeping its id (so every bot, ticker and ledger row
/// referring to it follows). A symbol is excluded when it resolves to more than one canonical, or its canonical carries
/// more than one alpaca identifier; a row is left alone when its canonical already exists. The flag is set unless the
/// payload carries no alpaca identifier at all. Every rewrite is planned, and checked, before the first is written.
fn backfill(c: &Connection, cipher: &crate::crypto::Cipher, body: &Value, now: DateTime<Utc>) -> Result<Vec<i64>, String> {
    use std::collections::{BTreeSet, HashMap, HashSet};
    // Keyed by the raw JSON value, as Ruby's Set and Hash key `false`, `5` and "5" apart.
    let mut canonicals: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut alpaca_ids: HashMap<String, usize> = HashMap::new();
    let mut raw: HashMap<String, &Value> = HashMap::new();
    for row in backfill_rows(body)? {
        let ext = row.get("external_id").unwrap_or(&Value::Null);
        let key = ext.to_string();
        for i in row.get("identifiers").and_then(Value::as_array).into_iter().flatten() {
            if i["scheme"].as_str() != Some("alpaca") { continue; }
            let value = match &i["value"] { Value::Null => String::new(), Value::String(s) => s.clone(), v => v.to_string() }; // to_s
            canonicals.entry(value.replacen("us_equity:", "", 1)).or_default().insert(key.clone()); // String#sub
            *alpaca_ids.entry(key.clone()).or_default() += 1;
            raw.insert(key.clone(), ext);
        }
    }
    if canonicals.is_empty() { return Ok(vec![]); } // no alpaca identifiers: the flag stays unset for a retry
    let to_canonical: HashMap<&String, &Value> = canonicals.iter().filter_map(|(symbol, set)| {
        let only = set.iter().next().filter(|_| set.len() == 1)?;
        (alpaca_ids[only] == 1).then(|| (symbol, raw[only]))
    }).collect();
    let mut s = c.prepare(&format!("SELECT id, symbol FROM assets WHERE {LEGACY_STOCK} ORDER BY id")).map_err(|e| e.to_string())?;
    let legacy: Vec<(i64, Option<String>)> = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?
        .collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    let mut claimed = HashSet::new();
    let mut plan = vec![];
    for (id, symbol) in legacy {
        let Some(&canonical) = symbol.as_ref().and_then(|s| to_canonical.get(s)) else { continue };
        // The string column's cast (import::string, as the stock upsert writes it): `false` is "f". nil is unmatched.
        let canonical = match canonical {
            Value::Null => continue,
            Value::Array(_) | Value::Object(_) => return Err(format!("stock backfill: canonical external_id {canonical} is not a string; nothing rewritten")),
            v => import::string(v).unwrap_or_default(),
        };
        let exists: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM assets WHERE external_id = ?1)", [&canonical], |r| r.get(0)).map_err(|e| e.to_string())?;
        if exists || !claimed.insert(canonical.clone()) { continue; }
        plan.push((id, canonical));
    }
    let mut rewritten = vec![];
    for (id, canonical) in plan {
        // update_columns: updated_at stays.
        c.execute("UPDATE assets SET external_id = ?1 WHERE id = ?2", rusqlite::params![canonical, id]).map_err(|e| e.to_string())?;
        rewritten.push(id);
    }
    app_config::set(c, cipher, BACKFILL_FLAG, &now.to_rfc3339_opts(SecondsFormat::Secs, true), now)?;
    Ok(rewritten)
}

/// The backfill in one unit, first checking `fence` when given.
async fn backfill_unit(cx: &Cx<'_>, body: Value, fence: Option<import::Fence>) -> Result<(), String> {
    let now = cx.clock.now();
    cx.db.run(move |c, cipher| import::in_transaction(c, cipher, "stock backfill", |c| {
        if let Some(f) = fence { f.check(c)?; }
        backfill(c, cipher, &body, now).map(|ids| ((), import::Touched::Assets(ids)))
    })).await
}

/// The sync is gated on the flag (sync_stocks_from_deltabadger_job.rb:62-64); Rails returns quietly.
async fn backfill_gate(cx: &Cx<'_>) -> Option<Outcome> {
    match cx.db.run(flag_unset).await {
        Ok(false) => None,
        Ok(true) => Some(Outcome::Failed("legacy alpaca_<uuid> stock assets and no completed canonical backfill (data-api gave no alpaca identifiers); stock sync skipped".into())),
        Err(m) => Some(Outcome::Failed(m)),
    }
}

/// Asset::SyncStocksFromDeltabadgerJob#perform, the jitter split out to the scheduler (Spec.jitter): the switch, the
/// backfill's no-op case, the stock assets, then the Alpaca listings.
async fn stocks<T: Transport>(feeds: &Feeds<'_, T>, cx: &Cx<'_>) -> Outcome {
    let now = cx.clock.now();
    let gate = cx.db.run(move |c, cipher| {
        // The per-container emergency switch: any value but "false" means on (:43-50).
        if app_config::get(c, cipher, "stock_sync_enabled")?.as_deref() == Some("false") {
            return Ok(Err("stock sync is switched off (app_configs stock_sync_enabled = false)".to_string()));
        }
        // backfill_canonical_stock_external_ids!: with no legacy row it only sets its flag (:741-744).
        let legacy: i64 = c.query_row(&format!("SELECT count(*) FROM assets WHERE {LEGACY_STOCK}"), [], |r| r.get(0)).map_err(|e| e.to_string())?;
        if legacy == 0 && flag_unset(c, cipher)? {
            app_config::set(c, cipher, BACKFILL_FLAG, &now.to_rfc3339_opts(SecondsFormat::Secs, true), now)?; // Time.current.iso8601
        }
        Ok(Ok(legacy > 0))
    }).await;
    let legacy = match gate { Ok(Ok(l)) => l, Ok(Err(m)) | Err(m) => return Outcome::Failed(m) };
    if legacy && !feeds.catalogue() {
        // The backfill's own fetch (:746-753): a 429 or a network failure raises out of the job; any other failure leaves
        // the flag as it is.
        match feeds.get(STOCK_ASSETS).await {
            Err(e) if e.rate_limited() || matches!(e, ApiError::Transient(_)) => return api_failure(e),
            Err(_) => {}
            Ok(body) => if let Err(m) = backfill_unit(cx, body, None).await { return Outcome::Failed(m); },
        }
        if let Some(out) = backfill_gate(cx).await { return out; }
    }

    // sync_stocks_from_deltabadger! (:428-463). A failure skips the listings (sync_stocks_from_deltabadger_job.rb:66-73).
    let body = match feeds.get(STOCK_ASSETS).await { Ok(b) => b, Err(e) => return api_failure(e) };
    // A catalogue source already at this revision (or without evidence) imports nothing; the listings still go on.
    let assets = match admit(feeds, cx, STOCK_ASSETS_FEED, &body).await { Ok(rev) => Some(rev), Err(Outcome::Failed(m)) => return Outcome::Failed(m), Err(_) => None };
    if legacy && feeds.catalogue() {
        // From the catalogue the backfill reads only a payload that passed validation and holds the fence: one without
        // evidence, or older than the import, rewrites nothing and sets no flag.
        if let Some(Some(rev)) = assets {
            if let Err(m) = backfill_unit(cx, body.clone(), Some(rev.fence())).await { return fenced(m); }
        }
        if let Some(out) = backfill_gate(cx).await { return out; }
    }
    if let Some(rev) = assets {
        let url = feeds.public_url();
        let rows = match cx.db.run(move |_, _| Ok(import::plan_stock_assets(&take_list(body), &url))).await { Ok(r) => r, Err(m) => return Outcome::Failed(m) };
        // A catalogue import's takeover marks its own record; the stock job's is data-api's.
        if rev.is_none() { if let Err(m) = incomplete(cx, STOCKS, now).await { return Outcome::Failed(m); } }
        if let Err(m) = import::import_stock_assets(&cx.db, rows, now, rev.map(Rev::fence)).await { return fenced(m); }
        if let out @ (Outcome::NothingNew | Outcome::Failed(_)) = finished(cx, rev, false).await { return out; }
    }

    // sync_alpaca_listings_from_deltabadger! (:475-582).
    let body = match feeds.get(ALPACA_LISTINGS).await { Ok(b) => b, Err(e) => return api_failure(e) };
    let rev = match admit(feeds, cx, STOCKS, &body).await { Ok(r) => r, Err(out) => return out };
    let step = cx.db.run(move |c, cipher| {
        import::in_transaction(c, cipher, "usd", |c| import::ensure_usd(c, true, now))?;
        // fractionable: false is dropped (:495), then Invariant B and the defaults (:500-504), before the guard counts.
        let listings: Vec<Value> = take_list(body).into_iter().filter(|l| l["fractionable"] != Value::Bool(false)).map(stock_listing).collect();
        let Some(alpaca) = alpaca_id(c)? else { return Ok(Err(Outcome::NothingNew)) };
        let resolved = import::ticker_records_for(c, &listings, Some("Stock"))?.len() as i64; // import_tickers!' own builder, post-dedup (:514-520)
        let last_good = app_config::get(c, cipher, ALPACA_LISTINGS_LAST_GOOD)?;
        if stocks_degraded(resolved, last_good.as_deref()) { return Ok(Err(degraded(resolved, listings.len(), last_good.as_deref()))); }
        // The legacy collision guard (:539-560): a listing whose ticker a legacy stock ticker on Alpaca holds is dropped.
        let held = legacy_tickers(c, alpaca)?;
        let (ids, names): (std::collections::HashSet<i64>, std::collections::HashSet<String>) = held.into_iter().unzip();
        let listings: Vec<Value> = listings.into_iter().filter(|l| !l["ticker"].as_str().is_some_and(|t| names.contains(t))).collect();
        let mut plan = import::plan_tickers(c, alpaca, &listings, Some("Stock"))?;
        // The sweep never marks a legacy stock ticker unavailable (:572-576).
        plan.sweep.retain(|id| !ids.contains(id));
        let mut bases: Vec<String> = vec![];
        for b in listings.iter().filter_map(|l| l["base_external_id"].as_str()) { if !bases.iter().any(|x| x == b) { bases.push(b.into()); } }
        let partial = rev.is_some() && (import::count_assets(c, &bases)? as usize) < bases.len();
        if rev.is_some_and(|r| !r.complete) || partial { plan.sweep.clear(); } // an incomplete revision is no delisting
        Ok(Ok((alpaca, resolved, plan, partial)))
    }).await;
    let (alpaca, resolved, plan, partial) = match step { Ok(Ok(x)) => x, Ok(Err(out)) => return out, Err(m) => return Outcome::Failed(m) };
    if let Err(m) = import::publish_tickers_fenced(&cx.db, alpaca, plan, now, rev.map(Rev::fence)).await { return fenced(m); }
    if let Err(m) = cx.db.run(move |c, cipher| app_config::set(c, cipher, ALPACA_LISTINGS_LAST_GOOD, &resolved.to_string(), now)).await { return Outcome::Failed(m); }
    finished(cx, rev, partial).await
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
