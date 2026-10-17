//! app/services/bot_api: what a tool or a REST route answers, before either surface formats it. Each service returns
//! `Res` (BotApi::Result): a domain status, and the data or an error code and message. `data` is the Hash Rails builds,
//! in its key order, with its values already as Rails renders them: a BigDecimal as its `to_s` text, a time as
//! ISO 8601 in UTC with milliseconds, a settings value as stored. REST encodes it (web::rest); MCP writes text from it.
//! Ported here: the services of the REST routes this build serves (slice A); the others stay where their MCP tool is.
use super::mcp::{protocol::metadata, tools::{asset, STATUSES}};
use super::{bot::action_params::numeric, App, WebError};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

/// BotApi::Result's status symbols.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Domain { Success, ValidationFailed, NotFound }

pub struct Res {
    pub status: Domain,
    pub data: Value,
    /// Set on a failure only (`success?` is `error_code.nil?`).
    pub code: Option<&'static str>,
    pub message: String,
}

impl Res {
    fn success(data: Value) -> Self { Self { status: Domain::Success, data, code: None, message: String::new() } }
    fn failure(status: Domain, code: &'static str, message: String) -> Self { Self { status, data: Value::Null, code: Some(code), message } }
}

/// Ruby's `present?` for a parameter: nil, false, a blank string and an empty list or hash are not.
fn present(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// `value.to_i` for a parameter: a string's leading integer, a number truncated. `None` where Ruby raises
/// (true, a list, a hash). A number past 64 bits saturates, which every caller here reads the same way.
fn to_i(v: &Value) -> Option<i64> {
    match v {
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        Value::Number(n) if n.is_f64() => n.as_f64().map(|f| f.trunc() as i64),
        _ => Some(match numeric(v, true) {
            Ok(n) => n.as_i64().unwrap_or(0),
            // Past i64: Ruby keeps the Bignum; its sign is all that matters to a clamp or an id lookup.
            Err(_) => if v.as_str().is_some_and(|s| s.trim_start().starts_with('-')) { i64::MIN } else { i64::MAX },
        }),
    }
}

/// BotApi::Bots::List. `status`: the filter as given (nil or blank lists all); a number names the enum's value.
pub fn bots(c: &Connection, user: i64, status: Option<&Value>) -> Result<Res, WebError> {
    let filter: Option<String> = match status.filter(|v| present(v)) {
        None => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        // ponytail: `where(status: true)` and friends match no row; such a filter is taken as one that matches nothing.
        Some(_) => Some("\0".into()),
    };
    let mut q = c.prepare("SELECT b.type,b.label,b.settings,b.status,e.name,b.id FROM bots b LEFT JOIN exchanges e ON b.exchange_id=e.id WHERE b.user_id=?1 AND b.status != 3")?;
    let rows = q.query_map([user],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,Option<String>>(4)?,r.get::<_,i64>(5)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut bots = vec![];
    for (kind, label, settings, status, exchange, id) in rows {
        if !["Bots::DcaMultiAsset","Bots::DcaSingleAsset","Bots::DcaIndex","Bots::Signal"].contains(&kind.as_str()) { return Err(WebError::Config("unknown bot class".into())); }
        let name = usize::try_from(status).ok().and_then(|i| STATUSES.get(i)).copied().unwrap_or("");
        // `where(status:)`: an enum name, or the stored number.
        if filter.as_deref().is_some_and(|f| f != name && f.parse::<i64>().ok() != Some(status)) { continue; }
        let s: Value = serde_json::from_str(&settings).unwrap_or(Value::Null);
        let quote = asset(c, &s["quote_asset_id"])?;
        let multi = kind == "Bots::DcaMultiAsset";
        let (pair, base) = if multi {
            // base_asset_ids: the allocations' keys, else the wizard's list; base_assets: those that exist.
            let ids: Vec<Value> = match s["allocations"].as_object().filter(|a| !a.is_empty()) {
                Some(a) => a.keys().map(|k| json!(k)).collect(),
                None => s["base_asset_ids"].as_array().cloned().unwrap_or_default(),
            };
            let mut symbols = vec![];
            for id in &ids { if let Some(symbol) = asset(c, id)? { symbols.push(symbol); } }
            // base_asset: the one member of a one-asset basket.
            let base = if ids.len() == 1 { symbols.first().cloned() } else { None };
            (Some(format!("{}/{}", symbols.join("+"), quote.clone().unwrap_or_default())), base)
        } else {
            let base = if kind == "Bots::DcaIndex" { None } else { asset(c, &s["base_asset_id"])? };
            (base.as_ref().zip(quote.as_ref()).map(|(b, q)| format!("{b}/{q}")), base)
        };
        // A bot stored without a name is listed under its generated one, as Rails loads it.
        let label = match label.filter(|l| !l.trim().is_empty()) { Some(l) => l, None => super::mcp::reads::generated_label(c, user, id, &kind, &s, "en")? };
        bots.push(json!({"id": id, "label": label, "type": kind, "pair": pair, "base_asset": base, "quote_asset": quote, "exchange": exchange,
                         "status": name, "interval": s.get("interval").cloned().unwrap_or(Value::Null),
                         "quote_amount": s.get("quote_amount").cloned().unwrap_or(Value::Null)}));
    }
    Ok(Res::success(json!({"count": bots.len(), "bots": bots})))
}

/// BotApi::Exchanges::List: the exchanges of the user's trading keys.
pub fn exchanges(c: &Connection, user: i64) -> Result<Res, WebError> {
    let mut q = c.prepare("SELECT e.id,e.name,k.status FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.key_type=0")?;
    let rows = q.query_map([user], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, usize>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    let exchanges: Vec<Value> = rows.into_iter().map(|(id, name, status)| {
        let status = ["pending_validation","correct","incorrect","pending_activation"].get(status).copied().unwrap_or("");
        json!({"id": id, "name": name, "api_key_status": status})
    }).collect();
    Ok(Res::success(json!({"count": exchanges.len(), "exchanges": exchanges})))
}

/// BotApi::Transactions::List: the user's trades, newest first; `bot_id` narrows to one bot, `limit` is 20 unless
/// it is 1 to 100. `None` where Rails raises on a parameter (`true.to_i`).
pub fn transactions(c: &Connection, user: i64, bot_id: &Value, limit: &Value) -> Result<Option<Res>, WebError> {
    let bot = if present(bot_id) {
        let Some(id) = to_i(bot_id) else { return Ok(None) };
        let found = c.query_row("SELECT id FROM bots WHERE id=?1 AND user_id=?2 AND status!=3", (id, user), |r| r.get::<_, i64>(0)).optional()?;
        let Some(found) = found else { return Ok(Some(Res::failure(Domain::NotFound, "bot_not_found", "Bot not found.".into()))) };
        Some(found)
    } else { None };
    let limit = match limit { Value::Null => 20, v => { let Some(n) = to_i(v) else { return Ok(None) }; if n <= 0 { 20 } else { n.min(100) } } };
    let mut q = c.prepare("SELECT t.id,t.bot_id,t.created_at,t.side,t.status,t.base,t.quote FROM transactions t JOIN bots b ON b.id=t.bot_id WHERE b.user_id=?1 AND (?2 IS NULL OR b.id=?2) ORDER BY t.created_at DESC LIMIT ?3")?;
    let rows = q.query_map(rusqlite::params![user, bot, limit], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?,
                                                                      r.get::<_, Option<usize>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for (id, bot_id, time, side, status, base, quote) in rows {
        let [price, _, _, amount, cost] = crate::figures::fill::display_amounts(c, id)?;
        let text = |n: Option<crate::ruby::BigDec>| n.map_or(Value::Null, |n| json!(n.to_s_f()));
        let at = crate::codec::parse_time(&time).map_err(|e| WebError::Config(format!("transaction timestamp: {e:?}")))?;
        out.push(json!({"id": id, "bot_id": bot_id, "created_at": crate::ruby::iso8601_ms(at),
                        "side": side.and_then(|s| usize::try_from(s).ok()).and_then(|s| ["buy","sell"].get(s).copied()).unwrap_or(""),
                        "status": status.and_then(|s| ["submitted","failed","skipped"].get(s).copied()).unwrap_or(""),
                        "amount_exec": text(amount), "base": base, "price": text(price), "quote": quote, "quote_amount_exec": text(cost)}));
    }
    Ok(Some(Res::success(json!({"count": out.len(), "transactions": out}))))
}

/// BotApi::Tax::ListJurisdictions, from the recorded registry.
pub fn tax_jurisdictions() -> Res {
    let rows: Vec<Value> = metadata()["tax"]["jurisdictions"].as_array().into_iter().flatten()
        .map(|r| json!({"code": r["code"], "name": r["name"], "method": r["method"], "currency": r["currency"]})).collect();
    Res::success(json!({"count": rows.len(), "jurisdictions": rows}))
}

/// BotApi::Indices::List: what the wizard's picker offers (stock indices only with the Deltabadger provider), by
/// name, optionally only those available on one tradeable exchange.
pub fn indices(c: &Connection, app: &App, exchange_name: &Value) -> Result<Res, WebError> {
    let (deltabadger, configured) = super::bots::market_data(c, app)?;
    if !configured {
        return Ok(Res::failure(Domain::ValidationFailed, "market_data_not_configured",
            "Market data provider is not configured. Set up CoinGecko or Deltabadger market data in Settings first.".into()));
    }
    // Exchange.tradeable: available, not retired.
    const TRADEABLE: &str = "SELECT type, name FROM exchanges WHERE available=1 AND type NOT IN ('Exchanges::Bitmart')";
    let mut on = None;
    if present(exchange_name) {
        let name = super::mcp::tools::str_value(exchange_name);
        let found: Option<String> = c.query_row(&format!("SELECT type FROM ({TRADEABLE}) WHERE LOWER(name) = ?1 LIMIT 1"), [name.to_lowercase()], |r| r.get(0)).optional()?;
        let Some(kind) = found else { return Ok(Res::failure(Domain::NotFound, "exchange_not_found", format!("Exchange '{name}' not found."))) };
        on = Some(format!("$.\"{kind}\""));
    }
    let names: Vec<(Option<String>, String)> = c.prepare(TRADEABLE)?.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut q = c.prepare("SELECT external_id, name, source, top_coins, available_exchanges FROM indices \
                           WHERE (?1 OR source != 'deltabadger') AND (?2 IS NULL OR json_extract(available_exchanges, ?2) IS NOT NULL) ORDER BY name")?;
    let rows = q.query_map(rusqlite::params![deltabadger, on], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?,
                                                                     r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
    let mut out = vec![];
    for (id, name, source, top, available) in rows {
        let parse = |text: Option<String>| text.and_then(|t| serde_json::from_str::<Value>(&t).ok()).unwrap_or(Value::Null);
        // Array(top_coins).size: nil is none, a list its length, a hash its pairs, anything else one.
        let coins = match parse(top) { Value::Null => 0, Value::Array(a) => a.len(), Value::Object(o) => o.len(), _ => 1 };
        // The names of the tradeable exchanges among its keys (`pluck(:type, :name).to_h`: a later row wins), sorted.
        let mut exchanges: Vec<&str> = parse(available).as_object().into_iter().flatten()
            .filter_map(|(kind, _)| names.iter().rev().find(|(t, _)| t.as_deref() == Some(kind.as_str())).map(|(_, n)| n.as_str())).collect();
        exchanges.sort_unstable();
        // Index#display_name: a count-named index goes by the container's name (Bots::DcaIndex::COUNT_NAMED_INDICES).
        let name = if id.as_deref() == Some("nasdaq-100") { Some("ND100".to_string()) } else { name };
        out.push(json!({"id": id, "name": name, "source": source, "coins": coins, "exchanges": exchanges}));
    }
    Ok(Res::success(json!({"count": out.len(), "indices": out})))
}
