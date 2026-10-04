//! The tracker's ledger walk, its figures and the portfolio snapshots, as Rails computes them (Plan D4a):
//! - `rows`: the account's ledger rows in the order every reader walks them (Tax::PriceService.ordered);
//! - `prices`: Tax::PriceService#price_at and Tax::AssetIdentity, over `historical_prices` and data-api;
//! - `walk`: Tracker::Ledger.scopes, the FIFO engine with the tracker's extensions, the cash book and money in;
//! - `figures`: Tracker::Figures, the ledger reconciled with the balances;
//! - `snapshot`: PortfolioSnapshot.record!, today's rows;
//! - `backfill`: PortfolioSnapshot::BackfillJob, every earlier day;
//! - `wash`: Tracker::LedgerJob#arm_wash_sale_locks;
//! - `jobs`: the scheduler's jobs that run them; `parity`: the Rust half of the parity harness.
//!
//! Ported for the rows the Alpaca ledger sync writes, in US dollars (stablecoins at face). A history with anything
//! else is refused (`FiguresError::NotComputed`, the reason in words): rows or balances of another venue, swap legs, a
//! fee carried on a row, cash in another currency, a linked transfer of anything but dollars, a trade leg with no cash
//! quote of its own; and so is a walk that needs a price it cannot get (CoinGecko's, one two fetches did not bring, or one no coin can be named for).
//! Rails computes those (Plan D4b, or at a zero basis); this build states no figure rather than a wrong one.
pub mod figures;
pub mod prices;
pub mod rows;
pub mod snapshot;
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
