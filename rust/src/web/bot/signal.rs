//! Bots::Signal as its pages read it (plan 8c-1): its rules (BotSignal), what refuses its pages, the settings column
//! (bots/signals/_settings with settings/_main, _unresolved_placement and _signal_widget), the panel it shares with
//! single-asset bots while its figures load (bots/dca_single_assets/_metrics), and its :start validation.
use super::{For, Bot};
use crate::enums::BotStatus;
use crate::figures::num::Num;
use crate::ruby::BigDec;
use crate::web::i18n::{self, escape, Arg};
use crate::web::layout::Ctx;
use crate::web::{colors, format, WebError};
use askama::Template;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;

/// A BotSignal row: one rule of a signal bot.
#[derive(Clone, Debug)]
pub struct Rule {
    pub id: i64,
    pub token: String,
    pub sell: bool,
    /// amount_type :percentage (else :fixed, in the quote).
    pub percentage: bool,
    pub amount: BigDec,
    pub enabled: bool,
    pub last_triggered_at: Option<DateTime<Utc>>,
}

/// Why a rule row is not read: its enums, amount and time are what Rails validates and every page prints.
const UNREAD_RULE: &str = "a signal rule stored in a shape this build does not read";

/// `bot.bot_signals`, in id order. A row that does not read (an enum Rails does not know, an amount that is no decimal, a
/// time that is no time) fails as data: `refusal` names it before any page reads the rules.
pub fn rules(c: &Connection, bot_id: i64) -> Result<Vec<Rule>, WebError> {
    read_rules(c, bot_id)?.map_err(super::data)
}

/// The rules, or what in a row does not read (the inner error).
fn read_rules(c: &Connection, bot_id: i64) -> Result<Result<Vec<Rule>, String>, WebError> {
    let mut statement = c.prepare("SELECT id, token, direction, amount_type, amount, enabled, last_triggered_at FROM bot_signals WHERE bot_id = ?1 ORDER BY id")?;
    let rows = statement.query_map([bot_id], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?,
            r.get::<_, rusqlite::types::Value>(4)?, r.get::<_, Option<bool>>(5)?, r.get::<_, Option<String>>(6)?))
    })?.collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().map(|(id, token, direction, amount_type, amount, enabled, triggered)| {
        let bad = |what: String| format!("bot_signals {id}: {what}");
        let sell = match direction { Some(0) => false, Some(1) => true, other => return Err(bad(format!("direction {other:?}"))) };
        let percentage = match amount_type { Some(0) => false, Some(1) => true, other => return Err(bad(format!("amount_type {other:?}"))) };
        let amount = format::stored(&amount).ok().flatten().ok_or_else(|| bad("amount".into()))?;
        let last_triggered_at = triggered.as_deref().map(crate::codec::parse_time).transpose().map_err(|e| bad(format!("last_triggered_at {e:?}")))?;
        Ok(Rule { id, token: token.ok_or_else(|| bad("token".into()))?, sell, percentage, amount, enabled: enabled.ok_or_else(|| bad("enabled".into()))?, last_triggered_at })
    }).collect())
}

/// The marker an order of unknown fate leaves (Bots::Signal#unresolved_placement): a Hash, else none.
fn marker(transient: &Value) -> Option<&serde_json::Map<String, Value>> {
    transient.get(crate::engine::signal::MARKER_KEY).and_then(Value::as_object)
}

/// `bot::refusal` for a signal bot: its pages, forms and actions are served before slice 8d admits the type only where
/// the app says so (App::admit_signal_bots_for_tests).
#[allow(clippy::too_many_arguments)]
pub(super) fn refusal(c: &Connection, bot_id: i64, exchange: Option<&str>, settings: &Value, transient: &Value, wash_sale_enabled: Option<bool>,
                      asked: For, admitted: bool) -> Result<Option<&'static str>, WebError> {
    if !admitted { return Ok(Some("a signal bot (plan 8d admits the type)")); }
    if let Some(why) = crate::engine::eligibility::admission(exchange, false, false) { return Ok(Some(why)); }
    // The two ids the wizard always stores, of assets that exist: every panel prints both.
    let id = |key: &str| settings.get(key).and_then(Value::as_i64);
    let (Some(base), Some(quote)) = (id("base_asset_id"), id("quote_asset_id")) else { return Ok(Some("settings the wizard always stores are missing")) };
    let assets: i64 = c.query_row("SELECT count(*) FROM assets WHERE id IN (?1, ?2)", [base, quote], |r| r.get(0))?;
    if assets != if base == quote { 1 } else { 2 } { return Ok(Some("a signal bot whose pair names an asset that is gone")); }
    let Ok(rules) = read_rules(c, bot_id)? else { return Ok(Some(UNREAD_RULE)) };
    // The marker's id is what its Resolve form posts back.
    if marker(transient).is_some_and(|m| !m.get("id").is_some_and(Value::is_string)) { return Ok(Some("an unresolved order marker without its id")); }
    let unbounded: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM tickers t WHERE t.exchange_id = (SELECT exchange_id FROM bots WHERE id = ?1) \
         AND (t.base_decimals NOT BETWEEN 0 AND ?2 OR t.quote_decimals NOT BETWEEN 0 AND ?2 OR t.price_decimals NOT BETWEEN 0 AND ?2))",
        (bot_id, super::MAX_DECIMALS), |r| r.get(0))?;
    if unbounded { return Ok(Some("a ticker with more decimals than this build rounds to")); }
    // The account-wide rule: its locked table (composition/_locked_table) is not rendered here.
    match wash_sale_enabled {
        _ if asked == For::Feed => {}
        Some(true) => return Ok(Some("the wash-sale rule")),
        // Bots::Signal#sell_capable?: an enabled sell rule. The question's modal opens over the page.
        None if rules.iter().any(|rule| rule.sell && rule.enabled) => return Ok(Some("the wash-sale question not answered yet")),
        _ => {}
    }
    Ok(None)
}

/// The pair's ticker (Bots::Signal#ticker: `tickers.first`) and its three precisions (Bots::Signal#decimals).
pub struct Decimals { pub base: Option<u8>, pub quote: Option<u8>, pub price: Option<u8> }

pub fn decimals(c: &Connection, bot: &Bot) -> Result<Decimals, WebError> {
    let Some(ticker) = bot.tickers.first() else { return Ok(Decimals { base: None, quote: None, price: None }) };
    let price: Option<i64> = c.query_row("SELECT price_decimals FROM tickers WHERE id = ?1", [ticker.id], |r| r.get(0))?;
    Ok(Decimals { base: Some(ticker.base_decimals), quote: Some(ticker.quote_decimals), price: price.and_then(|p| u8::try_from(p).ok()) })
}

/// `bot.invalid?(:start)` of a signal bot: Bot (not archived, a working trading key), Bot::AssetConfigurable's exchange
/// check (skipped for a stopped bot) and Bots::Signal's own: its ticker listed and trading. The retired-venue and
/// order-placement checks of Automation::ExchangeConnectable never fail on a venue `refusal` lets through.
pub fn start_invalid(c: &Connection, bot: &Bot) -> Result<bool, WebError> {
    if bot.status == BotStatus::Archived || !bot.api_key_correct() { return Ok(true); }
    let id = |key: &str| bot.settings.get(key).and_then(Value::as_i64);
    if !matches!(bot.status, BotStatus::Stopped | BotStatus::Deleted) {
        let listed: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM tickers WHERE exchange_id = ?1 AND available = 1 AND trading_enabled = 1 AND base_asset_id IS ?2 AND quote_asset_id IS ?3)",
                                       (bot.exchange.id, id("base_asset_id"), id("quote_asset_id")), |r| r.get(0))?;
        if !listed { return Ok(true); }
    }
    Ok(!start_ticker_ready(bot))
}

/// `ticker.present? && ticker.available? && ticker.trading_enabled?`.
fn start_ticker_ready(bot: &Bot) -> bool { bot.tickers.first().is_some_and(super::Ticker::tradable) }

/// Bot#start_blocked_by_unavailable_ticker? with Bots::Signal#tickers_for_start (`[ticker]`, nil included).
pub fn start_blocked(bot: &Bot) -> bool { !start_ticker_ready(bot) }

/// A ticker pill of the main rule (`tag.div symbol, class:, style:, data: ticker_data(asset)`).
struct Pill { class: &'static str, color: String, symbol: Option<String>, asset_id: Option<i64> }

struct RuleView { id: i64, path: String, token: String, sell: bool, percentage: bool, enabled: bool, amount: String, triggered: String }

#[derive(Template)]
#[template(path = "bots/signals/_settings.html")]
struct Settings<'a> {
    v: &'a Ctx,
    csrf: &'a str,
    path: String,
    base: Pill,
    quote: Pill,
    quote_symbol: String,
    unresolved: String,
    rules: Vec<RuleView>,
    several: bool,
    working: bool,
    hide_money: bool,
    base_url: &'a str,
    api_hint: String,
}

/// `signal.amount.to_d.to_s('F').sub(/([0-9]\d*)\.0$/, '\1')`.
fn amount_text(amount: &BigDec) -> String {
    let text = amount.to_s_f();
    text.strip_suffix(".0").map_or_else(|| text.clone(), str::to_string)
}

/// `time_ago_in_words(time)`: the dotiw gem's words from that time to now, its calendar counted from the earlier time. Of
/// two times (not a duration) dotiw leaves the seconds out wherever there are minutes (`include_seconds: false`).
/// ponytail: the calendar ends at `time` plus the whole seconds, not at now's own sub-second instant; the two can name
/// different hours only within a second of an hour's turn, past four weeks.
fn ago(time: DateTime<Utc>, now: DateTime<Utc>, locale: &str) -> String {
    let whole = (now - time).num_seconds().abs();
    let shown = if (whole % 3600) / 60 != 0 { whole - whole % 60 } else { whole };
    format::distance_of_time_in_words(shown as f64, if time <= now { time } else { now }, locale)
}

/// bots/signals/_unresolved_placement.html.erb. `csrf`: the page's token, or "" for a broadcast (no session, no field).
pub fn unresolved(ctx: &Ctx, csrf: &str, bot: &Bot) -> String {
    let Some(marker) = marker(&Value::Object(bot.transient.clone())).cloned() else {
        return "    <div id=\"signal-unresolved-placement\" hidden></div>\n".into();
    };
    let exchange = [("exchange", Arg::Text(&bot.exchange.name))];
    let token = if csrf.is_empty() { String::new() } else { format!("<input type=\"hidden\" name=\"authenticity_token\" value=\"{}\" />", escape(csrf)) };
    let id = marker.get("id").and_then(Value::as_str).unwrap_or_default();
    format!("  <div id=\"signal-unresolved-placement\" class=\"widget redeploy-prompt\">\n    <span class=\"text-error\">{}</span>\n    \
             <form class=\"button_to\" method=\"post\" action=\"{}/signal_resolution\"><button class=\"rbutton rbutton--danger\" type=\"submit\">{}</button>{token}\
             <input type=\"hidden\" name=\"marker_id\" value=\"{}\" /></form>\n  </div>\n",
            i18n::t(ctx.locale, "bot.signal.unresolved", &exchange), escape(&ctx.path(&format!("/bots/{}", bot.id))), i18n::t(ctx.locale, "bot.signal.resolve", &exchange), escape(id))
}

/// Bots::Signal#broadcast_unresolved_placement: the partial on its own (no column around it), without a form token.
pub fn unresolved_broadcast(ctx: &Ctx, bot: &Bot) -> String {
    let partial = unresolved(ctx, "", bot);
    let alone = partial.strip_prefix("  ").filter(|_| partial.starts_with("    <div id=\"signal-unresolved-placement\" hidden>")).map_or(partial.clone(), str::to_string);
    crate::web::turbo::stream("replace", "signal-unresolved-placement", &alone)
}

/// The settings column of a saved signal bot. `base_url`: the request's (`request.base_url`), which the webhook URL
/// starts with.
pub fn column(ctx: &Ctx, csrf: &str, bot: &Bot, hide_money: bool, base_url: &str) -> Result<String, WebError> {
    let base = bot.base_asset();
    let quote = bot.quote_asset.as_ref();
    let color = colors::pill_color(base.and_then(|asset| asset.color.as_deref()))
        .ok_or_else(|| WebError::Config(format!("bot {}: its base's colour is not a colour", bot.id)))?;
    let path = ctx.path(&format!("/bots/{}", bot.id));
    let rules = bot.signals.iter().map(|rule| RuleView {
        id: rule.id, path: format!("{path}/bot_signals/{}", rule.id), token: rule.token.clone(), sell: rule.sell, percentage: rule.percentage, enabled: rule.enabled,
        amount: amount_text(&rule.amount),
        triggered: match rule.last_triggered_at {
            Some(at) => i18n::text(ctx.locale, "bot.signal.last_triggered", &[("time", Arg::Text(&ago(at, ctx.now, ctx.locale)))]),
            None => i18n::text(ctx.locale, "bot.signal.never_triggered", &[]),
        },
    }).collect();
    Ok(Settings {
        v: ctx, csrf, path,
        base: Pill { class: colors::ticker_class(base.and_then(|a| a.category.as_deref()), base.and_then(|a| a.color.as_deref())), color,
                     symbol: base.and_then(|a| a.symbol.clone()), asset_id: base.map(|a| a.id) },
        quote: Pill { class: "ticker", color: String::new(), symbol: quote.and_then(|a| a.symbol.clone()), asset_id: quote.map(|a| a.id) },
        quote_symbol: bot.quote_symbol().unwrap_or("").to_string(),
        unresolved: unresolved(ctx, csrf, bot), rules, several: bot.signals.len() > 1, working: bot.working(), hide_money, base_url,
        api_hint: i18n::text(ctx.locale, "bot.signal.api_hint", &[("id", Arg::Text(&bot.id.to_string()))]),
    }.render()?)
}

/// NumbersHelper#format_value: below one, rounded to `max_decimals` and its trailing zeros dropped (a bare zero stays);
/// from one on, two decimals.
fn format_value(value: &Num, max_decimals: u8) -> Result<String, WebError> {
    let figure = |e: crate::figures::num::NumError| WebError::Config(format!("a figure out of range: {e:?}"));
    let abs = if value.is_negative() { Num::Int(0).sub(value).map_err(figure)? } else { value.clone() };
    if abs.lt(&Num::Int(1)).map_err(figure)? {
        let text = match value {
            Num::Int(i) => i.to_string(),
            Num::Dec(d) => d.round(i64::from(max_decimals)).map_err(figure)?.to_s_f(),
            Num::Float(f) => crate::figures::num::float_to_s(format::float_round(*f, i64::from(max_decimals))),
        };
        let trimmed = text.strip_suffix('0').map(|_| text.trim_end_matches('0').trim_end_matches('.')).unwrap_or(&text);
        Ok(if trimmed.is_empty() { "0".into() } else { trimmed.to_string() })
    } else {
        Ok(format!("{:.2}", value.to_f()))
    }
}

/// BotHelper#quoted_figure: the figure, then its quote in <small>.
fn quoted(figure: &str, quote: &str) -> String {
    if figure.trim().is_empty() || quote.trim().is_empty() { escape(figure) } else { format!("{} <small>{}</small>", escape(figure), escape(quote)) }
}

/// A figure as `<%= %>` prints it: an Integer, a BigDecimal (`to_s('F')`) or a Float.
fn shown(value: &Num) -> String {
    match value { Num::Int(i) => i.to_string(), Num::Dec(d) => d.to_s_f(), Num::Float(f) => crate::figures::num::float_to_s(*f) }
}

/// bots/dca_single_assets/_metrics.html.erb while the figures load (BotsController#show's `@bot.metrics`, the walk without
/// prices, and `loading: true`): what was invested is stated, everything priced waits. The wash-sale table is not
/// rendered: `refusal` serves no account under the rule.
pub fn loading_metrics(c: &Connection, ctx: &Ctx, bot: &Bot, hide_money: bool) -> Result<String, WebError> {
    let walked = crate::figures::pair::metrics(c, bot.id, crate::figures::at::At::from_utc(ctx.now).ok_or_else(|| WebError::Config("a time out of range".into()))?).map_err(super::fill_error)?;
    let figure = |e: crate::figures::num::NumError| WebError::Config(format!("a figure out of range: {e:?}"));
    let t = |key: &str| i18n::t(ctx.locale, key, &[]);
    let quote = bot.quote_symbol().unwrap_or("");
    let decimals = decimals(c, bot)?;
    let mut out = String::from("<div id=\"metrics\" style=\"display: flex; flex-direction: column; gap: 1rem;\">\n");
    if !hide_money {
        let item = |label: &str, value: Option<String>| format!("      <div class=\"data-grid__item\" data-controller=\"tooltip\">\n  <div class=\"label\">\n    {}\n  </div>\n  \
            <div class=\"data-grid__item__value \">\n      {}\n  </div>\n</div>\n\n", t(label),
            value.unwrap_or_else(|| "<div class=\"loader--small\" style=\"position: unset; margin: 0.5rem 0 0 auto;\"></div>".into()));
        let invested = quoted(&format_value(&walked.metrics.total_quote_amount_invested, decimals.quote.unwrap_or(8))?, quote);
        out.push_str(&format!("    <div class=\"widget data-grid data-grid--two-columns\">\n{}{}    </div>\n", item("data_labels.invested", Some(invested)), item("data_labels.portfolio_value", None)));
    }
    let columns = if hide_money { 4 } else { 6 };
    out.push_str("\n  <div id=\"assets_metrics_table\" class=\"widget widget--table\" data-controller=\"table-fit\">\n    <table class=\"table\">\n  <thead>\n    <tr>\n      \
                  <th scope=\"col\" class=\"table__logo\"></th>\n      <th scope=\"col\"></th>\n");
    if !hide_money { out.push_str(&format!("      <th scope=\"col\">{}</th>\n", t("data_labels.amount"))); }
    out.push_str(&format!("      <th scope=\"col\">{}</th>\n", t("data_labels.avg_price")));
    if !hide_money { out.push_str(&format!("      <th scope=\"col\">{}</th>\n", t("data_labels.value"))); }
    out.push_str(&format!("      <th scope=\"col\">{}</th>\n      \n    </tr>\n  </thead>\n  <tbody id=\"assets_metrics_list\">\n", t("data_labels.pnl")));
    match bot.base_asset().filter(|_| walked.metrics.total_quote_amount_invested.is_positive()) {
        Some(asset) => {
            let image: Option<String> = c.query_row("SELECT image_url FROM assets WHERE id = ?1", [asset.id], |r| r.get(0)).optional()?.flatten();
            let logo = crate::web::tracker::row::asset_logo(image, asset.color.clone(), asset.symbol().to_string(), asset.category.clone())?;
            let symbol = escape(asset.symbol());
            out.push_str(&format!("      <tr data-symbol=\"{symbol}\">\n        <td class=\"table__logo\">{logo}</td>\n        <td scope=\"row\">{symbol}</td>\n"));
            if !hide_money { out.push_str(&format!("        <td>{}</td>\n", escape(&shown(&walked.amount.round(i64::from(decimals.base.unwrap_or(8))).map_err(figure)?)))); }
            let average = walked.average.as_ref().map(|a| a.round(i64::from(decimals.price.unwrap_or(8)))).transpose().map_err(figure)?;
            out.push_str(&format!("        <td>{}</td>\n", average.map_or_else(|| crate::web::figure::NO_VALUE.to_string(), |a| quoted(&shown(&a), quote))));
            let loader = |indent: &str| format!("{indent}<div class=\"loader--small\" style=\"position: unset; float:right\"></div>\n");
            if !hide_money { out.push_str(&format!("          <td>\n{}          </td>\n", loader("              "))); }
            let class = match &walked.metrics.pnl { Some(p) if p.is_negative() => "text-danger", Some(p) if p.is_positive() => "text-success", _ => "" };
            out.push_str(&format!("        <td class=\"{class}\">\n{}        </td>\n        \n      </tr>\n", loader("            ")));
        }
        None => out.push_str(&format!("      <tr>\n        <td colspan=\"{columns}\" class=\"text-center text-inactive\" style=\"padding-bottom: 2rem;\">. . .</td>\n      </tr>\n")),
    }
    out.push_str("  </tbody>\n</table>\n\n  </div>\n\n  \n</div>\n");
    Ok(out)
}

/// Bot::AssetConfigurable#available_exchanges_for_current_settings without the bot's own venue: every venue a user can
/// pick that lists the pair, available and trading, in id order.
pub fn other_exchange_ids(c: &Connection, bot: &Bot) -> Result<Vec<i64>, WebError> {
    let id = |key: &str| bot.settings.get(key).and_then(Value::as_i64);
    let mut statement = c.prepare("SELECT DISTINCT t.exchange_id FROM tickers t JOIN exchanges e ON e.id = t.exchange_id \
                                   WHERE t.available = 1 AND t.trading_enabled = 1 AND e.available = 1 AND (?1 IS NULL OR t.quote_asset_id = ?1) \
                                   AND (?2 IS NULL OR t.base_asset_id = ?2) AND t.exchange_id != ?3 ORDER BY t.exchange_id")?;
    let ids = statement.query_map((id("quote_asset_id"), id("base_asset_id"), bot.exchange.id), |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
}

/// `bot.assets.pluck(:symbol)`: the pair's two assets in id order (`Asset.where(id: [...])`).
pub fn asset_symbols(c: &Connection, bot: &Bot) -> Result<Vec<String>, WebError> {
    let id = |key: &str| bot.settings.get(key).and_then(Value::as_i64);
    let mut statement = c.prepare("SELECT symbol FROM assets WHERE id IN (?1, ?2) ORDER BY id")?;
    let symbols = statement.query_map((id("base_asset_id"), id("quote_asset_id")), |r| r.get::<_, Option<String>>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(symbols.into_iter().map(Option::unwrap_or_default).collect())
}
