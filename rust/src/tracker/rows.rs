//! The account's ledger rows as the tracker reads them, in two sets, as 2.x has them:
//! - `load_all`: every `account_transactions` row of the user, the futures and margin wallets' included, by instant
//!   (`AccountTransaction.by_date_asc`): what PortfolioSnapshot::BackfillJob reads;
//! - `walk_rows`: those rows less the borrowed wallets' (Tracker::UnfundedCash.borrowed?), in the order every reader of
//!   the ledger walks them (Tax::PriceService.ordered): what Tracker::Ledger walks.
use super::{cash, fiat, stable};
pub use super::spellings::Label;
use crate::figures::{at::At, dec::Dec, FiguresError};
use chrono::NaiveDate;
use rusqlite::Connection;
use serde_json::Value;

/// AccountTransaction's `entry_type` enum, in its stored order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// Tracker::Ledger::COSTS: rows whose base leaving is a cost.
    pub fn cost(self) -> bool { matches!(self, Kind::Fee | Kind::WithholdingTax | Kind::Lost) }
    /// Tracker::Ledger::ACQUISITIONS.
    pub fn acquisition(self) -> bool { self.in_kind() || matches!(self, Kind::Buy | Kind::SwapIn) }
    /// Tax::PriceService's IN_LEGS / OUT_LEGS.
    pub fn in_leg(self) -> bool { matches!(self, Kind::Buy | Kind::SwapIn) }
    pub fn out_leg(self) -> bool { matches!(self, Kind::Sell | Kind::SwapOut) }
}

/// One stored row, as the walk and the backfill need it.
#[derive(Clone, Debug)]
pub struct Stored {
    pub id: i64,
    pub exchange_id: i64,
    /// The venue's `Exchange#name_id` ("alpaca", "binance", "binance_us").
    pub venue: String,
    pub kind: Kind,
    pub base: Label,
    pub amount: Dec,
    pub quote: Option<Label>,
    pub quote_amount: Option<Dec>,
    pub fee_currency: Option<Label>,
    pub fee_amount: Option<Dec>,
    pub tx_id: Option<String>,
    pub group: Option<String>,
    pub at: At,
    /// `raw_data['per_share_amount']`, a return of capital's rate per share.
    pub per_share: Option<Dec>,
    /// `manual_value(:price)`: a USD price the user stated.
    pub stated: Option<Dec>,
    /// The deposit a withdrawal is linked to, and that deposit's venue (`linked_transaction.exchange.name_id`).
    pub linked_to: Option<i64>,
    pub to_venue: Option<String>,
    pub to_exchange_id: Option<i64>,
    /// #710 slice 5 (PLAN-710-5 §5.2): where the row sits, its account (`venue_ids::row_account`; on an install without the
    /// account tables, or a number 1 with no row yet, the negated exchange id), and the linked deposit's. The walk locates
    /// lots, cash and transfer ends by it; `venue` stays the exchange's identity (pricing, the cash rule).
    pub account: i64,
    pub to_account: Option<i64>,
    /// `base_asset`: the asset the row recorded, as (id, external id, category).
    pub base_asset: Option<super::prices::Asset>,
    /// D-TAX-CFEE-PREVIOUS-DAY: an Alpaca CFEE, and what the fold made of a fill it was charged on.
    pub cfee: Cfee,
    /// `transaction_id`: the bot order the row is a fill of.
    pub bot_order: Option<i64>,
}

/// D-TAX-CFEE-PREVIOUS-DAY: Alpaca books its crypto fee as a `CFEE` activity of its own, a coin amount dated 00:00 UTC
/// of the day AFTER the trades it was charged on (posted at the end of their day: Alpaca's activities API names the UTC
/// day after the trade). Its fills are its coin's buys on its venue that UTC day before, or its sales where it bought
/// none; it is worth what those fills were traded at. D-TAX-CFEE-MATCHED-FILL: a CFEE whose raw `created_at` says when it
/// was charged (a timed fee) is the fee of one fill (`cfee_matches`), stands right after it (`place_cfees`), and is worth
/// Alpaca's own price of it (`raw_price`); only a fee without `created_at` takes the day before.
#[derive(Clone, Debug, Default)]
pub struct Cfee {
    /// The row is an Alpaca `CFEE` activity.
    pub fee: bool,
    /// On a timed CFEE: the instant it was charged (`charged`).
    pub charged: Option<At>,
    /// On a fill or a CFEE: its raw `price` (none when absent or unreadable) and `symbol`, the matcher's identity of it.
    pub price: Option<Dec>,
    pub symbol: Option<String>,
    /// On a timed CFEE: its raw price where it is dollars (`raw_price`), and the fill it was charged on (`cfee_matches`).
    pub raw_price: Option<Dec>,
    pub fill: Option<i64>,
    /// On a CFEE: the price it was charged at (`fill_price`), and that price's currency (a stablecoin's is USD).
    pub fill_price: Option<(Dec, String)>,
    /// On a fill (the tax report's fold): its amount once its fees' shares are folded in. The row is valued on `amount`.
    pub folded: Option<Dec>,
    /// On a fill (SE, GB): the fees it carries as its expense, each valued as the fee's own row is.
    pub expenses: Vec<Expense>,
}

impl Cfee {
    /// D-TAX-CFEE-MATCHED-FILL: the fee carries its charge instant; the D−1 reading is only for one that does not.
    pub fn timed(&self) -> bool { self.charged.is_some() }
}

/// D-TAX-CFEE-MATCHED-FILL: a CFEE's raw `created_at`, the instant it was charged. Alpaca may send it to the second: such
/// an instant is read as the end of its second, so a fill a fraction later in that second precedes it.
pub fn charged(raw: &Value) -> Option<At> {
    if raw.get("activity_type").and_then(Value::as_str) != Some("CFEE") { return None; }
    let text = raw.get("created_at")?.as_str()?;
    let at = At::from_utc(chrono::DateTime::parse_from_rfc3339(text).ok()?.with_timezone(&chrono::Utc))?;
    if text.contains('.') { Some(at) } else { at.0.checked_add(999_999_999).map(At) }
}

/// The quote of an Alpaca crypto pair `symbol` of `base` ("BTCUSD", "BTC/USD"): what is left after the base and an
/// optional `/`, when it is one of Alpaca's crypto quotes.
fn pair_quote(base: &str, symbol: &str) -> Option<&'static str> {
    let rest = symbol.strip_prefix(base).filter(|_| !base.is_empty())?;
    let rest = rest.strip_prefix('/').unwrap_or(rest);
    crate::sync::ledger::CRYPTO_QUOTES.iter().find(|q| **q == rest).copied()
}

/// A raw `price`, none when absent or unreadable.
fn raw_number(raw: &Value) -> Option<Dec> { json_number(raw.get("price")).ok().flatten() }

/// D-TAX-CFEE-MATCHED-FILL: a timed CFEE's raw `price` in dollars, where it is one: its pair is the fee's coin against
/// USD, USDT or USDC, and the price positive. A coin-quoted pair's price is in that coin, and a stablecoin's own fee is
/// cash at face: none. The walk and the ledger table read it alike.
pub fn raw_price(base: &str, raw: &Value) -> Option<Dec> {
    if cash(base) { return None; }
    let quote = pair_quote(base, raw.get("symbol")?.as_str()?)?;
    if !matches!(quote, "USD" | "USDT" | "USDC") { return None; }
    raw_number(raw).filter(Dec::is_positive)
}

/// A CFEE carried as a fill's expense: the fee's attribution (shared by its fills) and this fill's place in it.
#[derive(Clone, Debug)]
pub struct Expense { pub of: std::sync::Arc<Attribution>, pub index: usize }

/// A CFEE's row, its fills' amounts (the weights) and their sum.
#[derive(Debug)]
pub struct Attribution { pub fee: Stored, pub weights: Vec<Dec>, pub total: Dec }

fn charged_on(fee: &Stored, r: &Stored) -> bool {
    matches!(r.kind, Kind::Buy | Kind::Sell) && r.account == fee.account && r.base == fee.base && r.amount.is_positive() && !r.borrowed()
        && fee.date().pred_opt() == Some(r.date())
}

/// A CFEE's fills among `rows` (stored rows, before any leg split): the buys if any, else the sales.
pub fn cfee_fills<'a>(fee: &Stored, rows: impl IntoIterator<Item = &'a Stored>) -> Vec<&'a Stored> {
    let fills: Vec<&Stored> = rows.into_iter().filter(|r| charged_on(fee, r)).collect();
    if fills.iter().any(|r| r.kind == Kind::Buy) { fills.into_iter().filter(|r| r.kind == Kind::Buy).collect() } else { fills }
}

/// The price `fills` were traded at, Σquote / Σbase, when every one is quoted in cash of one currency (a stablecoin reads
/// as USD) at a positive amount; None otherwise (a coin-quoted fill's price is a market estimate, and sync stores a fill
/// with no price as a zero quote: the fee keeps the daily close).
pub fn fill_price(fills: &[&Stored]) -> Result<Option<(Dec, String)>, FiguresError> {
    let (mut currency, mut quote, mut base) = (None::<&str>, Dec::zero(), Dec::zero());
    for r in fills {
        let (Some(c), Some(paid)) = (r.quote.as_deref().filter(|_| r.quoted()), r.quote_amount.as_ref()) else { return Ok(None) };
        if !paid.is_positive() { return Ok(None); }
        let c = if stable(c) { "USD" } else { c };
        if currency.is_some_and(|known| known != c) { return Ok(None); }
        currency = Some(c);
        (quote, base) = ((&quote + paid)?, (&base + &r.amount)?);
    }
    match currency { Some(c) if base.is_positive() => Ok(Some((quote.div(&base)?, c.to_string()))), _ => Ok(None) }
}

/// Each CFEE of `rows` (time-ordered) with the places of its fills (`cfee_fills`), through one index of the trades. A timed
/// fee is not one of them (D-TAX-CFEE-MATCHED-FILL: `Cfee::fill`).
pub fn cfee_places(rows: &[Stored]) -> Vec<(usize, Vec<usize>)> {
    let mut trades: std::collections::HashMap<(i64, &str, NaiveDate), Vec<usize>> = std::collections::HashMap::new();
    for (i, r) in rows.iter().enumerate() { if matches!(r.kind, Kind::Buy | Kind::Sell) { trades.entry((r.account, &r.base, r.date())).or_default().push(i); } }
    let place: std::collections::HashMap<i64, usize> = trades.values().flatten().map(|&i| (rows[i].id, i)).collect();
    rows.iter().enumerate().filter(|(_, r)| r.cfee.fee && r.kind == Kind::Fee && !r.cfee.timed()).filter_map(|(i, fee)| {
        let pool = trades.get(&(fee.account, fee.base.as_str(), fee.date().pred_opt()?))?;
        Some((i, cfee_fills(fee, pool.iter().map(|&j| &rows[j])).into_iter().map(|r| place[&r.id]).collect()))
    }).collect()
}

/// How far back a timed CFEE's fill can be.
const CFEE_REACH_SECONDS: i64 = 72 * 3600;

/// D-TAX-CFEE-MATCHED-FILL: whether a timed fee was charged on a day its symbol named another coin than on its booking
/// day (a dated alias, LUNA across 27 May 2022): its booking day's prices are not its coin's.
pub fn charged_as_another_coin(base: &str, venue: &str, exchange_id: i64, charged: At, booked: NaiveDate) -> bool {
    let venue = super::prices::Venue { id: exchange_id, name_id: venue.to_string(), stock: true };
    super::prices::alias_coin(base, Some(&venue), Some(charged.utc().date_naive())) != super::prices::alias_coin(base, Some(&venue), Some(booked))
}

/// D-TAX-CFEE-MATCHED-FILL: each timed CFEE of `rows` (fee id → fill id) the one fill it was charged on. A fee's
/// candidates are the crypto buys and sales of its account and coin (the same recorded asset where both rows recorded
/// one; the same alias on the fill's, the charge's and the booking's day), quoted in its pair's quote, not borrowed, at
/// most 72 hours before the charge and not after it; a buy smaller than the fee is none. Its edges are the candidates
/// whose raw price is its own, else (none being) those within 0.1 % of it, else (it has no raw price) every candidate.
/// A fee matches the fill of its one edge when that fill is on or before the fee's booking day and no other fee that
/// may claim it (booked on or after its day) has an edge to it. A fill after the booking day is still an edge: it makes
/// the fee's choice ambiguous, never a match. No order, no passes: what matches does not depend on which fee came first.
/// Each candidate read, and the edges' bytes before they are stored, are charged to the open budget.
pub fn cfee_matches(rows: &[Stored]) -> Result<std::collections::HashMap<i64, i64>, FiguresError> {
    use std::collections::HashMap;
    use std::mem::size_of;
    // What the matcher holds, in 4-byte limbs, charged (with its steps) before each list is made.
    let mut held = 0u64;
    let mut hold = |steps: u64, bytes: usize| -> Result<(), FiguresError> {
        held = held.saturating_add((bytes / 4) as u64);
        Ok(crate::figures::budget::charge(steps, held)?)
    };
    let is_fill = |r: &Stored| matches!(r.kind, Kind::Buy | Kind::Sell) && r.amount.is_positive() && !r.borrowed();
    let is_fee = |r: &Stored| r.cfee.charged.is_some() && r.cfee.fee && r.kind == Kind::Fee && r.amount.is_positive();
    let (n_fills, n_fees) = rows.iter().fold((0usize, 0usize), |(a, b), r| (a + usize::from(is_fill(r)), b + usize::from(is_fee(r))));
    hold(rows.len() as u64, n_fills * (size_of::<usize>() + size_of::<((i64, &str), Vec<usize>)>()))?;
    let mut fills: HashMap<(i64, &str), Vec<usize>> = HashMap::new();
    for (i, r) in rows.iter().enumerate() { if is_fill(r) { fills.entry((r.account, r.base.as_str())).or_default().push(i); } }
    for v in fills.values_mut() { v.sort_by_key(|&i| (rows[i].at.0, rows[i].id)); }
    hold(0, n_fees * size_of::<(usize, At, &'static str)>())?;
    let fees: Vec<(usize, At, &'static str)> = rows.iter().enumerate().filter_map(|(i, r)| {
        let charged = r.cfee.charged.filter(|_| is_fee(r))?;
        Some((i, charged, pair_quote(&r.base, r.cfee.symbol.as_deref()?)?))
    }).collect();
    let crypto = |x: &Stored| match super::prices::recorded_class(x.base_asset.as_ref()) {
        Some(security) => !security,
        None => x.cfee.symbol.as_deref().is_some_and(|s| s.contains('/') || pair_quote(&x.base, s).is_some()),
    };
    let tolerance = Dec::strict("0.001")?;
    // Each candidate of fee `i`, as (place, exact price, within 0.1 %), each read a step.
    let candidates = |i: usize, charged: At, quote: &str, out: &mut dyn FnMut(usize, bool, bool)| -> Result<(), FiguresError> {
        let f = &rows[i];
        let venue = super::prices::Venue { id: f.exchange_id, name_id: f.venue.clone(), stock: true };
        let alias = |day: NaiveDate| super::prices::alias_coin(&f.base, Some(&venue), Some(day));
        let coin = alias(charged.utc().date_naive());
        if alias(f.date()) != coin { return Ok(()); }
        // A price of zero or less names no fill: no edge at all. No price (absent, unreadable): every candidate.
        let price = f.cfee.price.as_ref();
        if price.is_some_and(|p| !p.is_positive()) { return Ok(()); }
        // 0.1 % either side of the fee's price, once per fee.
        let band = match price { Some(p) => { let t = (p * &tolerance)?; Some(((p - &t)?, (p + &t)?)) } None => None };
        let from = charged.0.saturating_sub(CFEE_REACH_SECONDS * 1_000_000_000);
        let pool = fills.get(&(f.account, f.base.as_str())).map_or(&[][..], Vec::as_slice);
        for &j in &pool[pool.partition_point(|&j| rows[j].at.0 < from)..] {
            let x = &rows[j];
            if x.at.0 > charged.0 { break; }
            crate::figures::budget::charge(1, 0)?;
            if x.kind == Kind::Buy && x.amount < f.amount { continue; }
            if !crypto(x) || x.quote.as_deref() != Some(quote) { continue; }
            if let (Some(a), Some(b)) = (&x.base_asset, &f.base_asset) { if a.0 != b.0 { continue; } }
            if alias(x.date()) != coin { continue; }
            let (exact, near) = match (price, &band, x.cfee.price.as_ref()) {
                (Some(p), Some((lo, hi)), Some(q)) => (p == q, lo <= q && q <= hi),
                _ => (false, false),
            };
            out(j, exact, near);
        }
        Ok(())
    };
    // Which candidates are a fee's edges: its exact ones, else those within 0.1 %, else (no price) all.
    hold(0, fees.len() * size_of::<(usize, u8)>())?;
    let mut kinds: Vec<(usize, u8)> = Vec::with_capacity(fees.len());
    let mut total = 0usize;
    for &(i, charged, quote) in &fees {
        let (mut all, mut exact, mut near) = (0usize, 0usize, 0usize);
        candidates(i, charged, quote, &mut |_, e, n| { all += 1; exact += usize::from(e); near += usize::from(n); })?;
        let priced = rows[i].cfee.price.is_some();
        let (count, kind) = if exact > 0 { (exact, 0) } else if priced { (near, 1) } else { (all, 2) };
        total += count;
        kinds.push((count, kind));
    }
    // The edges (a place each, in a list per fee), and per fill how many fees that may claim it have an edge to it.
    let claimed = total.min(n_fills);
    hold(0, fees.len() * size_of::<Vec<usize>>() + total * size_of::<usize>() + claimed * 2 * size_of::<(usize, usize)>())?;
    let mut edges: Vec<Vec<usize>> = Vec::with_capacity(fees.len());
    let mut claimants: HashMap<usize, usize> = HashMap::with_capacity(claimed);
    for (n, &(i, charged, quote)) in fees.iter().enumerate() {
        let (count, kind) = kinds[n];
        let mut mine = Vec::with_capacity(count);
        candidates(i, charged, quote, &mut |j, e, near| { if match kind { 0 => e, 1 => near, _ => true } { mine.push(j); } })?;
        crate::figures::budget::charge(mine.len() as u64, 0)?;
        for &j in &mine { if rows[j].date() <= rows[i].date() { *claimants.entry(j).or_default() += 1; } }
        edges.push(mine);
    }
    hold(fees.len() as u64, fees.len().min(claimed) * 2 * size_of::<(i64, i64)>())?;
    let mut out = HashMap::new();
    for (n, &(i, _, _)) in fees.iter().enumerate() {
        if let [j] = edges[n][..] { if rows[j].date() <= rows[i].date() && claimants.get(&j) == Some(&1) { out.insert(rows[i].id, rows[j].id); } }
    }
    Ok(out)
}

/// D-TAX-CFEE-MATCHED-FILL: each timed CFEE its fill (`cfee_matches`) and its instant: its fill's when it matched, else
/// its charge instant, only ever later than its stored instant and within its booking UTC day; an unmatched fee charged
/// on a later day stands at its booking day's last nanosecond. Nothing moves back or across days. Then by (instant, id).
pub fn place_cfees(rows: &mut [Stored]) -> Result<(), FiguresError> {
    if !rows.iter().any(|r| r.cfee.timed()) { return Ok(()); }
    let matches = cfee_matches(rows)?;
    let at: std::collections::HashMap<i64, At> = rows.iter().filter(|r| matches!(r.kind, Kind::Buy | Kind::Sell)).map(|r| (r.id, r.at)).collect();
    for r in rows.iter_mut() {
        let Some(charged) = r.cfee.charged.filter(|_| r.cfee.fee && r.kind == Kind::Fee) else { continue };
        r.cfee.fill = matches.get(&r.id).copied();
        let day = r.date();
        let to = r.cfee.fill.and_then(|f| at.get(&f).copied()).unwrap_or(charged);
        let to_day = to.utc().date_naive();
        if to_day == day && to.0 > r.at.0 { r.at = to; }
        else if to_day > day && r.cfee.fill.is_none() {
            let next = day.succ_opt().and_then(|d| At::from_utc(d.and_hms_opt(0, 0, 0)?.and_utc()));
            if let Some(next) = next { r.at = At(next.0 - 1); }
        }
    }
    rows.sort_by_key(|r| (r.at.0, r.id));
    Ok(())
}

/// Each CFEE of `rows` its `fill_price`.
pub fn price_cfees(rows: &mut [Stored]) -> Result<(), FiguresError> {
    for (i, fills) in cfee_places(rows) {
        let price = fill_price(&fills.iter().map(|&j| &rows[j]).collect::<Vec<_>>())?;
        rows[i].cfee.fill_price = price;
    }
    Ok(())
}

/// The fill price of the user's CFEE row `id` (the tracker row's display): `price_cfees` over that fee and the rows of
/// its coin around its day. None for any other row, a timed fee's included.
pub fn cfee_price(c: &Connection, user_id: i64, id: i64) -> Result<Option<(Dec, String)>, FiguresError> {
    let Some(fee) = read(c, "t.user_id = ?1 AND t.id = ?2", rusqlite::params![user_id, id])?.pop().filter(|r| r.cfee.fee && r.kind == Kind::Fee && !r.cfee.timed()) else { return Ok(None) };
    let Some(day) = fee.date().pred_opt() else { return Ok(None) };
    // A day's margin either side: the stored instant's text is compared, `charged_on` decides.
    let (from, to) = (day.pred_opt().unwrap_or(day), fee.date().succ_opt().unwrap_or(fee.date()));
    let rows = read(c, "t.user_id = ?1 AND t.base_currency = ?2 AND t.entry_type IN (0, 1) AND t.transacted_at >= ?3 AND t.transacted_at < ?4",
                    rusqlite::params![user_id, fee.base, from.to_string(), to.to_string()])?;
    fill_price(&cfee_fills(&fee, &rows))
}

impl Stored {
    /// The day of its instant, in UTC (every job runs under Time.zone = UTC).
    pub fn date(&self) -> NaiveDate { self.at.utc().date_naive() }
    /// AccountTransaction#quoted?.
    pub fn quoted(&self) -> bool { self.quote_amount.is_some() && self.quote.as_deref().is_some_and(cash) }
    pub fn borrowed(&self) -> bool { borrowed(self.tx_id.as_deref()) }
}

/// What sorting `n` items costs, in comparisons: n log n.
pub fn sort_steps(n: usize) -> u64 { let n = n as u64; n.saturating_mul(u64::from(64 - n.leading_zeros())) }

/// Tracker::UnfundedCash.borrowed?: a futures or margin row, recognised by the id the importers give it.
pub fn borrowed(tx_id: Option<&str>) -> bool { ["futures", "margin-interest", "liquidation-"].iter().any(|m| tx_id.unwrap_or("").contains(m)) }

/// `Exchange#name_id`: the class name, demodulized and underscored.
pub fn name_id(exchange_type: &str) -> String {
    let class = exchange_type.rsplit("::").next().unwrap_or(exchange_type);
    let mut out = String::new();
    let chars: Vec<char> = class.chars().collect();
    for (i, ch) in chars.iter().enumerate() {
        if ch.is_ascii_uppercase() {
            let after_lower = i > 0 && (chars[i - 1].is_ascii_lowercase() || chars[i - 1].is_ascii_digit());
            let acronym_end = i > 0 && chars[i - 1].is_ascii_uppercase() && chars.get(i + 1).is_some_and(|c| c.is_ascii_lowercase());
            if after_lower || acronym_end { out.push('_'); }
            out.push(ch.to_ascii_lowercase());
        } else { out.push(*ch); }
    }
    out
}

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
        Some(t) => serde_json::from_str(&t).map_err(|_| data("a JSON column 2.x did not write")),
    }
}

/// Every row of the user, borrowed wallets' included, by instant then id (`by_date_asc`). Each row a step of the open
/// budget scope, and the sort n log n. A duplicate account's rows repeat its survivor's and are left out (#710,
/// PLAN-710-6 §5.2); a duplicate is numbered 2+, so only a membership row names one.
pub fn load_all(c: &Connection, user_id: i64) -> Result<Vec<Stored>, FiguresError> { Ok(load_all_mapped(c, user_id)?.0) }

/// `load_all`, and the owner's map its labels come from (built once: a relabelled row no longer carries its spelling).
pub fn load_all_mapped(c: &Connection, user_id: i64) -> Result<(Vec<Stored>, super::spellings::Spellings), FiguresError> {
    let counted = crate::identity::counted(c, user_id, &crate::venue_ids::Scope::install(c)?.member_sql("t"))?;
    let mut rows = read(c, &format!("t.user_id = ?1 AND {counted}"), rusqlite::params![user_id])?;
    // A row linked to a duplicate's row (left out above) reads unlinked: its partner is not in the walk.
    if counted != "1" {
        let ids: std::collections::HashSet<i64> = rows.iter().map(|r| r.id).collect();
        for r in rows.iter_mut().filter(|r| r.linked_to.is_some_and(|l| !ids.contains(&l))) { r.linked_to = None; r.to_venue = None; r.to_exchange_id = None; r.to_account = None; }
    }
    let map = super::labels::load(c, user_id, &rows)?;
    super::labels::relabel(&mut rows, &map);
    crate::figures::budget::charge(sort_steps(rows.len()), 0)?;
    rows.sort_by_key(|r| (r.at.0, r.id));
    place_cfees(&mut rows)?;
    price_cfees(&mut rows)?;
    Ok((rows, map))
}

/// #710: a group is its account's. A group id of an account 2+'s row reads as `<group>@<account>`, so every reader that
/// keys groups by venue (the walk, the tax reports) keeps two accounts' legs of one group id apart.
pub fn account_group(group: Option<String>, member: Option<i64>) -> Option<String> {
    group.filter(|g| !g.trim().is_empty()).map(|g| match member { Some(m) => format!("{g}@{m}"), None => g })
}

/// The rows `filter` (over `t`) selects, by id, each a step of the open budget scope.
fn read(c: &Connection, filter: &str, params: &[&dyn rusqlite::ToSql]) -> Result<Vec<Stored>, FiguresError> {
    // #710: each row's venue id and account, wherever its account keeps them.
    let any = crate::venue_ids::Scope::install(c)?;
    let mut s = c.prepare(&format!(
        "SELECT t.id, t.exchange_id, e.type, t.entry_type, t.base_currency, t.base_amount, t.quote_currency, t.quote_amount, \
                t.fee_currency, t.fee_amount, {}, t.group_id, t.transacted_at, t.raw_data, t.manual_values, t.linked_transaction_id, \
                le.type, l.exchange_id, ba.id, ba.external_id, ba.category, t.transaction_id, {}, COALESCE({}, -t.exchange_id), \
                CASE WHEN l.id IS NULL THEN NULL ELSE COALESCE({}, -l.exchange_id) END \
         FROM account_transactions t JOIN exchanges e ON e.id = t.exchange_id LEFT JOIN assets ba ON ba.id = t.base_asset_id \
         LEFT JOIN account_transactions l ON l.id = t.linked_transaction_id LEFT JOIN exchanges le ON le.id = l.exchange_id \
         WHERE {filter} ORDER BY t.id", any.row_id("t"), any.member_sql("t"), any.row_account("t"), any.row_account("l")))?;
    let mut q = s.query(params)?;
    let mut rows = vec![];
    while let Some(r) = q.next()? {
        crate::figures::budget::charge(1, 0)?;
        let kind = Kind::from_stored(r.get(3)?).ok_or_else(|| data("an entry type 2.x does not have"))?;
        let at_text: String = r.get(12)?;
        let raw = json_column(r.get(13)?)?;
        let manual = json_column(r.get(14)?)?;
        let venue_type: Option<String> = r.get(2)?;
        let to_type: Option<String> = r.get(16)?;
        let base: String = r.get(4)?;
        rows.push(Stored {
            id: r.get(0)?, exchange_id: r.get(1)?, venue: name_id(venue_type.as_deref().unwrap_or("")), kind, base: base.clone().into(),
            amount: Dec::from_sql(r.get_ref(5)?)?.ok_or_else(|| data("a row with no base amount"))?,
            quote: r.get(6)?, quote_amount: Dec::from_sql(r.get_ref(7)?)?,
            fee_currency: r.get(8)?, fee_amount: Dec::from_sql(r.get_ref(9)?)?,
            tx_id: r.get(10)?, group: account_group(r.get(11)?, r.get(22)?),
            at: At::from_sql(&at_text).map_err(|e| data(format!("{e:?}")))?,
            per_share: json_number(raw.get("per_share_amount"))?,
            stated: json_number(manual.get("price"))?,
            linked_to: r.get(15)?, to_venue: to_type.as_deref().map(name_id), to_exchange_id: r.get(17)?,
            base_asset: r.get::<_, Option<i64>>(18)?.map(|id| -> rusqlite::Result<_> { Ok((id, r.get(19)?, r.get(20)?)) }).transpose()?,
            cfee: Cfee {
                fee: raw.get("activity_type").and_then(Value::as_str) == Some("CFEE"),
                charged: (kind == Kind::Fee).then(|| charged(&raw)).flatten(),
                raw_price: (kind == Kind::Fee && charged(&raw).is_some()).then(|| raw_price(&base, &raw)).flatten(),
                price: raw_number(&raw), symbol: raw.get("symbol").and_then(Value::as_str).map(str::to_string),
                ..Cfee::default()
            },
            bot_order: r.get(21)?,
            account: r.get(23)?, to_account: r.get(24)?,
        });
    }
    Ok(rows)
}

/// The rows Tracker::Ledger walks: `load_all` less the borrowed wallets', in Tax::PriceService.ordered's order.
pub fn walk_rows(all: &[Stored]) -> Result<Vec<Stored>, FiguresError> {
    let rows: Vec<Stored> = all.iter().filter(|r| !r.borrowed()).cloned().collect();
    crate::figures::budget::charge(sort_steps(rows.len()), 0)?;
    ordered_legs(rows, true)
}

/// Both sets from one read.
pub fn load(c: &Connection, user_id: i64) -> Result<Vec<Stored>, FiguresError> { walk_rows(&load_all(c, user_id)?) }

/// `ordered`, then each coin-quoted single row read as its two legs (`split_coin_quoted_trades`): the order every reader
/// of the ledger walks, the tracker's (`stable_cash`: a stablecoin is cash) and the tax report's.
pub fn ordered_legs(rows: Vec<Stored>, stable_cash: bool) -> Result<Vec<Stored>, FiguresError> { split_coin_quoted_trades(ordered(rows), stable_cash) }

/// Whether `r` is a trade booked as one buy or sell row quoted in a coin (not fiat; not a stablecoin where `stable_cash`):
/// the rows `split_coin_quoted_trades` reads as two legs. The base may be anything, cash included.
pub fn coin_quoted(r: &Stored, stable_cash: bool) -> bool {
    matches!(r.kind, Kind::Buy | Kind::Sell) && r.group.is_none() && r.quote_amount.is_some()
        && r.quote.as_deref().is_some_and(|q| !(q.trim().is_empty() || fiat(q) || stable_cash && stable(q)))
}

/// D-TRACKER-COIN-QUOTE (and the tax report's D-TAX-COIN-QUOTE): a trade of a coin for a coin booked as one buy or sell row
/// quoted in the other coin (Bybit, MEXC, Bitget, Gemini, BingX, Bitrue, Bitvavo, Hyperliquid, KuCoin; Binance and Coinbase
/// store two swap legs) becomes the two legs it is: the row's coin coming in or going out, and the quote coin the other way,
/// grouped. 2.x read the row alone, so the quote coin never moved: the coin paid kept its holding and its lots, the coin
/// received had none. The fee goes with the leg of its own coin, else with the coin coming in (as Binance books it). The new
/// leg's id is the row's negated (a stored id is positive, so it is unique; never stored, never shown), its group
/// `coin-quote:<id>`, with `~` before it until no stored row of the venue carries that group. The out-leg comes first, at
/// the row's place. A cash base (a stablecoin or fiat sold or bought for a coin) is split too: its cash leg reads as the
/// cash trade it is.
pub fn split_coin_quoted_trades(rows: Vec<Stored>, stable_cash: bool) -> Result<Vec<Stored>, FiguresError> {
    if !rows.iter().any(|r| coin_quoted(r, stable_cash)) { return Ok(rows); }
    let taken: std::collections::HashSet<(i64, String)> = rows.iter().filter_map(|r| r.group.clone().map(|g| (r.exchange_id, g))).collect();
    let mut out = Vec::with_capacity(rows.len() + 8);
    for r in rows {
        crate::figures::budget::charge(1, 0)?;
        if !coin_quoted(&r, stable_cash) { out.push(r); continue; }
        if r.id <= 0 { return Err(data("a split row's id is not positive")); }
        let (Some(quote), Some(paid)) = (r.quote.clone(), r.quote_amount.clone()) else { out.push(r); continue };
        let buying = r.kind == Kind::Buy;
        let mut name = format!("coin-quote:{}", r.id);
        while taken.contains(&(r.exchange_id, name.clone())) { name.insert(0, '~'); }
        let group = Some(name);
        let fee_on_quote = r.fee_currency.as_deref() == Some(quote.as_str()) || (!buying && r.fee_currency.as_deref() != Some(r.base.as_str()));
        let (fee_currency, fee_amount) = (r.fee_currency.clone(), r.fee_amount.clone());
        let other = Stored {
            id: -r.id, kind: if buying { Kind::SwapOut } else { Kind::SwapIn }, base: quote, amount: paid, quote: None, quote_amount: None,
            fee_currency: if fee_on_quote { fee_currency.clone() } else { None }, fee_amount: if fee_on_quote { fee_amount.clone() } else { None },
            tx_id: r.tx_id.as_ref().map(|t| format!("{t}-quote")), group: group.clone(), stated: None, linked_to: None, to_venue: None,
            to_exchange_id: None, base_asset: None, per_share: None, cfee: Cfee::default(), ..r.clone()
        };
        let own = Stored { kind: if buying { Kind::SwapIn } else { Kind::SwapOut }, quote: None, quote_amount: None, group,
                           fee_currency: if fee_on_quote { None } else { fee_currency }, fee_amount: if fee_on_quote { None } else { fee_amount }, ..r };
        // The out-leg first, as `ordered` lays out a group.
        if buying { out.push(other); out.push(own); } else { out.push(own); out.push(other); }
    }
    Ok(out)
}

/// Tax::PriceService.ordered: `[transacted_at, anchor, out-leg first, id]`, the anchor being the smallest id of the
/// row's group (by exchange and group id), or its own id without a group. D-TAX-CFEE-MATCHED-FILL: a timed CFEE at its
/// fill's instant takes its fill's place, right after it; any other timed CFEE is the last of its instant.
pub fn ordered(mut rows: Vec<Stored>) -> Vec<Stored> {
    let mut anchors: std::collections::HashMap<(i64, Option<String>), i64> = std::collections::HashMap::new();
    for r in &rows { anchors.entry((r.exchange_id, r.group.clone())).and_modify(|a| *a = (*a).min(r.id)).or_insert(r.id); }
    let own = |r: &Stored| {
        let anchor = if r.group.is_some() { anchors.get(&(r.exchange_id, r.group.clone())).copied().unwrap_or(r.id) } else { r.id };
        (r.at.0, anchor, if matches!(r.kind, Kind::Sell | Kind::SwapOut) { 0 } else { 1 }, r.id, 0)
    };
    let fills: std::collections::HashMap<i64, (i64, i64, u8, i64, u8)> =
        rows.iter().filter(|r| matches!(r.kind, Kind::Buy | Kind::Sell)).map(|r| (r.id, own(r))).collect();
    let key = |r: &Stored| {
        if !r.cfee.timed() { return own(r); }
        match r.cfee.fill.and_then(|f| fills.get(&f)).filter(|k| k.0 == r.at.0) {
            Some(&(at, anchor, out, id, _)) => (at, anchor, out, id, 1),
            None => (r.at.0, i64::MAX, 2, r.id, 0),
        }
    };
    rows.sort_by_cached_key(key);
    rows
}

/// What the backfill this build carries sweeps (Plan D4a): one Alpaca account in dollars. Checked over every row, the
/// borrowed wallets' included, and every balance, before any row is filtered or anything written; `Err` names what
/// falls outside it.
pub fn alpaca_sweep_domain(c: &Connection, user_id: i64, all: &[Stored]) -> Result<(), String> {
    let elsewhere: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM exchange_account_balances b JOIN exchanges e ON e.id = b.exchange_id \
                                       WHERE b.user_id = ?1 AND e.type IS NOT ?2)", rusqlite::params![user_id, super::VENUE_TYPE], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if elsewhere { return Err("balances on a venue other than Alpaca".into()); }
    for r in all {
        if r.venue != super::VENUE { return Err("rows of a venue other than Alpaca".into()); }
        if r.borrowed() { return Err("futures or margin rows".into()); }
        if matches!(r.kind, Kind::SwapIn | Kind::SwapOut) { return Err("swap legs".into()); }
        if r.fee_currency.is_some() || r.fee_amount.is_some() { return Err("a fee carried on a row".into()); }
        if [Some(r.base.as_str()), r.quote.as_deref()].into_iter().flatten().any(|c| fiat(c) && c != "USD") { return Err("cash other than US dollars".into()); }
        if matches!(r.kind, Kind::Buy | Kind::Sell) && !r.quoted() { return Err("a trade leg with no cash quote of its own".into()); }
    }
    let linked_to: std::collections::HashSet<i64> = all.iter().filter_map(|r| r.linked_to).collect();
    if all.iter().any(|r| (r.linked_to.is_some() || linked_to.contains(&r.id)) && r.base != "USD") { return Err("a linked transfer of anything but US dollars".into()); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(id: i64, kind: Kind, group: Option<&str>, at: &str) -> Stored {
        Stored { id, exchange_id: 1, venue: "alpaca".into(), kind, base: "AAPL".into(), amount: Dec::one(), quote: None, quote_amount: None,
                 fee_currency: None, fee_amount: None, tx_id: None, group: group.map(String::from), at: At::from_sql(at).unwrap(), per_share: None,
                 stated: None, linked_to: None, to_venue: None, to_exchange_id: None, base_asset: None, cfee: Default::default(), bot_order: None, account: 1, to_account: None }
    }

    /// Tax::PriceService.ordered: by instant; a group where its first-stored leg was, its out-legs first; else by id.
    #[test]
    fn rows_walk_in_v2_order() {
        let t = "2026-09-02 14:30:00";
        let rows = vec![stored(6, Kind::Buy, Some("g"), t), stored(2, Kind::Deposit, None, "2026-09-03 00:00:00"), stored(7, Kind::Sell, Some("g"), t),
                        stored(5, Kind::Buy, None, t), stored(1, Kind::Fee, None, t)];
        assert_eq!(ordered(rows).iter().map(|r| r.id).collect::<Vec<_>>(), [1, 5, 7, 6, 2]);
    }

    fn trade(id: i64, kind: Kind, base: &str, quote: &str, paid: &str, fee: Option<(&str, &str)>) -> Stored {
        Stored { venue: "bybit".into(), exchange_id: 2, base: base.into(), quote: Some(quote.into()), quote_amount: Some(Dec::strict(paid).unwrap()),
                 fee_currency: fee.map(|f| f.0.into()), fee_amount: fee.map(|f| Dec::strict(f.1).unwrap()), tx_id: Some(format!("t{id}")),
                 ..stored(id, kind, None, "2026-09-02 10:00:00") }
    }
    type Leg = (i64, Kind, String, String, Option<String>, Option<String>);
    fn legs(rows: &[Stored]) -> Vec<Leg> {
        rows.iter().map(|r| (r.id, r.kind, r.base.to_string(), r.amount.to_s_f(), r.fee_currency.as_deref().map(str::to_owned), r.group.clone())).collect()
    }

    /// D-TRACKER-COIN-QUOTE: a coin-quoted row is its two legs, out first, at its place; the fee with its own coin's leg, else
    /// the coin coming in; a fiat quote, a stablecoin quote where it is cash, a grouped or a blank-quoted row stays.
    #[test]
    fn a_coin_quoted_row_is_its_two_legs() {
        let g = |id: i64| Some(format!("coin-quote:{id}"));
        let s = |x: &str| x.to_string();
        // A sale of BTC for ETH, the fee in ETH (the coin coming in): BTC out, then ETH in with the fee.
        let out = split_coin_quoted_trades(vec![trade(7, Kind::Sell, "BTC", "ETH", "10", Some(("ETH", "0.01")))], true).unwrap();
        assert_eq!(legs(&out), [(7, Kind::SwapOut, s("BTC"), s("1.0"), None, g(7)), (-7, Kind::SwapIn, s("ETH"), s("10.0"), Some(s("ETH")), g(7))]);
        assert_eq!((out[1].tx_id.as_deref(), out[1].quote.as_deref(), out[1].quote_amount.as_ref()), (Some("t7-quote"), None, None));
        // A sale, the fee in the coin sold: with the out-leg. A third coin: with the coin coming in.
        let out = split_coin_quoted_trades(vec![trade(7, Kind::Sell, "BTC", "ETH", "10", Some(("BTC", "0.01")))], true).unwrap();
        assert_eq!((out[0].fee_currency.as_deref(), out[1].fee_currency.as_deref()), (Some("BTC"), None));
        let out = split_coin_quoted_trades(vec![trade(7, Kind::Sell, "BTC", "ETH", "10", Some(("KCS", "1")))], true).unwrap();
        assert_eq!((out[0].fee_currency.as_deref(), out[1].fee_currency.as_deref()), (None, Some("KCS")));
        // A buy of ETH for BTC: BTC out first; the fee in BTC with it, in ETH or a third coin with the ETH coming in.
        let out = split_coin_quoted_trades(vec![trade(8, Kind::Buy, "ETH", "BTC", "0.5", Some(("BTC", "0.001")))], true).unwrap();
        assert_eq!(legs(&out), [(-8, Kind::SwapOut, s("BTC"), s("0.5"), Some(s("BTC")), g(8)), (8, Kind::SwapIn, s("ETH"), s("1.0"), None, g(8))]);
        for fee in ["ETH", "KCS"] {
            let out = split_coin_quoted_trades(vec![trade(8, Kind::Buy, "ETH", "BTC", "0.5", Some((fee, "1")))], true).unwrap();
            assert_eq!((out[0].fee_currency.as_deref(), out[1].fee_currency.as_deref()), (None, Some(fee)), "{fee}");
        }
        // A cash base sold for a coin is split too (its cash leg reads as the cash trade it is).
        let out = split_coin_quoted_trades(vec![trade(9, Kind::Sell, "USDT", "BTC", "1", None)], true).unwrap();
        assert_eq!(legs(&out), [(9, Kind::SwapOut, s("USDT"), s("1.0"), None, g(9)), (-9, Kind::SwapIn, s("BTC"), s("1.0"), None, g(9))]);
        // Not split: a fiat quote; a stablecoin quote where a stablecoin is cash (split where it is a coin); a group; a blank quote.
        let stays = |r: Stored, stable_cash: bool| split_coin_quoted_trades(vec![r], stable_cash).unwrap().len() == 1;
        assert!(stays(trade(1, Kind::Buy, "BTC", "USD", "1", None), true) && stays(trade(1, Kind::Buy, "BTC", "EUR", "1", None), false));
        assert!(stays(trade(1, Kind::Sell, "BTC", "USDT", "1", None), true) && !stays(trade(1, Kind::Sell, "BTC", "USDT", "1", None), false));
        assert!(stays(Stored { group: Some("g".into()), ..trade(1, Kind::Buy, "ETH", "BTC", "1", None) }, true));
        assert!(stays(trade(1, Kind::Buy, "ETH", " ", "1", None), true));
        assert!(stays(Stored { kind: Kind::Deposit, ..trade(1, Kind::Buy, "ETH", "BTC", "1", None) }, true));
        // Every other row keeps its place around the legs.
        let out = split_coin_quoted_trades(vec![stored(1, Kind::Deposit, None, "2026-09-01 00:00:00"), trade(2, Kind::Sell, "BTC", "ETH", "10", None),
                                                stored(3, Kind::Deposit, None, "2026-09-03 00:00:00")], true).unwrap();
        assert_eq!(out.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2, -2, 3]);
    }

    /// The new leg's identity can collide with nothing stored: a stored id must be positive (its negation is then unique, and
    /// the largest negates), and a stored group of the venue named like the new one pushes it aside.
    #[test]
    fn a_new_leg_collides_with_nothing_stored() {
        for id in [0, -3] {
            assert!(matches!(split_coin_quoted_trades(vec![trade(id, Kind::Sell, "BTC", "ETH", "1", None)], true), Err(FiguresError::Data(_))), "{id}");
        }
        let out = split_coin_quoted_trades(vec![trade(i64::MAX, Kind::Sell, "BTC", "ETH", "1", None)], true).unwrap();
        assert_eq!(out[1].id, -i64::MAX);
        let imported = |group: &str, exchange_id: i64| Stored { group: Some(group.into()), exchange_id, ..stored(1, Kind::Airdrop, None, "2026-09-01 00:00:00") };
        let out = split_coin_quoted_trades(vec![imported("coin-quote:7", 2), imported("~coin-quote:7", 2), trade(7, Kind::Sell, "BTC", "ETH", "1", None)], true).unwrap();
        assert_eq!(out[2].group.as_deref(), Some("~~coin-quote:7"));
        let out = split_coin_quoted_trades(vec![imported("coin-quote:7", 3), trade(7, Kind::Sell, "BTC", "ETH", "1", None)], true).unwrap();
        assert_eq!(out[1].group.as_deref(), Some("coin-quote:7"), "another venue's group is another group");
    }

    #[test]
    fn a_venue_is_named_as_v2_names_it() {
        assert_eq!(name_id("Exchanges::BinanceUs"), "binance_us");
        assert_eq!(name_id("Exchanges::Alpaca"), "alpaca");
        assert_eq!(name_id("Exchanges::Ibkr"), "ibkr");
    }

    fn db(rows: &[&str]) -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE exchanges (id INTEGER PRIMARY KEY, type TEXT); INSERT INTO exchanges VALUES (1, 'Exchanges::Alpaca'), (2, 'Exchanges::Binance');
                         CREATE TABLE exchange_account_balances (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, free NUMERIC, locked NUMERIC);
                         CREATE TABLE account_transactions (id INTEGER PRIMARY KEY, user_id INTEGER, exchange_id INTEGER, entry_type INTEGER, base_currency TEXT,
                         base_amount NUMERIC, quote_currency TEXT, quote_amount NUMERIC, fee_currency TEXT, fee_amount NUMERIC, tx_id TEXT, group_id TEXT,
                         transacted_at TEXT, raw_data TEXT, manual_values TEXT, linked_transaction_id INTEGER, base_asset_id INTEGER, transaction_id INTEGER);
                         CREATE TABLE assets (id INTEGER PRIMARY KEY, external_id TEXT, category TEXT);").unwrap();
        for values in rows {
            c.execute(&format!("INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, fee_currency, \
                                fee_amount, linked_transaction_id, tx_id, transacted_at, raw_data, manual_values) VALUES (1, {values}, '2026-09-01 00:00:00', '{{}}', '{{}}')"), []).unwrap();
        }
        c
    }

    /// The walk takes every venue's rows; the borrowed wallets stay in `load_all` and out of `walk_rows`.
    #[test]
    fn the_walk_sets_the_borrowed_wallets_aside_and_the_backfill_does_not() {
        let c = db(&["2, 0, 'BTC', 1, 'USDT', 60000, 'BNB', 0.01, NULL, NULL", "2, 11, 'USDT', 5, NULL, NULL, NULL, NULL, NULL, 'usdt-futures-1'",
                     "1, 4, 'USD', 100, NULL, NULL, NULL, NULL, NULL, NULL"]);
        let all = load_all(&c, 1).unwrap();
        assert_eq!(all.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(walk_rows(&all).unwrap().iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!((all[0].venue.as_str(), all[0].fee_currency.as_deref()), ("binance", Some("BNB")));
        assert_eq!(all[0].bot_order, None);
        c.execute("UPDATE account_transactions SET transaction_id = 7 WHERE id = 1", []).unwrap();
        assert_eq!(load_all(&c, 1).unwrap()[0].bot_order, Some(7));
    }

    /// The backfill's domain is D4a's Alpaca account in dollars; anything else is named.
    #[test]
    fn the_backfill_domain_names_what_falls_outside_it() {
        let ok = "1, 0, 'AAPL', 1, 'USD', 100, NULL, NULL, NULL, NULL";
        for (row, why) in [("2, 4, 'USD', 1, NULL, NULL, NULL, NULL, NULL, NULL", "rows of a venue other than Alpaca"),
                           ("1, 11, 'USD', 1, NULL, NULL, NULL, NULL, NULL, 'usdt-futures-1'", "futures or margin rows"),
                           ("1, 2, 'BTC', 1, NULL, NULL, NULL, NULL, NULL, NULL", "swap legs"),
                           ("1, 0, 'AAPL', 1, 'USD', 100, 'USD', 1, NULL, NULL", "a fee carried on a row"),
                           ("1, 4, 'EUR', 100, NULL, NULL, NULL, NULL, NULL, NULL", "cash other than US dollars"),
                           ("1, 5, 'BTC', 1, NULL, NULL, NULL, NULL, 1, NULL", "a linked transfer of anything but US dollars"),
                           ("1, 0, 'AAPL', 1, NULL, 100, NULL, NULL, NULL, NULL", "a trade leg with no cash quote of its own")] {
            let c = db(&[ok, row]);
            assert_eq!(alpaca_sweep_domain(&c, 1, &load_all(&c, 1).unwrap()), Err(why.to_string()), "{why}");
        }
        let c = db(&[ok]);
        assert_eq!(alpaca_sweep_domain(&c, 1, &load_all(&c, 1).unwrap()), Ok(()));
        c.execute("INSERT INTO exchange_account_balances (user_id, exchange_id, free, locked) VALUES (1, 2, 0.5, 0)", []).unwrap();
        assert_eq!(alpaca_sweep_domain(&c, 1, &load_all(&c, 1).unwrap()), Err("balances on a venue other than Alpaca".to_string()));
    }

    /// A long history loads in linear work, charged to the budget.
    #[test]
    fn a_long_history_loads_within_its_budget() {
        let c = db(&["1, 4, 'USD', 100, NULL, NULL, NULL, NULL, NULL, NULL"]);
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

    // ---- D-TAX-CFEE-MATCHED-FILL ----

    const NONE: [(i64, i64); 0] = [];

    fn dec(s: &str) -> Dec { Dec::strict(s).unwrap() }
    /// A BTC/USD fill of account 1, its raw price as Alpaca sends it.
    fn fill(id: i64, kind: Kind, amount: &str, price: &str, at: &str) -> Stored {
        Stored { base: "BTC".into(), amount: dec(amount), quote: Some("USD".into()), quote_amount: Some((&dec(amount) * &dec(price)).unwrap()),
                 cfee: Cfee { price: Some(dec(price)), symbol: Some("BTC/USD".into()), ..Cfee::default() }, ..stored(id, kind, None, at) }
    }
    /// A CFEE of `amount` BTC booked on `booked` (00:00 UTC), charged at `created`, as `read` lifts it.
    fn timed(id: i64, amount: &str, price: Option<&str>, booked: &str, created: &str) -> Stored { cfee(id, "BTC", "BTCUSD", amount, price, booked, created) }
    fn cfee(id: i64, base: &str, symbol: &str, amount: &str, price: Option<&str>, booked: &str, created: &str) -> Stored {
        let raw = serde_json::json!({ "activity_type": "CFEE", "created_at": created, "symbol": symbol, "price": price });
        Stored { base: base.into(), amount: dec(amount),
                 cfee: Cfee { fee: true, charged: charged(&raw), price: raw_number(&raw), symbol: Some(symbol.into()), raw_price: raw_price(base, &raw), ..Cfee::default() },
                 ..stored(id, Kind::Fee, None, &format!("{booked} 00:00:00")) }
    }
    fn matched(rows: &[Stored]) -> Vec<(i64, i64)> { let mut m: Vec<(i64, i64)> = cfee_matches(rows).unwrap().into_iter().collect(); m.sort(); m }
    fn placed(mut rows: Vec<Stored>) -> Vec<Stored> { place_cfees(&mut rows).unwrap(); ordered(rows) }
    fn at(rows: &[Stored], id: i64) -> String { rows.iter().find(|r| r.id == id).unwrap().at.utc().format("%Y-%m-%d %H:%M:%S%.9f").to_string() }
    fn ids(rows: &[Stored]) -> Vec<i64> { rows.iter().map(|r| r.id).collect() }

    /// The report's shape: Alpaca books the fee at 00:00 of the day; it was charged five seconds after its buy. Matched, it
    /// stands at the buy's instant right after it, the lower id notwithstanding.
    #[test]
    fn a_timed_cfee_stands_right_after_its_fill() {
        let rows = vec![timed(1, "0.00000029", Some("84674.02"), "2026-10-01", "2026-10-01T23:08:55Z"), fill(2, Kind::Buy, "0.0001", "84674.02", "2026-10-01 23:08:50")];
        assert_eq!(matched(&rows), [(1, 2)]);
        let rows = placed(rows);
        assert_eq!((ids(&rows), at(&rows, 1)), (vec![2, 1], "2026-10-01 23:08:50.000000000".to_string()));
        assert_eq!(rows[1].cfee.fill, Some(2));
    }

    /// Booked Monday for a Saturday fill: matched, and it stays at Monday 00:00 (nothing moves back or across days).
    #[test]
    fn a_weekend_fee_matches_its_saturday_fill_and_keeps_its_monday() {
        let rows = placed(vec![fill(1, Kind::Buy, "0.01", "60000", "2026-10-03 15:00:00"), timed(2, "0.00001", Some("60000"), "2026-10-05", "2026-10-03T15:00:04Z")]);
        assert_eq!((rows[1].cfee.fill, at(&rows, 2)), (Some(1), "2026-10-05 00:00:00.000000000".to_string()));
    }

    /// Exact prices name their fills (a fee with an exact candidate reads no other); a fill two fees find is neither's;
    /// two partial fills of one order, each with its exact fee, are one to one.
    #[test]
    fn exact_prices_name_their_fills() {
        let day = "2026-10-01";
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), fill(2, Kind::Buy, "0.01", "101.00", "2026-10-01 10:30:00"),
                    timed(3, "0.0001", Some("101.00"), day, "2026-10-01T11:00:00Z"), timed(4, "0.0001", Some("100.00"), day, "2026-10-01T11:00:01Z")];
        assert_eq!(matched(&rows), [(3, 2), (4, 1)]);
        // Fee 5 is within 0.1 % of buy 1, fee 6 exactly its price: the buy is found by two fees, so it is neither's.
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), timed(5, "0.0001", Some("100.05"), day, "2026-10-01T10:01:00Z"),
                    timed(6, "0.0001", Some("100.00"), day, "2026-10-01T10:02:00Z")];
        assert_eq!(matched(&rows), NONE);
        let rows = [fill(1, Kind::Buy, "0.5", "100.00", "2026-10-01 10:00:00.100"), fill(2, Kind::Buy, "0.5", "100.02", "2026-10-01 10:00:00.200"),
                    timed(3, "0.0001", Some("100.00"), day, "2026-10-01T10:00:01Z"), timed(4, "0.0001", Some("100.02"), day, "2026-10-01T10:00:01Z")];
        assert_eq!(matched(&rows), [(3, 1), (4, 2)]);
        // A rounding match where it is the only fill within 0.1 %.
        let rows = [fill(1, Kind::Buy, "0.01", "84673.9", "2026-10-01 10:00:00"), timed(2, "0.0001", Some("84674.02"), day, "2026-10-01T10:00:05Z")];
        assert_eq!(matched(&rows), [(2, 1)]);
    }

    /// What is not unique stays standalone: two fees finding one fill, a fill a buy and a sale share a price with, the
    /// same price spelled two ways.
    #[test]
    fn a_fee_that_is_not_unique_stays_standalone() {
        let day = "2026-10-01";
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), timed(2, "0.0001", Some("100.00"), day, "2026-10-01T10:01:00Z"),
                    timed(3, "0.0001", Some("100.00"), day, "2026-10-01T10:01:00Z"), timed(4, "0.0001", None, day, "2026-10-01T10:02:00Z")];
        assert_eq!(matched(&rows), NONE);
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), fill(2, Kind::Sell, "0.01", "100.04", "2026-10-01 11:00:00"),
                    timed(3, "0.0001", Some("100.00"), day, "2026-10-01T11:01:00Z"), timed(4, "0.0002", Some("100.00"), day, "2026-10-01T11:02:00Z")];
        assert_eq!(matched(&rows), NONE, "neither takes the sale");
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), fill(2, Kind::Sell, "0.005", "100.00", "2026-10-01 11:00:00"),
                    timed(3, "0.0001", Some("100.00"), day, "2026-10-01T11:01:00Z")];
        assert_eq!(matched(&rows), NONE);
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-09-29 10:00:00"), fill(2, Kind::Sell, "0.005", "100.0", "2026-09-30 10:00:00"),
                    timed(3, "0.0001", Some("100.00"), day, "2026-10-01T09:00:00Z")];
        assert_eq!(matched(&rows), NONE, "100.00 and 100.0 are one price");
        // A same-price fill after the booking day vetoes the earlier one, and is not claimed itself.
        let rows = [fill(1, Kind::Buy, "0.01", "100", "2026-09-29 10:00:00"), fill(2, Kind::Sell, "0.005", "100", "2026-10-02 00:00:01"),
                    timed(3, "0.0001", Some("100"), day, "2026-10-02T00:00:05Z")];
        assert_eq!(matched(&rows), NONE);
    }

    /// Codex r3-1: a raw price of zero (or less) is a price that names no fill: the fee has no edge, matches nothing and
    /// blocks nothing; a fee with no price at all still reads every candidate.
    #[test]
    fn a_zero_raw_price_names_no_fill() {
        let day = "2026-10-01";
        let buy = || fill(1, Kind::Buy, "0.01", "100", "2026-10-01 10:00:00");
        assert_eq!(matched(&[buy(), timed(2, "0.0001", Some("0"), day, "2026-10-01T10:01:00Z")]), NONE);
        assert_eq!(matched(&[buy(), timed(2, "0.0001", Some("0"), day, "2026-10-01T10:01:00Z"), timed(3, "0.0001", Some("100"), day, "2026-10-01T10:02:00Z")]), [(3, 1)]);
        assert_eq!(matched(&[buy(), timed(2, "0.0001", None, day, "2026-10-01T10:01:00Z")]), [(2, 1)]);
    }

    /// Codex r3-2: everything the matcher holds is charged at its size: 1,000 fees with no fill to read still hold their
    /// list, kinds and edge lists (about 18,000 limbs), refused under 12,000 (the earlier charge was 8,000).
    #[test]
    fn the_matchers_own_lists_are_charged() {
        use crate::figures::budget;
        let rows: Vec<Stored> = (0..1000).map(|n| timed(n + 1, "0.0001", Some("100"), "2026-10-01", "2026-10-01T10:00:00Z")).collect();
        let (out, _) = budget::scope(budget::Limits { steps: budget::FIGURE.steps, held: 12_000 }, || cfee_matches(&rows));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET), "{:?}", out.map(|m| m.len()));
        assert_eq!(budget::scope(budget::FIGURE, || cfee_matches(&rows)).0.unwrap().len(), 0);
    }

    /// Codex r3-3: every edge read is a step, the claimants' count included: 2,000 unpriced fees after 2,000 sales
    /// (4,000,000 edges, read three times) are refused under 9,000,000 steps.
    #[test]
    fn every_edge_read_is_a_step() {
        use crate::figures::budget;
        let t0 = At::from_sql("2026-10-01 00:00:00").unwrap().0;
        let mut rows: Vec<Stored> = (0..2000).map(|n| Stored { at: At(t0 + n * 1_000_000_000), ..fill(n + 1, Kind::Sell, "1", "100", "2026-10-01 00:00:00") }).collect();
        for n in 0..2000 { rows.push(timed(10_000 + n, "0.0001", None, "2026-10-01", &At(t0 + (3000 + n) * 1_000_000_000).utc().to_rfc3339())); }
        let (out, _) = budget::scope(budget::Limits { steps: 9_000_000, held: budget::FIGURE.held }, || cfee_matches(&rows));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET), "{:?}", out.map(|m| m.len()));
    }

    /// Mutual uniqueness, whatever the charge order: a match is a fee with one compatible fill that no other fee may
    /// claim. No fee ever attaches to a fill another fee's price names; a fill two fees find stays both fees' standalone.
    #[test]
    fn no_fee_takes_a_fill_another_fee_finds() {
        let day = "2026-10-01";
        let (a, b) = ("2026-10-01T11:01:00Z", "2026-10-01T11:02:00Z");
        for (first, second) in [(a, b), (b, a)] {
            // An unpriced fee and an exact fee both find the buy; the sale's exact fee alone finds the sale.
            let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), timed(2, "0.0001", None, day, "2026-10-01T10:01:00Z"),
                        fill(3, Kind::Sell, "0.01", "100.04", "2026-10-01 11:00:00"), timed(4, "0.0001", Some("100.00"), day, first),
                        timed(5, "0.0002", Some("100.04"), day, second)];
            assert_eq!(matched(&rows), [(5, 3)], "{first}");
            // A fee whose price names no candidate finds none, and disputes nothing.
            let rows = [fill(1, Kind::Buy, "0.01", "100", "2026-10-01 10:00:00"), timed(2, "0.0001", Some("120"), day, first),
                        fill(3, Kind::Sell, "0.01", "100.04", "2026-10-01 11:00:00"), timed(4, "0.0002", Some("100"), day, second)];
            assert_eq!(matched(&rows), [(4, 1)], "{first}");
            // Three fees find the buy (no price, exact, within 0.1 %); the sale is within 0.1 % of none of them.
            let rows = [fill(1, Kind::Buy, "0.01", "100", "2026-10-01 10:00:00"), timed(2, "0.0001", None, day, "2026-10-01T10:01:00Z"),
                        fill(3, Kind::Sell, "0.01", "100.09", "2026-10-01 11:00:00"), timed(4, "0.0002", Some("100"), day, first),
                        timed(5, "0.0003", Some("99.95"), day, second)];
            assert_eq!(matched(&rows), NONE, "{first}");
        }
        // The r15 shape: the unpriced fee and the exact fee both find the buy: neither takes it, and the exact fee, whose
        // price names only the buy, does not take the sale either.
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), timed(2, "0.0001", None, day, "2026-10-01T10:01:00Z"),
                    fill(3, Kind::Sell, "0.01", "100.04", "2026-10-01 11:00:00"), timed(4, "0.0001", Some("100.00"), day, "2026-10-01T11:01:00Z")];
        assert_eq!(matched(&rows), NONE);
        // Two unpriced fees with the buy as their only candidate: neither, whatever their order.
        let rows = [fill(1, Kind::Buy, "0.01", "100.00", "2026-10-01 10:00:00"), timed(2, "0.0001", None, day, "2026-10-01T10:01:00Z"),
                    timed(3, "0.0002", None, day, "2026-10-01T10:02:00Z"), fill(4, Kind::Sell, "0.01", "100.04", "2026-10-01 11:00:00"),
                    timed(5, "0.0001", Some("100.00"), day, "2026-10-01T11:01:00Z")];
        assert_eq!(matched(&rows), NONE);
    }

    /// Who is a candidate at all: a buy no smaller than the fee, on or before the booking day, a crypto fill (a recorded
    /// security never, whatever its raw symbol), of the fee's pair's quote and spelling, the same recorded asset and alias,
    /// at most 72 hours before the charge and not after it (a truncated `created_at` reads as the end of its second).
    #[test]
    fn only_the_fees_own_crypto_fills_are_candidates() {
        let day = "2026-10-01";
        let fee = || timed(9, "0.001", None, day, "2026-10-01T12:00:00Z");
        let one = |x: Stored| matched(&[x, fee()]);
        assert_eq!(one(fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00")), [(9, 1)], "the control");
        assert_eq!(one(fill(1, Kind::Buy, "0.0009", "100", "2026-10-01 11:00:00")), NONE, "a buy smaller than the fee");
        assert_eq!(one(fill(1, Kind::Sell, "0.0009", "100", "2026-10-01 11:00:00")), [(9, 1)], "a smaller sale is one");
        assert_eq!(one(fill(1, Kind::Buy, "0.001", "100", "2026-09-28 11:59:59")), NONE, "more than 72 hours before");
        assert_eq!(one(fill(1, Kind::Buy, "0.001", "100", "2026-10-01 12:00:01")), NONE, "after the charge");
        let stock = |category: &str, symbol: &str| Stored { base_asset: Some((5, Some("BTC.US".into()), Some(category.into()))),
                                                              cfee: Cfee { symbol: Some(symbol.into()), ..Cfee::default() }, ..fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00") };
        assert_eq!(one(stock("Stock", "BTC")), NONE, "the BTC security");
        assert_eq!(one(stock("ETF", "BTC/USD")), NONE, "a recorded ETF with a pair-shaped symbol");
        assert_eq!(one(Stored { cfee: Cfee { symbol: Some("BTC".into()), ..Cfee::default() }, ..fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00") }), [],
                   "no recorded asset and a security's symbol");
        assert_eq!(one(Stored { quote: Some("USDT".into()), ..fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00") }), NONE, "another quote");
        assert_eq!(one(Stored { tx_id: Some("margin-interest-1".into()), ..fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00") }), NONE, "borrowed");
        assert_eq!(one(Stored { account: 2, ..fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00") }), NONE, "another account");
        assert_eq!(one(Stored { base: "XBT".into(), ..fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00") }), NONE, "another spelling");
        let coin = |id: i64| Some((id, Some("bitcoin".to_string()), Some("Cryptocurrency".to_string())));
        assert_eq!(matched(&[Stored { base_asset: coin(1), ..fill(1, Kind::Buy, "0.001", "100", "2026-10-01 11:00:00") }, Stored { base_asset: coin(2), ..fee() }]), [],
                   "another recorded asset");
        // ETHBTC: a pair against a coin; its ETH/USD buy is not its fill.
        let eth = Stored { base: "ETH".into(), cfee: Cfee { price: Some(dec("2000")), symbol: Some("ETH/USD".into()), ..Cfee::default() }, ..fill(1, Kind::Buy, "1", "2000", "2026-10-01 11:00:00") };
        assert_eq!(matched(&[eth, cfee(2, "ETH", "ETHBTC", "0.001", Some("0.03"), day, "2026-10-01T11:00:01Z")]), NONE);
        // A fill on a later UTC day than the booking is never its fill.
        assert_eq!(matched(&[fill(1, Kind::Buy, "0.001", "100", "2026-10-02 00:01:00"), timed(2, "0.0001", None, day, "2026-10-02T00:05:00Z")]), NONE);
        // A zero fee is no fee: it claims nothing, and disputes nothing.
        let rows = [fill(1, Kind::Buy, "0.01", "100", "2026-10-01 10:00:00"), timed(2, "0", Some("100"), day, "2026-10-01T10:00:01Z"),
                    timed(3, "0.0001", Some("100"), day, "2026-10-01T10:00:02Z")];
        assert_eq!(matched(&rows), [(3, 1)]);
        // Sub-second fills: a truncated `created_at` admits the fill a fraction later in its second; a fractional one does not
        // admit one after it; an equal instant is a candidate.
        assert_eq!(matched(&[fill(1, Kind::Buy, "0.01", "100", "2026-10-01 23:08:50.400"), timed(2, "0.0001", None, day, "2026-10-01T23:08:50Z")]), [(2, 1)]);
        assert_eq!(matched(&[fill(1, Kind::Buy, "0.01", "100", "2026-10-01 12:00:01.100"), timed(2, "0.0001", None, day, "2026-10-01T12:00:00.900Z")]), NONE);
        assert_eq!(matched(&[fill(1, Kind::Buy, "0.01", "100", "2026-10-01 12:00:00.400"), timed(2, "0.0001", None, "2026-10-02", "2026-10-01T12:00:00.400Z")]), [(2, 1)]);
    }

    /// A dated alias: LUNA bought on 27 May 2022 was Terra; a fee charged on the 29th is Terra 2's, whatever id both rows
    /// recorded (sync names an id without the date).
    #[test]
    fn a_fee_and_a_fill_of_two_coins_under_one_alias_do_not_match() {
        let luna = |asset: Option<i64>| Stored { base: "LUNA".into(), base_asset: asset.map(|id| (id, Some("terra-luna".to_string()), Some("Cryptocurrency".to_string()))),
                                                 cfee: Cfee { price: Some(dec("0.5")), symbol: Some("LUNA/USD".into()), ..Cfee::default() }, ..fill(1, Kind::Buy, "100", "0.5", "2022-05-27 12:00:00") };
        let fee = |asset: Option<i64>, created: &str| Stored { base_asset: asset.map(|id| (id, Some("terra-luna".to_string()), Some("Cryptocurrency".to_string()))),
                                                                ..cfee(2, "LUNA", "LUNAUSD", "0.1", None, &created[..10], created) };
        for asset in [None, Some(3)] {
            assert_eq!(matched(&[luna(asset), fee(asset, "2022-05-29T01:00:00Z")]), NONE, "{asset:?}");
            assert_eq!(matched(&[luna(asset), fee(asset, "2022-05-27T12:00:05Z")]), [(2, 1)], "{asset:?}: the same day's");
        }
    }

    /// Codex review 2: a fee that may not claim a fill (booked the day before it) does not dispute it with a fee that may.
    #[test]
    fn a_fee_booked_before_a_fill_does_not_dispute_it() {
        let rows = [fill(1, Kind::Buy, "0.01", "100", "2026-10-02 00:00:01"), timed(2, "0.0001", Some("100"), "2026-10-01", "2026-10-02T00:00:03Z"),
                    timed(3, "0.0001", Some("100"), "2026-10-02", "2026-10-02T00:00:04Z")];
        assert_eq!(matched(&rows), [(3, 1)]);
    }

    /// Codex review 5: a fee booked on a day whose alias reads another coin than its fill's (a LUNA fee of 27 May 2022
    /// booked Monday 30 May) is not matched: its row, kept on its booking day, would be the other coin's.
    #[test]
    fn a_fee_booked_under_another_alias_than_its_fill_is_not_matched() {
        let luna = Stored { base: "LUNA".into(), cfee: Cfee { price: Some(dec("0.5")), symbol: Some("LUNA/USD".into()), ..Cfee::default() }, ..fill(1, Kind::Buy, "100", "0.5", "2022-05-27 12:00:00") };
        assert_eq!(matched(&[luna.clone(), cfee(2, "LUNA", "LUNAUSD", "0.1", None, "2022-05-27", "2022-05-27T12:00:05Z")]), [(2, 1)]);
        assert_eq!(matched(&[luna, cfee(2, "LUNA", "LUNAUSD", "0.1", None, "2022-05-30", "2022-05-27T12:00:05Z")]), NONE);
    }

    /// Codex review 6: a dense history (every fee within 72 hours of every fill) stores its candidates on the budget: under
    /// a small allowance it is refused, not grown. A long sparse one (one fee per fill) is linear.
    #[test]
    fn matching_is_bounded_by_the_budget() {
        use crate::figures::budget;
        let at = |n: i64| (At::from_sql("2026-10-01 00:00:00").unwrap().0 + n * 1_000_000_000, n);
        let mut rows: Vec<Stored> = (0..2000).map(|n| Stored { at: At(at(n).0), ..fill(n + 1, Kind::Buy, "1", "100", "2026-10-01 00:00:00") }).collect();
        for n in 0..2000 { rows.push(timed(10_000 + n, "0.0001", Some("100"), "2026-10-01", &At(at(3000 + n).0).utc().to_rfc3339())); }
        // 4,000,000 edges of 8 bytes: 8,000,000 limbs of 4. Refused under less, before it is stored; under a figure's
        // allowance it completes in linear work (each candidate read twice, its price compared), and every fill is two
        // fees' at least: nothing matches.
        let (out, _) = budget::scope(budget::Limits { steps: budget::FIGURE.steps, held: 7_900_000 }, || cfee_matches(&rows));
        assert!(matches!(out, Err(FiguresError::NotComputed(ref m)) if m == crate::figures::OVER_BUDGET), "{:?}", out.map(|m| m.len()));
        let (out, used) = budget::scope(budget::FIGURE, || cfee_matches(&rows));
        assert_eq!(out.unwrap().len(), 0);
        assert!(used.steps < 100_000_000, "{} steps: linear in the 4,000,000 candidates, each read twice", used.steps);
        let mut rows = vec![];
        for n in 0..20_000i64 {
            let t = At::from_sql("2026-01-01 00:00:00").unwrap().0 + n * 3_600_000_000_000;
            let price = format!("{}", 100 + n);
            rows.push(Stored { at: At(t), ..fill(n + 1, Kind::Buy, "1", &price, "2026-01-01 00:00:00") });
            let booked = At(t).utc().date_naive().to_string();
            rows.push(timed(100_000 + n, "0.0001", Some(&price), &booked, &At(t + 1_000_000_000).utc().to_rfc3339()));
        }
        let (out, used) = budget::scope(budget::FIGURE, || cfee_matches(&rows));
        assert_eq!(out.unwrap().len(), 20_000);
        assert!(used.steps < 20_000_000, "{} steps: linear in the 1,440,000 candidates (72 each), not the 400,000,000 pairs", used.steps);
    }

    /// Instants: a fee charged past midnight still moves to its fill; one that matched nothing and was charged on a later
    /// day stands at its booking day's last nanosecond, after every row of that day; fees disputing a fill stand after
    /// it, the last of their instant.
    #[test]
    fn a_timed_fee_moves_forward_within_its_booking_day() {
        let rows = placed(vec![timed(1, "0.0001", Some("100"), "2026-10-01", "2026-10-02T00:00:03Z"), fill(2, Kind::Buy, "0.01", "100", "2026-10-01 23:59:58")]);
        assert_eq!((ids(&rows), at(&rows, 1)), (vec![2, 1], "2026-10-01 23:59:58.000000000".to_string()));
        let rows = placed(vec![timed(1, "0.0001", Some("100"), "2026-10-01", "2026-10-02T00:00:02Z"), fill(2, Kind::Buy, "0.01", "100", "2026-10-01 23:00:00"),
                               fill(3, Kind::Buy, "0.01", "100", "2026-10-01 23:30:00")]);
        assert_eq!((ids(&rows), at(&rows, 1), rows[2].cfee.fill), (vec![2, 3, 1], "2026-10-01 23:59:59.999999999".to_string(), None));
        let rows = placed(vec![timed(1, "0.0001", Some("100"), "2026-10-01", "2026-10-01T00:00:00.400Z"), timed(2, "0.0001", Some("100"), "2026-10-01", "2026-10-01T00:00:00.400Z"),
                               fill(3, Kind::Buy, "0.01", "100", "2026-10-01 00:00:00.400")]);
        assert_eq!((ids(&rows), at(&rows, 1), at(&rows, 2)), (vec![3, 1, 2], "2026-10-01 00:00:00.400000000".to_string(), "2026-10-01 00:00:00.400000000".to_string()));
        assert!(rows.iter().all(|r| r.cfee.fill.is_none()));
        // A charge before its booking day keeps the booking instant; an unmatched fee charged on its day moves to the charge.
        let rows = placed(vec![timed(1, "0.0001", None, "2026-10-05", "2026-10-03T10:00:00Z")]);
        assert_eq!(at(&rows, 1), "2026-10-05 00:00:00.000000000");
        let rows = placed(vec![timed(1, "0.0001", None, "2026-10-05", "2026-10-05T10:00:00.250Z")]);
        assert_eq!(at(&rows, 1), "2026-10-05 10:00:00.250000000");
    }

    /// A matched fee at its fill's instant takes its fill's place, before a sale the fill precedes; with its fill out of the
    /// slice (a borrowed filter, the tax scope) it is the last of its instant, as a standalone fee; a legacy fee keeps v2's.
    #[test]
    fn a_matched_fee_takes_its_fills_place() {
        let t = "2026-10-01 10:00:00";
        let mut fee = timed(1, "0.1", Some("100"), "2026-10-01", "2026-10-01T10:00:01Z");
        (fee.at, fee.cfee.fill) = (At::from_sql(t).unwrap(), Some(2));
        let rows = vec![fill(3, Kind::Sell, "1", "100", t), fee.clone(), fill(2, Kind::Buy, "1", "100", t)];
        assert_eq!(ids(&ordered(rows.clone())), [2, 1, 3]);
        assert_eq!(ids(&ordered(rows.into_iter().filter(|r| r.id != 2).collect())), [3, 1]);
        let legacy = Stored { cfee: Cfee { fee: true, ..Cfee::default() }, ..stored(1, Kind::Fee, None, t) };
        assert_eq!(ids(&ordered(vec![fill(3, Kind::Sell, "1", "100", t), legacy, fill(2, Kind::Buy, "1", "100", t)])), [1, 2, 3]);
    }

    /// A timed fee's own price: dollars on a USD, USDT or USDC pair of its coin; none on a coin-quoted pair, for a
    /// stablecoin's own fee (cash at face), at no price, or an unreadable one. A legacy fee's fills priced at nothing
    /// price no fee.
    #[test]
    fn a_timed_fees_raw_price_is_dollars_only() {
        let raw = |symbol: &str, price: serde_json::Value| serde_json::json!({ "activity_type": "CFEE", "symbol": symbol, "price": price });
        assert_eq!(raw_price("BTC", &raw("BTCUSD", "84674.02".into())), Some(dec("84674.02")));
        assert_eq!(raw_price("BTC", &raw("BTC/USDC", "1".into())), Some(dec("1")));
        for (base, symbol, price) in [("ETH", "ETHBTC", "0.03"), ("USDC", "USDCUSD", "0.98"), ("BTC", "BTCUSD", "0"), ("BTC", "BTCUSD", "n/a"), ("BTC", "BTCEUR", "1")] {
            assert_eq!(raw_price(base, &raw(symbol, price.into())), None, "{symbol} {price}");
        }
        assert_eq!(charged(&serde_json::json!({ "activity_type": "CFEE" })), None, "a legacy fee");
        assert_eq!(charged(&serde_json::json!({ "activity_type": "FILL", "created_at": "2026-10-01T10:00:00Z" })), None);
        assert_eq!(charged(&serde_json::json!({ "activity_type": "CFEE", "created_at": "2026-10-01T10:00:00Z" })).map(|a| a.0 % 1_000_000_000), Some(999_999_999));
        let (a, b) = (fill(1, Kind::Buy, "1", "100", "2026-10-01 10:00:00"), fill(2, Kind::Buy, "1", "0", "2026-10-01 11:00:00"));
        assert_eq!(fill_price(&[&a, &b]).unwrap(), None);
        assert_eq!(fill_price(&[&a]).unwrap().map(|p| p.0.to_s_f()), Some("100.0".to_string()));
    }

    /// The legacy reading is for a fee without `created_at` only: a timed fee with a buy the day before is not its fill,
    /// and is priced at nothing of that buy's.
    #[test]
    fn a_timed_fee_takes_no_previous_day_fill() {
        let mut rows = vec![fill(1, Kind::Buy, "1", "90", "2026-09-30 10:00:00"), timed(2, "0.001", Some("100"), "2026-10-01", "2026-10-01T10:00:00Z")];
        let legacy = Stored { cfee: Cfee { fee: true, ..Cfee::default() }, ..timed(3, "0.001", None, "2026-10-01", "2026-10-01T10:00:00Z") };
        rows.push(legacy);
        place_cfees(&mut rows).unwrap();
        price_cfees(&mut rows).unwrap();
        assert_eq!(cfee_places(&rows).into_iter().map(|(i, f)| (rows[i].id, f.into_iter().map(|j| rows[j].id).collect::<Vec<_>>())).collect::<Vec<_>>(), [(3, vec![1])]);
        let fee = rows.iter().find(|r| r.id == 2).unwrap();
        assert_eq!((fee.cfee.fill, fee.cfee.fill_price.clone(), fee.cfee.raw_price.clone()), (None, None, Some(dec("100"))));
    }
}
