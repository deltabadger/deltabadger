//! The tracker's ledger walk, its figures and the portfolio snapshots, as 2.x computes them (Plan D4a):
//! - `rows`: the account's ledger rows in the order every reader walks them (Tax::PriceService.ordered);
//! - `prices`: Tax::PriceService#price_at and Tax::AssetIdentity, over `historical_prices` and data-api;
//! - `walk`: Tracker::Ledger.scopes, the FIFO engine with the tracker's extensions, the cash book and money in;
//! - `figures`: Tracker::Figures, the ledger reconciled with the balances;
//! - `snapshot`: PortfolioSnapshot.record!, today's rows;
//! - `backfill`: PortfolioSnapshot::BackfillJob, every earlier day;
//! - `wash`: Tracker::LedgerJob#arm_wash_sale_locks;
//! - `jobs`: the scheduler's jobs that run them; `parity`: the Rust half of the parity harness.
//!
//! - `fx`: Tax::EcbFxRates, the ECB's rates and their loading.
//! - `gaps`: the "import history" line (Rust only): a venue holding more than its readable history, judged after a publish.
//!
//! The walk takes every venue's rows as 2.x does: swaps, fees carried on rows, other cash at the ECB's rates, transfers
//! between venues, the funding a venue that does not lend cash never reported. A price nobody has is a zero and the
//! figure is marked incomplete, as 2.x marks it. Refused, with the reason: a walk that needs a price from CoinGecko
//! (this build fetches from data-api only), and a stock's closes from a venue other than Alpaca.
pub mod backfill;
pub mod figures;
pub mod fx;
pub mod gaps;
pub mod import;
pub mod jobs;
pub mod parity;
pub mod prices;
pub mod rows;
pub mod snapshot;
pub mod unpriced;
pub mod walk;
pub mod wash;

/// Tax::PriceService::FIAT_CURRENCIES.
pub const FIAT: [&str; 13] = ["USD", "EUR", "GBP", "CHF", "SEK", "PLN", "DKK", "CZK", "BGN", "AUD", "CAD", "JPY", "AED"];
/// Tax::PriceService::STABLECOINS.
pub const STABLECOINS: [&str; 8] = ["USDT", "USDC", "BUSD", "DAI", "FDUSD", "TUSD", "PYUSD", "RLUSD"];

pub fn fiat(currency: &str) -> bool { FIAT.contains(&currency) }
pub fn stable(currency: &str) -> bool { STABLECOINS.contains(&currency) }
/// Tracker::UnfundedCash.cash?.
pub fn cash(currency: &str) -> bool { fiat(currency) || stable(currency) }

/// The one venue this build walks: Exchanges::Alpaca, whose `name_id` is "alpaca".
pub const VENUE: &str = "alpaca";
pub const VENUE_TYPE: &str = "Exchanges::Alpaca";

/// Why a history is not walked here.
pub fn refused(why: &str) -> crate::figures::FiguresError {
    crate::figures::FiguresError::NotComputed(format!("the tracker walk is not ported for {why}"))
}

pub mod cache;

/// #710 slice 5 (PLAN-710-5 §5.1, §8 Q7): what a tracker figure reads. The account is the unit: the whole portfolio, or
/// one account; no figure combines an exchange's accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The whole portfolio.
    All,
    /// One account.
    Account(crate::identity::AccountRow),
    /// A known exchange an old `exchange_id` link names where the user has no account: it reads nothing (never `All`).
    EmptyLegacyExchange(i64),
}

impl Scope {
    /// The scope an `exchange_id` names (None: the whole portfolio): its number-1 account; while that has no row, its
    /// stand-in when the user has a key or a ledger row there (`AccountRow::unplaced`), else nothing.
    pub fn of_exchange(c: &rusqlite::Connection, user_id: i64, exchange_id: Option<i64>) -> rusqlite::Result<Self> {
        let Some(e) = exchange_id else { return Ok(Self::All) };
        if let Some(a) = crate::identity::AccountRow::first(c, user_id, e)? { return Ok(Self::Account(a)); }
        let named: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM api_keys WHERE user_id = ?1 AND exchange_id = ?2) OR EXISTS(SELECT 1 FROM account_transactions WHERE user_id = ?1 AND exchange_id = ?2)",
                                      [user_id, e], |r| r.get(0))?;
        Ok(if named { Self::Account(crate::identity::AccountRow::unplaced(user_id, e)) } else { Self::EmptyLegacyExchange(e) })
    }
    /// The exchange the scope is on (None: the whole portfolio).
    pub fn exchange(&self) -> Option<i64> {
        match self { Self::All => None, Self::Account(a) => Some(a.exchange_id), Self::EmptyLegacyExchange(e) => Some(*e) }
    }
    pub fn account(&self) -> Option<&crate::identity::AccountRow> { match self { Self::Account(a) => Some(a), _ => None } }
    /// SQL condition: store row `alias` (exchange_account_balances, exchange_account_snapshots) is in the scope.
    pub fn store(&self, alias: &str) -> String {
        // A stand-in's id (minus the exchange id) is every user's on that exchange: the owner's own rows.
        match self { Self::All => "1".into(), Self::Account(a) => format!("({alias}.account_id = {} AND {alias}.user_id = {})", a.id, a.user_id), Self::EmptyLegacyExchange(_) => "0".into() }
    }
    /// SQL condition: ledger row `alias` (account_transactions) is in the scope.
    pub fn rows(&self, alias: &str) -> String {
        match self {
            Self::All => "1".into(),
            Self::Account(a) => format!("({alias}.exchange_id = {} AND {})", a.exchange_id, a.venue_ids().rows(alias)),
            Self::EmptyLegacyExchange(_) => "0".into(),
        }
    }
    /// SQL condition: api key row `alias` is in the scope.
    pub fn keys(&self, alias: &str) -> String {
        match self { Self::All => "1".into(), Self::Account(a) => a.keys(alias), Self::EmptyLegacyExchange(_) => "0".into() }
    }
}
