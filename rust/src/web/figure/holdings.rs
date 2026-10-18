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

/// Bot::Composition::Redeployable#redeploy_offer: what may be put back, zero when nothing (or less than the smallest
/// placeable amount) is; None where Rails would rewrite the stored decline first (offset above the banked total).
pub(crate) fn redeploy_offer(c: &Connection, s: &Subject, m: &Metrics, _members: &HashSet<i64>) -> Result<Option<Dec>, FiguresError> {
    let (banked,spent)=crate::engine::accounting::index_redeploy_totals(&s.orders)?;
    let offset = decimal(c,"SELECT redeploy_declined_offset FROM bots WHERE id=?1",s.bot.id)?;
    let offer = (&(&banked - &spent)? - &offset)?;
    if offer.is_negative() { return Ok(None); }
    let cash = m.walked.as_ref().map(|w| w.realised_cash.to_d()).transpose()?.unwrap_or_else(Dec::zero);
    let offer = offer.min(cash);
    let Some(minimum) = crate::figures::db::composition_minimum(c, s)? else { return Ok(None); };
    Ok(Some(if offer >= minimum { offer } else { Dec::zero() }))
}

#[allow(clippy::too_many_arguments)]
/// A form's authenticity token: this one, or none (`Value("")`: a broadcast, where Rails' renderer has no session and
/// writes no field), or, for kept figures, nothing but the byte offset where each form's field goes (`Slots`): a page
/// writes its own token's field there and nowhere else (`with_token`), and a broadcast takes the fragment as it is.
pub(crate) enum Token<'a> { Value(&'a str), Slots(&'a mut Vec<usize>) }

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

#[allow(clippy::too_many_arguments)]
pub(super) fn render(c: &Connection, s: &Subject, m: &Metrics, missing: &[String], hidden: bool, locale: &str, csrf: &mut Token<'_>, prefix: &str, now: At) -> Result<String, FiguresError> {
    let quote = s.quote.as_deref().unwrap_or("");
    let (status, settings, transient): (i64, String, String) = c.query_row("SELECT status, settings, transient_data FROM bots WHERE id=?1", [s.bot.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    let settings: Value = serde_json::from_str(&settings).map_err(|_| FiguresError::Data("Invalid settings".into()))?;
    let transient: Value = serde_json::from_str(&transient).map_err(|_| FiguresError::Data("Invalid transient data".into()))?;
    let in_flight = ["rebalance_pending", "liquidation_pending", "liquidation_selling_since", "redeploy_pending"].iter().any(|k| !transient[k].is_null());
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
    let (enabled, jurisdiction): (Option<bool>,Option<String>) = c.query_row("SELECT wash_sale_enabled,wash_sale_jurisdiction FROM users WHERE id=?1",[s.bot.user_id],|r| Ok((r.get(0)?,r.get(1)?)))?;
    let mut locked_keys = HashSet::new();
    // User#wash_sale_jurisdiction uses presence, then the first wash-sale option (US).
    let jurisdiction = jurisdiction.as_deref().filter(|j| !j.trim().is_empty()).unwrap_or("US");
    if enabled == Some(true) && ["US","GB","IE"].contains(&jurisdiction) {
        let locks = c.prepare("SELECT l.asset_id,a.symbol,l.buy_locked_until,l.source FROM wash_sale_locks l JOIN assets a ON a.id=l.asset_id WHERE l.user_id=?1 AND l.asset_id IN (SELECT asset_id FROM bot_index_assets WHERE bot_id=?2) ORDER BY l.id")?
            .query_map([s.bot.user_id,s.bot.id],|r| Ok((r.get::<_,i64>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?,r.get::<_,Option<String>>(3)?)))?.collect::<Result<Vec<_>,_>>()?;
        let mut live = vec![];
        for (id,symbol,until,source) in locks {
            let Some(until)=until.as_deref().map(At::from_sql).transpose().map_err(|e|FiguresError::Data(format!("{e:?}")))? else {continue};
            if until > now {live.push((id,symbol,until,source.unwrap_or_default()));} // allow-swallow: optional source label
        }
        let candidates = live.iter().filter(|(id,_,_,_)| !m.key_assets.iter().any(|(_,asset)| *asset == Some(*id)))
            .map(|(id,symbol,_,_)| (crate::figures::keys::Identity::Asset(*id),symbol.as_deref().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("#{id}")))).collect::<Vec<_>>();
        let mut keys = crate::figures::keys::call(&candidates)?;
        let mut taken: HashSet<_> = m.key_assets.iter().map(|(k,_)| k.clone()).chain(keys.iter().filter(|(_,key)| !m.key_assets.iter().any(|(k,_)| k == key)).map(|(_,k)| k.clone())).collect();
        for (identity,key) in &mut keys {
            if m.key_assets.iter().any(|(k,_)| k == key) {
                let crate::figures::keys::Identity::Asset(id) = identity else { continue };
                while taken.contains(key) { budget::charge(1,0)?; *key = format!("{key}#{id}"); }
                taken.insert(key.clone());
            }
        }
        for (id,_,until,source) in live {
            let key = m.key_assets.iter().find(|(_,asset)| *asset == Some(id)).map(|(key,_)| key.clone())
                .or_else(|| keys.iter().find(|(asset,_)| *asset == crate::figures::keys::Identity::Asset(id)).map(|(_,key)| key.clone()));
            let Some(key) = key else { continue };
            let days = (until.utc().date_naive() - now.utc().date_naive()).num_days();
            let date = until.utc().format("%Y-%m-%d").to_string();
            if let Some(row) = rows.iter_mut().find(|r| r.key == key) { row.lock = Some((days,date,source)); }
            else { rows.push(Holding { key: key.clone(), asset: Some(id), amount: Num::Int(0),avg:None,value:None,pnl:None,harvest:false,sellable:false,can_sell:false,lock:Some((days,date,source)) }); }
            locked_keys.insert(key);
        }
    }
    let mut out = "<div id=\"metrics\" style=\"display: flex; flex-direction: column; gap: 1rem;\">\n".to_string();
    if m.prices_stale { out.push_str(&format!("<p class=\"status-button__hint\" role=\"status\">\n{}\n</p>\n", t(locale, "bot.details.stats.exchange_unavailable_html"))); }
    if !hidden {
        out.push_str(&format!("<div class=\"widget data-grid {}\">\n", if m.realised_pnl.is_zero() { "data-grid--two-columns" } else { "" }));
        for (label, value) in [("total_invested", Some(&m.total_quote_amount_invested)), ("portfolio_value", (!m.prices_stale && missing.is_empty()).then_some(&m.total_amount_value_in_quote))] {
            let value = value.map(|n| money(n).map(|v| quoted(v, quote))).transpose()?.unwrap_or(NO_VALUE.into());
            out.push_str(&format!("<div class=\"data-grid__item\" data-controller=\"tooltip\">\n<div class=\"label\">{}</div>\n<div class=\"data-grid__item__value\">\n{value}\n</div>\n</div>\n", t(locale, &format!("bot.details.stats.{label}"))));
        }
        if !m.realised_pnl.is_zero() {
            out.push_str(&format!("<div class=\"data-grid__item\" data-controller=\"tooltip\">\n<div class=\"label label--info\">\n{}\n<span class=\"tooltip-info-icon\" data-action=\"mouseenter->tooltip#showTooltip mouseleave->tooltip#hideTooltip click->tooltip#toggle\">\n{}\n</span>\n</div>\n<div class=\"data-grid__item__value {}\">\n{}{}\n</div>\n<div class=\"tooltip tooltip--note\">{}</div>\n</div>\n", t(locale,"bot.dca_index.realised_pnl"), include_str!("../../../templates/svg/_24x24_info.html"), if m.realised_pnl.is_negative() { "text-danger" } else { "text-success" }, if m.realised_pnl.is_negative() { "" } else { "+" }, quoted(precision(&m.realised_pnl,2,true)?,quote), t(locale,"bot.dca_index.realised_pnl_note")));
        }
        out.push_str("</div>\n");
    }
    let exited: Vec<_> = rows.iter().filter(|r| !locked_keys.contains(&r.key) && r.sellable && r.asset.is_some_and(|id| !members.is_empty() && !members.contains(&id))).collect();
    let composed: Vec<_> = rows.iter().filter(|r| !locked_keys.contains(&r.key) && (missing.contains(&r.key) || r.asset.is_none_or(|id| members.is_empty() || members.contains(&id)))).collect();
    let actions = rows.iter().any(|r| r.can_sell);
    if !composed.is_empty() { out.push_str(&format!("<div id=\"assets_metrics_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n{}\n</div>\n", table(c,s,&composed,"assets_metrics_list",actions,hidden,locale,prefix)?)); }
    if !exited.is_empty() {
        let title = if s.bot.kind == db::Kind::Index { "bot.dca_index.out_of_the_index" } else { "bot.dca_multi_asset.removed_from_portfolio" };
        out.push_str(&format!("<div id=\"exited_metrics_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n<div class=\"exited-header\">\n<span class=\"label\">{}</span>\n", t(locale,title)));
        if exited.len() > 1 && may_sell {
            let mut q = form_urlencoded::Serializer::new(String::new());
            for row in &exited { if let Some(id) = row.asset { q.append_pair("asset_id[]", &id.to_string()); } }
            for row in &exited { q.append_pair("symbol[]", &row.key); }
            out.push_str(&format!("<a class=\"rbutton rbutton--small rbutton--danger\" data-turbo-frame=\"modal\" href=\"{}/bots/{}/liquidation/new?{}\">{}</a>\n", escape(prefix),s.bot.id,escape(&q.finish()),t(locale,"bot.liquidation.sell_all")));
        }
        out.push_str(&format!("</div>\n{}\n</div>\n",table(c,s,&exited,"exited_metrics_list",actions,hidden,locale,prefix)?));
    }
    let locked: Vec<_> = rows.iter().filter(|r| locked_keys.contains(&r.key)).collect();
    if !locked.is_empty() {
        out.push_str(&format!("<div id=\"wash_sale_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n<div class=\"exited-header\">\n<span class=\"label\">{}</span>\n<a class=\"rbutton rbutton--small rbutton--secondary\" data-turbo-frame=\"_top\" href=\"{}/settings/account\">{}</a>\n</div>\n{}\n</div>\n",t(locale,"bot.wash_sale.table_title"),escape(prefix),t(locale,"bot.wash_sale.settings_link"),table(c,s,&locked,"wash_sale_list",true,hidden,locale,prefix)?));
    }
    if !hidden && !matches!(status,3|7) && settings["direction"] != "selling" && !in_flight {
        // Never rewrite the user's persisted decline from a GET. No redeploy offer for that invalid state.
        match redeploy_offer(c,s,m,&members)? {
            None => out.push_str("<p role=\"status\">Redeploy unavailable</p>\n"),
            Some(offer) if offer.is_positive() => {
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
            Some(_) => {}
        }
    }
    out.push_str("</div>\n");
    budget::check().map_err(FiguresError::from)?;
    Ok(out)
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
