//! A scripted Kraken: raw response bodies per path (the same script the Rails harness replays), plus a book
//! of placed orders so recovery by cl_ord_id behaves like the real venue.
use super::*;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum AddOutcome { Accept(String), Reject(Vec<String>), AmbiguousPlaced(String), AmbiguousNotPlaced(String), NotSent(String) }

#[derive(Default)]
struct State {
    http: HashMap<String, VecDeque<Value>>,
    last: HashMap<String, Value>,
    adds: VecDeque<AddOutcome>,
    book: Vec<(String, String, NewOrder)>, // cl_ord_id, txid, order
    orders: HashMap<String, Value>,       // raw Kraken orders by txid, beyond the QueryOrders script
    sent: Vec<NewOrder>,
    calls: HashMap<String, usize>,
    lookup_failures: u32,
    on_add: Option<Rc<dyn Fn()>>,   // runs while AddOrder "awaits its reply": a test's concurrent writer
    on_query: Option<Rc<dyn Fn()>>, // the same, while QueryOrders does (the pre-tick sweep)
}

#[derive(Clone, Default)]
pub struct FakeVenue { s: Rc<RefCell<State>> }

const ASSET_MAP: [(&str, &str); 10] = [("ZUSD", "USD"), ("ZEUR", "EUR"), ("ZGBP", "GBP"), ("ZJPY", "JPY"), ("ZCHF", "CHF"),
    ("ZCAD", "CAD"), ("ZAUD", "AUD"), ("XXBT", "XBT"), ("XETH", "ETH"), ("XXDG", "XDG")];

fn dec(v: &Value) -> Option<BigDec> { v.as_str().and_then(|s| BigDec::parse(s).ok()) }

/// A raw Kraken QueryOrders/ClosedOrders order, parsed as Exchanges::Kraken#parse_order_data does.
pub fn kraken_order(txid: &str, o: &Value) -> OrderState {
    let viqc = o["oflags"].as_str().is_some_and(|f| f.split(',').any(|x| x == "viqc"));
    let limit = o["descr"]["ordertype"] == "limit";
    let vol = dec(&o["vol"]);
    let mut price = dec(&o["price"]).unwrap_or_else(BigDec::zero);
    if price.is_zero() && limit { price = dec(&o["descr"]["price"]).unwrap_or_else(BigDec::zero); }
    OrderState {
        txid: txid.into(),
        status: match o["status"].as_str() { Some("open") => OrderStatus::Open, Some("closed") => OrderStatus::Closed,
                                             Some("canceled") | Some("expired") => OrderStatus::Cancelled, _ => OrderStatus::Unknown },
        price: (!price.is_zero()).then_some(price), amount: if viqc { None } else { vol.clone() }, quote_amount: if viqc { vol } else { None },
        amount_exec: dec(&o["vol_exec"]).unwrap_or_else(BigDec::zero), quote_amount_exec: dec(&o["cost"]).unwrap_or_else(BigDec::zero), limit,
        sell: o["descr"]["type"] == "sell",
    }
}

impl FakeVenue {
    pub fn new() -> Self { Self::default() }
    pub fn from_script(script: &Value) -> Self {
        let v = Self::new();
        if let Some(http) = script["http"].as_object() {
            for (path, bodies) in http {
                v.s.borrow_mut().http.insert(path.clone(), bodies.as_array().cloned().unwrap_or_default().into());
            }
        }
        v
    }
    fn push(self, path: &str, body: Value) -> Self { self.s.borrow_mut().http.entry(path.into()).or_default().push_back(body); self }
    pub fn ticker(self, key: &str, bid: &str, ask: &str, last: &str) -> Self {
        self.push("/0/public/Ticker", json!({ "error": [], "result": { key: { "a": [ask, "1", "1.000"], "b": [bid, "1", "1.000"], "c": [last, "0.001"], "v": ["12.5", "30.1"], "p": [last, last], "t": [100, 250], "l": [last, last], "h": [last, last], "o": last } } }))
    }
    pub fn balance_body(self, kraken_asset: &str, balance: &str, hold_trade: &str) -> Self {
        self.push("/0/private/BalanceEx", json!({ "error": [], "result": { kraken_asset: { "balance": balance, "hold_trade": hold_trade } } }))
    }
    pub fn next_add(self, o: AddOutcome) -> Self { self.s.borrow_mut().adds.push_back(o); self }
    pub fn order(self, txid: &str, raw: Value) -> Self { self.s.borrow_mut().orders.insert(txid.into(), raw); self }
    pub fn lookup_fails(self, n: u32) -> Self { self.s.borrow_mut().lookup_failures = n; self }
    pub fn on_add(self, f: impl Fn() + 'static) -> Self { self.s.borrow_mut().on_add = Some(Rc::new(f)); self }
    pub fn on_query(self, f: impl Fn() + 'static) -> Self { self.s.borrow_mut().on_query = Some(Rc::new(f)); self }
    pub fn sent(&self) -> Vec<NewOrder> { self.s.borrow().sent.clone() }
    pub fn calls(&self, path: &str) -> usize { self.s.borrow().calls.get(path).copied().unwrap_or(0) }

    /// The next scripted body for a path (the last one repeats); `None` when the path is unscripted.
    fn body(&self, path: &str) -> Option<Value> {
        let mut s = self.s.borrow_mut();
        *s.calls.entry(path.into()).or_default() += 1;
        let next = s.http.get_mut(path).and_then(|q| if q.len() > 1 { q.pop_front() } else { q.front().cloned() });
        if let Some(b) = &next { s.last.insert(path.into(), b.clone()); }
        next
    }
}

fn errors(body: &Value) -> Option<Vec<String>> {
    body["error"].as_array().filter(|e| !e.is_empty()).map(|e| e.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
}

impl Venue for FakeVenue {
    async fn prices(&self, _pair: &str) -> Result<Prices, VenueError> {
        let body = self.body("/0/public/Ticker").ok_or_else(|| VenueError::Transient("no scripted Ticker".into()))?;
        if let Some(e) = errors(&body) { return Err(VenueError::Rejected(e)); }
        let (_, t) = body["result"].as_object().and_then(|m| m.iter().next()).ok_or_else(|| VenueError::Ambiguous("Kraken: unreadable response".into()))?;
        let p = |k: &str| dec(&t[k][0]).unwrap_or_else(BigDec::zero);
        Ok(Prices { bid: p("b"), ask: p("a"), last: p("c") })
    }

    async fn add_order(&self, order: &NewOrder) -> Result<String, VenueError> {
        self.s.borrow_mut().sent.push(order.clone());
        let hook = self.s.borrow().on_add.clone();
        if let Some(f) = hook { f(); }
        let scripted = self.s.borrow_mut().adds.pop_front();
        let outcome = match scripted {
            Some(o) => o,
            None => match self.body("/0/private/AddOrder") {
                Some(b) => match (errors(&b), b["result"]["txid"][0].as_str()) {
                    (Some(e), _) => AddOutcome::Reject(e),
                    (None, Some(t)) => AddOutcome::Accept(t.to_string()),
                    (None, None) => AddOutcome::AmbiguousNotPlaced("Failed to set Kraken order (order_id is nil)".into()),
                },
                None => AddOutcome::Accept(format!("OFAKE-{}", self.s.borrow().sent.len())),
            },
        };
        let mut s = self.s.borrow_mut();
        match outcome {
            AddOutcome::Accept(t) => { s.book.push((order.cl_ord_id.clone(), t.clone(), order.clone())); Ok(t) }
            AddOutcome::AmbiguousPlaced(t) => { s.book.push((order.cl_ord_id.clone(), t, order.clone())); Err(VenueError::Ambiguous("reply lost".into())) }
            AddOutcome::AmbiguousNotPlaced(m) => Err(VenueError::Ambiguous(m)),
            AddOutcome::Reject(e) => Err(VenueError::Rejected(e)),
            AddOutcome::NotSent(m) => { s.sent.pop(); Err(VenueError::Transient(m)) }
        }
    }

    async fn orders(&self, txids: &[String]) -> Result<Vec<OrderState>, VenueError> {
        let hook = self.s.borrow().on_query.clone();
        if let Some(f) = hook { f(); }
        let mut out = vec![];
        if let Some(body) = self.body("/0/private/QueryOrders") {
            if let Some(e) = errors(&body) { return Err(VenueError::Rejected(e)); }
            for t in txids { if let Some(raw) = body["result"].get(t) { out.push(kraken_order(t, raw)); } }
        }
        let s = self.s.borrow();
        for t in txids {
            if out.iter().any(|o| &o.txid == t) { continue; }
            if let Some(raw) = s.orders.get(t) { out.push(kraken_order(t, raw)); }
        }
        Ok(out)
    }

    async fn order_by_client_id(&self, cl_ord_id: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        {
            let mut s = self.s.borrow_mut();
            if s.lookup_failures > 0 { s.lookup_failures -= 1; return Err(VenueError::Transient("ClosedOrders page failed".into())); }
        }
        let found = self.s.borrow().book.iter().find(|(c, _, _)| c == cl_ord_id).cloned();
        let Some((_, txid, order)) = found else { return Ok(None) };
        let known = self.orders(std::slice::from_ref(&txid)).await?;
        Ok(Some(known.into_iter().next().unwrap_or(OrderState {
            txid, status: OrderStatus::Open, price: None, amount: None, quote_amount: None,
            amount_exec: BigDec::zero(), quote_amount_exec: BigDec::zero(), limit: matches!(order.kind, OrderKind::Limit { .. }), sell: false,
        })))
    }

    async fn fills_from_trades(&self, txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> {
        // Honeymaker::Clients::Kraken#closed_orders_from_trades: page by ofs until count (max 20 pages), then aggregate.
        let mut trades: Vec<Value> = vec![];
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let Some(body) = self.body("/0/private/TradesHistory") else { break };
            if let Some(e) = errors(&body) { return Err(VenueError::Rejected(e)); }
            let page = body["result"]["trades"].as_object().cloned().unwrap_or_default();
            if page.is_empty() || page.keys().all(|k| seen.contains(k)) { break; } // the script's last page repeats
            for (id, t) in page { if seen.insert(id) { trades.push(t); } }
            if seen.len() as u64 >= body["result"]["count"].as_u64().unwrap_or(0) { break; }
        }
        let mut by: Vec<(String, BigDec, BigDec, bool, bool)> = vec![];
        for trade in trades.iter() {
            let Some(o) = trade["ordertxid"].as_str().filter(|o| txids.iter().any(|t| t == o)) else { continue };
            let (vol, cost) = (dec(&trade["vol"]).unwrap_or_else(BigDec::zero), dec(&trade["cost"]).unwrap_or_else(BigDec::zero));
            match by.iter_mut().find(|(t, ..)| t == o) {
                Some(e) => { e.1 = &e.1 + &vol; e.2 = &e.2 + &cost; }
                None => by.push((o.to_string(), vol, cost, trade["ordertype"] == "limit", trade["type"] == "sell")),
            }
        }
        Ok(by.into_iter().map(|(txid, vol, cost, limit, sell)| OrderState {
            txid, status: OrderStatus::Closed, price: cost.div(&vol), amount: None, quote_amount: None, amount_exec: vol, quote_amount_exec: cost, limit, sell,
        }).collect())
    }

    async fn balance(&self, asset_symbol: &str) -> Result<BigDec, VenueError> {
        let Some(body) = self.body("/0/private/BalanceEx") else { return Err(VenueError::Transient("no scripted BalanceEx".into())) };
        if let Some(e) = errors(&body) { return Err(VenueError::Rejected(e)); }
        let mut free = BigDec::zero();
        for (code, b) in body["result"].as_object().into_iter().flatten() {
            let name = ASSET_MAP.iter().find(|(k, _)| k == code).map(|(_, v)| *v).unwrap_or(code.as_str());
            if name == asset_symbol {
                free = &(&free + &dec(&b["balance"]).unwrap_or_else(BigDec::zero)) - &dec(&b["hold_trade"]).unwrap_or_else(BigDec::zero);
            }
        }
        Ok(free)
    }
}

#[derive(Clone, Default)]
pub struct FakeFactory(pub FakeVenue);
impl VenueFactory for FakeFactory {
    type V = FakeVenue;
    fn for_key(&self, _credentials: Option<Credentials>) -> FakeVenue { self.0.clone() }
}
