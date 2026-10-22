//! Exchanges::Coinbase and honeymaker's Advanced Trade client. D-CB-PAGING exhausts batch pages;
//! placement keeps its intent unless the answer proves rejection. Recovery can find, never prove absence.
use super::binance::Pairs;
use super::http::{self, HttpRequest, Transport};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, COINBASE as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
pub const URL: &str = "https://api.coinbase.com";
pub const COINBASE: &str = "Exchanges::Coinbase";
pub const PERMISSIONS: &str = "/api/v3/brokerage/key_permissions";
pub const ORDERS: &str = "/api/v3/brokerage/orders/historical/batch";
pub const PLACE: &str = "/api/v3/brokerage/orders";
pub const UNREADABLE: &str = "Coinbase: unreadable response";
pub const ABSENCE_UNPROVEN: &str = "Coinbase: the client order lookup did not find this order, and Coinbase cannot prove it absent; settle it with `deltabadger resolve-placement`";
fn unreadable() -> VenueError {
    VenueError::Ambiguous(UNREADABLE.into())
}
fn dec(v: &Value) -> Result<BigDec, VenueError> {
    super::json::required(v, unreadable)
}
fn optional(v: &Value) -> Result<Option<BigDec>, VenueError> {
    if v.is_null() {
        Ok(None)
    } else {
        dec(v).map(Some)
    }
}
pub fn parse_order(id: &str, o: &Value) -> Result<OrderState, VenueError> {
    let limit = match o["order_type"].as_str() {
        Some("MARKET") => false,
        Some("LIMIT") => true,
        _ => return Err(unreadable()),
    };
    let config = &o["order_configuration"][if limit {
        "limit_limit_gtc"
    } else {
        "market_market_ioc"
    }];
    let mut price = dec(&o["average_filled_price"])?;
    if price.is_zero() && limit {
        price = dec(&config["limit_price"])?;
    }
    let status = match o["status"].as_str() {
        Some("PENDING" | "UNKNOWN_ORDER_STATUS") => OrderStatus::Unknown,
        Some("OPEN" | "QUEUED" | "CANCEL_QUEUED" | "EDIT_QUEUED") => OrderStatus::Open,
        Some("FILLED") => OrderStatus::Closed,
        Some("CANCELLED" | "EXPIRED") => OrderStatus::Cancelled,
        Some("FAILED") => OrderStatus::Failed,
        _ => return Err(unreadable()),
    };
    Ok(OrderState {
        txid: id.into(),
        status,
        price: (!price.is_zero()).then_some(price),
        amount: optional(&config["base_size"])?,
        quote_amount: optional(&config["quote_size"])?,
        amount_exec: dec(&o["filled_size"])?,
        quote_amount_exec: Some(
            &dec(&o["total_value_after_fees"])? - &dec(&o["outstanding_hold_amount"])?,
        ),
        limit,
        sell: o["side"] == "SELL",
        pair: Some(o["product_id"].as_str().ok_or_else(unreadable)?.into()),
        asset_class: None,
    })
}
#[derive(Clone)]
pub struct CoinbaseVenue<T: Transport> {
    transport: T,
    base: String,
    pairs: Pairs,
    identity: Option<Value>,
}
impl<T: Transport> CoinbaseVenue<T> {
    pub fn new(transport: T, base: &str) -> Self {
        Self {
            transport,
            base: base.into(),
            pairs: Pairs::default(),
            identity: None,
        }
    }
    pub fn with_pairs(mut self, pairs: Pairs) -> Self {
        self.pairs = pairs;
        self
    }
    pub fn with_identity(mut self, c: &crate::crypto::Credentials) -> Self {
        self.identity = Some(super::key_identity(&self.base, c));
        self
    }
    pub fn request(
        &self,
        method: &'static str,
        path: &str,
        query: Vec<(&'static str, String)>,
        body: Option<Value>,
    ) -> HttpRequest {
        HttpRequest {
            method,
            base: self.base.clone(),
            path: path.into(),
            query,
            body,
            not_after: None,
        }
    }
    pub async fn read_text(
        &self,
        path: &str,
        query: Vec<(&'static str, String)>,
        limit: usize,
    ) -> Result<String, String> {
        let r = self.request("GET", path, query, None);
        match self.transport.send_limited(&r, limit).await {
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) => Err(if resp.body.is_empty() {
                format!(
                    "the server responded with status {} for GET {}",
                    resp.status,
                    r.url()
                )
            } else {
                resp.body
            }),
            Err(e) => Err(self.transport.redact_diagnostic(&format_transport(e))),
        }
    }
    async fn call(&self, r: &HttpRequest, placement: bool) -> Result<Value, VenueError> {
        let resp = self.transport.send(r).await.map_err(|e| {
            http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m))
        })?;
        if !(200..300).contains(&resp.status) {
            let msg = if resp.body.is_empty() {
                format!(
                    "the server responded with status {} for {} {}",
                    resp.status,
                    r.method,
                    r.url()
                )
            } else {
                resp.body
            };
            return Err(
                if placement && (!(400..500).contains(&resp.status) || resp.status == 408) {
                    VenueError::Ambiguous(msg)
                } else if resp.status == 401 && !placement {
                    VenueError::Unauthorized(vec![msg])
                } else {
                    VenueError::Rejected(vec![msg])
                },
            );
        }
        http::decode_json(&resp.body).map_err(|_| unreadable())
    }
    pub async fn balances_text(&self) -> Result<String, String> {
        let body = self.read_text(PERMISSIONS, vec![], 64 * 1024).await?;
        let data: Value = serde_json::from_str(&body).map_err(|_| UNREADABLE.to_string())?;
        let uuid = data["portfolio_uuid"].as_str().unwrap_or("");
        self.read_text(
            &format!("/api/v3/brokerage/portfolios/{uuid}"),
            vec![],
            4 * 1024 * 1024,
        )
        .await
    }
    async fn free(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let permissions = self
            .call(&self.request("GET", PERMISSIONS, vec![], None), false)
            .await?;
        let path = format!(
            "/api/v3/brokerage/portfolios/{}",
            permissions["portfolio_uuid"].as_str().unwrap_or("")
        );
        let body = self
            .call(&self.request("GET", &path, vec![], None), false)
            .await?;
        let mut free = BigDec::zero();
        for p in body["breakdown"]["spot_positions"]
            .as_array()
            .ok_or_else(unreadable)?
        {
            if p["asset"].as_str().is_some_and(&pick) {
                free = dec(&p["available_to_trade_crypto"])?;
                dec(&p["total_balance_crypto"])?;
            }
        }
        Ok(free)
    }
    async fn pages(
        &self,
        params: Vec<(&'static str, String)>,
        wanted: &[String],
        client: Option<&str>,
    ) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        let mut cursor: Option<String> = None;
        let mut seen = std::collections::HashSet::new();
        loop {
            let mut q = params.clone();
            if let Some(c) = &cursor {
                q.push(("cursor", c.clone()));
            }
            let body = self
                .call(&self.request("GET", ORDERS, q, None), false)
                .await?;
            for o in body["orders"].as_array().ok_or_else(unreadable)? {
                let id = o["order_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(unreadable)?;
                if client.is_some_and(|c| o["client_order_id"] != c)
                    || (!wanted.is_empty() && !wanted.iter().any(|w| w == id))
                {
                    continue;
                }
                if !out.iter().any(|o: &OrderState| o.txid == id) {
                    out.push(parse_order(id, o)?);
                }
            }
            if (!wanted.is_empty() && wanted.iter().all(|id| out.iter().any(|o| &o.txid == id)))
                || (client.is_some() && !out.is_empty())
            {
                break;
            }
            if body["has_next"] != true {
                break;
            }
            let next = body["cursor"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(unreadable)?;
            if !seen.insert(next.to_string()) {
                return Err(unreadable());
            }
            cursor = Some(next.to_string());
        }
        Ok(out)
    }
}
fn format_transport(e: http::TransportError) -> String {
    match e {
        http::TransportError::Permanent(m)
        | http::TransportError::NotSent(m)
        | http::TransportError::MaybeSent(m) => m,
    }
}
impl<T: Transport> super::Venue for CoinbaseVenue<T> {
    fn rules(&self) -> &'static VenueRules {
        &RULES
    }
    fn placement_identity(&self) -> Option<Value> {
        self.identity.clone()
    }
    async fn price(&self, t: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let (price, label) = if side == PriceSide::Last {
            let b = self
                .call(
                    &self.request(
                        "GET",
                        &format!("/api/v3/brokerage/market/products/{}", t.ticker),
                        vec![],
                        None,
                    ),
                    false,
                )
                .await?;
            (dec(&b["price"])?, "last")
        } else {
            let b = self
                .call(
                    &self.request(
                        "GET",
                        "/api/v3/brokerage/market/product_book",
                        vec![("product_id", t.ticker.clone()), ("limit", "1".into())],
                        None,
                    ),
                    false,
                )
                .await?;
            let book = &b["pricebook"];
            let bid = dec(&book["bids"][0]["price"])?;
            dec(&book["bids"][0]["size"])?;
            let ask = dec(&book["asks"][0]["price"])?;
            dec(&book["asks"][0]["size"])?;
            if side == PriceSide::Bid {
                (bid, "bid")
            } else {
                (ask, "ask")
            }
        };
        if price.is_zero() {
            return Err(VenueError::Rejected(vec![format!(
                "Wrong {label} price for {}: {}",
                t.ticker,
                price.to_s_f()
            )]));
        }
        Ok(price)
    }
    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        // Coinbase App changelog 2025-08-14: client_order_id maximum 128 characters. The intent UUID fits unchanged.
        if o.cl_ord_id.chars().count() > 128 {
            return Err(VenueError::Rejected(vec![
                "Coinbase: client order id exceeds 128 characters; not sent".into(),
            ]));
        }
        let mut config = json!({if o.quote_volume{"quote_size"}else{"base_size"}:o.volume});
        let kind = match &o.kind {
            OrderKind::Market => "market_market_ioc",
            OrderKind::Limit { price } => {
                config["limit_price"] = json!(price);
                "limit_limit_gtc"
            }
            OrderKind::Crossed { .. } => return Err(super::not_crossing("Coinbase")),
        };
        let mut r=self.request("POST",PLACE,vec![],Some(json!({"client_order_id":o.cl_ord_id,"product_id":o.pair,"side":o.side.as_str().to_ascii_uppercase(),"order_configuration":{kind:config}})));
        r.not_after =
            Some(o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s"));
        let body = self.call(&r, true).await?;
        if body["success"] == false {
            let error = body["error_response"]["error"].as_str().unwrap_or("");
            let message = body["error_response"]["message"]
                .as_str()
                .unwrap_or("Order creation failed")
                .to_string();
            // Only explicit validation/funding refusals prove the matching engine rejected the order.
            if matches!(
                error,
                "INSUFFICIENT_FUND"
                    | "INVALID_PRODUCT_ID"
                    | "INVALID_SIZE_PRECISION"
                    | "INVALID_PRICE_PRECISION"
                    | "INVALID_ORDER_CONFIGURATION"
                    | "INVALID_BASE_SIZE"
                    | "INVALID_QUOTE_SIZE"
                    | "INVALID_LIMIT_PRICE"
                    | "MIN_SIZE_FAILURE"
                    | "MAX_SIZE_FAILURE"
                    | "PRODUCT_DISABLED"
                    | "PRODUCT_NOT_TRADABLE"
            ) {
                return Err(VenueError::Rejected(vec![message]));
            }
            return Err(VenueError::Ambiguous(message));
        }
        if body["success"] != true {
            return Err(unreadable());
        }
        body["success_response"]["order_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(unreadable)
    }
    async fn orders(&self, ids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let mut out = vec![];
        for chunk in ids.chunks(50) {
            out.extend(
                self.pages(
                    chunk.iter().map(|id| ("order_ids", id.clone())).collect(),
                    chunk,
                    None,
                )
                .await?,
            );
        }
        Ok(out)
    }
    async fn order_identified<F>(
        &self,
        id: &str,
        _identify: F,
    ) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where
        F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError>,
    {
        let b = self
            .call(
                &self.request(
                    "GET",
                    &format!("/api/v3/brokerage/orders/historical/{id}"),
                    vec![],
                    None,
                ),
                false,
            )
            .await?;
        Ok((vec![parse_order(id, b.get("order").unwrap_or(&b))?], vec![]))
    }
    async fn order_by_client_id(
        &self,
        id: &str,
        symbol: &str,
        since: DateTime<Utc>,
    ) -> Result<Option<OrderState>, VenueError> {
        let out = self
            .pages(
                vec![
                    ("product_ids", symbol.into()),
                    (
                        "start_date",
                        since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                    ),
                ],
                &[],
                Some(id),
            )
            .await?;
        match out.into_iter().find(|o| o.pair.as_deref() == Some(symbol)) {
            Some(o) => Ok(Some(o)),
            None => Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())),
        }
    }
    async fn fills_from_trades(
        &self,
        _ids: &[String],
        _since: DateTime<Utc>,
    ) -> Result<Vec<OrderState>, VenueError> {
        Ok(vec![])
    }
    async fn balance(&self, symbol: &str, _crypto: bool) -> Result<BigDec, VenueError> {
        self.free(|c| self.pairs.assets.get(c).map(String::as_str) == Some(symbol))
            .await
    }
    async fn free_base_balance(&self, t: &Ticker) -> Result<BigDec, VenueError> {
        self.free(|c| c == t.base_code).await
    }
    async fn settled_quote_balance<F>(&self, t: &Ticker, _catalog: F) -> Result<BigDec, VenueError>
    where
        F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError>,
    {
        self.free(|c| c == t.quote_code).await
    }
    async fn tickers_prices(
        &self,
        _symbols: &[String],
    ) -> Result<Vec<(String, Value)>, VenueError> {
        let b = self
            .call(
                &self.request("GET", "/api/v3/brokerage/market/products", vec![], None),
                false,
            )
            .await?;
        let mut out = vec![];
        for p in b["products"].as_array().ok_or_else(unreadable)? {
            let symbol = p["product_id"].as_str().ok_or_else(unreadable)?;
            if let Ok(price) = dec(&p["price"]) {
                out.push((symbol.into(), json!(price.to_s_f())));
            }
        }
        Ok(out)
    }
    async fn cancel(&self, id: &str) -> Result<(), VenueError> {
        let b = self
            .call(
                &self.request(
                    "POST",
                    "/api/v3/brokerage/orders/batch_cancel",
                    vec![],
                    Some(json!({"order_ids":[id]})),
                ),
                false,
            )
            .await?;
        if b["results"][0]["success"] == true {
            Ok(())
        } else {
            Err(VenueError::Rejected(vec![b["results"][0]
                ["failure_reason"]
                .as_str()
                .ok_or_else(unreadable)?
                .into()]))
        }
    }
}
