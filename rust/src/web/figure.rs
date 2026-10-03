//! Read-only page figures. All decimals, including formatting, stay inside one budget and thread.
use crate::figures::{at::At, budget, chart, db::{self, Subject}, live, market::MarketData, num::Num, walk::{self, Metrics}, FiguresError};
use rusqlite::Connection;
use serde_json::{json, Map, Value};

mod holdings;
mod plot;
mod headline;
pub mod service;
pub mod loading;
use crate::figures::totals;

pub const NO_VALUE: &str = "<span class=\"no-value\">—</span>";
fn colour(n: &Num) -> &'static str { if n.is_negative() { "text-danger" } else if n.is_positive() { "text-success" } else { "" } }
fn precision(n: &Num, digits: usize, grouping: bool) -> Result<String, FiguresError> {
    let rounded = n.to_d()?.round(digits as i64)?.to_s_f();
    let (whole, fraction) = rounded.split_once('.').unwrap_or((&rounded, ""));
    let sign = if whole.starts_with('-') { "-" } else { "" };
    let unsigned = whole.strip_prefix('-').unwrap_or(whole);
    let mut out = sign.to_string();
    for (i, b) in unsigned.bytes().enumerate() {
        if grouping && i > 0 && (unsigned.len() - i).is_multiple_of(3) { out.push(','); }
        out.push(char::from(b));
    }
    if digits > 0 { out.push('.'); out.push_str(&format!("{fraction:0<digits$}")); }
    Ok(out)
}
fn percent(n: &Num, digits: usize) -> Result<String, FiguresError> {
    Ok(format!("{:.*}%", digits, n.mul(&Num::Int(100))?.to_f()))
}
fn dollars(n: &Num, digits: usize) -> Result<String, FiguresError> {
    let text = precision(n, digits, true)?;
    Ok(if let Some(rest) = text.strip_prefix('-') { format!("-<small>$</small>{rest}") } else { format!("<small>$</small>{text}") })
}
fn tile(s: &Subject, m: &Metrics, unavailable: bool, hidden: bool) -> Result<String, FiguresError> {
    let kind = if s.bot.kind == db::Kind::Basket { "dca_multi_asset" } else { "dca_index" };
    let mut body = String::new();
    if unavailable { body.push_str(NO_VALUE); }
    else if let Some(pnl) = &m.pnl {
        body = format!("<div class=\"bot-tile__pnl {}\">\n<span class=\"pnl-percent\">{}</span>\n", colour(pnl), percent(pnl, 2)?);
        if !hidden && s.quote.as_deref() == Some("USD") {
            let profit = m.total_amount_value_in_quote.sub(&m.total_quote_amount_invested)?;
            body.push_str(&format!("<span class=\"pnl-amount\">{}{}</span>\n", if profit.is_positive() { "+" } else { "" }, dollars(&profit, 2)?));
        }
        body.push_str("</div>\n");
    }
    Ok(format!("<div id=\"pnl_bots_{kind}_{}\">\n{body}</div>\n", s.bot.id))
}
fn missing(s: &Subject, m: &Metrics, market: &dyn MarketData) -> Result<Vec<String>, FiguresError> {
    if m.asset_breakdown.is_empty() { return Ok(vec![]); }
    let symbols = s.tickers.iter().map(|t| t.ticker.clone()).collect::<Vec<_>>();
    let prices = market.prices(&live::venue(s)?, &symbols);
    let mut keys = vec![];
    for (key, holding) in &m.asset_breakdown {
        if !holding.amount.is_positive() { continue; }
        let ticker = live::ticker_for_key(s, m, key);
        let omitted = ticker.is_none() || prices.as_ref().is_ok_and(|prices| ticker.is_some_and(|t| !prices.iter().any(|(code,_)| code == &t.ticker)));
        if omitted { keys.push(key.clone()); }
    }
    Ok(keys)
}

#[allow(clippy::too_many_arguments)]
pub fn account(c: &Connection, user_id: i64, market: &dyn MarketData, now: At, locale: &str, csrf: &str, prefix: &str) -> Result<Value, FiguresError> {
    budget::within(|| {
        let mut bots = Map::new();
        let mut computed = vec![];
        let user = db::user(c, user_id)?;
        for (id, _) in db::account_bots(c, user_id)? {
            let subject = Subject::load(c, id)?;
            let walked = walk::metrics(c, &subject, now)?;
            let marked_live = live::live(c, &subject, &walked, market, now)?;
            let missing = missing(&subject, &marked_live, market)?;
            let marked = chart::marked(c, &subject, &marked_live, market, now)?;
            bots.insert(id.to_string(), json!({ "tile": tile(&subject, &marked_live, !missing.is_empty(), user.hide_balances)?, "metrics": holdings::render(c, &subject, &marked_live, &missing, user.hide_balances, locale, csrf, prefix, now)?, "chart": plot::render(c, &subject, &marked, &missing, user.hide_balances, locale, &user.time_zone)?, "missing": missing }));
            computed.push((subject, marked_live, marked, missing));
        }
        let unavailable = computed.iter().any(|(s,_,m,missing)| !missing.is_empty() || !m.chart_omitted.is_empty() || s.quote.as_deref() != Some("USD"));
        let parts = |marked: bool| computed.iter().map(|(s,live,chart,_)| totals::Part { bot_id:s.bot.id, quote:s.quote.as_deref(),traded:!s.orders.is_empty(),figures:Ok(Some(if marked { chart } else { live })) }).collect::<Vec<_>>();
        let pnl = totals::global_pnl(c,market,&mut totals::Rates::default(),&parts(false))?;
        let history = totals::pnl_history(c,market,&mut totals::Rates::default(),&parts(true))?;
        Ok(json!({"bots": bots, "account": headline::render(pnl.as_ref(),history.result.as_ref(),unavailable,user.hide_balances)?}))
    })
}
