//! Bots::ExportsController and Bots::ImportsController (Bot::Exportable): the bot's closed orders as a CSV file, and such a
//! file read back into a bot. PLAN-A8-bot-csv (r0–r4) names every divergence:
//! - D-EXPORT-NO-REFRESH: no venue call; the file is the database as it stands (Rails polls the waiting rows first).
//! - D-EXPORT-NIL-CELL: a closed row without an order type or side is an empty cell (Rails raises).
//! - D-IMPORT-TIME / R-IMPORT-READABLE / D-IMPORT-UNREADABLE: a row Rails would insert is read strictly (its time, type,
//!   side and numbers) and as every reader reads it (`fill::for_engine`, 1970 ≤ time < 2200); one that fails refuses the
//!   whole file as malformed, nothing written, no exception text shown.
//! - D-IMPORT-REIMPORT / D-IMPORT-DUP: an id the bot already holds is never imported twice; the count is what was written.
//! - R-IMPORT-INDEX-NO-ASSET: an index bot's row naming no one asset is skipped (Rails stores it without one).
//! - D-IMPORT-MISMATCH-QUOTE / D-IMPORT-COUNTS: the mismatch names a mismatched row's quote; skipped rows are counted.
//! - D-IMPORT-BUSY: refused while the engine holds the bot or an operation is in flight.
//! - D-IMPORT-SIZE: the upload is bounded as the tracker's (web::IMPORT_LIMIT).
use super::write;
use crate::engine::{composition, eligibility, model};
use crate::figures::{at::At, dec::Dec, fill};
use crate::ruby::{self, BigDec};
use crate::tax::{line, Cell};
use crate::tracker::import::{csv::Table, deltabadger::unescape};
use crate::web::{auth, flash, i18n, layout::{self, Ctx}, App, WebError};
use axum::{extract::{Extension, Path, State}, http::{header, StatusCode}, response::{IntoResponse, Response}};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::LazyLock;

const HEADERS: [&str; 10] = ["Timestamp", "Order ID", "Type", "Side", "Amount", "Value", "Price", "Base Asset", "Quote Asset", "Status"];

/// `humanize.titleize` of an enum name: `limit_order` less its suffix, `buy`, `closed`.
fn title(name: &str) -> String {
    name.split('_').filter(|w| !w.is_empty()).map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

/// `TimeWithZone#to_s` in the owner's zone: `2026-09-09 15:30:01 +0200`, or `… UTC` where the zone's abbreviation is UTC/UCT.
fn timestamp(at: DateTime<Utc>, zone: &str) -> String {
    let local = crate::web::timezone::local(at, zone);
    let abbreviation = local.format("%Z").to_string();
    let offset = if abbreviation == "UTC" || abbreviation == "UCT" { "UTC".to_string() } else { local.format("%z").to_string() };
    format!("{} {offset}", local.format("%Y-%m-%d %H:%M:%S"))
}

/// The bot as `current_user.bots.find` finds it, not deleted.
struct Owned { exchange_id: i64, kind: String, settings: Value, zone: String }

fn owned(c: &Connection, user: i64, id: i64) -> Result<Option<Owned>, WebError> {
    Ok(c.query_row("SELECT b.exchange_id, coalesce(b.type, ''), b.settings, coalesce(u.time_zone, 'UTC') FROM bots b JOIN users u ON u.id = b.user_id \
                    WHERE b.id = ?1 AND b.user_id = ?2 AND b.status <> 3", (id, user), |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?))
    }).optional()?.map(|(exchange_id, kind, settings, zone)| Owned {
        exchange_id, kind, settings: settings.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null), zone,
    }))
}

/// Bot::Exportable#orders_csv.
fn orders_csv(c: &Connection, id: i64, bot: &Owned) -> Result<String, WebError> {
    let venues: BTreeMap<i64, String> = c.prepare("SELECT id, coalesce(name, '') FROM exchanges")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut out = String::new();
    line(&mut out, &HEADERS.map(|h| Cell::Text(h.into())));
    let mut s = c.prepare("SELECT created_at, external_id, order_type, side, amount, amount_exec, quote_amount, quote_amount_exec, price, base, quote, exchange_id \
                           FROM transactions WHERE bot_id = ?1 AND status = 0 AND external_status = 2 ORDER BY created_at, id")?;
    let mut rows = s.query([id])?;
    let dec = |r: &rusqlite::Row, i: usize| -> Result<Option<BigDec>, WebError> {
        ruby::from_sql(r.get_ref(i)?).map_err(|e| WebError::Config(format!("transaction decimal: {e:?}")))
    };
    let raw = |d: Option<BigDec>| d.map_or(Cell::Nil, |d| Cell::Raw(d.to_s_f()));
    let text = |t: Option<String>| t.map_or(Cell::Nil, Cell::Text);
    while let Some(r) = rows.next()? {
        let created: String = r.get(0)?;
        let at = crate::codec::parse_time(&created).map_err(|e| WebError::Config(format!("transaction time: {e:?}")))?;
        let external: Option<String> = r.get(1)?;
        let exchange: i64 = r.get(11)?;
        // venue_order_id: another venue's row names its venue.
        // A row with no id still names its venue (`"#{name}:#{nil}"`), so two such rows of different venues stay two.
        let order_id = if exchange == bot.exchange_id { external } else {
            Some(format!("{}:{}", venues.get(&exchange).cloned().unwrap_or_else(|| exchange.to_string()), external.unwrap_or_default()))
        };
        let order_type: Option<i64> = r.get(2)?;
        let side: Option<i64> = r.get(3)?;
        let (amount, amount_exec, quote_amount, quote_amount_exec, price) = (dec(r, 4)?, dec(r, 5)?, dec(r, 6)?, dec(r, 7)?, dec(r, 8)?);
        // parse_quote_amount: the executed value, the asked one, else amount × price when the price is positive.
        let value = quote_amount_exec.or(quote_amount).or_else(|| match (&amount, &price) {
            (Some(a), Some(p)) if p.is_positive() => Some(a * p),
            _ => None,
        });
        line(&mut out, &[
            Cell::Raw(timestamp(at, &bot.zone)),
            text(order_id),
            text(order_type.and_then(|t| match t { 0 => Some("Market"), 1 => Some("Limit"), _ => None }).map(str::to_string)),
            text(side.and_then(|s| match s { 0 => Some("Buy"), 1 => Some("Sell"), _ => None }).map(str::to_string)),
            raw(amount_exec.or(amount)),
            raw(value),
            raw(price),
            text(r.get::<_, Option<String>>(9)?.map(|b| b.to_uppercase())),
            text(r.get::<_, Option<String>>(10)?.map(|q| q.to_uppercase())),
            Cell::Text(title("closed")),
        ]);
    }
    Ok(out)
}

/// POST /bots/:id/export.
pub async fn export(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(super::page::not_found(&ctx)) };
    let file = app.db(move |c| match owned(c, user, id)? { Some(bot) => orders_csv(c, id, &bot).map(Some), None => Ok(None) }).await;
    match file {
        Ok(Some(body)) => Ok((StatusCode::OK, [(header::CONTENT_TYPE, "text/csv"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"orders.csv\"; filename*=UTF-8''orders.csv")], body).into_response()),
        Ok(None) => Ok(super::page::not_found(&ctx)),
        Err(e) => layout::or_refused(&ctx, e),
    }
}

/// `Time.zone.parse` as far as this build reads it (D-IMPORT-TIME): a date, optionally a time of day, optionally a zone.
static TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(concat!(
    r"^([0-9]{4})-([0-9]{1,2})-([0-9]{1,2})(?:[ T]([0-9]{1,2}):([0-9]{2})(?::([0-9]{2})(?:\.([0-9]{1,9}))?)?)?",
    r" *(Z|UTC|[+-][0-9]{2}:?[0-9]{2})?$")).unwrap());

fn read_time(cell: Option<&str>) -> Option<DateTime<Utc>> {
    let c = TIME.captures(ruby::strip(cell?))?;
    let n = |i: usize| c.get(i).map_or(Some(0), |m| m.as_str().parse::<u32>().ok());
    let date = NaiveDate::from_ymd_opt(c[1].parse().ok()?, n(2)?, n(3)?)?;
    let nanos = c.get(7).map_or(Some(0), |f| format!("{:0<9}", f.as_str()).parse::<u32>().ok())?;
    let wall = NaiveDateTime::new(date, NaiveTime::from_hms_nano_opt(n(4)?, n(5)?, n(6)?, nanos)?);
    let seconds = match c.get(8).map(|z| z.as_str()) {
        None | Some("Z" | "UTC") => 0,
        Some(z) => {
            let digits: String = z[1..].chars().filter(char::is_ascii_digit).collect();
            let s = digits[..2].parse::<i32>().ok()? * 3600 + digits[2..].parse::<i32>().ok()? * 60;
            if z.starts_with('-') { -s } else { s }
        }
    };
    let at = chrono::FixedOffset::east_opt(seconds)?.from_local_datetime(&wall).single()?.with_timezone(&Utc);
    // R-IMPORT-READABLE: decades inside the figures' range on both sides.
    let window = Utc.with_ymd_and_hms(1970, 1, 1, 0, 0, 0).unwrap()..Utc.with_ymd_and_hms(2200, 1, 1, 0, 0, 0).unwrap();
    window.contains(&at).then_some(at)
}

/// `row[name].to_d` where it holds a plain decimal; blank is 0, as Rails reads nil (D-IMPORT-UNREADABLE).
fn read_number(cell: Option<&str>) -> Option<BigDec> {
    match cell.filter(|v| !ruby::blank(v)) {
        None => Some(BigDec::zero()),
        Some(v) => crate::tracker::import::csv::amount(Some(v)).ok().map(|d| d.round(18)),
    }
}

/// { NAME => asset ids }: Bot::Exportable#import_names.
fn import_names(c: &Connection, exchange: i64, assets: &[i64]) -> Result<BTreeMap<String, BTreeSet<i64>>, WebError> {
    let mut names: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for id in assets {
        if let Some(symbol) = c.query_row("SELECT symbol FROM assets WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0)).optional()?.flatten() {
            if !symbol.trim().is_empty() { names.entry(symbol.to_uppercase()).or_default().insert(*id); }
        }
        let mut s = c.prepare("SELECT base FROM tickers WHERE exchange_id = ?1 AND base_asset_id = ?2")?;
        for base in s.query_map(params![exchange, id], |r| r.get::<_, Option<String>>(0))? {
            let base = base?.unwrap_or_default();
            let spelling = crate::figures::db::base_spelling(&base);
            if !spelling.is_empty() { names.entry(spelling.to_uppercase()).or_default().insert(*id); }
        }
    }
    Ok(names)
}

/// Bot::Exportable#venue_import_names: every name the venue lists an asset under at the bot's quote.
fn venue_import_names(c: &Connection, exchange: i64, quote: Option<i64>) -> Result<BTreeMap<String, BTreeSet<i64>>, WebError> {
    let mut names: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut s = c.prepare("SELECT t.base, a.symbol, t.base_asset_id FROM tickers t JOIN assets a ON a.id = t.base_asset_id WHERE t.exchange_id = ?1 AND t.quote_asset_id IS ?2")?;
    for row in s.query_map(params![exchange, quote], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?)))? {
        let (base, symbol, id) = row?;
        for name in [base.as_deref().map(crate::figures::db::base_spelling), symbol.as_deref()].into_iter().flatten().filter(|n| !ruby::blank(n)) {
            names.entry(name.to_uppercase()).or_default().insert(id);
        }
    }
    Ok(names)
}

/// What an import came to.
#[derive(Debug, PartialEq)]
pub enum Imported {
    Missing,
    NoFile,
    InvalidFormat,
    Malformed,
    /// D-IMPORT-BUSY or the install guard: the reason, shown through `engine.write_refused`.
    Refused(String),
    /// Nothing written and a row's quote differs from the bot's: that quote and the bot's.
    QuoteMismatch { csv: String, bot: String, skipped: usize },
    /// Rows written, and rows skipped as not closed, not this bot's or ambiguous (existing ones are Rails' sentence).
    Done { imported: usize, skipped: usize },
}

struct Candidate { order_id: String, at: DateTime<Utc>, order_type: i64, side: i64, amount: BigDec, value: BigDec, price: BigDec, base: String, asset: Option<i64> }

/// Bot::Exportable#import_orders_csv, under the write lock and the engine's guard. Call inside `App::db`.
pub fn import_orders_csv(c: &Connection, ctx: &Ctx, user: i64, id: i64, bytes: &[u8]) -> Result<Imported, WebError> {
    // The upload as Rails reads it: no byte order mark dropped (it fails the header check), invalid UTF-8 malformed.
    let Ok(text) = std::str::from_utf8(bytes) else { return Ok(Imported::Malformed) };
    let Ok(table) = Table::parse(text) else { return Ok(Imported::Malformed) };
    if table.headers.len() != HEADERS.len() || table.headers.iter().zip(HEADERS).any(|(h, e)| h.as_deref() != Some(e)) { return Ok(Imported::InvalidFormat) }
    let now = ctx.app.now();
    let tx = model::immediate(c)?;
    let Some(bot) = owned(&tx, user, id)? else { return Ok(Imported::Missing) };
    let engine_bot = model::load_bot(&tx, id)?;
    if write::engine_holds(&tx, &engine_bot)? || composition::in_flight(&tx, &engine_bot, now)? {
        return Ok(Imported::Refused(format!("bot {id} is placing an order or running an operation; try again once it is done")));
    }
    let setting = |k: &str| bot.settings.get(k).and_then(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok()));
    let quote_id = setting("quote_asset_id");
    let bot_quote = match quote_id { Some(q) => tx.query_row("SELECT symbol FROM assets WHERE id = ?1", [q], |r| r.get::<_, Option<String>>(0)).optional()?.flatten().map(|s| s.to_uppercase()), None => None };
    // importable_assets: one pair's base, a basket's members, nil for an index bot (any name).
    let members: Option<Vec<i64>> = if bot.kind == "Bots::DcaIndex" { None } else if let Some(base) = setting("base_asset_id") { Some(vec![base]) } else {
        let ids: Vec<i64> = match bot.settings.get("allocations").and_then(Value::as_object).filter(|a| !a.is_empty()) {
            Some(a) => a.keys().filter_map(|k| k.trim().parse().ok()).collect(),
            None => bot.settings.get("base_asset_ids").and_then(Value::as_array).into_iter().flatten().filter_map(|v| v.as_i64().or_else(|| v.as_str()?.trim().parse().ok())).collect(),
        };
        Some(ids)
    };
    let base_names = match &members {
        Some(ids) => import_names(&tx, bot.exchange_id, ids)?,
        None => {
            let ids: Vec<i64> = tx.prepare("SELECT DISTINCT asset_id FROM bot_index_assets WHERE bot_id = ?1 ORDER BY asset_id")?.query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
            import_names(&tx, bot.exchange_id, &ids)?
        }
    };
    let mut venue_names = None;
    let held: HashSet<String> = tx.prepare("SELECT external_id FROM transactions WHERE bot_id = ?1 AND external_id IS NOT NULL")?
        .query_map([id], |r| r.get(0))?.collect::<Result<_, _>>()?;
    let own = format!("imported_{id}_");
    let (mut skipped, mut mismatched_quote, mut seen, mut candidates) = (0usize, None::<String>, HashSet::new(), vec![]);
    for row in &table.rows {
        let get = |name: &str| table.get(row, name);
        let cell = |name: &str| get(name).map(|v| ruby::strip(unescape(v)).to_uppercase());
        let (csv_base, csv_quote) = (cell("Base Asset"), cell("Quote Asset"));
        let named = csv_base.as_ref().and_then(|b| base_names.get(b));
        if !((members.is_none() || named.is_some()) && csv_quote == bot_quote) {
            skipped += 1;
            if csv_quote != bot_quote && mismatched_quote.is_none() { mismatched_quote = Some(csv_quote.unwrap_or_default()); }
            continue;
        }
        let asset = if members.is_none() {
            let venue = match &venue_names { Some(v) => v, None => venue_names.insert(venue_import_names(&tx, bot.exchange_id, quote_id)?) };
            let key = csv_base.clone().unwrap_or_default();
            let ids: BTreeSet<i64> = base_names.get(&key).into_iter().chain(venue.get(&key)).flatten().copied().collect();
            // R-IMPORT-INDEX-NO-ASSET: a name no one asset answers to is skipped, not stored without an asset.
            if ids.len() != 1 { skipped += 1; continue; }
            ids.into_iter().next()
        } else {
            match named.filter(|ids| ids.len() == 1) { Some(ids) => ids.iter().next().copied(), None => { skipped += 1; continue; } }
        };
        let original = get("Order ID").map(unescape).unwrap_or("").to_string();
        // D-IMPORT-REIMPORT: an id this bot held before the import and that is already one of its imports is that row.
        if held.contains(&original) && original.starts_with(&own) { continue; }
        let order_id = format!("{own}{original}");
        if held.contains(&order_id) { continue; }
        let status = get("Status").map(|s| ruby::strip(unescape(s)).to_lowercase());
        if status.as_deref() != Some("closed") { skipped += 1; continue; }
        // D-IMPORT-DUP: the same id twice in the file, or one another bot holds on this venue.
        if !seen.insert(order_id.clone()) { continue; }
        let elsewhere: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM transactions WHERE exchange_id = ?1 AND external_id = ?2)", params![bot.exchange_id, order_id], |r| r.get(0))?;
        if elsewhere { continue; }
        let order_type = match get("Type").map(|t| unescape(t).to_lowercase()).as_deref() { Some("market") => 0, Some("limit") => 1, _ => return Ok(Imported::Malformed) };
        let side = match get("Side").map(|t| unescape(t).to_lowercase()).as_deref() { Some("buy") => 0, Some("sell") => 1, _ => return Ok(Imported::Malformed) };
        let (Some(at), Some(amount), Some(value), Some(price)) = (read_time(get("Timestamp")), read_number(get("Amount")), read_number(get("Value")), read_number(get("Price"))) else {
            return Ok(Imported::Malformed);
        };
        candidates.push(Candidate { order_id, at, order_type, side, amount, value, price, base: csv_base.unwrap_or_default(), asset });
    }
    if candidates.is_empty() {
        if let (Some(csv), Some(bot)) = (mismatched_quote, bot_quote.clone()) { return Ok(Imported::QuoteMismatch { csv, bot, skipped }) }
    }
    let stamp = crate::codec::format_time(now);
    let mut written = vec![];
    for row in &candidates {
        let (amount, value, price) = (ruby::to_sql(&row.amount), ruby::to_sql(&row.value), ruby::to_sql(&row.price));
        tx.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, created_at, updated_at, order_type, side, amount, amount_exec, quote_amount, \
                    quote_amount_exec, price, base, quote, base_asset_id, quote_asset_id, status, external_status) \
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?9, ?10, ?11, ?12, ?13, ?14, 0, 2)",
                   params![id, bot.exchange_id, row.order_id, crate::codec::format_time(row.at), stamp, row.order_type, row.side, amount, value, price,
                           row.base, bot_quote, row.asset, quote_id])?;
        written.push(tx.last_insert_rowid());
    }
    // R-IMPORT-READABLE: each row as stored, read as every money reader reads it.
    for &tid in &written {
        let readable = tx.query_row("SELECT created_at, price, amount, amount_exec, quote_amount_exec, side FROM transactions WHERE id = ?1", [tid], |r| {
            let d = |i: usize| Dec::from_sql(r.get_ref(i).expect("column")).ok();
            Ok((r.get::<_, String>(0)?, d(1), d(2), d(3), d(4), r.get::<_, i64>(5)?))
        })?;
        let (created, price, amount, amount_exec, quote_amount_exec, side) = readable;
        let ok = (|| -> Option<()> {
            let raw = fill::Raw::new(price?, amount?, amount_exec?, quote_amount_exec?);
            let order = crate::figures::db::Order { id: tid, at: At::from_sql(&created).ok()?, exchange_id: Some(bot.exchange_id), raw, base: None, asset_id: None,
                                                    sell: side == 1, buy: side == 0, closed: true, kind: "REGULAR".into(), abandoned_request: None };
            fill::for_engine(&order).ok().map(|_| ())
        })();
        if ok.is_none() { tx.rollback()?; return Ok(Imported::Malformed); }
    }
    if !written.is_empty() {
        if let Err(refusal) = eligibility::guard(&tx, &ctx.app.cipher, Some(id)) { tx.rollback()?; return Ok(Imported::Refused(refusal.reason())); }
    }
    tx.commit()?;
    if !written.is_empty() {
        // Bot::UpdateMetricsJob: the figures revision covers the bot's rows; an open page hears its panel.
        ctx.app.figure_service.mark(id);
        ctx.app.wake_engine();
    }
    Ok(Imported::Done { imported: written.len(), skipped })
}

/// POST /bots/:id/import.
pub async fn import(State(app): State<App>, Extension(ctx): Extension<Ctx>, Path(segment): Path<String>) -> Result<Response, WebError> {
    let Some(user) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)) };
    let Some(id) = super::id_from_path(&segment) else { return Ok(super::page::not_found(&ctx)) };
    let back = layout::redirect(StatusCode::FOUND, &ctx.path(&format!("/bots/{id}")));
    let upload = ctx.params.upload.as_ref().map(|(bytes, _)| bytes.clone());
    let view = ctx.clone();
    let result = app.db(move |c| {
        if owned(c, user, id)?.is_none() { return Ok(Imported::Missing) }
        match upload { None => Ok(Imported::NoFile), Some(bytes) => import_orders_csv(c, &view, user, id, &bytes) }
    }).await;
    let t = |key: &str, args: &[(&str, i18n::Arg)]| i18n::text(ctx.locale, &format!("bot.details.stats.{key}"), args);
    let (style, message) = match result {
        Err(e) => return layout::or_refused(&ctx, e),
        Ok(Imported::Missing) => return Ok(super::page::not_found(&ctx)),
        Ok(Imported::NoFile) => (flash::ALERT, t("import_no_file", &[])),
        Ok(Imported::InvalidFormat) => (flash::ALERT, t("import_invalid_format", &[])),
        Ok(Imported::Malformed) => (flash::ALERT, t("import_malformed_csv", &[])),
        Ok(Imported::Refused(reason)) => (flash::ALERT, i18n::text(ctx.locale, "engine.write_refused", &[("reason", i18n::Arg::Text(&reason))])),
        Ok(Imported::QuoteMismatch { csv, bot, skipped }) => (flash::ALERT, format!("{} {}",
            t("import_currency_mismatch", &[("csv_quote", i18n::Arg::Text(&csv)), ("bot_quote", i18n::Arg::Text(&bot))]),
            t("import_skipped", &[("count", i18n::Arg::Text(&skipped.to_string()))]))),
        Ok(Imported::Done { imported, skipped }) => {
            let mut text = if imported > 0 { t("import_success", &[("count", i18n::Arg::Count(imported as i64))]) } else { t("import_no_new_orders", &[]) };
            if skipped > 0 { text = format!("{text} {}", t("import_skipped", &[("count", i18n::Arg::Text(&skipped.to_string()))])); }
            (flash::NOTICE, text)
        }
    };
    flash::set(&ctx.session, style, message);
    Ok(back)
}
