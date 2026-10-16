//! The real venues behind the engine (spec E1): one enum dispatching the Venue trait by the bot's exchange type (Rails'
//! Exchange subclasses), and the per-venue proxy (ExchangeProxy.for). A ported venue trades as in Rails: there is no
//! live gate.
use super::alpaca::AlpacaVenue;
use super::binance::BinanceVenue;
use super::http::{self, ReqwestTransport};
use super::kraken::KrakenVenue;
use super::{ClockAnswer, NewOrder, OrderState, PriceSide, Venue, VenueError};
use crate::crypto::Cipher;
use crate::engine::model::Ticker;
use crate::engine::venue_rules::VenueRules;
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use rusqlite::Connection;

/// A bot's venue, picked by its exchange type (no trait objects: the engine stays generic over one concrete type).
pub enum LiveVenue { Alpaca(AlpacaVenue<ReqwestTransport>), Kraken(KrakenVenue<ReqwestTransport>), Binance(BinanceVenue<ReqwestTransport>) }

macro_rules! each { ($s:expr, $v:ident => $e:expr) => { match $s { LiveVenue::Alpaca($v) => $e, LiveVenue::Kraken($v) => $e, LiveVenue::Binance($v) => $e } } }

impl Venue for LiveVenue {
    fn rules(&self) -> &'static VenueRules { each!(self, v => v.rules()) }
    fn placement_identity(&self) -> Option<serde_json::Value> { each!(self, v => v.placement_identity()) }
    async fn positions(&self) -> Result<std::collections::HashMap<String, BigDec>, VenueError> { each!(self, v => v.positions().await) }
    async fn clock(&self) -> Result<ClockAnswer, VenueError> { each!(self, v => v.clock().await) }
    async fn price(&self, t: &Ticker, s: PriceSide) -> Result<BigDec, VenueError> { each!(self, v => v.price(t, s).await) }
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> { each!(self, v => v.add_order(o).await) }
    async fn orders(&self, ids: &[String]) -> Result<Vec<OrderState>, VenueError> { each!(self, v => v.orders(ids).await) }
    async fn orders_identified<F>(&self, ids: &[String], identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> { each!(self, v => v.orders_identified(ids, identify).await) }
    async fn order_identified<F>(&self, id: &str, identify: F) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError> { each!(self, v => v.order_identified(id, identify).await) }
    async fn order_by_client_id(&self, id: &str, symbol: &str, since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> { each!(self, v => v.order_by_client_id(id, symbol, since).await) }
    async fn fills_from_trades(&self, ids: &[String], since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> { each!(self, v => v.fills_from_trades(ids, since).await) }
    async fn balance(&self, s: &str, crypto: bool) -> Result<BigDec, VenueError> { each!(self, v => v.balance(s, crypto).await) }
    async fn free_base_balance(&self, t: &Ticker) -> Result<BigDec, VenueError> { each!(self, v => v.free_base_balance(t).await) }
    async fn free_base_identified<F>(&self, t: &Ticker, catalog: F) -> Result<BigDec, VenueError>
    where F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError> { each!(self, v => v.free_base_identified(t, catalog).await) }
    async fn cancel(&self, txid: &str) -> Result<(), VenueError> { each!(self, v => v.cancel(txid).await) }
}

/// A venue's egress: its client and its proxy URL (redacted from diagnostics: it may carry credentials), or why nothing
/// may be sent.
pub type Egress = Result<(reqwest::Client, Option<String>), String>;

/// `venue`'s egress from this install's settings now (`name` lowercase, as ExchangeProxy is called: "kraken"), as Rails
/// builds each Honeymaker client with ExchangeProxy.for: through the proxy when one is configured, else `direct`.
pub fn egress(c: &Connection, cipher: &Cipher, env: &dyn Fn(&str) -> Option<String>, name: &str, direct: &reqwest::Client) -> Egress {
    let stored = crate::app_config::get(c, cipher, &format!("proxy_{name}")).map_err(|_| format!("the {name} proxy setting is unreadable"))?;
    match http::proxy_for(name, stored, env) {
        None => Ok((direct.clone(), None)),
        Some(proxy) => Ok((http::client_via(&proxy).map_err(|e| format!("{e} ({name})"))?, Some(proxy))),
    }
}
