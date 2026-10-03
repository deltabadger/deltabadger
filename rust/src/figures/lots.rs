//! Bot::TaxLots: per-asset FIFO lots over the bot's own fills, the tax-shaped view of a position. A lot's cost can
//! be unknown (a fill with neither a reported cost nor a usable order price); unknown never becomes zero.
use super::dec::Dec;
use super::num::{Num, NumError};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq)]
pub struct Lot { pub amount: Dec, pub cost: Option<Dec> }

/// A holding's lots, oldest first. A deque: a sale takes lots off the front, one at a time, as Ruby's `shift` does.
pub type Lots = VecDeque<Lot>;

/// `lots.sum { |lot| lot[:cost] || 0.to_d }`: Ruby's Integer zero for no lots, else a BigDecimal.
pub fn basis(lots: &Lots) -> Result<Num, NumError> {
    if lots.is_empty() { return Ok(Num::Int(0)); }
    let zero = Dec::zero();
    Ok(Num::Dec(lots.iter().try_fold(Dec::zero(), |sum, lot| &sum + lot.cost.as_ref().unwrap_or(&zero))?))
}

/// `lots.sum { |lot| lot[:amount] }`.
pub fn units(lots: &Lots) -> Result<Num, NumError> {
    if lots.is_empty() { return Ok(Num::Int(0)); }
    Ok(Num::Dec(lots.iter().try_fold(Dec::zero(), |sum, lot| &sum + &lot.amount)?))
}

pub fn unknown_cost(lots: &Lots) -> bool { lots.iter().any(|lot| lot.cost.is_none()) }

/// Cost of the first `amount` units, FIFO, without consuming them. Units beyond the lots and units of unknown cost
/// carry none.
pub fn cost_of(lots: &Lots, amount: &Dec) -> Result<Dec, NumError> {
    let (mut remaining, mut cost, zero) = (amount.clone(), Dec::zero(), Dec::zero());
    for lot in lots {
        if !remaining.is_positive() { break; }
        let take = if remaining < lot.amount { remaining.clone() } else { lot.amount.clone() };
        cost = (&cost + &(lot.cost.as_ref().unwrap_or(&zero) * &take.div(&lot.amount)?)?)?;
        remaining = (&remaining - &take)?;
    }
    Ok(cost)
}

/// Whether selling the first `amount` units for `proceeds` loses on any lot consumed, at the sale's average price.
/// `None` is unknown: a consumed lot's cost is unknown and no other consumed lot already shows a loss.
pub fn loss_in(lots: &Lots, amount: &Dec, proceeds: &Dec) -> Result<Option<bool>, NumError> {
    if !amount.is_positive() { return Ok(Some(false)); }
    let (mut remaining, mut unknown) = (amount.clone(), false);
    for lot in lots {
        if !remaining.is_positive() { break; }
        match &lot.cost {
            None => unknown = true,
            Some(cost) if (proceeds * &lot.amount)? < (cost * amount)? => return Ok(Some(true)),
            Some(_) => {}
        }
        let take = if lot.amount < remaining { lot.amount.clone() } else { remaining.clone() };
        remaining = (&remaining - &take)?;
    }
    Ok(if unknown { None } else { Some(false) })
}

pub fn consume(lots: &mut Lots, amount: &Dec) -> Result<(), NumError> {
    let mut remaining = amount.clone();
    while remaining.is_positive() {
        let Some(lot) = lots.front_mut() else { break };
        if lot.amount <= remaining {
            remaining = (&remaining - &lot.amount)?;
            lots.pop_front();
        } else {
            if let Some(cost) = &lot.cost { lot.cost = Some((cost - &(cost * &remaining.div(&lot.amount)?)?)?); }
            lot.amount = (&lot.amount - &remaining)?;
            remaining = Dec::zero();
        }
    }
    Ok(())
}

pub fn split(lots: &mut Lots, factor: &Dec) -> Result<(), NumError> {
    for lot in lots.iter_mut() { lot.amount = (&lot.amount * factor)?; }
    Ok(())
}
