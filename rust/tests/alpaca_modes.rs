//! Paper and live Alpaca keys through the real factory: every order and every recovery lookup reaches only the host of
//! the mode (and the account) the order was sent under. Paper, live and market data are three local servers;
//! `Urls::pick` inside `LiveFactory` makes the choice, so a crossed wire shows as a request on the wrong server.
//!
//! The double-buy regressions: an order Alpaca accepted whose reply was lost leaves an intent; a lookup on another host
//! or another account would truthfully answer Alpaca's not-found envelope, and past the absence margin that would clear
//! the intent and let the bot buy again. Recovery must never ask there.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::placement::{self, Recovery};
use deltabadger::engine::tick::{self, Attempts, TickOutcome};
use deltabadger::engine::{model, Clock};
use deltabadger::store;
use deltabadger::venue::alpaca::{Hosts, LiveFactory};
use deltabadger::venue::live::LiveVenue;
use deltabadger::venue::VenueFactory;
use serde_json::json;
use std::sync::{Arc, Mutex};
use wiremock::matchers::{any, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Moving(Mutex<DateTime<Utc>>);
impl Clock for Moving { fn now(&self) -> DateTime<Utc> { *self.0.lock().unwrap() } }

const T0: &str = "2026-09-01T10:00:00.5Z";
const NOT_FOUND: &str = r#"{"code":40410000,"message":"order not found for the client order id"}"#;

struct Rig { _d: tempfile::TempDir, o: store::Opened, id: i64, paper: MockServer, live: MockServer, _data: MockServer, clock: Arc<Moving>, factory: LiveFactory }

impl Rig {
    /// One weekly crypto bot whose key is in `mode`; market data answers on its own server.
    async fn new(mode: &str) -> Rig {
        let (d, o, s) = common::install_alpaca();
        let id = seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
        o.primary.execute("UPDATE api_keys SET passphrase = ?1", [seed::cipher().encrypt(mode)]).unwrap();
        let (paper, live, data) = (MockServer::start().await, MockServer::start().await, MockServer::start().await);
        Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "quotes": { "BTC/USD": { "ap": 64321.5, "bp": 64300.25 } } }))).mount(&data).await;
        Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/trades"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "trades": { "BTC/USD": { "p": 64310.75 } } }))).mount(&data).await;
        let clock = Arc::new(Moving(Mutex::new(T0.parse().unwrap())));
        let factory = LiveFactory::with_hosts(Hosts { paper: paper.uri(), live: live.uri(), data: data.uri() }).with_clock(clock.clone());
        Rig { _d: d, o, id, paper, live, _data: data, clock, factory }
    }
    fn server(&self, mode: &str) -> &MockServer { if mode == "live" { &self.live } else { &self.paper } }
    /// The account and positions a tick reads, on `mode`'s server.
    async fn account(&self, mode: &str) {
        let s = self.server(mode);
        Mock::given(method("GET")).and(path("/v2/account"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "cash": "100000", "non_marginable_buying_power": "100000" }))).mount(s).await;
        Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(json!([]))).mount(s).await;
    }
    /// The bot's venue as the engine builds it now: from the stored key, through the factory.
    fn venue(&self) -> LiveVenue {
        let bot = model::load_bot(&self.o.primary, self.id).unwrap();
        self.factory.for_bot("Exchanges::Alpaca", model::credentials_for(&self.o.primary, &seed::cipher(), &bot).unwrap())
    }
    fn at(&self, t: DateTime<Utc>) { *self.clock.0.lock().unwrap() = t; }
    async fn tick(&self) -> TickOutcome { tick::tick(&self.o.primary, &self.venue(), self.id, &*self.clock, &mut Attempts::default()).await.unwrap() }
    /// A process that started at `restart` looks the intent up at `now`.
    async fn recover(&self, restart: DateTime<Utc>, now: DateTime<Utc>) -> Recovery {
        self.at(now);
        let bot = model::load_bot(&self.o.primary, self.id).unwrap();
        placement::recover_since(&self.o.primary, &self.venue(), &bot, &*self.clock, restart).await.unwrap()
    }
    fn intent(&self) -> Option<serde_json::Value> { model::load_bot(&self.o.primary, self.id).unwrap().rust_placement() }
    async fn posts(&self, mode: &str) -> usize { self.server(mode).received_requests().await.unwrap().iter().filter(|r| r.method.as_str() == "POST").count() }
    async fn lookups(&self, mode: &str) -> usize {
        self.server(mode).received_requests().await.unwrap().iter().filter(|r| r.url.path() == "/v2/orders:by_client_order_id").count()
    }
    fn set_key(&self, column: &str, value: &str) {
        self.o.primary.execute(&format!("UPDATE api_keys SET {column} = ?1"), [seed::cipher().encrypt(value)]).unwrap();
    }
    /// The order is sent and Alpaca's reply is lost (a 5xx to a placement is ambiguous): the intent stays.
    async fn lost_order(&self, mode: &str) -> serde_json::Value {
        self.account(mode).await;
        Mock::given(method("POST")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(500)).up_to_n_times(1).with_priority(1).mount(self.server(mode)).await;
        let out = self.tick().await;
        assert!(matches!(out, TickOutcome::AwaitingReconciliation | TickOutcome::RetryAfter(_)), "{mode}: {out:?}");
        assert_eq!(self.posts(mode).await, 1, "{mode}: sent to its own mode's host");
        self.intent().expect("an unresolved intent")
    }
}

fn other(mode: &str) -> &'static str { if mode == "live" { "paper" } else { "live" } }
fn t0() -> DateTime<Utc> { T0.parse().unwrap() }

/// T3: a tick sends a paper key's order to the paper host only and a live key's to the live host only.
#[tokio::test(flavor = "current_thread")]
async fn each_keys_order_reaches_only_its_own_modes_host() {
    for mode in ["paper", "live"] {
        let r = Rig::new(mode).await;
        r.account(mode).await;
        Mock::given(method("POST")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "OTX-1", "status": "pending_new" }))).mount(r.server(mode)).await;
        assert!(matches!(r.tick().await, TickOutcome::Done { placed: true }), "{mode}");
        assert_eq!(r.posts(mode).await, 1, "{mode}");
        assert!(r.server(other(mode)).received_requests().await.unwrap().is_empty(), "{mode}: nothing reaches the other mode's host");
    }
}

/// R1 + R4: the intent records the mode and the key's fingerprint before the send; after a restart the order is looked up
/// on the host it went to, found there, and recorded once. No second order.
#[tokio::test(flavor = "current_thread")]
async fn a_lost_reply_is_recovered_on_the_host_it_was_sent_to() {
    for mode in ["paper", "live"] {
        let r = Rig::new(mode).await;
        let intent = r.lost_order(mode).await;
        assert_eq!(intent["alpaca_live"], json!(mode == "live"), "{mode}: {intent}");
        assert!(intent["key_fingerprint"].as_str().is_some_and(|f| f.len() == 64), "{mode}: {intent}");
        Mock::given(method("GET")).and(path("/v2/orders:by_client_order_id"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "OTX-1", "client_order_id": intent["cl_ord_id"], "asset_class": "crypto", "status": "filled", "symbol": "BTC/USD", "side": "buy", "type": "market",
                "notional": "60", "filled_qty": "0.0009", "filled_avg_price": "64000" }))).mount(r.server(mode)).await;
        let restart = t0() + Duration::seconds(600);
        assert!(matches!(r.recover(restart, restart + Duration::seconds(1200)).await, Recovery::Recorded(_)), "{mode}");
        assert_eq!(r.o.primary.query_row("SELECT count(*) FROM transactions WHERE external_id = 'OTX-1'", [], |row| row.get::<_, i64>(0)).unwrap(), 1, "{mode}");
        assert_eq!((r.posts(mode).await, r.lookups(mode).await), (1, 1), "{mode}");
        assert!(r.server(other(mode)).received_requests().await.unwrap().is_empty(), "{mode}: the other host is never asked");
    }
}

/// R1: Alpaca's not-found envelope from the host the order went to proves absence past the margin; the next order goes
/// to that same host.
#[tokio::test(flavor = "current_thread")]
async fn absence_proven_on_the_sending_host_settles_and_the_next_order_goes_there() {
    for mode in ["paper", "live"] {
        let r = Rig::new(mode).await;
        r.lost_order(mode).await;
        Mock::given(method("GET")).and(path("/v2/orders:by_client_order_id")).respond_with(ResponseTemplate::new(404).set_body_string(NOT_FOUND)).mount(r.server(mode)).await;
        Mock::given(method("POST")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "OTX-2", "status": "pending_new" }))).with_priority(5).mount(r.server(mode)).await;
        let restart = t0() + Duration::seconds(600);
        assert!(matches!(r.recover(restart, restart + Duration::seconds(1200)).await, Recovery::NotPlaced), "{mode}");
        r.at(t0() + Duration::days(7));
        assert!(matches!(r.tick().await, TickOutcome::Done { placed: true }), "{mode}");
        assert_eq!(r.posts(mode).await, 2, "{mode}");
        assert!(r.server(other(mode)).received_requests().await.unwrap().is_empty(), "{mode}");
    }
}

/// Asserts an intent the current key must not look up: Pending at every try past the margin, nothing sent anywhere, the
/// intent untouched, and the bot places nothing.
async fn stays_pending(r: &Rig, label: &str, intent: &serde_json::Value) {
    let restart = t0() + Duration::seconds(600);
    for later in [1200, 3600, 86_400] {
        assert!(matches!(r.recover(restart, restart + Duration::seconds(later)).await, Recovery::Pending), "{label} +{later}s");
    }
    r.at(t0() + Duration::days(7));
    let out = r.tick().await;
    assert!(matches!(out, TickOutcome::AwaitingReconciliation), "{label}: {out:?}");
    assert_eq!(r.intent().as_ref(), Some(intent), "{label}: the intent is kept as it was");
    for mode in ["paper", "live"] {
        assert_eq!(r.lookups(mode).await, 0, "{label}: no lookup on {mode}");
        assert_eq!(r.posts(mode).await, usize::from(r.intent_mode() == mode), "{label}: no second order on {mode}");
    }
}

impl Rig {
    fn intent_mode(&self) -> &'static str { if self.intent().is_some_and(|i| i["alpaca_live"] == json!(true)) { "live" } else { "paper" } }
}

/// Every host answers Alpaca's genuine not-found envelope: whoever is asked about an order it does not hold says so.
async fn every_host_says_not_found(r: &Rig) {
    for s in [&r.paper, &r.live] {
        Mock::given(method("GET")).and(path("/v2/orders:by_client_order_id")).respond_with(ResponseTemplate::new(404).set_body_string(NOT_FOUND)).mount(s).await;
    }
}

/// R2: the key's mode flipped after the order was sent (outside the web, which refuses while an order is unresolved).
/// The other host would truthfully say not-found; recovery never asks it.
#[tokio::test(flavor = "current_thread")]
async fn a_mode_flipped_after_the_send_never_clears_the_intent() {
    for mode in ["paper", "live"] {
        let r = Rig::new(mode).await;
        let intent = r.lost_order(mode).await;
        r.set_key("passphrase", other(mode));
        r.account(other(mode)).await;
        every_host_says_not_found(&r).await;
        stays_pending(&r, &format!("{mode} -> {}", other(mode)), &intent).await;
        // Back to the mode it was sent in: recovery asks that host again.
        r.set_key("passphrase", mode);
        let restart = t0() + Duration::days(8);
        assert!(matches!(r.recover(restart, restart + Duration::seconds(1200)).await, Recovery::NotPlaced), "{mode}");
        assert_eq!((r.lookups(mode).await, r.lookups(other(mode)).await), (1, 0), "{mode}");
    }
}

/// R5: the key was replaced by another account's key in the same mode after the order was sent. That account would say
/// not-found; recovery never asks it.
#[tokio::test(flavor = "current_thread")]
async fn a_key_replaced_after_the_send_never_clears_the_intent() {
    for mode in ["paper", "live"] {
        let r = Rig::new(mode).await;
        let intent = r.lost_order(mode).await;
        let original: String = model::credentials_for(&r.o.primary, &seed::cipher(), &model::load_bot(&r.o.primary, r.id).unwrap()).unwrap().unwrap().key;
        r.set_key("key", "PK-ANOTHER-ACCOUNT");
        every_host_says_not_found(&r).await;
        stays_pending(&r, &format!("{mode} replaced"), &intent).await;
        r.set_key("key", &original);
        let restart = t0() + Duration::days(8);
        assert!(matches!(r.recover(restart, restart + Duration::seconds(1200)).await, Recovery::NotPlaced), "{mode}");
        assert_eq!(r.lookups(mode).await, 1, "{mode}");
    }
}

/// R3: an intent written before the mode and fingerprint were recorded was sent paper (that build sent every Alpaca order
/// to the paper host). A paper key recovers it on the paper host; a live key never looks it up.
#[tokio::test(flavor = "current_thread")]
async fn an_intent_from_the_paper_only_build_is_looked_up_on_the_paper_host_only() {
    let strip = |r: &Rig| { r.o.primary.execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_placement.alpaca_live', '$.rust_placement.key_fingerprint')", []).unwrap(); r.intent().unwrap() };
    let r = Rig::new("paper").await;
    r.lost_order("paper").await;
    strip(&r);
    every_host_says_not_found(&r).await;
    let restart = t0() + Duration::seconds(600);
    assert!(matches!(r.recover(restart, restart + Duration::seconds(1200)).await, Recovery::NotPlaced));
    assert_eq!((r.lookups("paper").await, r.lookups("live").await), (1, 0));

    let r = Rig::new("paper").await;
    r.lost_order("paper").await;
    let intent = strip(&r);
    r.set_key("passphrase", "live");
    r.account("live").await;
    every_host_says_not_found(&r).await;
    stays_pending(&r, "legacy intent, live key", &intent).await;
}

/// No server here answers anything it was not set up for.
#[tokio::test(flavor = "current_thread")]
async fn the_rig_reaches_no_real_host() {
    let r = Rig::new("live").await;
    Mock::given(any()).respond_with(ResponseTemplate::new(500)).expect(0).mount(&r.paper).await;
    let LiveVenue::Alpaca(v) = r.venue() else { panic!("an Alpaca venue") };
    assert_eq!(v.urls().trading, r.live.uri());
    assert!(!v.urls().data.contains("alpaca.markets"));
}
