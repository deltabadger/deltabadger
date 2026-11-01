//! Tracker writes under the existing authenticated, CSRF-checked web pipeline.
use super::{auth, bot::action_params::ActionParams, layout::{self, Ctx}, App, WebError};
use crate::{codec, engine::{eligibility, model}};
use axum::{extract::{Extension, State}, http::{header, HeaderMap, StatusCode}, response::{IntoResponse, Response}};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Map, Value};

pub mod fx;
pub mod read;
pub mod index;
pub mod first_sync;
pub mod holdings;
pub mod populated;
pub mod record;
pub mod row;
pub mod transaction;
pub mod modal;
pub mod classifications;
pub mod import;
pub mod gap;
pub mod api;
pub mod report;

struct Prepared {
    response: Response,
    jobs: Vec<(&'static str, String)>,
}

/// What a guarded change asks for: commit (then wake the engine and these jobs), or roll back and answer as it is (a
/// refusal found inside the transaction: no guard, no commit, no wake).
pub(crate) enum Step<T> { Commit(T, Vec<(&'static str, String)>), Abort(T) }
/// A guarded write's end: done (committed or aborted), the owner gone, or the engine's guard refused it (rolled back).
pub(crate) enum Guarded<T> { Done(T), NoOwner, Refused(String) }

/// All D5 database writes share the same immediate transaction and the engine's guard (the web forms and the API alike).
/// Run inside App::db: cancellation of the HTTP awaiter cannot lose the post-commit wake.
pub(crate) fn guarded<T>(c: &Connection, app: &App, owner: i64,
    change: impl FnOnce(&Connection, i64, &str) -> Result<Step<T>, WebError>,
) -> Result<Guarded<T>, WebError> {
    let tx = model::immediate(c)?;
    let owned = tx.query_row("SELECT id FROM users WHERE id=?1", [owner], |r| r.get::<_, i64>(0)).optional()?;
    if owned.is_none() { return Ok(Guarded::NoOwner); }
    let (value, jobs) = match change(&tx, owner, &codec::format_time(app.now()))? {
        Step::Abort(value) => { tx.rollback()?; return Ok(Guarded::Done(value)); }
        Step::Commit(value, jobs) => (value, jobs),
    };
    if let Err(refusal) = eligibility::guard(&tx, &app.cipher, None) {
        tx.rollback()?;
        return Ok(Guarded::Refused(refusal.reason().to_string()));
    }
    // Rails permits valid classification rows to survive another row's validation error.
    // Database errors above instead unwind the whole transaction and propagate to the 500.
    tx.commit()?;
    app.wake_engine();
    for (name,scope) in jobs { app.wake_job(name,&scope); }
    Ok(Guarded::Done(value))
}

fn write(c: &Connection, ctx: &Ctx, owner: i64,
    change: impl FnOnce(&Connection, i64, &str) -> Result<Prepared, WebError>,
) -> Result<Response, WebError> {
    match guarded(c, &ctx.app, owner, |c, owner, now| change(c, owner, now).map(|p| Step::Commit(p.response, p.jobs)))? {
        Guarded::Done(response) => Ok(response),
        Guarded::NoOwner => Ok(StatusCode::NOT_FOUND.into_response()),
        Guarded::Refused(reason) => {
            let message = super::i18n::text(ctx.locale, "engine.write_refused", &[("reason",super::i18n::Arg::Text(&reason))]);
            Ok((StatusCode::UNPROCESSABLE_ENTITY, [(header::CONTENT_TYPE,"text/plain; charset=utf-8")], message).into_response())
        }
    }
}

fn blank(value: &Value) -> bool {
    match value { Value::Null | Value::Bool(false) => true, Value::String(s) => s.trim().is_empty(),
        Value::Array(v) => v.is_empty(), Value::Object(v) => v.is_empty(), _ => false }
}

fn save_settings(c: &Connection, owner: i64, now: &str, params: &Value) -> Result<StatusCode, WebError> {
    let stored: Option<String> = c.query_row("SELECT tracker_settings FROM users WHERE id=?1", [owner], |r| r.get(0))?;
    let was_null = stored.is_none();
    let original: Value = match stored {
        Some(text) => serde_json::from_str(&text).map_err(|_| WebError::Config("invalid stored tracker settings".into()))?,
        None => Value::Object(Map::new()),
    };
    let mut settings = original.as_object().cloned().ok_or_else(|| WebError::Config("invalid stored tracker settings".into()))?;
    for name in ["export_type", "country", "year", "report_scope"] {
        if let Some(value) = params.get(name).filter(|v| !v.is_array() && !v.is_object() && !blank(v)) {
            settings.insert(name.into(),value.clone());
        }
    }
    let updated = Value::Object(settings);
    // D-TRACKER-SETTINGS-422: a refused save answers 422, as fund_classifications does (Rails ignores update's false
    // return and still answers head :ok).
    if !super::user::validate_save(c, owner)? { return Ok(StatusCode::UNPROCESSABLE_ENTITY); }
    if was_null || updated != original {
        c.execute("UPDATE users SET tracker_settings=?1, updated_at=?2 WHERE id=?3", (updated.to_string(),now,owner))?;
    }
    Ok(StatusCode::OK)
}

fn classifications(c: &Connection, owner: i64, now: &str, params: &Value) -> Result<StatusCode, WebError> {
    let Some(rows) = params.get("classifications").and_then(Value::as_array) else { return Ok(StatusCode::OK); };
    let mut invalid = false;
    for row in rows {
        let Some(symbol) = row.get("symbol").filter(|v| !blank(v)) else { continue };
        let Some(symbol) = super::string_column::cast(symbol) else { continue };
        let kind = match row.get("kind").and_then(Value::as_str) {
            Some("share") => 0, Some("fund") => 1, Some("other_security") => 2, _ => continue,
        };
        let category = if kind == 1 {
            ["equity_fund","mixed_fund","real_estate_fund","foreign_real_estate_fund","other_fund"].iter()
                .position(|name| Some(*name) == row.get("fund_category").and_then(Value::as_str)).map(|n| n as i64)
        } else { None };
        let old: Option<(i64, i64, Option<i64>)> = c.query_row("SELECT id,kind,fund_category FROM fund_classifications WHERE user_id=?1 AND symbol=?2",
            (owner,&symbol), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if !classifications::validate_save(c, owner, old.map(|r| r.0), &symbol, kind, category)? { invalid = true; continue; }
        match old {
            Some((_,old_kind,old_category)) if (old_kind,old_category) == (kind,category) => {},
            Some(_) => { c.execute("UPDATE fund_classifications SET kind=?1, fund_category=?2, updated_at=?3 WHERE user_id=?4 AND symbol=?5",
                (kind,category,now,owner,&symbol))?; },
            None => { c.execute("INSERT INTO fund_classifications(user_id,symbol,kind,fund_category,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?5)",
                (owner,&symbol,kind,category,now))?; },
        }
    }
    Ok(if invalid { StatusCode::UNPROCESSABLE_ENTITY } else { StatusCode::OK })
}

async fn mutate(app: App, ctx: Ctx, funds: bool, headers: HeaderMap) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)); };
    let params = match ActionParams::parse_rails(&ctx.params) { Ok(p) => p, Err(e) => return Ok(e.status().into_response()) };
    let stream = headers.get(header::ACCEPT).and_then(|h| h.to_str().ok()).is_some_and(|s| s.contains("text/vnd.turbo-stream.html"));
    app.db(move |c| write(c,&ctx,owner,|c,owner,now| {
        let status = if funds { classifications(c,owner,now,params.value())? } else { save_settings(c,owner,now,params.value())? };
        Ok(Prepared { response:(status, [(header::CONTENT_TYPE,if stream { "text/vnd.turbo-stream.html" } else { "text/html" })], "").into_response(), jobs:vec![] })
    })).await
}

pub async fn save_export_settings(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    mutate(app,ctx,false,headers).await
}
pub async fn fund_classifications(State(app): State<App>, Extension(ctx): Extension<Ctx>, headers: HeaderMap) -> Result<Response, WebError> {
    mutate(app,ctx,true,headers).await
}

/// What a sync reads (TrackerController#sync, BotApi::Tracker::Sync): refused when a reading key is on a venue whose ledger
/// this build does not read (its venues' names); else the venues and the two jobs per reading key.
pub(crate) enum SyncPlan { Unsupported(Vec<String>), Keys { names: Vec<String>, jobs: Vec<(&'static str, String)> } }

pub(crate) fn sync_plan(c: &Connection, owner: i64) -> Result<SyncPlan, WebError> {
    // This build syncs the ledger of the venues whose registry row has one (`sync::ledger_venues`) only. Never announce
    // a sync for a venue whose sync implementation is not available.
    let venues=crate::sync::ledger_venues();
    let mut statement = c.prepare(&format!("SELECT e.name FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.user_id=?1 AND k.status=1 AND k.key_type<>1 AND e.type NOT IN ({}) ORDER BY k.id",
        vec!["?";venues.len()].join(",")))?;
    let mut unsupported: Vec<String> = vec![];
    for name in statement.query_map(rusqlite::params_from_iter(std::iter::once(rusqlite::types::Value::Integer(owner)).chain(venues.iter().map(|v|rusqlite::types::Value::Text(v.to_string())))),|r|r.get::<_,Option<String>>(0))? {
        let name = name?.unwrap_or_default();
        if !unsupported.contains(&name) { unsupported.push(name); }
    }
    if !unsupported.is_empty() { return Ok(SyncPlan::Unsupported(unsupported)); }
    let keys = crate::sync::ledger_reading_keys(c).map_err(|_| WebError::Config("tracker reading keys unavailable".into()))?;
    let mut names = vec![];
    let mut jobs = vec![];
    for key in keys {
        let name: Option<String> = c.query_row("SELECT e.name FROM api_keys k JOIN exchanges e ON e.id=k.exchange_id WHERE k.id=?1 AND k.user_id=?2",(key,owner),|r|r.get(0)).optional()?;
        let Some(name) = name else { continue };
        names.push(name);
        for job in [crate::sync::jobs::LEDGER_SYNC,crate::sync::jobs::BALANCE_SYNC] { jobs.push((job,key.to_string())); }
    }
    Ok(SyncPlan::Keys { names, jobs })
}

pub async fn sync(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)); };
    app.db(move |c| write(c,&ctx,owner,|c,owner,_| {
        let (names, jobs) = match sync_plan(c, owner)? {
            SyncPlan::Unsupported(_) => return Ok(Prepared { response:layout::refused(&ctx,"tracker sync for a venue other than Alpaca, Kraken, Binance, Binance.US, KuCoin, Bitvavo, Gemini, Bitget, MEXC, Hyperliquid, Bybit, Bitrue, Coinbase and BingX"),jobs:vec![] }),
            SyncPlan::Keys { names, jobs } => (names, jobs),
        };
        if names.is_empty() { return Ok(Prepared { response:StatusCode::NO_CONTENT.into_response(),jobs }); }
        let text = super::i18n::t(ctx.locale,"tracker.importing_from",&[("exchange",super::i18n::Arg::Text(&names.join(", ")))]);
        let content = format!("  <div id=\"sync-progress\" class=\"flash__message salert salert--syncing\">\n    <div class=\"loader--syncing\"></div>\n    {text}\n  </div>\n");
        Ok(Prepared { response:([(header::CONTENT_TYPE,super::turbo::CONTENT_TYPE)],super::turbo::stream("append","flash",&content)).into_response(),jobs })
    })).await
}

/// TrackerController#export. Rails saves `export_type` before it reads, so this GET writes: the same write as
/// save_export_settings, committed in its own immediate transaction before the read, which runs with writes disabled.
pub async fn export(State(app): State<App>, Extension(ctx): Extension<Ctx>) -> Result<Response, WebError> {
    let Some(owner) = ctx.user().map(|u| u.id) else { return Ok(auth::unauthenticated(&ctx)); };
    app.db(move |c| {
        let saved = write(c, &ctx, owner, |c, owner, now| {
            save_settings(c, owner, now, &serde_json::json!({ "export_type": "transactions" }))?;
            Ok(Prepared { response: StatusCode::OK.into_response(), jobs: vec![] })
        })?;
        if saved.status() != StatusCode::OK { return Ok(saved); }
        read::only(c, |c| transactions_csv(c, &ctx, owner))
    }).await
}

/// AccountTransaction.csv_headers: the export's columns, and the columns the Deltabadger import reads back.
pub const CSV_HEADERS: [&str; 12] = ["date","type","base_currency","base_amount","quote_currency","quote_amount","fee_currency","fee_amount","exchange","tx_id","group_id","description"];

/// `filtered_transactions` (exchange, then the UTC days `from`..`to`) through `AccountTransaction.to_csv`.
fn transactions_csv(c: &Connection, ctx: &Ctx, owner: i64) -> Result<Response, WebError> {
    use rusqlite::types::Value as Sql;
    if ctx.params.query.iter().any(|(k, _)| k.starts_with("exchange_id[") || k.starts_with("from[") || k.starts_with("to[")) {
        return Ok(layout::refused(ctx, "tracker structured filters"));
    }
    let present = |key: &str| ctx.params.query(key).filter(|v| !crate::ruby::blank(v));
    // Rails' own query, so that rows at the same instant come out in the same order.
    let mut sql = format!("SELECT {CSV_COLUMNS} FROM account_transactions WHERE user_id=?1");
    let mut binds = vec![Sql::Integer(owner)];
    if let Some(raw) = present("exchange_id") {
        let Some(id) = super::bot::id_from_path(raw) else { return Ok(layout::missing()) };
        if !c.query_row("SELECT EXISTS(SELECT 1 FROM exchanges WHERE id=?1)", [id], |r| r.get::<_, bool>(0))? { return Ok(layout::missing()); }
        binds.push(Sql::Integer(id));
        sql.push_str(&format!(" AND exchange_id=?{}", binds.len()));
    }
    // beginning_of_day and end_of_day in UTC, quoted as Rails quotes a time for SQLite.
    for (key, op, clock) in [("from", ">=", "00:00:00"), ("to", "<=", "23:59:59.999999")] {
        let Some(raw) = present(key) else { continue };
        let Some(day) = first_sync::iso_day(raw)? else { return Ok(layout::refused(ctx, "tracker date syntax")) };
        binds.push(Sql::Text(format!("{day} {clock}")));
        sql.push_str(&format!(" AND transacted_at{op}?{}", binds.len()));
    }
    sql.push_str(" ORDER BY transacted_at ASC");
    let out = csv_rows(c, &sql, binds)?;
    let name = format!("deltabadger-transactions-{}.csv", ctx.now.date_naive());
    Ok(([(header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
         (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}\"; filename*=UTF-8''{name}")),
         (header::HeaderName::from_static("content-transfer-encoding"), "binary".to_string())], out).into_response())
}

/// The columns `csv_rows` reads, in its order.
pub(crate) const CSV_COLUMNS: &str = "transacted_at,entry_type,base_currency,base_amount,quote_currency,quote_amount,fee_currency,fee_amount,exchange_id,tx_id,group_id,description";

/// AccountTransaction.to_csv over the rows `sql` selects (CSV_COLUMNS, in the order the query gives): the web export's
/// and the API's one writer.
pub(crate) fn csv_rows(c: &Connection, sql: &str, binds: Vec<rusqlite::types::Value>) -> Result<String, WebError> {
    use crate::tax::{line, Cell};
    let mut out = String::new();
    line(&mut out, &CSV_HEADERS.map(|h| Cell::Text(h.into())));
    let mut venues: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
    let mut statement = c.prepare(sql)?;
    let mut rows = statement.query(rusqlite::params_from_iter(binds))?;
    while let Some(r) = rows.next()? {
        let text = |i: usize| -> Result<Cell, WebError> { Ok(r.get::<_, Option<String>>(i)?.map_or(Cell::Nil, Cell::Text)) };
        let amount = |i: usize| -> Result<Cell, WebError> { Ok(crate::ruby::from_sql(r.get_ref(i)?).map_err(|_| row::invalid())?.map_or(Cell::Nil, |d| Cell::Raw(d.to_s_f()))) };
        let at = codec::parse_time(&r.get::<_, String>(0)?).map_err(|_| row::invalid())?;
        let kind = r.get::<_, i64>(1)?;
        let exchange = r.get::<_, i64>(8)?;
        if let std::collections::hash_map::Entry::Vacant(slot) = venues.entry(exchange) {
            let kind: Option<Option<String>> = c.query_row("SELECT type FROM exchanges WHERE id=?1", [exchange], |r| r.get(0)).optional()?;
            // A row whose venue is gone: Rails fails on nil.name_id.
            let kind = kind.ok_or_else(row::invalid)?;
            slot.insert(crate::tracker::rows::name_id(kind.as_deref().unwrap_or("Exchange")));
        }
        line(&mut out, &[Cell::Text(at.format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            usize::try_from(kind).ok().and_then(|k| row::TYPES.get(k)).map_or(Cell::Nil, |k| Cell::Text((*k).into())),
            text(2)?, amount(3)?, text(4)?, amount(5)?, text(6)?, amount(7)?, Cell::Text(venues[&exchange].clone()), text(9)?, text(10)?, text(11)?]);
    }
    Ok(out)
}
