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

/// Credentials and their digest are captured once, before this handle is built.
/// The raw transport is private: production consumers use the attributed methods below.
pub struct Handle<V> { venue:V, producer:Option<crate::engine::model::CredentialVersion> }
impl<V:Venue> Handle<V> {
    pub(crate) fn new(venue:V,producer:Option<crate::engine::model::CredentialVersion>)->Self { Self{venue,producer} }
    pub(crate) fn for_bot(venue:V,c:&rusqlite::Connection,bot:&crate::engine::model::Bot)->Result<Self,crate::engine::EngineError>{
        let producer=match venue.producer(){Some(origin)=>Some(origin),None=>crate::engine::model::credential_version(c,bot)?};
        Ok(Self::new(venue,producer))
    }
}
impl<V:Venue> Venue for &V {
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{(*self).producer()}
    fn rules(&self)->&'static VenueRules{(*self).rules()}
    async fn positions(&self)->Result<std::collections::HashMap<String,BigDec>,VenueError>{(*self).positions().await}
    async fn clock(&self)->Result<ClockAnswer,VenueError>{(*self).clock().await}
    async fn price(&self,t:&Ticker,s:PriceSide)->Result<BigDec,VenueError>{(*self).price(t,s).await}
    async fn add_order(&self,o:&NewOrder)->Result<String,VenueError>{(*self).add_order(o).await}
    async fn orders(&self,ids:&[String])->Result<Vec<OrderState>,VenueError>{(*self).orders(ids).await}
    async fn orders_identified<F>(&self,ids:&[String],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{(*self).orders_identified(ids,identify).await}
    async fn order_by_client_id(&self,id:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{(*self).order_by_client_id(id,since).await}
    async fn fills_from_trades(&self,ids:&[String],since:DateTime<Utc>)->Result<Vec<OrderState>,VenueError>{(*self).fills_from_trades(ids,since).await}
    async fn balance(&self,symbol:&str,crypto:bool)->Result<BigDec,VenueError>{(*self).balance(symbol,crypto).await}
}
impl<V:Venue> Venue for Handle<V> {
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{self.producer.clone()}
    fn rules(&self)->&'static VenueRules{self.venue.rules()}
    async fn positions(&self)->Result<std::collections::HashMap<String,BigDec>,VenueError>{self.venue.positions().await}
    async fn clock(&self)->Result<ClockAnswer,VenueError>{self.venue.clock().await}
    async fn price(&self,t:&Ticker,s:PriceSide)->Result<BigDec,VenueError>{self.venue.price(t,s).await}
    async fn add_order(&self,o:&NewOrder)->Result<String,VenueError>{self.venue.add_order(o).await}
    async fn orders(&self,ids:&[String])->Result<Vec<OrderState>,VenueError>{self.venue.orders(ids).await}
    async fn orders_identified<F>(&self,ids:&[String],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.venue.orders_identified(ids,identify).await}
    async fn order_by_client_id(&self,id:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{self.venue.order_by_client_id(id,since).await}
    async fn fills_from_trades(&self,ids:&[String],since:DateTime<Utc>)->Result<Vec<OrderState>,VenueError>{self.venue.fills_from_trades(ids,since).await}
    async fn balance(&self,symbol:&str,crypto:bool)->Result<BigDec,VenueError>{self.venue.balance(symbol,crypto).await}
}
/// Apply one immutable handle producer to success and failure values, never from a post-call lookup.
mod sealed { pub trait Sealed {} impl<V:super::Venue> Sealed for super::Handle<V> {} }
pub trait Attributed:Venue + sealed::Sealed {
    async fn clock_result(&self)->crate::engine::model::Produced<Result<ClockAnswer,VenueError>>{let origin=self.producer();let value=self.clock().await;crate::engine::model::Produced::new(value,origin)}
    async fn price_result(&self,t:&Ticker,s:PriceSide)->crate::engine::model::Produced<Result<BigDec,VenueError>>{let origin=self.producer();let value=self.price(t,s).await;crate::engine::model::Produced::new(value,origin)}
    async fn positions_result(&self)->crate::engine::model::Produced<Result<std::collections::HashMap<String,BigDec>,VenueError>>{let origin=self.producer();let value=self.positions().await;crate::engine::model::Produced::new(value,origin)}
    async fn balance_result(&self,s:&str,crypto:bool)->crate::engine::model::Produced<Result<BigDec,VenueError>>{let origin=self.producer();let value=self.balance(s,crypto).await;crate::engine::model::Produced::new(value,origin)}
    async fn orders_result<F>(&self,ids:&[String],identify:F)->crate::engine::model::Produced<Result<(Vec<OrderState>,Vec<String>),VenueError>> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{let origin=self.producer();let value=self.orders_identified(ids,identify).await;crate::engine::model::Produced::new(value,origin)}
    async fn fills_result(&self,ids:&[String],since:DateTime<Utc>)->crate::engine::model::Produced<Result<Vec<OrderState>,VenueError>>{let origin=self.producer();let value=self.fills_from_trades(ids,since).await;crate::engine::model::Produced::new(value,origin)}
    async fn placement_result(&self,o:&NewOrder)->crate::engine::model::Produced<Result<String,VenueError>>{let origin=self.producer();let value=self.add_order(o).await;crate::engine::model::Produced::new(value,origin)}
    async fn recovery_result(&self,id:&str,since:DateTime<Utc>)->crate::engine::model::Produced<Result<Option<OrderState>,VenueError>>{let origin=self.producer();let value=self.order_by_client_id(id,since).await;crate::engine::model::Produced::new(value,origin)}
}
impl<V:Venue> Attributed for Handle<V> {}
pub trait Venue {
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{None}
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
