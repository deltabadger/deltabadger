//! PLAN index-as-ingredient slice 3a: a basket stored with `index_groups` edits, starts and is guarded through the real
//! settings writer (PATCH /bots/:id); merge and split ask `composition::current`. Grouped rows exist only in tests until
//! slice 4: they are seeded (`seed::insert_bot` with `index_groups`).
mod common;
use common::seed::{self, BotSpec};
use common::web::{Answer, Browser, Csrf};
use deltabadger::engine::{composition, index, model};
use deltabadger::store;
use deltabadger::web::App;
use serde_json::{json, Value};

const T0: &str = "2026-09-01T14:00:00.5Z";
const NOW: &str = "2026-09-01T14:00:30Z";
const SYMBOLS: [&str; 5] = ["AAA", "BBB", "CCC", "DDD", "EEE"];
const TURBO: [(&str, &str); 1] = [("accept", "text/vnd.turbo-stream.html")];

/// Five Alpaca stocks AAA..EEE in data-api's ND100 row, BTC/USD beside them, a configured provider and fresh catalogues
/// (tests/index_groups.rs's universe), a signed-in owner and the web app.
struct U { _d: tempfile::TempDir, o: store::Opened, s: seed::Seeded, m: Vec<(i64, i64)>, app: App, b: Browser }
fn universe() -> U {
    let (d, o, s) = common::install_alpaca();
    let m: Vec<(i64, i64)> = SYMBOLS.iter().map(|sym| seed::add_alpaca_stock(&o.primary, &s, sym)).collect();
    seed::insert_index(&o.primary, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"],
                       &json!({ "AAA.US": 5e12, "BBB.US": 3e12, "CCC.US": 2e12, "DDD.US": 1e12, "EEE.US": 5e11 }));
    let legacy = seed::index_bot(&o.primary, &s, "nasdaq-100", 3, 0.0, false);
    exec(&o, &format!("UPDATE bots SET status = 3 WHERE id = {legacy}"));
    seed::fresh_stock_jobs(&o.primary, common::web::at(T0));
    let hash = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
    o.primary.execute("UPDATE users SET encrypted_password=?1, confirmed_at='2026-01-01 00:00:00', wash_sale_enabled=0", [hash]).unwrap();
    let app = common::web::app(d.path(), "engine-test-secret", common::web::TestClock::at(NOW));
    use deltabadger::web::session::{self, SessionData};
    let b = Browser { cookie: Some(session::seal(&app.keys.session, &SessionData { user: Some((s.user_id, hash[..29].into())), ..Default::default() }, app.now())), page: None };
    U { _d: d, o, s, m, app, b }
}
fn exec(o: &store::Opened, sql: &str) { o.primary.execute_batch(sql).unwrap(); }
fn group(id: &str, n: i64, w: f64) -> Value { json!({ "index_type": "category", "index_category_id": id, "num_coins": n, "allocation_flattening": 1.0, "weight": w }) }
/// A stopped grouped basket.
fn grouped_bot(u: &U, allocations: Value, groups: Value) -> i64 {
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("allocations", allocations).with("index_groups", groups);
    spec.status = 2;
    seed::insert_bot(&u.o.primary, &u.s, &spec)
}
fn btc(u: &U, w: f64) -> Value { json!({ u.s.btc.to_string(): w }) }
fn settings(u: &U, id: i64) -> Value { model::load_bot(&u.o.primary, id).unwrap().settings }
fn transient(u: &U, id: i64) -> Value { model::load_bot(&u.o.primary, id).unwrap().transient }
fn status(u: &U, id: i64) -> i64 { u.o.primary.query_row("SELECT status FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap() }
async fn patch(u: &mut U, id: i64, fields: &[(&str, &str)]) -> Answer {
    let path = format!("/bots/{id}");
    assert_eq!(u.b.get(&u.app, &path).await.status, 200, "the page of bot {id}");
    u.b.send(&u.app, "PATCH", &path, Some(fields), Csrf::Header, &TURBO).await
}
async fn start(u: &mut U, id: i64) -> Answer {
    let path = format!("/bots/{id}");
    assert_eq!(u.b.get(&u.app, &path).await.status, 200);
    u.b.send(&u.app, "PATCH", &format!("{path}/start"), Some(&[]), Csrf::Header, &TURBO).await
}
fn weight(s: &Value, external_id: &str) -> f64 {
    s["index_groups"].as_array().unwrap().iter().find(|g| composition::external_id(g.as_object()) == external_id).unwrap()["weight"].as_f64().unwrap()
}
/// A composed digest as the grouped write leaves it (any revision): the settings' own.
fn compose(u: &U, id: i64) {
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let ranked: serde_json::Map<String, Value> = bot.index_external_ids().into_iter()
        .map(|g| { let r = index::ranked_row(&u.o.primary, &g).unwrap().unwrap(); (g, json!([r.id, r.ranked_at])) }).collect();
    composition::record(&u.o.primary, &bot, Value::Object(ranked), json!({})).unwrap();
}

#[tokio::test]
async fn a_lone_group_with_empty_allocations_edits_and_starts() {
    let mut u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    let saved = patch(&mut u, id, &[("bots_dca_multi_asset[quote_amount]", "70")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!(s["quote_amount"], json!(70.0));
    assert_eq!(s["index_groups"].as_array().unwrap().len(), 1, "{s}");
    let started = start(&mut u, id).await;
    assert_eq!(started.status, 200, "{}", started.body);
    assert_eq!(status(&u, id), 1, "{}", started.body);
}

#[tokio::test]
async fn btc_half_and_an_index_half_edits_reloads_and_starts() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    let key = format!("bots_dca_multi_asset[allocations][{}]", u.s.btc);
    let saved = patch(&mut u, id, &[(&key, "40"), ("bots_dca_multi_asset[index_groups][nasdaq-100][weight]", "60")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!(s["allocations"][u.s.btc.to_string()], json!(0.4));
    assert_eq!(weight(&s, "nasdaq-100"), 0.6);
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200, "reloads");
    let started = start(&mut u, id).await;
    assert_eq!(status(&u, id), 1, "{}", started.body);
    assert_eq!(settings(&u, id)["allocations"][u.s.btc.to_string()], json!(0.4), "the start keeps BTC at its weight");
}

#[tokio::test]
async fn the_one_asset_shortcut_leaves_btc_beside_an_index_at_its_weight() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[quote_amount]", "61")]).await.status, 200);
    assert_eq!(start(&mut u, id).await.status, 200);
    assert_eq!(settings(&u, id)["allocations"][u.s.btc.to_string()], json!(0.5), "never pinned to 100 %");
}

#[tokio::test]
async fn an_unbalanced_set_of_ingredients_refuses_the_start() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.4)]));
    let refused = start(&mut u, id).await;
    assert_eq!(status(&u, id), 2, "{}", refused.body);
}

#[tokio::test]
async fn more_than_max_assets_ingredients_refuse_the_start() {
    let mut u = universe();
    let mut weights = serde_json::Map::new();
    for i in 0..100 {
        exec(&u.o, &format!("INSERT INTO assets (external_id, symbol, name, created_at, updated_at) VALUES ('x{i}', 'X{i}', 'X{i}', '2026-01-01', '2026-01-01')"));
        weights.insert(u.o.primary.last_insert_rowid().to_string(), json!(0.0));
    }
    let id = grouped_bot(&u, Value::Object(weights), json!([group("nasdaq-100", 3, 1.0)]));
    let refused = start(&mut u, id).await;
    assert_eq!(status(&u, id), 2);
    assert!(refused.body.contains("too many") || refused.body.contains("Remove 1"), "{}", refused.body);
}

#[tokio::test]
async fn normalize_covers_group_weights_exactly() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.3), json!([group("nasdaq-100", 3, 0.1)]));
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[normalize_allocations]", "1")]).await.status, 200);
    let s = settings(&u, id);
    assert!((s["allocations"][u.s.btc.to_string()].as_f64().unwrap() - 0.75).abs() < 1e-12, "{s}");
    assert!((weight(&s, "nasdaq-100") - 0.25).abs() < 1e-12, "{s}");
    // 99.96 % and 0.04 % stay apart: no rounding to the slider's 0.1 %.
    let id = grouped_bot(&u, btc(&u, 0.9996), json!([group("nasdaq-100", 3, 0.0004)]));
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[normalize_allocations]", "1")]).await.status, 200);
    assert!((weight(&settings(&u, id), "nasdaq-100") - 0.0004).abs() < 1e-12);
}

#[tokio::test]
async fn a_group_count_or_flattening_out_of_bounds_is_refused() {
    let mut u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    for (field, value) in [("num_coins", "1"), ("allocation_flattening", "2"), ("allocation_flattening", "-1")] {
        let key = format!("bots_dca_multi_asset[index_groups][nasdaq-100][{field}]");
        let refused = patch(&mut u, id, &[(&key, value)]).await;
        assert_eq!(refused.status, 422, "{field}={value}: {}", refused.body);
    }
    assert_eq!(settings(&u, id)["index_groups"][0]["num_coins"], json!(3), "nothing written");
    // Past the bounded index's size it holds the whole index, as a `Bots::DcaIndex` does.
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][nasdaq-100][num_coins]", "6")]).await.status, 200);
    assert_eq!(settings(&u, id)["index_groups"][0]["hold_all"], json!(true));
    let refused = patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][top-coins][weight]", "10")]).await;
    assert_eq!(refused.status, 422, "a group the bot does not hold: {}", refused.body);
}

#[tokio::test]
async fn a_plain_basket_takes_no_group_fields() {
    let mut u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let id = seed::insert_bot(&u.o.primary, &u.s, &spec);
    let refused = patch(&mut u, id, &[("bots_dca_multi_asset[label]", "X"), ("bots_dca_multi_asset[index_groups][nasdaq-100][weight]", "10")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(settings(&u, id).get("index_groups").is_none());
}

/// Ten unbounded category groups (no index row: up to MAX_COINS each), the first at `first` and the rest at 100, and one
/// named asset: `first` + 901 members.
fn wide_bot(u: &U, first: i64) -> i64 {
    let groups: Vec<Value> = (0..10).map(|i| group(&format!("cat-{i}"), if i == 0 { first } else { 100 }, 0.09)).collect();
    grouped_bot(u, btc(u, 0.1), json!(groups))
}

#[tokio::test]
async fn a_summed_expansion_above_1000_refuses_a_growing_save() {
    let mut u = universe();
    let id = wide_bot(&u, 99);
    let refused = patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][cat-0][num_coins]", "100")]).await;
    assert_eq!(refused.status, 422, "1001 members: {}", refused.body);
    assert_eq!(settings(&u, id)["index_groups"][0]["num_coins"], json!(99));
    // A bot already above it (stored so) still saves, shrinks, and takes edits that do not grow it.
    let id = wide_bot(&u, 100);
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[label]", "Wide")]).await.status, 200);
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][cat-0][num_coins]", "99")]).await.status, 200);
}

#[tokio::test]
async fn growth_past_1000_active_members_then_a_reducing_save_succeeds() {
    let mut u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    for i in 0..1100 {
        exec(&u.o, &format!("INSERT INTO assets (external_id, symbol, name, created_at, updated_at) VALUES ('m{i}', 'M{i}', 'M{i}', '2026-01-01', '2026-01-01')"));
        let asset = u.o.primary.last_insert_rowid();
        exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                             VALUES ({id}, {asset}, {}, 0.0009, 1, '2026-01-01', '2026-01-01', '2026-01-01')", u.s.ticker_id));
    }
    let saved = patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][nasdaq-100][num_coins]", "2")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(settings(&u, id)["index_groups"][0]["num_coins"], json!(2));
}

#[tokio::test]
async fn a_bot_with_two_thousand_exited_members_opens_edits_and_starts() {
    let mut u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let id = seed::insert_bot(&u.o.primary, &u.s, &spec);
    for i in 0..2000 {
        exec(&u.o, &format!("INSERT INTO assets (external_id, symbol, name, created_at, updated_at) VALUES ('e{i}', 'E{i}', 'E{i}', '2026-01-01', '2026-01-01')"));
        let asset = u.o.primary.last_insert_rowid();
        exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, exited_at, created_at, updated_at) \
                             VALUES ({id}, {asset}, {}, 0, 0, '2026-01-01', '2026-02-01', '2026-01-01', '2026-01-01')", u.s.ticker_id));
    }
    let saved = patch(&mut u, id, &[("bots_dca_multi_asset[quote_amount]", "70")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let started = start(&mut u, id).await;
    assert_eq!(status(&u, id), 1, "{}", started.body);
}

#[tokio::test]
async fn a_grouped_edit_marks_a_resync_writes_no_member_and_forgets_the_digest() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                         VALUES ({id}, {}, {}, 1.0, 1, '2026-01-01', '2026-01-01', '2026-01-01')", u.m[0].0, u.m[0].1));
    exec(&u.o, &format!("INSERT INTO rust_bot_group_members VALUES ({id}, 'nasdaq-100', {}, {}, 1, 1.0)", u.m[0].0, u.m[0].1));
    compose(&u, id);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert!(composition::current(&u.o.primary, &bot, common::web::at(NOW)).unwrap(), "composed");
    let members = |u: &U| u.o.primary.query_row(&format!("SELECT group_concat(asset_id || ':' || target_allocation || ':' || in_index) FROM bot_index_assets WHERE bot_id = {id}"), [], |r| r.get::<_, String>(0)).unwrap();
    let before = members(&u);
    // A count edit keeps the digest's provenance rows and the members; it changes what the composition is derived from.
    let saved = patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][nasdaq-100][num_coins]", "4")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let t = transient(&u, id);
    assert!(t.get(composition::COMPOSED_OF_KEY).is_none(), "digest forgotten: {t}");
    assert!(t.get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()), "resync marked: {t}");
    assert_eq!(members(&u), before, "the web writes no member of a grouped bot");
    let rows: i64 = u.o.primary.query_row(&format!("SELECT count(*) FROM rust_bot_group_members WHERE bot_id = {id}"), [], |r| r.get(0)).unwrap();
    assert_eq!(rows, 1, "a count edit keeps the group rows");
}

#[tokio::test]
async fn only_a_digest_input_forgets_the_digest() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.limit_ordered', json('true')) WHERE id = {id}"));
    compose(&u, id);
    // The limit distance moves no probe (the digest reads the limit mode only): kept, no resync.
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[limit_order_pcnt_distance]", "0.5")]).await.status, 200);
    let t = transient(&u, id);
    assert!(t.get(composition::COMPOSED_OF_KEY).is_some(), "{t}");
    assert!(t.get(index::RESYNC_KEY).is_none_or(Value::is_null), "{t}");
    // The limit mode does.
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[limit_ordered]", "0")]).await.status, 200);
    let t = transient(&u, id);
    assert!(t.get(composition::COMPOSED_OF_KEY).is_none(), "{t}");
    assert!(t.get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()), "{t}");
}

#[tokio::test]
async fn a_group_edit_while_running_is_locked() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET status = 1 WHERE id = {id}"));
    let refused = patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][nasdaq-100][weight]", "40")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(weight(&settings(&u, id), "nasdaq-100"), 0.5);
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[label]", "Running")]).await.status, 200);
}

#[tokio::test]
async fn a_grouped_start_marks_a_resync() {
    let mut u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    let started = start(&mut u, id).await;
    assert_eq!(status(&u, id), 1, "{}", started.body);
    assert!(transient(&u, id).get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()));
}

#[tokio::test]
async fn a_stored_subject_survives_its_assets_exit() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    // EEE was a member; it left the index, and the price rule still watches it.
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, exited_at, created_at, updated_at) \
                         VALUES ({id}, {}, {}, 0, 0, '2026-01-01', '2026-02-01', '2026-01-01', '2026-01-01')", u.m[4].0, u.m[4].1));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit_in_ticker_id', {}) WHERE id = {id}", u.m[4].1));
    let saved = patch(&mut u, id, &[("bots_dca_multi_asset[label]", "Kept")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(settings(&u, id)["price_limit_in_ticker_id"], json!(u.m[4].1));
}

#[tokio::test]
async fn a_group_edit_never_arms_a_rule_on_a_nonmember() {
    let mut u = universe();
    // AAA sorts before BTC in the venue's universe; the bot holds BTC only (no member yet).
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true')) WHERE id = {id}"));
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][nasdaq-100][weight]", "50")]).await.status, 200);
    let subject = settings(&u, id)["price_limit_in_ticker_id"].clone();
    assert_ne!(subject, json!(u.m[0].1), "never AAA");
    assert!(subject.is_null() || subject == json!(u.s.ticker_id), "{subject}");
}

#[test]
fn current_holds_only_for_the_settings_own_fresh_composition() {
    let u = universe();
    let now = common::web::at(NOW);
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    let current = || composition::current(&u.o.primary, &model::load_bot(&u.o.primary, id).unwrap(), now).unwrap();
    assert!(!current(), "no digest");
    compose(&u, id);
    assert!(current());
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups[0].num_coins', 4) WHERE id = {id}"));
    assert!(!current(), "digest of other settings");
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups[0].num_coins', 3) WHERE id = {id}"));
    assert!(current());
    seed::rank_indices(&u.o.primary, "2099-01-02T00:00:00Z".parse().unwrap());
    assert!(!current(), "a ranking published after the composition");
    compose(&u, id);
    assert!(current());
    exec(&u.o, "INSERT INTO rust_index_retired VALUES ('nasdaq-100', 'data-api', '2026-09-01T00:00:00Z')");
    assert!(!current(), "a retired group");
    exec(&u.o, "DELETE FROM rust_index_retired; DELETE FROM rust_index_rankings;");
    assert!(!current(), "an unranked group");
    seed::rank_indices(&u.o.primary, "2099-01-01T00:00:00Z".parse().unwrap());
    compose(&u, id);
    assert!(current());
    seed::rank_indices(&u.o.primary, "2026-01-01T00:00:00Z".parse().unwrap());
    compose(&u, id);
    assert!(!current(), "a stale ranking");
    // A plain basket keeps today's rules.
    let plain = seed::insert_bot(&u.o.primary, &u.s, &BotSpec::weekly(5.0, "2026-09-01 14:00:00"));
    assert!(composition::current(&u.o.primary, &model::load_bot(&u.o.primary, plain).unwrap(), now).unwrap());
}

/// A stopped basket holding `assets` as current members at equal targets.
fn member_bot(u: &U, assets: &[(i64, i64)]) -> i64 {
    let w = 1.0 / assets.len() as f64;
    let weights: serde_json::Map<String, Value> = assets.iter().map(|(a, _)| (a.to_string(), json!(w))).collect();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("allocations", Value::Object(weights)); spec.status = 2;
    let id = seed::insert_bot(&u.o.primary, &u.s, &spec);
    for (asset, ticker) in assets {
        exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                             VALUES ({id}, {asset}, {ticker}, {w}, 1, '2026-01-01', '2026-01-01', '2026-01-01')"));
    }
    id
}
async fn merge(u: &mut U, ids: &[i64]) -> Answer {
    assert_eq!(u.b.get(&u.app, "/bots").await.status, 200);
    let ids: Vec<String> = ids.iter().map(i64::to_string).collect();
    let form: Vec<(&str, &str)> = ids.iter().map(|id| ("ids[]", id.as_str())).collect();
    u.b.send(&u.app, "POST", "/bots/merge", Some(&form), Csrf::Header, &TURBO).await
}
fn alive(u: &U) -> i64 { u.o.primary.query_row("SELECT count(*) FROM bots WHERE status <> 3", [], |r| r.get(0)).unwrap() }

#[tokio::test]
async fn merge_refuses_a_grouped_source_that_is_not_current() {
    let mut u = universe();
    let grouped = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                         VALUES ({grouped}, {}, {}, 1.0, 1, '2026-01-01', '2026-01-01', '2026-01-01')", u.m[0].0, u.m[0].1));
    exec(&u.o, &format!("UPDATE bots SET label = 'Grouped' WHERE id = {grouped}"));
    let plain = member_bot(&u, &[u.m[1]]);
    let before = alive(&u);
    let refused = merge(&mut u, &[grouped, plain]).await;
    assert!(refused.body.contains("Grouped can&#39;t be merged right now") || refused.body.contains("Grouped can't be merged right now"), "{}", refused.body);
    assert_eq!(alive(&u), before, "nothing merged");
    // Composed from its settings, the source is mergeable.
    compose(&u, grouped);
    let merged = merge(&mut u, &[grouped, plain]).await;
    assert!(!merged.body.contains("can&#39;t be merged") && !merged.body.contains("can't be merged"), "{}", merged.body);
}

#[tokio::test]
async fn split_refuses_a_grouped_source_that_is_not_current() {
    let mut u = universe();
    let grouped = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    exec(&u.o, &format!("UPDATE bots SET label = 'Grouped' WHERE id = {grouped}"));
    for (asset, ticker) in &u.m[..2] {
        exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                             VALUES ({grouped}, {asset}, {ticker}, 0.5, 1, '2026-01-01', '2026-01-01', '2026-01-01')"));
    }
    assert_eq!(u.b.get(&u.app, &format!("/bots/{grouped}")).await.status, 200);
    let id = grouped.to_string();
    let refused = u.b.send(&u.app, "POST", "/bots/split", Some(&[("ids[]", id.as_str())]), Csrf::Header, &TURBO).await;
    assert!(refused.body.contains("Grouped can&#39;t be split right now") || refused.body.contains("Grouped can't be split right now"), "{}", refused.body);
    assert_eq!(status(&u, grouped), 2);
}

#[tokio::test]
async fn a_merge_above_max_assets_refuses_before_any_history_moves() {
    let mut u = universe();
    let stocks: Vec<(i64, i64)> = (0..101).map(|i| seed::add_alpaca_stock(&u.o.primary, &u.s, &format!("S{i:03}"))).collect();
    let (a, b) = (member_bot(&u, &stocks[..51]), member_bot(&u, &stocks[51..]));
    let before = alive(&u);
    let refused = merge(&mut u, &[a, b]).await;
    assert!(refused.body.contains("up to 100 assets"), "{}", refused.body);
    assert_eq!(alive(&u), before);
}

/// An MCP tool's answer, called by `owner`'s personal token (tests/two_accounts_names_merge.rs's helper).
fn mcp(u: &U, name: &str, args: Value) -> Value {
    use deltabadger::web::{bearer::Bearer, mcp::tools::{self, Called}};
    let c = &u.o.primary;
    let on: serde_json::Map<String, Value> = tools::NAMES.iter().map(|t| (t.to_string(), json!(true))).collect();
    c.execute("UPDATE users SET mcp_settings=?1, rest_settings=?1 WHERE id=?2", rusqlite::params![json!({"tool_permissions": on}).to_string(), u.s.user_id]).unwrap();
    c.execute("INSERT OR IGNORE INTO oauth_applications(name,uid,personal_access_token,personal_owner_id,created_at,updated_at) VALUES('test','test-'||?1,1,?1,'2026-09-10','2026-09-10')", [u.s.user_id]).unwrap();
    let application: i64 = c.query_row("SELECT id FROM oauth_applications WHERE personal_owner_id = ?1", [u.s.user_id], |r| r.get(0)).unwrap();
    match tools::call(c, &u.app, Bearer { user_id: u.s.user_id, application_id: application, token_id: 1 }, name, &args).unwrap() { Called::Done(v) => v, _ => panic!("{name}: not answered at once") }
}

#[tokio::test]
async fn mcp_allocations_on_a_grouped_bot_keep_the_groups_and_resync() {
    let u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    compose(&u, id);
    let answer = mcp(&u, "update_bot_settings", json!({ "bot_id": id, "allocations": "BTC:40" }));
    assert!(answer.to_string().contains("settings updated"), "{answer}");
    let (s, t) = (settings(&u, id), transient(&u, id));
    assert_eq!(s["allocations"][u.s.btc.to_string()], json!(0.4), "{s}");
    assert_eq!(weight(&s, "nasdaq-100"), 0.5, "{s}");
    assert!(t.get(composition::COMPOSED_OF_KEY).is_none() && t.get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()), "{t}");
    // MCP's contract: no group field reaches the writer.
    let refused = mcp(&u, "update_bot_settings", json!({ "bot_id": id, "num_coins": 4 }));
    assert!(refused.to_string().contains("index bots only"), "{refused}");
}

// Codex 3a branch r1.

/// The group's bound is the ranked row the engine selects (the provider's newest), never the lowest-id catalogue row.
#[tokio::test]
async fn a_group_is_bounded_by_the_engines_ranked_row() {
    let mut u = universe();
    // A later catalogue row of the same id (another source), ranked after the data-api one: the engine composes from it.
    exec(&u.o, "INSERT INTO indices (external_id, source, name, top_coins, top_coins_by_exchange, available_exchanges, weights, weight, created_at, updated_at) \
                VALUES ('nasdaq-100', 'coingecko', 'ND', '[\"T0\",\"T1\",\"T2\",\"T3\",\"T4\",\"T5\",\"T6\",\"T7\",\"T8\",\"T9\"]', '{}', '{}', '{}', 0, '2026-01-01', '2026-01-01')");
    let newer = u.o.primary.last_insert_rowid();
    index::record_ranking(&u.o.primary, newer, "data-api", &deltabadger::engine::provider::fingerprint(&u.o.primary).unwrap().unwrap(), "2099-01-02T00:00:00Z".parse().unwrap()).unwrap();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 8, 1.0)]));
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[label]", "Eight")]).await.status, 200);
    assert_eq!(settings(&u, id)["index_groups"][0]["num_coins"], json!(8), "not clamped to the older five-member row");
}

#[tokio::test]
async fn a_grouped_subject_comes_from_the_pool_posted_or_not() {
    let mut u = universe();
    // A group-only bot before its first composition holds nothing: no subject may be posted, on or off, either side.
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    let aaa = u.m[0].1.to_string();
    for (on, key) in [("price_limited", "price_limit_in_ticker_id"), ("price_limited", "sell_price_limit_in_ticker_id")] {
        let (on, key) = (format!("bots_dca_multi_asset[{on}]"), format!("bots_dca_multi_asset[{key}]"));
        for enabled in ["1", "0"] {
            let refused = patch(&mut u, id, &[(&on, enabled), (&key, &aaa)]).await;
            assert_eq!(refused.status, 422, "{key} on={enabled}: {}", refused.body);
        }
    }
    assert!(settings(&u, id).get("price_limit_in_ticker_id").is_none_or(Value::is_null));
    // Beside a member, a posted member's ticker is taken; a non-member's is refused even with the rule off.
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                         VALUES ({id}, {}, {}, 0.5, 1, '2026-01-01', '2026-01-01', '2026-01-01')", u.m[1].0, u.m[1].1));
    let bbb = u.m[1].1.to_string();
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[price_limit_in_ticker_id]", &bbb)]).await.status, 200);
    assert_eq!(patch(&mut u, id, &[("bots_dca_multi_asset[price_limited]", "0"), ("bots_dca_multi_asset[price_limit_in_ticker_id]", &aaa)]).await.status, 422);
}

#[tokio::test]
async fn a_default_subject_is_recomputed_from_the_final_candidate() {
    let mut u = universe();
    // BTC is the only thing held; the rule is on with no subject. Removing BTC leaves nothing to watch: no default.
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true')) WHERE id = {id}"));
    let btc_id = u.s.btc.to_string();
    let saved = patch(&mut u, id, &[("bots_dca_multi_asset[remove_asset_id]", &btc_id)]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert!(s.get("price_limit_in_ticker_id").is_none_or(Value::is_null), "{s}");
}

#[tokio::test]
async fn a_grouped_start_judges_the_candidate_venues_pair() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    // A member row still on another venue's tradable BTC pair (kept until the resync); BTC here no longer trades.
    exec(&u.o, "INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Old', '0.1', '0.1', '2026-01-01', '2026-01-01')");
    let old = u.o.primary.last_insert_rowid();
    exec(&u.o, &format!("CREATE TEMP TABLE t AS SELECT * FROM tickers WHERE id = {}; UPDATE t SET id = NULL, exchange_id = {old}; INSERT INTO tickers SELECT * FROM t; DROP TABLE t;", u.s.ticker_id));
    let elsewhere = u.o.primary.last_insert_rowid();
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, created_at, updated_at) \
                         VALUES ({id}, {}, {elsewhere}, 0.5, 1, '2026-01-01', '2026-01-01', '2026-01-01'); UPDATE tickers SET trading_enabled = 0 WHERE id = {};", u.s.btc, u.s.ticker_id));
    let refused = start(&mut u, id).await;
    assert_eq!(status(&u, id), 2, "{}", refused.body);
}

#[tokio::test]
async fn a_reordered_repost_keeps_the_digest() {
    let mut u = universe();
    let (a, b) = (u.s.btc.to_string(), u.m[0].0.to_string());
    let id = grouped_bot(&u, json!({ a.clone(): 0.25, b.clone(): 0.25 }), json!([group("nasdaq-100", 3, 0.5)]));
    compose(&u, id);
    let (ka, kb) = (format!("bots_dca_multi_asset[allocations][{a}]"), format!("bots_dca_multi_asset[allocations][{b}]"));
    assert_eq!(patch(&mut u, id, &[(&kb, "25"), (&ka, "25")]).await.status, 200);
    let t = transient(&u, id);
    assert!(t.get(composition::COMPOSED_OF_KEY).is_some(), "{t}");
}

#[tokio::test]
async fn a_huge_group_count_is_a_422() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("cat-x", 3, 0.5)]));
    let refused = patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][cat-x][num_coins]", "9223372036854775807")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups[0].num_coins', 9223372036854775807) WHERE id = {id}"));
    let refused = patch(&mut u, id, &[("bots_dca_multi_asset[index_groups][cat-x][weight]", "50")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
}

#[test]
fn the_newest_exits_are_loaded_whatever_their_ids() {
    use deltabadger::web::bot::{Bot, For};
    let u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let id = seed::insert_bot(&u.o.primary, &u.s, &spec);
    let mut first = 0;
    for i in 0..1001 {
        exec(&u.o, &format!("INSERT INTO assets (external_id, symbol, name, created_at, updated_at) VALUES ('r{i}', 'R{i}', 'R{i}', '2026-01-01', '2026-01-01')"));
        let asset = u.o.primary.last_insert_rowid();
        // The first row re-entered and left again last.
        let exited = if i == 0 { "2026-08-01".to_string() } else { format!("2026-02-01 00:{:02}:{:02}", i / 60 % 60, i % 60) };
        exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, exited_at, created_at, updated_at) \
                             VALUES ({id}, {asset}, {}, 0, 0, '2026-01-01', '{exited}', '2026-01-01', '2026-01-01')", u.s.ticker_id));
        if i == 0 { first = asset; }
    }
    let bot = Bot::find(&u.o.primary, u.s.user_id, id, For::Page, "en").unwrap().unwrap();
    assert!(bot.memberships.iter().any(|m| m.asset.id == first), "the latest exit is loaded");
}
