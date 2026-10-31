//! What the engine needs from an exchange. One instance per API key; the factory picks the venue by the bot's
//! exchange type. Tests use `fake::FakeVenue` (raw Kraken bodies) and, from Task 6, `alpaca::AlpacaVenue` over
//! `http::ScriptedTransport`.
#![allow(async_fn_in_trait)] // single-threaded runtime: no Send bound needed
pub mod fake;
pub mod alpaca;
pub mod http;
pub mod binance;
pub mod bitvavo;
pub mod bitget;
pub mod bybit;
pub mod kraken;
pub mod kucoin;
pub mod kucoin_account;
pub mod gemini;
pub mod hyperliquid;
pub mod ibkr;
pub mod ibkr_account;
pub mod ibkr_reservations;
pub mod ibkr_venue;
pub mod live;
pub mod mexc;
pub mod bitrue;
pub mod bingx;
pub mod registry;
pub mod withdrawal;
pub mod sign;
pub mod signers;
pub(crate) mod json;

use crate::crypto::Credentials;
use crate::engine::candles::{Candle, Timeframe};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::VenueRules;
use crate::ruby::BigDec;
use crate::sync::balances::Catalog;
use chrono::{DateTime, Utc};

/// Utilities::Hash.dig_or_raise for one key: an absent or null value is Rails' KeyError, "Key path <key> not found", which
/// nothing on a price read rescues (`Ambiguous`, as the adapters' unreadable answers).
pub(crate) fn dig_or_raise<'a>(v: &'a serde_json::Value, key: &str) -> Result<&'a serde_json::Value, VenueError> {
    v.get(key).filter(|x| !x.is_null()).ok_or_else(|| VenueError::Ambiguous(format!("Key path {key} not found")))
}

/// Exchange#get_orders' loop over #get_order (plan VB §1.5; MEXC's, and its family's): each id asked in turn, an order the
/// venue no longer knows (`Ok(None)`) listed under `missing`, any other failure failing the whole batch.
pub(crate) async fn poll_each<F, Fut>(ids: &[String], mut get_one: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
where F: FnMut(String) -> Fut, Fut: std::future::Future<Output = Result<Option<OrderState>, VenueError>> {
    let (mut orders, mut missing) = (vec![], vec![]);
    for id in ids {
        match get_one(id.clone()).await? { Some(o) => orders.push(o), None => missing.push(id.clone()) }
    }
    Ok((orders, missing))
}

/// What an intent records of the key a Kraken, Binance, KuCoin, Bitvavo or MEXC order is sent under (Alpaca's is `alpaca::placement_identity`):
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

/// What a venue without a cross answers to `OrderKind::Crossed`, which only a crossing venue's plan carries: nothing is sent.
pub(crate) fn not_crossing(name: &str) -> VenueError { VenueError::Transient(format!("{name} takes no crossed order; not sent")) }

#[derive(Clone, Debug, PartialEq)]
pub enum OrderKind {
    Market,
    Limit { price: String },
    /// A market order a crossing venue has no market type for (Gemini's IOC `exchange limit`, Hyperliquid's IOC limit),
    /// sent at its persisted crossed price (OrderPlan::wire_price, D-679). Hyperliquid keeps Rails' 1% cross;
    /// D-HYPERLIQUID-MARKET-IOC changes only time-in-force. Only a venue whose rules cross receives one.
    Crossed { price: String },
}

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
    /// D-KUCOIN-ZERO-FILL-FUNDS: the `createdAt` of a market order KuCoin ended with nothing dealt (`isActive` and
    /// `cancelExist` false, `dealSize` and `dealFunds` 0). Rails reads it `:unknown` forever; the status stays Unknown here
    /// too until `settled` finds it older than ZERO_FILL_SETTLE, then it is Closed with nothing filled: a buy is then KuCoin's
    /// out-of-funds answer and fails as its insufficient-funds rejection does (polling::apply_in); a sell stays Closed.
    pub zero_fill_at: Option<DateTime<Utc>>,
}

/// How old a zero-fill answer must be before it is final: an order polled before the venue processed it may read
/// inactive with nothing dealt; a market order settles within milliseconds of placement.
pub const ZERO_FILL_SETTLE: chrono::Duration = chrono::Duration::minutes(1);
/// What a settled zero-fill buy is failed with: KuCoin's insufficient-funds rejection of a placement, word for word as the
/// venue reports it (kucoin::api_error), so the row reads as that rejection's row (D-KUCOIN-ZERO-FILL-FUNDS).
pub const ZERO_FILL_REFUSAL: &str = "KuCoin API error 200004: Balance insufficient!";

impl OrderState {
    /// When a zero-fill answer still too young to settle becomes final (D-KUCOIN-ZERO-FILL-FUNDS).
    pub fn settles_at(&self) -> Option<DateTime<Utc>> {
        self.zero_fill_at.filter(|_| self.status == OrderStatus::Unknown).and_then(|at| at.checked_add_signed(ZERO_FILL_SETTLE))
    }
    /// The answer as the poll at `now` reads it: a zero-fill answer older than ZERO_FILL_SETTLE is Closed.
    pub fn settled(mut self, now: DateTime<Utc>) -> Self {
        if self.settles_at().is_some_and(|at| now > at) { self.status = OrderStatus::Closed; }
        self
    }
    /// Closed by `settled` with nothing filled.
    pub fn closed_empty(&self) -> bool { self.status == OrderStatus::Closed && self.zero_fill_at.is_some() }
}

#[derive(Clone, PartialEq)]
pub enum VenueError {
    /// The venue answered and refused (its own error text). Nothing was placed.
    Rejected(Vec<String>),
    /// A read the venue answered with HTTP 401, with the text `Rejected` would carry: Exchange#invalid_key_error?'s status
    /// signal (exchange.rb:304-341). Everything but the index's price probe (index::priced) reads it as `Rejected`
    /// (`rejection`); a placement's 401 stays `Rejected`.
    Unauthorized(Vec<String>),
    /// `Rejected`, from a placement whose answer also carried the venue's structured error code (MEXC and Binance `code`,
    /// Bybit `retCode`), kept apart from the text the row records: binance::error_message keeps only `msg`. Only an
    /// adapter's AddOrder produces it, and only the below-minimum classifier (placement::below_minimum) reads the code;
    /// everything else reads it as `Rejected`.
    Coded { code: String, errors: Vec<String> },
    /// The request may have reached the venue (lost reply, timeout after send, a 5xx or unreadable answer to a placement).
    Ambiguous(String),
    /// Nothing reached the venue (refused connection, DNS, TLS before send), or a read failed in transport.
    Transient(String),
}

impl VenueError {
    /// A diagnostic for an activity or preflight refusal, without Rust enum formatting.
    pub fn message(&self) -> String {
        match self {
            Self::Transient(message) | Self::Ambiguous(message) => message.clone(),
            Self::Rejected(errors) | Self::Unauthorized(errors) | Self::Coded { errors, .. } => crate::ruby::to_sentence(errors),
        }
    }
    /// The refusal's text, for `Rejected` and `Unauthorized` alike.
    pub fn rejection(&self) -> Option<&[String]> { match self { Self::Rejected(e) | Self::Unauthorized(e) | Self::Coded { errors: e, .. } => Some(e), _ => None } }
}

/// As derived, except that `Unauthorized` and `Coded` print as `Rejected`: a message built from `{:?}` reads as it did before the
/// status was carried.
impl std::fmt::Debug for VenueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(e) | Self::Unauthorized(e) | Self::Coded { errors: e, .. } => f.debug_tuple("Rejected").field(e).finish(),
            Self::Ambiguous(m) => f.debug_tuple("Ambiguous").field(m).finish(),
            Self::Transient(m) => f.debug_tuple("Transient").field(m).finish(),
        }
    }
}

/// What a venue that caps sells by its own reservations (IBKR, D-IBKR-SELLCAP) reads before an intent, for the intent's
/// transaction to check (`ibkr_reservations::reserve`): the account and contract the order goes to and, for a sell, the
/// position, read under the login's lock, which is held until this is dropped. Dropped after the intent commits and
/// before the send (the send takes the lock again).
pub struct PlacementHold {
    pub(crate) _login: Option<ibkr::Held<'static>>,
    pub account_ref: String,
    pub conid: i64,
    pub position: Option<BigDec>,
}

/// Credentials and their digest are captured once, before this handle is built.
/// The raw transport is private: production consumers use the attributed methods below.
pub struct Handle<V> { venue:V, producer:Option<crate::engine::model::CredentialVersion> }
impl<V:Venue> Handle<V> {
    pub(crate) fn new(venue:V,producer:Option<crate::engine::model::CredentialVersion>)->Self {
        let producer = venue.producer().or(producer);
        Self{venue,producer}
    }
    /// Tests only: a venue that carries its own producer (a KuCoin venue with account context), so no answer is attributed
    /// to anything but the credentials it captured.
    #[doc(hidden)]
    pub fn attributed(venue:V)->Self { assert!(venue.producer().is_some(), "a venue without its own producer"); Self::new(venue,None) }
    pub(crate) fn for_bot(venue:V,c:&rusqlite::Connection,bot:&crate::engine::model::Bot)->Result<Self,crate::engine::EngineError>{
        let producer=match venue.producer(){Some(origin)=>Some(origin),None=>crate::engine::model::credential_version(c,bot)?};
        Ok(Self::new(venue,producer))
    }
}
impl<V:Venue> Venue for &V {
    async fn prepare(&self)->Result<Option<crate::engine::model::CredentialVersion>,VenueError>{(*self).prepare().await}
    async fn validate_preparation(&self,identity:Option<&serde_json::Value>)->Result<(),VenueError>{(*self).validate_preparation(identity).await}
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{(*self).producer()}
    fn placement_identity(&self)->Option<serde_json::Value>{(*self).placement_identity()}
    async fn placement_hold(&self,t:&Ticker,s:Side)->Result<Option<PlacementHold>,VenueError>{(*self).placement_hold(t,s).await}
    fn rules(&self)->&'static VenueRules{(*self).rules()}
    async fn positions(&self)->Result<std::collections::HashMap<String,BigDec>,VenueError>{(*self).positions().await}
    async fn clock(&self)->Result<ClockAnswer,VenueError>{(*self).clock().await}
    async fn price(&self,t:&Ticker,s:PriceSide)->Result<BigDec,VenueError>{(*self).price(t,s).await}
    async fn add_order(&self,o:&NewOrder)->Result<String,VenueError>{(*self).add_order(o).await}
    async fn orders(&self,ids:&[String])->Result<Vec<OrderState>,VenueError>{(*self).orders(ids).await}
    async fn orders_identified<F>(&self,ids:&[String],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{(*self).orders_identified(ids,identify).await}
    async fn order_identified<F>(&self,id:&str,identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{(*self).order_identified(id,identify).await}
    async fn order_by_client_id(&self,id:&str,symbol:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{(*self).order_by_client_id(id,symbol,since).await}
    async fn recovery_with(&self,intent:&serde_json::Value,id:&str,symbol:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{(*self).recovery_with(intent,id,symbol,since).await}
    async fn fills_from_trades(&self,ids:&[String],since:DateTime<Utc>)->Result<Vec<OrderState>,VenueError>{(*self).fills_from_trades(ids,since).await}
    async fn balance(&self,symbol:&str,crypto:bool)->Result<BigDec,VenueError>{(*self).balance(symbol,crypto).await}
    async fn free_base_balance(&self,t:&Ticker)->Result<BigDec,VenueError>{(*self).free_base_balance(t).await}
    async fn free_base_identified<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{(*self).free_base_identified(t,catalog).await}
    async fn settled_quote_balance<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{(*self).settled_quote_balance(t,catalog).await}
    async fn tickers_prices(&self,symbols:&[String])->Result<Vec<(String,serde_json::Value)>,VenueError>{(*self).tickers_prices(symbols).await}
    async fn cancel(&self,txid:&str)->Result<(),VenueError>{(*self).cancel(txid).await}
    async fn orders_with<F>(&self,refs:&[OrderRef],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{(*self).orders_with(refs,identify).await}
    async fn order_with<F>(&self,r:&OrderRef,identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{(*self).order_with(r,identify).await}
    async fn cancel_with(&self,r:&OrderRef)->Result<(),VenueError>{(*self).cancel_with(r).await}
    async fn origin_with(&self,r:&OrderRef)->Result<Origin,VenueError>{(*self).origin_with(r).await}
    async fn candles(&self,t:&Ticker,start_at:DateTime<Utc>,tf:Timeframe)->Result<Vec<Candle>,VenueError>{(*self).candles(t,start_at,tf).await}
    async fn withdrawal_addresses(&self,coin:&str)->Result<Vec<withdrawal::Entry>,VenueError>{(*self).withdrawal_addresses(coin).await}
    async fn withdrawal_networks(&self,coin:&str)->Result<Option<withdrawal::Coin>,VenueError>{(*self).withdrawal_networks(coin).await}
    async fn withdrawal_fee(&self,d:&withdrawal::Destination,amount:&BigDec)->Result<Option<BigDec>,VenueError>{(*self).withdrawal_fee(d,amount).await}
    async fn withdraw(&self,r:&crate::engine::withdrawal::Request)->crate::engine::withdrawal::SendResult{(*self).withdraw(r).await}
    async fn withdrawal_lookup(&self,r:&crate::engine::withdrawal::Request)->crate::engine::withdrawal::LookupResult{(*self).withdrawal_lookup(r).await}
}
impl<V:Venue> Venue for Handle<V> {
    async fn prepare(&self)->Result<Option<crate::engine::model::CredentialVersion>,VenueError>{self.venue.prepare().await}
    async fn validate_preparation(&self,identity:Option<&serde_json::Value>)->Result<(),VenueError>{self.venue.validate_preparation(identity).await}
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{self.producer.clone()}
    fn placement_identity(&self)->Option<serde_json::Value>{self.venue.placement_identity()}
    async fn placement_hold(&self,t:&Ticker,s:Side)->Result<Option<PlacementHold>,VenueError>{self.venue.placement_hold(t,s).await}
    fn rules(&self)->&'static VenueRules{self.venue.rules()}
    async fn positions(&self)->Result<std::collections::HashMap<String,BigDec>,VenueError>{self.venue.positions().await}
    async fn clock(&self)->Result<ClockAnswer,VenueError>{self.venue.clock().await}
    async fn price(&self,t:&Ticker,s:PriceSide)->Result<BigDec,VenueError>{self.venue.price(t,s).await}
    async fn add_order(&self,o:&NewOrder)->Result<String,VenueError>{self.venue.add_order(o).await}
    async fn orders(&self,ids:&[String])->Result<Vec<OrderState>,VenueError>{self.venue.orders(ids).await}
    async fn orders_identified<F>(&self,ids:&[String],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.venue.orders_identified(ids,identify).await}
    async fn order_identified<F>(&self,id:&str,identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.venue.order_identified(id,identify).await}
    async fn order_by_client_id(&self,id:&str,symbol:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{self.venue.order_by_client_id(id,symbol,since).await}
    async fn recovery_with(&self,intent:&serde_json::Value,id:&str,symbol:&str,since:DateTime<Utc>)->Result<Option<OrderState>,VenueError>{self.venue.recovery_with(intent,id,symbol,since).await}
    async fn fills_from_trades(&self,ids:&[String],since:DateTime<Utc>)->Result<Vec<OrderState>,VenueError>{self.venue.fills_from_trades(ids,since).await}
    async fn balance(&self,symbol:&str,crypto:bool)->Result<BigDec,VenueError>{self.venue.balance(symbol,crypto).await}
    async fn free_base_balance(&self,t:&Ticker)->Result<BigDec,VenueError>{self.venue.free_base_balance(t).await}
    async fn free_base_identified<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{self.venue.free_base_identified(t,catalog).await}
    async fn settled_quote_balance<F>(&self,t:&Ticker,catalog:F)->Result<BigDec,VenueError> where F:FnOnce()->Result<Catalog,VenueError>{self.venue.settled_quote_balance(t,catalog).await}
    async fn tickers_prices(&self,symbols:&[String])->Result<Vec<(String,serde_json::Value)>,VenueError>{self.venue.tickers_prices(symbols).await}
    async fn cancel(&self,txid:&str)->Result<(),VenueError>{self.venue.cancel(txid).await}
    async fn orders_with<F>(&self,refs:&[OrderRef],identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.venue.orders_with(refs,identify).await}
    async fn order_with<F>(&self,r:&OrderRef,identify:F)->Result<(Vec<OrderState>,Vec<String>),VenueError> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{self.venue.order_with(r,identify).await}
    async fn cancel_with(&self,r:&OrderRef)->Result<(),VenueError>{self.venue.cancel_with(r).await}
    async fn origin_with(&self,r:&OrderRef)->Result<Origin,VenueError>{self.venue.origin_with(r).await}
    async fn candles(&self,t:&Ticker,start_at:DateTime<Utc>,tf:Timeframe)->Result<Vec<Candle>,VenueError>{self.venue.candles(t,start_at,tf).await}
    async fn withdrawal_addresses(&self,coin:&str)->Result<Vec<withdrawal::Entry>,VenueError>{self.venue.withdrawal_addresses(coin).await}
    async fn withdrawal_networks(&self,coin:&str)->Result<Option<withdrawal::Coin>,VenueError>{self.venue.withdrawal_networks(coin).await}
    async fn withdrawal_fee(&self,d:&withdrawal::Destination,amount:&BigDec)->Result<Option<BigDec>,VenueError>{self.venue.withdrawal_fee(d,amount).await}
    async fn withdraw(&self,r:&crate::engine::withdrawal::Request)->crate::engine::withdrawal::SendResult{self.venue.withdraw(r).await}
    async fn withdrawal_lookup(&self,r:&crate::engine::withdrawal::Request)->crate::engine::withdrawal::LookupResult{self.venue.withdrawal_lookup(r).await}
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
    async fn orders_result<F>(&self,refs:&[OrderRef],identify:F)->crate::engine::model::Produced<Result<(Vec<OrderState>,Vec<String>),VenueError>> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{let origin=self.producer();let value=self.orders_with(refs,identify).await;crate::engine::model::Produced::new(value,origin)}
    async fn order_result<F>(&self,r:&OrderRef,identify:F)->crate::engine::model::Produced<Result<(Vec<OrderState>,Vec<String>),VenueError>> where F:FnMut(&str,Option<&str>,Option<&str>)->Result<bool,VenueError>{let origin=self.producer();let value=self.order_with(r,identify).await;crate::engine::model::Produced::new(value,origin)}
    async fn fills_result(&self,ids:&[String],since:DateTime<Utc>)->crate::engine::model::Produced<Result<Vec<OrderState>,VenueError>>{let origin=self.producer();let value=self.fills_from_trades(ids,since).await;crate::engine::model::Produced::new(value,origin)}
    async fn placement_result(&self,o:&NewOrder)->crate::engine::model::Produced<Result<String,VenueError>>{let origin=self.producer();let value=self.add_order(o).await;crate::engine::model::Produced::new(value,origin)}
    async fn recovery_result(&self,intent:&serde_json::Value,id:&str,symbol:&str,since:DateTime<Utc>)->crate::engine::model::Produced<Result<Option<OrderState>,VenueError>>{let origin=self.producer();let value=self.recovery_with(intent,id,symbol,since).await;crate::engine::model::Produced::new(value,origin)}
    async fn candles_result(&self,t:&Ticker,start_at:DateTime<Utc>,tf:Timeframe)->crate::engine::model::Produced<Result<Vec<Candle>,VenueError>>{let origin=self.producer();let value=self.candles(t,start_at,tf).await;crate::engine::model::Produced::new(value,origin)}
}
impl<V:Venue> Attributed for Handle<V> {}
/// The family an adopted row's record names until its route is proven.
pub const ADOPTED: &str = "Adopted";

/// D-KUCOIN-HF-RETENTION: what a positive answer proved of an adopted row's route: classic, or HF (its record to write).
#[derive(Clone, Debug, PartialEq)]
pub enum Origin { Classic, Hf(serde_json::Value), Unproven }

/// An order a row holds, with the record its venue wrote beside the row when it was accepted (D-KUCOIN-HF-ORDERS: a
/// KuCoin HF order's route, symbol, side, type and key; None for every other row).
#[derive(Clone, Debug, PartialEq)]
pub struct OrderRef { pub external_id: String, pub record: Option<serde_json::Value> }
impl OrderRef {
    /// The order of row `tx_id`, with its record.
    pub fn load(c: &rusqlite::Connection, tx_id: i64, external_id: &str) -> Result<Self, crate::engine::EngineError> {
        let record = kucoin::order_record(c, tx_id).map_err(crate::engine::EngineError::Data)?;
        if record.is_some() { return Ok(Self { external_id: external_id.into(), record }); }
        // D-KUCOIN-HF-RETENTION: a row adopted with no provenance (reconcile::ORIGIN unproven) is named by its own pair on
        // its venue (the ticker of its exchange and asset ids; none when that is not exactly one) and its side and type.
        let unproven = crate::app_config::get_plain(c, &format!("{}{tx_id}", crate::engine::reconcile::ORIGIN)).map_err(crate::engine::EngineError::Data)?
            .is_some_and(|v| v.contains("\"unproven\""));
        if !unproven { return Ok(Self { external_id: external_id.into(), record: None }); }
        let (symbol, sell, limit): (Option<String>, bool, bool) = c.query_row(
            "SELECT (SELECT CASE WHEN count(*) = 1 THEN max(k.ticker) END FROM tickers k WHERE k.exchange_id = t.exchange_id AND k.base_asset_id = t.base_asset_id \
             AND k.quote_asset_id = t.quote_asset_id), coalesce(t.side, 0) = 1, coalesce(t.order_type, 0) = 1 FROM transactions t WHERE t.id = ?1",
            [tx_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(Self { external_id: external_id.into(), record: Some(serde_json::json!({ "family": ADOPTED, "symbol": symbol, "sell": sell, "limit": limit })) })
    }
    /// D-KUCOIN-HF-RETENTION: a row adopted with no provenance, its route not proven yet (`Venue::origin_with`).
    pub fn adopted(&self) -> bool { self.record.as_ref().is_some_and(|r| r["family"] == ADOPTED) }
    /// D-KUCOIN-HF-RETENTION: a KuCoin HF or UTA row (its record's family), whose unread state is an obligation.
    pub fn routed(&self) -> bool { self.record.as_ref().is_some_and(kucoin::routed) }
}

pub trait Venue {
    /// Resolve immutable account context before sizing. Most venues need no preparation.
    async fn prepare(&self)->Result<Option<crate::engine::model::CredentialVersion>,VenueError>{Ok(None)}
    /// Refresh expired evidence before the dispatch credential fence; never change the intent's family.
    async fn validate_preparation(&self,_identity:Option<&serde_json::Value>)->Result<(),VenueError>{Ok(())}
    fn producer(&self)->Option<crate::engine::model::CredentialVersion>{None}
    /// The key an order is sent under, as an intent records it and recovery compares it (`alpaca::placement_identity`,
    /// `key_identity`): an object of fields merged into the intent. None where a venue has nothing to tell (no key, a
    /// scripted venue).
    fn placement_identity(&self)->Option<serde_json::Value>{None}
    /// What the intent of an order on `ticker` checks under the venue's lock (`PlacementHold`); None where nothing is.
    async fn placement_hold(&self, _ticker: &Ticker, _side: Side) -> Result<Option<PlacementHold>, VenueError> { Ok(None) }
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
    /// `orders_identified` with what each row recorded when it was accepted (`OrderRef::record`): every venue but KuCoin
    /// reads the ids alone (D-KUCOIN-HF-ORDERS).
    async fn orders_with<F>(&self, refs: &[OrderRef], identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> {
        self.orders_identified(&refs.iter().map(|r| r.external_id.clone()).collect::<Vec<_>>(), identify).await
    }
    /// `order_identified` with the row's record, as `orders_with`.
    async fn order_with<F>(&self, r: &OrderRef, identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> { self.order_identified(&r.external_id, identify).await }
    /// `cancel` with the row's record, as `orders_with`.
    async fn cancel_with(&self, r: &OrderRef) -> Result<(), VenueError> { self.cancel(&r.external_id).await }
    /// D-KUCOIN-HF-RETENTION: the route of an adopted row (`OrderRef::adopted`), proven only by a positive answer naming it.
    /// Only KuCoin rows are adopted so; any other venue proves nothing.
    async fn origin_with(&self, _r: &OrderRef) -> Result<Origin, VenueError> { Ok(Origin::Unproven) }
    /// The order with this client order id, if the venue holds one. `Err` unless the answer is complete. `symbol` is the pair
    /// the intent was sent for (Binance looks an order up per symbol; Kraken and Alpaca ignore it), never the bot's current one.
    async fn order_by_client_id(&self, cl_ord_id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError>;
    /// `order_by_client_id` for the unresolved `intent` as it was recorded: every venue but KuCoin asks as above; KuCoin
    /// asks the intent's own family's route, whatever the account's mode is now (D-KUCOIN-HF-RETENTION).
    async fn recovery_with(&self, _intent: &serde_json::Value, cl_ord_id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        self.order_by_client_id(cl_ord_id, symbol, since).await
    }
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
    /// Exchange#get_indicator_candles: the ticker's candles from `start_at` on (engine::candles), the still-forming one kept.
    async fn candles(&self, _ticker: &Ticker, _start_at: DateTime<Utc>, _timeframe: Timeframe) -> Result<Vec<Candle>, VenueError> {
        Err(VenueError::Rejected(vec!["candles are not connected for this venue".into()]))
    }
    /// The venue's withdrawal address book for `coin` (its own code), every entry with why the contract refuses it, if
    /// it does (plan V1, `withdrawal`). The rule's withdrawal key reads it.
    async fn withdrawal_addresses(&self, _coin: &str) -> Result<Vec<withdrawal::Entry>, VenueError> { Err(withdrawal::not_connected()) }
    /// capital/config/getall's networks for `coin` (Binance, MEXC); None when the coin is not listed.
    async fn withdrawal_networks(&self, _coin: &str) -> Result<Option<withdrawal::Coin>, VenueError> { Err(withdrawal::not_connected()) }
    /// Send only the immutable request committed by the withdrawal engine.
    async fn withdraw(&self, _request: &crate::engine::withdrawal::Request) -> crate::engine::withdrawal::SendResult {
        crate::engine::withdrawal::SendResult::Ambiguous { reason: "withdrawal adapter is not enabled".into() }
    }
    /// Exact matches may settle; candidate-only venues must always return Unknown.
    async fn withdrawal_lookup(&self, _request: &crate::engine::withdrawal::Request) -> crate::engine::withdrawal::LookupResult {
        crate::engine::withdrawal::LookupResult::Unknown { reason: "withdrawal lookup requires operator evidence".into(), candidates: vec![] }
    }
    /// The fee for sending `amount` to `destination` (§6): None when it is unknown.
    async fn withdrawal_fee(&self, _destination: &withdrawal::Destination, _amount: &BigDec) -> Result<Option<BigDec>, VenueError> { Err(withdrawal::not_connected()) }
}

pub trait VenueFactory {
    type V: Venue;
    /// `exchange_type` is the bot's exchanges.type. `None` credentials is Rails' unsaved fallback key: private calls
    /// fail at the venue, per bot.
    fn for_bot(&self, exchange_type: &str, credentials: Option<Credentials>) -> Self::V;
    /// Full stored material is used only to redact diagnostics, never venue financial data.
    fn for_bot_with_redaction(&self,exchange_type:&str,credentials:Option<Credentials>,_sensitive:Vec<String>)->Self::V{self.for_bot(exchange_type,credentials)}
    fn for_kucoin(&self,credentials:Option<Credentials>,sensitive:Vec<String>,_version:Option<crate::engine::model::CredentialVersion>)->Self::V{
        self.for_bot_with_redaction(kucoin::KUCOIN,credentials,sensitive)
    }
    /// An IBKR bot's venue: its key's login (by `version`) and `account`, the (account id, account_ref) the bot reaches IBKR
    /// as once `ibkr_account::check_bot_account` passes, else why not (every call is then refused).
    fn for_ibkr(&self,credentials:Option<Credentials>,sensitive:Vec<String>,_version:Option<crate::engine::model::CredentialVersion>,_account:Result<(String,String),String>)->Self::V{
        self.for_bot_with_redaction(ibkr::IBKR,credentials,sensitive)
    }
}

/// GET /v2/clock as Clients::Alpaca#get_clock returns it: the 2xx body as text, or the failure with its HTTP status (None for
/// a TLS failure, which Client.network_failure returns as a Failure) and with_rescue's message.
#[derive(Clone, Debug, PartialEq)]
pub enum ClockAnswer { Body(String), Failed { status: Option<u16>, message: String } }

pub mod coinbase;
/// A reversible UUID representation bounded to 32 characters, shared by query-HMAC venues.
pub(crate) fn client_order_id_32(id: &str, venue: &str) -> Result<String, VenueError> {
    match uuid::Uuid::parse_str(id) {
        Ok(uuid) => Ok(uuid.simple().to_string()),
        Err(_) if id.chars().count() <= 32 => Ok(id.to_string()),
        Err(_) => Err(VenueError::Rejected(vec![format!("{venue}: non-UUID client order id exceeds 32 characters; not sent")])),
    }
}
