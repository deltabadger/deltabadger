//! A14b (PLAN-A14b-split-base-cap.md, D-SPLIT-BASE-CAP): a one-asset seller's base cap ("don't sell more than N shares")
//! and the sales it counts are restated into today's shares across stock splits. Rails counts both as traded; with no
//! split Rust is Rails' number bit for bit (the stock grid's `cap_sold_nosplit`, the limit-sell grid, amount_limit.rs).
mod common;
use axum::{body::{to_bytes, Body}, http::Request};
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::{accounting, amount_limit, model, polling, splits, tick, Clock, FixedClock};
use deltabadger::engine::tick::Attempts;
use deltabadger::enums::BotStatus;
use deltabadger::ruby::BigDec;
use deltabadger::store;
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError};
use deltabadger::web;
use rusqlite::params;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Thursday 2026-09-10 14:00:00.5 UTC: a week and more after the bots start, the market open.
const NOW: &str = "2026-09-10T14:00:00.5Z";
/// When the cap was switched on (Time#as_json), and the settings saved with it.
const STAMP: &str = "2026-08-20T14:00:00.000Z";
const CHANGED: &str = "2026-08-20 14:00:00";
const SOLD_AT: &str = "2026-08-25 14:00:01";
const SPLIT_AT: &str = "2026-08-28 00:00:00";
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }
fn dec(s: &str) -> BigDec { BigDec::parse(s).unwrap() }
fn venue<T: Transport>(t: T) -> AlpacaVenue<T> { AlpacaVenue::new(t, Urls::for_passphrase(Some("paper"))) }

struct Fixture { _d: tempfile::TempDir, o: store::Opened, s: seed::Seeded, aapl: i64, msft: i64, id: i64 }
impl Fixture {
    fn c(&self) -> &rusqlite::Connection { &self.o.primary }
    fn bot(&self) -> model::Bot { model::load_bot(self.c(), self.id).unwrap() }
    /// A sell of `asset` (`amount` asked, `exec` reported) in `external_status`, created at `created`.
    fn sell(&self, asset: i64, status: i64, amount: &str, exec: Option<&str>, created: &str) -> i64 {
        seed::insert_row(self.c(), &self.s, self.id, asset, &json!({ "external_id": format!("S{created}{amount}"), "external_status": status, "side": 1,
            "order_type": 0, "price": "180", "amount": amount, "amount_exec": exec, "created_at": created }))
    }
    /// A split of `name` on the seeded venue that passes the row-evidence gate (or, without a ratio, an unsized report).
    fn split(&self, name: &str, when: &str, ratio: Option<&str>) -> i64 {
        let row = seed::insert_split(self.c(), &self.s, name, when, ratio);
        if let Some(ratio) = ratio {
            let (p, q): (f64, f64) = { let mut t = ratio.split(':').map(|x| x.parse::<f64>().unwrap()); (t.next().unwrap(), t.next().unwrap()) };
            let raw = json!({ "corporate_action": "split", "activity_type": "SPLIT", "symbol": name, "qty": "-1", "split_ratio": ratio, "merged_activity_ids": ["remove", "add"] });
            self.c().execute("UPDATE account_transactions SET base_amount = ?1, raw_data = ?2 WHERE id = ?3", params![p / q - 1.0, raw.to_string(), row]).unwrap();
        }
        row
    }
    fn set(&self, path: &str, value: Value) {
        self.c().execute("UPDATE bots SET settings = json_set(settings, ?1, json(?2)) WHERE id = ?3", params![path, value.to_string(), self.id]).unwrap();
    }
    fn set_transient(&self, path: &str, value: Value) {
        self.c().execute("UPDATE bots SET transient_data = json_set(transient_data, ?1, json(?2)) WHERE id = ?3", params![path, value.to_string(), self.id]).unwrap();
    }
    fn basis(&self) -> Option<Value> { self.bot().transient.get(accounting::BASIS_KEY).filter(|v| !v.is_null()).cloned() }
    /// What the cap leaves at `now`, restated.
    fn left_at(&self, now: &str) -> BigDec {
        accounting::base_amount_available_at(self.c(), &self.bot(), at(now)).unwrap().expect("the cap is on").to_dec().unwrap()
    }
    fn left(&self) -> BigDec { self.left_at(NOW) }
    fn reached(&self) -> bool { accounting::base_amount_limit_reached_at(self.c(), &self.bot(), at(NOW)).unwrap() }
}

/// A scheduled one-asset AAPL seller (MSFT listed beside it), 15 shares a week, capped at `limit` since STAMP and its
/// settings last saved then. No basis key: the cap reads the unknown-basis rule, exact here (nothing edited since).
fn seller(limit: Value) -> Fixture {
    let (d, o, s) = common::install_alpaca();
    let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
    let (msft, _) = seed::add_alpaca_stock(&o.primary, &s, "MSFT");
    o.primary.execute("UPDATE users SET confirmed_at = created_at, wash_sale_enabled = 0", []).unwrap();
    // The stock listings sync's ExchangeAsset rows: the position reader names a holding through them.
    for asset in [aapl, msft] {
        o.primary.execute("INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) VALUES (?1, ?2, 1, ?3, ?3)",
                          params![s.exchange_id, asset, seed::SYNCED]).unwrap();
    }
    let spec = BotSpec { settings_changed_at: Some(CHANGED.into()), ..BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(aapl, 1.0)])
        .with("direction", json!("selling")).with("sell_denomination", json!("base")).with("sell_amount", json!(15)).with("sell_interval", json!("week"))
        .with("base_amount_limited", json!(true)).with("base_amount_limit", limit).transient("base_amount_limit_enabled_at", json!(STAMP)) };
    let id = seed::insert_bot(&o.primary, &s, &spec);
    Fixture { _d: d, o, s, aapl, msft, id }
}
/// The recorded case: cap 10, 4 sold on 08-25, and (unless `ratio` is None) an AAPL split on 08-28.
fn recorded(ratio: Option<&str>) -> Fixture {
    let f = seller(json!(10));
    f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT);
    if let Some(r) = ratio { f.split("AAPL", SPLIT_AT, Some(r)); }
    f
}

/// An open clock, AAPL and MSFT quoted, a funded account, orders accepted, and these positions.
fn script(positions: Value) -> ScriptedTransport {
    ScriptedTransport::from_script(&json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-10T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "GET /v2/stocks/AAPL/quotes/latest": ok(json!({ "quote": { "ap": 187.43, "bp": 187.4 } })),
        "GET /v2/stocks/AAPL/trades/latest": ok(json!({ "trade": { "p": 187.41 } })),
        "GET /v2/stocks/MSFT/quotes/latest": ok(json!({ "quote": { "ap": 401.25, "bp": 401.2 } })),
        "GET /v2/stocks/MSFT/trades/latest": ok(json!({ "trade": { "p": 401.2 } })),
        "POST /v2/orders": (1..=3).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(positions),
        "GET /v2/stocks/snapshots": ok(json!({ "AAPL": { "latestTrade": { "p": 187.41 } }, "MSFT": { "latestTrade": { "p": 401.2 } } })),
    }))
}
fn holding(sym: &str, qty: &str) -> Value { json!([{ "symbol": sym, "asset_class": "us_equity", "qty": qty, "qty_available": qty }]) }
async fn tick_with<T: Transport>(f: &Fixture, t: T, clock: &dyn Clock) -> tick::TickOutcome {
    seed::fresh_stock_jobs(f.c(), clock.now());
    tick::tick(f.c(), &venue(t), f.id, clock, &mut Attempts::default()).await.unwrap()
}
fn sold(t: &ScriptedTransport) -> Vec<String> {
    t.posted_orders().iter().map(|o| { assert_eq!(o["side"], "sell"); o["qty"].as_str().unwrap().to_string() }).collect()
}
fn events(f: &Fixture) -> Vec<String> {
    f.c().prepare("SELECT event FROM bot_activity_logs WHERE bot_id = ?1 ORDER BY id").unwrap()
        .query_map([f.id], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}

// ---- 1-5: the basis rule on the recorded case ----

#[test]
fn a_forward_split_doubles_the_cap_and_the_sales_and_a_reverse_one_divides_them() {
    assert_eq!(recorded(None).left(), dec("6"), "no split: Rails' 10 − 4");
    assert_eq!(recorded(Some("2:1")).left(), dec("12"), "20 − 8, not Rails' 6");
    let f = seller(json!(10));
    f.sell(f.aapl, 2, "5", Some("5"), SOLD_AT);
    f.split("AAPL", SPLIT_AT, Some("1:10"));
    assert_eq!(f.left(), dec("0.5"), "10/10 − 5/10, not Rails' 5");
    // At the floor: 1e-9 at AAPL's 9 decimals. Restated, 1 − 0.9999999995 is under it; as traded 10 − 9.999999995 is not.
    let f = seller(json!(10));
    f.sell(f.aapl, 2, "9.999999995", Some("9.999999995"), SOLD_AT);
    assert!(!f.reached(), "no split: 5e-9 left");
    f.split("AAPL", SPLIT_AT, Some("1:10"));
    assert!(f.reached(), "after 1:10 less than one increment is left");
}

#[test]
fn a_split_before_the_cap_was_typed_moves_only_the_sales_before_it() {
    let f = recorded(Some("2:1"));
    // The number was typed (in Rust) after the split: today's shares.
    f.set_transient(&format!("$.{}", accounting::BASIS_KEY), json!({ "at": "2026-08-29T10:00:00.000000Z", "limit": 10, "asset_id": f.aapl, "enabled_at": STAMP }));
    assert_eq!(f.left(), dec("2"), "10 − 4×2");
}

#[test]
fn another_assets_split_leaves_the_cap_as_traded() {
    let f = recorded(None);
    f.split("MSFT", SPLIT_AT, Some("2:1"));
    assert_eq!(f.left(), dec("6"));
}

#[tokio::test(flavor = "current_thread")]
async fn a_wallet_seller_follows_the_split_without_any_bot_buy() {
    // No commitment, the cap typed after the split: nothing moves.
    let f = seller(json!(10));
    f.split("AAPL", SPLIT_AT, Some("2:1"));
    f.set_transient(&format!("$.{}", accounting::BASIS_KEY), json!({ "at": "2026-08-29T10:00:00.000000Z", "limit": 10, "asset_id": f.aapl, "enabled_at": STAMP }));
    assert_eq!(f.left(), dec("10"));
    // Typed before it: the cap doubles.
    let f = seller(json!(10));
    f.split("AAPL", SPLIT_AT, Some("2:1"));
    assert_eq!(f.left(), dec("20"));
    // An unresolved AAPL split stands the tick down: no order.
    let f = seller(json!(10));
    f.split("AAPL", SPLIT_AT, None);
    let t = script(holding("AAPL", "32"));
    tick_with(&f, t.clone(), &FixedClock(at(NOW))).await;
    assert!(t.posted_orders().is_empty());
    assert_eq!(events(&f), ["dca_skipped_restatement"]);
    assert!(accounting::base_amount_available_at(f.c(), &f.bot(), at(NOW)).is_err(), "unknown is never unbounded");
}

#[tokio::test(flavor = "current_thread")]
async fn the_recorded_case_sells_twelve_and_holds_nothing() {
    let f = recorded(Some("2:1"));
    let t = script(holding("AAPL", "32"));
    tick_with(&f, t.clone(), &FixedClock(at(NOW))).await;
    assert_eq!(sold(&t), ["12.000000000"], "min(15, 12, 32)");
    assert!(f.bot().transient.get("rust_split_hold").is_none(), "a wallet seller's walk holds nothing to reconcile");
    let f = recorded(None);
    let t = script(holding("AAPL", "32"));
    tick_with(&f, t.clone(), &FixedClock(at(NOW))).await;
    assert_eq!(sold(&t), ["6.000000000"], "Rails' 6 with no split");
}

#[test]
fn every_commitment_kind_is_restated_at_its_own_time() {
    let f = seller(json!(100));
    f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT); // closed: 8
    f.sell(f.aapl, 0, "1", None, SOLD_AT); // waiting: 2
    f.sell(f.aapl, 3, "3", Some("1"), SOLD_AT); // cancelled partial: 2
    f.sell(f.aapl, 4, "2", Some("1"), SOLD_AT); // abandoned, max(1, 2): 4
    f.sell(f.aapl, 2, "4", Some("4"), SPLIT_AT); // at the split's instant: already new shares, 4
    f.split("AAPL", SPLIT_AT, Some("2:1"));
    assert_eq!(f.left(), dec("180"), "200 − 20");
    let intent = |when: &str| json!({ "side": "sell", "transaction_type": "REGULAR", "amount": "1", "volume": "1", "at": when, "base_asset_id": f.aapl });
    f.set_transient("$.rust_placement", intent("2026-08-27T10:00:00+00:00"));
    assert_eq!(f.left(), dec("178"), "an intent before the split: 2");
    f.set_transient("$.rust_placement", intent("2026-09-01T10:00:00+00:00"));
    assert_eq!(f.left(), dec("179"), "after it: 1");
    // The settled reading leaves the open sell and the intent out before restating.
    assert!(!accounting::base_amount_limit_reached_settled_at(f.c(), &f.bot(), at(NOW)).unwrap());
}

// ---- 6, 16: the one basis writer, on every surface ----

/// An MCP and a REST client allowed update_bot_settings, and the app at NOW.
fn surfaces(f: &Fixture) -> web::App {
    let c = f.c();
    c.execute("UPDATE bots SET status = 2, label = 'Seller' WHERE id = ?1", [f.id]).unwrap();
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('A14b','a14b','http://localhost/cb',0,'mcp api','2026-09-10 12:00:00','2026-09-10 12:00:00')", []).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [("a14b-mcp", "mcp"), ("a14b-api", "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-10 12:00:00',86400)", (application, f.s.user_id, token, scope)).unwrap();
    }
    let tools = json!(["update_bot_settings", "get_bot_details"]).to_string();
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,?3,?3,'2026-09-10 12:00:00','2026-09-10 12:00:00')", (f.s.user_id, application, &tools)).unwrap();
    let permissions = json!({"tool_permissions":{"update_bot_settings":true,"get_bot_details":true}}).to_string();
    c.execute("UPDATE users SET mcp_settings = ?1, rest_settings = ?1", [&permissions]).unwrap();
    common::web::app(f._d.path(), "engine-test-secret", common::web::TestClock::at(NOW))
}
#[derive(Clone, Copy, Debug, PartialEq)]
enum Surface { Web, Mcp, Rest }
async fn send(app: &web::App, request: Request<Body>) -> (u16, String) {
    let response = web::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    (status, String::from_utf8(to_bytes(response.into_body(), 4_000_000).await.unwrap().to_vec()).unwrap())
}
/// One settings save: `fields` in the automation dialect; the page posts each as its form text.
async fn save(f: &Fixture, app: &web::App, surface: Surface, fields: Value) {
    let (status, body) = match surface {
        Surface::Web => {
            let form: serde_json::Map<String, Value> = fields.as_object().unwrap().iter().map(|(k, v)| (k.clone(), match v {
                Value::Bool(b) => json!(if *b { "1" } else { "0" }), Value::Number(n) => json!(n.to_string()), other => other.clone() })).collect();
            let token = web::csrf::new_token();
            let session = web::session::SessionData { user: Some((f.s.user_id, "x".into())), csrf: Some(token.clone()), ..Default::default() };
            let cookie = web::session::seal(&app.keys.session, &session, app.now());
            send(app, Request::builder().method("PATCH").uri(format!("/bots/{}", f.id)).header("host", "localhost:3000")
                .header("cookie", format!("{}={cookie}", web::session::COOKIE)).header("x-csrf-token", web::csrf::masked(&token))
                .header("content-type", "application/json").header("accept", "text/vnd.turbo-stream.html")
                .body(Body::from(json!({ "bots_dca_multi_asset": form }).to_string())).unwrap()).await
        }
        Surface::Rest => send(app, Request::builder().method("PATCH").uri(format!("/api/v1/bots/{}", f.id)).header("host", "localhost:3000")
            .header("authorization", "Bearer a14b-api").header("content-type", "application/json").header("accept", "application/json")
            .body(Body::from(fields.to_string())).unwrap()).await,
        Surface::Mcp => {
            let mut arguments = fields.as_object().unwrap().clone();
            arguments.insert("bot_id".into(), json!(f.id));
            let (status, body) = mcp(app, "update_bot_settings", Value::Object(arguments)).await;
            assert_ne!(body["result"]["isError"], true, "{body}");
            (status, body.to_string())
        }
    };
    assert_eq!(status, 200, "{surface:?} {fields}: {body}");
}
async fn mcp(app: &web::App, tool: &str, arguments: Value) -> (u16, Value) {
    let request = |body: Value, session: Option<&str>| {
        let mut r = Request::builder().method("POST").uri("/mcp").header("host", "localhost:3000").header("authorization", "Bearer a14b-mcp")
            .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
        if let Some(session) = session { r = r.header("mcp-session-id", session); }
        r.body(Body::from(body.to_string())).unwrap()
    };
    let response = web::router(app.clone()).oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"A14b","version":"1"}}}), None)).await.unwrap();
    let session = response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
    assert_eq!(send(app, request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), Some(&session))).await.0, 202);
    let (status, body) = send(app, request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":arguments}}), Some(&session))).await;
    (status, serde_json::from_str(&body).unwrap())
}

/// The basis each step leaves, on one surface: (basis, left). A basis written under the current stamp names it "current"
/// (each surface writes the stamp in its own zone).
async fn edits_on(surface: Surface) -> Vec<(Option<Value>, BigDec)> {
    let f = recorded(Some("2:1"));
    let app = surfaces(&f);
    let basis = |f: &Fixture| f.basis().map(|mut b| {
        if b["enabled_at"] == f.bot().transient["base_amount_limit_enabled_at"] { b["enabled_at"] = json!("current"); }
        if b["asset_id"] == f.aapl { b["asset_id"] = json!("AAPL"); }
        // Compared by value: the page writes the number as a Float, the API as typed (draft::equal reads them alike).
        b["limit"] = json!(b["limit"].as_f64());
        b
    });
    let mut out = vec![(basis(&f), f.left())];
    // An unchanged number (spelled 10.0) beside another change: still unknown (16). That save moved settings_changed_at past
    // the split, so the number may now be today's shares: the least reading, 10×1 − 8 (the plan's conservative case).
    save(&f, &app, surface, json!({ "base_amount_limit": 10.0, "sell_amount": 16 })).await;
    out.push((basis(&f), f.left()));
    for step in [json!({ "base_amount_limit": 20 }), json!({ "base_amount_limit": 20.0, "sell_amount": 15 }), json!({ "base_amount_limit": 10 }),
                 json!({ "base_amount_limited": false }), json!({ "base_amount_limited": true })] {
        save(&f, &app, surface, step).await;
        out.push((basis(&f), if f.bot().base_amount_limited() { f.left() } else { dec("-1") }));
    }
    out
}

#[tokio::test(flavor = "current_thread")]
async fn the_basis_moves_only_with_the_number_or_the_switch_on_every_surface() {
    let web = edits_on(Surface::Web).await;
    let key = |at: &str, limit: f64, _: &str| Some(json!({ "at": at, "limit": limit, "asset_id": "AAPL", "enabled_at": "current" }));
    let now = "2026-09-10T14:00:00.500000Z";
    let on = "";
    assert_eq!(web, vec![
        (None, dec("12")),
        (None, dec("2")),
        (key(now, 20.0, STAMP), dec("12")), // 20 typed after the split: 20 − 8
        (key(now, 20.0, STAMP), dec("12")), // the same number, re-spelled: kept
        (key(now, 10.0, STAMP), dec("2")), // 10 − 8: earlier sales stay counted, restated
        (None, dec("-1")), // off: removed
        (key(now, 10.0, on), dec("10")), // on: a new window, as in Rails
    ]);
    assert_eq!(edits_on(Surface::Mcp).await, web, "MCP writes the same basis");
    assert_eq!(edits_on(Surface::Rest).await, web, "REST writes the same basis");
}

#[tokio::test(flavor = "current_thread")]
async fn a_respelled_number_keeps_its_basis_across_a_reverse_split() {
    for surface in [Surface::Web, Surface::Mcp, Surface::Rest] {
        let f = seller(json!(10));
        let app = surfaces(&f);
        save(&f, &app, surface, json!({ "base_amount_limit": 12 })).await;
        save(&f, &app, surface, json!({ "base_amount_limit": 10 })).await;
        let mut basis = f.basis().expect("a changed number writes an exact basis");
        // Typed the day before, and a 1:10 split since: 10 old shares are 1.
        basis["at"] = json!("2026-09-09T10:00:00.000000Z");
        f.set_transient(&format!("$.{}", accounting::BASIS_KEY), basis.clone());
        f.split("AAPL", "2026-09-09 12:00:00", Some("1:10"));
        assert_eq!(f.left(), dec("1"), "{surface:?}");
        save(&f, &app, surface, json!({ "base_amount_limit": 10.0, "sell_amount": 16 })).await;
        assert_eq!(f.basis(), Some(basis), "{surface:?}: 10 resubmitted as 10.0 keeps the basis");
        assert_eq!(f.left(), dec("1"), "{surface:?}: the cap stays 1 share, not 10");
    }
}

/// The surfaces switch a basket's asset through their composition tools; every one ends in the one writer, so the switch
/// is driven through it here.
#[test]
fn an_asset_switch_counts_in_the_new_asset_and_keeps_the_old_sales_restated() {
    let f = recorded(Some("2:1"));
    f.c().execute("UPDATE bots SET status = 2 WHERE id = ?1", [f.id]).unwrap();
    let mut draft = web::bot::draft::Draft::load(f.c(), f.s.user_id, f.id, "en").unwrap().unwrap();
    draft.candidate.settings.insert("allocations".into(), json!({ f.msft.to_string(): 1.0 }));
    let effects = draft.save_effects(f.c(), at(NOW)).unwrap();
    let basis = effects.transient.set.get(accounting::BASIS_KEY).cloned().expect("the switch writes a basis");
    assert_eq!((basis["asset_id"].clone(), basis["limit"].clone()), (json!(f.msft), json!(10)));
    f.set("$.allocations", json!({ f.msft.to_string(): 1.0 }));
    f.set_transient(&format!("$.{}", accounting::BASIS_KEY), basis);
    f.split("MSFT", "2026-09-10 14:00:01", Some("2:1"));
    assert_eq!(f.left_at("2026-09-11T14:00:00Z"), dec("12"), "10 MSFT × 2, less 4 AAPL × 2");
}

// ---- 7, 13: unknown basis ----

#[test]
fn an_unknown_basis_takes_the_least_reading_the_history_allows() {
    // Saved after the split (any setting): the number may be today's. 20 − 8 = 12 would be the exact reading only if it was.
    let f = recorded(Some("2:1"));
    f.set("$.base_amount_limit", json!(20));
    f.c().execute("UPDATE bots SET settings_changed_at = '2026-08-30 10:00:00' WHERE id = ?1", [f.id]).unwrap();
    assert_eq!(f.left(), dec("12"), "20×1 − 8, not 40 − 8");
    // Nothing saved since the stamp: exact.
    assert_eq!(recorded(Some("2:1")).left(), dec("12"));
    // No settings_changed_at at all: the widest interval.
    let f = recorded(Some("2:1"));
    f.c().execute("UPDATE bots SET settings_changed_at = NULL WHERE id = ?1", [f.id]).unwrap();
    assert_eq!(f.left(), dec("2"), "10×1 − 8");
}

#[test]
fn a_stale_basis_is_read_as_unknown() {
    let key = |f: &Fixture, limit: Value, enabled: &str| f.set_transient(&format!("$.{}", accounting::BASIS_KEY),
        json!({ "at": "2026-08-29T10:00:00.000000Z", "limit": limit, "asset_id": f.aapl, "enabled_at": enabled }));
    let f = recorded(Some("2:1"));
    key(&f, json!(10), STAMP);
    assert_eq!(f.left(), dec("2"), "valid: typed after the split");
    key(&f, json!(10.0), STAMP);
    assert_eq!(f.left(), dec("2"), "10.0 is 10");
    // Rails toggled the switch off and on during a handback: a new cycle, the key rejected; unknown, exact here.
    key(&f, json!(10), "2026-08-19T14:00:00.000Z");
    assert_eq!(f.left(), dec("12"));
    key(&f, json!(11), STAMP);
    assert_eq!(f.left(), dec("12"), "a stale limit");
    f.set_transient(&format!("$.{}", accounting::BASIS_KEY), json!({ "at": "2026-08-29T10:00:00.000000Z", "limit": 10, "asset_id": f.msft, "enabled_at": STAMP }));
    assert_eq!(f.left(), dec("12"), "a stale asset");
}

#[test]
fn a_reclaimed_bot_whose_number_rails_changed_and_changed_back_reads_unknown() {
    // Rails edited 10 → 20 → 10 after the split: settings_changed_at moved, no stamp did; the handback removed the key.
    let f = recorded(Some("2:1"));
    f.c().execute("UPDATE bots SET settings_changed_at = '2026-09-02 10:00:00' WHERE id = ?1", [f.id]).unwrap();
    assert_eq!(f.left(), dec("2"), "10×1 − 8, not 12");
    let listed = deltabadger::engine::handover::HANDBACK_STATE.iter().find(|(_, name, _)| *name == accounting::BASIS_KEY).map(|(_, _, h)| *h);
    assert_eq!(listed, Some(deltabadger::engine::handover::Handback::Settled), "the handback removes the key");
}

#[test]
fn with_no_stamp_every_event_up_to_the_last_save_is_a_candidate() {
    let f = seller(json!(10));
    f.c().execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.base_amount_limit_enabled_at') WHERE id = ?1", [f.id]).unwrap();
    f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT);
    assert_eq!(f.left(), dec("10"), "no window: Rails' figure");
    f.split("AAPL", SPLIT_AT, Some("2:1"));
    assert_eq!(f.left(), dec("10"), "least reading 10×1");
    let f = seller(json!(10));
    f.c().execute("UPDATE bots SET transient_data = json_remove(transient_data, '$.base_amount_limit_enabled_at') WHERE id = ?1", [f.id]).unwrap();
    f.split("AAPL", SPLIT_AT, Some("1:10"));
    assert_eq!(f.left(), dec("1"), "least reading 10×0.1");
}

// ---- 14c, 14e, 15: venues and names ----

/// A second Alpaca venue row named Previous, with its own AAPL ticker, the bot's earlier sales on it.
fn previous_venue(f: &Fixture) -> i64 {
    let c = f.c();
    c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Previous', '0.1', '0.2', '2026-01-01', '2026-01-01')", []).unwrap();
    let venue = c.last_insert_rowid();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, \
               minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
               VALUES (?1, 'AAPLx', 'AAPL', 'USD', ?2, ?3, 9, 2, 2, '0.000000001', '1', 1, 1, '2026-01-01', '2026-01-01')", params![venue, f.aapl, f.s.quote]).unwrap();
    venue
}
fn on_venue(f: &Fixture, row: i64, venue: i64) { f.c().execute("UPDATE transactions SET exchange_id = ?1 WHERE id = ?2", [venue, row]).unwrap(); }
fn split_on(f: &Fixture, row: i64, venue: i64) { f.c().execute("UPDATE account_transactions SET exchange_id = ?1 WHERE id = ?2", [venue, row]).unwrap(); }
fn exact_basis(f: &Fixture, at: &str) {
    f.set_transient(&format!("$.{}", accounting::BASIS_KEY), json!({ "at": at, "limit": 10, "asset_id": f.aapl, "enabled_at": STAMP }));
}

#[test]
fn a_foreign_venues_sales_follow_that_venues_split() {
    for (ratio, left) in [("2:1", "2"), ("1:10", "9.6")] {
        let f = seller(json!(10));
        let previous = previous_venue(&f);
        let row = f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT);
        on_venue(&f, row, previous);
        let split = f.split("AAPL", SPLIT_AT, Some(ratio));
        split_on(&f, split, previous);
        exact_basis(&f, "2026-08-29T10:00:00.000000Z");
        assert_eq!(f.left(), dec(left), "{ratio}");
    }
}

#[test]
fn a_split_reported_on_one_venue_restates_only_that_venues_sales() {
    let f = seller(json!(10));
    let previous = previous_venue(&f);
    let row = f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT);
    on_venue(&f, row, previous);
    f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT);
    let split = f.split("AAPL", SPLIT_AT, Some("1:10"));
    split_on(&f, split, previous);
    exact_basis(&f, "2026-08-29T10:00:00.000000Z");
    assert_eq!(f.left(), dec("5.6"), "10 − 0.4 − 4");
}

#[test]
fn a_sale_under_a_former_symbol_is_found_by_its_name() {
    let f = seller(json!(10));
    let row = f.sell(f.aapl, 2, "5", Some("5"), SOLD_AT);
    f.c().execute("UPDATE transactions SET base = 'OLDAAPL' WHERE id = ?1", [row]).unwrap();
    // An asset-less 1:10 report under the old name, no ticker listing it any more.
    f.split("OLDAAPL", SPLIT_AT, Some("1:10"));
    f.c().execute("UPDATE account_transactions SET base_asset_id = NULL WHERE base_currency = 'OLDAAPL'", []).unwrap();
    exact_basis(&f, "2026-08-29T10:00:00.000000Z");
    assert_eq!(f.left(), dec("9.5"), "10 − 0.5, not 10 − 5");
    exact_basis(&f, "2026-08-27T10:00:00.000000Z");
    assert_eq!(f.left(), dec("0.5"), "typed before it: 1 − 0.5");
}

// ---- 8, 14: stops ----

#[test]
fn a_rotation_stops_on_restated_sales_only() {
    let f = seller(json!(10));
    f.set("$.direction", json!("buying"));
    f.sell(f.aapl, 2, "6", Some("6"), SOLD_AT);
    f.sell(f.aapl, 2, "4", Some("4"), "2026-08-30 14:00:01");
    f.split("AAPL", SPLIT_AT, Some("2:1"));
    f.set("$.direction", json!("selling"));
    assert!(!amount_limit::stop_at_rotation(f.c(), f.id, at(NOW)).unwrap(), "16 of 20: no stop (as traded, 10 of 10 would stop)");
    f.sell(f.aapl, 2, "4", Some("4"), "2026-09-02 14:00:01");
    assert!(amount_limit::stop_at_rotation(f.c(), f.id, at(NOW)).unwrap(), "20 of 20");
    assert!(!amount_limit::stop_at_rotation(f.c(), f.id, at(NOW)).unwrap(), "once per cap");
}

#[test]
fn an_unknown_count_at_a_stop_is_decided_as_traded() {
    // As traded 10 of 10: the decision is not lost to "unknown".
    let f = recorded(None);
    f.sell(f.aapl, 2, "6", Some("6"), "2026-08-26 14:00:01");
    f.split("AAPL", SPLIT_AT, None);
    assert!(f.reached());
    // Not exhausted as traded: no stop, and the tick stands down meanwhile.
    let f = recorded(None);
    f.split("AAPL", SPLIT_AT, None);
    assert!(!f.reached());
}

#[tokio::test(flavor = "current_thread")]
async fn a_poll_with_an_unresolved_split_decides_as_traded() {
    for (asked, stopped) in [("6", true), ("5", false)] {
        let f = recorded(None);
        f.split("AAPL", SPLIT_AT, None);
        let order = f.sell(f.aapl, 0, asked, None, "2026-09-09 14:00:01");
        let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/SO": ok(json!({ "id": "SO", "status": "filled", "symbol": "AAPL", "type": "market",
            "side": "sell", "notional": null, "qty": asked, "filled_qty": asked, "filled_avg_price": "180", "limit_price": null })) }));
        f.c().execute("UPDATE transactions SET external_id = 'SO' WHERE id = ?1", [order]).unwrap();
        polling::follow_up(f.c(), &venue(t), f.id, order, at(NOW)).await.unwrap();
        assert_eq!(f.bot().status == BotStatus::Stopped, stopped, "asked {asked}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_stop_decided_before_a_split_arrived_restarts_on_the_restated_tally() {
    // Stopped as traded (10 of 10: 4 on 08-25, 6 on 09-09), then a 2:1 split dated 09-01 arrives: restated, 20 − 8 − 6 = 6.
    // The start validation reads the restated tally, and the next tick sells what is left.
    let f = recorded(None);
    f.sell(f.aapl, 2, "6", Some("6"), "2026-09-09 14:00:01");
    assert!(f.reached(), "10 of 10 as traded");
    f.c().execute("UPDATE bots SET status = ?1 WHERE id = ?2", [BotStatus::Stopped as i64, f.id]).unwrap();
    f.split("AAPL", "2026-09-01 00:00:00", Some("2:1"));
    assert_eq!(f.left(), dec("6"), "20 − 8 − 6");
    let mut draft = web::bot::draft::Draft::load(f.c(), f.s.user_id, f.id, "en").unwrap().expect("owned bot");
    draft.candidate.status = BotStatus::Scheduled;
    draft.validate(f.c(), web::bot::draft::ValidationContext::Start, at(NOW), true, "en").unwrap();
    assert!(!draft.errors.iter().any(|e| e.field == "settings"), "the restart passes: {:?}", draft.errors.iter().map(|e| &e.message).collect::<Vec<_>>());
    f.c().execute("UPDATE bots SET status = 1 WHERE id = ?1", [f.id]).unwrap();
    let t = script(holding("AAPL", "32"));
    tick_with(&f, t.clone(), &FixedClock(at(NOW))).await;
    assert_eq!(sold(&t), ["6.000000000"]);
}

// ---- 9: a wider basket never reads the cap ----

#[tokio::test(flavor = "current_thread")]
async fn a_wider_basket_sells_across_a_split_as_with_the_switch_off() {
    let mut sent = vec![];
    for limited in [true, false] {
        let f = recorded(Some("2:1"));
        f.set("$.allocations", json!({ f.aapl.to_string(): 0.5, f.msft.to_string(): 0.5 }));
        f.set("$.sell_denomination", json!("quote"));
        f.set("$.sell_quote_amount", json!(100.0));
        f.set("$.base_amount_limited", json!(limited));
        assert!(!f.bot().base_amount_limited());
        assert!(splits::cap_dependencies(f.c(), &f.bot(), at(NOW)).unwrap() == Default::default(), "no dependencies while the cap is off");
        let t = script(json!([{ "symbol": "AAPL", "asset_class": "us_equity", "qty": "32", "qty_available": "32" },
                              { "symbol": "MSFT", "asset_class": "us_equity", "qty": "5", "qty_available": "5" }]));
        tick_with(&f, t.clone(), &FixedClock(at(NOW))).await;
        sent.push(t.posted_orders());
    }
    assert_eq!(sent[0], sent[1]);
}

// ---- 10, 14f: the fences ----

/// Runs `sql` once, when the tick reads AAPL's price: after the gate and the snapshot, before the placement begins.
struct OnPrice { inner: ScriptedTransport, db: std::path::PathBuf, sql: String, armed: std::cell::Cell<bool> }
impl Transport for OnPrice {
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
        if self.armed.get() && r.path.starts_with("/v2/stocks/AAPL/") {
            self.armed.set(false);
            rusqlite::Connection::open(&self.db).unwrap().execute_batch(&self.sql).unwrap();
        }
        self.inner.send(r).await
    }
}
const REVERSE_LATE: &str = "INSERT INTO account_transactions (user_id, exchange_id, entry_type, base_currency, base_amount, transacted_at, raw_data, created_at, updated_at) \
    SELECT user_id, exchange_id, 15, 'AAPL', -0.9, '2026-09-09 00:00:00', json_object('corporate_action','split','activity_type','SPLIT','symbol','AAPL','qty','-1','split_ratio','1:10','merged_activity_ids',json_array('remove','add')), '2026-09-09 00:00:00', '2026-09-09 00:00:00' FROM bots WHERE id = 1;";

#[tokio::test(flavor = "current_thread")]
async fn a_split_stored_mid_tick_refuses_the_begin_and_the_next_tick_sells_the_restated_cap() {
    let f = seller(json!(10));
    assert_eq!(f.id, 1);
    let t = script(holding("AAPL", "32"));
    let racing = OnPrice { inner: t.clone(), db: f.c().path().unwrap().into(), sql: REVERSE_LATE.into(), armed: true.into() };
    tick_with(&f, racing, &FixedClock(at(NOW))).await;
    assert!(t.posted_orders().is_empty(), "sized at 10 before the 1:10 was stored: never sent");
    assert!(f.bot().rust_placement().is_none());
    tick_with(&f, t.clone(), &FixedClock(at("2026-09-17T14:00:00.5Z"))).await;
    assert_eq!(sold(&t), ["1.000000000"], "re-sized: 10 old shares are 1");
}

#[tokio::test(flavor = "current_thread")]
async fn a_future_split_of_an_old_asset_coming_into_effect_before_the_begin_refuses_it() {
    // AAPL → MSFT with 4 AAPL counted; a 1:10 AAPL split dated between sizing and the begin. The cap clock moves on the read.
    let f = seller(json!(10));
    f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT);
    f.set("$.allocations", json!({ f.msft.to_string(): 1.0 }));
    f.set_transient(&format!("$.{}", accounting::BASIS_KEY), json!({ "at": "2026-09-01T10:00:00.000000Z", "limit": 10, "asset_id": f.msft, "enabled_at": STAMP }));
    f.split("AAPL", "2026-09-10 14:00:01", Some("1:10"));
    struct Later { db: std::path::PathBuf }
    impl Clock for Later {
        fn now(&self) -> DateTime<Utc> {
            // Before any price is read: the sizing time; once the MSFT price was asked, a second later.
            let asked: bool = rusqlite::Connection::open(&self.db).unwrap().query_row("SELECT count(*) > 0 FROM app_configs WHERE key = 'a14b_priced'", [], |r| r.get(0)).unwrap();
            at(if asked { "2026-09-10T14:00:01.5Z" } else { NOW })
        }
    }
    let t = script(holding("MSFT", "30"));
    struct Mark { inner: ScriptedTransport, db: std::path::PathBuf }
    impl Transport for Mark {
        async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> {
            if r.path.starts_with("/v2/stocks/MSFT/") {
                rusqlite::Connection::open(&self.db).unwrap().execute("INSERT OR IGNORE INTO app_configs (key, value, created_at, updated_at) VALUES ('a14b_priced', '1', '2026-01-01', '2026-01-01')", []).unwrap();
            }
            self.inner.send(r).await
        }
    }
    let clock = Later { db: f.c().path().unwrap().into() };
    tick_with(&f, Mark { inner: t.clone(), db: f.c().path().unwrap().into() }, &clock).await;
    assert!(t.posted_orders().is_empty(), "sized on 10 − 4 = 6, begun after 4 AAPL became 0.4: never sent");
    tick_with(&f, t.clone(), &FixedClock(at("2026-09-17T14:00:00.5Z"))).await;
    assert_eq!(sold(&t), ["9.600000000"], "re-sized: 10 − 0.4");
}

#[tokio::test(flavor = "current_thread")]
async fn an_unresolved_split_of_an_old_commitments_asset_stands_the_tick_down() {
    let f = seller(json!(10));
    f.sell(f.aapl, 2, "4", Some("4"), SOLD_AT);
    f.set("$.allocations", json!({ f.msft.to_string(): 1.0 }));
    f.split("AAPL", SPLIT_AT, None);
    let t = script(holding("MSFT", "30"));
    tick_with(&f, t.clone(), &FixedClock(at(NOW))).await;
    assert!(t.posted_orders().is_empty());
    assert_eq!(events(&f), ["dca_skipped_restatement"]);
}

/// After a successful begin and before the send's dispatch lock: once the intent exists, `hook` runs (once) and the clock
/// reads `late`.
struct AfterBegin { db: std::path::PathBuf, bot: i64, late: DateTime<Utc>, hook: String, ran: std::cell::Cell<bool> }
impl Clock for AfterBegin {
    fn now(&self) -> DateTime<Utc> {
        let c = rusqlite::Connection::open(&self.db).unwrap();
        let begun: bool = c.query_row("SELECT json_type(transient_data, '$.rust_placement') IS NOT NULL FROM bots WHERE id = ?1", [self.bot], |r| r.get(0)).unwrap();
        if !begun { return at(NOW); }
        if !self.ran.get() { self.ran.set(true); c.execute_batch(&self.hook).unwrap(); }
        self.late
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_split_between_the_begin_and_the_dispatch_sends_nothing() {
    for (hook, future) in [(REVERSE_LATE, false), ("", true)] {
        let f = seller(json!(10));
        if future { f.split("AAPL", "2026-09-10 14:00:00.700000", Some("1:10")); }
        let t = script(holding("AAPL", "32"));
        let clock = AfterBegin { db: f.c().path().unwrap().into(), bot: f.id, late: at("2026-09-10T14:00:01Z"), hook: hook.into(), ran: false.into() };
        tick_with(&f, t.clone(), &clock).await;
        assert!(t.posted_orders().is_empty(), "future {future}: nothing dispatched");
        assert!(f.bot().rust_placement().is_none(), "future {future}: the intent cleared as not placed");
        tick_with(&f, t.clone(), &FixedClock(at("2026-09-17T14:00:00.5Z"))).await;
        assert_eq!(sold(&t), ["1.000000000"], "future {future}: the retry sizes on the restated cap");
    }
}

// ---- 11: the surfaces agree ----

#[tokio::test(flavor = "current_thread")]
async fn the_page_the_engine_the_start_and_the_reads_agree() {
    let f = recorded(Some("2:1"));
    surfaces(&f);
    assert_eq!(f.left(), dec("12"));
    let bot = web::bot::Bot::find(f.c(), f.s.user_id, f.id, web::bot::For::Settings, "en").unwrap().unwrap();
    let info = web::bot::settings::base_amount_limit_info(f.c(), "en", &bot, false, at(NOW)).unwrap();
    assert!(info.contains("12") && info.contains("AAPL"), "{info}");
    let mut draft = web::bot::draft::Draft::load(f.c(), f.s.user_id, f.id, "en").unwrap().unwrap();
    draft.candidate.status = BotStatus::Scheduled;
    draft.validate(f.c(), web::bot::draft::ValidationContext::Start, at(NOW), true, "en").unwrap();
    assert!(!draft.errors.iter().any(|e| e.field == "settings"));
    // No MCP or REST read carries the cap; the stored setting, which the form and the stop mail show, stays the typed 10.
    assert_eq!(f.bot().settings["base_amount_limit"], json!(10));
    // Unknown: the page shows no "left" line.
    f.split("AAPL", "2026-09-01 00:00:00", None);
    let bot = web::bot::Bot::find(f.c(), f.s.user_id, f.id, web::bot::For::Settings, "en").unwrap().unwrap();
    assert_eq!(web::bot::settings::base_amount_limit_info(f.c(), "en", &bot, false, at(NOW)).unwrap(), "");
}
