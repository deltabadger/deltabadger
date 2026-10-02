//! Reference data only a Rails job refreshes. While the engine owns an install, Rails is stopped
//! and its recurring jobs do not run, so each source the tick reads is trusted only within a bound of about two missed
//! refreshes. Past it the engine refuses the bot's tick (tick.rs) and `check` refuses the install
//! (eligibility::check_install_at). A source Rails stamps nowhere is `Unknown`: logged at the door, never refused; after the
//! takeover only a job record the engine keeps itself can show such a source fresh.
use super::model::{self, Bot};
use super::EngineError;
use crate::codec::parse_time;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

pub struct Source { pub name: &'static str, pub column: &'static str, pub refreshed_by: &'static str, pub max_age_secs: i64 }

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
    refreshed_by: "Asset::SyncAlpacaCryptoFromDeltabadgerJob, daily at 10:15 UTC", max_age_secs: 49 * 3600,
};
/// MarketData::ALPACA_CRYPTO_LISTINGS_LAST_GOOD_KEY.
pub const ALPACA_CRYPTO_SYNCED_KEY: &str = "alpaca_crypto_listings_last_good_count";

pub struct Stale { pub source: &'static str, pub message: String }

/// What the engine knows of a bot's reference data at `now`.
pub enum Verdict { Fresh, Stale(Stale), Unknown(Stale) }

/// The verdict for `bot`'s sources. Kraken has none yet: it is not connected for real. Its tickers' bound would be 9 h
/// (Exchange::SyncAllTickersAndAssetsJob every 4 h).
pub fn verdict(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Verdict, EngineError> {
    if model::exchange_type(c, bot)? != "Exchanges::Alpaca" { return Ok(Verdict::Fresh); }
    let s = &ALPACA_CRYPTO_TICKERS;
    let (synced, newest): (bool, Option<String>) = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM app_configs WHERE key = ?2), \
                (SELECT max(ea.updated_at) FROM exchange_assets ea JOIN assets a ON a.id = ea.asset_id \
                  WHERE ea.exchange_id = ?1 AND a.category = 'Cryptocurrency')",
        params![bot.exchange_id, ALPACA_CRYPTO_SYNCED_KEY], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let (true, Some(text)) = (synced, newest) else {
        return Ok(Verdict::Unknown(Stale { source: s.name, message: format!(
            "reference data unknown: {} has no sync stamp on this install (no app_configs {ALPACA_CRYPTO_SYNCED_KEY}, or no {} row); \
             not refused; after the takeover only the engine's own job record (Plan 2f) can show it fresh", s.name, s.column) }));
    };
    let at = parse_time(&text).map_err(|e| EngineError::Data(format!("{}: {e:?}", s.column)))?;
    let age = (now - at).num_seconds();
    if age <= s.max_age_secs { return Ok(Verdict::Fresh); }
    Ok(Verdict::Stale(Stale { source: s.name, message: format!(
        "reference data stale: {} ({}, newest {text}) is {}h {}m old, over its {}h bound; refreshed by {}; \
         the bot does not tick until Rails refreshes it", s.name, s.column, age / 3600, age % 3600 / 60, s.max_age_secs / 3600, s.refreshed_by) }))
}

/// The source `bot` depends on that is past its bound, if any (the tick's refusal). Unknown is not stale: the tick runs.
pub fn stale(c: &Connection, bot: &Bot, now: DateTime<Utc>) -> Result<Option<Stale>, EngineError> {
    Ok(match verdict(c, bot, now)? { Verdict::Stale(s) => Some(s), Verdict::Fresh | Verdict::Unknown(_) => None })
}
