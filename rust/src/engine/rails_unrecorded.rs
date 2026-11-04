//! A28-1 (PLAN-A28 r2-r5): the orders Rails may have sent but never recorded (Rails' KNOWN GAP, action_job.rb:167-183),
//! reported to the bot's owner with what the imported ledger shows. Nothing here writes a `transactions` row, moves a
//! carry or calls a venue (D-RAILS-UNRECORDED-REPORTED).
//!
//! `collect` runs in a takeover's claim transaction when the install comes from Rails, and records one obligation per
//! source Rails wrote while it owned the install (r4.1, r5.1). `report` (the hourly job) writes each obligation's one line
//! once the ledger can say what it holds, or says why it can't (r3.4).

use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use super::model::{self, Level};
use crate::enums::BotStatus;
use super::EngineError;
use crate::codec::{format_time, parse_time};
use crate::venue::Side;

pub const JOB: &str = "rails_unrecorded_report";
/// Activity events (texts: web::i18n `bot_activity.events.<event>`; `rails_order_unchecked` by its `reason`).
pub const CANDIDATES: &str = "rails_order_candidates";
pub const NONE_SEEN: &str = "rails_order_none_seen";
pub const UNCHECKED: &str = "rails_order_unchecked";

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
    for id in open {
        let tx = model::immediate(c)?;
        let o = match load(&tx, id) {
            Ok(Some(o)) => o,
            Ok(None) => continue,
            // A row whose stored times can't be read is answered as such, so it never stops the obligations after it.
            Err(EngineError::Data(e)) => {
                let bot: i64 = tx.query_row("SELECT bot_id FROM rust_rails_unrecorded WHERE id = ?1", [id], |r| r.get(0))?;
                let line = Line { event: UNCHECKED, outcome: "time", details: json!({ "when": "at a time that can't be read", "venue": "exchange", "reason": "time", "error": e }) };
                model::log_activity(&tx, bot, line.event, Level::Warning, line.details, now)?;
                tx.execute("UPDATE rust_rails_unrecorded SET reported_at = ?2, outcome = ?3 WHERE id = ?1 AND reported_at IS NULL", params![id, format_time(now), line.outcome])?;
                tx.commit()?;
                written += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        let Some(line) = judge(&tx, &o, now)? else { continue };
        model::log_activity(&tx, o.bot, line.event, Level::Warning, line.details, now)?;
        tx.execute("UPDATE rust_rails_unrecorded SET reported_at = ?2, outcome = ?3 WHERE id = ?1 AND reported_at IS NULL", params![id, format_time(now), line.outcome])?;
        tx.commit()?;
        written += 1;
    }
    Ok(written)
}

struct Obligation { bot: i64, source: String, from: DateTime<Utc>, to: DateTime<Utc>, scope: Option<Value>, collected: DateTime<Utc> }

fn load(c: &Connection, id: i64) -> Result<Option<Obligation>, EngineError> {
    let row = c.query_row("SELECT bot_id, source, span_from, span_to, scope, collected_at FROM rust_rails_unrecorded WHERE id = ?1 AND reported_at IS NULL", [id],
                          |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?, r.get::<_, String>(5)?)))
        .optional()?;
    let Some((bot, source, from, to, scope, collected)) = row else { return Ok(None) };
    let t = |s: &str| parse_time(s).map_err(|e| EngineError::Data(format!("rust_rails_unrecorded {id}: {e:?}")));
    Ok(Some(Obligation { bot, source, from: t(&from)?, to: t(&to)?, scope: scope.and_then(|s| serde_json::from_str(&s).ok()), collected: t(&collected)? }))
}

struct Line { event: &'static str, outcome: &'static str, details: Value }

/// What the obligation's line says, or None while the ledger may still bring its answer.
fn judge(c: &Connection, o: &Obligation, now: DateTime<Utc>) -> Result<Option<Line>, EngineError> {
    let when = when(o);
    let bot: Option<(i64, i64, String, Option<String>, String, i64)> = c.query_row(
        "SELECT b.user_id, b.exchange_id, b.type, e.type, e.name, b.status FROM bots b LEFT JOIN exchanges e ON e.id = b.exchange_id WHERE b.id = ?1", [o.bot],
        |r| Ok((r.get::<_, Option<i64>>(0)?.unwrap_or(0), r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get(2)?, r.get(3)?,
                r.get::<_, Option<String>>(4)?.unwrap_or_default(), r.get(5)?))).optional()?;
    let Some((user, bot_exchange, bot_type, _, _, status)) = bot else { return Ok(None) };
    // The venue a line names when the scope is unknown: the exchange of the bot's order nearest the span, never today's
    // exchange of a bot that moved (r3.2); "exchange" when it has none.
    let then = || -> Result<String, EngineError> {
        let nearest: Option<i64> = c.query_row("SELECT exchange_id FROM transactions WHERE bot_id = ?1 ORDER BY abs(julianday(created_at) - julianday(?2)) LIMIT 1",
                                               params![o.bot, format_time(o.from)], |r| r.get(0)).optional()?;
        match nearest { Some(e) => venue_name(c, e).map(|v| if v.is_empty() { "exchange".into() } else { v }), None => Ok("exchange".into()) }
    };
    if status == BotStatus::Deleted as i64 { return Ok(Some(unchecked(&when, "exchange", "gone"))); }
    if o.scope.as_ref().is_some_and(|s| s.get("time_error").is_some()) {
        return Ok(Some(Line { event: UNCHECKED, outcome: "time", details: json!({ "when": "at a time that can't be read", "venue": then()?, "reason": "time" }) }));
    }
    // The pairs and side to look at, only where they are known (r3.2, r4.2).
    let scope = match &o.scope {
        Some(s) => {
            let get = |k: &str| s.get(k).and_then(Value::as_i64);
            match (get("exchange_id"), get("base_asset_id"), get("quote_asset_id"), get("side")) {
                (Some(e), Some(b), Some(q), Some(side)) => Some(Scope { exchange: e, bases: vec![b], quote: q, side: if side == 1 { Side::Sell } else { Side::Buy } }),
                _ => None,
            }
        }
        None if bot_type == "Bots::DcaIndex" => return Ok(Some(unchecked(&when, &then()?, "index"))),
        None => current_scope(c, o, bot_exchange)?,
    };
    let Some(scope) = scope else { return Ok(Some(unchecked(&when, &then()?, "settings_changed"))) };
    let (venue_type, venue) = c.query_row("SELECT type, name FROM exchanges WHERE id = ?1", [scope.exchange], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    if !crate::sync::fills::supported(&venue_type) { return Ok(Some(unchecked(&when, &venue, "no_history"))); }
    let search_from = o.from - Duration::seconds(AROUND);
    let search_to = o.to + Duration::seconds(AROUND);
    // The ledger has the window when a sync of the user's reading key on that venue reached the venue's end after it.
    let readable = match reading_key(c, user, scope.exchange)? {
        Some(key) => {
            let state = crate::jobs::state::read(c, crate::sync::jobs::LEDGER_SYNC, Some(&key.to_string())).map_err(EngineError::Data)?;
            state.incomplete_since.is_none() && state.last_success_at.is_some_and(|t| t >= search_to + Duration::seconds(SETTLE))
        }
        None => false,
    };
    if !readable {
        return Ok((now >= o.collected + Duration::days(GIVE_UP_DAYS)).then(|| unchecked(&when, &venue, "unread")));
    }
    let trades = match trades(c, user, scope.exchange, &venue_type, &scope, search_from, search_to)? {
        Ok(t) => t,
        Err(()) => return Ok(Some(unchecked(&when, &venue, "unreadable"))),
    };
    if trades.is_empty() {
        if SHORT_REACH.contains(&venue_type.as_str()) { return Ok(Some(unchecked(&when, &venue, "reach"))); }
        return Ok(Some(Line { event: NONE_SEEN, outcome: "none_seen", details: json!({ "when": when, "venue": venue }) }));
    }
    let mut listed: Vec<String> = trades.iter().take(LISTED).map(|t| t.describe(c)).collect::<Result<_, _>>()?;
    if trades.len() > LISTED { listed.push(format!("and {} more", trades.len() - LISTED)); }
    Ok(Some(Line { event: CANDIDATES, outcome: "candidates", details: json!({ "when": when, "venue": venue, "trades": listed.join("; "), "count": trades.len() }) }))
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

struct Scope { exchange: i64, bases: Vec<i64>, quote: i64, side: Side }

/// A DCA bot's pairs and side today, when nothing since the span can have changed them (r3.2): no settings change, no
/// (re)start, no merge or split, and every later order of the bot on this exchange, on these pairs and on this side.
/// The pairs come from the stored settings by asset id, delisted ones included (r4.2).
fn current_scope(c: &Connection, o: &Obligation, exchange: i64) -> Result<Option<Scope>, EngineError> {
    let bot = model::load_bot(c, o.bot)?;
    // From the span's start: a change while Rails was placing leaves the scope unknown too.
    let after = format_time(o.from);
    let changed: bool = c.query_row("SELECT (settings_changed_at IS NOT NULL AND settings_changed_at > ?2) OR (started_at IS NOT NULL AND started_at > ?2) FROM bots WHERE id = ?1",
                                    params![o.bot, after], |r| r.get(0))?;
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

/// One venue order no bot recorded, with its fills inside the window.
struct Trade { side: Side, ticker: i64, base: crate::ruby::BigDec, quote: crate::ruby::BigDec, at: DateTime<Utc> }

impl Trade {
    fn describe(&self, c: &Connection) -> Result<String, EngineError> {
        let (b, q): (String, String) = c.query_row("SELECT base, quote FROM tickers WHERE id = ?1", [self.ticker], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let verb = match self.side { Side::Buy => "bought", Side::Sell => "sold" };
        Ok(format!("{verb} {} {b} for {} {q} on {} (filled)", self.base.to_s_f(), self.quote.to_s_f(), self.at.format("%Y-%m-%d %H:%M:%S UTC")))
    }
}

/// The trades on the scope's pairs and side inside the window whose order no bot holds and none of whose fills is linked
/// to a bot's order (r2.1, r3.6). Err(()) when a ledger row inside the window can't be read (r3.6, r4.3).
fn trades(c: &Connection, user: i64, exchange: i64, venue: &str, scope: &Scope, from: DateTime<Utc>, to: DateTime<Utc>) -> Result<Result<Vec<Trade>, ()>, EngineError> {
    use crate::sync::fills::{self, LedgerRow, Read};
    let mut s = c.prepare("SELECT id, tx_id, entry_type, raw_data, transacted_at, transaction_id FROM account_transactions \
                           WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at >= ?3 AND transacted_at <= ?4 ORDER BY transacted_at, id")?;
    let rows = s.query_map(params![user, exchange, format_time(from), format_time(to)], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?,
                                                                                       r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?, r.get::<_, Option<i64>>(5)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(s);
    let mut seen: Vec<String> = vec![];
    let mut linked: Vec<String> = vec![];
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
        if link.is_some() && !linked.contains(&fill.order_key) { linked.push(fill.order_key.clone()); }
        if seen.contains(&fill.fill_id) { continue; }
        seen.push(fill.fill_id.clone());
        if fill.side != scope.side || fill.quote_asset_id != scope.quote || !scope.bases.contains(&fill.base_asset_id) { continue; }
        match orders.iter_mut().find(|(k, _)| *k == fill.order_key) {
            Some((_, t)) => {
                t.base = &t.base + &fill.base_qty;
                t.quote = &t.quote + &fill.quote_qty;
                t.at = t.at.min(fill.at);
            }
            None => orders.push((fill.order_key.clone(), Trade { side: fill.side, ticker: fill.ticker_id, base: fill.base_qty, quote: fill.quote_qty, at: fill.at })),
        }
    }
    // A fill of the order linked to a bot's order anywhere in the history, not only in the window, excludes it.
    if !orders.is_empty() {
        let mut s = c.prepare("SELECT id, tx_id, entry_type, raw_data, transacted_at FROM account_transactions \
                               WHERE user_id = ?1 AND exchange_id = ?2 AND transaction_id IS NOT NULL")?;
        let rows = s.query_map(params![user, exchange], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, i64>(2)?,
                                                                 r.get::<_, Option<String>>(3)?, r.get::<_, String>(4)?)))?.collect::<Result<Vec<_>, _>>()?;
        drop(s);
        for (id, tx_id, entry_type, raw, at) in rows {
            let raw: Value = match raw.as_deref().map(serde_json::from_str::<Value>) { Some(Ok(v)) => v, _ => Value::Null };
            // A linked row that can't be read may be a fill of a listed order: then no list can be trusted (unchecked).
            let Ok(transacted_at) = parse_time(&at) else { return Ok(Err(())) };
            match fills::read(c, venue, &LedgerRow { id, tx_id, entry_type, raw_data: raw, transacted_at, exchange_id: exchange }).map_err(EngineError::Data)? {
                Read::Fill(f) => if !linked.contains(&f.order_key) { linked.push(f.order_key); },
                Read::NotATrade => {}
                Read::Unreadable(_) => return Ok(Err(())),
            }
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

/// The report as a scheduler job: hourly, and at the first run after a start (r5.2). Its state is the table.
pub struct Report;

impl crate::jobs::Job for Report {
    fn spec(&self) -> crate::jobs::Spec {
        crate::jobs::Spec { name: JOB, scope: None, schedule: Some(crate::jobs::schedule::Schedule::EveryHours { every: 1, minute: 41 }),
                            jitter: crate::jobs::schedule::Jitter::NONE, retry: crate::jobs::Retry::None, deadline: crate::jobs::DEADLINE }
    }
    fn run<'a>(&'a self, cx: crate::jobs::Cx<'a>, _wakes: Vec<crate::jobs::Wake>) -> crate::jobs::JobFuture<'a> {
        Box::pin(async move {
            use crate::jobs::Outcome;
            let now = cx.clock.now();
            match cx.db.run(move |c, _| report(c, now).map_err(|e| format!("{e:?}"))).await {
                Ok(0) => Outcome::NothingNew,
                Ok(_) => Outcome::Done,
                Err(e) => Outcome::Failed(e),
            }
        })
    }
}
