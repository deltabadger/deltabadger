//! What the engine needs from an exchange. One instance per API key; the factory picks the venue by the bot's
//! exchange type. Tests use `fake::FakeVenue` (raw Kraken bodies) and, from Task 6, `alpaca::AlpacaVenue` over
//! `http::ScriptedTransport`.
#![allow(async_fn_in_trait)] // single-threaded runtime: no Send bound needed
pub mod fake;
pub mod alpaca;
pub mod http;

use crate::crypto::Credentials;
use crate::engine::model::Ticker;
use crate::engine::venue_rules::VenueRules;
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};

/// Which price a buy needs (Bot::OrderSetter#reference_price): the ask for a market buy, the last trade for a limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PriceSide { Ask, Last }

#[derive(Clone, Debug, PartialEq)]
pub enum OrderKind { Market, Limit { price: String } }

/// A buy, formatted as the venue's wire expects (VenueRules::wire).
#[derive(Clone, Debug, PartialEq)]
pub struct NewOrder {
    pub pair: String, pub kind: OrderKind, pub volume: String,
    /// The volume is in quote currency (Kraken `oflags=viqc`, Alpaca `notional`).
    pub quote_volume: bool,
    /// The venue-neutral client order id (Kraken `cl_ord_id`, Alpaca `client_order_id`).
    pub cl_ord_id: String,
    /// Kraken's AddOrder `deadline`. Recorded for every intent; sent only where VenueRules::deadline_sent.
    pub deadline: DateTime<Utc>, pub day: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OrderStatus {
    Unknown, Open, Closed, Cancelled,
    /// Alpaca `rejected` → :failed. Neither Rails poll job has a branch for it: the row is left as it is.
    Failed,
}

/// An order as the venue's parse_order_data reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderState {
    pub txid: String, pub status: OrderStatus, pub price: Option<BigDec>, pub amount: Option<BigDec>, pub quote_amount: Option<BigDec>,
    pub amount_exec: BigDec, pub quote_amount_exec: BigDec, pub limit: bool,
    /// The side as the exchange reports it (a buying bot can still hold an old sell).
    pub sell: bool,
    /// The pair as the venue names it in this order (Alpaca `symbol`, Kraken `descr.pair`): Rails backfills a row's blank
    /// asset fields from this pair's ticker (`order_data[:ticker]`), never from the bot. None where the venue gives none.
    pub pair: Option<String>,
    pub asset_class: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VenueError {
    /// The venue answered and refused (its own error text). Nothing was placed.
    Rejected(Vec<String>),
    /// The request may have reached the venue (lost reply, timeout after send, a 5xx or unreadable answer to a placement).
    Ambiguous(String),
    /// Nothing reached the venue (refused connection, DNS, TLS before send), or a read failed in transport.
    Transient(String),
}

pub trait Venue {
    async fn positions(&self) -> Result<std::collections::HashMap<String, BigDec>, VenueError> { Err(VenueError::Rejected(vec!["positions unavailable".into()])) }

    /// The market clock. Asked only where VenueRules::market_hours. A transport failure that Client.network_failure raises
    /// (refused, timeout, lost reply) is `Err(Transient)`.
    async fn clock(&self) -> Result<ClockAnswer, VenueError> { Err(VenueError::Rejected(vec!["market clock unsupported".into()])) }

    /// This venue's Rails rules (errors, minimums, wire, absence window).
    fn rules(&self) -> &'static VenueRules;
    /// One reference price. A zero or missing price is `Rejected` with the venue's own "Wrong … price" message.
    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError>;
    /// Sent at most once per call. Never retried inside.
    async fn add_order(&self, order: &NewOrder) -> Result<String, VenueError>;
    /// The orders asked for; ids the venue does not report are simply absent.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError>;
    /// Alpaca resolves identity before interpreting a row. Excluded IDs are present, not missing orders.
    /// Other venues retain their existing parsing and identity behavior.
    async fn orders_identified<F>(&self, txids: &[String], _identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        self.orders(txids).await.map(|orders| (orders, vec![]))
    }
    /// The order with this client order id, if the venue holds one. `Err` unless the answer is complete.
    async fn order_by_client_id(&self, cl_ord_id: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError>;
    /// Fills recovered from trade history (Kraken TradesHistory). Alpaca has none: Rails has no trade fallback there.
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError>;
    /// What Bot::Fundable compares with its buffer (Kraken: balance − hold_trade; Alpaca: #spendable_balance).
    async fn balance(&self, asset_symbol: &str, all_crypto: bool) -> Result<BigDec, VenueError>;
}

pub trait VenueFactory {
    type V: Venue;
    /// `exchange_type` is the bot's exchanges.type. `None` credentials is Rails' unsaved fallback key: private calls
    /// fail at the venue, per bot.
    fn for_bot(&self, exchange_type: &str, credentials: Option<Credentials>) -> Self::V;
    /// Full stored material is used only to redact diagnostics, never venue financial data.
    fn for_bot_with_redaction(&self,exchange_type:&str,credentials:Option<Credentials>,_sensitive:Vec<String>)->Self::V{self.for_bot(exchange_type,credentials)}
}

/// GET /v2/clock as Clients::Alpaca#get_clock returns it: the 2xx body as text, or the failure with its HTTP status (None for
/// a TLS failure, which Client.network_failure returns as a Failure) and with_rescue's message.
#[derive(Clone, Debug, PartialEq)]
pub enum ClockAnswer { Body(String), Failed { status: Option<u16>, message: String } }
