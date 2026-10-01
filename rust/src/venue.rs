//! What the engine needs from an exchange (Kraken in this milestone). One instance per API key.
//! The real Kraken implementation is Plan 2b (honeymaker's client); tests use `fake::FakeVenue`.
#![allow(async_fn_in_trait)] // single-threaded runtime: no Send bound needed
pub mod fake;

use crate::crypto::Credentials;
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq)]
pub struct Prices { pub bid: BigDec, pub ask: BigDec, pub last: BigDec }

#[derive(Clone, Debug, PartialEq)]
pub enum OrderKind { Market, Limit { price: String } }

/// A buy, formatted as the wire expects: volume and price already floored and in `to_s('F')` form.
#[derive(Clone, Debug, PartialEq)]
pub struct NewOrder {
    pub pair: String, pub kind: OrderKind, pub volume: String,
    /// Kraken `oflags=viqc`: the volume is in quote currency.
    pub quote_volume: bool,
    pub cl_ord_id: String, pub deadline: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OrderStatus { Unknown, Open, Closed, Cancelled }

/// An order as Exchanges::Kraken#parse_order_data reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct OrderState {
    pub txid: String, pub status: OrderStatus, pub price: Option<BigDec>, pub amount: Option<BigDec>, pub quote_amount: Option<BigDec>,
    pub amount_exec: BigDec, pub quote_amount_exec: BigDec, pub limit: bool,
    /// `descr.type == "sell"`: kept as the exchange reports it (a buying bot can still hold an old sell).
    pub sell: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum VenueError {
    /// Kraken answered with errors (its `error` array, verbatim). Nothing was placed.
    Rejected(Vec<String>),
    /// The request may have reached Kraken (lost reply, timeout after send, unreadable 2xx).
    Ambiguous(String),
    /// Nothing reached Kraken (refused connection, DNS, TLS before send).
    Transient(String),
}

pub trait Venue {
    async fn prices(&self, pair: &str) -> Result<Prices, VenueError>;
    /// Sent at most once per call. Never retried inside.
    async fn add_order(&self, order: &NewOrder) -> Result<String, VenueError>;
    /// QueryOrders; ids Kraken does not report are simply absent.
    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError>;
    /// OpenOrders, then every page of ClosedOrders since `since`. `Err` unless the whole scan completed.
    async fn order_by_client_id(&self, cl_ord_id: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError>;
    /// TradesHistory aggregated per order (Exchanges::Kraken#recover_missing_from_trades).
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError>;
    /// Free balance of an asset (Exchanges::Kraken#get_balances: balance − hold_trade; absent → 0).
    async fn balance(&self, asset_symbol: &str) -> Result<BigDec, VenueError>;
}

pub trait VenueFactory {
    type V: Venue;
    /// `None` is Rails' unsaved fallback key: private calls fail at the venue, per bot.
    fn for_key(&self, credentials: Option<Credentials>) -> Self::V;
}
