//! PLAN index-as-ingredient slice 5: the bot page of a basket stored with index groups, through the real routes: a group row
//! per index (stack, name, weight), the index sheet (count, flattening, preview; Change the index, Custom allocation,
//! Remove), "+ Add asset" / "+ Add index", the composition chart and the titles. Plain baskets and index bots draw none of it.
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
const ND: &str = "nasdaq-100";

/// tests/index_ingredients.rs's universe: five Alpaca stocks in data-api's ND100 row, BTC/USD, a provider, a signed-in owner.
struct U { _d: tempfile::TempDir, o: store::Opened, s: seed::Seeded, m: Vec<(i64, i64)>, app: App, b: Browser }
fn universe() -> U {
    let (d, o, s) = common::install_alpaca();
    let m: Vec<(i64, i64)> = SYMBOLS.iter().map(|sym| seed::add_alpaca_stock(&o.primary, &s, sym)).collect();
    seed::insert_index(&o.primary, ND, &["AAA.US", "BBB.US", "CCC.US", "DDD.US", "EEE.US"],
                       &json!({ "AAA.US": 5e12, "BBB.US": 3e12, "CCC.US": 2e12, "DDD.US": 1e12, "EEE.US": 5e11 }));
    seed::insert_index(&o.primary, "cat-b", &["AAA.US", "BBB.US", "CCC.US"], &json!({ "AAA.US": 3e12, "BBB.US": 2e12, "CCC.US": 1e12 }));
    let legacy = seed::index_bot(&o.primary, &s, ND, 3, 0.0, false);
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
fn grouped_bot(u: &U, allocations: Value, groups: Value) -> i64 {
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00").with("allocations", allocations).with("index_groups", groups);
    spec.status = 2;
    seed::insert_bot(&u.o.primary, &u.s, &spec)
}
fn mixed(u: &U) -> i64 { grouped_bot(u, btc(u, 0.5), json!([group(ND, 3, 0.5)])) }
fn lone(u: &U) -> i64 { grouped_bot(u, json!({}), json!([group(ND, 3, 1.0)])) }
fn btc(u: &U, w: f64) -> Value { json!({ u.s.btc.to_string(): w }) }
fn settings(u: &U, id: i64) -> Value { model::load_bot(&u.o.primary, id).unwrap().settings }
fn weight(s: &Value, external_id: &str) -> f64 {
    s["index_groups"].as_array().unwrap().iter().find(|g| composition::external_id(g.as_object()) == external_id).unwrap()["weight"].as_f64().unwrap()
}
fn the_group(u: &U, id: i64) -> Value { settings(u, id)["index_groups"][0].clone() }
fn compose(u: &U, id: i64, shares: Value) {
    let bot = model::load_bot(&u.o.primary, id).unwrap();
    let ranked: serde_json::Map<String, Value> = bot.index_external_ids().into_iter()
        .map(|g| { let r = index::ranked_row(&u.o.primary, &g).unwrap().unwrap(); (g, json!([r.id, r.ranked_at])) }).collect();
    composition::record(&u.o.primary, &bot, Value::Object(ranked), shares).unwrap();
}
/// A composition of `group` as the grouped write leaves it: the group's rows and the flat targets.
fn seat(u: &U, id: i64, group: &str, members: &[(usize, f64, f64)]) {
    for (m, within, target) in members {
        let (asset, ticker) = u.m[*m];
        exec(&u.o, &format!("INSERT INTO rust_bot_group_members VALUES ({id}, '{group}', {asset}, {ticker}, 1, {within})"));
        seed::insert_member(&u.o.primary, id, asset, ticker, *target, true, "2026-09-01 14:00:00", None);
    }
}
async fn page(u: &mut U, id: i64) -> String {
    let page = u.b.get(&u.app, &format!("/bots/{id}")).await;
    assert_eq!(page.status, 200, "{}", page.body.chars().take(800).collect::<String>());
    page.body
}
async fn send(u: &mut U, id: i64, method: &str, path: &str, fields: &[(&str, &str)]) -> Answer {
    page(u, id).await;
    u.b.send(&u.app, method, path, Some(fields), Csrf::Header, &TURBO).await
}
/// The named, enabled inputs of the composition form as the page drew it, with their values.
fn drawn_fields(body: &str) -> Vec<(String, String)> {
    let start = body.find("main-rule--multi-asset").expect("the basket form");
    let form = &body[start..start + body[start..].find("</form>").unwrap()];
    form.split("<input").skip(1).filter_map(|tag| {
        let mut quoted = false;
        let end = tag.char_indices().find(|(_, ch)| { if *ch == '"' { quoted = !quoted; } !quoted && *ch == '>' }).unwrap().0;
        let tag = &tag[..end];
        let attr = |name: &str| tag.split(&format!("{name}=\"")).nth(1).map(|rest| rest[..rest.find('"').unwrap()].to_string());
        let name = attr("name")?;
        if tag.contains(" disabled") || name == "authenticity_token" || name == "_method" || attr("type").as_deref() == Some("checkbox") { return None; }
        Some((name.replace("&#x5b;", "[").replace("&#x5d;", "]"), attr("value").unwrap_or_default()))
    }).collect()
}
fn segment<'a>(body: &'a str, from: &str, to: &str) -> &'a str {
    let start = body.find(from).unwrap_or_else(|| panic!("{from} in {body}"));
    &body[start..start + body[start..].find(to).unwrap()]
}
const ROW: &str = "bots_dca_multi_asset[index_groups][nasdaq-100]";

#[tokio::test]
async fn a_mixed_bot_renders_asset_and_group_rows_with_one_total() {
    let mut u = universe();
    let id = mixed(&u);
    let body = page(&mut u, id).await;
    let stack = segment(&body, "id=\"index_group_nasdaq-100_stack\"", "</a>");
    for want in [format!("href=\"/bots/{id}/index_groups/nasdaq-100/edit\""), "data-turbo-frame=\"modal\"".into(), "aria-label=\"ND3: settings\"".into(), "ticker-group--many".into()] {
        assert!(stack.contains(&want), "{want} in {stack}");
    }
    assert!(body.contains(&format!("name=\"{ROW}[weight]\"")), "the group's weight slider");
    assert!(body.contains(&format!("name=\"bots_dca_multi_asset[allocations][{}]\"", u.s.btc)), "BTC's slider");
    assert!(body.contains(">100.0%<"), "one total over assets and groups");
    assert!(body.contains("aria-label=\"ND3\""), "the weight is labelled by its index");
    assert!(body.contains(&format!("href=\"/bots/{id}/index_groups/new\"")), "+ Add index");
    assert!(body.contains("data-controller=\"donut-chart\""), "the composition");
}

#[tokio::test]
async fn btc_beside_an_index_is_not_a_one_asset_sentence() {
    let mut u = universe();
    let id = mixed(&u);
    let body = page(&mut u, id).await;
    assert!(body.contains("bot--allocation") && body.contains("asset-allocations"), "the weights, not BTC in the sentence");
}

#[tokio::test]
async fn group_chips_come_from_composed_rows_else_the_preview() {
    let mut u = universe();
    let id = mixed(&u);
    let body = page(&mut u, id).await;
    let stack = segment(&body, "id=\"index_group_nasdaq-100_stack\"", "</a>").to_string();
    assert!(stack.contains(">AAA<") && stack.contains(">BBB<"), "no composition yet: the preview: {stack}");
    seat(&u, id, ND, &[(2, 1.0, 0.5)]);
    let body = page(&mut u, id).await;
    let stack = segment(&body, "id=\"index_group_nasdaq-100_stack\"", "</a>");
    assert!(stack.contains(">CCC<") && !stack.contains(">AAA<"), "the composed rows: {stack}");
}

#[tokio::test]
async fn an_amount_edit_through_the_rendered_form_keeps_99_96_and_0_04() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.9996), json!([group(ND, 3, 0.0004)]));
    let body = page(&mut u, id).await;
    let mut fields = drawn_fields(&body);
    assert!(fields.iter().any(|(n, v)| n == &format!("{ROW}[weight]") && v == "0.0"), "{fields:?}");
    fields.retain(|(n, _)| !n.ends_with("[quote_amount]"));
    fields.push(("bots_dca_multi_asset[quote_amount]".into(), "61".into()));
    let posted: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    let saved = u.b.send(&u.app, "PATCH", &format!("/bots/{id}"), Some(&posted), Csrf::Header, &TURBO).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    let s = settings(&u, id);
    assert_eq!((s["quote_amount"].clone(), s["allocations"][u.s.btc.to_string()].clone(), weight(&s, ND)), (json!(61.0), json!(0.9996), 0.0004), "{s}");
}

#[tokio::test]
async fn normalize_of_the_drawn_form_keeps_exact_weights() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5996), json!([group(ND, 3, 0.0004)]));
    let body = page(&mut u, id).await;
    let mut fields = drawn_fields(&body);
    fields.push(("bots_dca_multi_asset[normalize_allocations]".into(), "1".into()));
    let posted: Vec<(&str, &str)> = fields.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
    assert_eq!(u.b.send(&u.app, "PATCH", &format!("/bots/{id}"), Some(&posted), Csrf::Header, &TURBO).await.status, 200);
    let s = settings(&u, id);
    assert!((s["allocations"][u.s.btc.to_string()].as_f64().unwrap() - 0.5996 / 0.6).abs() < 1e-12, "{s}");
    assert!((weight(&s, ND) - 0.0004 / 0.6).abs() < 1e-12, "the 0.04 % scaled, never zeroed: {s}");
}

#[tokio::test]
async fn a_market_cap_mixed_bot_shows_composed_shares_and_zero_for_a_dropped_asset() {
    let mut u = universe();
    let id = grouped_bot(&u, btc(&u, 0.5), json!([group(ND, 3, 0.5)]));
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.weighting', 'market_cap') WHERE id = {id}"));
    let body = page(&mut u, id).await;
    assert!(body.contains(&format!("name=\"{ROW}[weight]\"")), "{body}");
    let row = segment(&body, &format!("name=\"{ROW}[weight]\""), "slider__style\"");
    assert!(row.contains("value=\"50.0\"") && row.contains("disabled"), "no snapshot: the stored weight, read-only: {row}");
    compose(&u, id, json!({ "assets": {}, "groups": { ND: 1.0 } }));
    let body = page(&mut u, id).await;
    let row = segment(&body, &format!("name=\"{ROW}[weight]\""), "slider__style\"");
    assert!(row.contains("value=\"100.0\""), "{row}");
    let btc_row = segment(&body, &format!("name=\"bots_dca_multi_asset[allocations][{}]\"", u.s.btc), "slider__style\"");
    assert!(btc_row.contains("value=\"0.0\""), "the engine dropped BTC: its share is zero: {btc_row}");
}

#[tokio::test]
async fn an_all_index_basket_offers_market_cap_and_counts_ingredients() {
    let mut u = universe();
    let id = grouped_bot(&u, json!({}), json!([group(ND, 3, 0.5), group("cat-b", 3, 0.5)]));
    let body = page(&mut u, id).await;
    assert!(body.contains("name=\"bots_dca_multi_asset[weighting]\""), "two indexes, no asset: market cap is offered");
    let mut weights = serde_json::Map::new();
    for i in 0..99 {
        exec(&u.o, &format!("INSERT INTO assets (external_id, symbol, name, created_at, updated_at) VALUES ('x{i}', 'X{i}', 'X{i}', '2026-01-01', '2026-01-01')"));
        weights.insert(u.o.primary.last_insert_rowid().to_string(), json!(0.0));
    }
    let id = grouped_bot(&u, Value::Object(weights), json!([group(ND, 3, 0.5), group("cat-b", 3, 0.5)]));
    let body = page(&mut u, id).await;
    assert!(body.contains("Too many assets. Remove 1."), "99 assets and two indexes: one too many");
}

#[tokio::test]
async fn the_composition_reads_every_current_target() {
    let mut u = universe();
    let id = mixed(&u);
    for i in 0..1003 {
        exec(&u.o, &format!("INSERT INTO assets (external_id, symbol, name, created_at, updated_at) VALUES ('y{i}', 'Y{i}', 'Y{i}', '2026-01-01', '2026-01-01')"));
        let asset = u.o.primary.last_insert_rowid();
        seed::insert_member(&u.o.primary, id, asset, u.m[0].1, 0.0009, true, "2026-09-01 14:00:00", None);
    }
    let body = page(&mut u, id).await;
    let data = segment(&body, "data-donut-chart-data-value=", "data-donut-chart-storage-key-value");
    assert_eq!(data.matches("assetId").count(), 1003, "every current target, not the page's capped 1000");
}

#[tokio::test]
async fn a_pending_composition_is_captioned_and_an_empty_one_says_so() {
    let mut u = universe();
    let id = mixed(&u);
    let body = page(&mut u, id).await;
    assert!(body.contains("No members yet: they are chosen at the next update."), "empty");
    seat(&u, id, ND, &[(0, 1.0, 0.5)]);
    let body = page(&mut u, id).await;
    assert!(body.contains("Being updated: this is the last composition."), "rows, no digest: pending");
    compose(&u, id, json!({ "assets": {}, "groups": {} }));
    let body = page(&mut u, id).await;
    assert!(!body.contains("Being updated") && !body.contains("No members yet"), "composed");
}

#[tokio::test]
async fn a_lone_group_shows_its_composition_and_the_add_links() {
    let mut u = universe();
    let id = lone(&u);
    let body = page(&mut u, id).await;
    assert!(body.contains("data-controller=\"index-allocation\""), "the index view (4a)");
    assert!(body.contains("data-controller=\"donut-chart\"") && body.contains("No members yet"), "the composition beside the preview");
    assert!(body.contains(&format!("/bots/{id}/asset_search/edit?asset_field=add_asset_id")), "+ Add asset");
    assert!(body.contains(&format!("href=\"/bots/{id}/index_groups/new\"")), "+ Add index");
    assert!(body.contains("aria-label=\"Number of coins\"") && body.contains("aria-label=\"Allocation flattening\""), "labelled sliders");
}

#[tokio::test]
async fn adding_an_asset_to_a_lone_group_replaces_the_menu() {
    let mut u = universe();
    let id = lone(&u);
    let body = page(&mut u, id).await;
    let from = "<div class=\"flex-row\" id=\"";
    let menu = segment(&body, from, "\">")[from.len()..].to_string();
    assert!(menu.starts_with("menu_"), "{menu}");
    let btc = u.s.btc.to_string();
    let saved = send(&mut u, id, "PATCH", &format!("/bots/{id}"), &[("bots_dca_multi_asset[add_asset_id]", &btc)]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert!(saved.body.contains(&format!("action=\"replace\" target=\"{menu}\"")), "the menu of a mixed bot: {}", saved.body);
    let body = page(&mut u, id).await;
    assert!(body.contains("Follow index") && !body.contains(&format!("{id}/index/new\">Change the index")), "a mixed bot's menu");
}

#[tokio::test]
async fn add_index_picker_omits_held_indexes_and_follow_keeps_todays_offer() {
    let mut u = universe();
    let id = mixed(&u);
    let add = u.b.get(&u.app, &format!("/bots/{id}/index_groups/new")).await;
    assert_eq!(add.status, 200, "{}", add.body);
    assert!(add.body.contains("data-index-category-id=\"cat-b\"") && !add.body.contains("data-index-category-id=\"nasdaq-100\""), "{}", add.body);
    assert!(add.body.contains(&format!("action=\"/bots/{id}/index_groups\"")));
    let follow = u.b.get(&u.app, &format!("/bots/{id}/index/new")).await;
    assert!(follow.body.contains("data-index-category-id=\"nasdaq-100\""), "the whole-bot Follow picker is unchanged");
}

#[tokio::test]
async fn add_index_through_its_route_and_its_refusal_stays_in_the_sheet() {
    let mut u = universe();
    let id = mixed(&u);
    let added = send(&mut u, id, "POST", &format!("/bots/{id}/index_groups"), &[("index_type", "category"), ("index_category_id", "cat-b")]).await;
    assert_eq!(added.status, 200, "{}", added.body);
    assert!(added.body.contains("action=\"redirect\""), "{}", added.body);
    assert_eq!(weight(&settings(&u, id), "cat-b"), 0.0);
    let before = settings(&u, id);
    let refused = send(&mut u, id, "POST", &format!("/bots/{id}/index_groups"), &[("index_type", "category"), ("index_category_id", "cat-b")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("action=\"update\" target=\"modal_content\"") && refused.body.contains("form__info--invalid"), "{}", refused.body);
    assert_eq!(settings(&u, id), before);
}

#[tokio::test]
async fn the_sheet_opens_with_the_groups_sliders_and_404s_otherwise() {
    let mut u = universe();
    let id = mixed(&u);
    let sheet = u.b.get(&u.app, &format!("/bots/{id}/index_groups/nasdaq-100/edit")).await;
    assert_eq!(sheet.status, 200, "{}", sheet.body);
    for want in [format!("name=\"{ROW}[num_coins]\""), format!("name=\"{ROW}[num_coins_rendered]\""), format!("name=\"{ROW}[num_coins_ceiling]\""),
                 format!("name=\"{ROW}[allocation_flattening]\""), format!("action=\"/bots/{id}/index_groups/nasdaq-100\""), "sheet-dialog".into(),
                 "data-controller=\"index-allocation\"".into(), "aria-label=\"Number of coins\"".into(), "Change the index".into(), "Custom allocation".into(),
                 format!("href=\"/bots/{id}/index_groups/nasdaq-100/index/new\"")] {
        assert!(sheet.body.contains(&want), "{want}");
    }
    // The not-found answer of a bot page (a visit to the dashboard), for a group the bot does not hold and for an ungrouped bot.
    assert_eq!(u.b.get(&u.app, &format!("/bots/{id}/index_groups/cat-b/edit")).await.status, 302, "not held");
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00"); spec.status = 2;
    let plain = seed::insert_bot(&u.o.primary, &u.s, &spec);
    assert_eq!(u.b.get(&u.app, &format!("/bots/{plain}/index_groups/nasdaq-100/edit")).await.status, 302);
    assert_eq!(u.b.get(&u.app, &format!("/bots/{plain}/index_groups/new")).await.status, 302);
}

#[tokio::test]
async fn sheet_saves_count_3_4_3() {
    let mut u = universe();
    let id = mixed(&u);
    let path = format!("/bots/{id}/index_groups/nasdaq-100");
    let sheet = u.b.get(&u.app, &format!("{path}/edit")).await;
    let ceiling = segment(&sheet.body, &format!("name=\"{ROW}[num_coins_ceiling]\""), ">").split("value=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    let saved = send(&mut u, id, "PATCH", &path, &[(&format!("{ROW}[num_coins]"), "4"), (&format!("{ROW}[num_coins_rendered]"), "3"), (&format!("{ROW}[num_coins_ceiling]"), &ceiling)]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(the_group(&u, id)["num_coins"], json!(4));
    assert!(saved.body.contains("action=\"update\" target=\"modal_content\"") && saved.body.contains("action=\"replace\" target=\"settings\""), "{}", saved.body);
    let rendered = segment(&saved.body, &format!("name=\"{ROW}[num_coins_rendered]\""), ">").to_string();
    assert!(rendered.contains("value=\"4\""), "the sheet's hidden count follows the save: {rendered}");
    let saved = send(&mut u, id, "PATCH", &path, &[(&format!("{ROW}[num_coins]"), "3"), (&format!("{ROW}[num_coins_rendered]"), "4"), (&format!("{ROW}[num_coins_ceiling]"), &ceiling)]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(the_group(&u, id)["num_coins"], json!(3), "back to three");
}

#[tokio::test]
async fn an_unchanged_sheet_repost_keeps_num_coins_and_hold_all() {
    let mut u = universe();
    let id = mixed(&u);
    exec(&u.o, &format!("UPDATE bots SET settings = json_set(settings, '$.index_groups[0].num_coins', 5, '$.index_groups[0].hold_all', json('true')) WHERE id = {id}"));
    let before = the_group(&u, id);
    let saved = send(&mut u, id, "PATCH", &format!("/bots/{id}/index_groups/nasdaq-100"),
                     &[(&format!("{ROW}[num_coins]"), "5"), (&format!("{ROW}[num_coins_rendered]"), "5"), (&format!("{ROW}[num_coins_ceiling]"), "99"), (&format!("{ROW}[allocation_flattening]"), "1")]).await;
    assert_eq!(saved.status, 200, "{}", saved.body);
    assert_eq!(the_group(&u, id), before);
}

#[tokio::test]
async fn a_sheet_refusal_is_422_in_the_sheet_and_unchanged() {
    let mut u = universe();
    let id = mixed(&u);
    exec(&u.o, &format!("UPDATE bots SET status = 1 WHERE id = {id}"));
    let before = settings(&u, id);
    let refused = send(&mut u, id, "PATCH", &format!("/bots/{id}/index_groups/nasdaq-100"), &[(&format!("{ROW}[num_coins]"), "4"), (&format!("{ROW}[num_coins_rendered]"), "3")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("action=\"update\" target=\"modal_content\"") && refused.body.contains("form__info--invalid"), "{}", refused.body);
    assert_eq!(settings(&u, id), before);
}

#[tokio::test]
async fn change_from_the_sheet_keeps_the_weight() {
    let mut u = universe();
    let id = mixed(&u);
    let picker = u.b.get(&u.app, &format!("/bots/{id}/index_groups/nasdaq-100/index/new")).await;
    assert_eq!(picker.status, 200, "{}", picker.body);
    assert!(picker.body.contains("data-index-category-id=\"cat-b\"") && !picker.body.contains("data-index-category-id=\"nasdaq-100\""));
    assert!(picker.body.contains(&format!("action=\"/bots/{id}/index_groups/nasdaq-100\"")));
    let changed = send(&mut u, id, "POST", &format!("/bots/{id}/index_groups/nasdaq-100"), &[("index_type", "category"), ("index_category_id", "cat-b")]).await;
    assert_eq!(changed.status, 200, "{}", changed.body);
    let s = settings(&u, id);
    assert_eq!(weight(&s, "cat-b"), 0.5, "{s}");
    assert_eq!(s["index_groups"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn custom_allocation_from_the_sheet_ungroups_that_group() {
    let mut u = universe();
    let id = mixed(&u);
    seat(&u, id, ND, &[(0, 0.6, 0.3), (1, 0.4, 0.2)]);
    compose(&u, id, json!({ "assets": {}, "groups": {} }));
    let done = send(&mut u, id, "POST", &format!("/bots/{id}/index_groups/nasdaq-100"), &[("custom_allocation", "1")]).await;
    assert_eq!(done.status, 200, "{}", done.body);
    let s = settings(&u, id);
    assert!(s.get("index_groups").is_none(), "{s}");
    assert!((s["allocations"][u.m[0].0.to_string()].as_f64().unwrap() - 0.3).abs() < 1e-12, "{s}");
}

#[tokio::test]
async fn remove_from_the_sheet_and_weights_first_in_the_sheet() {
    let mut u = universe();
    let id = mixed(&u);
    let before = settings(&u, id);
    let refused = send(&mut u, id, "POST", &format!("/bots/{id}/index_groups/nasdaq-100"), &[("remove", "1")]).await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert!(refused.body.contains("action=\"update\" target=\"modal_content\""), "{}", refused.body);
    assert_eq!(settings(&u, id), before);
    let id = grouped_bot(&u, btc(&u, 1.0), json!([group(ND, 3, 0.0)]));
    let removed = send(&mut u, id, "POST", &format!("/bots/{id}/index_groups/nasdaq-100"), &[("remove", "1")]).await;
    assert_eq!(removed.status, 200, "{}", removed.body);
    assert!(removed.body.contains("action=\"redirect\""));
    assert!(settings(&u, id).get("index_groups").is_none());
}

#[tokio::test]
async fn a_retired_group_sheet_is_frozen_on_its_rows() {
    let mut u = universe();
    let id = mixed(&u);
    seat(&u, id, ND, &[(3, 1.0, 0.5)]);
    let provider = deltabadger::engine::provider::ranking_provider(&u.o.primary).unwrap().unwrap();
    exec(&u.o, &format!("INSERT INTO rust_index_retired VALUES ('nasdaq-100', '{provider}', '2026-09-01T00:00:00Z')"));
    let sheet = u.b.get(&u.app, &format!("/bots/{id}/index_groups/nasdaq-100/edit")).await;
    assert_eq!(sheet.status, 200, "{}", sheet.body);
    let assets = segment(&sheet.body, "class=\"index-assets\"", "</div>\n      <p");
    assert!(assets.contains(">DDD<") && !assets.contains(">AAA<"), "the group's own rows, not the bot's: {assets}");
}

#[tokio::test]
async fn a_resynced_grouped_bot_redraws_its_settings() {
    let u = universe();
    let id = mixed(&u);
    let streams = deltabadger::web::bot::live::render(&u.o.primary, &u.app, &deltabadger::engine::events::EngineEvent::Recomposed { bot_id: id }).unwrap();
    assert!(streams.iter().any(|(_, s)| s.contains("action=\"replace\" target=\"settings\"") && s.contains("index_group_nasdaq-100_stack")), "{streams:?}");
}

#[tokio::test]
async fn a_mixed_bot_is_titled_by_its_ingredients() {
    let mut u = universe();
    let id = mixed(&u);
    let _other = lone(&u);
    let list = u.b.get(&u.app, "/bots").await;
    assert!(list.body.contains("BTC, ND3"), "{}", list.body);
    let body = page(&mut u, id).await;
    assert!(body.contains("BTC, ND3"), "the default name");
}

#[tokio::test]
async fn plain_basket_and_index_bot_pages_draw_none_of_it() {
    let mut u = universe();
    let mut spec = BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(u.s.btc, 0.5), (u.m[0].0, 0.5)]); spec.status = 2;
    let plain = seed::insert_bot(&u.o.primary, &u.s, &spec);
    let index = seed::index_bot(&u.o.primary, &u.s, ND, 3, 0.0, false);
    for id in [plain, index] {
        let body = page(&mut u, id).await;
        for unwanted in ["index_groups", "donut-chart", "Add index", "data-modal-return-focus"] {
            assert!(!body.contains(unwanted), "{id}: {unwanted}");
        }
    }
}
