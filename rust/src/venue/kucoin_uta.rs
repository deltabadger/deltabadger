//! D-KUCOIN-UTA-ORDERS (issue #841, UTA.md §3 "Wire", r1.2, r1.3): a Unified Trading Account's spot orders, on KuCoin's
//! own `/api/ua/v2/unified/*` API. A sibling of the HF wire: the routing, the record (`family: "Uta"`), the no-decrease
//! guard and the hold are the HF ones; what differs is here.
//! - **Place**: `tradeType=SPOT`, the intent's UUID as `clientOid`, side and type upper-case, `size` with `sizeUnit`
//!   (QUOTECCY only for a market order by funds), `price` on a limit order.
//! - **Status**: an integer, 2 and 4 open, 3 filled, 5 cancelled with nothing filled, 6 cancelled with a part filled, each
//!   checked against `filledSize`. Status 5 is explicit, so there is no zero-fill delay.
//! - **Totals**: the order carries no quote total, and `avgPrice × filledSize` is not exact. A filled order's totals are
//!   its executions', each validated and kept once by `tradeId`, and only when their sizes sum to `filledSize` exactly.
//! - **Cancel**: a POST naming the order.
//! - **Balance**: the engine spends `max(0, min(available, balance − hold))` of the unified balance: `available` counts
//!   unrealised profit, which is never spent. A currency with a liability has nothing to spend (r6.2).
use super::*;
use std::collections::BTreeMap;

pub(super) const PLACE: &str = "/api/ua/v2/unified/order/place";
pub(super) const CANCEL: &str = "/api/ua/v2/unified/order/cancel";
const DETAIL: &str = "/api/ua/v2/unified/order/detail";
const EXECUTION: &str = "/api/ua/v2/unified/order/execution";
/// UTA placement codes that prove nothing was placed: authentication and permission (400003–400007), insufficient funds
/// (116355) and the throttles (429000, 429001); 400002 is the safe rejection, as on HF.
pub(super) const REFUSALS: [&str; 8] = ["400003", "400004", "400005", "400006", "400007", "116355", "429000", "429001"];
/// ponytail: an execution walk stops at 100 pages (20,000 fills of one order) and is unreadable, never complete.
const MAX_PAGES: usize = 100;

/// The place body. A limit order is always by base; a market order by funds sends them as QUOTECCY.
pub(super) fn place_body(o: &NewOrder) -> Result<Value, VenueError> {
    let mut body = serde_json::Map::new();
    for (k, v) in [("tradeType", "SPOT"), ("clientOid", o.cl_ord_id.as_str()), ("symbol", o.pair.as_str()), ("side", if matches!(o.side, crate::venue::Side::Sell) { "SELL" } else { "BUY" })] {
        body.insert(k.into(), v.into());
    }
    match &o.kind {
        OrderKind::Market => {
            body.insert("orderType".into(), "MARKET".into());
            body.insert("size".into(), o.volume.clone().into());
            body.insert("sizeUnit".into(), if o.quote_volume { "QUOTECCY" } else { "BASECCY" }.into());
        }
        OrderKind::Limit { price } => {
            body.insert("orderType".into(), "LIMIT".into());
            body.insert("size".into(), o.volume.clone().into());
            body.insert("sizeUnit".into(), "BASECCY".into());
            body.insert("price".into(), price.clone().into());
        }
        OrderKind::Crossed { .. } => return Err(crate::venue::not_crossing("KuCoin")),
    }
    Ok(Value::Object(body))
}

fn spot(v: &Value) -> bool { v["tradeType"] == "SPOT" }
fn side_is(v: &Value, sell: bool) -> bool { v["side"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(if sell { "sell" } else { "buy" })) }
/// A decimal sent as a string (a number is not KuCoin's shape: unreadable).
fn text_decimal(v: &Value) -> Option<BigDec> { v.as_str().and_then(|s| BigDec::parse(s).ok()) }

/// A UTA order answer (`/order/detail`), when it is `want` and its status agrees with `filledSize`. Unreadable (the row
/// waits) otherwise: another order, symbol, side, type or trade type; a status that is not an integer 2–6; 3, 4 or 6
/// with nothing filled, 2 or 5 with something. The totals of a filled answer are its executions' (`with_totals`); here
/// the dealt quote is 0 and the price the order's.
pub fn parse_uta_order(o: &Value, want: &HfOrder) -> Result<OrderState, VenueError> {
    if !spot(o) || o["orderId"].as_str() != Some(want.id.as_str()) || o["symbol"].as_str() != Some(want.symbol.as_str()) || !side_is(o, want.sell)
        || limit_type(&o["orderType"]).ok() != Some(want.limit) {
        return Err(unreadable());
    }
    let filled = text_decimal(&o["filledSize"]).filter(|f| *f >= BigDec::zero()).ok_or_else(unreadable)?;
    let status = match (o["status"].as_u64(), filled.is_positive()) {
        (Some(2), false) | (Some(4), true) => OrderStatus::Open,
        (Some(3), true) => OrderStatus::Closed,
        (Some(5), false) | (Some(6), true) => OrderStatus::Cancelled,
        _ => return Err(unreadable()),
    };
    let size = text_decimal(&o["size"]);
    let quote = o["sizeUnit"] == "QUOTECCY";
    let price = text_decimal(&o["price"]).filter(BigDec::is_positive);
    Ok(OrderState { zero_fill_at: None, txid: want.id.clone(), status, price, amount: size.clone().filter(|_| !quote), quote_amount: size.filter(|_| quote),
                    amount_exec: filled, quote_amount_exec: Some(BigDec::zero()), limit: want.limit, sell: want.sell, pair: Some(want.symbol.clone()), asset_class: None })
}

/// One validated execution: the item as answered, its size, value, |fee| and fee currency.
type Execution = (Value, BigDec, BigDec, BigDec, String);
/// What one execution page adds, or why it can't be read: every item is `want`'s (order, symbol, side, SPOT) with a
/// `tradeId`, a positive size and value and a decimal fee; a `tradeId` seen before must be the same item, field for field.
fn add_executions(items: &[Value], want: &HfOrder, seen: &mut BTreeMap<String, Execution>) -> Result<(), VenueError> {
    for e in items {
        let trade = e["tradeId"].as_str().filter(|t| !t.is_empty()).ok_or_else(unreadable)?;
        let (size, value) = (text_decimal(&e["size"]).ok_or_else(unreadable)?, text_decimal(&e["value"]).ok_or_else(unreadable)?);
        let fee = text_decimal(&e["fee"]).ok_or_else(unreadable)?;
        if !spot(e) || e["orderId"].as_str() != Some(want.id.as_str()) || e["symbol"].as_str() != Some(want.symbol.as_str()) || !side_is(e, want.sell)
            || !size.is_positive() || !value.is_positive() {
            return Err(unreadable());
        }
        match seen.get(trade) {
            Some(known) if known.0 != *e => return Err(unreadable()),
            Some(_) => {}
            None => {
                let fee = if fee < BigDec::zero() { &BigDec::zero() - &fee } else { fee };
                seen.insert(trade.into(), (e.clone(), size, value, fee, e["feeCurrency"].as_str().unwrap_or_default().to_string()));
            }
        }
    }
    Ok(())
}
fn sums(seen: &BTreeMap<String, Execution>) -> (BigDec, BigDec) { seen.values().fold((BigDec::zero(), BigDec::zero()), |(s, v), e| (&s + &e.1, &v + &e.2)) }

impl<T: Transport> KucoinVenue<T> {
    /// A UTA GET, its query signed and sent in the order given.
    fn uta_get(&self, path: &str, query: Vec<(&'static str, String)>) -> HttpRequest {
        HttpRequest { method: "GET", base: self.base.clone(), path: path.into(), query, body: None, not_after: None }
    }

    /// One stored UTA order: its detail, then its totals. As `hf_order`: a failure worth retrying is its error; any other
    /// answer that cannot be told (refused, unreadable, executions short of or above `filledSize`) is None, and the row
    /// waits.
    pub(super) async fn uta_order(&self, want: &HfOrder) -> Result<Option<OrderState>, VenueError> {
        let r = self.uta_get(DETAIL, vec![("tradeType", "SPOT".into()), ("symbol", want.symbol.clone()), ("orderId", want.id.clone())]);
        let Some(body) = self.read_ok(&r).await? else { return Ok(None) };
        let Ok(state) = parse_uta_order(&body["data"], want) else { return Ok(None) };
        self.with_totals(want, state).await
    }

    /// A filled answer's totals from its executions (r1.3): `lastId` pages to an empty one, each fill once; Σ size must be
    /// `filledSize` exactly; quote = Σ value, price = Σ value / Σ size at the pair's tick. None when they can't be told.
    async fn with_totals(&self, want: &HfOrder, mut state: OrderState) -> Result<Option<OrderState>, VenueError> {
        if !state.amount_exec.is_positive() { return Ok(Some(state)); }
        let mut seen = BTreeMap::new();
        let mut last: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let mut query = vec![("tradeType", "SPOT".to_string()), ("symbol", want.symbol.clone()), ("orderId", want.id.clone()), ("pageSize", "200".to_string())];
            if let Some(id) = &last { query.push(("lastId", id.clone())); }
            let Some(body) = self.read_ok(&self.uta_get(EXECUTION, query)).await? else { return Ok(None) };
            let Some(items) = body["data"]["items"].as_array() else { return Ok(None) };
            if items.is_empty() {
                let (size, value) = sums(&seen);
                if size != state.amount_exec { return Ok(None); }
                let mut price = value.div(&size).ok_or_else(unreadable)?;
                if let Some(p) = self.pairs.by_symbol.get(&want.symbol) { price = price.round(p.price_decimals); }
                (state.quote_amount_exec, state.price) = (Some(value), Some(price));
                return Ok(Some(state));
            }
            if add_executions(items, want, &mut seen).is_err() { return Ok(None); }
            // The cursor must move on: a repeat would read the same page forever.
            let next = match &body["data"]["lastId"] { Value::Number(n) => n.as_u64().map(|n| n.to_string()), Value::String(s) if s.parse::<u64>().is_ok() => Some(s.clone()), _ => None };
            match next { Some(n) if last.as_ref() != Some(&n) => last = Some(n), _ => return Ok(None) }
        }
        Ok(None)
    }

    /// Recovery's exact lookup, `/order/detail?clientOid=&symbol=`: only an answer naming this clientOid and symbol is the
    /// order (then read as `uta_order` reads it); a miss or any failure is never absence (ABSENCE_UNPROVEN).
    pub(super) async fn uta_by_client_id(&self, cl_ord_id: &str, symbol: &str) -> Result<Option<OrderState>, VenueError> {
        let r = self.uta_get(DETAIL, vec![("tradeType", "SPOT".into()), ("symbol", symbol.into()), ("clientOid", cl_ord_id.into())]);
        let found = match self.call(&r, false, http::MAX_BODY).await {
            Ok(b) if code(&b).ok().as_deref() == Some(OK) && b["data"]["clientOid"].as_str() == Some(cl_ord_id) && b["data"]["symbol"].as_str() == Some(symbol) => b["data"].clone(),
            _ => return Err(VenueError::Ambiguous(ABSENCE_UNPROVEN.into())),
        };
        let want = HfOrder { id: found["orderId"].as_str().filter(|s| !s.is_empty()).ok_or_else(unreadable)?.into(), symbol: symbol.into(),
                             sell: side_is(&found, true), limit: limit_type(&found["orderType"]).map_err(|_| unreadable())? };
        let state = parse_uta_order(&found, &want)?;
        self.with_totals(&want, state).await?.map(Some).ok_or_else(unreadable)
    }

    /// A UTA row's cancel, `POST /order/cancel {tradeType, symbol, orderId}`. Accepted only on 200000 naming this order; an
    /// error envelope below 500 is the venue's refusal; anything else (lost, 5xx, unreadable, another id) is Ambiguous. An
    /// accepted cancel is a request, never a terminal state: the row's poll settles it.
    pub(super) async fn uta_cancel(&self, want: &HfOrder) -> Result<(), VenueError> {
        let body = serde_json::json!({"tradeType": "SPOT", "symbol": want.symbol, "orderId": want.id});
        let req = HttpRequest { method: "POST", base: self.base.clone(), path: CANCEL.into(), query: vec![], body: Some(body), not_after: None };
        let resp = match self.transport.send_limited(&req, http::MAX_BODY).await {
            Ok(resp) => resp,
            Err(e) => return Err(VenueError::Ambiguous(self.transport.redact_diagnostic(&format!("{e:?}")))),
        };
        let body = http::decode_json(&resp.body).ok();
        match (resp.status, body.as_ref().and_then(|b| code(b).ok()).as_deref()) {
            (200..=299, Some(OK)) if body.as_ref().is_some_and(|b| b["data"]["orderId"].as_str() == Some(want.id.as_str())) => Ok(()),
            (200..=499, Some(c)) if c != OK => Err(VenueError::Rejected(vec![api_error(body.as_ref().expect("a code was read"))])),
            _ => Err(VenueError::Ambiguous(format!("KuCoin's answer to cancelling order {} cannot be told (HTTP {})", want.id, resp.status))),
        }
    }

    /// The unified balance as the classic account rows `available` reads: each currency's spendable as `available`, its
    /// `hold` as `locked`. Only the success code on a UNIFIED answer means balances.
    pub(super) async fn uta_accounts(&self) -> Result<Vec<Value>, VenueError> {
        let body = self.call(&self.uta_get(crate::sync::balances::KUCOIN_UTA_BALANCE_PATH, vec![]), false, http::MAX_BODY).await?;
        if code(&body).ok().as_deref() != Some(OK) || body["data"]["accountType"] != "UNIFIED" {
            return Err(VenueError::Rejected(vec![format!("Failed to get KuCoin balances: {}", code_and_msg(&body))]));
        }
        let mut rows = vec![];
        for account in body["data"]["accounts"].as_array().ok_or_else(unreadable)? {
            for c in account["currencies"].as_array().ok_or_else(unreadable)? {
                let d = |k: &str| required(field(c, k)?);
                let (available, balance, hold, liability) = (d("available")?, d("balance")?, d("hold")?, d("liability")?);
                let net = &balance - &hold;
                let spendable = if available < net { available } else { net };
                let spendable = if spendable.is_positive() && !liability.is_positive() { spendable } else { BigDec::zero() };
                rows.push(serde_json::json!({"currency": field(c, "currency")?, "available": spendable.to_s_f(), "locked": hold.to_s_f()}));
            }
        }
        Ok(rows)
    }
}
