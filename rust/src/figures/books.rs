//! Bot::RebalanceAccounting: how one fill moves a composition bot's books. The ledger is what each holding
//! amounts to and cost; beside it sit the sums no holding owns: cost basis and cash in flight between the two legs
//! of a swap, what came in from outside, liquidation proceeds waiting for a redeploy, proceeds of scheduled sales,
//! estimates for sales the venue did not price, and realised profit.
//! Every sum starts as Ruby's Integer zero and becomes a BigDecimal with the first fill that touches it.
use super::num::{Num, NumError};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub amount: Num, pub invested: Num }

/// `{ key => { amount:, invested: } }`, in the order the keys were first touched, as a Ruby Hash keeps them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ledger(pub Vec<(String, Entry)>);

impl Ledger {
    pub fn get(&self, key: &str) -> Option<&Entry> { self.0.iter().find(|(k, _)| k == key).map(|(_, e)| e) }
    /// `ledger[key]`: the Hash's default proc makes a zero entry for a key it has not seen, and that entry stays.
    pub fn entry(&mut self, key: &str) -> &mut Entry {
        let at = match self.0.iter().position(|(k, _)| k == key) {
            Some(at) => at,
            None => { self.0.push((key.to_string(), Entry { amount: Num::Int(0), invested: Num::Int(0) })); self.0.len() - 1 }
        };
        &mut self.0[at].1
    }
    /// Measurable#basis_share: the cost the ledger carries for `amount` of `key`, read without releasing it (and without
    /// touching the key).
    pub fn basis_share(&self, key: &str, amount: &super::dec::Dec) -> Result<Num, NumError> {
        let Some(entry) = self.get(key).filter(|entry| entry.amount.is_positive()) else { return Ok(Num::Dec(super::dec::Dec::zero())) };
        let held = entry.amount.to_d()?;
        Num::Dec(entry.invested.to_d()?).mul(&Num::min2(Num::Dec(amount.div(&held)?), Num::Int(1))?)
    }
    /// `ledger.sum { |key, entry| entry[:amount] * (prices[key] || 0) }`.
    pub fn value(&self, prices: &[(String, Num)]) -> Result<Num, NumError> {
        let mut sum = Num::Int(0);
        for (key, entry) in &self.0 {
            let price = prices.iter().find(|(k, _)| k == key).map_or(Num::Int(0), |(_, p)| p.clone());
            sum = sum.add(&entry.amount.mul(&price)?)?;
        }
        Ok(sum)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Books {
    pub basis: Num, pub cash: Num, pub contributed: Num, pub realised_cash: Num, pub realised_pnl: Num,
    pub estimated_cash: Num, pub divested: Num,
}

impl Default for Books {
    fn default() -> Self {
        let zero = || Num::Int(0);
        Self { basis: zero(), cash: zero(), contributed: zero(), realised_cash: zero(), realised_pnl: zero(), estimated_cash: zero(), divested: zero() }
    }
}

/// transactions.side and transactions.transaction_type, as #apply_fill routes them. A row with no side is a buy,
/// and a type this code does not know is a REGULAR buy or a REBALANCE sell, as Ruby's `else` branches read them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill { RebalanceSell, LiquidationSell, RegularSell, RebalanceBuy, RedeployBuy, RegularBuy }

impl Fill {
    pub fn of(sell: bool, transaction_type: &str) -> Fill {
        match (sell, transaction_type) {
            (true, "LIQUIDATION") => Fill::LiquidationSell,
            (true, "REGULAR") => Fill::RegularSell,
            (true, _) => Fill::RebalanceSell,
            (false, "REBALANCE") => Fill::RebalanceBuy,
            (false, "REDEPLOY") => Fill::RedeployBuy,
            (false, _) => Fill::RegularBuy,
        }
    }
}

impl Books {
    /// Money the bot holds and has not deployed: `cash + realised_cash + estimated_cash + divested`.
    pub fn uninvested_cash(&self) -> Result<Num, NumError> {
        self.cash.add(&self.realised_cash)?.add(&self.estimated_cash)?.add(&self.divested)
    }

    /// Takes the sold fraction's cost off the holding and hands it back. Base the bot never bought carries no cost
    /// of ours: it is valued at what it fetched, and counted as money that came in.
    fn release_basis(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<Num, NumError> {
        let entry = ledger.entry(key);
        let held = entry.amount.clone();
        let released = if held.is_positive() {
            let sold_fraction = Num::min2(amount.div(&held)?, Num::Int(1))?;
            let released = entry.invested.mul(&sold_fraction)?;
            entry.invested = entry.invested.sub(&released)?;
            released
        } else {
            self.contributed = self.contributed.add(quote)?;
            quote.clone()
        };
        entry.amount = Num::max2(held.sub(amount)?, Num::Int(0))?;
        Ok(released)
    }

    /// A swap's sell leg: the cost and the cash go in flight until the buy leg lands.
    pub fn rebalance_sell(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        let released = self.release_basis(ledger, key, amount, quote)?;
        self.basis = self.basis.add(&released)?;
        self.cash = self.cash.add(quote)?;
        Ok(())
    }

    /// A liquidation the venue executed and did not price: the units leave, and their cost is parked as an estimate.
    pub fn unpriced_sell(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        self.release_basis(ledger, key, amount, quote)?;
        self.estimated_cash = self.estimated_cash.add(quote)?;
        Ok(())
    }

    pub fn liquidation_sell(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        let released = self.release_basis(ledger, key, amount, quote)?;
        self.realised_cash = self.realised_cash.add(quote)?;
        self.realised_pnl = self.realised_pnl.add(&quote.sub(&released)?)?;
        Ok(())
    }

    /// A scheduled sale. Base sold beyond what the ledger holds is valued at its own sale price.
    pub fn regular_sell(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        let owned = Num::min2(amount.clone(), ledger.entry(key).amount.clone())?;
        let excess_proceeds = quote.mul(&amount.sub(&owned)?)?.div(amount)?;
        let released = if owned.is_positive() { self.release_basis(ledger, key, &owned, quote)? } else { Num::Int(0) };
        self.contributed = self.contributed.add(&excess_proceeds)?;
        self.divested = self.divested.add(quote)?;
        self.realised_pnl = self.realised_pnl.add(&quote.sub(&released)?.sub(&excess_proceeds)?)?;
        Ok(())
    }

    /// A swap's buy leg: cost moves in proportion to the cash this buy consumes.
    pub fn rebalance_buy(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        let entry = ledger.entry(key);
        let cash = Num::Dec(self.cash.to_d()?);
        let share = if cash.is_positive() { Num::min2(quote.div(&cash)?, Num::Int(1))? } else { Num::Int(1) };
        let moved = self.basis.mul(&share)?;
        entry.invested = entry.invested.add(&moved)?;
        self.basis = self.basis.sub(&moved)?;
        self.cash = Num::max2(cash.sub(quote)?, Num::Int(0))?;
        entry.amount = entry.amount.add(amount)?;
        Ok(())
    }

    /// A scheduled contribution: new money, except for what a half-finished swap left in flight, which it drains.
    pub fn regular_buy(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        let entry = ledger.entry(key);
        let cash = Num::Dec(self.cash.to_d()?);
        let zero = || Num::Dec(super::dec::Dec::zero());
        let (from_flight, moved_basis) = if cash.is_positive() {
            let from_flight = Num::min2(cash.clone(), quote.clone())?;
            let moved_basis = self.basis.mul(&from_flight.div(&cash)?)?;
            self.basis = self.basis.sub(&moved_basis)?;
            self.cash = self.cash.sub(&from_flight)?;
            (from_flight, moved_basis)
        } else {
            (zero(), zero())
        };
        let new_money = quote.sub(&from_flight)?;
        self.contributed = self.contributed.add(&new_money)?;
        entry.invested = entry.invested.add(&moved_basis.add(&new_money)?)?;
        entry.amount = entry.amount.add(amount)?;
        Ok(())
    }

    /// Liquidation proceeds spent back into the composition; what overshoots them is a contribution.
    pub fn redeploy_buy(&mut self, ledger: &mut Ledger, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        let entry = ledger.entry(key);
        let from_realised = Num::min2(self.realised_cash.clone(), quote.clone())?;
        self.realised_cash = self.realised_cash.sub(&from_realised)?;
        let new_money = quote.sub(&from_realised)?;
        self.contributed = self.contributed.add(&new_money)?;
        entry.invested = entry.invested.add(&from_realised.add(&new_money)?)?;
        entry.amount = entry.amount.add(amount)?;
        Ok(())
    }

    /// The walk's unpriced-sell branch for a LIQUIDATION (measurable.rb:136-148): the units leave and the basis they carried
    /// is parked as an estimate, never realised_cash. Returns it, for estimated_proceeds. (An unpriced REBALANCE sell is
    /// valued by fill::special_sell, R4.)
    pub fn unpriced_liquidation(&mut self, ledger: &mut Ledger, key: &str, amount: &super::dec::Dec) -> Result<Num, NumError> {
        let released = ledger.basis_share(key, amount)?;
        self.unpriced_sell(ledger, key, &Num::Dec(amount.clone()), &released)?;
        Ok(released)
    }

    /// #apply_fill.
    pub fn apply(&mut self, ledger: &mut Ledger, fill: Fill, key: &str, amount: &Num, quote: &Num) -> Result<(), NumError> {
        match fill {
            Fill::RebalanceSell => self.rebalance_sell(ledger, key, amount, quote),
            Fill::LiquidationSell => self.liquidation_sell(ledger, key, amount, quote),
            Fill::RegularSell => self.regular_sell(ledger, key, amount, quote),
            Fill::RebalanceBuy => self.rebalance_buy(ledger, key, amount, quote),
            Fill::RedeployBuy => self.redeploy_buy(ledger, key, amount, quote),
            Fill::RegularBuy => self.regular_buy(ledger, key, amount, quote),
        }
    }
}
