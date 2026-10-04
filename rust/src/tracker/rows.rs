//! The account's ledger rows as the tracker reads them: every `account_transactions` row of the user but the futures
//! and margin wallets' (Tracker::UnfundedCash.borrowed?), in the order every reader of the ledger walks them
//! (Tax::PriceService.ordered), each checked against what this build walks.
use super::{cash, fiat, refused, VENUE_TYPE};
use crate::figures::{at::At, dec::Dec, FiguresError};
use chrono::NaiveDate;
use rusqlite::Connection;
use serde_json::Value;

/// AccountTransaction's `entry_type` enum, in its stored order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Buy, Sell, SwapIn, SwapOut, Deposit, Withdrawal, StakingReward, LendingInterest, Airdrop, Mining, Fee, OtherIncome,
    Lost, WithholdingTax, ReturnOfCapital, Adjustment, Unsupported,
}

impl Kind {
    pub fn from_stored(n: i64) -> Option<Kind> {
        use Kind::*;
        [Buy, Sell, SwapIn, SwapOut, Deposit, Withdrawal, StakingReward, LendingInterest, Airdrop, Mining, Fee, OtherIncome,
         Lost, WithholdingTax, ReturnOfCapital, Adjustment, Unsupported].get(usize::try_from(n).ok()?).copied()
    }
    /// Tracker::UnfundedCash::BASE_IN.
    pub fn base_in(self) -> bool {
        use Kind::*;
        matches!(self, Buy | SwapIn | StakingReward | LendingInterest | Airdrop | Mining | OtherIncome | Deposit | Adjustment)
    }
    /// Tracker::UnfundedCash::BASE_OUT.
    pub fn base_out(self) -> bool { use Kind::*; matches!(self, Sell | SwapOut | Withdrawal | Fee | Lost | WithholdingTax) }
    /// Tracker::Ledger::IN_KIND: what arrives without a purchase behind it.
    pub fn in_kind(self) -> bool { use Kind::*; matches!(self, StakingReward | LendingInterest | Airdrop | Mining | OtherIncome) }
}

/// One stored row, as the walk needs it.
#[derive(Clone, Debug)]
pub struct Stored {
    pub id: i64,
    pub exchange_id: i64,
    pub kind: Kind,
    pub base: String,
    pub amount: Dec,
    pub quote: Option<String>,
    pub quote_amount: Option<Dec>,
    pub tx_id: Option<String>,
    pub group: Option<String>,
    pub at: At,
    /// `raw_data['per_share_amount']`, a return of capital's rate per share.
    pub per_share: Option<Dec>,
    /// `manual_value(:price)`: a USD price the user stated.
    pub stated: Option<Dec>,
    /// The deposit a withdrawal is linked to.
    pub linked_to: Option<i64>,
}

impl Stored {
    /// The day of its instant, in UTC (every job runs under Time.zone = UTC).
    pub fn date(&self) -> NaiveDate { self.at.utc().date_naive() }
    /// AccountTransaction#quoted?.
    pub fn quoted(&self) -> bool { self.quote_amount.is_some() && self.quote.as_deref().is_some_and(cash) }
}

/// What sorting `n` items costs, in comparisons: n log n.
pub fn sort_steps(n: usize) -> u64 { let n = n as u64; n.saturating_mul(u64::from(64 - n.leading_zeros())) }

/// Tracker::UnfundedCash.borrowed?: a futures or margin row, recognised by the id the importers give it.
pub fn borrowed(tx_id: Option<&str>) -> bool { ["futures", "margin-interest", "liquidation-"].iter().any(|m| tx_id.unwrap_or("").contains(m)) }

fn data(what: impl std::fmt::Display) -> FiguresError { FiguresError::Data(what.to_string()) }

/// `value.presence&.to_d` on a JSON value: absent, null and blank text are no number.
fn json_number(v: Option<&Value>) -> Result<Option<Dec>, FiguresError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(v) => Ok(Some(Dec::to_d(v)?)),
    }
}

fn json_column(text: Option<String>) -> Result<Value, FiguresError> {
    match text {
        None => Ok(Value::Null),
        Some(t) => serde_json::from_str(&t).map_err(|_| data("a JSON column Rails did not write")),
    }
}

/// Every row of the user the tracker walks, in Tax::PriceService.ordered's order: by instant; within one, a group sits
/// where its first-stored leg was, out-legs ahead of in-legs; everything else by id. Refused when a row is one this
/// build does not walk, or when the account holds balances on another venue. Every row is charged to the open budget
/// scope, and so is the sort.
pub fn load(c: &Connection, user_id: i64) -> Result<Vec<Stored>, FiguresError> {
    // Today's rows count every venue's balances, whether or not it has rows.
    let elsewhere: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_balances b JOIN exchanges e ON e.id = b.exchange_id \
                                       WHERE b.user_id = ?1 AND e.type IS NOT ?2)", rusqlite::params![user_id, VENUE_TYPE], |r| r.get(0))?;
    if elsewhere { return Err(refused("balances on a venue other than Alpaca")); }
    let mut s = c.prepare(
        "SELECT t.id, t.exchange_id, e.type, t.entry_type, t.base_currency, t.base_amount, t.quote_currency, t.quote_amount, \
                t.fee_currency, t.fee_amount, t.tx_id, t.group_id, t.transacted_at, t.raw_data, t.manual_values, t.linked_transaction_id \
         FROM account_transactions t JOIN exchanges e ON e.id = t.exchange_id WHERE t.user_id = ?1 ORDER BY t.id")?;
    let mut q = s.query([user_id])?;
    let mut rows = vec![];
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0)?;
        // Before the borrowed wallets are set aside: the backfill applies their rows (only Binance writes them).
        let venue: Option<String> = r.get(2)?;
        if venue.as_deref() != Some(VENUE_TYPE) { return Err(refused("rows of a venue other than Alpaca")); }
        let tx_id: Option<String> = r.get(10)?;
        if borrowed(tx_id.as_deref()) { continue; }
        let kind = Kind::from_stored(r.get(3)?).ok_or_else(|| data("an entry type Rails does not have"))?;
        if matches!(kind, Kind::SwapIn | Kind::SwapOut) { return Err(refused("swap legs")); }
        let (fee_currency, fee_amount): (Option<String>, Option<Dec>) = (r.get(8)?, Dec::from_sql(r.get_ref(9)?)?);
        if fee_currency.is_some() || fee_amount.is_some() { return Err(refused("a fee carried on a row")); }
        let base: String = r.get(4)?;
        let quote: Option<String> = r.get(6)?;
        if [Some(base.as_str()), quote.as_deref()].into_iter().flatten().any(|c| fiat(c) && c != "USD") { return Err(refused("cash other than US dollars")); }
        let quote_amount = Dec::from_sql(r.get_ref(7)?)?;
        // AccountTransaction#quoted?: an amount without a currency, or in a coin, is no cash quote.
        if matches!(kind, Kind::Buy | Kind::Sell) && !(quote_amount.is_some() && quote.as_deref().is_some_and(cash)) {
            return Err(refused("a trade leg with no cash quote of its own"));
        }
        let at_text: String = r.get(12)?;
        let raw = json_column(r.get(13)?)?;
        let manual = json_column(r.get(14)?)?;
        rows.push(Stored {
            id: r.get(0)?, exchange_id: r.get(1)?, kind, base,
            amount: Dec::from_sql(r.get_ref(5)?)?.ok_or_else(|| data("a row with no base amount"))?,
            quote, quote_amount, tx_id, group: r.get::<_, Option<String>>(11)?.filter(|g| !g.trim().is_empty()),
            at: At::from_sql(&at_text).ok_or_else(|| data(format!("a time Rails did not write: {at_text:?}")))?,
            per_share: json_number(raw.get("per_share_amount"))?,
            stated: json_number(manual.get("price"))?,
            linked_to: r.get(15)?,
        });
    }
    // A linked transfer is walked only in dollars: a coin's moves its lots, which this build does not port.
    let linked_to: std::collections::HashSet<i64> = rows.iter().filter_map(|r| r.linked_to).collect();
    if rows.iter().any(|r| (r.linked_to.is_some() || linked_to.contains(&r.id)) && r.base != "USD") { return Err(refused("a linked transfer of anything but US dollars")); }
    crate::figures::budget::charge(sort_steps(rows.len()), 0)?;
    Ok(ordered(rows))
}

/// Tax::PriceService.ordered: `[transacted_at, anchor, out-leg first, id]`, the anchor being the smallest id of the
/// row's group (by exchange and group id), or its own id without a group.
pub fn ordered(mut rows: Vec<Stored>) -> Vec<Stored> {
    let mut anchors: std::collections::HashMap<(i64, Option<String>), i64> = std::collections::HashMap::new();
    for r in &rows { anchors.entry((r.exchange_id, r.group.clone())).and_modify(|a| *a = (*a).min(r.id)).or_insert(r.id); }
    let key = |r: &Stored| {
        let anchor = if r.group.is_some() { anchors.get(&(r.exchange_id, r.group.clone())).copied().unwrap_or(r.id) } else { r.id };
        (r.at.0, anchor, if matches!(r.kind, Kind::Sell | Kind::SwapOut) { 0 } else { 1 }, r.id)
    };
    rows.sort_by_cached_key(key);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(id: i64, kind: Kind, group: Option<&str>, at: &str) -> Stored {
        Stored { id, exchange_id: 1, kind, base: "AAPL".into(), amount: Dec::one(), quote: None, quote_amount: None, tx_id: None,
                 group: group.map(String::from), at: At::from_sql(at).unwrap(), per_share: None, stated: None, linked_to: None }
    }

    /// Tax::PriceService.ordered: by instant; a group where its first-stored leg was, its out-legs first; else by id.
    #[test]
    fn rows_walk_in_rails_order() {
        let t = "2026-09-02 14:30:00";
        let rows = vec![stored(6, Kind::Buy, Some("g"), t), stored(2, Kind::Deposit, None, "2026-09-03 00:00:00"), stored(7, Kind::Sell, Some("g"), t),
                        stored(5, Kind::Buy, None, t), stored(1, Kind::Fee, None, t)];
        assert_eq!(ordered(rows).iter().map(|r| r.id).collect::<Vec<_>>(), [1, 5, 7, 6, 2]);
    }

    fn db(rows: &[&str]) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT); INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca'), (2, 'Exchanges::Binance');
                         CREATE TABLE account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, free NUMERIC, locked NUMERIC);
                         CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, entry_type INTEGER, base_currency TEXT,
                         base_amount NUMERIC, quote_currency TEXT, quote_amount NUMERIC, fee_currency TEXT, fee_amount NUMERIC, tx_id TEXT, group_id TEXT,
                         transacted_at TEXT, raw_data TEXT, manual_values TEXT, linked_transaction_id INTEGER);").unwrap();
        for values in rows {
            c.execute(&format!("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, \
                                fee_amount, linked_transaction_id, transacted_at, raw_data, manual_values) VALUES (1, {values}, '2026-09-01 00:00:00', '{{}}', '{{}}')"), []).unwrap();
        }
        c
    }

    /// A history with anything this build does not walk is refused, with the reason; the borrowed wallets are set aside.
    #[test]
    fn what_is_not_ported_is_refused_with_the_reason() {
        let ok = "1, 0, 'AAPL', 1, 'USD', 100, NULL, NULL, NULL";
        for (row, why) in [("2, 4, 'BTC', 1, NULL, NULL, NULL, NULL, NULL", "rows of a venue other than Alpaca"),
                           ("1, 2, 'BTC', 1, NULL, NULL, NULL, NULL, NULL", "swap legs"),
                           ("1, 0, 'AAPL', 1, 'USD', 100, 'USD', 1, NULL", "a fee carried on a row"),
                           ("1, 4, 'EUR', 100, NULL, NULL, NULL, NULL, NULL", "cash other than US dollars"),
                           ("1, 5, 'BTC', 1, NULL, NULL, NULL, NULL, 1", "a linked transfer of anything but US dollars"),
                           ("1, 0, 'AAPL', 1, NULL, NULL, NULL, NULL, NULL", "a trade leg with no cash quote of its own"),
                           ("1, 0, 'AAPL', 1, NULL, 100, NULL, NULL, NULL", "a trade leg with no cash quote of its own"),
                           ("1, 1, 'AAPL', 1, 'BTC', '0.003', NULL, NULL, NULL", "a trade leg with no cash quote of its own")] {
            let err = load(&db(&[ok, row]), 1).unwrap_err();
            assert!(matches!(&err, FiguresError::NotComputed(m) if m == &format!("the tracker walk is not ported for {why}")), "{why}: {err:?}");
        }
        // Balances on another venue, though it has no rows.
        let c = db(&[ok]);
        c.execute("INSERT INTO account_balances (user_id, exchange_id, free, locked) VALUES (1, 2, 0.5, 0)", []).unwrap();
        assert!(matches!(load(&c, 1).unwrap_err(), FiguresError::NotComputed(m) if m == "the tracker walk is not ported for balances on a venue other than Alpaca"));
        let c = db(&[ok, "1, 4, 'USD', 99, NULL, NULL, NULL, NULL, NULL", "1, 5, 'USD', 100, NULL, NULL, NULL, NULL, 2"]);
        c.execute("UPDATE account_transactions SET tx_id = 'usdt-futures-1' WHERE id = 1", []).unwrap();
        assert_eq!(load(&c, 1).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), [2, 3], "a linked dollar transfer is walked; a futures row is not");
    }

    /// A long history loads in linear work, charged to the budget: 100,000 rows well inside one figure's limits, and
    /// refused under a limit they exceed.
    #[test]
    fn a_long_history_loads_within_its_budget() {
        let c = db(&["1, 4, 'USD', 100, NULL, NULL, NULL, NULL, NULL"]);
        c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 2 UNION ALL SELECT i + 1 FROM n WHERE i < 100000)
                         INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, linked_transaction_id, transacted_at, raw_data, manual_values)
                         SELECT 1, 1, CASE i % 2 WHEN 0 THEN 4 ELSE 5 END, 'USD', 1, CASE WHEN i = 3 THEN 2 END, '2026-09-01 00:00:00', '{}', '{}' FROM n").unwrap();
        let (out, used) = crate::figures::budget::scope(crate::figures::budget::FIGURE, || load(&c, 1));
        assert_eq!(out.unwrap().len(), 100_000);
        assert!(used.steps >= 100_000 && used.steps < crate::figures::budget::FIGURE.steps / 100, "100,000 rows took {} steps", used.steps);
        let tight = crate::figures::budget::Limits { steps: 50_000, held: crate::figures::budget::FIGURE.held };
        let (out, _) = crate::figures::budget::scope(tight, || load(&c, 1));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET), "{:?}", out.err());
    }
}
