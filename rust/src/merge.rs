//! #710 slice 6 (PLAN-710-6 §6.2): Merge. A duplicate account D (`identity::duplicates`) joins its survivor S in one
//! transaction on the engine's write path: its keys, bots, live rules, orders, ledger rows (deduplicated by venue id, their
//! metadata kept) and KuCoin coverage move to S, its balances and the user's derived history go, and D is tombstoned.
//! Anything in flight, or any record the two disagree on, refuses the whole Merge with nothing written.
use crate::identity::{self, Account, AccountRow};
use crate::venue_ids::{self, Scope};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Why a Merge did not happen.
#[derive(Debug)]
pub enum Refusal {
    /// An order or a withdrawal of either account is still in flight.
    InFlight,
    /// Records the two disagree on, each named by its date, type, amount and asset.
    Conflict(Vec<String>),
    Db(crate::engine::EngineError),
}
impl From<rusqlite::Error> for Refusal { fn from(e: rusqlite::Error) -> Self { Self::Db(e.into()) } }
impl From<crate::engine::EngineError> for Refusal { fn from(e: crate::engine::EngineError) -> Self { Self::Db(e) } }

/// What a Merge did: the accounts, and the keys it deleted and kept (the caller's after-commit work).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Merged { pub duplicate: i64, pub survivor: i64, pub deleted_keys: Vec<i64>, pub kept_keys: Vec<i64> }

/// Whether a bot of `account` settles an order: a placement intent (an ambiguous placement's marker included) or an
/// order still polled (the key delete's check, #710 §4.2).
pub fn settling(c: &Connection, account: &Account) -> Result<bool, crate::engine::EngineError> {
    let mut stmt = c.prepare(&format!("SELECT b.id FROM bots b WHERE {} ORDER BY b.id", account.bots("b")))?;
    let ids = stmt.query_map([], |r| r.get::<_, i64>(0))?.collect::<Result<Vec<_>, _>>()?;
    for id in ids {
        let bot = crate::engine::model::load_bot(c, id)?;
        if bot.rust_placement().is_some() || !crate::engine::polling::waiting_ids(c, &bot)?.is_empty() { return Ok(true); }
    }
    Ok(false)
}

/// SQL: a placed standalone order (kept as the cancel's log) of account `account` the pre-flight has not found terminal.
pub fn unsettled_standalone(account: i64) -> String {
    format!("SELECT o.id FROM rust_api_orders o JOIN rust_api_order_standalone s ON s.order_id = o.id WHERE s.account_id = {account} \
             AND o.bot_id IS NULL AND o.state = 'done' AND json_extract(o.outcome, '$.kind') = 'placed' AND json_extract(o.outcome, '$.terminal') IS NULL")
}

/// The pre-flight's record: standalone order `order_id` is `status` at its venue (filled, cancelled, rejected, expired). It
/// stays a retained log, for a later cancel.
pub fn record_terminal(c: &Connection, order_id: i64, status: &str) -> rusqlite::Result<usize> {
    c.execute("UPDATE rust_api_orders SET outcome = json_set(outcome, '$.terminal', ?2) WHERE id = ?1 AND json_extract(outcome, '$.kind') = 'placed'", params![order_id, status])
}

/// A record named in a conflict: its date, type, amount and asset.
fn named(c: &Connection, row: i64) -> rusqlite::Result<String> {
    c.query_row("SELECT substr(transacted_at, 1, 10), entry_type, CAST(base_amount AS TEXT), base_currency FROM account_transactions WHERE id = ?1", [row],
                |r| Ok(format!("{} {} {} {}", r.get::<_, String>(0)?, kind(r.get(1)?), r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?.unwrap_or_default())))
}
fn order_named(c: &Connection, order: i64) -> rusqlite::Result<String> {
    c.query_row("SELECT substr(created_at, 1, 10), CASE side WHEN 1 THEN 'sell' ELSE 'buy' END, CAST(coalesce(amount, quote_amount) AS TEXT), coalesce(base, '') FROM transactions WHERE id = ?1", [order],
                |r| Ok(format!("{} {} {} {}", r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, String>(3)?)))
}
fn kind(entry_type: i64) -> &'static str {
    match entry_type { 0 => "buy", 1 => "sell", 2 => "swap in", 3 => "swap out", 4 => "deposit", 5 => "withdrawal", _ => "entry" }
}

/// One ledger row of D, or its twin in S: the fields the merge keeps.
#[derive(Clone, Debug)]
struct Meta { manual: serde_json::Map<String, serde_json::Value>, order: Option<i64>, rejected: bool }
fn meta(c: &Connection, row: i64) -> rusqlite::Result<Meta> {
    c.query_row("SELECT manual_values, transaction_id, coalesce(transfer_link_rejected, 0) FROM account_transactions WHERE id = ?1", [row], |r| {
        let manual = r.get::<_, Option<String>>(0)?.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        Ok(Meta { manual, order: r.get(1)?, rejected: r.get(2)? })
    })
}

/// The ledger plan of step 7: each D row's twin, the merged metadata of each twin, and the final transfer links.
struct Ledger { twin: HashMap<i64, i64>, merged: Vec<(i64, Meta)>, sources: BTreeSet<i64>, edges: BTreeMap<i64, i64>, conflicts: Vec<String> }

fn ledger(c: &Connection, sd: &Scope, ss: &Scope) -> rusqlite::Result<Ledger> {
    let mut s = c.prepare(&format!("SELECT a.id, {} FROM account_transactions a WHERE a.user_id = ?1 AND a.exchange_id = ?2 AND {} ORDER BY a.id", sd.row_id("a"), sd.rows("a")))?;
    let rows: Vec<(i64, Option<String>)> = s.query_map([sd.user_id, sd.exchange_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    let mut twin = HashMap::new();
    for (row, id) in &rows {
        let Some(id) = id else { continue };
        let found: Option<i64> = c.query_row(&format!("SELECT a.id FROM account_transactions a WHERE a.user_id = ?1 AND a.exchange_id = ?2 AND {}", ss.id_is("a", "?3")),
                                             params![ss.user_id, ss.exchange_id, id], |r| r.get(0)).optional()?;
        if let Some(found) = found { twin.insert(*row, found); }
    }
    let mut conflicts = vec![];
    let mut merged = vec![];
    for (d, s) in &twin {
        let (dm, mut sm) = (meta(c, *d)?, meta(c, *s)?);
        let mut clash = false;
        for (k, v) in &dm.manual {
            match sm.manual.get(k) { Some(mine) if mine != v => clash = true, Some(_) => {}, None => { sm.manual.insert(k.clone(), v.clone()); } }
        }
        match (sm.order, dm.order) { (Some(a), Some(b)) if a != b => clash = true, (None, Some(b)) => sm.order = Some(b), _ => {} }
        sm.rejected |= dm.rejected;
        if clash { conflicts.push(named(c, *s)?); }
        merged.push((*s, sm));
    }
    // The links as one graph (r3): every edge touching a row of D or a twin, normalised through the twins.
    let mapped = |id: i64| *twin.get(&id).unwrap_or(&id);
    let touched: BTreeSet<i64> = rows.iter().map(|(r, _)| *r).chain(twin.values().copied()).collect();
    let list = touched.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    let mut s = c.prepare(&format!("SELECT id, linked_transaction_id FROM account_transactions WHERE linked_transaction_id IS NOT NULL AND (id IN ({list}) OR linked_transaction_id IN ({list})) ORDER BY id"))?;
    let links: Vec<(i64, i64)> = if touched.is_empty() { vec![] } else { s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()? };
    let sources: BTreeSet<i64> = links.iter().map(|(w, _)| *w).collect();
    let mut edges: BTreeMap<i64, i64> = BTreeMap::new();
    let mut targets: BTreeMap<i64, i64> = BTreeMap::new();
    let rejected: HashMap<i64, bool> = merged.iter().map(|(s, m)| (*s, m.rejected)).collect();
    let is_rejected = |c: &Connection, id: i64| -> rusqlite::Result<bool> {
        match rejected.get(&id) { Some(r) => Ok(*r), None => c.query_row("SELECT coalesce(transfer_link_rejected, 0) FROM account_transactions WHERE id = ?1", [id], |r| r.get(0)) }
    };
    for (w, t) in links {
        let (w, t) = (mapped(w), mapped(t));
        if w == t { continue; }
        let bad = edges.get(&w).is_some_and(|x| *x != t) || targets.get(&t).is_some_and(|x| *x != w) || is_rejected(c, w)? || is_rejected(c, t)?;
        if bad { conflicts.push(named(c, w)?); continue; }
        edges.insert(w, t);
        targets.insert(t, w);
    }
    conflicts.sort();
    conflicts.dedup();
    Ok(Ledger { twin, merged, sources, edges, conflicts })
}

/// Merge duplicate `duplicate_id` of `user` into its survivor, in the caller's write transaction (`engine::model::immediate`).
/// None: it is no longer a duplicate of the user's (a 404). Any refusal leaves the transaction for the caller to roll back.
pub fn run(c: &Connection, user: i64, duplicate_id: i64, now: DateTime<Utc>) -> Result<Option<Merged>, Refusal> {
    // 0. Re-derived in the transaction.
    let Some(&(d, s)) = identity::duplicates(c, user)?.iter().find(|(d, _)| *d == duplicate_id) else { return Ok(None) };
    let (Some(rd), Some(rs)) = (AccountRow::by_id(c, d)?, AccountRow::by_id(c, s)?) else { return Ok(None) };
    let (ad, as_) = (rd.account(), rs.account());
    let (sd, ss) = (rd.venue_ids(), rs.venue_ids());
    let exchange = rd.exchange_id;

    // 1. In flight.
    let bots = format!("(SELECT b.id FROM bots b WHERE {} OR {})", ad.bots("b"), as_.bots("b"));
    let waiting: bool = c.query_row(&format!(
        "SELECT EXISTS(SELECT 1 FROM rust_api_orders o LEFT JOIN rust_api_order_standalone x ON x.order_id = o.id WHERE o.state IN ('queued', 'taken', 'sending') \
           AND (x.account_id IN ({d}, {s}) OR o.bot_id IN {bots})) \
         OR EXISTS({}) OR EXISTS({}) \
         OR EXISTS(SELECT 1 FROM rust_withdrawal_intents WHERE owner IN (SELECT 'rule:' || id FROM exchange_account_rules WHERE account_id IN ({d}, {s})))",
        unsettled_standalone(d), unsettled_standalone(s)), [], |r| r.get(0))?;
    if waiting || settling(c, &ad)? || settling(c, &as_)? { return Err(Refusal::InFlight); }

    // 2. Conflicts, before any write.
    let mut conflicts = vec![];
    let mut o = c.prepare(&format!("SELECT t.id, {} FROM transactions t WHERE t.exchange_id = ?1 AND {} ORDER BY t.id", venue_ids::order_id(c, "t")?, sd.orders("t")))?;
    let orders: Vec<(i64, Option<String>)> = o.query_map([exchange], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    drop(o);
    for (order, external) in &orders {
        let Some(external) = external else { continue };
        if let Some(other) = venue_ids::order_by_id(c, &ss, external)?.filter(|o| o != order) {
            conflicts.push(order_named(c, *order)?);
            conflicts.push(order_named(c, other)?);
        }
    }
    let binance: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE key LIKE 'rust_binance_a12_v1:%' AND (instr(key, ?1) > 0 OR substr(key, -length(?2)) = ?2))",
                                    [format!(":{user}:{exchange}@{d}:"), format!(":{user}:{exchange}@{d}")], |r| r.get(0))?;
    if binance { conflicts.push("Binance CSV import".into()); }
    let plan = ledger(c, &sd, &ss)?;
    conflicts.extend(plan.conflicts.iter().cloned());
    if !conflicts.is_empty() { return Err(Refusal::Conflict(conflicts)); }

    // 3. Keys, per key type.
    let mut k = c.prepare(&format!("SELECT k.id, k.key_type, k.status FROM api_keys k WHERE {} ORDER BY k.id", ad.keys("k")))?;
    let keys_d: Vec<(i64, i64, i64)> = k.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    drop(k);
    let (mut deleted, mut kept) = (vec![], vec![]);
    for key_type in [0, 1, 2] {
        let mine: Vec<i64> = keys_d.iter().filter(|(_, t, _)| *t == key_type).map(|(id, _, _)| *id).collect();
        let Some(&first) = mine.first() else { if let Some(k) = as_.key(c, key_type)? { kept.push(k) } continue };
        let theirs: Option<(i64, i64)> = match as_.key(c, key_type)? { Some(id) => Some((id, c.query_row("SELECT status FROM api_keys WHERE id = ?1", [id], |r| r.get(0))?)), None => None };
        let (keep, gone): (i64, Vec<i64>) = match theirs {
            Some((ks, 1)) => (ks, mine.clone()),
            Some((ks, _)) => { move_key(c, first, &as_)?; (first, std::iter::once(ks).chain(mine.iter().skip(1).copied()).collect()) }
            None => { move_key(c, first, &as_)?; (first, mine.iter().skip(1).copied().collect()) }
        };
        for id in gone { remove_key(c, id, Some(keep))?; deleted.push(id); }
        kept.push(keep);
    }

    // 4. Bots: status and settings unchanged.
    let mut b = c.prepare(&format!("SELECT b.id FROM bots b WHERE {} ORDER BY b.id", ad.bots("b")))?;
    let moved: Vec<i64> = b.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    drop(b);
    for bot in moved { identity::place_bot(c, bot, user, exchange, Some(as_), now)?; }

    // 5. Rules, collision-safe; D's deleted rules stay on D as history.
    let mut r = c.prepare("SELECT id, type, asset_id FROM exchange_account_rules WHERE account_id = ?1 AND status <> 3 ORDER BY id")?;
    let rules: Vec<(i64, String, Option<i64>)> = r.query_map([d], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    drop(r);
    let number_one = rs.number == 1;
    for (rule, kind, asset) in rules {
        let theirs: Option<(i64, i64)> = c.query_row("SELECT id, status FROM exchange_account_rules WHERE account_id = ?1 AND type = ?2 AND asset_id IS ?3",
                                                     params![s, kind, asset], |r| Ok((r.get(0)?, r.get(1)?))).optional()?;
        match theirs {
            Some((_, status)) if status != 3 => {
                c.execute("UPDATE exchange_account_rules SET status = 3, updated_at = ?2 WHERE id = ?1", params![rule, crate::codec::format_time(now)])?;
            }
            Some((theirs, _)) => {
                c.execute("UPDATE rules SET exchange_id = NULL, asset_id = NULL, status = 3 WHERE id = ?1", [theirs])?;
                let row: Vec<rusqlite::types::Value> = c.query_row("SELECT * FROM exchange_account_rules WHERE id = ?1", [theirs], |r| (0..r.as_ref().column_count()).map(|i| r.get(i)).collect())?;
                let columns: Vec<String> = c.prepare("SELECT * FROM exchange_account_rules LIMIT 0")?.column_names().iter().map(|s| s.to_string()).collect();
                c.execute("DELETE FROM exchange_account_rules WHERE id = ?1", [theirs])?;
                c.execute("UPDATE exchange_account_rules SET account_id = ?2 WHERE id = ?1", [rule, s])?;
                let at = columns.iter().position(|n| n == "account_id").expect("account_id");
                let mut values = row;
                values[at] = rusqlite::types::Value::Integer(d);
                c.execute(&format!("INSERT INTO exchange_account_rules ({}) VALUES ({})", columns.join(","), vec!["?"; columns.len()].join(",")), rusqlite::params_from_iter(values))?;
                if number_one { crate::engine::rules::mirror(c, Some(rule))?; }
            }
            None => {
                c.execute("UPDATE exchange_account_rules SET account_id = ?2 WHERE id = ?1", [rule, s])?;
                if number_one { crate::engine::rules::mirror(c, Some(rule))?; }
            }
        }
    }
    // Withdrawal ids: D's move where S lacks the venue id; the rest are the same withdrawal seen twice.
    c.execute("UPDATE rust_withdrawal_ids SET account_id = ?2 WHERE account_id = ?1 AND venue_id NOT IN (SELECT venue_id FROM rust_withdrawal_ids WHERE account_id = ?2)", [d, s])?;
    c.execute("DELETE FROM rust_withdrawal_ids WHERE account_id = ?1", [d])?;

    // 6. Orders, then standalone orders (their index is not unique).
    venue_ids::move_orders(c, &sd, &ss)?;
    c.execute("UPDATE rust_api_order_standalone SET account_id = ?2 WHERE account_id = ?1", [d, s])?;

    // 7. Ledger rows: every affected link cleared, the twins deleted (their membership with them), the rest moved, the
    // twins' merged metadata written, then the final links installed: no step meets a unique index its counterpart holds.
    if !plan.sources.is_empty() {
        c.execute(&format!("UPDATE account_transactions SET linked_transaction_id = NULL WHERE id IN ({})", plan.sources.iter().map(i64::to_string).collect::<Vec<_>>().join(",")), [])?;
    }
    for d_row in plan.twin.keys() { c.execute("DELETE FROM account_transactions WHERE id = ?1", [d_row])?; }
    venue_ids::move_rows(c, &sd, &ss)?;
    for (row, m) in &plan.merged {
        c.execute("UPDATE account_transactions SET manual_values = ?2, transaction_id = ?3, transfer_link_rejected = ?4 WHERE id = ?1",
                  params![row, serde_json::Value::Object(m.manual.clone()).to_string(), m.order, m.rejected])?;
    }
    for (w, t) in &plan.edges { c.execute("UPDATE account_transactions SET linked_transaction_id = ?2 WHERE id = ?1", [w, t])?; }

    // 8. KuCoin coverage (the HF marker once) and withheld fills (a twin once).
    c.execute("DELETE FROM kucoin_coverage WHERE account_id = ?1 AND source = ?3 AND EXISTS (SELECT 1 FROM kucoin_coverage WHERE account_id = ?2 AND source = ?3)",
              params![d, s, crate::sync::kucoin_hf::ENABLED])?;
    c.execute("UPDATE kucoin_coverage SET account_id = ?2 WHERE account_id = ?1", [d, s])?;
    c.execute("UPDATE kucoin_withheld_fills SET account_id = ?2 WHERE account_id = ?1 AND tx_id NOT IN (SELECT tx_id FROM kucoin_withheld_fills WHERE account_id = ?2)", [d, s])?;
    c.execute("DELETE FROM kucoin_withheld_fills WHERE account_id = ?1", [d])?;

    // 9. Balances, the user's derived history, D's provenance.
    c.execute("DELETE FROM exchange_account_balances WHERE account_id = ?1", [d])?;
    crate::tracker::invalidate_user(c, user)?;
    c.execute("DELETE FROM app_configs WHERE key = ?1", [crate::sync::cache::balance_origin_key_of(&rd)])?;
    c.execute("DELETE FROM app_configs WHERE key LIKE ?1", [crate::jobs::state::key("snapshot_origin", Some(&format!("{user}:{exchange}:{d}:%")))])?;
    c.execute("UPDATE app_configs SET value = json_remove(value, ?2) WHERE key = ?1 AND json_valid(value)",
              params![crate::sync::kucoin_hf::gap_key(user), format!("$.\"{exchange}:{d}\"")])?;

    // 11. Observations and the tombstone.
    c.execute("UPDATE exchange_account_overlaps SET other_account_id = ?2 WHERE other_account_id = ?1", [d, s])?;
    c.execute("DELETE FROM exchange_account_overlaps WHERE account_id = ?1", [d])?;
    identity::tombstone(c, d, now)?;
    Ok(Some(Merged { duplicate: d, survivor: s, deleted_keys: deleted, kept_keys: kept }))
}

/// Key `id` of the duplicate joins `to` (its membership goes to `to`, none for a number 1).
fn move_key(c: &Connection, id: i64, to: &Account) -> rusqlite::Result<()> {
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [id])?;
    to.add_key(c, id)
}
/// Key `id` goes as `keys::remove` removes one, no bot stopped: its rows' key is the kept key of its type (else none).
fn remove_key(c: &Connection, id: i64, keep: Option<i64>) -> rusqlite::Result<()> {
    c.execute("UPDATE account_transactions SET api_key_id = ?2 WHERE api_key_id = ?1", params![id, keep])?;
    c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_sync.%' AND substr(key, -length(?1) - 1) = ':' || ?1", [id.to_string()])?;
    c.execute("DELETE FROM api_keys WHERE id = ?1", [id])?;
    c.execute("DELETE FROM exchange_account_credentials WHERE api_key_id = ?1", [id])?;
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [id])?;
    crate::venue::ibkr_account::forget(c, id)
}
