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
        let dir = common::baseline_install();
        let o = store::open(&store::Paths::from_env(&|_| None, dir.path())).unwrap();
        let s = seed::seed_alpaca(&o.primary, &seed::cipher());
        let id = seed::insert_bot(&o.primary, &s, bot);
        Rig { dir, s, id }
    }
    fn c(&self) -> rusqlite::Connection { store::open(&store::Paths::from_env(&|_| None, self.dir.path())).unwrap().primary }
    fn order(&self, external_id: &str, external_status: i64) -> i64 { self.order_of(external_id, external_status, 0) }
    fn order_of(&self, external_id: &str, external_status: i64, order_type: i64) -> i64 {
        seed::insert_tx(&self.c(), &self.s, self.id, &TxSpec { status: 0, external_status: Some(external_status), external_id: Some(external_id.into()), order_type,
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
/// (action, target) of each message, without D-LIQUIDATION-ONE-LINE's two selling targets (tests/liquidation_web.rs pins them).
fn shape(messages: &[(String, String)]) -> Vec<(String, String)> {
    messages.iter().map(|(_, html)| {
        let attr = |name: &str| html.split(&format!("{name}=\"")).nth(1).and_then(|s| s.split('"').next()).unwrap_or_default().to_string();
        (attr("action"), attr("target"))
    }).filter(|(_, target)| !target.starts_with("orders_selling_") && !target.starts_with("selling_state_")).collect()
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
    // D-LIQUIDATION-ONE-LINE: the stop's line, newer than the order, is announced by a TickEnded at the polls' end.
    assert_eq!(heard(&mut rx), [EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order }, EngineEvent::StatusChanged { bot_id: r.id }, EngineEvent::TickEnded { bot_id: r.id }]);
    let dom = |name: &str| format!("{name}_bots_dca_multi_asset_{}", r.id);
    let pair = |a: &str, t: String| (a.to_string(), t);
    assert_eq!(shape(&r.render(&EngineEvent::StatusChanged { bot_id: r.id })), [pair("replace", dom("status_bar")), pair("replace", dom("progress_bar")),
        pair("replace", dom("status_button")), pair("replace", dom("tile_status_button")), pair("remove_class", dom("columns"))]);
}

/// A final sell left for the settlement scan stops the bot without a poll or tick to announce the change.
#[tokio::test(flavor = "current_thread")]
async fn a_rotation_settlement_stop_reaches_the_page_stream() {
    use deltabadger::engine::{amount_limit, model};
    use deltabadger::enums::BotStatus;
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("direction", json!("selling")).with("sell_denomination", json!("base"))
        .with("sell_amount", json!(0.001)).with("base_amount_limited", json!(true)).with("base_amount_limit", json!(0.01))
        .transient("base_amount_limit_enabled_at", json!("2026-09-01T10:00:00.000Z")).transient("last_action_job_at", json!(STAMPED)));
    let order = seed::insert_row(&r.c(), &r.s, r.id, r.s.btc, &json!({ "external_id": "OS", "external_status": 2, "side": 1,
        "order_type": 1, "price": "64000", "amount": "0.01", "amount_exec": "0.01", "created_at": IN }));
    r.c().execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3",
        rusqlite::params![format!("$.{}", amount_limit::ROTATION_PENDING), json!([order]).to_string(), r.id]).unwrap();
    let t = script(json!({}));
    let mut e = r.engine(&t, "2026-09-01T10:00:06Z");
    let mut rx = e.subscribe_to(EngineEvent::is_live);
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:06Z"))).await.unwrap();
    assert_eq!(model::load_bot(&r.c(), r.id).unwrap().status, BotStatus::Stopped);
    let events = heard(&mut rx);
    assert_eq!(events, [EngineEvent::StatusChanged { bot_id: r.id }]);
    let messages = r.render(&events[0]);
    assert!(messages.iter().all(|(stream, _)| *stream == format!("user_{}:bot_updates", r.s.user_id)));
    let dom = |name: &str| format!("{name}_bots_dca_multi_asset_{}", r.id);
    let pair = |action: &str, target: String| (action.to_string(), target);
    assert_eq!(shape(&messages), [pair("replace", dom("status_bar")), pair("replace", dom("progress_bar")),
        pair("replace", dom("status_button")), pair("replace", dom("tile_status_button")), pair("remove_class", dom("columns"))]);
    run::step(&mut e, &FixedClock(at("2026-09-01T10:11:06Z"))).await.unwrap();
    assert!(heard(&mut rx).is_empty(), "a settled decision is not announced again");
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
    // Limit orders: a pending market order has no row yet (D-PENDING-MARKET-HIDDEN).
    let ids = [r.order_of("OTX-1", 0, 1), r.order_of("OTX-2", 0, 1)];
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

/// D-PENDING-MARKET-HIDDEN and D-PENDING-MARKET-SETTING: a market order the venue has not finished gets no row and holds the
/// bar at "Setting orders…"; once a follow-up writes it final, its rows are prepended and the bar is redrawn as the schedule's.
/// A pending limit order keeps its "Open order" rows and the schedule's bar.
#[test]
fn a_pending_market_order_shows_setting_orders_until_it_is_final() {
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_action_job_at", json!(STAMPED)));
    let order = r.order("OTX-M", 0);
    let bar = |r: &Rig| r.render(&EngineEvent::StatusChanged { bot_id: r.id }).into_iter().find(|(_, h)| h.contains("status_bar_")).unwrap().1;
    let filters = format!("order_filters_bots_dca_multi_asset_{}", r.id);
    assert_eq!(shape(&r.render(&EngineEvent::OrderRecorded { bot_id: r.id, transaction_id: order })), [("replace".to_string(), filters.clone())], "no row while pending");
    assert!(bar(&r).contains("Setting orders"), "{}", bar(&r));
    let written = r.render(&EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order });
    assert!(shape(&written).iter().all(|(a, _)| a == "replace") && written.iter().all(|(_, h)| !h.contains("timeline_transaction_")), "still pending: no row");
    r.c().execute("UPDATE transactions SET external_status = 2, amount_exec = '0.0009375', quote_amount_exec = '60' WHERE id = ?1", [order]).unwrap();
    let written = r.render(&EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order });
    let shapes = shape(&written);
    assert!(shapes.contains(&("prepend".to_string(), "orders_list".to_string())), "final: its row is prepended: {shapes:?}");
    let bar_now = written.iter().find(|(_, h)| h.contains("status_bar_")).map(|(_, h)| h.clone()).unwrap_or_else(|| panic!("the bar is redrawn: {shapes:?}"));
    assert!(!bar_now.contains("Setting orders"), "{bar_now}");
    let limit = r.order_of("OTX-L", 1, 1);
    let recorded = r.render(&EngineEvent::OrderRecorded { bot_id: r.id, transaction_id: limit });
    assert!(recorded.iter().any(|(_, h)| h.contains(&format!("timeline_transaction_{limit}"))), "a limit order keeps its row");
    assert!(!bar(&r).contains("Setting orders"), "a resting limit order is no setting");
}

/// Slice 7s: the page's status-line request (conditions::want_info, as BroadcastsController#price_limit_info_update leaves it)
/// is served by the engine's pass on a stopped bot, announced with the request's locale and side, and rendered into the price
/// rule's line; the request is gone once served, and the bot's trading state is untouched (D-INFO-READONLY).
#[tokio::test(flavor = "current_thread")]
async fn the_engine_serves_the_price_status_line_and_the_page_hears_it() {
    use deltabadger::engine::conditions;
    let r = Rig::new(&BotSpec { status: deltabadger::enums::BotStatus::Stopped as i64, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
        .with("price_limited", json!(true)).with("price_limit", json!(70000)).with("price_limit_value_condition", json!("below")));
    assert!(conditions::want_info(&r.c(), r.id, Some(r.s.user_id), conditions::PRICE, None, "de").unwrap());
    let t = script(json!({ "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000.5 } } }))] }));
    let mut e = r.engine(&t, "2026-09-02T10:00:00Z");
    let mut rx = e.subscribe_to(EngineEvent::is_live);
    run::step(&mut e, &FixedClock(at("2026-09-02T10:00:00Z"))).await.unwrap();
    let event = EngineEvent::ConditionInfo { bot_id: r.id, rule: conditions::PRICE, side: conditions::BUYING, locale: Some("de".into()) };
    assert!(heard(&mut rx).contains(&event), "announced");
    let bot = deltabadger::engine::model::load_bot(&r.c(), r.id).unwrap();
    assert!(bot.transient.get(conditions::INFO_WANTED_KEY).is_none(), "served: {}", bot.transient);
    assert!(bot.transient.get("price_limit_condition_met_at").is_none(), "D-INFO-READONLY: the page view stamps nothing: {}", bot.transient);
    let shown = r.render(&event);
    assert_eq!(shape(&shown), [("replace".to_string(), "settings-price-limit-info".to_string())]);
    assert!(shown[0].1.contains("text-success") && shown[0].1.contains("64000.5"), "{}", shown[0].1);
}

/// D-INFO-SIDE: each side's line has its own element. The active side's is Rails' `settings-price-limit-info`; a signal bot's
/// sell group is `settings-sell-price-limit-info`; a basket's other side has no line on its page, so nothing is rendered.
#[test]
fn each_sides_status_line_renders_into_its_own_element() {
    use deltabadger::engine::conditions::{self, BUYING, SELLING};
    let info = |met: bool| json!({ "base": "BTC", "quote": "USD", "price": "64000.5", "condition_met": met, "expires_us": i64::MAX });
    let cached = json!({ conditions::INFO_KEY: { "price_limit": { "buying": info(true), "selling": info(false) } } });
    let rules = || BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("price_limited", json!(true)).with("sell_price_limited", json!(true));
    let r = Rig::new(&BotSpec { transient: cached.clone(), ..rules() });
    let c = r.c();
    c.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, started_at, created_at, updated_at) \
               VALUES ('Bots::Signal', 1, ?1, ?2, ?3, ?4, '2026-08-25 09:00:00', '2026-08-25 09:00:00', '2026-08-25 09:00:00')",
              rusqlite::params![r.s.exchange_id, r.s.user_id, json!({ "base_asset_id": r.s.btc, "quote_asset_id": r.s.quote, "price_limited": true, "sell_price_limited": true }).to_string(),
                                cached.to_string()]).unwrap();
    let signal = c.last_insert_rowid();
    let event = |bot_id: i64, side: &'static str| EngineEvent::ConditionInfo { bot_id, rule: conditions::PRICE, side, locale: None };
    let target = |bot_id: i64, side: &'static str| shape(&r.render(&event(bot_id, side))).into_iter().map(|(_, t)| t).collect::<Vec<_>>();
    assert_eq!(target(signal, BUYING), ["settings-price-limit-info"]);
    assert_eq!(target(signal, SELLING), ["settings-sell-price-limit-info"]);
    let sell = r.render(&event(signal, SELLING));
    assert!(sell[0].1.contains("id=\"settings-sell-price-limit-info\"") && !sell[0].1.contains("text-success"), "{}", sell[0].1);
    assert_eq!(target(r.id, BUYING), ["settings-price-limit-info"], "a buying basket's own line");
    assert!(target(r.id, SELLING).is_empty(), "a buying basket shows no sell line");
}

/// Bot::BaseAmountLimitable#handle_base_amount_limit_update: a sell fill of a one-asset seller under the base cap redraws
/// what is left of it on the open page (settings-amount-limit-info), in the order's own broadcast, before any stop.
#[tokio::test(flavor = "current_thread")]
async fn a_sell_fill_redraws_what_is_left_of_the_base_cap() {
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("direction", json!("selling")).with("sell_denomination", json!("base"))
        .with("sell_amount", json!(0.001)).with("base_amount_limited", json!(true)).with("base_amount_limit", json!(0.02))
        .transient("base_amount_limit_enabled_at", json!("2026-09-01T10:00:00.000Z")));
    let order = seed::insert_row(&r.c(), &r.s, r.id, r.s.btc, &json!({ "external_id": "OS", "external_status": 0, "side": 1, "order_type": 0,
        "price": "64000", "amount": "0.01", "created_at": IN }));
    let t = script(json!({ "GET /v2/orders/OS": [ok(json!({ "id": "OS", "status": "filled", "symbol": "BTC/USD", "type": "market", "side": "sell",
        "notional": null, "qty": "0.01", "filled_qty": "0.005", "filled_avg_price": "64000", "limit_price": null }))] }));
    deltabadger::engine::polling::follow_up(&r.c(), &venue(&t), r.id, order, at("2026-09-01T10:00:06Z")).await.unwrap();
    let messages = r.render(&EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order });
    let info = messages.iter().find(|(_, html)| html.contains("target=\"settings-amount-limit-info\"")).unwrap_or_else(|| panic!("{messages:?}"));
    assert!(info.0.ends_with(":bot_updates") && !info.0.starts_with("user_"), "the bot's page stream: {}", info.0);
    assert!(info.1.contains("0.015"), "0.02 less the 0.005 sold: {}", info.1);
}

#[tokio::test(flavor = "current_thread")]
async fn a_reversal_redraws_settings_and_queues_the_metrics_panel() {
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("direction", json!("selling")));
    let app = common::web::app(r.dir.path(), common::web::SECRET, common::web::TestClock::at("2026-09-08T10:00:00Z"))
        .with_figure_source(deltabadger::web::figure::loading::Source::Script(json!({}))).unwrap();
    let event = EngineEvent::Reversed { bot_id: r.id };
    assert!(event.is_live());
    assert!(!event.is_classic());
    let messages = deltabadger::web::bot::live::render(&r.c(), &app, &event).unwrap();
    assert_eq!(shape(&messages), vec![("replace".into(), "settings".into())]);
    assert!(messages[0].1.contains("sell_price_limit"));
    assert!(app.figure_service.panels_pending() > 0, "the metrics redraw uses the existing figure worker");
}

fn transient(r: &Rig, key: &str) -> Value {
    let raw: String = r.c().query_row("SELECT transient_data FROM bots WHERE id = ?1", [r.id], |row| row.get(0)).unwrap();
    serde_json::from_str::<Value>(&raw).unwrap()[key].clone()
}

/// D-OPEN-ORDERS-WAKE (3.0 A20): the orders table's refresh request reaches no venue; the engine's next pass sweeps the bot
/// (stopped: Rails' job runs whatever the status), announces the row it wrote, and serves the request.
#[tokio::test(flavor = "current_thread")]
async fn an_open_orders_refresh_is_swept_by_the_engine_and_never_by_the_request() {
    use deltabadger::engine::polling;
    let r = Rig::new(&BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }.transient("last_action_job_at", json!(STAMPED)));
    let t = script(json!({ "GET /v2/orders/OTX-R": [filled("OTX-R")] }));
    let mut e = r.engine(&t, "2026-09-01T10:00:06Z");
    let mut rx = e.subscribe_to(EngineEvent::is_live);
    // The first pass (and its start-up polls) runs before the order exists: only the refresh can read it.
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:06Z"))).await.unwrap();
    heard(&mut rx);
    let order = r.order("OTX-R", 1);
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:20Z"))).await.unwrap();
    assert!(t.requests().is_empty() && heard(&mut rx).is_empty(), "without a request nothing reads the stopped bot's order");
    assert!(polling::want_refresh(&r.c(), r.id, r.s.user_id).unwrap());
    assert!(!polling::want_refresh(&r.c(), r.id, r.s.user_id + 1).unwrap(), "another user's bot is not found");
    assert!(!polling::want_refresh(&r.c(), r.id + 100, r.s.user_id).unwrap(), "nor a bot that does not exist");
    assert_eq!(transient(&r, polling::REFRESH_KEY), json!(1));
    assert!(t.requests().is_empty(), "the request itself reads no venue");
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:36Z"))).await.unwrap();
    assert_eq!(t.requests().len(), 1, "the engine's sweep read the order");
    let ext: i64 = r.c().query_row("SELECT external_status FROM transactions WHERE id = ?1", [order], |r| r.get(0)).unwrap();
    assert_eq!(ext, 2, "the sweep filled it");
    assert_eq!(heard(&mut rx), [EngineEvent::OrderWritten { bot_id: r.id, transaction_id: order }], "the page hears the row the sweep wrote");
    assert_eq!(transient(&r, polling::REFRESH_KEY), Value::Null, "served");
    run::step(&mut e, &FixedClock(at("2026-09-01T10:01:06Z"))).await.unwrap();
    assert_eq!(t.requests().len(), 1, "a served request is not swept again");
}

/// A request made while the engine served an older one survives the older one's acknowledgement.
#[test]
fn a_refresh_request_made_meanwhile_survives_the_older_ones_acknowledgement() {
    use deltabadger::engine::polling;
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00"));
    let c = r.c();
    assert!(polling::want_refresh(&c, r.id, r.s.user_id).unwrap());
    assert_eq!(polling::refresh_claims(&c).unwrap(), [(r.id, 1)]);
    assert!(polling::want_refresh(&c, r.id, r.s.user_id).unwrap());
    polling::ack_refresh(&c, r.id, 1).unwrap();
    assert_eq!(transient(&r, polling::REFRESH_KEY), json!(2));
    polling::ack_refresh(&c, r.id, 2).unwrap();
    assert_eq!(transient(&r, polling::REFRESH_KEY), Value::Null);
}

/// success_or_kill: a refresh whose read fails is logged and served; the bot is neither failed nor stopped by it.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_refresh_is_served_and_leaves_the_bot_as_it_was() {
    use deltabadger::engine::polling;
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").transient("last_action_job_at", json!(STAMPED)));
    let t = script(json!({ "GET /v2/orders/OTX-F": [{ "status": 503, "body": { "message": "unavailable" } }] }));
    let mut e = r.engine(&t, "2026-09-01T10:00:06Z");
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:06Z"))).await.unwrap();
    let order = r.order("OTX-F", 1);
    let status_before: i64 = r.c().query_row("SELECT status FROM bots WHERE id = ?1", [r.id], |r| r.get(0)).unwrap();
    assert!(polling::want_refresh(&r.c(), r.id, r.s.user_id).unwrap());
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:36Z"))).await.unwrap();
    assert_eq!(t.requests().len(), 1);
    let (status, ext): (i64, i64) = r.c().query_row("SELECT b.status, t.external_status FROM bots b JOIN transactions t ON t.bot_id = b.id WHERE t.id = ?1", [order], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    assert_eq!((status, ext), (status_before, 1), "nothing written, the bot as it was");
    let failures: i64 = r.c().query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1 AND level = 'error'", [r.id], |r| r.get(0)).unwrap_or(0);
    assert_eq!(failures, 0, "no failure is recorded against the bot");
    assert_eq!(transient(&r, polling::REFRESH_KEY), Value::Null, "served all the same");
}

/// A refresh that commits a fill and then raises (an order whose status is unknown, after one that filled): the fill stays,
/// is announced, and its amount-limit stop runs (Rails' StopJob from the fill callback); the request is served.
#[tokio::test(flavor = "current_thread")]
async fn a_refresh_that_fills_then_raises_keeps_the_fill_and_its_amount_limit_stop() {
    use deltabadger::engine::{model, polling};
    use deltabadger::enums::BotStatus;
    let r = Rig::new(&BotSpec::weekly(60.0, "2026-09-01 10:00:00").with("quote_amount_limited", json!(true)).with("quote_amount_limit", json!(60.0))
        .transient("quote_amount_limit_enabled_at", json!("2026-09-01T10:00:00.000Z")).transient("last_action_job_at", json!(STAMPED)));
    let mystery = ok(json!({ "id": "OTX-B", "status": "mystery", "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "60", "qty": null,
                             "filled_qty": "0", "filled_avg_price": null, "limit_price": null }));
    let t = script(json!({ "GET /v2/orders/OTX-A": [filled("OTX-A")], "GET /v2/orders/OTX-B": [mystery] }));
    let mut e = r.engine(&t, "2026-09-01T10:00:06Z");
    let mut rx = e.subscribe_to(EngineEvent::is_live);
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:06Z"))).await.unwrap();
    heard(&mut rx);
    let (a, b) = (r.order("OTX-A", 1), r.order("OTX-B", 1));
    assert!(polling::want_refresh(&r.c(), r.id, r.s.user_id).unwrap());
    run::step(&mut e, &FixedClock(at("2026-09-01T10:00:36Z"))).await.unwrap();
    let ext = |id: i64| r.c().query_row("SELECT external_status FROM transactions WHERE id = ?1", [id], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!((ext(a), ext(b)), (2, 1), "the first fill is committed before the second raises");
    assert_eq!(model::load_bot(&r.c(), r.id).unwrap().status, BotStatus::Stopped, "the fill reached the limit: the bot stops");
    let events = heard(&mut rx);
    assert!(events.contains(&EngineEvent::OrderWritten { bot_id: r.id, transaction_id: a }) && events.contains(&EngineEvent::StatusChanged { bot_id: r.id }), "{events:?}");
    assert_eq!(transient(&r, polling::REFRESH_KEY), Value::Null, "served");
}

/// A refresh's failure is logged by its kind: a venue's answer (which can name the account) never reaches the log line.
#[test]
fn a_refresh_failure_is_logged_without_the_venues_text() {
    use deltabadger::engine::{polling::PollFailure, EngineError};
    for failure in [PollFailure::General("Account alice@example.com is disabled".into()), PollFailure::Transient("alice@example.com".into()),
                    PollFailure::RateLimited("alice".into())] {
        assert!(!failure.kind().contains("alice"), "{}", failure.kind());
    }
    assert!(!EngineError::Data("alice@example.com".into()).kind().contains("alice"));
}
