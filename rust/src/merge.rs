//! #710 slice 6 (PLAN-710-6 §6.2): Merge. A duplicate account D (its stored standing, `identity::duplicates`) joins its survivor S in one
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
    /// The data a Merge is checked on is older than `identity::LIVE_SECS` or missing: the syncs to wake (job, key), then try
    /// again.
    Stale(Vec<(&'static str, i64)>),
    /// A live rule of either account would withdraw with another key after the merge, each named by its account and asset.
    WithdrawalKeys(Vec<String>),
    /// The account no key can read the balances of (`identity::Live::Unreadable`).
    Unreadable(i64),
    /// Records the two disagree on, each named by its date, type, amount and asset.
    Conflict(Vec<String>),
    Db(crate::engine::EngineError),
}
impl From<rusqlite::Error> for Refusal { fn from(e: rusqlite::Error) -> Self { Self::Db(e.into()) } }
impl From<crate::engine::EngineError> for Refusal { fn from(e: crate::engine::EngineError) -> Self { Self::Db(e) } }

/// The conflict line naming an order another user's bot holds under the same venue id (shown in the user's words).
pub const FOREIGN_ORDER: &str = "\u{0}foreign_order";

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

/// A record named in a conflict: its account, date, type, amount and asset (`Kraken 2: 2026-09-01 buy 1 BTC`).
fn named(c: &Connection, row: i64) -> rusqlite::Result<String> {
    let (text, account): (String, Option<i64>) = c.query_row(&format!("SELECT substr(t.transacted_at, 1, 10), t.entry_type, CAST(t.base_amount AS TEXT), t.base_currency, {} \
                                                 FROM account_transactions t WHERE t.id = ?1", crate::venue_ids::row_account("t")), [row],
                |r| Ok((format!("{} {} {} {}", r.get::<_, String>(0)?, kind(r.get(1)?), r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, Option<String>>(3)?.unwrap_or_default()), r.get(4)?)))?;
    Ok(match account { Some(a) => format!("{}: {text}", identity::label(c, a)?.plain()), None => text })
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
/// `raw`: the venue's answer, when only the duplicate's twin has one (a restored row has none, or only a file's own fields):
/// the row kept carries it. `synced`: whether the row's raw is a venue's answer (`not_synced`).
/// `asset`, `group`: the duplicate's, written where the kept twin has none.
struct Meta { manual: serde_json::Map<String, serde_json::Value>, order: Option<i64>, rejected: bool, raw: Option<String>, synced: bool, asset: Option<i64>,
              group: Option<String>, description: Option<String> }
fn meta(c: &Connection, row: i64) -> rusqlite::Result<Meta> {
    c.query_row(&format!("SELECT x.manual_values, x.transaction_id, coalesce(x.transfer_link_rejected, 0), x.raw_data, x.base_asset_id, x.group_id, x.description, NOT {} \
                          FROM account_transactions x WHERE x.id = ?1", not_synced("x")), [row], |r| {
        let manual = r.get::<_, Option<String>>(0)?.and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        Ok(Meta { manual, order: r.get(1)?, rejected: r.get(2)?, raw: r.get::<_, Option<String>>(3)?.filter(|t| !matches!(t.as_str(), "" | "{}" | "null")), synced: r.get(7)?,
                  asset: match r.get_ref(4)? { rusqlite::types::ValueRef::Integer(i) => Some(i), _ => None },
                  group: r.get::<_, Option<String>>(5)?.filter(|g| !g.trim().is_empty()), description: r.get::<_, Option<String>>(6)?.filter(|d| !d.trim().is_empty()) })
    })
}

/// SQL condition: ledger row `x` was not written by a venue sync. A sync stores the venue's answer with the row; what
/// else writes rows stores none (`{}`: a Deltabadger CSV restore, a Binance CSV's dust and convert legs, a 2.x-era
/// hand entry or restore) or only a file's own fields: KuCoin's CSV (`source: kucoin_csv`), Binance's CSV (`operation`),
/// a restore routed to KuCoin (`amounts` and `amounts_inexact`, its exact decimals: `kucoin_hf::with_amounts`).
fn not_synced(x: &str) -> String {
    format!("COALESCE(COALESCE({x}.raw_data, '') IN ('', 'null', '{{}}') OR CASE WHEN json_valid({x}.raw_data) THEN \
             json_extract({x}.raw_data, '$.source') = 'kucoin_csv' OR (json_type({x}.raw_data) = 'object' \
             AND NOT EXISTS (SELECT 1 FROM json_each({x}.raw_data) WHERE key NOT IN ('operation', 'amounts', 'amounts_inexact'))) ELSE 0 END, 0)")
}

/// A restore twin's exact amounts (`amounts`: base, quote, fee) against an API twin's own fields (a fill's `size`,
/// `funds`, `fee`; a transfer's `amount`, `fee`), as decimals at full precision; any difference is a conflict. Twins of
/// any other shape agree here.
fn exact_amounts_agree(c: &Connection, d: i64, s: i64) -> rusqlite::Result<bool> {
    use crate::v2::BigDec;
    use serde_json::value::RawValue;
    // Each field as written: a number's own text, never through a float (an API amount may be unquoted and wider).
    type Fields = std::collections::HashMap<String, Box<RawValue>>;
    let raw = |row: i64| -> rusqlite::Result<Option<Fields>> {
        let t: Option<String> = c.query_row("SELECT raw_data FROM account_transactions WHERE id = ?1", [row], |r| r.get(0))?;
        Ok(t.and_then(|t| serde_json::from_str::<Fields>(&t).ok()))
    };
    let (Some(rd), Some(rs)) = (raw(d)?, raw(s)?) else { return Ok(true) };
    let (restore, api) = match (rd.contains_key("amounts"), rs.contains_key("amounts")) {
        (true, false) if rs.contains_key("createdAt") => (&rd, &rs),
        (false, true) if rd.contains_key("createdAt") => (&rs, &rd),
        _ => return Ok(true),
    };
    let Some(amounts) = restore.get("amounts").and_then(|a| serde_json::from_str::<Fields>(a.get()).ok()) else { return Ok(true) };
    let decimal = |v: &RawValue| -> Option<BigDec> {
        let text = v.get().trim();
        let text = if text.starts_with('"') { serde_json::from_str::<String>(text).ok()? } else { text.to_string() };
        BigDec::parse(text.trim()).ok()
    };
    let abs = |d: BigDec| if d < BigDec::zero() { &BigDec::zero() - &d } else { d };
    for (mine, theirs) in [("base", api.get("size").or_else(|| api.get("amount"))), ("quote", api.get("funds")), ("fee", api.get("fee"))] {
        let (Some(a), Some(b)) = (amounts.get(mine).and_then(|v| decimal(v)), theirs.and_then(|v| decimal(v))) else { continue };
        if abs(a) != abs(b) { return Ok(false); }
    }
    Ok(true)
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
    // A twin pair is one record only when every financial fact agrees (an API copy with another fee, a split aggregate
    // against one of its legs, are not): otherwise a conflict naming both rows.
    for (d, s) in &twin {
        // The raw answers are compared only when both are venue answers (a restore's own fields are no venue's); two
        // restores' exact amounts (KuCoin's, beside float columns that can hide a difference) are compared too.
        // The amounts as stored, exactly (`IS`: numbers by value, text by its characters), never through a cast to text,
        // which rounds a REAL to 15 digits.
        let same: bool = c.query_row(&format!("SELECT d.entry_type IS s.entry_type AND d.base_currency IS s.base_currency AND d.base_amount IS s.base_amount \
            AND d.quote_currency IS s.quote_currency AND d.quote_amount IS s.quote_amount AND d.fee_currency IS s.fee_currency \
            AND d.fee_amount IS s.fee_amount AND d.transacted_at IS s.transacted_at \
            AND ({} OR {} OR json(d.raw_data) IS json(s.raw_data)) \
            AND (NOT (json_valid(d.raw_data) AND json_valid(s.raw_data)) OR json_extract(d.raw_data, '$.amounts') IS NULL OR json_extract(s.raw_data, '$.amounts') IS NULL \
                 OR (json(json_extract(d.raw_data, '$.amounts')) IS json(json_extract(s.raw_data, '$.amounts')) \
                     AND json_extract(d.raw_data, '$.amounts_inexact') IS json_extract(s.raw_data, '$.amounts_inexact'))) \
            FROM account_transactions d, account_transactions s WHERE d.id = ?1 AND s.id = ?2", not_synced("d"), not_synced("s")), [d, s], |r| r.get(0))?;
        // The asset and the description, blank as the plan judges it (`meta`): a blank one agrees with any.
        let (md, ms) = (meta(c, *d)?, meta(c, *s)?);
        let agree = |a: &Option<String>, b: &Option<String>| a.is_none() || b.is_none() || a == b;
        let same = same && (md.asset.is_none() || ms.asset.is_none() || md.asset == ms.asset) && agree(&md.description, &ms.description);
        if !same || !exact_amounts_agree(c, *d, *s)? { conflicts.push(named(c, *s)?); conflicts.push(named(c, *d)?); }
    }
    // A row of D with no exact-id twin (an id-less one too) may be an event S holds under another spelling: another type,
    // another time, no id. Refused, naming it, when (a) no venue sync wrote it (a CSV import, a restore); (b) S holds a row
    // no venue sync wrote of its asset within a day, of any type (a withdrawal stamped at request and at completion, an
    // Earn payment under two names); (c) S holds a row of its type and asset within ten minutes (Coinbase's legacy ids).
    // Whole seconds, so each boundary is exact.
    let secs = |x: &str| format!("CAST(strftime('%s', {x}.transacted_at) AS INTEGER)");
    // A moved row's group, composed as the walks compose it (`tracker::rows::account_group`) on S after the move: one
    // another of the user's rows on the exchange already reads would join two groups into one. A conflict naming both,
    // unless one of D's rows in it is the twin of S's row in it (its legs then make one group). Each set is read once.
    let composed = |group: Option<String>, member: Option<i64>| crate::tracker::rows::account_group(group, member);
    let shared: std::collections::HashSet<String> = {
        let mut out = std::collections::HashSet::new();
        for (dr, sr) in &twin {
            let g = |id: &i64| c.query_row("SELECT group_id FROM account_transactions WHERE id = ?1", [id], |r| r.get::<_, Option<String>>(0));
            if let (Some(gd), Some(gs)) = (g(dr)?, g(sr)?) { if gd == gs { out.insert(gd); } }
        }
        out
    };
    let mut q = c.prepare(&format!("SELECT a.id, a.group_id, {} FROM account_transactions a WHERE a.user_id = ?1 AND a.exchange_id = ?2 AND NOT ({}) AND a.group_id IS NOT NULL",
                                   crate::venue_ids::Scope::install(c)?.member_sql("a"), sd.rows("a")))?;
    let others: Vec<(i64, Option<String>, Option<i64>)> = q.query_map([ss.user_id, ss.exchange_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    drop(q);
    let mut taken: HashMap<String, i64> = HashMap::new();
    for (id, group, member) in others { if let Some(key) = composed(group, member) { taken.entry(key).or_insert(id); } }
    for (row, _) in rows.iter().filter(|(row, _)| !twin.contains_key(row)) {
        let group: Option<String> = c.query_row("SELECT group_id FROM account_transactions WHERE id = ?1", [row], |r| r.get(0))?;
        let Some(group) = group.filter(|g| !g.trim().is_empty()) else { continue };
        if shared.contains(&group) { continue; }
        if let Some(theirs) = composed(Some(group), ss.member).and_then(|key| taken.get(&key).copied()) { conflicts.push(named(c, theirs)?); conflicts.push(named(c, *row)?); }
    }
    for (row, _) in rows.iter().filter(|(row, _)| !twin.contains_key(row)) {
        let refused: bool = c.query_row(&format!("SELECT {} OR EXISTS(SELECT 1 FROM account_transactions s WHERE s.user_id = ?1 AND s.exchange_id = ?2 AND {} \
                                                    AND s.base_currency IS d.base_currency AND ((({}) AND abs({} - {}) <= 86400) \
                                                    OR (s.entry_type = d.entry_type AND abs({} - {}) <= 600))) \
                                                  FROM account_transactions d WHERE d.id = ?3",
                                                 not_synced("d"), ss.rows("s"), not_synced("s"), secs("s"), secs("d"), secs("s"), secs("d")),
                                         params![ss.user_id, ss.exchange_id, row], |r| r.get(0))?;
        if refused { conflicts.push(named(c, *row)?); }
    }
    let mut merged = vec![];
    for (d, s) in &twin {
        let (dm, mut sm) = (meta(c, *d)?, meta(c, *s)?);
        let mut clash = false;
        for (k, v) in &dm.manual {
            match sm.manual.get(k) { Some(mine) if mine != v => clash = true, Some(_) => {}, None => { sm.manual.insert(k.clone(), v.clone()); } }
        }
        match (sm.order, dm.order) { (Some(a), Some(b)) if a != b => clash = true, (None, Some(b)) => sm.order = Some(b), _ => {} }
        sm.rejected |= dm.rejected;
        // A venue answer always beats a restore's shape: the kept twin takes the duplicate's answer over its own restore.
        sm.raw = if sm.raw.is_none() || (!sm.synced && dm.synced) { dm.raw.clone() } else { None };
        sm.asset = if sm.asset.is_none() { dm.asset } else { None };
        sm.description = if sm.description.is_none() { dm.description.clone() } else { None };
        // A group: D's where S's row has none and no other row reads its key on S (the group is S's then), by the same key
        // as a moved row's; two groups are a conflict.
        sm.group = match (&sm.group, &dm.group) {
            (Some(a), Some(b)) => { if a != b { clash = true; } None }
            (None, Some(g)) => {
                match composed(Some(g.clone()), ss.member).and_then(|key| taken.get(&key).copied()) {
                    Some(theirs) => { conflicts.push(named(c, theirs)?); clash = true; None }
                    None => Some(g.clone()),
                }
            }
            _ => None,
        };
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
/// `survivor`: the account the confirmation named; another derived survivor is a 404 (None).
pub fn run(c: &Connection, cipher: &crate::crypto::Cipher, user: i64, duplicate_id: i64, survivor: i64, now: DateTime<Utc>) -> Result<Option<Merged>, Refusal> {
    // 0. The stored standing, in the transaction: the survivor the confirmation named, its keys and credentials as at entry.
    let Some((s, basis, since)) = identity::standing_of(c, user, duplicate_id)? else { return Ok(None) };
    let d = duplicate_id;
    if s != survivor { return Ok(None); }
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
         OR EXISTS(SELECT 1 FROM rust_withdrawal_intents i WHERE i.owner IN (SELECT 'rule:' || id FROM exchange_account_rules WHERE account_id IN ({d}, {s})) \
           OR i.api_key_id IN (SELECT k.id FROM api_keys k WHERE {} OR {})) \
         OR EXISTS(SELECT 1 FROM app_configs x JOIN transactions t ON t.id = CAST(substr(x.key, instr(x.key, ':') + 1) AS INTEGER) \
           JOIN bots ob ON ob.id = t.bot_id AND ob.user_id = {user} \
           WHERE (x.key LIKE '{}%' OR (x.key LIKE '{}%' AND json_extract(x.value, '$.origin') = 'unproven')) AND t.exchange_id = {exchange} AND ({} OR {}))",
        unsettled_standalone(d), unsettled_standalone(s), ad.keys("k"), as_.keys("k"), crate::engine::reconcile::UNRESOLVED, crate::engine::reconcile::ORIGIN,
        sd.orders("t"), ss.orders("t")), [], |r| r.get(0))?;
    if waiting || settling(c, &ad)? || settling(c, &as_)? || recorder_waiting(c, cipher, &[rd, rs])? { return Err(Refusal::InFlight); }
    // Withdrawal keys: each live rule withdraws with its account's key as the withdrawal engine picks it
    // (`engine::model::withdrawal_key_id`); read now, and again on the merged key set below. D's rules step 5 drops beside
    // S's live one of their asset withdraw no more.
    let mut q = c.prepare("SELECT r.id, r.account_id, coalesce(a.symbol, '') FROM exchange_account_rules r LEFT JOIN assets a ON a.id = r.asset_id
                           WHERE r.account_id IN (?1, ?2) AND r.status <> 3 AND NOT (r.account_id = ?1 AND EXISTS (SELECT 1 FROM exchange_account_rules x
                             WHERE x.account_id = ?2 AND x.type = r.type AND x.asset_id IS r.asset_id AND x.status <> 3)) ORDER BY r.account_id, r.id")?;
    let live_rules: Vec<(i64, i64, String)> = q.query_map([d, s], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<Result<_, _>>()?;
    drop(q);
    let (wd, ws) = (crate::engine::model::withdrawal_key_id(c, &ad)?, crate::engine::model::withdrawal_key_id(c, &as_)?);
    let mut withdrawing = vec![];
    for (rule, account, symbol) in live_rules {
        withdrawing.push((rule, if account == d { wd } else { ws }, format!("{}: {symbol}", identity::label(c, account)?.plain())));
    }
    // The live check: fresh syncs of both accounts, on which the pair still agrees.
    match identity::live_check(c, &rd, &rs, now)? {
        identity::Live::Fresh => {}
        identity::Live::Stale(wake) => return Err(Refusal::Stale(wake)),
        identity::Live::Differ => return Ok(None),
        identity::Live::Unreadable(account) => return Err(Refusal::Unreadable(account)),
        identity::Live::Unresolved(held) => return Err(Refusal::Conflict(held)),
    }
    // The whole stored component: every other member's fresh reads too, and no veto between any two of its accounts.
    let mut q = c.prepare("SELECT duplicate_id FROM exchange_account_standings WHERE survivor_id = ?1 AND duplicate_id <> ?2 ORDER BY duplicate_id")?;
    let others: Vec<i64> = q.query_map([s, d], |r| r.get(0))?.collect::<Result<_, _>>()?;
    drop(q);
    for other in &others {
        let Some(ro) = AccountRow::by_id(c, *other)? else { return Ok(None) };
        match identity::live_check(c, &ro, &rs, now)? {
            identity::Live::Fresh => {}
            identity::Live::Stale(wake) => return Err(Refusal::Stale(wake)),
            identity::Live::Differ => return Ok(None),
            identity::Live::Unreadable(account) => return Err(Refusal::Unreadable(account)),
            identity::Live::Unresolved(held) => return Err(Refusal::Conflict(held)),
        }
    }
    let members: Vec<i64> = [d, s].into_iter().chain(others).collect();
    for (i, x) in members.iter().enumerate() { for y in &members[i + 1..] { if identity::vetoed(c, *x, *y)? { return Ok(None); } } }
    // The standing as entered: both accounts' keys and credentials unchanged since.
    if basis != identity::standing_basis(c, d, s)? || identity::identity_changed_since(c, d, s, &since)? { return Ok(None); }

    // 2. Conflicts, before any write.
    let mut conflicts = vec![];
    let mut o = c.prepare(&format!("SELECT t.id, {} FROM transactions t WHERE t.exchange_id = ?1 AND {} ORDER BY t.id", venue_ids::order_id(c, "t")?, sd.orders("t")))?;
    let orders: Vec<(i64, Option<String>)> = o.query_map([exchange], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
    drop(o);
    for (order, external) in &orders {
        let Some(external) = external else { continue };
        if let Some(other) = venue_ids::order_by_id(c, &ss, external)?.filter(|o| o != order) {
            conflicts.push(order_named(c, *order)?);
            // A number 1's index spans every user's bots: another user's order is named by nothing of its own.
            let mine: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM transactions t JOIN bots b ON b.id = t.bot_id WHERE t.id = ?1 AND b.user_id = ?2)", [other, user], |r| r.get(0))?;
            conflicts.push(if mine { order_named(c, other)? } else { FOREIGN_ORDER.to_string() });
        }
    }
    // The A12 buckets and repair state, and the history repair's per-account completion (`rust_job.binance_history_repair_account:…@<D>`).
    let binance: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM app_configs WHERE (key LIKE 'rust_binance_a12_v1:%' OR key LIKE 'rust_job.binance_history_repair_account:%') \
                                     AND (instr(key, ?1) > 0 OR substr(key, -length(?2)) = ?2))",
                                    [format!(":{user}:{exchange}@{d}:"), format!(":{user}:{exchange}@{d}")], |r| r.get(0))?;
    if binance { conflicts.push("Binance CSV import".into()); }
    // A withheld KuCoin fill both hold under one id: the same fill only when its entry is the same.
    let mut w = c.prepare("SELECT x.tx_id FROM kucoin_withheld_fills x JOIN kucoin_withheld_fills y ON y.tx_id = x.tx_id AND y.account_id = ?2 \
                           WHERE x.account_id = ?1 AND (json(x.entry) IS NOT json(y.entry) OR x.transacted_at IS NOT y.transacted_at) ORDER BY x.tx_id")?;
    let withheld: Vec<String> = w.query_map([d, s], |r| r.get(0))?.collect::<Result<_, _>>()?;
    drop(w);
    for tx in withheld { conflicts.push(format!("KuCoin fill {tx}")); }
    // A fill withheld on one account and visible on the other is one fill too: the same only when the visible row's
    // venue answer and time are the withheld entry's. D's identical withheld copy of a fill S shows goes in step 8.
    let crossed = |withheld_on: i64, visible: &Scope| -> rusqlite::Result<Vec<(String, i64, bool)>> {
        let mut q = c.prepare(&format!("SELECT x.tx_id, a.id, json(x.entry) IS json(a.raw_data) AND x.transacted_at IS a.transacted_at FROM kucoin_withheld_fills x \
                                        JOIN account_transactions a ON a.user_id = ?2 AND a.exchange_id = ?3 AND {} AND {} WHERE x.account_id = ?1 ORDER BY x.tx_id",
                                       visible.rows("a"), visible.id_is("a", "x.tx_id")))?;
        let out = q.query_map(params![withheld_on, user, exchange], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect();
        out
    };
    let (on_s, on_d) = (crossed(s, &sd)?, crossed(d, &ss)?);
    for (tx, _, same) in on_s.iter().chain(&on_d) { if !same { conflicts.push(format!("KuCoin fill {tx}")); } }
    // S's withheld copy of a fill D shows identically goes, and D's visible row moves as any other, with its metadata
    // (manual values, order link, transfer links, rejected flag): the visible copy is the one that carries them.
    if conflicts.is_empty() {
        for (tx, _, _) in &on_s { c.execute("DELETE FROM kucoin_withheld_fills WHERE account_id = ?1 AND tx_id = ?2", params![s, tx])?; }
    }
    let plan = ledger(c, &sd, &ss)?;
    conflicts.extend(plan.conflicts.iter().cloned());
    if !conflicts.is_empty() { return Err(Refusal::Conflict(conflicts)); }

    // 3. Keys, per key type (PLAN-710-6 §4.8: the other row is deleted): exactly one key of each type stays on the merged
    // account, the survivor's correct key, else the duplicate's lowest correct one, else the survivor's, else the
    // duplicate's lowest; every other key of the type goes through `remove_key` (its rows re-keyed, its pending work
    // handed on).
    let keys_of = |a: &Account| -> rusqlite::Result<Vec<(i64, i64, i64)>> {
        let mut k = c.prepare(&format!("SELECT k.id, k.key_type, k.status FROM api_keys k WHERE {} ORDER BY k.status <> 1, k.id", a.keys("k")))?;
        let out = k.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect();
        out
    };
    let (keys_d, keys_s) = (keys_of(&ad)?, keys_of(&as_)?);
    let mut deleted = vec![];
    for key_type in [0, 1, 2] {
        let of = |keys: &[(i64, i64, i64)]| -> Vec<(i64, i64)> { keys.iter().filter(|(_, t, _)| *t == key_type).map(|(id, _, st)| (*id, *st)).collect() };
        let (mine, theirs) = (of(&keys_d), of(&keys_s));
        // Each list is correct first, then by id: its first entry is its best.
        let keep = match (theirs.first(), mine.first()) {
            (Some(&(ks, 1)), _) => ks,
            (_, Some(&(kd, 1))) => kd,
            (Some(&(ks, _)), _) => ks,
            (None, Some(&(kd, _))) => kd,
            (None, None) => continue,
        };
        if mine.iter().any(|(id, _)| *id == keep) { move_key(c, keep, &as_)?; }
        for (id, _) in theirs.iter().chain(&mine).filter(|(id, _)| *id != keep) { remove_key(c, *id, Some(keep))?; deleted.push(*id); }
    }
    // Every key left on the merged account.
    let kept: Vec<i64> = keys_of(&as_)?.into_iter().map(|(id, _, _)| id).collect::<std::collections::BTreeSet<i64>>().into_iter().collect();

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
    // A rule whose withdrawal key changed on the merged key set (none to some included) refuses, named: the whole Merge
    // rolls back with the caller's transaction.
    let after = crate::engine::model::withdrawal_key_id(c, &as_)?;
    let switching: Vec<String> = withdrawing.into_iter().filter(|(_, before, _)| *before != after).map(|(_, _, line)| line).collect();
    if !switching.is_empty() { return Err(Refusal::WithdrawalKeys(switching)); }
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
        // A blank is no value, here as in the plan (`meta`): '' is NULL for each carried column.
        // Blankness was decided in the plan (`meta`, Rust's Unicode whitespace): each carried value is set only where the
        // plan found the survivor's blank, as the duplicate wrote it; a non-blank survivor value stays as it is.
        c.execute("UPDATE account_transactions SET manual_values = ?2, transaction_id = ?3, transfer_link_rejected = ?4, raw_data = COALESCE(?5, raw_data), \
                   base_asset_id = COALESCE(?6, base_asset_id), group_id = COALESCE(?7, group_id), description = COALESCE(?8, description) WHERE id = ?1",
                  params![row, serde_json::Value::Object(m.manual.clone()).to_string(), m.order, m.rejected, m.raw, m.asset, m.group, m.description])?;
    }
    for (w, t) in &plan.edges { c.execute("UPDATE account_transactions SET linked_transaction_id = ?2 WHERE id = ?1", [w, t])?; }

    // The duplicate's import observations are the survivor's.
    c.execute("UPDATE exchange_account_imports SET account_id = ?2 WHERE account_id = ?1", [d, s])?;
    // 8. KuCoin coverage (the HF marker once) and withheld fills (a twin once).
    c.execute("DELETE FROM kucoin_coverage WHERE account_id = ?1 AND source = ?3 AND EXISTS (SELECT 1 FROM kucoin_coverage WHERE account_id = ?2 AND source = ?3)",
              params![d, s, crate::sync::kucoin_hf::ENABLED])?;
    c.execute("UPDATE kucoin_coverage SET account_id = ?2 WHERE account_id = ?1", [d, s])?;
    // D's withheld fill S shows visibly is that fill already (identical, step 2): it goes with the rest below.
    let shown = ss.id_is("a", "kucoin_withheld_fills.tx_id");
    c.execute(&format!("UPDATE kucoin_withheld_fills SET account_id = ?2 WHERE account_id = ?1 AND tx_id NOT IN (SELECT tx_id FROM kucoin_withheld_fills WHERE account_id = ?2) \
                        AND NOT EXISTS (SELECT 1 FROM account_transactions a WHERE a.user_id = ?3 AND a.exchange_id = ?4 AND {} AND {shown})", ss.rows("a")),
              params![d, s, user, exchange])?;
    // The rest are S's twins, equal in every fact (step 2 refused any other).
    c.execute("DELETE FROM kucoin_withheld_fills WHERE account_id = ?1", [d])?;

    // 9. Balances, the user's derived history, D's provenance.
    c.execute("DELETE FROM exchange_account_balances WHERE account_id = ?1", [d])?;
    crate::tracker::invalidate_user(c, user)?;
    c.execute("DELETE FROM app_configs WHERE key = ?1", [crate::sync::cache::balance_origin_key_of(&rd)])?;
    c.execute("DELETE FROM app_configs WHERE key LIKE ?1", [crate::jobs::state::key("snapshot_origin", Some(&format!("{user}:{exchange}:{d}:%")))])?;
    c.execute("UPDATE app_configs SET value = json_remove(value, ?2) WHERE key = ?1 AND json_valid(value)",
              params![crate::sync::kucoin_hf::gap_key(user), format!("$.\"{exchange}:{d}\"")])?;

    // 11. The tombstone; the keys moved and removed cleared the standings of both (key events); the exchange's standing
    // is reconsidered once, on the merged account.
    identity::tombstone(c, d, now)?;
    // Evidence restarts: every kept key's change marker is the merge time, so no row imported before the Merge (its own,
    // or one re-keyed to it from a deleted key) is shared-trade evidence for the merged account again.
    for k in &kept { crate::app_config::set_plain(c, &identity::credential_changed_key(*k), &crate::codec::format_time(now), now).map_err(rusqlite::Error::InvalidParameterName)?; }
    // Pending split restatements go to the key that runs the merged account's ledger sync (the scheduler's ledger key).
    // The scheduler's own choice (`sync::ledger_reading_keys`), of the merged account's keys.
    let reader = crate::sync::ledger_reading_keys(c).map_err(|e| rusqlite::Error::InvalidParameterName(e.0))?.into_iter()
        .find(|k| identity::Account::of_key(c, *k).ok().flatten() == Some(as_));
    if let Some(reader) = reader {
        for k in kept.iter().filter(|k| **k != reader) { crate::sync::ledger::hand_owed(c, *k, reader).map_err(|e| rusqlite::Error::InvalidParameterName(e.0))?; }
    }
    identity::reconsider(c, user, exchange, now)?;
    Ok(Some(Merged { duplicate: d, survivor: s, deleted_keys: deleted, kept_keys: kept }))
}

/// Whether a recorder's withdrawal intent is on one of `accounts`. Its account is its request's (user, exchange)'s number 1
/// (`withdrawal::intent_account`), whatever became of its key; one whose request cannot be read counts.
fn recorder_waiting(c: &Connection, cipher: &crate::crypto::Cipher, accounts: &[AccountRow]) -> Result<bool, Refusal> {
    let mut q = c.prepare("SELECT params_enc FROM rust_withdrawal_intents WHERE owner LIKE 'recorder:%' ORDER BY owner")?;
    let params: Vec<String> = q.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
    for p in params {
        let Some(v) = cipher.decrypt(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { return Ok(true) };
        if accounts.iter().any(|a| a.number == 1 && v["user_id"].as_i64() == Some(a.user_id) && v["exchange_id"].as_i64() == Some(a.exchange_id)) { return Ok(true); }
    }
    Ok(false)
}

/// Key `id` of the duplicate joins `to` (its membership goes to `to`, none for a number 1).
fn move_key(c: &Connection, id: i64, to: &Account) -> rusqlite::Result<()> {
    // A key event of the account it leaves; `add_key` is the one of the account it joins.
    identity::key_event_of_key(c, id)?;
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [id])?;
    // Its balance time was the duplicate's balances' (deleted): read again before it can bring S's movements forward.
    c.execute("UPDATE api_keys SET balances_synced_at = NULL WHERE id = ?1", [id])?;
    c.execute("DELETE FROM app_configs WHERE key = ?1", [format!("rust_sync.read_at:{id}")])?;
    to.add_key(c, id)
}
/// Key `id` goes as `keys::remove` removes one, no bot stopped: its rows' key is the kept key of its type (else none).
fn remove_key(c: &Connection, id: i64, keep: Option<i64>) -> rusqlite::Result<()> {
    identity::key_event_of_key(c, id)?;
    // Its pending split restatements become the kept key's, merged with its own (`sync::ledger` owes them by key); the
    // run's end hands them on to the merged account's ledger key.
    if let Some(keep) = keep { crate::sync::ledger::hand_owed(c, id, keep).map_err(|e| rusqlite::Error::InvalidParameterName(e.0))?; }
    c.execute("UPDATE account_transactions SET api_key_id = ?2 WHERE api_key_id = ?1", params![id, keep])?;
    c.execute("DELETE FROM app_configs WHERE key LIKE 'rust_sync.%' AND substr(key, -length(?1) - 1) = ':' || ?1", [id.to_string()])?;
    c.execute("DELETE FROM api_keys WHERE id = ?1", [id])?;
    c.execute("DELETE FROM exchange_account_credentials WHERE api_key_id = ?1", [id])?;
    c.execute("DELETE FROM exchange_account_keys WHERE api_key_id = ?1", [id])?;
    crate::venue::ibkr_account::forget(c, id)
}
