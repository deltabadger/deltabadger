//! A scripted Kraken: raw response bodies per path (the same script the Rails harness replays), plus a book
//! of placed orders so recovery by cl_ord_id behaves like the real venue.
use super::*;
use crate::engine::venue_rules::KRAKEN;
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
    on_lookup: Option<Rc<dyn Fn()>>, // the same, while a recovery lookup by cl_ord_id does
    on_price: Option<Rc<dyn Fn()>>,  // the same, while a price read does
    hold_add: Option<Rc<tokio::sync::Notify>>, // AddOrder's reply waits for it: a tick held in hand while a test acts
    latency: Option<std::time::Duration>,      // every call first awaits this much timer: a slow venue whose await yields
}

#[derive(Clone, Default)]
pub struct FakeVenue { s: Rc<RefCell<State>> }

const ASSET_MAP: [(&str, &str); 10] = [("ZUSD", "USD"), ("ZEUR", "EUR"), ("ZGBP", "GBP"), ("ZJPY", "JPY"), ("ZCHF", "CHF"),
    ("ZCAD", "CAD"), ("ZAUD", "AUD"), ("XXBT", "XBT"), ("XETH", "ETH"), ("XXDG", "XDG")];

/// A Kraken decimal: absent or null is None (Ruby's nil). Anything present must be a finite decimal within BigDec's
/// bounds, or the whole answer is unreadable — never a zero price, fill or balance. DIVERGES from Ruby on purpose
/// (ruby::json_to_d): Rails' `.to_d` reads "garbage" as 0 and "NaN" as NaN.
fn dec(v: &Value) -> Result<Option<BigDec>, VenueError> { crate::ruby::json_to_d(v).map_err(|_| unreadable()) }

/// A raw Kraken QueryOrders/ClosedOrders order, parsed as Exchanges::Kraken#parse_order_data does.
pub fn kraken_order(txid: &str, o: &Value) -> OrderState {
    parse_order(txid, o).expect("a parseable Kraken order")
}

/// `parse_order_data` raises on an unknown status or order type; here that is an error, never a guess.
fn parse_order(txid: &str, o: &Value) -> Result<OrderState, VenueError> {
    let status = match o["status"].as_str() {
        Some("open") => OrderStatus::Open, Some("closed") => OrderStatus::Closed,
        Some("canceled") | Some("expired") => OrderStatus::Cancelled, Some("pending") => OrderStatus::Unknown,
        other => return Err(VenueError::Ambiguous(format!("Unknown Kraken order status: {}", other.unwrap_or("")))),
    };
    if !matches!(o["descr"]["ordertype"].as_str(), Some("market") | Some("limit")) {
        return Err(VenueError::Ambiguous(format!("Unknown Kraken order type: {}", o["descr"]["ordertype"].as_str().unwrap_or(""))));
    }
    let viqc = o["oflags"].as_str().is_some_and(|f| f.split(',').any(|x| x == "viqc"));
    let limit = o["descr"]["ordertype"] == "limit";
    // Kraken's parse_venue_number requires every order number; absent fills never mean zero.
    let required = |value: &Value| dec(value)?.ok_or_else(unreadable);
    let vol = Some(required(&o["vol"])?);
    let mut price = required(&o["price"])?;
    if price.is_zero() && limit { price = required(&o["descr"]["price"])?; }
    Ok(OrderState {
        txid: txid.into(),
        status,
        price: (!price.is_zero()).then_some(price), amount: if viqc { None } else { vol.clone() }, quote_amount: if viqc { vol } else { None },
        amount_exec: required(&o["vol_exec"])?, quote_amount_exec: required(&o["cost"])?, limit,
        sell: o["descr"]["type"] == "sell",
        pair: o["descr"]["pair"].as_str().map(str::to_string),
    })
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
    pub fn on_lookup(self, f: impl Fn() + 'static) -> Self { self.s.borrow_mut().on_lookup = Some(Rc::new(f)); self }
    pub fn on_price(self, f: impl Fn() + 'static) -> Self { self.s.borrow_mut().on_price = Some(Rc::new(f)); self }
    /// AddOrder records the order and runs `on_add`, then waits for `gate.notify_one()` before it answers: a tick held
    /// in hand at an await point while a test stops the process or writes as the web does.
    pub fn hold_add(self, gate: Rc<tokio::sync::Notify>) -> Self { self.s.borrow_mut().hold_add = Some(gate); self }
    /// Every venue call first awaits `d` of timer, as a slow venue's reply does: the runtime thread is free meanwhile.
    pub fn latency(self, d: std::time::Duration) -> Self { self.s.borrow_mut().latency = Some(d); self }
    async fn wait(&self) {
        let d = self.s.borrow().latency; // copied out: no borrow is held across the await
        if let Some(d) = d { tokio::time::sleep(d).await; }
    }
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

/// `Ok(())` for a clean body; `Rejected` for a non-empty `error`; `Ambiguous` when `error` is missing or not an
/// array, or a success body has no `result` (honeymaker 0.12.3: unreadable).
fn check(body: &Value) -> Result<(), VenueError> {
    let Some(e) = body["error"].as_array() else { return Err(unreadable()) };
    if !e.is_empty() { return Err(VenueError::Rejected(e.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())); }
    if body.get("result").is_none() { return Err(unreadable()); }
    Ok(())
}
fn unreadable() -> VenueError { VenueError::Ambiguous("Kraken: unreadable response".into()) }

impl Venue for FakeVenue {
    fn rules(&self) -> &'static VenueRules { &KRAKEN }

    async fn price(&self, ticker: &Ticker, side: PriceSide) -> Result<BigDec, VenueError> {
        self.wait().await;
        let hook = self.s.borrow().on_price.clone();
        if let Some(f) = hook { f(); }
        let body = self.body("/0/public/Ticker").ok_or_else(|| VenueError::Transient("no scripted Ticker".into()))?;
        check(&body)?;
        let (_, t) = body["result"].as_object().and_then(|m| m.iter().next()).ok_or_else(unreadable)?;
        let (key, label) = match side { PriceSide::Ask => ("a", "ask"), PriceSide::Last => ("c", "last") };
        let p = dec(&t[key][0])?.unwrap_or_else(BigDec::zero);
        // Exchanges::Kraken#get_ask_price / #get_last_price raise on a zero book, naming the pair (kraken.rb:225, :255).
        if p.is_zero() { return Err(VenueError::Rejected(vec![format!("Wrong {label} price for {}: {}", ticker.ticker, p.to_s_f())])); }
        Ok(p)
    }

    async fn add_order(&self, order: &NewOrder) -> Result<String, VenueError> {
        self.wait().await;
        if self.s.borrow().book.iter().any(|(c, _, _)| c == &order.cl_ord_id) {
            return Err(VenueError::Rejected(vec!["EOrder:Duplicate order".into()])); // Kraken refuses a repeated client order id
        }
        self.s.borrow_mut().sent.push(order.clone());
        let hook = self.s.borrow().on_add.clone();
        if let Some(f) = hook { f(); }
        let gate = self.s.borrow().hold_add.clone(); // cloned first: no borrow is held across the await
        if let Some(g) = gate { g.notified().await; }
        let scripted = self.s.borrow_mut().adds.pop_front();
        let outcome = match scripted {
            Some(o) => o,
            None => match self.body("/0/private/AddOrder") {
                Some(b) => match (check(&b), b["result"]["txid"][0].as_str()) {
                    (Err(VenueError::Rejected(e)), _) => AddOutcome::Reject(e),
                    (Err(e), _) => AddOutcome::AmbiguousNotPlaced(match e { VenueError::Ambiguous(m) => m, _ => String::new() }),
                    (Ok(()), Some(t)) => AddOutcome::Accept(t.to_string()),
                    (Ok(()), None) => AddOutcome::AmbiguousNotPlaced("Failed to set Kraken order (order_id is nil)".into()),
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
        self.wait().await;
        let hook = self.s.borrow().on_query.clone();
        if let Some(f) = hook { f(); }
        let mut out = vec![];
        if let Some(body) = self.body("/0/private/QueryOrders") {
            check(&body)?;
            for t in txids { if let Some(raw) = body["result"].get(t) { out.push(parse_order(t, raw)?); } }
        }
        let s = self.s.borrow();
        for t in txids {
            if out.iter().any(|o| &o.txid == t) { continue; }
            if let Some(raw) = s.orders.get(t) { out.push(parse_order(t, raw)?); }
        }
        Ok(out)
    }

    /// `since` is unused: the fake's scripted bodies and book are not time-windowed. Never touches QueryOrders.
    async fn order_by_client_id(&self, cl_ord_id: &str, _since: DateTime<Utc>) -> Result<Option<OrderState>, VenueError> {
        self.wait().await;
        let hook = self.s.borrow().on_lookup.clone();
        if let Some(f) = hook { f(); }
        {
            let mut s = self.s.borrow_mut();
            if s.lookup_failures > 0 { s.lookup_failures -= 1; return Err(VenueError::Transient("ClosedOrders page failed".into())); }
        }
        // OpenOrders first, then ClosedOrders (each repeats its last body, so one read per path).
        for (path, key) in [("/0/private/OpenOrders", "open"), ("/0/private/ClosedOrders", "closed")] {
            let Some(body) = self.body(path) else { continue };
            check(&body)?;
            for (txid, raw) in body["result"][key].as_object().into_iter().flatten() {
                if raw["cl_ord_id"] == cl_ord_id { return parse_order(txid, raw).map(Some); }
            }
        }
        let found = self.s.borrow().book.iter().find(|(c, _, _)| c == cl_ord_id).cloned();
        let Some((_, txid, order)) = found else { return Ok(None) };
        let raw = self.s.borrow().orders.get(&txid).cloned();
        Ok(Some(match raw {
            Some(raw) => parse_order(&txid, &raw)?,
            None => OrderState {
                txid, status: OrderStatus::Open, price: None, amount: None, quote_amount: None,
                amount_exec: BigDec::zero(), quote_amount_exec: BigDec::zero(), limit: matches!(order.kind, OrderKind::Limit { .. }), sell: false,
                pair: Some(order.pair.clone()),
            },
        }))
    }

    async fn fills_from_trades(&self, txids: &[String], _since: DateTime<Utc>) -> Result<Vec<OrderState>, VenueError> {
        self.wait().await;
        // Honeymaker::Clients::Kraken#closed_orders_from_trades: page by ofs until count (max 20 pages), then aggregate.
        let mut trades: Vec<Value> = vec![];
        let mut seen = std::collections::HashSet::new();
        for _ in 0..20 {
            let Some(body) = self.body("/0/private/TradesHistory") else { break };
            check(&body)?;
            let page = body["result"]["trades"].as_object().cloned().unwrap_or_default();
            if page.is_empty() || page.keys().all(|k| seen.contains(k)) { break; } // the script's last page repeats
            for (id, t) in page { if seen.insert(id) { trades.push(t); } }
            if seen.len() as u64 >= body["result"]["count"].as_u64().unwrap_or(0) { break; }
        }
        let mut by: Vec<(String, BigDec, BigDec, bool, bool, Option<String>)> = vec![];
        for trade in trades.iter() {
            let Some(o) = trade["ordertxid"].as_str().filter(|o| txids.iter().any(|t| t == o)) else { continue };
            let (vol, cost) = (dec(&trade["vol"])?.unwrap_or_else(BigDec::zero), dec(&trade["cost"])?.unwrap_or_else(BigDec::zero));
            match by.iter_mut().find(|(t, ..)| t == o) {
                Some(e) => { e.1 = &e.1 + &vol; e.2 = &e.2 + &cost; }
                None => by.push((o.to_string(), vol, cost, trade["ordertype"] == "limit", trade["type"] == "sell", trade["pair"].as_str().map(str::to_string))),
            }
        }
        // The aggregate names its first trade's pair (honeymaker's aggregate_trades), which Rails looks up as a ticker
        // (kraken.rb:833): a pair spelled like the ticker (SOLEUR) resolves, Kraken's own spelling (XXBTZEUR) does not.
        Ok(by.into_iter().map(|(txid, vol, cost, limit, sell, pair)| OrderState {
            txid, status: OrderStatus::Closed, price: cost.div(&vol), amount: None, quote_amount: None, amount_exec: vol, quote_amount_exec: cost, limit, sell, pair,
        }).collect())
    }

    async fn balance(&self, asset_symbol: &str, _all_crypto: bool) -> Result<BigDec, VenueError> {
        self.wait().await;
        let Some(body) = self.body("/0/private/BalanceEx") else { return Err(VenueError::Transient("no scripted BalanceEx".into())) };
        check(&body)?;
        // Rails assigns per row, so with several rows for one asset (ZEUR, EUR.HOLD) the last in body order wins.
        let mut free = BigDec::zero();
        for (code, b) in body["result"].as_object().into_iter().flatten() {
            let head = code.split('.').next().unwrap_or(code);
            let name = ASSET_MAP.iter().find(|(k, _)| *k == head).map(|(_, v)| *v).unwrap_or(head);
            if name == asset_symbol {
                free = &dec(&b["balance"])?.unwrap_or_else(BigDec::zero) - &dec(&b["hold_trade"])?.unwrap_or_else(BigDec::zero);
            }
        }
        Ok(free)
    }
}

#[derive(Clone, Default)]
pub struct FakeFactory(pub FakeVenue);
impl VenueFactory for FakeFactory {
    type V = FakeVenue;
    fn for_bot(&self, _exchange_type: &str, _credentials: Option<Credentials>) -> FakeVenue { self.0.clone() }
}
