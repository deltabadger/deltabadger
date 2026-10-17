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
//! - `fx`: Tax::EcbFxRates, the ECB's rates and their loading.
//!
//! The walk takes every venue's rows as Rails does: swaps, fees carried on rows, other cash at the ECB's rates, transfers
//! between venues, the funding a venue that does not lend cash never reported. A price nobody has is a zero and the
//! figure is marked incomplete, as Rails marks it. Refused, with the reason: a walk that needs a price from CoinGecko
//! (this build fetches from data-api only), and a stock's closes from a venue other than Alpaca.
pub mod backfill;
pub mod figures;
pub mod fx;
pub mod import;
pub mod jobs;
pub mod parity;
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

pub mod cache;
