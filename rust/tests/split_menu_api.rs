//! PLAN-split S2, this build's own (Rails has neither): Split from the bot page's menu (D-SPLIT-BOT-MENU), and the split
//! through REST's POST /api/v1/bots/split and MCP's split_bots (D-SPLIT-API). Every entry point is the web's one writer
//! (bot::split::perform, then split::complete in the same App::db job).
mod common;
use axum::{body::{to_bytes, Body}, http::Request};
use common::seed;
use deltabadger::web;
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

const NOW: &str = "2026-09-10T12:00:30.123456Z";

/// Alpaca baskets at 60 weekly, stopped, one per list of weights (a stock S<n>X<i> per weight), on an account whose MCP
/// and REST clients follow the owner's tools (a NULL grant) and whose owner has `tools` on for both.
struct Install { _dir: tempfile::TempDir, app: web::App, c: rusqlite::Connection, s: seed::Seeded, bots: Vec<i64>, assets: Vec<Vec<(i64, i64)>> }

fn baskets(weights: &[&[f64]], tools: &[&str]) -> Install {
    let (dir, o, s) = common::install_alpaca();
    let c = o.primary;
    c.execute("UPDATE users SET confirmed_at = created_at, wash_sale_enabled = 0", []).unwrap();
    let (mut bots, mut assets) = (vec![], vec![]);
    for (n, list) in weights.iter().enumerate() {
        let stocks: Vec<(i64, i64)> = (0..list.len()).map(|i| seed::add_alpaca_stock(&c, &s, &format!("S{n}X{i}"))).collect();
        let pairs: Vec<(i64, f64)> = stocks.iter().zip(list.iter()).map(|((asset, _), w)| (*asset, *w)).collect();
        let mut spec = seed::BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&pairs);
        spec.status = 2;
        let id = seed::insert_bot(&c, &s, &spec);
        c.execute("UPDATE bots SET position = ?1, label = ?2 WHERE id = ?3", (5 + n as i64, format!("Basket {n}"), id)).unwrap();
        for ((asset, ticker), w) in stocks.iter().zip(list.iter()) { seed::insert_member(&c, id, *asset, *ticker, *w, true, "2026-09-01 14:00:00", None); }
        bots.push(id);
        assets.push(stocks);
    }
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('S2','s2','http://localhost/cb',0,'mcp api','2026-09-10 12:00:00','2026-09-10 12:00:00')", []).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [("s2-mcp", "mcp"), ("s2-api", "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-10 12:00:00',86400)", (application, s.user_id, token, scope)).unwrap();
    }
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,NULL,NULL,'2026-09-10 12:00:00','2026-09-10 12:00:00')", (s.user_id, application)).unwrap();
    let permissions = json!({ "tool_permissions": tools.iter().map(|t| (t.to_string(), json!(true))).collect::<serde_json::Map<_, _>>() }).to_string();
    c.execute("UPDATE users SET mcp_settings = ?1, rest_settings = ?1", [&permissions]).unwrap();
    let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::at(NOW));
    Install { _dir: dir, app, c, s, bots, assets }
}

/// The same, as an Alpaca index bot (whose liquidation rows the split reads): label ND2.
fn index(weights: &[f64], tools: &[&str]) -> Install {
    let mut i = baskets(&[], tools);
    let stocks: Vec<(i64, i64)> = (0..weights.len()).map(|n| seed::add_alpaca_stock(&i.c, &i.s, &format!("S0X{n}"))).collect();
    let symbols: Vec<String> = (0..weights.len()).map(|n| format!("S0X{n}.US")).collect();
    seed::insert_index(&i.c, "nasdaq-100", &symbols.iter().map(String::as_str).collect::<Vec<_>>(), &json!({}));
    let id = seed::index_bot(&i.c, &i.s, "nasdaq-100", weights.len() as i64, 0.0, false);
    i.c.execute("UPDATE bots SET status = 2, position = 5 WHERE id = ?1", [id]).unwrap();
    for ((asset, ticker), w) in stocks.iter().zip(weights) { seed::insert_member(&i.c, id, *asset, *ticker, *w, true, "2026-09-01 14:00:00", None); }
    i.bots = vec![id];
    i.assets = vec![stocks];
    i
}

impl Install {
    fn cookie(&self) -> (String, String) {
        let token = web::csrf::new_token();
        let session = web::session::SessionData { user: Some((self.s.user_id, "x".into())), csrf: Some(token.clone()), ..Default::default() };
        (format!("{}={}", web::session::COOKIE, web::session::seal(&self.app.keys.session, &session, self.app.now())), web::csrf::masked(&token))
    }
    async fn send(&self, request: Request<Body>) -> (u16, Option<String>, String) {
        let response = web::router(self.app.clone()).oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let location = response.headers().get("location").map(|v| v.to_str().unwrap().to_string());
        (status, location, String::from_utf8(to_bytes(response.into_body(), 8_000_000).await.unwrap().to_vec()).unwrap())
    }
    async fn get(&self, path: &str) -> (u16, Option<String>, String) {
        let (cookie, _) = self.cookie();
        self.send(Request::builder().uri(path).header("host", "localhost:3000").header("cookie", cookie).body(Body::empty()).unwrap()).await
    }
    /// A web form post as Turbo sends it.
    async fn post(&self, path: &str, form: &str) -> (u16, Option<String>, String) {
        let (cookie, csrf) = self.cookie();
        self.send(Request::builder().method("POST").uri(path).header("host", "localhost:3000").header("cookie", cookie).header("x-csrf-token", csrf)
            .header("content-type", "application/x-www-form-urlencoded").header("accept", "text/vnd.turbo-stream.html, text/html, application/xhtml+xml")
            .body(Body::from(form.to_string())).unwrap()).await
    }
    async fn rest(&self, body: Value) -> (u16, String) {
        let (status, _, body) = self.send(Request::builder().method("POST").uri("/api/v1/bots/split").header("host", "localhost:3000").header("authorization", "Bearer s2-api")
            .header("content-type", "application/json").header("accept", "application/json").body(Body::from(body.to_string())).unwrap()).await;
        (status, body)
    }
    /// One tools/call of split_bots on a fresh session: the JSON-RPC answer.
    async fn mcp(&self, arguments: Value) -> Value {
        let request = |body: Value, session: Option<&str>| {
            let mut r = Request::builder().method("POST").uri("/mcp").header("host", "localhost:3000").header("authorization", "Bearer s2-mcp")
                .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
            if let Some(session) = session { r = r.header("mcp-session-id", session).header("mcp-protocol-version", "2025-11-25"); }
            r.body(Body::from(body.to_string())).unwrap()
        };
        let response = web::router(self.app.clone()).oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"S2","version":"1"}}}), None)).await.unwrap();
        let session = response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
        assert_eq!(self.send(request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), Some(&session))).await.0, 202);
        let (_, _, body) = self.send(request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"split_bots","arguments":arguments}}), Some(&session))).await;
        serde_json::from_str::<Value>(&body).unwrap()["result"].clone()
    }
    fn live(&self) -> Vec<i64> {
        self.c.prepare("SELECT id FROM bots WHERE status <> 3 ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap()
    }
    /// A closed fill of the first bot's first member.
    fn order(&self, side: i64, kind: &str, quote: &'static str, at: &str) {
        let (bot, asset) = (self.bots[0], self.assets[0][0].0);
        let id = seed::insert_stock_tx(&self.c, &self.s, bot, asset, "S0X0", &seed::TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("o-{at}")), order_type: 0,
            amount: Some("1"), quote_amount: Some(quote), price: Some(quote), quote_amount_exec: Some(quote), amount_exec: Some("1"), created_at: at.into() });
        self.c.execute("UPDATE transactions SET side=?1, transaction_type=?2, base_asset_id=?3 WHERE id=?4", (side, kind, asset, id)).unwrap();
    }
    /// A redeploy still working on the bot: the split is not served.
    fn redeploying(&self) {
        let (bot, (asset, _)) = (self.bots[0], self.assets[0][0]);
        let id = seed::insert_stock_tx(&self.c, &self.s, bot, asset, "S0X0", &seed::TxSpec { status: 0, external_status: Some(1), external_id: Some("resting".into()), order_type: 1,
            amount: Some("1"), quote_amount: None, price: Some("10"), quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-09-10 11:00:00".into() });
        self.c.execute("UPDATE transactions SET transaction_type='REDEPLOY' WHERE id=?1", [id]).unwrap();
    }
}

fn split_link(bot: i64) -> String { format!("<a class=\"dropdown__item\" data-turbo-frame=\"modal\" href=\"/bots/split/new?ids%5B%5D={bot}\">Split</a>") }

/// D-SPLIT-BOT-MENU: the menu offers Split where the dashboard's tile would (two members or more, not executing, no rebalance
/// pending, not archived), and nowhere else.
#[tokio::test(flavor = "current_thread")]
async fn the_menu_offers_split_only_on_a_splittable_bot() {
    let i = baskets(&[&[0.5, 0.5], &[1.0]], &[]);
    let page = |id: i64| { let i = &i; async move { let (status, _, body) = i.get(&format!("/bots/{id}")).await; (status, body) } };
    let menu = |body: &str| body.split("dropdown-wrapper--right").nth(1).unwrap().split("</div>\n</div>").next().unwrap().to_string();
    let (status, shown) = page(i.bots[0]).await;
    assert_eq!(status, 200, "{shown}");
    assert!(menu(&shown).contains(&split_link(i.bots[0])), "{}", menu(&shown));
    // Just before Archive, as a confirmation-modal item.
    let after = shown.split(&split_link(i.bots[0])).nth(1).unwrap().trim_start();
    assert!(after.starts_with(&format!("<a class=\"dropdown__item\" data-turbo-frame=\"modal\" href=\"/bots/{}/archive/edit\">", i.bots[0])), "{}", menu(&shown));
    let (status, single) = page(i.bots[1]).await;
    assert!(status == 200 && !single.contains("/bots/split/new"), "one member");
    for (sql, why) in [("UPDATE bots SET status = 4 WHERE id = ?1", "executing"),
                       ("UPDATE bots SET transient_data = json_set(transient_data, '$.rebalance_pending', json('{\"swap\":1}')) WHERE id = ?1", "a rebalance pending"),
                       ("UPDATE bots SET status = 7 WHERE id = ?1", "archived")] {
        let before: String = i.c.query_row("SELECT status || transient_data FROM bots WHERE id = ?1", [i.bots[0]], |r| r.get(0)).unwrap();
        i.c.execute(sql, [i.bots[0]]).unwrap();
        // A page this build does not draw (501) has no menu at all.
        let (status, body) = page(i.bots[0]).await;
        assert!([200, 501].contains(&status) && !body.contains("/bots/split/new"), "{why}: {status}");
        i.c.execute("UPDATE bots SET status = 2, transient_data = json_remove(transient_data, '$.rebalance_pending') WHERE id = ?1", [i.bots[0]]).unwrap();
        let after: String = i.c.query_row("SELECT status || transient_data FROM bots WHERE id = ?1", [i.bots[0]], |r| r.get(0)).unwrap();
        assert_eq!(after, before);
    }
    assert!(page(i.bots[0]).await.1.contains(&split_link(i.bots[0])));
}

/// The menu's Split opens the dashboard's split modal for this one bot: the same fragment, byte for byte, as split::modal draws
/// for `ids = [bot]`, with no bar of its own.
#[tokio::test(flavor = "current_thread")]
async fn the_menu_opens_the_split_modal_of_this_one_bot() {
    use deltabadger::web::bot::split::{modal, Loaded, Split};
    let i = baskets(&[&[0.6, 0.4], &[0.5, 0.5]], &[]);
    let (status, _, page) = i.get(&format!("/bots/split/new?ids%5B%5D={}", i.bots[0])).await;
    assert_eq!(status, 200, "{page}");
    let token = page.split("name=\"authenticity_token\" value=\"").nth(1).unwrap().split('"').next().unwrap().to_string();
    let ctx = web::layout::Ctx { app: i.app.clone(), params: Arc::new(web::Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: None }),
        session: web::session::Session::new(Default::default()), current: web::auth::Current::SignedOut, method: axum::http::Method::GET, locale: "en", nonce: String::new(),
        now: common::web::at(NOW), turbo_frame: None, stream: false };
    let Ok(Loaded::Split(mut split)) = Split::load(&i.c, &ctx, i.s.user_id, vec![i.bots[0]], vec![]) else { panic!("load") };
    let fragment = modal(&i.c, &ctx, &mut split, &token).unwrap().unwrap();
    assert!(fragment.starts_with("<turbo-frame complete=\"\" id=\"modal\">") && page.contains(&fragment), "{page}");
    assert!(fragment.contains(&format!("<input type=\"hidden\" name=\"ids[]\" value=\"{}\" />", i.bots[0])) && !fragment.contains(&format!("value=\"{}\"", i.bots[1])));
}

/// A lone bot's account (its /bots is the bot's page): the menu's split leaves the children, and /bots is the dashboard.
#[tokio::test(flavor = "current_thread")]
async fn a_lone_bot_split_from_its_menu_lands_on_the_dashboard() {
    let i = baskets(&[&[0.5, 0.3, 0.2]], &[]);
    let bot = i.bots[0];
    assert_eq!(i.get("/bots").await.1.as_deref(), Some(format!("/bots/{bot}").as_str()), "one bot: /bots is its page");
    let (status, _, body) = i.post("/bots/split", &format!("ids%5B%5D={bot}")).await;
    assert_eq!((status, body.as_str()), (200, "<turbo-stream action=\"redirect\" target=\"/bots\"><template></template></turbo-stream>"));
    assert_eq!(i.live().len(), 3);
    let (status, location, _) = i.get("/bots").await;
    assert_eq!((status, location), (200, None), "three bots: the dashboard");
}

/// With one member left that still trades, the split makes one child, and /bots is that child's page.
#[tokio::test(flavor = "current_thread")]
async fn a_lone_bot_split_with_one_survivor_lands_on_the_child() {
    let i = baskets(&[&[0.5, 0.5]], &[]);
    i.c.execute("UPDATE tickers SET trading_enabled = 0 WHERE base = 'S0X1'", []).unwrap();
    let (status, _, body) = i.post("/bots/split", &format!("ids%5B%5D={}", i.bots[0])).await;
    assert_eq!(status, 200, "{body}");
    let live = i.live();
    assert_eq!(live.len(), 1);
    assert_eq!(i.get("/bots").await.1, Some(format!("/bots/{}", live[0])));
}

/// D-SPLIT-API on REST: 200 with the children and the sources; the second call with the same ids finds them gone (`missing`);
/// the children's and sources' figures are marked and the engine is woken, as the web's split does.
#[tokio::test(flavor = "current_thread")]
async fn rest_splits_marks_the_figures_wakes_the_engine_and_then_finds_them_missing() {
    let i = baskets(&[&[0.6, 0.4]], &["split_bots"]);
    let notify = Arc::new(tokio::sync::Notify::new());
    i.app.attach_engine(notify.clone());
    let (status, body) = i.rest(json!({ "bot_ids": [i.bots[0]] })).await;
    let live = i.live();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, format!("{{\"data\":{{\"bots\":[{{\"id\":{},\"label\":\"S0X0\",\"asset_id\":{}}},{{\"id\":{},\"label\":\"S0X1\",\"asset_id\":{}}}],\"source_ids\":[{}]}},\"error\":null}}",
                             live[0], i.assets[0][0].0, live[1], i.assets[0][1].0, i.bots[0]));
    tokio::time::timeout(std::time::Duration::from_secs(1), notify.notified()).await.expect("the engine is woken");
    assert_eq!(i.app.figure_service.take_marked().into_iter().collect::<Vec<_>>(), { let mut all = vec![live[0], live[1], i.bots[0]]; all.sort(); all });
    assert_eq!(i.app.figure_service.take_owners().into_iter().collect::<Vec<_>>(), vec![i.s.user_id]);
    let (status, body) = i.rest(json!({ "bot_ids": [i.bots[0]] })).await;
    assert_eq!((status, body.as_str()), (422, "{\"data\":null,\"error\":{\"code\":\"missing\",\"message\":\"One of these bots is gone. Pick again.\",\"details\":{\"offers\":[]}}}"));
}

/// Money from past sales waiting for an answer: 422 `proceeds_unanswered` with the offer, nothing written; then the same call
/// with `keep_proceeds_for` splits.
#[tokio::test(flavor = "current_thread")]
async fn rest_returns_the_unanswered_offers_then_splits_with_keep_proceeds_for() {
    let i = index(&[0.6, 0.4], &["split_bots"]);
    let bot = i.bots[0];
    i.order(0, "REGULAR", "100", "2026-09-02 10:00:00");
    i.order(1, "LIQUIDATION", "150", "2026-09-03 10:00:00");
    let (status, body) = i.rest(json!({ "bot_ids": [bot] })).await;
    assert_eq!((status, body), (422, format!("{{\"data\":null,\"error\":{{\"code\":\"proceeds_unanswered\",\"message\":\"ND2 still has money from past sales. Reinvest it or keep it first.\",\"details\":{{\"offers\":[{{\"bot_id\":{bot},\"offer\":\"150.0\"}}]}}}}}}")));
    assert_eq!(i.live(), vec![bot]);
    let (status, body) = i.rest(json!({ "bot_ids": [bot], "keep_proceeds_for": [bot] })).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(i.live().len(), 2);
}

/// A split this build does not serve (a redeploy in progress) is the API's 501.
#[tokio::test(flavor = "current_thread")]
async fn rest_answers_an_unported_split_with_501() {
    let i = index(&[0.6, 0.4], &["split_bots"]);
    i.redeploying();
    let (status, body) = i.rest(json!({ "bot_ids": [i.bots[0]] })).await;
    assert_eq!((status, body.as_str()), (501, "{\"data\":null,\"error\":{\"code\":\"not_ported\",\"message\":\"This app can't run that yet: a redeploy in progress\"}}"));
    assert_eq!(i.live(), i.bots);
}

/// D-SPLIT-API on MCP (as mcp/control.rs answers): success is the data as JSON text; a refusal and an unserved split are
/// errors whose JSON text holds the code, the message and the offers.
#[tokio::test(flavor = "current_thread")]
async fn mcp_answers_success_missing_proceeds_and_unported() {
    let i = baskets(&[&[0.6, 0.4]], &["split_bots"]);
    let answer = i.mcp(json!({ "bot_ids": [i.bots[0]] })).await;
    let live = i.live();
    assert_eq!(answer, json!({ "content": [{ "type": "text", "text": format!("{{\"bots\":[{{\"id\":{},\"label\":\"S0X0\",\"asset_id\":{}}},{{\"id\":{},\"label\":\"S0X1\",\"asset_id\":{}}}],\"source_ids\":[{}]}}",
        live[0], i.assets[0][0].0, live[1], i.assets[0][1].0, i.bots[0]) }] }));
    assert_eq!(i.mcp(json!({ "bot_ids": [i.bots[0]] })).await,
               json!({ "isError": true, "content": [{ "type": "text", "text": "{\"code\":\"missing\",\"message\":\"One of these bots is gone. Pick again.\",\"offers\":[]}" }] }));
    assert_eq!(i.mcp(json!({ "bot_ids": "x" })).await, json!({ "isError": true, "content": [{ "type": "text", "text": "Invalid input: value at `/bot_ids` is not an array" }] }));

    let p = index(&[0.6, 0.4], &["split_bots"]);
    let bot = p.bots[0];
    p.order(0, "REGULAR", "100", "2026-09-02 10:00:00");
    p.order(1, "LIQUIDATION", "150", "2026-09-03 10:00:00");
    assert_eq!(p.mcp(json!({ "bot_ids": [bot] })).await, json!({ "isError": true, "content": [{ "type": "text",
        "text": format!("{{\"code\":\"proceeds_unanswered\",\"message\":\"ND2 still has money from past sales. Reinvest it or keep it first.\",\"offers\":[{{\"bot_id\":{bot},\"offer\":\"150.0\"}}]}}") }] }));
    assert!(p.mcp(json!({ "bot_ids": [bot], "keep_proceeds_for": [bot] })).await.get("isError").is_none());
    assert_eq!(p.live().len(), 2);

    let u = index(&[0.6, 0.4], &["split_bots"]);
    u.redeploying();
    assert_eq!(u.mcp(json!({ "bot_ids": [u.bots[0]] })).await, json!({ "isError": true, "content": [{ "type": "text",
        "text": "{\"code\":\"not_ported\",\"message\":\"This app can't run that yet: a redeploy in progress\",\"offers\":[]}" }] }));
    assert_eq!(u.live(), u.bots);
}

/// split_bots is off by default: a client following its owner's tools is refused on both surfaces until the owner turns it on
/// in Settings; then the split goes through.
#[tokio::test(flavor = "current_thread")]
async fn without_the_grant_it_is_refused_until_settings_turn_it_on() {
    let i = baskets(&[&[0.6, 0.4], &[0.5, 0.5]], &[]);
    let (status, body) = i.rest(json!({ "bot_ids": [i.bots[0]] })).await;
    assert_eq!((status, body.as_str()), (403, "{\"data\":null,\"error\":{\"code\":\"tool_disabled\",\"message\":\"Tool 'split_bots' is disabled for this user.\"}}"));
    let request = |session: Option<&str>, body: Value| {
        let mut r = Request::builder().method("POST").uri("/mcp").header("host", "localhost:3000").header("authorization", "Bearer s2-mcp")
            .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
        if let Some(session) = session { r = r.header("mcp-session-id", session).header("mcp-protocol-version", "2025-11-25"); }
        r.body(Body::from(body.to_string())).unwrap()
    };
    let response = web::router(i.app.clone()).oneshot(request(None, json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"S2","version":"1"}}}))).await.unwrap();
    let session = response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
    i.send(request(Some(&session), json!({"jsonrpc":"2.0","method":"notifications/initialized"}))).await;
    let (_, _, listed) = i.send(request(Some(&session), json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))).await;
    assert!(!listed.contains("split_bots"));
    let (_, _, called) = i.send(request(Some(&session), json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"split_bots","arguments":{"bot_ids":[i.bots[0]]}}}))).await;
    assert_eq!(serde_json::from_str::<Value>(&called).unwrap()["error"], json!({"code":-32602,"message":"Tool 'split_bots' not found or not registered for this session"}));
    assert_eq!(i.live(), i.bots);

    for surface in ["mcp", "rest"] {
        let (status, _, body) = i.post(&format!("/settings/update_{surface}_tool_permissions"), "_method=patch&tool_name=split_bots&enabled=1").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("data-tool=\"split_bots\""), "{body}");
    }
    let (_, _, listed) = i.send(request(Some(&session), json!({"jsonrpc":"2.0","id":4,"method":"tools/list"}))).await;
    let listed: Value = serde_json::from_str(&listed).unwrap();
    let schema = listed["result"]["tools"].as_array().unwrap().iter().find(|t| t["name"] == "split_bots").cloned();
    assert_eq!(schema.as_ref(), deltabadger::web::mcp::protocol::metadata()["tools"].as_array().unwrap().iter().find(|t| t["name"] == "split_bots"));
    assert!(i.mcp(json!({ "bot_ids": [i.bots[0]] })).await.get("isError").is_none());
    assert_eq!(i.rest(json!({ "bot_ids": [i.bots[1]] })).await.0, 200);
    assert_eq!(i.live().len(), 4);
}
