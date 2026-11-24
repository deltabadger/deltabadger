//! The tracker's missing-prices banner (Rust only, PLAN-venue-prices S1): per venue, the assets with no market price
//! now and the ones the chart could not value, and where no provider is set up, the way to connect one.
use super::{Ctx, WebError};
use crate::tracker::unpriced::{self, VenueGap};
use crate::web::i18n::{self, Arg};
use rusqlite::Connection;

fn line(ctx: &Ctx, key: &str, assets: &[String], venue: Option<&str>) -> String {
    let assets = assets.join(", ");
    let key = if venue.is_some() { key.to_string() } else { format!("{key}_anywhere") };
    format!("    <p class=\"notice__line\">{}</p>\n",
        i18n::t(ctx.locale, &format!("tracker.prices.{key}"), &[("assets", Arg::Text(&assets)), ("venue", Arg::Text(venue.unwrap_or_default()))]))
}

/// The banner for the scope; empty when every holding and every drawn day is priced. `arrived`: symbols held since the
/// last sync with no price (no balance row yet).
pub fn banner(c: &Connection, ctx: &Ctx, owner: i64, scope: &crate::tracker::Scope, arrived: &[String]) -> Result<String, WebError> {
    let gaps: Vec<VenueGap> = unpriced::read(c, owner, scope, arrived).map_err(|_| super::row::invalid())?;
    let chart = unpriced::chart_incomplete(c, owner, scope).map_err(|_| super::row::invalid())?;
    let mut lines = String::new();
    for g in gaps.iter().filter(|g| !g.now.is_empty()) { lines.push_str(&line(ctx, "missing", &g.now, g.venue.as_deref())); }
    if chart {
        // Named only from what the sweep recorded it could not price or open; a cause it could not name (on any account of
        // the scope) is the generic line, beside the names.
        let named: Vec<&VenueGap> = gaps.iter().filter(|g| g.exchange_id.is_some() && !(g.history.is_empty() && g.openings.is_empty())).collect();
        if named.is_empty() || gaps.iter().any(|g| g.unnamed) { lines.push_str(&format!("    <p class=\"notice__line\">{}</p>\n", ctx.t("tracker.prices.chart_incomplete"))); }
        for g in named {
            if !g.history.is_empty() { lines.push_str(&line(ctx, "chart_missing", &g.history, g.venue.as_deref())); }
            if !g.openings.is_empty() { lines.push_str(&line(ctx, "opening_unknown", &g.openings, g.venue.as_deref())); }
        }
    }
    // Delisted coins (plan r10 §1.8): a stored link the coin map voided (two different coins under one spelling) moves no
    // quantity; each side is counted where it was booked. Read from the warm ledger cache, its withdrawal's spelling and day.
    // ponytail: best-effort (a cold cache states none until the next ledger run), and in every scope.
    if let Ok(crate::tracker::cache::State::Warm(s)) = crate::tracker::cache::read(c, owner, None, ctx.now) {
        use rusqlite::OptionalExtension;
        for (a, b) in s.labels.void.iter().filter(|(a, b)| a < b) {
            let sent: Option<(String, String)> = c.query_row("SELECT base_currency, substr(transacted_at, 1, 10) FROM account_transactions WHERE user_id = ?1 AND id IN (?2, ?3) \
                                                             AND linked_transaction_id IN (?2, ?3) ORDER BY id LIMIT 1", rusqlite::params![owner, a, b], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional().map_err(|_| super::row::invalid())?;
            let Some((asset, day)) = sent else { continue };
            lines.push_str(&format!("    <p class=\"notice__line\">{}</p>\n",
                i18n::t(ctx.locale, "tracker.prices.transfer_conflict", &[("assets", Arg::Text(&asset)), ("date", Arg::Text(&day))])));
        }
    }
    if lines.is_empty() { return Ok(String::new()); }
    // Delisted coins (plan r10 §1.8): beside a named asset without a price, why that happens (an asset no evidence tells
    // apart from another under its ticker shows here under its label, as every unpriced asset does).
    let named = gaps.iter().any(|g| !g.now.is_empty()) || (chart && gaps.iter().any(|g| g.exchange_id.is_some() && !g.history.is_empty()));
    if named { lines.push_str(&format!("    <p class=\"notice__line\">{}</p>\n", ctx.t("tracker.prices.chart_missing_causes"))); }
    let connect = if crate::web::bots::market_data(c, &ctx.app)?.1 { String::new() } else {
        format!("    <p class=\"notice__line\">{} <a data-turbo-frame=\"modal\" href=\"{}\">{}</a></p>\n", ctx.t("tracker.prices.connect_market_data"),
            ctx.path("/tracker/connect_market_data"), ctx.t("button.connect"))
    };
    Ok(format!("<div class=\"widget notice\" role=\"status\">\n{lines}{connect}</div>\n"))
}
