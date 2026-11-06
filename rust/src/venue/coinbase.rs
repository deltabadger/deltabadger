//! Exchanges::Coinbase and honeymaker's Advanced Trade client. D-CB-PAGING exhausts batch pages;
//! placement keeps its intent unless the answer proves rejection. Recovery can find, never prove absence.
use super::binance::Pairs;
use super::http::{self, HttpRequest, Transport};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::candles::{self, Candle, Timeframe};
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
    Ok(OrderState { zero_fill_at: None,
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
/// A28-3d-3: the single order (`order_identified`'s path, the id behind it), the window list's page and page cap.
pub const ORDER: &str = "/api/v3/brokerage/orders/historical/";
pub const HISTORY_PAGE: usize = 100;
pub const HISTORY_PAGES: usize = 5;
/// A28-3d-3 (D-HISTORY-DOC-BUILT, D-HISTORY-COINBASE-QUOTE-AFTER-FEES): one historical order read strictly in `parse_order`'s
/// fields: its own `product_id` the asked one; `order_id` a non-empty string (the bare id `add_order` stores, as fills.rs keys
/// Coinbase fills); `created_time` RFC3339, kept to the microsecond (what A28-2's facts store); `side` BUY/SELL; LIMIT with
/// only `limit_limit_gtc` {`limit_price` > 0, `base_size` > 0, no quote size}, or MARKET with only `market_market_ioc` and
/// exactly one of `base_size` / `quote_size` positive (Rails places only these two); `filled_size` not negative; the filled
/// quote `total_value_after_fees` − `outstanding_hold_amount` as Rails' parse computes it, not negative. Status: OPEN /
/// QUEUED / CANCEL_QUEUED / EDIT_QUEUED open, FILLED (something filled) filled, CANCELLED / EXPIRED / FAILED ended; PENDING /
/// UNKNOWN_ORDER_STATUS (Unknown in Rails' map) and anything else None.
pub fn created_order(symbol: &str, o: &Value) -> Option<super::CreatedOrder> {
    use super::{CreatedKind, CreatedOrder, CreatedSize, CreatedState, Side};
    use chrono::Timelike;
    let zero = BigDec::zero();
    let amount = |v: &Value| crate::ruby::json_to_d(v).ok().flatten().filter(|d| *d >= zero);
    // Absent or null is none; present, a decimal not below zero, positive or none.
    let size = |c: &Value, k: &str| match c.get(k) { None | Some(Value::Null) => Some(None), Some(v) => amount(v).map(|d| d.is_positive().then_some(d)) };
    if o.get("product_id")?.as_str()? != symbol { return None; }
    let id = o.get("order_id")?.as_str().filter(|s| !s.is_empty())?;
    let t = DateTime::parse_from_rfc3339(o.get("created_time")?.as_str()?).ok()?.with_timezone(&Utc);
    let created_at = t.with_nanosecond(t.nanosecond() / 1_000 * 1_000)?;
    let side = match o.get("side")?.as_str()? { "BUY" => Side::Buy, "SELL" => Side::Sell, _ => return None };
    let config = o.get("order_configuration")?.as_object()?;
    let (kind, size) = match (o.get("order_type")?.as_str()?, config.len()) {
        ("LIMIT", 1) => {
            let c = config.get("limit_limit_gtc")?;
            let price = c.get("limit_price").and_then(amount).filter(BigDec::is_positive)?;
            match (size(c, "base_size")?, size(c, "quote_size")?) { (Some(base), None) => (CreatedKind::Limit { price }, CreatedSize::Base(base)), _ => return None }
        }
        ("MARKET", 1) => {
            let c = config.get("market_market_ioc")?;
            match (size(c, "base_size")?, size(c, "quote_size")?) {
                (Some(base), None) => (CreatedKind::Market, CreatedSize::Base(base)),
                (None, Some(quote)) => (CreatedKind::Market, CreatedSize::Quote(quote)),
                _ => return None,
            }
        }
        _ => return None,
    };
    let filled_base = o.get("filled_size").and_then(amount)?;
    let total = o.get("total_value_after_fees").and_then(|v| crate::ruby::json_to_d(v).ok().flatten())?;
    let hold = o.get("outstanding_hold_amount").and_then(|v| crate::ruby::json_to_d(v).ok().flatten())?;
    let filled_quote = Some(&total - &hold).filter(|q| *q >= zero)?;
    let filled = filled_base.is_positive();
    let state = match o.get("status")?.as_str()? {
        "OPEN" | "QUEUED" | "CANCEL_QUEUED" | "EDIT_QUEUED" => CreatedState::of(true, filled),
        "FILLED" if filled => CreatedState::Filled,
        "CANCELLED" | "EXPIRED" | "FAILED" => CreatedState::of(false, filled),
        _ => return None,
    };
    Some(CreatedOrder { external_id: id.into(), symbol: symbol.into(), created_at, side, kind, size, state, filled_base, filled_quote: Some(filled_quote) })
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
    /// A28-3d-3: one window page's rows and, while `has_next`, its cursor ("" when it names none). A failure is the key's
    /// refusal on HTTP 401/403 only (r1.2: the documented error envelope gives no symbolic code to rely on), else retryable.
    async fn history_page(&self, r: &HttpRequest) -> Result<(Vec<Value>, Option<String>), super::HistoryError> {
        let resp = super::binance::history_response(&self.transport, r).await?;
        let body = super::binance::history_body(&self.transport, resp, &[], UNREADABLE)?;
        let unreadable = || super::HistoryError::Retry(UNREADABLE.into());
        let rows = body.get("orders").and_then(Value::as_array).ok_or_else(unreadable)?.clone();
        let next = match body.get("has_next").and_then(Value::as_bool).ok_or_else(unreadable)? {
            true => Some(body.get("cursor").and_then(Value::as_str).unwrap_or("").to_string()),
            false => None,
        };
        Ok((rows, next))
    }
}
fn format_transport(e: http::TransportError) -> String {
    match e {
        http::TransportError::Permanent(m)
        | http::TransportError::NotSent(m)
        | http::TransportError::MaybeSent(m) => m,
    }
}
/// Exchanges::Coinbase#get_tickers_prices' path (Clients::Coinbase#list_public_products, no parameters).
pub const PRODUCTS: &str = "/api/v3/brokerage/market/products";
/// The prices of a market/products answer as Exchanges::Coinbase#get_tickers_prices reads them: every product's
/// `product_id` and `price`; a price that is no decimal is left out. Shared by the engine (`tickers_prices`) and the figures.
pub fn ticker_prices(b: &Value) -> Result<Vec<(String, BigDec)>, VenueError> {
    let mut out = vec![];
    for p in b["products"].as_array().ok_or_else(unreadable)? {
        let symbol = p["product_id"].as_str().ok_or_else(unreadable)?;
        if let Ok(price) = dec(&p["price"]) { out.push((symbol.into(), price)); }
    }
    Ok(out)
}
/// Exchanges::Coinbase#get_candles' granularity for each timeframe and its length in seconds: 4 hours are two-hourly,
/// 3 days, 1 week and 1 month daily, built.
pub fn candle_granularity(timeframe: Timeframe) -> (&'static str, i64) {
    match timeframe {
        Timeframe::Minute => ("ONE_MINUTE", 60), Timeframe::FiveMinutes => ("FIVE_MINUTE", 300), Timeframe::FifteenMinutes => ("FIFTEEN_MINUTE", 900),
        Timeframe::ThirtyMinutes => ("THIRTY_MINUTE", 1800), Timeframe::Hour => ("ONE_HOUR", 3600), Timeframe::FourHours => ("TWO_HOUR", 7200),
        Timeframe::Day | Timeframe::ThreeDays | Timeframe::Week | Timeframe::Month => ("ONE_DAY", 86_400),
    }
}

impl<T: Transport> super::Venue for CoinbaseVenue<T> {
    /// Exchanges::Coinbase#get_candles: windows of 350 periods (`start`/`end` in whole seconds), each ending at now at the
    /// latest and sorted by time; the walk ends at the window reaching now, the next one starting a second after the
    /// latest candle so far (at the window's end while there is none). Each `{start, open, high, low, close, volume}` at
    /// `start.to_i`; 4 hours built from two-hourly, 3 days, 1 week and 1 month from daily. DIVERGES
    /// (D-COND-CANDLE-COINBASE-EMPTY-WINDOW): an empty window after one with candles moves on from its end, where Rails
    /// asks the same window again forever; D-COND-CANDLE-CAP: past MAX_CANDLE_PAGES windows it is a settled failure.
    async fn candles(&self, ticker: &Ticker, start_at: DateTime<Utc>, timeframe: Timeframe) -> Result<Vec<Candle>, VenueError> {
        let (granularity, seconds) = candle_granularity(timeframe);
        let path = format!("/api/v3/brokerage/market/products/{}/candles", ticker.ticker);
        let (mut start, mut out) = (start_at, Vec::<Candle>::new());
        for _ in 0..candles::MAX_CANDLE_PAGES {
            let now = self.transport.now();
            let end = (start + chrono::Duration::seconds(350 * seconds)).min(now);
            // In Faraday's (sorted) order: a failure's text names the URL.
            let r = self.request("GET", &path, vec![("end", end.timestamp().to_string()), ("granularity", granularity.into()), ("start", start.timestamp().to_string())], None);
            let body = candles::get(&self.transport, &r, str::to_string, None, unreadable).await?;
            let mut page = body.as_object().and_then(|b| b.get("candles")).and_then(Value::as_array).ok_or_else(unreadable)?.iter().map(|row| {
                let at = row.get("start").and_then(candles::to_i).and_then(|s| DateTime::from_timestamp(s, 0));
                at.and_then(|at| candles::fields(at, row, ["open", "high", "low", "close", "volume"])).ok_or_else(unreadable)
            }).collect::<Result<Vec<_>, _>>()?;
            // ponytail: sorted by time; Rails sorts by the raw `start`, which differs only for times of different lengths.
            page.sort_by_key(|c| c.at);
            let latest = page.last().map(|c| c.at);
            out.extend(page);
            if end == now {
                return Ok(if matches!(timeframe, Timeframe::FourHours | Timeframe::ThreeDays | Timeframe::Week | Timeframe::Month) { candles::build(&out, timeframe) } else { out });
            }
            start = latest.map_or(end, |at| at + chrono::Duration::seconds(1));
        }
        Err(candles::too_many_pages("Coinbase"))
    }

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
    /// A28-3d-3 (D-HISTORY-DOC-BUILT: `end_date` and reading a window's every row are the docs'; Rails asks this list by ids):
    /// per product, GET historical/batch with `product_ids`, `start_date` = from floored to the second and `end_date` = to
    /// floored + 1 s (r1.1: the end is exclusive; RFC3339 whole seconds), limit 100, then `cursor` while `has_next`, five
    /// pages at most (a cap, or a missing or repeated cursor: incomplete). No status filter (open orders are listed) and no
    /// placement source (the default is an API key's, as Rails placed). Rows filtered here.
    async fn orders_created(&self, symbols: &[String], from: DateTime<Utc>, to: DateTime<Utc>, deadline: tokio::time::Instant) -> Result<super::Created, super::HistoryError> {
        let secs = |s: i64| DateTime::from_timestamp(s, 0).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)).unwrap_or_default();
        let (start, end) = (secs(from.timestamp()), secs(to.timestamp() + 1));
        let mut out = super::Created { orders: vec![], complete: true };
        let mut twice = vec![];
        for symbol in symbols {
            let mut cursors: Vec<String> = vec![];
            for n in 1..=HISTORY_PAGES {
                let mut q = vec![("product_ids", symbol.clone()), ("start_date", start.clone()), ("end_date", end.clone()), ("limit", HISTORY_PAGE.to_string())];
                q.extend(cursors.last().map(|c| ("cursor", c.clone())));
                let r = self.request("GET", ORDERS, q, None);
                let (rows, next) = match super::by_deadline(deadline, self.history_page(&r)).await {
                    Ok(page) => page,
                    Err(e) if !super::binance::any_in(&out, from, to) => return Err(e),
                    Err(_) => { out.complete = false; break; }
                };
                super::binance::read_rows(&mut out, &mut twice, &rows, |r| created_order(symbol, r));
                let Some(next) = next else { break };
                if n == HISTORY_PAGES || next.is_empty() || cursors.contains(&next) { out.complete = false; break; }
                cursors.push(next);
            }
        }
        Ok(super::binance::in_window(out, from, to))
    }
    /// A28-3d-3 (D-HISTORY-DOC-BOOK-CONFIRMED): `order_identified`'s GET historical/{id}, its `order` read strictly; HTTP 404
    /// is gone (None), and so is a body that isn't that order on `symbol`. An id that can't stand in a path is never sent.
    async fn order_created(&self, external_id: &str, symbol: &str, deadline: tokio::time::Instant) -> Result<Option<super::CreatedOrder>, super::HistoryError> {
        if external_id.is_empty() || !external_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { return Ok(None); }
        let r = self.request("GET", &format!("{ORDER}{external_id}"), vec![], None);
        super::by_deadline(deadline, async {
            let resp = super::binance::history_response(&self.transport, &r).await?;
            if resp.status == 404 { return Ok(None); }
            let body = super::binance::history_body(&self.transport, resp, &[], UNREADABLE)?;
            Ok(body.get("order").and_then(|o| created_order(symbol, o)).filter(|o| o.external_id == external_id))
        }).await
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
                &self.request("GET", PRODUCTS, vec![], None),
                false,
            )
            .await?;
        Ok(ticker_prices(&b)?.into_iter().map(|(symbol, price)| (symbol, json!(price.to_s_f()))).collect())
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
