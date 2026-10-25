//! IBKR's venue reads (Ibkr plan I4a): the order-critical price, the contract rules, an order's status and its cancel,
//! each for one account (the key's binding; a bot's through `ibkr_account::bot_account`). IBKR trades nothing yet
//! (`trades: false`); placement is slice I4b. Every read first lists the login's accounts (IBKR wants them read before
//! an order call) and is refused unless the login names exactly the bound account.
//!
//! Divergences from Exchanges::Ibkr:
//! - D-IBKR-PRICE: a snapshot price counts only when the answer echoes the conid asked for (Rails falls back to the first
//!   row), its value is a positive finite decimal (Rails' `to_d` reads "C189.5" or "NaN" as a number), field 6509 says
//!   live (R…) or delayed (D…) data (frozen Z/Y and unsubscribed N are refused), and `_updated` is at most 60 s old for
//!   live data, 20 min for delayed. One pre-flight retry; never cached (Rails caches 5 s).
//! - D-IBKR-RULES: the contract rules (`/iserver/contract/rules`, a read) give the size and price increments; a price
//!   magnifier other than 1, or none stated, is refused. Kept 24 h per (conid, side). Rails reads no rules.
//! - D-IBKR-STATUS-FIELDS: an order's fill, size and side are also read under the order-status answer's own names
//!   (`cum_fill`, `total_size`, side `B`/`S`), which Rails' `cumFill`/`totalSize`/`side.downcase` miss; a status naming
//!   another account is refused.
//! - D-IBKR-CANCEL: a cancel answered `{"error": …}` is refused (Rails counts any 2xx as done).
//!
//! The answer shapes are provisional: no IBKR session has been recorded yet (gate G1).
use super::http::Transport;
use super::ibkr::{Error, Session, ACCOUNTS_PATH};
use super::{OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::Clock;
use crate::ruby::{json_to_d, BigDec};
use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

pub const SNAPSHOT_PATH: &str = "/v1/api/iserver/marketdata/snapshot";
pub const SNAPSHOT_FIELDS: &str = "31,84,86,6509,_updated";
pub const RULES_PATH: &str = "/v1/api/iserver/contract/rules";
/// How long IBKR gets between the snapshot that subscribes a conid (the pre-flight) and the one that reads it.
pub const PREFLIGHT_PAUSE: std::time::Duration = std::time::Duration::from_millis(500);
pub const LIVE_MAX_AGE: TimeDelta = TimeDelta::seconds(60);
pub const DELAYED_MAX_AGE: TimeDelta = TimeDelta::minutes(20);
pub const RULES_TTL: TimeDelta = TimeDelta::hours(24);

/// A contract's trading increments (D-IBKR-RULES).
#[derive(Clone, Debug, PartialEq)]
pub struct ContractRules {
    pub size_increment: BigDec,
    /// (lowerEdge, increment), as IBKR lists them: from each edge up, prices step by its increment.
    pub increment_rules: Vec<(BigDec, BigDec)>,
}

type RulesCache = Mutex<HashMap<(i64, bool), (ContractRules, DateTime<Utc>)>>;
/// ponytail: never pruned (one entry per conid and side asked about); contract-level, so shared by every login.
static RULES: LazyLock<RulesCache> = LazyLock::new(Mutex::default);

/// IBKR's venue for `account` through `session`.
pub struct IbkrVenue<'a, T: Transport, C: Clock + ?Sized = dyn Clock> { session: Session<'a, T, C>, account: String }

impl<'a, T: Transport, C: Clock + ?Sized> IbkrVenue<'a, T, C> {
    pub fn new(session: Session<'a, T, C>, account: String) -> Self { Self { session, account } }

    /// `path` read under the login's lock, after the account check.
    async fn read(&self, method: &'static str, path: &str, query: Vec<(&'static str, String)>, body: Option<Value>) -> Result<Value, VenueError> {
        let held = self.session.lock().await.map_err(|e| self.error(e))?;
        let accounts = self.session.signed_request(&held, "GET", ACCOUNTS_PATH, vec![], None).await.map_err(|e| self.error(e))?;
        if !super::ibkr_account::names_only(&accounts, &self.account) {
            return Err(VenueError::Rejected(vec!["the IBKR login no longer names the account this connection belongs to".into()]));
        }
        self.session.signed_request(&held, method, path, query, body).await.map_err(|e| self.error(e))
    }

    /// A failed read: a 401 (IBKR answers a session blip as it does a revoked key), a 5xx, a busy login or a transport
    /// failure is Transient; another refusal Rejected; an unreadable answer Ambiguous.
    fn error(&self, e: Error) -> VenueError {
        let unreadable = matches!(e, Error::Unreadable);
        let rejected = matches!(&e, Error::Http { status, .. } if *status != 401 && *status < 500) || matches!(e, Error::Key(_) | Error::Mutation);
        let text = self.session.redact(&e.message());
        if unreadable { VenueError::Ambiguous(text) } else if rejected { VenueError::Rejected(vec![text]) } else { VenueError::Transient(text) }
    }

    /// The `side` price of `conid` from a market-data snapshot (D-IBKR-PRICE).
    pub async fn price(&self, conid: i64, side: PriceSide) -> Result<BigDec, VenueError> {
        let field = match side { PriceSide::Last => "31", PriceSide::Bid => "84", PriceSide::Ask => "86" };
        let query = || vec![("conids", conid.to_string()), ("fields", SNAPSHOT_FIELDS.to_string())];
        let mut answer = self.read("GET", SNAPSHOT_PATH, query(), None).await?;
        if first_row(&answer).is_some_and(|row| row.get(field).is_none()) {
            tokio::time::sleep(PREFLIGHT_PAUSE).await;
            answer = self.read("GET", SNAPSHOT_PATH, query(), None).await?;
        }
        usable_price(&answer, conid, field, self.session.now())
            .map_err(|why| VenueError::Rejected(vec![format!("no usable market data for conid {conid}: {why}")]))
    }

    /// The contract rules of `conid` for a buy or a sell (D-IBKR-RULES), kept RULES_TTL.
    pub async fn contract_rules(&self, conid: i64, is_buy: bool) -> Result<ContractRules, VenueError> {
        let now = self.session.now();
        let cached = RULES.lock().unwrap_or_else(|p| p.into_inner()).get(&(conid, is_buy)).filter(|(_, at)| now - *at < RULES_TTL).map(|(r, _)| r.clone());
        if let Some(rules) = cached { return Ok(rules) }
        let answer = self.read("POST", RULES_PATH, vec![], Some(json!({ "conid": conid, "isBuy": is_buy }))).await?;
        let rules = parse_rules(&answer).map_err(|why| VenueError::Rejected(vec![format!("IBKR contract {conid}: {why}")]))?;
        RULES.lock().unwrap_or_else(|p| p.into_inner()).insert((conid, is_buy), (rules.clone(), now));
        Ok(rules)
    }

    /// Exchanges::Ibkr#get_order: order `id`'s status, None when IBKR reports nothing for it.
    pub async fn order(&self, id: &str) -> Result<Option<OrderState>, VenueError> {
        let answer = self.read("GET", &format!("/v1/api/iserver/account/order/status/{id}"), vec![], None).await?;
        parse_order(&answer, id, &self.account)
    }

    /// Exchanges::Ibkr#cancel_order: DELETE against the bound account (D-IBKR-CANCEL).
    pub async fn cancel(&self, id: &str) -> Result<(), VenueError> {
        let answer = self.read("DELETE", &format!("/v1/api/iserver/account/{}/order/{id}", self.account), vec![], None).await?;
        match answer.get("error") {
            None | Some(Value::Null) => Ok(()),
            Some(e) => Err(VenueError::Rejected(vec![e.as_str().map(str::to_string).unwrap_or_else(|| e.to_string())])),
        }
    }
}

fn first_row(answer: &Value) -> Option<&Value> { answer.as_array().and_then(|rows| rows.first()) }

fn as_i64(v: &Value) -> Option<i64> { v.as_i64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok())) }

/// D-IBKR-PRICE over a snapshot answer for `conid`'s `field`, at `now`.
fn usable_price(answer: &Value, conid: i64, field: &str, now: DateTime<Utc>) -> Result<BigDec, String> {
    let row = first_row(answer).ok_or("no snapshot row")?;
    if row.get("conid").and_then(as_i64) != Some(conid) { return Err(format!("the answer is for conid {}", row.get("conid").map(crate::ruby::raw).unwrap_or_default())) }
    let value = match row.get(field).map(json_to_d) {
        Some(Ok(Some(v))) if v.is_positive() => v,
        Some(Ok(Some(_))) => return Err(format!("field {field} is not positive")),
        _ => return Err(format!("field {field} is missing or not a number")),
    };
    let availability = row.get("6509").and_then(Value::as_str).unwrap_or_default();
    let max_age = match availability.chars().next() { Some('R') => LIVE_MAX_AGE, Some('D') => DELAYED_MAX_AGE, _ => return Err(format!("market data {availability:?} is neither live nor delayed")) };
    let updated = row.get("_updated").and_then(as_i64).and_then(DateTime::from_timestamp_millis).ok_or("no update time")?;
    if (now - updated).abs() > max_age { return Err(format!("last updated {updated}")) }
    Ok(value)
}

/// D-IBKR-RULES over a `/iserver/contract/rules` answer (flat, or under `rules`).
fn parse_rules(answer: &Value) -> Result<ContractRules, String> {
    let rules = answer.get("rules").filter(|r| r.is_object()).unwrap_or(answer);
    let positive = |v: Option<&Value>, name: &str| match v.map(json_to_d) {
        Some(Ok(Some(d))) if d.is_positive() => Ok(d),
        _ => Err(format!("{name} is missing or not positive")),
    };
    match rules.get("priceMagnifier").or_else(|| answer.get("priceMagnifier")).map(json_to_d) {
        Some(Ok(Some(m))) if m == BigDec::one() => {}
        Some(Ok(Some(_))) => return Err(format!("price magnifier {} is not supported", crate::ruby::raw(rules.get("priceMagnifier").or_else(|| answer.get("priceMagnifier")).unwrap_or(&Value::Null)))),
        _ => return Err("no price magnifier stated".into()),
    }
    let size_increment = positive(rules.get("sizeIncrement"), "sizeIncrement")?;
    let listed = rules.get("incrementRules").and_then(Value::as_array).filter(|r| !r.is_empty()).ok_or("no incrementRules")?;
    let mut increment_rules = vec![];
    for r in listed {
        let edge = match r.get("lowerEdge").map(json_to_d) { Some(Ok(Some(e))) if e >= BigDec::zero() => e, _ => return Err("an increment rule without a lowerEdge".into()) };
        increment_rules.push((edge, positive(r.get("increment"), "an increment")?));
    }
    Ok(ContractRules { size_increment, increment_rules })
}

/// Exchanges::Ibkr#parse_order_status.
fn status(s: &str) -> OrderStatus {
    match s {
        "Filled" => OrderStatus::Closed,
        "Cancelled" | "PendingCancel" | "Inactive" => OrderStatus::Cancelled,
        "Submitted" | "PreSubmitted" | "PendingSubmit" => OrderStatus::Open,
        "Rejected" => OrderStatus::Failed,
        _ => OrderStatus::Unknown,
    }
}

/// Exchanges::Ibkr#parse_order_data over order `id`'s status answer (D-IBKR-STATUS-FIELDS).
fn parse_order(answer: &Value, id: &str, account: &str) -> Result<Option<OrderState>, VenueError> {
    let Some(o) = answer.as_object().filter(|o| !o.is_empty()) else { return Ok(None) };
    if o.get("account").and_then(Value::as_str).is_some_and(|a| a != account) {
        return Err(VenueError::Rejected(vec![format!("IBKR order {id} belongs to another account")]));
    }
    let unreadable = |e: crate::codec::CodecError| VenueError::Ambiguous(format!("IBKR order {id}: {e:?}"));
    // `filledQuantity.presence || cumFill || filled`, then IBKR's own name.
    let first = |names: &[&str]| names.iter().filter_map(|n| o.get(*n)).find(|v| !v.is_null() && v.as_str() != Some("")).cloned().unwrap_or(Value::Null);
    let num = |names: &[&str]| json_to_d(&first(names)).map(|v| v.unwrap_or_else(BigDec::zero)).map_err(unreadable);
    let filled = num(&["filledQuantity", "cumFill", "filled", "cum_fill"])?;
    let avg = num(&["avgPrice", "average_price"])?;
    let limit = num(&["price", "limit_price"])?;
    let amount = num(&["totalSize", "quantity", "total_size"])?;
    let text = |names: &[&str]| first(names).as_str().unwrap_or_default().to_string();
    let order_type = text(&["order_type", "orderType"]);
    Ok(Some(OrderState {
        txid: o.get("order_id").or_else(|| o.get("orderId")).map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_else(|| id.to_string()),
        status: status(&text(&["order_status", "status"])),
        price: Some(if avg.is_positive() { avg.clone() } else { limit }),
        amount: Some(amount),
        quote_amount: None,
        quote_amount_exec: Some(&filled * &avg),
        amount_exec: filled,
        limit: order_type.eq_ignore_ascii_case("LMT") || order_type.eq_ignore_ascii_case("LIMIT"),
        sell: text(&["side"]).to_ascii_uppercase().starts_with('S'),
        pair: None,
        asset_class: None,
        zero_fill_at: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venue::http::ScriptedTransport;
    use crate::venue::ibkr::tests::{at, new_login, scripted};

    const ACCOUNTS: &str = "GET /v1/api/iserver/accounts";
    const SNAPSHOT: &str = "GET /v1/api/iserver/marketdata/snapshot";
    const RULES_CALL: &str = "POST /v1/api/iserver/contract/rules";
    const NOW: &str = "2026-10-25T14:00:00Z";

    fn ms(t: &str) -> i64 { t.parse::<DateTime<Utc>>().unwrap().timestamp_millis() }

    /// A scripted IBKR whose login names `account`; `f` scripts the rest.
    fn run<R>(account: &str, f: impl FnOnce(&ScriptedTransport), call: impl AsyncFnOnce(&IbkrVenue<'_, ScriptedTransport, crate::engine::FixedClock>) -> R) -> (R, Vec<String>) {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply(ACCOUNTS, 200, json!({ "accounts": [account], "selectedAccount": account, "source": "provisional" }));
        f(&t);
        let dh_random = session["dh_random"].as_str().unwrap().to_string();
        let clock = at(NOW);
        let venue = IbkrVenue::new(Session::new(&t, login, &clock).with_dh_random(move || dh_random.clone()), "U1234567".into());
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().start_paused(true).build().unwrap();
        let r = rt.block_on(call(&venue));
        (r, t.requests().iter().map(|r| format!("{} {}", r.method, r.path)).collect())
    }

    fn snapshot(row: Value) -> Value { json!([row]) }
    fn live(conid: Value, last: Value, availability: &str, updated: &str) -> Value {
        json!({ "conid": conid, "31": last, "84": "189.40", "86": "189.60", "6509": availability, "_updated": ms(updated), "source": "provisional" })
    }

    fn price(row: Value) -> (Result<BigDec, VenueError>, Vec<String>) {
        run("U1234567", |t| { t.reply(SNAPSHOT, 200, snapshot(row)); }, async |v| v.price(265598, PriceSide::Last).await)
    }

    fn refused(r: Result<BigDec, VenueError>, why: &str) {
        match r { Err(VenueError::Rejected(e)) => assert!(e[0].starts_with("no usable market data for conid 265598") && e[0].contains(why), "{e:?}"), other => panic!("{other:?}") }
    }

    #[test]
    fn a_live_fresh_price_is_used_and_the_fields_are_asked_for_one_conid() {
        let (r, sent) = price(live(json!(265598), json!("189.50"), "RpB", "2026-10-25T13:59:30Z"));
        assert_eq!(r, Ok(BigDec::parse("189.5").unwrap()));
        assert_eq!(sent[sent.len() - 2..], [ACCOUNTS, SNAPSHOT]);
        let (r, _) = run("U1234567", |t| { t.reply(SNAPSHOT, 200, snapshot(live(json!("265598"), json!(1), "DpB", "2026-10-25T13:45:00Z"))); },
                         async |v| v.price(265598, PriceSide::Ask).await);
        assert_eq!(r, Ok(BigDec::parse("189.6").unwrap()), "delayed data 15 min old; the ask; a string conid");
    }

    #[test]
    fn the_snapshot_asks_for_one_conid_and_the_validation_fields() {
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply(ACCOUNTS, 200, json!({ "accounts": ["U1234567"] })).reply(SNAPSHOT, 200, snapshot(live(json!(265598), json!("1"), "R", NOW)));
        let dh = session["dh_random"].as_str().unwrap().to_string();
        let clock = at(NOW);
        let v = IbkrVenue::new(Session::new(&t, login, &clock).with_dh_random(move || dh.clone()), "U1234567".into());
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(v.price(265598, PriceSide::Bid)).unwrap();
        let q = t.requests().last().unwrap().query.clone();
        assert_eq!(q, [("conids", "265598".to_string()), ("fields", "31,84,86,6509,_updated".to_string())]);
    }

    #[test]
    fn a_wrong_conid_echo_is_refused() {
        let (r, _) = price(live(json!(8314), json!("189.50"), "RpB", NOW));
        refused(r, "the answer is for conid 8314");
    }

    #[test]
    fn nan_a_prefixed_value_zero_or_a_negative_price_is_refused() {
        for v in [json!("NaN"), json!("C189.50"), json!("Infinity"), json!(true)] { refused(price(live(json!(265598), v, "RpB", NOW)).0, "missing or not a number"); }
        for v in [json!("0"), json!(-1)] { refused(price(live(json!(265598), v, "RpB", NOW)).0, "not positive"); }
    }

    #[test]
    fn frozen_or_unsubscribed_data_is_refused() {
        for a in ["ZpB", "YpB", "N", ""] { refused(price(live(json!(265598), json!("189.5"), a, NOW)).0, "neither live nor delayed"); }
    }

    #[test]
    fn a_stale_update_is_refused() {
        refused(price(live(json!(265598), json!("189.5"), "RpB", "2026-10-25T13:58:59Z")).0, "last updated");
        assert!(price(live(json!(265598), json!("189.5"), "RpB", "2026-10-25T13:59:00Z")).0.is_ok(), "60 s is still fresh");
        refused(price(live(json!(265598), json!("189.5"), "DpB", "2026-10-25T13:39:59Z")).0, "last updated");
        let mut row = live(json!(265598), json!("189.5"), "RpB", NOW);
        row.as_object_mut().unwrap().remove("_updated");
        refused(price(row).0, "no update time");
    }

    #[test]
    fn a_pre_flight_answer_is_asked_again_once() {
        let preflight = json!([{ "conid": 265598, "conidEx": "265598", "_updated": ms(NOW), "source": "provisional" }]);
        let (r, sent) = run("U1234567", |t| { t.reply(SNAPSHOT, 200, preflight.clone()).reply(SNAPSHOT, 200, snapshot(live(json!(265598), json!("189.5"), "RpB", NOW))); },
                            async |v| v.price(265598, PriceSide::Last).await);
        assert_eq!(r, Ok(BigDec::parse("189.5").unwrap()));
        assert_eq!(sent.iter().filter(|s| *s == SNAPSHOT).count(), 2);
        let (r, sent) = run("U1234567", |t| { t.reply(SNAPSHOT, 200, preflight.clone()); }, async |v| v.price(265598, PriceSide::Last).await);
        refused(r, "missing");
        assert_eq!(sent.iter().filter(|s| *s == SNAPSHOT).count(), 2, "one retry only");
    }

    fn rules_answer(magnifier: Value) -> Value {
        json!({ "rules": { "sizeIncrement": 1, "incrementRules": [{ "lowerEdge": 0.0, "increment": 0.0001 }, { "lowerEdge": 1.0, "increment": "0.01" }], "priceMagnifier": magnifier },
                "source": "provisional" })
    }

    #[test]
    fn increment_rules_are_parsed_and_kept_a_day() {
        let (r, sent) = run("U1234567", |t| { t.reply(RULES_CALL, 200, rules_answer(json!(1))); }, async |v| {
            let first = v.contract_rules(910001, true).await;
            (first, v.contract_rules(910001, true).await)
        });
        let d = |s: &str| BigDec::parse(s).unwrap();
        let expected = ContractRules { size_increment: d("1"), increment_rules: vec![(d("0"), d("0.0001")), (d("1"), d("0.01"))] };
        assert_eq!(r.0, Ok(expected.clone()));
        assert_eq!(r.1, Ok(expected));
        assert_eq!(sent.iter().filter(|s| *s == RULES_CALL).count(), 1, "cached");
        let (login, session) = new_login();
        let t = scripted(&session);
        t.reply(ACCOUNTS, 200, json!({ "accounts": ["U1234567"] })).reply(RULES_CALL, 200, rules_answer(json!(1)));
        let dh = session["dh_random"].as_str().unwrap().to_string();
        let tomorrow = at("2026-10-26T14:00:00Z");
        let v = IbkrVenue::new(Session::new(&t, login, &tomorrow).with_dh_random(move || dh.clone()), "U1234567".into());
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(v.contract_rules(910001, true)).unwrap();
        assert_eq!(t.requests().last().unwrap().body, Some(json!({ "conid": 910001, "isBuy": true })), "asked again after 24 h");
    }

    #[test]
    fn a_magnifier_other_than_one_or_none_is_refused() {
        for (m, why) in [(json!(100), "price magnifier 100 is not supported"), (Value::Null, "no price magnifier stated")] {
            let (r, _) = run("U1234567", |t| { t.reply(RULES_CALL, 200, rules_answer(m.clone())); }, async |v| v.contract_rules(910002, false).await);
            assert_eq!(r, Err(VenueError::Rejected(vec![format!("IBKR contract 910002: {why}")])));
        }
        assert!(parse_rules(&json!({ "priceMagnifier": 1, "sizeIncrement": 0, "incrementRules": [{ "lowerEdge": 0, "increment": 0.01 }] })).is_err());
        assert!(parse_rules(&json!({ "priceMagnifier": 1, "sizeIncrement": 1, "incrementRules": [] })).is_err());
    }

    #[test]
    fn the_status_map_is_rails() {
        for (s, want) in [("Filled", OrderStatus::Closed), ("Cancelled", OrderStatus::Cancelled), ("PendingCancel", OrderStatus::Cancelled), ("Inactive", OrderStatus::Cancelled),
                          ("Submitted", OrderStatus::Open), ("PreSubmitted", OrderStatus::Open), ("PendingSubmit", OrderStatus::Open), ("Rejected", OrderStatus::Failed),
                          ("ApiPending", OrderStatus::Unknown), ("", OrderStatus::Unknown)] { assert_eq!(status(s), want, "{s}"); }
    }

    #[test]
    fn an_order_status_is_read_under_its_own_field_names() {
        let body = json!({ "order_id": 1799796559, "conid": 265598, "side": "S", "total_size": "5.0", "order_type": "MARKET", "cum_fill": "5.0",
                           "order_status": "Filled", "average_price": "192.26", "account": "U1234567", "source": "provisional" });
        let (r, sent) = run("U1234567", |t| { t.reply("GET /v1/api/iserver/account/order/status/1799796559", 200, body.clone()); }, async |v| v.order("1799796559").await);
        let o = r.unwrap().unwrap();
        let d = |s: &str| BigDec::parse(s).unwrap();
        assert_eq!((o.txid.as_str(), o.status, o.amount_exec.clone(), o.amount.clone(), o.price.clone(), o.sell, o.limit), ("1799796559", OrderStatus::Closed, d("5"), Some(d("5")), Some(d("192.26")), true, false));
        assert_eq!(o.quote_amount_exec, Some(d("961.3")));
        assert_eq!(sent[sent.len() - 2..], [ACCOUNTS, "GET /v1/api/iserver/account/order/status/1799796559"]);
        // Nothing reported: missing.
        let (r, _) = run("U1234567", |t| { t.reply("GET /v1/api/iserver/account/order/status/7", 200, json!({})); }, async |v| v.order("7").await);
        assert_eq!(r, Ok(None));
        // Another account's order.
        let other = json!({ "order_id": 8, "order_status": "Filled", "account": "U7654321" });
        let (r, _) = run("U1234567", |t| { t.reply("GET /v1/api/iserver/account/order/status/8", 200, other.clone()); }, async |v| v.order("8").await);
        assert_eq!(r, Err(VenueError::Rejected(vec!["IBKR order 8 belongs to another account".into()])));
    }

    #[test]
    fn a_cancel_is_a_delete_against_the_bound_account() {
        let path = "DELETE /v1/api/iserver/account/U1234567/order/42";
        let (r, sent) = run("U1234567", |t| { t.reply(path, 200, json!({ "msg": "Request was submitted", "order_id": 42, "source": "provisional" })); }, async |v| v.cancel("42").await);
        assert_eq!(r, Ok(()));
        assert_eq!(sent[sent.len() - 2..], [ACCOUNTS, path]);
        let (r, _) = run("U1234567", |t| { t.reply(path, 200, json!({ "error": "OrderID 42 doesn't exist" })); }, async |v| v.cancel("42").await);
        assert_eq!(r, Err(VenueError::Rejected(vec!["OrderID 42 doesn't exist".into()])));
    }

    #[test]
    fn every_read_is_refused_when_the_login_names_another_account() {
        let refused = || Err(VenueError::Rejected(vec!["the IBKR login no longer names the account this connection belongs to".into()]));
        let (r, sent) = run("U7654321", |_| {}, async |v| (v.price(265598, PriceSide::Last).await.map(|_| ()), v.contract_rules(910003, true).await.map(|_| ()),
                                                            v.order("1").await.map(|_| ()), v.cancel("1").await));
        assert_eq!(r, (refused(), refused(), refused(), refused()));
        assert!(sent.iter().all(|s| s == ACCOUNTS || s.contains("live_session_token") || s.contains("ssodh")), "nothing past the account list: {sent:?}");
    }

    #[test]
    fn a_read_failure_maps_to_its_venue_error() {
        let (r, _) = run("U1234567", |t| { t.reply(SNAPSHOT, 503, json!({})); }, async |v| v.price(265598, PriceSide::Last).await);
        assert!(matches!(r, Err(VenueError::Transient(_))));
        let (r, _) = run("U1234567", |t| { t.reply(SNAPSHOT, 400, json!({ "error": "bad conid" })); }, async |v| v.price(265598, PriceSide::Last).await);
        assert!(matches!(r, Err(VenueError::Rejected(_))));
        let (r, _) = run("U1234567", |t| { t.reply_typed(SNAPSHOT, 200, "application/json", "{"); }, async |v| v.price(265598, PriceSide::Last).await);
        assert!(matches!(r, Err(VenueError::Ambiguous(_))));
    }
}
