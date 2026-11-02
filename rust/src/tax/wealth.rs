//! Units held at an instant for the wealth reports (NL, CH), with no row priced (Rails prices none):
//! - D-TAX-WEALTH-HOLDINGS: each row moves what the ledger says it moves (`walk::quantity_moves`: an own- or third-coin fee, a
//!   linked transfer's network fee, both legs of a coin-quoted trade once split), where Rails' walk moved the base only; a
//!   stablecoin's own fee on its sale comes off on top as any coin's does (the tracker reads a stablecoin as cash there).
//! - D-TAX-SNAPSHOT-KEEPS: coins that left by a withdrawal linked to no deposit are still the user's (R4's self-transfer
//!   assumption): counted, less the withdrawal's own fee (listed by the report).
//! - D-TAX-WEALTH-LOST: a `lost` row leaves the holdings (Rails never removed it).
//! - Holdings by asset identity (D-TAX-ASSET-IDENTITY), the symbol of the first row shown.
use super::{cents, line, n, s, Cell, Outcome, Report};
use crate::figures::{at::At, dec::Dec, FiguresError};
use crate::tracker::prices::{self, Fetcher, PriceService};
use crate::tracker::rows::{Kind, Stored};
use crate::tracker::{fiat, stable, walk};
use crate::web::i18n::{text, Arg};
use chrono::NaiveDate;
use std::collections::{HashMap, HashSet};

/// One coin held: its identity, symbol and venue (of its first row), and the units.
#[derive(Debug, Clone, PartialEq)]
pub struct Held { pub asset: String, pub symbol: String, pub venue: String, pub units: Dec }

/// The units held of every coin just before `until`, from `rows` (every row the report read, in order); `deposits` the amount
/// of every row by id (a linked withdrawal's network fee is its amount less its deposit's); `identity(symbol, row)` the coin a
/// symbol means on that row. Coins with no unit left (or fewer than none: an oversold coin) are not held.
pub fn held(rows: &[Stored], deposits: &HashMap<i64, Dec>, until: At, identity: &dyn Fn(&str, &Stored) -> String) -> Result<Vec<Held>, FiguresError> {
    let linked_deposits: HashSet<i64> = rows.iter().filter_map(|r| r.linked_to).collect();
    let mut out: Vec<Held> = vec![];
    for r in rows.iter().filter(|r| r.at.0 < until.0) {
        let linked = r.linked_to.is_some() || linked_deposits.contains(&r.id);
        let transfer_fee = match (r.kind, r.linked_to) {
            (Kind::Withdrawal, Some(d)) => deposits.get(&d).map(|deposit| &r.amount - deposit).transpose()?,
            _ => None,
        };
        let own_fee = r.fee_amount.clone().filter(|f| f.is_positive() && r.fee_currency.as_deref() == Some(r.base.as_str()));
        let mut moves = walk::quantity_moves(r.kind, &r.base, &r.amount, r.fee_currency.as_deref(), r.fee_amount.as_ref(), linked, transfer_fee.as_ref())?;
        // r4 item 2: an own-coin fee attached to a linked deposit leaves too (`quantity_moves` takes a third coin's already; the
        // principal is not credited again, and the inferred network fee is the withdrawal's amount less the deposit's gross).
        if r.kind == Kind::Deposit && linked { if let Some(f) = &own_fee { moves.push((r.base.clone(), f.neg())); } }
        for (symbol, delta) in moves.iter_mut().filter(|(s, _)| *s == r.base) {
            // D-TAX-SNAPSHOT-KEEPS: an unlinked withdrawal takes out only its own fee.
            if r.kind == Kind::Withdrawal && !linked { *delta = own_fee.clone().map_or_else(Dec::zero, |f| f.neg()); }
            // A stablecoin is a coin here: its own fee on a sale leaves on top of the amount sold.
            else if matches!(r.kind, Kind::Sell | Kind::SwapOut) && stable(symbol) { if let Some(f) = &own_fee { *delta = (&*delta - f)?; } }
        }
        for (symbol, delta) in moves {
            if fiat(&symbol) || delta.is_zero() { continue; }
            let asset = identity(&symbol, r);
            match out.iter_mut().find(|h| h.asset == asset) {
                Some(h) => h.units = (&h.units + &delta)?,
                None => out.push(Held { asset, symbol, venue: r.venue.clone(), units: delta }),
            }
        }
    }
    out.retain(|h| h.units.is_positive());
    Ok(out)
}

// ---- the wealth report (NL, CH): the shared steps; each country's figures are nl.rs's and ch.rs's ----------------------

/// What a country's wealth figures give the file: the holdings valued on `day`, its summary (and income section) as CSV
/// lines, its NOTE blocks, and the instant up to which unlinked withdrawals and lost coins are listed.
pub(super) struct Wealth { pub day: NaiveDate, pub lines: Vec<(Held, Dec)>, pub summary: String, pub income: String, pub notes: String, pub until: At }

impl Report {
    pub fn is_wealth(&self) -> bool { self.j.method == "wealth_snapshot" }

    /// The coin `symbol` means on `venue` on `day` (D-TAX-ASSET-IDENTITY), or the symbol itself where none is known.
    pub(super) fn coin(&self, symbol: &str, venue: &str, day: NaiveDate) -> String {
        prices::coin_id(&self.reference, symbol, self.reference.venue(venue), Some(day)).unwrap_or_else(|| format!("symbol:{symbol}"))
    }

    /// `held` over the report's rows, each symbol read as the coin it meant on its row's day.
    pub(super) fn held_at(&self, until: At) -> Result<Vec<Held>, FiguresError> {
        held(&self.scoped, &self.deposits, until, &|symbol, r| self.coin(symbol, &r.venue, r.at.utc().date_naive()))
    }

    /// Whether a holding's ticker names it, and only it, on `day` (r3 item 2, r4 item 1): its price or ESTV value may be read.
    pub(super) fn priceable(&self, h: &Held, all: &[Held], day: NaiveDate) -> bool {
        !all.iter().any(|o| o.symbol == h.symbol && o.asset != h.asset) && self.coin(&h.symbol, &h.venue, day) == h.asset
    }

    /// The price of one unit of a holding dated `day` (r3 item 2): only when its symbol that day still means its coin, and
    /// (r4 item 1) no other coin of `all` is held under the same ticker (prices are stored by ticker); else none, listed with
    /// the price warnings. A zero price is none (`price_at` warned).
    pub(super) fn unit_price(&self, ps: &mut PriceService, h: &Held, all: &[Held], currency: &str, day: NaiveDate) -> Result<Option<Dec>, FiguresError> {
        if !self.priceable(h, all, day) {
            ps.warnings.push(format!("{}/{currency} {day}", h.symbol));
            return Ok(None);
        }
        let at = At::from_utc(day.and_time(chrono::NaiveTime::MIN).and_utc()).ok_or_else(|| FiguresError::Data("a day out of range".into()))?;
        let price = ps.price_at(&h.symbol, currency, at, Some(&h.venue))?;
        Ok(Some(price).filter(|p| !p.is_zero()))
    }

    /// D-TAX-NL-ACTUAL, D-TAX-CH-KS36: the fiat paid for coins and received for them in the report's year, gross of fees,
    /// in `currency` at the day's rate: a coin row's own fiat quote, or the fiat leg of a trade booked as a group (Kraken).
    /// A stablecoin is a coin.
    pub(super) fn fiat_trades(&self, ps: &mut PriceService, currency: &str) -> Result<(Dec, Dec), FiguresError> {
        // A trade's legs: bought and sold rows, and swap legs (a fiat-funded convert books its fiat as a swap leg).
        let leg = |k: Kind| matches!(k, Kind::Buy | Kind::Sell | Kind::SwapIn | Kind::SwapOut);
        let traded: HashSet<(i64, &str)> = self.scoped.iter().filter(|r| !fiat(&r.base) && leg(r.kind))
            .filter_map(|r| r.group.as_deref().map(|g| (r.exchange_id, g))).collect();
        let (mut paid, mut received) = (Dec::zero(), Dec::zero());
        for r in self.scoped.iter().filter(|r| self.in_year(r.at) && leg(r.kind)) {
            let (cash, amount, pays) = match (&r.group, fiat(&r.base)) {
                (Some(g), true) if traded.contains(&(r.exchange_id, g.as_str())) => (r.base.as_str(), &r.amount, r.kind.out_leg()),
                (None, false) => match (r.quote.as_deref(), &r.quote_amount) { (Some(q), Some(a)) if fiat(q) => (q, a, r.kind.in_leg()), _ => continue },
                _ => continue,
            };
            let value = ps.convert_fiat(amount, cash, currency, r.at)?;
            if pays { paid = (&paid + &value)?; } else { received = (&received + &value)?; }
        }
        Ok((paid, received))
    }

    /// Tax::Report#to_csv for a wealth snapshot: its rows by value, its summary, then Rust's blocks.
    pub(super) fn wealth_report(&self, fetcher: &mut dyn Fetcher, today: NaiveDate, on_progress: &mut dyn FnMut(u32)) -> Result<Outcome, FiguresError> {
        on_progress(50);
        let currency = self.j.currency(self.year);
        let mut ps = PriceService::new(&self.reference, &self.fx, today, fetcher);
        let mut w = if self.j.code == "CH" { self.ch(&mut ps, today)? } else { self.nl(&mut ps, today)? };
        let warnings = { let mut seen = HashSet::new(); ps.warnings.iter().filter(|w| seen.insert(w.as_str())).cloned().collect::<Vec<_>>() };
        drop(ps);
        on_progress(100);
        let l = self.j.locale;
        let mut out = String::new();
        line(&mut out, &["reference_date", "asset", "amount", "value", "currency"].map(|k| self.t(&format!("tax_report.headers.{k}"))));
        for b in &self.banners { line(&mut out, &[s(b.clone())]); }
        if !warnings.is_empty() {
            let banner = text(l, "tax_report.incomplete_banner", &[("missing", Arg::Text(&warnings.len().to_string()))]);
            line(&mut out, &[s(format!("{}: {banner}", text(l, "tax_report.incomplete_banner_prefix", &[])))]);
        }
        // Rails' order: by value, the highest first.
        w.lines.sort_by(|a, b| b.1.to_f().total_cmp(&a.1.to_f()));
        for (h, value) in &w.lines { line(&mut out, &[s(w.day.to_string()), s(h.symbol.clone()), n(&h.units), cents(value)?, s(currency)]); }
        out.push_str(&w.summary);
        out.push_str(&w.income);
        if !warnings.is_empty() {
            line(&mut out, &[]);
            line(&mut out, &[s(format!("WARNING: {}:", text(l, "tax_report.warnings.missing_prices", &[])))]);
            for x in &warnings { line(&mut out, &[s(x.clone())]); }
            line(&mut out, &[self.t("tax_report.warnings.upgrade_hint")]);
        }
        let before = |kind: Kind| self.scoped.iter().filter(move |r| r.kind == kind && r.at.0 < w.until.0 && !fiat(&r.base));
        let linked: HashSet<i64> = self.scoped.iter().filter_map(|r| r.linked_to).collect();
        let listed = |r: &Stored| format!("{} {} {}", r.base, r.amount.to_s_f(), r.at.utc().format("%Y-%m-%d"));
        // D-TAX-SNAPSHOT-KEEPS: still counted as the user's; D-TAX-WEALTH-LOST: no longer counted.
        self.block(&mut out, "WARNING", "unlinked_withdrawals", "unlinked_withdrawals_hint",
                   before(Kind::Withdrawal).filter(|r| r.linked_to.is_none() && !linked.contains(&r.id)).map(listed))?;
        self.block(&mut out, "WARNING", "lost", "lost_hint", before(Kind::Lost).map(listed))?;
        out.push_str(&w.notes);
        Ok(Outcome::Csv(out))
    }
}

/// A summary line as Rails writes one: `[nil, nil, label, value, nil]`.
pub(super) fn summary(out: &mut String, label: String, value: Option<Cell>) { line(out, &[Cell::Nil, Cell::Nil, s(label), value.unwrap_or(Cell::Nil), Cell::Nil]); }

/// A decimal with exactly `places` decimals.
pub(super) fn fixed(v: &Dec, places: i64) -> Result<String, FiguresError> {
    let t = v.round(places)?.to_s_f();
    let (whole, frac) = t.split_once('.').unwrap_or((&t, ""));
    if places <= 0 { return Ok(whole.to_string()); }
    Ok(format!("{whole}.{frac:0<width$}", width = usize::try_from(places).unwrap_or(2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(v: &str) -> Dec { Dec::strict(v).unwrap() }
    fn row(id: i64, kind: Kind, base: &str, amount: &str, day: i64) -> Stored {
        Stored { id, exchange_id: 1, venue: "kraken".into(), kind, base: base.into(), amount: d(amount), quote: None, quote_amount: None, fee_currency: None,
                 fee_amount: None, tx_id: None, group: None, at: At(day * 86_400_000_000_000), per_share: None, stated: None, linked_to: None, to_venue: None,
                 to_exchange_id: None, base_asset: None }
    }
    fn fee(mut r: Stored, coin: &str, amount: &str) -> Stored { (r.fee_currency, r.fee_amount) = (Some(coin.into()), Some(d(amount))); r }
    fn units(rows: &[Stored], until: i64) -> Vec<(String, String)> {
        let deposits = rows.iter().map(|r| (r.id, r.amount.clone())).collect();
        held(rows, &deposits, At(until * 86_400_000_000_000), &|s, _| s.to_string()).unwrap().into_iter().map(|h| (h.symbol, h.units.to_s_f())).collect()
    }

    /// r1 item 7: an unlinked withdrawal of 0.1 with a 0.001 own fee leaves 0.999; a USDT sale of 100 with a 1 USDT fee leaves 899;
    /// a lost coin leaves; a third-coin fee comes off that coin; an oversold coin is not held.
    #[test]
    fn units_at_an_instant() {
        let rows = [row(1, Kind::Buy, "BTC", "1", 0), fee(row(2, Kind::Withdrawal, "BTC", "0.1", 1), "BTC", "0.001"),
                    row(3, Kind::Buy, "USDT", "1000", 1), fee(row(4, Kind::Sell, "USDT", "100", 2), "USDT", "1"),
                    row(5, Kind::Buy, "ETH", "2", 2), row(6, Kind::Lost, "ETH", "0.5", 3), fee(row(7, Kind::Buy, "SOL", "10", 3), "ETH", "0.1"),
                    row(8, Kind::Sell, "ADA", "5", 3), row(9, Kind::Buy, "DOT", "1", 9)];
        assert_eq!(units(&rows, 5), [("BTC".to_string(), "0.999".to_string()), ("USDT".into(), "899.0".into()), ("ETH".into(), "1.4".into()), ("SOL".into(), "10.0".into())]);
    }

    /// A linked transfer moves nothing but its network fee, and a link whose deposit lands after the instant still counts.
    #[test]
    fn a_linked_transfer_takes_out_its_network_fee_only() {
        let mut out = row(2, Kind::Withdrawal, "BTC", "1", 3);
        out.linked_to = Some(3);
        let rows = [row(1, Kind::Buy, "BTC", "1", 0), out, row(3, Kind::Deposit, "BTC", "0.9995", 6)];
        assert_eq!(units(&rows, 5), [("BTC".to_string(), "0.9995".to_string())]);
        assert_eq!(units(&rows, 9), [("BTC".to_string(), "0.9995".to_string())]);
        // Codex r4: buy 1, withdraw 1, linked deposit gross 1 with a 0.001 BTC fee → 0.999.
        let mut out = row(2, Kind::Withdrawal, "BTC", "1", 3);
        out.linked_to = Some(3);
        let rows = [row(1, Kind::Buy, "BTC", "1", 0), out, fee(row(3, Kind::Deposit, "BTC", "1", 4), "BTC", "0.001")];
        assert_eq!(units(&rows, 9), [("BTC".to_string(), "0.999".to_string())]);
    }
}
