//! Reference data only a Rails job refreshes. While the engine owns an install, Rails is stopped
//! and its recurring jobs do not run, so each source the tick reads is trusted only within a bound of about two missed
//! refreshes of the job that refreshes it. Past it the engine refuses the bot's tick (tick.rs) and `check` refuses the install
//! (eligibility::check_install_at). A source Rails stamps nowhere is `Unknown`: logged at the door, never refused.
//! After the takeover a source is as fresh as its job's last complete run here (jobs::state), never its column's newest
//! stamp, which a partial import of this engine may have moved; `check` reports every source (`report`).
use super::model::{self, Bot};
use super::EngineError;
use crate::codec::{format_time, parse_time};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};

pub struct Source {
    pub name: &'static str,
    pub column: &'static str,
    pub refreshed_by: &'static str,
    pub max_age_secs: i64,
    /// The scheduler job that refreshes it: its record (`rust_job.<name>`) is the source's freshness after the takeover.
    pub job: &'static str,
    /// The column's newest stamp; `?1` is the exchange id when `per_exchange`.
    pub newest_sql: &'static str,
    pub per_exchange: bool,
    /// The `app_configs` row only a successful Rails sync writes: without it the column measures nothing (Unknown).
    pub gate: Option<&'static str>,
}

/// About two missed refreshes: two of the refreshing job's periods, plus one hour for a late run. The hour covers the
/// widest late-run window of any job here: the stock jitter (up to 15 min), the stock retry chain (3 + 18 + 83 + 258 s),
/// the indices retry chain (3 x 15 min), and Solid Queue's 1 h concurrency lease. tests/staleness.rs pins every bound
/// against its job's schedule (jobs::reference::specs).
pub const fn bound_secs(period_hours: i64) -> i64 { (2 * period_hours + 1) * 3600 }

/// The Alpaca crypto catalog: each pair's available and trading_enabled flags, its decimals and minimum sizes, which the tick
/// derives members, sizes, minimums and the cap's precision floor from. Its stamp is the newest `exchange_assets.updated_at`
/// among the Alpaca exchange's Cryptocurrency assets, NOT `tickers.updated_at`: Ticker::TechnicallyAnalyzable writes a
/// ticker's all-time high with `update!` (technically_analyzable.rb:125), which moves `tickers.updated_at` without any sync.
/// `exchange_assets` rows are written by MarketData.import_tickers! alone (market_data.rb:323-327: `ExchangeAsset.upsert_all`
/// with `updated_at: now`, which rewrites an unchanged row too), and for this exchange's crypto assets import_tickers! is
/// reached only from MarketData.sync_alpaca_crypto_listings_from_deltabadger! (:659), after its degraded-payload guard
/// (:647). Asset::SyncAlpacaCryptoFromDeltabadgerJob runs it daily at 10:15 UTC (config/recurring.yml:58-60). The generic
/// venue sync skips Alpaca (Exchange::SyncTickersAndAssetsJob returns for a stock venue; Exchange::SyncAllTickersAndAssetsJob
/// excludes Exchange::STOCK_TYPES), Alpaca's fee fetch writes nothing (exchanges/alpaca.rb:458), and the self-hosted
/// Exchange::SyncAlpacaAssetsJob only creates rows (`find_or_create_by!`, sync_alpaca_assets_job.rb:62, :122).
///
/// Whether this install runs that sync at all: the `app_configs` row `alpaca_crypto_listings_last_good_count`, which only a
/// successful run writes (market_data.rb:668). Without it (a self-hosted install, or one that never synced) no stamp
/// measures the catalog: the verdict is Unknown.
///
/// Bound: two missed daily refreshes (2 × 24 h), plus 1 h for a late run (its retry_on chain takes about 6 minutes; its
/// concurrency lease is 1 h): 49 h.
pub const ALPACA_CRYPTO_TICKERS: Source = Source {
    name: "Alpaca crypto tickers", column: "exchange_assets.updated_at",
    refreshed_by: "Asset::SyncAlpacaCryptoFromDeltabadgerJob, daily at 10:15 UTC", max_age_secs: bound_secs(24),
    job: crate::jobs::reference::ALPACA_CRYPTO,
    newest_sql: "SELECT max(ea.updated_at) FROM exchange_assets ea JOIN assets a ON a.id = ea.asset_id \
                 WHERE ea.exchange_id = ?1 AND a.category = 'Cryptocurrency'",
    per_exchange: true, gate: Some(ALPACA_CRYPTO_SYNCED_KEY),
};
/// MarketData::ALPACA_CRYPTO_LISTINGS_LAST_GOOD_KEY.
pub const ALPACA_CRYPTO_SYNCED_KEY: &str = "alpaca_crypto_listings_last_good_count";

pub struct Stale { pub source: &'static str, pub message: String }

/// What the engine knows of a bot's reference data at `now`.
pub enum Verdict { Fresh, Stale(Stale), Unknown(Stale) }


/// Alpaca's stock and ETF listings: sync_alpaca_listings_from_deltabadger! upserts the exchange asset of every listing it
/// resolves (import_tickers!, market_data.rb:323-327) and writes `alpaca_listings_last_good_count` only on success
/// (:575). Daily at 10:00 + up to 15 min (recurring.yml:53-56).
pub const ALPACA_STOCK_TICKERS: Source = Source {
    name: "Alpaca stock tickers", column: "exchange_assets.updated_at",
    refreshed_by: "Asset::SyncStocksFromDeltabadgerJob, daily at 10:00 UTC + up to 15 min", max_age_secs: bound_secs(24),
    job: crate::jobs::reference::STOCKS,
    newest_sql: "SELECT max(ea.updated_at) FROM exchange_assets ea JOIN assets a ON a.id = ea.asset_id \
                 WHERE ea.exchange_id = ?1 AND a.category = 'Stock'",
    per_exchange: true, gate: Some("alpaca_listings_last_good_count"),
};

/// A crypto venue's catalogue (import_tickers!, market_data.rb:316-338), every 4 h at :15 (recurring.yml:13-15).
/// ponytail: no gate row exists for it, and other jobs may touch a venue's exchange_assets; `check` reports it, nothing
/// refuses on it until the engine runs bots on those venues.
pub const VENUE_TICKERS: Source = Source {
    name: "venue tickers", column: "exchange_assets.updated_at",
    refreshed_by: "Exchange::SyncAllTickersAndAssetsJob, every 4 h at :15 UTC", max_age_secs: bound_secs(4),
    job: crate::jobs::reference::TICKERS,
    newest_sql: "SELECT max(updated_at) FROM exchange_assets WHERE exchange_id = ?1", per_exchange: true, gate: None,
};

/// Index membership and weights (import_indices!, market_data.rb:266-273, :904-905), daily at 10:30 (recurring.yml:33-35).
pub const INDICES: Source = Source {
    name: "Indices", column: "indices.updated_at",
    refreshed_by: "Index::SyncFromCoingeckoJob, daily at 10:30 UTC", max_age_secs: bound_secs(24),
    job: crate::jobs::reference::INDICES, newest_sql: "SELECT max(updated_at) FROM indices", per_exchange: false, gate: None,
};

/// The crypto catalogue's market caps (import_assets!, market_data.rb:233-245, :885-886), daily at 00:20 + up to 5 min.
pub const CRYPTO_ASSETS: Source = Source {
    name: "Crypto assets", column: "assets.updated_at",
    refreshed_by: "Asset::FetchAllAssetsDataFromCoingeckoJob, daily at 00:20 UTC + up to 5 min", max_age_secs: bound_secs(24),
    job: crate::jobs::reference::ASSETS,
    newest_sql: "SELECT max(updated_at) FROM assets WHERE category = 'Cryptocurrency'", per_exchange: false, gate: None,
};

pub const SOURCES: [&Source; 5] = [&ALPACA_CRYPTO_TICKERS, &ALPACA_STOCK_TICKERS, &VENUE_TICKERS, &INDICES, &CRYPTO_ASSETS];

/// Rails' last ownership window runs from `HANDED_BACK_AT` (written in the handback's own transaction) to `TAKEN_OVER_AT`
/// (written with the claim). Both are plain `app_configs` rows owned by `lease.rs`.
pub use crate::lease::{HANDED_BACK_AT, TAKEN_OVER_AT};

/// Rails' rule: the newest stamp of the column, where the install runs the sync at all (its gate row exists).
fn column_at(c: &Connection, s: &Source, exchange_id: Option<i64>) -> Result<Option<DateTime<Utc>>, EngineError> {
    if let Some(key) = s.gate {
        if !c.query_row("SELECT EXISTS (SELECT 1 FROM app_configs WHERE key = ?1)", [key], |r| r.get::<_, bool>(0))? { return Ok(None); }
    }
    let text: Option<String> = match exchange_id {
        Some(id) if s.per_exchange => c.query_row(s.newest_sql, [id], |r| r.get(0))?,
        _ => c.query_row(s.newest_sql, [], |r| r.get(0))?,
    };
    text.map(|t| parse_time(&t).map_err(|e| EngineError::Data(format!("{}: {e:?}", s.column)))).transpose()
}

/// Rails' column, where it can be trusted: always while the job has no import of this engine left incomplete
/// (`incomplete_since`, set before an import's first unit, cleared with its success); otherwise only a stamp Rails made
/// inside its last ownership window: after the last handback and, once this engine has taken the install over again, not
/// after that takeover. Without a handback stamp: None, freshness is the record alone.
fn trusted_column(c: &Connection, s: &Source, exchange_id: Option<i64>) -> Result<Option<DateTime<Utc>>, EngineError> {
    let at = column_at(c, s, exchange_id)?;
    let incomplete = crate::jobs::state::find(c, s.job, None).map_err(EngineError::Data)?.is_some_and(|st| st.incomplete_since.is_some());
    if !incomplete { return Ok(at); }
    let Some(back) = crate::lease::plain_at(c, HANDED_BACK_AT)? else { return Ok(None) };
    let taken = crate::lease::plain_at(c, TAKEN_OVER_AT)?.filter(|t| *t > back); // older: Rails owns the install now
    Ok(at.filter(|a| *a > back && taken.is_none_or(|t| *a <= t)))
}

/// The source's last complete refresh by this engine's record: None without a record (no takeover yet); else the newer of
/// the job's last success, which the runner writes only once the whole import has published, and Rails' baseline read at
/// the takeover (`seed`). A run that fails after committing some units moves neither.
fn record_at(c: &Connection, s: &Source) -> Result<Option<Option<DateTime<Utc>>>, EngineError> {
    Ok(crate::jobs::state::find(c, s.job, None).map_err(EngineError::Data)?.map(|st| st.last_success_at.max(st.rails_at)))
}

/// "; <job>: <its state>" for a refusal or a report line.
fn job_state(c: &Connection, s: &Source) -> String {
    match crate::jobs::state::find(c, s.job, None) {
        Ok(Some(st)) if st.last_success_at.is_some() || st.last_error_at.is_some() => format!("; {}: {}", s.job, st.describe()),
        _ => format!("; {}: never run here", s.job),
    }
}

/// Past its bound at `now`, refreshed (completely) at `at`.
fn judge(c: &Connection, s: &Source, at: DateTime<Utc>, now: DateTime<Utc>) -> Option<Stale> {
    let age = (now - at).num_seconds();
    (age > s.max_age_secs).then(|| Stale { source: s.name, message: format!(
        "reference data stale: {} ({}, newest complete refresh {}) is {}h {}m old, over its {}h bound; refreshed by {}{}; \
         the bot does not tick until it is refreshed", s.name, s.column, format_time(at), age / 3600, age % 3600 / 60,
        s.max_age_secs / 3600, s.refreshed_by, job_state(c, s)) })
}

/// The door (`check` and the takeover, through eligibility::check_install_at): the newer of the job record and Rails'
/// column where it is trusted, so a takeover after Rails refreshed the data is admitted even when a record from an earlier
/// takeover is older, and a partial import of this engine never is. With neither, Unknown: noted, never refused.
/// Kraken has no source yet: it is not connected for real.
fn source_verdict(c: &Connection, bot: &Bot, s: &Source, now: DateTime<Utc>) -> Result<Verdict, EngineError> {
    if model::exchange_type(c, bot)? != "Exchanges::Alpaca" { return Ok(Verdict::Fresh); }
    match record_at(c, s)?.flatten().max(trusted_column(c, s, s.per_exchange.then_some(bot.exchange_id))?) {
        Some(at) => Ok(judge(c, s, at, now).map_or(Verdict::Fresh, Verdict::Stale)),
        None => Ok(Verdict::Unknown(Stale { source: s.name, message: format!(
            "reference data unknown: {} has no sync stamp on this install (no app_configs {ALPACA_CRYPTO_SYNCED_KEY}, or no {} row); \
             not refused; after the takeover only the engine's own completed run of its job can show it fresh{}", s.name, s.column, job_state(c, s)) })),
    }
}

/// The tick (tick.rs). Once the job has a record (seeded at the takeover), only its completed runs count: no complete
/// refresh at all is stale. Before any takeover, the door's verdict (Unknown is not stale).
fn source_stale(c: &Connection, bot: &Bot, s: &Source, now: DateTime<Utc>) -> Result<Option<Stale>, EngineError> {
    if model::exchange_type(c, bot)? != "Exchanges::Alpaca" { return Ok(None); }
    Ok(match record_at(c, s)? {
        Some(Some(at)) => judge(c, s, at, now),
        Some(None) => Some(Stale { source: s.name, message: format!(
            "reference data stale: {} ({}) has had no complete refresh since this engine took the install over; refreshed by {}{}; \
             the bot does not tick until it is refreshed", s.name, s.column, s.refreshed_by, job_state(c, s)) }),
        None => match source_verdict(c, bot, s, now)? { Verdict::Stale(x) => Some(x), Verdict::Fresh | Verdict::Unknown(_) => None },
    })
}

/// A source this install has: the source, its venue for a per-venue source, and a label for `check`.
type Target = (&'static Source, Option<i64>, String);

fn targets(c: &Connection) -> Result<Vec<Target>, EngineError> {
    let mut targets: Vec<Target> = vec![];
    if let Some(alpaca) = c.query_row("SELECT id FROM exchanges WHERE type = 'Exchanges::Alpaca' ORDER BY id LIMIT 1", [], |r| r.get::<_, i64>(0)).optional()? {
        targets.push((&ALPACA_CRYPTO_TICKERS, Some(alpaca), String::new()));
        targets.push((&ALPACA_STOCK_TICKERS, Some(alpaca), String::new()));
    }
    let mut s = c.prepare("SELECT id, type FROM exchanges WHERE available = 1 AND type NOT IN ('Exchanges::Alpaca', 'Exchanges::Ibkr') ORDER BY id")?;
    for venue in s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
        let (id, ty) = venue?;
        targets.push((&VENUE_TICKERS, Some(id), format!(" ({})", crate::jobs::reference::name_id(&ty))));
    }
    targets.push((&INDICES, None, String::new()));
    targets.push((&CRYPTO_ASSETS, None, String::new()));
    Ok(targets)
}

/// The takeover (handover::take_over, in the claim's transaction): every source's job without a record gets one holding
/// Rails' baseline, the column as Rails left it (`column_at`; for a per-venue source the oldest venue's, none where a venue
/// has no stamp). A job with a record keeps it: once this engine has run it, only its completed runs count.
pub fn seed(c: &Connection, now: DateTime<Utc>) -> Result<(), EngineError> {
    let targets = targets(c)?;
    for s in SOURCES {
        let mut baseline: Option<Option<DateTime<Utc>>> = None;
        for (src, exchange, _) in targets.iter().filter(|(src, ..)| src.name == s.name) {
            let at = column_at(c, src, *exchange)?;
            baseline = Some(baseline.map_or(at, |b| b.min(at)));
        }
        crate::jobs::state::seed(c, s.job, baseline.flatten(), now).map_err(EngineError::Data)?;
    }
    Ok(())
}

/// `deltabadger check`: each source this install has, its last complete refresh as the door reads it, its age against
/// its bound, and its job's state. Informational: only a source an eligible bot depends on refuses (check_install_at).
pub fn report(c: &Connection, now: DateTime<Utc>) -> Result<Vec<String>, EngineError> {
    let mut lines = vec![];
    for (src, exchange, label) in targets(c)? {
        lines.push(match record_at(c, src)?.flatten().max(trusted_column(c, src, exchange)?) {
            None => format!("{}{label}: no row ({}){}", src.name, src.column, job_state(c, src)),
            Some(at) => {
                let age = (now - at).num_seconds().max(0);
                let verdict = if age > src.max_age_secs { ", STALE" } else { "" };
                format!("{}{label}: newest complete refresh {}, {}h {}m old, {}h bound{verdict}{}", src.name, format_time(at),
                        age / 3600, age % 3600 / 60, src.max_age_secs / 3600, job_state(c, src))
            }
        });
    }
    let mut holds = c.prepare("SELECT id,json_extract(transient_data,'$.rust_split_hold.reason') FROM bots WHERE json_extract(transient_data,'$.rust_split_hold.reason') IS NOT NULL ORDER BY id")?;
    for line in holds.query_map([], |r| Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))? {
        let (id,reason)=line?; lines.push(format!("bot {id}: last split refusal: {reason}; stop the engine, run sync ledger, correct the ledger with the Rails grouping fix if still inconsistent, then restart; no trade is needed"));
    }
    Ok(lines)
}


/// Only sources this bot sizes from; index candidates can include both stocks and crypto.
fn bot_sources(c: &Connection, bot: &Bot) -> Result<Vec<&'static Source>, EngineError> {
    if model::exchange_type(c, bot)? != "Exchanges::Alpaca" { return Ok(vec![]); }
    let index = bot.bot_type == "Bots::DcaIndex";
    let mut sources = vec![];
    let (stocks, crypto): (bool, bool) = if index {
        let pairs = serde_json::to_string(&super::index::candidate_pairs(c, bot)?).map_err(|e| EngineError::Data(e.to_string()))?;
        c.query_row("SELECT coalesce(max(a.category != 'Cryptocurrency'),0), coalesce(max(a.category = 'Cryptocurrency'),0) \
                     FROM tickers t JOIN assets a ON a.id=t.base_asset_id WHERE t.exchange_id=?1 AND t.quote_asset_id=?2 \
                     AND t.ticker IN (SELECT value FROM json_each(?3)) AND t.available=1 AND t.trading_enabled=1",
            rusqlite::params![bot.exchange_id, bot.quote_asset_id(), pairs], |r| Ok((r.get(0)?, r.get(1)?)))?
    } else {
        let ids = serde_json::to_string(&bot.asset_ids()).map_err(|_| EngineError::Data("unreadable allocations".into()))?;
        c.query_row("SELECT coalesce(max(category != 'Cryptocurrency'),0), coalesce(max(category = 'Cryptocurrency'),0) \
                     FROM assets WHERE id IN (SELECT value FROM json_each(?1))", [ids], |r| Ok((r.get(0)?, r.get(1)?)))?
    };
    if stocks { sources.push(&ALPACA_STOCK_TICKERS); }
    if crypto { sources.push(&ALPACA_CRYPTO_TICKERS); }
    if index { sources.push(&INDICES); if crypto { sources.push(&CRYPTO_ASSETS); } }
    Ok(sources)
}

pub fn ledger_stale(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Option<Stale>, EngineError> {
    // Timestamp, incomplete marker, producer stamp and current ciphertext share one read snapshot.
    // Placement already calls this inside its intent transaction; other callers get the same consistency.
    if c.is_autocommit() {
        let tx = c.unchecked_transaction()?;
        let stale = ledger_stale(&tx, bot, now)?;
        tx.commit()?;
        return Ok(stale);
    }
    if model::exchange_type(c, bot)? != "Exchanges::Alpaca" || model::all_crypto(c, bot)? { return Ok(None); }
    let key: Option<i64> = c.query_row("SELECT id FROM api_keys WHERE user_id=?1 AND exchange_id=?2 AND key_type=0 LIMIT 1", rusqlite::params![bot.user_id, bot.exchange_id], |r| r.get(0)).optional()?;
    let state = match key {
        Some(key) => crate::jobs::state::read(c, "ledger_sync", Some(&key.to_string())).map_err(EngineError::Data)?,
        None => crate::jobs::state::JobState::default(),
    };
    let produced_by_current = match key {
        Some(id) => crate::sync::cache::ledger_current_for(c,id)?,
        None => false,
    };
    if produced_by_current && state.incomplete_since.is_none() && state.last_success_at.is_some_and(|at| at <= now && (now-at).num_seconds() <= bound_secs(24)) { return Ok(None); }
    let provenance = if produced_by_current { "" } else { "credential provenance missing or changed; " };
    Ok(Some(Stale { source: "Alpaca account ledger", message: format!("reference data stale: Alpaca account ledger; ledger_sync:{}: {provenance}{}; a complete ledger refresh is required before this bot can trade", key.map_or_else(|| "missing".into(), |k| k.to_string()), state.describe_venue()) }))
}

pub fn verdict(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Verdict, EngineError> {
    // A restart must be able to start the index refresher. Runtime stale() still refuses all orders.
    if let Some(s) = super::provider::stale(c, bot)? { return Ok(Verdict::Unknown(s)); }
    let mut unknown = None;
    for source in bot_sources(c, bot)? {
        match source_verdict(c, bot, source, now)? {
            Verdict::Stale(s) => return Ok(Verdict::Stale(s)),
            Verdict::Unknown(s) => { if unknown.is_none() { unknown = Some(s); } }
            Verdict::Fresh => {}
        }
    }
    // At the door an unstamped ledger must allow takeover: only the running scheduler can refresh it.
    if let Some(s) = ledger_stale(c, bot, now)? { unknown = Some(s); }
    Ok(unknown.map_or(Verdict::Fresh, Verdict::Unknown))
}

pub fn stale(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Option<Stale>, EngineError> {
    if let Some(s) = super::provider::stale(c, bot)? { return Ok(Some(s)); }
    for source in bot_sources(c, bot)? { if let Some(s) = source_stale(c, bot, source, now)? { return Ok(Some(s)); } }
    ledger_stale(c, bot, now)
}
