//! A28-1 (PLAN-A28 r2-r5): the orders Rails may have sent but never recorded (Rails' KNOWN GAP, action_job.rb:167-183),
//! reported to the bot's owner with what the imported ledger shows. Nothing here writes a `transactions` row, moves a
//! carry or calls a venue (D-RAILS-UNRECORDED-REPORTED).
//!
//! `collect` runs in a takeover's claim transaction when the install comes from Rails, and records one obligation per
//! source Rails wrote while it owned the install (r4.1, r5.1). `report` (the hourly job) writes each obligation's one line
//! once the ledger can say what it holds, or says why it can't (r3.4).
//!
//! A28-3 (PLAN-A28-3 r1-r4, D-RAILS-UNRECORDED-LIVE-HISTORY): the job (`Report`) also reads the order history of a served
//! venue (Binance, Binance.US, Bybit) live once that line is due, through the bot's trading key, and names in the line the
//! orders it lists that no bot recorded, open ones first (D-RAILS-UNRECORDED-OPEN-NAMED). The live read only adds: with no
//! such order, or no answer, the line is A28-1's. Nothing is booked, cancelled or carried.

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::model::{self, Level};
use crate::enums::BotStatus;
use super::{amount, EngineError};
use crate::codec::{format_time, parse_time};
use crate::venue::Side;

pub const JOB: &str = "rails_unrecorded_report";
/// Activity events (texts: web::i18n `bot_activity.events.<event>`; `rails_order_unchecked` by its `reason`).
pub const CANDIDATES: &str = "rails_order_candidates";
pub const NONE_SEEN: &str = "rails_order_none_seen";
pub const UNCHECKED: &str = "rails_order_unchecked";
/// A28-3: a line naming the venue's own orders (r2.1): one still open among them, or none open.
pub const LIVE_OPEN: &str = "rails_order_open";
pub const LIVE_LISTED: &str = "rails_order_listed";

/// Trades listed around the span (r2.3: a heuristic window, never a proof of when the request went out).
const AROUND: i64 = 10 * 60;
/// What a ledger success must postdate the window by, for a venue's fills to have been readable (r3.4).
const SETTLE: i64 = 60 * 60;
/// An obligation the ledger can't answer within this is reported as unchecked (r3.4).
const GIVE_UP_DAYS: i64 = 7;
/// Trades named in one line; the rest are counted.
const LISTED: usize = 3;

/// The venues whose imported history may not reach back far (first import 7 d / ~1 month / 89 d; Hyperliquid keeps the
/// most recent 10 000 fills): their history never says "none seen" (r3.1).
const SHORT_REACH: &[&str] = &["Exchanges::Bingx", "Exchanges::Mexc", "Exchanges::Bitget", "Exchanges::Hyperliquid"];

const AMBIGUOUS_LINE: &str = "ambiguous_line";
const QUEUE_JOB: &str = "queue_job";
const FAILED_ROW: &str = "failed_row";

// ---- collection (in the claim transaction) -----------------------------------------------------------------------------

/// (bot, source, source_ref, window start, window end, details).
type Source = (i64, &'static str, i64, String, String, Option<Value>);

/// Records the obligations of Rails' ownership that ended with this takeover: sources created after `since` (the last
/// handback, r5.1; None = an install Rust never ran) and before `now`. Idempotent on (source, source_ref).
pub fn collect(tx: &Connection, queue: &Connection, since: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Result<usize, EngineError> {
    let owned = |t: DateTime<Utc>| since.is_none_or(|s| t > s) && t < now;
    let until = format_time(now);
    let mut found: Vec<Source> = vec![];
    // (a) every placement_ambiguous line on a non-signal bot (D-RAILS-UNRECORDED-ALL-LINES).
    // A line this engine wrote (placement.rs: a proved absence, an operator's resolution) carries its source and the
    // intent's client order id, which Rails' `{ error: }` never does (r4.1).
    let mut s = tx.prepare("SELECT l.id, l.bot_id, l.created_at, l.details FROM bot_activity_logs l JOIN bots b ON b.id = l.bot_id \
                            WHERE l.event = 'placement_ambiguous' AND b.type <> ?1 AND b.status <> ?2 ORDER BY l.id")?;
    for row in s.query_map(params![model::SIGNAL_TYPE, BotStatus::Deleted as i64],
                           |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))? {
        let (id, bot, at, details) = row?;
        // Rails' window is judged on the parsed time; one that can't be read is collected with its error (Codex r4).
        let at_error = match parse_time(&at) { Ok(t) if !owned(t) => continue, Ok(_) => None, Err(e) => Some(format!("{at:?}: {e:?}")) };
        let details: Value = match details.as_deref().map(serde_json::from_str::<Value>) { Some(Ok(v)) => v, _ => Value::Null };
        if details.get("source").is_some() || details.get("cl_ord_id").is_some() || details.get("resolution").is_some() { continue; }
        match at_error {
            None => found.push((bot, AMBIGUOUS_LINE, id, at.clone(), at, None)),
            Some(e) => found.push((bot, AMBIGUOUS_LINE, id, until.clone(), until.clone(), Some(json!({ "time_error": e })))),
        }
    }
    drop(s);
    // (c) failed REGULAR rows with no external id whose saved error is an ambiguous placement (D-RAILS-UNRECORDED-FAILED-ROWS).
    let mut s = tx.prepare("SELECT t.id, t.bot_id, t.created_at, t.error_messages, t.exchange_id, t.base_asset_id, t.quote_asset_id, t.side, e.type \
                            FROM transactions t JOIN bots b ON b.id = t.bot_id JOIN exchanges e ON e.id = t.exchange_id \
                            WHERE t.status = 1 AND t.transaction_type = 'REGULAR' AND t.external_id IS NULL AND t.run_identity IS NULL \
                            AND b.type <> ?1 AND b.status <> ?2 ORDER BY t.id")?;
    let rows = s.query_map(params![model::SIGNAL_TYPE, BotStatus::Deleted as i64], |r| Ok((
        r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, i64>(4)?,
        r.get::<_, Option<i64>>(5)?, r.get::<_, Option<i64>>(6)?, r.get::<_, Option<i64>>(7)?, r.get::<_, Option<String>>(8)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for (id, bot, at, errors, exchange, base, quote, side, venue) in rows {
        // Unreadable saved errors are of unknown provenance: collected (r3.3).
        let errors: Vec<String> = match errors.as_deref().map(serde_json::from_str::<Vec<String>>) { Some(Ok(e)) => e, _ => vec![] };
        if !ambiguous_failure(venue.as_deref().unwrap_or(""), &errors) { continue; }
        match parse_time(&at) {
            Ok(t) if !owned(t) => {}
            Ok(_) => found.push((bot, FAILED_ROW, id, at.clone(), at, Some(json!({ "exchange_id": exchange, "base_asset_id": base, "quote_asset_id": quote, "side": side })))),
            Err(e) => found.push((bot, FAILED_ROW, id, until.clone(), until.clone(), Some(json!({ "time_error": format!("{at:?}: {e:?}") })))),
        }
    }
    // (b) a claimed or failed Bot::ActionJob (C12's queue case): when it was claimed and when it can have ended.
    let mut s = queue.prepare("SELECT j.id, j.arguments, j.scheduled_at, j.created_at, c.created_at, p.last_heartbeat_at, f.created_at FROM solid_queue_jobs j \
                               LEFT JOIN solid_queue_claimed_executions c ON c.job_id = j.id LEFT JOIN solid_queue_processes p ON p.id = c.process_id \
                               LEFT JOIN solid_queue_failed_executions f ON f.job_id = j.id \
                               WHERE j.finished_at IS NULL AND j.class_name = 'Bot::ActionJob' AND (c.id IS NOT NULL OR f.id IS NOT NULL) ORDER BY j.id")?;
    let jobs = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?,
                                       r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?, r.get::<_, Option<String>>(6)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for (id, args, scheduled, created, claimed, heartbeat, failed) in jobs {
        let Some(bot) = super::handover::job_bot(args.as_deref()) else { continue };
        let live: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM bots WHERE id = ?1 AND type <> ?2 AND status <> ?3)",
                                      params![bot, model::SIGNAL_TYPE, BotStatus::Deleted as i64], |r| r.get(0))?;
        if !live { continue; }
        // A time that can't be read never fails the takeover: the obligation keeps the error and its line says the time
        // is unknown (the first readable of the claim, the schedule and the enqueue opens the span).
        let span = (|| -> Result<(DateTime<Utc>, DateTime<Utc>), String> {
            let t = |s: Option<&str>| -> Result<Option<DateTime<Utc>>, String> {
                match s { Some(s) => parse_time(s).map(Some).map_err(|e| format!("{s:?}: {e:?}")), None => Ok(None) }
            };
            let from = match t(claimed.as_deref())? { Some(c) => c, None => match t(scheduled.as_deref())? { Some(s) => s, None => t(Some(&created))?.ok_or("no time")? } };
            let bound = from + Duration::minutes(10);
            let to = match t(failed.as_deref())? {
                Some(f) => f,
                None => match t(heartbeat.as_deref())? { Some(h) => (h + Duration::seconds(60)).min(bound).max(from), None => bound },
            };
            Ok((from, to))
        })();
        match span {
            Ok((from, to)) => found.push((bot, QUEUE_JOB, id, format_time(from), format_time(to), None)),
            Err(e) => found.push((bot, QUEUE_JOB, id, until.clone(), until.clone(), Some(json!({ "time_error": e })))),
        }
    }
    let mut n = 0;
    for (bot, source, sref, from, to, scope) in found {
        n += tx.execute("INSERT INTO rust_rails_unrecorded (bot_id, source, source_ref, span_from, span_to, scope, collected_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT (source, source_ref) DO NOTHING",
                        params![bot, source, sref, from, to, scope.map(|s| s.to_string()), until])?;
    }
    Ok(n)
}

/// Does a failed row's saved error leave the order's fate unknown (r3.3, Exchange#ambiguous_placement_error? read from
/// what a Transaction keeps)? Never when the text proves nothing left (Client.pre_transmission?: a refused or unopened
/// connection, a name that didn't resolve, a refused proxy CONNECT) or the venue's own pre-trade rejection (-1021);
/// never for a rejection the venue's own error table names (insufficient funds, a bad key, a refused permission, a
/// restriction, a throttle) or a 4xx. Everything else is collected, the unknown included: a report too many, never one
/// too few.
pub fn ambiguous_failure(venue: &str, errors: &[String]) -> bool {
    const PRE_TRANSMISSION: &[&str] = &["Net::OpenTimeout", "Failed to open TCP connection", "SocketError", "getaddrinfo", "ResolutionError",
        "Resolv::ResolvError", "ECONNREFUSED", "EHOSTUNREACH", "EHOSTDOWN", "ENETUNREACH", "ENETDOWN", "EADDRNOTAVAIL", "Net::HTTPClientException"];
    const POST_TRANSMISSION: &[&str] = &["Net::ReadTimeout", "Net::WriteTimeout", "Faraday::TimeoutError", "Faraday::ConnectionFailed", "Faraday::SSLError",
        "execution expired", "Connection reset", "ECONNRESET", "EPIPE", "ETIMEDOUT", "end of file reached", "unexpected eof while reading", "EOFError",
        "Unreadable response", "order_id is nil", "with no order id", "no order ID returned", "no statuses returned"];
    let text = errors.join(" ");
    // Proof that nothing left decides first (Client.pre_transmission?), then a transport failure after the connection opened.
    let pre = PRE_TRANSMISSION.iter().any(|p| text.contains(p)) || text.to_ascii_lowercase().contains("connection refused");
    if pre { return false; }
    if POST_TRANSMISSION.iter().any(|p| text.contains(p)) { return true; }
    if super::placement::safe_rejection(errors) { return false; }
    let rejected = super::venue_rules::for_exchange(venue).is_some_and(|r| r.kinds.iter().filter(|(kind, _)| *kind != "transient")
        .any(|(_, texts)| texts.iter().any(|t| errors.iter().any(|e| e.contains(t)))));
    !(rejected || http_status(&text, '4'))
}

/// "HTTP 4xx" in the text (Client#with_rescue's HTML-bodied answer).
fn http_status(text: &str, class: char) -> bool {
    let tag = format!("HTTP {class}");
    text.match_indices(&tag).any(|(i, _)| text[i + tag.len()..].chars().take(2).filter(char::is_ascii_digit).count() == 2)
}

// ---- the report (the hourly job) ---------------------------------------------------------------------------------------

/// Writes the line of every obligation the ledger can now answer, or that can't be answered. One line per obligation,
/// ever: the row is marked in the line's transaction.
pub fn report(c: &Connection, now: DateTime<Utc>) -> Result<usize, EngineError> {
    let open: Vec<i64> = {
        let mut s = c.prepare("SELECT id FROM rust_rails_unrecorded WHERE reported_at IS NULL ORDER BY id")?;
        let ids = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids
    };
    let mut written = 0;
    // Per pass, per account: the order keys of every fill already linked to a bot's order (Codex r5: read once, as a set).
    let mut linked = Linked::new();
    for id in open {
        // Judged on a plain read; only the line and its mark take the write lock, re-checked there (one line, ever).
        let o = match load(c, id) {
            Ok(Some(o)) => o,
            Ok(None) => continue,
            // A row whose stored times can't be read is answered as such, so it never stops the obligations after it.
            Err(EngineError::Data(e)) => {
                let tx = model::immediate(c)?;
                let bot: i64 = tx.query_row("SELECT bot_id FROM rust_rails_unrecorded WHERE id = ?1", [id], |r| r.get(0))?;
                let line = Line { event: UNCHECKED, outcome: "time", details: json!({ "when": "at a time that can't be read", "venue": "exchange", "reason": "time", "error": e }) };
                if tx.execute("UPDATE rust_rails_unrecorded SET reported_at = ?2, outcome = ?3 WHERE id = ?1 AND reported_at IS NULL", params![id, format_time(now), line.outcome])? == 1 {
                    model::log_activity(&tx, bot, line.event, Level::Warning, line.details, now)?;
                    tx.commit()?;
                    written += 1;
                }
                continue;
            }
            Err(e) => return Err(e),
        };
        let Some(Judged { line, .. }) = judge(c, &o, now, &mut linked)? else { continue };
        let tx = model::immediate(c)?;
        if tx.execute("UPDATE rust_rails_unrecorded SET reported_at = ?2, outcome = ?3 WHERE id = ?1 AND reported_at IS NULL", params![id, format_time(now), line.outcome])? == 1 {
            model::log_activity(&tx, o.bot, line.event, Level::Warning, line.details, now)?;
            tx.commit()?;
            written += 1;
        }
    }
    Ok(written)
}

struct Obligation { bot: i64, source: String, source_ref: i64, from: DateTime<Utc>, to: DateTime<Utc>, scope: Option<Value>, collected: DateTime<Utc> }

fn load(c: &Connection, id: i64) -> Result<Option<Obligation>, EngineError> { load_where(c, id, true) }

/// The obligation, only while its line is unwritten when `open`.
fn load_where(c: &Connection, id: i64, open: bool) -> Result<Option<Obligation>, EngineError> {
    let row = c.query_row("SELECT bot_id, source, span_from, span_to, scope, collected_at, source_ref FROM rust_rails_unrecorded WHERE id = ?1 AND (reported_at IS NULL OR NOT ?2)",
                          params![id, open],
                          |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?, r.get::<_, i64>(6)?)))
        .optional()?;
    let Some((bot, source, from, to, scope, collected, source_ref)) = row else { return Ok(None) };
    let t = |s: &str| parse_time(s).map_err(|e| EngineError::Data(format!("rust_rails_unrecorded {id}: {e:?}")));
    Ok(Some(Obligation { bot, source, source_ref, from: t(&from)?, to: t(&to)?, scope: scope.and_then(|s| serde_json::from_str(&s).ok()), collected: t(&collected)? }))
}

/// The pair and side a failed row recorded (r3.2), when all four are known.
fn stored_scope(s: &Value) -> Option<Scope> {
    let get = |k: &str| s.get(k).and_then(Value::as_i64);
    match (get("exchange_id"), get("base_asset_id"), get("quote_asset_id"), get("side")) {
        (Some(e), Some(b), Some(q), Some(side)) => Some(Scope { exchange: e, bases: vec![b], quote: q, side: if side == 1 { Side::Sell } else { Side::Buy } }),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Line { event: &'static str, outcome: &'static str, details: Value }

/// A due line, and for an outcome judged on the ledger (none seen, candidates, unread, unreadable) what a live read of the
/// venue would be merged with (A28-3 r2.1). Prefix outcomes (gone, time, index, settings changed) carry none.
struct Judged { line: Line, basis: Option<Basis> }
struct Basis { user: i64, venue_type: String, venue: String, when: String, scope: Scope, from: DateTime<Utc>, to: DateTime<Utc>, trades: Vec<Trade> }

/// What the obligation's line says, or None while the ledger may still bring its answer.
fn judge(c: &Connection, o: &Obligation, now: DateTime<Utc>, linked: &mut Linked) -> Result<Option<Judged>, EngineError> {
    Ok(judged(c, o, now, linked)?.map(|(line, basis)| Judged { line, basis }))
}

#[allow(clippy::type_complexity)]
fn judged(c: &Connection, o: &Obligation, now: DateTime<Utc>, linked: &mut Linked) -> Result<Option<(Line, Option<Basis>)>, EngineError> {
    let when = when(o);
    let bot: Option<(i64, i64, String, Option<String>, String, i64)> = c.query_row(
        "SELECT b.user_id, b.exchange_id, b.type, e.type, e.name, b.status FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", [o.bot],
        |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get(2)?, r.get(3)?,
                r.get::<_, Option<String>>(4)?.unwrap_or_default(), r.get(5)?))).optional()?;
    let Some((user, bot_exchange, bot_type, _, _, status)) = bot else { return Ok(None) };
    let prefix = |line: Line| Ok(Some((line, None)));
    // The venue a line names when the scope is unknown: the exchange of the bot's order nearest the span, never today's
    // exchange of a bot that moved (r3.2); "exchange" when it has none.
    let then = || -> Result<String, EngineError> {
        let nearest: Option<i64> = c.query_row("SELECT exchange_id FROM transactions WHERE bot_id = ?1 ORDER BY abs(julianday(created_at) - julianday(?2)) LIMIT 1",
                                               params![o.bot, format_time(o.from)], |r| r.get(0)).optional()?;
        match nearest { Some(e) => venue_name(c, e).map(|v| if v.is_empty() { "exchange".into() } else { v }), None => Ok("exchange".into()) }
    };
    if status == BotStatus::Deleted as i64 { return prefix(unchecked(&when, "exchange", "gone")); }
    if o.scope.as_ref().is_some_and(|s| s.get("time_error").is_some()) {
        return prefix(Line { event: UNCHECKED, outcome: "time", details: json!({ "when": "at a time that can't be read", "venue": then()?, "reason": "time" }) });
    }
    // The pairs and side to look at, only where they are known (r3.2, r4.2).
    let scope = match &o.scope {
        Some(s) => stored_scope(s),
        None if bot_type == "Bots::DcaIndex" => return prefix(unchecked(&when, &then()?, "index")),
        None => current_scope(c, o, bot_exchange)?,
    };
    let Some(scope) = scope else { return prefix(unchecked(&when, &then()?, "settings_changed")) };
    let (venue_type, venue) = c.query_row("SELECT type, name FROM exchanges WHERE id = ?1", [scope.exchange], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    if !crate::sync::fills::supported(&venue_type) { return prefix(unchecked(&when, &venue, "no_history")); }
    let search_from = o.from - Duration::seconds(AROUND);
    let search_to = o.to + Duration::seconds(AROUND);
    let basis = |trades: Vec<Trade>, scope: Scope| Some(Basis { user, venue_type: venue_type.clone(), venue: venue.clone(), when: when.clone(), scope, from: search_from, to: search_to, trades });
    // The ledger has the window when a sync of the user's reading key on that venue reached the venue's end after it.
    let readable = match reading_key(c, user, scope.exchange)? {
        Some(key) => {
            let state = crate::jobs::state::read(c, crate::sync::jobs::LEDGER_SYNC, Some(&key.to_string())).map_err(EngineError::Data)?;
            state.incomplete_since.is_none() && state.last_success_at.is_some_and(|t| t >= search_to + Duration::seconds(SETTLE))
        }
        None => false,
    };
    if !readable {
        if now < o.collected + Duration::days(GIVE_UP_DAYS) { return Ok(None); }
        return Ok(Some((unchecked(&when, &venue, "unread"), basis(vec![], scope))));
    }
    let trades = match trades(c, user, scope.exchange, &venue_type, &scope, search_from, search_to, linked)? {
        Ok(t) => t,
        Err(()) => return Ok(Some((unchecked(&when, &venue, "unreadable"), basis(vec![], scope)))),
    };
    if trades.is_empty() {
        if SHORT_REACH.contains(&venue_type.as_str()) { return Ok(Some((unchecked(&when, &venue, "reach"), basis(vec![], scope)))); }
        return Ok(Some((Line { event: NONE_SEEN, outcome: "none_seen", details: json!({ "when": when, "venue": venue }) }, basis(vec![], scope))));
    }
    let mut listed: Vec<String> = trades.iter().take(LISTED).map(|t| t.describe(c)).collect::<Result<_, _>>()?;
    if trades.len() > LISTED { listed.push(format!("and {} more", trades.len() - LISTED)); }
    let line = Line { event: CANDIDATES, outcome: "candidates", details: json!({ "when": when, "venue": venue, "trades": listed.join("; "), "count": trades.len() }) };
    Ok(Some((line, basis(trades, scope))))
}

fn unchecked(when: &str, venue: &str, reason: &'static str) -> Line {
    Line { event: UNCHECKED, outcome: reason, details: json!({ "when": when, "venue": venue, "reason": reason }) }
}

fn venue_name(c: &Connection, exchange: i64) -> Result<String, EngineError> {
    Ok(c.query_row("SELECT name FROM exchanges WHERE id = ?1", [exchange], |r| r.get::<_, Option<String>>(0)).optional()?.flatten().unwrap_or_default())
}

fn when(o: &Obligation) -> String {
    let f = |t: DateTime<Utc>| t.format("%Y-%m-%d %H:%M UTC").to_string();
    if o.source == QUEUE_JOB && o.to > o.from {
        let to = if o.to.date_naive() == o.from.date_naive() { o.to.format("%H:%M UTC").to_string() } else { f(o.to) };
        format!("between {} and {to}", f(o.from))
    } else { format!("around {}", f(o.from)) }
}

#[derive(Clone, Debug, PartialEq)]
struct Scope { exchange: i64, bases: Vec<i64>, quote: i64, side: Side }

/// A DCA bot's pairs and side today, when nothing since the span can have changed them (r3.2): no settings change, no
/// (re)start, no merge or split, and every later order of the bot on this exchange, on these pairs and on this side.
/// The pairs come from the stored settings by asset id, delisted ones included (r4.2).
fn current_scope(c: &Connection, o: &Obligation, exchange: i64) -> Result<Option<Scope>, EngineError> {
    let bot = model::load_bot(c, o.bot)?;
    // From the span's start: a change while Rails was placing leaves the scope unknown too.
    let after = format_time(o.from);
    // Rails stores these marks to the second: one in the span's own second may follow it (Codex r5), so it counts.
    let second = o.from.format("%Y-%m-%d %H:%M:%S").to_string();
    let changed: bool = c.query_row("SELECT (settings_changed_at IS NOT NULL AND settings_changed_at >= ?2) OR (started_at IS NOT NULL AND started_at >= ?2) FROM bots WHERE id = ?1",
                                    params![o.bot, second], |r| r.get(0))?;
    if changed { return Ok(None); }
    let lines: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM bot_activity_logs WHERE bot_id = ?1 AND created_at > ?2 AND \
                                   (event IN ('started', 'stopped', 'merged', 'split', 'asset_split') OR event LIKE '%flip%' OR event LIKE 'start_selling%' OR event LIKE 'start_buying%'))",
                                  params![o.bot, after], |r| r.get(0))?;
    if lines { return Ok(None); }
    let mut bases = bot.asset_ids();
    if let Some(b) = bot.settings.get("base_asset_id").and_then(Value::as_i64) { if !bases.contains(&b) { bases.push(b); } }
    let Some(quote) = bot.quote_asset_id() else { return Ok(None) };
    if bases.is_empty() || exchange == 0 { return Ok(None); }
    let side = if bot.selling() { Side::Sell } else { Side::Buy };
    // The exchange then: Rails can move a bot to another exchange without touching any of the marks above. The bot's own
    // nearest order before the span names it; with none before it is unknown.
    let before: Option<i64> = c.query_row("SELECT exchange_id FROM transactions WHERE bot_id = ?1 AND created_at <= ?2 ORDER BY created_at DESC, id DESC LIMIT 1",
                                          params![o.bot, after], |r| r.get(0)).optional()?;
    // Only an order before the span names it: one after may follow a move (Codex r3).
    if before != Some(exchange) { return Ok(None); }
    let mut s = c.prepare("SELECT exchange_id, base_asset_id, quote_asset_id, side FROM transactions WHERE bot_id = ?1 AND created_at > ?2")?;
    for row in s.query_map(params![o.bot, after], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?)))? {
        let (e, b, q, sd) = row?;
        if e != exchange || q != Some(quote) || !b.is_some_and(|b| bases.contains(&b)) || sd != Some(side.code()) { return Ok(None); }
    }
    Ok(Some(Scope { exchange, bases, quote, side }))
}

/// ApiKey.reading for one user and venue (sync::ledger_reading_keys).
fn reading_key(c: &Connection, user: i64, exchange: i64) -> Result<Option<i64>, EngineError> {
    let keys = crate::sync::ledger_reading_keys(c).map_err(|e| EngineError::Data(e.0))?;
    for key in keys {
        let (u, e): (i64, i64) = c.query_row("SELECT user_id, exchange_id FROM api_keys WHERE id = ?1", [key], |r| Ok((r.get(0)?, r.get(1)?)))?;
        if (u, e) == (user, exchange) { return Ok(Some(key)); }
    }
    Ok(None)
}

/// One venue order no bot recorded (`key` in the external_id form), with its fills inside the window.
#[derive(Clone, Debug, PartialEq)]
struct Trade { key: String, side: Side, ticker: i64, base: crate::ruby::BigDec, quote: crate::ruby::BigDec, at: DateTime<Utc> }

impl Trade {
    fn describe(&self, c: &Connection) -> Result<String, EngineError> {
        let (b, q): (String, String) = c.query_row("SELECT base, quote FROM tickers WHERE id = ?1", [self.ticker], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let verb = match self.side { Side::Buy => "bought", Side::Sell => "sold" };
        Ok(format!("{verb} {} {b} for {} {q} on {} (filled)", self.base.to_s_f(), self.quote.to_s_f(), self.at.format("%Y-%m-%d %H:%M:%S UTC")))
    }
}

/// The trades on the scope's pairs and side inside the window whose order no bot holds and none of whose fills is linked
/// to a bot's order (r2.1, r3.6). Err(()) when a ledger row inside the window can't be read (r3.6, r4.3).
/// (user, exchange) → the order keys of its linked fills, or Err(()) when one can't be read.
type Linked = std::collections::HashMap<(i64, i64), Result<std::collections::HashSet<String>, ()>>;

#[allow(clippy::too_many_arguments)]
fn trades(c: &Connection, user: i64, exchange: i64, venue: &str, scope: &Scope, from: DateTime<Utc>, to: DateTime<Utc>, cache: &mut Linked) -> Result<Result<Vec<Trade>, ()>, EngineError> {
    use crate::sync::fills::{self, LedgerRow, Read};
    let mut s = c.prepare("SELECT id, tx_id, entry_type, raw_data, transacted_at, transaction_id FROM account_transactions \
                           WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at >= ?3 AND transacted_at <= ?4 ORDER BY transacted_at, id")?;
    let rows = s.query_map(params![user, exchange, format_time(from), format_time(to)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?,
                                                                                       r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<i64>>(5)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(s);
    let mut seen: Vec<String> = vec![];
    let mut linked: std::collections::HashSet<String> = Default::default();
    let mut orders: Vec<(String, Trade)> = vec![];
    for (id, tx_id, entry_type, raw, at, link) in rows {
        let raw: Value = raw.as_deref().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
        let Ok(transacted_at) = parse_time(&at) else { return Ok(Err(())) };
        let row = LedgerRow { id, tx_id, entry_type, raw_data: raw, transacted_at, exchange_id: exchange };
        let fill = match fills::read(c, venue, &row).map_err(EngineError::Data)? {
            Read::Fill(f) => f,
            Read::NotATrade => continue,
            Read::Unreadable(_) => return Ok(Err(())),
        };
        if link.is_some() { linked.insert(fill.order_key.clone()); }
        if seen.contains(&fill.fill_id) { continue; }
        seen.push(fill.fill_id.clone());
        if fill.side != scope.side || fill.quote_asset_id != scope.quote || !scope.bases.contains(&fill.base_asset_id) { continue; }
        match orders.iter_mut().find(|(k, _)| *k == fill.order_key) {
            Some((_, t)) => {
                t.base = &t.base + &fill.base_qty;
                t.quote = &t.quote + &fill.quote_qty;
                t.at = t.at.min(fill.at);
            }
            None => orders.push((fill.order_key.clone(), Trade { key: fill.order_key.clone(), side: fill.side, ticker: fill.ticker_id, base: fill.base_qty, quote: fill.quote_qty, at: fill.at })),
        }
    }
    // A fill of the order linked to a bot's order anywhere in the history, not only in the window, excludes it.
    if !orders.is_empty() {
        match linked_keys(c, user, exchange, venue, cache)? {
            Ok(keys) => linked.extend(keys.iter().cloned()),
            Err(()) => return Ok(Err(())),
        }
    }
    let mut out = vec![];
    for (key, trade) in orders {
        if linked.contains(&key) { continue; }
        // Rails' Bitget parser also takes a bare order id (bitget.rb:719), so a legacy row may hold that form.
        let bare = if venue == "Exchanges::Bitget" { key.rsplit_once('-').map(|(_, id)| id.to_string()) } else { None };
        let held: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM transactions WHERE exchange_id = ?1 AND (external_id = ?2 OR external_id = ?3))",
                                     params![exchange, key, bare.unwrap_or_else(|| key.clone())], |r| r.get(0))?;
        if !held { out.push(trade); }
    }
    Ok(Ok(out))
}

/// The order keys of every fill of the account linked to a bot's order, read once per pass (Codex r5), or Err(()) when a
/// linked row can't be read: it may be a fill of a listed order, so then no list can be trusted (unchecked).
fn linked_keys<'a>(c: &Connection, user: i64, exchange: i64, venue: &str, cache: &'a mut Linked) -> Result<&'a Result<std::collections::HashSet<String>, ()>, EngineError> {
    use crate::sync::fills::{self, LedgerRow, Read};
    if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry((user, exchange)) {
        let mut all: Result<std::collections::HashSet<String>, ()> = Ok(Default::default());
        let mut s = c.prepare("SELECT id, tx_id, entry_type, raw_data, transacted_at FROM account_transactions \
                               WHERE user_id = ?1 AND exchange_id = ?2 AND transaction_id IS NOT NULL")?;
        let rows = s.query_map(params![user, exchange], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?,
                                                                 r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
        drop(s);
        for (id, tx_id, entry_type, raw, at) in rows {
            let raw: Value = match raw.as_deref().map(serde_json::from_str::<Value>) { Some(Ok(v)) => v, _ => Value::Null };
            let Ok(transacted_at) = parse_time(&at) else { all = Err(()); break };
            match fills::read(c, venue, &LedgerRow { id, tx_id, entry_type, raw_data: raw, transacted_at, exchange_id: exchange }).map_err(EngineError::Data)? {
                Read::Fill(f) => { if let Ok(keys) = all.as_mut() { keys.insert(f.order_key); } }
                Read::NotATrade => {}
                Read::Unreadable(_) => { all = Err(()); break }
            }
        }
        slot.insert(all);
    }
    Ok(cache.get(&(user, exchange)).expect("inserted above"))
}

/// The report as a scheduler job: hourly, and at the first run after a start (r5.2). Its state is the table. A28-3: its
/// venues read the order history of a served venue live (`pass`).
pub struct Report<F> { venues: F }

impl<F> Report<F> {
    pub fn new(venues: F) -> Self { Self { venues } }
}

impl<F: crate::venue::VenueFactory> crate::jobs::Job for Report<F> {
    fn spec(&self) -> crate::jobs::Spec {
        crate::jobs::Spec { name: JOB, scope: None, schedule: Some(crate::jobs::schedule::Schedule::EveryHours { every: 1, minute: 41 }),
                            jitter: crate::jobs::schedule::Jitter::NONE, retry: crate::jobs::Retry::None, deadline: crate::jobs::DEADLINE }
    }
    fn run<'a>(&'a self, cx: crate::jobs::Cx<'a>, _wakes: Vec<crate::jobs::Wake>) -> crate::jobs::JobFuture<'a> {
        Box::pin(async move {
            use crate::jobs::Outcome;
            let now = cx.clock.now();
            match self.pass(&cx.db, now).await {
                Ok(0) => Outcome::NothingNew,
                Ok(_) => Outcome::Done,
                Err(e) => Outcome::Failed(e),
            }
        })
    }
}

// ---- A28-3: the venue's order history, read live once the line is due (PLAN-A28-3 r1-r4) --------------------------------

use crate::venue::binance::{BINANCE, BINANCE_US};
use crate::venue::{Created, CreatedKind, CreatedOrder, CreatedSize, CreatedState, HistoryError, Venue};

/// The venues whose order history is read live (A28-3a): Rails' own clients pin their order-list answer (§1).
const SERVED: [&str; 3] = [BINANCE, BINANCE_US, crate::venue::bybit::BYBIT];
/// Pairs one read may ask for (§4); more leave the line A28-1's.
const MAX_SYMBOLS: usize = 5;
/// Live reads per pass (r1.4), taken round-robin from `LIVE_CURSOR`.
const MAX_READS: usize = 20;
/// Failed reads (one per pass, at least `RETRY_AFTER` apart) before the line is written without them (r2.3).
const MAX_ATTEMPTS: i64 = 3;
const RETRY_AFTER: i64 = 60 * 60;
/// A read's deadline: its requests past it fail, the rows already validated kept (r2.2).
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
/// No read starts after this much of a pass (the job's deadline is 600 s).
const PASS_BUDGET: std::time::Duration = std::time::Duration::from_secs(300);
/// The id after the last obligation a pass read live (r1.4).
pub const LIVE_CURSOR: &str = "rust_rails_unrecorded.live_cursor";

/// A live read prepared from the database (phase 1) and compared again with what the write prepares (r1.5).
#[derive(Clone, Debug, PartialEq)]
struct Prepared { id: i64, bot: i64, user: i64, exchange: i64, venue_type: String, venue: String, when: String, symbols: Vec<String>,
                  from: DateTime<Utc>, to: DateTime<Utc>, side: Side, line: Line, trades: Vec<Trade>, stamp: String }

/// The read with the key it is made with: the credentials and their version from one snapshot (credential_write fences
/// the line on that version).
struct LiveRead { prepared: Prepared, credentials: crate::crypto::Credentials, sensitive: Vec<String>, version: model::CredentialVersion }

enum Prep { Wait, Write(i64, Line), Live(Box<LiveRead>) }

impl<F: crate::venue::VenueFactory> Report<F> {
    /// One pass (§5, r1.4): (1) one database unit writes every due line that needs no live read and prepares up to
    /// MAX_READS reads; (2) each read is awaited outside the database, under READ_TIMEOUT, none started after PASS_BUDGET;
    /// (3) each read's outcome is written in its own unit (`settle`). Returns the lines written.
    pub async fn pass(&self, db: &crate::jobs::Db, now: DateTime<Utc>) -> Result<usize, String> {
        let (mut written, reads) = db.run(move |c, cipher| plan(c, cipher, now).map_err(|e| format!("{e:?}"))).await?;
        let started = std::time::Instant::now();
        let mut last = None;
        for read in reads {
            if started.elapsed() >= PASS_BUDGET { break; }
            let p = &read.prepared;
            let venue = self.venues.for_bot_with_redaction(&p.venue_type, Some(read.credentials.clone()), read.sensitive.clone());
            let result = venue.orders_created(&p.symbols, p.from, p.to, tokio::time::Instant::now() + READ_TIMEOUT).await;
            last = Some(p.id);
            if db.run(move |c, cipher| settle(c, cipher, &read, result, now).map_err(|e| format!("{e:?}"))).await? { written += 1; }
        }
        if let Some(id) = last { db.run(move |c, _| crate::app_config::set_plain(c, LIVE_CURSOR, &(id + 1).to_string(), now)).await?; }
        Ok(written)
    }
}

/// Phase 1: every open obligation from the cursor on (then those before it), each judged as A28-1 judges it.
fn plan(c: &Connection, cipher: &crate::crypto::Cipher, now: DateTime<Utc>) -> Result<(usize, Vec<LiveRead>), EngineError> {
    let ids: Vec<i64> = {
        let mut s = c.prepare("SELECT id FROM rust_rails_unrecorded WHERE reported_at IS NULL ORDER BY id")?;
        let ids = s.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids
    };
    let cursor: i64 = crate::app_config::get_plain(c, LIVE_CURSOR).map_err(EngineError::Data)?.and_then(|v| v.parse().ok()).unwrap_or(0);
    let (after, before): (Vec<i64>, Vec<i64>) = ids.into_iter().partition(|id| *id >= cursor);
    let (mut written, mut reads) = (0, vec![]);
    let mut linked = Linked::new();
    for id in after.into_iter().chain(before) {
        match prepare(c, cipher, id, now, &mut linked)? {
            Prep::Wait => {}
            Prep::Write(bot, line) => { if write_line(c, id, bot, &line, now)? { written += 1; } }
            // The rest wait for the next pass.
            Prep::Live(read) => { if reads.len() < MAX_READS { reads.push(*read); } }
        }
    }
    Ok((written, reads))
}

/// What a pass does with one obligation now: nothing yet, write this line, or read the venue first. Live only for a
/// served venue whose line A28-1 would write now from the ledger (r2.1), read with the bot's trading key when it has not
/// changed since before the window (r1.1), on pairs that each name exactly one listing (§4), and while the key has not
/// been refused or failed MAX_ATTEMPTS times (r2.3).
fn prepare(c: &Connection, cipher: &crate::crypto::Cipher, id: i64, now: DateTime<Utc>, linked: &mut Linked) -> Result<Prep, EngineError> {
    let o = match load(c, id) {
        Ok(Some(o)) => o,
        Ok(None) => return Ok(Prep::Wait),
        Err(EngineError::Data(e)) => {
            let bot: i64 = c.query_row("SELECT bot_id FROM rust_rails_unrecorded WHERE id = ?1", [id], |r| r.get(0))?;
            return Ok(Prep::Write(bot, Line { event: UNCHECKED, outcome: "time", details: json!({ "when": "at a time that can't be read", "venue": "exchange", "reason": "time", "error": e }) }));
        }
        Err(e) => return Err(e),
    };
    let Some(Judged { line, basis }) = judge(c, &o, now, linked)? else { return Ok(Prep::Wait) };
    let Some(b) = basis.filter(|b| SERVED.contains(&b.venue_type.as_str())) else { return Ok(Prep::Write(o.bot, line)) };
    let exchange = b.scope.exchange;
    let (credentials, version, sensitive) = match model::credentials_with_redaction(c, cipher, &model::Bot::key_holder(b.user, exchange)) {
        Ok((Some(credentials), Some(version), sensitive)) => (credentials, version, sensitive),
        Ok(_) | Err(EngineError::Data(_)) => return Ok(Prep::Write(o.bot, line)),
        Err(e) => return Err(e),
    };
    // r1.1: a key replaced since before the window may be another account's; Rails stores the mark to the second.
    let updated: Option<String> = c.query_row("SELECT updated_at FROM api_keys WHERE id = ?1", [version.id], |r| r.get(0))?;
    let continuous = match updated.as_deref().map(parse_time) { Some(Ok(u)) => u + Duration::seconds(1) < b.from, _ => false };
    if !continuous { return Ok(Prep::Write(o.bot, line)); }
    let mut symbols = vec![];
    for base in &b.scope.bases {
        let mut s = c.prepare_cached("SELECT ticker FROM tickers WHERE exchange_id = ?1 AND base_asset_id = ?2 AND quote_asset_id = ?3 LIMIT 2")?;
        let rows = s.query_map(params![exchange, base, b.scope.quote], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        let [symbol] = &rows[..] else { return Ok(Prep::Write(o.bot, line)) };
        if symbol.starts_with("__stale_") || crate::sync::fills::symbol_reused(c, exchange, symbol).map_err(EngineError::Data)? { return Ok(Prep::Write(o.bot, line)); }
        symbols.push(symbol.clone());
    }
    if symbols.len() > MAX_SYMBOLS { return Ok(Prep::Write(o.bot, line)); }
    let stamp = version.cache_stamp().to_string();
    let state: Option<(String, i64, Option<String>, Option<String>)> = c.query_row(
        "SELECT stamp, attempts, refused_at, last_at FROM rust_rails_unrecorded_live WHERE obligation_id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    // A row of another key is no state of this one (a new key is read afresh).
    if let Some((_, attempts, refused, last)) = state.filter(|(s, ..)| same_stamp(s, &stamp)) {
        if refused.is_some() || attempts >= MAX_ATTEMPTS { return Ok(Prep::Write(o.bot, line)); }
        // One read per pass at most, a failed one at least RETRY_AFTER before the next (a mark that can't be read: none).
        if matches!(last.as_deref().map(parse_time), Some(Ok(l)) if now < l + Duration::seconds(RETRY_AFTER)) { return Ok(Prep::Wait); }
    }
    let prepared = Prepared { id, bot: o.bot, user: b.user, exchange, venue_type: b.venue_type, venue: b.venue, when: b.when, symbols, from: b.from, to: b.to,
                              side: b.scope.side, line, trades: b.trades, stamp };
    Ok(Prep::Live(Box::new(LiveRead { prepared, credentials, sensitive, version })))
}

fn same_stamp(a: &str, b: &str) -> bool {
    match (serde_json::from_str::<Value>(a), serde_json::from_str::<Value>(b)) { (Ok(a), Ok(b)) => model::stamp_current_for(&a, &b).is_fresh(), _ => false }
}

/// The line and its mark, in one transaction; false when the obligation already has its line.
fn write_line(c: &Connection, id: i64, bot: i64, line: &Line, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let tx = model::immediate(c)?;
    let wrote = mark(&tx, id, bot, line, now)?;
    tx.commit()?;
    Ok(wrote)
}

fn mark(c: &Connection, id: i64, bot: i64, line: &Line, now: DateTime<Utc>) -> Result<bool, EngineError> {
    if c.execute("UPDATE rust_rails_unrecorded SET reported_at = ?2, outcome = ?3 WHERE id = ?1 AND reported_at IS NULL", params![id, format_time(now), line.outcome])? != 1 { return Ok(false); }
    model::log_activity(c, bot, line.event, Level::Warning, line.details.clone(), now)?;
    Ok(true)
}

/// Phase 3 for one read. A failure is recorded in its own transaction (r2.3) and the line written only once that ends the
/// live part (a refusal, or MAX_ATTEMPTS failures): A28-1's. An answer is written in one transaction fenced on the key it
/// was read with, after the obligation is prepared again exactly as it was (r1.5): anything else writes nothing, and the
/// next pass reads again.
fn settle(c: &Connection, cipher: &crate::crypto::Cipher, read: &LiveRead, result: Result<Created, HistoryError>, now: DateTime<Utc>) -> Result<bool, EngineError> {
    let p = &read.prepared;
    let created = match result {
        Ok(created) => created,
        Err(e) => {
            let refused = matches!(e, HistoryError::Refused(_));
            let tx = model::immediate(c)?;
            let row: Option<(String, i64, Option<String>)> = tx.query_row("SELECT stamp, attempts, refused_at FROM rust_rails_unrecorded_live WHERE obligation_id = ?1", [p.id],
                                                                          |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
            let (attempts, refused_at) = match row { Some((s, a, r)) if same_stamp(&s, &p.stamp) => (a, r), _ => (0, None) };
            let attempts = attempts + i64::from(!refused);
            let refused_at = if refused { Some(format_time(now)) } else { refused_at };
            tx.execute("INSERT INTO rust_rails_unrecorded_live (obligation_id, stamp, attempts, refused_at, last_at) VALUES (?1, ?2, ?3, ?4, ?5) \
                        ON CONFLICT (obligation_id) DO UPDATE SET stamp = excluded.stamp, attempts = excluded.attempts, refused_at = excluded.refused_at, last_at = excluded.last_at",
                       params![p.id, p.stamp, attempts, refused_at, format_time(now)])?;
            tx.commit()?;
            // The line this failure may end the live part with is A28-1's, written only when the obligation is still as it was
            // prepared and under the key it was read with (r1.5); otherwise the next pass decides.
            let written = model::credential_write(c, &Some(read.version.clone()), |tx| {
                match prepare(tx, cipher, p.id, now, &mut Linked::new())? {
                    Prep::Write(bot, line) if bot == p.bot && line == p.line => mark(tx, p.id, bot, &line, now),
                    _ => Ok(false),
                }
            });
            return match written { Err(EngineError::CredentialsChanged) => Ok(false), other => other };
        }
    };
    let written = model::credential_write(c, &Some(read.version.clone()), |tx| {
        let mut linked = Linked::new();
        let again = match prepare(tx, cipher, p.id, now, &mut linked)? { Prep::Live(r) if r.prepared == *p => r.prepared, _ => return Ok(false) };
        // A28-2 §1: a line naming the venue's orders carries its obligation, and its bookable orders are stored with it.
        let (line, offers) = match merged(tx, &again, &created, &mut linked)? {
            Some((mut line, live)) => { line.details["obligation"] = json!(p.id); (line, live) }
            None => (again.line, vec![]),
        };
        if !mark(tx, p.id, p.bot, &line, now)? { return Ok(false); }
        for o in offers.iter().filter(|o| bookable(o)) {
            tx.execute("INSERT INTO rust_rails_unrecorded_offers (obligation_id, external_id, symbol, facts) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
                       params![p.id, o.external_id, o.symbol, facts_json(o).to_string()])?;
        }
        Ok(true)
    });
    match written { Err(EngineError::CredentialsChanged) => Ok(false), other => other }
}

/// r2.1, r3.1, r4.1: the line with the venue's orders on the scope's pairs and side that no bot holds (by external id, or
/// by a fill linked to a bot's order), each reconciled with every imported fill of its key, merged with the ledger's
/// candidates the venue did not list; open orders first. None when the venue lists no such order: A28-1's line stands.
fn merged(c: &Connection, p: &Prepared, created: &Created, linked: &mut Linked) -> Result<Option<(Line, Vec<CreatedOrder>)>, EngineError> {
    let mut live: Vec<&CreatedOrder> = vec![];
    for o in &created.orders {
        if o.side != p.side || !p.symbols.contains(&o.symbol) { continue; }
        let held: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM transactions WHERE exchange_id = ?1 AND external_id = ?2)", params![p.exchange, o.external_id], |r| r.get(0))?;
        if !held { live.push(o); }
    }
    // A linked fill that can't be read can't exclude anything: the order is listed (a line too many, never one too few).
    if !live.is_empty() {
        if let Ok(keys) = linked_keys(c, p.user, p.exchange, &p.venue_type, linked)? { live.retain(|o| !keys.contains(&o.external_id)); }
    }
    let Some(since) = live.iter().map(|o| o.created_at).min() else { return Ok(None) };
    let (sums, clean) = ledger_sums(c, p.user, p.exchange, &p.venue_type, since - Duration::seconds(60))?;
    let mut items: Vec<(bool, DateTime<Utc>, String)> = vec![];
    let mut conflict = false;
    for o in &live {
        let (base, quote): (String, String) = c.query_row("SELECT base, quote FROM tickers WHERE exchange_id = ?1 AND ticker = ?2 LIMIT 1", params![p.exchange, o.symbol],
                                                          |r| Ok((r.get(0)?, r.get(1)?)))?;
        let (text, differs) = describe_live(o, &base, &quote, sums.get(&o.external_id), clean);
        conflict |= differs;
        items.push((o.state.open(), o.created_at, text));
    }
    for t in p.trades.iter().filter(|t| !live.iter().any(|o| o.external_id == t.key)) { items.push((false, t.at, t.describe(c)?)); }
    items.sort_by_key(|(open, at, _)| (!*open, *at));
    let open = items.iter().any(|(open, ..)| *open);
    let mut listed: Vec<String> = items.iter().take(LISTED).map(|(.., text)| text.clone()).collect();
    if items.len() > LISTED { listed.push(format!("and {} more", items.len() - LISTED)); }
    let (event, outcome) = if open { (LIVE_OPEN, "live_open") } else { (LIVE_LISTED, "live_listed") };
    Ok(Some((Line { event, outcome, details: json!({ "when": p.when, "venue": p.venue, "orders": listed.join("; "), "count": items.len(),
                                                      "complete": created.complete, "conflict": conflict }) }, live.into_iter().cloned().collect())))
}

type BigDec = crate::ruby::BigDec;
/// Per order key, the base and quote of its imported fills.
type Sums = std::collections::HashMap<String, (BigDec, BigDec)>;

/// r4.1: per order key, the base and quote of every imported fill from `since` on (each fill once), and whether every row
/// in that range read cleanly.
fn ledger_sums(c: &Connection, user: i64, exchange: i64, venue: &str, since: DateTime<Utc>) -> Result<(Sums, bool), EngineError> {
    use crate::sync::fills::{self, LedgerRow, Read};
    let mut s = c.prepare("SELECT id, tx_id, entry_type, raw_data, transacted_at FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at >= ?3")?;
    let rows = s.query_map(params![user, exchange, format_time(since)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?,
                                                                                 r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    let (mut sums, mut seen, mut clean) = (Sums::new(), std::collections::HashSet::new(), true);
    for (id, tx_id, entry_type, raw, at) in rows {
        let raw: Value = match raw.as_deref().map(serde_json::from_str::<Value>) { Some(Ok(v)) => v, _ => Value::Null };
        let Ok(transacted_at) = parse_time(&at) else { clean = false; continue };
        match fills::read(c, venue, &LedgerRow { id, tx_id, entry_type, raw_data: raw, transacted_at, exchange_id: exchange }).map_err(EngineError::Data)? {
            Read::Fill(f) => {
                if !seen.insert(f.fill_id.clone()) { continue; }
                let sum = sums.entry(f.order_key).or_insert_with(|| (BigDec::zero(), BigDec::zero()));
                *sum = (&sum.0 + &f.base_qty, &sum.1 + &f.quote_qty);
            }
            Read::NotATrade => {}
            Read::Unreadable(_) => clean = false,
        }
    }
    Ok((sums, clean))
}

/// A decimal as the lines write it: "20", "0.0004".
fn num(d: &BigDec) -> String { let s = d.to_s_f(); s.strip_suffix(".0").map(str::to_string).unwrap_or(s) }

/// §2, r3.1, r4.1: one listed order. The fills shown are the larger of the venue's and the ledger's by base (a tie: the
/// venue's); a venue lower than the ledger proves is "at least" the ledger's, with "still open" only for an open order;
/// "nothing filled" only when both say zero and every ledger row read. Returns the text and whether the two differed.
fn describe_live(o: &CreatedOrder, base: &str, quote: &str, ledger: Option<&(BigDec, BigDec)>, clean: bool) -> (String, bool) {
    let zero = (BigDec::zero(), BigDec::zero());
    let (lb, lq) = ledger.unwrap_or(&zero);
    let (verb, past) = match o.side { Side::Buy => ("buy", "bought"), Side::Sell => ("sell", "sold") };
    let size = match &o.size { CreatedSize::Base(x) => format!("{} {base}", num(x)), CreatedSize::Quote(x) => format!("{} {quote}", num(x)) };
    let price = match &o.kind { CreatedKind::Limit { price } => format!(" at {} {quote}", num(price)), CreatedKind::Market => String::new() };
    let head = format!("{verb} of {size}{price}");
    let amounts = |b: &BigDec, q: Option<&BigDec>| match q {
        Some(q) => format!("{} {base} filled for {} {quote}", num(b), num(q)),
        None => format!("{} {base} filled", num(b)),
    };
    let at = o.created_at.format("%Y-%m-%d %H:%M:%S UTC");
    if o.filled_base < *lb {
        let open = if o.state.open() { ", still open" } else { "" };
        return (format!("{head}, at least {}{open}, placed {at}", amounts(lb, Some(lq))), true);
    }
    // The venue's quote, else the ledger's only when both name the same base.
    let fq = o.filled_quote.clone().or_else(|| (o.filled_base == *lb && lb.is_positive()).then(|| lq.clone()));
    let fb = &o.filled_base;
    let text = match o.state {
        CreatedState::Open => format!("{head}, still open"),
        CreatedState::PartlyFilledOpen => format!("{head}, {}, still open", amounts(fb, fq.as_ref())),
        CreatedState::Filled => match &fq {
            Some(q) => format!("{past} {} {base} for {} {quote} (filled)", num(fb), num(q)),
            None => format!("{past} {} {base} (filled)", num(fb)),
        },
        CreatedState::PartlyFilledCancelled => format!("{head} cancelled, {}", amounts(fb, fq.as_ref())),
        CreatedState::Cancelled if lb.is_zero() && clean => format!("{head} cancelled, nothing filled"),
        CreatedState::Cancelled => format!("{head} cancelled"),
    };
    (format!("{text}, placed {at}"), false)
}

// ---- A28-2: booking an order Rails sent unrecorded, on the bot owner's confirmation (PLAN-A28-2 Final shape) ----------------
//
// D-RAILS-UNRECORDED-RECOVERED: the owner names one order a served venue's live line offered; the engine re-reads it by its
// own history read (r1.1) and books it as `persist_accepted_order!` writes a row (r2.2), stamped at Rails' tick anchor; the
// follow-up poll then settles it as every order (FetchAndUpdateOrderJob). One booking per attempt group
// (D-RAILS-UNRECORDED-ONE-PER-ATTEMPT), never while the bot is mid-tick (D-RAILS-UNRECORDED-BOOK-BUSY), never on KuCoin or
// IBKR (D-RAILS-UNRECORDED-NO-ROUTE). A booked row is watched until it settles (D-RAILS-UNRECORDED-WATCHED), never
// abandoned by age (D-RAILS-UNRECORDED-NEVER-STALE) and never lowered below what the owner confirmed
// (D-RAILS-UNRECORDED-SETTLED-LOWER).

/// Activity events (texts: web::i18n `bot_activity.events.<event>`; `rails_order_not_added` by its `reason`).
pub const RECOVERED: &str = "rails_order_recovered";
pub const NOT_ADDED: &str = "rails_order_not_added";
pub const NOT_MINE: &str = "rails_order_not_mine";
pub const UNSETTLED: &str = "rails_order_unsettled";
pub const SETTLED_LOWER: &str = "rails_order_settled_lower";

/// A transient identity read is retried no sooner than this, at most `BOOK_ATTEMPTS` times (r2.1).
const BOOK_RETRY_AFTER: i64 = 60;
const BOOK_ATTEMPTS: i64 = 3;
/// A booked row no poll holds is enrolled again this long after (r3.2).
pub const WATCH_AFTER_US: i64 = 3_600_000_000;
/// An unsettled or held booked row gets its one line after this (r3.2, r5.1).
const NOTICE_DAYS: i64 = 7;

/// Something filled, or still open (§1, Q-CANCELLED): a cancelled order with nothing filled moved no money.
fn bookable(o: &CreatedOrder) -> bool { o.state != CreatedState::Cancelled }

fn state_name(s: CreatedState) -> &'static str {
    match s { CreatedState::Open => "open", CreatedState::PartlyFilledOpen => "partly_filled_open", CreatedState::Filled => "filled",
              CreatedState::Cancelled => "cancelled", CreatedState::PartlyFilledCancelled => "partly_filled_cancelled" }
}

/// The order as an offer stores it (`facts`), every decimal as its text.
fn facts_json(o: &CreatedOrder) -> Value {
    let (size, amount) = match &o.size { CreatedSize::Base(x) => ("base", x), CreatedSize::Quote(x) => ("quote", x) };
    let price = match &o.kind { CreatedKind::Limit { price } => Some(price.to_s_f()), CreatedKind::Market => None };
    json!({ "external_id": o.external_id, "symbol": o.symbol, "created_at": format_time(o.created_at),
            "side": o.side.as_str(), "limit": price, "size": { size: amount.to_s_f() }, "state": state_name(o.state),
            "filled_base": o.filled_base.to_s_f(), "filled_quote": o.filled_quote.as_ref().map(BigDec::to_s_f) })
}

/// `facts_json` read back; None when anything doesn't read.
fn facts_of(v: &Value) -> Option<CreatedOrder> {
    let dec = |v: &Value| v.as_str().and_then(|s| BigDec::parse(s).ok());
    let size = match (v["size"].get("base"), v["size"].get("quote")) { (Some(b), None) => CreatedSize::Base(dec(b)?), (None, Some(q)) => CreatedSize::Quote(dec(q)?), _ => return None };
    let state = ["open", "partly_filled_open", "filled", "cancelled", "partly_filled_cancelled"].iter().zip([CreatedState::Open, CreatedState::PartlyFilledOpen,
        CreatedState::Filled, CreatedState::Cancelled, CreatedState::PartlyFilledCancelled]).find(|(n, _)| v["state"] == **n)?.1;
    Some(CreatedOrder {
        external_id: v["external_id"].as_str()?.into(), symbol: v["symbol"].as_str()?.into(), created_at: match parse_time(v["created_at"].as_str()?) { Ok(t) => t, Err(_) => return None },
        side: match v["side"].as_str()? { "buy" => Side::Buy, "sell" => Side::Sell, _ => return None },
        kind: match &v["limit"] { Value::Null => CreatedKind::Market, p => CreatedKind::Limit { price: dec(p)? } },
        size, state, filled_base: dec(&v["filled_base"])?,
        filled_quote: match &v["filled_quote"] { Value::Null => None, q => Some(dec(q)?) },
    })
}

/// r1.6: the obligations of the bot whose search windows overlap the obligation's, transitively (itself included).
fn group(c: &Connection, obligation: i64) -> Result<Vec<i64>, EngineError> {
    let Some(bot) = c.query_row("SELECT bot_id FROM rust_rails_unrecorded WHERE id = ?1", [obligation], |r| r.get::<_, i64>(0)).optional()? else { return Ok(vec![]) };
    Ok(groups(c, bot)?.of(obligation))
}

/// Every obligation of a bot with its group (r1.6), built once (A28-2b r4-r5: an interval sweep, O(n log n)).
pub(crate) struct Groups { component: std::collections::HashMap<i64, usize>, members: Vec<Vec<i64>> }
impl Groups {
    /// The obligation's group; an id the bot doesn't hold is its own.
    pub(crate) fn of(&self, obligation: i64) -> Vec<i64> { self.component.get(&obligation).map_or_else(|| vec![obligation], |&g| self.members[g].clone()) }
}

pub(crate) fn groups(c: &Connection, bot: i64) -> Result<Groups, EngineError> {
    let mut s = c.prepare("SELECT id, span_from, span_to FROM rust_rails_unrecorded WHERE bot_id = ?1 ORDER BY id")?;
    let rows = s.query_map([bot], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?.collect::<Result<Vec<_>, _>>()?;
    let around = Duration::seconds(AROUND);
    // A window that can't be read joins nothing.
    type Window = Option<(DateTime<Utc>, DateTime<Utc>)>;
    let windows: Vec<(i64, Window)> = rows.iter().map(|(id, f, t)| (*id, match (parse_time(f), parse_time(t)) {
        (Ok(f), Ok(t)) => Some((f - around, t + around)), _ => None })).collect();
    let members = components(&windows);
    let component = members.iter().enumerate().flat_map(|(g, ids)| ids.iter().map(move |id| (*id, g))).collect();
    Ok(Groups { component, members })
}

/// The connected components of "windows (f, t) and (mf, mt) overlap": `f <= mt && mf <= t`, as r1.6's fixpoint read it.
/// Proper windows (f <= t) are swept in order of start: one joins the component before it while its start is at most
/// that component's latest end. An inverted window (f > t, collection accepts `failed_at < claimed_at`) overlaps no
/// other inverted one, and a proper one only when that one covers [t, f]; two such proper windows overlap each other, so
/// it joins at most one component, found by a binary search over the starts and their running maximum end. An unread
/// window is its own.
fn components<T: Ord + Copy>(windows: &[(i64, Option<(T, T)>)]) -> Vec<Vec<i64>> {
    let mut proper: Vec<(T, T, i64)> = windows.iter().filter_map(|(id, w)| w.filter(|(f, t)| f <= t).map(|(f, t)| (f, t, *id))).collect();
    proper.sort_unstable();
    let mut members: Vec<Vec<i64>> = vec![];
    // Per sorted window: its component, and the running maximum end with that end's component.
    let mut of = Vec::with_capacity(proper.len());
    let mut running: Vec<(T, usize)> = Vec::with_capacity(proper.len());
    let mut latest: Option<T> = None;
    for &(f, t, id) in &proper {
        if latest.is_none_or(|end| f > end) { members.push(vec![]); }
        let g = members.len() - 1;
        members[g].push(id);
        of.push(g);
        latest = Some(latest.map_or(t, |end| end.max(t)));
        let best = match running.last() { Some(&(end, at)) if end >= t => (end, at), _ => (t, g) };
        running.push(best);
    }
    for (id, w) in windows {
        match w {
            Some((f, t)) if f > t => {
                let before = proper.partition_point(|(start, _, _)| start <= t);
                match before.checked_sub(1).map(|i| running[i]) {
                    Some((end, g)) if end >= *f => members[g].push(*id),
                    _ => members.push(vec![*id]),
                }
            }
            Some(_) => {}
            None => members.push(vec![*id]),
        }
    }
    members
}

fn any_of(c: &Connection, table: &str, ids: &[i64]) -> Result<bool, EngineError> {
    let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    Ok(c.query_row(&format!("SELECT EXISTS (SELECT 1 FROM {table} WHERE obligation_id IN ({list}))"), [], |r| r.get(0))?)
}

/// Why the web's POST was not taken. Its text is the flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused { NotFound, Decided, Pending }
impl Refused {
    pub fn text(self) -> &'static str {
        match self {
            Self::NotFound => "That order isn't on this bot's list any more.",
            Self::Decided => "This was already decided.",
            Self::Pending => "Deltabadger is already checking an order for this. It's added to the bot's history in a moment if it checks out.",
        }
    }
}

/// The obligation's group, when the obligation is the user's bot's and its line was written; else NotFound. Decided or
/// Pending when the group has a resolution or a waiting request.
fn open_group(c: &Connection, user: i64, bot: i64, obligation: i64) -> Result<Result<Vec<i64>, Refused>, EngineError> {
    let mine: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM rust_rails_unrecorded o JOIN bots b ON b.id = o.bot_id WHERE o.id = ?1 AND o.bot_id = ?2 \
                                  AND b.user_id = ?3 AND o.reported_at IS NOT NULL)", params![obligation, bot, user], |r| r.get(0))?;
    if !mine { return Ok(Err(Refused::NotFound)); }
    let ids = group(c, obligation)?;
    if any_of(c, "rust_rails_unrecorded_resolutions", &ids)? { return Ok(Err(Refused::Decided)); }
    if any_of(c, "rust_rails_unrecorded_requests", &ids)? { return Ok(Err(Refused::Pending)); }
    Ok(Ok(ids))
}

/// POST "This was this bot's order" (r2.1): the request the engine serves (`serve_bookings`). The caller wakes the engine.
pub fn request_booking(c: &Connection, user: i64, bot: i64, obligation: i64, offer: i64, now: DateTime<Utc>) -> Result<Result<(), Refused>, EngineError> {
    let tx = model::immediate(c)?;
    if let Err(r) = open_group(&tx, user, bot, obligation)? { return Ok(Err(r)); }
    let offered: bool = tx.query_row("SELECT EXISTS (SELECT 1 FROM rust_rails_unrecorded_offers WHERE id = ?1 AND obligation_id = ?2)", params![offer, obligation], |r| r.get(0))?;
    if !offered { return Ok(Err(Refused::NotFound)); }
    tx.execute("INSERT INTO rust_rails_unrecorded_requests (obligation_id, offer_id, requested_at, attempts) VALUES (?1, ?2, ?3, 0)", params![obligation, offer, format_time(now)])?;
    tx.commit()?;
    Ok(Ok(()))
}

/// POST "None of these was this bot's" (D-RAILS-UNRECORDED-NOT-MINE): the group resolved `not_mine`, one line, no row.
pub fn dismiss(c: &Connection, user: i64, bot: i64, obligation: i64, now: DateTime<Utc>) -> Result<Result<(), Refused>, EngineError> {
    let tx = model::immediate(c)?;
    let ids = match open_group(&tx, user, bot, obligation)? { Ok(ids) => ids, Err(r) => return Ok(Err(r)) };
    for id in &ids {
        tx.execute("INSERT INTO rust_rails_unrecorded_resolutions (obligation_id, resolution, resolved_at) VALUES (?1, 'not_mine', ?2)", params![id, format_time(now)])?;
    }
    model::log_activity(&tx, bot, NOT_MINE, Level::Info, json!({ "obligation": obligation }), now)?;
    tx.commit()?;
    Ok(Ok(()))
}

/// One offer as the web shows it: its text and the bot's carry now and with it booked (None where it can't be read).
#[derive(Clone, Debug, PartialEq)]
pub struct OfferView { pub id: i64, pub order: String, pub carry_before: Option<BigDec>, pub carry_after: Option<BigDec> }
/// One line's obligation: whether its group is decided or being checked, and its offers.
#[derive(Clone, Debug, PartialEq)]
pub struct ObligationView { pub obligation: i64, pub resolved: bool, pub pending: bool, pub offers: Vec<OfferView> }

/// Read-only: the offers of the bot's lines named by `obligations` (their details' `obligation`), each with the carry
/// computed by the bot's own formula with and without the order as the booking would write it (§1, r1.7).
pub fn offers_for(c: &Connection, bot: i64, obligations: &[i64], now: DateTime<Utc>) -> Result<Vec<ObligationView>, EngineError> {
    let b = model::load_bot(c, bot)?;
    let now_us = now.timestamp_micros();
    let carry = |extra: Option<(&str, &BigDec)>| super::accounting::pending_quote_amount_plus(c, &b, now_us, extra).ok().and_then(|n| n.to_dec().ok());
    let before = carry(None);
    let groups = groups(c, bot)?;
    let mut out = vec![];
    for &id in obligations {
        let Some(o) = load_where(c, id, false)? else { continue };
        if o.bot != bot { continue; }
        let ids = groups.of(id);
        let mut view = ObligationView { obligation: id, resolved: any_of(c, "rust_rails_unrecorded_resolutions", &ids)?,
                                        pending: any_of(c, "rust_rails_unrecorded_requests", &ids)?, offers: vec![] };
        let mut s = c.prepare("SELECT id, facts FROM rust_rails_unrecorded_offers WHERE obligation_id = ?1 ORDER BY id")?;
        let rows = s.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
        for (offer, facts) in rows {
            let Some(f) = serde_json::from_str(&facts).ok().as_ref().and_then(facts_of) else { continue };
            let names: Option<(String, String, i64)> = c.query_row("SELECT base, quote, price_decimals FROM tickers WHERE exchange_id = ?1 AND ticker = ?2 LIMIT 1",
                                                                   params![b.exchange_id, f.symbol], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
            let Some((base, quote, places)) = names else { continue };
            // The price as the booking writes it: to the ticker's price step.
            let after = match (f.side, crate::ruby::scale(places).ok().and_then(|p| waiting_value(&f, p))) {
                (Side::Sell, _) => before.clone(),
                (Side::Buy, Some(v)) => carry(Some((&format_time(o.from), &v))),
                (Side::Buy, None) => None,
            };
            view.offers.push(OfferView { id: offer, order: describe_live(&f, &base, &quote, None, true).0, carry_before: before.clone(), carry_after: after });
        }
        out.push(view);
    }
    Ok(out)
}

/// A28-2b: where an obligation stands, as MCP and REST list it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListedState { Open, Checking, Booked, NotMine }
impl ListedState {
    pub fn as_str(self) -> &'static str { match self { Self::Open => "open", Self::Checking => "checking", Self::Booked => "booked", Self::NotMine => "not_mine" } }
}
/// One answerable obligation (reported, with offers): its group's state, its offers (their carry with the offer booked
/// only while open), the booked row, and the newest refusal line naming its group while open: (reason, venue, at).
#[derive(Clone, Debug, PartialEq)]
pub struct Listed { pub obligation: i64, pub state: ListedState, pub from: String, pub to: String, pub offers: Vec<OfferView>, pub transaction_id: Option<i64>,
                    pub refused: Option<(String, String, String)> }
/// The bot's answerable obligations for the API (PLAN-A28-2b §3): at most LISTED_API, open first (oldest first), then
/// checking, then decided (most recently decided first); `more` counts the rest. `carry` is the bot's carry now.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing { pub carry: Option<BigDec>, pub obligations: Vec<Listed>, pub more: usize }
pub const LISTED_API: usize = 20;
/// The newest activity lines of a bot a refusal is looked for in (ponytail: a window, not a durable refusal column).
const REFUSAL_LINES: usize = 500;

/// The group's resolution, from any member: ('booked' | 'not_mine', the booked row, when).
pub fn decision(c: &Connection, obligation: i64) -> Result<Option<(String, Option<i64>, String)>, EngineError> {
    let ids = group(c, obligation)?;
    decided(c, &ids)
}

fn decided(c: &Connection, ids: &[i64]) -> Result<Option<(String, Option<i64>, String)>, EngineError> {
    if ids.is_empty() { return Ok(None); }
    let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    Ok(c.query_row(&format!("SELECT resolution, transaction_id, resolved_at FROM rust_rails_unrecorded_resolutions WHERE obligation_id IN ({list}) ORDER BY obligation_id LIMIT 1"),
                   [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?)
}

/// Read-only: the bot's listing, or None when it has no answerable obligation.
pub fn listing(c: &Connection, bot: i64, now: DateTime<Utc>) -> Result<Option<Listing>, EngineError> {
    let eligible: Vec<(i64, String, String)> = {
        let mut s = c.prepare("SELECT o.id, o.span_from, o.span_to FROM rust_rails_unrecorded o WHERE o.bot_id = ?1 AND o.reported_at IS NOT NULL \
                               AND EXISTS (SELECT 1 FROM rust_rails_unrecorded_offers f WHERE f.obligation_id = o.id) ORDER BY o.id")?;
        let rows = s.query_map([bot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    if eligible.is_empty() { return Ok(None); }
    let groups = groups(c, bot)?;
    let mut all = vec![];
    for (id, from, to) in eligible {
        let ids = groups.of(id);
        let (state, transaction_id, at) = match decided(c, &ids)? {
            Some((r, tx, at)) => (if r == "booked" { ListedState::Booked } else { ListedState::NotMine }, tx, at),
            None if any_of(c, "rust_rails_unrecorded_requests", &ids)? => (ListedState::Checking, None, String::new()),
            None => (ListedState::Open, None, String::new()),
        };
        all.push((Listed { obligation: id, state, from, to, offers: vec![], transaction_id, refused: None }, at, ids));
    }
    let rank = |s: ListedState| match s { ListedState::Open => 0, ListedState::Checking => 1, _ => 2 };
    all.sort_by(|(a, at_a, _), (b, at_b, _)| rank(a.state).cmp(&rank(b.state)).then_with(|| match a.state {
        ListedState::Open | ListedState::Checking => a.obligation.cmp(&b.obligation),
        _ => at_b.cmp(at_a).then(b.obligation.cmp(&a.obligation)),
    }));
    let more = all.len().saturating_sub(LISTED_API);
    all.truncate(LISTED_API);
    let ids: Vec<i64> = all.iter().map(|(l, _, _)| l.obligation).collect();
    let mut views: std::collections::HashMap<i64, ObligationView> = offers_for(c, bot, &ids, now)?.into_iter().map(|v| (v.obligation, v)).collect();
    // The newest refusal line naming each open group, within the bot's newest REFUSAL_LINES lines (the index on
    // (bot_id, created_at) serves the order; a detail over 16 KiB is never loaded).
    let lines: Vec<(String, Option<String>, String)> = {
        let mut s = c.prepare(&format!("SELECT event, CASE WHEN octet_length(details) <= 16384 THEN details END, created_at FROM bot_activity_logs \
                                        WHERE bot_id = ?1 ORDER BY created_at DESC, id DESC LIMIT {REFUSAL_LINES}"))?;
        let rows = s.query_map([bot], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    let refusals: Vec<(i64, String, String, String)> = lines.into_iter().filter(|(event, _, _)| event == NOT_ADDED).filter_map(|(_, details, at)| {
        let d: Value = serde_json::from_str(&details?).ok()?;
        Some((d["obligation"].as_i64()?, d["reason"].as_str()?.to_string(), d["venue"].as_str().unwrap_or_default().to_string(), at))
    }).collect();
    let before = super::accounting::pending_quote_amount_plus(c, &model::load_bot(c, bot)?, now.timestamp_micros(), None).ok().and_then(|n| n.to_dec().ok());
    let obligations = all.into_iter().map(|(mut l, _, ids)| {
        let open = l.state == ListedState::Open;
        l.offers = views.remove(&l.obligation).map(|v| v.offers).unwrap_or_default();
        if !open { for o in &mut l.offers { o.carry_after = None; } }
        if open { l.refused = refusals.iter().find(|(o, ..)| ids.contains(o)).map(|(_, reason, venue, at)| (reason.clone(), venue.clone(), at.clone())); }
        l
    }).collect();
    Ok(Some(Listing { carry: before, obligations, more }))
}

/// What the booked row counts in the carry until its poll (#invested_quote): its quote, or amount × price, price being the
/// limit or a market order's gross average. None when the order doesn't say (refused at booking).
fn waiting_value(f: &CreatedOrder, places: u8) -> Option<BigDec> {
    match &f.size {
        CreatedSize::Quote(q) => Some(q.clone()),
        CreatedSize::Base(a) => Some(a * &row_price(f, places)?),
    }
}

/// r2.2: the limit price, or a market order's gross average fill (`filled_quote / filled_base`, to the ticker's price step).
fn row_price(f: &CreatedOrder, places: u8) -> Option<BigDec> {
    match &f.kind {
        CreatedKind::Limit { price } => Some(price.clone()),
        CreatedKind::Market => {
            let avg = f.filled_quote.as_ref()?.div(&f.filled_base).filter(|_| f.filled_base.is_positive())?;
            Some(avg.round(places))
        }
    }
}

/// What a pass of `serve_bookings` leaves the engine: the rows it booked (polled now, announced) and the booked rows to
/// enrol again (polled in an hour), each (bot, transaction).
#[derive(Debug, Default, PartialEq)]
pub struct Served { pub booked: Vec<(i64, i64)>, pub watch: Vec<(i64, i64)> }

/// The engine's pass (r2.1, r3.2): each due request re-checked and booked or refused, then the booked rows' watch.
/// `polled` says whether a row already has a poll owed.
pub async fn serve_bookings<F: crate::venue::VenueFactory>(c: &Connection, factory: &F, cipher: &crate::crypto::Cipher, polled: &dyn Fn(i64) -> bool,
                                                           now: DateTime<Utc>) -> Result<Served, EngineError> {
    let mut served = Served::default();
    let due: Vec<i64> = {
        let mut s = c.prepare("SELECT obligation_id FROM rust_rails_unrecorded_requests WHERE last_at IS NULL OR last_at <= ?1 ORDER BY requested_at, obligation_id")?;
        let ids = s.query_map([format_time(now - Duration::seconds(BOOK_RETRY_AFTER))], |r| r.get(0))?.collect::<Result<_, _>>()?;
        ids
    };
    for obligation in due {
        match book(c, factory, cipher, obligation, now).await {
            Ok(Some(row)) => served.booked.push(row),
            Ok(None) => {}
            Err(e @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(e),
            // One request that can't be served never holds the others or the rows already booked (Codex branch r1).
            Err(e) => {
                super::log(&format!("[engine] Rails-unrecorded booking request {obligation} refused: {}", e.kind()));
                let bot: Option<i64> = c.query_row("SELECT bot_id FROM rust_rails_unrecorded WHERE id = ?1", [obligation], |r| r.get(0)).optional()?;
                match bot { Some(bot) => { refuse(c, obligation, bot, "unreadable", None, now)?; } None => { drop_request(c, obligation)?; } }
            }
        }
    }
    // A row booked now is polled now, not in an hour.
    match watch(c, &|tx| polled(tx) || served.booked.iter().any(|(_, b)| *b == tx), now) {
        Ok(watch) => served.watch = watch,
        Err(e @ (EngineError::Lease(_) | EngineError::Store(_))) => return Err(e),
        Err(e) => super::log(&format!("[engine] booked Rails-unrecorded orders not watched this pass: {}", e.kind())),
    }
    Ok(served)
}

/// A request read for booking.
struct Booking { obligation: i64, offer: i64, attempts: i64, o: Obligation, facts: CreatedOrder }

async fn book<F: crate::venue::VenueFactory>(c: &Connection, factory: &F, cipher: &crate::crypto::Cipher, obligation: i64, now: DateTime<Utc>) -> Result<Option<(i64, i64)>, EngineError> {
    let req: Option<(i64, i64, Option<String>)> = c.query_row(
        "SELECT r.offer_id, r.attempts, (SELECT facts FROM rust_rails_unrecorded_offers WHERE id = r.offer_id AND obligation_id = r.obligation_id) \
         FROM rust_rails_unrecorded_requests r WHERE r.obligation_id = ?1", [obligation], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).optional()?;
    let Some((offer, attempts, facts)) = req else { return Ok(None) };
    let o = match load_where(c, obligation, false) { Ok(Some(o)) => o, Ok(None) | Err(EngineError::Data(_)) => return drop_request(c, obligation), Err(e) => return Err(e) };
    let Some(facts) = facts.and_then(|f| serde_json::from_str(&f).ok()).as_ref().and_then(facts_of) else { return refuse(c, obligation, o.bot, "unreadable", None, now) };
    let b = Booking { obligation, offer, attempts, o, facts };
    let bot = match model::load_bot(c, b.o.bot) { Ok(bot) => bot, Err(EngineError::Data(_)) => return refuse(c, obligation, b.o.bot, "unreadable", Some(&b.facts), now), Err(e) => return Err(e) };
    let refused = |reason| refuse(c, obligation, bot.id, reason, Some(&b.facts), now);
    // §2.1: read-side checks.
    if bot.status == BotStatus::Deleted { return refused("deleted"); }
    let venue_type = model::exchange_type(c, &bot)?;
    // D-RAILS-UNRECORDED-NO-ROUTE: KuCoin and IBKR are never served here, whatever SERVED holds.
    if !SERVED.contains(&venue_type.as_str()) || [crate::venue::kucoin::KUCOIN, crate::venue::ibkr::IBKR].contains(&venue_type.as_str()) { return refused("unsupported"); }
    if scope_now(c, &b.o, &bot)?.is_none_or(|s| s.exchange != bot.exchange_id) { return refused("settings_changed"); }
    let (venue, version) = super::run::venue_for(c, factory, cipher, &bot)?;
    let Some(version) = version else { return refused("key_changed") };
    let updated: Option<String> = c.query_row("SELECT updated_at FROM api_keys WHERE id = ?1", [version.id], |r| r.get(0))?;
    if !matches!(updated.as_deref().map(parse_time), Some(Ok(u)) if u + Duration::seconds(1) < b.o.from - Duration::seconds(AROUND)) { return refused("key_changed"); }
    // r1.1: the identity read, by the history read's own contract.
    let at = b.facts.created_at;
    let read = venue.orders_created(std::slice::from_ref(&b.facts.symbol), at - Duration::seconds(1), at + Duration::seconds(1), tokio::time::Instant::now() + READ_TIMEOUT).await;
    let created = match read {
        Ok(created) => created,
        Err(HistoryError::Refused(_)) => return refused("refused"),
        Err(HistoryError::Retry(_)) => {
            if b.attempts + 1 >= BOOK_ATTEMPTS { return refused("unreachable"); }
            c.execute("UPDATE rust_rails_unrecorded_requests SET attempts = attempts + 1, last_at = ?2 WHERE obligation_id = ?1", params![obligation, format_time(now)])?;
            return Ok(None);
        }
    };
    let same: Vec<&CreatedOrder> = created.orders.iter().filter(|o| o.external_id == b.facts.external_id && o.symbol == b.facts.symbol && o.side == b.facts.side
                                                                  && o.created_at == b.facts.created_at).collect();
    let [fresh] = same[..] else { return refused("not_found") };
    // r1.2: nothing filled, or a fill whose cost the venue doesn't state, can't be booked.
    if fresh.state == CreatedState::Cancelled { return refused("nothing_filled"); }
    if (fresh.kind == CreatedKind::Market && !fresh.filled_base.is_positive()) || (fresh.filled_base.is_positive() && fresh.filled_quote.is_none()) { return refused("no_cost"); }
    let tx = model::immediate(c)?;
    let written = match model::check_credential_result(&tx, &Some(version)) {
        Ok(fenced) => write_booking(&fenced, cipher, &b, &bot, &venue_type, fresh, now)?,
        Err(EngineError::CredentialsChanged) => Err("key_changed"),
        Err(e) => return Err(e),
    };
    match written {
        Ok(row) => { tx.commit()?; Ok(Some((bot.id, row))) }
        Err("gone") => Ok(None),
        Err(reason) => { drop(tx); refused(reason) }
    }
}

/// The request alone goes (its obligation is gone).
fn drop_request(c: &Connection, obligation: i64) -> Result<Option<(i64, i64)>, EngineError> {
    c.execute("DELETE FROM rust_rails_unrecorded_requests WHERE obligation_id = ?1", [obligation])?;
    Ok(None)
}

/// A refusal (r2.1): the request goes and the bot gets one `rails_order_not_added` line saying why; nothing else is written.
fn refuse(c: &Connection, obligation: i64, bot: i64, reason: &str, facts: Option<&CreatedOrder>, now: DateTime<Utc>) -> Result<Option<(i64, i64)>, EngineError> {
    let tx = model::immediate(c)?;
    if tx.execute("DELETE FROM rust_rails_unrecorded_requests WHERE obligation_id = ?1", [obligation])? == 1 {
        let venue: String = tx.query_row("SELECT coalesce(e.name, 'the exchange') FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", [bot], |r| r.get(0))
            .optional()?.unwrap_or_else(|| "the exchange".into());
        let placed = facts.map(|f| f.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string());
        model::log_activity(&tx, bot, NOT_ADDED, Level::Warning, json!({ "reason": reason, "venue": venue, "placed": placed, "obligation": obligation }), now)?;
    }
    tx.commit()?;
    Ok(None)
}

/// The obligation's pairs and side now, re-derived as `judge` derives them (§2.3); None when they can't be told.
fn scope_now(c: &Connection, o: &Obligation, bot: &model::Bot) -> Result<Option<Scope>, EngineError> {
    match &o.scope {
        Some(s) if s.get("time_error").is_some() => Ok(None),
        Some(s) => Ok(stored_scope(s)),
        None if bot.bot_type == "Bots::DcaIndex" => Ok(None),
        None => current_scope(c, o, bot.exchange_id),
    }
}

/// §2.3 + r2.1 step 3 + r2.3, in the one fenced transaction: every re-check, then Rails' accepted row (r2.2), the fills
/// linked, the group resolved, the request gone, the line, the history generation; readability and the eligibility guard
/// last. Err(reason) means the caller rolls everything back; "gone" is a request another pass already served.
fn write_booking(f: &model::FencedTransaction<'_>, cipher: &crate::crypto::Cipher, b: &Booking, bot_then: &model::Bot, venue_type: &str, fresh: &CreatedOrder,
                 now: DateTime<Utc>) -> Result<Result<i64, &'static str>, EngineError> {
    let c: &Connection = f;
    let still: Option<i64> = c.query_row("SELECT offer_id FROM rust_rails_unrecorded_requests WHERE obligation_id = ?1", [b.obligation], |r| r.get(0)).optional()?;
    if still != Some(b.offer) { return Ok(Err("gone")); }
    let ids = group(c, b.obligation)?;
    if any_of(c, "rust_rails_unrecorded_resolutions", &ids)? { return Ok(Err("resolved")); }
    let bot = model::load_bot(c, bot_then.id)?;
    if bot.status == BotStatus::Deleted { return Ok(Err("deleted")); }
    // §5: a tick that claimed the bot sized on the carry this would change.
    if bot.status == BotStatus::Executing || bot.rust_placement().is_some() { return Ok(Err("busy")); }
    let Some(scope) = scope_now(c, &b.o, &bot)?.filter(|s| s.exchange == bot.exchange_id && s.side == fresh.side) else { return Ok(Err("settings_changed")) };
    let exchange = bot.exchange_id;
    let tickers: Vec<(i64, i64, i64)> = {
        let mut s = c.prepare("SELECT id, base_asset_id, quote_asset_id FROM tickers WHERE exchange_id = ?1 AND ticker = ?2 LIMIT 2")?;
        let rows = s.query_map(params![exchange, fresh.symbol], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    let [(ticker_id, base, quote)] = tickers[..] else { return Ok(Err("settings_changed")) };
    if !scope.bases.contains(&base) || quote != scope.quote { return Ok(Err("settings_changed")); }
    // A failed row's stored scope is Rails' at T: the pair must also be in the bot's settings now (Codex branch r1).
    let mut now_bases = bot.asset_ids();
    if let Some(b) = bot.settings.get("base_asset_id").and_then(Value::as_i64) { now_bases.push(b); }
    if !now_bases.contains(&base) || bot.quote_asset_id() != Some(quote) { return Ok(Err("settings_changed")); }
    let held: bool = c.query_row("SELECT EXISTS (SELECT 1 FROM transactions WHERE exchange_id = ?1 AND external_id = ?2)", params![exchange, fresh.external_id], |r| r.get(0))?;
    if held { return Ok(Err("held")); }
    match linked_keys(c, bot.user_id, exchange, venue_type, &mut Linked::new())? {
        Err(()) => return Ok(Err("unreadable")),
        Ok(keys) if keys.contains(&fresh.external_id) => return Ok(Err("linked")),
        Ok(_) => {}
    }
    // r2.3: never less than the ledger proves, on both totals.
    let since = fresh.created_at - Duration::seconds(60);
    let (sums, clean) = ledger_sums(c, bot.user_id, exchange, venue_type, since)?;
    if !clean { return Ok(Err("ledger")); }
    let zero = BigDec::zero();
    if let Some((lb, lq)) = sums.get(&fresh.external_id) {
        if *lb > fresh.filled_base || *lq > *fresh.filled_quote.as_ref().unwrap_or(&zero) { return Ok(Err("ledger")); }
    }
    let Some(ticker) = model::ticker_by_id(c, exchange, ticker_id)? else { return Ok(Err("settings_changed")) };
    let Ok((_, _, places)) = ticker.scales() else { return Ok(Err("unreadable")) };
    let Some(price) = row_price(fresh, places) else { return Ok(Err("no_cost")) };
    let (amount, quote_amount, in_quote) = match &fresh.size { CreatedSize::Base(a) => (Some(a.clone()), None, false), CreatedSize::Quote(q) => (None, Some(q.clone()), true) };
    let plan = amount::OrderPlan { ticker, side: fresh.side, limit: matches!(fresh.kind, CreatedKind::Limit { .. }), price: price.clone(),
                                   amount: amount.clone().unwrap_or_else(BigDec::zero), quote_amount: quote_amount.clone().unwrap_or_else(BigDec::zero),
                                   quote_type: in_quote, volume: amount.clone().or(quote_amount.clone()).unwrap_or_else(BigDec::zero), kind: amount::TxKind::Regular,
                                   origin: if in_quote { amount::Origin::QuoteBudget } else { amount::Origin::Base }, wire_price: None };
    let row = amount::write_order_row(f, &bot, &plan, amount::RowKind::Submitted { external_id: fresh.external_id.clone() }, b.o.from)?;
    // r1.2 / r2.2: the order's own denomination only; Rails' BUY schedule columns for every side (base_order_values), or
    // the failed Rails row's own stamp (D-RAILS-UNRECORDED-FAILED-ROW-STAMP).
    let (interval, stamped): (Option<String>, Option<f64>) = if b.o.source == FAILED_ROW {
        c.query_row("SELECT bot_interval, bot_quote_amount FROM transactions WHERE id = ?1", [b.o.source_ref], |r| Ok((r.get(0)?, r.get(1)?)))?
    } else {
        (bot.interval().map(|i| i.as_str().to_string()), Some(crate::ruby::to_sql(&bot.quote_amount_num()?.to_dec().map_err(|e| EngineError::Data(format!("{e:?}")))?)))
    };
    c.execute("UPDATE transactions SET amount = ?2, quote_amount = ?3, price = ?4, bot_interval = ?5, bot_quote_amount = ?6 WHERE id = ?1",
              params![row, amount.as_ref().map(crate::ruby::to_sql), quote_amount.as_ref().map(crate::ruby::to_sql), crate::ruby::to_sql(&price), interval.unwrap_or_default(), stamped])?;
    link_fills(c, bot.user_id, exchange, venue_type, &fresh.external_id, since, row)?;
    for id in &ids {
        c.execute("INSERT INTO rust_rails_unrecorded_resolutions (obligation_id, resolution, offer_id, transaction_id, resolved_at, confirmed_base, confirmed_quote) \
                   VALUES (?1, 'booked', ?2, ?3, ?4, ?5, ?6)",
                  params![id, b.offer, row, format_time(now), fresh.filled_base.to_s_f(), fresh.filled_quote.as_ref().map(BigDec::to_s_f)])?;
        c.execute("DELETE FROM rust_rails_unrecorded_requests WHERE obligation_id = ?1", [id])?;
    }
    let (base_name, quote_name): (String, String) = c.query_row("SELECT base, quote FROM tickers WHERE id = ?1", [ticker_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
    model::log_activity(c, bot.id, RECOVERED, Level::Info, json!({ "obligation": b.obligation, "transaction_id": row,
        "venue_created_at": format_time(fresh.created_at), "order": describe_live(fresh, &base_name, &quote_name, None, true).0 }), now)?;
    // The bot's history changed: an operation sized from the old one is fenced (placement::begin_checked), as the CSV import.
    let key = format!("$.{}", model::HISTORY_GENERATION);
    c.execute("UPDATE bots SET transient_data = json_set(transient_data, ?2, coalesce(json_extract(transient_data, ?2), 0) + 1) WHERE id = ?1", params![bot.id, key])?;
    // R-IMPORT-READABLE: the row as every money reader reads it, then the install's guard.
    let readable = crate::figures::db::orders(c, bot.id).ok().and_then(|orders| orders.into_iter().find(|o| o.id == row))
        .is_some_and(|o| crate::figures::fill::for_engine(&o).is_ok());
    if !readable || super::eligibility::guard(c, cipher, Some(bot.id)).is_err() { return Ok(Err("unreadable")); }
    Ok(Ok(row))
}

/// Every unlinked imported fill of the order from `since` on, linked to the booked row (§3).
fn link_fills(c: &Connection, user: i64, exchange: i64, venue: &str, key: &str, since: DateTime<Utc>, row: i64) -> Result<(), EngineError> {
    use crate::sync::fills::{self, LedgerRow, Read};
    let mut s = c.prepare("SELECT id, tx_id, entry_type, raw_data, transacted_at FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at >= ?3 \
                           AND transaction_id IS NULL")?;
    let rows = s.query_map(params![user, exchange, format_time(since)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?,
                                                                                 r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for (id, tx_id, entry_type, raw, at) in rows {
        let raw: Value = match raw.as_deref().map(serde_json::from_str::<Value>) { Some(Ok(v)) => v, _ => Value::Null };
        let Ok(transacted_at) = parse_time(&at) else { continue };
        if let Read::Fill(fill) = fills::read(c, venue, &LedgerRow { id, tx_id, entry_type, raw_data: raw, transacted_at, exchange_id: exchange }).map_err(EngineError::Data)? {
            if fill.order_key == key { c.execute("UPDATE account_transactions SET transaction_id = ?2 WHERE id = ?1 AND transaction_id IS NULL", params![id, row])?; }
        }
    }
    Ok(())
}

/// A booked row's offer facts, its venue's name, the totals its owner confirmed and when.
struct Booked { facts: CreatedOrder, venue: String, base: Option<String>, quote: Option<String>, resolved: String }

/// The booked row's `Booked` (for the watch's lines and the r5.1 guard).
fn booked_facts(c: &Connection, tx: i64) -> Result<Option<Booked>, EngineError> {
    let row: Option<(String, Option<String>, Option<String>, String)> = c.query_row(
        "SELECT o.facts, r.confirmed_base, r.confirmed_quote, r.resolved_at FROM rust_rails_unrecorded_resolutions r JOIN rust_rails_unrecorded_offers o ON o.id = r.offer_id \
         WHERE r.transaction_id = ?1 AND r.resolution = 'booked' LIMIT 1", [tx], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    let Some((facts, base, quote, resolved)) = row else { return Ok(None) };
    let Some(facts) = serde_json::from_str(&facts).ok().as_ref().and_then(facts_of) else { return Ok(None) };
    let venue: String = c.query_row("SELECT coalesce(e.name, 'the exchange') FROM transactions t LEFT JOIN exchanges e ON e.id = t.exchange_id WHERE t.id = ?1", [tx], |r| r.get(0))?;
    Ok(Some(Booked { facts, venue, base, quote, resolved }))
}

/// r3.2: the booked rows still unknown or open, of bots not deleted: those no poll holds are enrolled again (in an hour);
/// an unknown one 7 days after booking gets its one `rails_order_unsettled` line.
fn watch(c: &Connection, polled: &dyn Fn(i64) -> bool, now: DateTime<Utc>) -> Result<Vec<(i64, i64)>, EngineError> {
    let rows: Vec<(i64, i64, i64)> = {
        let mut s = c.prepare("SELECT t.id, t.bot_id, t.external_status FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE t.status = 0 AND t.external_status IN (0, 1) \
                               AND b.status <> ?1 AND t.id IN (SELECT transaction_id FROM rust_rails_unrecorded_resolutions WHERE resolution = 'booked') ORDER BY t.id")?;
        let rows = s.query_map([BotStatus::Deleted as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
        rows
    };
    let mut out = vec![];
    for (tx, bot, status) in rows {
        if !polled(tx) { out.push((bot, tx)); }
        if status != UNKNOWN_STATUS { continue; }
        let Some(Booked { facts, venue, resolved, .. }) = booked_facts(c, tx)? else { continue };
        if !matches!(parse_time(&resolved), Ok(at) if at + Duration::days(NOTICE_DAYS) <= now) { continue; }
        notice_once(c, tx, bot, "unsettled", UNSETTLED, &facts, &venue, now)?;
    }
    Ok(out)
}
const UNKNOWN_STATUS: i64 = 0;

/// One line per row and kind, ever (the notices table), in its own transaction unless one is open.
#[allow(clippy::too_many_arguments)]
fn notice_once(c: &Connection, tx: i64, bot: i64, kind: &str, event: &str, facts: &CreatedOrder, venue: &str, now: DateTime<Utc>) -> Result<(), EngineError> {
    let own = c.is_autocommit().then(|| model::immediate(c)).transpose()?;
    if c.execute("INSERT INTO rust_rails_unrecorded_notices (transaction_id, kind, at) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING", params![tx, kind, format_time(now)])? == 1 {
        model::log_activity(c, bot, event, Level::Warning, json!({ "transaction_id": tx, "venue": venue,
            "placed": facts.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string() }), now)?;
    }
    if let Some(own) = own { own.commit()?; }
    Ok(())
}

/// r4.1 (D-RAILS-UNRECORDED-NEVER-STALE): is this row a booked one?
pub(crate) fn booked(c: &Connection, tx: i64) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT EXISTS (SELECT 1 FROM rust_rails_unrecorded_resolutions WHERE transaction_id = ?1 AND resolution = 'booked')", [tx], |r| r.get(0))?)
}

/// r5.1 (D-RAILS-UNRECORDED-SETTLED-LOWER), inside the poll's transaction: true when the answer would lower a booked row
/// below the gross totals the owner confirmed, net of the venue's taker fee (percent): it is then not applied, and the
/// hold is noted; after 7 days held, one `rails_order_settled_lower` line. A consistent answer ends the hold.
pub(crate) fn holds(c: &Connection, bot: i64, tx: i64, s: &crate::venue::OrderState, now: DateTime<Utc>) -> Result<bool, EngineError> {
    use crate::venue::OrderStatus;
    if matches!(s.status, OrderStatus::Unknown | OrderStatus::Failed) { return Ok(false); }
    let Some(Booked { facts, venue, base, quote, .. }) = booked_facts(c, tx)? else { return Ok(false) };
    let fee: Option<String> = c.query_row("SELECT CAST(e.taker_fee AS TEXT) FROM transactions t JOIN exchanges e ON e.id = t.exchange_id WHERE t.id = ?1", [tx], |r| r.get(0)).optional()?.flatten();
    let fee = fee.and_then(|f| BigDec::parse(&f).ok()).unwrap_or_else(BigDec::zero);
    let keep = &BigDec::from_i64(1) - &fee.div(&BigDec::from_i64(100)).unwrap_or_else(BigDec::zero);
    let below = |confirmed: Option<String>, answer: Option<&BigDec>| match (confirmed.and_then(|t| BigDec::parse(&t).ok()), answer) {
        (Some(confirmed), Some(answer)) => *answer < &confirmed * &keep,
        _ => false,
    };
    let lower = below(base, Some(&s.amount_exec)) || (!s.sell && below(quote, s.quote_amount_exec.as_ref()));
    if !lower {
        c.execute("DELETE FROM rust_rails_unrecorded_notices WHERE transaction_id = ?1 AND kind = 'held'", [tx])?;
        return Ok(false);
    }
    super::log(&format!("[engine] bot {bot}: {venue} answered transaction {tx} with less filled than its owner confirmed; the row waits"));
    c.execute("INSERT INTO rust_rails_unrecorded_notices (transaction_id, kind, at) VALUES (?1, 'held', ?2) ON CONFLICT DO NOTHING", params![tx, format_time(now)])?;
    let since: String = c.query_row("SELECT at FROM rust_rails_unrecorded_notices WHERE transaction_id = ?1 AND kind = 'held'", [tx], |r| r.get(0))?;
    if matches!(parse_time(&since), Ok(at) if at + Duration::days(NOTICE_DAYS) <= now) { notice_once(c, tx, bot, "lower", SETTLED_LOWER, &facts, &venue, now)?; }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::components;

    /// r1.6's fixpoint as A28-2a wrote it: the oracle `components` must equal.
    fn fixpoint(windows: &[(i64, Option<(i64, i64)>)], obligation: i64) -> Vec<i64> {
        let mut members = vec![obligation];
        loop {
            let before = members.len();
            for (id, w) in windows {
                if members.contains(id) { continue; }
                let joins = w.is_some_and(|(f, t)| windows.iter().any(|(m, mw)| members.contains(m) && mw.is_some_and(|(mf, mt)| f <= mt && mf <= t)));
                if joins { members.push(*id); }
            }
            if members.len() == before { members.sort_unstable(); return members; }
        }
    }

    fn same(windows: &[(i64, Option<(i64, i64)>)]) {
        let comps = components(windows);
        assert_eq!(comps.iter().map(Vec::len).sum::<usize>(), windows.len(), "each window in exactly one component: {comps:?}");
        for (id, _) in windows {
            let mut got = comps.iter().find(|c| c.contains(id)).unwrap().clone();
            got.sort_unstable();
            assert_eq!(got, fixpoint(windows, *id), "obligation {id} in {windows:?}");
        }
    }

    #[test]
    fn the_sweep_groups_as_the_fixpoint_did() {
        // Disjoint; touching ends; a chain listed against the order it is found in; unread windows.
        same(&[(1, Some((0, 10))), (2, Some((20, 30))), (3, Some((40, 50)))]);
        same(&[(1, Some((0, 10))), (2, Some((10, 20))), (3, Some((21, 30)))]);
        same(&[(1, Some((40, 50))), (2, Some((30, 41))), (3, Some((20, 31))), (4, Some((10, 21))), (5, Some((0, 11)))]);
        same(&[(1, None), (2, Some((0, 10))), (3, None), (4, Some((5, 15)))]);
        // Inverted windows: apart from a proper one that only overlaps their reach, joined to one covering them, never
        // to each other, and never a bridge.
        same(&[(1, Some((10, 40))), (2, Some((30, 0)))]);
        same(&[(1, Some((0, 50))), (2, Some((30, 20)))]);
        same(&[(1, Some((30, 20))), (2, Some((35, 25)))]);
        same(&[(1, Some((0, 25))), (2, Some((24, 50))), (3, Some((26, 23)))]);
        same(&[(1, Some((0, 10))), (2, Some((40, 60))), (3, Some((50, 5))), (4, Some((0, 100)))]);
        // A seeded pseudo-random set.
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: u64| { x ^= x << 13; x ^= x >> 7; x ^= x << 17; (x % n) as i64 };
        for _ in 0..20 {
            let windows: Vec<_> = (0..60).map(|id| (id, if next(10) == 0 { None } else { let f = next(1000); Some((f, f + next(80) - 15)) })).collect();
            same(&windows);
        }
    }

    #[test]
    fn the_sweep_is_bounded_on_many_inverted_windows() {
        let windows: Vec<_> = (0..10_000i64).map(|i| (i, Some(if i % 2 == 0 { (i * 10, i * 10 + 5) } else { (i * 10 + 5, i * 10) }))).collect();
        let started = std::time::Instant::now();
        let comps = components(&windows);
        assert_eq!(comps.len(), 10_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "{:?}", started.elapsed());
    }
}
