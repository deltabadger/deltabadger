//! Web action and authentication tests that require a Rails-prepared install.
mod common;

#[test]
fn settings_metrics_panel_prices_holdings_and_uses_current_members_without_candles() {
    use common::seed;
    use deltabadger::figures::{at::At, scripted::Scripted};
    use serde_json::json;
    let (_dir, opened, seeded) = common::install_alpaca();
    let c = &opened.primary;
    let (stock, ticker) = seed::add_alpaca_stock(c, &seeded, "QQQM");
    let id = seed::insert_bot(c, &seeded, &seed::BotSpec::weekly(50.0, "2026-09-09T12:00:00Z").weights(&[(stock, 1.0)]));
    seed::insert_member(c, id, stock, ticker, 1.0, true, "2026-09-09T12:00:00Z", None);
    seed::insert_stock_tx(c, &seeded, id, stock, "QQQM", &seed::TxSpec {
        status: 0, external_status: Some(3), external_id: Some("metrics-fill".into()), order_type: 0,
        amount: Some("2"), quote_amount: Some("200"), price: Some("100"), amount_exec: Some("2"), quote_amount_exec: Some("200"),
        created_at: "2026-09-09T12:00:01Z".into(),
    });
    let market = Scripted::new(&json!({"GET data.alpaca.markets/v2/stocks/snapshots": {"status":200,"body":{"QQQM":{"latestTrade":{"p":125}}}}}), None);
    let now = At::from_sql("2026-09-10T12:00:30Z").unwrap();
    let render = || deltabadger::web::figure::metrics_panel(c, id, &market, now, "pl", "/pl").unwrap();
    let member = render();
    assert!(member.contains("250.00 <small>USD</small>"), "the holding is valued at the current 125 price, not its 100 fill");
    assert!(member.contains("+25.0%"));
    assert!(member.contains("id=\"assets_metrics_table\""));
    assert!(member.contains(&format!("href=\"/pl/bots/{id}/liquidation/new?")));
    assert!(!member.contains("loading-placeholder") && !member.contains("authenticity_token"));
    // A committed composition change moves this holding to the exited table without changing its value.
    let (other, other_ticker) = seed::add_alpaca_stock(c, &seeded, "NVDA");
    seed::insert_member(c, id, other, other_ticker, 1.0, true, "2026-09-10T12:00:30Z", None);
    c.execute("UPDATE bot_index_assets SET in_index=0 WHERE bot_id=?1 AND asset_id=?2", (id, stock)).unwrap();
    let exited = render();
    assert!(exited.contains("id=\"exited_metrics_table\""));
    assert!(!exited.contains("id=\"assets_metrics_table\""));
    assert!(exited.contains("250.00 <small>USD</small>"));
    assert!(market.gaps.borrow().is_empty());
    assert!(market.requests.borrow().iter().all(|request| request.contains("/v2/stocks/snapshots")), "the panel never requests candles");
}

/// The transport can be observed without installing any bot mutation handler.
mod action_transport {
    use super::common;
    use axum::{body::{Body, to_bytes}, extract::Request, response::{IntoResponse, Response}, Router};
    use deltabadger::web::{self, Params, bot::{Kind, action_params::{self, ActionParams}}};
    use serde_json::{json, Value};
    use std::{sync::Arc, time::Duration};
    use tower::ServiceExt;

    async fn probe(request: Request) -> Response {
        let Some(params) = request.extensions().get::<Arc<Params>>() else { panic!("entry supplies params") };
        let Some(action) = action_params::action(&params.route_path, request.method()) else {
            return axum::http::StatusCode::NOT_IMPLEMENTED.into_response();
        };
        if !action_params::format_allowed(action, &params.route_path, request.headers()) {
            return axum::http::StatusCode::NOT_ACCEPTABLE.into_response();
        }
        let parsed = match ActionParams::parse(params) { Ok(p) => p, Err(e) => return e.status().into_response() };
        let permitted = if action == action_params::Action::Update {
            match parsed.permitted(Kind::Basket) { Ok(p) => p, Err(e) => return e.status().into_response() }
        } else { Value::Null };
        let value = json!({"permitted": permitted, "merged": parsed.value(), "method": request.method().as_str(), "form": params.form,
            "json": params.json, "query": params.query, "locale": params.path_locale,
            "path": params.route_path});
        (axum::http::StatusCode::OK, value.to_string()).into_response()
    }

    struct Harness { _dir: tempfile::TempDir, app: web::App, token: String }
    impl Harness {
        fn new() -> Self {
            let (dir, opened, seeded) = common::install_alpaca();
            common::seed::insert_bot(&opened.primary, &seeded,
                &common::seed::BotSpec::weekly(60.0, "2026-01-01T00:00:00Z").weights(&[(seeded.btc, 1.0)]));
            assert!(opened.primary.execute("UPDATE users SET confirmed_at = created_at", []).is_ok());
            drop(opened);
            let app = common::web::app(dir.path(), common::web::SECRET, common::web::TestClock::at("2026-09-10T12:00:30Z"));
            Self { _dir: dir, app, token: web::csrf::new_token() }
        }
        async fn snapshot(&self) -> String {
            match self.app.db(|c| {
                let mut out = String::new();
                for table in ["bots", "bot_index_assets", "bot_activity_logs", "transactions", "api_keys", "users"] {
                    let mut stmt = c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
                    let columns = stmt.column_count();
                    let mut rows = stmt.query([])?;
                    while let Some(row) = rows.next()? {
                        for i in 0..columns { out.push_str(&format!("{:?}|", row.get_ref(i)?)); }
                    }
                }
                Ok(out)
            }).await { Ok(v) => v, Err(e) => panic!("snapshot: {e:?}") }
        }
        async fn send(&self, method: &str, path: &str, media: &str, body: Body) -> (u16, Value) {
            self.send_accept(method, path, media, body, "text/vnd.turbo-stream.html").await
        }
        async fn send_accept(&self, method: &str, path: &str, media: &str, body: Body, accept: &str) -> (u16, Value) {
            let before = self.snapshot().await;
            let router = web::router_with_routes(self.app.clone(), Router::new().fallback(probe), Duration::from_millis(20));
            let request = match Request::builder().method(method).uri(path).header("content-type", media).header("accept", accept).body(body) {
                Ok(r) => r, Err(e) => panic!("request: {e}")
            };
            let response = match router.oneshot(request).await { Ok(r) => r, Err(e) => match e {} };
            let status = response.status().as_u16();
            if status == 408 { assert_eq!(response.headers().get("connection").and_then(|v| v.to_str().ok()), Some("close")); }
            let bytes = match to_bytes(response.into_body(), 200_000).await { Ok(b) => b, Err(e) => panic!("body: {e}") };
            let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            assert_eq!(before, self.snapshot().await, "transport never writes primary rows");
            (status, value)
        }
        async fn live(&self, method: &str, path: &str, media: &str, body: &str, headers: &[(&str,&str)]) -> u16 {
            let before = self.snapshot().await;
            let user = match self.app.db(|c| Ok(c.query_row("SELECT id, encrypted_password FROM users ORDER BY id LIMIT 1", [], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,String>(1)?)))?)).await {
                Ok((id,hash)) => (id,hash.chars().take(29).collect()), Err(e) => panic!("user: {e:?}")
            };
            let session = web::session::SessionData { user: Some(user), csrf: Some(self.token.clone()), ..Default::default() };
            let cookie = web::session::seal(&self.app.keys.session, &session, self.app.now());
            // Implemented actions now perform writes. A missing id exercises the real transport
            // and ownership lookup while keeping this transport-only fixture read-only.
            let live_path = path.replace("/bots/1", "/bots/999");
            let mut request = Request::builder().method(method).uri(&live_path).header("host","localhost:3000")
                .header("cookie",format!("{}={cookie}",web::session::COOKIE)).header("content-type",media);
            for (key,value) in headers { request = request.header(*key,*value); }
            let request = match request.body(Body::from(body.replace("TOKEN", &web::csrf::masked(&self.token)))) { Ok(r) => r, Err(e) => panic!("request: {e}") };
            let response = match web::router(self.app.clone()).oneshot(request).await { Ok(r) => r, Err(e) => match e {} };
            let status = response.status().as_u16();
            assert_eq!(before,self.snapshot().await,"real routes do not write");
            status
        }
        async fn form(&self, method: &str, text: &str) -> (u16, Value) {
            self.send(method, "/bots/1", "application/x-www-form-urlencoded", Body::from(text.to_string())).await
        }
    }
    fn rails_vectors() {
        let vectors = common::vectors();
        let Some(cases) = vectors["action_transport"]["cases"].as_array() else { panic!("recorded transport cases") };
        let decode = |v: &Value| form_urlencoded::parse(v.as_str().unwrap_or("").as_bytes()).map(|(k,v)| (k.into_owned(),v.into_owned())).collect();
        for case in cases {
            if case["name"] == "invalid_json" { assert!(action_params::json(b"{").is_err()); continue; }
            let params = Params { upload: None, full_path: "/bots/1".into(), fullpath: "/bots/1".into(), route_path: "/bots/1".into(), path_locale: None,
                query: decode(&case["query"]), form: decode(&case["form"]), json: (!case["json"].is_null()).then(|| case["json"].clone()) };
            let parsed = ActionParams::parse(&params);
            if case.get("merged").is_none() {
                assert!(parsed.is_err(), "{case}"); assert_eq!(case["bounded_input_divergence"], true); continue;
            }
            let parsed = match parsed { Ok(p) => p, Err(e) => panic!("{}: {e:?}", case["name"]) };
            // Task 2 explicitly requires body precedence and nested merging. Installed Rails
            // instead shallow-merges query over body. Pin BOTH results; do not pretend parity.
            let divergence = match case["name"].as_str() {
                Some("query_body_precedence") => Some((json!({ROOT:{"label":"query","interval":"day"}}),json!({ROOT:{"label":"body","interval":"day"}}))),
                Some("nested_merge") => Some((json!({ROOT:{"allocations":{"2":"20","1":"80"}}}),json!({ROOT:{"allocations":{"2":"30","1":"80"}}}))),
                _ => None,
            };
            if let Some((rails, rust)) = divergence {
                assert_eq!(case["merged"],rails,"Rails request merge changed");
                assert_eq!(case["permitted"],rails[ROOT]);
                assert_eq!(parsed.value(),&rust,"plan_body_precedence: {}",case["name"]);
                assert_eq!(parsed.permitted(Kind::Basket),Ok(rust[ROOT].clone()));
            } else {
                assert_eq!(parsed.value(), &case["merged"], "{}", case["name"]);
                let permitted = parsed.permitted(Kind::Basket);
                if let Some(expected) = case.get("permitted") { assert_eq!(permitted, Ok(expected.clone()), "{}", case["name"]); }
                else { assert!(permitted.is_err(), "{case}"); }
            }
        }
        for (root, keys, kind) in [("bots_dca_multi_asset",action_params::BASKET_SCALARS,Kind::Basket),("bots_dca_index",action_params::INDEX_SCALARS,Kind::Index)] {
            assert_eq!(json!(keys), vectors["action_transport"]["allowlists"][root]);
            let fields: serde_json::Map<String,Value> = keys.iter().map(|k| (k.to_string(), json!("value"))).collect();
            let params = Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None,
                query: vec![], form: vec![], json: Some(json!({root:fields})) };
            let parsed = match ActionParams::parse(&params) { Ok(p) => p, Err(e) => panic!("{e:?}") };
            assert_eq!(parsed.permitted(kind), Ok(json!(fields)));
        }
        // The recorder invokes update_params, proving typed presence/in? differs from form strings.
        let case = |name: &str| cases.iter().find(|c| c["name"] == name).unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(case("typed_scalars")["updated"]["settings"]["smart_intervaled"], false);
        assert_eq!(case("string_scalars")["updated"]["settings"]["smart_intervaled"], true);
    }

    const ROOT: &str = "bots_dca_multi_asset";
    const VALID: &str = "bots_dca_multi_asset[quote_amount]=12.5";

    #[tokio::test]
    async fn native_patch_form() {
        let h = Harness::new(); let (status, got) = h.form("PATCH", VALID).await;
        assert_eq!(status, 200); assert_eq!(got["form"], json!([[format!("{ROOT}[quote_amount]"), "12.5"]]));
        assert_eq!(h.form("PATCH", "a=1&a[b]=2").await.0, 400);
    }
    #[tokio::test]
    async fn post_patch_override() {
        let h = Harness::new(); let (_, got) = h.form("POST", &format!("{VALID}&_method=patch")).await;
        assert_eq!(got["method"], "PATCH"); assert!(got["form"].as_array().is_some_and(|v| v.len() == 2));
        // A form Rack cannot parse names no method (Rack::MethodOverride logs and reads the header): it stays a POST,
        // which no bot route takes.
        assert_eq!(h.form("POST", "_method=patch&a=1&a[b]=2").await.0, 501);
    }
    /// Rack::MethodOverride's header turns a JSON POST into the action's method before its body is classified: the start
    /// keeps `start_fresh: false` and an update keeps its value (override_* pages scenarios record the Rails side).
    #[tokio::test]
    async fn the_override_header_keeps_a_json_body() {
        let h = Harness::new();
        for (path, body, method) in [("/bots/1/start", r#"{"start_fresh":false}"#, "PATCH"), ("/bots/1", r#"{"bots_dca_multi_asset":{"quote_amount":"37.25"}}"#, "PATCH")] {
            let router = web::router_with_routes(h.app.clone(), Router::new().fallback(probe), Duration::from_millis(200));
            let request = match Request::builder().method("POST").uri(path).header("content-type", "application/json")
                .header("accept", "text/vnd.turbo-stream.html").header("x-http-method-override", "PATCH").body(Body::from(body)) {
                Ok(r) => r, Err(e) => panic!("request: {e}")
            };
            let response = match router.oneshot(request).await { Ok(r) => r, Err(e) => match e {} };
            assert_eq!(response.status().as_u16(), 200, "{path}");
            let bytes = match to_bytes(response.into_body(), 200_000).await { Ok(b) => b, Err(e) => panic!("body: {e}") };
            let got: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
            assert_eq!(got["method"], method, "{path}");
            assert_eq!(got["merged"], serde_json::from_str::<Value>(body).unwrap(), "{path}: the JSON body was read");
        }
    }
    #[tokio::test]
    async fn native_method_not_overridden() {
        let h = Harness::new(); let (_, got) = h.form("PATCH", &format!("{VALID}&_method=delete")).await;
        assert_eq!(got["method"], "PATCH"); assert!(got["form"].as_array().is_some_and(|v| v.len() == 2));
        assert_eq!(h.form("PATCH", "_method=delete&a=1&a[b]=2").await.0, 400);
    }
    #[tokio::test]
    async fn native_patch_json() {
        let h = Harness::new(); let value = json!({ROOT: {"quote_amount":12.5,"smart_intervaled":true,"allocations":{"2":20,"1":80}}});
        let (status, got) = h.send("PATCH", "/bots/1", "application/json", Body::from(value.to_string())).await;
        assert_eq!(status, 200); assert_eq!(got["json"], value);
        assert_eq!(got["permitted"]["smart_intervaled"], true);
        for (flag, expected) in [(json!(true),Ok(true)),(json!(false),Ok(false)),(json!(1),Ok(true)),(json!(0),Ok(false)),(json!({}),Err(action_params::StartFlagError::Invalid)),(json!([]),Err(action_params::StartFlagError::Invalid)),(Value::Null,Ok(true)),(json!(1.0),Err(action_params::StartFlagError::Invalid))] {
            let params = Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None,
                query: vec![("start_fresh".into(),"true".into())], form: vec![], json: Some(json!({"start_fresh":flag})) };
            let parsed = match ActionParams::parse(&params) { Ok(p) => p, Err(e) => panic!("{e:?}") };
            assert_eq!(parsed.start_fresh(),expected);
        }
        assert_eq!(h.send("PATCH", "/bots/1", "application/json", Body::from("{")).await.0, 400);
    }
    #[tokio::test]
    async fn query_body_precedence() {
        let h = Harness::new(); let (status, got) = h.send("PATCH", "/bots/1?bots_dca_multi_asset[label]=query&bots_dca_multi_asset[interval]=day", "application/x-www-form-urlencoded", Body::from("bots_dca_multi_asset[label]=body")).await;
        assert_eq!(status, 200); assert_eq!(got["form"], json!([["bots_dca_multi_asset[label]","body"]]));
        assert_eq!(got["query"].as_array().map(Vec::len), Some(2));
        assert_eq!(got["permitted"], json!({"label":"body","interval":"day"}));
        assert_eq!(h.send("PATCH", "/bots/1?a=1&a[b]=2", "application/x-www-form-urlencoded", Body::from(VALID)).await.0, 400);
    }
    #[tokio::test]
    async fn checkbox_duplicate() {
        let h = Harness::new();
        for (a,b) in [("0","1"),("1","0")] {
            let (_, got) = h.form("PATCH", &format!("{ROOT}[smart_intervaled]={a}&{ROOT}[smart_intervaled]={b}")).await;
            assert_eq!(got["form"], json!([[format!("{ROOT}[smart_intervaled]"),a],[format!("{ROOT}[smart_intervaled]"),b]]));
            assert_eq!(got["permitted"]["smart_intervaled"], b);
        }
        assert_eq!(h.form("PATCH", "a[]=0&a[b]=1").await.0, 400);
    }
    #[tokio::test]
    async fn allocation_order() {
        let h = Harness::new(); let (_, got) = h.form("PATCH", &format!("{ROOT}[allocations][2]=20&{ROOT}[allocations][1]=80&{ROOT}[allocations][2]=30")).await;
        assert_eq!(got["form"].as_array().map(Vec::len), Some(3));
        assert_eq!(got["permitted"]["allocations"], json!({"2":"30","1":"80"}));
        assert_eq!(got["permitted"]["allocations"].as_object().map(|v| v.keys().map(String::as_str).collect::<Vec<_>>()), Some(vec!["2","1"]));
        assert_eq!(h.form("PATCH", &format!("{ROOT}[allocations]=x&{ROOT}[allocations][2]=20")).await.0, 400);
    }
    #[tokio::test]
    async fn missing_root() {
        let h = Harness::new(); assert_eq!(h.form("PATCH", VALID).await.0, 200);
        for body in ["", "bots_dca_index[quote_amount]=1", "bots_dca_multi_asset=", "bots_dca_multi_asset[]="] {
            assert_eq!(h.form("PATCH", body).await.0, 400, "{body}");
        }
    }
    #[tokio::test]
    async fn strong_scalar_shape() {
        let h = Harness::new();
        let value = json!({ROOT:{"label":["bad"],"quote_amount":{"x":1},"allocations":{"2":20}}});
        let (status, got) = h.send("PATCH", "/bots/1", "application/json", Body::from(value.to_string())).await;
        assert_eq!(status, 200); assert_eq!(got["permitted"], json!({"allocations":{"2":20}}));
        rails_vectors();
        assert_eq!(h.form("PATCH", "a[]=1&a[b]=2").await.0, 400);
    }
    #[tokio::test]
    async fn malformed_nested_shape() {
        let h = Harness::new(); assert_eq!(h.form("PATCH", VALID).await.0, 200);
        for body in ["a=1&a[b]=2", "a[]=1&a[b]=2", "a[b]=1&a[]=2"] { assert_eq!(h.form("PATCH", body).await.0, 400, "{body}"); }
        for body in ["{", "[]", "true"] { assert_eq!(h.send("PATCH", "/bots/1", "application/json", Body::from(body)).await.0, 400); }
    }
    #[tokio::test]
    async fn body_size() {
        let h = Harness::new(); let prefix = format!("{ROOT}[label]=");
        for (size,status) in [(web::FORM_LIMIT,200),(web::FORM_LIMIT+1,413)] {
            assert_eq!(h.form("PATCH", &(prefix.clone()+&"x".repeat(size-prefix.len()))).await.0, status);
        }
    }
    #[tokio::test]
    async fn field_count() {
        let h = Harness::new();
        for (n,status) in [(1000,200),(1001,400)] {
            let body = (0..n).map(|i| format!("{ROOT}[x{i}]=1")).collect::<Vec<_>>().join("&");
            assert_eq!(h.form("PATCH", &body).await.0, status);
            let fields: serde_json::Map<String,Value> = (0..n).map(|i| (format!("x{i}"), json!(1))).collect();
            assert_eq!(h.send("PATCH", "/bots/1", "application/json", Body::from(json!({ROOT:fields}).to_string())).await.0, status);
        }
    }
    #[tokio::test]
    async fn depth() {
        let h = Harness::new(); assert_eq!(h.form("PATCH", &format!("{ROOT}[allocations][1]=100")).await.0, 200);
        assert_eq!(h.form("PATCH", &format!("{ROOT}{}=1", "[a]".repeat(100))).await.0, 400);
        assert_eq!(h.send("PATCH", "/bots/1", "application/json", Body::from(format!("{}0{}", "{\"a\":".repeat(100), "}".repeat(100)))).await.0, 400);
    }
    #[tokio::test]
    async fn body_deadline() {
        let h = Harness::new();
        for (method,path) in [("PATCH","/bots/1"),("DELETE","/bots/1/delete")] {
            let body = Body::from_stream(futures_util::stream::pending::<Result<axum::body::Bytes,std::io::Error>>());
            assert_eq!(h.send(method,path,"application/x-www-form-urlencoded",body).await.0, 408);
        }
        assert_eq!(h.form("PATCH", VALID).await.0, 200);
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await { Ok(v) => v, Err(e) => panic!("bind: {e}") };
        let address = match listener.local_addr() { Ok(v) => v, Err(e) => panic!("address: {e}") };
        let before = h.snapshot().await;
        let limits = web::server::Limits { body_read_timeout: Duration::from_millis(20), ..Default::default() };
        let server = tokio::spawn(web::server::serve_on(listener,h.app.clone(),limits));
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            for (method,path) in [("PATCH","/bots/1"),("DELETE","/bots/1/delete")] {
                let mut stream = tokio::net::TcpStream::connect(address).await?;
                stream.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: 100\r\n\r\nx").as_bytes()).await?;
                let mut response = String::new(); stream.read_to_string(&mut response).await?;
                assert!(response.starts_with("HTTP/1.1 408"),"{response}");
                assert!(response.to_ascii_lowercase().contains("connection: close"));
            }
            Ok::<_,std::io::Error>(())
        }).await;
        server.abort(); let _ = server.await;
        assert!(matches!(result,Ok(Ok(()))),"server must close stalled requests: {result:?}");
        assert_eq!(before,h.snapshot().await);
    }
    #[tokio::test]
    async fn format_before_write() {
        let h = Harness::new();
        for (method,path) in [("PATCH","/bots/1"),("PATCH","/bots/1/start"),("PATCH","/bots/1/stop"),("POST","/bots/1/archive")] {
            for accept in ["text/html", "text/html, text/vnd.turbo-stream.html;q=0.000"] {
                assert_eq!(h.send_accept(method,path,"application/x-www-form-urlencoded",Body::from(VALID),accept).await.0, 406);
            }
            assert_eq!(h.send_accept(method,path,"application/x-www-form-urlencoded",Body::from(VALID),"text/html, text/vnd.turbo-stream.html;q=0.9").await.0, 200);
        }
        for path in ["/bots/1/delete", "/bots/1/archive"] {
            assert_eq!(h.send_accept("DELETE",path,"application/x-www-form-urlencoded",Body::empty(),"text/html").await.0, 200);
        }
        assert_eq!(h.send_accept("PATCH","/bots/1.turbo_stream","application/x-www-form-urlencoded",Body::from(VALID),"text/html").await.0, 200);
        assert_eq!(h.form("PATCH", VALID).await.0, 200);
        for (method,path) in [("PATCH","/bots/1"),("PUT","/bots/1/start"),("PATCH","/bots/1/stop"),("POST","/bots/1/archive")] {
            let body = format!("{VALID}&authenticity_token=TOKEN");
            assert_eq!(h.live(method,path,"application/x-www-form-urlencoded",&body,&[("accept","text/html")]).await,406);
            assert_eq!(h.live(method,path,"application/x-www-form-urlencoded",&body,&[("accept","text/vnd.turbo-stream.html")]).await,if method=="PUT" {501}else{302});
        }
        for path in ["/bots/1/delete","/bots/1/archive"] {
            assert_eq!(h.live("DELETE",path,"application/x-www-form-urlencoded","authenticity_token=TOKEN",&[("accept","text/html")]).await,302);
        }
    }
    #[tokio::test]
    async fn localized_paths() {
        let h = Harness::new(); let (_, got) = h.send("POST", "/de/bots/1.turbo_stream?locale=pl", "application/x-www-form-urlencoded", Body::from(format!("{VALID}&_method=patch"))).await;
        assert_eq!(got["locale"], "de"); assert_eq!(got["method"], "PATCH");
        assert_eq!(h.send("PATCH", "/de/bots/1.turbo_stream", "application/json", Body::from("{")).await.0, 400);
        assert_eq!(h.live("POST","/de/bots/1.turbo_stream?locale=pl","application/x-www-form-urlencoded",&format!("{VALID}&_method=patch&authenticity_token=TOKEN"),&[("accept","text/html")]).await,302);
        for token in [json!("TOKEN"),json!(["TOKEN"]),json!({"x":"TOKEN"}),json!(true),Value::Null] {
            let expected = 302;
            let body = json!({ROOT:{"quote_amount":12.5},"authenticity_token":token}).to_string();
            assert_eq!(h.live("PATCH","/de/bots/1.turbo_stream","application/json",&body,&[]).await,expected);
        }
        assert_eq!(h.live("PATCH","/de/bots/1","application/json",&json!({ROOT:{"quote_amount":12.5},"authenticity_token":"TOKEN"}).to_string(),&[("origin","https://foreign.example")]).await,302);
        // JSON remains unparsed on login and on wrong bot action verbs/paths.
        for (method,path) in [("POST","/login"),("GET","/bots/1"),("POST","/bots/1/start"),("PATCH","/bots/1/chart"),("PATCH","/bots/new")] {
            let token = web::csrf::masked(&h.token);
            let status = h.live(method,path,"application/json","{",&[("x-csrf-token",&token)]).await;
            assert_ne!(status,400,"{method} {path} is not a JSON action");
        }
        for (method,path) in [("PATCH","/bots/1"),("PUT","/bots/1"),("PATCH","/bots/1/start"),("PUT","/bots/1/stop"),("POST","/bots/1/archive"),("DELETE","/bots/1/archive"),("DELETE","/bots/1/delete")] {
            assert_eq!(h.live(method,path,"application/json","{",&[]).await,400,"{method} {path}");
        }
    }
}

mod action_draft {
    use super::common;
    use deltabadger::web::bot::{draft::{Draft, ValidationContext}, action_params::ActionParams, Kind};
    use deltabadger::web::Params;
    use serde_json::{json, Value};
    use rusqlite::{Connection, params_from_iter};

    fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
        value.get(key).unwrap_or_else(|| panic!("missing {key}"))
    }

    #[test]
    fn action_draft_rails_vectors() -> Result<(), Box<dyn std::error::Error>> {
        let vectors = common::vectors();
        let group = field(&vectors, "bot_actions");
        let cases = field(group, "cases").as_array().ok_or("cases are not an array")?;
        assert_eq!(cases.len(), 5562, "pin the implementation-base inventory");
        assert_eq!(cases.len(), field(group, "count").as_u64().ok_or("count")? as usize);
        let dir = common::rails_install();
        let c = Connection::open(dir.path().join("production.sqlite3"))?;
        c.execute_batch("PRAGMA foreign_keys = OFF")?;
        for (table, rows) in field(group, "rows").as_object().ok_or("rows")? {
            // Table and column names are generated fixture data, never request input.
            for row in rows.as_array().ok_or("table rows")? {
                let row = row.as_object().ok_or("row")?;
                let columns = row.keys().map(|key| format!("\"{key}\"")).collect::<Vec<_>>().join(",");
                let values = row.values().map(|value| match value {
                    Value::Null => rusqlite::types::Value::Null,
                    Value::Number(n) => n.as_i64().map(rusqlite::types::Value::Integer)
                        .unwrap_or_else(|| rusqlite::types::Value::Real(n.as_f64().unwrap_or_default())),
                    Value::Bool(b) => rusqlite::types::Value::Integer(i64::from(*b)),
                    Value::String(s) => rusqlite::types::Value::Text(s.clone()),
                    other => rusqlite::types::Value::Text(other.to_string()),
                }).collect::<Vec<_>>();
                let binds = vec!["?"; values.len()].join(",");
                c.execute(&format!("INSERT INTO {table} ({columns}) VALUES ({binds})"), params_from_iter(values))?;
            }
        }
        let now = field(group, "now").as_str().ok_or("now")?.parse()?;
        let mut failures = Vec::new();
        macro_rules! compare {
            ($left:expr, $right:expr, $($message:tt)*) => {
                if $left != $right && failures.len() < 40 {
                    failures.push(format!("{}: left={:?}, right={:?}", format!($($message)*), $left, $right));
                }
            };
        }
        let ctx = deltabadger::web::layout::Ctx {
            app: common::web::app(dir.path(), common::web::SECRET, common::web::TestClock::at(field(group, "now").as_str().ok_or("now")?)),
            params: std::sync::Arc::new(Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: None }),
            session: deltabadger::web::session::Session::new(Default::default()),
            current: deltabadger::web::auth::Current::SignedOut, method: axum::http::Method::PATCH, locale: "en", nonce: String::new(), now, turbo_frame: None, stream: false,
        };
        for recorded in cases {
            let mut resolved = recorded.clone();
            for key in ["raw", "baseline", "candidate", "save_settings", "save_transient"] {
                if let Some(index) = recorded.get(key).and_then(Value::as_u64) {
                    let state = field(group, "states").as_array().and_then(|s| s.get(index as usize)).ok_or("snapshot reference")?.clone();
                    resolved.as_object_mut().ok_or("case object")?.insert(key.into(), state);
                }
            }
            let case = &resolved;
            let name = field(case, "name");
            let id = field(case, "bot_id").as_i64().ok_or("bot_id")?;
            c.execute("UPDATE bots SET settings = ?1, status = ?2 WHERE id = ?3",
                (field(case, "raw").to_string(), field(case, "persisted_status").as_i64(), id))?;
            c.execute("UPDATE tickers SET available = ?1 WHERE exchange_id = 1 AND base = 'QQQM'", [!case.get("delisted").and_then(Value::as_bool).unwrap_or(false)])?;
            let provider = case.get("provider").and_then(Value::as_bool).unwrap_or(true);
            let before: String = c.query_row("SELECT settings FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
            let mut draft = Draft::load(&c, 1, id,"en").map_err(|e| format!("{e:?}"))?.ok_or("owned bot")?;
            compare!(json!(draft.baseline), *field(case, "baseline"), "{name}: default-filled baseline");
            draft.candidate.settings.extend(field(case, "stored").as_object().ok_or("stored")?.clone());
            draft.candidate.transient.extend(field(case, "transient").as_object().ok_or("transient")?.clone());
            let root = if draft.candidate.kind == Kind::Basket { "bots_dca_multi_asset" } else { "bots_dca_index" };
            let mut submitted = field(case, "submitted").as_object().ok_or("submitted")?.clone();
            submitted.insert("unknown".into(), json!("ignored"));
            let transport = Params { upload: None, json: Some(json!({root: submitted})), full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![] };
            let parsed = ActionParams::parse(&transport).map_err(|_| "transport")?;
            let permitted = parsed.permitted(draft.candidate.kind).map_err(|_| "strong parameters")?;
            compare!(permitted, *field(case, "permitted"), "{name}: permitted");
            let parse = draft.parse(&c, &permitted, field(case, "zone").as_str().ok_or("zone")?, now);
            let overflow = case.get("parsed").is_some_and(|p| p.to_string().contains("nonfinite"))
                || field(case, "submitted").get("allocations").is_some_and(|p| p.to_string().contains("1e999"));
            if overflow {
                assert!(parse.is_err() || !draft.errors.is_empty(), "{name}: overflow is a 422, never zero");
                if let Err(error) = parse { assert_eq!(error.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY); }
                if name == "1/quote_amount/overflow/update" {
                    let html = deltabadger::web::bot::settings::draft_column(&c, &ctx, "csrf", &draft, "UTC", false).map_err(|e| format!("{e:?}"))?;
                    assert!(html.contains("value=\"1e999\""), "{html}");
                }
                continue;
            }
            if case.get("exception").is_some() {
                // Rails exceptions are explicitly refused without writing. No panic or HTTP 500.
                if parse.is_ok() {
                    let context = if field(case, "context") == "start" { ValidationContext::Start } else { ValidationContext::Update };
                    { if context == ValidationContext::Start { draft.candidate.status = deltabadger::enums::BotStatus::Scheduled; } draft.validate(&c, context, now, provider, "en") }.map_err(|e| format!("{name}: {e:?}"))?;
                    assert!(!draft.errors.is_empty(), "{name}: Rails exception must produce field errors");
                    if name == "1/interval/normal/update" {
                        let html = deltabadger::web::bot::settings::draft_column(&c, &ctx, "csrf", &draft, "UTC", false).map_err(|e| format!("{e:?}"))?;
                        assert!(html.contains("is-invalid") && html.contains("value=\"12.5\""), "{html}");
                        for error in draft.errors.iter().filter(|e| e.field == "interval") {
                            // The existing form helper capitalizes field messages.
                            assert!(html.to_lowercase().contains(&error.message.to_lowercase()), "{html}");
                        }
                    }
                }
                continue;
            }
            // A wall time that repeats in the owner's zone is refused (timezone::resolve_local), never stored as Rails' pick.
            if parse.as_ref().is_err_and(|e| format!("{e:?}").contains("occurs twice")) { continue; }
            parse.map_err(|e| format!("{name}: {e:?}"))?;
            compare!(json!(draft.parsed), *field(case, "parsed"), "{name}: parse chain");
            let context = if field(case, "context") == "start" { ValidationContext::Start } else { ValidationContext::Update };
            { if context == ValidationContext::Start { draft.candidate.status = deltabadger::enums::BotStatus::Scheduled; } draft.validate(&c, context, now, provider, "en") }.map_err(|e| format!("{name}: {e:?}"))?;
            compare!(json!(draft.candidate.settings), *field(case, "candidate"), "{name}: candidate");
            let errors = draft.errors.iter().map(|e| json!({"field":e.field,"message":e.message})).collect::<Vec<_>>();
            let mut expected_errors = field(case, "errors").as_array().ok_or("errors")?.clone();
            if let Some(lock) = case.get("rust_lock_error") { expected_errors.push(lock.clone()); }
            compare!(json!(errors), json!(expected_errors), "{name}: ordered errors");
            compare!(draft.error_sentence("en"), case.get("rust_sentence").unwrap_or(field(case, "sentence")).as_str().ok_or("sentence")?, "{name}: sentence");
            compare!(draft.errors.is_empty(), field(case, "valid").as_bool().ok_or("valid")? && case.get("rust_lock_error").is_none(), "{name}: valid");
            if name == "1/quote_asset_id/upper/update" || name == "1/exchange_id/upper/update" {
                let html = deltabadger::web::bot::settings::draft_column(&c, &ctx, "csrf", &draft, "UTC", false).map_err(|e| format!("{e:?}"))?;
                assert!(html.contains("100: is invalid"), "{name}: {html}");
            }
            if name == "1/quote_amount/lower/update" {
                let html = deltabadger::web::bot::settings::draft_column(&c, &ctx, "csrf", &draft, "UTC", false).map_err(|e| format!("{e:?}"))?;
                assert!(html.contains("is-invalid") && html.contains("value=\"0\""), "{html}");
                assert!(html.contains("The invested amount must be greater than 0"), "{html}");
            }
            let effects = draft.save_effects(&c, now).map_err(|e| format!("{e:?}"))?;
            compare!(effects.settings_changed, field(case, "settings_changed").as_bool().ok_or("settings_changed")?, "{name}: defaults are separate");
            if let Some(saved) = case.get("save_settings") {
                let mut settings = draft.raw_settings.clone();
                for key in &effects.settings.remove { settings.shift_remove(key); }
                settings.extend(effects.settings.set.clone());
                compare!(json!(settings), *saved, "{name}: save settings callbacks");
                let mut transient = draft.raw_transient.clone();
                for key in &effects.transient.remove { transient.shift_remove(key); }
                transient.extend(effects.transient.set.clone());
                compare!(json!(transient), *field(case, "save_transient"), "{name}: save transient callbacks");
            }
            compare!(draft.candidate.label, field(case, "label").as_str().ok_or("label")?, "{name}: label");
            let after: String = c.query_row("SELECT settings FROM bots WHERE id = ?1", [id], |r| r.get(0))?;
            compare!(before, after, "{name}: a draft never writes");
        }
        // The documented HTTP work bound is a 501 divergence, checked on both sides without
        // replacing the actual cap aggregation or collecting an unbounded history in memory.
        c.execute("DELETE FROM transactions", [])?;
        c.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x + 1 FROM n WHERE x < 100000) INSERT INTO transactions(bot_id, exchange_id, side, status, external_status, quote_amount_exec, created_at, updated_at) SELECT 3, 1, 0, 0, 2, 0, '2026-09-09 00:00:00', '2026-09-09 00:00:00' FROM n")?;
        let budget = Draft::load(&c, 1, 3,"en").map_err(|e| format!("{e:?}"))?.ok_or("budget bot")?;
        assert!(deltabadger::web::bot::start::amount_limit(&c, &budget.candidate).is_ok());
        c.execute("INSERT INTO transactions(bot_id, exchange_id, side, status, external_status, quote_amount_exec, created_at, updated_at) VALUES(3, 1, 0, 0, 2, 0, '2026-09-09 00:00:00', '2026-09-09 00:00:00')", [])?;
        let error = match deltabadger::web::bot::start::amount_limit(&c, &budget.candidate) {
            Err(error) => error, Ok(_) => return Err("100001 relevant rows must be refused".into()),
        };
        let response = deltabadger::web::layout::or_refused(&ctx, error).map_err(|e| format!("{e:?}"))?;
        assert_eq!(response.status(), axum::http::StatusCode::NOT_IMPLEMENTED);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        Ok(())
    }
}

mod action_write {
    use super::common;
    use common::{seed, web as harness};
    use deltabadger::web::{self, bot::{action_params::ActionParams, draft::Draft, write::{self, Outcome, Prepared}}, layout::Ctx, Params, WebError};
    use deltabadger::engine::eligibility;
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::sync::Arc;
    type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
    const NOW: &str = "2026-09-10T12:00:30.123456Z";

    struct Fixture { _dir: tempfile::TempDir, c: Connection, ctx: Ctx, seed: seed::Seeded, id: i64 }
    impl Fixture {
        fn new() -> Result<Self> {
            let (dir, opened, s) = common::install_alpaca();
            let c = opened.primary;
            c.execute("UPDATE users SET wash_sale_enabled=0",[])?;
            let mut spec = seed::BotSpec::weekly(5.0, "2026-09-10 12:00:00");
            spec.status = 2;
            spec.transient = json!({"engine_private":{"keep":null}, "mail":"untouched", "failure":7});
            let id = seed::insert_bot(&c, &s, &spec);
            c.execute("UPDATE bots SET label = 'Original', position = 17 WHERE id = ?1", [id])?;
            let app = harness::app(dir.path(), "engine-test-secret", harness::TestClock::at(NOW));
            let ctx = Ctx { app, params: Arc::new(Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: None }),
                session: web::session::Session::new(Default::default()), current: web::auth::Current::SignedOut,
                method: axum::http::Method::PATCH, locale: "en", nonce: String::new(), now: harness::at(NOW), turbo_frame: None, stream: false };
            Ok(Self { _dir: dir, c, ctx, seed: s, id })
        }
        fn params(fields: Value) -> Result<ActionParams> {
            let p = Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: Some(json!({"bots_dca_multi_asset":fields})) };
            ActionParams::parse(&p).map_err(|e| format!("{e:?}").into())
        }
        fn write(&self, fields: Value) -> Result<Outcome<Value>> {
            Ok(write::settings(&self.c, &self.ctx, self.seed.user_id, self.id, &Self::params(fields)?, |_, ctx, draft| {
                assert_eq!(ctx.now, harness::at(NOW));
                Ok(Prepared { response: json!({"settings":draft.candidate.settings,"errors":draft.errors.iter().map(|e| &e.message).collect::<Vec<_>>()}), broadcasts: vec![], after_scheduling: None })
            }).map_err(|e| format!("{e:?}"))?)
        }
        fn stored(&self) -> Result<Value> {
            let (settings, transient, updated, changed, label): (String,String,String,Option<String>,String) = self.c.query_row("SELECT settings, transient_data, updated_at, settings_changed_at, label FROM bots WHERE id=?1", [self.id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
            Ok(json!({"settings":serde_json::from_str::<Value>(&settings)?,"transient":serde_json::from_str::<Value>(&transient)?,"updated":updated,"changed":changed,"label":label}))
        }
        fn snapshot(&self) -> Result<Vec<String>> {
            let mut all = vec![];
            for table in ["bots","bot_index_assets","bot_activity_logs","transactions","api_keys","users"] {
                let mut q = self.c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
                let n = q.column_count();
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? { let values = (0..n).map(|i| r.get_ref(i).map(|v| format!("{v:?}"))).collect::<std::result::Result<Vec<_>,_>>()?; all.push(format!("{table}:{values:?}")); }
            }
            Ok(all)
        }
        fn stabilize(&self) -> Result { assert!(matches!(self.write(json!({"label":"Original"}))?, Outcome::Committed(_) | Outcome::NoChange(_))); Ok(()) }
    }

    #[test]
    fn divergence_settings_held_until_reconciliation() -> Result {
        use deltabadger::engine::{amount, model, placement, FixedClock};
        use deltabadger::ruby::BigDec;
        let rails = common::ownership_case("settings_held_until_reconciliation");
        assert_eq!(rails["rails_update_succeeded"], true);
        assert_eq!(rails["rails_quote_amount"], 100.0);
        let f = Fixture::new()?;
        let bot = model::load_bot(&f.c, f.id).unwrap();
        let ticker = model::ticker_for(&f.c, &bot).unwrap().ok_or("ticker")?;
        let amount::Sizing::Place(plan) = amount::size(&bot, &ticker, &BigDec::from_i64(5), &BigDec::from_i64(50_000), &deltabadger::engine::venue_rules::ALPACA).unwrap() else { panic!("plan") };
        placement::begin(&f.c, &bot, &plan, &FixedClock(harness::at(NOW))).unwrap();
        let before = f.snapshot()?;
        assert!(matches!(f.write(json!({"quote_amount":"100"}))?, Outcome::GuardRefused(_)));
        assert_eq!(f.snapshot()?, before, "refusal preserves the carry window and unresolved order");
        Ok(())
    }
    #[test]
    fn action_write_amount_preserves_columns_and_json() -> Result {
        let f = Fixture::new()?;
        assert!(matches!(f.write(json!({"quote_amount":"37.25"}))?, Outcome::Committed(_)));
        let v = f.stored()?;
        assert_eq!(v["settings"]["quote_amount"], json!(37.25));
        assert_eq!(v["transient"]["engine_private"], json!({"keep":null}));
        assert_eq!(v["transient"]["mail"], "untouched");
        assert_eq!(v["changed"], "2026-09-10 12:00:30.123456");
        assert_eq!(f.c.query_row("SELECT position FROM bots WHERE id=?1", [f.id], |r| r.get::<_,i64>(0))?, 17);
        Ok(())
    }
    #[test]
    fn smart_interval_hint_preserves_submitted_float_without_dirtying_equal_settings() -> Result {
        use deltabadger::web::bot::{Bot, For, settings::Forms};
        for (submitted, expected, changed) in [("50", "50.0", false), ("50.0", "50.0", false),
                                               ("80", "80.0", true), ("37.25", "37.25", true), ("50.004", "50.0", true)] {
            let f = Fixture::new()?;
            f.stabilize()?;
            f.c.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount',50,'$.smart_intervaled',json('true'),'$.smart_interval_quote_amount',5) WHERE id=?1", [f.id])?;
            let before = f.snapshot()?;
            let render = |bot: &Bot| Forms { ctx: &f.ctx, csrf: "token", bot, path: format!("/bots/{}", f.id),
                hide_balances: false, time_zone: "UTC", check: None, rebalance_info: None }.smart_intervals();
            let bot = Bot::find(&f.c, f.seed.user_id, f.id, For::Page, "en").map_err(|e| format!("{e:?}"))?.unwrap();
            let quote = bot.quote_symbol().unwrap();
            assert!(render(&bot).contains(&format!("50 {quote}")), "an untouched Integer stays an Integer");
            let outcome = write::settings(&f.c, &f.ctx, f.seed.user_id, f.id, &Fixture::params(json!({"quote_amount":submitted}))?,
                |_, _, draft| Ok(Prepared { response: render(&draft.candidate), broadcasts: vec![], after_scheduling: None }))
                .map_err(|e| format!("{e:?}"))?;
            let response = match outcome {
                Outcome::NoChange(body) if !changed => body,
                Outcome::Committed(body) if changed => body,
                _ => return Err(format!("unexpected save outcome for {submitted}").into()),
            };
            assert!(response.contains(&format!("{expected} {quote}")), "submitted {submitted}: {response}");
            if !changed { assert_eq!(f.snapshot()?, before, "equal amounts preserve every stored column"); }
            let reloaded = Bot::find(&f.c, f.seed.user_id, f.id, For::Page, "en").map_err(|e| format!("{e:?}"))?.unwrap();
            let persisted = if changed { expected } else { "50" };
            assert!(render(&reloaded).contains(&format!("{persisted} {quote}")), "a later request reads the persisted numeric type");
        }
        Ok(())
    }

    #[test]
    fn action_write_rename_blank_identical_preserve_window() -> Result {
        let f = Fixture::new()?; f.stabilize()?;
        let before = f.stored()?;
        assert!(matches!(f.write(json!({"quote_amount":"", "label":"", "interval":""}))?, Outcome::NoChange(_)));
        assert_eq!(f.stored()?, before);
        assert!(matches!(f.write(json!({"quote_amount":"5"}))?, Outcome::NoChange(_)));
        assert!(matches!(f.write(json!({"label":"Renamed"}))?, Outcome::Committed(_)));
        let after = f.stored()?;
        assert_eq!(after["transient"], before["transient"]); assert_eq!(after["changed"], before["changed"]);
        Ok(())
    }
    #[test]
    fn action_write_defaults_do_not_move_window() -> Result {
        let f = Fixture::new()?;
        f.c.execute("UPDATE bots SET settings=json_set(json_remove(settings,'$.smart_intervaled'),'$.limit_ordered',null) WHERE id=?1", [f.id])?;
        assert!(matches!(f.write(json!({"label":"Original"}))?, Outcome::Committed(_)));
        let v = f.stored()?;
        assert_eq!(v["settings"]["smart_intervaled"], false); assert_eq!(v["settings"]["limit_ordered"], false);
        assert_eq!(v["changed"], Value::Null);
        Ok(())
    }
    #[test]
    fn action_write_carry_partial_fills_inclusive_and_class() -> Result {
        for status in [3,4] {
            let f = Fixture::new()?;
            seed::insert_row(&f.c,&f.seed,f.id,f.seed.btc,&json!({"external_status":status,"amount_exec":"0.000025","quote_amount_exec":"1.25","created_at":"2026-09-10 12:00:00","external_id":format!("cancel-{status}")}));
            assert!(matches!(f.write(json!({"quote_amount":"7"}))?, Outcome::Committed(_)));
            assert_eq!(f.stored()?["transient"]["missed_quote_amount"], "3.75");
        }
        let f = Fixture::new()?;
        f.write(json!({"quote_amount":"2"}))?;
        assert_eq!(f.stored()?["transient"]["missed_quote_amount"], json!(2.0));
        Ok(())
    }
    #[test]
    fn action_write_unfilled_cancel_preserves_numeric_class() -> Result {
        let f=Fixture::new()?;
        seed::insert_row(&f.c,&f.seed,f.id,f.seed.btc,&json!({"external_status":3,"created_at":"2026-09-10 12:00:00"}));
        let draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e|format!("{e:?}"))?.ok_or("draft")?;
        let pending=write::pending(&f.c,&draft.original,harness::at(NOW)).map_err(|e|format!("{e:?}"))?;
        assert!(matches!(pending,web::format::Num::Float(5.0)),"{pending:?}");
        Ok(())
    }

    #[test]
    fn action_write_toggle_keys_preserve_private_data() -> Result {
        let f = Fixture::new()?;
        assert!(matches!(f.write(json!({"quote_amount_limited":"true","quote_amount_limit":"100"}))?, Outcome::Committed(_)));
        assert_eq!(f.stored()?["transient"]["quote_amount_limit_enabled_at"], "2026-09-10T12:00:30.123Z");
        f.write(json!({"quote_amount_limited":"false"}))?;
        let v = f.stored()?;
        assert_eq!(v["transient"]["quote_amount_limit_enabled_at"], Value::Null);
        assert_eq!(v["transient"]["failure"], 7);
        Ok(())
    }
    #[test]
    fn action_write_membership_removal_reentry_zero_and_scale() -> Result {
        let f = Fixture::new()?;
        let (eth, sol) = seed::add_eth_sol(&f.c,&f.seed);
        for fields in [json!({"add_asset_id":eth.to_string()}), json!({"allocations":{f.seed.btc.to_string():"20",eth.to_string():"20"}})] {
            assert!(matches!(f.write(fields)?, Outcome::Committed(_)));
        }
        let first: (String,String,String) = f.c.query_row("SELECT entered_at,created_at,updated_at FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2", (f.id,eth), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        f.c.execute("UPDATE bot_index_assets SET updated_at='2001-01-01 00:00:00' WHERE bot_id=?1 AND asset_id=?2", (f.id,eth))?;
        f.write(json!({"remove_asset_id":eth.to_string()}))?;
        let exited: (bool,String,String) = f.c.query_row("SELECT in_index,exited_at,updated_at FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2", (f.id,eth), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        assert_eq!(exited,(false,"2026-09-10 12:00:30.123456".into(),"2001-01-01 00:00:00".into()));
        assert_eq!(f.stored()?["settings"]["allocations"][f.seed.btc.to_string()],json!(1.0));
        f.write(json!({"add_asset_id":eth.to_string()}))?;
        let returned: (String,String,Option<String>,f64) = f.c.query_row("SELECT entered_at,created_at,exited_at,target_allocation FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2", (f.id,eth), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
        assert_eq!(returned,(first.0,first.1,None,0.0));
        f.write(json!({"allocations":{f.seed.btc.to_string():"20",eth.to_string():"20",sol.to_string():"20"}}))?;
        let weight: f64 = f.c.query_row("SELECT target_allocation FROM bot_index_assets WHERE bot_id=?1 LIMIT 1", [f.id], |r| r.get(0))?;
        assert_eq!(weight,0.333333);
        let before = f.snapshot()?;
        f.write(json!({"allocations":{f.seed.btc.to_string():"0",eth.to_string():"0",sol.to_string():"0"}}))?;
        let members = |rows: Vec<String>| rows.into_iter().filter(|s| s.starts_with("bot_index_assets:")).collect::<Vec<_>>();
        assert_eq!(members(before),members(f.snapshot()?));
        Ok(())
    }
    #[test]
    fn action_write_validation_retains_draft_rolls_back_all_rows() -> Result {
        let f = Fixture::new()?; let before=f.snapshot()?;
        let outcome=f.write(json!({"quote_amount":"0","label":"Rejected"}))?;
        let Outcome::Invalid(response,_)=outcome else { return Err("expected invalid".into()) };
        assert_eq!(response["settings"]["quote_amount"],json!(0.0));
        assert!(!response["errors"].as_array().ok_or("errors")?.is_empty());
        assert_eq!(f.snapshot()?,before); Ok(())
    }
    #[test]
    fn action_write_guard_rolls_back_all_rows() -> Result {
        let f=Fixture::new()?;
        f.c.execute("UPDATE bots SET status=1 WHERE id=?1",[f.id])?;
        f.c.execute("DELETE FROM api_keys",[])?;
        let before=f.snapshot()?;
        assert!(matches!(f.write(json!({"quote_amount":"7"}))?,Outcome::GuardRefused(_)));
        assert_eq!(f.snapshot()?,before); Ok(())
    }
    #[test]
    fn action_write_renderer_and_commit_failure_roll_back() -> Result {
        let f=Fixture::new()?; let before=f.snapshot()?;
        let render=write::settings::<()>(&f.c,&f.ctx,f.seed.user_id,f.id,&Fixture::params(json!({"quote_amount":"7"}))?,|_,_,_| Err(WebError::Config("renderer failed".into())));
        assert!(render.is_err()); assert_eq!(f.snapshot()?,before);
        f.c.execute_batch("CREATE TABLE commit_parent(id INTEGER PRIMARY KEY); CREATE TABLE commit_child(id INTEGER REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_commit AFTER UPDATE ON bots BEGIN INSERT INTO commit_child VALUES(42); END;")?;
        assert!(f.write(json!({"quote_amount":"7"})).is_err()); assert_eq!(f.snapshot()?,before);
        Ok(())
    }
    #[test]
    fn action_write_second_browser_preserves_unrelated_setting() -> Result {
        let f=Fixture::new()?;
        let submitted=Fixture::params(json!({"quote_amount":"7"}))?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.limit_order_pcnt_distance',0.07) WHERE id=?1",[f.id])?;
        write::settings(&f.c,&f.ctx,f.seed.user_id,f.id,&submitted,|_,_,_|Ok(Prepared {response:(),broadcasts:vec![], after_scheduling: None })).map_err(|e| format!("{e:?}"))?;
        assert_eq!(f.stored()?["settings"]["limit_order_pcnt_distance"],json!(0.07)); Ok(())
    }
    #[test]
    fn action_write_composition_locks_precede_scope_refusal() -> Result {
        for pending in [false,true] {
            let f=Fixture::new()?; let (eth,_)=seed::add_eth_sol(&f.c,&f.seed);
            if pending { f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rebalance_pending',1) WHERE id=?1",[f.id])?; }
            else { f.c.execute("UPDATE bots SET status=1 WHERE id=?1",[f.id])?; }
            let before=f.snapshot()?;
            let Outcome::Invalid(response,_)=f.write(json!({"add_asset_id":eth.to_string()}))? else { return Err("expected composition lock".into()) };
            assert!(response["errors"].to_string().contains("cannot be changed while the bot is running"),"{response}"); assert_eq!(f.snapshot()?,before);
        }
        Ok(())
    }
    #[test]
    fn action_write_history_and_input_bounds() -> Result {
        let f=Fixture::new()?;
        f.c.execute_batch(&format!("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO transactions(bot_id,exchange_id,status,side,external_status,quote_amount_exec,created_at,updated_at) SELECT {},{},0,0,3,0,'2026-09-10 12:00:00','2026-09-10 12:00:00' FROM n",f.id,f.seed.exchange_id))?;
        let draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e| format!("{e:?}"))?.ok_or("draft")?;
        assert!(write::pending(&f.c,&draft.original,harness::at(NOW)).is_ok());
        f.c.execute("INSERT INTO transactions(bot_id,exchange_id,status,side,external_status,quote_amount_exec,created_at,updated_at) VALUES(?1,?2,0,0,3,0,'2026-09-10 12:00:00','2026-09-10 12:00:00')",(f.id,f.seed.exchange_id))?;
        let before=f.snapshot()?;
        assert!(matches!(f.write(json!({"quote_amount":"7"}))?,Outcome::Unported(_)));
        assert_eq!(f.snapshot()?,before);
        // Stored history refusal precedes submitted-input validation; neither writes.
        assert!(matches!(f.write(json!({"quote_amount":"1e999"}))?,Outcome::Unported(_)));
        assert_eq!(f.snapshot()?,before);
        f.c.execute("UPDATE bots SET transient_data='{' WHERE id=?1",[f.id])?;
        let before=f.snapshot()?; assert!(matches!(f.write(json!({"label":"bad"}))?,Outcome::Unported(_))); assert_eq!(f.snapshot()?,before);
        Ok(())
    }
    #[test]
    fn action_write_carry_decorator_oracle() -> Result {
        // Fresh Rails runner measurements, Ruby 4.0.7: Accountable + Startable + conditions
        // + SmartIntervalable + QuoteAmountLimitable, at the microsecond request clock.
        let cases = [
            ("plain", json!({}), json!({}), "2026-09-10 12:00:00", None, "5.0"),
            ("future", json!({"start_time_enabled":true,"start_at":"2026-09-11T12:00:00Z"}), json!({}), "2026-09-10 12:00:00", None, "0.0"),
            ("month", json!({"interval":"month"}), json!({}), "2026-01-31 12:00:00.123456", None, "35.0"),
            ("microseconds", json!({"interval":"hour"}), json!({}), "2026-09-10 11:00:30.123455", None, "10.0"),
            ("changed", json!({"interval":"hour"}), json!({"missed_quote_amount":"3.75"}), "2026-09-10 10:00:00", Some("2026-09-10 11:30:00"), "8.75"),
            ("price_paused", json!({"price_limited":true}), json!({}), "2026-09-10 12:00:00", None, "0"),
            ("price_resumed", json!({"price_limited":true}), json!({"price_limit_condition_met_at":"2026-09-10T11:30:00Z"}), "2026-09-10 10:00:00", None, "0.0"),
            ("cap", json!({"quote_amount_limited":true,"quote_amount_limit":3.0}), json!({}), "2026-09-10 12:00:00", None, "3.0"),
        ];
        for (name,settings,transient,started,changed,expected) in cases {
            let f=Fixture::new()?;
            let mut stored=f.stored()?;
            stored["settings"].as_object_mut().ok_or("settings")?.extend(settings.as_object().ok_or("settings")?.clone());
            stored["transient"]["missed_quote_amount"]=json!("0.0");
            stored["transient"].as_object_mut().ok_or("transient")?.extend(transient.as_object().ok_or("transient")?.clone());
            f.c.execute("UPDATE bots SET settings=?1,transient_data=?2,started_at=?3,settings_changed_at=?4 WHERE id=?5",(stored["settings"].to_string(),stored["transient"].to_string(),started,changed,f.id))?;
            let draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e| format!("{e:?}"))?.ok_or("draft")?;
            let amount=write::pending(&f.c,&draft.original,harness::at(NOW)).map_err(|e| format!("{e:?}"))?;
            assert_eq!(amount.to_s(),expected,"{name}");
        }
        Ok(())
    }

    #[test]
    fn action_write_carry_reads_an_index_bots_price_rule() -> Result {
        // D-IDX-CONDITIONS (reverses #742): an index bot's buy-side price rule decorates its carry window as a basket's does; its
        // flip action and its sell side have nothing to act on there and leave the carry as an unflagged bot's.
        let pending=|settings:Value,transient:Value| -> std::result::Result<String,String> {
            let f=Fixture::new().map_err(|e| e.to_string())?;
            let mut stored=f.stored().map_err(|e| e.to_string())?;
            stored["settings"].as_object_mut().ok_or("settings")?.extend(settings.as_object().ok_or("settings")?.clone());
            stored["transient"]["missed_quote_amount"]=json!("2.5");
            stored["transient"].as_object_mut().ok_or("transient")?.extend(transient.as_object().ok_or("transient")?.clone());
            f.c.execute("UPDATE bots SET type='Bots::DcaIndex',settings=json_set(?1,'$.index_type','top','$.num_coins',2),transient_data=?2,started_at='2026-09-10 10:00:00' WHERE id=?3",
                        (stored["settings"].to_string(),stored["transient"].to_string(),f.id)).map_err(|e| e.to_string())?;
            let draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e| format!("{e:?}"))?.ok_or("draft")?;
            Ok(write::pending(&f.c,&draft.original,harness::at(NOW)).map_err(|e| format!("{e:?}"))?.to_s())
        };
        let plain=pending(json!({}),json!({}))?;
        assert_ne!(plain,"0","the unflagged index bot owes something");
        assert_eq!(pending(json!({"price_limited":true}),json!({}))?,"0","unmet: the decorated started_at is nil, nothing owed");
        for (name,settings,transient) in [
            ("flip",json!({"price_limited":true,"price_limit_action":"start_selling"}),json!({})),
            ("sell side",json!({"sell_price_limited":true,"sell_price_drop_limited":true}),json!({})),
            ("met before the start",json!({"price_limited":true}),json!({"price_limit_condition_met_at":"2026-09-01T00:00:00Z"})),
        ] { assert_eq!(pending(settings,transient)?,plain,"{name}"); }
        Ok(())
    }

    #[test]
    fn action_write_carry_caps_an_index_bot_by_its_spending_cap() -> Result {
        // D-IDX-CAP (reverses #752): the cap, its figure and the page refusal of a cap below the quote's smallest amount are an
        // index bot's as a basket's.
        let read=|settings:Value| -> std::result::Result<(String,bool,Option<&'static str>),String> {
            let f=Fixture::new().map_err(|e| e.to_string())?;
            let mut stored=f.stored().map_err(|e| e.to_string())?;
            stored["settings"].as_object_mut().ok_or("settings")?.extend(settings.as_object().ok_or("settings")?.clone());
            stored["transient"]["missed_quote_amount"]=json!("2.5");
            stored["transient"]["quote_amount_limit_enabled_at"]=json!("2026-09-01T00:00:00.000Z");
            f.c.execute("UPDATE bots SET type='Bots::DcaIndex',settings=json_set(?1,'$.index_type','top','$.num_coins',2),transient_data=?2,started_at='2026-09-10 10:00:00' WHERE id=?3",
                        (stored["settings"].to_string(),stored["transient"].to_string(),f.id)).map_err(|e| e.to_string())?;
            let draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e| format!("{e:?}"))?.ok_or("draft")?;
            let pending=write::pending(&f.c,&draft.original,harness::at(NOW)).map_err(|e| format!("{e:?}"))?.to_s();
            let capped=deltabadger::web::bot::start::amount_limit(&f.c,&draft.original).map_err(|e| format!("{e:?}"))?.is_some();
            Ok((pending,capped,draft.original.unrendered()))
        };
        let plain=read(json!({"quote_amount_limited":false}))?;
        assert_ne!(plain.0,"0","the unflagged index bot owes something");
        assert!(plain.0.parse::<f64>().map_err(|e| e.to_string())? > 1.0, "{}", plain.0);
        assert_eq!((plain.1,plain.2),(false,None));
        let one=read(json!({"quote_amount_limited":true,"quote_amount_limit":1.0}))?;
        assert_eq!((one.0.parse::<f64>().map_err(|e| e.to_string())?,one.1,one.2),(1.0,true,None),"the carry is cut to the cap");
        let tiny=read(json!({"quote_amount_limited":true,"quote_amount_limit":0.000001}))?;
        assert_eq!((tiny.1,tiny.2),(true,Some("a spending cap below the smallest amount its quote states")));
        Ok(())
    }

    #[tokio::test]
    async fn action_write_cancelled_awaiter_still_wakes_once() -> Result {
        let f=Fixture::new()?;
        let notify=Arc::new(tokio::sync::Notify::new()); f.ctx.app.attach_engine(notify.clone());
        let (entered_tx,entered_rx)=tokio::sync::oneshot::channel();
        let (release_tx,release_rx)=std::sync::mpsc::channel();
        let ctx=f.ctx.clone(); let app=ctx.app.clone(); let owner=f.seed.user_id; let id=f.id;
        let params=Fixture::params(json!({"quote_amount":"7"}))?;
        let job=tokio::spawn(async move { app.db(move |c| write::settings(c,&ctx,owner,id,&params,|_,_,_| {
            let _=entered_tx.send(());
            release_rx.recv_timeout(std::time::Duration::from_secs(10)).map_err(|e| WebError::Task(e.to_string()))?;
            Ok(Prepared {response:(),broadcasts:vec![], after_scheduling: None })
        })).await });
        tokio::time::timeout(std::time::Duration::from_secs(10),entered_rx).await??;
        job.abort(); let _=job.await;
        release_tx.send(())?;
        tokio::time::timeout(std::time::Duration::from_secs(10),notify.notified()).await?;
        // A DB barrier proves the entire post-commit tail completed before checking a duplicate.
        f.ctx.app.db(|_|Ok(())).await.map_err(|e|format!("{e:?}"))?;
        assert_eq!(f.stored()?["settings"]["quote_amount"],json!(7.0));
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20),notify.notified()).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn action_write_real_guard_variants_and_no_notifications() -> Result {
        for kind in ["ineligible","unreadable","reconciling","untradable","failed"] {
            let f=Fixture::new()?;
            let (eth,_)=seed::add_eth_sol(&f.c,&f.seed);
            match kind {
                "ineligible" => { f.c.execute("INSERT INTO rules(type,status,user_id,created_at,updated_at) VALUES('Rule',1,?1,'2026-01-01','2026-01-01')",[f.seed.user_id])?; },
                "unreadable" => { let id=seed::insert_bot(&f.c,&f.seed,&seed::BotSpec::weekly(5.0,"2026-01-01 00:00:00")); f.c.execute("UPDATE bots SET settings='{}' WHERE id=?1",[id])?; },
                "reconciling" => { f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement',json(?1)) WHERE id=?2",(json!({"allocations":{f.seed.btc.to_string():1.0},"exchange_id":f.seed.exchange_id,"quote_asset_id":f.seed.quote}).to_string(),f.id))?; },
                "untradable" => { f.c.execute("UPDATE bots SET status=1 WHERE id=?1",[f.id])?; f.c.execute("DELETE FROM api_keys",[])?; },
                "failed" => { f.c.execute_batch("DROP TABLE rules")?; },
                _=>return Err("guard variant".into()),
            }
            let notify=Arc::new(tokio::sync::Notify::new()); f.ctx.app.attach_engine(notify.clone());
            let before=f.snapshot()?;
            let fields=if kind=="reconciling" {json!({"add_asset_id":eth.to_string()})} else {json!({"quote_amount":"7"})};
            let Outcome::GuardRefused(response)=f.write(fields)? else {return Err(format!("{kind}: expected guard refusal").into())};
            assert!(!response["errors"].as_array().ok_or("errors")?.is_empty());
            if kind=="failed" { assert!(response.to_string().contains("the check could not be completed")); }
            assert_eq!(f.snapshot()?,before,"{kind}");
            assert!(tokio::time::timeout(std::time::Duration::from_millis(10),notify.notified()).await.is_err());
        }
        Ok(())
    }

    #[test]
    fn action_write_ownership_and_stored_bounds() -> Result {
        let f=Fixture::new()?;
        let p=Fixture::params(json!({"quote_amount":"7"}))?;
        let before=f.snapshot()?;
        for (owner,id) in [(f.seed.user_id+1,f.id),(f.seed.user_id,i64::MAX)] {
            let out=write::settings::<()>(&f.c,&f.ctx,owner,id,&p,|_,_,_|panic!("foreign bot rendered")).map_err(|e|format!("{e:?}"))?;
            assert!(matches!(out,Outcome::Missing)); assert_eq!(f.snapshot()?,before);
        }
        f.c.execute("UPDATE bots SET status=3 WHERE id=?1",[f.id])?;
        assert!(matches!(f.write(json!({"quote_amount":"7"}))?,Outcome::Missing));
        f.c.execute("UPDATE bots SET status=2 WHERE id=?1",[f.id])?;
        let mut deep=json!(0); for _ in 0..34 { deep=json!({"x":deep}); }
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.deep',json(?1)) WHERE id=?2",(deep.to_string(),f.id))?;
        let before=f.snapshot()?;
        assert!(matches!(f.write(json!({"label":"bound"}))?,Outcome::Unported(_)));
        assert_eq!(f.snapshot()?,before);
        Ok(())
    }

    #[tokio::test]
    async fn action_write_broadcasts_follow_commit_only() -> Result {
        use futures_util::{SinkExt,StreamExt};
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest,Message};
        use deltabadger::web::{cable,session};
        let f=Fixture::new()?; f.stabilize()?;
        let hash="$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
        f.c.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00'",[hash])?;
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address=listener.local_addr()?;
        let app=f.ctx.app.clone();
        struct Server(tokio::task::JoinHandle<()>);
        impl Drop for Server { fn drop(&mut self) { self.0.abort(); } }
        let _server=Server(tokio::spawn(async move { let _=axum::serve(listener,web::router(app).into_make_service_with_connect_info::<std::net::SocketAddr>()).await; }));
        let cookie=session::seal(&f.ctx.app.keys.session,&session::SessionData {user:Some((f.seed.user_id,hash.chars().take(29).collect())),..Default::default()},f.ctx.app.now());
        let mut request=format!("ws://{address}/cable").into_client_request()?;
        request.headers_mut().insert("origin",format!("http://{address}").parse()?);
        request.headers_mut().insert("sec-websocket-protocol","actioncable-v1-json".parse()?);
        request.headers_mut().insert("cookie",format!("_deltabadger_rust_session={cookie}").parse()?);
        let (mut socket,_)=tokio::time::timeout(std::time::Duration::from_secs(5),tokio_tungstenite::connect_async(request)).await??;
        let stream=format!("user_{}:bot_{}",f.seed.user_id,f.id);
        let signed=cable::signed_stream_name(&f.ctx.app.keys.streams,&stream);
        let identifier=json!({"channel":"Turbo::StreamsChannel","signed_stream_name":signed}).to_string();
        socket.send(Message::Text(json!({"command":"subscribe","identifier":identifier}).to_string().into())).await?;
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            while let Some(message)=socket.next().await {
                let value:Value=serde_json::from_str(message?.to_text()?)?;
                if value["type"]=="confirm_subscription" { return Ok::<(),Box<dyn std::error::Error>>(()); }
            }
            Err("socket closed".into())
        }).await??;
        for (kind,fields) in [("success",json!({"quote_amount":"7"})),("noop",json!({"quote_amount":"7"})),("invalid",json!({"quote_amount":"0"})),("commit",json!({"quote_amount":"8"})),("renderer",json!({"quote_amount":"8"})),("guard",json!({"quote_amount":"8"}))] {
            if kind=="commit" { f.c.execute_batch("CREATE TABLE commit_parent(id INTEGER PRIMARY KEY); CREATE TABLE commit_child(id INTEGER REFERENCES commit_parent(id) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_commit AFTER UPDATE ON bots BEGIN INSERT INTO commit_child VALUES(42); END;")?; }
            if kind=="guard" {f.c.execute_batch("DROP TABLE rules")?;}
            let before=f.snapshot()?;
            let result=write::settings(&f.c,&f.ctx,f.seed.user_id,f.id,&Fixture::params(fields)?,|_,_,_| {
                if kind=="renderer" {return Err(WebError::Config("renderer failed".into()));}
                Ok(Prepared {response:(),broadcasts:vec![(stream.clone(),"committed".into())],after_scheduling:None})
            });
            if kind=="success" { assert!(matches!(result,Ok(Outcome::Committed(_)))); }
            else { assert!(!matches!(result,Ok(Outcome::Committed(_)))); assert_eq!(f.snapshot()?,before); }
            if kind=="commit" {f.c.execute_batch("DROP TRIGGER fail_commit")?;}
            f.ctx.app.hub.broadcast(&stream,"barrier");
            let mut messages=vec![];
            tokio::time::timeout(std::time::Duration::from_secs(5),async {
                while let Some(message)=socket.next().await {
                    let value:Value=serde_json::from_str(message?.to_text()?)?;
                    if let Some(text)=value["message"].as_str() {
                        if text=="barrier" {return Ok::<(),Box<dyn std::error::Error>>(());}
                        messages.push(text.to_string());
                    }
                }
                Err("socket closed".into())
            }).await??;
            assert_eq!(messages,if kind=="success" {vec!["committed"]} else {vec![]},"{kind}");
        }
        socket.close(None).await?;
        Ok(())
    }

    #[test]
    fn action_write_stopped_stock_and_index_vectors() -> Result {
        let f=Fixture::new()?;
        // Reuse the recorder's complete stock/index model rows. No fake guard: every bot is
        // stopped, and this fixture has no outstanding unsupported venue work.
        f.c.execute_batch("PRAGMA foreign_keys=OFF; DELETE FROM api_keys; DELETE FROM bot_index_assets; DELETE FROM bots; DELETE FROM exchange_assets; DELETE FROM tickers; DELETE FROM assets; DELETE FROM exchanges; DELETE FROM users;")?;
        let vectors=common::vectors();
        let group=&vectors["bot_actions"];
        for (table,rows) in group["rows"].as_object().ok_or("rows")? {
            for row in rows.as_array().ok_or("table rows")? {
                let row=row.as_object().ok_or("row")?;
                let columns=row.keys().map(|k|format!("\"{k}\"")).collect::<Vec<_>>().join(",");
                let values=row.values().map(|v|match v {
                    Value::Null=>rusqlite::types::Value::Null,
                    Value::Bool(b)=>rusqlite::types::Value::Integer(i64::from(*b)),
                    Value::Number(n)=>n.as_i64().map(rusqlite::types::Value::Integer).unwrap_or_else(||rusqlite::types::Value::Real(n.as_f64().unwrap_or_default())),
                    Value::String(s)=>rusqlite::types::Value::Text(s.clone()),
                    v=>rusqlite::types::Value::Text(v.to_string()),
                }).collect::<Vec<_>>();
                f.c.execute(&format!("INSERT INTO {table} ({columns}) VALUES ({})",vec!["?";values.len()].join(",")),rusqlite::params_from_iter(values))?;
            }
        }
        f.c.execute_batch("PRAGMA foreign_keys=ON; UPDATE users SET wash_sale_enabled=0; UPDATE bots SET status=2; DELETE FROM transactions WHERE external_status IN (0,1);")?;
        for (key,value) in [("market_data_provider","deltabadger"),("market_data_url","http://example.test"),("market_data_token","test")] {
            let encrypted=f.ctx.app.cipher.encrypt(value);
            f.c.execute("INSERT INTO app_configs(key,value,created_at,updated_at) VALUES(?1,?2,'2026-01-01','2026-01-01')",(key,encrypted))?;
        }
        for id in [1,2,3] {
            let bot=Draft::load(&f.c,1,id,"en").map_err(|e|format!("{e:?}"))?.ok_or("bot")?;
            let root=bot.original.param_key();
            let p=Params { upload: None,json:Some(json!({root:{"quote_amount":"37.25"}})),full_path:String::new(),fullpath:String::new(),route_path:String::new(),path_locale:None,query:vec![],form:vec![]};
            let params=ActionParams::parse(&p).map_err(|e|format!("{e:?}"))?;
            let out=write::settings(&f.c,&f.ctx,1,id,&params,|_,_,draft|Ok(Prepared {response:draft.candidate.settings.clone(),broadcasts:vec![], after_scheduling: None })).map_err(|e|format!("{e:?}"))?;
            let Outcome::Committed(settings)=out else {return Err(format!("stock shape {id} did not commit").into())};
            assert_eq!(settings.get("quote_amount"),Some(&json!(37.25)));
        }
        // A moved index slider writes settings and leaves every membership byte unchanged.
        let before=f.snapshot()?.into_iter().filter(|r|r.starts_with("bot_index_assets:")).collect::<Vec<_>>();
        let p=Params { upload: None,json:Some(json!({"bots_dca_index":{"num_coins":"5","num_coins_rendered":"10","num_coins_ceiling":"5"}})),full_path:String::new(),fullpath:String::new(),route_path:String::new(),path_locale:None,query:vec![],form:vec![]};
        let params=ActionParams::parse(&p).map_err(|e|format!("{e:?}"))?;
        let outcome=write::settings(&f.c,&f.ctx,1,2,&params,|_,_,_|Ok(Prepared {response:(),broadcasts:vec![], after_scheduling: None })).map_err(|e|format!("{e:?}"))?;
        assert!(matches!(outcome,Outcome::Committed(_)));
        assert_eq!(f.snapshot()?.into_iter().filter(|r|r.starts_with("bot_index_assets:")).collect::<Vec<_>>(),before);
        Ok(())
    }

    impl Fixture {
        fn lifecycle(&self, action: write::Action, flag: Option<Value>) -> Result<Outcome<Value>> {
            let p = Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: Some(flag.map_or_else(|| json!({}), |v| json!({"start_fresh":v}))) };
            let params = ActionParams::parse(&p).map_err(|e|format!("{e:?}"))?;
            Ok(write::lifecycle(&self.c, &self.ctx, self.seed.user_id, self.id, action, &params, |_, ctx, view| {
                assert_eq!(ctx.now, harness::at(NOW));
                Ok(Prepared { response: json!({"errors":view.errors.iter().map(|e| &e.message).collect::<Vec<_>>(),"minimal":view.draft.is_none()}), broadcasts: vec![], after_scheduling: None })
            }).map_err(|e|format!("{e:?}"))?)
        }
        fn status(&self) -> Result<i64> { Ok(self.c.query_row("SELECT status FROM bots WHERE id=?1",[self.id],|r|r.get(0))?) }
    }

    /// A Stop through the services (REST, MCP) commits whatever the answer can show: an unnamed bot whose page cannot be
    /// read (a stored time it cannot parse) is stopped all the same, answered under its stored name; a named one too.
    #[test]
    fn service_stop_survives_an_unrenderable_label() -> Result {
        for label in ["NULL", "'Named'"] {
            let f=Fixture::new()?;
            f.c.execute(&format!("UPDATE bots SET type='Bots::DcaIndex', label={label}, status=1, settings_changed_at='garbage' WHERE id=?1"),[f.id])?;
            let submitted=ActionParams::from_mcp(json!({})).map_err(|e|format!("{e:?}"))?;
            let out=web::bot_api::bot_write(&f.c,&f.ctx.app,f.seed.user_id,f.id,web::bot_api::Write::Lifecycle(write::Action::Stop),&submitted).map_err(|e|format!("{e:?}"))?;
            let Outcome::Committed(shown)=out else { return Err(format!("the stop of a bot labelled {label} was not committed").into()) };
            assert_eq!(shown.data["status"],"stopped");
            assert_eq!(f.status()?,2);
        }
        Ok(())
    }

    async fn lifecycle_wake(notify: &tokio::sync::Notify, expected: bool) -> Result {
        assert_eq!(tokio::time::timeout(std::time::Duration::from_millis(5), notify.notified()).await.is_ok(), expected);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(5), notify.notified()).await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn action_lifecycle_forty_status_transitions() -> Result {
        use write::Action::*;
        let mut cases = 0;
        for action in [Start, Stop, Delete, Archive, Unarchive] {
            for status in 0..8 {
                let f = Fixture::new()?;
                f.c.execute("UPDATE bots SET status=?1, stopped_at='2026-01-02 00:00:00', stop_message_key='old' WHERE id=?2",(status,f.id))?;
                let before = f.snapshot()?;
                let notify = Arc::new(tokio::sync::Notify::new()); f.ctx.app.attach_engine(notify.clone());
                let result = f.lifecycle(action,None)?;
                let no_change = status == 3 || action == Start && ![0,2].contains(&status) || action == Archive && status == 7 || action == Unarchive && status != 7;
                if status == 3 { assert!(matches!(result,Outcome::Missing)); }
                else if action == Start && ![0,2].contains(&status) { assert!(matches!(result,Outcome::Invalid(..)),"{action:?} {status}"); }
                else if no_change { assert!(matches!(result,Outcome::NoChange(_))); }
                else { assert!(matches!(result,Outcome::Committed(_)),"{action:?} {status}"); }
                if no_change { assert_eq!(f.snapshot()?,before,"{action:?} {status}"); }
                else {
                    let expected = match action { Start=>1, Stop|Unarchive=>2, Delete=>3, Archive=>7 };
                    assert_eq!(f.status()?,expected);
                    let (stopped,message,updated):(Option<String>,Option<String>,String) = f.c.query_row("SELECT stopped_at,stop_message_key,updated_at FROM bots WHERE id=?1",[f.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
                    assert_eq!(updated,"2026-09-10 12:00:30.123456");
                    assert_eq!(stopped.as_deref(),Some(if matches!(action,Stop|Archive) { "2026-09-10 12:00:30.123456" } else { "2026-01-02 00:00:00" }));
                    assert_eq!(message.as_deref(),if matches!(action,Delete|Unarchive) {Some("old")} else {None});
                    let logs: i64=f.c.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1",[f.id],|r|r.get(0))?;
                    assert_eq!(logs,if matches!(action,Start|Stop|Archive){1}else{0});
                    if logs==1 {
                        let (event,level,message,details,at):(String,i64,Option<String>,String,String)=f.c.query_row("SELECT event,level,message,details,created_at FROM bot_activity_logs WHERE bot_id=?1",[f.id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))?;
                        assert_eq!((event.as_str(),level,message),(if action==Start {"started"}else{"stopped"},0,None));
                        assert_eq!(serde_json::from_str::<Value>(&details)?,if action==Start {json!({"start_fresh":true})}else{json!({})});
                        assert_eq!(at,updated);
                    }
                }
                lifecycle_wake(&notify,!no_change).await?;
                cases+=1;
            }
        }
        assert_eq!(cases,40);
        Ok(())
    }

    #[tokio::test]
    async fn action_lifecycle_start_flags_carry_and_handoff() -> Result {
        for status in [0,2] {
            for paid in [false,true] {
                // A JSON null starts fresh, as no flag does (Rails reads params[:start_fresh].nil?).
                for flag in [None,Some(Value::Null),Some(json!(true)),Some(json!(false)),Some(json!("TRUE")),Some(json!("0"))] {
                    let f=Fixture::new()?;
                    f.c.execute("UPDATE bots SET status=?1,transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-10T12:00:00.000Z','$.missed_quote_amount','3.75') WHERE id=?2",(status,f.id))?;
                    if paid { seed::insert_tx(&f.c,&f.seed,f.id,&seed::TxSpec {status:0,external_status:Some(2),external_id:Some("paid".into()),order_type:0,amount:Some("0.001"),quote_amount:Some("10"),price:Some("10000"),quote_amount_exec:Some("10"),amount_exec:Some("0.001"),created_at:"2026-09-10 12:00:01".into()}); }
                    let before=f.stored()?;
                    let fresh=flag!=Some(json!(false)) && flag!=Some(json!("0"));
                    assert!(matches!(f.lifecycle(write::Action::Start,flag)?,Outcome::Committed(_)));
                    let after=f.stored()?;
                    let mut expected=before["transient"].clone();
                    expected["run_generation"]=json!(1);
                    if fresh { expected["last_action_job_at"]=Value::Null; expected["missed_quote_amount"]=Value::Null; }
                    else { expected["rust_continue_start"]=json!({"requested_at":NOW,"was_stopped":status==2}); }
                    assert_eq!(after["transient"],expected);
                    let anchor:String=f.c.query_row("SELECT started_at FROM bots WHERE id=?1",[f.id],|r|r.get(0))?;
                    assert_eq!(anchor,if fresh {"2026-09-10 12:00:30.123456"}else{"2026-09-10 12:00:00"});
                }
            }
        }
        for flag in [json!("bad"),json!([]),json!({})] {
            let f=Fixture::new()?; let before=f.snapshot()?;
            assert!(matches!(f.lifecycle(write::Action::Start,Some(flag))?,Outcome::Invalid(..)));
            assert_eq!(f.snapshot()?,before);
        }
        Ok(())
    }

    #[tokio::test]
    async fn action_lifecycle_failure_is_atomic_and_never_wakes() -> Result {
        for (action,sql) in [
            (write::Action::Start,"CREATE TRIGGER fail_log BEFORE INSERT ON bot_activity_logs BEGIN SELECT RAISE(ABORT,'log failure'); END"),
            (write::Action::Archive,"CREATE TRIGGER fail_archive BEFORE UPDATE OF status ON bots WHEN NEW.status=7 BEGIN SELECT RAISE(ABORT,'archive failure'); END"),
            (write::Action::Start,"UPDATE bots SET transient_data=json_set(transient_data,'$.rebalance_pending',json('true'))"),
        ] {
            let f=Fixture::new()?; f.c.execute_batch(sql)?; let before=f.snapshot()?;
            let notify=Arc::new(tokio::sync::Notify::new()); f.ctx.app.attach_engine(notify.clone());
            let result=f.lifecycle(action,Some(json!(false)));
            assert!(result.is_err() || matches!(result,Ok(Outcome::GuardRefused(_)|Outcome::Unported(_))));
            assert_eq!(f.snapshot()?,before); lifecycle_wake(&notify,false).await?;
        }
        let f=Fixture::new()?; let before=f.snapshot()?;
        let notify=Arc::new(tokio::sync::Notify::new()); f.ctx.app.attach_engine(notify.clone());
        let result=write::lifecycle::<()>(&f.c,&f.ctx,f.seed.user_id,f.id,write::Action::Start,&Fixture::params(json!({}))?,|_,_,_|Err(WebError::Config("render failed".into())));
        assert!(result.is_err()); assert_eq!(f.snapshot()?,before); lifecycle_wake(&notify,false).await?;
        Ok(())
    }

    #[tokio::test]
    async fn action_lifecycle_start_key_and_validation_refusals() -> Result {
        for sql in [
            "DELETE FROM api_keys",
            "UPDATE api_keys SET status=0",
            "UPDATE api_keys SET status=2",
            r#"UPDATE api_keys SET key='{"p":"!!!","h":{"iv":"!!!","at":"!!!"}}'"#,
            "UPDATE tickers SET available=0",
            "UPDATE bots SET settings=json_set(settings,'$.allocations',json('{}'))",
            "UPDATE bots SET settings=json_set(settings,'$.allocations',json('{\"999999\":1.0}'))",
            "UPDATE bots SET settings=json_set(settings,'$.quote_amount',0)",
            "UPDATE bots SET settings=json_set(settings,'$.start_time_enabled',json('true'),'$.start_time_mode','date','$.start_at','2026-09-01T00:00:00Z')",
            "UPDATE assets SET category='Stock' WHERE symbol='BTC'",
            "UPDATE bots SET type='Bots::DcaIndex'",
        ] {
            let f=Fixture::new()?; f.c.execute_batch(sql)?; let before=f.snapshot()?;
            let notify=Arc::new(tokio::sync::Notify::new()); f.ctx.app.attach_engine(notify.clone());
            let result=f.lifecycle(write::Action::Start,Some(json!(false)))?;
            if sql.contains("SET key=") || sql.contains("category='Stock'") {
                assert!(matches!(result,Outcome::GuardRefused(_)),"guard must roll back the continue request: {sql}");
            } else {
                assert!(matches!(result,Outcome::Invalid(..)|Outcome::GuardRefused(_)|Outcome::Unported(_)),"{sql}");
            }
            assert_eq!(f.snapshot()?,before,"{sql}"); lifecycle_wake(&notify,false).await?;
        }
        Ok(())
    }

    /// The delayed start as Bot::Lifecycle#start writes it, at the same clock (fixtures/start_time_vectors.json, recorded
    /// at NOW in UTC): the anchor, start_at, and the carry capture, which lands only when start_at changed (hour mode) and
    /// is discarded when it did not (date mode: the stored start_at is written back unchanged).
    #[test]
    fn action_lifecycle_delayed_date_hour_start_as_rails_writes_it() -> Result {
        let vectors:Value=serde_json::from_str(include_str!("fixtures/start_time_vectors.json"))?;
        for (mode,time,rails) in [("date","2026-09-11T13:45:00Z",&vectors["schedule"][0]["started"]),("hour","13:45",&vectors["schedule"][3]["started"])] {
            let f=Fixture::new()?;
            f.c.execute("UPDATE users SET time_zone='UTC'",[])?;
            f.c.execute("UPDATE bots SET settings=json_set(settings,'$.start_time_enabled',json('true'),'$.start_time_mode',?1,'$.start_at',?2,'$.start_time_of_day',?2)",(mode,time))?;
            if mode=="hour" { f.c.execute("UPDATE bots SET settings=json_set(settings,'$.start_at','2026-09-11T13:45:00Z')",[])?; }
            // transient_data exactly as the recorded bot had it before its start.
            let created=&vectors["schedule"][if mode=="date" {0} else {3}]["created"];
            f.c.execute("UPDATE bots SET transient_data=?1",[created.to_string()])?;
            let changed_before=f.stored()?["changed"].clone();
            assert!(matches!(f.lifecycle(write::Action::Start,None)?,Outcome::Committed(_)),"{mode}");
            let stored=f.stored()?;
            let anchor:String=f.c.query_row("SELECT started_at FROM bots WHERE id=?1",[f.id],|r|r.get(0))?;
            assert_eq!(anchor,rails["started_at"].as_str().ok_or("started_at")?,"{mode}");
            assert_eq!(stored["settings"]["start_at"],rails["settings"]["start_at"],"{mode}");
            let mut expected_transient = rails["transient_data"].clone();
            assert!(expected_transient.get("run_generation").is_none(), "Rails has no ownership generation");
            expected_transient["run_generation"] = json!(1);
            assert_eq!(stored["transient"], expected_transient, "{mode}: Rails state plus exactly one Rust start generation");
            let status:i64=f.c.query_row("SELECT status FROM bots WHERE id=?1",[f.id],|r|r.get(0))?;
            assert_eq!(json!(status),rails["status"],"{mode}");
            let changed=if mode=="date" {changed_before} else {json!(rails["settings_changed_at"])};
            assert_eq!(stored["changed"],changed,"{mode}: the window");
            let report=eligibility::check_install(&f.c).map_err(|e|format!("{e:?}"))?;
            assert!(report.eligible.contains(&f.id),"{mode}: the engine runs it: {:?}",report.problems);
        }
        Ok(())
    }

    /// A continue never honours a starting time in Rails (it may buy at once): the write guard refuses it.
    #[test]
    fn action_lifecycle_continue_with_a_starting_time_is_refused() -> Result {
        let f=Fixture::new()?;
        // As a delayed start leaves it (start_at = started_at), stopped before its first run.
        f.c.execute("UPDATE bots SET status=2,settings=json_set(settings,'$.start_time_enabled',json('true'),'$.start_time_mode','hour','$.start_time_of_day','13:45','$.start_at','2026-09-10T12:00:00Z'),transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-01T00:00:00.000Z') WHERE id=?1",[f.id])?;
        let before=f.snapshot()?;
        assert!(matches!(f.lifecycle(write::Action::Start,Some(json!(false)))?,Outcome::GuardRefused(_)));
        assert_eq!(f.snapshot()?,before);
        Ok(())
    }

    /// Bot::Lifecycle#start (fresh) drops the reserves a reversal left waiting for their buy's final state, with the carry;
    /// a continue keeps them.
    #[test]
    fn action_lifecycle_fresh_start_discards_waiting_flip_reserves() -> Result {
        for (fresh,kept) in [(true,false),(false,true)] {
            let f=Fixture::new()?;
            f.c.execute("UPDATE bots SET status=2, transient_data=json_set(transient_data,'$.flip_reserve_pending',json('[7]')) WHERE id=?1",[f.id])?;
            // The flag is a top-level param (Bots::StartsController reads params[:start_fresh]), never a form field.
            let _=f.lifecycle(write::Action::Start,Some(json!(fresh)))?;
            let status:i64=f.c.query_row("SELECT status FROM bots WHERE id=?1",[f.id],|r|r.get(0))?;
            assert_eq!(status,1,"fresh={fresh}: the start went through");
            let pending:Option<String>=f.c.query_row("SELECT json_extract(transient_data,'$.flip_reserve_pending') FROM bots WHERE id=?1",[f.id],|r|r.get(0))?;
            assert_eq!(pending.is_some(),kept,"fresh={fresh}: {pending:?}");
        }
        Ok(())
    }

    /// Rails steps a passed time forward in fixed UTC days: across the autumn change, a day that starts an hour early.
    #[test]
    fn action_lifecycle_start_an_hour_early_is_refused() -> Result {
        let f=Fixture::new()?;
        f.c.execute("UPDATE users SET time_zone='Warsaw'",[])?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.start_time_enabled',json('true'),'$.start_time_mode','hour','$.start_time_of_day','09:30') WHERE id=?1",[f.id])?;
        // Saturday 2026-10-24, 10:00 in Warsaw: 09:30 has passed, and a fixed day later is 08:30 after the change.
        let clock=harness::TestClock::at("2026-10-24T08:00:00Z");
        let mut ctx=f.ctx.clone(); ctx.app=harness::app(f._dir.path(),"engine-test-secret",clock.clone());
        let before=f.snapshot()?;
        let result=write::lifecycle(&f.c,&ctx,f.seed.user_id,f.id,write::Action::Start,&Fixture::params(json!({}))?,|_,_,view| {
            Ok(Prepared {response:json!(view.errors.iter().map(|e|e.message.clone()).collect::<Vec<_>>()),broadcasts:vec![], after_scheduling: None })
        }).map_err(|e|format!("{e:?}"))?;
        let Outcome::GuardRefused(errors)=result else { return Err("an hour-early start must be refused".into()) };
        assert!(errors.to_string().contains("daylight-saving"),"{errors}");
        assert_eq!(f.snapshot()?,before);
        // The spring change makes it an hour late (10:30 local): not the chosen time either, refused without a write.
        clock.set(harness::at("2026-03-28T09:00:00Z"));
        assert!(matches!(write::lifecycle(&f.c,&ctx,f.seed.user_id,f.id,write::Action::Start,&Fixture::params(json!({}))?,|_,_,_| Ok(Prepared {response:(),broadcasts:vec![], after_scheduling: None })).map_err(|e|format!("{e:?}"))?,Outcome::GuardRefused(_)));
        assert_eq!(f.snapshot()?,before);
        Ok(())
    }

    /// The date field renders in the owner's zone with the resolver that parses it (a Rails name or an IANA id): turning the
    /// rule on resubmits the rendered field, which keeps the instant, and the start runs at it.
    #[test]
    fn action_date_start_round_trips_through_the_form_in_the_owners_zone() -> Result {
        for zone in ["Asia/Tokyo","Tokyo"] {
            let f=Fixture::new()?;
            f.c.execute("UPDATE users SET time_zone=?1",[zone])?;
            assert!(matches!(f.write(json!({"start_time_mode":"date","start_at":"2026-09-12T09:30"}))?,Outcome::Committed(_)),"{zone}");
            assert_eq!(f.stored()?["settings"]["start_at"],json!("2026-09-12T00:30:00Z"),"{zone}");
            let draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e|format!("{e:?}"))?.ok_or("draft")?;
            let html=web::bot::settings::draft_column(&f.c,&f.ctx,"csrf",&draft,zone,false).map_err(|e|format!("{e:?}"))?;
            let field=html.split("sinput--datetime").next().and_then(|before|before.rsplit("value=\"").next()).and_then(|v|v.split('"').next()).ok_or("date field")?.to_owned();
            assert_eq!(field,"2026-09-12T09:30","{zone}: the field shows the owner's wall time");
            assert!(matches!(f.write(json!({"start_time_enabled":"1","start_time_mode":"date","start_at":field}))?,Outcome::Committed(_)|Outcome::NoChange(_)),"{zone}");
            assert_eq!(f.stored()?["settings"]["start_at"],json!("2026-09-12T00:30:00Z"),"{zone}: resubmitting keeps the instant");
            assert!(matches!(f.lifecycle(write::Action::Start,None)?,Outcome::Committed(_)),"{zone}");
            let anchor:String=f.c.query_row("SELECT started_at FROM bots WHERE id=?1",[f.id],|r|r.get(0))?;
            assert_eq!(anchor,"2026-09-12 00:30:00","{zone}");
        }
        Ok(())
    }

    /// ActiveSupport::TimeZone[] knows Rails' names and IANA identifiers; for any other name the start is refused and nothing
    /// is written, never computed in UTC.
    #[test]
    fn action_lifecycle_start_in_an_unknown_zone_is_refused() -> Result {
        let f=Fixture::new()?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.start_time_enabled',json('true'),'$.start_time_mode','hour','$.start_time_of_day','13:45') WHERE id=?1",[f.id])?;
        f.c.execute("UPDATE users SET time_zone='Mars/Olympus'",[])?;
        let before=f.snapshot()?;
        let Err(error)=f.lifecycle(write::Action::Start,None) else { return Err("a start in an unknown zone must be refused".into()) };
        assert!(error.to_string().contains("unknown time zone"),"{error}");
        assert_eq!(f.snapshot()?,before);
        // The IANA name of the same kind is read.
        f.c.execute("UPDATE users SET time_zone='Europe/Warsaw'",[])?;
        assert!(matches!(f.lifecycle(write::Action::Start,None)?,Outcome::Committed(_)));
        Ok(())
    }

    #[test]
    fn action_lifecycle_stop_delete_unrenderable_and_preserve_orders() -> Result {
        for action in [write::Action::Stop,write::Action::Delete] {
            let f=Fixture::new()?;
            f.c.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount',0,'$.unknown',json('{\"keep\":null}'))",[])?;
            assert!(matches!(f.lifecycle(action,None)?,Outcome::Committed(_)));
            assert_eq!(f.stored()?["settings"]["unknown"],json!({"keep":null}));
        }
        let f=Fixture::new()?;
        seed::insert_tx(&f.c,&f.seed,f.id,&seed::TxSpec {status:0,external_status:Some(0),external_id:Some("in-flight".into()),order_type:0,amount:Some("0.001"),quote_amount:Some("5"),price:Some("5000"),quote_amount_exec:None,amount_exec:None,created_at:"2026-09-10 12:00:01".into()});
        let before=f.snapshot()?.into_iter().filter(|s|s.starts_with("transactions:")).collect::<Vec<_>>();
        assert!(matches!(f.lifecycle(write::Action::Stop,None)?,Outcome::Committed(_)));
        assert_eq!(before,f.snapshot()?.into_iter().filter(|s|s.starts_with("transactions:")).collect::<Vec<_>>());
        Ok(())
    }

    #[test]
    fn action_lifecycle_start_time_matches_rails_vectors() -> Result {
        let vectors=common::vectors();
        let cases=vectors.get("action_lifecycle").and_then(Value::as_array).ok_or("missing lifecycle vectors")?;
        assert!(cases.len()>=6);
        let f=Fixture::new()?;
        for case in cases {
            let mut draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e|format!("{e:?}"))?.ok_or("draft missing")?;
            draft.candidate.settings.extend(case["settings"].as_object().ok_or("settings missing")?.clone());
            let now=case["now"].as_str().ok_or("now missing")?.parse()?;
            let actual=match draft.initial_start_at(now,case["zone"].as_str().ok_or("zone missing")?) {
                // A wall time that repeats in the owner's zone is refused, never resolved to Rails' pick.
                Err(e) if format!("{e:?}").contains("occurs twice") => continue,
                other => other.map_err(|e|format!("{e:?}"))?,
            };
            assert_eq!(actual.map(|t|t.at.to_rfc3339_opts(chrono::SecondsFormat::Secs,true)),case["expected"].as_str().map(str::to_owned),"{case}");
        }
        Ok(())
    }

    /// B2-1 normalization takes precedence over #503's raw-NULL cap refusal:
    /// the accepted quantity/price prove $5 spent, so $995 remains and start succeeds.

    #[test]
    fn action_lifecycle_reconstructed_cap_spend_allows_start() -> Result {
        let f=Fixture::new()?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'),'$.quote_amount_limit',1000),transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at','2026-09-01T00:00:00Z')",[])?;
        seed::insert_tx(&f.c,&f.seed,f.id,&seed::TxSpec {status:0,external_status:Some(2),external_id:Some("nocost".into()),order_type:0,amount:Some("0.001"),quote_amount:Some("5"),price:Some("5000"),quote_amount_exec:None,amount_exec:Some("0.001"),created_at:"2026-09-10 12:00:01".into()});
        let draft=Draft::load(&f.c,f.seed.user_id,f.id,"en").map_err(|e|format!("{e:?}"))?.ok_or("bot")?;
        let limit=deltabadger::web::bot::start::amount_limit(&f.c,&draft.candidate).map_err(|e|format!("{e:?}"))?.ok_or("the cap is on")?;
        assert_eq!(limit.left.as_ref().and_then(|n|n.to_d()).ok_or("normalized cap")?.to_s_f(),"995.0");
        assert!(!limit.reached);
        assert!(matches!(f.lifecycle(write::Action::Start,None)?,Outcome::Committed(_)));
        Ok(())
    }

    #[test]
    fn action_lifecycle_cap_basket_and_intent_contracts() -> Result {
        let f=Fixture::new()?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount_limited',json('true'),'$.quote_amount_limit',5),transient_data=json_set(transient_data,'$.quote_amount_limit_enabled_at','2026-09-01T00:00:00Z')",[])?;
        seed::insert_tx(&f.c,&f.seed,f.id,&seed::TxSpec {status:0,external_status:Some(2),external_id:Some("cap".into()),order_type:0,amount:Some("0.001"),quote_amount:Some("5"),price:Some("5000"),quote_amount_exec:Some("5"),amount_exec:Some("0.001"),created_at:"2026-09-10 12:00:01".into()});
        let before=f.snapshot()?;
        assert!(matches!(f.lifecycle(write::Action::Start,None)?,Outcome::Invalid(..)));
        assert_eq!(f.snapshot()?,before);
        for unbalanced in [false,true] {
            let f=Fixture::new()?;
            f.c.execute("INSERT INTO assets (external_id,symbol,name,category,created_at,updated_at) VALUES ('ethereum','ETH','Ethereum','Cryptocurrency','2026-01-01','2026-01-01')",[])?;
            let eth=f.c.last_insert_rowid();
            f.c.execute("INSERT INTO tickers (exchange_id,ticker,base,quote,base_asset_id,quote_asset_id,base_decimals,quote_decimals,price_decimals,minimum_base_size,minimum_quote_size,trading_enabled,available,created_at,updated_at) SELECT exchange_id,'ETH/USD','ETH',quote,?1,quote_asset_id,base_decimals,quote_decimals,price_decimals,minimum_base_size,minimum_quote_size,trading_enabled,available,created_at,updated_at FROM tickers WHERE id=?2",(eth,f.seed.ticker_id))?;
            let weights=json!({f.seed.btc.to_string():0.5,eth.to_string():if unbalanced {0.2}else{0.5}});
            f.c.execute("UPDATE bots SET settings=json_set(settings,'$.allocations',json(?1))",[weights.to_string()])?;
            let before=f.snapshot()?;
            let result=f.lifecycle(write::Action::Start,None)?;
            if unbalanced { assert!(matches!(result,Outcome::Invalid(..))); assert_eq!(f.snapshot()?,before); }
            else { assert!(matches!(result,Outcome::Committed(_)),"PR #457's supported crypto basket must remain eligible"); }
        }
        for action in [write::Action::Stop,write::Action::Delete,write::Action::Archive] {
            let f=Fixture::new()?;
            let intent=json!({"allocations":{f.seed.btc.to_string():1.0},"exchange_id":f.seed.exchange_id,"quote_asset_id":f.seed.quote,"cl_ord_id":"in-flight"});
            f.c.execute("UPDATE bots SET status=4,transient_data=json_set(transient_data,'$.rust_placement',json(?1))",[intent.to_string()])?;
            assert!(matches!(f.lifecycle(action,None)?,Outcome::Committed(_)));
            assert_eq!(f.stored()?["transient"]["rust_placement"],intent);
        }
        Ok(())
    }

    #[test]
    fn action_lifecycle_minimal_reader_and_corrupt_json() -> Result {
        for sql in ["UPDATE bots SET exchange_id=NULL", "UPDATE bots SET settings=json_set(settings,'$.interval','unknown')"] {
            let f=Fixture::new()?; f.c.execute_batch(sql)?;
            let result=f.lifecycle(write::Action::Stop,None)?;
            let Outcome::Committed(view)=result else { return Err(format!("minimal stop failed: {sql}").into()) };
            assert_eq!(view["minimal"],true);
        }
        let f=Fixture::new()?;
        f.c.execute("UPDATE bots SET transient_data='not json'",[])?;
        let before=f.snapshot()?;
        assert!(matches!(f.lifecycle(write::Action::Stop,None)?,Outcome::GuardRefused(_)));
        assert_eq!(before,f.snapshot()?);
        Ok(())
    }

    #[tokio::test]
    async fn action_lifecycle_index_guard_ownership_and_unarchive_validation() -> Result {
        let f=Fixture::new()?;
        for (key,value) in [("market_data_provider","deltabadger"),("market_data_url","http://example.test"),("market_data_token","test")] {
            f.c.execute("INSERT INTO app_configs(key,value,created_at,updated_at) VALUES(?1,?2,'2026-01-01','2026-01-01')",(key,f.ctx.app.cipher.encrypt(value)))?;
        }
        f.c.execute("UPDATE bots SET type='Bots::DcaIndex',settings=json_set(settings,'$.index_type','top','$.num_coins',2,'$.quote_amount',60)",[])?;
        // The engine's own refusal of an Alpaca index (eligibility::index_reasons: only USD), reached through the guard.
        f.c.execute("UPDATE assets SET symbol='USDT' WHERE id=?1",[f.seed.quote])?;
        let before=f.snapshot()?;
        let result=f.lifecycle(write::Action::Start,Some(json!(false)))?;
        let Outcome::GuardRefused(_) = result else { return Err("index must reach the actual engine guard".into()) };
        assert_eq!(f.snapshot()?,before);
        // A Top coins index the engine runs starts, and its start leaves the composition resync for the engine (Bots::DcaIndex#start).
        f.c.execute("UPDATE assets SET symbol='USD' WHERE id=?1",[f.seed.quote])?;
        assert!(matches!(f.lifecycle(write::Action::Start,Some(json!(false)))?,Outcome::Committed(_)));
        let marker:String=f.c.query_row("SELECT json_extract(transient_data,'$.rust_index_resync') FROM bots WHERE id=?1",[f.id],|r|r.get(0))?;
        assert_eq!(serde_json::from_str::<Value>(&marker)?,json!({"n":1,"at":"2026-09-10T12:00:30.123Z"}));
        f.c.execute("UPDATE bots SET status=2",[])?;
        // A save that changes an index-definition key resyncs (Bots::DcaIndex#resync_index_composition); one that does not, does not.
        let index_params=|fields:Value| -> Result<ActionParams> {
            let p=Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: Some(json!({"bots_dca_index":fields})) };
            ActionParams::parse(&p).map_err(|e| format!("{e:?}").into())
        };
        let save=|fields:Value| -> Result<bool> {
            let done=write::settings(&f.c,&f.ctx,f.seed.user_id,f.id,&index_params(fields)?,|_,_,_| Ok(Prepared { response: (), broadcasts: vec![], after_scheduling: None })).map_err(|e|format!("{e:?}"))?;
            Ok(matches!(done,Outcome::Committed(_)))
        };
        let marker=|| -> Result<Option<String>> { Ok(f.c.query_row("SELECT json_extract(transient_data,'$.rust_index_resync.n') FROM bots WHERE id=?1",[f.id],|r|r.get::<_,Option<i64>>(0))?.map(|n|n.to_string())) };
        assert!(save(json!({"label":"Renamed"}))?);
        assert_eq!(marker()?.as_deref(),Some("1"),"a label alone changes no index-definition key");
        assert!(save(json!({"num_coins":"3"}))?);
        assert_eq!(marker()?.as_deref(),Some("2"),"num_coins is an index-definition key");
        let before=f.snapshot()?;
        for action in [write::Action::Start,write::Action::Stop,write::Action::Delete,write::Action::Archive,write::Action::Unarchive] {
            let result=write::lifecycle::<()>(&f.c,&f.ctx,f.seed.user_id+1,f.id,action,&Fixture::params(json!({}))?,|_,_,_|panic!("foreign bot rendered")).map_err(|e|format!("{e:?}"))?;
            assert!(matches!(result,Outcome::Missing));
            assert_eq!(f.snapshot()?,before);
        }
        let f=Fixture::new()?;
        f.c.execute("UPDATE bots SET status=7,settings=json_set(settings,'$.quote_amount',0)",[])?;
        let before=f.snapshot()?;
        let notify=Arc::new(tokio::sync::Notify::new()); f.ctx.app.attach_engine(notify.clone());
        assert!(matches!(f.lifecycle(write::Action::Unarchive,None)?,Outcome::Invalid(..)));
        assert_eq!(f.snapshot()?,before);
        lifecycle_wake(&notify,false).await?;
        Ok(())
    }

}

#[tokio::test(flavor="current_thread")]
async fn action_http_fragments_locales_formats_and_hostile_drafts() -> Result<(),Box<dyn std::error::Error>> {
    use common::{seed,web::{Browser,Csrf}};
    use deltabadger::web::session::{self,SessionData};
    let (dir,opened,seeded) = common::install_alpaca();
    let hash = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
    opened.primary.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00',wash_sale_enabled=0",[hash])?;
    let mut spec=seed::BotSpec::weekly(5.0,"2026-09-10 12:00:00"); spec.status=2;
    let id=seed::insert_bot(&opened.primary,&seeded,&spec);
    opened.primary.execute("UPDATE bots SET label='Original' WHERE id=?1",[id])?;
    let app=common::web::app(dir.path(),"engine-test-secret",common::web::TestClock::at("2026-09-10T12:00:30Z"));
    let mut browser=Browser { cookie:Some(session::seal(&app.keys.session,&SessionData { user:Some((seeded.user_id,hash.get(..29).ok_or("salt")?.into())),..Default::default() },app.now())),page:None };
    let empty=browser.get(&app,&format!("/bots/{id}/start/edit")).await;
    assert_eq!(empty.status,200);
    assert_eq!(empty.body,"<turbo-frame id=\"modal\"></turbo-frame>");
    assert_eq!(empty.header("set-cookie"),None,"an empty modal has no forms and must not create a CSRF token");
    for prefix in ["","/de"] {
        let path=format!("{prefix}/bots/{id}");
        assert_eq!(browser.get(&app,&path).await.status,200);
        let before:String=opened.primary.query_row("SELECT settings FROM bots WHERE id=?1",[id],|r|r.get(0))?;
        let headers=[("accept","text/vnd.turbo-stream.html")];
        let rejected=browser.send(&app,"PATCH",&path,Some(&[("bots_dca_multi_asset[quote_amount]","0"),("bots_dca_multi_asset[label]","<script>alert(1)</script>")]),Csrf::Header,&headers).await;
        assert_eq!(rejected.status,422,"{}",rejected.body);
        assert_eq!(rejected.body.matches("<turbo-stream ").count(),8);
        assert!(rejected.body.contains("value=\"0\"") && rejected.body.contains("is-invalid"));
        assert!(!rejected.body.contains("<script>alert(1)</script>"));
        assert!(rejected.body.contains("&lt;script&gt;"));
        let after:String=opened.primary.query_row("SELECT settings FROM bots WHERE id=?1",[id],|r|r.get(0))?;
        assert_eq!(before,after);
        let hostile=browser.send(&app,"PATCH",&path,Some(&[("bots_dca_multi_asset[interval]","<img src=x onerror=alert(1)>")]),Csrf::Header,&headers).await;
        assert_eq!(hostile.status,422,"{}",hostile.body);
        // The unknown interval redraws the bot as stored (BotsController#update): the value is never echoed.
        assert!(!hostile.body.contains("<img src=x") && !hostile.body.contains("&lt;img"),"{}",hostile.body);
        let bad=browser.send(&app,"PATCH",&path,Some(&[("bots_dca_multi_asset[quote_amount]","7")]),Csrf::Header,&[("accept","text/html")]).await;
        assert_eq!(bad.status,406);
        let accepted=browser.send(&app,"POST",&format!("{path}.turbo_stream"),Some(&[("_method","patch"),("bots_dca_multi_asset[label]","Renamed")]),Csrf::Header,&[("accept","text/html")]).await;
        assert_eq!(accepted.status,200,"{}",accepted.body);
        assert_eq!(accepted.body.matches("<turbo-stream ").count(),8);
        for suffix in ["/edit","/delete/edit","/archive/edit","/start/edit"] {
            let modal=browser.send(&app,"GET",&format!("{path}{suffix}"),None,Csrf::None,&[("turbo-frame","modal")]).await;
            assert_eq!(modal.status,200,"{}",modal.body);
            assert!(modal.body.contains("id=\"modal\""));
        }
    }
    Ok(())
}

/// Task 7: requests use the normal security pipeline, a file-backed install and a
/// connection distinct from the engine's. No timing guesses establish an overlap.
mod action_race {
    use super::common;
    use common::{seed, scripted, web::{self as harness, Browser, Csrf, TestClock}};
    use deltabadger::{engine::{model, run, tick, Clock}, web::{App, Config}, venue::{alpaca::{AlpacaVenue, Urls}, http::{HttpRequest, HttpResponse, ScriptedTransport, Transport, TransportError}, VenueFactory}};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::{cell::RefCell, rc::Rc, sync::{Arc, Mutex, mpsc}, time::Duration};
    use tokio::sync::Notify;
    type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
    const NOW: &str = "2026-09-03T15:00:00Z";
    const ANCHOR: &str = "2026-09-01 10:00:00";
    const LIMIT: Duration = Duration::from_secs(15);
    const HEADERS: &[(&str, &str)] = &[("accept", "text/vnd.turbo-stream.html")];

    // rusqlite's safe busy-handler interface takes a function pointer. Serialize only
    // these probes, and keep the callback bounded even if an assertion unwinds.
    static PROBE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static BUSY: Mutex<Option<(Arc<Notify>, mpsc::Receiver<()>)>> = Mutex::new(None);
    fn busy(attempt: i32) -> bool {
        if attempt != 0 { return false; }
        let Ok(probe) = BUSY.lock() else { return false; };
        let Some((entered, release)) = &*probe else { return false; };
        entered.notify_one();
        release.recv_timeout(LIMIT).is_ok()
    }

    struct Fixture {
        dir: tempfile::TempDir, c: Connection, app: App, browser: Browser,
        seed: seed::Seeded, id: i64, eth: i64, clock: Arc<TestClock>, wake: Arc<Notify>,
    }
    impl Fixture {
        async fn new() -> Result<Self> {
            let (dir, opened, seed) = common::install_alpaca();
            let c = opened.primary;
            // A real password login supplies the session and rotated CSRF token.
            let hash = deltabadger::crypto::hash_password("Correct-horse-9").map_err(|e| format!("{e:?}"))?;
            c.execute("UPDATE users SET encrypted_password=?1,confirmed_at=created_at,wash_sale_enabled=0", [hash])?;
            let (eth, _) = seed::add_eth_sol(&c, &seed);
            let mut spec = seed::BotSpec::weekly(60.0, ANCHOR).with("interval", json!("day"));
            spec.status = 2;
            spec.transient = json!({"private":{"null":null,"value":"keep"}});
            let id = seed::insert_bot(&c, &seed, &spec);
            c.execute("UPDATE bots SET label='Race' WHERE id=?1", [id])?;
            c.execute("UPDATE exchange_assets SET updated_at=?1", ["2026-09-03 15:00:00"])?;
            let clock = TestClock::at(NOW);
            let own = Connection::open(dir.path().join("production.sqlite3"))?;
            own.busy_timeout(LIMIT)?;
            own.pragma_update(None, "foreign_keys", true)?;
            let env = |k: &str| match k { "SECRET_KEY_BASE" => Some("engine-test-secret".into()), _ => None };
            let app = App::new(Config::from_env(&env).map_err(|e|format!("{e:?}"))?, &env, own, clock.clone()).map_err(|e|format!("{e:?}"))?;
            // No market data: the figures stay cold, and a subscription asks for no publication (`loading::resubscribed`),
            // so only the actions' own fragments are broadcast.
            let app = app.with_figure_source(deltabadger::web::figure::loading::Source::Disabled).map_err(|e|format!("{e:?}"))?;
            let wake = Arc::new(Notify::new()); app.attach_engine(wake.clone());
            let mut browser = Browser::default();
            browser.get(&app, "/login").await;
            assert_eq!(browser.post(&app, "/login", &[("user[email]","o@example.com"),("user[password]","Correct-horse-9")]).await.status,303);
            assert_eq!(browser.get(&app,&format!("/bots/{id}")).await.status,200);
            Ok(Self { dir, c, app, browser, seed, id, eth, clock, wake })
        }
        fn path(&self, suffix: &str) -> String { format!("/bots/{}{suffix}", self.id) }
        async fn send(&mut self, method: &str, suffix: &str, fields: &[(&str,&str)]) -> Result<harness::Answer> {
            let path = self.path(suffix);
            Ok(tokio::time::timeout(LIMIT,self.browser.send(&self.app,method,&path,Some(fields),Csrf::Header,HEADERS)).await?)
        }
        async fn ok(&mut self, method: &str, suffix: &str, fields: &[(&str,&str)]) -> Result<harness::Answer> {
            let answer=self.send(method,suffix,fields).await?;
            assert_eq!(answer.status,200,"{method} {suffix}: {}",answer.body); Ok(answer)
        }
        fn one<T: rusqlite::types::FromSql>(&self, sql: &str) -> Result<T> { Ok(self.c.query_row(sql,[self.id],|r|r.get(0))?) }
        fn snapshot(&self) -> Result<Vec<String>> {
            let mut out=vec![];
            for table in ["bots","bot_index_assets","bot_activity_logs","transactions","api_keys","users"] {
                let mut q=self.c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
                let columns=q.column_count(); let mut rows=q.query([])?;
                while let Some(row)=rows.next()? {
                    let values=(0..columns).map(|i|row.get_ref(i).map(|v|format!("{v:?}"))).collect::<std::result::Result<Vec<_>,_>>()?;
                    out.push(format!("{table}:{values:?}"));
                }
            } Ok(out)
        }
        fn transient(&self) -> Result<Value> { Ok(serde_json::from_str(&self.one::<String>("SELECT transient_data FROM bots WHERE id=?1")?)?) }
        async fn woke(&self, expected: bool) -> Result {
            // Notify stores a permit; a zero-duration poll observes it without a sleep.
            assert_eq!(tokio::time::timeout(Duration::ZERO,self.wake.notified()).await.is_ok(),expected,"post-commit wake"); Ok(())
        }
        async fn contended(&mut self, method: &str, suffix: &str, fields: &[(&str,&str)], edit: impl FnOnce(&Self)->Result, advance: Option<&str>) -> Result<(harness::Answer,Vec<String>)> {
            let _serial=PROBE_LOCK.lock().await;
            self.app.db(|c| { c.busy_handler(Some(busy))?; Ok(()) }).await.map_err(|e|format!("{e:?}"))?;
            let hit=Arc::new(Notify::new()); let (release, receiver)=mpsc::channel();
            *BUSY.lock().map_err(|e|e.to_string())?=Some((hit.clone(),receiver));
            self.c.execute_batch("BEGIN IMMEDIATE")?;
            edit(self)?;
            let before=self.snapshot()?;
            let path=self.path(suffix);
            let request=self.browser.send(&self.app,method,&path,Some(fields),Csrf::Header,HEADERS);
            tokio::pin!(request);
            let deadline=tokio::time::Instant::now()+LIMIT;
            tokio::select! {
                result=&mut request => return Err(format!("request finished before SQLite contention: {} {}; writer still holds BEGIN IMMEDIATE",result.status,result.body).into()),
                _=hit.notified()=>{},
                _=tokio::time::sleep_until(deadline)=>return Err("request did not enter SQLite busy handler; external writer holds BEGIN IMMEDIATE".into()),
            }
            if let Some(now)=advance { self.clock.set(harness::at(now)); }
            self.c.execute_batch("COMMIT")?;
            release.send(())?;
            let answer=tokio::time::timeout_at(deadline,&mut request).await.map_err(|_|"writer committed and released busy handler, request did not finish")?;
            *BUSY.lock().map_err(|e|e.to_string())?=None;
            Ok((answer,before))
        }
    }

    async fn accepted_order_stop_notice(external_status: i64) -> Result {
        let mut f = Fixture::new().await?;
        // Accepted order: its intent has already been cleared, only the submitted row remains.
        f.c.execute("UPDATE bots SET status=1 WHERE id=?1", [f.id])?;
        seed::insert_tx(&f.c, &f.seed, f.id, &seed::TxSpec {
            status: 0, external_status: Some(external_status), external_id: Some("accepted-limit".into()),
            order_type: 1, amount: Some("0.0012"), quote_amount: Some("60"), price: Some("50000"),
            quote_amount_exec: None, amount_exec: None, created_at: "2026-09-03 14:59:00".into(),
        });
        assert!(f.transient()?.get("rust_placement").is_none());
        let answer = f.ok("PATCH", "/stop", &[]).await?;
        assert!(answer.body.contains("Orders already sent to the exchange may still execute."), "{}", answer.body);
        assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?, 2);
        // Terminal and non-submitted rows must not warn, even though the order id remains.
        for (status, external) in [(0, 2), (0, 3), (0, 4), (1, 0), (2, 1)] {
            f.c.execute("UPDATE bots SET status=1 WHERE id=?1", [f.id])?;
            f.c.execute("UPDATE transactions SET status=?1,external_status=?2 WHERE bot_id=?3", (status, external, f.id))?;
            let answer = f.ok("PATCH", "/stop", &[]).await?;
            assert!(!answer.body.contains("Orders already sent to the exchange may still execute."), "{}", answer.body);
        }
        // Another bot's open order cannot cause this bot's notice.
        let other = seed::insert_bot(&f.c, &f.seed, &seed::BotSpec::weekly(60.0, ANCHOR));
        f.c.execute("UPDATE transactions SET bot_id=?1,status=0,external_status=1 WHERE bot_id=?2", (other, f.id))?;
        f.c.execute("UPDATE bots SET status=1 WHERE id=?1", [f.id])?;
        let answer = f.ok("PATCH", "/stop", &[]).await?;
        assert!(!answer.body.contains("Orders already sent to the exchange may still execute."), "{}", answer.body);
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn stop_notice_for_accepted_unknown_order() -> Result { accepted_order_stop_notice(0).await }
    #[tokio::test(flavor="current_thread")]
    async fn stop_notice_for_accepted_open_order() -> Result { accepted_order_stop_notice(1).await }
    /// Bot#sell_capable? counts the rebalance rule: with the wash-sale question unanswered and nothing held, Start is refused
    /// for it, and nothing is written.
    #[tokio::test(flavor="current_thread")]
    async fn a_rebalancing_bot_owes_the_wash_sale_question_before_it_starts() -> Result {
        let mut f=Fixture::new().await?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.rebalance_enabled',json('true')) WHERE id=?1",[f.id])?;
        f.c.execute("UPDATE users SET wash_sale_enabled=NULL",[])?;
        let before=f.snapshot()?;
        // The reason the refusal names (the page answers only that it is not served yet).
        for asked in [deltabadger::web::bot::For::Settings, deltabadger::web::bot::For::Page] {
            assert_eq!(deltabadger::web::bot::refusal(&f.c,f.id,None,false,asked).map_err(|e|format!("{e:?}"))?,Some("the wash-sale question not answered yet"));
        }
        let answer=f.send("PATCH","/start",&[]).await?;
        assert!(answer.status!=200 && answer.body.contains("not-ported"),"{}: {}",answer.status,answer.body);
        assert_eq!(f.snapshot()?,before);
        let page=f.browser.get(&f.app,&f.path("")).await;
        assert!(page.body.contains("not-ported"),"{}",page.body);
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn divergence_in_flight_stop_notice_in_the_browser() -> Result {
        let mut f=Fixture::new().await?;
        f.ok("PATCH","/start",&[]).await?;
        let value=json!({"rust_placement":{"dispatched":true}});
        f.c.execute("UPDATE bots SET transient_data=?1 WHERE id=?2",(value.to_string(),f.id))?;
        let answer=f.ok("PATCH","/stop",&[]).await?;
        assert!(answer.body.contains("Orders already sent to the exchange may still execute."),"{}",answer.body);
        assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,2);
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn stop_with_archived_bot_without_exchange() -> Result {
        let mut f=Fixture::new().await?;
        f.ok("PATCH","/start",&[]).await?;
        assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,1);
        let mut archived=seed::BotSpec::weekly(60.0,ANCHOR);
        archived.status=7; // Automation::Statusable's archived enum value.
        let other=seed::insert_bot(&f.c,&f.seed,&archived);
        f.c.execute("UPDATE bots SET exchange_id=NULL WHERE id=?1",[other])?;

        let answer=f.ok("PATCH","/stop",&[]).await?;
        assert_eq!(answer.header("content-type"),Some("text/vnd.turbo-stream.html; charset=utf-8"));
        assert_eq!(answer.body,"<turbo-stream action=\"refresh\"></turbo-stream>");
        assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,2);
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn start_after_delete() -> Result {
        let mut f=Fixture::new().await?;
        assert_eq!(f.browser.get(&f.app,&f.path("/start/edit")).await.status,200);
        let (a,before)=f.contended("PATCH","/start?start_fresh=true",&[],|f| {f.c.execute("UPDATE bots SET status=3 WHERE id=?1",[f.id])?;Ok(())},None).await?;
        assert_eq!((a.status,a.header("location")),(302,Some("/bots")));
        assert_eq!(f.snapshot()?,before); f.woke(false).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn start_after_start() -> Result {
        let mut f=Fixture::new().await?;
        f.ok("PATCH","/start?start_fresh=true",&[]).await?; f.woke(true).await?;
        let (a,before)=f.contended("PATCH","/start?start_fresh=true",&[], |_|Ok(()),Some("2026-09-03T16:00:00Z")).await?;
        assert_eq!(a.status,422,"{}",a.body); assert!(a.body.contains("already running"));
        assert_eq!(f.snapshot()?,before);
        assert_eq!(f.one::<i64>("SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1 AND event='started'")?,1);
        f.woke(false).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn start_after_archive() -> Result {
        let mut f=Fixture::new().await?;
        let (a,before)=f.contended("PATCH","/start",&[],|f|{f.c.execute("UPDATE bots SET status=7 WHERE id=?1",[f.id])?;Ok(())},None).await?;
        assert_eq!(a.status,422,"{}",a.body); assert_eq!(f.snapshot()?,before); f.woke(false).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn start_after_type_switch() -> Result {
        let mut f=Fixture::new().await?;
        // The stale root is a settings request: Start itself has no STI parameter root.
        let (a,before)=f.contended("PATCH","",&[("bots_dca_multi_asset[label]","stale")],|f|{
            f.c.execute("UPDATE bots SET type='Bots::DcaIndex',settings=json_set(settings,'$.index_type','top','$.num_coins',1,'$.weighting','manual') WHERE id=?1",[f.id])?;Ok(())
        },None).await?;
        assert_eq!(a.status,400,"{}",a.body); assert_eq!(f.snapshot()?,before); f.woke(false).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn unarchive_after_start() -> Result {
        let mut f=Fixture::new().await?;
        f.c.execute("UPDATE bots SET status=7 WHERE id=?1",[f.id])?;
        f.ok("DELETE","/archive",&[]).await?; f.woke(true).await?;
        f.ok("PATCH","/start",&[]).await?; f.woke(true).await?;
        let (a,before)=f.contended("DELETE","/archive",&[],|_|Ok(()),None).await?;
        assert_eq!(a.status,200); assert_eq!(f.snapshot()?,before); f.woke(false).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn settings_after_settings() -> Result {
        let mut f=Fixture::new().await?;
        let (a,_)=f.contended("PATCH","",&[("bots_dca_multi_asset[label]","second tab")],|f|{
            f.c.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount',37.25),transient_data=json_set(transient_data,'$.missed_quote_amount','3.75') WHERE id=?1",[f.id])?;Ok(())
        },None).await?;
        assert_eq!(a.status,200,"{}",a.body); f.woke(true).await?;
        assert_eq!(f.one::<f64>("SELECT json_extract(settings,'$.quote_amount') FROM bots WHERE id=?1")?,37.25);
        assert_eq!(f.transient()?["missed_quote_amount"],"3.75");
        let (a,_)=f.contended("PATCH","",&[("bots_dca_multi_asset[quote_amount]","42")],|_|Ok(()),None).await?;
        assert_eq!(a.status,200); assert_eq!(f.one::<f64>("SELECT json_extract(settings,'$.quote_amount') FROM bots WHERE id=?1")?,42.0); f.woke(true).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn settings_after_engine_json() -> Result {
        let mut f=Fixture::new().await?;
        let (a,_)=f.contended("PATCH","",&[("bots_dca_multi_asset[label]","second tab")],|f| {
            f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.last_action_job_at','2026-09-03T10:00:00Z','$.last_failure_kind','transient','$.mail_private',json('{\"x\":null}'),'$.placement_private',json('[1,\"2\"]')) WHERE id=?1",[f.id])?;Ok(())
        },None).await?;
        assert_eq!(a.status,200,"{}",a.body);
        assert_eq!(f.transient()?,json!({"private":{"null":null,"value":"keep"},"last_action_job_at":"2026-09-03T10:00:00Z","last_failure_kind":"transient","mail_private":{"x":null},"placement_private":[1,"2"]})); f.woke(true).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn composition_after_intent() -> Result {
        let mut f=Fixture::new().await?; let eth=f.eth.to_string();
        let (a,before)=f.contended("PATCH","",&[("bots_dca_multi_asset[add_asset_id]",&eth)],|f|{
            f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement',json(?1)) WHERE id=?2",(json!({"allocations":{f.seed.btc.to_string():1.0},"exchange_id":f.seed.exchange_id,"quote_asset_id":f.seed.quote}).to_string(),f.id))?;Ok(())
        },None).await?;
        assert_eq!(a.status,422,"{}",a.body); assert_eq!(f.snapshot()?,before); f.woke(false).await
    }
    #[tokio::test(flavor="current_thread")]
    async fn queued_clock() -> Result {
        let mut f=Fixture::new().await?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.start_time_enabled',json('true'),'$.start_time_mode','date','$.start_at','2026-09-03T15:30:00Z') WHERE id=?1",[f.id])?;
        let (a,before)=f.contended("PATCH","/start",&[],|_|Ok(()),Some("2026-09-03T16:00:00Z")).await?;
        assert_eq!(a.status,422,"date became past while waiting: {}",a.body); assert_eq!(f.snapshot()?,before); f.woke(false).await?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.start_time_enabled',json('false')),status=1,settings_changed_at=NULL WHERE id=?1",[f.id])?;
        let (a,_)=f.contended("PATCH","",&[("bots_dca_multi_asset[quote_amount]","100")],|_|Ok(()),Some("2026-09-04T10:00:01Z")).await?;
        assert_eq!(a.status,200,"{}",a.body);
        assert_eq!(f.one::<String>("SELECT settings_changed_at FROM bots WHERE id=?1")?,"2026-09-04 10:00:01");
        assert_eq!(f.one::<String>("SELECT updated_at FROM bots WHERE id=?1")?,"2026-09-04 10:00:01");
        assert_eq!(f.transient()?["missed_quote_amount"],json!(100.0)); f.woke(true).await
    }

    type Submission=(chrono::DateTime<chrono::Utc>,Value);
    #[derive(Clone)]
    struct Script {
        transport: ScriptedTransport, clock: Arc<TestClock>,
        gate: Option<(&'static str,Rc<Notify>,Rc<Notify>)>,
        submissions: Rc<RefCell<Vec<Submission>>>,
    }
    impl Script {
        fn new(clock: Arc<TestClock>) -> Self {
            let transport=scripted::script(json!({"GET /v2/orders/OTX-1":[scripted::ok(json!({"id":"OTX-1","status":"filled","symbol":"BTC/USD","type":"market","side":"buy","notional":"60","qty":null,"filled_qty":"0.0009375","filled_avg_price":"64000","limit_price":null}))]}));
            Self {transport,clock,gate:None,submissions:Rc::new(RefCell::new(vec![]))}
        }
        fn venue(&self) -> AlpacaVenue<Self> { AlpacaVenue::new(self.clone(),Urls::for_passphrase(Some("paper"))) }
    }
    impl Transport for Script {
        async fn send(&self, req: &HttpRequest) -> std::result::Result<HttpResponse,TransportError> {
            if req.method=="POST" { self.submissions.borrow_mut().push((self.clock.now(),req.body.clone().unwrap_or(Value::Null))); }
            if let Some((path,entered,release))=&self.gate {
                if req.path==*path { entered.notify_one(); tokio::time::timeout(LIMIT,release.notified()).await.map_err(|_|TransportError::Permanent(format!("script held {} awaiting HTTP writer",req.path)))?; }
            }
            self.transport.send(req).await
        }
    }
    impl VenueFactory for Script {
        type V=AlpacaVenue<Self>;
        fn for_bot(&self,_: &str,_: Option<deltabadger::crypto::Credentials>)->Self::V {self.venue()}
    }
    async fn price_race(edit: bool, restart: bool, after_send: bool) -> Result {
        let mut f=Fixture::new().await?; f.ok("PATCH","/start",&[]).await?; f.woke(true).await?;
        f.clock.set(harness::at("2026-09-03T15:00:00.001Z"));
        let mut script=Script::new(f.clock.clone()); let entered=Rc::new(Notify::new()); let release=Rc::new(Notify::new());
        script.gate=Some((if after_send {"/v2/orders"} else {"/v1beta3/crypto/us/latest/quotes"},entered.clone(),release.clone()));
        let engine_db=Connection::open(f.dir.path().join("production.sqlite3"))?;
        let venue=script.venue(); let mut attempts=tick::Attempts::default();
        let clock=f.clock.clone();
        let future=tick::tick(&engine_db,&venue,f.id,&*clock,&mut attempts); tokio::pin!(future);
        tokio::select! {
            _=entered.notified()=>{},
            result=&mut future=>return Err(format!("engine ended before barrier: {result:?}").into()),
            _=tokio::time::sleep(LIMIT)=>return Err("engine did not reach scripted barrier; HTTP writer not started".into()),
        }
        if !after_send {assert!(f.transient()?.get("rust_placement").is_none());}
        let completed = if after_send {
            tokio::time::timeout(LIMIT, f.ok("PATCH","/stop",&[])).await??;
            assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?, 2);
            assert!(f.transient()?.get("rust_placement").is_some(), "STOP keeps the in-flight intent");
            release.notify_one();
            Some(tokio::time::timeout(LIMIT,&mut future).await)
        } else { f.ok("PATCH","/stop",&[]).await?; None };
        if edit {
            let eth=f.eth.to_string(); let btc=f.seed.btc.to_string();
            f.ok("PATCH","",&[("bots_dca_multi_asset[add_asset_id]",&eth),("bots_dca_multi_asset[remove_asset_id]",&btc)]).await?;
            if restart { f.ok("PATCH","/start",&[]).await?; f.clock.set(harness::at("2026-09-03T15:00:00.002Z")); }
        }
        let outcome = match completed {
            Some(outcome) => outcome,
            None => { release.notify_one(); tokio::time::timeout(LIMIT,&mut future).await }
        }?.map_err(|e|format!("{e:?}"))?;
        assert!(f.transient()?.get("rust_placement").is_none(),"{outcome:?}");
        assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,if restart {1} else {2});
        assert_eq!(script.submissions.borrow().len(),usize::from(after_send));
        assert_eq!(f.one::<i64>("SELECT count(*) FROM transactions WHERE bot_id=?1")?,i64::from(after_send));
        if edit {
            let settings:Value=serde_json::from_str(&f.one::<String>("SELECT settings FROM bots WHERE id=?1")?)?;
            assert_eq!(settings["allocations"],json!({f.eth.to_string():1.0}));
        }
        if after_send {
            deltabadger::engine::polling::sweep(&engine_db,&venue,&model::load_bot(&engine_db,f.id).map_err(|e|format!("{e:?}"))?,f.clock.now()).await.map_err(|e|format!("{e:?}"))?;
            assert_eq!(f.one::<i64>("SELECT external_status FROM transactions WHERE bot_id=?1")?,2);
        }
        let plain=Script::new(f.clock.clone());
        if restart {
            tick::tick(&engine_db,&plain.venue(),f.id,&*f.clock,&mut tick::Attempts::default()).await.map_err(|e|format!("{e:?}"))?;
            assert_eq!(plain.submissions.borrow().len(),1);
            assert_eq!(plain.submissions.borrow().first().ok_or("new composition order")?.1["symbol"],"ETH/USD");
        } else {
            tick::tick(&engine_db,&plain.venue(),f.id,&*f.clock,&mut tick::Attempts::default()).await.map_err(|e|format!("{e:?}"))?;
            assert!(plain.submissions.borrow().is_empty()); assert_eq!(f.one::<i64>("SELECT status FROM bots WHERE id=?1")?,2);
        }
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn stop_during_price()->Result {price_race(false,false,false).await}
    #[tokio::test(flavor="current_thread")]
    async fn composition_during_price()->Result {price_race(true,false,false).await?;price_race(true,true,false).await}
    #[tokio::test(flavor="current_thread")]
    async fn stop_during_tick()->Result {price_race(false,false,true).await}

    /// Re-run only the existing recorder's basket fixture and Continue block on a
    /// scratch Rails install. Capture the predicate before Start changes the status,
    /// and retain the actual ActionJob timestamp as well as its decision label.
    async fn rails_continue() -> Result<Value> {
        tokio::task::spawn_blocking(|| -> std::result::Result<Value,String> {
            let dir=common::rails_install();
            let output=dir.path().join("continue.json");
            let code=r###"
require 'json'
require 'active_support/testing/time_helpers'
include ActiveSupport::Testing::TimeHelpers
source = File.read(Rails.root.join('script/rust/record_vectors.rb'))
fixture = source.split("# Alpaca crypto baskets (rust/src/engine/basket.rs)", 2).fetch(1)
fixture = fixture.split("vector_row_id = 0", 2).fetch(0)
eval("# Alpaca crypto baskets (rust/src/engine/basket.rs)" + fixture, TOPLEVEL_BINDING)
record = source.split("continue_row = lambda", 2).fetch(1).split("# The web UI (rust/src/web).", 2).fetch(0)
record = "vectors = {}; continue_row = lambda" + record
record = record.sub('      raise "#{name}: Rails refused the start:', '      within = bot.restarting_within_interval?' + "\n" + '      raise "#{name}: Rails refused the start:')
record = record.sub("'decision' => decision }", "'decision' => decision, 'within' => within, 'job_at' => at&.iso8601(6), 'checkpoint' => bot.next_interval_checkpoint_at.utc.iso8601(6) }")
eval(record + "\nFile.write(ENV.fetch('ACTION_RACE_RECORD'), JSON.generate(vectors.fetch('continue_start')))", TOPLEVEL_BINDING)
"###;
            let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("repository root")?;
            let log=std::fs::File::create(dir.path().join("recorder.log")).map_err(|e|e.to_string())?;
            let mut command=std::process::Command::new(root.join("bin/rails"));
            command.current_dir(root).args(["runner",code]).env_remove("DATABASE_URL")
                .env("APP_ROOT_URL","http://localhost:3000").env("RAILS_ENV","test").env("SKIP_TEST_DATABASE","true")
                .env("ACTION_RACE_RECORD",&output).stdout(log.try_clone().map_err(|e|e.to_string())?).stderr(log);
            for (name,file) in [("PRIMARY","production.sqlite3"),("QUEUE","production_queue.sqlite3"),("CACHE","cache.sqlite3"),("CABLE","cable.sqlite3")] {
                command.env(format!("{name}_DATABASE_URL"),format!("sqlite3:{}",dir.path().join(file).display()));
            }
            struct Child(std::process::Child);
            impl Drop for Child { fn drop(&mut self) {let _=self.0.kill();let _=self.0.wait();} }
            let mut child=Child(command.spawn().map_err(|e|e.to_string())?);
            let deadline=std::time::Instant::now()+Duration::from_secs(60);
            loop {
                if let Some(status)=child.0.try_wait().map_err(|e|e.to_string())? {
                    if !status.success() {return Err(std::fs::read_to_string(dir.path().join("recorder.log")).map_err(|e|e.to_string())?);}
                    break;
                }
                if std::time::Instant::now()>=deadline {return Err("Rails Continue recorder exceeded its absolute 60-second deadline".into());}
                std::thread::sleep(Duration::from_millis(10)); // child readiness, never race ordering
            }
            serde_json::from_str(&std::fs::read_to_string(output).map_err(|e|e.to_string())?).map_err(|e|e.to_string())
        }).await?.map_err(Into::into)
    }

    async fn continued(name: &str, waits: bool)->Result {
        let mut f=Fixture::new().await?;
        let vectors=rails_continue().await?;
        let row=vectors.as_array().ok_or("continue vectors")?.iter().find(|v|v["name"]==name).ok_or("continue case")?;
        assert_eq!(row["decision"],if waits {"checkpoint"} else {"now"});
        assert_eq!(row["within"],json!(waits));
        assert_eq!(row["job_at"],if waits {json!("2026-09-04T10:00:00.000000Z")} else {Value::Null});
        f.clock.set(harness::at(row["now"].as_str().ok_or("recorded clock")?));
        f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.last_action_job_at',?1) WHERE id=?2",(row["last_action_job_at"].as_str().ok_or("recorded stamp")?,f.id))?;
        for tx in row["rows"].as_array().ok_or("recorded orders")? {seed::insert_row(&f.c,&f.seed,f.id,f.seed.btc,tx);}
        f.ok("PATCH","/start?start_fresh=false",&[]).await?;
        let request=f.transient()?;
        assert_eq!(request["rust_continue_start"],json!({"requested_at":"2026-09-03T15:00:00Z","was_stopped":true}));
        f.woke(true).await?;
        let script=Script::new(f.clock.clone());
        let paths=deltabadger::store::Paths::from_env(&|_|None,f.dir.path());
        let lock=deltabadger::lease::lock(&paths,f.clock.now()).map_err(|e|format!("{e:?}"))?;
        let own=Connection::open(f.dir.path().join("production.sqlite3"))?;
        let mut engine=run::Engine::new(own,script.clone(),seed::cipher(),lock);
        run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
        assert!(f.transient()?.get("rust_continue_start").is_none());
        assert_eq!(f.transient()?["private"],request["private"]);
        if waits {
            assert!(script.submissions.borrow().is_empty());
            f.clock.set(harness::at("2026-09-04T09:59:59.999999Z"));
            run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
            assert!(script.submissions.borrow().is_empty());
            f.clock.set(harness::at("2026-09-04T10:00:00Z"));
            run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
            assert!(script.submissions.borrow().is_empty(), "a checkpoint is due strictly after its instant");
            f.clock.set(harness::at("2026-09-04T10:00:00.000001Z"));
            run::step(&mut engine,&*f.clock).await.map_err(|e|format!("{e:?}"))?;
        }
        assert_eq!(script.submissions.borrow().len(),1,"Rails recorded {name}: {}; real scheduler at {}: transient={}",row["decision"],f.clock.now(),f.transient()?);
        assert_eq!(script.submissions.borrow().first().ok_or("submission")?.0,f.clock.now());
        assert_eq!(f.one::<String>("SELECT started_at FROM bots WHERE id=?1")?,ANCHOR);
        Ok(())
    }
    #[tokio::test(flavor="current_thread")]
    async fn continue_owes_contribution()->Result {continued("owes_its_contribution",false).await}
    #[tokio::test(flavor="current_thread")]
    async fn continue_within_interval()->Result {continued("nothing_owed",true).await}
    /// Every new route uses the same authentication, ownership and CSRF pipeline.
    #[tokio::test(flavor="current_thread")]
    async fn action_security_grid() -> Result {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};
        let f=Fixture::new().await?;
        f.c.execute("INSERT INTO users(email,encrypted_password,created_at,updated_at) VALUES('foreign@example.com','x','2026-01-01','2026-01-01')",[])?;
        let foreign=f.c.last_insert_rowid();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address=listener.local_addr()?;
        let app=f.app.clone();
        struct Server(tokio::task::JoinHandle<()>);
        impl Drop for Server {fn drop(&mut self){self.0.abort();}}
        let _server=Server(tokio::spawn(async move {let _=deltabadger::web::server::serve_on(listener,app,Default::default()).await;}));
        let mut request=format!("ws://{address}/cable").into_client_request()?;
        request.headers_mut().insert("origin",format!("http://{address}").parse()?);
        request.headers_mut().insert("sec-websocket-protocol","actioncable-v1-json".parse()?);
        request.headers_mut().insert("cookie",format!("_deltabadger_rust_session={}",f.browser.cookie.as_deref().ok_or("session")?).parse()?);
        let (mut socket,_)=tokio::time::timeout(LIMIT,tokio_tungstenite::connect_async(request)).await??;
        let streams=[format!("user_{}:bot_updates",f.seed.user_id),format!("user_{}:bot_{}",f.seed.user_id,f.id)];
        for stream in &streams {
            let identifier=json!({"channel":"Turbo::StreamsChannel","signed_stream_name":deltabadger::web::cable::signed_stream_name(&f.app.keys.streams,stream)}).to_string();
            socket.send(Message::Text(json!({"command":"subscribe","identifier":identifier}).to_string().into())).await?;
            tokio::time::timeout(LIMIT,async {
                while let Some(message)=socket.next().await {
                    let value:Value=serde_json::from_str(message?.to_text()?)?;
                    if value["type"]=="confirm_subscription" {return Ok::<(),Box<dyn std::error::Error>>(());}
                } Err("subscription closed".into())
            }).await??;
        }
        let routes=[("PATCH",""),("PATCH","/start"),("PATCH","/stop"),("DELETE","/delete"),("POST","/archive"),("DELETE","/archive"),("GET","/start/edit"),("GET","/edit"),("GET","/delete/edit"),("GET","/archive/edit")];
        for (method,suffix) in routes {
            for prefix in ["","/de"] {
                let mutation=method!="GET";
                let cases=if mutation {vec!["signed_out","missing","foreign","deleted","invalid","missing_csrf","wrong_csrf","foreign_origin","header_only","override"]} else {vec!["signed_out","missing","foreign","deleted","invalid","owned"]};
                for case in cases {
                    f.c.execute("UPDATE bots SET status=?1,user_id=?2 WHERE id=?3",(if suffix=="/archive" && method=="DELETE" {7} else if suffix=="/stop" {1} else {2},f.seed.user_id,f.id))?;
                    let mut browser=Browser {cookie:f.browser.cookie.clone(),page:f.browser.page.clone()};
                    if case=="signed_out" {browser=Browser::default();browser.get(&f.app,"/login").await;}
                    if case=="foreign" {f.c.execute("UPDATE bots SET user_id=?1 WHERE id=?2",(foreign,f.id))?;}
                    if case=="deleted" {f.c.execute("UPDATE bots SET status=3 WHERE id=?1",[f.id])?;}
                    let id=match case {"missing"=>"999999".to_string(),"invalid"=>"invalid".to_string(),_=>f.id.to_string()};
                    let path=format!("{prefix}/bots/{id}{suffix}");
                    let label=format!("{prefix}-{case}");
                    let mut fields=vec![("bots_dca_multi_asset[label]",label.as_str())];
                    let mut headers=HEADERS.to_vec();
                    let csrf=if !mutation || matches!(case,"missing_csrf"|"wrong_csrf") {Csrf::None} else {Csrf::Header};
                    if case=="wrong_csrf" {headers.push(("x-csrf-token","wrong"));}
                    if case=="foreign_origin" {headers.push(("origin","https://foreign.example"));}
                    if case=="override" {fields.push(("_method",method));}
                    let before=f.snapshot()?;
                    let answer=tokio::time::timeout(LIMIT,browser.send(&f.app,if case=="override" {"POST"} else {method},&path,Some(&fields),csrf,&headers)).await?;
                    let expected=match case {"signed_out"|"missing"|"foreign"|"deleted"|"invalid"=>302,"missing_csrf"|"wrong_csrf"|"foreign_origin"=>302,_=>200};
                    assert_eq!(answer.status,expected,"{method} {path} {case}: {}",answer.body);
                    if matches!(case,"missing_csrf"|"wrong_csrf"|"foreign_origin") {assert_eq!(answer.header("location"),Some("/"));}
                    if matches!(case,"missing"|"foreign"|"deleted"|"invalid") {assert_eq!(answer.header("location"),Some(format!("{prefix}/bots").as_str()));}
                    let changed=expected==200 && mutation;
                    if !changed {assert_eq!(f.snapshot()?,before,"{method} {path} {case}");}
                    else {assert_ne!(f.snapshot()?,before,"successful write {method} {path} {case}");}
                    f.woke(changed).await?;
                    let mut messages=vec![];
                    f.app.hub.broadcast(&streams[0],"security-grid-marker");
                    tokio::time::timeout(LIMIT,async {
                        while let Some(message)=socket.next().await {
                            let value:Value=serde_json::from_str(message?.to_text()?)?;
                            if let Some(body)=value["message"].as_str() {
                                if body=="security-grid-marker" {return Ok::<(),Box<dyn std::error::Error>>(());}
                                messages.push(body.to_string());
                            }
                        } Err("broadcast socket closed".into())
                    }).await??;
                    if !changed {assert!(messages.is_empty(),"{method} {path} {case}: {messages:?}");}
                    if changed && matches!(suffix,"/start"|"/stop"|"/archive") {assert!(!messages.is_empty(),"committed status broadcast {method} {path} {case}");}
                }
            }
        }
        socket.close(None).await?;
        Ok(())
    }

}

mod auth_decision_clock {
    use super::common::{self, seed, web::{self as harness, Browser, Csrf, TestClock}};
    use deltabadger::{codec::format_time, crypto::totp_at, engine::Clock, web::{App, Config, session}};
    use rusqlite::Connection;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    use tokio::sync::Notify;
    type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
    const LIMIT: Duration = Duration::from_secs(15);
    const NOW: &str = "2026-09-10T12:00:00Z";
    const LATER: &str = "2026-09-10T12:01:01Z";
    const OTP: &str = "JBSWY3DPEHPK3PXP";
    const PASSWORD: &str = "Correct-horse-9";

    struct ArrivalClock { clock: Arc<TestClock>, arrived: Arc<Notify> }
    impl Clock for ArrivalClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            let now = self.clock.now();
            self.arrived.notify_one();
            now
        }
    }
    struct Fixture {
        _dir: tempfile::TempDir, path: std::path::PathBuf, c: Connection,
        app: App, browser: Browser, clock: Arc<TestClock>, arrived: Arc<Notify>,
    }
    impl Fixture {
        async fn new(otp: bool) -> Result<Self> {
            let (dir, opened, _) = common::install_alpaca();
            let c = opened.primary;
            let hash = deltabadger::crypto::hash_password(PASSWORD).map_err(|e| format!("{e:?}"))?;
            c.execute("UPDATE users SET encrypted_password=?1,confirmed_at=created_at,otp_module=?2,otp_secret_key=?3", rusqlite::params![hash, otp, seed::cipher().encrypt(OTP)])?;
            let path = dir.path().join("production.sqlite3");
            let clock = TestClock::at(NOW);
            let arrived = Arc::new(Notify::new());
            let env = harness::env("engine-test-secret");
            let app = App::new(Config::from_env(&env).map_err(|e|format!("{e:?}"))?, &env, Connection::open(&path)?, Arc::new(ArrivalClock {clock:clock.clone(),arrived:arrived.clone()})).map_err(|e|format!("{e:?}"))?;
            let mut browser = Browser::default();
            browser.get(&app,"/login").await;
            Ok(Self {_dir:dir,path,c,app,browser,clock,arrived})
        }
        fn pending(&mut self, started: i64) -> Result {
            let cookie = self.browser.cookie.as_deref().ok_or("cookie")?;
            let mut data = session::open(&self.app.keys.session,cookie,self.clock.now()).ok_or("session")?;
            let id = self.c.query_row("SELECT id FROM users",[],|r|r.get(0))?;
            data.pending=Some(session::Pending {user_id:id,started_at:started});
            self.browser.cookie=Some(session::seal(&self.app.keys.session,&data,self.clock.now()));
            Ok(())
        }
        fn state(&self) -> Result<(i64,Option<String>,Option<String>,String)> {
            Ok(self.c.query_row("SELECT failed_attempts,locked_at,last_otp_at,updated_at FROM users",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?)
        }
        fn session(&self) -> Result<session::SessionData> {
            session::open(&self.app.keys.session,self.browser.cookie.as_deref().ok_or("cookie")?,self.clock.now()).ok_or("session".into())
        }
        // The request's arrival clock is observed while another worker owns App's mutex.
        async fn mutex_wait(&mut self, method: &str, path: &str, fields: &[(&str,&str)], later: &str) -> Result<harness::Answer> {
            let deadline=tokio::time::Instant::now()+LIMIT;
            let held=Arc::new(Notify::new()); let signal=held.clone();
            let (release,rx)=mpsc::channel(); let app=self.app.clone();
            let holder=tokio::spawn(async move {app.db(move |_| {
                signal.notify_one(); rx.recv_timeout(LIMIT).map_err(|e|deltabadger::web::WebError::Task(e.to_string()))?; Ok(())
            }).await});
            tokio::time::timeout_at(deadline,held.notified()).await?;
            // Drain notifications from fixture setup.
            let _=tokio::time::timeout(Duration::ZERO,self.arrived.notified()).await;
            let request=self.browser.send(&self.app,method,path,Some(fields),Csrf::Header,&[]);
            tokio::pin!(request);
            tokio::select! {
                answer=&mut request => return Err(format!("request escaped held mutex: {}",answer.status).into()),
                _=self.arrived.notified()=>{},
                _=tokio::time::sleep_until(deadline)=>return Err("no arrival".into()),
            }
            self.clock.set(harness::at(later)); release.send(())?;
            holder.await?.map_err(|e|format!("{e:?}"))?;
            Ok(tokio::time::timeout_at(deadline,request).await?)
        }
    }

    #[tokio::test(flavor="current_thread")]
    async fn bcrypt_slot() -> Result {
        let mut f=Fixture::new(false).await?;
        f.c.execute("UPDATE users SET failed_attempts=4",[])?;
        let (release,rx)=mpsc::channel(); let rx=Mutex::new(rx);
        let entered=Arc::new(Notify::new()); let hit=entered.clone();
        f.app=f.app.with_password_hook(Arc::new(move || {
            hit.notify_one();
            if let Ok(rx)=rx.lock() { let _=rx.recv_timeout(LIMIT); }
        })).map_err(|e|format!("{e:?}"))?;
        let deadline=tokio::time::Instant::now()+LIMIT;
        let mut workers=Vec::new();
        for _ in 0..2 {
            let app=f.app.clone();
            let mut browser=Browser {cookie:f.browser.cookie.clone(),page:f.browser.page.clone()};
            workers.push(tokio::spawn(async move {browser.post(&app,"/login",&[("user[email]","missing@example.com"),("user[password]","wrong")]).await}));
            tokio::time::timeout_at(deadline,entered.notified()).await?;
        }
        let fields=[("user[email]","o@example.com"),("user[password]","wrong")];
        {
        let request=f.browser.post(&f.app,"/login",&fields); tokio::pin!(request);
        tokio::select! {
            answer=&mut request=>return Err(format!("not queued: {}",answer.status).into()),
            _=async { while f.app.password_checks_waiting()!=1 {tokio::task::yield_now().await;} }=>{},
            _=tokio::time::sleep_until(deadline)=>return Err("no bcrypt waiter".into()),
        }
        f.clock.set(harness::at(LATER));
        for _ in 0..3 {release.send(())?;}
        assert_eq!(tokio::time::timeout_at(deadline,request).await?.status,422);
        }
        for worker in workers {tokio::time::timeout_at(deadline,worker).await??;}
        let state=f.state()?; assert_eq!(state.0,5);
        assert_eq!(state.1,Some(format_time(harness::at(LATER))));
        assert_eq!(state.3,format_time(harness::at(LATER))); Ok(())
    }

    #[tokio::test(flavor="current_thread")]
    async fn password_database_expiry() -> Result {
        let mut f=Fixture::new(false).await?;
        f.c.execute("UPDATE users SET failed_attempts=5,locked_at='2026-09-10 11:45:30'",[])?;
        let a=f.mutex_wait("POST","/login",&[("user[email]","o@example.com"),("user[password]",PASSWORD)],LATER).await?;
        assert_eq!(a.status,303); assert!(f.session()?.user.is_some());
        let state=f.state()?; assert_eq!((state.0,state.1),(0,None)); assert_eq!(state.3,format_time(harness::at(LATER))); Ok(())
    }

    #[tokio::test(flavor="current_thread")]
    async fn otp_database_expiry_and_replay() -> Result {
        let mut f=Fixture::new(true).await?;
        f.pending(harness::at(NOW).timestamp())?;
        f.c.execute("UPDATE users SET failed_attempts=5,locked_at='2026-09-10 11:45:30'",[])?;
        let code=totp_at(OTP,harness::at(LATER).timestamp() as u64).ok_or("OTP")?;
        let a=f.mutex_wait("POST","/verify_two_factor",&[("user[otp_code_token]",&code)],LATER).await?;
        assert_eq!(a.status,303);
        let state=f.state()?; assert_eq!((state.0,state.1),(0,None));
        assert_eq!(state.2,Some("2026-09-10 12:01:00".into())); assert_eq!(state.3,format_time(harness::at(LATER)));
        // A separate pending browser reuses the consumed code, never an authenticated session.
        f.browser=Browser::default(); f.browser.get(&f.app,"/login").await; f.pending(harness::at(LATER).timestamp())?;
        let a=f.browser.send(&f.app,"POST","/verify_two_factor",Some(&[("user[otp_code_token]",&code)]),Csrf::Header,&[]).await;
        assert_eq!(a.status,422); assert_eq!(f.state()?.0,1); assert_eq!(f.state()?.2,state.2); Ok(())
    }

    #[tokio::test(flavor="current_thread")]
    async fn pending_ttl_and_password_start() -> Result {
        let mut f=Fixture::new(true).await?;
        let a=f.mutex_wait("POST","/login",&[("user[email]","o@example.com"),("user[password]",PASSWORD)],LATER).await?;
        assert_eq!(a.status,302); assert_eq!(f.session()?.pending.ok_or("pending")?.started_at,harness::at(LATER).timestamp());
        for method in ["GET","POST"] {
            f.clock.set(harness::at(NOW));
            f.browser=Browser::default(); f.browser.get(&f.app,"/login").await;
            f.pending(harness::at(NOW).timestamp()-299)?;
            let before=f.state()?;
            let code=totp_at(OTP,harness::at(NOW).timestamp() as u64).ok_or("OTP")?;
            let a=f.mutex_wait(method,"/verify_two_factor",&[("user[otp_code_token]",&code)],"2026-09-10T12:00:01Z").await?;
            assert_eq!((a.status,a.header("location")),(302,Some("/login")));
            assert!(f.session()?.pending.is_none()); assert_eq!(f.state()?,before);
        } Ok(())
    }

    static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    static BUSY: Mutex<Option<(Arc<Notify>,mpsc::Receiver<()>)>> = Mutex::new(None);
    fn busy(attempt: i32) -> bool {
        if attempt!=0 {return false;}
        let Ok(probe)=BUSY.lock() else {return false;};
        let Some((hit,rx))=&*probe else {return false;};
        hit.notify_one(); rx.recv_timeout(LIMIT).is_ok()
    }
    #[tokio::test(flavor="current_thread")]
    async fn external_sqlite_contention() -> Result {
        let _serial=SERIAL.lock().await;
        for case in ["wrong","expiry","GET","POST"] {
            let mut f=Fixture::new(matches!(case,"GET"|"POST")).await?;
            match case {
                "wrong"=>{f.c.execute("UPDATE users SET failed_attempts=4",[])?;},
                "expiry"=>{f.c.execute("UPDATE users SET failed_attempts=5,locked_at='2026-09-10 11:45:30'",[])?;},
                _=>{f.pending(harness::at(NOW).timestamp()-299)?; f.c.execute("UPDATE users SET failed_attempts=3,locked_at='2026-09-10 11:45:30'",[])?;},
            }
            let before=f.state()?;
            f.app.db(|c| {c.busy_handler(Some(busy))?;Ok(())}).await.map_err(|e|format!("{e:?}"))?;
            let hit=Arc::new(Notify::new()); let (retry,rx)=mpsc::channel();
            *BUSY.lock().map_err(|e|e.to_string())?=Some((hit.clone(),rx));
            let (start,start_rx)=mpsc::channel(); let (release,release_rx)=mpsc::channel();
            let held=Arc::new(Notify::new()); let held_worker=held.clone();
            let (ready,ready_rx)=mpsc::channel(); let path=f.path.clone();
            let writer=tokio::task::spawn_blocking(move || -> std::result::Result<(),String> {
                start_rx.recv_timeout(LIMIT).map_err(|e|e.to_string())?;
                let c=Connection::open(path).map_err(|e|e.to_string())?;
                c.busy_timeout(LIMIT).map_err(|e|e.to_string())?;
                c.execute_batch("BEGIN IMMEDIATE").map_err(|e|e.to_string())?;
                held_worker.notify_one(); ready.send(()).map_err(|e|e.to_string())?;
                let released=release_rx.recv_timeout(LIMIT);
                c.execute_batch("ROLLBACK").map_err(|e|e.to_string())?;
                released.map_err(|e|e.to_string())
            });
            if matches!(case,"wrong"|"expiry") {
                let ready_rx=Mutex::new(ready_rx);
                // Existing hook runs after hash read and before bcrypt; the lock is held
                // through bcrypt, so only the subsequent decision transaction contends.
                f.app=f.app.with_password_hook(Arc::new(move || {
                    let _=start.send(());
                    if let Ok(rx)=ready_rx.lock() {let _=rx.recv_timeout(LIMIT);}
                })).map_err(|e|format!("{e:?}"))?;
            } else {start.send(())?; tokio::time::timeout(LIMIT,held.notified()).await?;}
            let code=totp_at(OTP,harness::at(LATER).timestamp() as u64).ok_or("OTP")?;
            let fields=if case=="wrong" {vec![("user[email]","o@example.com"),("user[password]","wrong")]} else if case=="expiry" {vec![("user[email]","o@example.com"),("user[password]",PASSWORD)]} else {vec![("user[otp_code_token]",code.as_str())]};
            let path=if matches!(case,"wrong"|"expiry") {"/login"} else {"/verify_two_factor"};
            let method=if case=="GET" {"GET"} else {"POST"};
            let deadline=tokio::time::Instant::now()+LIMIT;
            let a = {
            let request=f.browser.send(&f.app,method,path,Some(&fields),Csrf::Header,&[]); tokio::pin!(request);
            let waiting=tokio::select! {
                answer=&mut request=>Err(format!("{case}: decision escaped SQLite write lock: {}",answer.status)),
                _=hit.notified()=>Ok(()),
                _=tokio::time::sleep_until(deadline)=>Err(format!("{case}: no SQLite busy callback")),
            };
            f.clock.set(harness::at(LATER)); release.send(())?;
            tokio::time::timeout_at(deadline,writer).await??.map_err(|e|format!("writer: {e}"))?;
            retry.send(()).ok();
            waiting?;
            tokio::time::timeout_at(deadline,request).await?
            };
            *BUSY.lock().map_err(|e|e.to_string())?=None;
            let after=f.state()?;
            match case {
                "wrong"=>{assert_eq!(a.status,422); assert_eq!(after.0,5); assert_eq!(after.1,Some(format_time(harness::at(LATER)))); assert_eq!(after.3,format_time(harness::at(LATER)));},
                "expiry"=>{assert_eq!(a.status,303); assert!(f.session()?.user.is_some()); assert_eq!((after.0,after.1),(0,None)); assert_eq!(after.3,format_time(harness::at(LATER)));},
                _=>{assert_eq!((a.status,a.header("location")),(302,Some("/login"))); assert_eq!(after,before); assert!(f.session()?.pending.is_none()); assert!(f.session()?.user.is_none());},
            }
        } Ok(())
    }
}

/// A bot stored without a name shows a generated one. A save that does not name it leaves the row
/// unnamed (Rails' in-memory name is not dirty); submitting the shown name saves it
/// (Automation::Labelable#label=).
#[tokio::test(flavor="current_thread")]
async fn submitting_the_shown_name_of_an_unnamed_bot_saves_it() -> Result<(),Box<dyn std::error::Error>> {
    use common::{seed,web::{Browser,Csrf}};
    use deltabadger::web::{bot::{Bot,For},session::{self,SessionData}};
    let (dir,opened,seeded) = common::install_alpaca();
    let hash = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";
    opened.primary.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00',wash_sale_enabled=0",[hash])?;
    let mut spec=seed::BotSpec::weekly(5.0,"2026-09-10 12:00:00"); spec.status=2;
    let id=seed::insert_bot(&opened.primary,&seeded,&spec);
    opened.primary.execute("UPDATE bots SET label=NULL WHERE id=?1",[id])?;
    let name=Bot::find(&opened.primary,seeded.user_id,id,For::Page,"en").map_err(|e|format!("{e:?}"))?.ok_or("bot")?.label;
    let app=common::web::app(dir.path(),"engine-test-secret",common::web::TestClock::at("2026-09-10T12:00:30Z"));
    let mut browser=Browser { cookie:Some(session::seal(&app.keys.session,&SessionData { user:Some((seeded.user_id,hash.get(..29).ok_or("salt")?.into())),..Default::default() },app.now())),page:None };
    let path=format!("/bots/{id}");
    assert_eq!(browser.get(&app,&path).await.status,200);
    let headers=[("accept","text/vnd.turbo-stream.html")];
    let label=|| opened.primary.query_row("SELECT label FROM bots WHERE id=?1",[id],|r|r.get::<_,Option<String>>(0));
    let unnamed=browser.send(&app,"PATCH",&path,Some(&[("bots_dca_multi_asset[quote_amount]","7")]),Csrf::Header,&headers).await;
    assert_eq!(unnamed.status,200,"{}",unnamed.body);
    assert!(unnamed.body.contains(&format!("<template>{name}</template>")),"the label stream shows the generated name: {}",unnamed.body);
    assert_eq!(label()?,None,"a save that names nothing leaves the row unnamed");
    let named=browser.send(&app,"PATCH",&path,Some(&[("bots_dca_multi_asset[label]",name.as_str())]),Csrf::Header,&headers).await;
    assert_eq!(named.status,200,"{}",named.body);
    assert_eq!(label()?,Some(name),"the shown name, submitted, is saved");
    Ok(())
}

/// The bot menu's Custom allocation (web::bot::index_switch) where this build parts from Rails: it customizes from the stored
/// members (D-IDX-CUSTOM-STORED: Rails re-derives the index in the request) and refuses while the engine owns the bot or a
/// resync is due (D-FENCE-PER-BOT); and on success it bumps the run generation.
mod custom_allocation {
    use super::common;
    use common::{seed, web as harness};
    use deltabadger::web::{self, bot::index_switch::{customize, Switched}, layout::Ctx, Params};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::sync::Arc;
    type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
    const NOW: &str = "2026-09-10T12:00:30.123456Z";

    struct Fixture { _dir: tempfile::TempDir, c: Connection, ctx: Ctx, user: i64, id: i64, assets: Vec<i64> }
    impl Fixture {
        /// A stopped ND-style index bot of three stocks, stored at weights the index's own (and its caps') would not give.
        fn new() -> Result<Self> {
            let (dir, opened, s) = common::install_alpaca();
            let c = opened.primary;
            c.execute("UPDATE users SET wash_sale_enabled=0", [])?;
            let members: Vec<(i64, i64)> = ["NVDA", "MSFT", "AAPL"].iter().map(|symbol| seed::add_alpaca_stock(&c, &s, symbol)).collect();
            seed::insert_index(&c, "nasdaq-100", &["NVDA.US", "MSFT.US", "AAPL.US"], &json!({"NVDA.US": 1, "MSFT.US": 1, "AAPL.US": 98}));
            let id = seed::index_bot(&c, &s, "nasdaq-100", 3, 0.0, false);
            c.execute("UPDATE bots SET status=2 WHERE id=?1", [id])?;
            for ((asset, ticker), weight) in members.iter().zip([0.5, 0.3, 0.2]) {
                seed::insert_member(&c, id, *asset, *ticker, weight, true, "2026-09-01 14:00:00", None);
            }
            let app = harness::app(dir.path(), "engine-test-secret", harness::TestClock::at(NOW));
            let ctx = Ctx { app, params: Arc::new(Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: None }),
                session: web::session::Session::new(Default::default()), current: web::auth::Current::SignedOut,
                method: axum::http::Method::POST, locale: "en", nonce: String::new(), now: harness::at(NOW), turbo_frame: None, stream: false };
            Ok(Self { _dir: dir, c, ctx, user: s.user_id, id, assets: members.into_iter().map(|(asset, _)| asset).collect() })
        }
        fn customize(&self) -> Result<Switched> { Ok(customize(&self.c, &self.ctx, self.user, self.id).map_err(|e| format!("{e:?}"))?) }
        fn row(&self) -> Result<(String, Value, Value)> {
            let (class, settings, transient): (String, String, String) = self.c.query_row("SELECT type, settings, transient_data FROM bots WHERE id=?1", [self.id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok((class, serde_json::from_str(&settings)?, serde_json::from_str(&transient)?))
        }
        fn snapshot(&self) -> Result<Vec<String>> {
            let mut all = vec![];
            for table in ["bots", "bot_index_assets"] {
                let mut q = self.c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
                let n = q.column_count();
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? { all.push(format!("{table}:{:?}", (0..n).map(|i| r.get_ref(i).map(|v| format!("{v:?}"))).collect::<std::result::Result<Vec<_>, _>>()?)); }
            }
            Ok(all)
        }
        fn refused_unchanged(&self, expected: &str) -> Result {
            let before = self.snapshot()?;
            let Switched::Refused(text) = self.customize()? else { return Err("expected a refusal".into()) };
            assert_eq!(text, expected);
            assert_eq!(self.snapshot()?, before, "a refused switch writes nothing");
            Ok(())
        }
    }

    /// D-IDX-CUSTOM-STORED: the stored weights are kept, though the index (and a resync that failed to run) would weigh AAPL
    /// at 98%; a member its venue no longer trades is left out and exits. The run generation moves, fencing an engine write.
    #[test]
    fn customizes_the_stored_members_and_bumps_the_generation() -> Result {
        let f = Fixture::new()?;
        f.c.execute("UPDATE tickers SET available=0 WHERE base='AAPL'", [])?;
        assert!(matches!(f.customize()?, Switched::Done));
        let (class, settings, transient) = f.row()?;
        assert_eq!(class, "Bots::DcaMultiAsset");
        assert_eq!(settings["allocations"], json!({f.assets[0].to_string(): 0.625, f.assets[1].to_string(): 0.375}));
        assert!(settings.get("num_coins").is_none() && settings.get("index_category_id").is_none(), "only the portfolio's own settings stay");
        assert_eq!(transient["run_generation"], json!(1));
        let exited: bool = f.c.query_row("SELECT in_index = 0 AND exited_at IS NOT NULL FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2", [f.id, f.assets[2]], |r| r.get(0))?;
        assert!(exited);
        Ok(())
    }

    /// D-IDX-CUSTOM-STORED: a resync the engine still owes (a settings save or a start asked for it) refuses, so the stored
    /// members are never older than what Rails would derive.
    #[test]
    fn a_resync_due_refuses() -> Result {
        let f = Fixture::new()?;
        deltabadger::engine::index::mark_resync(&f.c, f.id, harness::at(NOW)).map_err(|e| format!("{e:?}"))?;
        f.refused_unchanged("Wait for the pending swap or sale to finish.")
    }

    /// D-FENCE-PER-BOT: a stopped bot whose placement intent is still recorded is the engine's; nothing is written.
    #[test]
    fn a_placement_intent_refuses() -> Result {
        let f = Fixture::new()?;
        f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement',json('{\"asset\":1}')) WHERE id=?1", [f.id])?;
        f.refused_unchanged("Wait for the pending swap or sale to finish.")
    }

    /// The bot menu's Follow index over the fixture's members as a portfolio.
    fn as_basket(f: &Fixture) -> Result {
        let allocations = json!({f.assets[0].to_string(): 0.5, f.assets[1].to_string(): 0.3, f.assets[2].to_string(): 0.2});
        let quote: i64 = f.c.query_row("SELECT json_extract(settings,'$.quote_asset_id') FROM bots WHERE id=?1", [f.id], |r| r.get(0))?;
        f.c.execute("UPDATE bots SET type='Bots::DcaMultiAsset', settings=?1 WHERE id=?2",
            (json!({"quote_asset_id": quote, "quote_amount": 60.0, "interval": "week", "allocations": allocations}).to_string(), f.id))?;
        Ok(())
    }
    fn follow(f: &Fixture, category: &str) -> Result<Switched> { Ok(deltabadger::web::bot::index_switch::follow(&f.c, &f.ctx, f.user, f.id, Some(category)).map_err(|e| format!("{e:?}"))?) }
    fn follow_refused_unchanged(f: &Fixture, category: &str, expected: &str) -> Result {
        let before = f.snapshot()?;
        let Switched::Refused(text) = follow(f, category)? else { return Err("expected a refusal".into()) };
        assert_eq!(text, expected);
        assert_eq!(f.snapshot()?, before, "a refused switch writes nothing");
        Ok(())
    }

    /// D-IDX-FOLLOW-LATE: the switch leaves the members as they were and the engine a resync marker; the bounded index is held
    /// whole, and the run generation moves, fencing an engine write.
    #[test]
    fn follows_an_index_and_leaves_the_engine_its_resync() -> Result {
        let f = Fixture::new()?;
        as_basket(&f)?;
        let members = f.snapshot()?.into_iter().filter(|row| row.starts_with("bot_index_assets")).collect::<Vec<_>>();
        assert!(matches!(follow(&f, "nasdaq-100")?, Switched::Done));
        let (class, settings, transient) = f.row()?;
        assert_eq!(class, "Bots::DcaIndex");
        assert_eq!((&settings["index_type"], &settings["index_category_id"], &settings["num_coins"], &settings["hold_all"]), (&json!("category"), &json!("nasdaq-100"), &json!(3), &json!(true)));
        assert!(settings.get("allocations").is_none(), "only the index bot's own settings stay");
        assert_eq!(transient["run_generation"], json!(1));
        assert!(transient[deltabadger::engine::index::RESYNC_KEY].is_object(), "the engine owes the bot its derivation");
        assert_eq!(f.snapshot()?.into_iter().filter(|row| row.starts_with("bot_index_assets")).collect::<Vec<_>>(), members);
        Ok(())
    }

    /// D-FENCE-PER-BOT: a portfolio whose placement intent is still recorded is the engine's; nothing is written.
    #[test]
    fn a_placement_intent_refuses_a_follow() -> Result {
        let f = Fixture::new()?;
        as_basket(&f)?;
        f.c.execute("UPDATE bots SET transient_data=json_set(transient_data,'$.rust_placement',json('{\"asset\":1}')) WHERE id=?1", [f.id])?;
        follow_refused_unchanged(&f, "nasdaq-100", "Wait for the pending swap or sale to finish.")
    }

    /// D-IDX-COINGECKO: on a CoinGecko install this engine derives no index, so the switch Rails would make is refused.
    #[test]
    fn a_coingecko_install_refuses_a_follow() -> Result {
        let f = Fixture::new()?;
        as_basket(&f)?;
        f.c.execute("UPDATE indices SET source='coingecko'", [])?;
        deltabadger::app_config::set_plain(&f.c, "market_data_provider", "coingecko", harness::at(NOW)).map_err(|e| format!("{e:?}"))?;
        follow_refused_unchanged(&f, "nasdaq-100", "This app can't run that yet: an index bot whose market data comes from CoinGecko")
    }

    /// An index bot is not offered the index it follows; a selling portfolio may not follow one.
    #[test]
    fn the_followed_index_is_not_offered_and_a_seller_refuses() -> Result {
        let f = Fixture::new()?;
        follow_refused_unchanged(&f, "nasdaq-100", "This index isn't available for this bot.")?;
        as_basket(&f)?;
        f.c.execute("UPDATE bots SET settings=json_set(settings,'$.direction','selling') WHERE id=?1", [f.id])?;
        follow_refused_unchanged(&f, "nasdaq-100", "A bot that is selling can't follow an index. Switch it back to buying first.")
    }

    #[test]
    fn a_working_bot_refuses_and_a_basket_is_not_found() -> Result {
        let f = Fixture::new()?;
        f.c.execute("UPDATE bots SET status=1 WHERE id=?1", [f.id])?;
        f.refused_unchanged("Stop the bot first.")?;
        f.c.execute("UPDATE bots SET status=2, type='Bots::DcaMultiAsset' WHERE id=?1", [f.id])?;
        assert!(matches!(f.customize()?, Switched::NotIndex));
        Ok(())
    }
}

/// The dashboard's Merge (web::bot::merge) beyond the page grid: the per-bot fence, a pending reversal, and the asset cap.
mod merge {
    use super::common;
    use common::{seed, web as harness};
    use deltabadger::web::{self, bot::merge::{perform, Merged}, layout::Ctx, Params};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::sync::Arc;
    type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
    const NOW: &str = "2026-09-10T12:00:30.123456Z";

    struct Fixture { _dir: tempfile::TempDir, c: Connection, ctx: Ctx, s: seed::Seeded, bots: Vec<i64> }
    impl Fixture {
        /// Two stopped Alpaca baskets: `first` stocks in one, `second` in the other, each a member row per stock.
        fn new(first: usize, second: usize) -> Result<Self> {
            let (dir, opened, s) = common::install_alpaca();
            let c = opened.primary;
            c.execute("UPDATE users SET wash_sale_enabled=0", [])?;
            let mut bots = vec![];
            for (n, count) in [first, second].into_iter().enumerate() {
                let stocks: Vec<(i64, i64)> = (0..count).map(|i| seed::add_alpaca_stock(&c, &s, &format!("S{n}X{i}"))).collect();
                let weights: Vec<(i64, f64)> = stocks.iter().map(|(asset, _)| (*asset, 1.0 / count as f64)).collect();
                let mut spec = seed::BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&weights);
                spec.status = 2;
                let id = seed::insert_bot(&c, &s, &spec);
                for (asset, ticker) in &stocks { seed::insert_member(&c, id, *asset, *ticker, 1.0 / count as f64, true, "2026-09-01 14:00:00", None); }
                bots.push(id);
            }
            let app = harness::app(dir.path(), "engine-test-secret", harness::TestClock::at(NOW));
            let ctx = Ctx { app, params: Arc::new(Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: None }),
                session: web::session::Session::new(Default::default()), current: web::auth::Current::SignedOut,
                method: axum::http::Method::POST, locale: "en", nonce: String::new(), now: harness::at(NOW), turbo_frame: None, stream: false };
            Ok(Self { _dir: dir, c, ctx, s, bots })
        }
        fn merge(&self) -> Result<Merged> { self.merge_onto(None) }
        fn merge_onto(&self, exchange: Option<i64>) -> Result<Merged> { Ok(perform(&self.c, &self.ctx, self.s.user_id, self.bots.clone(), exchange).map_err(|e| format!("{e:?}"))?) }
        fn transient(&self, id: i64, key: &str, value: Value) -> Result {
            self.c.execute("UPDATE bots SET transient_data=json_set(transient_data,?1,json(?2)) WHERE id=?3", (format!("$.{key}"), value.to_string(), id))?;
            Ok(())
        }
        fn snapshot(&self) -> Result<Vec<String>> {
            let mut all = vec![];
            for table in ["bots", "bot_index_assets", "transactions", "bot_activity_logs"] {
                let mut q = self.c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
                let n = q.column_count();
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? { all.push(format!("{table}:{:?}", (0..n).map(|i| r.get_ref(i).map(|v| format!("{v:?}"))).collect::<std::result::Result<Vec<_>, _>>()?)); }
            }
            Ok(all)
        }
        fn refused_unchanged(&self, expected: &str) -> Result { self.refused_onto(None, expected) }
        fn refused_onto(&self, exchange: Option<i64>, expected: &str) -> Result {
            let before = self.snapshot()?;
            match self.merge_onto(exchange)? { Merged::Refused(text) => assert!(text.starts_with(expected), "{text}"), _ => return Err("expected a refusal".into()) }
            assert_eq!(self.snapshot()?, before, "a refused merge writes nothing");
            Ok(())
        }
        fn waiting_order(&self, bot: i64) -> Result {
            let asset: i64 = self.c.query_row("SELECT asset_id FROM bot_index_assets WHERE bot_id=?1 ORDER BY id LIMIT 1", [bot], |r| r.get(0))?;
            let symbol: String = self.c.query_row("SELECT symbol FROM assets WHERE id=?1", [asset], |r| r.get(0))?;
            seed::insert_stock_tx(&self.c, &self.s, bot, asset, &symbol, &seed::TxSpec { status: 0, external_status: Some(0), external_id: Some("resting".into()), order_type: 1,
                amount: Some("1"), quote_amount: None, price: Some("10"), quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-09-10 11:00:00".into() });
            Ok(())
        }
    }

    /// The sources are deleted and their run generation moves, fencing an engine write already in flight.
    #[test]
    fn merges_and_fences_the_sources() -> Result {
        let f = Fixture::new(1, 1)?;
        let Merged::Done(id) = f.merge()? else { return Err("expected a merge".into()) };
        for source in &f.bots {
            let (status, generation): (i64, i64) = f.c.query_row("SELECT status, json_extract(transient_data,'$.run_generation') FROM bots WHERE id=?1", [source], |r| Ok((r.get(0)?, r.get(1)?)))?;
            assert_eq!((status, generation), (3, 1));
        }
        let (status, members): (i64, i64) = f.c.query_row("SELECT status, (SELECT count(*) FROM bot_index_assets WHERE bot_id=?1 AND in_index=1) FROM bots WHERE id=?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!((status, members), (2, 2));
        Ok(())
    }

    /// D-FENCE-PER-BOT: a source whose placement intent is still recorded is the engine's; nothing is written.
    #[test]
    fn a_placement_intent_refuses() -> Result {
        let f = Fixture::new(1, 1)?;
        f.transient(f.bots[1], "rust_placement", json!({"asset": 1}))?;
        f.refused_unchanged("S0X0 can't be merged right now.")
    }

    /// D-MERGE-REVERSAL-PENDING: a reversal's cancels, then its reserve credit alone, refuse; once settled the merge goes ahead,
    /// and the merged bot does not inherit the source's carry.
    #[test]
    fn a_pending_reversal_refuses_until_it_settles() -> Result {
        let f = Fixture::new(1, 1)?;
        f.transient(f.bots[1], "missed_quote_amount", json!("12.5"))?;
        f.transient(f.bots[1], deltabadger::engine::reversal::CANCELS, json!([{"id": 9, "registered": true, "sent": false}]))?;
        f.transient(f.bots[1], deltabadger::engine::reversal::RESERVE_PENDING, json!([9]))?;
        f.refused_unchanged("S1X0 can't be merged right now.")?;
        f.c.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.rust_reversal_cancels') WHERE id=?1", [f.bots[1]])?;
        f.refused_unchanged("S1X0 can't be merged right now.")?;
        f.c.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.flip_reserve_pending') WHERE id=?1", [f.bots[1]])?;
        let Merged::Done(id) = f.merge()? else { return Err("expected a merge once settled".into()) };
        let carry: i64 = f.c.query_row("SELECT json_extract(transient_data,'$.missed_quote_amount') FROM bots WHERE id=?1", [id], |r| r.get(0))?;
        assert_eq!(carry, 0);
        Ok(())
    }

    /// D-MERGE-CAP: past MAX_ASSETS the merged bot is saved as Rails saves it (stopped, not startable), unless a waiting order
    /// moves with it, which this engine's guard refuses.
    #[test]
    fn over_the_cap_a_moved_waiting_order_refuses() -> Result {
        let f = Fixture::new(60, 41)?;
        f.waiting_order(f.bots[0])?;
        f.refused_unchanged("This app can't run that yet: ")?;
        let g = Fixture::new(60, 41)?;
        assert!(matches!(g.merge()?, Merged::Done(_)), "without outstanding work the over-cap bot is saved");
        Ok(())
    }

    /// Interactive Brokers lists every member too, but this build serves no bot there: it is not offered, and picked
    /// anyway the merge is refused before it writes, the sources and the dashboard served as they were.
    #[tokio::test]
    async fn a_venue_this_build_does_not_run_is_neither_offered_nor_merged_onto() -> Result {
        use deltabadger::web::bot::merge::{modal, Loaded, Merge};
        let f = Fixture::new(1, 1)?;
        f.c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Ibkr', 'Interactive Brokers', '0.05', '0.05', ?1, ?1)", [NOW])?;
        let ibkr = f.c.last_insert_rowid();
        f.c.execute_batch(&format!("CREATE TEMP TABLE listed AS SELECT * FROM tickers WHERE exchange_id = {}; UPDATE listed SET id = NULL, exchange_id = {ibkr}; \
                                    INSERT INTO tickers SELECT * FROM listed; DROP TABLE listed;", f.s.exchange_id))?;
        let Loaded::Merge(mut merge) = Merge::load(&f.c, &f.ctx, f.s.user_id, f.bots.clone(), None).map_err(|e| format!("{e:?}"))? else { return Err("expected a merge".into()) };
        let page = modal(&f.c, &f.ctx, &mut merge, "token").map_err(|e| format!("{e:?}"))?;
        assert!(page.contains("Alpaca") && !page.contains("Interactive Brokers") && !page.contains("<select"), "{page}");
        f.refused_onto(Some(ibkr), "This app can't run that yet: a bot on an exchange other than ")?;
        let hash = deltabadger::crypto::hash_password("Correct-horse-9").map_err(|e| format!("{e:?}"))?;
        f.c.execute("UPDATE users SET encrypted_password=?1,confirmed_at=created_at", [hash])?;
        let app = harness::app(f._dir.path(), "engine-test-secret", harness::TestClock::at(NOW))
            .with_figure_source(deltabadger::web::figure::loading::Source::Disabled).map_err(|e| format!("{e:?}"))?;
        let mut browser = harness::Browser::default();
        browser.get(&app, "/login").await;
        assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
        assert_eq!(browser.get(&app, "/bots").await.status, 200);
        Ok(())
    }
}

mod split {
    use super::common;
    use common::{seed, web as harness};
    use deltabadger::figures::{at::At, db::Subject, dec::Dec, num::Num, walk::{self, Metrics}};
    use deltabadger::web::{self, bot::split::{modal, perform, Loaded, Performed, Split}, layout::Ctx, Params};
    use rusqlite::Connection;
    use serde_json::{json, Value};
    use std::sync::Arc;
    type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
    const NOW: &str = "2026-09-10T12:00:30.123456Z";

    struct Fixture { _dir: tempfile::TempDir, c: Connection, queue: Connection, ctx: Ctx, s: seed::Seeded, bots: Vec<i64>, assets: Vec<Vec<(i64, i64)>> }
    impl Fixture {
        fn open() -> Result<(tempfile::TempDir, Connection, Connection, Ctx, seed::Seeded)> {
            let (dir, opened, s) = common::install_alpaca();
            opened.primary.execute("UPDATE users SET wash_sale_enabled=0", [])?;
            let app = harness::app(dir.path(), "engine-test-secret", harness::TestClock::at(NOW));
            let ctx = Ctx { app, params: Arc::new(Params { upload: None, full_path: String::new(), fullpath: String::new(), route_path: String::new(), path_locale: None, query: vec![], form: vec![], json: None }),
                session: web::session::Session::new(Default::default()), current: web::auth::Current::SignedOut,
                method: axum::http::Method::POST, locale: "en", nonce: String::new(), now: harness::at(NOW), turbo_frame: None, stream: false };
            Ok((dir, opened.primary, opened.queue, ctx, s))
        }
        /// Stopped Alpaca baskets at 60 weekly, one per list of weights: a stock (S<n>X<i>) per weight, each a member row.
        fn baskets(weights: &[&[f64]]) -> Result<Self> {
            let (dir, c, queue, ctx, s) = Self::open()?;
            let (mut bots, mut assets) = (vec![], vec![]);
            for (n, list) in weights.iter().enumerate() {
                let stocks: Vec<(i64, i64)> = (0..list.len()).map(|i| seed::add_alpaca_stock(&c, &s, &format!("S{n}X{i}"))).collect();
                let pairs: Vec<(i64, f64)> = stocks.iter().zip(list.iter()).map(|((asset, _), w)| (*asset, *w)).collect();
                let mut spec = seed::BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&pairs);
                spec.status = 2;
                let id = seed::insert_bot(&c, &s, &spec);
                c.execute("UPDATE bots SET position=?1 WHERE id=?2", (5 + n as i64, id))?;
                for ((asset, ticker), w) in stocks.iter().zip(list.iter()) { seed::insert_member(&c, id, *asset, *ticker, *w, true, "2026-09-01 14:00:00", None); }
                bots.push(id);
                assets.push(stocks);
            }
            Ok(Self { _dir: dir, c, queue, ctx, s, bots, assets })
        }
        /// A stopped Alpaca index bot at 60 weekly (an index's special rows are read; a basket's are refused before the split).
        fn index(weights: &[f64]) -> Result<Self> {
            let (dir, c, queue, ctx, s) = Self::open()?;
            let stocks: Vec<(i64, i64)> = (0..weights.len()).map(|i| seed::add_alpaca_stock(&c, &s, &format!("S0X{i}"))).collect();
            let symbols: Vec<String> = (0..weights.len()).map(|i| format!("S0X{i}.US")).collect();
            seed::insert_index(&c, "nasdaq-100", &symbols.iter().map(String::as_str).collect::<Vec<_>>(), &json!({}));
            let id = seed::index_bot(&c, &s, "nasdaq-100", weights.len() as i64, 0.0, false);
            c.execute("UPDATE bots SET status=2, position=5 WHERE id=?1", [id])?;
            for ((asset, ticker), w) in stocks.iter().zip(weights) { seed::insert_member(&c, id, *asset, *ticker, *w, true, "2026-09-01 14:00:00", None); }
            Ok(Self { _dir: dir, c, queue, ctx, s, bots: vec![id], assets: vec![stocks] })
        }
        /// Another stock in the source's history: an exited member row on `bot`.
        fn exited(&self, bot: i64, symbol: &str) -> Result<(i64, i64)> {
            let (asset, ticker) = seed::add_alpaca_stock(&self.c, &self.s, symbol);
            seed::insert_member(&self.c, bot, asset, ticker, 0.0, false, "2026-08-01 14:00:00", Some("2026-09-01 14:00:00"));
            Ok((asset, ticker))
        }
        /// A closed fill: `side` 0 buy / 1 sell, of `kind`, `amount` units for `quote`, recorded under `symbol` (None: no asset).
        #[allow(clippy::too_many_arguments)]
        fn order(&self, bot: i64, asset: Option<i64>, symbol: &str, side: i64, kind: &str, amount: &'static str, quote: &'static str, price: &'static str, at: &str) -> Result<i64> {
            let id = seed::insert_stock_tx(&self.c, &self.s, bot, asset.unwrap_or(0), symbol, &seed::TxSpec { status: 0, external_status: Some(2), external_id: Some(format!("o-{at}-{symbol}")), order_type: 0,
                amount: Some(amount), quote_amount: Some(quote), price: Some(price), quote_amount_exec: Some(quote), amount_exec: Some(amount), created_at: at.into() });
            self.c.execute("UPDATE transactions SET side=?1, transaction_type=?2, base_asset_id=?3 WHERE id=?4", (side, kind, asset, id))?;
            Ok(id)
        }
        fn waiting(&self, bot: i64, asset: i64, symbol: &str, kind: &str) -> Result<i64> {
            let id = seed::insert_stock_tx(&self.c, &self.s, bot, asset, symbol, &seed::TxSpec { status: 0, external_status: Some(1), external_id: Some("resting".into()), order_type: 1,
                amount: Some("1"), quote_amount: None, price: Some("10"), quote_amount_exec: Some("0"), amount_exec: Some("0"), created_at: "2026-09-10 11:00:00".into() });
            self.c.execute("UPDATE transactions SET transaction_type=?1 WHERE id=?2", (kind, id))?;
            Ok(id)
        }
        fn split(&self, keep: &[i64]) -> Result<Performed> { self.split_ids(self.bots.clone(), keep) }
        fn split_ids(&self, ids: Vec<i64>, keep: &[i64]) -> Result<Performed> { Ok(perform(&self.c, &self.ctx, self.s.user_id, ids, keep.to_vec()).map_err(|e| format!("{e:?}"))?) }
        fn done(&self, keep: &[i64]) -> Result<Vec<i64>> {
            match self.split(keep)? {
                Performed::Done(done) => { assert_eq!(done.sources, self.bots); Ok(done.children.iter().map(|child| child.id).collect()) }
                Performed::Refused(r) => Err(format!("refused: {} {}", r.code, r.reason).into()),
                Performed::Unported(why) => Err(format!("unported: {why}").into()),
            }
        }
        fn transient(&self, id: i64, key: &str, value: Value) -> Result {
            self.c.execute("UPDATE bots SET transient_data=json_set(transient_data,?1,json(?2)) WHERE id=?3", (format!("$.{key}"), value.to_string(), id))?;
            Ok(())
        }
        fn snapshot(&self) -> Result<Vec<String>> {
            let mut all = vec![];
            for table in ["bots", "bot_index_assets", "transactions", "bot_activity_logs"] {
                let mut q = self.c.prepare(&format!("SELECT * FROM {table} ORDER BY id"))?;
                let n = q.column_count();
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? { all.push(format!("{table}:{:?}", (0..n).map(|i| r.get_ref(i).map(|v| format!("{v:?}"))).collect::<std::result::Result<Vec<_>, _>>()?)); }
            }
            Ok(all)
        }
        fn refused_unchanged(&self, keep: &[i64], code: &str, text: &str) -> Result {
            let before = self.snapshot()?;
            match self.split(keep)? { Performed::Refused(r) => { assert_eq!(r.code, code); assert!(r.reason.starts_with(text), "{}", r.reason); } _ => return Err("expected a refusal".into()) }
            assert_eq!(self.snapshot()?, before, "a refused split writes nothing");
            Ok(())
        }
        fn unported_unchanged(&self) -> Result<&'static str> {
            let before = self.snapshot()?;
            let Performed::Unported(why) = self.split(&[])? else { return Err("expected Unported".into()) };
            assert_eq!(self.snapshot()?, before, "an unserved split writes nothing");
            Ok(why)
        }
        fn page(&self) -> Result<std::result::Result<String, &'static str>> {
            let Loaded::Split(mut split) = Split::load(&self.c, &self.ctx, self.s.user_id, self.bots.clone(), vec![]).map_err(|e| format!("{e:?}"))? else { return Ok(Err("load: unported")) };
            Ok(modal(&self.c, &self.ctx, &mut split, "token").map_err(|e| format!("{e:?}"))?)
        }
        fn bot_of(&self, row: i64) -> Result<(i64, String)> { Ok(self.c.query_row("SELECT bot_id, transaction_type FROM transactions WHERE id=?1", [row], |r| Ok((r.get(0)?, r.get(1)?)))?) }
        fn books(&self, bot: i64) -> Result<Metrics> {
            let at = At::from_utc(harness::at(NOW)).ok_or("time")?;
            Ok(walk::metrics(&self.c, &Subject::load(&self.c, bot).map_err(|e| format!("{e:?}"))?, at).map_err(|e| format!("{e:?}"))?)
        }
    }

    fn d(n: &Num) -> Dec { n.to_d().unwrap() }
    fn dec(text: &str) -> Dec { Dec::parse(text).unwrap() }
    fn sum(values: impl IntoIterator<Item = Dec>) -> Dec { values.into_iter().fold(Dec::zero(), |acc, v| (&acc + &v).unwrap()) }
    /// What the bot made, in money: value less what went in (split_test.rb's `profit`).
    fn profit(m: &Metrics) -> Dec { (&d(&m.total_amount_value_in_quote) - &d(&m.total_quote_amount_invested)).unwrap() }
    /// Units, tax units and tax basis of the holding of `asset`.
    fn holding(m: &Metrics, asset: i64) -> (Dec, Dec, Dec) {
        let Some(key) = m.key_for(asset) else { return (Dec::zero(), Dec::zero(), Dec::zero()) };
        let entry = &m.asset_breakdown.iter().find(|(k, _)| k == key).unwrap().1;
        (d(&entry.amount), d(&entry.tax_units), d(&entry.tax_basis))
    }
    /// Each asset's units and tax lots, summed over the children, are the source's.
    fn lots_kept(before: &Metrics, after: &[Metrics], assets: &[i64]) {
        for asset in assets {
            let (units, tax_units, basis) = holding(before, *asset);
            let parts: Vec<_> = after.iter().map(|m| holding(m, *asset)).collect();
            assert_eq!(sum(parts.iter().map(|p| p.0.clone())), units, "units of {asset}");
            assert_eq!(sum(parts.iter().map(|p| p.1.clone())), tax_units, "tax units of {asset}");
            assert_eq!(sum(parts.iter().map(|p| p.2.clone())), basis, "tax basis of {asset}");
        }
    }

    /// Two stopped one-asset children at 30 each, labelled by their asset, at the source's position; the source is deleted with
    /// its run generation moved; its own activity stays on it and each child logs `split`.
    #[test]
    fn splits_and_fences_the_source() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5]])?;
        deltabadger::engine::model::log_activity(&f.c, f.bots[0], "started", deltabadger::engine::model::Level::Info, json!({}), harness::at(NOW)).map_err(|e| format!("{e:?}"))?;
        let children = f.done(&[])?;
        assert_eq!(children.len(), 2);
        let (status, generation): (i64, i64) = f.c.query_row("SELECT status, json_extract(transient_data,'$.run_generation') FROM bots WHERE id=?1", [f.bots[0]], |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!((status, generation), (3, 1));
        for (child, symbol) in children.iter().zip(["S0X0", "S0X1"]) {
            let (class, status, label, position, amount, interval): (String, i64, String, i64, f64, String) = f.c.query_row(
                "SELECT type, status, label, position, json_extract(settings,'$.quote_amount'), json_extract(settings,'$.interval') FROM bots WHERE id=?1", [child],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))?;
            assert_eq!((class.as_str(), status, label.as_str(), position, amount, interval.as_str()), ("Bots::DcaMultiAsset", 2, symbol, 5, 30.0, "week"));
            let logs: Vec<(String, String)> = f.c.prepare("SELECT event, details FROM bot_activity_logs WHERE bot_id=?1")?.query_map([child], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<std::result::Result<_, _>>()?;
            assert_eq!(logs, vec![("split".to_string(), json!({"source_id": f.bots[0], "source_label": "S0X0, S0X1"}).to_string())]);
        }
        let kept: i64 = f.c.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1 AND event='started'", [f.bots[0]], |r| r.get(0))?;
        assert_eq!(kept, 1, "the source's activity stays on the deleted source");
        Ok(())
    }

    /// Unequal weights (70/20/10) give each child its share; a member that no longer trades gets no child, and the survivors
    /// share the whole schedule (Rails sums the weights with Kahan-Babuska summation, crate::ruby::float_sum).
    #[test]
    fn shares_follow_the_weights_and_the_survivors_keep_the_schedule() -> Result {
        let f = Fixture::baskets(&[&[0.7, 0.2, 0.1]])?;
        let amounts = |children: &[i64]| -> Result<Vec<f64>> {
            children.iter().map(|id| Ok(f.c.query_row("SELECT json_extract(settings,'$.quote_amount') FROM bots WHERE id=?1", [id], |r| r.get::<_, f64>(0))?)).collect()
        };
        let shares = amounts(&f.done(&[])?)?;
        for (share, expected) in shares.iter().zip([42.0, 12.0, 6.0]) { assert!((share - expected).abs() < 1e-9, "{shares:?}"); }
        let g = Fixture::baskets(&[&[0.5, 0.3, 0.2]])?;
        g.c.execute("UPDATE bots SET settings=json_set(settings,'$.quote_amount',100) WHERE id=?1", [g.bots[0]])?;
        g.c.execute("UPDATE tickers SET trading_enabled=0 WHERE base='S0X2'", [])?;
        let children = g.done(&[])?;
        let shares: Vec<f64> = children.iter().map(|id| g.c.query_row("SELECT json_extract(settings,'$.quote_amount') FROM bots WHERE id=?1", [id], |r| r.get::<_, f64>(0))).collect::<std::result::Result<_, _>>()?;
        assert_eq!(shares.len(), 2);
        assert!((shares[0] - 62.5).abs() < 1e-9 && (shares.iter().sum::<f64>() - 100.0).abs() < 1e-9, "{shares:?}");
        let exited: i64 = g.c.query_row("SELECT in_index FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2", (children[0], g.assets[0][2].0), |r| r.get(0))?;
        assert_eq!(exited, 0, "the member without a child lands on the first child, exited");
        Ok(())
    }

    /// Every row moves to its asset's child as REGULAR (a rebalance, a liquidation, a redeploy among them); an exited asset's rows
    /// and an orphan row go to the first child with the exited membership; a legacy row goes by its symbol.
    #[test]
    fn rows_follow_their_asset_as_regular() -> Result {
        let f = Fixture::index(&[0.6, 0.4])?;
        let (bot, (a, b)) = (f.bots[0], (f.assets[0][0].0, f.assets[0][1].0));
        let (c, _) = f.exited(bot, "S0X9")?;
        let a_buy = f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        let b_swap = f.order(bot, Some(b), "S0X1", 0, "REBALANCE", "1", "100", "100", "2026-09-02 11:00:00")?;
        let c_buy = f.order(bot, Some(c), "S0X9", 0, "REGULAR", "1", "100", "100", "2026-09-02 12:00:00")?;
        let c_sold = f.order(bot, Some(c), "S0X9", 1, "LIQUIDATION", "1", "100", "100", "2026-09-03 12:00:00")?;
        let a_redeploy = f.order(bot, Some(a), "S0X0", 0, "REDEPLOY", "1", "100", "100", "2026-09-04 12:00:00")?;
        let legacy = f.order(bot, None, "S0X1", 0, "REGULAR", "1", "100", "100", "2026-09-05 12:00:00")?;
        let orphan = f.order(bot, None, "ZZZ", 0, "REGULAR", "1", "100", "100", "2026-09-05 13:00:00")?;
        let children = f.done(&[])?;
        for (row, child) in [(a_buy, 0), (a_redeploy, 0), (b_swap, 1), (legacy, 1), (c_buy, 0), (c_sold, 0), (orphan, 0)] {
            assert_eq!(f.bot_of(row)?, (children[child], "REGULAR".to_string()), "row {row}");
        }
        let (in_index, exited): (i64, Option<String>) = f.c.query_row("SELECT in_index, exited_at FROM bot_index_assets WHERE bot_id=?1 AND asset_id=?2", (children[0], c), |r| Ok((r.get(0)?, r.get(1)?)))?;
        assert_eq!((in_index, exited.as_deref()), (0, Some("2026-09-01 14:00:00")));
        let members: Vec<i64> = f.c.prepare("SELECT asset_id FROM bot_index_assets WHERE bot_id=?1 AND in_index=1")?.query_map([children[0]], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        assert_eq!(members, vec![a]);
        Ok(())
    }

    /// The child keeps no redeploy offset (its history offers nothing again), carries the source's resolved sales, and records
    /// its own last row as merged history; a child with no rows has no such key.
    #[test]
    fn transient_and_offset() -> Result {
        let f = Fixture::index(&[0.5, 0.3, 0.2])?;
        let (bot, a, b) = (f.bots[0], f.assets[0][0].0, f.assets[0][1].0);
        f.transient(bot, "liquidation_resolved_orders", json!([7, 9]))?;
        f.c.execute("UPDATE bots SET redeploy_declined_offset='12.5' WHERE id=?1", [bot])?;
        f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.order(bot, Some(a), "S0X0", 1, "LIQUIDATION", "1", "100", "100", "2026-09-03 10:00:00")?;
        f.order(bot, Some(b), "S0X1", 0, "REDEPLOY", "1", "87.5", "87.5", "2026-09-04 10:00:00")?;
        let children = f.done(&[])?;
        assert_eq!(children.len(), 3);
        for (n, child) in children.iter().enumerate() {
            let (offset, resolved, merged, last): (f64, Option<String>, Option<i64>, Option<i64>) = f.c.query_row(
                "SELECT CAST(redeploy_declined_offset AS REAL), json_extract(transient_data,'$.liquidation_resolved_orders'), json_extract(transient_data,'$.merged_history_until_id'), \
                 (SELECT max(id) FROM transactions WHERE bot_id=?1) FROM bots WHERE id=?1", [child], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            assert_eq!(offset, 0.0);
            assert_eq!(resolved.as_deref(), Some("[7,9]"));
            assert_eq!(merged, last);
            assert_eq!(merged.is_some(), n < 2, "child {n}");
            let special: i64 = f.c.query_row("SELECT count(*) FROM transactions WHERE bot_id=?1 AND transaction_type<>'REGULAR'", [child], |r| r.get(0))?;
            assert_eq!(special, 0, "nothing banked or spent: the child offers nothing");
        }
        Ok(())
    }

    /// Money from past sales waits for an answer: unanswered, the split is refused with each bot's offer; kept, it goes through.
    #[test]
    fn proceeds_wait_for_an_answer() -> Result {
        let f = Fixture::index(&[0.6, 0.4])?;
        let (bot, a) = (f.bots[0], f.assets[0][0].0);
        f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.order(bot, Some(a), "S0X0", 1, "LIQUIDATION", "1", "150", "150", "2026-09-03 10:00:00")?;
        let before = f.snapshot()?;
        let Performed::Refused(refusal) = f.split(&[])? else { return Err("expected the proceeds question".into()) };
        assert_eq!((refusal.code, refusal.reason.as_str()), ("proceeds_unanswered", "ND2 still has money from past sales. Reinvest it or keep it first."));
        assert_eq!(refusal.offers.iter().map(|(id, offer)| (*id, offer.to_s_f())).collect::<Vec<_>>(), vec![(bot, "150.0".to_string())]);
        assert_eq!(f.snapshot()?, before);
        let page = f.page()?.map_err(|e| e.to_string())?;
        assert!(page.contains("has 150.00 USD from past sales. Reinvest?") && page.contains(&format!("action=\"/bots/{bot}/redeploy\"")), "{page}");
        assert_eq!(f.done(&[bot])?.len(), 2);
        Ok(())
    }

    /// D-FENCE-PER-BOT: a source whose placement intent is still recorded is the engine's: busy, naming its venue.
    #[test]
    fn a_placement_intent_is_busy() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5]])?;
        f.transient(f.bots[0], "rust_placement", json!({"asset": 1}))?;
        f.refused_unchanged(&[], "busy", "Alpaca is busy with another order. Try again in a moment.")
    }

    /// D-SPLIT-REVERSAL-PENDING: a reversal's cancels, then its reserve credit alone, refuse; once settled the split goes ahead,
    /// and no child inherits the source's carry.
    #[test]
    fn a_pending_reversal_refuses_until_it_settles() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5]])?;
        let bot = f.bots[0];
        f.transient(bot, "missed_quote_amount", json!("12.5"))?;
        f.transient(bot, deltabadger::engine::reversal::CANCELS, json!([{"id": 9, "registered": true, "sent": false}]))?;
        f.transient(bot, deltabadger::engine::reversal::RESERVE_PENDING, json!([9]))?;
        f.refused_unchanged(&[], "unavailable", "S0X0, S0X1 can't be split right now.")?;
        f.c.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.rust_reversal_cancels') WHERE id=?1", [bot])?;
        f.refused_unchanged(&[], "unavailable", "S0X0, S0X1 can't be split right now.")?;
        f.c.execute("UPDATE bots SET transient_data=json_remove(transient_data,'$.flip_reserve_pending') WHERE id=?1", [bot])?;
        for child in f.done(&[])? {
            let carry: i64 = f.c.query_row("SELECT json_extract(transient_data,'$.missed_quote_amount') FROM bots WHERE id=?1", [child], |r| r.get(0))?;
            assert_eq!(carry, 0);
        }
        Ok(())
    }

    /// A redeploy still working (a waiting REDEPLOY row) is not served, on an index and on a basket, the modal and the split alike.
    #[test]
    fn a_redeploy_in_flight_is_not_served() -> Result {
        let f = Fixture::index(&[0.6, 0.4])?;
        f.waiting(f.bots[0], f.assets[0][0].0, "S0X0", "REDEPLOY")?;
        assert_eq!(f.page()?, Err("load: unported"));
        assert_eq!(f.unported_unchanged()?, "a redeploy in progress");
        let g = Fixture::baskets(&[&[0.5, 0.5]])?;
        g.waiting(g.bots[0], g.assets[0][0].0, "S0X0", "REDEPLOY")?;
        assert_eq!(g.page()?, Err("load: unported"));
        g.unported_unchanged()?;
        Ok(())
    }

    fn queue_job(f: &Fixture, execution: &str) -> Result {
        let gid = format!("gid://deltabadger/Bots::DcaIndex/{}", f.bots[0]);
        f.queue.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, priority, active_job_id, created_at, updated_at) VALUES ('default', 'Bot::RedeployJob', ?1, 0, 'aj', ?2, ?2)",
                        (json!({"job_class": "Bot::RedeployJob", "arguments": [{"_aj_globalid": gid}]}).to_string(), NOW))?;
        let job = f.queue.last_insert_rowid();
        match execution {
            "ready" => f.queue.execute("INSERT INTO solid_queue_ready_executions (job_id, queue_name, priority, created_at) VALUES (?1, 'default', 0, ?2)", (job, NOW))?,
            _ => f.queue.execute("INSERT INTO solid_queue_failed_executions (job_id, error, created_at) VALUES (?1, 'boom', ?2)", (job, NOW))?,
        };
        Ok(())
    }

    /// A Yes Rails still holds as a Bot::RedeployJob (no pending flag, no waiting order) is a redeploy in progress: not served.
    #[test]
    fn a_queued_redeploy_job_is_not_served() -> Result {
        let f = Fixture::index(&[0.6, 0.4])?;
        queue_job(&f, "ready")?;
        assert_eq!(f.page()?, Err("load: unported"));
        assert_eq!(f.unported_unchanged()?, "a redeploy in progress");
        Ok(())
    }

    /// The queue startup resolved (storage's, with DATABASE_PATH elsewhere and no QUEUE_DATABASE_PATH) is the one asked: a
    /// job held there is a redeploy in progress, though nothing sits beside the primary.
    #[test]
    fn the_queue_startup_resolved_is_asked() -> Result {
        let mut f = Fixture::index(&[0.6, 0.4])?;
        queue_job(&f, "ready")?;
        let (install, storage) = (f._dir.path().to_path_buf(), tempfile::tempdir()?);
        let primary = install.join("production.sqlite3").display().to_string();
        let env = |name: &str| match name { "SECRET_KEY_BASE" => Some("engine-test-secret".to_string()), "DATABASE_PATH" => Some(primary.clone()), _ => None };
        let paths = deltabadger::store::Paths::from_env(&env, storage.path());
        assert_eq!(paths.primary, install.join("production.sqlite3"));
        f.queue.execute("VACUUM INTO ?1", [paths.queue.display().to_string()])?;
        for file in ["production_queue.sqlite3", "production_queue.sqlite3-wal", "production_queue.sqlite3-shm"] { let _ = std::fs::remove_file(install.join(file)); }
        f.ctx.app = web::App::new(web::Config::from_env(&env).map_err(|e| format!("{e:?}"))?, &env, Connection::open(&primary)?, harness::TestClock::at(NOW))
            .and_then(|app| app.with_queue_path(paths.queue)).and_then(|app| app.with_figure_source(web::figure::loading::Source::Disabled)).map_err(|e| format!("{e:?}"))?;
        assert_eq!(f.page()?, Err("load: unported"));
        assert_eq!(f.unported_unchanged()?, "a redeploy in progress");
        Ok(())
    }

    /// A queue database that isn't there is never "no job": the split is refused and writes nothing.
    #[test]
    fn a_missing_queue_is_refused() -> Result {
        let f = Fixture::index(&[0.6, 0.4])?;
        for file in ["production_queue.sqlite3", "production_queue.sqlite3-wal", "production_queue.sqlite3-shm"] { let _ = std::fs::remove_file(f._dir.path().join(file)); }
        assert_eq!(f.page()?, Err("load: unported"));
        assert_eq!(f.unported_unchanged()?, "the job queue can't be read");
        Ok(())
    }

    /// A Bot::RedeployJob that failed is over (Automation::Schedulable#active_job?): the modal asks, and a kept answer splits.
    #[test]
    fn a_failed_redeploy_job_is_over() -> Result {
        let f = Fixture::index(&[0.6, 0.4])?;
        let (bot, a) = (f.bots[0], f.assets[0][0].0);
        f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.order(bot, Some(a), "S0X0", 1, "LIQUIDATION", "1", "150", "150", "2026-09-03 10:00:00")?;
        queue_job(&f, "failed")?;
        let page = f.page()?.map_err(|e| e.to_string())?;
        assert!(page.contains("data-split-row=\"offer\""), "{page}");
        assert_eq!(f.done(&[bot])?.len(), 2);
        Ok(())
    }

    /// A source on a venue this build does not run the children on: the modal says so by name and offers no Split; the split is
    /// refused before it writes.
    #[test]
    fn an_unrun_venue_is_refused_before_writing() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5]])?;
        f.c.execute("INSERT INTO exchanges (type, name, maker_fee, taker_fee, created_at, updated_at) VALUES ('Exchanges::Ibkr', 'Interactive Brokers', '0.05', '0.05', ?1, ?1)", [NOW])?;
        let ibkr = f.c.last_insert_rowid();
        f.c.execute_batch(&format!("CREATE TEMP TABLE listed AS SELECT * FROM tickers WHERE exchange_id = {}; UPDATE listed SET id = NULL, exchange_id = {ibkr}; \
                                    INSERT INTO tickers SELECT * FROM listed; DROP TABLE listed; UPDATE bots SET exchange_id = {ibkr};", f.s.exchange_id))?;
        let page = f.page()?.map_err(|e| e.to_string())?;
        assert!(page.contains("This app can&#39;t run that yet: a bot on an exchange other than "), "{page}");
        assert!(!page.contains("action=\"/bots/split\""), "{page}");
        f.refused_unchanged(&[], "write_refused", "This app can't run that yet: ")
    }

    /// A failure after the children exist and the rows have moved (the second child's `split` log) rolls everything back.
    #[test]
    fn a_failure_mid_write_leaves_nothing() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5]])?;
        f.order(f.bots[0], Some(f.assets[0][0].0), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.c.execute_batch("CREATE TEMP TRIGGER crash BEFORE INSERT ON bot_activity_logs WHEN NEW.event = 'split' AND (SELECT count(*) FROM bot_activity_logs WHERE event = 'split') >= 1 \
                           BEGIN SELECT RAISE(ABORT, 'crash'); END;")?;
        let before = f.snapshot()?;
        assert!(perform(&f.c, &f.ctx, f.s.user_id, f.bots.clone(), vec![]).is_err());
        assert_eq!(f.snapshot()?, before);
        Ok(())
    }

    /// The same selection posted twice: the second finds its bots gone (`missing`) and makes nothing.
    #[test]
    fn a_second_submit_is_missing() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5]])?;
        f.done(&[])?;
        f.refused_unchanged(&[], "missing", "One of these bots is gone. Pick again.")
    }

    /// The selection is looked up first: a stranger's bot and a deleted one are `missing`; nothing at all is `none`.
    #[test]
    fn a_foreign_or_deleted_bot_is_missing() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5], &[0.5, 0.5]])?;
        f.c.execute("INSERT INTO users (email, encrypted_password, name, created_at, updated_at) VALUES ('x@example.com', 'x', 'X', ?1, ?1)", [NOW])?;
        f.c.execute("UPDATE bots SET user_id=?1 WHERE id=?2", (f.c.last_insert_rowid(), f.bots[1]))?;
        f.refused_unchanged(&[], "missing", "One of these bots is gone.")?;
        f.c.execute("UPDATE bots SET status=3 WHERE id=?1", [f.bots[1]])?;
        f.refused_unchanged(&[], "missing", "One of these bots is gone.")?;
        let before = f.snapshot()?;
        let Performed::Refused(r) = f.split_ids(vec![], &[])? else { return Err("expected none".into()) };
        assert_eq!((r.code, r.reason.as_str()), ("none", "Pick at least one bot."));
        assert_eq!(f.snapshot()?, before);
        Ok(())
    }

    /// Two sources, the second refused (its reversal pending) or failing during its own writes: the first is not split either.
    #[test]
    fn the_second_source_failing_rolls_back_the_first() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5], &[0.5, 0.5]])?;
        f.transient(f.bots[1], deltabadger::engine::reversal::RESERVE_PENDING, json!([9]))?;
        f.refused_unchanged(&[], "unavailable", "S1X0, S1X1 can't be split right now.")?;
        let g = Fixture::baskets(&[&[0.5, 0.5], &[0.5, 0.5]])?;
        g.c.execute_batch(&format!("CREATE TEMP TRIGGER crash BEFORE UPDATE OF status ON bots WHEN NEW.id = {} AND NEW.status = 3 BEGIN SELECT RAISE(ABORT, 'crash'); END;", g.bots[1]))?;
        let before = g.snapshot()?;
        assert!(perform(&g.c, &g.ctx, g.s.user_id, g.bots.clone(), vec![]).is_err());
        assert_eq!(g.snapshot()?, before);
        Ok(())
    }

    /// Several sources split in one go, in request order: their children and positions interleave as Rails creates them.
    #[test]
    fn several_sources_split_in_request_order() -> Result {
        let f = Fixture::baskets(&[&[0.5, 0.5], &[0.6, 0.4]])?;
        let children = f.split_ids(vec![f.bots[1], f.bots[0]], &[])?;
        let Performed::Done(done) = children else { return Err("expected a split".into()) };
        assert_eq!(done.sources, vec![f.bots[1], f.bots[0]]);
        assert_eq!(done.children.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), vec!["S1X0", "S1X1", "S0X0", "S0X1"]);
        let positions: Vec<i64> = done.children.iter().map(|c| f.c.query_row("SELECT position FROM bots WHERE id=?1", [c.id], |r| r.get(0))).collect::<std::result::Result<_, _>>()?;
        assert_eq!(positions, vec![6, 6, 5, 5]);
        Ok(())
    }

    /// D-SPLIT-LEGACY-SHADOW: a legacy row goes by walk::listing_names: case-insensitively, through a tombstoned listing.
    #[test]
    fn a_legacy_row_goes_by_any_spelling_of_one_asset() -> Result {
        let f = Fixture::baskets(&[&[0.6, 0.4]])?;
        let bot = f.bots[0];
        let lower = f.order(bot, None, "s0x1", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.c.execute("UPDATE tickers SET base='__stale_17_SX1' WHERE id=?1", [f.assets[0][1].1])?;
        let tombstoned = f.order(bot, None, "sx1", 0, "REGULAR", "1", "100", "100", "2026-09-02 11:00:00")?;
        let children = f.done(&[])?;
        assert_eq!(f.bot_of(lower)?.0, children[1]);
        assert_eq!(f.bot_of(tombstoned)?.0, children[1]);
        let asset: Option<i64> = f.c.query_row("SELECT base_asset_id FROM transactions WHERE id=?1", [lower], |r| r.get(0))?;
        assert_eq!(asset, None, "the row is moved, not rewritten");
        Ok(())
    }

    /// D-SPLIT-LEGACY-SHADOW: a legacy row whose name is a surviving member's (bound for the second child) and an exited asset's
    /// (bound for the first) is refused, never guessed; one whose every match is bound for the same child moves there.
    #[test]
    fn a_legacy_row_shared_by_two_children_refuses() -> Result {
        let f = Fixture::index(&[0.6, 0.4])?;
        let bot = f.bots[0];
        let (exited, ticker) = f.exited(bot, "S0X9")?;
        f.c.execute("UPDATE tickers SET base='S0X1' WHERE id=?1", [ticker])?;
        f.order(bot, Some(exited), "S0X9", 0, "REGULAR", "1", "100", "100", "2026-09-02 09:00:00")?;
        let shared = f.order(bot, None, "s0x1", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        let lots = f.books(bot)?.walked.map(|w| w.loss_lot_by_transaction);
        f.refused_unchanged(&[], "unavailable", "ND2 can't be split right now.")?;
        assert_eq!(f.books(bot)?.walked.map(|w| w.loss_lot_by_transaction), lots);
        // The alias now names the first member instead: both matches are bound for the first child.
        f.c.execute("UPDATE tickers SET base='S0X0' WHERE id=?1", [ticker])?;
        f.c.execute("UPDATE transactions SET base='s0x0' WHERE id=?1", [shared])?;
        let children = f.done(&[])?;
        assert_eq!(f.bot_of(shared)?.0, children[0]);
        Ok(())
    }

    /// split_test.rb's swap: the sale is money out for its asset's child and the purchase new money for the other's; units and
    /// tax lots per asset are the source's, and so is what the children made together.
    #[test]
    fn a_swap_splits_into_a_sale_and_a_purchase() -> Result {
        let f = Fixture::index(&[0.5, 0.5])?;
        let (bot, a, b) = (f.bots[0], f.assets[0][0].0, f.assets[0][1].0);
        f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.order(bot, Some(a), "S0X0", 1, "REBALANCE", "0.5", "60", "120", "2026-09-02 11:00:00")?;
        f.order(bot, Some(b), "S0X1", 0, "REBALANCE", "1", "60", "60", "2026-09-02 12:00:00")?;
        let before = f.books(bot)?;
        let after: Vec<Metrics> = f.done(&[])?.into_iter().map(|id| f.books(id)).collect::<Result<_>>()?;
        assert_eq!(sum(after.iter().map(profit)), profit(&before));
        assert_eq!(d(&after[1].total_quote_amount_invested), dec("60"), "the swap buy is new money for its child");
        assert_eq!(d(&after[0].rebalance_cash), dec("60"), "the swap sale is money out for its child");
        assert_eq!(holding(&after[0], a).0, dec("0.5"));
        lots_kept(&before, &after, &[a, b]);
        Ok(())
    }

    /// A liquidation and its redeploy split into a sale and a purchase, what the bot made kept.
    #[test]
    fn a_liquidation_and_its_redeploy_split_with_the_books_kept() -> Result {
        let f = Fixture::index(&[0.5, 0.5])?;
        let (bot, a, b) = (f.bots[0], f.assets[0][0].0, f.assets[0][1].0);
        let (exited, _) = f.exited(bot, "S0X9")?;
        f.order(bot, Some(exited), "S0X9", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:01:00")?;
        f.order(bot, Some(exited), "S0X9", 1, "LIQUIDATION", "1", "150", "150", "2026-09-02 11:00:00")?;
        f.order(bot, Some(b), "S0X1", 0, "REDEPLOY", "1", "150", "150", "2026-09-02 12:00:00")?;
        let before = f.books(bot)?;
        let after: Vec<Metrics> = f.done(&[])?.into_iter().map(|id| f.books(id)).collect::<Result<_>>()?;
        assert_eq!(sum(after.iter().map(profit)), profit(&before));
        lots_kept(&before, &after, &[a, b, exited]);
        Ok(())
    }

    /// A swap buy that overspent its sale is new money on its child, where the source never booked it: one less made.
    #[test]
    fn an_overspent_swap_buy_is_new_money() -> Result {
        let f = Fixture::index(&[0.5, 0.5])?;
        let (bot, a, b) = (f.bots[0], f.assets[0][0].0, f.assets[0][1].0);
        f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.order(bot, Some(a), "S0X0", 1, "REBALANCE", "0.5", "60", "120", "2026-09-02 11:00:00")?;
        f.order(bot, Some(b), "S0X1", 0, "REBALANCE", "1", "61", "61", "2026-09-02 12:00:00")?;
        let before = f.books(bot)?;
        let after: Vec<Metrics> = f.done(&[])?.into_iter().map(|id| f.books(id)).collect::<Result<_>>()?;
        assert_eq!(sum(after.iter().map(profit)), (&profit(&before) - &dec("1")).unwrap());
        lots_kept(&before, &after, &[a, b]);
        Ok(())
    }

    /// A child merged later keeps the books it was split with.
    #[test]
    fn a_child_merged_later_keeps_its_books() -> Result {
        use deltabadger::web::bot::merge::{perform as merge, Merged};
        let f = Fixture::index(&[0.5, 0.5])?;
        let (bot, a, b) = (f.bots[0], f.assets[0][0].0, f.assets[0][1].0);
        f.order(bot, Some(a), "S0X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 10:00:00")?;
        f.order(bot, Some(a), "S0X0", 1, "REBALANCE", "0.5", "60", "120", "2026-09-02 11:00:00")?;
        f.order(bot, Some(b), "S0X1", 0, "REBALANCE", "1", "60", "60", "2026-09-02 12:00:00")?;
        let children = f.done(&[])?;
        let (other_asset, other_ticker) = seed::add_alpaca_stock(&f.c, &f.s, "S5X0");
        let mut spec = seed::BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(other_asset, 1.0)]);
        spec.status = 2;
        let other = seed::insert_bot(&f.c, &f.s, &spec);
        seed::insert_member(&f.c, other, other_asset, other_ticker, 1.0, true, "2026-09-01 14:00:00", None);
        f.order(other, Some(other_asset), "S5X0", 0, "REGULAR", "1", "100", "100", "2026-09-02 13:00:00")?;
        let expected = (&profit(&f.books(children[1])?) + &profit(&f.books(other)?)).unwrap();
        let Merged::Done(merged) = merge(&f.c, &f.ctx, f.s.user_id, vec![children[1], other], None).map_err(|e| format!("{e:?}"))? else { return Err("expected a merge".into()) };
        assert_eq!(profit(&f.books(merged)?), expected);
        Ok(())
    }
}
