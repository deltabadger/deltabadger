//! The orders feed of the bot page: BotActivityFeed, bots/show.turbo_stream.erb and the three rows
//! it appends (orders/_order, _order_timeline, _activity), ten at a time behind a cursor.
use super::status::humanized_error;
use super::Bot;
use crate::codec::{format_time, parse_time};
use crate::ruby::{to_sentence, BigDec};
use crate::web::auth::User;
use crate::web::bots::CASH;
use crate::web::format::{self, Num};
use crate::web::i18n::{self, escape, Arg};
use crate::web::layout::Ctx;
use crate::web::{turbo, WebError};
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::Value;
use std::collections::HashMap;

/// BotActivityFeed: rows per page.
pub const PAGE: usize = 10;

struct Order {
    id: i64,
    created_at: DateTime<Utc>,
    /// 0 submitted, 1 failed, 2 skipped.
    status: Option<i64>,
    /// 0 unknown, 1 open, 2 closed, 3 cancelled, 4 abandoned.
    external_status: Option<i64>,
    sell: bool,
    /// A scheduled buy: `side` is 0 and `transaction_type` is REGULAR. What `bot::refusal` asks of every order of a bot whose page is served.
    scheduled_buy: bool,
    price: Option<BigDec>,
    amount: Option<BigDec>,
    quote_amount: Option<BigDec>,
    amount_exec: Option<BigDec>,
    quote_amount_exec: Option<BigDec>,
    base: Option<String>,
    quote: Option<String>,
    base_asset_id: Option<i64>,
    quote_asset_id: Option<i64>,
    error_messages: Vec<String>,
    /// `price`, `amount`, `quote_amount`, `amount_exec` and `quote_amount_exec` as the row holds them. They are read into the
    /// fields above (`read`) only for a row that is shown: the eleventh row of a page says that there is a next page, and no more.
    raw: [rusqlite::types::Value; 5],
}

impl Order {
    /// Reads the row's five amounts. One that this build does not read fails as `bot::Unreadable`: the handler answers the 501 page.
    fn read(&mut self) -> Result<(), WebError> {
        let [price, amount, quote_amount, amount_exec, quote_amount_exec] = &self.raw;
        (self.price, self.amount, self.quote_amount) = (super::stored(price)?, super::stored(amount)?, super::stored(quote_amount)?);
        (self.amount_exec, self.quote_amount_exec) = (super::stored(amount_exec)?, super::stored(quote_amount_exec)?);
        Ok(())
    }

    fn submitted(&self) -> bool { self.status == Some(0) }
    fn failed(&self) -> bool { self.status == Some(1) }
    fn skipped(&self) -> bool { self.status == Some(2) }
    fn open(&self) -> bool { self.external_status == Some(1) }
    fn pending(&self) -> bool { matches!(self.external_status, Some(0 | 1)) }
    fn closed(&self) -> bool { self.external_status == Some(2) }
    fn stopped(&self) -> bool { matches!(self.external_status, Some(3 | 4)) }
    /// Transaction#other?: no column of its own in the table; shown as its sentence under "Other".
    fn other(&self) -> bool { !self.submitted() || self.stopped() }
    /// `failed? ? 'text-danger' : (inactive_order_row? ? 'text-inactive' : '')`.
    fn row_class(&self) -> &'static str {
        if self.failed() { "text-danger" } else if self.skipped() || (self.submitted() && self.stopped()) { "text-inactive" } else { "" }
    }
}

struct Log {
    id: i64,
    created_at: DateTime<Utc>,
    event: String,
    message: Option<String>,
    details: Value,
}

enum Item {
    Order(Box<Order>),
    Log(Log),
}

impl Item {
    fn key(&self) -> (DateTime<Utc>, i64, i64) {
        // Newest first; at one instant an activity row before a transaction; then the higher id.
        match self { Item::Log(log) => (log.created_at, 0, log.id), Item::Order(order) => (order.created_at, -1, order.id) }
    }
}

/// BotActivityFeed::Cursor: `<created_at, UTC, six decimals>|<activity|transaction>|<id>`.
struct Cursor {
    at: DateTime<Utc>,
    activity: bool,
    id: i64,
}

fn cursor(value: Option<&str>) -> Option<Cursor> {
    let mut parts = value?.splitn(3, '|');
    let (at, kind, id) = (parts.next()?, parts.next()?, parts.next().unwrap_or(""));
    let activity = match kind { "activity" => true, "transaction" => false, _ => return None };
    let at = DateTime::parse_from_rfc3339(at).ok()?.with_timezone(&Utc);
    // `id.to_i`, which Rails writes into the query as it is: past the column's range it compares as the range's end does.
    let id = format::to_i(id).clamp(i128::from(i64::MIN), i128::from(i64::MAX));
    Some(Cursor { at, activity, id: i64::try_from(id).unwrap_or(0) })
}

/// The cursor's condition on one table, as BotActivityFeed#cursor_predicate words it.
fn before(cursor: Option<&Cursor>, activity: bool) -> (String, Vec<rusqlite::types::Value>) {
    let Some(cursor) = cursor else { return (String::new(), vec![]) };
    let at = rusqlite::types::Value::Text(format_time(cursor.at));
    if activity == cursor.activity {
        (" AND (created_at < ?2 OR (created_at = ?2 AND id < ?3))".into(), vec![at, rusqlite::types::Value::Integer(cursor.id)])
    } else if !activity {
        // A transaction ranks after an activity of the same instant.
        (" AND created_at <= ?2".into(), vec![at])
    } else {
        (" AND created_at < ?2".into(), vec![at])
    }
}

fn load(c: &Connection, bot: &Bot, cursor: Option<&Cursor>) -> Result<Vec<Item>, WebError> {
    let time = |text: String| parse_time(&text).map_err(|e| rusqlite::Error::InvalidColumnName(format!("{e:?}")));
    let mut items = vec![];
    let (condition, mut values) = before(cursor, false);
    values.insert(0, rusqlite::types::Value::Integer(bot.id));
    let mut orders = c.prepare(&format!("SELECT id, created_at, status, external_status, side, price, amount, quote_amount, amount_exec, quote_amount_exec, base, quote, \
                                         base_asset_id, quote_asset_id, error_messages, transaction_type FROM transactions WHERE bot_id = ?1{condition} ORDER BY created_at DESC, id DESC LIMIT {}", PAGE + 1))?;
    let rows = orders.query_map(rusqlite::params_from_iter(values), |r| {
        let messages: Option<String> = r.get(14)?;
        Ok(Order {
            id: r.get(0)?, created_at: time(r.get(1)?)?, status: r.get(2)?, external_status: r.get(3)?, sell: r.get::<_, Option<i64>>(4)? == Some(1), scheduled_buy: r.get::<_, Option<i64>>(4)? == Some(0) && r.get::<_, String>(15)? == "REGULAR",
            price: None, amount: None, quote_amount: None, amount_exec: None, quote_amount_exec: None, raw: [r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?],
            base: r.get(10)?, quote: r.get(11)?, base_asset_id: r.get(12)?, quote_asset_id: r.get(13)?,
            error_messages: messages.and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok()).unwrap_or_default().iter()
                .map(|m| m.as_str().map_or_else(|| m.to_string(), str::to_string)).collect(),
        })
    })?.collect::<Result<Vec<_>, _>>()?;
    items.extend(rows.into_iter().map(|order| Item::Order(Box::new(order))));
    let (condition, mut values) = before(cursor, true);
    values.insert(0, rusqlite::types::Value::Integer(bot.id));
    let mut logs = c.prepare(&format!("SELECT id, created_at, event, message, details FROM bot_activity_logs WHERE bot_id = ?1 AND event NOT IN ('order_skipped', 'order_ignored'){condition} \
                                       ORDER BY created_at DESC, id DESC LIMIT {}", PAGE + 1))?;
    let rows = logs.query_map(rusqlite::params_from_iter(values), |r| {
        Ok(Log { id: r.get(0)?, created_at: time(r.get(1)?)?, event: r.get(2)?, message: r.get(3)?,
                 details: r.get::<_, Option<String>>(4)?.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null) })
    })?.collect::<Result<Vec<_>, _>>()?;
    items.extend(rows.into_iter().map(Item::Log));
    items.sort_by_key(|item| std::cmp::Reverse(item.key()));
    Ok(items)
}

/// What every row of one response shares.
struct Rows<'a> {
    ctx: &'a Ctx,
    bot: &'a Bot,
    zone: &'a str,
    hidden: bool,
    /// BotsController#composition_decimals: the precision of each asset, by its id and by its symbols.
    decimals: HashMap<String, u8>,
}

const NO_VALUE: &str = "<span class=\"no-value\">—</span>";

/// BotHelper#round_amount, printed: money to the cent, anything else at its ticker's precision,
/// and as it is when the precision is not known.
fn round_amount(value: &BigDec, decimals: Option<u8>, currency: Option<&str>) -> String {
    if currency.is_some_and(|currency| CASH.contains(&currency)) {
        return format::number_with_precision(&Num::Dec(value.clone()), 2, false).unwrap_or_default();
    }
    decimals.map_or_else(|| value.to_s_f(), |decimals| value.round(decimals).to_s_f())
}

impl Rows<'_> {
    fn t(&self, key: &str, args: &[(&str, Arg)]) -> String {
        i18n::t(self.ctx.locale, key, args)
    }

    /// BotHelper#order_decimals: by the asset's id, else by the symbol the row recorded.
    fn decimals_of(&self, asset_id: Option<i64>, symbol: Option<&str>) -> Option<u8> {
        asset_id.and_then(|id| self.decimals.get(&id.to_string())).or_else(|| symbol.and_then(|symbol| self.decimals.get(symbol))).copied()
    }

    fn base(&self, order: &Order, value: &BigDec) -> String {
        round_amount(value, self.decimals_of(order.base_asset_id, order.base.as_deref()), order.base.as_deref())
    }

    fn quote(&self, order: &Order, value: &BigDec) -> String {
        round_amount(value, self.decimals_of(order.quote_asset_id, order.quote.as_deref()), order.quote.as_deref())
    }

    /// ApplicationHelper#table_when: the date, and the clock behind it in `<small>`.
    fn when(&self, at: DateTime<Utc>) -> String {
        format!("{} <small>{}</small>", format::table_date(at, self.zone), format::table_clock(at, self.zone, self.ctx.locale))
    }

    /// The Cancel button of an open order. Its form is the only thing in the feed that carries the
    /// session's token, so a feed without one leaves the session as it was.
    fn cancel(&self, order: &Order) -> String {
        if !order.open() { return "\n  ".to_string(); }
        format!("\n      <div data-controller=\"class-toggle\" data-class-toggle-toggle-classes-value='[\"hidden\"]'>\n        \
                 <form class=\"button_to\" method=\"post\" action=\"{}\"><input type=\"hidden\" name=\"_method\" value=\"delete\" /><button class=\"sinput sinput--small sinput--hover-warning\" \
                 data-class-toggle-target=\"togglable\" data-action=\"click-&gt;class-toggle#toggle\" type=\"submit\">{}</button><input type=\"hidden\" name=\"authenticity_token\" value=\"{}\" /></form>\n        \
                 <div class=\"loader--small hidden\" style=\"position: unset; float: right; margin-right: 0.5rem;\" data-class-toggle-target=\"togglable\"></div>\n      </div>\n  ",
                escape(&self.ctx.path(&format!("/bots/{}/transactions/{}", self.bot.id, order.id))), self.t("bot.cancel_order", &[]), escape(&self.ctx.csrf_token()))
    }

    /// orders/_order.html.erb: the columnar row of an accepted order.
    fn order(&self, order: &Order) -> String {
        let executed = order.submitted() && order.closed();
        let amount = if executed { order.amount_exec.clone().or_else(|| order.amount.clone()) } else { order.amount.clone() };
        let mut quote_amount = if executed { order.quote_amount_exec.clone().or_else(|| order.quote_amount.clone()) } else { order.quote_amount.clone() };
        if quote_amount.is_none() {
            if let (Some(price), Some(amount)) = (&order.price, &amount) { quote_amount = Some(price * amount); }
        }
        let small = |symbol: &Option<String>| format!("<small>{}</small>", escape(symbol.as_deref().unwrap_or("")));
        let mut cells = String::new();
        if !self.hidden {
            cells.push_str(&format!("    <td>{} {}</td>\n", amount.as_ref().map_or_else(|| NO_VALUE.to_string(), |amount| escape(&self.base(order, amount))), small(&order.base)));
            cells.push_str(&format!("    <td>{} {}</td>\n", quote_amount.as_ref().map_or_else(|| NO_VALUE.to_string(), |amount| escape(&self.quote(order, amount))), small(&order.quote)));
        }
        let unit_price = match (&amount, &quote_amount) { (Some(amount), Some(quote_amount)) => quote_amount.div(amount), _ => None };
        let price = unit_price.map_or_else(|| NO_VALUE.to_string(), |price| format!("{} {}", escape(&self.quote(order, &price)), small(&order.quote)));
        let tab = if order.submitted() && order.pending() { " data-order-type=\"waiting\"" } else if executed { " data-order-type=\"successful\"" } else { "" };
        format!("<tr id=\"transaction_{}\" class=\"{}\" data-hw-animate-in-prepend=\"animate-order-in\" data-order-filter-target=\"row\"{tab}>\n  <td scope=\"row\" class=\"table__when\">{}</td>\n\n{cells}\n  <td>{price}</td>\n\n  <td>{}</td>\n</tr>",
                order.id, order.row_class(), self.when(order.created_at), self.cancel(order))
    }

    /// BotHelper#transaction_summary: the order as a sentence.
    fn summary(&self, order: &Order) -> String {
        let (base, quote) = (order.base.as_deref().unwrap_or(""), order.quote.as_deref().unwrap_or(""));
        let sentence = |key: &str, amount: &str, quote_amount: &str| i18n::text(self.ctx.locale, &format!("bot_activity.transactions.{key}"), &[
            ("amount", Arg::Text(amount)), ("base", Arg::Text(base)), ("quote_amount", Arg::Text(quote_amount)), ("quote", Arg::Text(quote)),
        ]);
        if order.failed() {
            let error = humanized_error(self.bot, &order.error_messages, self.ctx.locale);
            // The attempted amounts are the only money in a failed row; with balances hidden the error alone remains.
            let amounts = if self.hidden { (None, None) } else { (order.amount.as_ref().map(|a| self.base(order, a)), order.quote_amount.as_ref().map(|a| self.quote(order, a))) };
            return match (amounts, error) {
                ((None, None), Some(error)) => i18n::text(self.ctx.locale, "bot_activity.transactions.failed_with_error", &[("error", Arg::Text(&error))]),
                ((None, None), None) => i18n::text(self.ctx.locale, "bot_activity.transactions.failed", &[]),
                ((amount, quote_amount), error) => {
                    let summary = sentence(if order.sell { "failed_sell" } else { "failed_buy" }, amount.as_deref().unwrap_or(""), quote_amount.as_deref().unwrap_or(""));
                    error.map_or(summary.clone(), |error| format!("{summary}: {error}"))
                }
            };
        }
        if order.skipped() { return i18n::text(self.ctx.locale, "bot_activity.transactions.skipped", &[]); }
        if order.stopped() { return i18n::text(self.ctx.locale, "bot_activity.transactions.cancelled", &[]); }
        // BotHelper#display_amount: what was asked while the order waits, what was executed once it is done.
        let pending = order.pending();
        let shown = |executed: &Option<BigDec>, requested: &Option<BigDec>| {
            if !pending && executed.as_ref().is_some_and(BigDec::is_positive) { executed.clone() } else { requested.clone() }
        };
        let amount = shown(&order.amount_exec, &order.amount).map(|amount| self.base(order, &amount)).unwrap_or_default();
        let quote_amount = shown(&order.quote_amount_exec, &order.quote_amount).map(|amount| self.quote(order, &amount)).unwrap_or_default();
        let key = match (order.sell, pending) { (true, true) => "open_sell", (true, false) => "sold", (false, true) => "open_buy", (false, false) => "bought" };
        sentence(key, &amount, &quote_amount)
    }

    /// orders/_order_timeline.html.erb: the same order as a sentence, for "All" and "Other".
    fn timeline(&self, order: &Order) -> String {
        if self.hidden && !order.other() {
            // Nothing to show while balances are hidden, but the row must exist: a later broadcast replaces it by id.
            return format!("<tr id=\"timeline_transaction_{}\" data-order-filter-target=\"row\" data-order-type=\"\"></tr>", order.id);
        }
        let tab = if self.hidden { "other" } else if order.other() { "all other" } else { "all" };
        format!("<tr id=\"timeline_transaction_{}\" class=\"{}\" data-hw-animate-in-prepend=\"animate-order-in\" data-order-filter-target=\"row\" data-order-type=\"{tab}\">\n    <td scope=\"row\" class=\"table__when\">{}</td>\n    \
                 <td colspan=\"{}\" class=\"table__sentence\">{}</td>\n    <td>{}</td>\n</tr>",
                order.id, order.row_class(), self.when(order.created_at), if self.hidden { 1 } else { 3 }, escape(&self.summary(order)), self.cancel(order))
    }

    /// BotHelper#bot_activity_summary.
    fn activity_summary(&self, log: &Log) -> String {
        if let Some(message) = log.message.as_deref().filter(|message| !message.trim().is_empty()) { return message.to_string(); }
        let detail = |key: &str| match log.details.get(key) {
            Some(Value::String(text)) => text.clone(),
            None | Some(Value::Null) => String::new(),
            Some(other) => other.to_string(),
        };
        let event = |name: &str, args: &[(&str, Arg)]| i18n::text(self.ctx.locale, &format!("bot_activity.events.{name}"), args);
        match log.event.as_str() {
            "market_closed" => {
                // BotHelper#format_activity_time: the date and the clock in the reader's zone; what is not a time stays as it is.
                let value = detail("next_market_open_at");
                let time = DateTime::parse_from_rfc3339(&value).map(|at| at.with_timezone(&Utc))
                    .map_or(value, |at| format!("{} {}", format::table_date(at, self.zone), format::table_clock(at, self.zone, self.ctx.locale)));
                event("market_closed", &[("time", Arg::Text(&time))])
            }
            "merged" => {
                let labels: Vec<String> = match log.details.get("source_labels") {
                    Some(Value::Array(labels)) => labels.iter().map(|label| label.as_str().map_or_else(|| label.to_string(), str::to_string)).collect(),
                    Some(Value::String(label)) => vec![label.clone()],
                    _ => vec![],
                };
                event("merged", &[("labels", Arg::Text(&to_sentence(&labels)))])
            }
            "split" => event("split", &[("label", Arg::Text(&detail("source_label")))]),
            "limit_paused" => event("limit_paused", &[("limit", Arg::Text(&detail("limit_type").replace('_', " ")))]),
            "execution_failed" => match humanized_error(self.bot, &[detail("error")], self.ctx.locale) {
                Some(error) => event("execution_failed_with_error", &[("error", Arg::Text(&error))]),
                None => event("execution_failed", &[]),
            },
            "stopped" => match detail("stop_message_key") {
                key if key.trim().is_empty() => event("stopped", &[]),
                key => event("stopped_with_reason", &[("reason", Arg::Text(&i18n::text(self.ctx.locale, &key, &[])))]),
            },
            "liquidation_failed" | "redeploy_failed" => event(&log.event, &[("error", Arg::Text(&detail("reason")))]),
            "liquidation_batch_cut_short" => event(&log.event, &[("bases", Arg::Text(&detail("bases")))]),
            "orders_below_minimum" => event(&log.event, &[("count", Arg::Count(log.details.get("count").and_then(Value::as_i64).unwrap_or(0))), ("bases", Arg::Text(&detail("bases")))]),
            "wash_sale_locked" => event(&log.event, &[("base", Arg::Text(&detail("base"))), ("until", Arg::Text(&detail("until")))]),
            "order_abandoned" => event(&log.event, &[("order_id", Arg::Text(&detail("order_id")))]),
            "asset_split" => match detail("ratio") {
                ratio if ratio.trim().is_empty() => event("asset_split_unknown_ratio", &[("base", Arg::Text(&detail("base")))]),
                ratio => event("asset_split", &[("base", Arg::Text(&detail("base"))), ("ratio", Arg::Text(&ratio))]),
            },
            other => event(other, &[]),
        }
    }

    /// orders/_activity.html.erb.
    fn activity(&self, log: &Log) -> String {
        format!("<tr id=\"bot_activity_log_{}\" class=\"text-inactive\" data-hw-animate-in-prepend=\"animate-order-in\" data-order-filter-target=\"row\" data-order-type=\"{}\">\n  \
                 <td scope=\"row\" class=\"table__when\">{}</td>\n  <td colspan=\"{}\" class=\"table__sentence\">{}</td>\n</tr>",
                log.id, if self.hidden { "other" } else { "all" }, self.when(log.created_at), if self.hidden { 1 } else { 3 }, escape(&self.activity_summary(log)))
    }
}

/// `CGI.escape`, as `to_query` writes a value into a URL.
fn query_escape(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// GET /bots/:id.turbo_stream for the `orders_pagination` frame: the next ten rows appended to the
/// list, and the frame replaced by one that asks for the ten after them, while there are more.
/// `Err` names why this page of the feed is not served: one of its rows is an order that is not a
/// scheduled buy, which `bot::refusal` leaves to the feed to find (it would walk the whole history
/// for it). Rails picks the merged page first (BotActivityFeed#page) and reads on from there, so
/// only the ten rows that are shown are asked and read: the rows after them belong to a later page.
pub fn feed(c: &Connection, ctx: &Ctx, bot: &Bot, user: &User) -> Result<Result<String, &'static str>, WebError> {
    let cursor = cursor(ctx.params.query("before"));
    let mut items = load(c, bot, cursor.as_ref())?;
    for item in items.iter_mut().take(PAGE) {
        let Item::Order(order) = item else { continue };
        if !order.scheduled_buy { return Ok(Err(super::BEYOND_BUYS)); }
        order.read()?;
    }
    let mut decimals = HashMap::new();
    if let (Some(quote), Some(precision)) = (bot.quote_asset.as_ref(), bot.quote_decimals()) {
        decimals.insert(quote.id.to_string(), precision);
        decimals.insert(quote.symbol().to_string(), precision);
    }
    // Holdings that left the composition still appear in the feed and keep their ticker's precision.
    for membership in &bot.memberships {
        let Some(ticker) = membership.ticker.as_ref() else { continue };
        decimals.insert(membership.asset.id.to_string(), ticker.base_decimals);
        decimals.insert(ticker.base.clone(), ticker.base_decimals);
        if let Some(symbol) = membership.asset.symbol.as_deref().filter(|symbol| !symbol.is_empty()) { decimals.entry(symbol.to_string()).or_insert(ticker.base_decimals); }
    }
    let rows = Rows { ctx, bot, zone: &user.time_zone, hidden: user.hide_balances, decimals };
    let mut appended = String::new();
    for item in items.iter().take(PAGE) {
        match item {
            Item::Order(order) => {
                if order.submitted() { appended.push_str(&format!("\n        {}", rows.order(order))); }
                appended.push_str(&format!("\n        {}", rows.timeline(order)));
            }
            Item::Log(log) => appended.push_str(&format!("\n        {}", rows.activity(log))),
        }
    }
    if !appended.is_empty() { appended.push('\n'); }
    let mut body = format!("{}\n\n", turbo::stream("append", "orders_list", &appended));
    if let (true, Some(last)) = (items.len() > PAGE, items.get(PAGE - 1)) {
        let (at, kind, id) = match last { Item::Order(order) => (order.created_at, "transaction", order.id), Item::Log(log) => (log.created_at, "activity", log.id) };
        let next = format!("{}|{kind}|{id}", at.format("%Y-%m-%dT%H:%M:%S%.6fZ"));
        let frame = format!("<turbo-frame id=\"orders_pagination\" src=\"{}.turbo_stream?before={}\"></turbo-frame>", escape(&ctx.path(&format!("/bots/{}", bot.id))), query_escape(&next));
        body.push_str(&turbo::stream("replace", "orders_pagination", &format!("\n    {frame}\n")));
    }
    Ok(Ok(body))
}
