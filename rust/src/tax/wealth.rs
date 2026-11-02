//! Units held at an instant for the wealth reports (NL, CH), with no row priced (Rails prices none):
//! - D-TAX-WEALTH-HOLDINGS: each row moves what the ledger says it moves (`walk::quantity_moves`: an own- or third-coin fee, a
//!   linked transfer's network fee, both legs of a coin-quoted trade once split), where Rails' walk moved the base only; a
//!   stablecoin's own fee on its sale comes off on top as any coin's does (the tracker reads a stablecoin as cash there).
//! - D-TAX-SNAPSHOT-KEEPS: coins that left by a withdrawal linked to no deposit are still the user's (R4's self-transfer
//!   assumption): counted, less the withdrawal's own fee (listed by the report).
//! - D-TAX-WEALTH-LOST: a `lost` row leaves the holdings (Rails never removed it).
//! - Holdings by asset identity (D-TAX-ASSET-IDENTITY), the symbol of the first row shown.
use crate::figures::{at::At, dec::Dec, FiguresError};
use crate::tracker::rows::{Kind, Stored};
use crate::tracker::{fiat, stable, walk};
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
    }
}
