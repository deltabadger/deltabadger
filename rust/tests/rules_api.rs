//! A1: real REST/MCP entry points against every X0 API cell, with narrow W2 divergences.
mod common;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use common::{seed, web::TestClock};
use deltabadger::web;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

const AT: &str = "2026-10-01T12:00:00Z";
const ADDRESS: &str = "bc1qx0destination0000000000000000000000";
const TOOLS: [&str; 6] = [
    "list_rules",
    "create_rule",
    "update_rule_settings",
    "start_rule",
    "stop_rule",
    "delete_rule",
];
fn vectors() -> Value {
    serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap()
}
fn settings() -> Value {
    json!({"withdrawal_percentage":"100","threshold_type":"fee_percentage","max_fee_percentage":"1"})
}
struct Install {
    _dir: tempfile::TempDir,
    app: web::App,
    c: Connection,
    s: seed::Seeded,
}
impl Install {
    fn new() -> Self {
        let dir = common::rails_install();
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        let s = seed::seed_kraken(&c, &seed::cipher());
        c.execute("UPDATE users SET confirmed_at=created_at", [])
            .unwrap();
        c.execute(
            "UPDATE exchanges SET type='Exchanges::Binance',name='Binance'",
            [],
        )
        .unwrap();
        c.execute("UPDATE tickers SET base='BTC'", []).unwrap();
        c.execute("UPDATE api_keys SET key_type=1", []).unwrap();
        for (class, name) in [
            ("Kraken", "Kraken"),
            ("Gemini", "Gemini"),
            ("Mexc", "MEXC"),
            ("Kucoin", "KuCoin"),
        ] {
            c.execute("INSERT INTO exchanges(type,name,maker_fee,taker_fee,created_at,updated_at) VALUES (?1,?2,'0.1','0.1',?3,?3)", (format!("Exchanges::{class}"),name,AT)).unwrap();
        }
        c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('A1','a1','http://localhost/cb',0,'mcp api',?1,?1)", [AT]).unwrap();
        let application = c.last_insert_rowid();
        c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,?2,'a1-token','mcp api',?3,86400)", (application,s.user_id,AT)).unwrap();
        c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (?1,?2,?3,?3,?4,?4)", (s.user_id,application,json!(TOOLS).to_string(),AT)).unwrap();
        let permissions = json!({"tool_permissions":TOOLS.iter().map(|t|(t.to_string(),json!(true))).collect::<serde_json::Map<_,_>>(),"dry_run":false});
        c.execute(
            "UPDATE users SET mcp_settings=?1,rest_settings=?1",
            [permissions.to_string()],
        )
        .unwrap();
        let app = common::web::app(dir.path(), "engine-test-secret", TestClock::at(AT));
        Self {
            _dir: dir,
            app,
            c,
            s,
        }
    }
    fn rule(&self, status: i64, exchange: i64, settings: Value) {
        self.c.execute("INSERT INTO rules(user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES (?1,'Rules::Withdrawal',?2,?3,?4,?5,?6,?7,?7)",params![self.s.user_id,status,exchange,self.s.btc,self.app.cipher.encrypt(ADDRESS),settings.to_string(),AT]).unwrap();
    }
    fn setup(&self, name: &str) {
        if name == "list_each_status" {
            for n in 0..4 {
                let mut s = settings();
                if n == 3 {
                    s["network"] = json!("BTC");
                }
                self.rule(n, n + 1, s);
            }
        } else if name == "create_exists" {
            self.rule(2, 1, settings());
        } else if name == "create_revives_deleted" {
            let mut s = settings();
            s["max_interval"] = json!("7");
            s["address_tag"] = json!("old");
            self.rule(3, 1, s);
        } else if !name.starts_with("create_")
            && !name.starts_with("list_")
            && name != "start_missing"
        {
            let status = match name.rsplit('_').next().unwrap() {
                "created" => 0,
                "scheduled" => 1,
                "deleted" => 3,
                _ => 2,
            };
            self.rule(status, 1, settings());
        }
    }
    fn rows(&self) -> Value {
        let rows=self.c.prepare("SELECT id,status,exchange_id,asset_id,address,settings FROM rules ORDER BY id").unwrap().query_map([],|r| {
            let address:String=r.get(4)?;let settings:String=r.get(5)?;
            Ok(json!({"id":r.get::<_,i64>(0)?,"type":"Rules::Withdrawal","status":(["created","scheduled","stopped","deleted"][r.get::<_,usize>(1)?]),"exchange_id":r.get::<_,i64>(2)?,"asset_id":r.get::<_,i64>(3)?,"address":self.app.cipher.decrypt(&address).unwrap(),"settings":serde_json::from_str::<Value>(&settings).unwrap()}))
        }).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
        json!(rows)
    }
    async fn send(&self, req: Request<Body>) -> (u16, Value) {
        let r = web::router(self.app.clone()).oneshot(req).await.unwrap();
        let status = r.status().as_u16();
        let bytes = to_bytes(r.into_body(), 8_000_000).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))),
        )
    }
    async fn rest(&self, tool: &str, id: i64, args: Value) -> (u16, Value) {
        let (method, path) = match tool {
            "list_rules" => ("GET", "/api/v1/rules".into()),
            "create_rule" => ("POST", "/api/v1/rules".into()),
            "update_rule_settings" => ("PATCH", format!("/api/v1/rules/{id}")),
            "delete_rule" => ("DELETE", format!("/api/v1/rules/{id}")),
            "start_rule" => ("POST", format!("/api/v1/rules/{id}/start")),
            "stop_rule" => ("POST", format!("/api/v1/rules/{id}/stop")),
            _ => panic!("{tool}"),
        };
        self.send(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "localhost:3000")
                .header("authorization", "Bearer a1-token")
                .header("content-type", "application/json")
                .body(Body::from(args.to_string()))
                .unwrap(),
        )
        .await
    }
    async fn mcp(&self, tool: &str, id: i64, mut args: Value) -> (u16, Value) {
        let request = |body: Value, session: Option<&str>| {
            let mut r = Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost:3000")
                .header("authorization", "Bearer a1-token")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream");
            if let Some(s) = session {
                r = r
                    .header("mcp-session-id", s)
                    .header("mcp-protocol-version", "2025-11-25");
            }
            r.body(Body::from(body.to_string())).unwrap()
        };
        let r=web::router(self.app.clone()).oneshot(request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"A1","version":"1"}}}),None)).await.unwrap();
        let session = r.headers()["mcp-session-id"].to_str().unwrap().to_string();
        let r = web::router(self.app.clone())
            .oneshot(request(
                json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), 202);
        if !matches!(tool, "list_rules" | "create_rule") {
            args["rule_id"] = json!(id);
        }
        self.send(request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":tool,"arguments":args}}),Some(&session))).await
    }
    async fn web(&self, tool: &str, id: i64, args: &Value) -> (u16, Value) {
        let token = web::csrf::new_token();
        let create = tool == "create_rule";
        let text = web::rules::model::string;
        let exchange = self
            .c
            .query_row(
                "SELECT id FROM exchanges WHERE LOWER(name)=LOWER(?1)",
                [args["exchange_name"].as_str().unwrap_or_default()],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .unwrap()
            .unwrap_or(999);
        let asset = if args["asset"]
            .as_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("BTC"))
        {
            self.s.btc
        } else {
            999
        };
        let mut config = json!({"exchange_id":exchange.to_string(),"asset_id":asset.to_string(),"address":args["address"].as_str().unwrap_or_default()});
        if create {
            for (k, v) in args.as_object().unwrap() {
                config[k] = json!(text(v));
            }
        }
        let data = web::session::SessionData {
            user: Some((self.s.user_id, "x".into())),
            csrf: Some(token.clone()),
            withdrawal_rule_config: create.then_some(config),
            ..Default::default()
        };
        let cookie = format!(
            "{}={}",
            web::session::COOKIE,
            web::session::seal(&self.app.keys.session, &data, self.app.now())
        );
        let (method, path) = if tool == "list_rules" {
            ("GET", "/rules".into())
        } else if create {
            ("POST", "/rules/withdrawals/confirm_settings".into())
        } else {
            (
                if tool == "delete_rule" {
                    "DELETE"
                } else {
                    "PATCH"
                },
                format!("/rules/withdrawals/{id}"),
            )
        };
        let mut fields = vec![];
        for (k, v) in args.as_object().unwrap() {
            fields.push((
                if create {
                    k.clone()
                } else {
                    format!("rules_withdrawal[{k}]")
                },
                text(v),
            ));
        }
        if matches!(tool, "start_rule" | "stop_rule") {
            fields.push((
                "rules_withdrawal[status]".into(),
                if tool == "start_rule" {
                    "scheduled"
                } else {
                    "stopped"
                }
                .into(),
            ));
        }
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        self.send(
            Request::builder()
                .method(method)
                .uri(path)
                .header("host", "localhost:3000")
                .header("cookie", cookie)
                .header("x-csrf-token", web::csrf::masked(&token))
                .header("content-type", "application/x-www-form-urlencoded")
                .header("accept", "text/vnd.turbo-stream.html")
                .body(Body::from(form))
                .unwrap(),
        )
        .await
    }
}

// Exact old and new messages: the adapter uses W2's typed refusal unchanged (D-RULE-ONE-MATRIX).
fn refusal(name: &str) -> Option<(u16, &'static str, &'static str, &'static str)> {
    Some(match name {
        "create_missing"=>(422,"missing_required_parameter","Missing required parameter(s): asset, address.","exchange_name, asset and address are required."),
        "create_unknown_exchange"=>(404,"exchange_not_found","Exchange 'Nowhere' not found. Available: Binance, Kraken, Gemini, MEXC, KuCoin","Exchange not found."),
        "create_unsupported_venue"=>(422,"withdrawal_unsupported","KuCoin does not support withdrawals here.","This exchange does not support withdrawals here."),
        "create_unlisted_asset"=>(404,"asset_not_found","Asset 'ETH' is not listed on Binance.","Asset is not listed on this exchange."),
        "create_exists"=>(409,"rule_exists","A withdrawal rule for BTC on Binance already exists (#1).","A withdrawal rule for this asset and exchange already exists."),
        "create_address_not_listed"|"create_listing_failed"=>(422,"address_not_listed","Address is not in Binance's withdrawal allow-list for BTC. Add it on the exchange first.","That address is not on the exchange's withdrawal allowlist"),
        "start_scheduled"=>(409,"rule_already_active","Rule #1 is already active (scheduled).","The withdrawal rule is already active."),
        "stop_created"=>(409,"rule_not_active","Rule #1 is not active (created).","The withdrawal rule is not active."),
        "stop_stopped"=>(409,"rule_not_active","Rule #1 is not active (stopped).","The withdrawal rule is not active."),
        "delete_scheduled"=>(409,"rule_active","Rule must be stopped before deleting. Current status: scheduled.","Stop the withdrawal rule before deleting it."),
        "update_scheduled"=>(409,"rule_active","Rule must be stopped before updating settings. Current status: scheduled.","Stop the withdrawal rule before updating it."),
        _=>return None,
    })
}
fn error(surface: &str, code: &str, message: &str) -> Value {
    if surface == "rest" {
        json!({"data":null,"error":{"code":code,"message":message}})
    } else {
        json!({"jsonrpc":"2.0","id":2,"result":{"isError":true,"content":[{"type":"text","text":message}]}})
    }
}
fn expected(cell: &Value, before: &Value) -> (u16, Value, Value) {
    let name = cell["case"].as_str().unwrap();
    let surface = cell["surface"].as_str().unwrap();
    let mut body = cell["body"].clone();
    let mut status = cell["status"].as_u64().unwrap() as u16;
    let mut rows = cell["rules"].clone();
    if let Some((http, code, old, new)) = refusal(name) {
        if !(name == "create_missing" && surface == "mcp") {
            assert_eq!(
                body,
                error(surface, code, old),
                "D-RULE-ONE-MATRIX recorded {name}"
            );
            status = if surface == "rest" { http } else { 200 };
            body = error(surface, code, new);
        }
    }
    if matches!(name, "start_created" | "start_stopped") {
        assert_eq!(cell["jobs"], json!(["Rule::EvaluateAllJob"]));
        assert_eq!(rows[0]["status"], "scheduled");
        if surface == "rest" {
            assert_eq!(
                body,
                json!({"data":{"id":1,"status":"scheduled"},"error":null})
            );
        } else {
            assert_eq!(body["result"]["content"][0]["text"], "Rule #1 started.");
        }
    }
    if matches!(name, "create_listed" | "create_invalid_network") {
        // D-RULE-LISTED-ENTRY: free-form tags and networks outside the listed entry are refused.
        if name == "create_invalid_network" {
            assert_eq!(
                body,
                error(
                    surface,
                    "invalid_network",
                    "Binance lists no networks for BTC; omit network."
                )
            );
        } else if surface == "rest" {
            assert_eq!(status, 201);
            assert_eq!(body["data"]["address_tag"], "free-form");
        } else {
            assert_eq!(body["result"]["content"][0]["text"], "Rule #1 created (stopped): 50.0% of BTC on Binance to bc1qx0…0000. Use 'start_rule' to activate.");
        }
        status = if surface == "rest" { 422 } else { 200 };
        body = error(
            surface,
            "address_not_listed",
            "That address is not on the exchange's withdrawal allowlist",
        );
        rows = before.clone();
    }
    if matches!(
        name,
        "create_min_amount"
            | "create_revives_deleted"
            | "update_created"
            | "update_stopped"
            | "update_all_four"
            | "start_created"
            | "start_stopped"
    ) {
        // W2's X0 replay supplies V1's required complete listing (start rechecks it too); persist its exact network.
        assert!(rows[0]["settings"].get("network").is_none());
        rows[0]["settings"]["network"] = json!("BTC"); // D-RULE-LISTED-ENTRY
        if name == "create_revives_deleted" {
            assert_eq!(rows[0]["settings"]["threshold_type"], "min_amount");
            rows[0]["settings"]["threshold_type"] = json!("fee_percentage"); // D-RULE-ONE-MATRIX
        }
        if name.starts_with("create_") && surface == "rest" {
            assert!(body["data"]["network"].is_null());
            body["data"]["network"] = json!("BTC");
            body["data"]["max_interval"] = Value::Null;
            body["data"]["unresolved_withdrawal"] = Value::Null;
            if name == "create_revives_deleted" {
                body["data"]["threshold_type"] = json!("fee_percentage");
            }
        }
    }
    if name == "list_each_status" && surface == "rest" {
        // D-RULE-ONE-MATRIX and D-RULE-UNRESOLVED-VISIBLE: additive fields only.
        for row in body["data"]["rules"].as_array_mut().unwrap() {
            row["max_interval"] = Value::Null;
            row["unresolved_withdrawal"] = Value::Null;
        }
    }
    (status, body, canonical(rows))
}
fn canonical(mut rows: Value) -> Value {
    // Rails store_accessors read absent and JSON null alike (W2's X0 row comparison).
    for row in rows.as_array_mut().unwrap() {
        row["settings"]
            .as_object_mut()
            .unwrap()
            .retain(|_, v| !v.is_null());
    }
    rows
}
fn needs_reads(name: &str) -> bool {
    matches!(
        name,
        "create_address_not_listed"
            | "create_listing_failed"
            | "create_listed"
            | "create_min_amount"
            | "create_revives_deleted"
            | "create_invalid_network"
            | "update_created"
            | "update_stopped"
            | "update_all_four"
            | "start_created"
            | "start_stopped"
    )
}
async fn replay(reads: bool) {
    let server = if reads {
        Some(MockServer::start().await)
    } else {
        None
    };
    let mut count = 0;
    for cell in vectors()["api"].as_array().unwrap() {
        let name = cell["case"].as_str().unwrap();
        if needs_reads(name) != reads {
            continue;
        }
        let mut i = Install::new();
        i.setup(name);
        if let Some(server) = &server {
            server.reset().await;
            i.app = i
                .app
                .with_venue_key_url("Exchanges::Binance", &server.uri())
                .unwrap();
            if name != "create_listing_failed" {
                listed(server).await;
            }
        }
        let before = i.rows();
        let want = expected(cell, &before);
        let tool = cell["tool"].as_str().unwrap();
        let id = if before.as_array().unwrap().is_empty() {
            999
        } else {
            1
        };
        let got = if cell["surface"] == "rest" {
            i.rest(tool, id, cell["arguments"].clone()).await
        } else {
            i.mcp(tool, id, cell["arguments"].clone()).await
        };
        assert_eq!(got, (want.0, want.1), "{name} {}", cell["surface"]);
        assert_eq!(canonical(i.rows()), want.2, "{name} rows");
        // Every X0 request also goes through the equivalent authenticated web entry point.
        let mut browser = Install::new();
        browser.setup(name);
        if let Some(server) = &server {
            browser.app = browser
                .app
                .with_venue_key_url("Exchanges::Binance", &server.uri())
                .unwrap();
        }
        let mut args = cell["arguments"].clone();
        if cell["surface"] == "mcp" {
            for field in ["withdrawal_percentage", "max_fee_percentage", "min_amount"] {
                if let Some(n) = args[field].as_f64() {
                    args[field] = json!(deltabadger::figures::num::float_to_s(n));
                }
            }
        }
        let web = browser.web(tool, id, &args).await;
        assert!(web.0 < 500, "{name} web: {web:?}");
        assert_eq!(
            canonical(browser.rows()),
            canonical(i.rows()),
            "{name} web/{}",
            cell["surface"]
        );
        // A start fans out one `start:` evaluation per scheduled rule (Rails' EvaluateAllJob); nothing else starts.
        let starts = i64::from(matches!(name, "start_created" | "start_stopped"));
        let evaluations = |sql: &str| i.c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(evaluations("SELECT count(*) FROM rust_rule_evaluations"), starts, "{name}: evaluations");
        assert_eq!(
            evaluations("SELECT count(*) FROM rust_rule_evaluations WHERE rule_id=1 AND source LIKE 'start:%' AND consumed_at IS NULL"),
            starts,
            "{name}: one pending start for rule 1"
        );
        assert_eq!(
            i.c.query_row("SELECT count(*) FROM rule_logs", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0,
            "{name}: no execution"
        );
        if let Some(server) = &server {
            assert!(
                server
                    .received_requests()
                    .await
                    .unwrap()
                    .iter()
                    .all(|r| r.method == "GET"),
                "never sends"
            );
        }
        count += 1;
    }
    assert_eq!(
        count,
        if reads { 11 * 2 } else { 24 * 2 },
        "35 X0 cells on each surface = 48 local + 22 venue-read cells (the starts re-list, D-RULE-ADDRESS-RECHECK)"
    );
}
#[tokio::test]
async fn x0_api_cells_without_venue_reads() {
    replay(false).await;
}
#[tokio::test]
async fn x0_api_cells_with_venue_reads() {
    replay(true).await;
}

#[tokio::test]
async fn every_rule_tool_requires_the_owners_permission_and_the_clients_grant() {
    for surface in ["rest", "mcp"] {
        for tool in TOOLS {
            for grant in [false, true] {
                let i = Install::new();
                i.rule(0, 1, settings());
                let before = i.rows();
                if grant {
                    i.c.execute(
                        "UPDATE connected_clients SET mcp_tools='[]',rest_tools='[]'",
                        [],
                    )
                    .unwrap();
                } else {
                    i.c.execute("UPDATE users SET mcp_settings='{\"tool_permissions\":{\"list_rules\":false}}',rest_settings='{}'",[]).unwrap();
                }
                let got = if surface == "rest" {
                    i.rest(tool, 1, json!({})).await
                } else {
                    i.mcp(tool, 1, json!({})).await
                };
                if surface == "rest" {
                    assert_eq!(
                        got,
                        (
                            403,
                            error(
                                "rest",
                                "tool_disabled",
                                &format!("Tool '{tool}' is disabled for this user.")
                            )
                        )
                    );
                } else {
                    assert_eq!(got.1["error"]["code"], -32602);
                }
                assert_eq!(i.rows(), before);
            }
        }
    }
}

#[tokio::test]
async fn mcp_paper_trading_refuses_start_and_other_users_rows_are_not_found() {
    let i = Install::new();
    i.rule(0, 1, settings());
    i.c.execute(
        "UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('true'))",
        [],
    )
    .unwrap();
    let got = i.mcp("start_rule", 1, json!({})).await;
    assert_eq!(got.1["result"]["isError"], true);
    assert!(got.1["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("[DRY RUN]"));
    i.c.execute(
        "UPDATE users SET mcp_settings=json_set(mcp_settings,'$.dry_run',json('false'))",
        [],
    )
    .unwrap();
    i.c.execute("INSERT INTO users(email,encrypted_password,name,created_at,updated_at) VALUES ('other@example.com','x','Other',?1,?1)",[AT]).unwrap();
    i.c.execute("UPDATE rules SET user_id=?1", [i.c.last_insert_rowid()])
        .unwrap();
    let before = i.rows();
    for tool in &TOOLS[2..] {
        assert_eq!(
            i.rest(tool, 1, json!({})).await,
            (404, error("rest", "rule_not_found", "Rule not found."))
        );
        assert_eq!(
            i.mcp(tool, 1, json!({})).await.1,
            error("mcp", "rule_not_found", "Rule not found.")
        );
    }
    assert_eq!(
        i.rest("list_rules", 0, json!({})).await.1,
        json!({"data":{"count":0,"rules":[]},"error":null})
    );
    assert_eq!(i.rows(), before);
}

#[tokio::test]
async fn lifecycle_requests_leave_the_same_rows_on_all_three_surfaces() {
    for (tool, status) in [
        ("start_rule", 0),
        ("start_rule", 2),
        ("stop_rule", 1),
        ("delete_rule", 0),
        ("delete_rule", 2),
        ("delete_rule", 1),
        ("update_rule_settings", 1),
    ] {
        let mut rows = vec![];
        for surface in ["rest", "mcp", "web"] {
            let i = Install::new();
            i.rule(status, 1, settings());
            let args = if tool == "update_rule_settings" {
                json!({"withdrawal_percentage":50.0})
            } else {
                json!({})
            };
            let answer = match surface {
                "rest" => i.rest(tool, 1, args).await,
                "mcp" => i.mcp(tool, 1, args).await,
                _ => i.web(tool, 1, &args).await,
            };
            assert!(answer.0 < 500, "{surface} {tool}: {answer:?}");
            rows.push(i.rows());
        }
        assert_eq!(rows[0], rows[1], "{tool} REST/MCP");
        assert_eq!(rows[0], rows[2], "{tool} REST/web");
    }
}

#[tokio::test]
async fn starts_on_unsupported_venues_refuse_without_scheduling() {
    // D-RULE-BINANCE-US-UNSUPPORTED: no address book, so no destination contract; every other Rails venue withdraws.
    for (class, name, code) in [("BinanceUs", "Binance.US", "withdrawal_unsupported")] {
        let i = Install::new();
        i.c.execute("DELETE FROM exchanges WHERE id<>1", [])
            .unwrap();
        i.c.execute(
            "UPDATE exchanges SET type=?1,name=?2 WHERE id=1",
            (format!("Exchanges::{class}"), name),
        )
        .unwrap();
        i.rule(0, 1, settings());
        let before = i.rows();
        let rest = i.rest("start_rule", 1, json!({})).await;
        assert_eq!(rest.0, 422);
        assert_eq!(rest.1["error"]["code"], code);
        let message = rest.1["error"]["message"].as_str().unwrap();
        assert_eq!(
            i.mcp("start_rule", 1, json!({})).await.1,
            error("mcp", code, message)
        );
        assert_eq!(i.web("start_rule", 1, &json!({})).await.0, 422);
        assert_eq!(i.rows(), before);
        assert_eq!(
            i.c.query_row("SELECT count(*) FROM rust_rule_evaluations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

fn intent(i: &Install, state: &str) {
    i.c.execute("INSERT INTO rust_withdrawal_intents(owner,state,client_id,created_at,api_key_id,credential_digest,host,account_scope,key_identity,params_enc,fee_basis,budget,free_balance,reason) VALUES ('rule:1',?1,'test',?2,1,'digest','host','spot','key','encrypted','flat','1','1','reply lost')",(state,AT)).unwrap();
}
#[tokio::test]
async fn unresolved_withdrawal_is_visible_masked_and_fences_writes_but_not_stop() {
    for state in ["dispatched", "unresolved"] {
        let i = Install::new();
        i.rule(1, 1, settings());
        intent(&i, state);
        i.c.execute("INSERT INTO rust_rule_evaluations(rule_id,source,created_at) VALUES (1,'start:test',?1)",[AT]).unwrap();
        let list = i.rest("list_rules", 0, json!({})).await;
        assert_eq!(list.0, 200);
        assert_eq!(list.1["data"]["rules"][0]["address"], "bc1qx0…0000");
        assert_eq!(
            list.1["data"]["rules"][0]["unresolved_withdrawal"],
            json!({"state":state,"reason":"reply lost","since":AT,"candidates":[]})
        );
        let mcp = i.mcp("list_rules", 0, json!({})).await;
        let text = mcp.1["result"]["content"][0]["text"].as_str().unwrap();
        assert!(
            text.contains("unresolved_withdrawal:")
                && text.contains(state)
                && text.contains("reply lost")
        );
        assert!(!text.contains(ADDRESS));
        for tool in ["start_rule", "delete_rule", "update_rule_settings"] {
            assert_eq!(i.rest(tool,1,json!({})).await,(409,error("rest","withdrawal_unconfirmed","The withdrawal status is unconfirmed. Resolve it before changing this rule.")));
            assert_eq!(
                i.mcp(tool, 1, json!({})).await.1,
                error(
                    "mcp",
                    "withdrawal_unconfirmed",
                    "The withdrawal status is unconfirmed. Resolve it before changing this rule."
                )
            );
        }
        assert_eq!(i.rest("stop_rule", 1, json!({})).await.0, 200);
        assert_eq!(
            i.c.query_row("SELECT outcome FROM rust_rule_evaluations", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "skipped"
        );
        assert_eq!(
            i.c.query_row("SELECT count(*) FROM rust_withdrawal_intents", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

/// A complete V1 destination, unlike the X0 API's address-only listing fixture.
async fn listed(server: &MockServer) {
    Mock::given(matchers::path("/sapi/v1/capital/withdraw/address/list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"coin":"BTC","address":ADDRESS,"name":"cold","network":"BTC","addressTag":""}
        ])))
        .mount(server)
        .await;
    Mock::given(matchers::path("/sapi/v1/capital/config/getall")).respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"coin":"BTC","networkList":[{"network":"BTC","isDefault":true,"withdrawEnable":true,"withdrawTag":false,"withdrawFee":"0.0001","withdrawIntegerMultiple":"0.00000001"}]}
    ]))).mount(server).await;
}
#[tokio::test]
async fn create_update_delete_revive_use_the_same_matrix_on_all_three_surfaces() {
    let server = MockServer::start().await;
    listed(&server).await;
    let create = json!({"exchange_name":"Binance","asset":"BTC","address":ADDRESS,"network":"BTC","withdrawal_percentage":25.0,"threshold_type":"min_amount","min_amount":0.5,"max_fee_percentage":2.0,"max_interval":7});
    let update = json!({"withdrawal_percentage":50.0,"min_amount":0.8,"max_interval":3,"address":ADDRESS,"network":"BTC"});
    let mut results = vec![];
    for surface in ["rest", "mcp", "web"] {
        let mut i = Install::new();
        i.app = i
            .app
            .with_venue_key_url("Exchanges::Binance", &server.uri())
            .unwrap();
        let mut snapshots = vec![];
        for (tool, args) in [
            ("create_rule", create.clone()),
            ("update_rule_settings", update.clone()),
            (
                "update_rule_settings",
                json!({"address":"","max_interval":null}),
            ),
            ("delete_rule", json!({})),
            ("create_rule", create.clone()),
        ] {
            let got = match surface {
                "rest" => i.rest(tool, 1, args).await,
                "mcp" => i.mcp(tool, 1, args).await,
                _ => i.web(tool, 1, &args).await,
            };
            assert_eq!(
                got.0,
                if surface == "rest" && tool == "create_rule" {
                    201
                } else {
                    200
                },
                "{surface} {tool}: {got:?}"
            );
            if surface == "mcp" {
                assert!(got.1["result"].get("isError").is_none(), "{got:?}");
            }
            let rows = i.rows();
            assert_eq!(rows.as_array().unwrap().len(), 1);
            snapshots.push(rows);
        }
        results.push(snapshots);
    }
    assert_eq!(results[0], results[1], "REST/MCP");
    assert_eq!(results[0], results[2], "REST/web");
    assert_eq!(results[0][0][0]["status"], "created");
    assert_eq!(results[0][1][0]["settings"]["max_interval"], "3");
    assert!(results[0][2][0]["settings"]["max_interval"].is_null());
    assert_eq!(results[0][2][0]["address"], ADDRESS);
    assert_eq!(results[0][3][0]["status"], "deleted");
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|r| r.method == "GET"));
}

/// A destination change on the API carries the whole destination, as the web picker does: moving a rule whose
/// destination has a memo to a listed destination without one clears the memo (review finding on A1).
#[tokio::test]
async fn an_api_destination_change_clears_the_previous_memo() {
    const PLAIN: &str = "bc1qx0destination1111111111111111111111";
    let server = MockServer::start().await;
    Mock::given(matchers::path("/sapi/v1/capital/withdraw/address/list"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"coin":"BTC","address":ADDRESS,"name":"memo","network":"BNB","addressTag":"m1"},
            {"coin":"BTC","address":PLAIN,"name":"plain","network":"BTC","addressTag":""}
        ])))
        .mount(&server).await;
    Mock::given(matchers::path("/sapi/v1/capital/config/getall")).respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"coin":"BTC","networkList":[
            {"network":"BTC","isDefault":true,"withdrawEnable":true,"withdrawTag":false,"withdrawFee":"0.0001","withdrawIntegerMultiple":"0.00000001"},
            {"network":"BNB","isDefault":false,"withdrawEnable":true,"withdrawTag":true,"withdrawFee":"0.00001","withdrawIntegerMultiple":"0.00000001"}]}
    ]))).mount(&server).await;
    for surface in ["rest", "mcp"] {
        let mut i = Install::new();
        i.app = i.app.with_venue_key_url("Exchanges::Binance", &server.uri()).unwrap();
        let create = json!({"exchange_name":"Binance","asset":"BTC","address":ADDRESS,"network":"BNB","address_tag":"m1","withdrawal_percentage":25.0,"threshold_type":"min_amount","min_amount":0.5,"max_fee_percentage":2.0});
        let update = json!({"address":PLAIN,"network":"BTC","address_tag":""});
        for (tool, args) in [("create_rule", create), ("update_rule_settings", update)] {
            let got = if surface == "rest" { i.rest(tool, 1, args).await } else { i.mcp(tool, 1, args).await };
            assert!(got.0 < 300 && got.1["result"].get("isError").is_none(), "{surface} {tool}: {got:?}");
        }
        let row = &i.rows()[0];
        assert_eq!(row["address"], PLAIN, "{surface}");
        assert!(crate_blank(&row["settings"]["address_tag"]), "{surface}: the old memo survived: {row}");
    }
}
fn crate_blank(v: &Value) -> bool { v.is_null() || v.as_str().is_some_and(str::is_empty) }
