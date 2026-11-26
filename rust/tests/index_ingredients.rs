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
    let page = u.b.get(&u.app, &path).await;
    assert_eq!(page.status, 200, "the page of bot {id}: {}", page.body.chars().take(600).collect::<String>());
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
    // Slice 7: a wider basket's group fields go through `index_groups`; the flat ones say so.
    let refused = mcp(&u, "update_bot_settings", json!({ "bot_id": id, "num_coins": 4 }));
    assert!(refused.to_string().contains("use index_groups"), "{refused}");
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

// Slice 3b: ingredient operations on a row already grouped.

fn member(u: &U, bot: i64, (asset, ticker): (i64, i64), target: f64, in_index: bool) {
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, entered_at, exited_at, created_at, updated_at) \
                         VALUES ({bot}, {asset}, {ticker}, {target}, {}, '2026-01-01', {}, '2026-01-01', '2026-01-01')", in_index as i64, if in_index { "NULL" } else { "'2026-02-01'" }));
}
fn group_row(u: &U, bot: i64, external_id: &str, (asset, ticker): (i64, i64), weight: f64) {
    exec(&u.o, &format!("INSERT INTO rust_bot_group_members VALUES ({bot}, '{external_id}', {asset}, {ticker}, 1, {weight})"));
}
fn group_rows(u: &U, bot: i64) -> Vec<String> {
    u.o.primary.prepare(&format!("SELECT index_external_id || ':' || asset_id FROM rust_bot_group_members WHERE bot_id = {bot} ORDER BY 1")).unwrap()
        .query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
}
fn targets(u: &U, bot: i64) -> Vec<(i64, f64)> {
    u.o.primary.prepare(&format!("SELECT asset_id, CAST(target_allocation AS REAL) FROM bot_index_assets WHERE bot_id = {bot} AND in_index = 1 ORDER BY asset_id")).unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
}
fn generation(u: &U, bot: i64) -> i64 { transient(u, bot)["run_generation"].as_i64().unwrap_or(0) }
/// A second category, cat-b over AAA, BBB, CCC (offered here: three members at the venue and quote).
fn cat_b(u: &U) { seed::insert_index(&u.o.primary, "cat-b", &["AAA.US", "BBB.US", "CCC.US"], &json!({ "AAA.US": 3e12, "BBB.US": 2e12, "CCC.US": 1e12 })); }
const ROOT: &str = "bots_dca_multi_asset";
fn key(name: &str) -> String { format!("{ROOT}[{name}]") }

/// PLAN slice 4c: a first group opens on the web form: `add_index` on a plain basket appends the group at 0 % beside its
/// assets (the user then sets the weights).
#[tokio::test]
async fn a_first_group_opens_on_the_web_form() {
    let mut u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let id = seed::insert_bot(&u.o.primary, &u.s, &spec);
    let added = patch(&mut u, id, &[(&key("add_index"), "nasdaq-100"), (&key("quote_amount"), "70")]).await;
    assert_eq!(added.status, 200, "{}", added.body);
    let s = settings(&u, id);
    assert_eq!(s["quote_amount"], json!(70.0));
    assert_eq!(s["index_groups"].as_array().map(|g| (g.len(), weight(&s, "nasdaq-100"))), Some((1, 0.0)), "{s}");
}

/// D-REST-INGREDIENTS (slice 7): REST takes `add_index` as the form does; a group field naming a group the bot lacks is
/// refused whole, its sibling amount unsaved.
#[tokio::test]
async fn a_first_group_opens_over_rest_and_a_foreign_group_field_is_refused_whole() {
    let u = universe();
    rest_token(&u);
    let id = plain(&u);
    let (status, text) = rest(&u, "PATCH", &format!("/bots/{id}"), json!({ "quote_amount": 70, "index_groups": { "nasdaq-100": { "weight": 50 } } })).await;
    assert_eq!(status, 422, "{text}");
    assert_eq!(settings(&u, id)["quote_amount"], json!(60.0), "the whole request refused: {text}");
    let (status, text) = rest(&u, "PATCH", &format!("/bots/{id}?add_index=nasdaq-100"), json!({ "quote_amount": 70 })).await;
    assert_eq!(status, 200, "{text}");
    let s = settings(&u, id);
    assert_eq!((s["quote_amount"].clone(), s["index_groups"].as_array().map(Vec::len)), (json!(70.0), Some(1)), "{s}");
}

/// BTC 0.5 + AAA 0.5 + the index at 0, the index's members still standing.
fn halves(u: &U) -> i64 {
    let id = grouped_bot(u, json!({ u.s.btc.to_string(): 0.5, u.m[0].0.to_string(): 0.5 }), json!([group("nasdaq-100", 3, 0.0)]));
    member(u, id, (u.s.btc, u.s.ticker_id), 0.5, true);
    member(u, id, u.m[0], 0.5, true);
    member(u, id, u.m[1], 0.0, true);
    group_row(u, id, "nasdaq-100", u.m[1], 1.0);
    compose(u, id);
    id
}

#[tokio::test]
async fn removing_the_last_group_of_btc_and_aaa_halves_writes_their_members() {
    let mut u = universe();
    let id = halves(&u);
    let before = generation(&u, id);
    let saved = patch(&mut u, id, &[(&key("remove_index_id"), "nasdaq-100")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let (s, t) = (settings(&u, id), transient(&u, id));
    assert!(s.get("index_groups").is_none(), "the key is removed, not left empty: {s}");
    let mut want = vec![(u.s.btc, 0.5), (u.m[0].0, 0.5)]; want.sort_by_key(|w| w.0);
    assert_eq!(targets(&u, id), want, "the basket's members, written in the same transaction");
    assert!(group_rows(&u, id).is_empty());
    assert!(t.get(composition::COMPOSED_OF_KEY).is_none() && t.get(index::RESYNC_KEY).is_none_or(Value::is_null), "{t}");
    assert_eq!(generation(&u, id), before + 1);
    // The same group put back (as slice 4 will allow) composes anew: the old digest cannot be current.
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups', json('[{}]')) WHERE id = {id}", group("nasdaq-100", 3, 0.0)));
    assert!(!composition::current(&u.o.primary, &model::load_bot(&u.o.primary, id).unwrap(), common::web::at(NOW)).unwrap());
}

#[tokio::test]
async fn the_last_group_goes_only_from_a_balanced_basket() {
    let mut u = universe();
    let lone = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    let refused = patch(&mut u, lone, &[(&key("remove_index_id"), "nasdaq-100")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("Set the asset weights first"), "{}", refused.body);
    let zero = grouped_bot(&u, btc(&u, 0.0), json!([group("nasdaq-100", 3, 1.0)]));
    assert_eq!(patch(&mut u, zero, &[(&key("remove_index_id"), "nasdaq-100")]).await.status, 422);
    assert!(settings(&u, zero).get("index_groups").is_some());
    // Weights posted in the same save count.
    let k = format!("{ROOT}[allocations][{}]", u.s.btc);
    assert_eq!(patch(&mut u, zero, &[(&k, "100"), (&key("remove_index_id"), "nasdaq-100")]).await.status, 200);
}

#[tokio::test]
async fn a_selling_bot_keeps_its_last_group_and_a_buying_one_drops_its_stored_sale() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 1.0), json!([group("nasdaq-100", 3, 0.0)]));
    member(&u, id, (u.s.btc, u.s.ticker_id), 1.0, true);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling', '$.sell_quote_amount', 100) WHERE id = {id}"));
    let refused = patch(&mut u, id, &[(&key("remove_index_id"), "nasdaq-100")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("back to buying first"), "{}", refused.body);
    // Buying, with a base sale (0.5 BTC) and a base cap stored from before: both cleared as the last group goes.
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.direction', 'buying', '$.sell_denomination', 'base', '$.sell_amount', 0.5, \
                         '$.base_amount_limited', json('true'), '$.base_amount_limit', 2) WHERE id = {id}"));
    let saved = patch(&mut u, id, &[(&key("remove_index_id"), "nasdaq-100")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!((s["sell_denomination"].clone(), s["base_amount_limited"].clone()), (json!("quote"), json!(false)), "{s}");
    assert!(s.get("sell_quote_amount").is_none_or(Value::is_null) && s.get("sell_amount").is_none_or(Value::is_null), "the sale starts anew: {s}");
}

#[tokio::test]
async fn every_group_operation_takes_the_full_refusal() {
    let mut u = universe();
    cat_b(&u);
    let ops: [&[(&str, &str)]; 4] = [&[("remove_index_id", "cat-b")], &[("change_index_id", "cat-b"), ("change_index_to", "top-coins")],
                                     &[("ungroup_index_id", "cat-b")], &[("add_index", "top-coins")]];
    let cases: [(&str, &dyn Fn(i64) -> String); 4] = [
        ("running", &|id| format!("UPDATE bots SET status = 1 WHERE id = {id}")),
        ("rebalance pending", &|id| format!("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json('{{\"at\":1}}')) WHERE id = {id}")),
        ("an open REGULAR order, no intent", &|_| String::new()),
        ("a redeploy in progress", &|_| String::new()),
    ];
    for (name, case) in cases {
        let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.25), group("cat-b", 3, 0.25)]));
        compose(&u, id);
        exec(&u.o, &case(id));
        if name.contains("order") || name.contains("redeploy") {
            let tx = seed::insert_stock_tx(&u.o.primary, &u.s, id, u.m[0].0, "AAA", &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some(format!("resting-{id}")), order_type: 1,
                amount: Some("1"), quote_amount: None, price: Some("10"), quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-09-01 11:00:00".into() });
            if name.contains("redeploy") { exec(&u.o, &format!("UPDATE transactions SET transaction_type = 'REDEPLOY' WHERE id = {tx}")); }
        }
        let before = settings(&u, id);
        for op in ops {
            let fields: Vec<(String, &str)> = op.iter().map(|(k, v)| (key(k), *v)).collect();
            let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let refused = patch(&mut u, id, &fields).await;
            assert_eq!(refused.status, 422, "{name} / {op:?}: {}", refused.body);
            let why = if name == "running" { "Stop the bot first." } else { "Wait for the pending swap or sale to finish." };
            assert!(refused.body.contains(why), "{name} / {op:?}: {}", refused.body);
        }
        assert_eq!(settings(&u, id), before, "{name}");
    }
}

#[tokio::test]
async fn change_index_keeps_the_weight_and_forgets_only_that_groups_rows() {
    let mut u = universe();
    cat_b(&u);
    let id = grouped_bot(&u, btc(&u, 0.4), json!([group("nasdaq-100", 3, 0.35), group("top-coins-x", 3, 0.25)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups[1]', json('{}')) WHERE id = {id}", json!({ "index_type": "top", "num_coins": 3, "allocation_flattening": 0.0, "weight": 0.25 })));
    group_row(&u, id, "nasdaq-100", u.m[0], 1.0);
    group_row(&u, id, "top-coins", (u.s.btc, u.s.ticker_id), 1.0);
    let before = generation(&u, id);
    let saved = patch(&mut u, id, &[(&key("change_index_id"), "nasdaq-100"), (&key("change_index_to"), "cat-b")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!(composition::external_id(s["index_groups"][0].as_object()), "cat-b", "{s}");
    assert_eq!(weight(&s, "cat-b"), 0.35);
    assert_eq!(group_rows(&u, id), vec![format!("top-coins:{}", u.s.btc)]);
    assert_eq!(generation(&u, id), before + 1);
    assert!(transient(&u, id).get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()));
}

#[tokio::test]
async fn lone_a_to_lone_b_through_the_index_route() {
    let mut u = universe();
    cat_b(&u);
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200);
    let answer = u.b.send(&u.app, "POST", &format!("/bots/{id}/index"), Some(&[("index_category_id", "cat-b")]), Csrf::Header, &TURBO).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    let s = settings(&u, id);
    assert_eq!(s["index_groups"].as_array().unwrap().len(), 1, "{s}");
    assert_eq!(composition::external_id(s["index_groups"][0].as_object()), "cat-b");
    assert_eq!(weight(&s, "cat-b"), 1.0);
    assert_eq!(model::load_bot(&u.o.primary, id).unwrap().bot_type, "Bots::DcaMultiAsset", "no class change");
}

/// A grouped composition record with these shares (as the grouped write leaves it).
fn compose_shares(u: &U, id: i64, shares: Value) {
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let ranked: serde_json::Map<String, Value> = bot.index_external_ids().into_iter()
        .map(|g| { let r = index::ranked_row(&u.o.primary, &g).unwrap().unwrap(); (g, json!([r.id, r.ranked_at])) }).collect();
    composition::record(&u.o.primary, &bot, Value::Object(ranked), shares).unwrap();
}

#[tokio::test]
async fn ungroup_expands_only_that_groups_contribution() {
    let mut u = universe();
    cat_b(&u);
    let id = grouped_bot(&u, btc(&u, 0.2), json!([group("nasdaq-100", 3, 0.4), group("cat-b", 3, 0.4)]));
    group_row(&u, id, "nasdaq-100", u.m[0], 0.75);
    group_row(&u, id, "nasdaq-100", u.m[1], 0.25);
    group_row(&u, id, "cat-b", u.m[0], 0.5);
    group_row(&u, id, "cat-b", u.m[2], 0.5);
    compose(&u, id);
    let saved = patch(&mut u, id, &[(&key("ungroup_index_id"), "nasdaq-100")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    let a = |asset: i64| s["allocations"][asset.to_string()].as_f64().unwrap();
    assert!((a(u.m[0].0) - 0.3).abs() < 1e-12 && (a(u.m[1].0) - 0.1).abs() < 1e-12 && (a(u.s.btc) - 0.2).abs() < 1e-12, "{s}");
    assert_eq!(s["index_groups"].as_array().unwrap().len(), 1);
    assert_eq!(weight(&s, "cat-b"), 0.4, "the overlapping group is untouched");
    assert_eq!(group_rows(&u, id), vec![format!("cat-b:{}", u.m[0].0), format!("cat-b:{}", u.m[2].0)]);
}

#[tokio::test]
async fn custom_allocation_of_two_overlapping_groups_under_market_cap_keeps_the_exact_shares() {
    let mut u = universe();
    cat_b(&u);
    let id = grouped_bot(&u, btc(&u, 0.2), json!([group("nasdaq-100", 3, 0.4), group("cat-b", 3, 0.4)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = {id}"));
    group_row(&u, id, "nasdaq-100", u.m[0], 0.9996);
    group_row(&u, id, "nasdaq-100", u.m[1], 0.0004);
    group_row(&u, id, "cat-b", u.m[0], 0.5);
    group_row(&u, id, "cat-b", u.m[2], 0.5);
    compose_shares(&u, id, json!({ "assets": { u.s.btc.to_string(): 0.1 }, "groups": { "nasdaq-100": 0.6, "cat-b": 0.3 } }));
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200);
    let answer = u.b.send(&u.app, "POST", &format!("/bots/{id}/custom_allocation"), Some(&[]), Csrf::Header, &TURBO).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    let s = settings(&u, id);
    assert_eq!(s["weighting"], json!("manual"));
    assert!(s.get("index_groups").is_none(), "{s}");
    let got = targets(&u, id);
    let t = |asset: i64| got.iter().find(|(a, _)| *a == asset).map(|(_, w)| *w).unwrap();
    assert!((t(u.m[1].0) - 0.6 * 0.0004).abs() < 1e-9, "0.04 % of the index stays: {got:?}");
    assert!((t(u.m[0].0) - (0.6 * 0.9996 + 0.15)).abs() < 1e-9 && (t(u.m[2].0) - 0.15).abs() < 1e-9 && (t(u.s.btc) - 0.1).abs() < 1e-9, "{got:?}");
}

#[tokio::test]
async fn ungroup_refuses_beside_an_edit_or_while_not_current_and_writes_nothing() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    group_row(&u, id, "nasdaq-100", u.m[0], 1.0);
    let before = settings(&u, id);
    assert_eq!(patch(&mut u, id, &[(&key("ungroup_index_id"), "nasdaq-100")]).await.status, 422, "not composed");
    compose(&u, id);
    let k = format!("{ROOT}[allocations][{}]", u.s.btc);
    assert_eq!(patch(&mut u, id, &[(&k, "40"), (&key("ungroup_index_id"), "nasdaq-100")]).await.status, 422, "beside an edit");
    assert_eq!(patch(&mut u, id, &[(&key("index_groups][nasdaq-100][weight"), "40"), (&key("ungroup_index_id"), "*")]).await.status, 422);
    assert_eq!(settings(&u, id), before);
    assert_eq!(group_rows(&u, id).len(), 1);
}

#[tokio::test]
async fn ungroup_counts_assets_and_surviving_groups_against_max_assets() {
    let mut u = universe();
    cat_b(&u);
    let stocks: Vec<(i64, i64)> = (0..97).map(|i| seed::add_alpaca_stock(&u.o.primary, &u.s, &format!("N{i:02}"))).collect();
    let weights = |n: usize| -> Value { Value::Object(stocks[..n].iter().map(|(a, _)| (a.to_string(), json!(0.5 / n as f64))).collect()) };
    // 97 assets + AAA and BBB from the index + cat-b surviving = 100: allowed, and the result still opens and saves.
    let id = grouped_bot(&u, weights(97), json!([group("nasdaq-100", 3, 0.25), group("cat-b", 3, 0.25)]));
    group_row(&u, id, "nasdaq-100", u.m[0], 0.5);
    group_row(&u, id, "nasdaq-100", u.m[1], 0.5);
    compose(&u, id);
    let saved = patch(&mut u, id, &[(&key("ungroup_index_id"), "nasdaq-100")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(patch(&mut u, id, &[(&key("label"), "Wide")]).await.status, 200, "a corrective save after the largest ungroup");
    // One more named asset: 101 ingredients, refused.
    let more = seed::add_alpaca_stock(&u.o.primary, &u.s, "NXX");
    let mut w = weights(97); w[more.0.to_string()] = json!(0.0);
    let id = grouped_bot(&u, w, json!([group("nasdaq-100", 3, 0.25), group("cat-b", 3, 0.25)]));
    group_row(&u, id, "nasdaq-100", u.m[0], 0.5);
    group_row(&u, id, "nasdaq-100", u.m[1], 0.5);
    compose(&u, id);
    let refused = patch(&mut u, id, &[(&key("ungroup_index_id"), "nasdaq-100")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("up to 100 assets"), "{}", refused.body);
}

#[tokio::test]
async fn an_index_form_with_an_op_key_is_422_and_unchanged() {
    let mut u = universe();
    let id = seed::index_bot(&u.o.primary, &u.s, "nasdaq-100", 3, 0.0, false);
    exec(&u.o, &format!("UPDATE bots SET status = 2 WHERE id = {id}"));
    let before = settings(&u, id);
    let refused = patch(&mut u, id, &[("bots_dca_index[label]", "Renamed"), ("bots_dca_index[add_index]", "top-coins")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(settings(&u, id), before);
    let label: Option<String> = u.o.primary.query_row("SELECT label FROM bots WHERE id = ?1", [id], |r| r.get(0)).unwrap();
    assert_ne!(label.as_deref(), Some("Renamed"));
}

#[tokio::test]
async fn add_index_appends_an_offered_index_at_zero() {
    let mut u = universe();
    cat_b(&u);
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = {id}"));
    for unoffered in ["nasdaq-100", "no-such-index"] {
        assert_eq!(patch(&mut u, id, &[(&key("add_index"), unoffered)]).await.status, 422, "{unoffered}");
    }
    let saved = patch(&mut u, id, &[(&key("add_index"), "cat-b")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!(weight(&s, "cat-b"), 0.0);
    assert_eq!(s["index_groups"][1]["num_coins"], json!(3), "a bounded index held whole: {s}");
    assert_eq!(s["weighting"], json!("market_cap"), "a market-cap bot gaining an index stays market cap");
}

// Codex slice 3 branch r1.

/// What a bot's rules would act on, engine-side: (rule, selling side, action) of every rule that reads as on.
fn armed(u: &U, id: i64) -> Vec<String> {
    use deltabadger::engine::conditions::{read_rule, Rule};
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let mut on = vec![];
    for rule in [Rule::Price, Rule::PriceDrop, Rule::MovingAverage, Rule::Indicator] {
        for selling in [false, true] {
            if let Ok(Some(r)) = read_rule(&bot, rule, selling) { on.push(format!("{rule:?}/{selling}/{}", r.common.action)); }
        }
    }
    on
}

#[tokio::test]
async fn losing_the_last_group_switches_the_sell_side_off_and_says_so() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 1.0), json!([group("nasdaq-100", 3, 0.0)]));
    member(&u, id, (u.s.btc, u.s.ticker_id), 1.0, true);
    // A stopped buyer armed to start selling, with a sell-side rule and a stored base sale and cap.
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.price_drop_limited', json('true'), '$.price_drop_limit_action', 'start_selling', '$.price_drop_limit', 0.1, \
                         '$.sell_price_limited', json('true'), '$.sell_price_limit', 70000, '$.sell_price_limit_action', 'pause', \
                         '$.sell_denomination', 'base', '$.sell_amount', 0.5, '$.sell_quote_amount', 100, '$.base_amount_limited', json('true'), '$.base_amount_limit', 2), \
                         transient_data = json_set(transient_data, '$.price_drop_limit_condition_met_at', '2026-09-01T00:00:00Z') WHERE id = {id}"));
    assert!(!armed(&u, id).is_empty());
    let saved = patch(&mut u, id, &[(&key("remove_index_id"), "nasdaq-100")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert!(saved.body.contains("Selling rules were switched off"), "the removal is named: {}", saved.body);
    assert!(armed(&u, id).is_empty(), "no rule left to flip or sell: {:?}", armed(&u, id));
    let (s, t) = (settings(&u, id), transient(&u, id));
    for k in ["sell_amount", "sell_quote_amount", "smart_interval_base_amount", "base_amount_limit"] { assert!(s.get(k).is_none_or(Value::is_null), "{k}: {s}"); }
    assert_eq!((s["base_amount_limited"].clone(), s["sell_denomination"].clone()), (json!(false), json!("quote")), "{s}");
    assert!(t.get("price_drop_limit_condition_met_at").is_none_or(Value::is_null), "the latch goes: {t}");
    // Reversed into selling its one asset, nothing sizes from the old base amount.
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200);
    let reversed = u.b.send(&u.app, "POST", &format!("/bots/{id}/reverse"), Some(&[]), Csrf::Header, &TURBO).await;
    assert!(reversed.status < 400, "{}", reversed.body);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    assert!(bot.selling());
    let (base, quote) = bot.sell_amounts();
    assert!(base.as_ref().is_none_or(|b| !b.is_positive()) && !quote.is_positive(), "{base:?} {quote:?}");
}

#[tokio::test]
async fn a_venue_move_keeps_the_subject_the_save_validated() {
    let mut u = universe();
    // Another venue listing BTC and AAA at the same quote, with a key: the bot moves there.
    exec(&u.o, "INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '0.1', '0.1', '2026-01-01', '2026-01-01')");
    let other = u.o.primary.last_insert_rowid();
    let copy = |ticker: i64| { exec(&u.o, &format!("CREATE TEMP TABLE t AS SELECT * FROM tickers WHERE id = {ticker}; UPDATE t SET id = NULL, exchange_id = {other}; INSERT INTO tickers SELECT * FROM t; DROP TABLE t;")); u.o.primary.last_insert_rowid() };
    let (btc_there, aaa_there) = (copy(u.s.ticker_id), copy(u.m[0].1));
    exec(&u.o, &format!("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) SELECT user_id, {other}, key, secret, status, key_type, created_at, updated_at FROM api_keys WHERE id = {}", u.s.api_key_id));
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    // The rule watched AAA, a member that has left; the save posts BTC there.
    member(&u, id, u.m[0], 0.0, false);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit_in_ticker_id', {}) WHERE id = {id}", u.m[0].1));
    let saved = patch(&mut u, id, &[(&key("exchange_id"), &other.to_string()), (&key("price_limit_in_ticker_id"), &btc_there.to_string())]).await;
    if saved.status == 200 {
        assert_eq!(settings(&u, id)["price_limit_in_ticker_id"], json!(btc_there), "never remapped onto {aaa_there}");
    } else {
        assert_eq!(settings(&u, id)["price_limit_in_ticker_id"], json!(u.m[0].1), "refused whole: {}", saved.body);
    }
}

#[tokio::test]
async fn add_index_judges_the_engines_ranked_row() {
    let mut u = universe();
    cat_b(&u);
    // The engine composes cat-b from a later row naming nothing that trades here.
    exec(&u.o, "INSERT INTO indices (external_id, source, name, top_coins, top_coins_by_exchange, available_exchanges, weights, weight, created_at, updated_at) \
                VALUES ('cat-b', 'coingecko', 'B', '[\"Z1\",\"Z2\",\"Z3\"]', '{}', '{}', '{}', 0, '2026-01-01', '2026-01-01')");
    let newer = u.o.primary.last_insert_rowid();
    index::record_ranking(&u.o.primary, newer, "data-api", &deltabadger::engine::provider::fingerprint(&u.o.primary).unwrap().unwrap(), "2099-01-03T00:00:00Z".parse().unwrap()).unwrap();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    assert_eq!(patch(&mut u, id, &[(&key("add_index"), "cat-b")]).await.status, 422);
    assert_eq!(settings(&u, id)["index_groups"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn a_group_count_above_1000_is_refused_whatever_hold_all() {
    let mut u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    let refused = patch(&mut u, id, &[(&key("index_groups][nasdaq-100][num_coins"), "1001")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(settings(&u, id)["index_groups"][0]["num_coins"], json!(3));
}

#[tokio::test]
async fn an_operation_whose_stored_row_would_not_reopen_is_refused() {
    let mut u = universe();
    cat_b(&u);
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    // Escapes kept as stored: the column is 6 bytes a character, near Draft::load's 65 536-byte bound.
    let stored: usize = u.o.primary.query_row(&format!("SELECT length(CAST(settings AS BLOB)) FROM bots WHERE id = {id}"), [], |r| r.get(0)).unwrap();
    let pad = "\\u00e9".repeat((65_536 - stored - 40) / 6);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.note', json('\"{pad}\"')) WHERE id = {id}"));
    let before: String = u.o.primary.query_row(&format!("SELECT settings FROM bots WHERE id = {id}"), [], |r| r.get(0)).unwrap();
    assert!(before.len() <= 65_536 && before.contains("\\u00e9"));
    let refused = patch(&mut u, id, &[(&key("add_index"), "cat-b")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    let after: String = u.o.primary.query_row(&format!("SELECT settings FROM bots WHERE id = {id}"), [], |r| r.get(0)).unwrap();
    assert_eq!(after, before, "rolled back");
}

#[tokio::test]
async fn a_malformed_operation_is_a_422_and_writes_nothing() {
    let mut u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let plain = seed::insert_bot(&u.o.primary, &u.s, &spec);
    for fields in [vec![(key("add_index][]"), "nasdaq-100"), (key("quote_amount"), "70")], vec![(key("add_index"), ""), (key("quote_amount"), "70")],
                   vec![(key("index_groups][]"), "x"), (key("quote_amount"), "70")]] {
        let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let refused = patch(&mut u, plain, &fields).await;
        assert_eq!(refused.status, 422, "{fields:?}: {}", refused.body);
        assert_eq!(settings(&u, plain)["quote_amount"], json!(60.0), "{fields:?}");
    }
    // The grouped Follow route without a category.
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    let before = settings(&u, id);
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200);
    let refused = u.b.send(&u.app, "POST", &format!("/bots/{id}/index"), Some(&[]), Csrf::Header, &TURBO).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(settings(&u, id), before);
}

// Codex slice 3 check r2.

#[tokio::test]
async fn a_group_only_venue_move_with_no_member_there_drops_the_subject() {
    let mut u = universe();
    exec(&u.o, "INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', '0.1', '0.1', '2026-01-01', '2026-01-01')");
    let other = u.o.primary.last_insert_rowid();
    exec(&u.o, &format!("CREATE TEMP TABLE t AS SELECT * FROM tickers WHERE id = {}; UPDATE t SET id = NULL, exchange_id = {other}; INSERT INTO tickers SELECT * FROM t; DROP TABLE t;", u.s.ticker_id));
    exec(&u.o, &format!("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) SELECT user_id, {other}, key, secret, status, key_type, created_at, updated_at FROM api_keys WHERE id = {}", u.s.api_key_id));
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    member(&u, id, u.m[0], 1.0, true);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit_in_ticker_id', {}) WHERE id = {id}", u.m[0].1));
    let saved = patch(&mut u, id, &[(&key("exchange_id"), &other.to_string())]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert!(s.get("price_limit_in_ticker_id").is_none_or(Value::is_null), "the old venue's ticker is not kept: {s}");
}

#[tokio::test]
async fn an_index_of_stablecoins_is_not_offered() {
    let mut u = universe();
    let coins: Vec<(i64, i64)> = ["USDX", "USDY", "USDZ"].iter().map(|s| seed::add_alpaca_stock(&u.o.primary, &u.s, s)).collect();
    for (asset, _) in &coins { exec(&u.o, &format!("UPDATE assets SET category = 'Stablecoin' WHERE id = {asset}")); }
    seed::insert_index(&u.o.primary, "stable", &["USDX.US", "USDY.US", "USDZ.US"], &json!({}));
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    let refused = patch(&mut u, id, &[(&key("add_index"), "stable")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
}

#[tokio::test]
async fn a_field_edit_whose_resync_marker_would_not_reopen_is_refused() {
    let mut u = universe();
    let id = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    // A transient document at Draft::load's leaf bound: the resync marker would take it past.
    let leaves: i64 = u.o.primary.query_row(&format!("SELECT count(*) FROM bots, json_tree(bots.transient_data) WHERE bots.id = {id} AND json_tree.type NOT IN ('object', 'array')"), [], |r| r.get(0)).unwrap();
    let pad: serde_json::Map<String, Value> = (0..(1000 - leaves - 2)).map(|i| (format!("k{i}"), json!(i))).collect();
    exec(&u.o, &format!("UPDATE bots SET transient_data = json_set(transient_data, '$.pad', json('{}')) WHERE id = {id}", Value::Object(pad)));
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200, "loads at the bound");
    let before = (settings(&u, id), transient(&u, id));
    let refused = patch(&mut u, id, &[(&key("index_groups][nasdaq-100][num_coins"), "4")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!((settings(&u, id), transient(&u, id)), before, "rolled back");
}

#[test]
fn clearing_a_lone_stored_threshold_counts_as_a_reset() {
    let mut s = json!({ "sell_price_limited": false, "sell_price_limit": 70000 }).as_object().unwrap().clone();
    let mut t = serde_json::Map::new();
    assert!(composition::reset_sell_side(&mut s, &mut t));
    assert!(!s.contains_key("sell_price_limit"));
}

// reset_sell_side discards only real flips and sell rules.

#[tokio::test]
async fn a_blank_action_buy_gate_still_holds_after_the_last_group_goes() {
    use deltabadger::engine::conditions::{read_rule, Rule};
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 1.0), json!([group("nasdaq-100", 3, 0.0)]));
    member(&u, id, (u.s.btc, u.s.ticker_id), 1.0, true);
    // Buy only below 50 000, no action stored (the engine reads pause; a blank string, which the page does not serve, is
    // pinned on the function below), the gate latched.
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit', 50000, '$.price_limit_value_condition', 'below', \
                         '$.price_limit_in_ticker_id', {}), transient_data = json_set(transient_data, '$.price_limit_condition_met_at', '2026-09-01T00:00:00Z') WHERE id = {id}", u.s.ticker_id));
    let saved = patch(&mut u, id, &[(&key("remove_index_id"), "nasdaq-100")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let gate = read_rule(&bot, Rule::Price, false).unwrap().expect("the buy gate is still on");
    assert_eq!(gate.common.action, "pause");
    assert_eq!(bot.settings["price_limit"], json!(50000));
    assert!(transient(&u, id).get("price_limit_condition_met_at").is_some_and(|v| !v.is_null()), "its latch kept");
}

#[test]
fn reset_sell_side_keeps_every_buy_rule_but_a_real_flip() {
    let mut s = json!({ "price_limited": true, "price_limit": 50000, "price_limit_action": " ",
                        "price_drop_limited": true, "price_drop_limit": 0.1, "price_drop_limit_action": "stop",
                        "indicator_limited": true, "indicator_limit": 30, "indicator_limit_action": "start_selling", "indicator_limit_in_ticker_id": 7,
                        "indicator_limit_timing_condition": "while", "indicator_limit_value_condition": "below", "indicator_limit_in_indicator": "rsi", "indicator_limit_in_timeframe": "one_day" })
        .as_object().unwrap().clone();
    let mut t = json!({ "price_limit_condition_met_at": "x", "price_drop_limit_condition_met_at": "x", "indicator_limit_condition_met_at": "x" }).as_object().unwrap().clone();
    assert!(composition::reset_sell_side(&mut s, &mut t));
    // A blank action is pause, an unsupported one stays for eligibility to refuse: both untouched, latches included.
    for (k, v) in [("price_limited", json!(true)), ("price_limit_action", json!(" ")), ("price_drop_limited", json!(true)), ("price_drop_limit_action", json!("stop"))] { assert_eq!(s[k], v, "{k}"); }
    assert!(t.contains_key("price_limit_condition_met_at") && t.contains_key("price_drop_limit_condition_met_at"));
    // The flip is discarded whole: its switch off and nothing else of its family left.
    assert_eq!(s["indicator_limited"], json!(false));
    assert!(s.keys().all(|k| k == "indicator_limited" || !k.starts_with("indicator_limit")), "{s:?}");
    assert!(!t.contains_key("indicator_limit_condition_met_at"));
}

#[test]
fn a_discarded_sell_rule_leaves_no_field_of_its_family() {
    let mut s = json!({ "sell_price_limited": true, "sell_price_limit": 70000, "sell_price_limit_range_lower_bound": 1, "sell_price_limit_range_upper_bound": 2,
                        "sell_price_limit_timing_condition": "while", "sell_price_limit_value_condition": "above", "sell_price_limit_action": "pause", "sell_price_limit_in_ticker_id": 9,
                        "price_limit_in_ticker_id": 5 }).as_object().unwrap().clone();
    let mut t = json!({ "sell_price_limit_condition_met_at": "x", "sell_price_limit_enabled_at": "x" }).as_object().unwrap().clone();
    assert!(composition::reset_sell_side(&mut s, &mut t));
    assert_eq!(s["sell_price_limited"], json!(false));
    assert!(s.keys().all(|k| k == "sell_price_limited" || !k.starts_with("sell_price_limit")), "{s:?}");
    assert_eq!(s["price_limit_in_ticker_id"], json!(5), "the buy side's own field stays");
    assert!(t.is_empty(), "{t:?}");
}

// ---- Slice 4a: a basket stored as one index group (what an index bot converts into) reads and edits as that index bot.

/// A stopped lone group (no assets, one group at 1.0), `extra` settings beside it.
fn lone(u: &U, n: i64, extra: &[(&str, Value)]) -> i64 {
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("allocations", json!({})).with("index_groups", json!([group("nasdaq-100", n, 1.0)]));
    for (key, value) in extra { spec = spec.with(key, value.clone()); }
    spec.status = 2;
    seed::insert_bot(&u.o.primary, &u.s, &spec)
}
fn the_group(u: &U, id: i64) -> Value { settings(u, id)["index_groups"][0].clone() }

#[tokio::test]
async fn a_lone_grouped_row_renders_the_index_view_under_the_basket_root() {
    let mut u = universe();
    let id = lone(&u, 3, &[]);
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    assert_eq!(page.status, 200, "{}", page.body);
    for want in ["data-controller=\"index-allocation\"", "name=\"bots_dca_multi_asset[num_coins]\"", "name=\"bots_dca_multi_asset[num_coins_rendered]\"",
                 "name=\"bots_dca_multi_asset[allocation_flattening]\"", "ND3", "class=\"reverse-toggle\"", "/custom_allocation", "/index/new"] {
        assert!(page.body.contains(want), "{want}");
    }
    assert!(!page.body.contains("bots_dca_index[") && !page.body.contains("bots_dca_multi_asset[allocations]"), "the index view, not a basket's sliders");
    seed::insert_bot(&u.o.primary, &u.s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00")); // two bots: the list is shown
    let list = u.b.get(&u.app, "/bots").await;
    assert!(list.body.contains("ND3"), "the dashboard titles it as its index");
}

#[tokio::test]
async fn a_selling_lone_group_shows_the_sell_sentence_and_reverse() {
    let mut u = universe();
    let id = lone(&u, 3, &[("direction", json!("selling")), ("sell_quote_amount", json!(30.0))]);
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(page.body.contains("name=\"bots_dca_multi_asset[sell_quote_amount]\"") && page.body.contains("class=\"reverse-toggle\""));
    assert!(!page.body.contains("name=\"bots_dca_multi_asset[quote_amount]\""), "the sentence edits the sale, not the buy");
    let saved = patch(&mut u, id, &[("bots_dca_multi_asset[sell_quote_amount]", "35")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(settings(&u, id)["sell_quote_amount"], json!(35.0));
}

#[tokio::test]
async fn flat_num_coins_and_flattening_edit_a_lone_group_on_the_web_and_mcp() {
    let mut u = universe();
    let id = lone(&u, 3, &[]);
    let saved = patch(&mut u, id, &[(&key("num_coins"), "4"), (&key("num_coins_rendered"), "3"), (&key("num_coins_ceiling"), "5"), (&key("allocation_flattening"), "0.5")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let (s, g) = (settings(&u, id), the_group(&u, id));
    assert_eq!((&g["num_coins"], &g["hold_all"], &g["allocation_flattening"], &g["weight"]), (&json!(4), &json!(false), &json!(0.5), &json!(1.0)), "{s}");
    assert!(s.get("num_coins").is_none() && s.get("allocation_flattening").is_none(), "written onto the group only: {s}");
    assert_eq!(patch(&mut u, id, &[(&key("num_coins"), "5"), (&key("num_coins_rendered"), "4"), (&key("num_coins_ceiling"), "5")]).await.status, 200);
    assert_eq!((&the_group(&u, id)["num_coins"], &the_group(&u, id)["hold_all"]), (&json!(5), &json!(true)), "the whole index at the ceiling");
    let answer = mcp(&u, "update_bot_settings", json!({ "bot_id": id, "num_coins": 3, "allocation_flattening": 0.25 }));
    assert!(answer.to_string().contains("settings updated"), "{answer}");
    let g = the_group(&u, id);
    assert_eq!((&g["num_coins"], &g["hold_all"], &g["allocation_flattening"]), (&json!(3), &json!(false), &json!(0.25)), "{g}");
}

#[tokio::test]
async fn an_unchanged_lone_group_repost_keeps_num_coins_and_hold_all() {
    let mut u = universe();
    let id = lone(&u, 3, &[]);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups[0].num_coins', 5, '$.index_groups[0].hold_all', json('true')) WHERE id = {id}"));
    let before = the_group(&u, id);
    // The page drew the count it stored; a ceiling read from another ranking does not recompute hold_all.
    let saved = patch(&mut u, id, &[(&key("num_coins"), "5"), (&key("num_coins_rendered"), "5"), (&key("num_coins_ceiling"), "99"), (&key("quote_amount"), "61")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(the_group(&u, id), before);
}

#[tokio::test]
async fn flat_index_fields_on_a_wider_basket_are_422() {
    let mut u = universe();
    let mixed = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let plain = seed::insert_bot(&u.o.primary, &u.s, &spec);
    for id in [mixed, plain] {
        let before = settings(&u, id);
        let refused = patch(&mut u, id, &[(&key("num_coins"), "4"), (&key("quote_amount"), "70")]).await;
        assert_eq!(refused.status, 422, "{id}: {}", refused.body);
        assert_eq!(settings(&u, id), before);
    }
}

#[tokio::test]
async fn a_departed_subject_survives_an_unchanged_repost() {
    let mut u = universe();
    // BTC/USD is no named asset and no member: a subject that left (the engine still watches it).
    let id = lone(&u, 3, &[("price_limited", json!(true)), ("price_limit", json!(50000.0)), ("price_limit_in_ticker_id", json!(u.s.ticker_id))]);
    // One current member (AAA), so the rule's subject is a choice.
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, created_at, updated_at) VALUES ({id}, {}, {}, 1, 1, '2026-09-01', '2026-09-01')", u.m[0].0, u.m[0].1));
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    assert!(page.body.contains("BTC (not in the index)"), "the stored subject is offered, named");
    let ticker = u.s.ticker_id.to_string();
    let saved = patch(&mut u, id, &[(&key("price_limit_in_ticker_id"), &ticker), (&key("quote_amount"), "61")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(settings(&u, id)["price_limit_in_ticker_id"], json!(u.s.ticker_id));
}

#[tokio::test]
async fn the_picker_excludes_a_lone_groups_index() {
    let mut u = universe();
    cat_b(&u);
    let id = lone(&u, 3, &[]);
    let picker = u.b.get(&u.app, &format!("/bots/{id}/index/new")).await;
    assert_eq!(picker.status, 200, "{}", picker.body);
    assert!(picker.body.contains("cat-b") && !picker.body.contains("nasdaq-100"), "{}", picker.body);
    let before = the_group(&u, id);
    let refused = patch(&mut u, id, &[(&key("follow_index"), "nasdaq-100")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(the_group(&u, id), before, "count and flattening kept");
}

#[tokio::test]
async fn rest_and_mcp_read_a_lone_group_as_its_index() {
    use axum::{body::{to_bytes, Body}, http::Request};
    use tower::ServiceExt;
    let u = universe();
    let id = lone(&u, 3, &[]);
    let details = mcp(&u, "get_bot_details", json!({ "bot_id": id })).to_string();
    assert!(details.contains("Bots::DcaIndex") || details.contains("Dca Index"), "{details}");
    let listed = mcp(&u, "list_bots", json!({})).to_string();
    assert!(listed.contains("Dca Index"), "{listed}");
    let c = &u.o.primary;
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('I4','i4','http://localhost/cb',0,'api','2026-09-01','2026-09-01')", []).unwrap();
    let application = c.last_insert_rowid();
    c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'i4-api','api','2026-09-01 12:00:00',864000)", (application, u.s.user_id)).unwrap();
    let tools = json!(["list_bots"]).to_string();
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,?3,?3,'2026-09-01','2026-09-01')", (u.s.user_id, application, &tools)).unwrap();
    let request = Request::builder().method("GET").uri("/api/v1/bots").header("host", "localhost:3000").header("authorization", "Bearer i4-api")
        .header("accept", "application/json").body(Body::empty()).unwrap();
    let response = deltabadger::web::router(u.app.clone()).oneshot(request).await.unwrap();
    let text = String::from_utf8(to_bytes(response.into_body(), 1_000_000).await.unwrap().to_vec()).unwrap();
    let body: Value = serde_json::from_str(&text).unwrap_or_else(|_| panic!("{text}"));
    let bot = body["data"]["bots"].as_array().or(body["bots"].as_array()).unwrap_or_else(|| panic!("{text}")).iter().find(|b| b["id"] == json!(id)).cloned().unwrap();
    assert_eq!((&bot["type"], &bot["pair"], &bot["base_asset"]), (&json!("Bots::DcaIndex"), &Value::Null, &Value::Null), "{bot}");
}

/// A grouped bot's figures read every tradable listing of its venue in its quote (figures::db::tickers): a new listing there
/// moves the figures' revision, as it does for an index bot.
#[test]
fn a_grouped_bots_figures_revision_moves_with_a_venue_listing() {
    let u = universe();
    // Only the grouped bot follows an index here (the fixture's deleted index bot would move it under slice 3's query too).
    exec(&u.o, "DELETE FROM bots WHERE type = 'Bots::DcaIndex'");
    lone(&u, 3, &[]);
    let revision = || deltabadger::web::figure::loading::revision(&u.o.primary, u.s.user_id, common::web::at(NOW)).unwrap().0;
    let before = revision();
    seed::add_alpaca_stock(&u.o.primary, &u.s, "FFF");
    assert_ne!(revision(), before);
}

// Codex 4a branch r1.

fn seat(u: &U, id: i64, at: usize) {
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, created_at, updated_at) VALUES ({id}, {}, {}, 0.5, 1, '2026-09-01', '2026-09-01')", u.m[at].0, u.m[at].1));
}

/// P1: an enabled rule with no stored subject pauses the engine; an unrelated save never stores a default one.
#[tokio::test]
async fn an_unrelated_save_never_stores_a_default_subject() {
    let mut u = universe();
    let id = lone(&u, 3, &[("price_limited", json!(true)), ("price_limit", json!(50000.0))]);
    seat(&u, id, 0);
    seat(&u, id, 1);
    let saved = patch(&mut u, id, &[(&key("quote_amount"), "61")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!(s["quote_amount"], json!(61.0));
    assert!(s.get("price_limit_in_ticker_id").is_none_or(Value::is_null), "no subject chosen for the owner: {s}");
}

/// P1: string count and flattening (as the group codec reads them) are drawn as they are, and posting what was drawn with a
/// budget edit leaves both exactly as stored.
#[tokio::test]
async fn string_count_and_flattening_draw_and_survive_a_budget_save() {
    let mut u = universe();
    let id = lone(&u, 3, &[]);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups[0].num_coins', '4', '$.index_groups[0].allocation_flattening', '0.5') WHERE id = {id}"));
    let before = the_group(&u, id);
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    let field = |name: &str| page.body.split(&format!("name=\"{ROOT}[{name}]\"")).nth(1).and_then(|rest| rest.split("value=\"").nth(1)).and_then(|v| v.split('"').next()).map(str::to_string);
    assert_eq!((field("num_coins").as_deref(), field("allocation_flattening").as_deref()), (Some("4"), Some("0.5")));
    let saved = patch(&mut u, id, &[(&key("num_coins"), "4"), (&key("num_coins_rendered"), "4"), (&key("num_coins_ceiling"), "5"), (&key("allocation_flattening"), "0.5"), (&key("quote_amount"), "61")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(the_group(&u, id), before);
}

/// P2: a balanced lone group's status bar shows its state, not "normalize first".
#[tokio::test]
async fn a_balanced_lone_groups_status_is_its_own() {
    let mut u = universe();
    let id = lone(&u, 3, &[]);
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    assert!(!page.body.contains("Normalize allocation"), "{}", page.body.split("status").nth(1).unwrap_or_default().chars().take(300).collect::<String>());
    let mixed = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.4)]));
    assert!(u.b.get(&u.app, &format!("/bots/{mixed}")).await.body.contains("Normalize allocation"), "an unbalanced set still says so");
}

/// P2: the moving-average and indicator rules offer only what the writer takes: the subject pool, and the stored subject
/// named as departed.
#[tokio::test]
async fn trend_rule_subjects_are_the_pool_and_the_departed_stored_one() {
    let mut u = universe();
    let id = lone(&u, 3, &[("moving_average_limit_in_ticker_id", json!(u.s.ticker_id))]);
    seat(&u, id, 0);
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    let select = page.body.split(&format!("name=\"{ROOT}[moving_average_limit_in_ticker_id]\"")).nth(1).and_then(|r| r.split("</select>").next()).unwrap_or_default().to_string();
    assert!(select.contains("(not in the index)") && select.contains(&format!("value=\"{}\"", u.m[0].1)), "{select}");
    assert!(!select.contains(&format!("value=\"{}\"", u.m[1].1)), "a pair the bot does not hold is not offered: {select}");
}

/// P2: a ranking naming AAA twice previews AAA once, as the grouped engine holds it (D-INDEX-DISTINCT-SEATS).
#[tokio::test]
async fn a_lone_group_previews_distinct_members() {
    let mut u = universe();
    exec(&u.o, "UPDATE indices SET top_coins = '[\"AAA.US\",\"AAA.US\",\"BBB.US\",\"CCC.US\"]' WHERE external_id = 'nasdaq-100'");
    let id = lone(&u, 2, &[]);
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    assert_eq!(page.body.matches(&format!("data-asset-id=\"{}\" data-index=", u.m[0].0)).count(), 1, "AAA once");
}

/// P2: Custom allocation posted with a slider edit is a conflicting composition edit.
#[tokio::test]
async fn ungroup_with_a_slider_edit_is_refused() {
    let mut u = universe();
    let id = lone(&u, 3, &[]);
    group_row(&u, id, "nasdaq-100", u.m[0], 1.0);
    compose(&u, id);
    for (name, value) in [("num_coins", "4"), ("allocation_flattening", "0.5")] {
        let before = settings(&u, id);
        let refused = patch(&mut u, id, &[(&key("ungroup_index_id"), "*"), (&key(name), value)]).await;
        assert_eq!(refused.status, 422, "{name}: {}", refused.body);
        assert_eq!(settings(&u, id), before);
        assert!(refused.body.contains("changed in the meantime"), "refused as a conflicting edit: {}", refused.body);
    }
    let alone = patch(&mut u, id, &[(&key("ungroup_index_id"), "*")]).await;
    assert_eq!(alone.status, 200, "{}", alone.body);
}

/// P2: the current index is left out of the picker by its external id, whichever provider's row the tile is.
#[tokio::test]
async fn the_picker_excludes_the_current_index_of_any_provider() {
    let mut u = universe();
    cat_b(&u);
    exec(&u.o, "INSERT INTO indices (external_id, source, name, top_coins, top_coins_by_exchange, available_exchanges, weights, weight, created_at, updated_at) \
                SELECT external_id, 'coingecko', name, top_coins, top_coins_by_exchange, available_exchanges, weights, weight, created_at, updated_at FROM indices WHERE external_id = 'nasdaq-100'");
    let id = lone(&u, 3, &[]);
    let picker = u.b.get(&u.app, &format!("/bots/{id}/index/new")).await;
    assert!(picker.body.contains("cat-b") && !picker.body.contains("nasdaq-100"), "{}", picker.body);
}

// ---------------------------------------------------------------- slice 4b: a converted index bot on the web

/// The settings form as the page renders it: every `bots_dca_multi_asset[…]` input with its value (a checkbox or radio only
/// when checked) and each select's selected option, so a post of it is the unchanged form.
fn rendered_form(page: &str) -> Vec<(String, String)> {
    let attr = |tag: &str, name: &str| tag.split(&format!(" {name}=\"")).nth(1).and_then(|r| r.split('"').next()).map(|v| v.replace("&quot;", "\"").replace("&amp;", "&"));
    let mut out = vec![];
    for tag in page.split("<input").skip(1).map(|t| t.split('>').next().unwrap()) {
        let Some(name) = attr(tag, "name").filter(|n| n.starts_with("bots_dca_multi_asset[")) else { continue };
        if matches!(attr(tag, "type").as_deref(), Some("checkbox" | "radio")) && !tag.contains(" checked") { continue; }
        out.push((name, attr(tag, "value").unwrap_or_default()));
    }
    for select in page.split("<select").skip(1) {
        let (open, rest) = select.split_once('>').unwrap();
        let Some(name) = attr(open, "name").filter(|n| n.starts_with("bots_dca_multi_asset[")) else { continue };
        let body = rest.split("</select>").next().unwrap();
        if let Some(option) = body.split("<option").skip(1).map(|o| o.split('>').next().unwrap()).find(|o| o.contains(" selected")) {
            out.push((name, attr(option, "value").unwrap_or_default()));
        }
    }
    out
}

/// PLAN slice 4 §4b step 2: a 2.x index row with a stored `start_selling` price rule, converted (the rule's family removed,
/// its switch off): its page renders the basket's reverse toggle and the flip actions, and saving the form as rendered is
/// a 200 that leaves the rule off.
#[tokio::test]
async fn a_converted_start_selling_price_rule_opens_and_saves_unchanged_off() {
    let mut u = universe();
    let id = seed::index_bot(&u.o.primary, &u.s, "nasdaq-100", 3, 0.0, false);
    exec(&u.o, &format!("UPDATE bots SET status = 2, settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit', 50000, '$.price_limit_value_condition', 'below',
        '$.price_limit_action', 'start_selling', '$.price_limit_in_ticker_id', {}) WHERE id = {id}", u.m[0].1));
    exec(&u.o, &format!("INSERT INTO bot_index_assets (bot_id, asset_id, ticker_id, target_allocation, in_index, created_at, updated_at) VALUES ({id}, {}, {}, 1, 1, '2026-09-01', '2026-09-01')", u.m[0].0, u.m[0].1));
    let tx = model::immediate(&u.o.primary).unwrap();
    let (converted, _) = deltabadger::engine::index_bot::convert_all(&tx, common::web::at(T0)).unwrap();
    tx.commit().unwrap();
    assert_eq!(converted.iter().map(|c| c.id).collect::<Vec<_>>(), vec![id]);
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    assert_eq!(page.status, 200, "{}", page.body);
    assert!(page.body.contains("class=\"reverse-toggle") && page.body.contains("value=\"flip\""), "the basket's reverse and flips");
    let form = rendered_form(&page.body);
    assert!(form.iter().any(|(k, _)| k == "bots_dca_multi_asset[quote_amount]"), "{form:?}");
    let fields: Vec<(&str, &str)> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let saved = u.b.send(&u.app, "PATCH", &format!("/bots/{id}"), Some(&fields), Csrf::Header, &TURBO).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!(s["price_limited"], json!(false), "{s}");
    assert_ne!(s.get("price_limit_action"), Some(&json!("start_selling")), "{s}");
}

// ---------------------------------------------------------------- slice 4c: a first group on the web form

/// A plain basket gains its first group only as a clean buyer, judged on the stored row and on the final candidate (a
/// combined post can't slip past): a seller, an armed flip or sell-side rule, a base cap or a base sale is refused whole,
/// nothing cleared; a clean buyer adds the index.
#[tokio::test]
async fn a_first_group_needs_a_clean_buyer() {
    let mut u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let refused_with = |extra: &str| format!("UPDATE bots SET settings = json_set(settings, {extra}) WHERE id = ");
    let cases = [
        ("a quote seller", "'$.direction', 'selling', '$.sell_denomination', 'quote', '$.sell_quote_amount', 30", vec![]),
        ("an armed start_selling buyer with a base cap", "'$.price_limited', json('true'), '$.price_limit', 50000, '$.price_limit_value_condition', 'below', \
          '$.price_limit_action', 'start_selling', '$.sell_denomination', 'quote', '$.sell_quote_amount', 30, '$.base_amount_limited', json('true'), '$.base_amount_limit', 0.5", vec![]),
        ("a buyer with a sell-side rule on", "'$.sell_price_limited', json('true'), '$.sell_price_limit', 90000", vec![]),
        ("a base sale", "'$.sell_denomination', 'base', '$.sell_amount', 0.5", vec![]),
        ("a stored base cap cleared in the same post", "'$.base_amount_limited', json('true'), '$.base_amount_limit', 0.5", vec![(key("base_amount_limited"), "0".to_string())]),
    ];
    let message = deltabadger::web::i18n::text("en", "errors.bots.index_switch.sell_off_first", &[]);
    for (case, sql, extra) in cases {
        let id = seed::insert_bot(&u.o.primary, &u.s, &spec);
        exec(&u.o, &format!("{}{id}", refused_with(sql)));
        let before = settings(&u, id);
        let add = key("add_index");
        let mut fields: Vec<(&str, &str)> = vec![(add.as_str(), "nasdaq-100")];
        fields.extend(extra.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        let answer = patch(&mut u, id, &fields).await;
        assert_eq!(answer.status, 422, "{case}: {}", answer.body);
        assert!(answer.body.contains(&message), "{case}: {}", answer.body);
        assert_eq!(settings(&u, id), before, "{case}: nothing written");
    }
    let clean = seed::insert_bot(&u.o.primary, &u.s, &spec);
    assert_eq!(patch(&mut u, clean, &[(&key("add_index"), "nasdaq-100")]).await.status, 200);
    assert!(settings(&u, clean)["index_groups"].is_array());
}

/// Change the index on a stopped, selling lone group keeps it selling.
#[tokio::test]
async fn change_the_index_while_stopped_and_selling_keeps_it_selling() {
    let mut u = universe();
    cat_b(&u);
    let id = lone(&u, 3, &[("direction", json!("selling")), ("sell_denomination", json!("quote")), ("sell_quote_amount", json!(30.0))]);
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200);
    let answer = u.b.send(&u.app, "POST", &format!("/bots/{id}/index"), Some(&[("index_category_id", "cat-b")]), Csrf::Header, &TURBO).await;
    assert!(answer.status < 400, "{}: {}", answer.status, answer.body);
    let s = settings(&u, id);
    assert_eq!((s["direction"].as_str(), composition::external_id(s["index_groups"][0].as_object())), (Some("selling"), "cat-b"), "{s}");
}

/// A basket's `in_index` rows are its named assets, not members of the index it adds: the first group starts with no
/// `rust_bot_group_members` rows, so its first refresh probes every member as a newcomer, a named asset the ranking names
/// included (it gets no incumbent seat).
#[tokio::test]
async fn a_first_group_seats_none_of_the_baskets_named_assets() {
    use deltabadger::venue::{alpaca::{AlpacaVenue, Urls}, http::ScriptedTransport};
    let mut u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("allocations", json!({ u.m[0].0.to_string(): 0.6, u.m[1].0.to_string(): 0.4 }));
    spec.status = 2;
    let id = seed::insert_bot(&u.o.primary, &u.s, &spec);
    for (i, w) in [0.6, 0.4].into_iter().enumerate() { seed::insert_member(&u.o.primary, id, u.m[i].0, u.m[i].1, w, true, "2026-08-01 00:00:00", None); }
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}")).await.status, 200);
    let answer = patch(&mut u, id, &[(&key("add_index"), "nasdaq-100")]).await;
    assert_eq!(answer.status, 200, "{}", answer.body);
    let rows: Vec<(i64, i64, f64)> = u.o.primary.prepare("SELECT asset_id, composed, weight FROM rust_bot_group_members WHERE bot_id = ?1 ORDER BY asset_id").unwrap()
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(Result::unwrap).collect();
    assert_eq!(rows, vec![], "no seat for a named asset");
    let ok = |body: Value| json!([{ "status": 200, "body": body }]);
    let mut m = json!({ "GET /v2/clock": ok(json!({ "timestamp": "2026-09-01T10:00:00.5-04:00", "is_open": true, "next_open": "2099-01-02T09:30:00-05:00", "next_close": "2099-01-01T16:00:00-05:00" })) });
    for sym in SYMBOLS { m[format!("GET /v2/stocks/{sym}/quotes/latest")] = ok(json!({ "quote": { "ap": 100, "bp": 100 } })); }
    let t = ScriptedTransport::from_script(&m);
    exec(&u.o, &format!("UPDATE bots SET status = 1 WHERE id = {id}")); // a derivation as a run makes it
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let venue = AlpacaVenue::new(t.clone(), Urls::for_passphrase(Some("paper")));
    index::refresh_composition(&u.o.primary, &venue, &bot, &deltabadger::engine::FixedClock(common::web::at(T0)), &deltabadger::engine::tick::PriceCache::default()).await.unwrap().unwrap();
    let probed: Vec<String> = t.requests().iter().filter(|r| r.path.contains("/quotes/")).map(|r| r.path.clone()).collect();
    assert!(probed.iter().any(|p| p.contains("/AAA/")) && probed.iter().any(|p| p.contains("/BBB/")), "the named assets are probed as newcomers: {probed:?}");
}

// PLAN index-as-ingredient slice 7: REST and MCP take the ingredient operations, through the form's own step.

/// A personal REST token of the owner, every tool granted.
fn rest_token(u: &U) {
    let c = &u.o.primary;
    let names = json!(deltabadger::web::mcp::tools::NAMES.to_vec()).to_string();
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('I7','i7','http://localhost/cb',0,'api','2026-09-01','2026-09-01')", []).unwrap();
    let application = c.last_insert_rowid();
    c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'i7-api','api','2026-09-01 12:00:00',864000)", (application, u.s.user_id)).unwrap();
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,?3,?3,'2026-09-01','2026-09-01')", (u.s.user_id, application, &names)).unwrap();
    let on: serde_json::Map<String, Value> = deltabadger::web::mcp::tools::NAMES.iter().map(|t| (t.to_string(), json!(true))).collect();
    c.execute("UPDATE users SET rest_settings = ?1", [json!({"tool_permissions": on}).to_string()]).unwrap();
}
async fn rest(u: &U, method: &str, uri: &str, body: Value) -> (u16, Value) {
    use axum::{body::{to_bytes, Body}, http::Request};
    use tower::ServiceExt;
    let request = Request::builder().method(method).uri(format!("/api/v1{uri}")).header("host", "localhost:3000")
        .header("authorization", "Bearer i7-api").header("content-type", "application/json").header("accept", "application/json")
        .body(Body::from(body.to_string())).unwrap();
    let response = deltabadger::web::router(u.app.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let text = String::from_utf8(to_bytes(response.into_body(), 1_000_000).await.unwrap().to_vec()).unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}
fn plain(u: &U) -> i64 { let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2; seed::insert_bot(&u.o.primary, &u.s, &spec) }
fn row_state(u: &U, id: i64) -> (Value, bool, i64) {
    (settings(u, id), transient(u, id).get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()), generation(u, id))
}

#[tokio::test]
async fn each_api_op_writes_what_the_form_writes() {
    let mut u = universe();
    cat_b(&u);
    rest_token(&u);
    let (web, by_mcp, by_rest) = (plain(&u), plain(&u), plain(&u));
    type Step<'a> = (&'a [(&'a str, &'a str)], Value);
    let steps: [Step; 4] = [
        (&[("add_index", "nasdaq-100")], json!({ "add_index": "nasdaq-100" })),
        (&[("index_groups][nasdaq-100][weight", "50"), ("index_groups][nasdaq-100][num_coins", "4"), ("index_groups][nasdaq-100][allocation_flattening", "0.5")],
         json!({ "index_groups": { "nasdaq-100": { "weight": 50, "num_coins": 4, "allocation_flattening": 0.5 } } })),
        (&[("change_index_id", "nasdaq-100"), ("change_index_to", "cat-b")], json!({ "change_index": { "from": "nasdaq-100", "to": "cat-b" } })),
        (&[("remove_index_id", "cat-b")], json!({ "remove_index": "cat-b" })),
    ];
    for (form, args) in steps {
        let fields: Vec<(String, &str)> = form.iter().map(|(k, v)| (key(k), *v)).collect();
        let fields: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (k.as_str(), *v)).collect();
        let saved = patch(&mut u, web, &fields).await;
        assert_eq!(saved.status, 200, "{form:?}: {}", saved.body);
        let mut call = args.clone(); call["bot_id"] = json!(by_mcp);
        let answer = mcp(&u, "update_bot_settings", call);
        assert!(answer.to_string().contains("settings updated"), "{args}: {answer}");
        let public: Vec<&String> = args.as_object().unwrap().keys().collect();
        assert!(answer.to_string().contains(&format!("updated: {}.", public[0])), "public names in the answer: {answer}");
        let (status, body) = rest(&u, "PATCH", &format!("/bots/{by_rest}"), args.clone()).await;
        assert_eq!(status, 200, "{args}: {body}");
        assert_eq!(body["data"]["updated"], json!(public), "{body}");
        let want = row_state(&u, web);
        assert_eq!(row_state(&u, by_mcp), want, "MCP after {args}");
        assert_eq!(row_state(&u, by_rest), want, "REST after {args}");
    }
    assert!(settings(&u, web).get("index_groups").is_none());
}

#[tokio::test]
async fn ungroup_through_mcp_is_the_forms() {
    let mut u = universe();
    let (web, by_mcp) = (halves(&u), halves(&u));
    assert_eq!(patch(&mut u, web, &[(&key("ungroup_index_id"), "nasdaq-100")]).await.status, 200);
    let answer = mcp(&u, "update_bot_settings", json!({ "bot_id": by_mcp, "ungroup_index": "nasdaq-100" }));
    assert!(answer.to_string().contains("updated: ungroup_index."), "{answer}");
    assert_eq!(row_state(&u, by_mcp), row_state(&u, web));
}

#[tokio::test]
async fn api_refusals_are_the_forms_and_write_nothing() {
    let u = universe();
    rest_token(&u);
    let basket = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.5)]));
    let lone = grouped_bot(&u, json!({}), json!([group("nasdaq-100", 3, 1.0)]));
    let legacy = seed::index_bot(&u.o.primary, &u.s, "nasdaq-100", 3, 0.0, false);
    exec(&u.o, &format!("UPDATE bots SET status = 2 WHERE id = {legacy}"));
    let cases: [(i64, Value, &str); 7] = [
        (lone, json!({ "remove_index": "nasdaq-100" }), "Set the asset weights"),
        (basket, json!({ "add_index": "cat-x" }), "isn't available"),
        (basket, json!({ "add_index": "nasdaq-100", "remove_index": "nasdaq-100" }), "isn't available"),
        (legacy, json!({ "add_index": "nasdaq-100" }), "isn't available"),
        (basket, json!({ "num_coins": 4 }), "use index_groups"),
        (basket, json!({ "index_groups": { "nasdaq-100": { "weight": "abc" } } }), "must be a number"),
        (basket, json!({ "index_groups": { "nasdaq-100": { "colour": 1 } } }), "index_groups"),
    ];
    for (id, args, why) in cases {
        let before = settings(&u, id);
        let mut call = args.clone(); call["bot_id"] = json!(id);
        let answer = mcp(&u, "update_bot_settings", call).to_string();
        assert!(answer.contains(why) && answer.contains("\"isError\":true"), "{args}: {answer}");
        let mut body = args.clone(); body["quote_amount"] = json!(70);
        let (status, text) = rest(&u, "PATCH", &format!("/bots/{id}"), body).await;
        assert_eq!(status, 422, "{args}: {text}");
        assert!(text.to_string().contains(why), "{args}: {text}");
        assert_eq!(settings(&u, id), before, "{args}: nothing written");
    }
    // REST takes no form-only key and drops no malformed public one: each refused whole, beside a valid amount.
    for body in [json!({ "remove_index_id": "nasdaq-100" }), json!({ "add_index": ["nasdaq-100"] }), json!({ "index_groups": [1] }), json!({ "change_index": "nasdaq-100" })] {
        let mut body = body; body["quote_amount"] = json!(70);
        let (status, text) = rest(&u, "PATCH", &format!("/bots/{basket}"), body.clone()).await;
        assert_eq!(status, 422, "{body}: {text}");
        assert_eq!(settings(&u, basket)["quote_amount"], json!(60.0), "{body}: {text}");
    }
    // A running bot: MCP's recorded refusal, before anything is parsed.
    exec(&u.o, &format!("UPDATE bots SET status = 1 WHERE id = {basket}"));
    let answer = mcp(&u, "update_bot_settings", json!({ "bot_id": basket, "add_index": "nasdaq-100" })).to_string();
    assert!(answer.contains("Bot must be stopped"), "{answer}");
}

#[tokio::test]
async fn a_resubmitted_count_keeps_hold_all() {
    let u = universe();
    let mut g = group("nasdaq-100", 5, 0.5); g["hold_all"] = json!(true);
    let id = grouped_bot(&u, btc(&u, 0.5), json!([g]));
    // The index grew past the held count since: the count shown is still "all of them".
    exec(&u.o, r#"UPDATE indices SET top_coins = '["AAA.US","BBB.US","CCC.US","DDD.US","EEE.US","FFF.US"]' WHERE external_id = 'nasdaq-100'"#);
    let answer = mcp(&u, "update_bot_settings", json!({ "bot_id": id, "index_groups": { "nasdaq-100": { "num_coins": 5, "allocation_flattening": 1 } } }));
    assert!(answer.to_string().contains("settings updated"), "{answer}");
    assert_eq!(settings(&u, id)["index_groups"][0]["hold_all"], json!(true));
}

/// Slice 7: a grouped bot's reads carry its ingredients (with the ids the writes take) and its current members; a plain
/// basket's answer gains nothing. A group whose rows a conversion carried over (`composed = 0`) lists no members yet.
#[tokio::test]
async fn reads_carry_ingredients_and_members_with_ids() {
    let u = universe();
    rest_token(&u);
    cat_b(&u);
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group("nasdaq-100", 3, 0.3), group("cat-b", 3, 0.2)]));
    member(&u, id, (u.s.btc, u.s.ticker_id), 0.5, true);
    member(&u, id, u.m[0], 0.3, true);
    member(&u, id, u.m[1], 0.2, true);
    group_row(&u, id, "nasdaq-100", u.m[0], 0.75);
    group_row(&u, id, "nasdaq-100", u.m[1], 0.25);
    group_row(&u, id, "cat-b", u.m[0], 1.0);
    exec(&u.o, &format!("UPDATE rust_bot_group_members SET composed = 0 WHERE bot_id = {id} AND index_external_id = 'cat-b'"));
    let (status, read) = rest(&u, "GET", &format!("/bots/{id}"), json!({})).await;
    assert_eq!(status, 200, "{read}");
    let d = &read["data"];
    let ingredients = d["ingredients"].as_array().unwrap_or_else(|| panic!("{d}"));
    assert_eq!(ingredients.len(), 3, "{d}");
    assert_eq!((ingredients[0]["kind"].clone(), ingredients[0]["asset_id"].clone(), ingredients[0]["allocation"].clone()), (json!("asset"), json!(u.s.btc), json!(50.0)));
    assert_eq!((ingredients[1]["kind"].clone(), ingredients[1]["id"].clone(), ingredients[1]["allocation"].clone(), ingredients[1]["num_coins"].clone()),
               (json!("index"), json!("nasdaq-100"), json!(30.0), json!(3)));
    assert_eq!(ingredients[1]["members"], json!([{ "asset_id": u.m[0].0, "symbol": "AAA", "allocation": 75.0 }, { "asset_id": u.m[1].0, "symbol": "BBB", "allocation": 25.0 }]));
    assert_eq!(ingredients[2]["members"], json!([]), "carried-over seats are not this composition's");
    let members = d["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[0], json!({ "asset_id": u.s.btc, "ticker_id": u.s.ticker_id, "symbol": "BTC", "allocation": 50.0 }));
    let names: Vec<String> = ingredients[1..].iter().map(|g| g["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(d["pair"], json!(format!("BTC+{}/USD", names.join("+"))), "{d}");
    // The list answers the same, with the same pair; MCP's text names each ingredient with its id.
    let (_, list) = rest(&u, "GET", "/bots", json!({})).await;
    let row = list["data"]["bots"].as_array().unwrap().iter().find(|b| b["id"] == json!(id)).unwrap().clone();
    assert_eq!((row["pair"].clone(), row["ingredients"].clone(), row["members"].clone()), (d["pair"].clone(), d["ingredients"].clone(), d["members"].clone()));
    let text = mcp(&u, "get_bot_details", json!({ "bot_id": id })).to_string();
    assert!(text.contains("Ingredients: BTC 50.00% (asset id") && text.contains("(index id nasdaq-100)"), "{text}");
    // A plain basket: no new key.
    let plain_id = plain(&u);
    let (_, read) = rest(&u, "GET", &format!("/bots/{plain_id}"), json!({})).await;
    assert!(read["data"].get("ingredients").is_none() && read["data"].get("members").is_none(), "{read}");
}

fn newest_bot(u: &U) -> (i64, String, Option<String>) {
    u.o.primary.query_row("SELECT id, type, label FROM bots WHERE status <> 3 ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap()
}
fn bot_count(u: &U) -> i64 { u.o.primary.query_row("SELECT count(*) FROM bots", [], |r| r.get(0)).unwrap() }
fn create_args(extra: Value) -> Value {
    let mut args = json!({ "exchange_name": "Alpaca", "quote_asset": "USD", "quote_amount": 100, "interval": "week" });
    for (k, v) in extra.as_object().unwrap() { args[k] = v.clone(); }
    args
}

/// Slice 7: create_bot's `indexes` beside `assets` makes the wizard's mixed basket (slice 6's `group` and `insert_grouped`):
/// equal thousandths over every ingredient, the groups Add index writes, a resync marked, the caller's label kept.
#[tokio::test]
async fn create_bot_with_assets_and_indexes_writes_the_wizards_row() {
    let u = universe();
    cat_b(&u);
    let answer = mcp(&u, "create_bot", create_args(json!({ "assets": "BTC", "indexes": "nasdaq-100,cat-b", "label": "Mine" }))).to_string();
    assert!(!answer.contains("\"isError\":true"), "{answer}");
    let (id, class, label) = newest_bot(&u);
    assert_eq!((class.as_str(), label.as_deref()), ("Bots::DcaMultiAsset", Some("Mine")));
    let s = settings(&u, id);
    let thousandths = |w: f64| (w * 1000.0).round() as i64;
    let groups: Vec<(String, i64)> = s["index_groups"].as_array().unwrap().iter().map(|g| (composition::external_id(g.as_object()).to_string(), thousandths(g["weight"].as_f64().unwrap()))).collect();
    assert_eq!(groups.iter().map(|g| g.0.as_str()).collect::<Vec<_>>(), ["nasdaq-100", "cat-b"], "{s}");
    let btc_share = thousandths(s["allocations"][u.s.btc.to_string()].as_f64().unwrap());
    assert_eq!(btc_share + groups.iter().map(|g| g.1).sum::<i64>(), 1000, "{s}");
    assert!(groups.iter().all(|g| (g.1 - btc_share).abs() <= 1), "equal shares: {s}");
    assert!(transient(&u, id).get(index::RESYNC_KEY).is_some_and(|v| !v.is_null()));
    assert!(answer.contains("BTC+"), "the pair names the ingredients: {answer}");
}

#[tokio::test]
async fn create_bot_with_two_weighted_indexes_and_no_asset_over_rest() {
    let u = universe();
    cat_b(&u);
    rest_token(&u);
    let body = create_args(json!({ "type": "dca", "indexes": [{ "id": "nasdaq-100", "allocation": 60 }, { "id": "cat-b", "allocation": 40 }] }));
    let (status, answer) = rest(&u, "POST", "/bots", body).await;
    assert_eq!(status, 201, "{answer}");
    let (id, class, _) = newest_bot(&u);
    assert_eq!(answer["data"]["id"], json!(id));
    assert_eq!(class, "Bots::DcaMultiAsset");
    let s = settings(&u, id);
    assert_eq!((weight(&s, "nasdaq-100"), weight(&s, "cat-b")), (0.6, 0.4), "{s}");
    assert_eq!(s["quote_asset_id"], json!(u.s.quote), "the quote asked, with no asset to take it from");
    assert_eq!(answer["data"]["ingredients"].as_array().map(Vec::len), Some(2), "{answer}");
}

#[tokio::test]
async fn create_bot_index_refusals_write_nothing() {
    let u = universe();
    cat_b(&u);
    rest_token(&u);
    seed::insert_index(&u.o.primary, "cat-x", &["ZZZ.US", "YYY.US", "XXX.US"], &json!({}));
    let before = bot_count(&u);
    for (extra, code) in [
        (json!({ "indexes": "nasdaq-100" }), "use_create_index_bot"),
        (json!({ "indexes": ["nasdaq-100"] }), "use_create_index_bot"),
        (json!({ "assets": "BTC", "indexes": "no-such-index" }), "index_not_found"),
        (json!({ "assets": "BTC", "indexes": "nasdaq-100,nasdaq-100" }), "invalid_indexes"),
        (json!({ "assets": "BTC", "indexes": ["nasdaq-100", { "id": 5 }] }), "invalid_indexes"),
        (json!({ "indexes": [{ "id": "nasdaq-100", "allocation": 60 }, { "id": "cat-b", "allocation": 30 }] }), "allocations_unbalanced"),
        (json!({ "assets": "BTC", "indexes": "cat-x" }), "index_not_offered"),
    ] {
        let mut body = create_args(extra.clone()); body["type"] = json!("dca");
        let (status, answer) = rest(&u, "POST", "/bots", body).await;
        assert_eq!((status, answer["error"]["code"].clone()), (if code == "index_not_found" { 404 } else { 422 }, json!(code)), "{extra}: {answer}");
        assert_eq!(bot_count(&u), before, "{extra}: nothing written");
    }
}

/// The pinned contract: create_index_bot still writes `Bots::DcaIndex` and answers as 2.x does.
#[tokio::test]
async fn create_index_bot_answers_its_pinned_contract() {
    let u = universe();
    exec(&u.o, r#"UPDATE indices SET available_exchanges = '{"Exchanges::Alpaca": {}}' WHERE external_id = 'nasdaq-100'"#);
    let answer = mcp(&u, "create_index_bot", create_args(json!({ "index": "nasdaq-100", "num_coins": 3 }))).to_string();
    assert!(!answer.contains("\"isError\":true"), "{answer}");
    let (id, class, _) = newest_bot(&u);
    assert_eq!(class, "Bots::DcaIndex");
    let s = settings(&u, id);
    assert!(s.get("index_groups").is_none() && s["index_category_id"] == json!("nasdaq-100") && s["num_coins"] == json!(3), "{s}");
}
