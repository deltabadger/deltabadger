//! BingX spot trading: Rails B1–B8 and S7, with D9 and conservative placement classification.
use super::binance::{error_message, Pairs, Shape};
use super::http::{self, HttpRequest, HttpResponse, Transport, TransportError};
use super::{NewOrder, OrderKind, OrderState, OrderStatus, PriceSide, VenueError};
use crate::engine::model::Ticker;
use crate::engine::venue_rules::{VenueRules, BINGX as RULES};
use crate::ruby::BigDec;
use chrono::{DateTime, Utc};
use serde_json::Value;

pub const URL: &str = "https://open-api.bingx.com";
pub const BINGX: &str = "Exchanges::Bingx";
pub const UNREADABLE: &str = "BingX: unreadable response";
pub const ORDER_PATH: &str = "/openApi/spot/v1/trade/order";
pub const QUERY_PATH: &str = "/openApi/spot/v1/trade/query";
pub const CANCEL_PATH: &str = "/openApi/spot/v1/trade/cancel";
pub const ACCOUNT_PATH: &str = "/openApi/spot/v1/account/balance";
pub const ABSENCE_UNPROVEN: &str = "BingX: the client order lookup did not find this order, and BingX cannot prove it absent; settle it with `deltabadger resolve-placement`";

pub const SHAPE: Shape = Shape {
    statuses: &[
        ("NEW", OrderStatus::Open),
        ("PARTIALLY_FILLED", OrderStatus::Open),
        ("FILLED", OrderStatus::Closed),
        ("CANCELED", OrderStatus::Cancelled),
        ("PENDING", OrderStatus::Open),
        ("FAILED", OrderStatus::Failed),
        ("EXPIRED", OrderStatus::Cancelled),
        ("REJECTED", OrderStatus::Failed),
    ],
    unlisted: None,
    average_whenever_filled: true,
    quote_optional: true,
};

pub(crate) fn unreadable() -> VenueError {
    VenueError::Ambiguous(UNREADABLE.into())
}
fn required(v: &Value) -> Result<BigDec, VenueError> {
    super::json::required(v, unreadable)
}

// BingX permits 1–40 ASCII letters/digits/underscores. UUIDs use the shared reversible encoding.
fn client_order_id(id: &str) -> Result<String, VenueError> {
    let id = if uuid::Uuid::parse_str(id).is_ok() {
        super::client_order_id_32(id, "BingX")?
    } else {
        id.into()
    };
    if id.is_empty() || id.len() > 40 || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err(VenueError::Rejected(vec![
            "BingX: invalid client order id".into()
        ]));
    }
    Ok(id)
}
pub fn code(v: &Value) -> Option<i64> {
    match &v["code"] {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}
fn gone(body: &str) -> bool {
    serde_json::from_str::<Value>(body).ok().is_some_and(|v| {
        let msg = v["msg"].as_str().unwrap_or_default().to_ascii_lowercase();
        matches!(code(&v), Some(100404 | 100400))
            && (msg.contains("order does not exist") || msg.contains("order not exist"))
    })
}
fn api_error(v: &Value) -> String {
    let detail: Vec<String> = ["code", "msg"]
        .iter()
        .filter_map(|k| v.get(*k))
        .filter(|v| !v.is_null())
        .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
        .filter(|s| !s.is_empty())
        .collect();
    if detail.is_empty() {
        "BingX API error".into()
    } else {
        format!("BingX API error {}", detail.join(": "))
    }
}

pub fn parse_order(order_id: &str, o: &Value, pairs: &Pairs) -> Result<OrderState, VenueError> {
    super::binance::parse_binance_shaped(RULES.name, order_id, o, pairs, &SHAPE).map(|o| o.state)
}

fn split_order_id(order_id: &str) -> (String, Option<String>) {
    let (symbol, ext) = order_id.rsplit_once('-').unwrap_or(("", order_id));
    (symbol.into(), Some(ext.into()))
}

#[derive(Clone)]
pub struct BingxVenue<T: Transport> {
    transport: T,
    base: String,
    secret: String,
    pairs: Pairs,
    identity: Option<Value>,
}

impl<T: Transport> BingxVenue<T> {
    pub fn new(transport: T, base: &str, secret: &str) -> Self {
        Self {
            transport,
            base: base.into(),
            secret: secret.into(),
            pairs: Pairs::default(),
            identity: None,
        }
    }
    pub fn with_identity(mut self, credentials: &crate::crypto::Credentials) -> Self {
        self.identity = Some(super::key_identity(&self.base, credentials));
        self
    }
    pub fn with_pairs(mut self, pairs: Pairs) -> Self {
        self.pairs = pairs;
        self
    }

    pub fn signed(
        &self,
        method: &'static str,
        path: &str,
        params: Vec<(&'static str, String)>,
        now: DateTime<Utc>,
    ) -> HttpRequest {
        self.request(
            method,
            path,
            super::sign::BINGX_QUERY.sign(&self.secret, params, now),
        )
    }

    pub fn request(
        &self,
        method: &'static str,
        path: &str,
        mut params: Vec<(&'static str, String)>,
    ) -> HttpRequest {
        params.sort_by(|a, b| a.0.cmp(b.0));
        HttpRequest {
            method,
            base: self.base.clone(),
            path: path.into(),
            query: params,
            body: None,
            not_after: None,
        }
    }

    fn signed_now(
        &self,
        method: &'static str,
        path: &str,
        params: Vec<(&'static str, String)>,
    ) -> HttpRequest {
        self.signed(method, path, params, self.transport.now())
    }

    pub async fn read_text(&self, r: &HttpRequest, limit: usize) -> Result<String, String> {
        match self.transport.send_limited(r, limit).await {
            Err(
                TransportError::Permanent(m)
                | TransportError::NotSent(m)
                | TransportError::MaybeSent(m),
            ) => Err(self.transport.redact_diagnostic(&m)),
            Ok(resp) if (200..300).contains(&resp.status) => Ok(resp.body),
            Ok(resp) if !resp.body.is_empty() => Err(resp.body),
            Ok(resp) => Err(format!(
                "the server responded with status {} for {} {}",
                resp.status,
                r.method,
                r.url()
            )),
        }
    }

    async fn send(&self, r: &HttpRequest, placement: bool) -> Result<HttpResponse, VenueError> {
        self.transport.send(r).await.map_err(|e| {
            http::transport_failure(e, placement, |m| self.transport.redact_diagnostic(&m))
        })
    }

    fn classify(
        &self,
        r: &HttpRequest,
        resp: HttpResponse,
        placement: bool,
        order: bool,
    ) -> Result<String, VenueError> {
        let decoded = http::decode_json(&resp.body);
        let value = decoded.as_ref().ok();
        let result_code = value.and_then(code);
        if (200..300).contains(&resp.status) && result_code == Some(0) {
            return Ok(resp.body);
        }
        let message = if (200..300).contains(&resp.status) {
            value
                .map(|v| {
                    if order {
                        api_error(v)
                    } else {
                        v["msg"].as_str().unwrap_or(UNREADABLE).into()
                    }
                })
                .unwrap_or_else(|| resp.body.clone())
        } else if resp.body.is_empty() {
            format!(
                "the server responded with status {} for {} {}",
                resp.status,
                r.method,
                r.url()
            )
        } else {
            error_message(&resp.body)
        };
        if !placement && resp.status == 401 {
            return Err(VenueError::Unauthorized(vec![message]));
        }
        if !placement && resp.status == 429 {
            return Err(VenueError::Rejected(vec!["Too Many Requests".into()]));
        }
        // D-BINGX-AMBIG: a result code is required, and only a documented pre-placement refusal is definitive.
        let definitive = resp.status < 500
            && resp.status != 429
            && matches!(
                result_code,
                Some(100001 | 100004 | 100202 | 100412 | 100413 | 100419 | 100421)
            );
        Err(if placement && !definitive {
            VenueError::Ambiguous(message)
        } else {
            VenueError::Rejected(vec![message])
        })
    }

    async fn call(&self, r: &HttpRequest, placement: bool) -> Result<Value, VenueError> {
        let resp = self.send(r, placement).await?;
        let body = http::decode_json(&self.classify(r, resp, placement, placement)?)
            .map_err(|_| unreadable())?;
        Ok(body.get("data").cloned().unwrap_or(body))
    }

    async fn lookup(&self, r: &HttpRequest) -> Result<Option<Value>, VenueError> {
        let resp = self.send(r, false).await?;
        if gone(&resp.body) {
            return Ok(None);
        }
        let body =
            http::decode_json(&self.classify(r, resp, false, true)?).map_err(|_| unreadable())?;
        Ok(Some(body.get("data").cloned().unwrap_or(body)))
    }

    async fn get_order(&self, order_id: &str) -> Result<Option<OrderState>, VenueError> {
        let (symbol, ext) = split_order_id(order_id);
        let mut params = vec![("symbol", symbol)];
        params.extend(ext.map(|e| ("orderId", e)));
        match self
            .lookup(&self.signed_now("GET", QUERY_PATH, params))
            .await?
        {
            Some(body) => parse_order(order_id, &body, &self.pairs).map(Some),
            None => Ok(None),
        }
    }

    async fn free(&self, pick: impl Fn(&str) -> bool) -> Result<BigDec, VenueError> {
        let body = self
            .call(&self.signed_now("GET", ACCOUNT_PATH, vec![]), false)
            .await?;
        let mut free = BigDec::zero();
        for row in super::dig_or_raise(&body, "balances")?
            .as_array()
            .ok_or_else(unreadable)?
        {
            if !row["asset"].as_str().is_some_and(&pick) {
                continue;
            }
            free = required(super::dig_or_raise(row, "free")?)?;
            required(super::dig_or_raise(row, "locked")?)?;
        }
        Ok(free)
    }
}

impl<T: Transport> super::Venue for BingxVenue<T> {
    fn rules(&self) -> &'static VenueRules {
        &RULES
    }
    fn placement_identity(&self) -> Option<Value> {
        self.identity.clone()
    }

    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        let symbol = vec![("symbol", ticker.ticker.clone())];
        let (price, label) = match side {
            PriceSide::Ask | PriceSide::Bid => {
                let mut q = symbol;
                q.push(("limit", "1".into()));
                let body = self
                    .call(
                        &self.request("GET", "/openApi/spot/v1/market/depth", q),
                        false,
                    )
                    .await?;
                let level = |k: &str| -> Result<BigDec, VenueError> {
                    let row = super::dig_or_raise(&body, k)?
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(Value::as_array)
                        .ok_or_else(unreadable)?;
                    required(row.get(1).ok_or_else(unreadable)?)?;
                    required(row.first().ok_or_else(unreadable)?)
                };
                let bid = level("bids")?;
                let ask = level("asks")?;
                if side == PriceSide::Bid {
                    (bid, "bid")
                } else {
                    (ask, "ask")
                }
            }
            PriceSide::Last => {
                let body = self
                    .call(
                        &self.request("GET", "/openApi/spot/v2/market/ticker", symbol),
                        false,
                    )
                    .await?;
                (
                    required(super::dig_or_raise(
                        body.as_array().and_then(|a| a.first()).unwrap_or(&body),
                        "lastPrice",
                    )?)?,
                    "last",
                )
            }
        };
        if price.is_zero() {
            return Err(VenueError::Rejected(vec![format!(
                "Wrong {label} price for {}: {}",
                ticker.ticker,
                price.to_s_f()
            )]));
        }
        Ok(price)
    }

    async fn add_order(&self, o: &NewOrder) -> Result<String, VenueError> {
        let mut params = vec![
            ("symbol", o.pair.clone()),
            ("side", o.side.as_str().to_ascii_uppercase()),
        ];
        let kind = match &o.kind {
            OrderKind::Market => {
                params.extend([
                    ("type", "MARKET".into()),
                    (
                        if o.quote_volume {
                            "quoteOrderQty"
                        } else {
                            "quantity"
                        },
                        o.volume.clone(),
                    ),
                ]);
                "market"
            }
            OrderKind::Limit { price } => {
                params.extend([
                    ("type", "LIMIT".into()),
                    ("timeInForce", "GTC".into()),
                    ("price", price.clone()),
                    ("quantity", o.volume.clone()),
                ]);
                "limit"
            }
            OrderKind::Crossed { .. } => return Err(super::not_crossing("BingX")),
        };
        params.push(("newClientOrderId", client_order_id(&o.cl_ord_id)?));
        let r = HttpRequest {
            not_after: Some(
                o.deadline + chrono::Duration::from_std(http::TOTAL_TIMEOUT).expect("45 s"),
            ),
            ..self.signed_now("POST", ORDER_PATH, params)
        };
        let body = self.call(&r, true).await?;
        let id = match body
            .get("orderId")
            .filter(|v| !matches!(v, Value::Null | Value::Bool(false)))
            .unwrap_or(&body["data"]["orderId"])
        {
            Value::Number(n) if n.is_u64() || n.is_i64() => n.to_string(),
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => {
                return Err(VenueError::Ambiguous(format!(
                    "Failed to set {} {kind} order (order_id is {}-)",
                    RULES.name, o.pair
                )))
            }
        };
        Ok(format!("{}-{id}", o.pair))
    }

    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        Ok(
            super::poll_each(txids, |id| async move { self.get_order(&id).await })
                .await?
                .0,
        )
    }

    async fn order_identified<F>(
        &self,
        txid: &str,
        _identify: F,
    ) -> Result<(Vec<OrderState>, Vec<String>), VenueError>
    where
        F: FnMut(&str, Option<&str>, Option<&str>) -> Result<bool, VenueError>,
    {
        match self.get_order(txid).await? {
            Some(o) => Ok((vec![o], vec![])),
            None => Err(VenueError::Rejected(vec![format!(
                "{} no longer knows order {txid}",
                RULES.name
            )])),
        }
    }

    async fn order_by_client_id(
        &self,
        cl_ord_id: &str,
        symbol: &str,
        _since: DateTime<Utc>,
    ) -> Result<Option<OrderState>, VenueError> {
        let cl_ord_id = client_order_id(cl_ord_id)?;
        let r = self.signed_now(
            "GET",
            QUERY_PATH,
            vec![
                ("symbol", symbol.to_string()),
                ("clientOrderID", cl_ord_id.to_string()),
            ],
        );
        let Some(body) = self.lookup(&r).await? else {
            return Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into()));
        };
        let ext = match &body["orderId"] {
            Value::Number(n) if n.is_i64() || n.is_u64() => n.to_string(),
            Value::String(s) if !s.is_empty() => s.clone(),
            _ => return Err(unreadable()),
        };
        if body["symbol"].as_str() != Some(symbol)
            || body["clientOrderID"].as_str() != Some(cl_ord_id.as_str())
        {
            return Err(VenueError::Rejected(vec![format!(
                "{} answered for client order id {cl_ord_id} on {symbol} with another order",
                RULES.name
            )]));
        }
        parse_order(&format!("{symbol}-{ext}"), &body, &self.pairs).map(Some)
    }

    async fn fills_from_trades(
        &self,
        _txids: &[String],
        _since: DateTime<Utc>,
    ) -> Result<Vec<OrderState>, VenueError> {
        Ok(vec![])
    }

    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.free(|code| self.pairs.assets.get(code).map(String::as_str) == Some(asset_symbol))
            .await
    }

    async fn free_base_balance(&self, ticker: &Ticker) -> Result<BigDec, VenueError> {
        self.free(|code| code == ticker.base_code).await
    }

    async fn settled_quote_balance<F>(
        &self,
        ticker: &Ticker,
        _catalog: F,
    ) -> Result<BigDec, VenueError>
    where
        F: FnOnce() -> Result<crate::sync::balances::Catalog, VenueError>,
    {
        self.free(|code| code == ticker.quote_code).await
    }

    async fn tickers_prices(
        &self,
        _symbols: &[String],
    ) -> Result<Vec<(String, Value)>, VenueError> {
        let body = self
            .call(
                &self.request("GET", "/openApi/spot/v2/market/ticker", vec![]),
                false,
            )
            .await?;
        let mut out: Vec<(String, Value)> = vec![];
        for row in body.as_array().ok_or_else(unreadable)? {
            let symbol = super::dig_or_raise(row, "symbol")?
                .as_str()
                .ok_or_else(unreadable)?
                .to_string();
            let p = required(super::dig_or_raise(row, "lastPrice")?)?;
            match out.iter_mut().find(|(s, _)| *s == symbol) {
                Some((_, old)) => *old = Value::String(p.to_s_f()),
                None => out.push((symbol, Value::String(p.to_s_f()))),
            }
        }
        Ok(out)
    }

    async fn cancel(&self, txid: &str) -> Result<(), VenueError> {
        let (symbol, ext) = split_order_id(txid);
        let mut params = vec![("symbol", symbol)];
        params.extend(ext.map(|e| ("orderId", e)));
        self.call(&self.signed_now("POST", CANCEL_PATH, params), false)
            .await
            .map(|_| ())
    }
}
