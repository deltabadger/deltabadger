//! Bot::Composition::Measurable#metrics_with_current_prices: the walk's figures marked at the venue's current
//! prices, with one more chart point at `now`. When the prices cannot be had or cannot be trusted, the walk's own
//! figures stand and the result says so (`prices_stale`).
use super::at::At;
use super::budget;
use super::db::{Subject, Ticker};
use super::market::{Failure, MarketData, Venue};
use super::num::Num;
use super::splits::{self, QUARANTINE_SECONDS};
use super::walk::{self, AssetValue, Metrics, Unpriced, Why};
use super::FiguresError;
use rusqlite::Connection;

/// Allocatable#ticker_for_key: the ticker a holding is priced on. Its asset's, or, for a holding recorded only by a
/// symbol string, the one the venue spells that way.
pub fn ticker_for_key<'a>(s: &'a Subject, metrics: &Metrics, key: &str) -> Option<&'a Ticker> {
    match metrics.key_assets.iter().find(|(k, _)| k == key).and_then(|(_, id)| *id) {
        // index_by keeps the last of two tickers that share a key.
        Some(asset_id) => s.tickers.iter().rev().find(|t| t.base_asset_id == asset_id),
        None => {
            let spelling = metrics.key_strings.iter().find(|(k, _)| k == key).and_then(|(_, strings)| strings.first()).map_or(key, String::as_str);
            s.tickers.iter().rev().find(|t| t.base == spelling)
        }
    }
}

pub fn venue(s: &Subject) -> Result<Venue, FiguresError> {
    match (s.bot.exchange_id, &s.bot.exchange_type) {
        (Some(exchange_id), Some(exchange_type)) => Ok(Venue { exchange_id, exchange_type: exchange_type.clone() }),
        _ => Err(FiguresError::Data(format!("bot {} has no exchange", s.bot.id))),
    }
}

fn stale(mut metrics: Metrics) -> Metrics { metrics.prices_stale = true; metrics }

/// Restatable#restated_prices_untrusted?: a split this bot can see and cannot size, or a restatement newer than
/// the market's last chance to reprice the security.
fn restated_prices_untrusted(c: &Connection, s: &Subject, metrics: &Metrics, now: At) -> Result<bool, FiguresError> {
    if splits::unresolved(c, s.bot.user_id, &s.orders, &metrics.holdings(), now)? { return Ok(true); }
    let restated_at = metrics.walked.as_ref().and_then(|w| w.restated_at);
    Ok(restated_at.is_some_and(|at| now.plus_seconds(-QUARANTINE_SECONDS).is_none_or(|cutoff| at > cutoff)))
}

/// Why a holding has no ticker to be priced on: the venue lists its asset, though not as an available, trading
/// ticker in the bot's quote currency, or lists nothing for it.
pub fn unlisted(c: &Connection, s: &Subject, asset_id: Option<i64>) -> Result<Why, FiguresError> {
    let listed = match (asset_id, s.bot.exchange_id) { (Some(id), Some(exchange)) => !super::db::listings(c, exchange, &[id])?.is_empty(), _ => false };
    Ok(if listed { Why::Delisted } else { Why::NoTicker })
}

pub fn live(c: &Connection, s: &Subject, metrics: &Metrics, market: &dyn MarketData, now: At) -> Result<Metrics, FiguresError> {
    budget::within(|| marked_live(c, s, metrics, market, now))
}

fn marked_live(c: &Connection, s: &Subject, metrics: &Metrics, market: &dyn MarketData, now: At) -> Result<Metrics, FiguresError> {
    let mut data = metrics.clone();
    if data.chart.labels.is_empty() { return Ok(data); }
    if restated_prices_untrusted(c, s, &data, now)? { return Ok(stale(data)); }

    let symbols: Vec<String> = s.tickers.iter().map(|t| t.ticker.clone()).collect();
    let prices = match market.prices(&venue(s)?, &symbols) {
        Ok(prices) => prices,
        Err(Failure::Failed(_)) => return Ok(stale(data)),
        Err(Failure::Raised(error)) => return Err(FiguresError::Raised(error)),
        Err(Failure::NotComputed(reason)) => return Err(FiguresError::NotComputed(reason)),
    };

    let mut total_value = Num::Int(0);
    // Only holdings that priced land here: a holding missing from it has no mark at `now`.
    let mut live_prices: Vec<(String, Num)> = vec![];
    let mut asset_values: Vec<(String, AssetValue)> = vec![];
    // What Rails skips in silence is named here, beside the figures.
    let mut unpriced: Vec<Unpriced> = vec![];
    for (key, asset) in &data.asset_breakdown {
        // A holding sold out keeps its ledger row and has nothing left to show.
        if !asset.amount.is_positive() { continue; }
        let asset_id = data.key_assets.iter().find(|(k, _)| k == key).and_then(|(_, id)| *id);
        let Some(ticker) = ticker_for_key(s, &data, key) else {
            unpriced.push(Unpriced { key: key.clone(), asset_id, reason: unlisted(c, s, asset_id)? });
            continue;
        };
        let Some((_, price)) = prices.iter().find(|(code, _)| *code == ticker.ticker) else {
            unpriced.push(Unpriced { key: key.clone(), asset_id, reason: Why::NoPrice });
            continue;
        };
        // The price is used here, so here is where one that is no number stops the figure.
        let price = Num::Dec(price.clone()?);
        live_prices.push((key.clone(), price.clone()));
        let value = asset.amount.mul(&price)?;
        total_value = total_value.add(&value)?;
        let avg_price = asset.quote_invested.div(&asset.amount)?;
        let pnl_percentage = if asset.quote_invested.is_positive() { value.sub(&asset.quote_invested)?.div(&asset.quote_invested)? } else { Num::Int(0) };
        // Judged on the tax quantity, and never on a position whose lots include one of unknown cost.
        let tax_units = Num::Dec(asset.tax_units.to_d()?);
        let harvestable = tax_units.is_positive() && !asset.tax_cost_unknown && tax_units.mul(&price)?.lt(&Num::Dec(asset.tax_basis.to_d()?))?;
        asset_values.push((key.clone(), AssetValue {
            amount: asset.amount.clone(), quote_invested: asset.quote_invested.clone(), current_value: value, current_price: price, avg_price,
            pnl_percentage, tax_basis: asset.tax_basis.clone(), harvestable,
        }));
    }

    // The pair bot's rule, for the one-asset basket that replaces it: a lone holding it cannot price makes the
    // figures stale. Not for a wider basket, where one unpriced member is simply left out of the value.
    if s.bot.kind == super::db::Kind::Basket && s.bot.base_asset_ids.len() == 1 {
        let key = data.key_for(s.bot.base_asset_ids[0]).map(str::to_string);
        // Bots::DcaMultiAsset#base_assets holds only assets that exist.
        let exists = !super::db::existing_assets(c, &s.bot.base_asset_ids)?.is_empty();
        let held = key.as_ref().filter(|_| exists).and_then(|key| data.asset_breakdown.iter().find(|(k, _)| k == key)).map(|(_, asset)| asset.amount.to_d()).transpose()?;
        if held.is_some_and(|amount| amount.is_positive()) && !live_prices.iter().any(|(k, _)| Some(k) == key.as_ref()) { return Ok(stale(data)); }
    }

    let cash = Num::Dec(data.rebalance_cash.to_d()?);
    total_value = total_value.add(&cash)?;
    data.pnl = Some(walk::pnl(&data.total_quote_amount_invested, &total_value)?);
    data.total_amount_value_in_quote = total_value.clone();
    data.asset_values = asset_values;
    data.chart.value.push(total_value);
    data.chart.invested.push(data.total_quote_amount_invested.clone());
    data.chart.labels.push(now);
    // extra_series stays parallel with the labels: the chart reads holdings by position.
    data.chart.extra.push(data.asset_breakdown.iter().map(|(key, asset)| (key.clone(), asset.amount.clone())).collect());
    data.chart.invested_by.push(data.asset_breakdown.iter().map(|(key, asset)| (key.clone(), asset.quote_invested.clone())).collect());
    data.chart.cash.push(cash);
    data.live_prices = Some(live_prices);
    data.unpriced = unpriced;
    Ok(data)
}
