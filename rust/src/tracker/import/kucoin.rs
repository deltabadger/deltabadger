//! KuCoin's "Account History" export (Trading Account and Funding Account), Rust-only (plan KI): one row per balance
//! change. The rows are stored as they are (`import_rows`, keyed by an id of the row itself), and the ledger entries are
//! derived from the stored rows of one UTC second at a time, so supersets, subsets and re-uploads reconcile row by row
//! and the result never depends on upload order. `read` is database-free; `store` runs in the import's transaction.
use super::csv::{self, Malformed, Table};
use crate::ruby::BigDec;
use crate::sync::activities::{Entry, Raw, ADJUSTMENT, BUY, DEPOSIT, FEE, SELL, SWAP_IN, SWAP_OUT, WITHDRAWAL};
use crate::sync::{ledger, sql_time, Key, SyncError};
use chrono::{DateTime, Timelike, Utc};
use regex::Regex;
use rusqlite::params;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

/// The export's columns (its `UID` is read by `identity::export_account_id`, never stored here).
const COLUMNS: [&str; 7] = ["Account Type", "Currency", "Side", "Amount", "Fee", "Remark", "Type"];
/// The time column names its zone: `Time(UTC+03:00)`, `Time(UTC-5)`, `Time(UTC)`.
static TIME_HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^Time\(UTC(?:([+-])([0-9]{1,2})(?::?([0-5][0-9]))?)?\)$").unwrap());
/// The exhaustive pairing search runs up to this many legs on the smaller side; past it, legs are paired in size order.
const EXHAUSTIVE: usize = 8;

/// What each export `Type` is. A Type absent here is a signed adjustment of its currency (`unmapped_type`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind { Spot, Deposit, Withdrawal, Internal, Convert, Income(&'static str) }

/// Every Type seen in the provided exports, and the ones KuCoin's ledger names (bizType) whose meaning is certain.
/// Earn subscriptions, redemptions and interest are not here: the export's names for them are unconfirmed, so they
/// stay signed adjustments, which keep holdings exact.
const TYPES: &[(&str, Kind)] = &[
    ("Spot", Kind::Spot), ("Deposit", Kind::Deposit), ("Withdrawal", Kind::Withdrawal), ("Transfer", Kind::Internal),
    ("Convert", Kind::Convert), ("Instant Exchange", Kind::Convert), ("Convert to KCS", Kind::Convert),
    ("Staking Profits", Kind::Income("staking_reward")), ("Soft Staking Profits", Kind::Income("staking_reward")),
    ("Rewards", Kind::Income("other_income")), ("KuCoin Bonus", Kind::Income("other_income")), ("Referral Bonus", Kind::Income("other_income")),
    ("Fee Rebate", Kind::Income("other_income")), ("Refunded Fees", Kind::Income("other_income")), ("Other rewards", Kind::Income("other_income")),
    ("Distribution", Kind::Income("other_income")), ("Airdrop/Fork", Kind::Income("airdrop")),
];

/// One export row, as stored. `inflow` is Side Deposit (in) or Withdrawal (out); `at` its UTC second.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRow {
    pub row_id: String, pub account_type: String, pub currency: String, pub inflow: bool,
    pub amount: BigDec, pub fee: BigDec, pub at: DateTime<Utc>, pub remark: String, pub kind: String,
}

fn hex32(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts { h.update(p.as_bytes()); h.update([0]); }
    hex::encode(h.finalize())[..32].to_string()
}
fn second(at: &DateTime<Utc>) -> String { at.format("%Y-%m-%d %H:%M:%S").to_string() }
fn side(inflow: bool) -> &'static str { if inflow { "Deposit" } else { "Withdrawal" } }

/// The export's rows, each with its id: a hash of the row (amounts as canonical decimals, the time as its UTC second)
/// and its occurrence among identical rows of the file (1, 2 …). `table` is the file as `CSV.parse` reads it.
pub fn read(table: &Table) -> Result<Vec<SourceRow>, Malformed> {
    for column in COLUMNS { if !table.has_header(column) { return Err(Malformed(format!("no {column} column"))); } }
    let (time_header, offset) = table.headers.iter().flatten().find_map(|h| {
        let m = TIME_HEADER.captures(h)?;
        let hours: u32 = m.get(2).map_or(Some(0), |x| x.as_str().parse().ok())?;
        let minutes: u32 = m.get(3).map_or(Some(0), |x| x.as_str().parse().ok())?;
        Some((h.clone(), format!("{}{hours:02}:{minutes:02}", m.get(1).map_or("+", |s| s.as_str()))))
    }).ok_or_else(|| Malformed("no time column with a readable zone".into()))?;
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut rows = vec![];
    for row in table.rows.iter().filter(|r| !r.is_empty()) {
        let cell = |name: &str| crate::ruby::strip(table.get(row, name).unwrap_or("")).to_string();
        let (currency, kind) = (cell("Currency"), cell("Type"));
        if currency.is_empty() || kind.is_empty() { return Err(Malformed("a row without its currency or type".into())); }
        let inflow = match cell("Side").as_str() { "Deposit" => true, "Withdrawal" => false, _ => return Err(Malformed("a row whose side is neither Deposit nor Withdrawal".into())) };
        let amount = csv::amount(table.get(row, "Amount"))?;
        let fee = if cell("Fee").is_empty() { BigDec::zero() } else { csv::amount(table.get(row, "Fee"))? };
        if [&amount, &fee].iter().any(|d| !d.is_zero() && !d.is_positive() || !d.to_f().is_finite()) { return Err(Malformed("a negative or unstorable amount".into())); }
        let at = csv::time(table.get(row, &time_header), Some(&offset))?;
        let at = at.with_nanosecond(0).unwrap_or(at);
        let (account_type, remark) = (cell("Account Type"), cell("Remark"));
        let (amount_s, fee_s, at_s) = (amount.to_s_f(), fee.to_s_f(), second(&at));
        let identity = [account_type.as_str(), &currency, side(inflow), &amount_s, &fee_s, &at_s, &kind, &remark];
        let occurrence = seen.entry(identity.join("\0")).or_insert(0);
        *occurrence += 1;
        let row_id = format!("kucoin-csv-row-{}", hex32(&[&identity[..], &[occurrence.to_string().as_str()]].concat()));
        rows.push(SourceRow { row_id, account_type, currency, inflow, amount, fee, at, remark, kind });
    }
    Ok(rows)
}

/// The KuCoin catalogue: each listed pair's price decimals, by (base, quote).
pub struct Catalogue(HashMap<(String, String), u8>);

impl Catalogue {
    pub fn load(c: &rusqlite::Connection, exchange_id: i64) -> Result<Self, SyncError> {
        let pairs = crate::venue::binance::Pairs::load(c, exchange_id).map_err(SyncError)?;
        Ok(Self(pairs.by_symbol.into_values().map(|p| ((p.base, p.quote), p.price_decimals)).collect()))
    }
    pub fn of(pairs: &[(&str, &str, u8)]) -> Self { Self(pairs.iter().map(|(b, q, d)| ((b.to_string(), q.to_string()), *d)).collect()) }
    /// (base, quote, price decimals) of the pair two currencies form, when exactly one of A-B and B-A is listed.
    fn pair(&self, a: &str, b: &str) -> Option<(String, String, u8)> {
        match (self.0.get(&(a.to_string(), b.to_string())), self.0.get(&(b.to_string(), a.to_string()))) {
            (Some(d), None) => Some((a.to_string(), b.to_string(), *d)),
            (None, Some(d)) => Some((b.to_string(), a.to_string(), *d)),
            _ => None,
        }
    }
}

/// A derived ledger entry, and whether it carries a generic label (aggregated, unpaired, unmapped type).
#[derive(Clone, Debug)]
pub struct Derived { pub entry: Entry, pub generic: bool }

/// One entry from `rows` (their ids name it), of `class` (fills, deposits, withdrawals, other).
fn derived(at: DateTime<Utc>, entry_type: i64, currency: &str, amount: BigDec, rows: &[&SourceRow], class: &str, reason: Option<&str>) -> Derived {
    let mut ids: Vec<&str> = rows.iter().map(|r| r.row_id.as_str()).collect();
    ids.sort_unstable();
    let mut raw = json!({ "source": "kucoin_csv", "class": class, "source_rows": ids });
    if let Some(reason) = reason { raw["reason"] = json!(reason); }
    Derived {
        entry: Entry { entry_type, base_currency: Some(currency.to_string()), base_amount: amount, quote_currency: None, quote_amount: None,
                       fee_currency: None, fee_amount: None, tx_id: Some(format!("kucoin-csv-{}", hex32(&ids))), group_id: None, description: None,
                       transacted_at: Some(at), raw: Raw::from_value(raw) },
        generic: matches!(reason, Some("aggregated" | "unpaired" | "unmapped_type")),
    }
}

/// A leg the ledger cannot place: a signed adjustment of its currency.
fn adjustment(r: &SourceRow, class: &str, reason: &str) -> Derived {
    let amount = if r.inflow { r.amount.clone() } else { &BigDec::zero() - &r.amount };
    derived(r.at, ADJUSTMENT, &r.currency, amount, &[r], class, Some(reason))
}

/// The entries of one UTC second's stored rows (all of one second; the order they come in does not matter).
pub fn derive(rows: &[SourceRow], catalogue: &Catalogue) -> Vec<Derived> {
    let mut rows: Vec<&SourceRow> = rows.iter().filter(|r| !r.amount.is_zero()).collect();
    rows.sort_by(|a, b| a.row_id.cmp(&b.row_id));
    let Some(at) = rows.first().map(|r| r.at) else { return vec![] };
    let kind = |r: &SourceRow| TYPES.iter().find(|(t, _)| *t == r.kind).map(|(_, k)| *k);
    let mut out = vec![];
    let (mut spot, mut convert) = (vec![], vec![]);
    for r in rows {
        match kind(r) {
            Some(Kind::Spot) => spot.push(r),
            Some(Kind::Convert) => convert.push(r),
            Some(Kind::Internal) => {}
            Some(Kind::Deposit) if r.inflow => {
                // The row's Amount is the balance change; the entry's own fee is taken from its amount.
                let mut d = derived(at, DEPOSIT, &r.currency, &r.amount + &r.fee, &[r], "deposits", None);
                (d.entry.fee_currency, d.entry.fee_amount) = (Some(r.currency.clone()), Some(r.fee.clone()));
                out.push(d);
            }
            Some(Kind::Withdrawal) if !r.inflow => {
                let mut d = derived(at, WITHDRAWAL, &r.currency, r.amount.clone(), &[r], "withdrawals", None);
                (d.entry.fee_currency, d.entry.fee_amount) = (Some(r.currency.clone()), Some(r.fee.clone()));
                out.push(d);
            }
            Some(Kind::Income(name)) if r.inflow => out.push(derived(at, super::entry_type(name).expect("an entry type"), &r.currency, r.amount.clone(), &[r], "other", None)),
            _ => out.push(adjustment(r, "other", "unmapped_type")),
        }
    }
    out.extend(swaps(at, &convert));
    out.extend(fills(at, &spot, catalogue));
    out
}

/// A Convert's legs in one second: one swap of its two legs, or one swap group of all of them; a lone half is unpaired.
fn swaps(at: DateTime<Utc>, legs: &[&SourceRow]) -> Vec<Derived> {
    let (gone, came): (Vec<&SourceRow>, Vec<&SourceRow>) = legs.iter().partition(|r| !r.inflow);
    if gone.is_empty() || came.is_empty() { return legs.iter().map(|r| adjustment(r, "other", "unpaired")).collect(); }
    let mut ids: Vec<&str> = legs.iter().map(|r| r.row_id.as_str()).collect();
    ids.sort_unstable();
    let group = format!("kucoin-csv-{}", hex32(&ids));
    gone.iter().map(|r| derived(at, SWAP_OUT, &r.currency, r.amount.clone(), &[r], "other", None))
        .chain(came.iter().map(|r| derived(at, SWAP_IN, &r.currency, r.amount.clone(), &[r], "other", None)))
        .map(|mut d| { d.entry.group_id = Some(group.clone()); d }).collect()
}

/// One trade as the API's fill is (sync::kucoin_ledger::fill): `size` of base, `funds` of quote, the quote leg's fee.
struct Fill<'a> { buy: bool, base: String, quote: String, size: BigDec, funds: BigDec, fee: BigDec, rows: Vec<&'a SourceRow>, aggregated: bool }

/// A quote leg's funds: what it moved less its fee for a buy, plus its fee for a sell.
fn funds(quote: &SourceRow, buy: bool) -> BigDec { if buy { &quote.amount - &quote.fee } else { &quote.amount + &quote.fee } }

/// The fill's price, when `funds / size` terminates within the pair's price decimals.
fn price(size: &BigDec, funds: &BigDec, decimals: u8) -> Option<BigDec> {
    let p = funds.div(size)?.round(decimals);
    (p.is_positive() && &p * size == *funds).then_some(p)
}

/// Every price within 1 % of every other.
fn close(prices: &[BigDec]) -> bool {
    let (Some(lo), Some(hi)) = (prices.iter().min(), prices.iter().max()) else { return true };
    *hi <= lo * &BigDec::parse("1.01").expect("a decimal")
}

/// The one assignment of the `few` legs to distinct `many` legs under which every pair has a valid price and all prices
/// are close, as (few index, many index) pairs. A leg is its size on the base side, its funds on the quote side.
/// Assignments that differ only between legs of equal amounts make the same trades and are one.
fn assign(few: &[BigDec], many: &[BigDec], few_is_base: bool, decimals: u8) -> Option<Vec<(usize, usize)>> {
    let valid = |f: usize, m: usize| if few_is_base { price(&few[f], &many[m], decimals) } else { price(&many[m], &few[f], decimals) };
    if few.len() > EXHAUSTIVE {
        // ponytail: past EXHAUSTIVE legs, size order only (validated); an exact search there is factorial.
        if few.len() != many.len() { return None; }
        let order = |legs: &[BigDec]| { let mut i: Vec<usize> = (0..legs.len()).collect(); i.sort_by(|a, b| legs[*a].cmp(&legs[*b])); i };
        let pairs: Vec<(usize, usize)> = order(few).into_iter().zip(order(many)).collect();
        let prices: Option<Vec<BigDec>> = pairs.iter().map(|(f, m)| valid(*f, *m)).collect();
        return prices.filter(|p| close(p)).map(|_| pairs);
    }
    /// An assignment, and the trades it makes (size and funds, sorted).
    type Found = (Vec<(usize, usize)>, Vec<(BigDec, BigDec)>);
    struct Search<'a> { valid: &'a dyn Fn(usize, usize) -> Option<BigDec>, few: &'a [BigDec], many: &'a [BigDec], path: Vec<(usize, usize, BigDec)>,
                        found: Option<Found>, ambiguous: bool }
    impl Search<'_> {
        fn walk(&mut self, f: usize) {
            if self.ambiguous { return; }
            if f == self.few.len() {
                if !close(&self.path.iter().map(|p| p.2.clone()).collect::<Vec<_>>()) { return; }
                let mut trades: Vec<(BigDec, BigDec)> = self.path.iter().map(|(f, m, _)| (self.few[*f].clone(), self.many[*m].clone())).collect();
                trades.sort();
                match &self.found {
                    Some((_, seen)) if *seen != trades => self.ambiguous = true,
                    Some(_) => {}
                    None => self.found = Some((self.path.iter().map(|p| (p.0, p.1)).collect(), trades)),
                }
                return;
            }
            for m in 0..self.many.len() {
                if self.path.iter().any(|p| p.1 == m) { continue; }
                if let Some(p) = (self.valid)(f, m) { self.path.push((f, m, p)); self.walk(f + 1); self.path.pop(); }
            }
        }
    }
    let mut search = Search { valid: &valid, few, many, path: vec![], found: None, ambiguous: false };
    search.walk(0);
    if search.ambiguous { None } else { search.found.map(|(pairs, _)| pairs) }
}

/// The Spot legs of one second: paired into fills by price, per listed pair and direction; what no unique pairing
/// takes is summed into one trade per pair and direction (`aggregated`); a third-currency leg beside the fills is their
/// fee; anything else is a signed adjustment (`unpaired`). Every entry is of class `fills`.
fn fills(at: DateTime<Utc>, legs: &[&SourceRow], catalogue: &Catalogue) -> Vec<Derived> {
    let currencies: BTreeSet<&str> = legs.iter().map(|r| r.currency.as_str()).collect();
    let mut pairs: Vec<(String, String, u8)> = vec![];
    for a in &currencies { for b in &currencies { if a < b { pairs.extend(catalogue.pair(a, b)); } } }
    pairs.sort();
    let mut used = vec![false; legs.len()];
    let mut made: Vec<Fill> = vec![];
    let side_of = |used: &[bool], currency: &str, inflow: bool| -> Vec<usize> {
        (0..legs.len()).filter(|i| !used[*i] && legs[*i].currency == currency && legs[*i].inflow == inflow).collect()
    };
    // Pass 1: each pair and direction paired by price, where exactly one pairing holds.
    for (base, quote, decimals) in &pairs {
        for buy in [true, false] {
            let (b, q) = (side_of(&used, base, buy), side_of(&used, quote, !buy));
            if b.is_empty() || q.is_empty() { continue; }
            let bl: Vec<BigDec> = b.iter().map(|i| legs[*i].amount.clone()).collect();
            let ql: Vec<BigDec> = q.iter().map(|i| funds(legs[*i], buy)).collect();
            let found = if b.len() <= q.len() { assign(&bl, &ql, true, *decimals).map(|p| p.into_iter().map(|(f, m)| (b[f], q[m])).collect::<Vec<_>>()) }
                        else { assign(&ql, &bl, false, *decimals).map(|p| p.into_iter().map(|(f, m)| (b[m], q[f])).collect()) };
            for (bi, qi) in found.unwrap_or_default() {
                used[bi] = true;
                used[qi] = true;
                made.push(Fill { buy, base: base.clone(), quote: quote.clone(), size: legs[bi].amount.clone(), funds: funds(legs[qi], buy), fee: legs[qi].fee.clone(),
                                 rows: vec![legs[bi], legs[qi]], aggregated: false });
            }
        }
    }
    // Pass 2: what is left of a pair and direction, summed into one trade.
    for (base, quote, _) in &pairs {
        for buy in [true, false] {
            let (b, q) = (side_of(&used, base, buy), side_of(&used, quote, !buy));
            if b.is_empty() || q.is_empty() { continue; }
            let total = |i: &[usize], f: &dyn Fn(&SourceRow) -> BigDec| i.iter().fold(BigDec::zero(), |a, x| &a + &f(legs[*x]));
            made.push(Fill { buy, base: base.clone(), quote: quote.clone(), size: total(&b, &|r| r.amount.clone()), funds: total(&q, &|r| funds(r, buy)),
                             fee: total(&q, &|r| r.fee.clone()), rows: b.iter().chain(&q).map(|i| legs[*i]).collect(), aggregated: true });
            for i in b.iter().chain(&q) { used[*i] = true; }
        }
    }
    // Pass 3: a leg leaving in a currency no fill trades is a fee of the fills (KCS): attached to the one fill when it is
    // unique and holds no fee yet, else a Fee entry.
    let traded: BTreeSet<&str> = made.iter().flat_map(|f| [f.base.as_str(), f.quote.as_str()]).collect();
    let fee_legs: Vec<usize> = (0..legs.len()).filter(|i| !used[*i] && !made.is_empty() && !legs[*i].inflow && !traded.contains(legs[*i].currency.as_str())).collect();
    let mut attached: Option<&SourceRow> = None;
    if let ([leg], [fill]) = (fee_legs.as_slice(), made.as_mut_slice()) {
        if fill.fee.is_zero() { attached = Some(legs[*leg]); fill.rows.push(legs[*leg]); used[*leg] = true; }
    }
    let mut out: Vec<Derived> = made.iter().map(|f| {
        let mut d = derived(at, if f.buy { BUY } else { SELL }, &f.base, f.size.clone(), &f.rows, "fills", f.aggregated.then_some("aggregated"));
        (d.entry.quote_currency, d.entry.quote_amount) = (Some(f.quote.clone()), Some(f.funds.clone()));
        match attached {
            Some(fee) => (d.entry.fee_currency, d.entry.fee_amount) = (Some(fee.currency.clone()), Some(fee.amount.clone())),
            None => (d.entry.fee_currency, d.entry.fee_amount) = (Some(f.quote.clone()), Some(f.fee.clone())),
        }
        d
    }).collect();
    for i in fee_legs {
        if used[i] { continue; }
        used[i] = true;
        out.push(derived(at, FEE, &legs[i].currency, legs[i].amount.clone(), &[legs[i]], "fills", None));
    }
    out.extend((0..legs.len()).filter(|i| !used[*i]).map(|i| adjustment(legs[i], "fills", "unpaired")));
    out
}

/// What `store` wrote: entries new to the ledger, rows already held, rows the ledger could not save, and how many of
/// the new entries carry a generic label.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stored { pub imported: usize, pub duplicates: usize, pub skipped: usize, pub generic: usize }

/// The file's rows for `key`'s user and exchange, inside the import's transaction: rows not yet held are stored, and
/// every UTC second that gained one has its `kucoin_csv` entries rebuilt from all its stored rows (incoming links to
/// the entries it replaces are cleared, as Rails' `dependent: :nullify` clears them).
pub(crate) fn store(c: &crate::engine::model::FencedTransaction<'_>, key: &Key, rows: &[SourceRow], now: DateTime<Utc>) -> Result<Stored, SyncError> {
    let mut out = Stored::default();
    let mut touched: BTreeSet<DateTime<Utc>> = BTreeSet::new();
    {
        let mut insert = c.prepare_cached("INSERT INTO import_rows (user_id, exchange_id, row_id, account_type, currency, side, amount, fee, transacted_at, remark, row_type, created_at)
                                           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) ON CONFLICT (user_id, exchange_id, row_id) DO NOTHING")?;
        for r in rows {
            let n = insert.execute(params![key.user_id, key.exchange_id, r.row_id, r.account_type, r.currency, side(r.inflow), r.amount.to_s_f(), r.fee.to_s_f(),
                                           sql_time(r.at), r.remark, r.kind, sql_time(now)])?;
            if n == 1 { touched.insert(r.at); } else { out.duplicates += 1; }
        }
    }
    if touched.is_empty() { return Ok(out); }
    let catalogue = Catalogue::load(c, key.exchange_id)?;
    for at in touched {
        let held = held_rows(c, key, at)?;
        let derived = derive(&held, &catalogue);
        let mut s = c.prepare_cached("SELECT id, tx_id FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at = ?3 AND tx_id LIKE 'kucoin-csv-%'")?;
        let old: Vec<(i64, String)> = s.query_map(params![key.user_id, key.exchange_id, sql_time(at)], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
        for (id, _) in &old {
            c.execute("UPDATE account_transactions SET linked_transaction_id = NULL WHERE linked_transaction_id = ?1", [id])?;
            c.execute("DELETE FROM account_transactions WHERE id = ?1", [id])?;
        }
        // Entries the second held before go back as they were; only the others are new.
        let (again, fresh): (Vec<Derived>, Vec<Derived>) = derived.into_iter().partition(|d| old.iter().any(|(_, t)| Some(t) == d.entry.tx_id.as_ref()));
        ledger::store_file(c, key, again.into_iter().map(|d| d.entry).collect(), now)?;
        let generic: Vec<String> = fresh.iter().filter(|d| d.generic).filter_map(|d| d.entry.tx_id.clone()).collect();
        let stored = ledger::store_file(c, key, fresh.into_iter().map(|d| d.entry).collect(), now)?;
        (out.imported, out.duplicates, out.skipped) = (out.imported + stored.imported, out.duplicates + stored.duplicates, out.skipped + stored.skipped);
        for tx_id in generic {
            let kept: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2 AND tx_id = ?3)",
                                         params![key.user_id, key.exchange_id, tx_id], |r| r.get(0))?;
            out.generic += usize::from(kept);
        }
    }
    Ok(out)
}

/// Every stored row of (user, exchange) at one UTC second.
fn held_rows(c: &rusqlite::Connection, key: &Key, at: DateTime<Utc>) -> Result<Vec<SourceRow>, SyncError> {
    let mut s = c.prepare_cached("SELECT row_id, account_type, currency, side, amount, fee, remark, row_type FROM import_rows
                                  WHERE user_id = ?1 AND exchange_id = ?2 AND transacted_at = ?3 ORDER BY row_id")?;
    let rows = s.query_map(params![key.user_id, key.exchange_id, sql_time(at)], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?, r.get::<_, String>(4)?, r.get::<_, String>(5)?,
            r.get::<_, String>(6)?, r.get::<_, String>(7)?))
    })?.collect::<Result<Vec<_>, _>>()?;
    rows.into_iter().map(|(row_id, account_type, currency, side, amount, fee, remark, kind)| {
        let number = |s: &str| BigDec::parse(s).map_err(|e| SyncError(format!("import_rows {row_id}: {e:?}")));
        Ok(SourceRow { amount: number(&amount)?, fee: number(&fee)?, inflow: side == "Deposit", row_id, account_type, currency, at, remark, kind })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trading-account export's rows (numbers as exported; the UID a constant), grouped by UTC second.
    fn seconds() -> std::collections::BTreeMap<DateTime<Utc>, Vec<SourceRow>> {
        let text = csv::as_text(include_bytes!("../../../tests/fixtures/kucoin/trading_account.csv"));
        let mut by: std::collections::BTreeMap<DateTime<Utc>, Vec<SourceRow>> = Default::default();
        for r in read(&Table::parse(&text).unwrap()).unwrap() { by.entry(r.at).or_default().push(r); }
        by
    }
    fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
    fn d(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
    fn eth_btc() -> Catalogue { Catalogue::of(&[("ETH", "BTC", 6)]) }

    /// What the API's fill of the same trade is (sync::kucoin_ledger::fill), field by field.
    fn api(side: &str, size: &str, funds: &str, fee: &str, at: DateTime<Utc>) -> Entry {
        crate::sync::kucoin_ledger::fill(Raw::from_value(json!({ "symbol": "ETH-BTC", "tradeId": "1", "side": side, "size": size, "funds": funds, "fee": fee,
                                                               "feeCurrency": "BTC", "createdAt": at.timestamp_millis() }))).unwrap()
    }
    fn same_trade(file: &Entry, api: &Entry) {
        let fields = |e: &Entry| (e.entry_type, e.base_currency.clone(), e.base_amount.clone(), e.quote_currency.clone(), e.quote_amount.clone(),
                                  e.fee_currency.clone(), e.fee_amount.clone(), e.transacted_at);
        assert_eq!(fields(file), fields(api));
    }

    #[test]
    fn the_first_buys_and_the_sell_are_the_apis_fills() {
        let seconds = seconds();
        for second in ["2022-09-28T05:20:58Z", "2022-09-28T05:21:06Z"] {
            let out = derive(&seconds[&at(second)], &eth_btc());
            assert_eq!(out.len(), 1);
            same_trade(&out[0].entry, &api("buy", "0.0001459", "0.000009995609", "0.000000009995609", at(second)));
            assert!(!out[0].generic);
        }
        let sell = derive(&seconds[&at("2022-09-28T10:13:02Z")], &eth_btc());
        assert_eq!(sell.len(), 1);
        same_trade(&sell[0].entry, &api("sell", "0.0203034", "0.0013858288704", "0.0000013858288704", at("2022-09-28T10:13:02Z")));
    }

    #[test]
    fn the_four_row_second_is_two_buys_at_their_own_prices() {
        let out = derive(&seconds()[&at("2022-09-28T05:34:27Z")], &eth_btc());
        let mut prices: Vec<BigDec> = out.iter().map(|o| o.entry.quote_amount.as_ref().unwrap().div(&o.entry.base_amount).unwrap()).collect();
        prices.sort();
        assert_eq!(prices, [d("0.068426"), d("0.068427")]);
        assert!(out.iter().all(|o| o.entry.entry_type == BUY && !o.generic));
    }

    #[test]
    fn the_whole_export_pairs_every_spot_leg_and_the_transfer_derives_nothing() {
        let all: Vec<Derived> = seconds().values().flat_map(|rows| derive(rows, &eth_btc())).collect();
        assert_eq!(all.len(), 163, "one trade per Spot pair");
        assert!(all.iter().all(|o| !o.generic && [BUY, SELL].contains(&o.entry.entry_type)));
    }

    #[test]
    fn a_price_must_terminate_within_the_pairs_decimals() {
        let d = |s: &str| BigDec::parse(s).unwrap();
        assert_eq!(price(&d("0.0001459"), &d("0.000009995609"), 6), Some(d("0.06851")));
        assert_eq!(price(&d("0.000146"), &d("0.0000000068426"), 6), None, "the other assignment of the 4-row second");
        assert!(close(&[d("0.068426"), d("0.068427")]) && !close(&[d("1"), d("1.02")]));
    }
}
