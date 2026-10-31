//! D-KUCOIN-HF-RETENTION: what a KuCoin HF row still owes, and the one way it settles.
//!
//! - **Obligation** (`rust_kucoin_unresolved:<tx>`): a row whose state cannot be told (unreadable, unknown, lower totals
//!   than already reported, fewer than its known fills, contradicting its settlement). While one stands, its row's bot
//!   places nothing (`barrier`, read where every intent is born). A positive read that applies, or a settlement, ends it.
//! - **Receipt** (`rust_kucoin_evidence:<tx>`): the facts a row settled on (status, exact dealt totals, fees when known,
//!   the operator's evidence). A row settles once: the same facts again are a no-op, other facts are a conflict.
//!
//! Every terminal state applied to a row with an HF record goes through `admit` (polling, recovery, `reconcile-order`).
use super::model;
use super::EngineError;
use crate::ruby::BigDec;
use crate::venue::{OrderState, OrderStatus};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

pub const UNRESOLVED: &str = "rust_kucoin_unresolved:";
pub const RECEIPT: &str = "rust_kucoin_evidence:";

fn data(e: String) -> EngineError { EngineError::Data(e) }

/// The row's open obligation, if any.
pub fn obligation(c: &Connection, tx_id: i64) -> Result<Option<Value>, EngineError> {
    Ok(crate::app_config::get_plain(c, &format!("{UNRESOLVED}{tx_id}")).map_err(data)?.and_then(|s| serde_json::from_str(&s).ok()))
}

pub(crate) fn mark(c: &Connection, tx_id: i64, why: &str, now: DateTime<Utc>) -> Result<(), EngineError> {
    if obligation(c, tx_id)?.is_some_and(|o| o["why"] == why) { return Ok(()); }
    crate::app_config::set_plain(c, &format!("{UNRESOLVED}{tx_id}"), &json!({ "why": why, "at": crate::codec::format_time(now) }).to_string(), now).map_err(data)
}

pub(crate) fn clear(c: &Connection, tx_id: i64) -> Result<(), EngineError> {
    crate::app_config::delete(c, &format!("{UNRESOLVED}{tx_id}")).map_err(data)
}

/// The first row of `bot_id` that holds its spending: an open obligation, or an adopted row whose route is unproven
/// (`ORIGIN`). Owned through the row, so a Merge or Split that moves the row moves it. (transaction id, why)
pub fn barrier(c: &Connection, bot_id: i64) -> Result<Option<(i64, String)>, EngineError> {
    let row = |prefix: &str, filter: &str| -> Result<Option<(i64, String)>, EngineError> {
        // A key range, so the scan touches only these rows (app_configs.key is unique).
        let sql = format!("SELECT t.id, k.value FROM app_configs k JOIN transactions t ON t.id = CAST(substr(k.key, ?2) AS INTEGER) \
                           WHERE k.key >= ?1 AND k.key < ?4 AND t.bot_id = ?3 {filter} ORDER BY t.id LIMIT 1");
        // Every prefix ends in ':'; ';' is the next character, so [prefix, upper) is exactly the prefix's keys.
        let upper = format!("{};", &prefix[..prefix.len() - 1]);
        Ok(c.query_row(&sql, params![prefix, prefix.len() as i64 + 1, bot_id, upper], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))).optional()?
            .map(|(id, v)| (id, v.and_then(|v| serde_json::from_str::<Value>(&v).ok()).and_then(|v| v["why"].as_str().map(str::to_string)).unwrap_or_default())))
    };
    if let Some(found) = row(UNRESOLVED, "")? { return Ok(Some(found)); }
    Ok(row(ORIGIN, "AND json_extract(k.value, '$.origin') = 'unproven'")?
        .map(|(id, _)| (id, "it was placed before this engine took over and is found on neither KuCoin route".into())))
}

/// D-KUCOIN-HF-RETENTION (§5): an adopted row's route, `{"origin": "classic" | "unproven"}`. An HF origin is the HF record.
pub const ORIGIN: &str = "rust_kucoin_origin:";

/// The dealt totals settled on, as exact decimals, and the status ("closed" | "cancelled").
#[derive(Clone, Debug, PartialEq)]
pub struct Facts { pub status: &'static str, pub size: BigDec, pub funds: BigDec }

impl Facts {
    pub fn of(s: &OrderState) -> Option<Self> {
        let status = match s.status { OrderStatus::Closed => "closed", OrderStatus::Cancelled => "cancelled", _ => return None };
        Some(Self { status, size: s.amount_exec.clone(), funds: s.quote_amount_exec.clone().unwrap_or_else(BigDec::zero) })
    }
    fn json(&self) -> Value { json!({ "status": self.status, "deal_size": self.size.to_s_f(), "deal_funds": self.funds.to_s_f() }) }
}

/// The row's settlement: its receipt, or, for a terminal row settled before receipts existed, its status with its record's
/// exact totals (`implicit`).
pub struct Settled { pub facts: Facts, pub receipt: Option<Value> }

pub fn receipt(c: &Connection, tx_id: i64) -> Result<Option<Value>, EngineError> {
    Ok(crate::app_config::get_plain(c, &format!("{RECEIPT}{tx_id}")).map_err(data)?.and_then(|s| serde_json::from_str(&s).ok()))
}

fn decimal(v: &Value, key: &str, tx_id: i64) -> Result<BigDec, EngineError> {
    v[key].as_str().and_then(|s| BigDec::parse(s).ok()).ok_or_else(|| data(format!("unreadable {key} for transaction {tx_id}")))
}

pub fn settled(c: &Connection, tx_id: i64, record: &Value) -> Result<Option<Settled>, EngineError> {
    if let Some(r) = receipt(c, tx_id)? {
        let status = match r["status"].as_str() { Some("closed") => "closed", Some("cancelled") => "cancelled", _ => return Err(data(format!("unreadable receipt of transaction {tx_id}"))) };
        return Ok(Some(Settled { facts: Facts { status, size: decimal(&r, "deal_size", tx_id)?, funds: decimal(&r, "deal_funds", tx_id)? }, receipt: Some(r) }));
    }
    let external: Option<i64> = c.query_row("SELECT external_status FROM transactions WHERE id = ?1", [tx_id], |r| r.get(0))?;
    let status = match external { Some(2) => "closed", Some(3) => "cancelled", _ => return Ok(None) };
    Ok(Some(Settled { facts: Facts { status, size: decimal(record, "deal_size", tx_id)?, funds: decimal(record, "deal_funds", tx_id)? }, receipt: None }))
}

pub(crate) fn write_receipt(c: &Connection, tx_id: i64, facts: &Facts, source: &str, extra: Value, now: DateTime<Utc>) -> Result<(), EngineError> {
    let mut r = facts.json();
    r["v"] = json!(1);
    r["source"] = json!(source);
    r["at"] = json!(crate::codec::format_time(now));
    r["fees"] = Value::Null;
    if let Value::Object(extra) = extra { for (k, v) in extra { r[k] = v; } }
    crate::app_config::set_plain(c, &format!("{RECEIPT}{tx_id}"), &r.to_string(), now).map_err(data)
}

/// What the ledger already holds of order `order_id`: its API fills of `family` (the record's: "ClassicHf" or "Uta"; visible,
/// and withheld where HF-3's table exists), the owner's and the row's exchange's only, each id once. Σ size, Σ funds,
/// Σ |fee| per currency.
#[derive(Debug, PartialEq)]
pub struct Known { pub size: BigDec, pub funds: BigDec, pub fees: std::collections::BTreeMap<String, BigDec> }

pub fn known_fills(c: &Connection, user_id: i64, exchange_id: i64, order_id: &str, family: &str) -> Result<Known, EngineError> {
    let mut rows: Vec<(String, String)> = vec![];
    let mut s = c.prepare_cached("SELECT tx_id, raw_data FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id IS NOT NULL \
                                  AND json_valid(raw_data) AND json_extract(raw_data, '$.orderId') = ?3 AND json_extract(raw_data, '$.createdAt') IS NOT NULL")?;
    rows.extend(s.query_map(params![user_id, exchange_id, order_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?);
    let withheld: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'kucoin_withheld_fills')", [], |r| r.get(0))?;
    if withheld {
        let mut s = c.prepare_cached("SELECT tx_id, entry FROM kucoin_withheld_fills WHERE user_id = ?1 AND exchange_id = ?2 \
                                      AND json_valid(entry) AND json_extract(entry, '$.orderId') = ?3")?;
        rows.extend(s.query_map(params![user_id, exchange_id, order_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<_>, _>>()?);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut known = Known { size: BigDec::zero(), funds: BigDec::zero(), fees: Default::default() };
    for (id, raw) in rows {
        if !seen.insert(id.clone()) { continue; }
        let raw: Value = serde_json::from_str(&raw).map_err(|e| data(format!("fill {id}: {e}")))?;
        // A classic fill carries no family (kucoin_ledger::fill).
        if raw["family"].as_str().unwrap_or("Classic") != family { continue; }
        let n = |k: &str| raw[k].as_str().and_then(|s| BigDec::parse(s).ok()).ok_or_else(|| data(format!("fill {id}: unreadable {k}")));
        known.size = &known.size + &n("size")?;
        known.funds = &known.funds + &n("funds")?;
        // The ledger's fee: its absolute value (kucoin_ledger::fill).
        let fee = n("fee")?;
        let fee = if fee < BigDec::zero() { &BigDec::zero() - &fee } else { fee };
        if let Some(currency) = raw["feeCurrency"].as_str() {
            let sum = known.fees.entry(currency.to_string()).or_insert_with(BigDec::zero);
            *sum = &*sum + &fee;
        }
    }
    known.fees.retain(|_, v| !v.is_zero()); // a zero fee is none, as a bundle's fees read (fee_map)
    Ok(known)
}

/// The owner and exchange `known_fills` reads for row `tx_id`: the row's bot's user, the row's own exchange.
pub(crate) fn owner(c: &Connection, tx_id: i64) -> Result<(i64, i64), EngineError> {
    Ok(c.query_row("SELECT b.user_id, t.exchange_id FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE t.id = ?1", [tx_id], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

/// What `admit` decided for one HF answer, in the caller's write transaction.
pub(crate) enum Admit {
    /// Apply it; `terminal`: it settles the row (the caller writes the receipt after its writes).
    Proceed { terminal: bool },
    /// Nothing more to write (a replay, or an obligation now stands).
    Done,
}

/// The gate before an HF row takes an answer (`polling::apply_order_in`). A settled row takes only its own facts again
/// (a no-op that ends an obligation; for a row settled before receipts, its receipt and what it still owed). An unsettled
/// row takes no unknown state, no lower totals than reported, nor fewer than its known fills: each is an obligation.
/// A terminal answer ends the row's reversal cancel here, before its settlement credits the reserve.
pub(crate) fn admit(c: &Connection, bot_id: i64, tx_id: i64, external_id: &str, record: &Value, s: &OrderState, now: DateTime<Utc>) -> Result<Admit, EngineError> {
    let facts = Facts::of(s);
    // The fills the ledger already holds bound every answer, a replay of a settlement included.
    let (user, exchange) = owner(c, tx_id)?;
    let known = known_fills(c, user, exchange, external_id, record["family"].as_str().unwrap_or_default())?;
    let funds = s.quote_amount_exec.clone().unwrap_or_else(BigDec::zero);
    if !matches!(s.status, OrderStatus::Unknown | OrderStatus::Failed) && (s.amount_exec < known.size || funds < known.funds) {
        mark(c, tx_id, "KuCoin answered less than the fills already recorded for it", now)?;
        return Ok(Admit::Done);
    }
    if let Some(settled) = settled(c, tx_id, record)? {
        if facts.as_ref() == Some(&settled.facts) {
            if settled.receipt.is_none() {
                discharge(c, bot_id, tx_id)?;
                write_receipt(c, tx_id, &settled.facts, "api", Value::Null, now)?;
            }
            clear(c, tx_id)?;
        } else {
            mark(c, tx_id, "KuCoin's answer contradicts the order's settlement", now)?;
        }
        return Ok(Admit::Done);
    }
    if matches!(s.status, OrderStatus::Unknown | OrderStatus::Failed) {
        mark(c, tx_id, "its state cannot be told yet", now)?;
        return Ok(Admit::Done);
    }
    if s.amount_exec < decimal(record, "deal_size", tx_id)? || funds < decimal(record, "deal_funds", tx_id)? {
        mark(c, tx_id, "KuCoin answered lower dealt totals than it already reported", now)?;
        return Ok(Admit::Done);
    }
    let terminal = facts.is_some();
    if terminal { super::reversal::drop_cancel(c, bot_id, tx_id)?; }
    Ok(Admit::Proceed { terminal })
}

/// A row settled before receipts: what it still owes once its settlement is confirmed (the reversal's cancel entry, a
/// pending reserve).
fn discharge(c: &Connection, bot_id: i64, tx_id: i64) -> Result<(), EngineError> {
    super::reversal::drop_cancel(c, bot_id, tx_id)?;
    super::reversal::settle(c, bot_id, tx_id).map(|_| ())
}

/// An HF row a read could not tell (excluded, or a failed single read), in its own fenced write: an obligation, unless the
/// row is already settled (then a missing remote answer owes nothing).
pub(crate) fn unread(c: &Connection, bot_id: i64, tx_id: i64, why: &str, version: &Option<model::CredentialVersion>, now: DateTime<Utc>) -> Result<(), EngineError> {
    model::credential_write(c, version, |tx| {
        super::polling::require_owner(tx, bot_id, tx_id)?;
        if let Some(record) = crate::venue::kucoin::order_record(tx, tx_id).map_err(data)? {
            if settled(tx, tx_id, &record)?.is_some() { return Ok(()); }
        }
        mark(tx, tx_id, why, now)
    })
}

// ---- `deltabadger reconcile-order <transaction_id> --evidence <file>` ------------------------------------------------

/// What `by_operator` did.
#[derive(Debug, PartialEq)]
pub enum Operator { Reconciled, AlreadyReconciled }

/// Evidence bundle v1 (HF-4 plan §4). No field holds a credential.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Bundle { version: u32, transaction_id: i64, account: Account, order: Order, terminal: Terminal, executed: Executed, fees: Vec<Fee>, source: Source }
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Account { host: String, key_fingerprint: String }
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Order { id: String, symbol: String, side: String, #[serde(rename = "type")] kind: String, family: String }
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Terminal { state: String, at: String }
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Executed { base: String, quote: String }
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Fee { currency: String, amount: String }
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Source { kind: String, #[serde(default)] note: Option<String>, sha256: String, raw: String, #[serde(default)] coverage: Option<String>, #[serde(default)] fills: Option<Vec<ListedFill>> }
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ListedFill { id: String, base: String, quote: String, fee: String, fee_currency: String }

type Fees = std::collections::BTreeMap<String, BigDec>;
/// A row as `by_operator` reads it: bot, external id, status, created_at, exchange type.
type RowFacts = (Option<i64>, Option<String>, Option<i64>, String, String);

/// Fees by currency, zero amounts left out (so `[]` and a zero fee agree).
fn fee_map(fees: impl IntoIterator<Item = (String, BigDec)>) -> Fees {
    let mut m = Fees::new();
    for (currency, amount) in fees {
        let sum = m.entry(currency).or_insert_with(BigDec::zero);
        *sum = &*sum + &amount;
    }
    m.retain(|_, v| !v.is_zero());
    m
}
fn fees_json(m: &Fees) -> Value { Value::Array(m.iter().map(|(c, a)| json!({ "currency": c, "amount": a.to_s_f() })).collect()) }
fn abs(d: BigDec) -> BigDec { if d < BigDec::zero() { &BigDec::zero() - &d } else { d } }

/// The bundle's own checks, before anything is read: every problem found, or the facts and fees it proves.
fn validate(b: &Bundle, tx_id: i64, now: DateTime<Utc>) -> Result<(Facts, Fees, DateTime<Utc>), Vec<String>> {
    let mut why = vec![];
    let dec = |v: &str, name: &str, why: &mut Vec<String>| match BigDec::parse(v) {
        Ok(d) if d >= BigDec::zero() => Some(d),
        _ => { why.push(format!("{name} must be an exact non-negative decimal, not {v:?}")); None }
    };
    if b.version != 1 { why.push(format!("version must be 1, not {}", b.version)); }
    if b.transaction_id != tx_id { why.push(format!("transaction_id {} is not transaction {tx_id}", b.transaction_id)); }
    if !matches!(b.order.family.as_str(), "ClassicHf" | "Uta" | "Classic") { why.push("order.family must be ClassicHf, Uta or Classic".into()); }
    let status = match b.terminal.state.as_str() { "closed" => Some("closed"), "cancelled" => Some("cancelled"),
        other => { why.push(format!("terminal.state must be closed or cancelled, not {other:?} (no terminal proof)")); None } };
    let at = crate::codec::parse_time(&b.terminal.at).map_err(|_| why.push(format!("terminal.at {:?} is not a time", b.terminal.at))).ok();
    if at.is_some_and(|t| t > now) { why.push("terminal.at is in the future".into()); }
    let (size, funds) = (dec(&b.executed.base, "executed.base", &mut why), dec(&b.executed.quote, "executed.quote", &mut why));
    if let (Some(s), Some(f)) = (&size, &funds) {
        if s.is_positive() != f.is_positive() { why.push("executed.base and executed.quote must be positive together".into()); }
        if !s.is_positive() && status == Some("closed") && b.order.kind != "market" { why.push("a limit order's proven zero fill is cancelled, not closed".into()); }
    }
    let mut fees = vec![];
    for f in &b.fees {
        if fees.iter().any(|(c, _): &(String, BigDec)| c == &f.currency) { why.push(format!("fee currency {} is listed twice", f.currency)); }
        if let Some(a) = dec(&f.amount, "fees.amount", &mut why) { fees.push((f.currency.clone(), a)); }
    }
    let fees = fee_map(fees);
    use sha2::{Digest, Sha256};
    if hex::encode(Sha256::digest(b.source.raw.as_bytes())) != b.source.sha256.to_lowercase() { why.push("source.sha256 is not the hash of source.raw".into()); }
    if let (Some(status), Some(size), Some(funds)) = (status, size, funds) {
        let facts = Facts { status, size, funds };
        match b.source.kind.as_str() {
            "exchange_order_record" => {}
            "api_order_detail" => match api_detail(b, &facts, &fees, now) { Ok(()) => {}, Err(e) => why.push(e) },
            "execution_list" => match listed(b, &facts, &fees) { Ok(()) => {}, Err(e) => why.push(e) },
            other => why.push(format!("source.kind {other:?} is not evidence of a final order (api_order_detail, exchange_order_record or execution_list)")),
        }
        if why.is_empty() { if let Some(at) = at { return Ok((facts, fees, at)); } }
    }
    Err(why)
}

/// A retained order-detail answer: read as the poll reads it, it must give this terminal state, these totals and fees. A
/// UTA order's detail carries no quote total (D-KUCOIN-UTA-ORDERS): its evidence is the detail followed by its execution
/// pages to an empty one, a JSON array, and the totals and fees are the executions' (r1.7).
fn api_detail(b: &Bundle, facts: &Facts, fees: &Fees, now: DateTime<Utc>) -> Result<(), String> {
    if b.order.family == "Uta" {
        let answers: Vec<Value> = serde_json::from_str(&b.source.raw).map_err(|_| "source.raw is not the UTA order's detail and executions (a JSON array)".to_string())?;
        let want = crate::venue::kucoin::HfOrder { id: b.order.id.clone(), symbol: b.order.symbol.clone(), sell: b.order.side == "sell", limit: b.order.kind == "limit" };
        let (state, raw_fees) = crate::venue::kucoin::uta_evidence(&answers, &want).map_err(|_| "source.raw is not this order's detail and its full execution list".to_string())?;
        if Facts::of(&state).as_ref() != Some(facts) { return Err("source.raw does not show this terminal state and these totals".into()); }
        if &fee_map(raw_fees) != fees { return Err("fees differ from source.raw's executions".into()); }
        return Ok(());
    }
    let body: Value = serde_json::from_str(&b.source.raw).map_err(|_| "source.raw is not a KuCoin answer".to_string())?;
    if body["code"] != "200000" { return Err("source.raw is not a successful KuCoin order answer".into()); }
    let want = crate::venue::kucoin::HfOrder { id: b.order.id.clone(), symbol: b.order.symbol.clone(), sell: b.order.side == "sell", limit: b.order.kind == "limit" };
    let state = if b.order.family == "Classic" {
        // A classic order answer (`GET /api/v1/orders/{id}`), checked against the bundle's order as an HF one is.
        let o = &body["data"];
        let same = o["id"].as_str() == Some(want.id.as_str()) && o["symbol"].as_str() == Some(want.symbol.as_str())
            && o["side"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(&b.order.side)) && o["type"].as_str().is_some_and(|t| t.eq_ignore_ascii_case(&b.order.kind));
        if !same { return Err("source.raw is not this order's detail".into()); }
        crate::venue::kucoin::parse_order(&want.id, o, &Default::default()).map_err(|_| "source.raw is not this order's detail".to_string())?.settled(now)
    } else {
        crate::venue::kucoin::parse_hf_order(&body["data"], &want, &Default::default()).map_err(|_| "source.raw is not this order's detail".to_string())?.settled(now)
    };
    if Facts::of(&state).as_ref() != Some(facts) { return Err("source.raw does not show this terminal state and these totals".into()); }
    let fee = body["data"]["fee"].as_str().and_then(|s| BigDec::parse(s).ok()).map(abs);
    let raw_fees = fee_map(body["data"]["feeCurrency"].as_str().zip(fee).map(|(c, a)| (c.to_string(), a)));
    if &raw_fees != fees { return Err("fees differ from source.raw's fee".into()); }
    Ok(())
}

/// An execution list proves totals only as the order's full list: unique fill ids summing exactly to the bundle.
fn listed(b: &Bundle, facts: &Facts, fees: &Fees) -> Result<(), String> {
    if b.source.coverage.as_deref() != Some("full_order") { return Err("an execution list needs coverage \"full_order\" (a trade list alone proves no final total)".into()); }
    let fills = b.source.fills.as_deref().filter(|f| !f.is_empty()).ok_or("an execution list needs its fills")?;
    let mut ids = std::collections::BTreeSet::new();
    let (mut size, mut funds, mut fee) = (BigDec::zero(), BigDec::zero(), vec![]);
    for f in fills {
        if !ids.insert(&f.id) { return Err(format!("fill {} is listed twice", f.id)); }
        let n = |v: &str| BigDec::parse(v).ok().filter(|d| *d >= BigDec::zero()).ok_or_else(|| format!("fill {}: {v:?} is not an exact non-negative decimal", f.id));
        size = &size + &n(&f.base)?;
        funds = &funds + &n(&f.quote)?;
        fee.push((f.fee_currency.clone(), abs(BigDec::parse(&f.fee).map_err(|_| format!("fill {}: unreadable fee", f.id))?)));
    }
    if size != facts.size || funds != facts.funds { return Err("the listed fills do not sum to executed".into()); }
    if &fee_map(fee) != fees { return Err("the listed fills' fees do not sum to fees".into()); }
    Ok(())
}

/// `deltabadger reconcile-order`: an operator's evidence settles an existing HF row whose order KuCoin no longer serves.
/// Never a request, an intent or a row. The bundle is checked first; then, in one write transaction under the engine's
/// lock (the caller holds it), the row, its record, its settlement and its known fills; then the settlement gate every HF
/// answer passes (`polling::apply_evidenced_in`), the cap's stop and mail, and the operator's activity line.
pub fn by_operator(c: &Connection, tx_id: i64, text: &str, now: DateTime<Utc>) -> Result<Operator, EngineError> {
    let refuse = |why: Vec<String>| EngineError::Data(format!("transaction {tx_id} not reconciled: {}", why.join("; ")));
    let b: Bundle = serde_json::from_str(text).map_err(|e| refuse(vec![format!("unreadable evidence: {e}")]))?;
    let (facts, fees, terminal_at) = validate(&b, tx_id, now).map_err(refuse)?;
    let tx = model::immediate(c)?;
    let row: Option<RowFacts> = tx.query_row(
        "SELECT t.bot_id, t.external_id, t.status, t.created_at, e.type FROM transactions t JOIN exchanges e ON e.id = t.exchange_id WHERE t.id = ?1",
        [tx_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
    let Some((Some(bot_id), external_id, status, created_at, exchange)) = row else { return Err(refuse(vec!["no such order row".into()])) };
    if exchange != "Exchanges::Kucoin" { return Err(refuse(vec!["not a KuCoin row".into()])); }
    let record = match crate::venue::kucoin::order_record(&tx, tx_id).map_err(data)? {
        Some(record) => record,
        // UTA r6.1: a classic row (no record, its route the classic one, never an unproven adoption) takes Classic evidence:
        // its record is written here, from the row, and goes with this transaction.
        None if b.order.family == "Classic" && !unproven(&tx, tx_id)? => classic_record(&tx, tx_id, now)?,
        None => return Err(refuse(vec!["not an HF order with a recorded route (only a positive identity match establishes one)".into()])),
    };
    let mut why = vec![];
    // r1.7: evidence of one family never settles another's order.
    if record["family"].as_str() != Some(b.order.family.as_str()) { why.push("order.family is not the order's".into()); }
    let classic = record["family"] == "Classic";
    if external_id.as_deref() != Some(b.order.id.as_str()) { why.push("order.id is not the row's order".into()); }
    if record["symbol"].as_str() != Some(b.order.symbol.as_str()) { why.push("order.symbol is not the order's".into()); }
    if record["sell"].as_bool() != Some(b.order.side == "sell") || !matches!(b.order.side.as_str(), "buy" | "sell") { why.push("order.side is not the order's".into()); }
    if record["limit"].as_bool() != Some(b.order.kind == "limit") || !matches!(b.order.kind.as_str(), "market" | "limit") { why.push("order.type is not the order's".into()); }
    // A classic row stores no key identity (r12.5): its own order id binds the evidence.
    if !classic && (record["host"].as_str() != Some(b.account.host.as_str()) || record["key_fingerprint"].as_str() != Some(b.account.key_fingerprint.as_str())) {
        why.push("account is not the one the order was placed with".into());
    }
    if !why.is_empty() { return Err(refuse(why)); }
    let (user, exchange_id) = owner(&tx, tx_id)?;
    let known = known_fills(&tx, user, exchange_id, &b.order.id, &b.order.family)?;
    if facts.size < known.size || facts.funds < known.funds { return Err(refuse(vec!["executed is lower than the fills already recorded for it".into()])); }
    if known.fees.iter().any(|(currency, amount)| fees.get(currency).is_none_or(|f| f < amount)) { return Err(refuse(vec!["fees are lower than the fills already recorded for it".into()])); }
    let evidence = serde_json::to_value(&b).map_err(|e| data(e.to_string()))?;
    let extra = json!({ "fees": fees_json(&fees), "sha256": b.source.sha256.to_lowercase(), "evidence": evidence });
    // A settled row: the same facts are a replay (an API settlement takes the fees once); anything else is a conflict.
    if let Some(settled) = settled(&tx, tx_id, &record)? {
        if settled.facts != facts { return Err(refuse(vec![format!("conflict: the order is already settled as {} {} / {}", settled.facts.status, settled.facts.size.to_s_f(), settled.facts.funds.to_s_f())])); }
        // None: settled before receipts; the gate below confirms it and settles what it still owes.
        if let Some(mut r) = settled.receipt {
                if r["fees"].is_null() {
                    for (k, v) in extra.as_object().expect("an object") { r[k] = v.clone(); }
                    crate::app_config::set_plain(&tx, &format!("{RECEIPT}{tx_id}"), &r.to_string(), now).map_err(data)?;
                } else if r["fees"] != extra["fees"] {
                    return Err(refuse(vec!["conflict: the order is already settled with other fees".into()]));
                }
                clear(&tx, tx_id)?;
                tx.commit()?;
                return Ok(Operator::AlreadyReconciled);
        }
    } else if status != Some(0) {
        return Err(refuse(vec!["the row is not a submitted order".into()]));
    }
    let created = crate::codec::parse_time(&created_at).map_err(|e| data(format!("{e:?}")))?;
    if terminal_at < created { return Err(refuse(vec!["terminal.at is before the order was placed".into()])); }
    let recorded = (decimal(&record, "deal_size", tx_id)?, decimal(&record, "deal_funds", tx_id)?);
    if facts.size < recorded.0 || facts.funds < recorded.1 { return Err(refuse(vec!["executed is lower than KuCoin already reported for it".into()])); }
    let price = if facts.size.is_positive() {
        let decimals: Option<i64> = tx.query_row("SELECT k.price_decimals FROM tickers k JOIN transactions t ON t.exchange_id = k.exchange_id AND t.base_asset_id = k.base_asset_id \
                                                  AND t.quote_asset_id = k.quote_asset_id WHERE t.id = ?1", [tx_id], |r| r.get(0)).optional()?;
        facts.funds.div(&facts.size).map(|p| match decimals { Some(d) => p.round(u8::try_from(d).unwrap_or(u8::MAX)), None => p })
    } else { None };
    // A market order closed with nothing dealt is the existing zero-fill reading (D-KUCOIN-ZERO-FILL-FUNDS: `closed_empty`).
    let zero_fill_at = (facts.status == "closed" && !facts.size.is_positive()).then_some(terminal_at);
    let state = OrderState { zero_fill_at, txid: b.order.id.clone(), status: if facts.status == "closed" { OrderStatus::Closed } else { OrderStatus::Cancelled },
                             price, amount: None, quote_amount: None, amount_exec: facts.size.clone(), quote_amount_exec: Some(facts.funds.clone()),
                             limit: b.order.kind == "limit", sell: b.order.side == "sell", pair: Some(b.order.symbol.clone()), asset_class: None };
    let fenced = model::check_credential_result(&tx, &None)?;
    let cap = super::polling::apply_evidenced_in(&fenced, bot_id, tx_id, &state, "operator", extra.clone(), now)?;
    // The gate may still refuse (an obligation it found): then nothing of this commits.
    let Some(mut r) = receipt(&tx, tx_id)? else { return Err(refuse(vec![obligation(&tx, tx_id)?.and_then(|o| o["why"].as_str().map(str::to_string)).unwrap_or_else(|| "it did not settle".into())])) };
    if r["source"] != "operator" {
        // Settled before receipts: the gate confirmed it as the API's; the operator's evidence is the receipt.
        for (k, v) in extra.as_object().expect("an object") { r[k] = v.clone(); }
        r["source"] = json!("operator");
        crate::app_config::set_plain(&tx, &format!("{RECEIPT}{tx_id}"), &r.to_string(), now).map_err(data)?;
    }
    if let Some(cap) = cap {
        super::amount_limit::owe_mail(&tx, bot_id, cap, now)?;
        super::tick::stop_for_amount_limit(&tx, bot_id, cap, now)?;
    }
    model::log_activity(&tx, bot_id, "placement_ambiguous", model::Level::Warning,
        json!({ "error": "resolved by the operator", "resolution": "reconciled", "source": "operator", "order_id": b.order.id }), now)?;
    drop(fenced);
    tx.commit()?;
    Ok(Operator::Reconciled)
}

fn unproven(c: &Connection, tx_id: i64) -> Result<bool, EngineError> {
    Ok(crate::app_config::get_plain(c, &format!("{ORIGIN}{tx_id}")).map_err(data)?.is_some_and(|v| v.contains("\"unproven\"")))
}

/// UTA r6.1: a KuCoin classic row's record, written from the row (its ticker's symbol, side, type, exact executed totals).
fn classic_record(c: &Connection, tx_id: i64, now: DateTime<Utc>) -> Result<Value, EngineError> {
    let row = crate::figures::fill::stored_order(c, tx_id)?;
    let (symbol, sell, limit): (Option<String>, bool, bool) = c.query_row(
        "SELECT (SELECT CASE WHEN count(*) = 1 THEN max(k.ticker) END FROM tickers k WHERE k.exchange_id = t.exchange_id AND k.base_asset_id = t.base_asset_id \
         AND k.quote_asset_id = t.quote_asset_id), coalesce(t.side, 0) = 1, coalesce(t.order_type, 0) = 1 FROM transactions t WHERE t.id = ?1",
        [tx_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
    let exact = |v: Option<BigDec>| v.unwrap_or_else(BigDec::zero).to_s_f();
    let record = json!({ "family": "Classic", "symbol": symbol, "sell": sell, "limit": limit, "deal_size": exact(row.amount_exec), "deal_funds": exact(row.quote_amount_exec) });
    crate::app_config::set_plain(c, &format!("{}{tx_id}", crate::venue::kucoin::ORDER_RECORD), &record.to_string(), now).map_err(data)?;
    Ok(record)
}

/// UTA r3.1, r12.3: the account's mode recorded before a bot's reversal cancels and polls (`sync::kucoin_hf::observe_mode`,
/// which holds the history's record-less rows when it sees a switch), on the balance sync's condition. False: the
/// venue is KuCoin and its mode could not be detected, so the caller skips what a switch could change.
pub async fn observe<V: crate::venue::Venue>(c: &Connection, venue: &V, now: DateTime<Utc>) -> Result<bool, EngineError> {
    use crate::venue::kucoin_account::AccountMode;
    let mode = match venue.kucoin_mode().await { None => return Ok(true), Some(Err(_)) => return Ok(false), Some(Ok(mode)) => mode };
    // Under the venue's own credentials: a mode read under a replaced key is not this key's.
    let Some(version) = venue.producer() else { return Ok(true) };
    let id = version.id;
    model::credential_write(c, &Some(version), |tx| {
        let key = crate::sync::load_key_of(tx, id, &[crate::venue::kucoin::KUCOIN]).map_err(|e| data(e.0))?;
        if mode != AccountMode::ClassicLf || crate::sync::kucoin_hf::enabled(tx, key.user_id, key.exchange_id).map_err(|e| data(e.0))? {
            crate::sync::kucoin_hf::observe_mode(tx, &key, mode, now).map_err(|e| data(e.0))?;
        }
        Ok(())
    })?;
    Ok(true)
}

/// D-KUCOIN-HF-RETENTION (§5): keeps what `Venue::origin_with` proved of adopted row `tx_id`, in one fenced write: classic,
/// its mark; HF, its record (dealt totals floored at the row's own execution and its known fills, so no smaller answer
/// replaces what the row already holds) and no mark. Returns the record the row is read by from now on (None: classic).
pub(crate) fn prove(c: &Connection, bot_id: i64, tx_id: i64, origin: &crate::venue::Origin, version: &Option<model::CredentialVersion>, now: DateTime<Utc>) -> Result<Option<Value>, EngineError> {
    use crate::venue::Origin;
    model::credential_write(c, version, |tx| {
        super::polling::require_owner(tx, bot_id, tx_id)?;
        let key = format!("{ORIGIN}{tx_id}");
        match origin {
            Origin::Unproven => Err(data("an unproven origin is not kept".into())),
            Origin::Classic => {
                crate::app_config::set_plain(tx, &key, r#"{"origin":"classic"}"#, now).map_err(data)?;
                Ok(None)
            }
            Origin::Hf(record) => {
                let row = crate::figures::fill::stored_order(tx, tx_id)?;
                let ext: String = tx.query_row("SELECT external_id FROM transactions WHERE id = ?1", [tx_id], |r| r.get(0))?;
                let (user, exchange) = owner(tx, tx_id)?;
                let known = known_fills(tx, user, exchange, &ext, "ClassicHf")?;
                let floor = |a: Option<BigDec>, b: &BigDec| a.filter(|a| a > b).unwrap_or_else(|| b.clone());
                let mut record = record.clone();
                record["deal_size"] = json!(floor(row.amount_exec, &known.size).to_s_f());
                record["deal_funds"] = json!(floor(row.quote_amount_exec, &known.funds).to_s_f());
                crate::app_config::set_plain(tx, &format!("{}{tx_id}", crate::venue::kucoin::ORDER_RECORD), &record.to_string(), now).map_err(data)?;
                crate::app_config::delete(tx, &key).map_err(data)?;
                // The barrier moves, never lapses: the row owes until an HF answer of it applies.
                mark(tx, tx_id, "its route is proven, its state not read yet", now)?;
                Ok(Some(record))
            }
        }
    })
}

/// D-KUCOIN-HF-RETENTION (r2.6): in the claim's transaction, before the claim moves `rust_taken_over_at`, every open KuCoin
/// row with no HF record and no mark is marked by who placed it: `classic` when it was created in this engine's ownership
/// window (after the last takeover and before a handback since; Rust places HF orders only with a record), `unproven`
/// otherwise (a Rails window, or older than any recorded takeover). A mark is never rewritten. Returns the rows marked.
pub(crate) fn mark_adopted(c: &Connection, cipher: &crate::crypto::Cipher, now: DateTime<Utc>) -> Result<usize, EngineError> {
    use crate::lease;
    let after_crash = lease::read(c, cipher)?.is_some_and(|v| v["engine"] == "rust");
    let (taken, back) = (lease::plain_at(c, lease::TAKEN_OVER_AT)?, lease::plain_at(c, lease::HANDED_BACK_AT)?);
    // [start, end): this engine's last window. A crash restart is still in it; any other claim follows a window that ended
    // at its handback (none recorded: no window can be proven).
    let window = taken.map(|start| {
        let end = back.filter(|b| *b > start);
        (start, if after_crash { end } else { Some(end.unwrap_or(start)) })
    });
    let mut s = c.prepare(&format!(
        "SELECT t.id, t.created_at FROM transactions t JOIN exchanges e ON e.id = t.exchange_id WHERE e.type = 'Exchanges::Kucoin' AND t.status = 0 \
         AND t.external_status IN (0, 1) AND t.external_id IS NOT NULL AND t.external_id NOT LIKE 'imported_%' \
         AND NOT EXISTS (SELECT 1 FROM app_configs k WHERE k.key IN ('{}' || t.id, '{ORIGIN}' || t.id)) ORDER BY t.id", crate::venue::kucoin::ORDER_RECORD))?;
    let rows = s.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?.collect::<Result<Vec<_>, _>>()?;
    drop(s);
    for (id, created) in &rows {
        let created = crate::codec::parse_time(created).map_err(|e| data(format!("transaction {id}: {e:?}")))?;
        let ours = window.is_some_and(|(start, end)| created >= start && end.is_none_or(|end| created < end));
        let mark = if ours { r#"{"origin":"classic"}"# } else { r#"{"origin":"unproven"}"# };
        crate::app_config::set_plain(c, &format!("{ORIGIN}{id}"), mark, now).map_err(data)?;
    }
    Ok(rows.len())
}

/// D-KUCOIN-HF-RETENTION (R08): what Rails could not manage after a handback: a bot with an HF or UTA order (any row with
/// a record, or an unresolved intent sent on an HF or UTA account). Rails trades only classic, so that account is Rust's. An adopted
/// row whose route is unproven is not refused: Rails placed it, on its own route, and reads it as it always did.
pub(crate) fn handback_refusals(c: &Connection) -> Result<Vec<String>, EngineError> {
    let prefix = crate::venue::kucoin::ORDER_RECORD;
    let upper = format!("{};", &prefix[..prefix.len() - 1]);
    let mut s = c.prepare("SELECT DISTINCT t.bot_id FROM app_configs k JOIN transactions t ON t.id = CAST(substr(k.key, ?2) AS INTEGER) \
                           WHERE k.key >= ?1 AND k.key < ?3 AND t.bot_id IS NOT NULL AND coalesce(json_extract(k.value, '$.family'), '') <> 'Classic' \
                           UNION SELECT id FROM bots WHERE json_extract(transient_data, '$.rust_placement.kucoin_account_mode') IN ('ClassicHf', 'UnsupportedUta') ORDER BY 1")?;
    let found = s.query_map(params![prefix, prefix.len() as i64 + 1, upper], |r| r.get::<_, i64>(0))?
        .map(|id| id.map(|id| format!("bot {id}: KuCoin HF orders are Rust-only"))).collect::<Result<_, _>>()?;
    Ok(found)
}
