//! PLAN-A9 O3: an order sent through MCP or the REST API naming a signal bot (D-ORDER-QUEUED). The web validates it, writes
//! one `rust_api_orders` row (`queued`) and waits; the engine takes it at the start of a pass and between bots, places it
//! through the bot (engine::signal::run_api_order) and answers it (`done`, `outcome`). Rails places it in the request
//! under the exchange's lease (BotApi::Orders::BotOrder); here a request never calls a venue, and a crash between send and
//! record is the intent's recovery's, as for a webhook call.
//!
//! The states: `queued` (the web's) → `taken` (this engine's; nothing sent) → `sending` (consumed with the order's intent,
//! placement::begin_signal) → `done`; or `queued` → `withdrawn` (the web gave up first; exactly one of take and withdraw
//! wins). The web deletes a `done` row once it has read it; what nobody read is swept after an hour.
//!
//! PLAN-A9 O4: a standalone order (no bot) goes the same way: the web writes it `queued` with the account's venue and
//! pair, the engine takes it, prepares and sizes it (`place_standalone`), claims it (`sending`, the last write before the
//! request) and sends it once. A placed standalone row is never deleted (D-ORDER-STANDALONE-LOG): its route is what an API
//! cancel by exchange id sends on. ponytail: kept forever, one small row per accepted API order; prune by age if it matters.
use super::model::{self, CredentialVersion};
use super::EngineError;
use crate::codec::{format_time, parse_time};
use crate::ruby::BigDec;
use crate::venue::Side;
use chrono::{DateTime, Duration, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// BotOrder::LEASE_WAIT: a request no pass took within this is withdrawn by the web (`venue_busy`, nothing placed).
pub const WINDOW_SECONDS: i64 = 10;
/// The last instant after `created_at` an order request may leave (PLAN-A9 r1 §2): past it, `busy`, nothing sent.
pub const SEND_SECONDS: i64 = 60;
/// How long one drain may keep taking requests, so polls, recovery and ticks still run under sustained API traffic.
pub const DRAIN: std::time::Duration = std::time::Duration::from_secs(10);
/// How long an answered or withdrawn row nobody read stays.
const KEEP_HOURS: i64 = 1;

/// What the engine answers a request with (bot_api::orders maps each to Rails' result).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    /// The venue holds the order; its row (None only if recovery recorded it first) and the venue's id.
    /// `pair`: the bot's pair as placed (its asset symbols), which a stop, edit and start may have changed since validation.
    Submitted { transaction_id: Option<i64>, order_id: String, #[serde(default)] pair: Option<String> },
    /// Below the venue's minimum: a skipped row.
    Skipped,
    /// An earlier order of the bot is unresolved: nothing sized or sent.
    Unresolved,
    /// A wash-sale lock refused the buy: nothing recorded.
    Locked,
    /// The request may have gone out: the marker and a placement_ambiguous line, no row.
    Ambiguous { errors: Vec<String> },
    /// Nothing reached the venue, or it refused: a failed row.
    Failed { errors: Vec<String> },
    /// BotOrder's own refusal on the bot as the engine found it (its code and text), or the market gate's.
    Refused { code: String, message: String },
    /// An API buy the spending cap does not cover: what it leaves, in quote.
    CapReached { left: String },
    /// Nothing was placed: taken too late, or past its send deadline.
    Busy,
    /// A crash left it mid-flight; nobody waits for it.
    Unknown,
    /// A standalone order the venue holds: its id (PLAN-A9 O4).
    Placed { order_id: String },
}

/// A request as the engine took it.
#[derive(Debug, Clone)]
pub struct Request {
    pub id: i64,
    pub user_id: i64,
    /// None for a standalone order (then `standalone` is set).
    pub bot_id: Option<i64>,
    pub standalone: Option<Standalone>,
    pub side: Side,
    pub amount: BigDec,
    pub in_base: bool,
    /// The caller's exchange_name/base_asset/quote_asset, null where left out (BotOrder#pair_mismatch).
    pub supplied: serde_json::Map<String, Value>,
    pub credential_version: Option<CredentialVersion>,
    pub created_at: DateTime<Utc>,
    pub send_by: DateTime<Utc>,
}

/// A standalone order's account venue and pair as validation found them (PLAN-A9 O4).
#[derive(Debug, Clone)]
pub struct Standalone { pub exchange_id: i64, pub ticker_id: i64, pub limit: bool, pub price: Option<BigDec>, pub mcp: bool }

/// What the web writes: the order as the caller sized it and what validation saw; through a bot, or on the account.
pub enum New<'a> {
    Bot { user_id: i64, bot: &'a model::Bot, side: Side, amount: &'a BigDec, in_base: bool, supplied: &'a serde_json::Map<String, Value> },
    /// `supplied`: the pair as the caller named it (base_asset, quote_asset, upcased), for a late `pair_not_found`.
    Standalone { user_id: i64, side: Side, amount: &'a BigDec, in_base: bool, supplied: &'a serde_json::Map<String, Value>, standalone: Standalone },
}

/// The request, with the credential version of the key as validation saw it (in the same database job): the bot's, or
/// the account's trading key on that venue (`Bot::key_holder`).
pub fn insert(c: &Connection, new: &New<'_>, now: DateTime<Utc>) -> Result<i64, EngineError> {
    let send_by = now.checked_add_signed(Duration::seconds(SEND_SECONDS)).ok_or_else(super::schedule::time_range_error)?;
    let persisted = |a: &BigDec| a.to_persisted().map_err(|e| EngineError::Data(format!("api order amount {e:?}")));
    let (user_id, side, amount, in_base, supplied) = match new {
        New::Bot { user_id, side, amount, in_base, supplied, .. } | New::Standalone { user_id, side, amount, in_base, supplied, .. } => (*user_id, *side, *amount, *in_base, *supplied),
    };
    let (bot_id, version, standalone) = match new {
        New::Bot { bot, .. } => (Some(bot.id), model::credential_version(c, bot)?, None),
        New::Standalone { user_id, standalone, .. } => (None, model::credential_version(c, &model::Bot::key_holder(*user_id, standalone.exchange_id))?, Some(standalone)),
    };
    atomically(c, || {
    c.execute("INSERT INTO rust_api_orders (user_id, bot_id, side, amount, amount_type, supplied, credential_version, state, created_at, send_by) \
               VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9)",
              params![user_id, bot_id, side.as_str(), persisted(amount)?, if in_base { "base" } else { "quote" }, Value::Object(supplied.clone()).to_string(),
                      version.map(|v| v.cache_stamp().to_string()), format_time(now), format_time(send_by)])?;
    let id = c.last_insert_rowid();
    if let Some(s) = standalone {
        c.execute("INSERT INTO rust_api_order_standalone (order_id, exchange_id, ticker_id, order_type, price, surface) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                  params![id, s.exchange_id, s.ticker_id, if s.limit { "limit" } else { "market" }, s.price.as_ref().map(persisted).transpose()?, if s.mcp { "mcp" } else { "rest" }])?;
    }
    Ok(id)
    })
}

/// The web's read while it waits: the state, and the outcome once `done`.
pub fn state(c: &Connection, id: i64) -> Result<Option<(String, Option<Outcome>)>, EngineError> {
    let row: Option<(String, Option<String>)> = c.query_row("SELECT state, outcome FROM rust_api_orders WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    row.map(|(state, outcome)| Ok((state, outcome.map(|o| serde_json::from_str(&o).map_err(|e| EngineError::Data(format!("api order outcome: {e}")))).transpose()?))).transpose()
}

/// The web gave up on a request no pass took: true when it was still queued (nothing will be sent).
pub fn withdraw(c: &Connection, id: i64) -> Result<bool, EngineError> {
    Ok(c.execute("UPDATE rust_api_orders SET state = 'withdrawn' WHERE id = ?1 AND state = 'queued'", [id])? == 1)
}

/// `f` as one unit, inside the caller's transaction or on its own (a savepoint nests in either): a request row and its
/// standalone side row are never seen apart (Codex O4 review).
fn atomically<T>(c: &Connection, f: impl FnOnce() -> Result<T, EngineError>) -> Result<T, EngineError> {
    c.execute_batch("SAVEPOINT api_order")?;
    match f() {
        Ok(v) => { c.execute_batch("RELEASE api_order")?; Ok(v) }
        Err(e) => { let _ = c.execute_batch("ROLLBACK TO api_order; RELEASE api_order"); Err(e) }
    }
}

/// A placed standalone order: kept as the log an API cancel reads (D-ORDER-STANDALONE-LOG).
const RETAINED: &str = "(bot_id IS NULL AND state = 'done' AND json_extract(outcome, '$.kind') = 'placed')";

/// A standalone side row whose request row is gone.
const ORPHANS: &str = "DELETE FROM rust_api_order_standalone WHERE order_id NOT IN (SELECT id FROM rust_api_orders)";

/// The web read the answer. A placed standalone order stays (RETAINED).
pub fn forget(c: &Connection, id: i64) -> Result<(), EngineError> {
    if c.execute(&format!("DELETE FROM rust_api_orders WHERE id = ?1 AND state IN ('done', 'withdrawn') AND NOT {RETAINED}"), [id])? == 1 {
        c.execute("DELETE FROM rust_api_order_standalone WHERE order_id = ?1", [id])?;
    }
    Ok(())
}

/// The retained standalone order a user placed on `exchange_id` with the venue's id `external_id` (the latest): the
/// venue's placement identity at its claim and its wire order (`claim`).
pub fn log_order(c: &Connection, user_id: i64, exchange_id: i64, external_id: &str) -> Result<Option<(Option<Value>, Value)>, EngineError> {
    let row: Option<(Option<String>, Option<String>)> = c.query_row(&format!(
        "SELECT s.identity, s.order_snapshot FROM rust_api_order_standalone s JOIN rust_api_orders o ON o.id = s.order_id \
         WHERE o.user_id = ?1 AND s.exchange_id = ?2 AND s.external_id = ?3 AND {RETAINED} ORDER BY o.id DESC LIMIT 1"),
        params![user_id, exchange_id, external_id], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
    let Some((identity, snapshot)) = row else { return Ok(None) };
    let parse = |t: &str| serde_json::from_str::<Value>(t).map_err(|e| EngineError::Data(format!("api order log: {e}")));
    Ok(Some((identity.as_deref().map(parse).transpose()?, parse(snapshot.as_deref().unwrap_or("null"))?)))
}

/// Every queued request, oldest first.
pub fn queued(c: &Connection) -> Result<Vec<i64>, EngineError> {
    let mut s = c.prepare("SELECT id FROM rust_api_orders WHERE state = 'queued' ORDER BY id")?;
    let ids = s.query_map([], |r| r.get(0))?.collect::<Result<Vec<i64>, _>>()?;
    Ok(ids)
}

/// Takes a queued request (exactly one of this and `withdraw` wins).
pub fn take(c: &Connection, id: i64, now: DateTime<Utc>) -> Result<Option<Request>, EngineError> {
    if c.execute("UPDATE rust_api_orders SET state = 'taken', taken_at = ?2 WHERE id = ?1 AND state = 'queued'", params![id, format_time(now)])? != 1 { return Ok(None); }
    type Row = (i64, Option<i64>, String, String, String, String, Option<String>, String, String);
    let (user_id, bot_id, side, amount, amount_type, supplied, version, created_at, send_by): Row = c.query_row(
        "SELECT user_id, bot_id, side, amount, amount_type, supplied, credential_version, created_at, send_by FROM rust_api_orders WHERE id = ?1", [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?)))?;
    let bad = |what: &str| EngineError::Data(format!("api order {id}: {what}"));
    let time = |t: &str| parse_time(t).map_err(|_| bad("time"));
    let standalone = if bot_id.is_some() { None } else {
        type Own = (Option<i64>, Option<i64>, String, Option<String>, Option<String>);
        let (exchange_id, ticker_id, order_type, price, surface): Own = c.query_row(
            "SELECT exchange_id, ticker_id, order_type, price, surface FROM rust_api_order_standalone WHERE order_id = ?1", [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?.ok_or_else(|| bad("no bot and no standalone row"))?;
        Some(Standalone { exchange_id: exchange_id.ok_or_else(|| bad("no bot and no exchange"))?, ticker_id: ticker_id.ok_or_else(|| bad("no ticker"))?,
                          limit: order_type == "limit", price: price.map(|p| BigDec::parse(&p).map_err(|_| bad("price"))).transpose()?,
                          mcp: surface.as_deref() == Some("mcp") })
    };
    Ok(Some(Request {
        id, user_id, bot_id, standalone,
        side: match side.as_str() { "buy" => Side::Buy, "sell" => Side::Sell, _ => return Err(bad("side")) },
        amount: BigDec::parse(&amount).map_err(|_| bad("amount"))?, in_base: amount_type == "base",
        supplied: serde_json::from_str(&supplied).map_err(|_| bad("supplied"))?,
        credential_version: version.map(|v| serde_json::from_str(&v).ok().and_then(|v| CredentialVersion::from_stamp(&v)).ok_or_else(|| bad("credential version"))).transpose()?,
        created_at: time(&created_at)?, send_by: time(&send_by)?,
    }))
}

/// Whether this engine still holds the request, nothing sent (placement::begin_signal, in the claim's transaction).
pub fn still_taken(c: &Connection, id: i64) -> Result<bool, EngineError> {
    Ok(c.query_row("SELECT state FROM rust_api_orders WHERE id = ?1", [id], |r| r.get::<_, String>(0)).optional()?.as_deref() == Some("taken"))
}

/// Consumed with the order's intent, in its transaction.
pub fn sending(c: &Connection, id: i64, cl_ord_id: &str) -> Result<(), EngineError> {
    c.execute("UPDATE rust_api_orders SET state = 'sending', cl_ord_id = ?2 WHERE id = ?1 AND state = 'taken'", params![id, cl_ord_id])?;
    Ok(())
}

/// A standalone order's claim (PLAN-A9 O4 §2.3 step 6), in the caller's transaction: still taken, it becomes `sending`
/// with what it will send. The last write before the request: `sending` always means "may have gone out".
pub fn claim(c: &Connection, id: i64, cl_ord_id: &str, identity: Option<&Value>, snapshot: &Value) -> Result<bool, EngineError> {
    if c.execute("UPDATE rust_api_orders SET state = 'sending', cl_ord_id = ?2 WHERE id = ?1 AND state = 'taken'", params![id, cl_ord_id])? != 1 { return Ok(false) }
    c.execute("UPDATE rust_api_order_standalone SET identity = ?2, order_snapshot = ?3 WHERE order_id = ?1", params![id, identity.map(Value::to_string), snapshot.to_string()])?;
    Ok(true)
}

/// The answer, in the transaction of whatever the order wrote (its row, its marker), or its own. A placed standalone
/// order keeps the venue's id beside it (the log `log_order` reads).
pub fn finish(c: &Connection, id: i64, outcome: &Outcome) -> Result<(), EngineError> {
    let text = serde_json::to_string(outcome).map_err(|e| EngineError::Data(format!("api order outcome: {e}")))?;
    atomically(c, || {
        let done = c.execute("UPDATE rust_api_orders SET state = 'done', outcome = ?2 WHERE id = ?1 AND state IN ('taken', 'sending')", params![id, text])?;
        if let (1, Outcome::Placed { order_id }) = (done, outcome) {
            c.execute("UPDATE rust_api_order_standalone SET external_id = ?2 WHERE order_id = ?1", params![id, order_id])?;
        }
        Ok(())
    })?;
    Ok(())
}

/// At engine start, for the requests written before `built` (when this engine was built, before the web was attached):
/// the process that waited for these died with the last one. A queued request is withdrawn, a taken one (nothing sent)
/// answered `busy`, a sending one (its intent is recovery's) `unknown`; answered rows older than an hour go.
pub fn sweep_at_start(c: &Connection, built: DateTime<Utc>) -> Result<(), EngineError> {
    let before = format_time(built);
    let busy = serde_json::to_string(&Outcome::Busy).map_err(|e| EngineError::Data(e.to_string()))?;
    let unknown = serde_json::to_string(&Outcome::Unknown).map_err(|e| EngineError::Data(e.to_string()))?;
    model::locked(c, |tx| {
        tx.execute("UPDATE rust_api_orders SET state = 'withdrawn' WHERE state = 'queued' AND created_at < ?1", [&before])?;
        tx.execute("UPDATE rust_api_orders SET state = 'done', outcome = ?1 WHERE state = 'taken' AND created_at < ?2", [&busy, &before])?;
        let mut s = tx.prepare("SELECT id, bot_id, cl_ord_id FROM rust_api_orders WHERE state = 'sending' AND created_at < ?1")?;
        for row in s.query_map([&before], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<String>>(2)?)))? {
            let (id, bot, cl_ord_id) = row?;
            match bot {
                Some(bot) => super::log(&format!("[engine] api order {id} (bot {bot}, cl_ord_id {cl_ord_id:?}) was mid-flight at a restart; its intent's recovery settles it")),
                // §2.5: no intent recovers it, and it is never sent again; the venue's history shows what happened.
                None => super::log(&format!("[engine] standalone api order {id} (cl_ord_id {cl_ord_id:?}) was mid-flight at a restart; never sent again, its outcome is unknown")),
            }
        }
        drop(s);
        tx.execute("UPDATE rust_api_orders SET state = 'done', outcome = ?1 WHERE state = 'sending' AND created_at < ?2", [&unknown, &before])?;
        let before = built.checked_sub_signed(Duration::hours(KEEP_HOURS)).ok_or_else(super::schedule::time_range_error)?;
        tx.execute(&format!("DELETE FROM rust_api_orders WHERE state IN ('done', 'withdrawn') AND created_at < ?1 AND NOT {RETAINED}"), [format_time(before)])?;
        tx.execute(ORPHANS, [])?;
        Ok(())
    })
}

/// PLAN-A9 O4 §2.3: one taken standalone request, in the engine, nothing sent until its claim. `venue`: the account's
/// trading-key venue on the request's exchange (run::venue_for_key), whose producer the caller checked against the
/// request's credential version. Every path answers the request.
pub async fn place_standalone<V: crate::venue::Venue>(c: &Connection, venue: &V, request: &Request, clock: &dyn super::Clock, cx: &super::tick::TickContext<'_>) -> Result<(), EngineError> {
    use super::amount::{self, MarketWire};
    use crate::venue::Venue;
    use super::placement::{self, Sent};
    let s = request.standalone.as_ref().ok_or_else(|| EngineError::Data(format!("api order {}: not standalone", request.id)))?;
    let handle = crate::venue::Handle::new(venue, cx.credential_version.clone());
    let venue = &handle;
    let answer = |outcome: Outcome| finish(c, request.id, &outcome);
    let failed = |m: String| answer(Outcome::Failed { errors: vec![m] });
    let key_missing = || -> Result<(), EngineError> {
        let exchange: String = c.query_row("SELECT coalesce(name, '') FROM exchanges WHERE id = ?1", [s.exchange_id], |r| r.get(0)).optional()?.unwrap_or_default();
        answer(Outcome::Refused { code: "api_key_missing".into(), message: format!("No valid API key found for {exchange}. Please add an API key in Settings.") })
    };
    // 1. Taken past its send deadline: nothing is sent.
    if clock.now() > request.send_by { return answer(Outcome::Busy); }
    // 3. The venue's context (KuCoin's account mode), while still taken; r1 §5: a key replaced meanwhile is the request's own refusal.
    match placement::prepare(c, venue).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return failed(e.message()),
        Err(EngineError::CredentialsChanged) => return key_missing(),
        Err(e) => return Err(e),
    }
    let Some(ticker) = model::ticker_by_id(c, s.exchange_id, s.ticker_id)? else { return late(c, request, clock) };
    // 4. Sizing, as Rails' market_* / place_limit_order: no minimum, no balance.
    let rules = venue.rules();
    let in_base = request.in_base;
    let plan = if s.limit {
        let price = s.price.as_ref().ok_or_else(|| EngineError::Data(format!("api order {}: a limit order without a price", request.id)))?;
        amount::limit_order(&ticker, rules, request.side, &request.amount, !in_base, price)
    } else {
        let touch = match amount::market_wire(rules, request.side, in_base) {
            MarketWire::Refused(text) => return failed(text.into()),
            MarketWire::Direct => None,
            MarketWire::Priced => {
                let holder = model::Bot::key_holder(request.user_id, s.exchange_id);
                match super::signal::retrying(venue, || super::tick::market_price_read(venue, &holder, &ticker, request.side, clock, cx)).await? {
                    Ok(p) if p.is_positive() => Some(p),
                    Ok(_) => return failed("divided by 0".into()),
                    Err(e) => return failed(super::signal::message(e)),
                }
            }
        };
        amount::market_order(&ticker, rules, request.side, &request.amount, in_base, touch.as_ref())
    };
    let plan = match plan { Ok(plan) => plan, Err(m) => return failed(m) };
    // 5. The prepared context must still be the one this order is sent under.
    let identity = venue.placement_identity();
    if let Err(e) = venue.validate_preparation(identity.as_ref()).await { return failed(e.message()); }
    // 6. The claim: still taken, inside the send deadline, the key validation saw, and the request's own late refusals.
    let cl_ord_id = uuid::Uuid::new_v4().to_string();
    let order = plan.to_order(cl_ord_id.clone(), request.send_by, rules.wire).map_err(EngineError::Data)?;
    let snapshot = serde_json::json!({ "symbol": order.pair, "side": order.side.as_str(), "limit": plan.limit, "kind": format!("{:?}", order.kind),
                                       "volume": order.volume, "quote": order.quote_volume });
    let (claimed, claimed_at) = {
        let tx = model::immediate(c)?;
        // Read under the write lock (Codex O4 review): the claim's time is the time it holds it.
        let claimed_at = clock.now();
        if !still_taken(&tx, request.id)? { return Ok(()); }
        if claimed_at > request.send_by { finish(&tx, request.id, &Outcome::Busy)?; tx.commit()?; return Ok(()); }
        match model::check_credential_result(&tx, &cx.credential_version) {
            Ok(_) => {}
            Err(EngineError::CredentialsChanged) => { drop(tx); return key_missing(); }
            Err(e) => return Err(e),
        }
        if let Some(res) = crate::web::bot_api::orders::standalone_late_refusal(&tx, claimed_at, request).map_err(|e| EngineError::Data(format!("{e:?}")))? {
            finish(&tx, request.id, &Outcome::Refused { code: res.code.unwrap_or("order_failed").into(), message: res.message })?;
            tx.commit()?;
            return Ok(());
        }
        let claimed = claim(&tx, request.id, &cl_ord_id, identity.as_ref(), &snapshot)?;
        tx.commit()?;
        (claimed, claimed_at)
    };
    if !claimed { return Ok(()); }
    // 7. One send (Rails has no -1021 retry here), bounded; nothing leaves after `send_until` (r2). The commit may have
    // waited: past `send_until` now, nothing is sent and nothing was placed (send_to_venue's own fence on the bot path).
    let window = claimed_at.checked_add_signed(Duration::seconds(placement::SEND_WINDOW_SECONDS)).ok_or_else(super::schedule::time_range_error)?;
    let send_until = window.min(request.send_by);
    if clock.now() > send_until { return answer(Outcome::Busy); }
    let sent = tokio::time::timeout(crate::venue::http::TOTAL_TIMEOUT, placement::dispatch(venue, &plan, cl_ord_id, send_until, false)).await;
    // 8. The answer.
    answer(match sent {
        Ok(Sent::Accepted(order_id)) => Outcome::Placed { order_id },
        Ok(Sent::Rejected(errors) | Sent::BelowMinimum(errors)) => Outcome::Failed { errors },
        Ok(Sent::NotSent(m)) => Outcome::Failed { errors: vec![m] },
        Ok(Sent::Ambiguous(m)) => Outcome::Ambiguous { errors: vec![m] },
        Ok(Sent::Fenced) => Outcome::Failed { errors: vec!["the order was not sent".into()] },
        Err(_) => Outcome::Ambiguous { errors: vec!["the exchange did not answer in time".into()] },
    })
}

/// The pair is gone since validation: the request's late refusal names why (`pair_not_found`, as Lookup.find_ticker).
fn late(c: &Connection, request: &Request, clock: &dyn super::Clock) -> Result<(), EngineError> {
    let res = crate::web::bot_api::orders::standalone_late_refusal(c, clock.now(), request).map_err(|e| EngineError::Data(format!("{e:?}")))?
        .ok_or_else(|| EngineError::Data(format!("api order {}: its ticker is gone and nothing refused it", request.id)))?;
    finish(c, request.id, &Outcome::Refused { code: res.code.unwrap_or("order_failed").into(), message: res.message })
}
