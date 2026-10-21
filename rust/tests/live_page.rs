//! What the bot page hears from the engine outside a tick's claim and failure (those: tests/index_resync.rs): a follow-up
//! fill that stops the bot, a tick's sweep that writes a resting order, and the rows of a tick that placed several.
mod common;
use chrono::{DateTime, Utc};
use common::scripted::{ok, script, venue};
use common::seed::{self, BotSpec, TxSpec};
use deltabadger::engine::events::EngineEvent;
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::FixedClock;
use deltabadger::store;
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};

fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const IN: &str = "2026-09-01 10:00:01";
/// The tick of the 10:00 checkpoint already ran: the bot is not due until a week on.
const STAMPED: &str = "2026-09-01T10:00:00.500Z";

struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = deltabadger::venue::alpaca::AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { venue(&self.0) }
}

struct Rig { dir: tempfile::TempDir, s: seed::Seeded, id: i64 }
impl Rig {
    fn new(bot: &BotSpec) -> Self {
        let dir = common::rails_install();
        let o = store::open(&store::Paths::from_env(&|_| None, dir.path())).unwrap();
        let s = seed::seed_alpaca(&o.primary, &seed::cipher());
        let id = seed::insert_bot(&o.primary, &s, bot);
        Rig { dir, s, id }
    }
    fn c(&self) -> rusqlite::Connection { store::open(&store::Paths::from_env(&|_| None, self.dir.path())).unwrap().primary }
    fn order(&self, external_id: &str, external_status: i64) -> i64 {
        seed::insert_tx(&self.c(), &self.s, self.id, &TxSpec { status: 0, external_status: Some(external_status), external_id: Some(external_id.into()), order_type: 0,
            amount: None, quote_amount: Some("60"), price: Some("64000"), quote_amount_exec: None, amount_exec: None, created_at: IN.into() })
    }
    fn engine(&self, t: &ScriptedTransport, now: &str) -> Engine<Factory> {
        let paths = store::Paths::from_env(&|_| None, self.dir.path());
        let lock = deltabadger::lease::lock(&paths, at(now)).unwrap();
        Engine::new(store::open(&paths).unwrap().primary, Factory(t.clone()), seed::cipher(), lock)
    }
    fn render(&self, event: &EngineEvent) -> Vec<(String, String)> {
        let app = common::web::app(self.dir.path(), common::web::SECRET, common::web::TestClock::at("2026-09-08T10:00:00Z"));
        deltabadger::web::bot::live::render(&self.c(), &app, event).unwrap()
    }
}

fn filled(id: &str) -> Value {
    ok(json!({ "id": id, "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "60", "qty": null,
               "filled_qty": "0.0009375", "filled_avg_price": "64000", "limit_price": null }))
}
fn heard(rx: &mut tokio::sync::mpsc::UnboundedReceiver<EngineEvent>) -> Vec<EngineEvent> {
    let mut out = vec![];
    while let Ok(e) = rx.try_recv() { out.push(e); }
    out
}
/// (action, target) of each message.
fn shape(messages: &[(String, String)]) -> Vec<(String, String)> {
    messages.iter().map(|(_, html)| {
        let attr = |name: &str| html.split(&format!("{name}=\"")).nth(1).and_then(|s| s.split('"').next()).unwrap_or_default().to_string();
        (attr("action"), attr("target"))
    }).collect()
}

/// A follow-up fill that spends the amount limit stops the bot (Bot::StopJob → Bot#stop, an update! of the status): Rails'
/// after_update_commit redraws the bars, both buttons and unlocks the columns (app/models/bot.rb:38-40), after the row's own
/// broadcast_updated_order.
#[tokio::test(flavor = "current_thread")]
async fn a_follow_up_fill_that_stops_the_bot_redraws_its_status_and_unlocks_its_settings() {
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(60.0))
        .transient("quote_amount_limit_enabled_at", json!("2026-09-01T10:00:00.000Z")).transient("last_action_job_at", json!(STAMPED)));
    let order = r.order("OTX-P", 0);
    let t = script(json!({ "GET /v2/orders/OTX-P": [filled("OTX-P")] }));
    let mut e = r.engine(&t, "2026-09-01T10:00:06Z");
    let mut rx = e.subscribe_to(EngineEvent::is_live);
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:06Z"))).await.unwrap();
    assert_eq!(heard(&mut rx), [EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order }, EngineEvent::StatusChanged { bot_id: r.id }]);
    let dom = |name: &str| format!("{name}_bots_dca_multi_asset_{}", r.id);
    let pair = |a: &str, t: String| (a.to_string(), t);
    assert_eq!(shape(&r.render(&EngineEvent::StatusChanged { bot_id: r.id })), [pair("replace", dom("status_bar")), pair("replace", dom("progress_bar")),
        pair("replace", dom("status_button")), pair("replace", dom("tile_status_button")), pair("remove_class", dom("columns"))]);
}

/// FetchAndUpdateOpenOrdersJob in the tick (polling::sweep) writes a resting order: its rows are replaced as the follow-up's
/// are (Transaction after_update_commit → Bot#broadcast_updated_order), announced before the tick's end.
#[tokio::test(flavor = "current_thread")]
async fn a_tick_whose_sweep_fills_a_resting_order_announces_it() {
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_action_job_at", json!(STAMPED)));
    let order = r.order("OTX-S", 1);
    // The engine's start polls it once, and that chain ends on a refusal; the sweep a week on finds it filled.
    let refused = json!({ "status": 422, "body": { "code": 42210000, "message": "request refused" } });
    let t = script(json!({ "GET /v2/orders/OTX-S": [refused, filled("OTX-S")] }));
    let mut e = r.engine(&t, "2026-09-01T10:00:06Z");
    let mut rx = e.subscribe_to(EngineEvent::is_live);
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:06Z"))).await.unwrap();
    let first = heard(&mut rx);
    assert_eq!(first, [EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order }], "the start's poll is announced, written or not");
    let ext: i64 = r.c().query_row("SELECT external_status FROM transactions WHERE id = ?1", [order], |r| r.get(0)).unwrap();
    assert_eq!(ext, 1, "still open: only the sweep can fill it");
    run::step(&mut e, &FixedClock(at("2026-09-08T10:00:00.5Z"))).await.unwrap();
    let second = heard(&mut rx);
    let written = second.iter().position(|e| *e == EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order }).unwrap_or_else(|| panic!("{second:?}"));
    assert_eq!(second.last(), Some(&EngineEvent::TickEnded { bot_id: r.id }));
    assert!(written < second.len() - 1);
    let ext: i64 = r.c().query_row("SELECT external_status FROM transactions WHERE id = ?1", [order], |r| r.get(0)).unwrap();
    assert_eq!(ext, 2, "the sweep filled it");
}

/// A tick that placed several orders: each row is rendered once all are in, so none reads as the bot's first. Each removes
/// the placeholder (Rails removes it with the first; a remove of a missing target does nothing in Turbo).
#[test]
fn every_new_row_removes_the_placeholder() {
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let ids = [r.order("OTX-1", 0), r.order("OTX-2", 0)];
    for id in ids {
        let shape = shape(&r.render(&EngineEvent::OrderRecorded { bot_id: r.id, transaction_id: id }));
        assert_eq!(shape[0], ("remove".to_string(), "orders_list_placeholder".to_string()), "row {id}");
    }
}

/// Slice 8c-1: a signal bot's marker, set or lifted by the engine, redraws bots/signals/_unresolved_placement on the bot's
/// own page stream (Bots::Signal#broadcast_unresolved_placement): the Resolve form while it stands, without a token (the
/// renderer has no session), and the hidden placeholder once it is gone. Any other bot hears nothing of it.
#[test]
fn a_signal_bots_marker_redraws_its_unresolved_placement_panel() {
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let c = r.c();
    c.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
               VALUES ('Bots::Signal', 1, ?1, ?2, ?3, ?4, '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
              rusqlite::params![r.s.exchange_id, r.s.user_id, json!({ "base_asset_id": r.s.btc, "quote_asset_id": r.s.quote }).to_string(),
                                json!({ "signal_placement_unresolved": { "id": "cl-1", "at": "2026-09-01T10:00:00Z", "side": "buy", "error": "timeout" } }).to_string()]).unwrap();
    let signal = c.last_insert_rowid();
    let stream = |id: i64| { use base64::Engine; format!("{}:bot_updates", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!("gid://deltabadger/Bots::Signal/{id}"))) };
    let raised = r.render(&EngineEvent::SignalMarkerChanged { bot_id: signal });
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0].0, stream(signal));
    assert_eq!(shape(&raised), [("replace".to_string(), "signal-unresolved-placement".to_string())]);
    let html = &raised[0].1;
    assert!(html.contains(&format!("action=\"/bots/{signal}/signal_resolution\"")) && html.contains("name=\"marker_id\" value=\"cl-1\"") && html.contains("Alpaca"), "{html}");
    assert!(!html.contains("authenticity_token"), "a broadcast carries no token: {html}");
    c.execute("UPDATE bots SET transient_data = '{}' WHERE id = ?1", [signal]).unwrap();
    let lifted = r.render(&EngineEvent::SignalMarkerChanged { bot_id: signal });
    // The partial as Rails renders it on its own (signal_action_resolution records it), not as the settings column indents it.
    assert_eq!(lifted, vec![(stream(signal), "<turbo-stream action=\"replace\" target=\"signal-unresolved-placement\"><template>  <div id=\"signal-unresolved-placement\" hidden></div>\n</template></turbo-stream>".to_string())]);
    assert!(r.render(&EngineEvent::SignalMarkerChanged { bot_id: r.id }).is_empty(), "a basket has no such panel");
}
