//! The figures of the bots list and the bot page, computed as Rails computes them: the metrics walk over a
//! bot's orders (`walk`), the live-price pass (`live`), the chart marked at market (`chart`), and the account's
//! totals (`totals`). A library: no routes, no cache, and it only reads the database.
//!
//! Rails is the oracle. Each value keeps the kind Ruby gave it (`num::Num`), each hash keeps Ruby's key order,
//! and `json` writes them as Rails' encoder does, so a figure can be compared with Rails' character by character
//! (rust/tests/figures.rs, script/rust/figures.rb).
pub mod at;
pub mod budget;
pub mod dec;
pub mod json;
pub mod num;

#[derive(Debug)]
pub enum FiguresError {
    Sqlite(rusqlite::Error),
    /// A stored value this build cannot read the way Rails wrote it.
    Data(String),
    /// Rails computes this and this build does not: a bot of another type, a number beyond the limits of `dec`, a
    /// value that is not a finite decimal number, arithmetic beyond its budget. No figure is returned, never an approximate one.
    NotComputed(String),
    /// Rails raises here, and so shows no figure either: a number that cannot be made (a division by zero),
    /// or a market-data failure Rails hands to its caller's retry.
    Raised(String),
}
impl From<rusqlite::Error> for FiguresError { fn from(e: rusqlite::Error) -> Self { Self::Sqlite(e) } }
impl From<num::NumError> for FiguresError {
    fn from(e: num::NumError) -> Self {
        match e {
            num::NumError::Raised(error) => Self::Raised(error),
            num::NumError::OutOfRange => Self::NotComputed(OUT_OF_RANGE.into()),
            num::NumError::OverBudget => Self::NotComputed(OVER_BUDGET.into()),
            num::NumError::NotANumber => Self::NotComputed(NOT_A_NUMBER.into()),
        }
    }
}

/// Why a figure is not there, for a caller that goes on to the next one: what Rails raised computing it, or why this
/// library does not compute it.
#[derive(Clone, Debug, PartialEq)]
pub enum Absent { Raised(String), NotComputed(String) }

impl From<&Absent> for FiguresError {
    fn from(absent: &Absent) -> FiguresError {
        match absent { Absent::Raised(error) => FiguresError::Raised(error.clone()), Absent::NotComputed(reason) => FiguresError::NotComputed(reason.clone()) }
    }
}

/// Why a figure with a number beyond `dec`'s limits is not computed.
pub const OUT_OF_RANGE: &str = "number out of range";
/// Why a figure whose arithmetic passed `budget::CEILING` is not computed.
pub const OVER_BUDGET: &str = "the arithmetic of this history is beyond what one figure may cost";
/// Why a figure made from Infinity, NaN or a text that is no decimal is not computed.
pub const NOT_A_NUMBER: &str = "a value that is not a finite decimal number";
