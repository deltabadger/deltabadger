//! The market-data feed a run asks (PLAN-A17 "A17-3 in detail" item 1): the install's provider, data-api or the user's
//! own CoinGecko key, built from `market_data::Settings` once per run by jobs/resolve.rs `configured`. Each method is one
//! question both providers answer in one shape; Rails reads the configuration per call, a run here reads it once.
//!
//! Consumers: tracker/jobs.rs and tracker/prices.rs `fetch_range` (historical, the walk and the backfill), tax/job.rs
//! (historical). The live-price and FX consumers are built beside these by the balance/FX track: the balance
//! `PriceSource` (jobs/data_api.rs, sync/balances.rs) and web/tracker/fx.rs with web/figure/* use `prices` and
//! `exchange_rates`; the fallback key there is `market_data::Settings`' own, never this enum.
use super::coingecko::{self, CoinGecko};
use super::data_api::{ApiError, ApiTransport, Config, DataApi, RateCache};
use crate::market_data::{Provider, Settings};
use crate::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};
use std::sync::Arc;

pub enum Feed<T: Transport> { DataApi(DataApi<T>), CoinGecko(CoinGecko<T>) }

/// Coingecko's guard (coingecko.rb: `return Result::Failure.new('CoinGecko API key not configured') if @api_key.blank?`).
fn no_key() -> ApiError { ApiError::Failed { status: None, message: "CoinGecko API key not configured".into() } }

/// D-CG-429-STOPS-PRICES: what a stopped run answers instead of asking.
fn stopped() -> ApiError { ApiError::Failed { status: Some(429), message: "CoinGecko rate limit met earlier in this run: not asked".into() } }

fn uniq(ids: &[String]) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for id in ids { if !out.contains(id) { out.push(id.clone()); } }
    out
}

impl<T: Transport> Feed<T> {
    /// The feed `settings` name, its clients made by `data_api` and `coingecko` (the key "" when none is set: every call
    /// then fails as Rails' does). None: no provider.
    pub fn from_settings(settings: &Settings, data_api: impl FnOnce(Config) -> DataApi<T>, coingecko: impl FnOnce(&str) -> CoinGecko<T>) -> Option<Self> {
        match &settings.provider {
            Provider::DataApi(config) => Some(Self::DataApi(data_api(config.clone()))),
            Provider::CoinGecko { key } => Some(Self::CoinGecko(coingecko(key.as_deref().unwrap_or("")))),
            Provider::None => None,
        }
    }

    /// The data-api client, when that is the provider.
    pub fn into_data_api(self) -> Option<DataApi<T>> { match self { Self::DataApi(api) => Some(api), Self::CoinGecko(_) => None } }

    /// `{prices: [[ms, p], …]}` (CoinGecko market_chart/range interval=daily precision=full; data-api api/v1/historical_prices)
    pub async fn historical_range(&self, coin: &str, currency: &str, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Value, ApiError> {
        match self {
            Self::DataApi(api) => {
                let (from, to) = (from.timestamp().to_string(), to.timestamp().to_string());
                api.get("/api/v1/historical_prices", &[("coin_id", coin), ("currency", currency), ("from", &from), ("to", &to)], false).await
            }
            Self::CoinGecko(cg) => {
                if !cg.has_key() { return Err(no_key()); }
                if cg.limited() { return Err(stopped()); }
                let out = cg.market_chart_range(coin, currency, from.timestamp(), to.timestamp()).await;
                if out.as_ref().is_err_and(ApiError::rate_limited) { cg.limit(); }
                out
            }
        }
    }

    /// `{id: {cur: p}}` (CoinGecko simple/price unwrapped as-is; data-api api/v1/prices `data` unwrapped)
    pub async fn prices(&self, ids: &[String], currency: &str) -> Result<Value, ApiError> {
        if let Self::CoinGecko(cg) = self { if !cg.has_key() { return Err(no_key()); } }
        let ids = uniq(ids);
        if ids.is_empty() { return Ok(json!({})); }
        match self {
            Self::DataApi(api) => Ok(api.get("/api/v1/prices", &[("coin_ids", &ids.join(",")), ("vs_currencies", currency)], false).await?["data"].clone()),
            Self::CoinGecko(cg) => cg.simple_price(&ids, &currency.to_lowercase()).await,
        }
    }

    /// `{cur: {value, …}}` (CoinGecko exchange_rates `rates`; data-api `data`)
    pub async fn exchange_rates(&self) -> Result<Value, ApiError> {
        match self {
            Self::DataApi(api) => api.exchange_rates().await,
            Self::CoinGecko(cg) => if cg.has_key() { cg.exchange_rates().await } else { Err(no_key()) },
        }
    }

    /// D-CG-HISTORY-WINDOW: the first day a demo key's market_chart/range serves at `now` (the first UTC midnight at or
    /// after `now − 365 days`); None: no clip (pro, data-api). Asks the plan ping once per client; its 429 stops the run.
    pub async fn history_starts(&self, now: DateTime<Utc>) -> Result<Option<NaiveDate>, ApiError> {
        let Self::CoinGecko(cg) = self else { return Ok(None) };
        if !cg.has_key() { return Err(no_key()); }
        if cg.limited() { return Err(stopped()); }
        match cg.plan().await {
            Ok(coingecko::Plan::Pro) => Ok(None),
            Ok(coingecko::Plan::Demo) => Ok(Some(window_start(now))),
            Err(e) => { if e.rate_limited() { cg.limit(); } Err(e) }
        }
    }

    /// D-CG-429-STOPS-PRICES: whether this run's price requests met a 429.
    pub fn rate_limited(&self) -> bool { matches!(self, Self::CoinGecko(cg) if cg.limited()) }
}

/// The first UTC midnight at or after `now − 365 days`.
pub fn window_start(now: DateTime<Utc>) -> NaiveDate {
    let back = now - chrono::TimeDelta::days(365);
    let day = back.date_naive();
    if back.time() == chrono::NaiveTime::MIN { day } else { day.succ_opt().unwrap_or(day) }
}

/// The live transports of both providers under one type, so a run's feed is one `Feed<Live>`.
pub enum Live { DataApi(ApiTransport), CoinGecko(coingecko::LiveTransport) }

impl Transport for Live {
    fn redact_diagnostic(&self, text: &str) -> String {
        match self { Self::DataApi(t) => t.redact_diagnostic(text), Self::CoinGecko(t) => t.redact_diagnostic(text) }
    }
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        match self { Self::DataApi(t) => t.send(r).await, Self::CoinGecko(t) => t.send(r).await }
    }
    async fn send_limited(&self, r: &HttpRequest, limit: usize) -> Result<HttpResponse, TransportError> {
        match self { Self::DataApi(t) => t.send_limited(r, limit).await, Self::CoinGecko(t) => t.send_limited(r, limit).await }
    }
}

impl Feed<Live> {
    /// The live feed of `settings`; the data-api client shares the engine's rate cache.
    pub fn live(settings: &Settings, rates: Arc<RateCache>) -> Option<Self> {
        Self::from_settings(settings, |c| DataApi::live(c).with_rates(rates).map(Live::DataApi),
                            |key| CoinGecko::new(key, Live::CoinGecko(coingecko::LiveTransport::new(key))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venue::http::ScriptedTransport;

    fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }

    #[test]
    fn the_window_starts_at_the_first_midnight_at_or_after_a_year_back() {
        assert_eq!(window_start(at("2026-11-03T00:00:00Z")).to_string(), "2025-11-03");
        assert_eq!(window_start(at("2026-11-03T00:00:00.001Z")).to_string(), "2025-11-04");
        assert_eq!(window_start(at("2026-11-03T23:59:59Z")).to_string(), "2025-11-04");
    }

    #[test]
    fn coingecko_asks_rails_params_and_a_429_stops_the_client() {
        tokio::runtime::Builder::new_current_thread().enable_time().start_paused(true).build().unwrap().block_on(async {
            let t = ScriptedTransport::default();
            t.reply("GET /api/v3/ping", 401, json!({})).reply("GET /api/v3/coins/bitcoin/market_chart/range", 429, json!({}));
            let feed = Feed::CoinGecko(CoinGecko::new("k", t.clone()).with_throttle(Arc::new(coingecko::Throttle::new(30))));
            assert_eq!(feed.history_starts(at("2026-11-03T12:00:00Z")).await, Ok(Some(NaiveDate::from_ymd_opt(2025, 11, 4).unwrap())));
            let (from, to) = (at("2026-01-01T00:00:00Z"), at("2026-01-03T00:00:00Z"));
            assert!(feed.historical_range("bitcoin", "usd", from, to).await.unwrap_err().rate_limited());
            assert!(feed.rate_limited());
            assert!(feed.historical_range("ethereum", "usd", from, to).await.unwrap_err().rate_limited());
            let r = t.requests();
            assert_eq!(r.len(), 2, "nothing asked after the 429");
            assert_eq!(r[1].query, vec![("from", "1767225600".to_string()), ("interval", "daily".into()), ("precision", "full".into()), ("to", "1767398400".into()), ("vs_currency", "usd".into())]);
            let blank = Feed::CoinGecko(CoinGecko::new("", t.clone()));
            assert_eq!(blank.historical_range("bitcoin", "usd", from, to).await.unwrap_err().message(), "CoinGecko API key not configured");
            assert_eq!(t.requests().len(), 2, "a blank key asks nothing");
        });
    }
}
