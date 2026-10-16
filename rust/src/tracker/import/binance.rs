//! Import::BinanceCsv, line for line: Binance's own transaction export read into the entries
//! Exchanges::Binance#get_ledger makes of the same events. Every list keeps Ruby's order (`group_by` by first
//! appearance, `partition`, `filter_map`) and every `sort_by` is a stable sort, as Ruby's is on the app's Linux image
//! (glibc's qsort_r is a merge sort). Pinned by rust/tests/fixtures/import_vectors.json.
use super::csv::{self, Malformed, Table, UTC_TIME};
use crate::ruby::{self, BigDec};
use crate::sync::activities::{Entry, Raw};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::json;
use std::sync::LazyLock;

/// Import::BinanceCsv::OPERATIONS, in its order: every operation the export writes and what it is. None is a
/// deliberate skip (money moving between the user's own Binance wallets); an operation absent here is reported.
pub const OPERATIONS: &[(&str, Option<&str>)] = &[
    ("Transaction Buy", Some("trade_in")), ("Transaction Spend", Some("trade_out")),
    ("Transaction Sold", Some("trade_out")), ("Transaction Revenue", Some("trade_in")),
    ("Transaction Fee", Some("trade_fee")),
    ("Buy", Some("trade_in")), ("Sell", Some("trade_out")), ("Fee", Some("trade_fee")),
    ("Binance Convert", Some("convert")),
    ("Small Assets Exchange BNB", Some("dust")),
    ("Deposit", Some("deposit")), ("Fiat Deposit", Some("deposit")),
    ("Withdraw", Some("withdrawal")), ("Fiat Withdraw", Some("withdrawal")),
    ("Commission Rebate", Some("other_income")),
    ("Commission History", Some("other_income")),
    ("Referral Kickback", Some("other_income")),
    ("Commission Fee Shared With You", Some("other_income")),
    ("Distribution", Some("other_income")),
    ("Asset Recovery", Some("other_income")),
    ("Asset - Transfer", Some("other_income")),
    ("Token Swap - Redenomination/Rebranding", Some("other_income")),
    ("Pool Distribution", Some("staking_reward")),
    ("Staking Rewards", Some("staking_reward")),
    ("Simple Earn Flexible Interest", Some("lending_interest")),
    ("Simple Earn Locked Rewards", Some("lending_interest")),
    ("Savings Interest", Some("lending_interest")),
    ("Airdrop Assets", Some("airdrop")),
    ("Token Swap - Distribution", Some("airdrop")),
    ("Transfer Between Spot and Funding", None),
    ("Transfer Between Main and Funding Account", None),
    ("Ledger - Fund Migration", None),
    ("transfer_in", None), ("transfer_out", None),
];

/// Import::BinanceCsv::CASH: a trade against one of these is a purchase or a sale, against anything else a swap.
pub const CASH: &str = r"\A(USDT|USDC|BUSD|DAI|FDUSD|TUSD|USD|EUR|GBP|CHF|SEK|PLN|DKK|CZK|BGN|AUD|CAD|JPY|AED|TRY|BRL|RUB|UAH|NGN|ZAR)\z";
static CASH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(CASH).unwrap());
/// Import::BinanceCsv::CONVERT_WINDOW: how far apart, in seconds, a Convert's two halves may be written.
pub const CONVERT_WINDOW: i64 = 60;

fn operation(name: &str) -> Option<Option<&'static str>> {
    OPERATIONS.iter().find(|(op, _)| *op == name).map(|(_, kind)| *kind)
}

/// One parsed row (`parse_rows`' hash). Equality is the hash's: every field, the amount by value.
#[derive(Clone, Debug, PartialEq)]
struct Row { kind: &'static str, operation: String, coin: String, change: BigDec, at: DateTime<Utc>, account: String, remark: String }

/// A coin and a signed amount: all a trade leg, a split leg or a fee is read for.
#[derive(Clone, Debug)]
struct Leg { coin: String, change: BigDec }

impl From<&Row> for Leg { fn from(r: &Row) -> Self { Self { coin: r.coin.clone(), change: r.change.clone() } } }

fn negative(d: &BigDec) -> bool { !d.is_zero() && !d.is_positive() }
fn abs(d: &BigDec) -> BigDec { if negative(d) { &BigDec::zero() - d } else { d.clone() } }
fn sum<'a>(ds: impl Iterator<Item = &'a BigDec>) -> BigDec { ds.fold(BigDec::zero(), |a, d| &a + d) }
fn legs(rows: &[&Row]) -> Vec<Leg> { rows.iter().map(|r| Leg::from(*r)).collect() }
/// `sort_by` on a key, stable.
fn sorted<T: Clone>(rows: &[T], key: impl Fn(&T, &T) -> std::cmp::Ordering) -> Vec<T> { let mut v = rows.to_vec(); v.sort_by(key); v }
/// `group_by`: groups in the order their keys first appear.
fn group_by<T: Clone, K: PartialEq>(rows: &[T], key: impl Fn(&T) -> K) -> Vec<(K, Vec<T>)> {
    let mut groups: Vec<(K, Vec<T>)> = vec![];
    for row in rows {
        let k = key(row);
        match groups.iter_mut().find(|(g, _)| *g == k) { Some((_, g)) => g.push(row.clone()), None => groups.push((k, vec![row.clone()])) }
    }
    groups
}
fn utc(at: &DateTime<Utc>) -> String { at.format("%Y-%m-%d %H:%M:%S").to_string() }

/// One read of one file: `entries` and what `unrecognised` holds after them.
#[derive(Debug, Clone, PartialEq)]
pub struct Read { pub entries: Vec<Entry>, pub unrecognised: Vec<String> }

/// Import::BinanceCsv.new(text, offset:).entries, with #unrecognised. `text` is already Import::Run.as_text's.
pub fn read(text: &str, offset: Option<&str>) -> Result<Read, Malformed> {
    let offset = offset.filter(|o| !ruby::blank(o)).unwrap_or("+00:00");
    let mut reader = Reader { offset: offset.to_string(), unrecognised: vec![] };
    let entries = reader.entries(text)?;
    Ok(Read { entries, unrecognised: reader.unrecognised })
}

struct Reader { offset: String, unrecognised: Vec<String> }

impl Reader {
    fn note(&mut self, note: String) { if !self.unrecognised.contains(&note) { self.unrecognised.push(note); } }

    fn entries(&mut self, text: &str) -> Result<Vec<Entry>, Malformed> {
        let rows = self.parse_rows(text)?;
        let (trades, singles): (Vec<Row>, Vec<Row>) =
            rows.into_iter().partition(|r| ["trade_in", "trade_out", "trade_fee", "convert", "dust"].contains(&r.kind));
        let mut out: Vec<Entry> = self.movement_entries(&singles).into_iter().flatten().collect();
        out.extend(self.grouped_entries(&trades).into_iter().flatten());
        out.sort_by_key(|e| e.transacted_at);
        Ok(out)
    }

    fn parse_rows(&mut self, text: &str) -> Result<Vec<Row>, Malformed> {
        let table = Table::parse(text)?;
        let utc = table.has_header(UTC_TIME);
        let mut rows = vec![];
        for row in &table.rows {
            let cell = |name: &str| ruby::strip(table.get(row, name).unwrap_or("")).to_string();
            let op = cell("Operation");
            if ruby::blank(&op) { continue; }
            let Some(kind) = operation(&op) else { self.note(op); continue };
            let Some(kind) = kind else { continue }; // a deliberate skip, not an unknown
            let change = csv::amount(table.get(row, "Change"))?;
            let at = if utc { csv::time(table.get(row, UTC_TIME), Some("+00:00")) } else { csv::time(table.get(row, "Time"), Some(&self.offset)) }?;
            rows.push(Row { kind, operation: op, coin: cell("Coin"), change, at, account: cell("Account"), remark: cell("Remark") });
        }
        Ok(rows)
    }

    /// A movement's direction is its Change's sign: a group of one (time, operation) with legs both ways is a swap.
    fn movement_entries(&self, rows: &[Row]) -> Vec<Option<Entry>> {
        group_by(rows, |r| (r.at, r.operation.clone())).into_iter().flat_map(|((at, _), group)| {
            let (out, inn): (Vec<&Row>, Vec<&Row>) = group.iter().partition(|r| negative(&r.change));
            if !out.is_empty() && !inn.is_empty() { return swap_pairs(at, &legs(&out), &legs(&inn)).into_iter().map(Some).collect(); }
            group.iter().map(single_entry).collect::<Vec<_>>()
        }).collect()
    }

    fn grouped_entries(&mut self, rows: &[Row]) -> Vec<Option<Entry>> {
        let (converts, rest): (Vec<Row>, Vec<Row>) = rows.iter().cloned().partition(|r| r.kind == "convert");
        let mut out: Vec<Option<Entry>> = self.convert_entries(&converts).into_iter().map(Some).collect();
        for ((at, _), group) in group_by(&rest, |r| (r.at, r.account.clone())) {
            let (dust, trade): (Vec<Row>, Vec<Row>) = group.into_iter().partition(|r| r.kind == "dust");
            out.extend(dust_entries(at, &dust));
            out.extend(self.trade_entries(at, &trade).into_iter().map(Some));
        }
        out
    }

    /// Each spend paired with the nearest-in-order credit within CONVERT_WINDOW, always as a swap; a half with no
    /// partner is reported, not imported.
    fn convert_entries(&mut self, rows: &[Row]) -> Vec<Entry> {
        let (spent, received): (Vec<Row>, Vec<Row>) = rows.iter().cloned().partition(|r| negative(&r.change));
        let mut received = sorted(&received, |a, b| a.at.cmp(&b.at));
        let mut out = vec![];
        for spend in sorted(&spent, |a, b| a.at.cmp(&b.at)) {
            let Some(found) = received.iter().find(|r| (r.at - spend.at).abs() <= chrono::Duration::seconds(CONVERT_WINDOW)).cloned() else {
                self.lone_convert(&spend);
                continue;
            };
            received.retain(|r| *r != found); // Array#delete: every equal row
            out.extend(swap(spend.at, &(&spend).into(), &(&found).into(), None));
        }
        for row in &received { self.lone_convert(row); }
        out
    }

    fn lone_convert(&mut self, row: &Row) {
        let change = row.change.to_s_f();
        let change = change.strip_suffix(".0").unwrap_or(&change);
        self.note(format!("{} {} {} {} UTC", row.operation, row.coin, change, utc(&row.at)));
    }

    /// One trade per fill, legs paired by size; a group with nothing on one side is reported row by row.
    fn trade_entries(&mut self, at: DateTime<Utc>, rows: &[Row]) -> Vec<Entry> {
        let fees: Vec<&Row> = rows.iter().filter(|r| r.kind == "trade_fee").collect();
        let legs_: Vec<&Row> = rows.iter().filter(|r| !fees.contains(r)).collect(); // rows - fees
        let incoming = sorted(&legs_.iter().copied().filter(|r| r.change.is_positive()).collect::<Vec<_>>(), |a, b| b.change.cmp(&a.change));
        let outgoing = sorted(&legs_.iter().copied().filter(|r| negative(&r.change)).collect::<Vec<_>>(), |a, b| a.change.cmp(&b.change));
        if incoming.is_empty() || outgoing.is_empty() {
            for r in rows { self.note(format!("{} {} {} UTC", r.operation, r.coin, utc(&r.at))); }
            return vec![];
        }
        let (outgoing, incoming, fees) = (legs(&outgoing), legs(&incoming), legs(&fees));
        if mixed_coins(&outgoing) || mixed_coins(&incoming) { return swap_group(at, &outgoing, &incoming, &fees); }
        let (outgoing, incoming) = even_up(outgoing, incoming);
        let (slots, extra) = spread_fees(at, &fees, &incoming.iter().map(|l| l.change.clone()).collect::<Vec<_>>());
        let mut out: Vec<Entry> = outgoing.iter().zip(&incoming).zip(&slots).flat_map(|((spent, bought), fee)| trade(at, spent, bought, fee.as_ref())).collect();
        out.extend(extra);
        out
    }
}

fn mixed_coins(legs: &[Leg]) -> bool { legs.iter().any(|l| l.coin != legs[0].coin) }

/// A rename's legs paired by size, or one swap holding every leg when the two sides differ in count.
fn swap_pairs(at: DateTime<Utc>, out: &[Leg], inn: &[Leg]) -> Vec<Entry> {
    let out = sorted(out, |a, b| a.change.cmp(&b.change));
    let inn = sorted(inn, |a, b| b.change.cmp(&a.change));
    if out.len() != inn.len() { return swap_group(at, &out, &inn, &[]); }
    out.iter().zip(&inn).flat_map(|(sold, bought)| swap(at, sold, bought, None)).collect()
}

fn single_entry(row: &Row) -> Option<Entry> {
    let amount = abs(&row.change);
    if amount.is_zero() { return None; }
    // outward_type: a negative change that is not a withdrawal is an asset simply gone.
    let kind = if row.change.is_positive() || row.kind == "withdrawal" { row.kind } else { "lost" };
    let mut e = leg(row.at, &row.coin, amount, kind, None);
    e.description = Some(row.operation.clone());
    e.raw = Raw::from_value(json!({ "operation": row.operation }));
    Some(e)
}

/// Dust: one pair per Remark; a batch with any blank remark is one pair against the summed receipt.
fn dust_entries(at: DateTime<Utc>, rows: &[Row]) -> Vec<Option<Entry>> {
    if rows.is_empty() { return vec![]; }
    if rows.iter().any(|r| ruby::blank(&r.remark)) { return dust_batch(at, rows, "dustcsv"); }
    group_by(rows, |r| r.remark.clone()).into_iter().flat_map(|(remark, legs)| dust_batch(at, &legs, &format!("dustcsv_{remark}"))).collect()
}

fn dust_batch(at: DateTime<Utc>, rows: &[Row], key: &str) -> Vec<Option<Entry>> {
    let (spent, received): (Vec<&Row>, Vec<&Row>) = rows.iter().partition(|r| negative(&r.change));
    if spent.is_empty() { return received.iter().map(|r| single_entry(&Row { kind: "other_income", ..(*r).clone() })).collect(); }
    swap_legs(at, &legs(&spent), &legs(&received), &format!("{key}_{}", at.timestamp()), &[]).into_iter().map(Some).collect()
}

/// Where one side was written in fewer rows than the other has fills, that side summed and split by quantity.
fn even_up(outgoing: Vec<Leg>, incoming: Vec<Leg>) -> (Vec<Leg>, Vec<Leg>) {
    if outgoing.len() == incoming.len() { return (outgoing, incoming); }
    let many_out = outgoing.len() > incoming.len();
    let (many, few) = if many_out { (&outgoing, &incoming) } else { (&incoming, &outgoing) };
    let total = sum(few.iter().map(|l| &l.change));
    let split: Vec<Leg> = split(&total, &many.iter().map(|l| abs(&l.change)).collect::<Vec<_>>())
        .into_iter().map(|change| Leg { coin: few[0].coin.clone(), change }).collect();
    if many_out { (outgoing, split) } else { (split, incoming) }
}

/// `total` in parts proportional to `weights`, each cut toward zero at 8 places (or the total's own); the last part
/// takes the remainder, so the parts sum to the total exactly.
fn split(total: &BigDec, weights: &[BigDec]) -> Vec<BigDec> {
    let s = total.to_s_f();
    let places = s.split('.').nth(1).map_or(0, str::len).max(8);
    let places = u8::try_from(places).unwrap_or(u8::MAX);
    let all = sum(weights.iter());
    let mut parts: Vec<BigDec> = weights[..weights.len().saturating_sub(1)].iter().map(|w| {
        let share = (total * w).div(&all).unwrap_or_else(BigDec::zero);
        if negative(&share) { share.ceil(places) } else { share.floor(places) } // ROUND_DOWN
    }).collect();
    parts.push(total - &sum(parts.iter()));
    parts
}

/// [a fee per slot, the fee rows no slot could hold].
fn spread_fees(at: DateTime<Utc>, fees: &[Leg], weights: &[BigDec]) -> (Vec<Option<Leg>>, Vec<Entry>) {
    let slots = weights.len();
    if fees.is_empty() { return (vec![None; slots], vec![]); }
    let sorted_fees = sorted(fees, |a, b| a.change.cmp(&b.change));
    if sorted_fees.len() == slots { return (sorted_fees.into_iter().map(Some).collect(), vec![]); }
    let coin = sorted_fees[0].coin.clone();
    if sorted_fees.iter().all(|f| f.coin == coin) {
        let total = sum(sorted_fees.iter().map(|f| &f.change));
        let parts = split(&total, &weights.iter().map(abs).collect::<Vec<_>>());
        return (parts.into_iter().map(|change| Some(Leg { coin: coin.clone(), change })).collect(), vec![]);
    }
    let per_coin: Vec<Leg> = group_by(&sorted_fees, |f| f.coin.clone()).into_iter()
        .map(|(coin, rows)| Leg { coin, change: sum(rows.iter().map(|f| &f.change)) }).collect();
    let extra = per_coin.iter().skip(slots).map(|f| leg(at, &f.coin, abs(&f.change), "fee", None)).collect();
    let mut held: Vec<Option<Leg>> = per_coin.into_iter().take(slots).map(Some).collect();
    held.resize(slots, None);
    (held, extra)
}

/// Legs that cannot be paired: one swap, every leg its own row.
fn swap_group(at: DateTime<Utc>, outgoing: &[Leg], incoming: &[Leg], fees: &[Leg]) -> Vec<Entry> {
    let coins = |legs: &[Leg]| {
        let mut seen: Vec<&str> = vec![];
        for l in legs { if !seen.contains(&l.coin.as_str()) { seen.push(&l.coin); } }
        seen.join("-")
    };
    swap_legs(at, outgoing, incoming, &format!("swapcsv_{}_{}_{}", at.timestamp(), coins(outgoing), coins(incoming)), fees)
}

fn swap_legs(at: DateTime<Utc>, spent: &[Leg], received: &[Leg], group_id: &str, fees: &[Leg]) -> Vec<Entry> {
    let (slots, extra) = spread_fees(at, fees, &received.iter().map(|l| l.change.clone()).collect::<Vec<_>>());
    let mut out: Vec<Entry> = spent.iter().map(|l| leg(at, &l.coin, abs(&l.change), "swap_out", Some(group_id))).collect();
    out.extend(received.iter().zip(slots).map(|(l, fee)| with_fee(leg(at, &l.coin, l.change.clone(), "swap_in", Some(group_id)), fee.as_ref())));
    out.extend(extra);
    out
}

/// A fill is a purchase when it spent cash, a sale when it received cash, else a swap.
fn trade(at: DateTime<Utc>, spent: &Leg, bought: &Leg, fee: Option<&Leg>) -> Vec<Entry> {
    if CASH_RE.is_match(&spent.coin) {
        let mut e = leg(at, &bought.coin, bought.change.clone(), "buy", None);
        (e.quote_currency, e.quote_amount) = (Some(spent.coin.clone()), Some(abs(&spent.change)));
        return vec![with_fee(e, fee)];
    }
    if CASH_RE.is_match(&bought.coin) {
        let mut e = leg(at, &spent.coin, abs(&spent.change), "sell", None);
        (e.quote_currency, e.quote_amount) = (Some(bought.coin.clone()), Some(bought.change.clone()));
        return vec![with_fee(e, fee)];
    }
    swap(at, spent, bought, fee)
}

fn swap(at: DateTime<Utc>, sold: &Leg, bought: &Leg, fee: Option<&Leg>) -> Vec<Entry> {
    let group_id = format!("swapcsv_{}_{}_{}", at.timestamp(), sold.coin, bought.coin);
    vec![leg(at, &sold.coin, abs(&sold.change), "swap_out", Some(&group_id)),
         with_fee(leg(at, &bought.coin, bought.change.clone(), "swap_in", Some(&group_id)), fee)]
}

fn with_fee(mut e: Entry, fee: Option<&Leg>) -> Entry {
    if let Some(fee) = fee { (e.fee_currency, e.fee_amount) = (Some(fee.coin.clone()), Some(abs(&fee.change))); }
    e
}

fn leg(at: DateTime<Utc>, coin: &str, amount: BigDec, entry_type: &str, group_id: Option<&str>) -> Entry {
    Entry {
        entry_type: super::entry_type(entry_type).expect("a ledger entry type"),
        base_currency: Some(coin.to_string()), base_amount: amount,
        quote_currency: None, quote_amount: None, fee_currency: None, fee_amount: None,
        tx_id: None, group_id: group_id.map(str::to_string), description: None,
        transacted_at: Some(at), raw: Raw::from_value(json!({})),
    }
}
