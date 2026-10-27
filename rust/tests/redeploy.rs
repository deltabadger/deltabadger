//! Slice 6c-4: the redeploy engine (Yes taken by the engine, the offer, the fold, D-6C-12 sizing), the decline and the
//! Clear services, and REDEPLOY rows settled on stopped bots.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::scripted::{accepted, ok};
use common::seed::{self, BotSpec};
use deltabadger::engine::{model, redeploy, run, FixedClock};
use deltabadger::ruby::BigDec;
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::ScriptedTransport;
use serde_json::{json, Value};
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
const T0: &str = "2026-09-01T12:30:00Z";
fn one<T: rusqlite::types::FromSql>(c: &rusqlite::Connection, sql: &str) -> T { c.query_row(sql, [], |r| r.get(0)).unwrap() }
fn d(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

/// A stopped 50/50 BTC/ETH basket worth `btc` and `eth` USD at 64000 and 2500, whose sale of an exited SOL holding banked 100.
fn proceeds_bot(btc: &str, eth: &str) -> (tempfile::TempDir, store::Opened, i64, seed::Seeded, i64) {
    let (dir, o, s) = common::install_alpaca();
    let (eth_id, sol) = seed::add_eth_sol(&o.primary, &s);
    let id = seed::insert_bot(&o.primary, &s, &BotSpec { status: 2, ..BotSpec::weekly(60.0, "2026-09-01 10:00:00") }
        .with("rebalance_enabled", json!(false)).weights(&[(s.btc, 0.5), (eth_id, 0.5)]));
    deltabadger::engine::basket::refresh_composition(&o.primary, &model::load_bot(&o.primary, id).unwrap(), at("2026-09-01T09:00:00Z")).unwrap().unwrap();
    let qty = |value: &str, price: &str| d(value).div(&d(price)).unwrap().to_s_f();
    for (asset, value, price, n) in [(s.btc, btc, "64000", 1), (eth_id, eth, "2500", 2), (sol, "100", "100", 3)] {
        seed::insert_row(&o.primary, &s, id, asset, &json!({ "external_status": 2, "external_id": format!("OHELD-{n}"), "amount": qty(value, price),
            "amount_exec": qty(value, price), "quote_amount": value, "quote_amount_exec": value, "price": price, "created_at": "2026-09-01 10:00:01" }));
    }
    let sold = seed::insert_row(&o.primary, &s, id, sol, &json!({ "external_status": 2, "external_id": "OSOLD", "side": 1, "amount": "1", "amount_exec": "1",
        "quote_amount": "100", "quote_amount_exec": "100", "price": "100", "created_at": "2026-09-01 10:00:02" }));
    o.primary.execute("UPDATE transactions SET transaction_type='LIQUIDATION' WHERE id=?1", [sold]).unwrap();
    (dir, o, id, s, eth_id)
}
fn script(cash: &str, over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v1beta3/crypto/us/latest/quotes": [ok(json!({ "quotes": { "BTC/USD": { "ap": 64000, "bp": 64000 }, "ETH/USD": { "ap": 2500, "bp": 2500 }, "SOL/USD": { "ap": 100, "bp": 100 } } }))],
        "GET /v1beta3/crypto/us/latest/trades": [ok(json!({ "trades": { "BTC/USD": { "p": 64000 }, "ETH/USD": { "p": 2500 }, "SOL/USD": { "p": 100 } } }))],
        "POST /v2/orders": [accepted("OTX-1"), accepted("OTX-2"), accepted("OTX-3")],
        "GET /v2/account": [ok(json!({ "cash": cash, "non_marginable_buying_power": cash }))],
        "GET /v2/positions": [ok(json!([]))],
    });
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue(t: &ScriptedTransport) -> AlpacaVenue<ScriptedTransport> { AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper"))) }
struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _t: &str, _c: Option<deltabadger::crypto::Credentials>) -> Self::V { venue(&self.0) }
}
fn engine(d: &tempfile::TempDir, o: store::Opened, t: &ScriptedTransport) -> run::Engine<Factory> {
    let paths = store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
    run::Engine::new(o.primary, Factory(t.clone()), seed::cipher(), lock)
}
fn request(c: &rusqlite::Connection, id: i64) {
    model::merge_transient_compact(c, id, &[(redeploy::REQUEST_KEY, json!({"id":"yes-1","at":at(T0).timestamp()}))]).unwrap();
}
fn logs(c: &rusqlite::Connection) -> Vec<(String, Value)> {
    c.prepare("SELECT event,details FROM bot_activity_logs ORDER BY id").unwrap().query_map([], |r| {
        let details: String = r.get(1)?;
        Ok((r.get(0)?, serde_json::from_str(&details).unwrap()))
    }).unwrap().map(Result::unwrap).collect()
}
fn offer(c: &rusqlite::Connection, id: i64) -> f64 {
    let s = deltabadger::figures::db::Subject::load(c, id).unwrap();
    let m = deltabadger::figures::walk::metrics(c, &s, deltabadger::figures::at::At::from_utc(at(T0)).unwrap()).unwrap();
    redeploy::offer(c, &s, &m).unwrap().to_s_f().parse().unwrap()
}
async fn yes(e: &mut run::Engine<Factory>, id: i64, when: DateTime<Utc>) {
    request(&e.primary, id);
    run::run_commands(e, &FixedClock(when)).await.unwrap();
    assert!(model::load_bot(&e.primary, id).unwrap().transient[redeploy::REQUEST_KEY].is_null(), "compare-cleared");
}

#[tokio::test(flavor = "current_thread")]
async fn yes_buys_the_underweight_member_with_the_offer() {
    let (d, o, id, _, _) = proceeds_bot("640", "100");
    assert_eq!(offer(&o.primary, id), 100.0);
    let t = script("100000", json!({}));
    let mut e = engine(&d, o, &t);
    yes(&mut e, id, at(T0)).await;
    let posted = t.posted_orders();
    assert_eq!(posted.len(), 1);
    assert_eq!((posted[0]["symbol"].as_str(), posted[0]["side"].as_str(), posted[0]["type"].as_str(), posted[0]["notional"].as_str()), (Some("ETH/USD"), Some("buy"), Some("market"), Some("100.00")));
    assert_eq!(one::<String>(&e.primary, "SELECT transaction_type FROM transactions WHERE external_id='OTX-1'"), "REDEPLOY");
    assert!(logs(&e.primary).iter().any(|(event, _)| event == "redeploy_placed"));
    assert!(model::load_bot(&e.primary, id).unwrap().transient[redeploy::PENDING_KEY].is_null());
}

/// A No that arrives while Yes waits is refused; a No that lands first leaves Yes nothing to buy.
#[tokio::test(flavor = "current_thread")]
async fn yes_and_no_race_in_either_order() {
    for no_first in [false, true] {
        let (d, o, id, _, _) = proceeds_bot("640", "100");
        let user: i64 = one(&o.primary, "SELECT user_id FROM bots");
        if no_first { assert_eq!(redeploy::decline(&o.primary, id, user, at(T0)).unwrap(), redeploy::Declined::Declined); }
        request(&o.primary, id);
        if !no_first { assert_eq!(redeploy::decline(&o.primary, id, user, at(T0)).unwrap(), redeploy::Declined::Refused); }
        let t = script("100000", json!({}));
        let mut e = engine(&d, o, &t);
        run::run_commands(&mut e, &FixedClock(at(T0))).await.unwrap();
        assert_eq!(t.posted_orders().len(), usize::from(!no_first), "no_first={no_first}");
        let offset: f64 = one(&e.primary, "SELECT CAST(redeploy_declined_offset AS REAL) FROM bots");
        assert_eq!(offset, if no_first { 100.0 } else { 0.0 });
        let events: Vec<_> = logs(&e.primary).into_iter().collect();
        if no_first {
            assert!(events.contains(&("redeploy_not_started".into(), json!({"reason":["nothing_to_redeploy"]}))), "{events:?}");
        } else {
            assert!(events.contains(&("redeploy_decline_refused".into(), json!({"reason":["redeploy_in_flight"]}))), "{events:?}");
            // In flight now (a waiting row): a second No is still refused.
            assert_eq!(redeploy::decline(&e.primary, id, user, at(T0)).unwrap(), redeploy::Declined::Refused);
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_share_under_the_floor_folds_into_the_largest() {
    let (d, o, id, _, eth) = proceeds_bot("305", "395");
    o.primary.execute("UPDATE tickers SET minimum_quote_size='10' WHERE base_asset_id=?1", [eth]).unwrap();
    let t = script("100000", json!({}));
    let mut e = engine(&d, o, &t);
    yes(&mut e, id, at(T0)).await;
    let posted = t.posted_orders();
    assert_eq!(posted.len(), 1);
    assert_eq!((posted[0]["symbol"].as_str(), posted[0]["notional"].as_str()), (Some("BTC/USD"), Some("100.00")));
    assert!(logs(&e.primary).contains(&("redeploy_folded".into(), json!({"into":"BTC","amount":5.0,"from":["ETH"]}))), "{:?}", logs(&e.primary));
}

#[tokio::test(flavor = "current_thread")]
async fn nothing_placeable_is_below_minimums() {
    let (d, o, id, _, _) = proceeds_bot("640", "100");
    o.primary.execute("UPDATE tickers SET minimum_base_size='1000'", []).unwrap();
    let t = script("100000", json!({}));
    let mut e = engine(&d, o, &t);
    yes(&mut e, id, at(T0)).await;
    assert!(t.posted_orders().is_empty());
    assert_eq!(logs(&e.primary), vec![("redeploy_skipped".into(), json!({"base":"ETH","reason":"below_minimum"})),
                                      ("redeploy_below_minimums".into(), json!({"reason":["below_minimums"]}))]);
}

#[tokio::test(flavor = "current_thread")]
async fn free_quote_caps_the_amount() {
    let (d, o, id, _, _) = proceeds_bot("640", "100");
    let t = script("40", json!({}));
    let mut e = engine(&d, o, &t);
    yes(&mut e, id, at(T0)).await;
    assert_eq!(t.posted_orders()[0]["notional"], "40.00");
}

/// D-6C-5: every abandoned base is named. Clear removes the halt; the abandoned spend still counts, so the offer stands.
#[tokio::test(flavor = "current_thread")]
async fn abandoned_redeploys_halt_naming_every_base_and_clear_keeps_the_offer() {
    use deltabadger::venue::fake::{FakeFactory, FakeVenue};
    let (d, o, id, s, eth) = proceeds_bot("640", "100");
    for (asset, ext) in [(s.btc, "LOST-1"), (eth, "LOST-2")] {
        let row = seed::insert_row(&o.primary, &s, id, asset, &json!({"external_id":ext,"external_status":1,"amount":"0.001","quote_amount":"20","price":"20000","created_at":"2026-08-01 10:00:02"}));
        o.primary.execute("UPDATE transactions SET transaction_type='REDEPLOY' WHERE id=?1", [row]).unwrap();
    }
    assert_eq!(offer(&o.primary, id), 100.0, "a waiting buy has spent nothing yet");
    let paths = store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
    let mut e = run::Engine::new(o.primary, FakeFactory(FakeVenue::new()), seed::cipher(), lock);
    for minutes in [0, 10, 20] { run::step(&mut e, &FixedClock(at(T0) + Duration::minutes(minutes))).await.unwrap(); }
    assert_eq!(one::<i64>(&e.primary, "SELECT count(*) FROM transactions WHERE transaction_type='REDEPLOY' AND external_status=4"), 2);
    let halts: Vec<_> = logs(&e.primary).into_iter().filter(|(event, _)| event == "redeploy_ambiguous").collect();
    assert_eq!(halts, vec![("redeploy_ambiguous".into(), json!({"base":"BTC, ETH","reason":"exchange stopped reporting the order"}))]);
    let bot = model::load_bot(&e.primary, id).unwrap();
    let nonce = bot.transient[redeploy::PENDING_KEY]["id"].as_str().unwrap().to_string();
    assert_eq!(bot.transient[redeploy::PENDING_KEY]["state"], "ambiguous");
    assert!(nonce.len() == 12 && nonce.bytes().all(|b| b.is_ascii_hexdigit()));
    let halted = offer(&e.primary, id);
    assert_eq!(halted, 60.0, "an abandoned buy counts what it asked for");
    assert_eq!(redeploy::resolve(&e.primary, id, bot.user_id, "stale-tab", at(T0)).unwrap(), None);
    assert!(model::load_bot(&e.primary, id).unwrap().transient[redeploy::PENDING_KEY].is_object(), "a stale nonce clears nothing");
    assert_eq!(redeploy::resolve(&e.primary, id, bot.user_id, &nonce, at(T0)).unwrap(), None);
    let bot = model::load_bot(&e.primary, id).unwrap();
    assert!(bot.transient[redeploy::PENDING_KEY].is_null());
    assert_eq!(offer(&e.primary, id), halted, "abandon -> clear -> offer unchanged");
    assert!(!deltabadger::engine::composition::redeploy_in_flight(&e.primary, &bot).unwrap());
    assert_eq!(one::<f64>(&e.primary, "SELECT CAST(redeploy_declined_offset AS REAL) FROM bots"), 0.0);
}

#[tokio::test(flavor = "current_thread")]
async fn a_locked_member_is_skipped_with_a_log_line_only() {
    for all in [false, true] {
        let (d, o, id, s, eth) = proceeds_bot("640", "100");
        o.primary.execute("UPDATE users SET wash_sale_enabled = 1", []).unwrap();
        for asset in if all { vec![s.btc, eth] } else { vec![eth] } {
            o.primary.execute("INSERT INTO wash_sale_locks (asset_id, user_id, buy_locked_until, created_at, updated_at) \
                               SELECT ?1, id, '2026-10-01 00:00:00', '2026-09-01 00:00:00', '2026-09-01 00:00:00' FROM users", [asset]).unwrap();
        }
        let t = script("100000", json!({}));
        let mut e = engine(&d, o, &t);
        yes(&mut e, id, at(T0)).await;
        let posted = t.posted_orders();
        if all {
            assert!(posted.is_empty());
            assert!(logs(&e.primary).is_empty(), "a lock writes no activity row: {:?}", logs(&e.primary));
        } else {
            assert_eq!((posted.len(), posted[0]["symbol"].as_str()), (1, Some("BTC/USD")), "the lock re-weights the split to the other member");
        }
    }
}

/// D-6C-11: Clear of an ambiguous Yes counts what it asked for into the declined offset, so a second Yes on an account
/// with plenty of cash does not buy the same proceeds again.
#[tokio::test(flavor = "current_thread")]
async fn timeout_then_clear_then_yes_with_surplus_cash_buys_nothing_twice() {
    let (d, o, id, _, _) = proceeds_bot("640", "100");
    let t = script("100000", json!({"POST /v2/orders":[{"network":"post_send","message":common::scripted::POST_SEND}]}));
    let mut e = engine(&d, o, &t);
    yes(&mut e, id, at(T0)).await;
    let bot = model::load_bot(&e.primary, id).unwrap();
    assert_eq!(bot.transient[redeploy::PENDING_KEY]["state"], "ambiguous");
    assert!(bot.transient["rust_placement"].is_object());
    let nonce = bot.transient[redeploy::PENDING_KEY]["id"].as_str().unwrap().to_string();
    assert_eq!(redeploy::decline(&e.primary, id, bot.user_id, at(T0)).unwrap(), redeploy::Declined::Refused, "a halt is in flight");
    assert_eq!(redeploy::resolve(&e.primary, id, bot.user_id, &nonce, at(T0)).unwrap(), None);
    let bot = model::load_bot(&e.primary, id).unwrap();
    assert!(bot.transient["rust_placement"].is_null() && bot.transient[redeploy::PENDING_KEY].is_null());
    assert_eq!(one::<f64>(&e.primary, "SELECT CAST(redeploy_declined_offset AS REAL) FROM bots"), 100.0);
    assert_eq!(offer(&e.primary, id), 0.0);
    yes(&mut e, id, at(T0) + Duration::hours(1)).await;
    assert_eq!(t.posted_orders().len(), 1, "only the first, ambiguous send");
    assert!(logs(&e.primary).contains(&("redeploy_not_started".into(), json!({"reason":["nothing_to_redeploy"]}))));
    assert!(logs(&e.primary).iter().any(|(event, details)| event == "redeploy_manually_resolved" && details["user_id"] == bot.user_id));
}

#[test]
fn an_offset_above_banked_offers_zero_and_dust_offers_zero() {
    let (_d, o, id, _, _) = proceeds_bot("640", "100");
    o.primary.execute("UPDATE bots SET redeploy_declined_offset='150'", []).unwrap();
    assert_eq!(offer(&o.primary, id), 0.0, "D-6C-9: Rails clamps, never unavailable");
    o.primary.execute("UPDATE bots SET redeploy_declined_offset='99.5'", []).unwrap();
    assert_eq!(offer(&o.primary, id), 0.0, "0.5 is under the composition's 1 USD floor");
}

fn ticker(base_decimals: i64, price_decimals: i64, min_base: &str, min_quote: &str) -> model::Ticker {
    model::Ticker { id: 1, ticker: "BTCUSDT".into(), base_code: "BTC".into(), quote_code: "USDT".into(), base_symbol: "BTC".into(), quote_symbol: "USDT".into(),
        exchange_name: "Bitrue".into(), base_asset_id: 1, quote_asset_id: 2, base_decimals, quote_decimals: 2, price_decimals,
        minimum_base_size: d(min_base), minimum_quote_size: d(min_quote), trading_enabled: true, available: true, crypto: true }
}

/// D-6C-12 (r5 rows 1, 6, 8): a base-denominated buy on a venue that cannot bound a market order is a limit at ref × 1.01,
/// its quantity floored, so a fill can never spend more than the share; a quote-capable venue sends the quote itself.
#[test]
fn base_wire_buys_are_bounded_limits_and_quote_venues_send_quote() {
    use deltabadger::engine::venue_rules::{BITRUE, GEMINI, KRAKEN};
    let plan = redeploy::size(&ticker(2, 2, "0.01", "1"), &d("100"), &d("1000"), &BITRUE, true).unwrap().unwrap();
    assert!(plan.limit && !plan.quote_type);
    assert_eq!((plan.price.to_s_f(), plan.volume.to_s_f()), ("101.0".into(), "9.9".into()));
    // A fill at 103 against ref 100 cannot happen above the limit: the most this order spends is 9.90 × 101.
    assert!(&plan.volume * &plan.price <= d("1000"));
    // Row 8: the limit at 101 sizes to 0.09, worth 9.09 under the 10 quote minimum: skipped before any venue call.
    assert!(redeploy::size(&ticker(2, 2, "0.01", "10"), &d("100"), &d("10"), &BITRUE, true).unwrap().is_none());
    // Codex's boundary (row 6): ref 1.01, share 0.109, base step 0.1, quote precision 2, on every non-crossing venue.
    for rules in [&BITRUE, &KRAKEN] {
        let plan = redeploy::size(&ticker(1, 4, "0.1", "0.1"), &d("1.01"), &d("0.109"), rules, true).unwrap().unwrap();
        assert!(plan.limit && &plan.volume * &plan.price <= d("0.109"), "{}", rules.name);
    }
    // Kraken's quote minimum fails at 0.10 there, so its order went base-denominated: bounded as Bitrue's is.
    let plan = redeploy::size(&ticker(8, 1, "0.0001", "5"), &d("64000"), &d("100"), &KRAKEN, true).unwrap().unwrap();
    assert!(plan.quote_type && !plan.limit);
    assert_eq!(plan.volume.to_s_f(), "100.0");
    // A crossing venue keeps Rails' path: base, the 1% haircut, its persisted crossed price.
    let plan = redeploy::size(&ticker(5, 2, "0.00001", "1"), &d("64000"), &d("100"), &GEMINI, true).unwrap().unwrap();
    assert!(!plan.limit && !plan.quote_type && plan.wire_price.is_some());
    assert_eq!(plan.volume.to_s_f(), "0.00154");
}

#[tokio::test(flavor = "current_thread")]
async fn taken_or_archived_requests_are_cleared_without_buying() {
    for case in ["taken", "archived"] {
        let (d, o, id, _, _) = proceeds_bot("640", "100");
        let mut req = json!({"id":"yes-1","at":1});
        if case == "taken" { req["taken_at"] = json!(2); }
        model::merge_transient_compact(&o.primary, id, &[(redeploy::REQUEST_KEY, req)]).unwrap();
        if case == "archived" { o.primary.execute("UPDATE bots SET status=?1", [deltabadger::enums::BotStatus::Archived as i64]).unwrap(); }
        let t = script("100000", json!({}));
        let mut e = engine(&d, o, &t);
        run::run_commands(&mut e, &FixedClock(at(T0))).await.unwrap();
        assert!(t.posted_orders().is_empty());
        assert!(model::load_bot(&e.primary, id).unwrap().transient[redeploy::REQUEST_KEY].is_null());
        if case == "archived" { assert_eq!(logs(&e.primary), vec![("redeploy_not_started".into(), json!({"reason":"archived"}))]); }
    }
}

/// T2b: a stopped bot's REDEPLOY row is polled past the eight follow-up attempts, and after a restart.
#[tokio::test(flavor = "current_thread")]
async fn a_stopped_redeploy_row_is_repolled_and_closes() {
    for restart in [false, true] {
        let (d, o, id, s, _) = proceeds_bot("640", "100");
        let row = seed::insert_row(&o.primary, &s, id, s.btc, &json!({"external_id":"WAIT","external_status":1,"amount":"0.001","quote_amount":"64","created_at":"2026-09-01 10:00:03"}));
        o.primary.execute("UPDATE transactions SET transaction_type='REDEPLOY' WHERE id=?1", [row]).unwrap();
        let order = |status: &str, filled: &str, avg: Option<&str>| ok(json!({ "id": "WAIT", "status": status, "symbol": "BTC/USD", "type": "market", "side": "buy",
            "notional": "64", "qty": null, "filled_qty": filled, "filled_avg_price": avg, "limit_price": null }));
        let mut replies = vec![order("new", "0", None); 8];
        replies.push(order("filled", "0.001", Some("64000")));
        let t = script("100000", json!({"GET /v2/orders/WAIT": replies}));
        let paths = store::Paths::from_env(&|_| None, d.path());
        let mut e = engine(&d, o, &t);
        for n in 0..8 { run::step(&mut e, &FixedClock(at(T0) + Duration::hours(n))).await.unwrap(); }
        if restart {
            drop(e);
            let o = store::open(&paths).unwrap();
            e = engine(&d, o, &t);
        }
        run::step(&mut e, &FixedClock(at(T0) + Duration::hours(8))).await.unwrap();
        assert_eq!(e.primary.query_row("SELECT external_status FROM transactions WHERE id=?1", [row], |r| r.get::<_, i64>(0)).unwrap(), 2, "restart={restart}");
    }
}

/// T2b, r5 row 4: an open poll reporting zero, a terminal state with the figure missing, then the late figure: a REDEPLOY row
/// counts its spend once the settle scan reads it, whether it closed or was cancelled after a partial fill.
#[tokio::test(flavor = "current_thread")]
async fn late_redeploy_spend_is_read_by_the_settle_scan() {
    use deltabadger::engine::polling;
    for terminal in ["filled", "canceled"] {
        let (dir, o, id, s, _) = proceeds_bot("640", "100");
        let row = seed::insert_row(&o.primary, &s, id, s.btc, &json!({"external_id":"LATE","external_status":1,"amount":"0.002","quote_amount":"128","created_at":"2026-09-01 10:00:03"}));
        o.primary.execute("UPDATE transactions SET transaction_type='REDEPLOY',quote_amount_exec=NULL WHERE id=?1", [row]).unwrap();
        for (seconds, status, exec, quote) in [(0, deltabadger::venue::OrderStatus::Open, "0", Some("0")),
            (5, if terminal == "filled" { deltabadger::venue::OrderStatus::Closed } else { deltabadger::venue::OrderStatus::Cancelled }, "0.001", None)] {
            let mut state = deltabadger::venue::fake::kraken_order("LATE", &json!({"status":"open","vol":"0.002","vol_exec":"0","cost":"0","price":"0","oflags":"","descr":{"pair":"BTCUSD","type":"buy","ordertype":"market"}}));
            state.status = status;
            state.amount_exec = d(exec);
            state.quote_amount_exec = quote.map(d);
            let version = model::credential_version(&o.primary, &model::load_bot(&o.primary, id).unwrap()).unwrap();
            model::credential_write(&o.primary, &version, |tx| polling::apply_in(tx, id, row, &state, false, at(T0) + Duration::seconds(seconds))).unwrap();
        }
        let order = ok(json!({ "id": "LATE", "status": terminal, "symbol": "BTC/USD", "type": "market", "side": "buy", "notional": "128", "qty": null,
            "filled_qty": "0.001", "filled_avg_price": "64000", "limit_price": null }));
        let t = script("100000", json!({"GET /v2/orders/LATE": [order]}));
        let mut e = engine(&dir, o, &t);
        run::step(&mut e, &FixedClock(at(T0) + Duration::minutes(10))).await.unwrap();
        assert_eq!(e.primary.query_row("SELECT quote_amount_exec FROM transactions WHERE id=?1", [row], |r| r.get::<_, f64>(0)).unwrap(), 64.0, "{terminal}");
        assert_eq!(offer(&e.primary, id), 36.0, "{terminal}: 100 banked, 64 spent");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn handback_refuses_a_redeploy_request() {
    let (d, o, id, _, _) = proceeds_bot("640", "100");
    request(&o.primary, id);
    let paths = store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
    deltabadger::lease::claim(&lock, &o.primary, &seed::cipher(), "test", at(T0)).unwrap();
    let result = deltabadger::engine::handover::hand_back(&lock, &o, &Factory(script("1", json!({}))), &seed::cipher(), &FixedClock(at(T0))).await;
    assert!(matches!(result, Err(deltabadger::engine::EngineError::Ineligible(ref r)) if r.iter().any(|r| r.contains("redeploy request"))));
}

#[test]
fn redeploy_sources_match_the_live_rails_grid_oracle() {
    use sha2::{Digest, Sha256};
    let pins: Value = serde_json::from_str(include_str!("fixtures/redeploy_sources.json")).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    assert_eq!(pins.as_object().unwrap().len(), 6);
    for (file, hash) in pins.as_object().unwrap() {
        assert_eq!(hex::encode(Sha256::digest(std::fs::read(root.join(file)).unwrap())), hash.as_str().unwrap(), "{file}: re-record grid-redeploy and check the engine");
    }
}

/// The venues where D-6C-12 changes what is sent: Rails sizes their buy in base (with its 1% haircut); Rust sends the quote
/// where the venue takes it (Kraken `viqc`, KuCoin `funds`, Bitvavo `amountQuote`) and a GTC limit at ref × 1.01 on Bitrue.
#[cfg(unix)]
const D_6C_12: [&str; 4] = ["kraken", "kucoin", "bitvavo", "bitrue"];

#[cfg(unix)]
fn redeploy_grid_difference(name: &str, _: &std::path::Path, rails: &Value, rust: &Value) -> Result<(), String> {
    let mut expected = rails.clone();
    let venue = name.split('-').nth(1).unwrap();
    let (timeout, pre_send) = (name.ends_with("-timeout"), name.ends_with("-pre_send"));
    let (alpaca, kraken, hyperliquid) = (venue == "alpaca", venue == "kraken", venue == "hyperliquid");
    if name == "redeploy-alpaca-basket-below_minimum" {
        // D-ALPACA-BASE-FLOOR: Rails sends a notional whose base is under minimum_base_size; Rust skips it, and nothing placed
        // is Rails' own below_minimums line.
        assert_eq!(rails["sent"].as_array().unwrap().len(), 1);
        let logs: Vec<_> = rust["changes"]["bot_activity_logs"].as_array().unwrap().iter().map(|l| (l["after"]["event"].clone(), l["after"]["details"].clone())).collect();
        assert_eq!(logs, vec![(json!("redeploy_skipped"), json!({"base":"BTC","reason":"below_minimum"})), (json!("redeploy_below_minimums"), json!({"reason":["below_minimums"]}))]);
        expected["sent"] = json!([]);
        expected["changes"]["transactions"] = json!([]);
        expected["changes"]["bot_activity_logs"] = rust["changes"]["bot_activity_logs"].clone();
        return if expected == *rust { Ok(()) } else { Err(format!("rails: {expected}\nrust: {rust}")) };
    }
    if D_6C_12.contains(&venue) {
        let (want, got) = (&rails["sent"][0], &rust["sent"][0]);
        let ok = match venue {
            "kraken" => want.get("oflags").is_none() && got["oflags"] == "viqc" && got["volume"] == "320.0",
            "kucoin" => want.get("size").is_some() && want.get("funds").is_none() && got["funds"] == "320.0" && got.get("size").is_none(),
            "bitvavo" => want.get("amount").is_some() && got["amountQuote"] == "320.0" && got.get("amount").is_none(),
            _ => want["type"] == "MARKET" && got["type"] == "LIMIT" && got["timeInForce"] == "GTC"
                && &BigDec::parse(got["quantity"].as_str().unwrap()).unwrap() * &BigDec::parse(got["price"].as_str().unwrap()).unwrap() <= d("320"),
        };
        if !ok { return Err(format!("D-6C-12 {venue}: rails sent {want}, rust sent {got}")); }
        expected["sent"] = rust["sent"].clone();
    }
    // Kraken's refused connection is a skip on the Rust side, whose line carries no sizing (normalised below).
    if D_6C_12.contains(&venue) && !(kraken && pre_send) {
        if let Some(row) = expected["changes"]["transactions"].as_array_mut().and_then(|rows| rows.first_mut()) {
            let ours = &rust["changes"]["transactions"][0]["after"];
            assert_eq!(row["after"]["quote_amount"], ours["quote_amount"], "{name}: the share is the same");
            for key in ["amount", "price", "order_type"] { row["after"][key] = ours[key].clone(); }
        }
        let log = &mut expected["changes"]["bot_activity_logs"][0]["after"]["details"];
        let ours = &rust["changes"]["bot_activity_logs"][0]["after"]["details"];
        assert_eq!(log["quote_amount"], ours["quote_amount"], "{name}: the share is the same");
        for key in ["amount", "price"] { log[key] = ours[key].clone(); }
    }
    // Alpaca proves a refused connection sent nothing, on both sides alike.
    if timeout || (pre_send && !alpaca) {
        let bots = expected["changes"]["bots"].as_array_mut().unwrap();
        if timeout || !alpaca {
            assert_eq!(bots.len(), 1);
            let pending = &bots[0]["after"]["transient_data"]["redeploy_pending"];
            assert_eq!(pending["state"], "ambiguous");
            let nonce = pending["id"].as_str().unwrap();
            assert!(nonce.len() == 12 && nonce.bytes().all(|b| b.is_ascii_hexdigit()));
            if timeout || !kraken {
                // Rails draws a fresh nonce; Rust's halt is named by the intent's client order id.
                let nonce = &rust["changes"]["bots"][0]["after"]["transient_data"]["redeploy_pending"]["id"];
                assert!(uuid::Uuid::parse_str(nonce.as_str().unwrap()).is_ok());
                bots[0]["after"]["transient_data"]["redeploy_pending"]["id"] = nonce.clone();
            } else {
                // D-6C-PRE-SEND: Kraken's transport proves a refused connection sent nothing; Rails leaves it ambiguous.
                bots[0]["after"]["transient_data"].as_object_mut().unwrap().remove("redeploy_pending");
                bots.retain(|b| b["before"] != b["after"]);
            }
        }
        let logs = expected["changes"]["bot_activity_logs"].as_array_mut().unwrap();
        // Rails' batch ends on the halt with nothing placed and reports it as `redeploy_below_minimums` too
        // (Redeployable#place_redeploy_orders!). D-6C-HALT-NOT-BELOW: Rust leaves only the halt. Kraken's refused connection
        // is a skip on the Rust side (below), after which nothing placed is the below-minimums line on both.
        if logs.len() != 2 || logs[1]["after"]["event"] != "redeploy_below_minimums" || logs[0]["after"]["event"] != "redeploy_ambiguous" {
            return Err(format!("rails logs: {logs:?}\nrust: {}", rust["changes"]["bot_activity_logs"]));
        }
        if !(kraken && pre_send) { logs.truncate(1); }
        let log = &mut logs[0]["after"];
        if timeout {
            let reason = if alpaca { "placement outcome unknown: Faraday::TimeoutError: Net::ReadTimeout" }
                else if hyperliquid { "placement failed: Hyperliquid::Error: Network error: Net::ReadTimeout" } else { "placement failed: Net::ReadTimeout" };
            assert_eq!(log["details"]["reason"], reason);
            // The transport boundary keeps Client.network_failure's spelling on every venue.
            log["details"]["reason"] = json!("placement outcome unknown: Faraday::TimeoutError: Net::ReadTimeout");
        } else if !alpaca {
            assert_eq!((&log["event"], &log["level"], &log["details"]["base"]), (&json!("redeploy_ambiguous"), &json!(2), &json!("BTC")));
            let prefix = if hyperliquid { "placement failed: Hyperliquid::Error: Network error: " } else { "placement failed: " };
            let message = log["details"]["reason"].as_str().unwrap().strip_prefix(prefix).unwrap().to_string();
            assert!(message.starts_with("Connection refused - connect(2) for \"") && message.ends_with("\" port 443"));
            if kraken {
                log["details"] = json!({"base":"BTC","reason":format!("transient: Faraday::ConnectionFailed: {message}")});
                log["event"] = json!("redeploy_skipped");
                log["level"] = json!(0);
            } else {
                log["details"]["reason"] = json!(format!("placement failed: Faraday::ConnectionFailed: {message}"));
            }
        }
    }
    if expected == *rust { Ok(()) } else { Err(format!("rails: {expected}\nrust: {rust}")) }
}

#[cfg(unix)]
#[tokio::test(flavor = "current_thread")]
async fn rails_redeploy_grid_matches_every_venue() {
    let outputs = common::venue_grid::run_venue_grid("grid-redeploy", 168, &[common::venue_grid::Validator {
        names: &|name| name.ends_with("-timeout") || name.ends_with("-pre_send") || name == "redeploy-alpaca-basket-below_minimum"
            || (name.ends_with("-accepted") && D_6C_12.iter().any(|v| name.starts_with(&format!("redeploy-{v}-")))),
        check: &redeploy_grid_difference,
    }]).await;
    for (name, out) in outputs {
        let count = out["sent"].as_array().unwrap().len();
        let sends = ["-accepted", "-timeout", "-pre_send"].iter().any(|suffix| name.ends_with(suffix)) || name == "redeploy-alpaca-basket-below_minimum";
        assert_eq!(count, usize::from(sends), "{name}: {out}");
    }
}

/// Two abandoned-by-staleness REDEPLOY rows (BTC, ETH) on the stopped proceeds bot.
fn abandoned_rows(o: &store::Opened, s: &seed::Seeded, id: i64, eth: i64, status: i64) -> Vec<i64> {
    [(s.btc, "LOST-1"), (eth, "LOST-2")].into_iter().map(|(asset, ext)| {
        let row = seed::insert_row(&o.primary, s, id, asset, &json!({"external_id":ext,"external_status":status,"amount":"0.001","quote_amount":"20","price":"20000","created_at":"2026-08-01 10:00:02"}));
        o.primary.execute("UPDATE transactions SET transaction_type='REDEPLOY', updated_at='2026-08-20 10:00:00.000000' WHERE id=?1", [row]).unwrap();
        row
    }).collect()
}
fn fake_engine(d: &tempfile::TempDir, c: rusqlite::Connection) -> run::Engine<deltabadger::venue::fake::FakeFactory> {
    use deltabadger::venue::fake::{FakeFactory, FakeVenue};
    let paths = store::Paths::from_env(&|_| None, d.path());
    let lock = deltabadger::lease::lock(&paths, at(T0)).unwrap();
    run::Engine::new(c, FakeFactory(FakeVenue::new()), seed::cipher(), lock)
}
fn stood_down(c: &rusqlite::Connection, id: i64) -> bool {
    let bot = model::load_bot(c, id).unwrap();
    bot.transient[redeploy::PENDING_KEY]["state"] == "ambiguous" && deltabadger::engine::composition::in_flight(c, &bot, at(T0)).unwrap()
}

/// The halt commits with the abandonment: a crash before the announcing pass still leaves the bot halted after a restart.
#[tokio::test(flavor = "current_thread")]
async fn a_crash_after_the_abandoning_commit_keeps_the_halt() {
    let (d, o, id, s, eth) = proceeds_bot("640", "100");
    let rows = abandoned_rows(&o, &s, id, eth, 1);
    let venue = deltabadger::venue::fake::FakeVenue::new();
    deltabadger::engine::polling::follow_up(&o.primary, &venue, id, rows[0], at(T0)).await.unwrap();
    assert_eq!(one::<i64>(&o.primary, "SELECT count(*) FROM transactions WHERE external_status=4"), 1);
    assert!(stood_down(&o.primary, id), "halted in the abandoning transaction");
    // The process dies here; the restarted engine neither loses nor re-raises the halt.
    let nonce = model::load_bot(&o.primary, id).unwrap().transient[redeploy::PENDING_KEY]["id"].clone();
    let mut e = fake_engine(&d, o.primary);
    for minutes in [0, 10, 20] { run::step(&mut e, &FixedClock(at(T0) + Duration::minutes(minutes))).await.unwrap(); }
    assert!(stood_down(&e.primary, id));
    assert_eq!(model::load_bot(&e.primary, id).unwrap().transient[redeploy::PENDING_KEY]["id"], nonce);
    let halts: Vec<_> = logs(&e.primary).into_iter().filter(|(event, _)| event == "redeploy_ambiguous").collect();
    assert_eq!(halts, vec![], "the halt stood before the second row's pass: nothing new is announced");
}

/// A halt that cannot be written rolls the abandonment back: the row stays waiting and is polled again.
#[tokio::test(flavor = "current_thread")]
async fn a_failed_halt_write_rolls_back_the_abandonment() {
    let (_d, o, id, s, eth) = proceeds_bot("640", "100");
    let rows = abandoned_rows(&o, &s, id, eth, 1);
    o.primary.execute_batch("CREATE TRIGGER no_halt BEFORE UPDATE OF transient_data ON bots WHEN NEW.transient_data LIKE '%redeploy_pending%' \
                             BEGIN SELECT RAISE(ABORT, 'halt write failed'); END;").unwrap();
    let venue = deltabadger::venue::fake::FakeVenue::new();
    assert!(deltabadger::engine::polling::follow_up(&o.primary, &venue, id, rows[0], at(T0)).await.is_err());
    assert_eq!(one::<i64>(&o.primary, "SELECT external_status FROM transactions WHERE external_id='LOST-1'"), 1);
    assert_eq!(one::<i64>(&o.primary, "SELECT count(*) FROM bot_activity_logs WHERE event='order_abandoned'"), 0);
    assert!(model::load_bot(&o.primary, id).unwrap().transient[redeploy::PENDING_KEY].is_null());
}

/// Rails halts only when it abandons a row: an abandoned row cleared before (no halt standing) is not re-halted by the scan.
#[tokio::test(flavor = "current_thread")]
async fn the_scan_leaves_a_cleared_abandoned_row_alone() {
    let (d, o, id, s, eth) = proceeds_bot("640", "100");
    abandoned_rows(&o, &s, id, eth, 4);
    let mut e = fake_engine(&d, o.primary);
    for minutes in [0, 10, 20] { run::step(&mut e, &FixedClock(at(T0) + Duration::minutes(minutes))).await.unwrap(); }
    assert!(model::load_bot(&e.primary, id).unwrap().transient[redeploy::PENDING_KEY].is_null());
    assert_eq!(logs(&e.primary).into_iter().filter(|(event, _)| event == "redeploy_ambiguous").count(), 0);
}
