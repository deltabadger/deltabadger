//! Where the figures get what the database does not hold: current prices, candles and exchange rates. Rails reads
//! each through a cache that background jobs and page requests fill; this library has no cache and no jobs, so a
//! `MarketData` is asked each time and answers with what it has. The real ones belong to the plan that serves the
//! pages; `scripted::Scripted` replays a scenario's recorded answers.
use super::at::At;
use super::db::Ticker;
use super::dec::Dec;
use super::num::{Num, NumError};

/// How a read can fail, as Rails' clients report it.
#[derive(Clone, Debug, PartialEq)]
pub enum Failure {
    /// A `Result::Failure`: the venue or the provider answered with an error, or has no such figure.
    Failed(String),
    /// An exception raised to the caller's retry (`Client::TransientNetworkError`, a `KeyError`), as
    /// `"<class>: <message>"`.
    Raised(String),
    /// An answer this library will not compute with: a number beyond `dec`'s limits, Infinity or NaN, a value
    /// that is no number. Rails carries such a value on; here the figure that needed it is not computed.
    NotComputed(String),
}

impl From<NumError> for Failure {
    fn from(e: NumError) -> Failure {
        match e {
            NumError::Raised(error) => Failure::Raised(error),
            NumError::OutOfRange => Failure::NotComputed(super::OUT_OF_RANGE.into()),
            NumError::OverBudget => Failure::NotComputed(super::OVER_BUDGET.into()),
            NumError::NotANumber => Failure::NotComputed(super::NOT_A_NUMBER.into()),
        }
    }
}

/// A coin's price as the provider's JSON carried it. Rails hands a String on as it is, and what becomes of it
/// depends on who asked: `Bot#profit_in_usd` and `User::PnlHistory` call `to_d` on it, `batch_convert` multiplies by
/// it and raises.
#[derive(Clone, Debug, PartialEq)]
pub enum Quoted { Num(Num), Text(String) }

impl Quoted {
    /// `rate.to_d`.
    pub fn to_d(&self) -> Result<Dec, NumError> {
        match self { Quoted::Num(n) => n.to_d(), Quoted::Text(text) => Dec::to_d(&serde_json::Value::String(text.clone())) }
    }
    /// What the provider's JSON holds at a price: a number, a string, or nothing (`None`: nil, or a value Ruby
    /// has no arithmetic for).
    pub fn from_json(v: &serde_json::Value) -> Result<Option<Quoted>, NumError> {
        match v {
            serde_json::Value::String(text) => Ok(Some(Quoted::Text(text.clone()))),
            other => Ok(Num::from_json(other)?.map(Quoted::Num)),
        }
    }
}

pub type Fetch<T> = Result<T, Failure>;

/// One number of an answer that holds many: read, or why it could not be. The reason is kept with the member and
/// becomes a figure's only when the figure uses that member: an index bot asks for every ticker of its venue, and a
/// price that is no number for a ticker it never held is not its concern, as it is not Rails'.
pub type Member<T> = Result<T, NumError>;

/// The bot's exchange: its id and its `exchanges.type`.
#[derive(Clone, Debug)]
pub struct Venue { pub exchange_id: i64, pub exchange_type: String }

pub trait MarketData {
    /// Exchange#get_tickers_prices(symbols:): the last price of each ticker code the venue priced; a code it did
    /// not answer for is absent. No codes, no request. Every price is made by `Dec::to_d` (or another constructor
    /// of `dec`), which is what keeps a hostile answer out.
    fn prices(&self, venue: &Venue, symbols: &[String]) -> Fetch<Vec<(String, Member<Dec>)>>;
    /// Ticker#get_candles (or #get_indicator_candles when `restated`): `(open time, open)` of every candle from
    /// `since` on, of `timeframe` seconds each, as the venue returns them: unsorted, and the open one included.
    fn candles(&self, venue: &Venue, ticker: &Ticker, since: At, timeframe: i64, restated: bool) -> Fetch<Vec<(At, Dec)>>;
    /// MarketData.get_exchange_rates: `currency (lower case) => its BTC-based value`.
    fn exchange_rates(&self) -> Fetch<Vec<(String, Member<Num>)>>;
    /// MarketData.get_price: one coin in one currency (lower case), as the provider's JSON carried it.
    fn coin_price(&self, coin_id: &str, currency: &str) -> Fetch<Quoted>;
}
