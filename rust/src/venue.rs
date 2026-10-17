//! What the engine needs from an exchange. One instance per API key; the factory picks the venue by the bot's
//! exchange type. Tests use `fake::FakeVenue` (raw Kraken bodies) and, from Task 6, `alpaca::AlpacaVenue` over
//! `http::ScriptedTransport`.
#![allow(async_fn_in_trait)] // single-threaded runtime: no Send bound needed
pub mod fake;
pub mod alpaca;
pub mod http;
pub mod binance;
pub mod kraken;
pub mod kucoin;
pub mod live;

use crate::crypto::Credentials;
use crate::engine::model::Ticker;
use crate::engine::venue_rules::VenueRules;
use crate::ruby::BigDec;
use crate::sync::balances::Catalog;
use chrono::{DateTime, Utc};

/// What an intent records of the key a Kraken, Binance or KuCoin order is sent under (Alpaca's is `alpaca::placement_identity`):
/// the host and a fingerprint of the public key id. Never the secret.
pub fn key_identity(host: &str, credentials: &Credentials) -> serde_json::Value {
    use sha2::{Digest, Sha256};
    serde_json::json!({ "host": host, "key_fingerprint": hex::encode(Sha256::digest(credentials.key.as_bytes())) })
}

/// Which price an order needs (Bot::OrderSetter#reference_price): the ask for a market buy, the bid for a market sell, the
/// last trade for a limit either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PriceSide { Ask, Bid, Last }

/// An order's side, as the venue's wire spells it (Kraken `type`, Binance `side` upcased, Alpaca `side`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Side { #[default] Buy, Sell }

impl Side {
    pub fn as_str(self) -> &'static str { match self { Self::Buy => "buy", Self::Sell => "sell" } }
    /// transactions.side (Transaction enum: buy 0, sell 1).
    pub fn code(self) -> i64 { match self { Self::Buy => 0, Self::Sell => 1 } }
}

#[derive(Clone, Debug, PartialEq)]
pub enum OrderKind { Market, Limit { price: String } }

/// An order, formatted as the venue's wire expects (VenueRules::wire).
#[derive(Clone, Debug, PartialEq)]
pub struct NewOrder {
    pub pair: String, pub side: Side, pub kind: OrderKind, pub volume: String,
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
    pub amount_exec: BigDec,
    /// None where the venue stated none (Binance's negative cummulativeQuoteQty on some historical orders): the poll leaves
    /// the column as it is, as Transaction#update_with_order_data's `.compact` omits a nil. Never a zero for an unknown.
    pub quote_amount_exec: Option<BigDec>, pub limit: bool,
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
    fn placement_identity(&self)->Option<serde_json::Value>{(*self).placement_identity()}
    fn rules(&self)->&'static VenueRules{(*self).rules()}
    async fn positions(&self)->Result<std::collections::HashMap<String,BigDec>,VenueError>{(*self).positions().await}
    async fn clock(&self)->Result<ClockAnswer,VenueError>{(*self).clock().await}
    async fn price(&self,t:&Ticker,s:PriceSide)->Result<BigDec,VenueError>{(*self).price(t,s).await}
    async fn add_order(&self,o:&NewOrder)->Result<String,VenueError>{(*self).add_order(o).await}
    async fn orders(&self,ids:&[String])->Result<Vec<OrderState>,VenueError>{(*self).orders(ids).await}
    async fn orders_identified<F>(&self,ids:&[String],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{(*self).orders_identified(ids,identify).await}
    async fn order_identified<F>(&self,id:&str,identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{(*self).order_identified(id,identify).await}
    async fn order_by_client_id(&self,id:&str,symbol:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{(*self).order_by_client_id(id,symbol,since).await}
    async fn fills_from_trades(&self,ids:&[String],since:DateTime<Utc>)->Result<Vec<OrderState>,VenueError>{(*self).fills_from_trades(ids,since).await}
    async fn balance(&self,symbol:&str,crypto:bool)->Result<BigDec,VenueError>{(*self).balance(symbol,crypto).await}
    async fn free_base_balance(&self,t:&Ticker)->Result<BigDec,VenueError>{(*self).free_base_balance(t).await}
    async fn free_base_identified<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{(*self).free_base_identified(t,catalog).await}
    async fn settled_quote_balance<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{(*self).settled_quote_balance(t,catalog).await}
    async fn tickers_prices(&self,symbols:&[String])->Result<Vec<(String,serde_json::Value)>,VenueError>{(*self).tickers_prices(symbols).await}
    async fn cancel(&self,txid:&str)->Result<(),VenueError>{(*self).cancel(txid).await}
}
impl<V:Venue> Venue for Handle<V> {
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{self.producer.clone()}
    fn placement_identity(&self)->Option<serde_json::Value>{self.venue.placement_identity()}
    fn rules(&self)->&'static VenueRules{self.venue.rules()}
    async fn positions(&self)->Result<std::collections::HashMap<String,BigDec>,VenueError>{self.venue.positions().await}
    async fn clock(&self)->Result<ClockAnswer,VenueError>{self.venue.clock().await}
    async fn price(&self,t:&Ticker,s:PriceSide)->Result<BigDec,VenueError>{self.venue.price(t,s).await}
    async fn add_order(&self,o:&NewOrder)->Result<String,VenueError>{self.venue.add_order(o).await}
    async fn orders(&self,ids:&[String])->Result<Vec<OrderState>,VenueError>{self.venue.orders(ids).await}
    async fn orders_identified<F>(&self,ids:&[String],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.venue.orders_identified(ids,identify).await}
    async fn order_identified<F>(&self,id:&str,identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.venue.order_identified(id,identify).await}
    async fn order_by_client_id(&self,id:&str,symbol:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{self.venue.order_by_client_id(id,symbol,since).await}
    async fn fills_from_trades(&self,ids:&[String],since:DateTime<Utc>)->Result<Vec<OrderState>,VenueError>{self.venue.fills_from_trades(ids,since).await}
    async fn balance(&self,symbol:&str,crypto:bool)->Result<BigDec,VenueError>{self.venue.balance(symbol,crypto).await}
    async fn free_base_balance(&self,t:&Ticker)->Result<BigDec,VenueError>{self.venue.free_base_balance(t).await}
    async fn free_base_identified<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{self.venue.free_base_identified(t,catalog).await}
    async fn settled_quote_balance<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{self.venue.settled_quote_balance(t,catalog).await}
    async fn tickers_prices(&self,symbols:&[String])->Result<Vec<(String,serde_json::Value)>,VenueError>{self.venue.tickers_prices(symbols).await}
    async fn cancel(&self,txid:&str)->Result<(),VenueError>{self.venue.cancel(txid).await}
}
/// Apply one immutable handle producer to success and failure values, never from a post-call lookup.
mod sealed { pub trait Sealed {} impl<V:super::Venue> Sealed for super::Handle<V> {} }
pub trait Attributed:Venue + sealed::Sealed {
    async fn clock_result(&self)->crate::engine::model::Produced<Result<ClockAnswer,VenueError>>{let origin=self.producer();let value=self.clock().await;crate::engine::model::Produced::new(value,origin)}
    async fn price_result(&self,t:&Ticker,s:PriceSide)->crate::engine::model::Produced<Result<BigDec,VenueError>>{let origin=self.producer();let value=self.price(t,s).await;crate::engine::model::Produced::new(value,origin)}
    async fn positions_result(&self)->crate::engine::model::Produced<Result<std::collections::HashMap<String,BigDec>,VenueError>>{let origin=self.producer();let value=self.positions().await;crate::engine::model::Produced::new(value,origin)}
    async fn free_base_result<F>(&self,t:&Ticker,catalog:F)->crate::engine::model::Produced<Result<BigDec,VenueError>> where F:FnOnce()->Result<Catalog,VenueError>{let origin=self.producer();let value=self.free_base_identified(t,catalog).await;crate::engine::model::Produced::new(value,origin)}
    async fn settled_quote_result<F>(&self,t:&Ticker,catalog:F)->crate::engine::model::Produced<Result<BigDec,VenueError>> where F:FnOnce()->Result<Catalog,VenueError>{let origin=self.producer();let value=self.settled_quote_balance(t,catalog).await;crate::engine::model::Produced::new(value,origin)}
    async fn balance_result(&self,s:&str,crypto:bool)->crate::engine::model::Produced<Result<BigDec,VenueError>>{let origin=self.producer();let value=self.balance(s,crypto).await;crate::engine::model::Produced::new(value,origin)}
    async fn orders_result<F>(&self,ids:&[String],identify:F)->crate::engine::model::Produced<Result<(Vec<OrderState>,Vec<String>),VenueError>> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{let origin=self.producer();let value=self.orders_identified(ids,identify).await;crate::engine::model::Produced::new(value,origin)}
    async fn order_result<F>(&self,id:&str,identify:F)->crate::engine::model::Produced<Result<(Vec<OrderState>,Vec<String>),VenueError>> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{let origin=self.producer();let value=self.order_identified(id,identify).await;crate::engine::model::Produced::new(value,origin)}
    async fn fills_result(&self,ids:&[String],since:DateTime<Utc>)->crate::engine::model::Produced<Result<Vec<OrderState>,VenueError>>{let origin=self.producer();let value=self.fills_from_trades(ids,since).await;crate::engine::model::Produced::new(value,origin)}
    async fn placement_result(&self,o:&NewOrder)->crate::engine::model::Produced<Result<String,VenueError>>{let origin=self.producer();let value=self.add_order(o).await;crate::engine::model::Produced::new(value,origin)}
    async fn recovery_result(&self,id:&str,symbol:&str,since:DateTime<Utc>)->crate::engine::model::Produced<Result<Option<OrderState>,VenueError>>{let origin=self.producer();let value=self.order_by_client_id(id,symbol,since).await;crate::engine::model::Produced::new(value,origin)}
}
impl<V:Venue> Attributed for Handle<V> {}
pub trait Venue {
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{None}
    /// The key an order is sent under, as an intent records it and recovery compares it (`alpaca::placement_identity`,
    /// `key_identity`): an object of fields merged into the intent. None where a venue has nothing to tell (no key, a
    /// scripted venue).
    fn placement_identity(&self)->Option<serde_json::Value>{None}
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
    /// One order, as Bot::FetchAndUpdateOrderJob asks for it (Exchange#get_order): `orders_identified` of that one id, unless
    /// the venue asks for one order its own way (Binance: GET /api/v3/order). Identity first, as there.
    async fn order_identified<F>(&self, txid: &str, identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        self.orders_identified(&[txid.to_string()], identify).await
    }
    /// The order with this client order id, if the venue holds one. `Err` unless the answer is complete. `symbol` is the pair
    /// the intent was sent for (Binance looks an order up per symbol; Kraken and Alpaca ignore it), never the bot's current one.
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError>;
    /// Fills recovered from trade history (Kraken TradesHistory). Alpaca has none: Rails has no trade fallback there. An
    /// `Unknown` entry is an id the history could not rule a fill out for: it stays waiting, never abandoned.
    async fn fills_from_trades(&self, txids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError>;
    /// What Bot::Fundable compares with its buffer (Kraken: balance − hold_trade; Alpaca: #spendable_balance).
    async fn balance(&self, asset_symbol: &str, all_crypto: bool) -> Result<BigDec, VenueError>;
    /// Bot::Rebalancer#live_free_balance for a one-asset seller: the free amount of the ticker's BASE asset, resolved as the
    /// venue's #get_balances resolves an asset (its own code for the ticker's base, never a bare symbol that may name another
    /// asset). Absent is 0 (`no_holdings`); an unreadable answer is `Err`, never 0, which would silently never sell.
    async fn free_base_balance(&self, _ticker: &Ticker) -> Result<BigDec, VenueError> { Err(VenueError::Rejected(vec!["selling is not connected for this venue".into()])) }
    /// `free_base_balance` through the venue's asset identity; the engine always asks through this. Kraken and Binance name
    /// an asset by its own code and ignore `catalog`. Alpaca names a position by class + symbol, which only the catalog
    /// (Exchanges::Alpaca#live_ticker_index, built when asked) resolves: a symbol shared by a stock and a crypto, or by cash
    /// and a security, is never mistaken for the base.
    async fn free_base_identified<F>(&self, ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<Catalog, VenueError> { self.free_base_balance(ticker).await }
    /// The ticker's QUOTE as #get_balances reports it free, the cash Bot::Rebalancer counts (`free: account['cash']` on
    /// Alpaca: settled cash, never the borrowed buying power `balance` funds buys with; Kraken balance − hold_trade; Binance
    /// free). Absent is 0; unreadable is `Err`, never 0.
    async fn settled_quote_balance<F>(&self, _ticker: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<Catalog, VenueError> { Err(VenueError::Rejected(vec!["the settled quote balance is not connected for this venue".into()])) }
    /// Exchange#get_tickers_prices(symbols:): each priced code's last price as the venue's JSON holds it (null included),
    /// for Bot::Composition::Measurable#metrics_with_current_prices. A failed answer is `Rejected` (the figures go stale), a
    /// transport failure `Transient` (Rails raises it to the job's retry).
    async fn tickers_prices(&self, _symbols: &[String]) -> Result<Vec<(String, serde_json::Value)>, VenueError> { Err(VenueError::Rejected(vec!["bulk prices are not connected for this venue".into()])) }
    /// Exchange#cancel_order: best effort, as Bot::Reversible#cancel_unfilled_orders uses it. `Ok` only when the venue
    /// accepted the cancel; anything else is the venue's (or the transport's) failure.
    async fn cancel(&self, _txid: &str) -> Result<(), VenueError> { Err(VenueError::Rejected(vec!["cancel is not connected for this venue".into()])) }
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
