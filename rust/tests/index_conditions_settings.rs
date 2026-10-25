//! PLAN-conditions-every-bot CE-2: an index bot's spending cap and buy-side price rule through its settings page, MCP's
//! update_bot_settings and REST's PATCH /api/v1/bots/:id. All three are the one settings writer (bot::write::settings): the
//! same rows and, between MCP and REST, the same answer. Rails' index bot has neither rule (D-IDX-CONDITIONS, D-IDX-CAP);
//! the subject is persisted on enable (D-IDX-SUBJECT-DEFAULT) and a subject change clears the latch
//! (D-SUBJECT-CHANGE-CLEARS-LATCH). What an index bot has nothing to act on is refused by name (unsupported_setting); what
//! this build does not run yet is the write guard's refusal.
mod common;
use axum::{body::{to_bytes, Body}, http::Request};
use common::seed;
use deltabadger::web;
use serde_json::{json, Value};
use tower::ServiceExt;

const NOW: &str = "2026-09-10T12:00:30.123456Z";

/// A stopped Alpaca index bot over AAA, BBB, CCC (targets in `targets`, 0 = no member row), its index naming them in that
/// order, and an MCP and a REST client allowed update_bot_settings: (dir, app, bot id, [ticker id] in that order).
struct Install { _dir: tempfile::TempDir, app: web::App, c: rusqlite::Connection, user: i64, id: i64, tickers: Vec<i64> }

fn install(targets: [f64; 3]) -> Install {
    let (dir, o, s) = common::install_alpaca();
    let c = o.primary;
    let members: Vec<(i64, i64)> = ["AAA", "BBB", "CCC"].iter().map(|sym| seed::add_alpaca_stock(&c, &s, sym)).collect();
    seed::insert_index(&c, "nasdaq-100", &["AAA.US", "BBB.US", "CCC.US"], &json!({ "AAA.US": 3e12, "BBB.US": 2e12, "CCC.US": 1e12 }));
    let id = seed::index_bot(&c, &s, "nasdaq-100", 3, 0.0, false);
    for ((asset, ticker), target) in members.iter().zip(targets) {
        if target > 0.0 { seed::insert_member(&c, id, *asset, *ticker, target, true, "2026-09-01 14:00:00", None); }
    }
    c.execute("UPDATE bots SET status = 2, label = 'Index' WHERE id = ?1", [id]).unwrap();
    c.execute("UPDATE users SET confirmed_at = created_at, wash_sale_enabled = 0", []).unwrap();
    c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('CE2','ce2','http://localhost/cb',0,'mcp api','2026-09-10 12:00:00','2026-09-10 12:00:00')", []).unwrap();
    let application = c.last_insert_rowid();
    for (token, scope) in [("ce2-mcp", "mcp"), ("ce2-api", "api")] {
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,?3,?4,'2026-09-10 12:00:00',86400)", (application, s.user_id, token, scope)).unwrap();
    }
    let tools = json!(["update_bot_settings"]).to_string();
    c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,?3,?3,'2026-09-10 12:00:00','2026-09-10 12:00:00')", (s.user_id, application, &tools)).unwrap();
    let permissions = json!({"tool_permissions":{"update_bot_settings":true}}).to_string();
    c.execute("UPDATE users SET mcp_settings = ?1, rest_settings = ?1", [&permissions]).unwrap();
    let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::at(NOW));
    Install { _dir: dir, app, c, user: s.user_id, id, tickers: members.iter().map(|(_, t)| *t).collect() }
}

#[derive(Clone, Copy, Debug)]
enum Surface { Web, Mcp, Rest }

/// What a save answered: the HTTP status, the service's answer (MCP's text, REST's JSON body; the web's page as text).
struct Saved { status: u16, answer: Value }

async fn send(app: &web::App, request: Request<Body>) -> (u16, String) {
    let response = web::router(app.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    (status, String::from_utf8(to_bytes(response.into_body(), 4_000_000).await.unwrap().to_vec()).unwrap())
}

/// One settings save of `web` (the page's post, under `bots_dca_index`) or `api` (the automation dialect) on `surface`.
async fn save(i: &Install, surface: Surface, web_fields: Value, api_fields: Value) -> Saved {
    match surface {
        Surface::Web => {
            let token = web::csrf::new_token();
            let session = web::session::SessionData { user: Some((i.user, "x".into())), csrf: Some(token.clone()), ..Default::default() };
            let cookie = web::session::seal(&i.app.keys.session, &session, i.app.now());
            let request = Request::builder().method("PATCH").uri(format!("/bots/{}", i.id)).header("host", "localhost:3000")
                .header("cookie", format!("{}={cookie}", web::session::COOKIE)).header("x-csrf-token", web::csrf::masked(&token))
                .header("content-type", "application/json").header("accept", "text/vnd.turbo-stream.html")
                .body(Body::from(json!({ "bots_dca_index": web_fields }).to_string())).unwrap();
            let (status, body) = send(&i.app, request).await;
            Saved { status, answer: json!(body) }
        }
        Surface::Rest => {
            let request = Request::builder().method("PATCH").uri(format!("/api/v1/bots/{}", i.id)).header("host", "localhost:3000")
                .header("authorization", "Bearer ce2-api").header("content-type", "application/json").header("accept", "application/json")
                .body(Body::from(api_fields.to_string())).unwrap();
            let (status, body) = send(&i.app, request).await;
            Saved { status, answer: serde_json::from_str(&body).unwrap_or(json!(body)) }
        }
        Surface::Mcp => {
            let request = |body: Value, session: Option<&str>| {
                let mut r = Request::builder().method("POST").uri("/mcp").header("host", "localhost:3000").header("authorization", "Bearer ce2-mcp")
                    .header("content-type", "application/json").header("accept", "application/json, text/event-stream");
                if let Some(session) = session { r = r.header("mcp-session-id", session); }
                r.body(Body::from(body.to_string())).unwrap()
            };
            let response = web::router(i.app.clone()).oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"CE2","version":"1"}}}), None)).await.unwrap();
            let session = response.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
            assert_eq!(send(&i.app, request(json!({"jsonrpc":"2.0","method":"notifications/initialized"}), Some(&session))).await.0, 202);
            let mut arguments = api_fields.as_object().unwrap().clone();
            arguments.insert("bot_id".into(), json!(i.id));
            let (status, body) = send(&i.app, request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"update_bot_settings","arguments":arguments}}), Some(&session))).await;
            let body: Value = serde_json::from_str(&body).unwrap();
            Saved { status, answer: json!({ "text": body["result"]["content"][0]["text"], "error": body["result"]["isError"] }) }
        }
    }
}

/// The bot's row as a save leaves it: every column the settings writer touches.
fn row(i: &Install) -> Value {
    let (settings, transient, changed, updated): (String, String, Option<String>, String) = i.c.query_row(
        "SELECT settings, transient_data, settings_changed_at, updated_at FROM bots WHERE id = ?1", [i.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap();
    json!({ "settings": serde_json::from_str::<Value>(&settings).unwrap(), "transient": serde_json::from_str::<Value>(&transient).unwrap(), "changed": changed, "updated": updated })
}

/// The same change on each surface, each on a fresh install: the rows are equal, and MCP and REST answer the same.
async fn on_every_surface(targets: [f64; 3], setup: impl Fn(&Install), web: impl Fn(&Install) -> Value, api: impl Fn(&Install) -> Value) -> (Vec<Saved>, Value, Install) {
    let mut saved = vec![];
    let mut rows = vec![];
    let mut last = None;
    for surface in [Surface::Web, Surface::Mcp, Surface::Rest] {
        let i = install(targets);
        setup(&i);
        saved.push(save(&i, surface, web(&i), api(&i)).await);
        rows.push(row(&i));
        last = Some(i);
    }
    assert_eq!(rows[0], rows[1], "web and MCP write the same row");
    assert_eq!(rows[0], rows[2], "web and REST write the same row");
    (saved, rows.remove(0), last.unwrap())
}

fn mcp_updated(saved: &Saved) -> String { saved.answer["text"].as_str().unwrap().to_string() }

#[tokio::test(flavor = "current_thread")]
async fn a_price_rule_saves_the_same_row_on_the_page_mcp_and_rest() {
    let (saved, row, i) = on_every_surface([0.5, 0.3, 0.2], |_| {},
        |i| json!({ "price_limited": "1", "price_limit_mode": "restrict", "price_limit_value_condition": "below", "price_limit": "150", "price_limit_in_ticker_id": i.tickers[1].to_string() }),
        |i| json!({ "price_limited": true, "price_limit_mode": "restrict", "price_limit_value_condition": "below", "price_limit": 150, "price_limit_in_ticker_id": i.tickers[1] })).await;
    assert_eq!(saved[0].status, 200, "{}", saved[0].answer);
    assert_eq!(mcp_updated(&saved[1]), "Bot 'Index' settings updated: price_limited, price_limit_mode, price_limit_value_condition, price_limit, price_limit_in_ticker_id.");
    assert_eq!(saved[2].status, 200);
    assert_eq!(saved[2].answer["data"]["updated"], json!(["price_limited", "price_limit_mode", "price_limit_value_condition", "price_limit", "price_limit_in_ticker_id"]));
    let s = &row["settings"];
    assert_eq!((&s["price_limited"], &s["price_limit"], &s["price_limit_value_condition"], &s["price_limit_timing_condition"], &s["price_limit_action"], &s["price_limit_in_ticker_id"]),
               (&json!(true), &json!(150.0), &json!("below"), &json!("while"), &json!("pause"), &json!(i.tickers[1])));
    // Read-time defaults are not written: only what was posted, and the subject.
    for key in ["price_limit_range_lower_bound", "price_limit_range_upper_bound", "quote_amount_limited", "quote_amount_limit", "sell_price_limited"] {
        assert!(s.get(key).is_none(), "{key} written: {s}");
    }
    assert_eq!(row["transient"]["price_limit_enabled_at"], json!("2026-09-10T12:00:30.123Z"));
    assert!(row["transient"].get("price_limit_condition_met_at").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn a_spending_cap_saves_the_same_row_on_the_page_mcp_and_rest() {
    let (saved, row, _) = on_every_surface([0.5, 0.3, 0.2], |_| {},
        |_| json!({ "quote_amount_limited": "1", "quote_amount_limit": "300" }),
        |_| json!({ "quote_amount_limited": true, "quote_amount_limit": 300 })).await;
    assert_eq!(saved[0].status, 200, "{}", saved[0].answer);
    assert_eq!(mcp_updated(&saved[1]), "Bot 'Index' settings updated: quote_amount_limited, quote_amount_limit.");
    assert_eq!((&row["settings"]["quote_amount_limited"], &row["settings"]["quote_amount_limit"]), (&json!(true), &json!(300.0)));
    assert_eq!(row["transient"]["quote_amount_limit_enabled_at"], json!("2026-09-10T12:00:30.123Z"));
}

/// D-IDX-SUBJECT-DEFAULT: switched on with no subject posted, the rule watches the member with the highest target; on equal
/// targets the first in the index's order; with no member rows the first of the index's coins listed here.
#[tokio::test(flavor = "current_thread")]
async fn switched_on_without_a_subject_the_rule_watches_the_default_member() {
    for (targets, expected) in [([0.2, 0.5, 0.3], 1), ([0.4, 0.4, 0.2], 0)] {
        let (_, row, i) = on_every_surface(targets, |_| {}, |_| json!({ "price_limited": "1" }), |_| json!({ "price_limited": true })).await;
        assert_eq!(row["settings"]["price_limit_in_ticker_id"], json!(i.tickers[expected]), "{targets:?}");
    }
    // No member rows yet (before the first composition): the first of the index's coins listed here.
    let i = install([0.0, 0.0, 0.0]);
    let saved = save(&i, Surface::Mcp, json!({}), json!({ "price_limited": true })).await;
    assert_eq!(row(&i)["settings"]["price_limit_in_ticker_id"], json!(i.tickers[0]), "{}", saved.answer);
}

#[tokio::test(flavor = "current_thread")]
async fn with_no_member_to_watch_the_rule_is_refused() {
    let i = install([0.0, 0.0, 0.0]);
    i.c.execute("UPDATE tickers SET available = 0 WHERE id IN (?1, ?2, ?3)", (i.tickers[0], i.tickers[1], i.tickers[2])).unwrap();
    let before = row(&i);
    let saved = save(&i, Surface::Mcp, json!({}), json!({ "price_limited": true })).await;
    assert_eq!(saved.answer["error"], true);
    assert!(mcp_updated(&saved).contains("no member to watch yet"), "{}", saved.answer);
    assert_eq!(row(&i), before, "nothing written");
}

/// A posted subject is one of the members, or the subject the rule watches now (a former member).
#[tokio::test(flavor = "current_thread")]
async fn a_subject_outside_the_members_is_refused() {
    let i = install([0.5, 0.5, 0.0]);
    let before = row(&i);
    let saved = save(&i, Surface::Mcp, json!({}), json!({ "price_limited": true, "price_limit_in_ticker_id": i.tickers[2] })).await;
    assert_eq!(saved.answer["error"], true, "{}", saved.answer);
    assert!(mcp_updated(&saved).contains("This condition must watch an asset the bot holds."), "{}", saved.answer);
    assert_eq!(row(&i), before);
    // The departed subject it watches now is kept on a save.
    i.c.execute("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit_in_ticker_id', ?1) WHERE id = ?2", (i.tickers[2], i.id)).unwrap();
    let saved = save(&i, Surface::Mcp, json!({}), json!({ "price_limit": 99, "price_limit_in_ticker_id": i.tickers[2] })).await;
    assert_ne!(saved.answer["error"], true, "{}", saved.answer);
}

/// D-SUBJECT-CHANGE-CLEARS-LATCH: a new subject clears the met_at the old one latched; a save that keeps it does not.
#[tokio::test(flavor = "current_thread")]
async fn a_subject_change_clears_the_latch() {
    let latched = |i: &Install| {
        i.c.execute("UPDATE bots SET settings = json_set(settings, '$.price_limited', json('true'), '$.price_limit_timing_condition', 'after', '$.price_limit_in_ticker_id', ?1), \
                     transient_data = json_set(transient_data, '$.price_limit_condition_met_at', '2026-09-05T00:00:00.000Z') WHERE id = ?2", (i.tickers[0], i.id)).unwrap();
    };
    let (_, row, _) = on_every_surface([0.5, 0.3, 0.2], latched, |i| json!({ "price_limit_in_ticker_id": i.tickers[1].to_string() }), |i| json!({ "price_limit_in_ticker_id": i.tickers[1] })).await;
    assert_eq!(row["transient"]["price_limit_condition_met_at"], Value::Null);
    let (_, row, _) = on_every_surface([0.5, 0.3, 0.2], latched, |_| json!({ "price_limit": "99" }), |_| json!({ "price_limit": 99 })).await;
    assert_eq!(row["transient"]["price_limit_condition_met_at"], json!("2026-09-05T00:00:00.000Z"));
}

/// What an index bot has nothing to act on (its sell side, a flip) is refused by name over MCP and REST, as unsupported_setting;
/// a rule this build does not run on it yet is the write guard's refusal. Nothing is written.
#[tokio::test(flavor = "current_thread")]
async fn the_api_refuses_what_an_index_bot_cannot_run() {
    for (fields, code, status, text) in [
        (json!({ "sell_price_limited": true }), "unsupported_setting", 422, "sell_price_limited applies to basket bots only."),
        (json!({ "price_limit_mode": "flip" }), "unsupported_setting", 422, "flip applies to basket bots only."),
        (json!({ "price_drop_limited": true }), "write_refused", 409, "This app can't run that yet: price_drop_limited on an index bot"),
    ] {
        for surface in [Surface::Mcp, Surface::Rest] {
            let i = install([0.5, 0.3, 0.2]);
            let before = row(&i);
            let saved = save(&i, surface, json!({}), fields.clone()).await;
            match surface {
                Surface::Mcp => assert_eq!((mcp_updated(&saved).as_str(), &saved.answer["error"]), (text, &json!(true)), "{fields}"),
                _ => assert_eq!((saved.status, saved.answer["error"]["code"].as_str(), saved.answer["error"]["message"].as_str()), (status, Some(code), Some(text)), "{fields}"),
            }
            assert_eq!(row(&i), before, "{fields}: nothing written");
        }
    }
}

/// The page posts no flip for an index bot; one sent anyway is refused, and its sell side is not a setting it takes.
#[tokio::test(flavor = "current_thread")]
async fn the_page_refuses_a_flip_and_ignores_the_sell_side() {
    let i = install([0.5, 0.3, 0.2]);
    let before = row(&i);
    let saved = save(&i, Surface::Web, json!({ "price_limited": "1", "price_limit_mode": "flip" }), json!({})).await;
    assert_eq!(saved.status, 422, "{}", saved.answer);
    assert!(saved.answer.as_str().unwrap().to_lowercase().contains("flip applies to basket bots only."), "{}", saved.answer);
    assert_eq!(row(&i), before);
    let saved = save(&i, Surface::Web, json!({ "sell_price_limited": "1" }), json!({})).await;
    assert_eq!(saved.status, 200);
    assert!(row(&i)["settings"].get("sell_price_limited").is_none(), "not a setting an index bot's page takes");
}

/// Inventory row 2: a rebalancing index bot with a price rule is no bot "with work only the full app handles": it saves.
#[tokio::test(flavor = "current_thread")]
async fn a_rebalancing_index_bot_with_a_price_rule_saves() {
    let rebalancing = |i: &Install| { i.c.execute("UPDATE bots SET settings = json_set(settings, '$.rebalance_enabled', json('true'), '$.rebalance_threshold', 0.05) WHERE id = ?1", [i.id]).unwrap(); };
    let (saved, row, _) = on_every_surface([0.5, 0.3, 0.2], rebalancing,
        |_| json!({ "price_limited": "1", "price_limit": "150" }), |_| json!({ "price_limited": true, "price_limit": 150 })).await;
    assert_eq!(saved[0].status, 200, "{}", saved[0].answer);
    assert_ne!(saved[1].answer["error"], true, "{}", saved[1].answer);
    assert_eq!(saved[2].status, 200, "{}", saved[2].answer);
    assert_eq!((&row["settings"]["price_limited"], &row["settings"]["rebalance_enabled"]), (&json!(true), &json!(true)));
}
