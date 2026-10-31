use super::*;
use crate::figures::dec::Dec;
use crate::web::{colors, i18n::{self, escape, Arg}};
use rusqlite::OptionalExtension;
use std::collections::HashSet;

fn t(locale: &str, key: &str) -> String { i18n::t(locale, key, &[]) }
fn money(n: &Num) -> Result<String, FiguresError> {
    let value = precision(n, 2, true)?;
    Ok(if n.to_d()?.round(2)?.is_zero() { format!("<span class=\"is-zero\">{value}</span>") } else { value })
}
fn quoted(value: String, quote: &str) -> String { if quote.is_empty() { value } else { format!("{value} <small>{}</small>", escape(quote)) } }
fn decimal(c: &Connection, sql: &str, id: i64) -> Result<Dec, FiguresError> {
    let v = c.query_row(sql, [id], |r| r.get::<_, rusqlite::types::Value>(0))?;
    Ok(Dec::from_sql((&v).into())?.unwrap_or_else(Dec::zero))
}
fn image(c: &Connection, asset: Option<i64>) -> Result<String, FiguresError> {
    let row = asset.map(|id| c.query_row("SELECT image_url, color FROM assets WHERE id = ?1", [id], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?))).optional()).transpose()?.flatten();
    let (url, colour) = row.unwrap_or_default();
    Ok(match url.filter(|s| !s.is_empty()) {
        Some(url) => format!("\n<img class=\"asset-logo\" src=\"{}\" alt=\"\" loading=\"lazy\">\n", escape(&url)),
        None => format!("\n<span class=\"asset-logo\" style=\"background: {}\"></span>\n", escape(&colors::ensure_contrast(colour.as_deref().unwrap_or(colors::NEUTRAL)).ok_or_else(|| FiguresError::Data("Invalid asset colour".into()))?)),
    })
}
struct Holding {
    key: String, asset: Option<i64>, amount: Num, avg: Option<Num>, value: Option<Num>, pnl: Option<Num>, harvest: bool, sellable: bool, can_sell: bool, lock: Option<(i64, String, String)>,
}
#[allow(clippy::too_many_arguments)]
fn table(c: &Connection, s: &Subject, rows: &[&Holding], body: &str, actions: bool, hidden: bool, locale: &str, prefix: &str) -> Result<String, FiguresError> {
    let quote = s.quote.as_deref().unwrap_or("");
    let mut out = "<table class=\"table\">\n<thead>\n<tr>\n<th scope=\"col\" class=\"table__logo\"></th>\n<th scope=\"col\"></th>\n".to_string();
    for (key, shown) in [("amount", !hidden), ("avg_price", true), ("value", !hidden), ("pnl", true)] {
        if shown { out.push_str(&format!("<th scope=\"col\">{}</th>\n", t(locale, &format!("data_labels.{key}")))); }
    }
    if actions { out.push_str("<th scope=\"col\" class=\"table__action\"></th>\n"); }
    out.push_str(&format!("</tr>\n</thead>\n<tbody id=\"{body}\">\n"));
    for row in rows {
        let asset = row.asset.or_else(|| s.tickers.iter().rev().find(|t| t.base == row.key).map(|t| t.base_asset_id));
        out.push_str(&format!("<tr data-symbol=\"{}\">\n<td class=\"table__logo\">{}</td>\n<td scope=\"row\">{}</td>\n", escape(&row.key), image(c, asset)?, escape(&row.key)));
        if !hidden {
            let amount = precision(&row.amount, 6, false)?;
            let amount = amount.trim_end_matches('0').trim_end_matches('.');
            out.push_str(&format!("<td>{amount}</td>\n"));
        }
        out.push_str(&format!("<td>{}</td>\n", row.avg.as_ref().map(|n| money(n).map(|v| quoted(v, quote))).transpose()?.unwrap_or(NO_VALUE.into())));
        if !hidden { out.push_str(&format!("<td>\n{}\n</td>\n", row.value.as_ref().map(|n| money(n).map(|v| quoted(v, quote))).transpose()?.unwrap_or(NO_VALUE.into()))); }
        let pnl = row.pnl.as_ref().map(|n| percent(n, 1).map(|v| format!("<b>{}{v}</b>", if n.is_negative() { "" } else { "+" }))).transpose()?.unwrap_or(NO_VALUE.into());
        out.push_str(&format!("<td class=\"{}\">\n{pnl}\n</td>\n", row.pnl.as_ref().map_or("", colour)));
        if actions {
            let action = if let Some(id) = row.asset.filter(|_| row.can_sell) {
                let query = form_urlencoded::Serializer::new(String::new()).append_pair("asset_id", &id.to_string()).append_pair("symbol", &row.key).finish();
                let title = if row.harvest { format!(" title=\"{}\"", escape(&i18n::text(locale, "bot.liquidation.harvest_hint", &[]))) } else { String::new() };
                format!("<a class=\"rbutton rbutton--small {}\"{title} data-turbo-frame=\"modal\" href=\"{}/bots/{}/liquidation/new?{}\">{}</a>", if row.harvest { "rbutton--success" } else { "rbutton--secondary" }, escape(prefix), s.bot.id, escape(&query), t(locale, "bot.liquidation.sell"))
            } else { String::new() };
            let lock = row.lock.as_ref().map(|(days, until, source)| {
                let label = i18n::t(locale,"bot.liquidation.locked",&[("days",Arg::Count(*days))]);
                let mut text = format!("<span class=\"text-inactive\" title=\"{}\">{label}</span>",escape(until));
                if source == "ledger" { text.push_str(&format!(" <span class=\"text-inactive\">{}</span>",t(locale,"bot.wash_sale.from_ledger"))); }
                text
            }).unwrap_or_default();
            let separator = if !lock.is_empty() && !action.is_empty() { " " } else { "" };
            out.push_str(&format!("<td class=\"table__action\">{lock}{separator}{action}</td>\n"));
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</tbody>\n</table>\n");
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
/// A form's authenticity token: this one, or none (`Value("")`: a broadcast, where Rails' renderer has no session and
/// writes no field), or, for kept figures, nothing but the byte offset where each form's field goes (`Slots`): a page
/// writes its own token's field there and nowhere else (`with_token`), and a broadcast takes the fragment as it is.
pub(crate) enum Token<'a> { Value(&'a str), Slots(&'a mut Vec<usize>) }

/// A bot whose every order is a sell (it holds what it sells from outside): what it sold, when that is one asset (its
/// amount and symbol), and the proceeds. None for any bot that bought.
/// (amount and symbol when one asset, proceeds)
type SellOnly = (Option<(crate::figures::dec::Dec, String)>, crate::figures::dec::Dec);
/// `pair`: read as the pair's walk reads its rows (`fill::pair_row`).
fn sell_only(s: &Subject, pair: bool) -> Result<Option<SellOnly>, FiguresError> {
    use crate::figures::{dec::Dec, fill};
    if s.orders.is_empty() || s.orders.iter().any(|order| !order.sell) { return Ok(None); }
    let (mut amount, mut received, mut bases) = (Dec::zero(), Dec::zero(), HashSet::new());
    for order in &s.orders {
        let Some(filled) = (if pair { fill::pair_row(order)? } else { fill::parse(order)? }) else { continue };
        amount = (&amount + &filled.quantity)?;
        received = (&received + &filled.value)?;
        if let Some(base) = &order.base { bases.insert(base.clone()); }
    }
    let sold = (bases.len() == 1).then(|| (amount, bases.into_iter().next().unwrap_or_default()));
    Ok(Some((sold, received)))
}

fn token_field(csrf: &str) -> String { format!("<input type=\"hidden\" name=\"authenticity_token\" value=\"{}\" autocomplete=\"off\" />", escape(csrf)) }
/// Kept metrics with the request's token field written at each recorded offset (`Token::Slots`), and nowhere else:
/// stored text is never searched. No token (a broadcast): the fragment as it is.
pub(crate) fn with_token(metrics: &str, offsets: &[usize], csrf: &str) -> String {
    let mut out = metrics.to_string();
    if csrf.is_empty() { return out; }
    let field = token_field(csrf);
    for &at in offsets.iter().rev() { out.insert_str(at, &field); }
    out
}

/// One live wash-sale lock: (asset, its symbol, locked until, source).
type Lock = (i64, Option<String>, At, String);
/// Bot#locked_members: the user's live locks on the assets the bot `known`s, while the rule has days (User#wash_sale_days).
pub(super) fn live_locks(c: &Connection, user: i64, known: &HashSet<i64>, now: At) -> Result<Vec<Lock>, FiguresError> {
    let (enabled, jurisdiction): (Option<bool>,Option<String>) = c.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1",[user],|r| Ok((r.get(0)?,r.get(1)?)))?;
    // User#wash_sale_jurisdiction uses presence, then the first wash-sale option (US).
    let jurisdiction = jurisdiction.as_deref().filter(|j| !j.trim().is_empty()).unwrap_or("US");
    if enabled != Some(true) || !["US","GB","IE"].contains(&jurisdiction) { return Ok(vec![]); }
    let locks = c.prepare("SELECT l.asset_id,a.symbol,l.buy_locked_until,l.source FROM wash_sale_locks l JOIN assets a ON a.id=l.asset_id WHERE l.user_id=?1 ORDER BY l.id")?
        .query_map([user],|r| Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut live = vec![];
    for (id,symbol,until,source) in locks {
        if !known.contains(&id) { continue; }
        let Some(until)=until.as_deref().map(At::from_sql).transpose().map_err(|e|FiguresError::Data(format!("{e:?}")))? else {continue};
        if until > now {live.push((id,symbol,until,source.unwrap_or_default()));} // allow-swallow: optional source label
    }
    Ok(live)
}
/// BotHelper#locked_member_keys: each lock under its holding's key (`key_assets`), or under a key of its own no holding has.
fn locked(key_assets: &[(String, Option<i64>)], live: Vec<Lock>) -> Result<Vec<(String, i64, At, String)>, FiguresError> {
    let candidates = live.iter().filter(|(id,_,_,_)| !key_assets.iter().any(|(_,asset)| *asset == Some(*id)))
        .map(|(id,symbol,_,_)| (crate::figures::keys::Identity::Asset(*id),symbol.as_deref().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("#{id}")))).collect::<Vec<_>>();
    let mut keys = crate::figures::keys::call(&candidates)?;
    let mut taken: HashSet<_> = key_assets.iter().map(|(k,_)| k.clone()).chain(keys.iter().filter(|(_,key)| !key_assets.iter().any(|(k,_)| k == key)).map(|(_,k)| k.clone())).collect();
    for (identity,key) in &mut keys {
        if key_assets.iter().any(|(k,_)| k == key) {
            let crate::figures::keys::Identity::Asset(id) = identity else { continue };
            while taken.contains(key) { budget::charge(1,0)?; *key = format!("{key}#{id}"); }
            taken.insert(key.clone());
        }
    }
    let mut out = vec![];
    for (id,_,until,source) in live {
        let key = key_assets.iter().find(|(_,asset)| *asset == Some(id)).map(|(key,_)| key.clone())
            .or_else(|| keys.iter().find(|(asset,_)| *asset == crate::figures::keys::Identity::Asset(id)).map(|(_,key)| key.clone()));
        if let Some(key) = key { out.push((key, id, until, source)); }
    }
    Ok(out)
}
/// A lock as its row shows it: days left, the day it ends, its source.
fn lock_shown(until: At, source: String, now: At) -> (i64, String, String) {
    ((until.utc().date_naive() - now.utc().date_naive()).num_days(), until.utc().format("%Y-%m-%d").to_string(), source)
}
/// bots/composition/_locked_table: what the account-wide rule is doing to this bot, nothing when it locks nothing.
fn wash_sale_table(c: &Connection, s: &Subject, locked: &[&Holding], hidden: bool, locale: &str, prefix: &str) -> Result<String, FiguresError> {
    if locked.is_empty() { return Ok(String::new()); }
    Ok(format!("<div id=\"wash_sale_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n<div class=\"exited-header\">\n<span class=\"label\">{}</span>\n<a class=\"rbutton rbutton--small rbutton--secondary\" data-turbo-frame=\"_top\" href=\"{}/settings/account\">{}</a>\n</div>\n{}\n</div>\n",t(locale,"bot.wash_sale.table_title"),escape(prefix),t(locale,"bot.wash_sale.settings_link"),table(c,s,locked,"wash_sale_list",true,hidden,locale,prefix)?))
}

/// bots/composition/_locked_table while the figures load (BotsController#show's `@bot.metrics`, no prices): each live lock
/// unheld (`values: {}`): amount 0, no Sell, under its own key. A signal bot's are the locks on its pair's base.
pub(super) fn loading_locked_table(c: &Connection, bot_id: i64, signal: bool, hidden: bool, locale: &str, prefix: &str, now: At) -> Result<String, FiguresError> {
    // Only a live lock on an asset this bot trades is drawn; without one, the bot's orders are not read for it.
    let (user, settings): (i64, String) = c.query_row("SELECT user_id, settings FROM bots WHERE id=?1", [bot_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let known: HashSet<i64> = if signal { serde_json::from_str::<serde_json::Value>(&settings).ok().and_then(|s| s.get("base_asset_id").and_then(serde_json::Value::as_i64)).into_iter().collect() }
        else { c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id=?1")?.query_map([bot_id], |r| r.get(0))?.collect::<Result<_,_>>()? };
    let live = live_locks(c, user, &known, now)?;
    if live.is_empty() { return Ok(String::new()); }
    // BotHelper#locked_member_keys reads the walk's `key_assets` (`@bot.metrics`, no prices), so a lock beside a holding of
    // the same name is told apart from it. A pair holds one asset: no name to share.
    let (s, key_assets) = if signal { (crate::figures::pair::load(c, bot_id)?, vec![]) } else {
        budget::within(|| -> Result<_, FiguresError> { let s = Subject::load(c, bot_id)?; let keys = crate::figures::walk::metrics(c, &s, now)?.key_assets; Ok((s, keys)) })?
    };
    let rows = locked(&key_assets, live)?.into_iter().map(|(key, id, until, source)|
        Holding { key, asset: Some(id), amount: Num::Int(0), avg: None, value: None, pnl: None, harvest: false, sellable: false, can_sell: false, lock: Some(lock_shown(until, source, now)) }).collect::<Vec<_>>();
    wash_sale_table(c, &s, &rows.iter().collect::<Vec<_>>(), hidden, locale, prefix)
}

/// bots/dca_single_assets/_metrics, from a pair bot's live figures (`pair::live`). Rust only (3.0): stale figures state
/// nothing priced (D3, as a basket's), the note only for a venue the figures read; a pair that only sells states what it
/// sold and what that brought in (D4).
pub(super) fn render_pair(c: &Connection, p: &crate::figures::pair::Pair, hidden: bool, locale: &str, prefix: &str, now: At) -> Result<String, FiguresError> {
    let (s, m) = (&p.subject, &p.metrics);
    let quote = s.quote.as_deref().unwrap_or("");
    let quoted = |figure: &str| if figure.trim().is_empty() || quote.trim().is_empty() { escape(figure) } else { format!("{} <small>{}</small>", escape(figure), escape(quote)) };
    let ticker = s.tickers.first();
    // Bot#decimals: the ticker's three precisions, none without a ticker.
    let (base_decimals, quote_decimals, price_decimals): (Option<i64>, Option<i64>, Option<i64>) = match ticker {
        Some(t) => c.query_row("SELECT base_decimals, quote_decimals, price_decimals FROM tickers WHERE id=?1", [t.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?,
        None => (None, None, None),
    };
    let max = |d: Option<i64>| d.and_then(|d| u8::try_from(d).ok()).unwrap_or(8);
    let stale = m.prices_stale;
    let mut out = "<div id=\"metrics\" style=\"display: flex; flex-direction: column; gap: 1rem;\">\n".to_string();
    if stale && crate::figures::page_market::notes(&live::venue(s)?, s.bot.user_id, &s.tickers) {
        out.push_str(&format!("  <p class=\"status-button__hint\" role=\"status\">\n    {}\n  </p>\n", t(locale, "bot.details.stats.exchange_unavailable_html")));
    }
    if !hidden {
        let item = |label: &str, value: String| format!("      <div class=\"data-grid__item\" data-controller=\"tooltip\">\n  <div class=\"label\">\n    {}\n  </div>\n  \
            <div class=\"data-grid__item__value \">\n      {value}\n  </div>\n</div>\n\n", t(locale, label));
        out.push_str("    <div class=\"widget data-grid data-grid--two-columns\">\n");
        if let Some((sold, received)) = sell_only(s, true)? {
            // D4 (3.0, Rust only): a bot that only sells has no investment to state: what it sold, and what that brought in.
            if let Some((amount, base)) = sold {
                let amount = precision(&Num::Dec(amount), 6, false)?;
                out.push_str(&item("bot.details.stats.sold", format!("{} <small>{}</small>", amount.trim_end_matches('0').trim_end_matches('.'), escape(&base))));
            }
            // money() returns trusted markup (the is-zero span); only the currency is escaped.
            out.push_str(&item("bot.details.stats.received", self::quoted(money(&Num::Dec(received))?, quote.trim())));
        } else {
            out.push_str(&item("data_labels.invested", quoted(&format_value(&m.total_quote_amount_invested, max(quote_decimals))?)));
            let value = if stale { NO_VALUE.to_string() } else { quoted(&format_value(&m.total_amount_value_in_quote, max(quote_decimals))?) };
            out.push_str(&item("data_labels.portfolio_value", value));
        }
        out.push_str("    </div>\n");
    }
    let columns = if hidden { 4 } else { 6 };
    out.push_str("\n  <div id=\"assets_metrics_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n    <table class=\"table\">\n  <thead>\n    <tr>\n      \
                  <th scope=\"col\" class=\"table__logo\"></th>\n      <th scope=\"col\"></th>\n");
    if !hidden { out.push_str(&format!("      <th scope=\"col\">{}</th>\n", t(locale, "data_labels.amount"))); }
    out.push_str(&format!("      <th scope=\"col\">{}</th>\n", t(locale, "data_labels.avg_price")));
    if !hidden { out.push_str(&format!("      <th scope=\"col\">{}</th>\n", t(locale, "data_labels.value"))); }
    out.push_str(&format!("      <th scope=\"col\">{}</th>\n      \n    </tr>\n  </thead>\n  <tbody id=\"assets_metrics_list\">\n", t(locale, "data_labels.pnl")));
    // bot.base_asset: the settings' base, when that asset exists.
    let base = s.bot.base_asset_ids.first().map(|id| c.query_row("SELECT image_url, color, symbol, category FROM assets WHERE id=?1", [id],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?))).optional()).transpose()?.flatten();
    match base.filter(|_| m.total_quote_amount_invested.is_positive()) {
        Some((image, tint, symbol, category)) => {
            let symbol = symbol.unwrap_or_default();
            let logo = crate::web::tracker::row::asset_logo(image, tint, symbol.clone(), category).map_err(|e| FiguresError::Data(format!("{e:?}")))?;
            let symbol = escape(&symbol);
            out.push_str(&format!("      <tr data-symbol=\"{symbol}\">\n        <td class=\"table__logo\">{logo}</td>\n        <td scope=\"row\">{symbol}</td>\n"));
            if !hidden { out.push_str(&format!("        <td>{}</td>\n", escape(&printed(&p.amount.round(i64::from(max(base_decimals)))?)))); }
            let average = p.average.as_ref().map(|a| a.round(i64::from(max(price_decimals)))).transpose()?;
            out.push_str(&format!("        <td>{}</td>\n", average.map_or_else(|| NO_VALUE.to_string(), |a| quoted(&printed(&a)))));
            let pnl = m.pnl.as_ref().filter(|_| !stale);
            if !hidden {
                let value = if stale { NO_VALUE.to_string() } else { quoted(&format_value(&m.total_amount_value_in_quote, max(quote_decimals))?) };
                out.push_str(&format!("          <td>\n              {value}\n          </td>\n"));
            }
            let shown = pnl.map(|n| percent(n, 1).map(|v| format!("<b>{}{v}</b>", if n.is_negative() { "" } else { "+" }))).transpose()?.unwrap_or(NO_VALUE.into());
            out.push_str(&format!("        <td class=\"{}\">\n            {shown}\n        </td>\n        \n      </tr>\n", pnl.map_or("", colour)));
        }
        None => out.push_str(&format!("      <tr>\n        <td colspan=\"{columns}\" class=\"text-center text-inactive\" style=\"padding-bottom: 2rem;\">. . .</td>\n      </tr>\n")),
    }
    out.push_str("  </tbody>\n</table>\n\n  </div>\n\n  ");
    // Bot#locked_members: the locks on the ticker's base asset, each unheld (`values: {}`): amount 0, no Sell.
    let known: HashSet<i64> = ticker.map(|t| t.base_asset_id).into_iter().collect();
    let rows = locked(&[], live_locks(c, s.bot.user_id, &known, now)?)?.into_iter().map(|(key, id, until, source)|
        Holding { key, asset: Some(id), amount: Num::Int(0), avg: None, value: None, pnl: None, harvest: false, sellable: false, can_sell: false, lock: Some(lock_shown(until, source, now)) }).collect::<Vec<_>>();
    out.push_str(&wash_sale_table(c, s, &rows.iter().collect::<Vec<_>>(), hidden, locale, prefix)?);
    out.push_str("\n</div>\n");
    budget::check().map_err(FiguresError::from)?;
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render(c: &Connection, s: &Subject, m: &Metrics, missing: &[String], hidden: bool, locale: &str, csrf: &mut Token<'_>, prefix: &str, now: At) -> Result<String, FiguresError> {
    let quote = s.quote.as_deref().unwrap_or("");
    let (status, settings): (i64, String) = c.query_row("SELECT status, settings FROM bots WHERE id=?1", [s.bot.id], |r| Ok((r.get(0)?,r.get(1)?)))?;
    let settings: Value = serde_json::from_str(&settings).map_err(|_| FiguresError::Data("Invalid settings".into()))?;
    let engine_bot = crate::engine::model::load_bot(c, s.bot.id).map_err(|e| FiguresError::Data(format!("{e:?}")))?;
    let in_flight = crate::engine::composition::in_flight(c, &engine_bot, now.utc()).map_err(|e| FiguresError::Data(format!("{e:?}")))?;
    let may_sell = !matches!(status, 3 | 7) && !in_flight;
    let members: HashSet<i64> = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id=?1 AND in_index=1")?.query_map([s.bot.id], |r| r.get(0))?.collect::<Result<_,_>>()?;
    let mut rows = vec![];
    for (key, data) in &m.asset_values {
        let id = m.key_assets.iter().find(|(k,_)| k == key).and_then(|(_,id)| *id);
        let sellable = if let Some(ticker) = id.and_then(|_| live::ticker_for_key(s, m, key)) {
            data.amount.to_d()? >= decimal(c, "SELECT minimum_base_size FROM tickers WHERE id=?1", ticker.id)?
        } else { false };
        rows.push(Holding { key: key.clone(), asset: id, amount: data.amount.clone(), avg: Some(data.avg_price.clone()), value: Some(data.current_value.clone()), pnl: Some(data.pnl_percentage.clone()), harvest: data.harvestable, sellable, can_sell: sellable && may_sell, lock: None });
    }
    // Missing holdings stay visible, including the one-asset stale fallback which has no Unpriced entry.
    for key in missing {
        rows.retain(|r| r.key != *key);
        if let Some((_, data)) = m.asset_breakdown.iter().find(|(k,_)| k == key) {
            rows.push(Holding { key: key.clone(), asset: m.key_assets.iter().find(|(k,_)| k == key).and_then(|(_,id)| *id), amount: data.amount.clone(),
                avg: Some(data.quote_invested.div(&data.amount)?), value: None, pnl: None, harvest: false, sellable: false, can_sell: false, lock: None });
        }
    }
    let mut locked_keys = HashSet::new();
    let known: HashSet<i64> = c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id=?1")?.query_map([s.bot.id], |r| r.get(0))?.collect::<Result<_,_>>()?;
    for (key, id, until, source) in locked(&m.key_assets, live_locks(c, s.bot.user_id, &known, now)?)? {
        let lock = lock_shown(until, source, now);
        if let Some(row) = rows.iter_mut().find(|r| r.key == key) { row.lock = Some(lock); }
        else { rows.push(Holding { key: key.clone(), asset: Some(id), amount: Num::Int(0),avg:None,value:None,pnl:None,harvest:false,sellable:false,can_sell:false,lock:Some(lock) }); }
        locked_keys.insert(key);
    }
    let mut out = "<div id=\"metrics\" style=\"display: flex; flex-direction: column; gap: 1rem;\">\n".to_string();
    // 3.0 (Rust only): a venue the figures do not read yet (or an IBKR login without the subscription) did not fail to answer.
    if m.prices_stale && crate::figures::page_market::notes(&crate::figures::live::venue(s)?, s.bot.user_id, &s.tickers) { out.push_str(&format!("<p class=\"status-button__hint\" role=\"status\">\n{}\n</p>\n", t(locale, "bot.details.stats.exchange_unavailable_html"))); }
    if !hidden {
        out.push_str(&format!("    <div class=\"widget data-grid {}\">\n", if m.realised_pnl.is_zero() { "data-grid--two-columns" } else { "" }));
        let item = |label: &str, value: String| format!("      <div class=\"data-grid__item\" data-controller=\"tooltip\">\n        <div class=\"label\">{}</div>\n        <div class=\"data-grid__item__value\">\n            {value}\n        </div>\n      </div>\n", t(locale, &format!("bot.details.stats.{label}")));
        if let Some((sold, received)) = sell_only(s, false)? {
            // 3.0 (Rust only): a bot that only sells has no investment to state: what it sold, and what that brought in.
            if let Some((amount, base)) = sold {
                let amount = precision(&Num::Dec(amount), 6, false)?;
                out.push_str(&item("sold", quoted(amount.trim_end_matches('0').trim_end_matches('.').to_string(), &base)));
            }
            out.push_str(&item("received", quoted(money(&Num::Dec(received))?, quote)));
        } else {
            for (label, value) in [("total_invested", Some(&m.total_quote_amount_invested)), ("portfolio_value", (!m.prices_stale && missing.is_empty()).then_some(&m.total_amount_value_in_quote))] {
                out.push_str(&item(label, value.map(|n| money(n).map(|v| quoted(v, quote))).transpose()?.unwrap_or(NO_VALUE.into())));
            }
        }
        if !m.realised_pnl.is_zero() {
            out.push_str(&format!("<div class=\"data-grid__item\" data-controller=\"tooltip\">\n<div class=\"label label--info\">\n{}\n<span class=\"tooltip-info-icon\" data-action=\"mouseenter->tooltip#showTooltip mouseleave->tooltip#hideTooltip click->tooltip#toggle\">\n{}\n</span>\n</div>\n<div class=\"data-grid__item__value {}\">\n{}{}\n</div>\n<div class=\"tooltip tooltip--note\">{}</div>\n</div>\n", t(locale,"bot.dca_index.realised_pnl"), include_str!("../../../templates/svg/_24x24_info.html"), if m.realised_pnl.is_negative() { "text-danger" } else { "text-success" }, if m.realised_pnl.is_negative() { "" } else { "+" }, quoted(precision(&m.realised_pnl,2,true)?,quote), t(locale,"bot.dca_index.realised_pnl_note")));
        }
        out.push_str("    </div>\n");
    }
    out.push('\n');
    let exited: Vec<_> = rows.iter().filter(|r| !locked_keys.contains(&r.key) && r.sellable && r.asset.is_some_and(|id| !members.is_empty() && !members.contains(&id))).collect();
    let composed: Vec<_> = rows.iter().filter(|r| !locked_keys.contains(&r.key) && (missing.contains(&r.key) || r.asset.is_none_or(|id| members.is_empty() || members.contains(&id)))).collect();
    let actions = rows.iter().any(|r| r.can_sell);
    if !composed.is_empty() { out.push_str(&format!("<div id=\"assets_metrics_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n{}\n</div>\n", table(c,s,&composed,"assets_metrics_list",actions,hidden,locale,prefix)?)); }
    out.push('\n');
    let halt = liquidation_halt(c,s.bot.id)?;
    if !exited.is_empty() || !halt.bases.is_empty() {
        let title = if s.bot.kind == db::Kind::Index { "bot.dca_index.out_of_the_index" } else { "bot.dca_multi_asset.removed_from_portfolio" };
        out.push_str(&format!("<div id=\"exited_metrics_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n<div class=\"exited-header\">\n<span class=\"label\">{}</span>\n", t(locale,title)));
        if !halt.bases.is_empty() {
            let offset=out.len();
            let mut slots=vec![];
            match csrf {
                Token::Slots(all)=>{out.push_str(&halt_control(s.bot.id,&halt,locale,prefix,&mut Token::Slots(&mut slots)));all.extend(slots.into_iter().map(|at|at+offset));}
                Token::Value(token)=>out.push_str(&halt_control(s.bot.id,&halt,locale,prefix,&mut Token::Value(token))),
            }
        } else if exited.len() > 1 && may_sell {
            let mut q = form_urlencoded::Serializer::new(String::new());
            for row in &exited { if let Some(id) = row.asset { q.append_pair("asset_id[]", &id.to_string()); } }
            for row in &exited { q.append_pair("symbol[]", &row.key); }
            out.push_str(&format!("<a class=\"rbutton rbutton--small rbutton--danger\" data-turbo-frame=\"modal\" href=\"{}/bots/{}/liquidation/new?{}\">{}</a>\n", escape(prefix),s.bot.id,escape(&q.finish()),t(locale,"bot.liquidation.sell_all")));
        }
        out.push_str("</div>\n");
        if !exited.is_empty() { out.push_str(&table(c,s,&exited,"exited_metrics_list",actions,hidden,locale,prefix)?); }
        out.push_str("</div>\n");
    }
    let locked: Vec<_> = rows.iter().filter(|r| locked_keys.contains(&r.key)).collect();
    out.push_str("\n  ");
    out.push_str(&wash_sale_table(c,s,&locked,hidden,locale,prefix)?);
    out.push_str("\n\n");
    let mut control = crate::web::bot::rebalance::control(s.bot.id,&engine_bot.transient,locale,prefix);
    if let Some(slot) = control.find("<input type=\"hidden\" name=\"halt_token\"") {
        match csrf {
            Token::Value("") => {}, Token::Value(token) => control.insert_str(slot,&token_field(token)), Token::Slots(slots) => slots.push(out.len()+slot),
        }
        out.push_str(&control);
    }
    let redeploy_pending = &engine_bot.transient[crate::engine::redeploy::PENDING_KEY];
    if redeploy_pending["state"] == "ambiguous" {
        // The halt's Clear, before any balance, status or direction condition: it names the intent this render shows.
        let mut q = form_urlencoded::Serializer::new(String::new());
        if !redeploy_pending["id"].is_null() { q.append_pair("intent_id", &crate::web::mcp::tools::str_value(&redeploy_pending["id"])); }
        let query = q.finish();
        out.push_str(&format!("<div class=\"widget redeploy-prompt\">\n<span class=\"text-error\">{}</span>\n<form class=\"button_to\" data-turbo-confirm=\"{}\" method=\"post\" action=\"{}/bots/{}/redeploy_resolutions{}{}\"><button class=\"rbutton rbutton--danger\" type=\"submit\">{}</button>",
            t(locale,"bot.redeploy.halted"), escape(&i18n::text(locale,"bot.redeploy.resolve_confirm",&[])), escape(prefix), s.bot.id, if query.is_empty() {""} else {"?"}, escape(&query), t(locale,"bot.redeploy.resolve")));
        match csrf {
            Token::Value("") => {}
            Token::Value(token) => out.push_str(&token_field(token)),
            Token::Slots(slots) => slots.push(out.len()),
        }
        out.push_str("</form>\n</div>\n");
    } else if !hidden && !matches!(status,3|7) && settings["direction"] != "selling" && !in_flight {
        // D-6C-9: Rails' clamp and minimum fallback; a stored decline is never rewritten from a GET.
        match crate::engine::redeploy::offer(c,s,m)? {
            offer if offer.is_positive() => {
                let amount = precision(&Num::Dec(offer),2,true)?;
                let prompt = i18n::t(locale,"bot.redeploy.prompt",&[("amount",Arg::Text(&amount)),("symbol",Arg::Text(quote))]);
                out.push_str(&format!("<div class=\"widget redeploy-prompt\">\n<span class=\"label\">\n{prompt}\n</span>\n<div class=\"redeploy-prompt__actions\" id=\"redeploy-prompt-actions\">\n"));
                for (decline, colour, label) in [(false,"success","confirm"),(true,"danger","decline")] {
                    let method = if decline { "<input type=\"hidden\" name=\"_method\" value=\"delete\" autocomplete=\"off\" />" } else { "" };
                    out.push_str(&format!("<form class=\"button_to\" method=\"post\" action=\"{}/bots/{}/redeploy\">{method}<button class=\"rbutton rbutton--{colour}\" type=\"submit\">{}</button>",escape(prefix),s.bot.id,t(locale,&format!("bot.redeploy.{label}"))));
                    match csrf {
                        Token::Value("") => {}
                        Token::Value(token) => out.push_str(&token_field(token)),
                        Token::Slots(slots) => slots.push(out.len()),
                    }
                    out.push_str("</form>\n");
                }
                out.push_str("</div>\n</div>\n");
            }
            _ => {}
        }
    }
    out.push_str("</div>\n");
    budget::check().map_err(FiguresError::from)?;
    Ok(out)
}

/// The nonce and row ids are read from one snapshot, including when prices are cold.
struct LiquidationHalt { bases:Vec<String>, nonce:Option<String>, rows:Vec<i64> }
fn liquidation_halt(c:&Connection,id:i64)->Result<LiquidationHalt,FiguresError> {
    if c.is_autocommit() {let tx=c.unchecked_transaction()?;let halt=liquidation_halt(&tx,id)?;tx.commit()?;return Ok(halt)}
    let bot=crate::engine::model::load_bot(c,id).map_err(|e|FiguresError::Data(format!("{e:?}")))?;
    let pending=&bot.transient["liquidation_pending"];
    let ambiguous=pending["state"]=="ambiguous";
    let mut bases=vec![];
    if ambiguous {if let Some(base)=pending["symbol"].as_str().filter(|s|!s.trim().is_empty()) {bases.push(base.to_string());}}
    let resolved:Vec<i64>=bot.transient["liquidation_resolved_orders"].as_array().into_iter().flatten().filter_map(|v|v.as_i64().or_else(||v.as_str()?.parse().ok())).collect();
    let mut rows=vec![];
    for row in c.prepare("SELECT id,base FROM transactions WHERE bot_id=?1 AND transaction_type='LIQUIDATION' AND external_status=4 ORDER BY id")?
        .query_map([id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?)))? {
        let (id,base)=row?;
        if resolved.contains(&id) {continue}
        rows.push(id);
        if let Some(base)=base.filter(|s|!s.trim().is_empty()) {if !bases.contains(&base) {bases.push(base);}}
    }
    Ok(LiquidationHalt {bases,rows,nonce:ambiguous.then(||pending["id"].as_str().map(str::to_string)).flatten()})
}
fn halt_control(id:i64,halt:&LiquidationHalt,locale:&str,prefix:&str,csrf:&mut Token<'_>)->String {
    let mut query=form_urlencoded::Serializer::new(String::new());
    if let Some(nonce)=&halt.nonce {query.append_pair("intent_id",nonce);}
    for id in &halt.rows {query.append_pair("order_ids[]",&id.to_string());}
    let mut out=format!("<span class=\"text-error\">{} · {}</span>\n<form class=\"button_to\" data-turbo-confirm=\"{}\" method=\"post\" action=\"{}/bots/{id}/liquidation_resolutions?{}\"><button class=\"rbutton rbutton--danger\" type=\"submit\">{}</button>",
        t(locale,"bot.liquidation.halted"),escape(&halt.bases.join(", ")),escape(&i18n::text(locale,"bot.liquidation.resolve_confirm",&[])),escape(prefix),escape(&query.finish()),t(locale,"bot.liquidation.resolve"));
    match csrf {Token::Value("")=>{},Token::Value(token)=>out.push_str(&token_field(token)),Token::Slots(slots)=>slots.push(out.len())}
    out.push_str("</form>\n");out
}

/// LiquidationsController#new: cached prices where present, the ledger amount where cold.
pub(crate) fn liquidation_table(c:&Connection,ctx:&crate::web::layout::Ctx,id:i64,selected:&[(String,i64)])->Result<(String,bool,bool),FiguresError> {
    budget::within(|| {
        let now=At::from_utc(ctx.now).ok_or_else(||FiguresError::Data("invalid time".into()))?;
        let s=Subject::load(c,id)?;
        let ledger=walk::metrics(c,&s,now)?;
        let cache=super::loading::cached_market(c,&ctx.app,s.bot.user_id).map_err(|e|FiguresError::Data(format!("{e:?}")))?;
        let reader=crate::figures::page_market::Reader::new(&cache,ctx.now.timestamp()).with_current(c);
        let priced=live::live(c,&s,&ledger,&reader,now)?;
        let members:HashSet<i64>=c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id=?1 AND in_index=1")?.query_map([id],|r|r.get(0))?.collect::<Result<_,_>>()?;
        let mut rows=vec![];
        for (key,asset) in selected {
            let value=priced.asset_values.iter().find(|(k,_)|k==key).map(|(_,v)|v).filter(|_|!priced.prices_stale && !priced.unpriced.iter().any(|p|p.key==*key));
            let amount=ledger.asset_breakdown.iter().find(|(k,_)|k==key).map(|(_,v)|v.amount.clone()).ok_or_else(||FiguresError::Data("holding disappeared".into()))?;
            rows.push(Holding {key:key.clone(),asset:Some(*asset),amount,
                avg:value.map(|v|v.avg_price.clone()),value:value.map(|v|v.current_value.clone()),pnl:value.map(|v|v.pnl_percentage.clone()),
                harvest:value.is_some_and(|v|v.harvestable),sellable:true,can_sell:false,lock:None});
        }
        let harvest=rows.iter().any(|r|r.harvest);
        let exited=!members.is_empty() && selected.iter().all(|(_,id)|!members.contains(id));
        let table=table(c,&s,&rows.iter().collect::<Vec<_>>(),"liquidation_confirm_list",false,ctx.user().is_some_and(|u|u.hide_balances),ctx.locale,"")?;
        Ok((table,harvest,exited))
    })
}

#[cfg(test)]
#[path="../../../tests/support/normalized_redeploy.rs"]
mod normalized_redeploy_tests;

#[cfg(test)]
mod token_tests {
    use super::*;
    /// A page's form token goes only where the renderer put a form, and a broadcast is the kept fragment as it is: stored
    /// text that looks like a token field, the old placeholder or a form gets nothing.
    #[test]
    fn token_slots_only_at_renderer_positions() {
        let hostile = "figuresFormTokenPlaceholder7f3a9c\" name=\"authenticity_token\" value=\"\" <input type=\"hidden\" value=\"\" />";
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../../tests/fixtures/settings_primary_schema.sql")).unwrap();
        let t = "'2026-01-01'";
        c.execute_batch(&format!("
            INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES (1,'o@example.com','x',{t},{t});
            INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (1,'Exchanges::Alpaca','Alpaca',{t},{t});
            INSERT INTO assets(id,category,external_id,symbol,name,created_at,updated_at) VALUES (1,'Fiat','usd','USD','US Dollar',{t},{t}),(2,'Stock','aaa','AAA','AAA Inc',{t},{t});
            INSERT INTO tickers(id,base,base_asset_id,base_decimals,exchange_id,minimum_base_size,minimum_quote_size,price_decimals,quote,quote_asset_id,quote_decimals,ticker,created_at,updated_at)
                VALUES (1,'AAA',2,9,1,0,1,2,'USD',1,2,'AAA',{t},{t});
            INSERT INTO bots(id,user_id,exchange_id,type,status,label,settings,transient_data,created_at,updated_at) VALUES (1,1,1,'Bots::DcaIndex',1,?1,'{{}}','{{}}',{t},{t});
            INSERT INTO bot_index_assets(bot_id,asset_id,ticker_id,in_index,created_at,updated_at) VALUES (1,2,1,1,{t},{t});
            INSERT INTO transactions(bot_id,exchange_id,base,base_asset_id,quote,quote_asset_id,amount,amount_exec,price,quote_amount_exec,side,status,external_status,external_id,transaction_type,created_at,updated_at)
                VALUES (1,1,'AAA',2,'USD',1,1,1,110,110,1,0,2,'o1','LIQUIDATION','2026-03-02 00:00:00',{t});
        ").replace("?1", &format!("'{}'", hostile.replace('\'', "''")))).unwrap();
        c.execute("UPDATE assets SET symbol=?1, image_url=?2 WHERE id=2",
                  [hostile, &format!("https://images.example/{hostile}.png")]).unwrap();
        let s = Subject { bot: db::Bot { id: 1, user_id: 1, exchange_id: Some(1), kind: db::Kind::Index, exchange_type: Some("Exchanges::Alpaca".into()), quote_asset_id: Some(1), base_asset_ids: vec![2] },
                          orders: db::orders(&c, 1).unwrap(), tickers: vec![], quote: Some("USD".into()) };
        let mut m = Metrics::empty();
        let one = Num::Int(1);
        m.asset_values = vec![(hostile.into(), walk::AssetValue { amount: one.clone(), quote_invested: one.clone(), current_value: one.clone(), current_price: one.clone(), avg_price: one.clone(), pnl_percentage: Num::Int(0), tax_basis: one.clone(), harvestable: false })];
        m.key_assets = vec![(hostile.into(), Some(2))];
        m.walked = Some(walk::Walked { shadowed_by: vec![], external_sales: false, restated_at: None, restated_before_external_sales: false, realised_cash: Num::Int(110), asset_lots: vec![], tax_pnl_by_transaction: vec![], loss_lot_by_transaction: vec![] });
        let now = At::from_sql("2026-03-10 00:00:00").unwrap();
        let rendered = |token: &mut Token<'_>| render(&c, &s, &m, &[], false, "en", token, "", now).unwrap();
        let token = "a\"&<>'b";
        let page = rendered(&mut Token::Value(token));
        let broadcast = rendered(&mut Token::Value(""));
        let mut slots = vec![];
        let kept = rendered(&mut Token::Slots(&mut slots));
        assert_eq!(slots.len(), 2, "one slot per redeploy form: {kept}");
        assert!(broadcast.contains("redeploy-prompt__actions") && !broadcast.contains("value=\"\" autocomplete=\"off\""), "a broadcast has no session: Rails writes no token field: {broadcast}");
        assert!(page.contains("value=\"a&quot;&amp;&lt;&gt;&#39;b\" autocomplete=\"off\" />"), "the token is escaped: {page}");
        assert_eq!(with_token(&kept, &slots, token), page, "the page: each request's token where the renderer put a form, and nowhere else");
        assert_eq!(kept, broadcast, "the broadcast: the kept fragment as it is");
        assert_eq!(with_token(&kept, &slots, ""), broadcast, "no token, no field");
        assert_eq!(page.matches("name=\"authenticity_token\" value=\"a&quot;").count(), 2);
    }
}
