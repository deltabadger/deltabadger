//! The tracker's missing-prices banner (Rust only, PLAN-venue-prices S1): per venue, the assets with no market price
//! now and the ones the chart could not value, and where no provider is set up, the way to connect one.
use super::{Ctx, WebError};
use crate::tracker::unpriced::{self, VenueGap};
use crate::web::i18n::{self, Arg};
use rusqlite::Connection;

fn line(ctx: &Ctx, key: &str, assets: &[String], venue: Option<&str>) -> String {
    let assets = assets.join(", ");
    let key = if venue.is_some() { key.to_string() } else { format!("{key}_anywhere") };
    format!("    <p class=\"tracker-notice__line\">{}</p>\n",
        i18n::t(ctx.locale, &format!("tracker.prices.{key}"), &[("assets", Arg::Text(&assets)), ("venue", Arg::Text(venue.unwrap_or_default()))]))
}

/// The banner for the scope; empty when every holding and every drawn day is priced. `arrived`: symbols held since the
/// last sync with no price (no balance row yet).
pub fn banner(c: &Connection, ctx: &Ctx, owner: i64, scope: Option<i64>, arrived: &[String]) -> Result<String, WebError> {
    let gaps: Vec<VenueGap> = unpriced::read(c, owner, scope, arrived).map_err(|_| super::row::invalid())?;
    let chart = unpriced::chart_incomplete(c, owner, scope).map_err(|_| super::row::invalid())?;
    let mut lines = String::new();
    for g in gaps.iter().filter(|g| !g.now.is_empty()) { lines.push_str(&line(ctx, "missing", &g.now, g.venue.as_deref())); }
    if chart {
        let named: Vec<(&VenueGap, Vec<String>)> = gaps.iter().filter(|g| g.exchange_id.is_some()).map(|g| {
            let mut all: Vec<String> = g.history.iter().chain(&g.now).cloned().collect();
            all.sort();
            all.dedup();
            (g, all)
        }).filter(|(_, all)| !all.is_empty()).collect();
        if named.is_empty() { lines.push_str(&format!("    <p class=\"tracker-notice__line\">{}</p>\n", ctx.t("tracker.prices.chart_incomplete"))); }
        for (g, all) in named { lines.push_str(&line(ctx, "chart_missing", &all, g.venue.as_deref())); }
    }
    if lines.is_empty() { return Ok(String::new()); }
    let connect = if crate::web::bots::market_data(c, &ctx.app)?.1 { String::new() } else {
        format!("    <p class=\"tracker-notice__line\">{} <a data-turbo-frame=\"modal\" href=\"{}\">{}</a></p>\n", ctx.t("tracker.prices.connect_coingecko"),
            ctx.path("/tracker/connect_market_data"), i18n::t(ctx.locale, "bot.setup.connect_to", &[("name", Arg::Text("CoinGecko"))]))
    };
    Ok(format!("<div class=\"widget tracker-notice\" role=\"status\">\n{lines}{connect}</div>\n"))
}
