//! PLAN index-as-ingredient slice 4b: a stored index bot (`Bots::DcaIndex`) becomes a basket that follows its index at every
//! start (engine::index_bot, in set_aside::apply), or is kept, named, for a later start.
mod common;
use chrono::{DateTime, Utc};
use common::seed;
use deltabadger::engine::index_bot::{self, Converted, Kept};
use deltabadger::engine::tick::{self, Attempts, PriceCache, TickOutcome};
use deltabadger::engine::{eligibility, handover, index, model, FixedClock};
use deltabadger::lease;
use deltabadger::store::{self, Paths};
use deltabadger::venue::alpaca::{AlpacaVenue, Urls};
use deltabadger::venue::fake::FakeFactory;
use deltabadger::venue::http::{ScriptedTransport, Transport};
use serde_json::{json, Value};

const T0: &str = "2026-09-01T14:00:00.5Z";
const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];
fn at(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
fn ok(body: Value) -> Value { json!([{ "status": 200, "body": body }]) }

/// Five Alpaca stocks in the ND100 row (caps 5, 3, 2, 1, 0.5 T), fresh catalogues, a provider; `v2`: a 2.x install a takeover
/// can claim, else a baseline one.
struct U { d: tempfile::TempDir, o: store::Opened, s: seed::Seeded, m: Vec<(i64, i64)> }
fn universe_on(d: tempfile::TempDir) -> U {
    let o = store::open(&Paths::from_env(&|_| None, d.path())).unwrap();
    let s = seed::seed_alpaca(&o.primary, &seed::cipher());
    let m: Vec<(i64, i64)> = SYMBOLS.iter().map(|sym| seed::add_alpaca_stock(&o.primary, &s, sym)).collect();
    seed::insert_index(&o.primary, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"],
                       &json!({ "AAA.US": 5e12, "BBB.US": 3e12, "CCC.US": 2e12, "DDD.US": 1e12, "EEE.US": 5e11 }));
    // Binds the provider; this one is deleted, out of the way (the conversion never touches it).
    let deleted = seed::index_bot(&o.primary, &s, "nasdaq-100", 3, 0.0, false);
    o.primary.execute("UPDATE bots SET status = 3 WHERE id = ?1", [deleted]).unwrap();
    seed::fresh_stock_jobs(&o.primary, at(T0));
    U { d, o, s, m }
}
fn universe() -> U { universe_on(common::baseline_install()) }
fn exec(u: &U, sql: &str) { u.o.primary.execute_batch(sql).unwrap(); }
fn index_bot(u: &U, n: i64) -> i64 { seed::index_bot(&u.o.primary, &u.s, "nasdaq-100", n, 0.0, false) }
fn member(u: &U, bot: i64, i: usize, target: f64) { seed::insert_member(&u.o.primary, bot, u.m[i].0, u.m[i].1, target, true, "2026-08-01 00:00:00", None); }

fn script(asks: [f64; 5]) -> ScriptedTransport { script_with(asks, json!({})) }
fn script_with(asks: [f64; 5], over: Value) -> ScriptedTransport {
    let mut m = json!({
        "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })),
        "POST /v2/orders": (1..=8).map(|n| json!({ "status": 200, "body": { "id": format!("OTX-{n}"), "status": "accepted" } })).collect::<Vec<_>>(),
        "GET /v2/account": ok(json!({ "cash": "100000", "buying_power": "200000", "non_marginable_buying_power": "100000" })),
        "GET /v2/positions": ok(json!([])),
    });
    for (sym, ask) in SYMBOLS.iter().zip(asks) { m[format!("GET /v2/stocks/{sym}/quotes/latest")] = Value::Array(vec![json!({ "status": 200, "body": { "quote": { "ap": ask, "bp": ask } } }); 4]); }
    for (k, v) in over.as_object().unwrap() { m[k] = v.clone(); }
    ScriptedTransport::from_script(&m)
}
fn venue<T: Transport>(t: T) -> AlpacaVenue<T> { AlpacaVenue::new(t, Urls::for_passphrase(Some("paper"))) }
const ASKS: [f64; 5] = [100.0, 200.0, 300.0, 400.0, 500.0];

/// The scheduler over this universe, every venue the scripted Alpaca one.
struct Factory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for Factory {
    type V = AlpacaVenue<ScriptedTransport>;
    fn for_bot(&self, _: &str, _: Option<deltabadger::crypto::Credentials>) -> Self::V { venue(self.0.clone()) }
}
fn engine(u: &U, t: &ScriptedTransport) -> deltabadger::engine::run::Engine<Factory> {
    let paths = Paths::from_env(&|_| None, u.d.path());
    let lock = lease::lock(&paths, at(T0)).unwrap();
    deltabadger::engine::run::Engine::new(store::open(&paths).unwrap().primary, Factory(t.clone()), seed::cipher(), lock)
}
async fn step(e: &mut deltabadger::engine::run::Engine<Factory>, now: &str) { deltabadger::engine::run::step(e, &FixedClock(at(now))).await.unwrap(); }

fn convert(u: &U) -> (Vec<Converted>, Vec<(i64, Kept)>) {
    let tx = model::immediate(&u.o.primary).unwrap();
    let out = index_bot::convert_all(&tx, at(T0)).unwrap();
    tx.commit().unwrap();
    out
}
fn ids(c: &[Converted]) -> Vec<i64> { c.iter().map(|c| c.id).collect() }
fn row(u: &U, id: i64) -> (String, String, String) {
    u.o.primary.query_row("SELECT type, settings, transient_data FROM bots WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
}
/// in_index members: (symbol, target_allocation as stored, updated_at), by asset.
fn members(u: &U, id: i64) -> Vec<(String, f64, String)> {
    u.o.primary.prepare("SELECT a.symbol, m.target_allocation, m.updated_at FROM bot_index_assets m JOIN assets a ON a.id = m.asset_id WHERE m.bot_id = ?1 AND m.in_index = 1 ORDER BY m.asset_id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect()
}
fn symbols(u: &U, id: i64) -> Vec<String> { members(u, id).into_iter().map(|m| m.0).collect() }
/// rust_bot_group_members: (index, symbol, composed, weight).
fn group_rows(u: &U, id: i64) -> Vec<(String, String, i64, f64)> {
    u.o.primary.prepare("SELECT g.index_external_id, a.symbol, g.composed, g.weight FROM rust_bot_group_members g JOIN assets a ON a.id = g.asset_id WHERE g.bot_id = ?1 ORDER BY g.asset_id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}
fn retire(u: &U) {
    let provider = deltabadger::engine::provider::ranking_provider(&u.o.primary).unwrap().unwrap();
    exec(u, &format!("INSERT INTO rust_index_retired VALUES ('nasdaq-100', '{provider}', '2026-09-01T00:00:00Z')"));
}
async fn refresh<T: Transport>(u: &U, t: T, id: i64) -> Result<(), index::Refusal> {
    index::refresh_composition(&u.o.primary, &venue(t), &model::load_bot(&u.o.primary, id).unwrap(), &FixedClock(at(T0)), &PriceCache::default()).await.unwrap()
}
async fn tick_at<T: Transport>(u: &U, t: T, id: i64, now: &str) -> TickOutcome {
    tick::tick(&u.o.primary, &venue(t), id, &FixedClock(at(now)), &mut Attempts::default()).await.unwrap()
}
fn reasons(u: &U, id: i64) -> Vec<String> {
    eligibility::bot_reasons(&u.o.primary, &model::load_bot(&u.o.primary, id).unwrap()).unwrap().into_iter().map(|r| r.to_string()).collect()
}

/// Codex P1-4: SQLite edits the row in place, so a 17-digit smart-interval amount, a carry and every other untouched value
/// keep their stored text. The index keys move raw into the one group; `settings_changed_at`, `run_generation` and the
/// members' rows stay as they were.
#[test]
fn conversion_edits_settings_in_place_and_keeps_every_untouched_byte() {
    let u = universe();
    let id = index_bot(&u, 3);
    exec(&u, &format!("UPDATE bots SET settings = '{{\"quote_asset_id\":{q},\"quote_amount\":60.0,\"interval\":\"week\",\"smart_intervaled\":true,\"smart_interval_quote_amount\":0.30000000000000004,\
\"num_coins\":\"3\",\"allocation_flattening\":0.0,\"index_type\":\"category\",\"index_category_id\":\"nasdaq-100\",\"hold_all\":false,\"index_name\":\"Nasdaq 100\",\
\"direction\":\"buying\",\"sell_interval\":\"day\",\"base_asset_ids\":[1,2],\"extra\":{{\"n\":1.10000000000000009}}}}',
        transient_data = '{{\"rust_carry\":12.345678901234567,\"rust_composed_index\":\"nasdaq-100\",\"run_generation\":4,\"last_action_job_at\":\"2026-08-25T14:00:00.000Z\"}}',
        settings_changed_at = '2026-08-01 10:00:00' WHERE id = {id}", q = u.s.quote));
    member(&u, id, 0, 0.6);
    member(&u, id, 1, 0.4);
    let before = members(&u, id);
    let (converted, kept) = convert(&u);
    assert_eq!((ids(&converted), kept), (vec![id], vec![]));
    let (ty, settings, transient) = row(&u, id);
    assert_eq!(ty, "Bots::DcaMultiAsset");
    assert_eq!(settings, format!("{{\"quote_asset_id\":{},\"quote_amount\":60.0,\"interval\":\"week\",\"smart_intervaled\":true,\"smart_interval_quote_amount\":0.30000000000000004,\
\"extra\":{{\"n\":1.10000000000000009}},\"index_groups\":[{{\"index_type\":\"category\",\"index_category_id\":\"nasdaq-100\",\"index_name\":\"Nasdaq 100\",\"num_coins\":\"3\",\
\"hold_all\":false,\"allocation_flattening\":0.0,\"weight\":1.0}}],\"allocations\":{{}},\"weighting\":\"manual\"}}", u.s.quote));
    assert_eq!(transient, r#"{"rust_carry":12.345678901234567,"run_generation":4,"last_action_job_at":"2026-08-25T14:00:00.000Z"}"#);
    let changed: String = u.o.primary.query_row("SELECT settings_changed_at FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_eq!(changed, "2026-08-01 10:00:00");
    assert_eq!(members(&u, id), before, "bot_index_assets untouched");
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert_eq!(bot.lone_group().map(|g| (g.external_id, g.num_coins)), Some(("nasdaq-100".into(), Some(3))));
}

/// The rows each pass converts: every stored index bot not deleted (the universe's deleted one stays), once; a second pass
/// finds none.
#[test]
fn conversion_is_idempotent_and_never_touches_a_deleted_row() {
    let u = universe();
    let deleted = u.o.primary.query_row("SELECT id FROM bots WHERE type = 'Bots::DcaIndex' AND status = 3", [], |r| r.get::<_, i64>(0)).unwrap();
    let before = row(&u, deleted);
    let (a, b) = (index_bot(&u, 3), index_bot(&u, 2));
    exec(&u, &format!("UPDATE bots SET status = 7 WHERE id = {b}")); // archived converts too
    assert_eq!(ids(&convert(&u).0), vec![a, b]);
    let after = (row(&u, a), row(&u, b));
    assert_eq!(convert(&u), (vec![], vec![]));
    assert_eq!((row(&u, a), row(&u, b)), after, "a second pass writes nothing");
    assert_eq!(row(&u, deleted), before);
}

/// Seeded always, weights as stored (Codex P1-3: no normalisation): composed only when `rust_composed_index` named this very
/// index; an exited member is no seat; the key goes.
#[test]
fn members_are_always_seeded_and_composed_only_when_the_key_named_the_index() {
    let u = universe();
    let mut bots = vec![];
    for key in [Some("nasdaq-100"), Some("top-coins"), None] {
        let id = index_bot(&u, 3);
        if let Some(key) = key { exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_composed_index', '{key}') WHERE id = {id}")); }
        member(&u, id, 0, 0.5);
        member(&u, id, 1, 0.3);
        seed::insert_member(&u.o.primary, id, u.m[2].0, u.m[2].1, 0.2, false, "2026-08-01 00:00:00", Some("2026-08-20 00:00:00"));
        bots.push(id);
    }
    convert(&u);
    for (id, composed) in bots.into_iter().zip([1, 0, 0]) {
        assert_eq!(group_rows(&u, id), vec![("nasdaq-100".into(), "AAA".into(), composed, 0.5), ("nasdaq-100".into(), "BBB".into(), composed, 0.3)], "bot {id}");
        assert!(model::load_bot(&u.o.primary, id).unwrap().transient.get(index::COMPOSED_KEY).is_none());
    }
}

/// Codex P1-3: a composed retired index's frozen targets go through the first grouped refresh unchanged, all zero (nothing
/// is bought) and six-decimal thirds alike: no equal-weight seeding, no second rounding.
#[tokio::test(flavor = "current_thread")]
async fn all_zero_and_six_decimal_targets_survive_the_first_frozen_refresh() {
    for targets in [[0.0, 0.0, 0.0], [0.333333, 0.333333, 0.333334]] {
        let u = universe();
        let id = index_bot(&u, 3);
        exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_composed_index', 'nasdaq-100') WHERE id = {id}"));
        for (i, t) in targets.iter().enumerate() { member(&u, id, i, *t); }
        retire(&u);
        let before = members(&u, id);
        convert(&u);
        refresh(&u, script(ASKS), id).await.unwrap();
        assert_eq!(members(&u, id), before, "{targets:?}: same targets, nothing rewritten");
        if targets[0] == 0.0 {
            let t = script(ASKS);
            tick_at(&u, t.clone(), id, T0).await;
            assert!(t.posted_orders().is_empty(), "all zero buys nothing");
        }
    }
}

/// `Kept::Targets`: a member whose target can't be carried as a weight (here none stored) keeps the row as it is.
#[test]
fn unrepresentable_targets_keep_the_row() {
    let u = universe();
    let id = index_bot(&u, 3);
    member(&u, id, 0, 1.0);
    exec(&u, &format!("UPDATE bot_index_assets SET target_allocation = NULL WHERE bot_id = {id}"));
    let before = row(&u, id);
    assert_eq!(convert(&u), (vec![], vec![(id, Kept::Targets)]));
    assert_eq!(row(&u, id), before);
    assert!(group_rows(&u, id).is_empty());
}

/// D-INDEX-RETIRED-PAIR on a converted retired composed index: a member whose pair left drops and the survivors renormalise;
/// with none left the composition refuses as retired.
#[tokio::test(flavor = "current_thread")]
async fn a_retired_composed_index_keeps_survivors_and_refuses_with_none() {
    let u = universe();
    let id = index_bot(&u, 3);
    exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_composed_index', 'nasdaq-100') WHERE id = {id}"));
    for (i, t) in [0.5, 0.3, 0.2].into_iter().enumerate() { member(&u, id, i, t); }
    retire(&u);
    convert(&u);
    exec(&u, &format!("UPDATE tickers SET available = 0 WHERE id = {}", u.m[0].1));
    refresh(&u, script(ASKS), id).await.unwrap();
    let got: Vec<(String, f64)> = members(&u, id).into_iter().map(|m| (m.0, m.1)).collect();
    assert_eq!(got, vec![("BBB".into(), 0.6), ("CCC".into(), 0.4)]);
    exec(&u, &format!("UPDATE tickers SET available = 0 WHERE id IN ({}, {})", u.m[1].1, u.m[2].1));
    assert_eq!(refresh(&u, script(ASKS), id).await, Err(index::Refusal::Failure(index::RETIRED.into())));
}

/// Two copies of one working composed index bot with missed intervals and a stored deferral, driven by the scheduler
/// (run::step): the converted one owes the same amount at the same checkpoint, places nothing before the deferral expires,
/// as the other, then buys the same members for the same amounts with the same targets.
#[tokio::test(flavor = "current_thread")]
async fn a_composed_index_bot_converted_buys_what_and_when_its_unconverted_copy_does() {
    let copy = || {
        let u = universe();
        let id = index_bot(&u, 3);
        exec(&u, &format!("UPDATE bots SET started_at = '2026-08-11 14:00:00', settings_changed_at = '2026-08-11 14:00:00',
            transient_data = json_set(transient_data, '$.rust_composed_index', 'nasdaq-100') WHERE id = {id}"));
        let schedule = model::load_bot(&u.o.primary, id).unwrap().schedule_key().unwrap().unwrap();
        let defer = json!({ "until": "2026-09-01T15:00:00.000000Z", "schedule": schedule, "origin": "local" });
        u.o.primary.execute("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_defer_until', json(?1)) WHERE id = ?2", rusqlite::params![defer.to_string(), id]).unwrap();
        for (i, t) in [0.5, 0.3, 0.2].into_iter().enumerate() { member(&u, id, i, t); }
        (u, id)
    };
    let ((plain, a), (converted, b)) = (copy(), copy());
    assert_eq!(a, b);
    let due = |u: &U| {
        let bot = model::load_bot(&u.o.primary, a).unwrap();
        let now = at("2026-09-01T16:00:00Z").timestamp_micros();
        let next = deltabadger::engine::schedule::checkpoints(bot.started_at_us().unwrap().unwrap(), now, bot.effective().unwrap()).unwrap().next_us;
        (next, deltabadger::engine::amount::pending_quote_amount(&u.o.primary, &bot, now).unwrap())
    };
    let before = due(&plain);
    let out = convert(&converted);
    assert_eq!(ids(&out.0), vec![b], "{out:?}");
    assert_eq!(due(&converted), before, "next checkpoint and owed amount");
    let mut placed = vec![];
    for u in [&plain, &converted] {
        let t = script(ASKS);
        let mut e = engine(u, &t);
        step(&mut e, T0).await;
        let held = t.posted_orders().len();
        step(&mut e, "2026-09-01T15:00:01Z").await;
        let orders: Vec<(Value, Value)> = t.posted_orders().iter().map(|o| (o["symbol"].clone(), o["notional"].clone())).collect();
        let targets: Vec<(String, f64)> = members(u, a).into_iter().map(|m| (m.0, m.1)).collect();
        placed.push((held, orders, targets));
    }
    // Without the deferral the same pass buys at once: the hold is the deferral's.
    let (undeferred, _) = copy();
    exec(&undeferred, &format!("UPDATE bots SET transient_data = json_remove(transient_data, '$.rust_defer_until') WHERE id = {a}"));
    let t = script(ASKS);
    step(&mut engine(&undeferred, &t), T0).await;
    assert_eq!(t.posted_orders().len(), 3, "undeferred, the first pass buys");
    assert_eq!(placed[0].0, 0, "nothing before the deferral expires: {:?}", placed[0]);
    assert_eq!(placed[0].1.len(), 3, "then the missed weeks are bought: {:?}", placed[0]);
    assert_eq!(placed[1], placed[0]);
}

/// D-INDEX-DISTINCT-SEATS (lead #2): a ranking naming AAA twice with two seats. The legacy reader seats AAA twice and holds
/// one member, a 2.x bug; the converted bot seats AAA and BBB.
#[tokio::test(flavor = "current_thread")]
async fn a_ranking_naming_an_asset_twice_seats_distinct_assets_once_converted() {
    let mut seats = vec![];
    for conv in [false, true] {
        let u = universe();
        exec(&u, "UPDATE indices SET top_coins = '[\"AAA.US\",\"AAA.US\",\"BBB.US\",\"CCC.US\"]' WHERE external_id = 'nasdaq-100'");
        let id = index_bot(&u, 2);
        if conv { convert(&u); }
        refresh(&u, script(ASKS), id).await.unwrap();
        seats.push(symbols(&u, id));
    }
    assert_eq!(seats, vec![vec!["AAA".to_string()], vec!["AAA".to_string(), "BBB".into()]]);
}

/// §5.1: a stray start_selling rule (its whole family) and a large stored sale are dropped; the converted bot buys.
#[tokio::test(flavor = "current_thread")]
async fn a_stray_start_selling_rule_and_large_sell_quote_amount_are_dropped_and_the_bot_buys() {
    let u = universe();
    let id = index_bot(&u, 3);
    exec(&u, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit', 1000000, '$.price_limit_value_condition', 'below',
        '$.price_limit_action', 'start_selling', '$.price_limit_in_ticker_id', {t}, '$.sell_quote_amount', 1000000, '$.sell_denomination', 'quote'),
        transient_data = json_set(transient_data, '$.price_limit_condition_met_at', '2026-08-31T00:00:00.000Z') WHERE id = {id}", t = u.m[0].1));
    convert(&u);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert_eq!(bot.settings["price_limited"], json!(false));
    for k in ["price_limit", "price_limit_value_condition", "price_limit_action", "price_limit_in_ticker_id"] { assert!(bot.settings.get(k).is_none(), "{k}"); }
    assert_eq!(bot.settings["sell_quote_amount"], Value::Null);
    assert!(bot.transient.get("price_limit_condition_met_at").is_none());
    let t = script_with(ASKS, json!({ "GET /v2/stocks/AAA/trades/latest": ok(json!({ "trade": { "p": 150 } })) }));
    let out = tick_at(&u, t.clone(), id, T0).await;
    assert!(matches!(out, TickOutcome::Done { placed: true }), "{out:?}");
    assert!(t.posted_orders().iter().all(|o| o["side"] == "buy") && !bot.selling());
}

/// Codex P1-1: a buy price gate with a blank action is the engine's pause, not a flip: kept by the conversion, it holds a due
/// tick exactly as on the unconverted copy.
#[tokio::test(flavor = "current_thread")]
async fn a_blank_action_price_gate_still_holds_a_due_tick_after_conversion() {
    let mut out = vec![];
    for conv in [false, true] {
        let u = universe();
        let id = index_bot(&u, 3);
        exec(&u, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit', 100, '$.price_limit_value_condition', 'below',
            '$.price_limit_action', '', '$.price_limit_in_ticker_id', {}) WHERE id = {id}", u.m[0].1));
        if conv { convert(&u); assert_eq!(model::load_bot(&u.o.primary, id).unwrap().settings["price_limit_action"], json!("")); }
        let t = script_with(ASKS, json!({ "GET /v2/stocks/AAA/trades/latest": ok(json!({ "trade": { "p": 150 } })) }));
        let o = tick_at(&u, t.clone(), id, T0).await;
        out.push((format!("{o:?}"), t.posted_orders().len(), model::load_bot(&u.o.primary, id).unwrap().status));
    }
    assert_eq!(out[0], ("Paused".into(), 0, deltabadger::enums::BotStatus::Waiting));
    assert_eq!(out[1], out[0]);
}

/// D-INDEX-ONE-CLASS: a working stored seller (set aside today as "an index bot that sells") converts and buys, as 2.70.21
/// ran it; a stopped one converts and stays stopped. `check` names the first.
#[tokio::test(flavor = "current_thread")]
async fn a_working_stored_seller_converts_and_buys_and_a_stopped_one_stays_stopped() {
    let u = universe();
    let (working, stopped) = (index_bot(&u, 3), index_bot(&u, 3));
    exec(&u, &format!("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling') WHERE id IN ({working}, {stopped}); UPDATE bots SET status = 2 WHERE id = {stopped}"));
    assert!(reasons(&u, working).iter().any(|r| r.contains("an index bot that sells")));
    let (converted, _) = convert(&u);
    assert_eq!(converted, vec![Converted { id: working, selling: true, rebalances: false }, Converted { id: stopped, selling: false, rebalances: false }]);
    assert!(index_bot::would_convert(&u.o.primary, &converted[0]).unwrap().ends_with("at the next start, and it will buy: it is stored as selling, which an index bot never did"));
    assert!(index_bot::would_convert(&u.o.primary, &converted[1]).unwrap().ends_with("at the next start"));
    assert_eq!(model::load_bot(&u.o.primary, stopped).unwrap().status, deltabadger::enums::BotStatus::Stopped);
    assert!(reasons(&u, working).is_empty(), "{:?}", reasons(&u, working));
    let t = script(ASKS);
    assert!(matches!(tick_at(&u, t.clone(), working, T0).await, TickOutcome::Done { placed: true }));
    assert!(t.posted_orders().iter().all(|o| o["side"] == "buy"));
}

/// §5.1: an unresolved intent or a pending rebalance keeps the row as it is (named), and the start after it settles converts it.
#[test]
fn an_unresolved_intent_or_pending_rebalance_keeps_the_row_until_the_start_after_it_settles() {
    for (key, why) in [("rust_placement", Kept::Intent), ("rebalance_pending", Kept::Rebalance)] {
        let u = universe();
        let id = index_bot(&u, 3);
        exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{key}', json('{{\"phase\":\"selling\"}}')) WHERE id = {id}"));
        let before = row(&u, id);
        assert_eq!(convert(&u), (vec![], vec![(id, why)]));
        assert_eq!(row(&u, id), before);
        assert_eq!(index_bot::would_keep(&u.o.primary, id, why).unwrap(), format!("Bot {id} stays an index bot for now: {}", why.text()));
        exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.{key}', json('null')) WHERE id = {id}"));
        assert_eq!(ids(&convert(&u).0), vec![id], "{key}: settled, the next start converts it");
    }
}

/// `Kept::Unreadable` (a scalar transient, which load_bot accepts) and `Kept::Bounds` (a row whose converted settings would
/// pass draft::loadable's 65 536 bytes).
#[test]
fn a_scalar_transient_or_an_oversized_result_keeps_the_row_named() {
    let u = universe();
    let (scalar, large) = (index_bot(&u, 3), index_bot(&u, 3));
    exec(&u, &format!("UPDATE bots SET transient_data = '\"x\"' WHERE id = {scalar}"));
    let len: usize = row(&u, large).1.len();
    exec(&u, &format!("UPDATE bots SET settings = json_set(settings, '$.note', '{}') WHERE id = {large}", "x".repeat(65_536 - len - 12)));
    assert!(row(&u, large).1.len() <= 65_536, "editable today");
    let (scalar_before, large_before) = (row(&u, scalar), row(&u, large));
    assert_eq!(convert(&u), (vec![], vec![(scalar, Kept::Unreadable), (large, Kept::Bounds)]));
    assert_eq!((row(&u, scalar), row(&u, large)), (scalar_before, large_before));
}

/// The settings checks follow the content: each refusal of the stored index bot is a refusal of the converted one.
#[test]
fn a_converted_index_bot_is_refused_exactly_when_its_unconverted_copy_is() {
    for (key, value) in [("num_coins", json!(0)), ("num_coins", json!(1001)), ("allocation_flattening", json!(2)), ("index_type", json!("bogus")),
                         ("index_category_id", json!("gone")), ("num_coins", json!(3)), ("direction", json!("hold")), ("direction", json!("buying"))] {
        let u = universe();
        let id = index_bot(&u, 3);
        exec(&u, &format!("UPDATE bots SET settings = json_set(settings, '$.{key}', json('{value}')) WHERE id = {id}"));
        let before = !reasons(&u, id).is_empty();
        convert(&u);
        assert_eq!(!reasons(&u, id).is_empty(), before, "{key} = {value}: {:?}", reasons(&u, id));
        assert_eq!(before, ![json!(3), json!("gone"), json!("buying")].contains(&value), "{key} = {value}: the case refuses or not as meant");
    }
}

/// An open limit order of the index bot is untouched by the conversion and polled into the books afterwards.
#[tokio::test(flavor = "current_thread")]
async fn an_open_limit_order_survives_the_conversion_and_is_booked() {
    let u = universe();
    let id = index_bot(&u, 3);
    member(&u, id, 0, 1.0);
    let tx = seed::insert_row(&u.o.primary, &u.s, id, u.m[0].0, &json!({ "external_id": "OLIM-1", "status": 0, "external_status": 0, "order_type": 1,
        "amount": "0.5", "quote_amount": "50", "price": "100", "created_at": "2026-09-01 10:00:00" }));
    let order = |c: &rusqlite::Connection| c.query_row("SELECT external_status, amount_exec, quote_amount_exec FROM transactions WHERE id = ?1", [tx],
        |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<f64>>(2)?))).unwrap();
    let before = order(&u.o.primary);
    convert(&u);
    assert_eq!(order(&u.o.primary), before);
    let t = ScriptedTransport::from_script(&json!({ "GET /v2/orders/OLIM-1": ok(json!({ "id": "OLIM-1", "status": "filled", "symbol": "AAA", "type": "limit", "side": "buy",
        "qty": "0.5", "filled_qty": "0.5", "filled_avg_price": "100", "limit_price": "100", "notional": null })) }));
    deltabadger::engine::polling::follow_up(&u.o.primary, &venue(t), id, tx, at(T0)).await.unwrap();
    let (status, amount, quote) = order(&u.o.primary);
    assert_eq!((status != before.0, amount, quote), (true, Some(0.5), Some(50.0)));
}

// ---------------------------------------------------------------- through real starts

fn v2_universe() -> (U, lease::EngineLock) {
    let u = universe_on(common::v2_install());
    let l = lease::lock(&Paths::from_env(&|_| None, u.d.path()), at(T0)).unwrap();
    (u, l)
}
fn take_over(u: &U, l: &lease::EngineLock) -> handover::Takeover { handover::take_over(l, &u.o, &seed::cipher(), "0.2.0", at(T0)).unwrap() }

/// An incumbent its venue doesn't price right now keeps its seat through a takeover from 2.x: the conversion seeds the group
/// rows, the claim forgets only their provenance (lease, slice 4a), and the first grouped derivation seats AAA unprobed.
#[tokio::test(flavor = "current_thread")]
async fn an_unpriced_incumbent_keeps_its_seat_through_a_real_takeover() {
    let (u, l) = v2_universe();
    let id = index_bot(&u, 2);
    exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_composed_index', 'nasdaq-100') WHERE id = {id}"));
    member(&u, id, 0, 1.0);
    take_over(&u, &l);
    assert_eq!(group_rows(&u, id), vec![("nasdaq-100".into(), "AAA".into(), 0, 1.0)], "seeded, provenance forgotten by the new ownership");
    seed::rank_indices(&u.o.primary, at("2099-01-01T00:00:00Z")); // the new ownership dropped 2.x's rankings; this engine's own
    refresh(&u, script([0.0, 200.0, 300.0, 400.0, 500.0]), id).await.unwrap();
    assert_eq!(symbols(&u, id), vec!["AAA", "BBB"]);
}

/// A composed retired index on an install this engine already owns stays composed through a restart (no new ownership):
/// the conversion seeds composed rows and the frozen group keeps its members.
#[tokio::test(flavor = "current_thread")]
async fn a_composed_retired_row_stays_composed_through_a_same_owner_restart() {
    let (u, l) = v2_universe();
    take_over(&u, &l);
    let id = index_bot(&u, 3);
    exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_composed_index', 'nasdaq-100') WHERE id = {id}"));
    member(&u, id, 0, 0.7);
    member(&u, id, 1, 0.3);
    retire(&u);
    let before = members(&u, id);
    assert!(matches!(take_over(&u, &l).claim, lease::Claim::AfterCrash));
    assert_eq!(group_rows(&u, id).iter().map(|g| g.2).collect::<Vec<_>>(), vec![1, 1], "still composed");
    refresh(&u, script(ASKS), id).await.unwrap();
    assert_eq!(members(&u, id), before);
}

/// An index bot 2.x created after an earlier takeover and handback converts at the next takeover; the takeover logs it.
#[tokio::test(flavor = "current_thread")]
async fn an_index_bot_2x_created_between_takeovers_converts_at_the_next() {
    let (u, l) = v2_universe();
    take_over(&u, &l);
    handover::hand_back(&l, &u.o, &FakeFactory::default(), &seed::cipher(), &FixedClock(at(T0))).await.unwrap();
    let id = index_bot(&u, 3);
    take_over(&u, &l);
    assert!(model::load_bot(&u.o.primary, id).unwrap().lone_group().is_some_and(|g| g.external_id == "nasdaq-100"));
    assert_eq!(index_bot::converted(&u.o.primary, id).unwrap(), format!("converted Bot {id} into a basket that follows its index"));
}

/// Q7: from its conversion on, the bot refuses the handback (and the lease stays Rust's).
#[tokio::test(flavor = "current_thread")]
async fn a_converted_bot_refuses_the_handback() {
    let (u, l) = v2_universe();
    let id = index_bot(&u, 3);
    take_over(&u, &l);
    let r = handover::hand_back(&l, &u.o, &FakeFactory::default(), &seed::cipher(), &FixedClock(at(T0))).await;
    assert!(matches!(r, Err(deltabadger::engine::EngineError::Ineligible(ref p)) if p == &vec![format!("bot {id}: its indexes run only on this engine; 2.x cannot run this bot")]), "{r:?}");
    assert_eq!(lease::read(&u.o.primary, &seed::cipher()).unwrap().unwrap()["engine"], "rust");
}

// ---------------------------------------------------------------- Codex branch review r1

/// A key named twice: the legacy reader (serde) reads the last, SQLite's `->` the first, so the row is kept, named.
#[test]
fn a_key_named_twice_keeps_the_row_unreadable() {
    let u = universe();
    let id = index_bot(&u, 3);
    exec(&u, &format!("UPDATE bots SET settings = replace(settings, '\"num_coins\":3', '\"num_coins\":1,\"num_coins\":3') WHERE id = {id}"));
    assert_eq!(model::load_bot(&u.o.primary, id).unwrap().settings["num_coins"], json!(3), "serde reads the last");
    let before = row(&u, id);
    assert_eq!(convert(&u), (vec![], vec![(id, Kept::Unreadable)]));
    assert_eq!(row(&u, id), before);
}

/// An index row with assets stored in `allocations` is kept as it is; an empty or null one converts.
#[test]
fn stored_allocations_keep_the_row() {
    let u = universe();
    let (assets, empty) = (index_bot(&u, 3), index_bot(&u, 3));
    exec(&u, &format!("UPDATE bots SET settings = json_set(settings, '$.allocations', json('{{\"{}\":1.0}}')) WHERE id = {assets};
                       UPDATE bots SET settings = json_set(settings, '$.allocations', json('{{}}')) WHERE id = {empty}", u.m[0].0));
    let before = row(&u, assets);
    let (converted, kept) = convert(&u);
    assert_eq!((ids(&converted), kept), (vec![empty], vec![(assets, Kept::Allocations)]));
    assert_eq!(row(&u, assets), before);
    assert_eq!(index_bot::would_keep(&u.o.primary, assets, Kept::Allocations).unwrap(), format!("Bot {assets} stays an index bot for now: it has assets stored beside its index"));
}

/// A member row whose ids don't decode (an archived index with a text ticker_id) keeps that bot and the pass goes on.
#[test]
fn a_damaged_member_row_keeps_its_bot_and_the_pass_goes_on() {
    let u = universe();
    let (damaged, fine) = (index_bot(&u, 3), index_bot(&u, 3));
    member(&u, damaged, 0, 1.0);
    exec(&u, &format!("UPDATE bots SET status = 7 WHERE id = {damaged}; PRAGMA foreign_keys = OFF; UPDATE bot_index_assets SET ticker_id = 'abc' WHERE bot_id = {damaged}; PRAGMA foreign_keys = ON"));
    let (converted, kept) = convert(&u);
    assert_eq!((ids(&converted), kept), (vec![fine], vec![(damaged, Kept::Unreadable)]));
}

/// `{}` is no rebalance in progress (rebalance::is_pending): it converts.
#[test]
fn an_empty_rebalance_marker_converts() {
    let u = universe();
    let id = index_bot(&u, 3);
    exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json('{{}}')) WHERE id = {id}"));
    assert_eq!(ids(&convert(&u).0), vec![id]);
}

/// A direction this build can't read is refused today and kept, named: never deleted into a live buy.
#[test]
fn an_unreadable_direction_keeps_the_row() {
    let u = universe();
    let id = index_bot(&u, 3);
    exec(&u, &format!("UPDATE bots SET settings = json_set(settings, '$.direction', 'hold') WHERE id = {id}"));
    assert_eq!(convert(&u), (vec![], vec![(id, Kept::Direction)]));
    assert!(!reasons(&u, id).is_empty());
}

/// Bounds count the `rust_composed_of` the first grouped refresh writes: a row that would stop opening after that refresh
/// is kept now; one with room converts, refreshes and still opens (Draft::load).
#[tokio::test(flavor = "current_thread")]
async fn bounds_leave_room_for_the_first_refresh() {
    for (slack, kept) in [(40usize, true), (400, false)] {
        let u = universe();
        let id = index_bot(&u, 3);
        let len = row(&u, id).2.len();
        exec(&u, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.note', '{}') WHERE id = {id}", "x".repeat(65_536 - slack - len - 10)));
        assert!(row(&u, id).2.len() <= 65_536 - slack + 5, "editable today");
        let (converted, k) = convert(&u);
        if kept { assert_eq!(k, vec![(id, Kept::Bounds)]); continue; }
        assert_eq!(ids(&converted), vec![id]);
        refresh(&u, script(ASKS), id).await.unwrap();
        assert!(row(&u, id).2.contains("rust_composed_of"));
        let user = model::load_bot(&u.o.primary, id).unwrap().user_id;
        let draft = deltabadger::web::bot::draft::Draft::load(&u.o.primary, user, id, "en");
        assert!(matches!(draft, Ok(Some(_))), "slack {slack}: the refreshed row still opens");
    }
}

/// D-INDEX-ONE-CLASS, rebalancing: a stopped index bot stored as selling with rebalancing on and AAA drifted past the
/// threshold. Converted, the scheduler's rebalance fire sells and buys exactly as it does for the same bot stored buying,
/// unconverted (2.x ignored an index bot's direction and rebalanced it); `check` names it.
#[tokio::test(flavor = "current_thread")]
async fn a_stopped_stored_seller_with_rebalancing_rebalances_once_converted_and_check_names_it() {
    let mut runs = vec![];
    for seller in [false, true] {
        let u = universe();
        let id = index_bot(&u, 3);
        let direction = if seller { "'$.direction', 'selling', " } else { "" };
        exec(&u, &format!("UPDATE bots SET status = 2, settings = json_set(settings, {direction}'$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.01) WHERE id = {id};
            INSERT INTO exchange_assets (exchange_id, asset_id, available, created_at, updated_at) SELECT exchange_id, base_asset_id, 1, created_at, updated_at FROM tickers WHERE quote = 'USD' AND ticker NOT LIKE '%/%'"));
        for (i, t) in [0.5, 0.3, 0.2].into_iter().enumerate() { member(&u, id, i, t); }
        for (i, (sym, qty)) in [("AAA", "10"), ("BBB", "3"), ("CCC", "2")].into_iter().enumerate() {
            seed::insert_stock_tx(&u.o.primary, &u.s, id, u.m[i].0, sym, &seed::TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("held-{sym}")), order_type: 0,
                amount: Some(qty), quote_amount: Some(match sym { "AAA" => "1000", "BBB" => "300", _ => "200" }), price: Some("100"), amount_exec: Some(qty),
                quote_amount_exec: Some(match sym { "AAA" => "1000", "BBB" => "300", _ => "200" }), created_at: "2026-08-01 14:00:00".into() });
        }
        if seller {
            let (converted, _) = convert(&u);
            assert_eq!(converted, vec![Converted { id, selling: false, rebalances: true }]);
            assert!(index_bot::would_convert(&u.o.primary, &converted[0]).unwrap().ends_with("at the next start, and it will rebalance: it is stored as selling, which an index bot never did"));
        }
        let snapshots: serde_json::Map<String, Value> = SYMBOLS.iter().map(|s| (s.to_string(), json!({ "latestTrade": { "p": 100 } }))).collect();
        let t = script_with([100.0; 5], json!({
            "GET /v2/positions": ok(json!([{ "symbol": "AAA", "asset_class": "us_equity", "qty": "10", "qty_available": "10" },
                                          { "symbol": "BBB", "asset_class": "us_equity", "qty": "3", "qty_available": "3" },
                                          { "symbol": "CCC", "asset_class": "us_equity", "qty": "2", "qty_available": "2" }])),
            "GET /v2/stocks/snapshots": ok(Value::Object(snapshots)),
            "GET /v1beta3/crypto/us/latest/trades": ok(json!({ "trades": {} })),
            "GET /v2/orders/OTX-1": ok(json!({ "id": "OTX-1", "status": "new", "symbol": "AAA", "asset_class": "us_equity", "type": "market", "side": "sell",
                                              "qty": "2", "notional": null, "filled_qty": "0", "filled_avg_price": null, "limit_price": null })),
        }));
        let mut e = engine(&u, &t);
        e.inject_rebalance_at(at(T0));
        step(&mut e, "2026-09-01T16:30:00Z").await;
        let orders: Vec<(Value, Value, Value)> = t.posted_orders().iter().map(|o| (o["symbol"].clone(), o["side"].clone(), o["qty"].clone())).collect();
        runs.push((orders, model::load_bot(&u.o.primary, id).unwrap().transient.get("rebalance_pending").cloned()));
    }
    assert!(!runs[0].0.is_empty(), "the unconverted buying index bot rebalances: {:?}", runs[0]);
    assert_eq!(runs[1], runs[0]);
}
