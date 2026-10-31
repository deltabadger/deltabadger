//! PLAN-A11: the tracker through REST and MCP. BotApi::Transactions::{ListAccount, ExportCsv} and
//! BotApi::Tracker::{SetTransferLink, SetTransactionPrice, Sync}, one service each for both surfaces, on the web's own
//! pieces: its CSV writer, candidate query, link and price stores, sync plan and guarded write (`super::guarded`).
//! The tools' texts are the Rails tools'. Named divergences: specs/PLAN-A11-tracker-api.md.
use super::{csv_rows, first_sync, guarded, read, row::Row, sync_plan, transaction, Guarded, Step, SyncPlan, CSV_COLUMNS};
use crate::{codec, ruby::{self, BigDec}, web::{bot_api::{self, Domain, Res}, bot::mcp_input, mcp::{protocol::tool_text, read_limits, tools::str_value}, timezone, App, WebError}};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use rusqlite::{types::Value as Sql, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// The MCP tools served here (`list_indices` reads REST's `bot_api::indices`).
pub const NAMES: [&str; 6] = ["list_account_transactions", "export_transactions_csv", "set_transfer_link", "set_transaction_price", "sync_tracker", "list_indices"];
/// The REST routes served here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route { List, Export, Link, Price, Sync }

/// AccountTransaction.entry_types, in enum order.
const ENTRY_TYPES: [&str; 17] = ["buy", "sell", "swap_in", "swap_out", "deposit", "withdrawal", "staking_reward", "lending_interest", "airdrop", "mining",
                                 "fee", "other_income", "lost", "withholding_tax", "return_of_capital", "adjustment", "unsupported_activity"];
/// D-API-ENTRY-TYPE.
pub const INVALID_ENTRY_TYPE: &str = "entry_type must be one of: buy, sell, swap_in, swap_out, deposit, withdrawal, staking_reward, lending_interest, airdrop, mining, fee, other_income, lost, withholding_tax, return_of_capital, adjustment, unsupported_activity.";
/// D-LINK-KIND.
pub const NOT_TRANSFERABLE: &str = "Only a deposit or a withdrawal can be linked as a transfer.";
/// D-EXPORT-SIZE.
pub const EXPORT_TOO_LARGE: &str = "The export is larger than 4 MB. Use date filters to narrow the scope.";
/// D-SYNC-VENUE: the venues a reading key is on whose ledger this build does not read.
pub fn sync_unsupported(venues: &[&str]) -> String { format!("This build does not sync {} yet.", venues.join(", ")) }
/// ExportCsv::MAX_ROWS.
pub const MAX_ROWS: i64 = 5000;
const INVALID_DATE: &str = "Invalid date format. Use YYYY-MM-DD. (invalid date)";

fn failure(status: Domain, code: &'static str, message: &str) -> Res { Res::failure(status, code, message.into()) }
fn not_found() -> Res { failure(Domain::NotFound, "transaction_not_found", "Transaction not found.") }

/// The CSV a REST export serves (the one non-JSON answer) and its counts.
pub struct Csv { pub csv: String, pub total: i64, pub returned: i64, pub truncated: bool }
/// A REST answer: the envelope, the envelope with sync's 202, or the CSV.
pub enum Out { Res(Res), Accepted(Res), Csv(Csv) }

// ---- filters ----------------------------------------------------------------------------------------------------------

/// The WHERE clauses after `user_id=?1`, and their binds (the user's first).
struct Filter { sql: String, binds: Vec<Sql> }

impl Filter {
    fn push(&mut self, clause: &str, bind: Sql) { self.binds.push(bind); self.sql.push_str(&format!(" AND {clause}?{}", self.binds.len())); }
}

/// `Date.parse(text).beginning_of_day` / `.end_of_day` in `zone`, D-API-DATE-STRICT: `YYYY-MM-DD` only, every other text
/// (and a well-formed day that does not exist) is `invalid_date`.
fn day(v: &Value) -> Result<Option<NaiveDate>, ()> {
    if !bot_api::present(v) { return Ok(None); }
    let text = match v { Value::String(s) => s.clone(), Value::Number(n) => n.to_string(), _ => return Err(()) };
    match first_sync::iso_day(&text) { Ok(Some(day)) => Ok(Some(day)), _ => Err(()) }
}

/// A day's bound in `zone` as Date#beginning_of_day / #end_of_day resolve it (TimeZone#period_for_local(t, dst = true) {
/// periods.last }): a repeated wall time is the occurrence in DST when exactly one is, else the later one (Magadan
/// 2014-10-26 00:00 is +10:00); a missing one moves forward an hour at a time (Santiago 2026-09-06 00:00 is 01:00).
fn wall(zone: Tz, naive: chrono::NaiveDateTime) -> Option<DateTime<Utc>> {
    use chrono::{LocalResult, TimeZone};
    use chrono_tz::OffsetComponents;
    let dst = |t: &DateTime<Tz>| !t.offset().dst_offset().is_zero();
    let mut at = naive;
    for _ in 0..48 {
        match zone.from_local_datetime(&at) {
            LocalResult::Single(t) => return Some(t.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => return Some(if dst(&first) && !dst(&second) { first } else { second }.with_timezone(&Utc)),
            LocalResult::None => at = at.checked_add_signed(chrono::Duration::hours(1))?,
        }
    }
    None
}


/// ListAccount's and ExportCsv's shared checks, in Rails' order: the exchange (any venue row), then the dates.
/// D-API-NONSCALAR: an `exchange_id` that is no scalar is an unknown exchange (Rails raises on Array#to_i).
fn filter(c: &Connection, user: i64, args: &Map<String, Value>, zone: Tz) -> Result<Result<Filter, Res>, WebError> {
    let none = Value::Null;
    let arg = |name: &str| args.get(name).unwrap_or(&none);
    let mut out = Filter { sql: String::new(), binds: vec![Sql::Integer(user)] };
    if bot_api::present(arg("exchange_id")) {
        let found = match bot_api::to_i(arg("exchange_id")) {
            Some(id) => c.query_row("SELECT id FROM exchanges WHERE id=?1", [id], |r| r.get::<_, i64>(0)).optional()?,
            None => None,
        };
        let Some(id) = found else { return Ok(Err(failure(Domain::NotFound, "exchange_not_found", "Exchange not found. Use 'list_exchanges' to see available exchanges."))) };
        out.push("exchange_id=", Sql::Integer(id));
    }
    let (Ok(from), Ok(to)) = (day(arg("from_date")), day(arg("to_date"))) else { return Ok(Err(failure(Domain::ValidationFailed, "invalid_date", INVALID_DATE))) };
    let bound = |day: NaiveDate, clock: (u32, u32, u32, u32)| -> Result<Sql, WebError> {
        let naive = day.and_hms_micro_opt(clock.0, clock.1, clock.2, clock.3).ok_or_else(super::row::invalid)?;
        Ok(Sql::Text(codec::format_time(wall(zone, naive).ok_or_else(super::row::invalid)?)))
    };
    if let Some(day) = from { out.push("transacted_at>=", bound(day, (0, 0, 0, 0))?); }
    if let Some(day) = to { out.push("transacted_at<=", bound(day, (23, 59, 59, 999_999))?); }
    Ok(Ok(out))
}

// ---- list -------------------------------------------------------------------------------------------------------------

/// BotApi::Transactions::ListAccount: its rows, and the MCP tool's lines (the dates in `zone`).
fn list(c: &Connection, user: i64, args: &Map<String, Value>, zone: Tz) -> Result<(Res, String), WebError> {
    read::only(c, |c| {
        let mut f = match filter(c, user, args, zone)? { Ok(f) => f, Err(res) => { let text = res.message.clone(); return Ok((res, text)) } };
        let entry = args.get("entry_type").unwrap_or(&Value::Null);
        if bot_api::present(entry) {
            let Some(kind) = entry.as_str().and_then(|name| ENTRY_TYPES.iter().position(|t| *t == name)) else {
                return Ok((failure(Domain::ValidationFailed, "invalid_entry_type", INVALID_ENTRY_TYPE), INVALID_ENTRY_TYPE.into()));
            };
            f.push("entry_type=", Sql::Integer(kind as i64));
        }
        // `raw&.to_i || 50`, ≤ 0 → 50, at most 200; D-API-NONSCALAR: a value that is no scalar is the default.
        let limit = match args.get("limit") { None | Some(Value::Null) => 50, Some(v) => bot_api::to_i(v).filter(|n| *n > 0).map_or(50, |n| n.min(200)) };
        f.binds.push(Sql::Integer(limit));
        // Rails' own query (by_date, then the filters), so that rows at one instant come out in the same order.
        let sql = format!("SELECT * FROM account_transactions WHERE user_id=?1{} ORDER BY transacted_at DESC LIMIT ?{}", f.sql, f.binds.len());
        let mut statement = c.prepare(&sql)?;
        let mut rows = statement.query(rusqlite::params_from_iter(f.binds))?;
        let (mut out, mut lines) = (vec![], vec![]);
        while let Some(r) = rows.next()? {
            let id: i64 = r.get("id")?;
            let decimal = |name: &str| -> Result<Value, WebError> { Ok(ruby::from_sql(r.get_ref(name)?).map_err(|_| super::row::invalid())?.map_or(Value::Null, |d| json!(d.to_s_f()))) };
            let at = codec::parse_time(&r.get::<_, String>("transacted_at")?).map_err(|_| super::row::invalid())?;
            let kind: i64 = r.get("entry_type")?;
            let kind = usize::try_from(kind).ok().and_then(|k| ENTRY_TYPES.get(k)).copied().unwrap_or("");
            let linked: Option<i64> = r.get("linked_transaction_id")?;
            let inverse: Option<i64> = c.query_row("SELECT id FROM account_transactions WHERE linked_transaction_id=?1", [id], |r| r.get(0)).optional()?;
            let exchange: Option<String> = c.query_row("SELECT name FROM exchanges WHERE id=?1", [r.get::<_, i64>("exchange_id")?], |r| r.get(0)).optional()?.flatten();
            let manual: Option<String> = r.get("manual_values")?;
            let stated = stated_price(manual.as_deref())?;
            let venue_valued = Row::load(c, user, id)?.ok_or_else(super::row::invalid)?.counterpart(c)?.is_some();
            let row = json!({"id": id, "transacted_at": ruby::iso8601_ms(at), "entry_type": kind, "base_amount": decimal("base_amount")?,
                "base_currency": r.get::<_, Option<String>>("base_currency")?, "quote_amount": decimal("quote_amount")?, "quote_currency": r.get::<_, Option<String>>("quote_currency")?,
                "fee_amount": decimal("fee_amount")?, "fee_currency": r.get::<_, Option<String>>("fee_currency")?, "exchange": exchange,
                "linked": linked.is_some() || inverse.is_some(), "linked_transaction_id": if kind == "withdrawal" { linked } else { inverse },
                "stated_price_usd": stated, "venue_valued": venue_valued});
            lines.push(line(&row, at.with_timezone(&zone)));
            out.push(row);
        }
        let text = if out.is_empty() { "No account transactions found.".to_string() } else { format!("Account transactions ({}):\n{}", out.len(), lines.join("\n")) };
        Ok((Res::success(json!({"count": out.len(), "transactions": out})), text))
    })
}

/// `manual_value(:price)&.to_s('F')`: the stored price when present.
fn stated_price(manual: Option<&str>) -> Result<Value, WebError> {
    let Some(text) = manual else { return Ok(Value::Null) };
    let manual: Value = serde_json::from_str(text).map_err(|_| super::row::invalid())?;
    let price = &manual["price"];
    if !bot_api::present(price) { return Ok(Value::Null); }
    let text = match price { Value::String(s) => s.clone(), Value::Number(n) => n.to_string(), _ => return Err(super::row::invalid()) };
    Ok(json!(BigDec::parse(&text).map_err(|_| super::row::invalid())?.to_s_f()))
}

/// ListAccountTransactionsTool#present's line.
fn line(row: &Value, at: DateTime<Tz>) -> String {
    let text = |v: &Value| str_value(v);
    let quote = if row["quote_amount"].is_null() { String::new() } else { format!(" / {} {}", text(&row["quote_amount"]), text(&row["quote_currency"])) };
    let fee = if row["fee_amount"].is_null() { String::new() } else { format!(" | Fee: {} {}", text(&row["fee_amount"]), text(&row["fee_currency"])) };
    let link = if row["linked"] == true { format!(" | transfer ↔ #{}", text(&row["linked_transaction_id"])) } else { String::new() };
    let stated = if row["stated_price_usd"].is_null() { String::new() } else { format!(" | stated price {} USD", text(&row["stated_price_usd"])) };
    let exchange = if row["exchange"].is_null() { "N/A".to_string() } else { text(&row["exchange"]) };
    format!("- #{} [{}] {} {} {}{quote}{fee} | {exchange}{link}{stated}", row["id"], at.format("%Y-%m-%d %H:%M"), text(&row["entry_type"]).to_uppercase(),
            text(&row["base_amount"]), text(&row["base_currency"]))
}

// ---- export -----------------------------------------------------------------------------------------------------------

/// BotApi::Transactions::ExportCsv: the newest MAX_ROWS rows (Rails' `order(desc)` then `to_csv`'s appended `order(asc)`),
/// through the web export's writer. D-EXPORT-SIZE: past read_limits::STORED_BYTES, refused.
fn export(c: &Connection, user: i64, args: &Map<String, Value>, zone: Tz) -> Result<Result<Csv, Res>, WebError> {
    read::only(c, |c| {
        let mut f = match filter(c, user, args, zone)? { Ok(f) => f, Err(res) => return Ok(Err(res)) };
        let total: i64 = c.query_row(&format!("SELECT count(*) FROM account_transactions WHERE user_id=?1{}", f.sql), rusqlite::params_from_iter(f.binds.iter()), |r| r.get(0))?;
        if total == 0 { return Ok(Err(failure(Domain::NotFound, "no_transactions", "No account transactions found matching the filters."))); }
        f.binds.push(Sql::Integer(MAX_ROWS));
        let sql = format!("SELECT {CSV_COLUMNS} FROM account_transactions WHERE user_id=?1{} ORDER BY transacted_at DESC, transacted_at ASC LIMIT ?{}", f.sql, f.binds.len());
        let csv = csv_rows(c, &sql, f.binds)?;
        if csv.len() > read_limits::STORED_BYTES { return Ok(Err(failure(Domain::ValidationFailed, "export_too_large", EXPORT_TOO_LARGE))); }
        Ok(Ok(Csv { csv, total, returned: total.min(MAX_ROWS), truncated: total > MAX_ROWS }))
    })
}

// ---- writes -----------------------------------------------------------------------------------------------------------

/// A guarded write's answer: the engine guard's refusal is this build's 409 `write_refused`, as the bot writes'.
fn written(out: Guarded<Res>) -> Res {
    match out {
        Guarded::Done(res) => res,
        Guarded::NoOwner => not_found(),
        Guarded::Refused(reason) => Res::failure(Domain::Conflict, "write_refused", bot_api::refused(&reason)),
    }
}

/// BotApi::Boolean.parse: only true, false, 'true' and 'false'.
fn boolean(v: Option<&Value>) -> Option<bool> {
    match v? { Value::Bool(b) => Some(*b), Value::String(s) if s == "true" => Some(true), Value::String(s) if s == "false" => Some(false), _ => None }
}

/// BotApi::Tracker::SetTransferLink, after `linked` was read. The window is 14 calendar days in `zone` (r1, r3).
fn link(c: &Connection, app: &App, user: i64, id: i64, linked: bool, zone: Tz) -> Result<Res, WebError> {
    Ok(written(guarded(c, app, user, |c, owner, now| {
        let Some(row) = Row::load(c, owner, id)? else { return Ok(Step::Abort(not_found())) };
        let (withdrawal, deposit) = if linked {
            if row.linked() { return Ok(Step::Abort(failure(Domain::Conflict, "already_linked", "This transaction is already linked to a transfer."))); }
            // D-LINK-KIND.
            if !matches!(row.kind, 4 | 5) { return Ok(Step::Abort(failure(Domain::ValidationFailed, "not_transferable", NOT_TRANSFERABLE))); }
            let is_withdrawal = row.kind == 5;
            let edge = timezone::advance_days(row.at, zone, if is_withdrawal { 14 } else { -14 }).ok_or_else(super::row::invalid)?;
            let (from, to) = if is_withdrawal { (row.at, edge) } else { (edge, row.at) };
            match transaction::candidates(c, owner, &row, from, to)?.as_slice() {
                [] => return Ok(Step::Abort(failure(Domain::NotFound, "no_transfer_candidate", "No matching transfer within 14 days."))),
                [one] => if is_withdrawal { (row.id, *one) } else { (*one, row.id) },
                _ => return Ok(Step::Abort(failure(Domain::Conflict, "ambiguous_transfer_candidate", "More than one transfer matches; link it from the tracker."))),
            }
        } else {
            match (row.linked, row.inverse) {
                (Some(deposit), _) => (row.id, deposit),
                (_, Some(withdrawal)) => (withdrawal, row.id),
                _ => return Ok(Step::Abort(failure(Domain::Conflict, "not_linked", "This transaction is not linked to a transfer."))),
            }
        };
        transaction::set_link(c, owner, now, withdrawal, deposit, linked)?;
        let jobs = vec![(crate::tracker::jobs::PORTFOLIO_BACKFILL, owner.to_string()), (crate::tracker::jobs::TRACKER_LEDGER, owner.to_string())];
        Ok(Step::Commit(Res::success(json!({"withdrawal_id": withdrawal, "deposit_id": deposit, "linked": linked})), jobs))
    })?))
}

/// BotApi::Tracker::SetTransactionPrice. Validated as BotApi::Number reads the value (a Float written `to_d.to_s('F')`);
/// stored as `parse_manual` reads it (`value.to_s.strip`: a Float as Ruby writes it), r1 item 2. D-PRICE-NONTEXT: a value
/// that is neither null, text nor a number is `invalid_price` (Rails clears on false, [] and {}).
fn price(c: &Connection, app: &App, user: i64, id: i64, value: &Value) -> Result<Res, WebError> {
    let invalid = || failure(Domain::ValidationFailed, "invalid_price", "price_usd must be a non-negative number, or empty to clear.");
    // None: clear; Some(Ok(text)): the text to store once validated; Some(Err): not a value.
    let given: Option<Result<String, ()>> = match value {
        Value::Null => None,
        Value::String(_) if !mcp_input::present(value) => None,
        Value::String(s) => Some(Ok(mcp_input::strip(s).to_string())),
        Value::Number(n) => Some(Ok(match n.as_f64().filter(|_| n.is_f64()) { Some(f) => crate::web::format::float_to_s(f), None => n.to_string() })),
        _ => Some(Err(())),
    };
    Ok(written(guarded(c, app, user, |c, owner, now| {
        let Some(row) = Row::load(c, owner, id)? else { return Ok(Step::Abort(not_found())) };
        if given.is_some() && row.counterpart(c)?.is_some() { return Ok(Step::Abort(failure(Domain::Conflict, "venue_valued", "The venue priced this row; it cannot be restated."))); }
        let stored = match &given {
            None => None,
            Some(Err(())) => return Ok(Step::Abort(invalid())),
            Some(Ok(text)) => {
                if mcp_input::number(value).is_none() { return Ok(Step::Abort(invalid())); }
                match BigDec::parse(text) { Ok(d) if d.is_positive() || d.is_zero() => Some(d.to_s_f()), _ => return Ok(Step::Abort(invalid())) }
            }
        };
        transaction::store_price(c, owner, now, &row, stored.clone())?;
        Ok(Step::Commit(Res::success(json!({"id": id, "price_usd": stored})), vec![(crate::tracker::jobs::TRACKER_LEDGER, owner.to_string())]))
    })?))
}

/// BotApi::Tracker::Sync: the request only wakes the ledger and balance syncs (no venue call), after the same lock and
/// guard as every tracker write. D-SYNC-VENUE, D-SYNC-WAKES.
fn sync(c: &Connection, app: &App, user: i64) -> Result<Res, WebError> {
    Ok(written(guarded(c, app, user, |c, owner, _| Ok(match sync_plan(c, owner)? {
        SyncPlan::Unsupported(venues) => Step::Abort(Res::failure(Domain::NotPorted, "not_ported", sync_unsupported(&venues.iter().map(String::as_str).collect::<Vec<_>>()))),
        SyncPlan::Keys { names, .. } if names.is_empty() => Step::Abort(failure(Domain::ValidationFailed, "no_reading_keys", "No exchange key can read history. Add one in Settings.")),
        SyncPlan::Keys { names, jobs } => Step::Commit(Res::success(json!({"exchanges": names})), jobs),
    }))?))
}

// ---- surfaces ---------------------------------------------------------------------------------------------------------

/// Api::V1::TransactionsController and TrackerController past their gates: `params` as Rails merges them (sync reads none).
/// REST runs in UTC (no Time.zone is set for it).
pub fn rest(c: &Connection, app: &App, user: i64, route: Route, params: &Map<String, Value>) -> Result<Out, WebError> {
    let id = || params.get("id").and_then(bot_api::to_i).unwrap_or(0);
    Ok(match route {
        Route::List => Out::Res(list(c, user, params, Tz::UTC)?.0),
        Route::Export => match export(c, user, params, Tz::UTC)? { Ok(csv) => Out::Csv(csv), Err(res) => Out::Res(res) },
        Route::Link => Out::Res(match boolean(params.get("linked")) {
            None => failure(Domain::ValidationFailed, "linked_required", "linked must be true or false."),
            Some(linked) => link(c, app, user, id(), linked, Tz::UTC)?,
        }),
        // `params.key?(:price_usd)`: null and "" clear, an absent key is a mistake.
        Route::Price => Out::Res(match params.get("price_usd") {
            None => failure(Domain::ValidationFailed, "price_usd_required", "price_usd is required (null or \"\" clears it)."),
            Some(value) => price(c, app, user, id(), value)?,
        }),
        Route::Sync => { let res = sync(c, app, user)?; if res.code.is_none() { Out::Accepted(res) } else { Out::Res(res) } }
    })
}

/// The six tools past ApplicationMCPTool's gate and the schema: Rails' texts, failures as `report_error`. MCP runs in the
/// caller's zone (action_mcp_time_zone.rb): the list's dates, the day bounds and the transfer window.
pub fn mcp(c: &Connection, app: &App, user: i64, name: &str, args: &Value) -> Result<Value, WebError> {
    let none = Map::new();
    let args_map = args.as_object().unwrap_or(&none);
    let zone_name: String = c.query_row("SELECT time_zone FROM users WHERE id=?1", [user], |r| r.get::<_, Option<String>>(0))?.unwrap_or_default();
    let zone = timezone::strict(&zone_name).unwrap_or(Tz::UTC);
    let failed = |res: &Res| tool_text(&res.message, true);
    let id = || args.get("transaction_id").and_then(bot_api::to_i).unwrap_or(0);
    Ok(match name {
        "list_account_transactions" => {
            let (res, text) = list(c, user, args_map, zone)?;
            if res.code.is_some() { failed(&res) } else { tool_text(read_limits::text(&text), false) }
        }
        "export_transactions_csv" => match export(c, user, args_map, zone)? {
            Err(res) => failed(&res),
            Ok(csv) => {
                let warning = if csv.truncated { format!("\n\n(Showing first {MAX_ROWS} of {} transactions. Use date filters to narrow the scope.)", csv.total) } else { String::new() };
                tool_text(&format!("{}{warning}", csv.csv), false)
            }
        },
        "set_transfer_link" => {
            // The schema requires a boolean.
            let linked = boolean(args.get("linked")).unwrap_or(false);
            let res = link(c, app, user, id(), linked, zone)?;
            if res.code.is_some() { failed(&res) } else if res.data["linked"] == true {
                tool_text(&format!("Linked withdrawal #{} with deposit #{} as a transfer.", res.data["withdrawal_id"], res.data["deposit_id"]), false)
            } else { tool_text(&format!("Unlinked withdrawal #{} from deposit #{}.", res.data["withdrawal_id"], res.data["deposit_id"]), false) }
        }
        "set_transaction_price" => {
            let res = price(c, app, user, id(), args.get("price_usd").unwrap_or(&Value::Null))?;
            if res.code.is_some() { failed(&res) } else if let Some(price) = res.data["price_usd"].as_str() {
                tool_text(&format!("Transaction #{} priced at {price} USD.", res.data["id"]), false)
            } else { tool_text(&format!("Stated price cleared on transaction #{}.", res.data["id"]), false) }
        }
        "sync_tracker" => {
            let res = sync(c, app, user)?;
            if res.code.is_some() { failed(&res) } else {
                let names: Vec<String> = res.data["exchanges"].as_array().into_iter().flatten().map(str_value).collect();
                tool_text(&format!("Sync started for {}. New transactions appear in list_account_transactions once it finishes.", names.join(", ")), false)
            }
        }
        "list_indices" => {
            let res = bot_api::indices(c, app, args.get("exchange_name").unwrap_or(&Value::Null))?;
            if res.code.is_some() { failed(&res) } else {
                let lines: Vec<String> = res.data["indices"].as_array().into_iter().flatten().map(|r| {
                    let venues: Vec<String> = r["exchanges"].as_array().into_iter().flatten().map(str_value).collect();
                    format!("- {} | {} | {} assets | {}", str_value(&r["id"]), str_value(&r["name"]), r["coins"], if venues.is_empty() { "no exchange".to_string() } else { venues.join(", ") })
                }).collect();
                tool_text(&format!("Indices ({}):\n{}", res.data["count"], lines.join("\n")), false)
            }
        }
        _ => return Err(WebError::Config("unregistered tracker tool".into())),
    })
}

#[cfg(test)]
mod tests {
    /// Recorded with activesupport 8.1.4 (`Time.zone = zone; Date.parse(day).beginning_of_day` and `.end_of_day`).
    #[test]
    fn day_bounds_resolve_as_activesupport() {
        for (zone, day, from, to) in [("Magadan", "2014-10-26", "2014-10-25T14:00:00Z", "2014-10-26T13:59:59.999999Z"),
                                      ("Santiago", "2026-09-06", "2026-09-06T04:00:00Z", "2026-09-07T02:59:59.999999Z"),
                                      ("Santiago", "2026-04-05", "2026-04-05T04:00:00Z", "2026-04-06T03:59:59.999999Z"),
                                      ("Warsaw", "2026-10-25", "2026-10-24T22:00:00Z", "2026-10-25T22:59:59.999999Z")] {
            let zone = crate::web::timezone::strict(zone).unwrap();
            let day = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
            let at = |t: &str| chrono::DateTime::parse_from_rfc3339(t).unwrap().with_timezone(&chrono::Utc);
            assert_eq!(super::wall(zone, day.and_hms_micro_opt(0, 0, 0, 0).unwrap()), Some(at(from)), "{zone} {day}");
            assert_eq!(super::wall(zone, day.and_hms_micro_opt(23, 59, 59, 999_999).unwrap()), Some(at(to)), "{zone} {day}");
        }
    }
}
