//! Ordinary browser tests exercise the production handlers; only exchange HTTP and the application logger are captured.
mod common;
#[path="support/s1_legacy.rs"] mod legacy;
#[path = "support/settings_fixture.rs"]
mod fixture;
use common::web::{self, Browser, Csrf};
use deltabadger::web::{App, Config};
use std::sync::{Arc, Mutex};
use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};
#[derive(Default)]
struct Logs(Mutex<Vec<String>>);
impl deltabadger::web::settings::keys::Logger for Logs {
    fn warn(&self, line: &str) {
        self.0.lock().unwrap().push(line.into());
    }
}
#[tokio::test(flavor = "current_thread")]
async fn credential_value_is_absent_from_application_logs_after_a_real_failed_save() {
    let (dir, opened, seed) = fixture::install();
    let server = MockServer::start().await;
    let placeholder = "placeholder-value-123";
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"message":format!("validation service unavailable for {placeholder}")}))).expect(1).mount(&server).await;
    let logs = Arc::new(Logs::default());
    let env = web::env(web::SECRET);
    let app = App::new(
        Config::from_env(&env).unwrap(),
        &env,
        opened.primary,
        web::TestClock::at("2026-09-10T12:00:30.123456Z"),
    )
    .unwrap()
    .with_figure_source(deltabadger::web::figure::loading::Source::Disabled)
    .unwrap()
    .with_settings_key_boundary(server.uri(), logs.clone())
    .unwrap();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    let id: i64 = c
        .query_row(
            "SELECT id FROM api_keys WHERE user_id=?1",
            [seed.user_id],
            |r| r.get(0),
        )
        .unwrap();
    c.execute(
        "UPDATE api_keys SET key=?1,secret=?2,passphrase=?3 WHERE id=?4",
        (
            app.cipher.encrypt("placeholder-key"),
            app.cipher.encrypt(placeholder),
            app.cipher.encrypt("paper"),
            id,
        ),
    )
    .unwrap();
    let before: Vec<String> = c
        .query_row(
            "SELECT key,secret,passphrase FROM api_keys WHERE id=?1",
            [id],
            |r| Ok(vec![r.get(0)?, r.get(1)?, r.get(2)?]),
        )
        .unwrap();
    c.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00',setup_completed=1 WHERE id=?2",(deltabadger::crypto::hash_password("Correct-horse-9").unwrap(),seed.user_id)).unwrap();
    let mut browser = Browser::default();
    browser
        .send(&app, "GET", "/login", None, Csrf::None, &[])
        .await;
    assert_eq!(
        browser
            .send(
                &app,
                "POST",
                "/login",
                Some(&[
                    ("user[email]", "o@example.com"),
                    ("user[password]", "Correct-horse-9")
                ]),
                Csrf::Form,
                &[]
            )
            .await
            .status,
        303
    );
    assert_eq!(
        browser
            .send(&app, "GET", "/settings/account", None, Csrf::None, &[])
            .await
            .status,
        200
    );
    let response = browser
        .send(
            &app,
            "POST",
            "/tracker/add_api_key",
            Some(&[
                ("exchange_id", &seed.exchange_id.to_string()),
                ("key_type", "trading"),
                ("api_key[key]", "placeholder-key"),
                ("api_key[secret]", placeholder),
                ("api_key[passphrase]", "paper"),
            ]),
            Csrf::Header,
            &[
                ("accept", deltabadger::web::turbo::CONTENT_TYPE),
                ("origin", "http://localhost:3000"),
            ],
        )
        .await;
    assert_eq!(response.status, 422);
    assert!(!response.body.contains(placeholder));
    let after: Vec<String> = c
        .query_row(
            "SELECT key,secret,passphrase FROM api_keys WHERE id=?1",
            [id],
            |r| Ok(vec![r.get(0)?, r.get(1)?, r.get(2)?]),
        )
        .unwrap();
    assert_eq!(before, after);
    let log = logs.0.lock().unwrap().join("\n");
    assert!(
        log.contains("API key validation failed:"),
        "a connected nonempty logger is required"
    );
    assert!(!log.contains(placeholder), "credential must not enter logs");
    server.verify().await;
}
#[test]
fn production_source_contains_no_live_alpaca_host() {
    fn walk(root: &std::path::Path) {
        for entry in std::fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path)
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                let host = ["https://", "api.alpaca.markets"].concat();
                assert!(
                    !text.contains(&host),
                    "{} includes the live host",
                    path.display()
                );
            }
        }
    }
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"));
}

#[derive(Default)]
struct CapturedMail(Mutex<Vec<deltabadger::mail::Message>>);
impl deltabadger::web::settings::mail::Mailer for CapturedMail {
    fn deliver(&self,message:deltabadger::mail::Message,_settings:Option<deltabadger::mail::smtp::Settings>,now:chrono::DateTime<chrono::Utc>)->deltabadger::web::settings::mail::Delivery {
        message.encode(now,"fixture@deltabadger").unwrap();
        self.0.lock().unwrap().push(message);
        Box::pin(async {Ok(())})
    }
}
struct Harness {
    _dir: tempfile::TempDir,
    app: App,
    c: rusqlite::Connection,
    seed: common::seed::Seeded,
    browser: Browser,
    clock: Arc<web::TestClock>,
    mail: Arc<CapturedMail>,
    logs: Arc<Logs>,
}
impl Harness {
    async fn new(url: String) -> Self {Self::with_hook(url,None).await}
    /// Signed in at the real time, for tests whose venue sends check a deadline against the real clock. Moving a
    /// fixed-date harness to the real time instead would outlive its 30-day session and turn every request into a 302.
    async fn at_real_now(url: String) -> Self {Self::starting(url,None,web::TestClock::starting(chrono::Utc::now())).await}
    async fn with_hook(url:String, hook:Option<deltabadger::web::PasswordHook>)->Self{
        Self::starting(url,hook,web::TestClock::at("2026-09-10T12:00:30.123456Z")).await
    }
    async fn starting(url:String, hook:Option<deltabadger::web::PasswordHook>, clock:Arc<web::TestClock>)->Self{
        let (dir, opened, seed) = fixture::install();
        let env = web::env(web::SECRET);
        let mail=Arc::new(CapturedMail::default());
        let logs=Arc::new(Logs::default());
        let app = App::new(
            Config::from_env(&env).unwrap(),
            &env,
            opened.primary,
            clock.clone(),
        )
        .unwrap()
        .with_figure_source(deltabadger::web::figure::loading::Source::Disabled)
        .unwrap()
        .with_settings_mailer(mail.clone()).unwrap()
        .with_settings_key_boundary(url, logs.clone())
        .unwrap();
        let app=if let Some(hook)=hook{app.with_password_hook(hook).unwrap()}else{app};
        app.attach_jobs(Default::default()).unwrap();
        let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
        c.execute("UPDATE users SET encrypted_password=?1,confirmed_at='2026-01-01 00:00:00',setup_completed=1,wash_sale_enabled=0 WHERE id=?2",(deltabadger::crypto::hash_password("Correct-horse-9").unwrap(),seed.user_id)).unwrap();
        c.execute(
            "UPDATE api_keys SET key=?1,secret=?2,passphrase=?3 WHERE id=?4",
            (
                app.cipher.encrypt("previous-key"),
                app.cipher.encrypt("previous-secret"),
                app.cipher.encrypt("paper"),
                seed.api_key_id,
            ),
        )
        .unwrap();
        let mut browser = Browser::default();
        browser
            .send(&app, "GET", "/login", None, Csrf::None, &[])
            .await;
        assert_eq!(
            browser
                .send(
                    &app,
                    "POST",
                    "/login",
                    Some(&[
                        ("user[email]", "o@example.com"),
                        ("user[password]", "Correct-horse-9")
                    ]),
                    Csrf::Form,
                    &[]
                )
                .await
                .status,
            303
        );
        assert_eq!(
            browser
                .send(&app, "GET", "/settings/account", None, Csrf::None, &[])
                .await
                .status,
            200
        );
        Self {
            _dir: dir,
            app,
            c,
            seed,
            browser,
            clock,
            mail,
            logs,
        }
    }
    async fn submit(
        &mut self,
        method: &str,
        path: &str,
        form: &[(&str, &str)],
        csrf: Csrf,
    ) -> common::web::Answer {
        self.browser
            .send(
                &self.app,
                method,
                path,
                Some(form),
                csrf,
                &[
                    ("accept", deltabadger::web::turbo::CONTENT_TYPE),
                    ("origin", "http://localhost:3000"),
                ],
            )
            .await
    }
    fn snapshot(&self) -> serde_json::Value {
        let mut tables = serde_json::Map::new();
        for table in [
            "users",
            "api_keys",
            "bots",
            "transactions",
            "bot_activity_logs",
            "account_transactions",
            "oauth_applications",
            "oauth_access_tokens",
            "oauth_access_grants",
            "connected_clients",
            "wash_sale_locks",
        ] {
            let mut s = self
                .c
                .prepare(&format!("SELECT * FROM {table} ORDER BY id"))
                .unwrap();
            let names = s
                .column_names()
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>();
            let rows = s
                .query_map([], |r| {
                    Ok(names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| {
                            let value = match r.get_ref(i).unwrap() {
                                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                                rusqlite::types::ValueRef::Integer(n) => serde_json::json!(n),
                                rusqlite::types::ValueRef::Real(n) => serde_json::json!(n),
                                rusqlite::types::ValueRef::Text(s) => {
                                    serde_json::json!(std::str::from_utf8(s).unwrap())
                                }
                                _ => panic!("unexpected blob"),
                            };
                            (name.clone(), value)
                        })
                        .collect::<serde_json::Map<_, _>>())
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            tables.insert(table.into(), serde_json::json!(rows));
        }
        tables.into()
    }
    fn link(&self) {
        self.c.execute("INSERT INTO account_transactions(api_key_id,base_amount,base_currency,entry_type,exchange_id,user_id,transacted_at,created_at,updated_at)VALUES(?1,0.25,'BTC',0,?2,?3,'2026-09-01 00:00:00','2026-09-01 00:00:00','2026-09-01 00:00:00')",(self.seed.api_key_id,self.seed.exchange_id,self.seed.user_id)).unwrap();
    }
}
#[tokio::test(flavor = "current_thread")]
async fn every_settings_write_rejects_missing_csrf_and_preserves_all_rows() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    h.c.execute("INSERT INTO oauth_applications(name,uid,secret,redirect_uri,scopes,confidential,created_at,updated_at)VALUES('CSRF client','csrf-client','','http://localhost/callback','mcp api',0,'2026-01-01','2026-01-01')",[]).unwrap();
    h.c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,refresh_token,scopes,created_at)VALUES(1,?1,'csrf-client-token','csrf-refresh','mcp','2026-01-01')",[h.seed.user_id]).unwrap();
    common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
    h.c.execute("UPDATE users SET otp_secret_key=?1,unconfirmed_email='csrf-pending@example.com',confirmation_token='abcdefghijklmnopqrst' WHERE id=?2",(h.app.cipher.encrypt("JBSWY3DPEHPK3PXP"),h.seed.user_id)).unwrap();
    let code=deltabadger::crypto::totp_at("JBSWY3DPEHPK3PXP",h.app.now().timestamp() as u64).unwrap();
    let mut writes = vec![
        (
            "PATCH",
            "/settings/update_name",
            vec![("user[name]", "Changed")],
        ),
        (
            "PATCH",
            "/settings/update_email",
            vec![
                ("user[email]", "changed@example.com"),
                ("user[current_password]", "Correct-horse-9"),
            ],
        ),
        (
            "PATCH",
            "/settings/update_password",
            vec![
                ("user[password]", "Another-horse-7"),
                ("user[current_password]", "Correct-horse-9"),
            ],
        ),
        (
            "PATCH",
            "/settings/update_time_zone",
            vec![("user[time_zone]", "Warsaw")],
        ),
        (
            "PATCH",
            "/settings/update_locale",
            vec![("user[locale]", "de")],
        ),
        (
            "PATCH",
            "/settings/update_two_fa",
            vec![("user[otp_code_token]", code.as_str())],
        ),
        (
            "PATCH",
            "/settings/update_wash_sale",
            vec![("wash_sale[enabled]", "0"),("wash_sale[jurisdiction]","GB")],
        ),
        ("DELETE", "/settings/destroy_api_key/1", vec![]),
        (
            "PATCH",
            "/settings/update_client_tool_permissions/1",
            vec![("surface", "mcp"), ("group", "read"), ("enabled", "1")],
        ),
        ("DELETE", "/settings/revoke_mcp_client/1", vec![]),
        (
            "POST",
            "/tracker/add_api_key",
            vec![
                ("exchange_id", "1"),
                ("key_type", "trading"),
                ("api_key[key]", "new-key"),
                ("api_key[secret]", "new-secret"),
                ("api_key[passphrase]", "paper"),
            ],
        ),
    ];
    writes.extend([
        ("POST","/bots/1/add_api_key",vec![("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")]),
        ("POST","/api/api_keys",vec![("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")]),
        ("POST","/confirmation",vec![("user[email]","o@example.com")]),
        ("PATCH","/settings/update_mcp_tool_permissions",vec![("tool_name","list_bots"),("enabled","0")]),
        ("PATCH","/settings/update_rest_tool_permissions",vec![("tool_name","list_bots"),("enabled","0")]),
        ("PATCH","/settings/update_mcp_tool_group_permissions",vec![("group","read"),("enabled","0")]),
        ("PATCH","/settings/update_rest_tool_group_permissions",vec![("group","read"),("enabled","0")]),
        ("PATCH","/settings/update_mcp_dry_run",vec![("enabled","1")]),
    ]);

    for (method, path, form) in writes {
        assert_eq!(h.browser.send(&h.app,"GET","/settings/account",None,Csrf::None,&[]).await.status,200,"every case starts signed in");
        let before = h.snapshot();
        let answer = h.submit(method, path, &form, Csrf::None).await;
        assert_eq!(answer.status,302,"CSRF must refuse {method} {path}");
        assert_eq!(h.snapshot(),before,"CSRF must preserve rows for {method} {path}");
        assert_eq!(h.browser.send(&h.app,"GET","/settings/account",None,Csrf::None,&[]).await.status,200);
        let before=h.snapshot();let origin=h.browser.send(&h.app,method,path,Some(&form),Csrf::Header,&[("Origin","https://foreign.example")]).await;
        assert_eq!(origin.status,302,"foreign Origin must refuse {method} {path}");assert_eq!(h.snapshot(),before,"foreign Origin preserves {path}");
    }
    assert!(h.mail.0.lock().unwrap().is_empty(),"CSRF refusal must not send mail");
    assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor = "current_thread")]
async fn missing_otp_seed_get_is_idempotent_foreign_get_enables_nothing_and_patch_needs_csrf() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    h.c.execute(
        "UPDATE users SET otp_secret_key=NULL,otp_module=0 WHERE id=?1",
        [h.seed.user_id],
    )
    .unwrap();
    let answer = h
        .browser
        .send(
            &h.app,
            "GET",
            "/settings/edit_two_fa",
            None,
            Csrf::None,
            &[("origin", "http://foreign.example")],
        )
        .await;
    assert_eq!(answer.status, 200);
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    let seed: String =
        h.c.query_row(
            "SELECT otp_secret_key FROM users WHERE id=?1",
            [h.seed.user_id],
            |r| r.get(0),
        )
        .unwrap();
    let plain = h.app.cipher.decrypt(&seed).unwrap();
    assert_ne!(seed, plain);
    assert_eq!(plain.len(), 32);
    assert_eq!(
        h.browser
            .send(
                &h.app,
                "GET",
                "/settings/edit_two_fa",
                None,
                Csrf::None,
                &[]
            )
            .await
            .status,
        200
    );
    let next: String =
        h.c.query_row(
            "SELECT otp_secret_key FROM users WHERE id=?1",
            [h.seed.user_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(next, seed);
    let enabled: i64 =
        h.c.query_row(
            "SELECT otp_module FROM users WHERE id=?1",
            [h.seed.user_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(enabled, 0);
    let code = deltabadger::crypto::totp_at(
        &plain,
        web::at("2026-09-10T12:00:30.123456Z")
            .timestamp()
            .try_into()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        h.submit(
            "PATCH",
            "/settings/update_two_fa",
            &[("user[otp_code_token]", &code)],
            Csrf::None
        )
        .await
        .status,
        302
    );
    assert_eq!(
        h.submit(
            "PATCH",
            "/settings/update_two_fa",
            &[("user[otp_code_token]", "000000")],
            Csrf::Header
        )
        .await
        .status,
        422
    );
    assert_eq!(
        h.submit(
            "PATCH",
            "/settings/update_two_fa",
            &[("user[otp_code_token]", &code)],
            Csrf::Header
        )
        .await
        .status,
        200
    );
    h.clock.set(web::at("2026-09-10T12:01:00.123456Z"));
    let code = deltabadger::crypto::totp_at(
        &plain,
        web::at("2026-09-10T12:01:00.123456Z")
            .timestamp()
            .try_into()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        h.submit(
            "PATCH",
            "/settings/update_two_fa",
            &[("user[otp_code_token]", &code)],
            Csrf::Header
        )
        .await
        .status,
        200
    );
    let (enabled, retained): (i64, String) =
        h.c.query_row(
            "SELECT otp_module,otp_secret_key FROM users WHERE id=?1",
            [h.seed.user_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(enabled, 0);
    assert_eq!(retained, seed);
}
#[tokio::test(flavor = "current_thread")]
async fn live_submitted_or_inherited_mode_is_refused_before_any_network_request() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    let before = h.snapshot();
    let response = h
        .submit(
            "POST",
            "/tracker/add_api_key",
            &[
                ("exchange_id", "1"),
                ("key_type", "trading"),
                ("api_key[key]", "new-key"),
                ("api_key[secret]", "new-secret"),
                ("api_key[passphrase]", "live"),
            ],
            Csrf::Header,
        )
        .await;
    assert!(server.received_requests().await.unwrap().is_empty(),"E refuses live submissions before any HTTP request");
    assert_eq!(response.status, 422);
    assert!(response
        .body
        .contains("This build accepts paper keys only."));
    assert_eq!(h.snapshot(), before);
    h.c.execute(
        "UPDATE api_keys SET passphrase=?1 WHERE id=?2",
        (h.app.cipher.encrypt("live"), h.seed.api_key_id),
    )
    .unwrap();
    let before = h.snapshot();
    assert_eq!(
        h.submit(
            "POST",
            "/tracker/add_api_key",
            &[
                ("exchange_id", "1"),
                ("key_type", "trading"),
                ("api_key[key]", "new-key")
            ],
            Csrf::Header
        )
        .await
        .status,
        422
    );
    assert_eq!(h.snapshot(), before);
    assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor = "current_thread")]
async fn rejected_replacement_preserves_encrypted_credential_and_ledger_links() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/account"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;
    let mut h = Harness::new(server.uri()).await;
    h.link();
    let before = h.snapshot();
    let response = h
        .submit(
            "POST",
            "/tracker/add_api_key",
            &[
                ("exchange_id", "1"),
                ("key_type", "trading"),
                ("api_key[key]", "rejected-key"),
                ("api_key[secret]", "rejected-secret"),
                ("api_key[passphrase]", "paper"),
            ],
            Csrf::Header,
        )
        .await;
    assert_eq!(response.status, 422);
    assert_eq!(h.snapshot(), before);
    assert!(!response.body.contains("rejected-secret"));
    server.verify().await;
}
#[tokio::test(flavor = "current_thread")]
async fn tax_guard_rolls_back_unsupported_protection_and_wakes_only_after_commit() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    common::seed::insert_bot(
        &h.c,
        &h.seed,
        &common::seed::BotSpec::weekly(60.0, "2026-09-01 00:00:00"),
    );
    let wake = Arc::new(tokio::sync::Notify::new());
    h.app.attach_engine(wake.clone());
    let before = h.snapshot();
    let response = h
        .submit(
            "PATCH",
            "/settings/update_wash_sale",
            &[
                ("wash_sale[enabled]", "1"),
                ("wash_sale[jurisdiction]", "GB"),
            ],
            Csrf::Header,
        )
        .await;
    assert_eq!(response.status, 422);
    assert_eq!(h.snapshot(), before);
    assert!(
        tokio::time::timeout(std::time::Duration::ZERO, wake.notified())
            .await
            .is_err()
    );
    h.c.execute("UPDATE bots SET status=2", []).unwrap();
    assert_eq!(
        h.submit(
            "PATCH",
            "/settings/update_wash_sale",
            &[
                ("wash_sale[enabled]", "1"),
                ("wash_sale[jurisdiction]", "GB")
            ],
            Csrf::Header
        )
        .await
        .status,
        303
    );
    assert!(
        tokio::time::timeout(std::time::Duration::ZERO, wake.notified())
            .await
            .is_ok()
    );
    let jurisdiction: String =
        h.c.query_row("SELECT wash_sale_jurisdiction FROM users", [], |r| r.get(0))
            .unwrap();
    assert_eq!(jurisdiction, "GB");
    assert_eq!(
        h.submit(
            "PATCH",
            "/settings/update_wash_sale",
            &[("wash_sale[enabled]", "0")],
            Csrf::Header
        )
        .await
        .status,
        303
    );
    let jurisdiction: String =
        h.c.query_row("SELECT wash_sale_jurisdiction FROM users", [], |r| r.get(0))
            .unwrap();
    assert_eq!(jurisdiction, "GB");
}
#[tokio::test(flavor = "current_thread")]
async fn account_writes_validate_current_password_name_and_preference_values() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    for (path, form) in [
        (
            "/settings/update_email",
            vec![
                ("user[email]", "next@example.com"),
                ("user[current_password]", "wrong"),
            ],
        ),
        (
            "/settings/update_password",
            vec![
                ("user[password]", "Another-horse-7"),
                ("user[password_confirmation]", "Another-horse-7"),
                ("user[current_password]", "wrong"),
            ],
        ),
        ("/settings/update_name", vec![("user[name]", "123")]),
        (
            "/settings/update_time_zone",
            vec![("user[time_zone]", "invalid")],
        ),
        ("/settings/update_locale", vec![("user[locale]", "invalid")]),
    ] {
        let before = h.snapshot();
        assert_eq!(
            h.submit("PATCH", path, &form, Csrf::Header).await.status,
            422,
            "{path}"
        );
        assert_eq!(h.snapshot(), before, "{path}");
    }
    assert!(h.logs.0.lock().unwrap().is_empty(),"invalid account writes produce no credential diagnostic");
    assert!(h.mail.0.lock().unwrap().is_empty(),"invalid account writes deliver no confirmation mail");
    assert!(server.received_requests().await.unwrap().is_empty(),"account validation never calls a venue");
}

#[tokio::test(flavor = "current_thread")]
async fn credential_save_and_delete_guard_the_result_and_preserve_rows_on_refusal() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/account"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut h = Harness::new(server.uri()).await;
    h.link();
    common::seed::insert_bot(
        &h.c,
        &h.seed,
        &common::seed::BotSpec::weekly(60.0, "2026-09-01 00:00:00"),
    );
    let other = common::seed::insert_bot(
        &h.c,
        &h.seed,
        &common::seed::BotSpec::weekly(60.0, "2026-09-01 00:00:00"),
    );
    h.c.execute("UPDATE bots SET exchange_id=NULL WHERE id=?1", [other])
        .unwrap();
    let wake = Arc::new(tokio::sync::Notify::new());
    h.app.attach_engine(wake.clone());
    let before = h.snapshot();
    assert_eq!(
        h.submit(
            "POST",
            "/tracker/add_api_key",
            &[
                ("exchange_id", "1"),
                ("key_type", "trading"),
                ("api_key[key]", "new-key"),
                ("api_key[secret]", "new-secret"),
                ("api_key[passphrase]", "paper")
            ],
            Csrf::Header
        )
        .await
        .status,
        422
    );
    assert_eq!(h.snapshot(), before);
    assert!(
        tokio::time::timeout(std::time::Duration::ZERO, wake.notified())
            .await
            .is_err()
    );
    // A read-only key's deletion leaves both working bots in the guarded state.
    h.c.execute(
        "UPDATE api_keys SET key_type=2 WHERE id=?1",
        [h.seed.api_key_id],
    )
    .unwrap();
    let before = h.snapshot();
    assert_eq!(
        h.submit("DELETE", "/settings/destroy_api_key/1", &[], Csrf::Header)
            .await
            .status,
        422
    );
    assert_eq!(h.snapshot(), before);
    assert!(
        tokio::time::timeout(std::time::Duration::ZERO, wake.notified())
            .await
            .is_err()
    );
    h.c.execute("UPDATE bots SET status=2", []).unwrap();
    assert_eq!(
        h.submit("DELETE", "/settings/destroy_api_key/1", &[], Csrf::Header)
            .await
            .status,
        200
    );
    assert!(
        tokio::time::timeout(std::time::Duration::ZERO, wake.notified())
            .await
            .is_ok()
    );
    let link: Option<i64> =
        h.c.query_row("SELECT api_key_id FROM account_transactions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(link.is_none());
    server.verify().await;
}
#[tokio::test(flavor = "current_thread")]
async fn connected_client_grants_intersect_owner_permissions_and_revoke_preserves_other_users() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    h.c.execute("INSERT INTO oauth_applications(name,uid,secret,redirect_uri,scopes,confidential,created_at,updated_at)VALUES('Shared','settings-client','','http://localhost/callback','mcp api',0,'2026-01-01 00:00:00','2026-01-01 00:00:00')",[]).unwrap();
    let id = h.c.last_insert_rowid();
    h.c.execute("INSERT INTO users(email,encrypted_password,name,created_at,updated_at)VALUES('other@example.com','x','Other','2026-01-01 00:00:00','2026-01-01 00:00:00')",[]).unwrap();
    let other = h.c.last_insert_rowid();
    for user in [h.seed.user_id, other] {
        h.c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,refresh_token,scopes,expires_in,created_at)VALUES(?1,?2,?3,?4,'mcp',1,'2026-01-01 00:00:00')",(id,user,format!("token-{user}"),format!("refresh-{user}"))).unwrap();
    }
    let route = format!("/settings/update_client_tool_permissions/{id}");
    assert_eq!(
        h.submit(
            "PATCH",
            &route,
            &[("surface", "mcp"), ("group", "read"), ("enabled", "1")],
            Csrf::Header
        )
        .await
        .status,
        200
    );
    let granted = || {
        h.c.query_row(
            "SELECT mcp_tools FROM connected_clients WHERE user_id=?1",
            [h.seed.user_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    let original = granted();
    assert!(original.contains("list_bots"));
    assert_eq!(
        h.submit(
            "PATCH",
            &route,
            &[("surface", "mcp"), ("group", "trade"), ("enabled", "1")],
            Csrf::Header
        )
        .await
        .status,
        200
    );
    let current: String =
        h.c.query_row(
            "SELECT mcp_tools FROM connected_clients WHERE user_id=?1",
            [h.seed.user_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(current, original, "disabled owner tools cannot be granted");
    assert_eq!(
        h.submit(
            "PATCH",
            "/settings/update_mcp_tool_group_permissions",
            &[("group", "trade"), ("enabled", "1")],
            Csrf::Header
        )
        .await
        .status,
        200
    );
    assert_eq!(
        h.submit(
            "PATCH",
            &route,
            &[("surface", "mcp"), ("group", "trade"), ("enabled", "1")],
            Csrf::Header
        )
        .await
        .status,
        200
    );
    let current: String =
        h.c.query_row(
            "SELECT mcp_tools FROM connected_clients WHERE user_id=?1",
            [h.seed.user_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(current.contains("market_buy"));
    let before = h.snapshot();
    assert_eq!(
        h.submit(
            "PATCH",
            &route,
            &[("surface", "mcp"), ("group", "invented"), ("enabled", "1")],
            Csrf::Header
        )
        .await
        .status,
        422
    );
    assert_eq!(h.snapshot(), before);
    assert_eq!(
        h.submit(
            "DELETE",
            &format!("/settings/revoke_mcp_client/{id}"),
            &[],
            Csrf::Header
        )
        .await
        .status,
        200
    );
    let other_live: bool =
        h.c.query_row(
            "SELECT revoked_at IS NULL FROM oauth_access_tokens WHERE resource_owner_id=?1",
            [other],
            |r| r.get(0),
        )
        .unwrap();
    assert!(other_live);
    let owner_live: bool =
        h.c.query_row(
            "SELECT revoked_at IS NULL FROM oauth_access_tokens WHERE resource_owner_id=?1",
            [h.seed.user_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!owner_live);
    assert_eq!(
        h.submit(
            "PATCH",
            &route,
            &[("surface", "mcp"), ("group", "read"), ("enabled", "1")],
            Csrf::Header
        )
        .await
        .status,
        404
    );
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_rejected_hyperliquid_replacement_preserves_the_credential_and_ledger_links() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    h.link();
    let old_key = "0x1111111111111111111111111111111111111111";
    let old_secret = "2222222222222222222222222222222222222222222222222222222222222222";
    h.c.execute(
        "UPDATE exchanges SET type='Exchanges::Hyperliquid',name='Hyperliquid' WHERE id=?1",
        [h.seed.exchange_id],
    )
    .unwrap();
    h.c.execute(
        "UPDATE api_keys SET key=?1,secret=?2,passphrase=NULL WHERE id=?3",
        (
            h.app.cipher.encrypt(old_key),
            h.app.cipher.encrypt(old_secret),
            h.seed.api_key_id,
        ),
    )
    .unwrap();
    let before = h.snapshot();
    let response = h
        .submit(
            "POST",
            "/api/api_keys",
            &[
                ("api_key[exchange_id]", "1"),
                ("api_key[key_type]", "trading"),
                ("api_key[key]", "invalid-wallet"),
                ("api_key[secret]", "invalid-agent"),
            ],
            Csrf::Header,
        )
        .await;
    assert_eq!(response.status, 422);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response.body).unwrap(),
        serde_json::json!({"data":false})
    );
    assert_eq!(h.snapshot(), before);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn confirmation_is_single_use_wrong_and_superseded_tokens_preserve_rows_and_old_tokens_work() {
    let server = MockServer::start().await;
    let mut h = Harness::new(server.uri()).await;
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]","next@example.com"),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let old:String=h.c.query_row("SELECT confirmation_token FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();
    assert_eq!(old.len(),20);
    assert_eq!(h.mail.0.lock().unwrap().len(),1);
    assert!(h.mail.0.lock().unwrap()[0].html.contains(&old));
    assert_eq!(h.mail.0.lock().unwrap()[0].to,"next@example.com");
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]","later@example.com"),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let current:String=h.c.query_row("SELECT confirmation_token FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();
    assert_ne!(old,current);
    assert_eq!(h.mail.0.lock().unwrap().len(),2);
    assert!(h.mail.0.lock().unwrap()[1].html.contains(&current));
    assert_eq!(h.mail.0.lock().unwrap()[1].to,"later@example.com");
    h.c.execute("UPDATE users SET confirmation_sent_at='1926-01-01 00:00:00' WHERE id=?1",[h.seed.user_id]).unwrap();
    let before = h.snapshot();
    for token in [old.as_str(),"wrong-placeholder-token"] {
        let r=h.browser.send(&h.app,"GET",&format!("/confirmation?confirmation_token={token}"),None,Csrf::None,&[("origin","http://foreign.example")]).await;
        assert_eq!(r.status,200);
        assert!(r.body.contains("name=\"user[email]\""));
        assert_eq!(h.snapshot(),before);
        assert!(!r.body.contains(&current));
    }
    let r=h.browser.send(&h.app,"GET",&format!("/confirmation?confirmation_token={current}"),None,Csrf::None,&[("origin","http://foreign.example")]).await;
    assert_eq!(r.status,302);
    let (email,pending):(String,Option<String>)=h.c.query_row("SELECT email,unconfirmed_email FROM users WHERE id=?1",[h.seed.user_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(email,"later@example.com");assert_eq!(pending,None);
    let after=h.snapshot();
    let r=h.browser.send(&h.app,"GET",&format!("/confirmation?confirmation_token={current}"),None,Csrf::None,&[]).await;
    assert_eq!(r.status,200);assert_eq!(h.snapshot(),after);
    assert!(!r.body.contains(&current),"stored token must be redacted from language URLs");
}

// The HTTP boundary models two distinct paper accounts, never a real venue.
// Account A accepted the order; recovery with B cannot see it. Keep the engine
// and eligibility guard unchanged while fencing the credential write race.
#[tokio::test(flavor="current_thread")]
async fn account_rotation_must_not_erase_an_unresolved_accepted_order() {
    use deltabadger::{engine::{amount::{self,Sizing},model,placement::{self,Recovery,Sent},FixedClock},ruby::BigDec,venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}}};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    let mut h=Harness::at_real_now(server.uri()).await;
    let at=h.app.now();
    let bot_id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at-chrono::Duration::seconds(1))));
    let bot=model::load_bot(&h.c,bot_id).unwrap();
    let ticker=model::ticker_for(&h.c,&bot).unwrap().unwrap();
    let Sizing::Place(plan)=amount::size(&bot,&ticker,&BigDec::from_i64(60),&BigDec::from_i64(50_000),deltabadger::engine::venue_rules::ALPACA.minimum_logic).unwrap() else {panic!("fixture must size a real order")};
    let intent=placement::begin(&h.c,&bot,&plan,&FixedClock(at)).unwrap();
    Mock::given(method("POST")).and(path("/v2/orders")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"account-a-accepted-order"}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/orders:by_client_order_id")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({"code":40410000,"message":"order not found"}))).expect(0).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/orders:by_client_order_id")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"account-a-accepted-order","client_order_id":intent.cl_ord_id,"status":"filled","qty":"0.0012","filled_qty":"0.0012","filled_avg_price":"50000","type":"market","side":"buy","symbol":"BTC/USD"}))).expect(1).mount(&server).await;
    let urls=Urls{trading:server.uri(),data:server.uri()};
    let original=AlpacaVenue::new(ReqwestTransport::new(http::client(),"previous-key".into(),"previous-secret".into()),urls.clone());
    assert!(matches!(placement::send(&original,&intent,&FixedClock(at)).await,Sent::Accepted(ref id) if id=="account-a-accepted-order"));
    // Crash after acceptance: there is an intent, but no recorded order row.
    assert!(model::load_bot(&h.c,bot_id).unwrap().rust_placement().is_some());
    h.link();
    let before=h.snapshot();
    let wake=Arc::new(tokio::sync::Notify::new());h.app.attach_engine(wake.clone());
    let response=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(response.status,422,"K refuses rotation while the accepted order is unresolved");
    assert!(response.body.contains("An order is still being settled. Please try again shortly."));
    assert_eq!(h.snapshot(),before,"credential, intent and ledger links survive refusal");
    assert!(tokio::time::timeout(std::time::Duration::ZERO,wake.notified()).await.is_err());
    assert!(deltabadger::engine::eligibility::guard(&h.c,&h.app.cipher,None).is_ok(),"guard stays unchanged");
    let current=model::load_bot(&h.c,bot_id).unwrap();
    let credentials=model::credentials_for(&h.c,&h.app.cipher,&current).unwrap().unwrap();
    assert_eq!(credentials.key,"previous-key");
    let recovery=AlpacaVenue::new(ReqwestTransport::new(http::client(),credentials.key,credentials.secret),urls);
    let recovered=placement::recover(&h.c,&recovery,&current,&FixedClock(at+chrono::Duration::hours(1))).await.unwrap();
    assert!(matches!(recovered,Recovery::Recorded(_)),"account A's accepted fill remains recoverable");
    let (rows,quote):(i64,f64)=h.c.query_row("SELECT count(*),sum(quote_amount_exec) FROM transactions WHERE bot_id=?1",[bot_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(rows,1);assert_eq!(quote,60.0);
    assert!(model::load_bot(&h.c,bot_id).unwrap().rust_placement().is_none());
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn credential_replacement_without_unresolved_orders_succeeds() {
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;h.link();
    let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
    let wake=Arc::new(tokio::sync::Notify::new());h.app.attach_engine(wake.clone());
    let r=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(r.status,200);
    let current=deltabadger::engine::model::load_bot(&h.c,bot).unwrap();
    assert_eq!(deltabadger::engine::model::credentials_for(&h.c,&h.app.cipher,&current).unwrap().unwrap().key,"account-b-key");
    assert!(tokio::time::timeout(std::time::Duration::ZERO,wake.notified()).await.is_ok());
    let link:i64=h.c.query_row("SELECT api_key_id FROM account_transactions",[],|r|r.get(0)).unwrap();assert_eq!(link,h.seed.api_key_id);
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn credential_fence_covers_every_writer_intents_and_tracked_open_orders() {
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;h.link();
    let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
    let wake=Arc::new(tokio::sync::Notify::new());h.app.attach_engine(wake.clone());
    // Stopping a bot must not unlock credentials that still recover an order.
    h.c.execute("UPDATE bots SET status=2 WHERE id=?1",[bot]).unwrap();
    for unresolved in ["intent","pending","open"] {
        h.c.execute("DELETE FROM transactions",[]).unwrap();
        h.c.execute("UPDATE bots SET transient_data='{}' WHERE id=?1",[bot]).unwrap();
        match unresolved {
            "intent"=>{h.c.execute("UPDATE bots SET transient_data='{\"rust_placement\":{}}' WHERE id=?1",[bot]).unwrap();},
            _=>{h.c.execute("INSERT INTO transactions(bot_id,exchange_id,external_id,status,external_status,created_at,updated_at)VALUES(?1,?2,'order-a',0,?3,'2026-09-10 00:00:00','2026-09-10 00:00:00')",(bot,h.seed.exchange_id,if unresolved=="pending"{0}else{1})).unwrap();}
        }
        for writer in ["modern","legacy","delete"] {
            let before=h.snapshot();
            let r=match writer {
                "modern"=>h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")],Csrf::Header).await,
                "legacy"=>h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")],Csrf::Header).await,
                _=>h.submit("DELETE","/settings/destroy_api_key/1",&[],Csrf::Header).await,
            };
            assert_eq!(r.status,422,"{writer} with {unresolved}");
            assert!(r.body.contains("An order is still being settled. Please try again shortly."),"{writer} with {unresolved}");
            assert_eq!(h.snapshot(),before,"{writer} must preserve rows with {unresolved}");
            assert!(tokio::time::timeout(std::time::Duration::ZERO,wake.notified()).await.is_err());
        }
    }
    // Match the existing polling predicate exactly: imported, terminal and failed rows do not fence.
    h.c.execute("DELETE FROM transactions",[]).unwrap();
    h.c.execute("UPDATE bots SET transient_data='{}' WHERE id=?1",[bot]).unwrap();
    for (external,status,external_status) in [("imported_a",0,1),("closed",0,2),("failed",1,0)] {
        h.c.execute("INSERT INTO transactions(bot_id,exchange_id,external_id,status,external_status,created_at,updated_at)VALUES(?1,?2,?3,?4,?5,'2026-09-10 00:00:00','2026-09-10 00:00:00')",(bot,h.seed.exchange_id,external,status,external_status)).unwrap();
    }
    let before=h.snapshot();
    let answer=h.submit("DELETE","/settings/destroy_api_key/1",&[],Csrf::Header).await;
    assert_eq!(answer.status,422,"the unchanged guard rejects imported history");
    assert!(!answer.body.contains("An order is still being settled."),"K does not track imported or terminal rows");
    assert_eq!(h.snapshot(),before);
    h.c.execute("DELETE FROM transactions",[]).unwrap();
    assert_eq!(h.submit("DELETE","/settings/destroy_api_key/1",&[],Csrf::Header).await.status,200);
    assert!(tokio::time::timeout(std::time::Duration::ZERO,wake.notified()).await.is_ok());
}

#[tokio::test(flavor="current_thread")]
async fn credential_fence_rereads_intents_created_during_network_validation() {
    use deltabadger::{engine::{amount::{self,Sizing},model,placement,FixedClock},ruby::BigDec};
    let server=MockServer::start().await;
    let mut h=Harness::new(server.uri()).await;
    let bot_id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
    let db_path=h._dir.path().join("production.sqlite3");
    let at=deltabadger::engine::Clock::now(h.clock.as_ref());
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(move |_request:&wiremock::Request| {
        // A second connection represents the engine committing while validation awaited HTTP.
        let c=rusqlite::Connection::open(&db_path).unwrap();
        let bot=model::load_bot(&c,bot_id).unwrap();
        let ticker=model::ticker_for(&c,&bot).unwrap().unwrap();
        let Sizing::Place(plan)=amount::size(&bot,&ticker,&BigDec::from_i64(60),&BigDec::from_i64(50_000),deltabadger::engine::venue_rules::ALPACA.minimum_logic).unwrap() else {panic!("fixture")};
        placement::begin(&c,&bot,&plan,&FixedClock(at)).unwrap();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))
    }).expect(1).mount(&server).await;
    let r=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(r.status,422);assert!(r.body.contains("An order is still being settled."));
    let bot=model::load_bot(&h.c,bot_id).unwrap();assert!(bot.rust_placement().is_some());
    assert_eq!(model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap().key,"previous-key");
    server.verify().await;
}

// L: pause the production price boundary, replace before intent, and retry with B.
async fn tick_with_a_possible_pre_intent_replacement(replace: bool, same_stamp: bool, backwards: bool, sync_during: bool) {
    use deltabadger::{engine::{model,tick,FixedClock},venue::{alpaca::{AlpacaVenue,Urls},http::{self,HttpRequest,HttpResponse,ReqwestTransport,Transport,TransportError}}};
    use wiremock::matchers::header;
    struct PausedPrice { inner: ReqwestTransport, entered: Arc<tokio::sync::Notify>, resume: Arc<tokio::sync::Notify> }
    impl Transport for PausedPrice {
        async fn send(&self, request:&HttpRequest)->Result<HttpResponse,TransportError> {
            if request.path=="/v1beta3/crypto/us/latest/quotes" { self.entered.notify_one();self.resume.notified().await; }
            self.inner.send(request).await
        }
    }
    let server=MockServer::start().await;
    let mut h=Harness::at_real_now(server.uri()).await;
    let at=h.app.now();
    let bot_id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at-chrono::Duration::seconds(1))));
    Mock::given(method("POST")).and(path("/v2/orders")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"account-a-order"}))).expect(if replace {0} else {1}).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quotes":{"BTC/USD":{"ap":"50000"}}}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","non_marginable_buying_power":"10000"}))).expect(if same_stamp {2} else if replace {0} else {1}).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","non_marginable_buying_power":"10000"}))).expect(if replace {2} else {0}).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quotes":{"BTC/USD":{"ap":"50000"}}}))).expect(if replace {1} else {0}).mount(&server).await;
    Mock::given(method("POST")).and(path("/v2/orders")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"account-b-order"}))).expect(if replace {1} else {0}).mount(&server).await;
    if same_stamp {
        // First stamp A with an ordinary authorized save; the next save uses the same
        // wall clock, as a repeated persisted timestamp or a backward clock step can.
        assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","previous-key"),("api_key[secret]","rotated-a-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
        let bot=model::load_bot(&h.c,bot_id).unwrap();
        let first=model::credential_version(&h.c,&bot).unwrap();
        let first_stamp:String=h.c.query_row("SELECT updated_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();
        // A real identical save at the repeated wall clock keeps Rails' timestamp and still changes
        // the ciphertext version. Assert this before the paused tick so a timestamp-only bypass is
        // caught by the data contract, even if it would skip before reaching the paused HTTP boundary.
        assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","previous-key"),("api_key[secret]","rotated-a-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
        let second_stamp:String=h.c.query_row("SELECT updated_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();
        assert_eq!(first_stamp,second_stamp,"the real repeated-clock saves have the same persisted timestamp");
        assert!(model::credential_version(&h.c,&bot).unwrap()!=first,"fresh encryption must version an identical save even at the same persisted timestamp");
    }
    let entered=Arc::new(tokio::sync::Notify::new());let resume=Arc::new(tokio::sync::Notify::new());
    let bot=model::load_bot(&h.c,bot_id).unwrap();
    assert_eq!(deltabadger::engine::amount::pending_quote_amount(&h.c,&bot,at.timestamp_micros()).unwrap().to_s_f(),"60.0");
    let (credentials,credential_version)=model::credentials_with_version(&h.c,&h.app.cipher,&bot).unwrap();
    let credentials=credentials.unwrap();
    let syncing_credentials=credentials.clone();
    let urls=Urls{trading:server.uri(),data:server.uri()};
    let venue=AlpacaVenue::new(PausedPrice{inner:ReqwestTransport::new(http::client(),credentials.key,credentials.secret),entered:entered.clone(),resume:resume.clone()},urls.clone());
    let engine_db=rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap();
    let mut attempts=tick::Attempts::default();let tick_clock=FixedClock(at);
    let prices=tick::PriceCache::default();
    let cx=tick::TickContext{credential_version,prices:&prices,process_start:chrono::DateTime::<chrono::Utc>::MIN_UTC,stopping:&||false};
    let mut recovered=None;
    let running=tick::tick_recovering(&engine_db,&venue,bot_id,&tick_clock,&mut attempts,&mut recovered,&cx);
    async fn rotate_only(h:&mut Harness,bot:&model::Bot,version:&Option<model::CredentialVersion>,at:chrono::DateTime<chrono::Utc>,flags:(bool,bool,bool)) {
        let (replace,same_stamp,backwards)=flags;
        if replace {
            if backwards { h.clock.set(at-chrono::Duration::days(1)); }
            let r=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
            assert_eq!(r.status,200,"K permits a replacement with no unresolved order");
            if same_stamp {
                let current=model::credential_version(&h.c,bot).unwrap();
                assert_eq!(current.as_ref().unwrap().id,version.as_ref().unwrap().id,"modern replacement keeps the row identity");
                assert_eq!(model::credentials_for(&h.c,&h.app.cipher,bot).unwrap().unwrap().key,"account-b-key");
            }
        }
    }
    let replacing=async {
        entered.notified().await;
        assert!(model::load_bot(&h.c,bot_id).unwrap().rust_placement().is_none());
        if sync_during {
            struct PausedLedger { inner: ReqwestTransport, entered: Arc<tokio::sync::Notify>, resume: Arc<tokio::sync::Notify> }
            impl Transport for PausedLedger {
                async fn send(&self,request:&HttpRequest)->Result<HttpResponse,TransportError> {
                    if request.path=="/v2/account/activities" { self.entered.notify_one();self.resume.notified().await; }
                    self.inner.send(request).await
                }
            }
            let sync_entered=Arc::new(tokio::sync::Notify::new());let sync_resume=Arc::new(tokio::sync::Notify::new());
            Mock::given(method("GET")).and(path("/v2/account/activities")).and(header("APCA-API-KEY-ID","previous-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{"id":"account-a-interest","activity_type":"INT","net_amount":"0.07","date":"2026-09-01"}])))
                .expect(1).mount(&server).await;
            let sync_venue=AlpacaVenue::new(PausedLedger{inner:ReqwestTransport::new(http::client(),syncing_credentials.key.clone(),syncing_credentials.secret.clone()),entered:sync_entered.clone(),resume:sync_resume.clone()},urls.clone());
            let sync_db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
            let sync_clock=FixedClock(at);
            let sync=deltabadger::sync::ledger::sync(&sync_db,&sync_venue,h.seed.api_key_id,&syncing_credentials,&sync_clock);
            let replacement=async {
                sync_entered.notified().await;
                rotate_only(&mut h,&bot,&cx.credential_version,at,(replace,same_stamp,backwards)).await;
                let committed=model::credential_version(&h.c,&bot).unwrap();
                assert_eq!(committed == cx.credential_version,!replace,"a replacement changes the ciphertext version; sync alone does not");
                sync_resume.notify_one();
            };
            let (summary,())=tokio::join!(sync,replacement);
            if replace {
                assert_eq!(summary.unwrap_err().0,"credentials changed; venue result discarded", "Q must discard the old-account ledger run");
                let rows:i64=h.c.query_row("SELECT count(*) FROM account_transactions WHERE tx_id='account-a-interest'",[],|r|r.get(0)).unwrap();
                assert_eq!(rows,0,"Q must not import A's ledger after B is saved");
            } else {
                assert_eq!(summary.unwrap().unwrap().imported,1,"the unchanged-account sync still imports");
                let linked:(i64,f64)=h.c.query_row("SELECT api_key_id,base_amount FROM account_transactions WHERE tx_id='account-a-interest'",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
                assert_eq!(linked,(h.seed.api_key_id,0.07));
            }
            assert_eq!(model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap().key,if replace {"account-b-key"} else {"previous-key"});
            assert_eq!(model::credential_version(&h.c,&bot).unwrap() == cx.credential_version,!replace,"real sync must neither restore nor change the credential digest");
        } else { rotate_only(&mut h,&bot,&cx.credential_version,at,(replace,same_stamp,backwards)).await; }
        resume.notify_one();
    };
    let (outcome,())=tokio::time::timeout(std::time::Duration::from_secs(20),async {tokio::join!(running,replacing)}).await.unwrap();
    if same_stamp && replace {
        let calls=server.received_requests().await.unwrap();
        let stale=calls.iter().filter(|request|request.method=="POST"&&request.url.path()=="/v2/orders"&&request.headers.get("APCA-API-KEY-ID").is_some_and(|value|value=="previous-key")).count();
        let (rows,usd):(i64,f64)=h.c.query_row("SELECT count(*),coalesce(sum(quote_amount),0) FROM transactions WHERE bot_id=?1",[bot_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!((stale,rows,usd),(0,0,0.0),"the paused tick must never place with a replaced account");
    }
    if replace {
        assert!(matches!(outcome.unwrap(),tick::TickOutcome::Skipped),"L skips normally without sending with A");
        let current=model::load_bot(&h.c,bot_id).unwrap();assert!(current.rust_placement().is_none());
        let count:i64=h.c.query_row("SELECT count(*) FROM transactions WHERE bot_id=?1",[bot_id],|r|r.get(0)).unwrap();assert_eq!(count,0);
        assert_eq!(deltabadger::engine::amount::pending_quote_amount(&h.c,&current,at.timestamp_micros()).unwrap().to_s_f(),"60.0");
        let (until,_)=current.rust_defer().unwrap().unwrap();assert!(until<=at.timestamp_micros(),"next engine pass retries; it must not wait a week");
        assert_eq!((attempts.transient,attempts.rate),(0,0));
        let credentials=model::credentials_for(&h.c,&h.app.cipher,&current).unwrap().unwrap();assert_eq!(credentials.key,"account-b-key");
        let next=AlpacaVenue::new(ReqwestTransport::new(http::client(),credentials.key,credentials.secret),urls);
        assert!(matches!(tick::tick(&h.c,&next,bot_id,&FixedClock(at+chrono::Duration::seconds(1)),&mut attempts).await.unwrap(),tick::TickOutcome::Done{placed:true}));
    } else { assert!(matches!(outcome.unwrap(),tick::TickOutcome::Done{placed:true})); }
    let (count,amount,external):(i64,f64,String)=h.c.query_row("SELECT count(*),quote_amount,external_id FROM transactions WHERE bot_id=?1",[bot_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(count,1);assert_eq!(amount,60.0);assert_eq!(external,if replace {"account-b-order"} else {"account-a-order"});
    server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn replacement_before_intent_prevents_a_send_and_retries_with_fresh_credentials() { tick_with_a_possible_pre_intent_replacement(true, false, false, false).await; }
#[tokio::test(flavor="current_thread")]
async fn unchanged_credentials_before_intent_place_exactly_as_before() { tick_with_a_possible_pre_intent_replacement(false, false, false, false).await; }

// M: the privacy branch must clear authorization delivered for the previous pending address.
// Every change/freeing operation below goes through real password+CSRF handlers.
#[tokio::test(flavor="current_thread")]
async fn taken_email_request_must_not_rebind_a_token_sent_to_the_previous_address() {
    let server=MockServer::start().await;
    let mut h=Harness::new(server.uri()).await;
    h.c.execute("INSERT INTO users(email,encrypted_password,name,confirmed_at,setup_completed,wash_sale_enabled,created_at,updated_at) SELECT 'taken@example.com',encrypted_password,'Other',confirmed_at,1,0,created_at,updated_at FROM users WHERE id=?1",[h.seed.user_id]).unwrap();
    let other=h.c.last_insert_rowid();
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]","next@example.com"),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let first:String=h.c.query_row("SELECT confirmation_token FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();
    assert_eq!(h.mail.0.lock().unwrap().len(),1);
    assert!(h.mail.0.lock().unwrap()[0].to.contains("next@example.com"));
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]","taken@example.com"),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let (pending,retained,sent):(String,Option<String>,Option<String>)=h.c.query_row("SELECT unconfirmed_email,confirmation_token,confirmation_sent_at FROM users WHERE id=?1",[h.seed.user_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    assert_eq!(pending,"taken@example.com");assert!(retained.is_none(),"superseded authorization survived");assert!(sent.is_none(),"superseded delivery timestamp survived");
    let mut b=Browser::default();
    b.send(&h.app,"GET","/login",None,Csrf::None,&[]).await;
    assert_eq!(b.send(&h.app,"POST","/login",Some(&[("user[email]","taken@example.com"),("user[password]","Correct-horse-9")]),Csrf::Form,&[]).await.status,303);
    assert_eq!(b.send(&h.app,"GET","/settings/account",None,Csrf::None,&[]).await.status,200);
    assert_eq!(b.send(&h.app,"PATCH","/settings/update_email",Some(&[("user[email]","moved@example.com"),("user[current_password]","Correct-horse-9")]),Csrf::Header,&[]).await.status,200);
    let other_token:String=h.c.query_row("SELECT confirmation_token FROM users WHERE id=?1",[other],|r|r.get(0)).unwrap();
    assert_eq!(b.send(&h.app,"GET",&format!("/confirmation?confirmation_token={other_token}"),None,Csrf::None,&[]).await.status,302);
    let moved:String=h.c.query_row("SELECT email FROM users WHERE id=?1",[other],|r|r.get(0)).unwrap();assert_eq!(moved,"moved@example.com");
    assert_eq!(h.browser.send(&h.app,"GET",&format!("/confirmation?confirmation_token={first}"),None,Csrf::None,&[("Origin","http://foreign.example")]).await.status,200);
    assert_eq!(h.mail.0.lock().unwrap().len(),2);
    assert!(h.mail.0.lock().unwrap().iter().all(|mail|!mail.to.contains("taken@example.com")));
    let current:String=h.c.query_row("SELECT email FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();
    assert_eq!(current,"o@example.com","security: a token delivered for the superseded address must not authorize the later address");
}

#[tokio::test(flavor="current_thread")]
async fn replacement_at_the_same_persisted_timestamp_must_prevent_a_stale_account_send() {
    tick_with_a_possible_pre_intent_replacement(true, true, false, false).await;
}

#[tokio::test(flavor="current_thread")]
async fn replacement_after_a_backward_clock_step_must_prevent_a_stale_account_send() {
    tick_with_a_possible_pre_intent_replacement(true, true, true, false).await;
}

// P: each encrypted column participates; metadata timestamps never version credentials.
#[tokio::test(flavor="current_thread")]
async fn credential_digest_covers_each_stored_column_and_ignores_sync_metadata() {
    use deltabadger::engine::{model, placement, amount::{self, Sizing}, FixedClock};
    let server=MockServer::start().await;
    let h=Harness::new(server.uri()).await;
    let at=h.app.now();
    let id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at-chrono::Duration::seconds(1))));
    let bot=model::load_bot(&h.c,id).unwrap();
    let ticker=model::ticker_for(&h.c,&bot).unwrap().unwrap();
    let Sizing::Place(plan)=amount::size(&bot,&ticker,&deltabadger::ruby::BigDec::from_i64(60),&deltabadger::ruby::BigDec::from_i64(50_000),deltabadger::engine::venue_rules::ALPACA.minimum_logic).unwrap() else {panic!("fixture")};
    let composition=placement::composition_snapshot(&h.c,&bot).unwrap();
    let reconciled=deltabadger::engine::splits::snapshot(&h.c,&bot).unwrap();
    for column in ["key","secret","passphrase","access_token","rsa_signature_key","rsa_encryption_key","dh_param"] {
        let (_,captured)=model::credentials_with_version(&h.c,&h.app.cipher,&bot).unwrap();
        let stored:Option<String>=h.c.query_row(&format!("SELECT {column} FROM api_keys WHERE id=?1"),[h.seed.api_key_id],|r|r.get(0)).unwrap();
        let plaintext=stored.map(|v|h.app.cipher.decrypt(&v).unwrap()).unwrap_or_else(||"synthetic-value".into());
        // Identical plaintext, fresh Rails-compatible encryption nonce. No timestamp change.
        h.c.execute(&format!("UPDATE api_keys SET {column}=?1 WHERE id=?2"),(h.app.cipher.encrypt(&plaintext),h.seed.api_key_id)).unwrap();
        let result=placement::begin_with_credentials(&h.c,&bot,&plan,&[&ticker],&composition,&reconciled,&captured,&FixedClock(at)).unwrap();
        assert!(matches!(result,placement::Begun::CredentialsChanged),"each encrypted credential column must prevent an intent after re-encryption: {column}");
        assert!(model::load_bot(&h.c,id).unwrap().rust_placement().is_none());
    }
    let captured=model::credential_version(&h.c,&bot).unwrap();
    h.c.execute("UPDATE api_keys SET updated_at='2099-01-01 00:00:00',last_sync_error=NULL WHERE id=?1",[h.seed.api_key_id]).unwrap();
    assert_eq!(captured,model::credential_version(&h.c,&bot).unwrap(),"sync metadata must not change a credential version");
    assert!(matches!(placement::begin_with_credentials(&h.c,&bot,&plan,&[&ticker],&composition,&reconciled,&captured,&FixedClock(at)).unwrap(),placement::Begun::Intent(_)),"metadata-only changes must allow placement");
}

// A real sync starts with A while the actual tick is paused before intent. B is
// committed with a new encrypted credential digest. Finishing A's sync must not restore A's version.
#[tokio::test(flavor="current_thread")]
async fn finishing_a_paused_ledger_sync_after_replacement_must_not_restore_a_stale_credential_version() {
    tick_with_a_possible_pre_intent_replacement(true, true, false, true).await;
}

#[tokio::test(flavor="current_thread")]
async fn finishing_a_ledger_sync_without_replacement_does_not_block_placement() {
    tick_with_a_possible_pre_intent_replacement(false, false, false, true).await;
}

#[tokio::test(flavor="current_thread")]
async fn legacy_key_fields_come_from_api_key_never_a_top_level_query_parameter() {
    // Rails reads params.require(:api_key): a top-level `?key=`/`?secret=` never wins over the body.
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let status=h.submit("POST","/api/api_keys?key=url-key&secret=url-secret",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","body-key"),("api_key[secret]","body-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status;
    assert_eq!(status,201);
    let (key,secret):(String,String)=h.c.query_row("SELECT key,secret FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!((h.app.cipher.decrypt(&key).unwrap(),h.app.cipher.decrypt(&secret).unwrap()),("body-key".to_string(),"body-secret".to_string()),"the body's api_key fields are stored, not the query string's");
}

#[tokio::test(flavor="current_thread")]
async fn identical_saves_change_ciphertext_version_and_keep_rails_timestamps() {
    use deltabadger::engine::model;
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"})))
        .expect(3).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let at=h.app.now();
    let id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at-chrono::Duration::seconds(1))));
    let bot=model::load_bot(&h.c,id).unwrap();
    let fields=[("exchange_id","1"),("key_type","trading"),("api_key[key]","same-key"),("api_key[secret]","same-secret"),("api_key[passphrase]","paper")];
    assert_eq!(h.submit("POST","/tracker/add_api_key",&fields,Csrf::Header).await.status,200);
    let first=model::credential_version(&h.c,&bot).unwrap();
    let stamp:String=h.c.query_row("SELECT updated_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();
    h.clock.set(at-chrono::Duration::days(1));
    assert_eq!(h.submit("POST","/tracker/add_api_key",&fields,Csrf::Header).await.status,200);
    assert_ne!(first,model::credential_version(&h.c,&bot).unwrap(),"identical plaintext must receive fresh nonces");
    assert_eq!(stamp,h.c.query_row("SELECT updated_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get::<_,String>(0)).unwrap(),"Rails does not touch time on an unchanged correct key");
    assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","replacement"),("api_key[secret]","replacement-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
    assert_eq!(deltabadger::codec::format_time(at-chrono::Duration::days(1)),h.c.query_row("SELECT updated_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get::<_,String>(0)).unwrap(),"P drops the monotonic timestamp rule");
    server.verify().await;
}

// Follow a rotation through the actual balance writer and tracker snapshot reader.
// Only the exchange HTTP boundary is paused; no SQL manufactures balances or timestamps.
#[tokio::test(flavor="current_thread")]
async fn a_balance_sync_started_before_replacement_must_not_publish_old_account_money_as_current() {
    use deltabadger::{engine::{model,FixedClock},sync::balances::{self,NoPrices},tracker::{snapshot,walk::Summary},venue::{alpaca::{AlpacaVenue,Urls},http::{self,HttpRequest,HttpResponse,ReqwestTransport,Transport,TransportError}}};
    use wiremock::matchers::header;
    struct PausedPositions { inner:ReqwestTransport, entered:Arc<tokio::sync::Notify>, resume:Arc<tokio::sync::Notify> }
    impl Transport for PausedPositions {
        async fn send(&self,request:&HttpRequest)->Result<HttpResponse,TransportError> {
            if request.path=="/v2/positions" { self.entered.notify_one();self.resume.notified().await; }
            self.inner.send(request).await
        }
    }
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"10000"}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"2000"}))).expect(2).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).expect(2).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let at=h.app.now();
    let bot_id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at-chrono::Duration::seconds(1))));
    let bot=model::load_bot(&h.c,bot_id).unwrap();
    let (a,version_a)=model::credentials_with_version(&h.c,&h.app.cipher,&bot).unwrap();let a=a.unwrap();
    let entered=Arc::new(tokio::sync::Notify::new());let resume=Arc::new(tokio::sync::Notify::new());
    let urls=Urls{trading:server.uri(),data:server.uri()};
    let venue_a=AlpacaVenue::new(PausedPositions{inner:ReqwestTransport::new(http::client(),a.key.clone(),a.secret.clone()),entered:entered.clone(),resume:resume.clone()},urls.clone());
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let old_clock=FixedClock(at);
    let running=balances::sync(&db,&venue_a,&NoPrices,h.seed.api_key_id,&a,&old_clock);
    let replacing=async {
        entered.notified().await;
        assert_eq!(h.c.query_row("SELECT count(*) FROM account_balances",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        h.clock.set(at+chrono::Duration::seconds(1));
        let r=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
        assert_eq!(r.status,200);assert_ne!(version_a,model::credential_version(&h.c,&bot).unwrap());
        assert!(model::load_bot(&h.c,bot_id).unwrap().rust_placement().is_none());
        resume.notify_one();
    };
    let (old,())=tokio::time::timeout(std::time::Duration::from_secs(20),async {tokio::join!(running,replacing)}).await.unwrap();
    assert_eq!(old.unwrap_err().0,"credentials changed; venue result discarded", "Q must discard the old-account balance run");
    let stale=snapshot::today_row(&h.c,h.seed.user_id,Some(h.seed.exchange_id),&Summary::empty()).unwrap();
    let stale_value=stale.as_ref().map(|day|(day.value.to_s_f(),day.partial));
    assert_eq!(model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap().key,"account-b-key");
    // The new account control actually uses B's saved credentials and the same production writer/reader.
    let b=model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap();
    let venue_b=AlpacaVenue::new(ReqwestTransport::new(http::client(),b.key.clone(),b.secret.clone()),urls);
    assert_eq!(balances::sync(&db,&venue_b,&NoPrices,h.seed.api_key_id,&b,&FixedClock(at+chrono::Duration::seconds(2))).await.unwrap().unwrap().synced,1);
    let current=snapshot::today_row(&h.c,h.seed.user_id,Some(h.seed.exchange_id),&Summary::empty()).unwrap().unwrap();
    assert_eq!((current.value.to_s_f(),current.partial),("2000.0".into(),false));
    server.verify().await;
    assert_eq!(stale_value,None,"money: an in-flight A sync must not publish A's USD10000 as the current B account's complete portfolio after B has been saved (B is USD2000)");
}

// Q also covers failures: an A response cannot condemn B or publish A diagnostics.
async fn refused_old_sync_cannot_change_replacement(kind: &str) {
    use deltabadger::{engine::{model,FixedClock},sync::{balances::{self,NoPrices},ledger},venue::{alpaca::{AlpacaVenue,Urls},http::{self,HttpRequest,HttpResponse,ReqwestTransport,Transport,TransportError}}};
    use wiremock::matchers::header;
    struct Paused { inner:ReqwestTransport, entered:Arc<tokio::sync::Notify>, resume:Arc<tokio::sync::Notify> }
    impl Transport for Paused {
        async fn send(&self,request:&HttpRequest)->Result<HttpResponse,TransportError> {
            self.entered.notify_one();self.resume.notified().await;self.inner.send(request).await
        }
    }
    let server=MockServer::start().await;
    let endpoint=if kind=="balance" {"/v2/account"} else {"/v2/account/activities"};
    Mock::given(method("GET")).and(path(endpoint)).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(if kind=="balance" {401} else {503}).set_body_json(serde_json::json!({"message":"unauthorized previous-secret"}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;let at=h.app.now();
    let a=deltabadger::sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
    let entered=Arc::new(tokio::sync::Notify::new());let resume=Arc::new(tokio::sync::Notify::new());
    let venue=AlpacaVenue::new(Paused{inner:ReqwestTransport::new(http::client(),a.key.clone(),a.secret.clone()),entered:entered.clone(),resume:resume.clone()},Urls{trading:server.uri(),data:server.uri()});
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let key_id=h.seed.api_key_id;
    let running=async {
        if kind=="balance" { balances::sync(&db,&venue,&NoPrices,key_id,&a,&FixedClock(at)).await.map(|_|()) }
        else { ledger::sync(&db,&venue,key_id,&a,&FixedClock(at)).await.map(|_|()) }
    };
    let replacing=async {
        entered.notified().await;
        assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
        resume.notify_one();
    };
    let (result,())=tokio::time::timeout(std::time::Duration::from_secs(20),async {tokio::join!(running,replacing)}).await.unwrap();
    let stored:(i64,Option<String>,Option<String>,Option<String>)=h.c.query_row("SELECT status,last_sync_error,last_synced_at,balances_synced_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(stored,(1,None,None,None),"Q stale status/error/watermark must leave B untouched: {kind}");
    assert_eq!(result.unwrap_err().0,model::CREDENTIALS_CHANGED,"Q stale failure must be a discarded run: {kind}");
    server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn old_balance_failure_must_not_condemn_or_write_diagnostics_for_b() { refused_old_sync_cannot_change_replacement("balance").await; }
#[tokio::test(flavor="current_thread")]
async fn old_ledger_failure_must_not_write_diagnostics_for_b() { refused_old_sync_cannot_change_replacement("ledger").await; }

// Q's engine checks also survive an external encrypted-row writer. K separately
// proves that the settings handler refuses these rotations while an order exists.
struct RotatesAtResponse {
    inner:deltabadger::venue::http::ReqwestTransport,
    file:std::path::PathBuf,
    cipher:deltabadger::crypto::Cipher,
    key_id:i64,
}
impl deltabadger::venue::http::Transport for RotatesAtResponse {
    async fn send(&self,request:&deltabadger::venue::http::HttpRequest)->Result<deltabadger::venue::http::HttpResponse,deltabadger::venue::http::TransportError> {
        let answer=self.inner.send(request).await?;
        let c=rusqlite::Connection::open(&self.file).unwrap();
        c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(self.cipher.encrypt("externally-replaced-key"),self.cipher.encrypt("externally-replaced-secret"),self.key_id)).unwrap();
        Ok(answer)
    }
}
fn q_order(h:&Harness)->(deltabadger::engine::model::Bot,deltabadger::engine::amount::OrderPlan) {
    use deltabadger::{engine::{model,amount::{self,Sizing}},ruby::BigDec};
    let at=h.app.now();
    let id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at-chrono::Duration::seconds(1))));
    let bot=model::load_bot(&h.c,id).unwrap();let ticker=model::ticker_for(&h.c,&bot).unwrap().unwrap();
    let Sizing::Place(plan)=amount::size(&bot,&ticker,&BigDec::from_i64(60),&BigDec::from_i64(50_000),deltabadger::engine::venue_rules::ALPACA.minimum_logic).unwrap() else {panic!("fixture")};
    (bot,plan)
}
fn q_venue(h:&Harness,url:&str)->deltabadger::venue::alpaca::AlpacaVenue<RotatesAtResponse> {
    use deltabadger::venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}};
    AlpacaVenue::new(RotatesAtResponse{inner:ReqwestTransport::new(http::client(),"previous-key".into(),"previous-secret".into()),file:h._dir.path().join("production.sqlite3"),cipher:h.app.cipher.clone(),key_id:h.seed.api_key_id},Urls{trading:url.into(),data:url.into()})
}
fn q_fill()->serde_json::Value { serde_json::json!({"id":"q-order","status":"filled","side":"buy","type":"market","symbol":"BTCUSD","qty":"0.0012","filled_qty":"0.0012","filled_avg_price":"50000"}) }
#[tokio::test(flavor="current_thread")]
async fn q_placement_result_must_keep_intent_when_credentials_changed_during_send() {
    use deltabadger::engine::{placement::{self,Sent},EngineError,FixedClock};
    let server=MockServer::start().await;
    Mock::given(method("POST")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"q-order"}))).expect(1).mount(&server).await;
    let h=Harness::at_real_now(server.uri()).await;let (bot,plan)=q_order(&h);
    let intent=placement::begin(&h.c,&bot,&plan,&FixedClock(h.app.now())).unwrap();
    let Sent::Accepted(id)=placement::send(&q_venue(&h,&server.uri()),&intent,&FixedClock(h.app.now())).await else {panic!("accepted")};
    let result=placement::record_accepted(&h.c,&bot,&intent,&id);
    assert!(matches!(result,Err(EngineError::CredentialsChanged)),"Q placement must discard its result after credential replacement");
    assert!(deltabadger::engine::model::load_bot(&h.c,bot.id).unwrap().rust_placement().is_some());
    assert_eq!(h.c.query_row("SELECT count(*) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn q_recovery_result_must_keep_intent_when_credentials_changed_during_lookup() {
    use deltabadger::engine::{placement::{self},EngineError,FixedClock};
    let server=MockServer::start().await;
    let h=Harness::at_real_now(server.uri()).await;let (bot,plan)=q_order(&h);
    let intent=placement::begin(&h.c,&bot,&plan,&FixedClock(h.app.now())).unwrap();
    let mut fill=q_fill();fill["client_order_id"]=serde_json::json!(intent.cl_ord_id);
    Mock::given(method("GET")).and(path("/v2/orders:by_client_order_id")).respond_with(ResponseTemplate::new(200).set_body_json(fill)).expect(1).mount(&server).await;
    let bot=deltabadger::engine::model::load_bot(&h.c,bot.id).unwrap();
    let result=placement::recover(&h.c,&q_venue(&h,&server.uri()),&bot,&FixedClock(h.app.now())).await;
    assert!(matches!(result,Err(EngineError::CredentialsChanged)),"Q recovery must discard its result after credential replacement");
    assert!(deltabadger::engine::model::load_bot(&h.c,bot.id).unwrap().rust_placement().is_some());
    assert_eq!(h.c.query_row("SELECT count(*) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn q_poll_result_must_not_apply_fill_when_credentials_changed_during_lookup() {
    use deltabadger::engine::{placement::{self},polling,FixedClock};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/orders/q-order")).respond_with(ResponseTemplate::new(200).set_body_json(q_fill())).expect(1).mount(&server).await;
    let h=Harness::at_real_now(server.uri()).await;let (bot,plan)=q_order(&h);
    let intent=placement::begin(&h.c,&bot,&plan,&FixedClock(h.app.now())).unwrap();
    let id=placement::record_accepted(&h.c,&bot,&intent,"q-order").unwrap();
    let before:(i64,f64)=h.c.query_row("SELECT external_status,coalesce(quote_amount_exec,0) FROM transactions WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    let result=polling::follow_up(&h.c,&q_venue(&h,&server.uri()),bot.id,id,h.app.now()).await;
    assert_eq!(format!("{:?}", result.unwrap_err()), "CredentialsChanged", "Q stale poll must request a normal fresh-credential retry");
    assert_eq!(before,h.c.query_row("SELECT external_status,coalesce(quote_amount_exec,0) FROM transactions WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap());
    server.verify().await;
}

// A completed sync has already committed before B is saved: there is no in-flight
// result for Q to discard. Check the reader before B's first refresh.
#[tokio::test(flavor="current_thread")]
async fn previously_committed_balances_must_not_be_reported_as_replacement_accounts_current_money() {
    use deltabadger::{engine::{FixedClock,model},sync::balances::{self,NoPrices},tracker::{snapshot,walk::Summary},venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}}};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"10000"}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"2000"}))).expect(2).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).expect(2).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;let at=h.app.now();
    let bot_id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at-chrono::Duration::seconds(1))));
    let bot=model::load_bot(&h.c,bot_id).unwrap();
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let urls=Urls{trading:server.uri(),data:server.uri()};
    let a=model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap();
    let va=AlpacaVenue::new(ReqwestTransport::new(http::client(),a.key.clone(),a.secret.clone()),urls.clone());
    assert_eq!(balances::sync(&db,&va,&NoPrices,h.seed.api_key_id,&a,&FixedClock(at)).await.unwrap().unwrap().synced,1);
    let before=snapshot::today_row(&h.c,h.seed.user_id,Some(h.seed.exchange_id),&Summary::empty()).unwrap().unwrap();
    assert_eq!((before.value.to_s_f(),before.partial),("10000.0".into(),false));
    h.clock.set(at+chrono::Duration::seconds(1));
    assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
    let b=model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap();assert_eq!(b.key,"account-b-key");
    assert!(model::load_bot(&h.c,bot_id).unwrap().rust_placement().is_none());
    let stale=snapshot::today_row(&h.c,h.seed.user_id,Some(h.seed.exchange_id),&Summary::empty()).unwrap();
    let after_save=stale.map(|day|(day.value.to_s_f(),day.partial));
    let vb=AlpacaVenue::new(ReqwestTransport::new(http::client(),b.key.clone(),b.secret.clone()),urls);
    assert_eq!(balances::sync(&db,&vb,&NoPrices,h.seed.api_key_id,&b,&FixedClock(at+chrono::Duration::seconds(2))).await.unwrap().unwrap().synced,1);
    let current=snapshot::today_row(&h.c,h.seed.user_id,Some(h.seed.exchange_id),&Summary::empty()).unwrap().unwrap();
    assert_eq!((current.value.to_s_f(),current.partial),("2000.0".into(),false));
    server.verify().await;
    assert_ne!(after_save,Some(("10000.0".into(),false)),"money: after B is saved, already committed A balances must not be reported as B's complete current USD10000 portfolio; B has USD2000");
}

// Each remaining Q writer runs against an actual Alpaca HTTP response. The
// boundary rotates encrypted stored credentials before returning that response.
async fn q_tick_result(kind: &str) {
    use deltabadger::engine::{tick::{self, TickOutcome}, model, FixedClock};
    let server = MockServer::start().await;
    let h = Harness::at_real_now(server.uri()).await;
    let (bot, _) = q_order(&h);
    if kind == "clock" {
        h.c.execute("UPDATE assets SET category='Stock' WHERE id=?1", [h.seed.btc]).unwrap();
        common::seed::fresh_stock_jobs(&h.c,h.app.now());
        Mock::given(method("GET")).and(path("/v2/clock")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"is_open":false,"next_open":(h.app.now()+chrono::Duration::hours(1)).to_rfc3339(),"next_close":(h.app.now()+chrono::Duration::hours(8)).to_rfc3339()}))).expect(1).mount(&server).await;
    } else {
        if kind == "funds" {
            // No contribution is due yet, so execution reaches Fundable directly.
            h.c.execute("UPDATE bots SET started_at=?1 WHERE id=?2", (deltabadger::codec::format_time(h.app.now()),bot.id)).unwrap();
        } else {
            if kind == "skipped" { h.c.execute("UPDATE tickers SET minimum_quote_size=100000 WHERE id=?1", [h.seed.ticker_id]).unwrap(); }
            let response = if kind == "failure" { ResponseTemplate::new(401).set_body_json(serde_json::json!({"message":"unauthorized"})) }
                else { ResponseTemplate::new(200).set_body_json(serde_json::json!({"quotes":{"BTC/USD":{"ap":"50000"}}})) };
            Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).respond_with(response).expect(1).mount(&server).await;
        }
        Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"non_marginable_buying_power":"0"}))).mount(&server).await;
    }
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let mut attempts = tick::Attempts::default();
    let result = tick::tick(&h.c, &q_venue(&h, &server.uri()), bot.id, &FixedClock(h.app.now()), &mut attempts).await.unwrap();
    let current = model::load_bot(&h.c, bot.id).unwrap();
    let rows: i64 = h.c.query_row("SELECT count(*) FROM transactions", [], |r| r.get(0)).unwrap();
    let logs: i64 = h.c.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1 AND event IN ('market_closed','execution_failed','execution_retrying','order_skipped','orders_below_minimum')", [bot.id], |r| r.get(0)).unwrap();
    let funds: Option<String> = h.c.query_row("SELECT last_end_of_funds_notification FROM bots WHERE id=?1", [bot.id], |r| r.get(0)).unwrap();
    server.verify().await;
    assert_eq!((rows,logs,funds),(0,0,None),"Q stale {kind} must write no credential-derived rows, diagnostics or funds stamp");
    assert!(current.last_failure_kind().is_none());
    assert!(matches!(result, TickOutcome::Skipped), "Q stale {kind} must retry normally: {result:?}");
    assert_eq!((attempts.transient,attempts.rate),(0,0));
}
#[tokio::test(flavor="current_thread")]
async fn q_clock_result_must_not_park_replacement_credentials() { q_tick_result("clock").await; }
#[tokio::test(flavor="current_thread")]
async fn q_funds_result_must_not_notify_for_replacement_credentials() { q_tick_result("funds").await; }
#[tokio::test(flavor="current_thread")]
async fn q_failure_result_must_not_record_failure_for_replacement_credentials() { q_tick_result("failure").await; }
#[tokio::test(flavor="current_thread")]
async fn q_skipped_result_must_not_record_rows_for_replacement_credentials() { q_tick_result("skipped").await; }

#[tokio::test(flavor="current_thread")]
async fn q_index_probe_result_must_not_change_members_for_replacement_credentials() {
    use deltabadger::engine::{index, model, tick::PriceCache, FixedClock, EngineError};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/stocks/AAA/quotes/latest")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quote":{"ap":100,"bp":100}}))).expect(1).mount(&server).await;
    let h=Harness::new(server.uri()).await;
    common::seed::add_alpaca_stock(&h.c,&h.seed,"AAA");
    common::seed::insert_index(&h.c,"nasdaq-100",&["AAA.US"],&serde_json::json!({"AAA.US":100}));
    let id=common::seed::index_bot(&h.c,&h.seed,"nasdaq-100",1,0.0,false);
    common::seed::fresh_stock_jobs(&h.c,h.app.now());
    let bot=model::load_bot(&h.c,id).unwrap();
    let result=index::refresh_composition(&h.c,&q_venue(&h,&server.uri()),&bot,&FixedClock(h.app.now()),&PriceCache::default()).await;
    let members:i64=h.c.query_row("SELECT count(*) FROM bot_index_assets WHERE bot_id=?1",[id],|r|r.get(0)).unwrap();
    server.verify().await;
    assert_eq!(members,0,"Q stale credential-derived index probes must not persist members");
    assert!(matches!(result,Err(EngineError::CredentialsChanged)));
}

#[tokio::test(flavor="current_thread")]
async fn q_tracker_bar_result_must_not_cache_after_credential_replacement() {
    use deltabadger::{engine::FixedClock, sync::jobs::Connect, tracker::jobs::backfill_run, venue::{alpaca::{AlpacaVenue,Urls},http::ReqwestTransport}};
    struct RotatingFactory { file:std::path::PathBuf, cipher:deltabadger::crypto::Cipher, key:i64, url:String }
    impl Connect for RotatingFactory {
        type T=RotatesAtResponse;
        fn connect(&self,c:&deltabadger::crypto::Credentials)->AlpacaVenue<Self::T> {
            AlpacaVenue::new(RotatesAtResponse{inner:ReqwestTransport::new(deltabadger::venue::http::client(),c.key.clone(),c.secret.clone()),file:self.file.clone(),cipher:self.cipher.clone(),key_id:self.key},Urls{trading:self.url.clone(),data:self.url.clone()})
        }
    }
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/stocks/AAA/bars")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"bars":[{"t":"2026-09-01T04:00:00Z","c":100}]}))).expect(1).mount(&server).await;
    let h=Harness::new(server.uri()).await;
    common::seed::add_alpaca_stock(&h.c,&h.seed,"AAA");
    h.c.execute("INSERT INTO account_transactions (user_id,exchange_id,api_key_id,entry_type,base_currency,base_amount,quote_currency,quote_amount,tx_id,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES (?1,?2,?3,0,'AAA',2,'USD',200,'q-trade','2026-09-01 14:30:00','{}','{}','2026-09-01 14:30:00','2026-09-01 14:30:00')",(h.seed.user_id,h.seed.exchange_id,h.seed.api_key_id)).unwrap();
    let file=h._dir.path().join("production.sqlite3");
    let factory=RotatingFactory{file:file.clone(),cipher:h.app.cipher.clone(),key:h.seed.api_key_id,url:server.uri()};
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(file).unwrap(),h.app.cipher.clone());
    let result=backfill_run(&db,&factory,None::<&deltabadger::jobs::data_api::DataApi<ReqwestTransport>>,h.seed.user_id,&FixedClock(h.app.now())).await;
    server.verify().await;
    assert_eq!(h.c.query_row("SELECT count(*) FROM historical_prices WHERE asset='stock:AAA'",[],|r|r.get::<_,i64>(0)).unwrap(),0,"Q stale credential-derived bars must not enter the price cache");
    assert!(result.is_err());
}
async fn q_direct_old_credentials(kind:&str) {
    use deltabadger::{sync::{self,balances::{self,NoPrices},ledger},engine::{FixedClock,model},venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}}};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"10000"}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let a=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
    assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
    let before=h.snapshot();
    let venue=AlpacaVenue::new(ReqwestTransport::new(http::client(),a.key.clone(),a.secret.clone()),Urls{trading:server.uri(),data:server.uri()});
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let result=if kind=="balance" {balances::sync(&db,&venue,&NoPrices,h.seed.api_key_id,&a,&FixedClock(h.app.now())).await.map(|_|())}
        else {ledger::sync(&db,&venue,h.seed.api_key_id,&a,&FixedClock(h.app.now())).await.map(|_|())};
    assert_eq!(result.unwrap_err().0,model::CREDENTIALS_CHANGED,"Q direct wrapper must bind the caller's credentials to the captured version");
    assert_eq!(h.snapshot(),before);
    assert_eq!(server.received_requests().await.unwrap().len(),1,"only B's validation request; stale wrappers must send nothing");
}
#[tokio::test(flavor="current_thread")]
async fn q_direct_balance_wrapper_must_not_bind_a_to_b_digest() {q_direct_old_credentials("balance").await;}
#[tokio::test(flavor="current_thread")]
async fn q_direct_ledger_wrapper_must_not_bind_a_to_b_digest() {q_direct_old_credentials("ledger").await;}

#[tokio::test(flavor="current_thread")]
async fn r_price_cache_must_refetch_for_replacement_credentials() {
    use deltabadger::{engine::{model,tick::{self,TickContext,PriceCache},FixedClock},venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}}};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quotes":{"BTC/USD":{"ap":"50000"}}}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quotes":{"BTC/USD":{"ap":"200000"}}}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","non_marginable_buying_power":"10000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let mut h=Harness::at_real_now(server.uri()).await;
    let (bot,_)=q_order(&h);h.c.execute("UPDATE tickers SET minimum_quote_size=100000 WHERE id=?1",[h.seed.ticker_id]).unwrap();
    let prices=PriceCache::default();let mut attempts=tick::Attempts::default();let mut recovered=None;
    for replace in [false,true] {
        if replace { assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200); }
        let (credentials,version)=model::credentials_with_version(&h.c,&h.app.cipher,&bot).unwrap();let credentials=credentials.unwrap();
        let venue=AlpacaVenue::new(ReqwestTransport::new(http::client(),credentials.key,credentials.secret),Urls{trading:server.uri(),data:server.uri()});
        let cx=TickContext{credential_version:version,prices:&prices,process_start:chrono::DateTime::<chrono::Utc>::MIN_UTC,stopping:&||false};
        tick::tick_recovering(&h.c,&venue,bot.id,&FixedClock(h.app.now()),&mut attempts,&mut recovered,&cx).await.unwrap();
    }
    let b_requests=server.received_requests().await.unwrap().into_iter().filter(|r|r.url.path().ends_with("/quotes") && r.headers.get("APCA-API-KEY-ID").is_some_and(|v|v=="account-b-key")).count();
    assert_eq!(b_requests,1,"R cached A price must not be reused to size or report a B tick");
    server.verify().await;
}

#[derive(Clone)]
struct LocalAlpaca(String);
impl deltabadger::sync::jobs::Connect for LocalAlpaca {
    type T=deltabadger::venue::http::ReqwestTransport;
    fn connect(&self,c:&deltabadger::crypto::Credentials)->deltabadger::venue::alpaca::AlpacaVenue<Self::T> {
        deltabadger::venue::alpaca::AlpacaVenue::new(deltabadger::venue::http::ReqwestTransport::new(deltabadger::venue::http::client(),c.key.clone(),c.secret.clone()),deltabadger::venue::alpaca::Urls{trading:self.0.clone(),data:self.0.clone()})
    }
}
#[tokio::test(flavor="current_thread")]
async fn saving_a_new_reading_slot_syncs_it_in_the_running_scheduler_without_restart() {
    use deltabadger::{jobs::{self,state},sync::jobs as sync_jobs};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"2000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{ "id":"new-slot-interest", "activity_type":"INT", "net_amount":"0.07", "date":"2026-09-01" }]))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    for job in [sync_jobs::LEDGER_SYNC,sync_jobs::BALANCE_SYNC] {state::record_success(&h.c,job,Some(&h.seed.api_key_id.to_string()),h.app.now()).unwrap();}
    let factory=LocalAlpaca(server.uri());
    let registered=sync_jobs::register(&h.c,&factory,std::rc::Rc::new(deltabadger::sync::balances::NoPrices)).unwrap();
    let scheduler=jobs::Scheduler::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone(),registered,None).with_resolver(jobs::resolve::all(LocalAlpaca(server.uri()),std::rc::Rc::new(None::<jobs::data_api::DataApi<deltabadger::venue::http::ReqwestTransport>>),deltabadger::tracker::jobs::system_wall()));
    let env=web::env(web::SECRET);
    h.app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.clock.clone()).unwrap().with_figure_source(deltabadger::web::figure::loading::Source::Disabled).unwrap().with_settings_key_boundary(server.uri(),Arc::new(Logs::default())).unwrap();
    h.app.attach_jobs(scheduler.wakers()).unwrap();
    let (stop,stopped)=tokio::sync::watch::channel(false);
    let clock=h.clock.clone();
    let control=async {
        let response=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","read_only"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
        let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(20); /* exits at the condition; the bound only fails a hang */
        let mut synced=false;
        while tokio::time::Instant::now()<deadline {
            synced=h.c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions t JOIN api_keys k ON k.id=t.api_key_id WHERE k.user_id=?1 AND k.key_type=2 AND t.tx_id='new-slot-interest' AND k.last_synced_at IS NOT NULL)",[h.seed.user_id],|r|r.get(0)).unwrap();
            if synced {break}tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        stop.send(true).unwrap();(response.status,synced)
    };
    let (run,(status,synced))=tokio::join!(scheduler.run(stopped,clock.as_ref()),control);run.unwrap();
    assert_eq!(status,200);
    assert!(synced,"successful new-key save must wake and register its real ledger sync in the already running scheduler");
}

#[tokio::test(flavor="current_thread")]
async fn r_new_account_cannot_reuse_old_accounts_balance_price() {
    use deltabadger::{engine::FixedClock,sync::{self,balances::{self,NoPrices}},venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}}};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{"symbol":"AAA","qty":"2","asset_class":"us_equity"}]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/stocks/snapshots")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"AAA":{"latestTrade":{"p":120}}}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/stocks/snapshots")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let (asset,_)=common::seed::add_alpaca_stock(&h.c,&h.seed,"AAA");
    h.c.execute("INSERT INTO exchange_assets(exchange_id,asset_id,created_at,updated_at) VALUES (?1,?2,'2026-01-01','2026-01-01')",(h.seed.exchange_id,asset)).unwrap();
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    for replacement in [false,true] {
        if replacement {assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);}
        let credentials=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
        let venue=AlpacaVenue::new(ReqwestTransport::new(http::client(),credentials.key.clone(),credentials.secret.clone()),Urls{trading:server.uri(),data:server.uri()});
        balances::sync(&db,&venue,&NoPrices,h.seed.api_key_id,&credentials,&FixedClock(h.app.now())).await.unwrap().unwrap();
        if !replacement {assert_eq!(h.c.query_row("SELECT usd_value FROM account_balances",[],|r|r.get::<_,f64>(0)).unwrap(),240.0);}
    }
    assert_eq!(h.c.query_row("SELECT usd_value FROM account_balances",[],|r|r.get::<_,Option<f64>>(0)).unwrap(),None,"R B's completed sync must not publish the cached A venue price when B has no price");
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn r_navbar_allocation_hides_a_after_b_is_saved() {
    use deltabadger::{engine::FixedClock,sync::{self,balances::{self,NoPrices}},venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}},web::{auth::User,shell::Shell}};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"10000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE users SET tracker_settings='{\"show_cash\":true}' WHERE id=?1",[h.seed.user_id]).unwrap();
    h.c.execute("UPDATE assets SET color='#123456' WHERE symbol='USD'",[]).unwrap();
    let a=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
    let venue=AlpacaVenue::new(ReqwestTransport::new(http::client(),a.key.clone(),a.secret.clone()),Urls{trading:server.uri(),data:server.uri()});
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    balances::sync(&db,&venue,&NoPrices,h.seed.api_key_id,&a,&FixedClock(h.app.now())).await.unwrap().unwrap();
    let user=User::find(&h.c,h.seed.user_id).unwrap().unwrap();
    assert!(!Shell::load(&h.c,&h.app,&user).unwrap().arcs.is_empty());
    assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
    assert!(Shell::load(&h.c,&h.app,&user).unwrap().arcs.is_empty(),"R navbar must not present A's cached allocation as B's current allocation");
    assert_eq!(h.c.query_row("SELECT usd_value FROM account_balances",[],|r|r.get::<_,f64>(0)).unwrap(),10000.0,"R retains history/cache rows");
}

#[tokio::test(flavor="current_thread")]
async fn r_mcp_inflight_balances_and_orders_must_not_publish_a_for_b() {
    use deltabadger::web::{figure::loading::Source,mcp::{reads,tools::Called}};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let mut script=serde_json::json!({});
    script["GET paper-api.alpaca.markets/v2/account"]=serde_json::json!({"status":200,"delay_ms":100,"body":{"cash":"10000"}});
    script["GET paper-api.alpaca.markets/v2/positions"]=serde_json::json!({"status":200,"body":[]});
    script["GET paper-api.alpaca.markets/v2/orders?limit=50&status=open"]=serde_json::json!({"status":200,"delay_ms":100,"body":[{"id":"order-a","symbol":"BTC/USD","asset_class":"crypto","side":"buy","type":"market","status":"new","qty":"0.2","filled_qty":"0","filled_avg_price":null}]});
    let env=web::env(web::SECRET);
    h.app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.clock.clone()).unwrap().with_figure_source(Source::Script(script)).unwrap().with_settings_key_boundary(server.uri(),Arc::new(Logs::default())).unwrap();
    h.app.attach_jobs(Default::default()).unwrap();
    for tool in ["get_exchange_balances","list_open_orders"] {
        let unchanged=reads::plan(&h.c,&h.app,h.seed.user_id,tool,&serde_json::json!({"exchange_name":"Alpaca"})).unwrap();
        let Called::Fetch(unchanged)=unchanged else{panic!("real unchanged venue fetch required")};
        let unchanged=reads::fetch(&h.app,unchanged).await;
        let current=reads::finish(&h.c,unchanged);
        assert!(current.is_ok(),"R4 unchanged MCP envelope must finish in one transaction: {tool}");
        let current=current.unwrap().to_string();
        assert!(current.contains(if tool=="get_exchange_balances"{"10000"}else{"order-a"}),"R4 unchanged MCP result must retain the actual venue value: {current}");
        h.c.execute("UPDATE api_keys SET key=?1 WHERE id=?2",(h.app.cipher.encrypt("previous-key"),h.seed.api_key_id)).unwrap();
        let planned=reads::plan(&h.c,&h.app,h.seed.user_id,tool,&serde_json::json!({"exchange_name":"Alpaca"})).unwrap();
        let Called::Fetch(planned)=planned else{panic!("real venue fetch required")};
        let app=h.app.clone();
        let replacing=async {tokio::time::sleep(std::time::Duration::from_millis(25)).await;assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);};
        let (fetched,())=tokio::join!(reads::fetch(&app,planned),replacing);
        let response=reads::finish(&h.c,fetched).unwrap().to_string();
        assert!(!response.contains("10000")&&!response.contains("order-a"),"R must discard an in-flight A presentation for B: {tool}: {response}");
        assert!(response.contains("unavailable"),"replacement is an unavailable result, not a fabricated zero");
    }
}

#[tokio::test(flavor="current_thread")]
async fn a_first_reading_key_wakes_its_real_tracker_in_the_running_scheduler() {
    use deltabadger::{jobs::{self,state},sync::jobs as sync_jobs};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"2000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{ "id":"new-slot-interest", "activity_type":"INT", "net_amount":"0.07", "date":"2026-09-01" }]))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    for job in [sync_jobs::LEDGER_SYNC,sync_jobs::BALANCE_SYNC] {state::record_success(&h.c,job,Some(&h.seed.api_key_id.to_string()),h.app.now()).unwrap();}
    let factory=LocalAlpaca(server.uri());
    h.c.execute("UPDATE api_keys SET status=2 WHERE user_id=?1",[h.seed.user_id]).unwrap();
    let mut registered=sync_jobs::register(&h.c,&factory,std::rc::Rc::new(deltabadger::sync::balances::NoPrices)).unwrap();
    registered.extend(deltabadger::tracker::jobs::register(&h.c,&factory,std::rc::Rc::new(None::<deltabadger::jobs::data_api::DataApi<deltabadger::venue::http::ReqwestTransport>>),std::sync::Arc::new({let now=h.app.now();move||now})).unwrap());
    let scheduler=jobs::Scheduler::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone(),registered,None).with_resolver(jobs::resolve::all(LocalAlpaca(server.uri()),std::rc::Rc::new(None::<jobs::data_api::DataApi<deltabadger::venue::http::ReqwestTransport>>),deltabadger::tracker::jobs::system_wall()));
    let env=web::env(web::SECRET);
    h.app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.clock.clone()).unwrap().with_figure_source(deltabadger::web::figure::loading::Source::Disabled).unwrap().with_settings_key_boundary(server.uri(),Arc::new(Logs::default())).unwrap();
    h.app.attach_jobs(scheduler.wakers()).unwrap();
    let (stop,stopped)=tokio::sync::watch::channel(false);
    let clock=h.clock.clone();
    let control=async {
        let response=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","read_only"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
        let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(20); /* exits at the condition; the bound only fails a hang */
        let mut synced=false;
        while tokio::time::Instant::now()<deadline {
            synced=h.c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions t JOIN api_keys k ON k.id=t.api_key_id WHERE k.user_id=?1 AND k.key_type=2 AND t.tx_id='new-slot-interest' AND k.last_synced_at IS NOT NULL)",[h.seed.user_id],|r|r.get(0)).unwrap();
            synced &= state::read(&h.c,deltabadger::tracker::jobs::TRACKER_LEDGER,Some(&h.seed.user_id.to_string())).unwrap().last_success_at.is_some();
            if synced {break}tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        stop.send(true).unwrap();(response.status,synced)
    };
    let (run,(status,synced))=tokio::join!(scheduler.run(stopped,clock.as_ref()),control);run.unwrap();
    assert_eq!(status,200);
    assert!(synced,"first reading key must complete ledger import and the real tracker walk without a restart");
}

impl deltabadger::venue::VenueFactory for LocalAlpaca {
    type V=deltabadger::venue::alpaca::AlpacaVenue<deltabadger::venue::http::ReqwestTransport>;
    fn for_bot(&self,_exchange:&str,credentials:Option<deltabadger::crypto::Credentials>)->Self::V {
        let credentials=credentials.unwrap();
        deltabadger::sync::jobs::Connect::connect(self,&credentials)
    }
}
#[tokio::test(flavor="current_thread")]
async fn r_replacement_invalidates_the_engines_cached_market_wait() {
    use deltabadger::{engine::{run::{self,Engine},FixedClock},lease,store::Paths};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    let mut h=Harness::at_real_now(server.uri()).await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","buying_power":"10000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/clock")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"is_open":false,"next_open":(h.app.now()+chrono::Duration::hours(1)).to_rfc3339(),"next_close":(h.app.now()+chrono::Duration::hours(8)).to_rfc3339()}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/clock")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"is_open":true,"next_open":(h.app.now()+chrono::Duration::hours(1)).to_rfc3339(),"next_close":(h.app.now()+chrono::Duration::hours(8)).to_rfc3339()}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/stocks/AAPL/quotes/latest")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quotes":{"BTC/USD":{"ap":200000}}}))).mount(&server).await;

    let (bot,_)=q_order(&h);h.c.execute("UPDATE assets SET category='Stock',instrument_type='stock' WHERE id=?1",[h.seed.btc]).unwrap();
    h.c.execute("UPDATE tickers SET minimum_quote_size=100000 WHERE id=?1",[h.seed.ticker_id]).unwrap();
    common::seed::fresh_stock_jobs(&h.c,h.app.now());
    let paths=Paths::from_env(&|_|None,h._dir.path());let lock=lease::lock(&paths,h.app.now()).unwrap();
    let mut engine=Engine::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),LocalAlpaca(server.uri()),h.app.cipher.clone(),lock);
    run::step(&mut engine,&FixedClock(h.app.now())).await.unwrap();
    assert!(deltabadger::engine::model::load_bot(&h.c,bot.id).unwrap().transient["waiting_for_market_open"].as_bool().unwrap());
    assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
    common::seed::fresh_stock_jobs(&h.c,h.app.now()); // This clock-cache control starts B with its own completed ledger.
    run::step(&mut engine,&FixedClock(h.app.now()+chrono::Duration::seconds(1))).await.unwrap();
    let requests=server.received_requests().await.unwrap();
    assert!(requests.iter().any(|r|r.url.path()=="/v2/clock"&&r.headers.get("APCA-API-KEY-ID").is_some_and(|h|h=="account-b-key")),"R B tick must read B's market clock instead of waiting on A's cached next_open");
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn legacy_save_runs_validation_and_its_real_balance_sync_without_restart() {
    use deltabadger::{jobs::{self,state},sync::jobs as sync_jobs};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"2000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{ "id":"new-slot-interest", "activity_type":"INT", "net_amount":"0.07", "date":"2026-09-01" }]))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    for job in [sync_jobs::LEDGER_SYNC,sync_jobs::BALANCE_SYNC] {state::record_success(&h.c,job,Some(&h.seed.api_key_id.to_string()),h.app.now()).unwrap();}
    let factory=LocalAlpaca(server.uri());
    let registered=sync_jobs::register(&h.c,&factory,std::rc::Rc::new(deltabadger::sync::balances::NoPrices)).unwrap();
    let scheduler=jobs::Scheduler::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone(),registered,None).with_resolver(jobs::resolve::all(LocalAlpaca(server.uri()),std::rc::Rc::new(None::<jobs::data_api::DataApi<deltabadger::venue::http::ReqwestTransport>>),deltabadger::tracker::jobs::system_wall()));
    let env=web::env(web::SECRET);
    h.app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.clock.clone()).unwrap().with_figure_source(deltabadger::web::figure::loading::Source::Disabled).unwrap().with_settings_key_boundary(server.uri(),Arc::new(Logs::default())).unwrap();
    h.app.attach_jobs(scheduler.wakers()).unwrap();
    let (stop,stopped)=tokio::sync::watch::channel(false);
    let clock=h.clock.clone();
    let control=async {
        let response=h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
        let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(20); /* exits at the condition; the bound only fails a hang */
        let mut synced=false;
        while tokio::time::Instant::now()<deadline {
            let ready:bool=h.c.query_row("SELECT EXISTS(SELECT 1 FROM account_balances b JOIN api_keys k ON k.exchange_id=b.exchange_id AND k.user_id=b.user_id WHERE k.user_id=?1 AND k.key_type=0 AND k.status=1 AND k.id=?2 AND b.usd_value=2000)",[h.seed.user_id,h.seed.api_key_id],|r|r.get(0)).unwrap();
            synced=ready&&!deltabadger::sync::cache::stale(&h.c,h.seed.user_id,None).unwrap();
            if synced {break}tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        stop.send(true).unwrap();(response.status,synced)
    };
    let (run,(status,synced))=tokio::join!(scheduler.run(stopped,clock.as_ref()),control);run.unwrap();
    assert_eq!(status,201);
    assert!(synced,"legacy save must complete actual validation and the enqueued balance sync without restart");
    assert!(!deltabadger::sync::cache::stale(&h.c,h.seed.user_id,None).unwrap(),"the completed legacy sync publishes B's producer digest under the retained identity");
    let requests=server.received_requests().await.unwrap();assert!(!requests.is_empty(),"the sync must exercise real HTTP");
    for request in requests{assert_eq!(request.headers.get("apca-api-key-id").unwrap().to_str().unwrap(),"account-b-key","the woken validator and sync must use B, never the old credential");}

}

#[tokio::test(flavor="current_thread")]
async fn successful_save_encrypts_all_submitted_credentials_and_preserves_unsent_fields() {
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(2).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let extras=[("access_token","placeholder-token"),("rsa_signature_key","placeholder-signature"),("rsa_encryption_key","placeholder-encryption"),("dh_param","placeholder-dh")];
    let fields:Vec<String>=extras.iter().map(|(name,_)|format!("api_key[{name}]")).collect();
    let mut form=vec![("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper"),("api_key[ibkr_realm]","limited_poa")];
    for ((_,value),field) in extras.iter().zip(&fields){form.push((field.as_str(),*value));}
    let response=h.submit("POST","/tracker/add_api_key",&form,Csrf::Header).await;
    assert_eq!(response.status,200);
    for (column,value) in extras {
        let stored:Option<String>=h.c.query_row(&format!("SELECT {column} FROM api_keys WHERE id=?1"),[h.seed.api_key_id],|r|r.get(0)).unwrap();
        let stored=stored.expect("every submitted credential must be stored");
        assert_ne!(stored,value);assert_eq!(h.app.cipher.decrypt(&stored).unwrap(),value);
        assert!(!response.body.contains(value));
    }
    assert_eq!(h.c.query_row("SELECT ibkr_realm FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get::<_,String>(0)).unwrap(),"limited_poa");
    assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret")],Csrf::Header).await.status,200);
    for (column,value) in extras {let stored:String=h.c.query_row(&format!("SELECT {column} FROM api_keys WHERE id=?1"),[h.seed.api_key_id],|r|r.get(0)).unwrap();assert_eq!(h.app.cipher.decrypt(&stored).unwrap(),value,"omitted credential material must survive");}
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn reconnect_get_revalidates_a_stored_pending_key_without_exposing_it() {
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET status=0 WHERE id=?1",[h.seed.api_key_id]).unwrap();
    let response=h.browser.send(&h.app,"GET","/tracker/add_api_key/new?exchange_id=1&key_type=trading",None,Csrf::None,&[("Turbo-Frame","modal")]).await;
    assert_eq!(response.status,200);
    assert_eq!(h.c.query_row("SELECT status FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get::<_,i64>(0)).unwrap(),1,"GET must perform Rails' stored-key validity recheck");
    assert!(!response.body.contains("previous-key")&&!response.body.contains("previous-secret"));
    server.verify().await;
}

async fn q_validation_cannot_condemn_b(passive:bool) {
    use deltabadger::{jobs::{Cx,Db,Job,Outcome},engine::FixedClock,sync::jobs::Validator};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({})).set_delay(std::time::Duration::from_millis(200))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET status=0 WHERE id=?1",[h.seed.api_key_id]).unwrap();
    let mut getter=Browser{cookie:h.browser.cookie.clone(),page:h.browser.page.clone()};let app=h.app.clone();
    let db=Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let clock=FixedClock(h.app.now());let job=Validator{venues:LocalAlpaca(server.uri()),key_id:h.seed.api_key_id};
    let running=async{
        if passive {assert_eq!(getter.send(&app,"GET","/tracker/add_api_key/new?exchange_id=1&key_type=trading",None,Csrf::None,&[("Turbo-Frame","modal")]).await.status,200);}
        else {assert_eq!(job.run(Cx{db,clock:&clock,wakers:Default::default()},vec![]).await,Outcome::NothingNew);}
    };
    let replacing=async{
        while !server.received_requests().await.unwrap().iter().any(|r|r.headers.get("APCA-API-KEY-ID").is_some_and(|h|h=="previous-key")){tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
        assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,200);
    };
    tokio::time::timeout(std::time::Duration::from_secs(20),async{tokio::join!(running,replacing)}).await.unwrap();
    assert_eq!(h.c.query_row("SELECT status FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get::<_,i64>(0)).unwrap(),1,"Q A's validation must not condemn B");
    server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn q_passive_validation_cannot_condemn_replaced_credentials(){q_validation_cannot_condemn_b(true).await;}
#[tokio::test(flavor="current_thread")]
async fn q_validator_job_cannot_condemn_replaced_credentials(){q_validation_cannot_condemn_b(false).await;}

#[tokio::test(flavor="current_thread")]
async fn sync_metadata_during_validation_does_not_refuse_a_credential_replacement() {
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"})).set_delay(std::time::Duration::from_millis(100))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;let file=h._dir.path().join("production.sqlite3");
    let updating=async{
        while server.received_requests().await.unwrap().is_empty(){tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
        let c=rusqlite::Connection::open(file).unwrap();
        c.execute("UPDATE api_keys SET last_synced_at='2026-09-10 12:00:31',updated_at='2026-09-10 12:00:31' WHERE id=1",[]).unwrap();
    };
    let (response,())=tokio::join!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header),updating);
    assert_eq!(response.status,200,"P sync metadata does not change credential identity or invalidate the owner's replacement");
    assert_eq!(deltabadger::sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap().key,"account-b-key");
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn saving_one_key_keeps_other_venues_persisted_sync_warnings_and_correct_fix_links() {
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    h.c.execute("INSERT INTO exchanges(name,type,maker_fee,taker_fee,created_at,updated_at)VALUES('Kraken','Exchanges::Kraken',0,0,'2026-01-01','2026-01-01')",[]).unwrap();
    let exchange=h.c.last_insert_rowid();
    h.c.execute("INSERT INTO api_keys(user_id,exchange_id,key_type,status,last_sync_error,key,secret,created_at,updated_at)VALUES(?1,?2,2,2,'EAPI:Invalid key',?3,?4,'2026-01-01','2026-01-01')",(h.seed.user_id,exchange,h.app.cipher.encrypt("other-key"),h.app.cipher.encrypt("other-secret"))).unwrap();
    let response=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(response.status,200);
    assert!(response.body.contains("Kraken"),"success must rebuild warnings from every persisted venue, not clear them all");
    assert!(response.body.replace("&#38;","&amp;").contains(&format!("exchange_id={exchange}&amp;key_type=read_only")),"a dead tracker key must offer its own replacement: {}",response.body);
    assert!(!response.body.contains("other-secret"));
}

#[tokio::test(flavor="current_thread")]
async fn q_scheduler_does_not_stamp_a_finished_old_run_as_b_current_success() {
    use deltabadger::{jobs::{self,Job,JobFuture,Cx,Wake,Spec,state},sync::jobs::LedgerSync,engine::Clock};
    use std::sync::atomic::{AtomicBool,Ordering};
    struct EndingClock{now:chrono::DateTime<chrono::Utc>,armed:Arc<AtomicBool>,file:std::path::PathBuf,cipher:deltabadger::crypto::Cipher}
    impl Clock for EndingClock{fn now(&self)->chrono::DateTime<chrono::Utc>{if self.armed.swap(false,Ordering::SeqCst){let c=rusqlite::Connection::open(&self.file).unwrap();c.execute("UPDATE api_keys SET key=?1 WHERE id=1",[self.cipher.encrypt("account-b-key")]).unwrap();}self.now}}
    struct RealSync{inner:LedgerSync<LocalAlpaca>,armed:Arc<AtomicBool>}
    impl Job for RealSync{
        fn spec(&self)->Spec{self.inner.spec()}
        fn run<'a>(&'a self,cx:Cx<'a>,wakes:Vec<Wake>)->JobFuture<'a>{Box::pin(async move{self.run_attributed(cx,wakes).await.value})}
        fn run_attributed<'a>(&'a self,cx:Cx<'a>,wakes:Vec<Wake>)->jobs::AttributedJobFuture<'a>{Box::pin(async move{let result=self.inner.run_attributed(cx,wakes).await;self.armed.store(true,Ordering::SeqCst);result})}
    }
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let h=Harness::new(server.uri()).await;let file=h._dir.path().join("production.sqlite3");
    let armed=Arc::new(AtomicBool::new(false));let clock=EndingClock{now:h.app.now(),armed:armed.clone(),file:file.clone(),cipher:h.app.cipher.clone()};
    let scheduler=jobs::Scheduler::new(rusqlite::Connection::open(file).unwrap(),h.app.cipher.clone(),vec![Box::new(RealSync{inner:LedgerSync::new(LocalAlpaca(server.uri()),h.seed.api_key_id),armed})],None).with_resolver(jobs::resolve::all(LocalAlpaca(server.uri()),std::rc::Rc::new(None::<jobs::data_api::DataApi<deltabadger::venue::http::ScriptedTransport>>),deltabadger::tracker::jobs::system_wall()));
    let (stop,stopped)=tokio::sync::watch::channel(false);
    let control=async{
        while deltabadger::sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap().key!="account-b-key"{tokio::time::sleep(std::time::Duration::from_millis(5)).await;}
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;stop.send(true).unwrap();
    };
    tokio::time::timeout(std::time::Duration::from_secs(20),async{let (result,())=tokio::join!(scheduler.run(stopped,&clock),control);result.unwrap();}).await.unwrap();
    assert_eq!(state::read(&h.c,"ledger_sync",Some("1")).unwrap().last_success_at,None,"Q scheduler success must be checked inside its state transaction, after the actual venue run");
}

#[tokio::test(flavor="current_thread")]
async fn bot_credential_replacement_uses_the_owned_trading_slot_and_refreshes_the_page(){
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
    let form=h.browser.send(&h.app,"GET",&format!("/bots/{bot}/add_api_key/new"),None,Csrf::None,&[("turbo-frame","modal")]).await;
    assert_eq!(form.status,200,"the bot replacement form must exist");
    assert!(form.body.contains(&format!("/bots/{bot}/add_api_key")));
    assert!(!form.body.contains("previous-secret"));
    let answer=h.browser.send(&h.app,"POST",&format!("/bots/{bot}/add_api_key"),Some(&[("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")]),Csrf::Header,&[("accept",deltabadger::web::turbo::CONTENT_TYPE)]).await;
    assert_eq!(answer.status,200);assert!(answer.body.contains("action=\"refresh\""));
    let stored:String=h.c.query_row("SELECT key FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();assert_eq!(h.app.cipher.decrypt(&stored).unwrap(),"new-key");
    h.c.execute("INSERT INTO users(id,email,encrypted_password,created_at,updated_at)VALUES(999,'other@example.com','x','2026-01-01','2026-01-01')",[]).unwrap();
    h.c.execute("UPDATE bots SET user_id=999 WHERE id=?1",[bot]).unwrap();
    let foreign=h.browser.send(&h.app,"GET",&format!("/bots/{bot}/add_api_key/new"),None,Csrf::None,&[]).await;assert_eq!(foreign.status,404);
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn confirmation_resend_is_private_csrf_protected_and_delivers_only_the_pending_address(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE users SET unconfirmed_email='next@example.com',confirmation_token='abcdefghijklmnopqrst',confirmation_sent_at='1926-01-01 00:00:00' WHERE id=?1",[h.seed.user_id]).unwrap();
    let before=h.snapshot();
    let denied=h.submit("POST","/confirmation",&[("user[email]","o@example.com")],Csrf::None).await;
    assert_eq!(denied.status,302);assert_eq!(h.snapshot(),before);assert!(h.mail.0.lock().unwrap().is_empty());
    let answer=h.submit("POST","/confirmation",&[("user[email]","o@example.com")],Csrf::Header).await;
    assert_eq!(answer.status,303,"Rails resends and redirects to the sign-in page");
    {let mail=h.mail.0.lock().unwrap();assert_eq!(mail.len(),1);assert!(mail[0].to.contains("next@example.com"));assert!(!answer.body.contains("abcdefghijklmnopqrst"));}
    let wrong=h.submit("POST","/confirmation",&[("user[email]","absent@example.com")],Csrf::Header).await;assert_eq!(wrong.status,answer.status);assert_eq!(wrong.header("location"),answer.header("location"));assert_eq!(h.mail.0.lock().unwrap().len(),1);
}

#[tokio::test(flavor="current_thread")]
async fn configured_connections_show_real_catalog_and_provider_state_without_echoing_credentials(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for (key,value) in [("alpaca_api_key","catalog-key-placeholder"),("alpaca_api_secret","catalog-secret-placeholder"),("alpaca_mode","paper"),("coingecko_api_key","coingecko-key-placeholder"),("market_data_provider","coingecko")]{
        deltabadger::app_config::set(&h.c,&h.app.cipher,key,value,h.app.now()).unwrap();
    }
    let answer=h.browser.send(&h.app,"GET","/settings/connect",None,Csrf::None,&[]).await;
    assert_eq!(answer.status,200);
    assert!(answer.body.contains("settings/stocks") || answer.body.contains("stocks_settings"));
    let stock=answer.body.split("id=\"stocks_settings\"").nth(1).unwrap().split("</turbo-frame>").next().unwrap();
    assert!(stock.contains("text-success\">ON"),"an existing stock-venue catalog must show ON");
    assert!(stock.contains("/settings/disconnect_stocks"),"persisted catalog credentials have Rails' disconnect action");
    let market=answer.body.split("id=\"market_data_settings\"").nth(1).unwrap().split("</turbo-frame>").next().unwrap();
    assert!(market.contains("text-success\">ON"));assert!(market.contains("/settings/disconnect_market_data"));
    for value in ["catalog-key-placeholder","catalog-secret-placeholder","coingecko-key-placeholder"]{assert!(!answer.body.contains(value),"stored credential must never enter a page");}
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn validator_status_commit_wakes_the_running_engine(){
    use deltabadger::{jobs::{Job,Cx,Db},sync::jobs::Validator};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET status=0 WHERE id=?1",[h.seed.api_key_id]).unwrap();
    let notify=Arc::new(tokio::sync::Notify::new());h.app.attach_engine(notify.clone());
    let cx=Cx{db:Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone()),clock:h.clock.as_ref(),wakers:h.app.job_wakers().unwrap()};
    let result=Validator{venues:LocalAlpaca(server.uri()),key_id:h.seed.api_key_id}.run(cx,vec![]).await;
    assert_eq!(result,deltabadger::jobs::Outcome::Done);
    assert_eq!(h.c.query_row("SELECT status FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get::<_,i64>(0)).unwrap(),1);
    assert!(tokio::time::timeout(std::time::Duration::from_millis(50),notify.notified()).await.is_ok(),"a guarded status commit must wake the running engine");
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn legacy_live_save_uses_the_same_paper_only_message_and_sends_nothing(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;let before=h.snapshot();
    let answer=h.submit("POST","/api/api_keys",&[("api_key[exchange_id]",&h.seed.exchange_id.to_string()),("api_key[key_type]","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","live")],Csrf::Header).await;
    assert_eq!(answer.status,422);let body:serde_json::Value=serde_json::from_str(&answer.body).unwrap();assert_eq!(body["message"],"This build accepts paper keys only.");assert_eq!(h.snapshot(),before);assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor="current_thread")]
async fn reading_key_positions_explicit_unauthorized_is_an_incorrect_key_as_in_rails(){
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).with_priority(10).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","bad-account")).respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({}))).with_priority(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"message":"unauthorized"}))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;let ex=h.seed.exchange_id.to_string();let before=h.snapshot();
    let bad=h.submit("POST","/tracker/add_api_key",&[("exchange_id",&ex),("key_type","trading"),("api_key[key]","bad-account"),("api_key[secret]","bad-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    let read=h.submit("POST","/tracker/add_api_key",&[("exchange_id",&ex),("key_type","read_only"),("api_key[key]","positions-bad"),("api_key[secret]","bad-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(bad.status,422);assert_eq!(read.status,422);assert_eq!(read.body,bad.body,"a positions 401 must be the same incorrect-key error, not an inconclusive validation error");assert_eq!(h.snapshot(),before);
}

#[tokio::test(flavor="current_thread")]
async fn reading_key_bare_401_is_inconclusive_and_does_not_condemn_a_stored_key(){
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;let ex=h.seed.exchange_id.to_string();let before=h.snapshot();
    let answer=h.submit("POST","/tracker/add_api_key",&[("exchange_id",&ex),("key_type","read_only"),("api_key[key]","candidate"),("api_key[secret]","candidate-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(answer.status,422);assert!(answer.body.contains("Failed to validate API key permissions"),"Rails does not condemn an unattributed reading-key 401");assert_eq!(h.snapshot(),before);
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn stored_extra_credential_material_is_redacted_from_passive_and_job_validation_errors(){
    use deltabadger::{jobs::{Job,Cx,Db,Outcome},sync::jobs::Validator};
    let server=MockServer::start().await;
    let secret="short-extra-secret";
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"message":format!("expired {secret}")}))).expect(2).mount(&server).await;
    for job in [false,true]{
        let mut h=Harness::new(server.uri()).await;
        h.c.execute("UPDATE api_keys SET status=0,access_token=?1,rsa_signature_key=?1,rsa_encryption_key=?1,dh_param=?1 WHERE id=?2",(h.app.cipher.encrypt(secret),h.seed.api_key_id)).unwrap();
        if job{
            let cx=Cx{db:Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone()),clock:h.clock.as_ref(),wakers:h.app.job_wakers().unwrap()};
            let result=Validator{venues:LocalAlpaca(server.uri()),key_id:h.seed.api_key_id}.run(cx,vec![]).await;
            let Outcome::Failed(diagnostic)=result else{panic!("real failed validation must be surfaced")};
            assert!(diagnostic.contains("HTTP 503")&&!diagnostic.contains("expired"));assert!(!diagnostic.contains(secret),"the scheduler stores and logs this error, so every stored credential must be removed");
        }else{
            let form=h.browser.send(&h.app,"GET",&format!("/tracker/add_api_key/new?exchange_id={}&key_type=trading",h.seed.exchange_id),None,Csrf::None,&[("turbo-frame","modal")]).await;assert_eq!(form.status,200);assert!(!form.body.contains(secret));
            let logs=h.logs.0.lock().unwrap().join("\n");assert!(logs.contains("API key validation failed"));assert!(!logs.contains(secret),"every stored credential must be removed from passive validation logs");
        }
    }
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn scheduler_state_write_failure_is_propagated_and_does_not_claim_success(){
    use deltabadger::{jobs::Scheduler,sync::jobs::{Validator,API_KEY_VALIDATOR},jobs::state};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET status=0 WHERE id=?1",[h.seed.api_key_id]).unwrap();
    h.c.execute_batch("CREATE TRIGGER reject_job_state BEFORE INSERT ON app_configs WHEN NEW.key LIKE 'rust_job.api_key_validator:%' BEGIN SELECT RAISE(ABORT,'forced scheduler state failure'); END").unwrap();
    let scheduler=Scheduler::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone(),vec![Box::new(Validator{venues:LocalAlpaca(server.uri()),key_id:h.seed.api_key_id})],None);
    scheduler.wakers().wake(API_KEY_VALIDATOR,Some(&h.seed.api_key_id.to_string()),None);
    let (_stop,rx)=tokio::sync::watch::channel(false);
    let result=tokio::time::timeout(std::time::Duration::from_secs(3),scheduler.run(rx,h.clock.as_ref())).await;
    assert!(matches!(result,Ok(Err(ref message)) if message.contains("state write")),"a failed venue-derived state write must return an error, not be swallowed");
    assert!(state::read(&h.c,API_KEY_VALIDATOR,Some(&h.seed.api_key_id.to_string())).unwrap().last_success_at.is_none());
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn each_credential_status_and_legacy_write_keeps_the_eligibility_guard_and_no_wake_on_refusal(){
    use deltabadger::{jobs::{Job,Cx,Db,Outcome},sync::jobs::Validator};
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(3).mount(&server).await;
    for path in ["legacy","passive","job","bot"]{
        let mut h=Harness::new(server.uri()).await;
        let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
        h.c.execute("UPDATE bots SET exchange_id=NULL WHERE id=?1",[bot]).unwrap();
        let owned=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
        h.c.execute("UPDATE api_keys SET status=0 WHERE id=?1",[h.seed.api_key_id]).unwrap();
        let wake=Arc::new(tokio::sync::Notify::new());h.app.attach_engine(wake.clone());
        let before=h.snapshot();
        match path{
            "legacy"=>assert_eq!(h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,422),
            "passive"=>assert_eq!(h.browser.send(&h.app,"GET","/tracker/add_api_key/new?exchange_id=1&key_type=trading",None,Csrf::None,&[("turbo-frame","modal")]).await.status,422),
            "job"=>{
                let cx=Cx{db:Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone()),clock:h.clock.as_ref(),wakers:h.app.job_wakers().unwrap()};
                assert!(matches!(Validator{venues:LocalAlpaca(server.uri()),key_id:h.seed.api_key_id}.run(cx,vec![]).await,Outcome::Failed(_)));
            },
            "bot"=>assert_eq!(h.submit("POST",&format!("/bots/{owned}/add_api_key"),&[("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,422),
            _=>unreachable!(),
        }
        assert_eq!(h.snapshot(),before,"guard must roll back {path}");
        assert!(tokio::time::timeout(std::time::Duration::ZERO,wake.notified()).await.is_err(),"{path} must not wake before a commit");
    }
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn tracker_exchange_resolution_reuses_a_valid_session_when_a_requested_id_is_unknown(){
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    assert_eq!(h.browser.send(&h.app,"GET","/tracker/add_api_key/new?exchange_id=1&key_type=trading",None,Csrf::None,&[("turbo-frame","modal")]).await.status,200);
    let form=h.browser.send(&h.app,"GET","/tracker/add_api_key/new?exchange_id=999&key_type=trading",None,Csrf::None,&[("turbo-frame","modal")]).await;
    assert_eq!(form.status,200,"Rails falls back to the valid saved exchange");
    let save=h.submit("POST","/tracker/add_api_key",&[("exchange_id","999"),("key_type","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(save.status,200);assert_eq!(h.c.query_row("SELECT COUNT(*) FROM api_keys WHERE exchange_id=1",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn malformed_settings_values_are_refused_without_any_write_or_side_effect(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for (path,form) in [
        ("/settings/update_wash_sale",vec![("wash_sale[enabled]","1"),("wash_sale[jurisdiction]","ZZ")]),
        ("/settings/update_wash_sale",vec![("wash_sale[jurisdiction]","GB")]),
        ("/settings/update_mcp_tool_permissions",vec![("tool_name","invented"),("enabled","1")]),
        ("/settings/update_rest_tool_permissions",vec![("tool_name","invented"),("enabled","1")]),
        ("/settings/update_mcp_tool_group_permissions",vec![("group","invented"),("enabled","1")]),
        ("/settings/update_rest_tool_group_permissions",vec![("group","invented"),("enabled","1")]),
        ("/settings/update_password",vec![("user[password]","simple"),("user[current_password]","Correct-horse-9")]),
        ("/settings/update_password",vec![("user[password]","Another-horse-7"),("user[password_confirmation]","mismatch"),("user[current_password]","Correct-horse-9")]),
        ("/settings/update_email",vec![("user[email]","invalid"),("user[current_password]","Correct-horse-9")]),
    ]{
        let before=h.snapshot();assert_eq!(h.submit("PATCH",path,&form,Csrf::Header).await.status,422,"{path}");assert_eq!(h.snapshot(),before,"{path}");
    }
    assert!(server.received_requests().await.unwrap().is_empty());assert!(h.mail.0.lock().unwrap().is_empty());
}

async fn rails_password_value(value:&str,expected:u16){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;let before=h.snapshot();
    let answer=h.submit("PATCH","/settings/update_password",&[("user[password]",value),("user[password_confirmation]",value),("user[current_password]","Correct-horse-9")],Csrf::Header).await;
    assert_eq!(answer.status,expected);
    if expected==422||value.trim().is_empty(){assert_eq!(h.snapshot(),before)}else{
        let hash:String=h.c.query_row("SELECT encrypted_password FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();assert!(deltabadger::crypto::verify_password(value,&hash));
    }
    assert!(h.mail.0.lock().unwrap().is_empty());assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor="current_thread")]
async fn password_non_ascii_letter_counts_as_a_symbol_exactly_as_rails(){rails_password_value("Correcthorse9Ż",200).await;}
#[tokio::test(flavor="current_thread")]
async fn password_non_ascii_digit_does_not_satisfy_rails_ascii_digit_rule(){rails_password_value("Correcthorse١!",422).await;}
#[tokio::test(flavor="current_thread")]
async fn password_complexity_cannot_combine_characters_from_different_lines(){rails_password_value("Correct\nhorse-9",422).await;}
#[tokio::test(flavor="current_thread")]
async fn whitespace_password_is_rails_noop_and_does_not_replace_the_hash(){rails_password_value("   ",200).await;}

#[tokio::test(flavor="current_thread")]
async fn google_alias_privacy_branch_clears_the_old_delivered_token_and_sends_no_mail(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]","pending@example.com"),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let old:String=h.c.query_row("SELECT confirmation_token FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();
    h.c.execute("INSERT INTO users(email,encrypted_password,confirmed_at,created_at,updated_at)VALUES('alice@gmail.com',?1,'2026-01-01','2026-01-01','2026-01-01')",[deltabadger::crypto::hash_password("Correct-horse-9").unwrap()]).unwrap();
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]","alice+tag@googlemail.com"),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let (email,pending,token,at):(String,Option<String>,Option<String>,Option<String>)=h.c.query_row("SELECT email,unconfirmed_email,confirmation_token,confirmation_sent_at FROM users WHERE id=?1",[h.seed.user_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(email,"o@example.com");assert_eq!(pending.as_deref(),Some("alice+tag@googlemail.com"));assert!(token.is_none()&&at.is_none(),"Rails' Google alias privacy branch invalidates the old token");assert_eq!(h.mail.0.lock().unwrap().len(),1);
    let old:String=form_urlencoded::byte_serialize(old.as_bytes()).collect();let before=h.snapshot();
    assert_eq!(h.browser.send(&h.app,"GET",&format!("/confirmation?confirmation_token={old}"),None,Csrf::None,&[]).await.status,200);assert_eq!(h.snapshot(),before);
}
#[tokio::test(flavor="current_thread")]
async fn a_google_alias_taken_after_delivery_is_refused_at_confirmation(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]","alice+tag@googlemail.com"),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let token:String=h.c.query_row("SELECT confirmation_token FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();
    h.c.execute("INSERT INTO users(email,encrypted_password,confirmed_at,created_at,updated_at)VALUES('alice@gmail.com',?1,'2026-01-01','2026-01-01','2026-01-01')",[deltabadger::crypto::hash_password("Correct-horse-9").unwrap()]).unwrap();
    let token:String=form_urlencoded::byte_serialize(token.as_bytes()).collect();let before=h.snapshot();
    assert_eq!(h.browser.send(&h.app,"GET",&format!("/confirmation?confirmation_token={token}"),None,Csrf::None,&[]).await.status,200);assert_eq!(h.snapshot(),before,"confirmation reruns Rails' email uniqueness rules");
}
async fn reauth_during_password_rotation(path:&str){
    use std::sync::{Condvar,atomic::{AtomicUsize,AtomicBool,Ordering}};
    let server=MockServer::start().await;
    let calls=Arc::new(AtomicUsize::new(0));let entered=Arc::new(AtomicBool::new(false));let release=Arc::new((Mutex::new(false),Condvar::new()));
    let hook:deltabadger::web::PasswordHook={let calls=calls.clone();let entered=entered.clone();let release=release.clone();Arc::new(move||{
        if calls.fetch_add(1,Ordering::SeqCst)>0{entered.store(true,Ordering::SeqCst);let(lock,wake)=&*release;let mut done=lock.lock().unwrap();while !*done{done=wake.wait(done).unwrap();}}
    })};
    let h=Harness::with_hook(server.uri(),Some(hook)).await;
    let mut browser=h.browser;let app=h.app.clone();let route=path.to_string();
    let task=tokio::spawn(async move{let form=if route.ends_with("email"){vec![("user[email]","next@example.com"),("user[current_password]","Correct-horse-9")]}else{vec![("user[password]","Next-horse-8"),("user[current_password]","Correct-horse-9")]};browser.send(&app,"PATCH",&route,Some(&form),Csrf::Header,&[("origin","http://localhost:3000"),("accept",deltabadger::web::turbo::CONTENT_TYPE)]).await});
    tokio::time::timeout(std::time::Duration::from_secs(3),async{while !entered.load(Ordering::SeqCst){tokio::task::yield_now().await}}).await.unwrap();
    let changed=deltabadger::crypto::hash_password("Another-horse-7").unwrap();
    h.c.execute("UPDATE users SET encrypted_password=?1 WHERE id=?2",(&changed,h.seed.user_id)).unwrap();
    {let(lock,wake)=&*release;*lock.lock().unwrap()=true;wake.notify_all();}
    let answer=task.await.unwrap();assert_eq!(answer.status,422,"a bcrypt result for the old hash never authorizes a write after password rotation");
    let (hash,pending):(String,Option<String>)=h.c.query_row("SELECT encrypted_password,unconfirmed_email FROM users WHERE id=?1",[h.seed.user_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();assert_eq!(hash,changed);assert!(pending.is_none());assert!(h.mail.0.lock().unwrap().is_empty());
}
#[tokio::test(flavor="current_thread")]
async fn email_reauth_rejects_an_old_password_when_the_hash_changes_during_bcrypt(){reauth_during_password_rotation("/settings/update_email").await;}
#[tokio::test(flavor="current_thread")]
async fn password_reauth_rejects_an_old_password_when_the_hash_changes_during_bcrypt(){reauth_during_password_rotation("/settings/update_password").await;}

async fn r_reader_during_a_completed_replacement_sync(navbar:bool){
    use deltabadger::{engine::FixedClock,sync::{self,balances::{self,NoPrices}},tracker::{snapshot,walk::Summary},venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}},web::{auth::User,shell::Shell}};
    use rusqlite::hooks::{AuthAction,Authorization};use std::sync::atomic::{AtomicBool,Ordering};use wiremock::matchers::header;
    let server=MockServer::start().await;
    for (key,cash) in [("previous-key","10000"),("account-b-key","2000")]{Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID",key)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":cash}))).expect(1).mount(&server).await;}
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).expect(2).mount(&server).await;
    let h=Harness::new(server.uri()).await;let at=h.app.now();let file=h._dir.path().join("production.sqlite3");let id=h.seed.api_key_id;let owner=h.seed.user_id;
    h.c.execute_batch("PRAGMA journal_mode=WAL").unwrap();h.c.execute("UPDATE users SET tracker_settings='{\"show_cash\":true}' WHERE id=?1",[owner]).unwrap();h.c.execute("UPDATE assets SET color='#123456' WHERE symbol='USD'",[]).unwrap();
    let a=sync::credentials(&h.c,&h.app.cipher,id).unwrap();let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(&file).unwrap(),h.app.cipher.clone());
    let venue=AlpacaVenue::new(ReqwestTransport::new(http::client(),a.key.clone(),a.secret.clone()),Urls{trading:server.uri(),data:server.uri()});
    balances::sync(&db,&venue,&NoPrices,id,&a,&FixedClock(at)).await.unwrap().unwrap();
    let a_day=snapshot::today_row(&h.c,owner,None,&Summary::empty()).unwrap().unwrap();assert_eq!(a_day.value.to_s_f(),"10000.0");assert!(!a_day.partial);
    let user=User::find(&h.c,owner).unwrap().unwrap();assert!(!Shell::load(&h.c,&h.app,&user).unwrap().arcs.is_empty());
    let cipher=h.app.cipher.clone();let app=h.app.clone();let uri=server.uri();let fired=Arc::new(AtomicBool::new(false));let signal=fired.clone();let c=h.c;
    let (captured_partial,captured_arcs,current_value,current_partial,current_arcs)=tokio::task::spawn_blocking(move||{
        c.authorizer(Some(move|context:rusqlite::hooks::AuthContext<'_>|{
            if matches!(context.action,AuthAction::Read{table_name:"api_keys",column_name:"access_token"})&&!signal.swap(true,Ordering::SeqCst){
                let file=file.clone();let cipher=cipher.clone();let uri=uri.clone();
                std::thread::spawn(move||{
                    let writer=rusqlite::Connection::open(&file).unwrap();writer.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(cipher.encrypt("account-b-key"),cipher.encrypt("account-b-secret"),id)).unwrap();
                    let b=sync::credentials(&writer,&cipher,id).unwrap();let db=deltabadger::jobs::Db::new(writer,cipher);let venue=AlpacaVenue::new(ReqwestTransport::new(http::client(),b.key.clone(),b.secret.clone()),Urls{trading:uri.clone(),data:uri});
                    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(balances::sync(&db,&venue,&NoPrices,id,&b,&FixedClock(at))).unwrap().unwrap();
                }).join().unwrap();
            }
            Authorization::Allow
        }));
        let (partial,arcs)=if navbar{(false,Shell::load(&c,&app,&user).unwrap().arcs.len())}else{(snapshot::today_row(&c,owner,None,&Summary::empty()).unwrap().unwrap().partial,0)};
        let current=snapshot::today_row(&c,owner,None,&Summary::empty()).unwrap().unwrap();let current_arcs=Shell::load(&c,&app,&user).unwrap().arcs.len();
        (partial,arcs,current.value.to_s_f(),current.partial,current_arcs)
    }).await.unwrap();
    assert!(fired.load(Ordering::SeqCst),"the external encrypted replacement and real B sync must run within the reader");
    if navbar{assert_eq!(captured_arcs,0,"A's allocation read must not be shown as current using B's new cache stamp")}else{assert!(captured_partial,"A's balance read must not become current when B finishes before the provenance read")}
    assert_eq!(current_value,"2000.0");assert!(!current_partial);assert!(current_arcs>0);server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn r_tracker_reader_must_not_pair_a_balances_with_b_completed_provenance(){r_reader_during_a_completed_replacement_sync(false).await;}
#[tokio::test(flavor="current_thread")]
async fn r_navbar_reader_must_not_pair_a_balances_with_b_completed_provenance(){r_reader_during_a_completed_replacement_sync(true).await;}

fn invalid_field_texts(body:&str)->Vec<String>{let doc=scraper::Html::parse_document(body);doc.select(&scraper::Selector::parse(".form__info--invalid").unwrap()).map(|field|field.text().collect()).collect()}
fn upcase_first(text:String)->String{let mut chars=text.chars();chars.next().map(|ch|ch.to_uppercase().collect::<String>()+chars.as_str()).unwrap_or_default()}
#[tokio::test(flavor="current_thread")]
async fn email_errors_distinguish_blank_custom_format_and_devise_format_rules(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    let translated=|key|deltabadger::web::i18n::text("en",key,&[]);
    let custom=translated("devise.registrations.new.email_invalid");
    for (email,expected) in [("",format!("{}, {}",translated("errors.messages.blank"),custom)),("name@localhost",custom.clone()),("invalid",format!("{}, {}",translated("errors.messages.invalid"),custom))]{
        let before=h.snapshot();let answer=h.submit("PATCH","/settings/update_email",&[("user[email]",email),("user[current_password]","Correct-horse-9")],Csrf::Header).await;
        assert_eq!(answer.status,422);assert_eq!(invalid_field_texts(&answer.body),vec![upcase_first(expected.clone()),upcase_first(expected)]);assert_eq!(h.snapshot(),before);
    }
}
#[tokio::test(flavor="current_thread")]
async fn password_confirmation_without_a_password_has_rails_presence_error(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;let before=h.snapshot();
    let answer=h.submit("PATCH","/settings/update_password",&[("user[password_confirmation]","Another-horse-7"),("user[current_password]","Correct-horse-9")],Csrf::Header).await;
    assert_eq!(answer.status,422);assert_eq!(invalid_field_texts(&answer.body),vec![upcase_first(deltabadger::web::i18n::text("en","errors.messages.blank",&[])),upcase_first(deltabadger::web::i18n::text("en","errors.messages.confirmation",&[("attribute",deltabadger::web::i18n::Arg::Text("Password"))]))]);assert_eq!(h.snapshot(),before);
}
#[tokio::test(flavor="current_thread")]
async fn whitespace_current_password_has_the_rails_blank_error(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;let before=h.snapshot();
    let answer=h.submit("PATCH","/settings/update_email",&[("user[email]","next@example.com"),("user[current_password]","   ")],Csrf::Header).await;
    assert_eq!(answer.status,422);assert_eq!(invalid_field_texts(&answer.body),vec![upcase_first(deltabadger::web::i18n::text("en","errors.messages.blank",&[]));2]);assert_eq!(h.snapshot(),before);
}
#[tokio::test(flavor="current_thread")]
async fn taken_email_privacy_branch_retains_the_raw_pending_input_as_rails_does(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    h.c.execute("INSERT INTO users(email,encrypted_password,confirmed_at,created_at,updated_at)VALUES('alice@gmail.com',?1,'2026-01-01','2026-01-01','2026-01-01')",[deltabadger::crypto::hash_password("Correct-horse-9").unwrap()]).unwrap();
    let raw=" ALICE+TAG@googlemail.com ";let answer=h.submit("PATCH","/settings/update_email",&[("user[email]",raw),("user[current_password]","Correct-horse-9")],Csrf::Header).await;
    assert_eq!(answer.status,200);assert_eq!(h.c.query_row("SELECT unconfirmed_email FROM users WHERE id=?1",[h.seed.user_id],|r|r.get::<_,String>(0)).unwrap(),raw);assert!(h.mail.0.lock().unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn stored_credentials_in_query_parameters_are_omitted_from_language_links_and_refusal_pages(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for (url,status,has_links) in [("/tracker/add_api_key/new?exchange_id=1&key_type=trading&api_key%5Bsecret%5D=previous-secret&marker=keep",200,false),("/settings/connect?api_key%5Bsecret%5D=previous-secret&marker=keep",200,false),("/confirmation?confirmation_token=wrong&api_key%5Bsecret%5D=previous-secret&marker=keep",200,true),("/settings/unimplemented?api_key%5Bsecret%5D=previous-secret&marker=keep",501,true)]{
        let before=h.snapshot();let answer=h.browser.send(&h.app,"GET",url,None,Csrf::None,&[]).await;assert_eq!(answer.status,status);
        assert!(!answer.body.contains("previous-secret"),"I redacts credential query fields from rendered URLs as well as form values");if has_links{assert!(answer.body.contains("marker=keep"),"{url}: {}",answer.body);}assert_eq!(h.snapshot(),before);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor="current_thread")]
async fn job_connection_reports_failed_rollback_instead_of_hiding_it(){
    use rusqlite::hooks::{AuthAction,TransactionOperation,Authorization};
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let result:Result<(),String>=db.run(|c,_|{
        c.execute_batch("BEGIN IMMEDIATE; INSERT INTO app_configs(key,value,created_at,updated_at)VALUES('rollback_probe','x','2026-01-01','2026-01-01')").map_err(|e|e.to_string())?;
        c.authorizer(Some(|context:rusqlite::hooks::AuthContext<'_>|if matches!(context.action,AuthAction::Transaction{operation:TransactionOperation::Rollback}){Authorization::Deny}else{Authorization::Allow}));
        Err("original work failed".into())
    }).await;
    // Clean the owned connection even when the regression fails; no test leaves a lock held.
    db.run(|c,_|{c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>)->Authorization>);if !c.is_autocommit(){c.execute_batch("ROLLBACK").map_err(|e|e.to_string())?;}Ok(())}).await.unwrap();
    assert!(result.unwrap_err().contains("rollback"),"a failed write cleanup must be propagated");assert_eq!(h.c.query_row("SELECT COUNT(*) FROM app_configs WHERE key='rollback_probe'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
#[tokio::test(flavor="current_thread")]
async fn job_connection_does_not_claim_success_for_an_uncommitted_write(){
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let result=db.run(|c,_|{c.execute_batch("BEGIN IMMEDIATE; INSERT INTO app_configs(key,value,created_at,updated_at)VALUES('uncommitted_probe','x','2026-01-01','2026-01-01')").map_err(|e|e.to_string())?;Ok(())}).await;
    assert!(result.is_err(),"rolling an unfinished write back cannot be reported as a successful job unit");assert_eq!(h.c.query_row("SELECT COUNT(*) FROM app_configs WHERE key='uncommitted_probe'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
#[tokio::test(flavor="current_thread")]
async fn tracker_write_reports_failed_rollback_instead_of_hiding_it(){
    use rusqlite::hooks::{AuthAction,TransactionOperation,Authorization};use deltabadger::figures::FiguresError;
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let result:Result<(),FiguresError>=deltabadger::tracker::jobs::written(&h.c,&||h.app.now(),|c,_|{
        c.execute("INSERT INTO app_configs(key,value,created_at,updated_at)VALUES('tracker_rollback_probe','x','2026-01-01','2026-01-01')",[])?;
        c.authorizer(Some(|context:rusqlite::hooks::AuthContext<'_>|if matches!(context.action,AuthAction::Transaction{operation:TransactionOperation::Rollback}){Authorization::Deny}else{Authorization::Allow}));
        Err(FiguresError::Data("original work failed".into()))
    });
    h.c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>)->Authorization>);if !h.c.is_autocommit(){h.c.execute_batch("ROLLBACK").unwrap();}
    assert!(format!("{:?}",result.unwrap_err()).contains("rollback"),"a failed tracker write cleanup must be propagated");assert_eq!(h.c.query_row("SELECT COUNT(*) FROM app_configs WHERE key='tracker_rollback_probe'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

#[tokio::test(flavor="current_thread")]
async fn web_connection_reports_failed_rollback_instead_of_hiding_it(){
    use rusqlite::hooks::{AuthAction,TransactionOperation,Authorization};use deltabadger::web::WebError;
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let result:Result<(),WebError>=h.app.db(|c|{c.execute_batch("BEGIN IMMEDIATE; INSERT INTO app_configs(key,value,created_at,updated_at)VALUES('web_rollback_probe','x','2026-01-01','2026-01-01')")?;
        c.authorizer(Some(|context:rusqlite::hooks::AuthContext<'_>|if matches!(context.action,AuthAction::Transaction{operation:TransactionOperation::Rollback}){Authorization::Deny}else{Authorization::Allow}));Err(WebError::Config("original work failed".into()))}).await;
    h.app.db(|c|{c.authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>)->Authorization>);if !c.is_autocommit(){c.execute_batch("ROLLBACK")?;}Ok(())}).await.unwrap();
    assert!(format!("{:?}",result.unwrap_err()).contains("rollback"));assert_eq!(h.c.query_row("SELECT COUNT(*) FROM app_configs WHERE key='web_rollback_probe'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}
#[tokio::test(flavor="current_thread")]
async fn web_connection_does_not_claim_success_for_an_uncommitted_write(){
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let result=h.app.db(|c|{c.execute_batch("BEGIN IMMEDIATE; INSERT INTO app_configs(key,value,created_at,updated_at)VALUES('web_uncommitted_probe','x','2026-01-01','2026-01-01')")?;Ok(())}).await;
    h.app.db(|c|{if !c.is_autocommit(){c.execute_batch("ROLLBACK")?;}Ok(())}).await.unwrap();
    assert!(result.is_err());assert_eq!(h.c.query_row("SELECT COUNT(*) FROM app_configs WHERE key='web_uncommitted_probe'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
}

#[tokio::test(flavor="current_thread")]
async fn stored_query_credentials_are_not_echoed_by_the_after_login_redirect(){
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let mut browser=Browser::default();
    assert_eq!(browser.get(&h.app,"/settings/account?api_key%5Bsecret%5D=previous-secret&marker=keep").await.status,302);
    let saved=deltabadger::web::session::open(&h.app.keys.session,browser.cookie.as_ref().unwrap(),h.app.now()).unwrap();
    let return_to=saved.return_to.unwrap();assert!(!return_to.contains("previous-secret"));assert!(return_to.contains("marker=keep"));
    assert_eq!(browser.get(&h.app,"/login").await.status,200);
    let answer=browser.send(&h.app,"POST","/login",Some(&[("user[email]","o@example.com"),("user[password]","Correct-horse-9")]),Csrf::Form,&[]).await;
    assert_eq!(answer.status,303);let destination=answer.header("location").unwrap();
    assert!(!destination.contains("previous-secret"),"I must cover redirect headers as well as page markup");assert!(destination.contains("marker=keep"));assert!(server.received_requests().await.unwrap().is_empty());
}
#[tokio::test(flavor="current_thread")]
async fn q_rejected_placement_result_cannot_erase_a_after_an_external_credential_replacement(){
    use deltabadger::engine::{placement::{self,Sent},EngineError,FixedClock};
    let server=MockServer::start().await;Mock::given(method("POST")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({"message":"order refused"}))).expect(1).mount(&server).await;
    let h=Harness::at_real_now(server.uri()).await;let (bot,plan)=q_order(&h);let intent=placement::begin(&h.c,&bot,&plan,&FixedClock(h.app.now())).unwrap();
    let Sent::Rejected(errors)=placement::send(&q_venue(&h,&server.uri()),&intent,&FixedClock(h.app.now())).await else{panic!("fixture must obtain the actual rejected venue answer")};
    let before=h.snapshot();assert!(matches!(placement::record_rejected(&h.c,&bot,&intent,&errors),Err(EngineError::CredentialsChanged)));assert_eq!(h.snapshot(),before);server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn q_absent_recovery_result_cannot_erase_a_after_an_external_credential_replacement(){
    use deltabadger::engine::{placement,EngineError,FixedClock};
    let server=MockServer::start().await;Mock::given(method("GET")).and(path("/v2/orders:by_client_order_id")).respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({"code":40410000,"message":"order not found"}))).expect(1).mount(&server).await;
    let h=Harness::new(server.uri()).await;let (bot,plan)=q_order(&h);placement::begin(&h.c,&bot,&plan,&FixedClock(h.app.now()-chrono::Duration::hours(2))).unwrap();let bot=deltabadger::engine::model::load_bot(&h.c,bot.id).unwrap();
    let result=placement::recover(&h.c,&q_venue(&h,&server.uri()),&bot,&FixedClock(h.app.now())).await;
    assert!(matches!(result,Err(EngineError::CredentialsChanged)));assert!(deltabadger::engine::model::load_bot(&h.c,bot.id).unwrap().rust_placement().is_some());assert_eq!(h.c.query_row("SELECT COUNT(*) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),0);server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn owner_slice_refuses_other_venue_add_and_replace_routes_without_changes_or_network(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;h.link();
    for venue in ["Binance","BinanceUs","Bingx","Bitget","Bitmart","Bitrue","Bitvavo","Bybit","Coinbase","Gemini","Hyperliquid","Ibkr","Kraken","Kucoin","Mexc"]{
        h.c.execute("UPDATE exchanges SET type=?1,name=?2 WHERE id=1",(format!("Exchanges::{venue}"),venue)).unwrap();let before=h.snapshot();
        assert_eq!(h.browser.get(&h.app,"/tracker/add_api_key/new?exchange_id=1&key_type=trading").await.status,501);
        let modern=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","new-key"),("api_key[secret]","new-secret")],Csrf::Header).await;assert_eq!(modern.status,501);assert_eq!(h.snapshot(),before);
        let key=format!("0x{}","a".repeat(40));let secret="b".repeat(64);
        let legacy=h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]",&key),("api_key[secret]",&secret)],Csrf::Header).await;
        assert_eq!(legacy.status,501,"{venue} must visibly refuse instead of accepting a pending key with no validator");assert_eq!(h.snapshot(),before);
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn encoded_stored_credentials_are_removed_from_malformed_venue_diagnostics_on_every_boundary(){
    use deltabadger::{jobs::{Job,Cx,Db,Outcome},sync::jobs::Validator};
    let secret="Short\"key&?Ł🔑";let quoted=serde_json::to_string(secret).unwrap();let escaped=&quoted[1..quoted.len()-1];let percent=form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>();
    let ascii=escaped.replace('Ł',r"\u0141").replace('🔑',r"\ud83d\udd11");let lower=percent.replace("%C5%81","%c5%81").replace("%F0%9F%94%91","%f0%9f%94%91");
    let server=MockServer::start().await;Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(503).set_body_string(format!("malformed response contains {escaped} and {percent} and {ascii} and {lower}"))).expect(4).mount(&server).await;
    for boundary in ["modern","passive","job","sync"]{
        let mut h=Harness::new(server.uri()).await;h.c.execute("UPDATE api_keys SET status=0,secret=?1,access_token=?1,rsa_signature_key=?1,rsa_encryption_key=?1,dh_param=?1 WHERE id=?2",(h.app.cipher.encrypt(secret),h.seed.api_key_id)).unwrap();
        if boundary=="sync"{h.c.execute("UPDATE api_keys SET status=1 WHERE id=?1",[h.seed.api_key_id]).unwrap();}
        let diagnostic=match boundary{
            "modern"=>{assert_eq!(h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","new-key"),("api_key[secret]",secret),("api_key[passphrase]","paper")],Csrf::Header).await.status,422);h.logs.0.lock().unwrap().join("\n")},
            "passive"=>{assert_eq!(h.browser.get(&h.app,"/tracker/add_api_key/new?exchange_id=1&key_type=trading").await.status,200);h.logs.0.lock().unwrap().join("\n")},
            "job"=>{let cx=Cx{db:Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone()),clock:h.clock.as_ref(),wakers:h.app.job_wakers().unwrap()};let Outcome::Failed(diagnostic)=Validator{venues:LocalAlpaca(server.uri()),key_id:h.seed.api_key_id}.run(cx,vec![]).await else{panic!("actual malformed venue answer must remain a visible failure")};diagnostic},
            _=>{let db=Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());let credentials=deltabadger::crypto::Credentials{ redaction_values:vec![],key:"previous-key".into(),secret:secret.into(),passphrase:Some("paper".into())};let transport=deltabadger::venue::http::ReqwestTransport::new(deltabadger::venue::http::client(),credentials.key.clone(),credentials.secret.clone());let venue=deltabadger::venue::alpaca::AlpacaVenue::new(transport,deltabadger::venue::alpaca::Urls{trading:server.uri(),data:server.uri()});let error=deltabadger::sync::balances::sync(&db,&venue,&deltabadger::sync::balances::NoPrices,h.seed.api_key_id,&credentials,&deltabadger::engine::FixedClock(h.app.now())).await.unwrap().unwrap_err();let stored:String=h.c.query_row("SELECT last_sync_error FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();format!("{} {stored}",error.error)},
        };
        assert!(if boundary=="sync"{diagnostic.contains(deltabadger::crypto::VENUE_TEXT_REDACTED)}else{diagnostic.contains("HTTP 503")&&!diagnostic.contains(deltabadger::crypto::VENUE_TEXT_REDACTED)},"R1 log metadata and stored sync errors have their respective contracts for {boundary}");assert!(!diagnostic.contains(secret)&&!diagnostic.contains(escaped)&&!diagnostic.contains(&percent)&&!diagnostic.contains(&ascii)&&!diagnostic.contains(&lower),"I removes recoverable encoded credential material from {boundary} diagnostics");
    }
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn stored_query_credentials_are_not_echoed_by_the_tax_referer_redirect(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    let answer=h.browser.send(&h.app,"POST","/settings/update_wash_sale",Some(&[("_method","patch"),("wash_sale[enabled]","0"),("wash_sale[jurisdiction]","US")]),Csrf::Header,&[("referer","http://localhost:3000/settings/account?api_key%5Bsecret%5D=previous-secret&marker=keep")]).await;
    assert_eq!(answer.status,303);let destination=answer.header("location").unwrap();assert!(!destination.contains("previous-secret"));assert!(destination.contains("marker=keep"));assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn successful_legacy_replacement_retains_credential_identity_and_every_history_link(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;h.link();
    h.c.execute("UPDATE api_keys SET key_type=0,access_token=?1,rsa_signature_key=?1,rsa_encryption_key=?1,dh_param=?1 WHERE id=?2",(h.app.cipher.encrypt("retained-extra"),h.seed.api_key_id)).unwrap();
    let before=h.snapshot();
    let answer=h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","replacement-key"),("api_key[secret]","replacement-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(answer.status,201);let after=h.snapshot();
    assert_eq!(after["account_transactions"],before["account_transactions"],"G retains history links even when the replacement succeeds");
    assert_eq!(after["api_keys"].as_array().unwrap().len(),1);assert_eq!(after["api_keys"][0]["id"],before["api_keys"][0]["id"]);
    assert_eq!(after["api_keys"][0]["created_at"],before["api_keys"][0]["created_at"]);
    for column in ["access_token","rsa_signature_key","rsa_encryption_key","dh_param"]{assert_eq!(after["api_keys"][0][column],before["api_keys"][0][column],"an unsent encrypted field is retained: {column}");}
    for (column,expected) in [("key","replacement-key"),("secret","replacement-secret"),("passphrase","paper")]{let stored:String=h.c.query_row(&format!("SELECT {column} FROM api_keys WHERE id=?1"),[h.seed.api_key_id],|r|r.get(0)).unwrap();assert_eq!(h.app.cipher.decrypt(&stored).unwrap(),expected);}
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn legacy_same_key_revalidation_keeps_the_stored_realm_as_rails_does(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET key_type=0,passphrase=?1 WHERE id=?2",(h.app.cipher.encrypt("live"),h.seed.api_key_id)).unwrap();
    let answer=h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","previous-key"),("api_key[secret]","previous-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(answer.status,201);let stored:String=h.c.query_row("SELECT passphrase FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();assert_eq!(h.app.cipher.decrypt(&stored).unwrap(),"live","Rails same-key revalidation keeps the stored realm; engine still refuses it");
    assert!(server.received_requests().await.unwrap().is_empty());
}

async fn ledger_progress_after_rotation(partial:bool, unknown:bool, empty:bool){
    use deltabadger::{engine::FixedClock,sync::{self,ledger},venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}}};
    use wiremock::matchers::header;
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let count=if partial{100}else{1};
    let activities:Vec<_>=(0..count).map(|i|serde_json::json!({"id":format!("a-{i:03}"),"activity_type":"INT","net_amount":"0.25","date":"2026-09-09"})).collect();
    Mock::given(method("GET")).and(path("/v2/account/activities")).and(header("APCA-API-KEY-ID","previous-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(activities)).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).and(header("APCA-API-KEY-ID","account-b-key"))
        .respond_with(move |r:&wiremock::Request|{let continued=r.url.query_pairs().any(|(k,_)|k=="after"||k=="page_token");ResponseTemplate::new(200).set_body_json(if continued||empty{serde_json::json!([])}else{serde_json::json!([{"id":"b-old","activity_type":"INT","net_amount":"0.07","date":"2020-01-01"}])})}).mount(&server).await;
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let limits=ledger::Limits{pages:1,runs:10};let clock=FixedClock(h.app.now());
    let a=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
    let av=AlpacaVenue::new(ReqwestTransport::new(http::client(),a.key.clone(),a.secret.clone()),Urls{trading:server.uri(),data:server.uri()});
    if unknown{h.c.execute("UPDATE api_keys SET last_synced_at='2026-09-09 00:00:00' WHERE id=?1",[h.seed.api_key_id]).unwrap();}
    else{
        let out=ledger::sync_within(&db,&av,h.seed.api_key_id,&a,&clock,limits).await.unwrap().unwrap();assert_eq!(out.imported,count);assert_eq!(out.complete,!partial);
        if !partial{ledger::sync_within(&db,&av,h.seed.api_key_id,&a,&clock,limits).await.unwrap().unwrap();let requests=server.received_requests().await.unwrap();assert!(requests.last().unwrap().url.query_pairs().any(|(k,_)|k=="after"),"unchanged A keeps its incremental watermark");}
    }
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();
    let b=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();let bv=AlpacaVenue::new(ReqwestTransport::new(http::client(),b.key.clone(),b.secret.clone()),Urls{trading:server.uri(),data:server.uri()});
    let out=ledger::sync_within(&db,&bv,h.seed.api_key_id,&b,&clock,limits).await.unwrap().unwrap();
    assert_eq!(out.imported,if empty{0}else{1},"B must read its older history without A's cursor or watermark");assert!(out.complete);
    let preserved:i64=h.c.query_row("SELECT count(*) FROM account_transactions WHERE tx_id LIKE 'a-%'",[],|r|r.get(0)).unwrap();assert_eq!(preserved,if unknown{0}else{count as i64},"A's stored history remains linked and retained");
    let watermark:Option<String>=h.c.query_row("SELECT last_synced_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();if empty{assert!(watermark.is_none(),"an empty B ledger clears A's watermark before recording B's producer");}else{assert!(watermark.unwrap().starts_with("2020-01-01"),"B cannot commit A's saved maximum date");}
    let requests=server.received_requests().await.unwrap();let last=requests.last().unwrap();assert!(!last.url.query_pairs().any(|(k,_)|k=="after"||k=="page_token"));
}
#[tokio::test(flavor="current_thread")]
async fn r_ledger_watermark_must_restart_after_rotation(){ledger_progress_after_rotation(false,false,false).await;}
#[tokio::test(flavor="current_thread")]
async fn r_ledger_cursor_must_restart_after_rotation(){ledger_progress_after_rotation(true,false,false).await;}
#[tokio::test(flavor="current_thread")]
async fn r_ledger_unknown_watermark_must_restart_after_rotation(){ledger_progress_after_rotation(false,true,false).await;}

#[tokio::test(flavor="current_thread")]
async fn r_legacy_replacement_clears_the_previous_accounts_diagnostic(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET last_sync_error='account A rejected its credentials' WHERE id=?1",[h.seed.api_key_id]).unwrap();
    assert_eq!(h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","replacement-key"),("api_key[secret]","replacement-secret"),("api_key[passphrase]","paper")],Csrf::Header).await.status,201);
    let diagnostic:Option<String>=h.c.query_row("SELECT last_sync_error FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();assert!(diagnostic.is_none(),"B's pending validation cannot display A's diagnostic as current");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn r_ledger_empty_replacement_clears_a_watermark(){ledger_progress_after_rotation(false,false,true).await;}

// Q: count actual production log lines and prove the next poll/reconciliation uses B.
async fn credential_change_loop_probe(idle: bool) {
    use deltabadger::{engine::{model,placement,run::{self,Engine},FixedClock},venue::{VenueFactory,alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport,Transport,HttpRequest,HttpResponse,TransportError}}};
    use wiremock::matchers::header;
    struct Once { inner:ReqwestTransport,file:std::path::PathBuf,cipher:deltabadger::crypto::Cipher,key_id:i64,rotate:Arc<std::sync::atomic::AtomicBool> }
    impl Transport for Once {
        async fn send(&self,r:&HttpRequest)->Result<HttpResponse,TransportError>{
            let response=self.inner.send(r).await?;
            if self.rotate.swap(false,std::sync::atomic::Ordering::SeqCst){
                let c=rusqlite::Connection::open(&self.file).unwrap();
                c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(self.cipher.encrypt("externally-replaced-key"),self.cipher.encrypt("externally-replaced-secret"),self.key_id)).unwrap();
            }
            Ok(response)
        }
    }
    struct Factory { file:std::path::PathBuf,cipher:deltabadger::crypto::Cipher,key_id:i64,url:String,rotate:Arc<std::sync::atomic::AtomicBool> }
    impl VenueFactory for Factory {
        type V=AlpacaVenue<Once>;
        fn for_bot(&self,kind:&str,credentials:Option<deltabadger::crypto::Credentials>)->Self::V{
            assert_eq!(kind,"Exchanges::Alpaca");let c=credentials.unwrap();
            AlpacaVenue::new(Once{inner:ReqwestTransport::new(http::client(),c.key,c.secret),file:self.file.clone(),cipher:self.cipher.clone(),key_id:self.key_id,rotate:self.rotate.clone()},Urls{trading:self.url.clone(),data:self.url.clone()})
        }
    }
    let server=MockServer::start().await;let h=Harness::at_real_now(server.uri()).await;
    let at=h.app.now();let (bot,plan)=q_order(&h);let intent=placement::begin(&h.c,&bot,&plan,&FixedClock(at)).unwrap();
    if !idle{placement::record_accepted(&h.c,&bot,&intent,"q-order").unwrap();}
    h.c.execute("UPDATE bots SET status=2 WHERE id=?1",[bot.id]).unwrap();
    let endpoint=if idle{"/v2/orders:by_client_order_id"}else{"/v2/orders/q-order"};
    let mut fill=q_fill();fill["client_order_id"]=serde_json::json!(intent.cl_ord_id);
    Mock::given(method("GET")).and(path(endpoint)).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(fill.clone())).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path(endpoint)).and(header("APCA-API-KEY-ID","externally-replaced-key")).respond_with(ResponseTemplate::new(200).set_body_json(fill)).expect(1).mount(&server).await;
    let paths=deltabadger::store::Paths::from_env(&|_|None,h._dir.path());let lock=deltabadger::lease::lock(&paths,at).unwrap();
    let factory=Factory{file:h._dir.path().join("production.sqlite3"),cipher:h.app.cipher.clone(),key_id:h.seed.api_key_id,url:server.uri(),rotate:Arc::new(std::sync::atomic::AtomicBool::new(true))};
    let mut engine=Engine::new(rusqlite::Connection::open(&paths.primary).unwrap(),factory,h.app.cipher.clone(),lock);
    let wake=run::step(&mut engine,&FixedClock(at)).await.unwrap();
    println!("S1_RETRY_US={}",wake-at.timestamp_micros());
    if idle{assert!(model::load_bot(&h.c,bot.id).unwrap().rust_placement().is_some());assert_eq!(h.c.query_row("SELECT count(*) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),0);}
    else{assert_eq!(h.c.query_row("SELECT external_status FROM transactions WHERE external_id='q-order'",[],|r|r.get::<_,i64>(0)).unwrap(),0);}
    run::step(&mut engine,&FixedClock(at+chrono::Duration::seconds(31))).await.unwrap();
    assert_eq!(h.c.query_row("SELECT external_status FROM transactions WHERE external_id='q-order'",[],|r|r.get::<_,i64>(0)).unwrap(),2,"B's fresh response is committed");
    assert!(model::load_bot(&h.c,bot.id).unwrap().rust_placement().is_none());server.verify().await;
}
async fn checked_credential_change_loop_probe(name:&str,idle:bool){
    if std::env::var("S1_CREDENTIAL_LOG_CHILD").as_deref()==Ok(name){credential_change_loop_probe(idle).await;return;}
    let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact",name,"--nocapture"]).env("S1_CREDENTIAL_LOG_CHILD",name).output().unwrap();
    assert!(output.status.success(),"native loop probe failed: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    let text=String::from_utf8(output.stdout).unwrap();
    let logs=text.lines().filter(|line|line.contains("credentials changed")||line.contains("CredentialsChanged")).count();
    assert_eq!(logs,1,"Q announces this discarded venue result once: {text}");
    let retry=text.lines().find_map(|line|line.strip_prefix("S1_RETRY_US=")).unwrap().parse::<i64>().unwrap();
    assert_eq!(retry,1,"Q retries with fresh credentials on the next engine pass");
}
#[tokio::test(flavor="current_thread")]
async fn q_changed_poll_logs_once_and_retries_with_fresh_credentials(){checked_credential_change_loop_probe("q_changed_poll_logs_once_and_retries_with_fresh_credentials",false).await;}
#[tokio::test(flavor="current_thread")]
async fn q_changed_idle_recovery_logs_once_and_retries_with_fresh_credentials(){checked_credential_change_loop_probe("q_changed_idle_recovery_logs_once_and_retries_with_fresh_credentials",true).await;}

// I: the real engine's diagnostics, persisted activity and retry state are all secret sinks.
#[tokio::test(flavor="current_thread")]
async fn i_engine_diagnostics_redact_all_stored_material_in_every_encoding(){
    use deltabadger::{engine::{run::{self,Engine},FixedClock},venue::alpaca::LiveFactory};
    if let Ok(kind)=std::env::var("S1_ENGINE_DIAGNOSTIC_CHILD"){
        let server=MockServer::start().await;let h=Harness::at_real_now(server.uri()).await;
        let values=["engine-quoted-\"key","engine-backslash-\\secret","unused-access-🗝","unused-signing-\"material","unused-encryption-\\material","unused-dh-🗝","paper"];
        h.c.execute("UPDATE api_keys SET key=?1,secret=?2,access_token=?3,rsa_signature_key=?4,rsa_encryption_key=?5,dh_param=?6 WHERE id=?7",(h.app.cipher.encrypt(values[0]),h.app.cipher.encrypt(values[1]),h.app.cipher.encrypt(values[2]),h.app.cipher.encrypt(values[3]),h.app.cipher.encrypt(values[4]),h.app.cipher.encrypt(values[5]),h.seed.api_key_id)).unwrap();
        let encode=|value:&str|match kind.as_str(){
            "raw"=>value.to_string(),
            "partial"=>value.replace('-',"%2D"),
            "double"=>value.replace('-',"%252d"),
            "form"=>value.replace('-',"%2D").replace(' ',"+"),
            "html"=>value.replace('-',"&#x2D;"),
            "url"=>value.as_bytes().iter().map(|byte|if byte.is_ascii_alphanumeric()||b"-._~".contains(byte){(*byte as char).to_string()}else{format!("%{byte:02X}")}).collect::<String>(),
            "json"=>serde_json::to_string(value).unwrap().trim_matches('"').to_string(),
            "ascii"=>serde_json::to_string(value).unwrap().trim_matches('"').chars().map(|ch|if ch.is_ascii(){ch.to_string()}else{ch.encode_utf16(&mut [0u16;2]).iter().map(|unit|format!(r"\u{unit:04x}")).collect::<String>()}).collect::<String>(),
            _=>panic!("unknown test encoding"),
        };
        let at=h.app.now();let paths=deltabadger::store::Paths::from_env(&|_|None,h._dir.path());let lock=deltabadger::lease::lock(&paths,at).unwrap();
        let mut engine=Engine::new(rusqlite::Connection::open(&paths.primary).unwrap(),LiveFactory::with_paper_boundary(server.uri()),h.app.cipher.clone(),lock);
        for (column,value) in values.iter().enumerate(){
            server.reset().await;let material=encode(value);let message=format!("engine-diagnostic-visible {material}");
            Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"message":message}))).expect(4).mount(&server).await;
            h.clock.set(at+chrono::Duration::seconds(column as i64*1500));let (bot,_)=q_order(&h);
            for pass in 0..4 {run::step(&mut engine,&FixedClock(h.app.now()+chrono::Duration::seconds(pass*300))).await.unwrap();}
            server.verify().await;
            let retry:String=h.c.query_row("SELECT transient_data FROM bots WHERE id=?1",[bot.id],|r|r.get(0)).unwrap();
            let mut statement=h.c.prepare("SELECT details || COALESCE(message,'') FROM bot_activity_logs WHERE bot_id=?1 ORDER BY id").unwrap();let rows=statement.query_map([bot.id],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
            let messages=rows.iter().map(|row|serde_json::from_str::<serde_json::Value>(row).unwrap()["error"].as_str().unwrap_or("").to_string()).collect::<Vec<_>>();let stored=format!("{retry} {}",messages.join(" "));assert!(stored.contains(deltabadger::crypto::VENUE_TEXT_REDACTED),"R2 discards the whole reflected diagnostic for column {column}: {kind}");
            assert!(!stored.contains(&material),"I removes each stored credential representation from activity and retry state: {column} {kind}");
        }
        return;
    }
    for kind in ["raw","json","ascii","url","partial","double","form","html"]{
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","i_engine_diagnostics_redact_all_stored_material_in_every_encoding","--nocapture"]).env("S1_ENGINE_DIAGNOSTIC_CHILD",kind).output().unwrap();
        assert!(output.status.success(),"real engine diagnostic probe failed: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        let text=String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("engine-quoted")&&!text.contains("engine-backslash")&&!text.contains("unused-access")&&!text.contains("unused-signing")&&!text.contains("unused-encryption")&&!text.contains("unused-dh"),"I removes every stored credential from production engine logs: {kind}: {text}");
    }
}


#[tokio::test(flavor="current_thread")]
async fn r1_ruby_name_whitespace_is_ascii() {
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for name in ["Alice\u{a0}Smith","Alice\u{2003}Smith"] {
        let before=h.snapshot();
        let answer=h.submit("PATCH","/settings/update_name",&[("user[name]",name)],Csrf::Header).await;
        assert_eq!(answer.status,422,"R1 Ruby rejects this Unicode separator");
        assert_eq!(h.snapshot(),before,"R1 rejected names leave every row unchanged");
    }
    assert_eq!(h.submit("PATCH","/settings/update_name",&[("user[name]","Alice Smith")],Csrf::Header).await.status,200);
}

#[tokio::test(flavor="current_thread")]
async fn r1_validation_logs_no_venue_body_in_any_encoding() {
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    let variants=["previous-secret","previous%2Dsecret","previous%2dsecret",r"previous\u002dsecret","previous%252Dsecret","previous%2Dse%63ret","previous%2dse%63ret"];
    for variant in variants {
        server.reset().await;
        Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"code":50310000,"message":format!("venue-free-text {variant}")}))).mount(&server).await;
        let response=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","candidate-key"),("api_key[secret]","candidate-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
        assert_eq!(response.status,422);
        let logs=h.logs.0.lock().unwrap().join("\n");
        assert!(!logs.contains(variant)&&!logs.contains("venue-free-text"),"R1 free-text bodies are never logged: {variant}");
        assert!(logs.contains("Alpaca")&&logs.contains("503")&&logs.contains("50310000"),"R1 retains structured metadata");
    }
}

#[tokio::test(flavor="current_thread")]
async fn r1_snapshot_replacement_after_calculation_does_not_commit() {
    use deltabadger::{engine::model,tracker::{snapshot,jobs},figures::dec::Dec};
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let owner=h.seed.user_id;let date=h.app.now().date_naive();
    let origin=deltabadger::sync::cache::capture_read(&h.c,owner,None).unwrap();
    let rows=vec![(None,snapshot::Day{value:Dec::strict("10000").unwrap(),invested:Dec::zero(),held_value:None,held_cost:None,partial:false})];
    h.c.execute("UPDATE api_keys SET secret=?1 WHERE id=?2",(h.app.cipher.encrypt("replacement-B"),h.seed.api_key_id)).unwrap();
    assert!(!deltabadger::sync::cache::read_is_current(&h.c,&origin).unwrap());
    let out=jobs::written_for(&h.c,&origin,&||h.app.now(),|c,_|snapshot::write(c,owner,&rows,date));
    assert!(out.is_err(),"R1 A's complete $10,000 snapshot must be discarded inside its writing transaction");
    assert_eq!(h.c.query_row("SELECT count(*) FROM portfolio_snapshots",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    let _=model::credential_version_by_id(&h.c,h.seed.api_key_id).unwrap();
}

// The replacement transaction commits only after the writer reaches SQLite's busy handler.
// This orders the race without sleeps: a check outside the write lock still observes A.
#[tokio::test(flavor="current_thread")]
async fn r1_snapshot_checks_provenance_after_waiting_for_the_write_lock(){
    use deltabadger::{tracker::{snapshot,jobs},figures::dec::Dec,sync::cache};
    static RELEASE:std::sync::OnceLock<std::sync::mpsc::Sender<()>>=std::sync::OnceLock::new();
    fn busy(_:i32)->bool{let _=RELEASE.get().unwrap().send(());true}
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let origin=cache::capture_read(&h.c,h.seed.user_id,None).unwrap();
    let rows=vec![(None,snapshot::Day{value:Dec::strict("10000").unwrap(),invested:Dec::zero(),held_value:None,held_cost:None,partial:false})];
    let file=h._dir.path().join("production.sqlite3");let ciphertext=h.app.cipher.encrypt("replacement-B");
    let (ready,held)=std::sync::mpsc::channel();let (release,wait)=std::sync::mpsc::channel();RELEASE.set(release).unwrap();
    let writer=std::thread::spawn(move||{
        let c=rusqlite::Connection::open(file).unwrap();let tx=c.unchecked_transaction().unwrap();
        tx.execute("UPDATE api_keys SET secret=?1 WHERE id=1",[ciphertext]).unwrap();ready.send(()).unwrap();
        wait.recv_timeout(std::time::Duration::from_secs(10)).unwrap();tx.commit().unwrap();
    });
    held.recv_timeout(std::time::Duration::from_secs(10)).unwrap();h.c.busy_handler(Some(busy)).unwrap();
    let out=jobs::written_for(&h.c,&origin,&||h.app.now(),|c,now|snapshot::write(c,h.seed.user_id,&rows,now.date_naive()));
    writer.join().unwrap();h.c.busy_handler(None).unwrap();
    assert!(out.is_err(),"R1 the digest must be re-read after acquiring the snapshot write lock");
    assert_eq!(h.c.query_row("SELECT count(*) FROM portfolio_snapshots",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    let fresh=cache::capture_read(&h.c,h.seed.user_id,None).unwrap();
    jobs::written_for(&h.c,&fresh,&||h.app.now(),|c,now|snapshot::write(c,h.seed.user_id,&rows,now.date_naive())).unwrap();
    assert_eq!(h.c.query_row("SELECT count(*) FROM portfolio_snapshots",[],|r|r.get::<_,i64>(0)).unwrap(),1,"unchanged producer control still commits");
    let origin_key=deltabadger::jobs::state::key("snapshot_origin",Some(&format!("{}:all:{}",h.seed.user_id,h.app.now().date_naive())));
    let stamp=deltabadger::app_config::get_plain(&h.c,&origin_key).unwrap();
    assert!(stamp.is_some(),"R1 each snapshot records its checked producer in the same transaction");
    let stamp:serde_json::Value=serde_json::from_str(&stamp.unwrap()).unwrap();
    assert_eq!(stamp.as_array().unwrap().len(),1);assert_eq!(stamp[0]["key_id"],1);
    assert_eq!(stamp[0]["ciphertext_digest"].as_str().unwrap().len(),64);

}

#[tokio::test(flavor="current_thread")]
async fn r1_sync_and_validation_scheduler_logs_only_structured_diagnostics(){
    use deltabadger::{jobs::{self,Job,state},sync::{balances::NoPrices,jobs::{LedgerSync,BalanceSync,Validator}}};
    let variants=["previous-secret","previous%2Dsecret","previous%2dsecret",r"previous\u002dsecret","previous%252Dsecret","previous%2Dse%63ret","previous%2dse%63ret"];
    if let Ok(kind)=std::env::var("S1_R1_LOG_CHILD"){
        let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
        let text=format!("venue-free-text {}",variants.join(" | "));
        let endpoint=if kind=="ledger"{"/v2/account/activities"}else{"/v2/account"};
        Mock::given(method("GET")).and(path(endpoint)).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"message":text,"code":50310000}))).mount(&server).await;
        let job:Box<dyn Job>=match kind.as_str(){"ledger"=>Box::new(LedgerSync::new(LocalAlpaca(server.uri()),1)),"balance"=>Box::new(BalanceSync::new(LocalAlpaca(server.uri()),std::rc::Rc::new(NoPrices),1)),_=>Box::new(Validator{venues:LocalAlpaca(server.uri()),key_id:1})};
        let spec=job.spec();let name=spec.name;let scope=spec.scope.clone();
        let scheduler=jobs::Scheduler::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone(),vec![job],None);
        scheduler.wakers().wake(name,scope.as_deref(),None);
        let (stop,rx)=tokio::sync::watch::channel(false);
        let control=async{loop{
            match state::read(&h.c,name,scope.as_deref()){
                Ok(state) if state.last_error.is_some()=>break,
                Ok(_)=>{},
                Err(error) if error.contains("database is locked")=>{},
                Err(error)=>panic!("fixture state read failed: {error}"),
            }
            tokio::task::yield_now().await;
        }stop.send(true).unwrap();};
        tokio::time::timeout(std::time::Duration::from_secs(15),async{let (out,())=tokio::join!(scheduler.run(rx,h.clock.as_ref()),control);out.unwrap();}).await.unwrap();
        assert!(state::read(&h.c,name,scope.as_deref()).unwrap().last_error.is_some(),"real failed job ran");
        if kind=="ledger"{
            h.c.execute("UPDATE assets SET category='Stock' WHERE id=?1",[h.seed.btc]).unwrap();
            let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
            let stale=deltabadger::engine::staleness::ledger_stale(&h.c,&deltabadger::engine::model::load_bot(&h.c,bot).unwrap(),h.app.now()).unwrap().unwrap();
            deltabadger::engine::log(&stale.message);
        }
        return;
    }
    for kind in ["ledger","balance","validator"]{
        let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","r1_sync_and_validation_scheduler_logs_only_structured_diagnostics","--nocapture"]).env("S1_R1_LOG_CHILD",kind).output().unwrap();
        assert!(output.status.success(),"real scheduler probe failed: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        let logs=format!("{} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        assert!(!logs.contains("venue-free-text")&&variants.iter().all(|v|!logs.contains(v)),"R1 raw and encoded venue bodies absent from {kind} logs: {logs}");
        assert!(logs.contains("Alpaca")&&logs.contains("503")&&logs.contains("50310000"),"R1 log keeps structured {kind} metadata");
    }
}

#[tokio::test(flavor="current_thread")]
async fn r1_each_sync_kind_checks_after_waiting_for_the_write_lock(){
    use deltabadger::{engine::model,sync::{self,cache},jobs::Db,tracker::prices,web::settings::keys};
    use std::sync::atomic::{AtomicBool,Ordering};
    static RELEASE:std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>=std::sync::Mutex::new(None);
    fn busy(_:i32)->bool{if let Some(sender)=RELEASE.lock().unwrap().as_ref(){let _=sender.send(());}true}
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({}))).expect(1).mount(&server).await;
    for kind in ["balance","ledger","status","bars"]{
        let version=model::credential_version_by_id(&h.c,1).unwrap().unwrap();
        let status=if kind=="status"{
            use deltabadger::jobs::Job;
            let job=sync::jobs::Validator{venues:LocalAlpaca(server.uri()),key_id:1};
            let captured=job.run_attributed(deltabadger::jobs::Cx{db:Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone()),clock:h.clock.as_ref(),wakers:Default::default()},vec![]).await;
            assert!(captured.origin().is_some(),"fixture forwards the actual validation job producer");
            Some(captured.map(|_|2))
        }else{None};
        let writer_file=h._dir.path().join("production.sqlite3");let ciphertext=h.app.cipher.encrypt(&format!("replacement-{kind}"));
        let connection=rusqlite::Connection::open(&writer_file).unwrap();connection.busy_handler(Some(busy)).unwrap();
        let db=Db::new(connection,h.app.cipher.clone());
        let (ready,held)=std::sync::mpsc::channel();let (release,wait)=std::sync::mpsc::channel();*RELEASE.lock().unwrap()=Some(release);
        let writer=std::thread::spawn(move||{let c=rusqlite::Connection::open(writer_file).unwrap();let tx=c.unchecked_transaction().unwrap();tx.execute("UPDATE api_keys SET secret=?1 WHERE id=1",[ciphertext]).unwrap();ready.send(()).unwrap();wait.recv_timeout(std::time::Duration::from_secs(10)).unwrap();tx.commit().unwrap();});
        held.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let invoked=Arc::new(AtomicBool::new(false));let entered=invoked.clone();let origin=version.clone();let now=h.app.now();
        // The result has been calculated already; these are the actual production commit writers.
        let out=sync::commit_for(&db,&version,move|c|{
            entered.store(true,Ordering::SeqCst);
            match kind{
                "balance"=>cache::record(c,1,1,&origin,true,now).map_err(Into::into),
                "ledger"=>cache::record_ledger(c,1,&origin,now).map_err(Into::into),
                "status"=>keys::store_status(c,status.as_ref().unwrap(),1,now),
                _=>prices::store_bars(c,&[]).map_err(|_|sync::SyncError("bar write failed".into()))
            }
        }).await;
        writer.join().unwrap();
        assert!(out.is_err(),"R1 {kind} checks the captured digest inside the writing transaction");
        assert!(!invoked.load(Ordering::SeqCst),"R1 {kind} must not enter the writer after replacement");
    }
}


#[test]
fn r2_repeated_decoding_closes_partly_encoded_secret_class(){
    let variants=["previous-secret","previous%2Dsecret","previous%2dsecret",r"previous\u002dsecret","previous&#45;secret","previous%252Dsecret","previous%25252dsecret"];
    for value in variants{
        let got=deltabadger::crypto::scrub_known(&format!("venue: {value}"),&["previous-secret"]);
        assert_eq!(got,"Venue diagnostic omitted to protect stored credentials.","R2 whole-text refusal for {value}");
    }
    for (secret,text) in [("previous secret","previous+secret"),("previous&secret","previous&amp;secret"),("previousésecret",r"previous\u00e9secret"),("previousésecret","previous%C3%A9secret"),("previous🔑secret",r"previous\ud83d\udd11secret")]{
        assert_eq!(deltabadger::crypto::scrub_known(text,&[secret]),"Venue diagnostic omitted to protect stored credentials.","R2 JSON/HTML/form decoding: {text}");
    }
    let got=deltabadger::crypto::scrub_known("normal venue message",&["previous-secret"]);
    assert_eq!(got,"normal venue message","ordinary text unchanged");
}
#[tokio::test(flavor="current_thread")]
async fn r2_confirmation_normalizes_and_validates_pending_email(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    let other=h.app.cipher.encrypt("not-used");drop(other);
    let hash=deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    h.c.execute("INSERT INTO users(name,email,encrypted_password,confirmed_at,created_at,updated_at,time_zone,display_currency)VALUES('Other','taken@example.com',?1,'2026-01-01','2026-01-01','2026-01-01','UTC','USD')",[hash]).unwrap();
    assert_eq!(h.submit("PATCH","/settings/update_email",&[("user[email]"," TAKEN@example.com "),("user[current_password]","Correct-horse-9")],Csrf::Header).await.status,200);
    let pending:String=h.c.query_row("SELECT unconfirmed_email FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();assert_eq!(pending," TAKEN@example.com ");
    h.browser.get(&h.app,"/settings/account").await;
    assert_eq!(h.submit("POST","/confirmation",&[("user[email]","o@example.com")],Csrf::Header).await.status,303);
    let token:String=h.c.query_row("SELECT confirmation_token FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();
    assert_eq!(h.mail.0.lock().unwrap().last().unwrap().to," TAKEN@example.com ");
    let before=h.snapshot();let token:String=form_urlencoded::byte_serialize(token.as_bytes()).collect();
    let answer=h.browser.get(&h.app,&format!("/confirmation?confirmation_token={token}")).await;
    assert_eq!(answer.status,200,"taken normalized address must be refused");assert_eq!(h.snapshot(),before,"R2 failed confirmation leaves all stored rows untouched");
    h.c.execute("DELETE FROM users WHERE email='taken@example.com'",[]).unwrap();
    assert_eq!(h.browser.get(&h.app,&format!("/confirmation?confirmation_token={token}")).await.status,302);
    let email:String=h.c.query_row("SELECT email FROM users WHERE id=?1",[h.seed.user_id],|r|r.get(0)).unwrap();assert_eq!(email,"taken@example.com","R2 successful confirmation strips and downcases");
    h.c.execute("UPDATE users SET unconfirmed_email='broken-address',confirmation_token='abcdefghijklmnopqrst' WHERE id=?1",[h.seed.user_id]).unwrap();
    let before=h.snapshot();assert_eq!(h.browser.get(&h.app,"/confirmation?confirmation_token=abcdefghijklmnopqrst").await.status,200);assert_eq!(h.snapshot(),before,"User email format validations run at confirmation");
}
#[tokio::test(flavor="current_thread")]
async fn r2_json_api_key_create_and_validation_failure(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for (body,status) in [(serde_json::json!({"api_key":{"exchange_id":1,"key_type":"read_only","key":"json-key","secret":"json-secret","passphrase":"paper"}}),201),(serde_json::json!({"api_key":{"exchange_id":"999999","key":"json-key","secret":"json-secret"}}),422)]{
        h.browser.get(&h.app,"/settings/account").await;let before=h.snapshot();
        let response=h.browser.send_body(&h.app,"POST","/api/api_keys",Some(body.to_string()),Csrf::Header,&[("content-type","application/json"),("origin","http://localhost:3000")]).await;
        assert_eq!(response.status,status,"Rails accepts nested JSON parameters with CSRF header");
        assert_eq!(response.body,format!("{{\"data\":{}}}",status==201));
        if status==422{assert_eq!(h.snapshot(),before);}else{let saved:(String,String)=h.c.query_row("SELECT key,secret FROM api_keys WHERE key_type=2",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();assert_eq!(h.app.cipher.decrypt(&saved.0).unwrap(),"json-key");assert_eq!(h.app.cipher.decrypt(&saved.1).unwrap(),"json-secret");}
    }
}


fn r2_variants(secret:&str)->Vec<String>{
    let json=secret.chars().map(|ch|if ch=='-'{r"\u002d".into()}else if ch=='"'{"\\\"".into()}else{ch.to_string()}).collect::<String>();
    let full=secret.as_bytes().iter().map(|byte|format!("%{byte:02X}")).collect::<String>();
    vec![secret.into(),secret.replace('-',"%2D"),secret.replace('-',"%2d"),json,secret.replace('-',"&#45;"),secret.replace('-',"&#x2D;"),secret.replace('-',"%252D"),full,secret.replace('-',"%25252d"),secret.replace(' ',"+"),secret.replace('-',"%26%2345%3B")]
}
#[tokio::test(flavor="current_thread")]
async fn r2_stored_errors_and_mcp_answers_check_every_column_and_encoding(){
    use deltabadger::{sync,engine::model,web::{figure::loading::Source,mcp::reads}};
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    let columns=["key","secret","passphrase","access_token","rsa_signature_key","rsa_encryption_key","dh_param"];
    let material=["previous-key","previous-secret","paper","previous-access","previous-signing","previous-encryption","previous-dh"];
    for (column,value) in columns.iter().zip(material){h.c.execute(&format!("UPDATE api_keys SET {column}=?1 WHERE id=?2"),(h.app.cipher.encrypt(value),h.seed.api_key_id)).unwrap();}
    let credentials=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
    assert_eq!(credentials.redaction_values.len(),7,"all seven encrypted columns captured before the call");
    for secret in material{for encoded in r2_variants(secret){
        let text=format!("venue-free-text: {encoded}");
        let version=model::credential_version_by_id(&h.c,h.seed.api_key_id).unwrap();
        let stored=model::credential_write(&h.c,&version,|tx|Ok(sync::record_sync_error(tx,h.seed.api_key_id,&text,&credentials).unwrap())).unwrap();
        assert_eq!(stored,deltabadger::crypto::VENUE_TEXT_REDACTED,"stored diagnostics: {encoded}");
        let db:String=h.c.query_row("SELECT last_sync_error FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();assert_eq!(db,stored);
    }}
    let env=web::env(web::SECRET);
    h.app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.clock.clone()).unwrap();
    for secret in material{for encoded in r2_variants(secret){
        let text=format!("venue-free-text: {encoded}");
        let mut script=serde_json::json!({});
        script["GET paper-api.alpaca.markets/v2/account"]=serde_json::json!({"status":503,"body":{"message":text}});
        script["GET paper-api.alpaca.markets/v2/orders?limit=50&status=open"]=serde_json::json!({"status":503,"body":{"message":text}});
        h.app=h.app.with_figure_source(Source::Script(script)).unwrap();
        let balances=reads::fetch(&h.app,reads::Fetch::Balances{user:h.seed.user_id,exchange:h.seed.exchange_id,name:"Alpaca".into(),credentials:deltabadger::web::mcp::reads::Material::load(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap()}).await;
        let response=reads::finish(&h.c,balances).unwrap().to_string();assert!(response.contains(deltabadger::crypto::VENUE_TEXT_REDACTED),"MCP balances: {encoded:?}");assert!(!response.contains(&encoded),"no reflected balances diagnostic");
        let orders=reads::fetch(&h.app,reads::Fetch::Orders{user:h.seed.user_id,local:vec![],ids:Default::default(),venues:vec![(h.seed.exchange_id,"Alpaca".into(),Some(deltabadger::web::mcp::reads::Material::load(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap()))]}).await;
        let response=reads::finish(&h.c,orders).unwrap().to_string();assert!(response.contains(deltabadger::crypto::VENUE_TEXT_REDACTED),"MCP orders: {encoded:?}");assert!(!response.contains(&encoded),"no reflected orders diagnostic");
    }
    }
    assert_eq!(credentials.venue_text("venue says ordinary error"),"venue says ordinary error");
}
#[tokio::test(flavor="current_thread")]
async fn r2_validation_logs_flashes_and_pages_check_every_encoding(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    let columns=["key","secret","passphrase","access_token","rsa_signature_key","rsa_encryption_key","dh_param"];
    let material=["previous-key","previous-secret","paper","previous-access","previous-signing","previous-encryption","previous-dh"];
    for (column,value) in columns.iter().zip(material){h.c.execute(&format!("UPDATE api_keys SET {column}=?1 WHERE id=?2"),(h.app.cipher.encrypt(value),h.seed.api_key_id)).unwrap();}
    for secret in material{for encoded in r2_variants(secret){
        server.reset().await;
        Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"message":format!("venue-free-text {encoded}")}))).expect(2).mount(&server).await;
        let stored=deltabadger::sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
        let submitted=deltabadger::crypto::Credentials{key:"next-key".into(),secret:"next-secret".into(),passphrase:Some("paper".into()),redaction_values:stored.redaction_values};
        let validity=deltabadger::web::settings::validator::check(&server.uri(),&submitted,0).await.unwrap();
        let deltabadger::web::settings::validator::Validity::Pending(diagnostic)=validity else{panic!("fixture response is inconclusive")};
        assert_eq!(diagnostic.text,deltabadger::crypto::VENUE_TEXT_REDACTED,"validation returns a whole-text decision for each captured stored column: {encoded}");
        let before=h.snapshot();let response=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","trading"),("api_key[key]","next-key"),("api_key[secret]","next-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
        assert_eq!(response.status,422);assert!(response.body.contains("Failed to validate API key permissions"));assert!(!response.body.contains(&encoded));assert!(!response.body.contains("venue-free-text"));assert_eq!(h.snapshot(),before);
        let logs=h.logs.0.lock().unwrap().join("\n");assert!(logs.contains("HTTP 503"));assert!(!logs.contains(&encoded)&&!logs.contains("venue-free-text"),"validation logger: {encoded}");
        let page=h.browser.get(&h.app,"/settings/connect").await;assert_eq!(page.status,200);assert!(!page.body.contains("venue-free-text"));if secret!="paper"{assert!(!page.body.contains(&encoded));}server.verify().await;
    }}
}

#[test]
fn r2_every_venue_text_sink_and_upstream_route_is_gated(){
    let contract:serde_json::Value=serde_json::from_str(include_str!("../../script/rust/settings_r2_sinks.json")).unwrap();
    for row in contract.as_array().unwrap(){
        let file=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(row["file"].as_str().unwrap());
        let source=std::fs::read_to_string(file).unwrap();
        assert_eq!(source.matches(row["anchor"].as_str().unwrap()).count(),row["count"].as_u64().unwrap() as usize,"R2 venue-text sink bypass: {}",row["label"]);
    }
}

// R3: trading freshness belongs to the completed ledger's encrypted credential digest.
async fn r3_sync_ledger(h:&Harness,url:&str) {
    use deltabadger::{engine::FixedClock,jobs::{Db,state},sync::{self,jobs::Connect,ledger}};
    let credentials=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
    let db=Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    assert!(ledger::sync(&db,&LocalAlpaca(url.into()).connect(&credentials),h.seed.api_key_id,&credentials,&FixedClock(h.app.now())).await.unwrap().unwrap().complete);
    state::record_success(&h.c,"ledger_sync",Some(&h.seed.api_key_id.to_string()),h.app.now()).unwrap();
}
#[tokio::test(flavor="current_thread")]
async fn r3_replacement_cannot_inherit_a_completed_ledgers_trading_freshness() {
    use deltabadger::{engine::{model,staleness,tick::{self,TickOutcome},FixedClock},sync::jobs::Connect};
    let server=MockServer::start().await;
    let mut h=Harness::at_real_now(server.uri()).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","buying_power":"10000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/clock")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"is_open":true,"next_open":(h.app.now()+chrono::Duration::hours(1)).to_rfc3339(),"next_close":(h.app.now()+chrono::Duration::hours(8)).to_rfc3339()}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/stocks/AAPL/quotes/latest")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quote":{"ap":"100"}}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("POST")).and(path("/v2/orders")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"id":"b-placement"}))).mount(&server).await;

    let (asset,_)=common::seed::add_alpaca_stock(&h.c,&h.seed,"AAPL");
    let bot_id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(h.app.now()-chrono::Duration::seconds(1))).weights(&[(asset,1.0)]));
    common::seed::fresh_stock_jobs(&h.c,h.app.now());
    r3_sync_ledger(&h,&server.uri()).await;
    let bot=model::load_bot(&h.c,bot_id).unwrap();
    assert!(staleness::ledger_stale(&h.c,&bot,h.app.now()).unwrap().is_none(),"A's completed ledger is current for A");
    let a_stamp=deltabadger::app_config::get_plain(&h.c,&deltabadger::jobs::state::key("ledger_origin",Some(&h.seed.api_key_id.to_string()))).unwrap();
    let response=h.submit("POST",&format!("/bots/{bot_id}/add_api_key"),&[("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(response.status,200);
    assert_eq!(deltabadger::app_config::get_plain(&h.c,&deltabadger::jobs::state::key("ledger_origin",Some(&h.seed.api_key_id.to_string()))).unwrap(),a_stamp,"retain A's producer stamp until a complete B sync");
    let b=deltabadger::sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();
    let venue=LocalAlpaca(server.uri()).connect(&b);let mut attempts=tick::Attempts::default();
    let outcome=tick::tick(&h.c,&venue,bot_id,&FixedClock(h.app.now()),&mut attempts).await.unwrap();
    assert!(matches!(outcome,TickOutcome::Stale{source:"Alpaca account ledger",ref message} if message.contains("credential provenance missing or changed") && message.contains("complete ledger refresh is required")),"R3 must refuse A freshness for B with the stated stale reason: {outcome:?}");
    assert!(server.received_requests().await.unwrap().iter().all(|r|r.method.as_str()!="POST"),"R3 no placement before B completes ledger sync");
    assert_eq!(h.c.query_row("SELECT count(*) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    assert!(model::load_bot(&h.c,bot_id).unwrap().rust_placement().is_none());
    r3_sync_ledger(&h,&server.uri()).await;
    assert!(staleness::ledger_stale(&h.c,&bot,h.app.now()).unwrap().is_none(),"B completion makes B fresh");
    let outcome=tick::tick(&h.c,&venue,bot_id,&FixedClock(h.app.now()),&mut attempts).await.unwrap();
    assert!(matches!(outcome,TickOutcome::Done{placed:true}),"B sync restores normal placement: {outcome:?}");
    assert_eq!(server.received_requests().await.unwrap().iter().filter(|r|r.method.as_str()=="POST").count(),1,"exactly one placement, after B sync");
}
#[tokio::test(flavor="current_thread")]
async fn r3_missing_or_malformed_ledger_origin_is_stale_in_the_guard_transaction(){
    use deltabadger::{engine::{model,staleness},jobs::state};
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let (asset,_)=common::seed::add_alpaca_stock(&h.c,&h.seed,"AAPL");
    let id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00").weights(&[(asset,1.0)]));
    let bot=model::load_bot(&h.c,id).unwrap();state::record_success(&h.c,"ledger_sync",Some("1"),h.app.now()).unwrap();
    let key=state::key("ledger_origin",Some("1"));
    for stamp in [None,Some("not-json"),Some("{}"),Some("{\"key_id\":1,\"ciphertext_digest\":\"wrong\"}")]{
        h.c.execute("DELETE FROM app_configs WHERE key=?1",[&key]).unwrap();
        if let Some(stamp)=stamp{deltabadger::app_config::set_plain(&h.c,&key,stamp,h.app.now()).unwrap();}
        let tx=h.c.unchecked_transaction().unwrap();
        assert!(staleness::ledger_stale(&tx,&bot,h.app.now()).unwrap().is_some(),"R3 absent or malformed producer stamp never proves trading freshness");
        tx.commit().unwrap();
    }
}
async fn r3_replacement_wake(route:&str){
    use deltabadger::{engine::model,jobs::{self,state},sync::{self,jobs as sync_jobs}};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"2000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([{ "id":"r3-b-interest", "activity_type":"INT", "net_amount":"0.07", "date":"2026-09-01" }]))).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;r3_sync_ledger(&h,&server.uri()).await;
    let old=model::credential_version_by_id(&h.c,h.seed.api_key_id).unwrap().unwrap();
    for job in [sync_jobs::BALANCE_SYNC,deltabadger::tracker::jobs::TRACKER_LEDGER,deltabadger::tracker::jobs::PORTFOLIO_BACKFILL]{state::record_success(&h.c,job,Some("1"),h.app.now()).unwrap();}
    let factory=LocalAlpaca(server.uri());let registered=sync_jobs::register(&h.c,&factory,std::rc::Rc::new(sync::balances::NoPrices)).unwrap();
    let scheduler=jobs::Scheduler::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone(),registered,None).with_resolver(jobs::resolve::all(factory,std::rc::Rc::new(None::<jobs::data_api::DataApi<deltabadger::venue::http::ReqwestTransport>>),deltabadger::tracker::jobs::system_wall()));
    let env=web::env(web::SECRET);
    h.app=App::new(Config::from_env(&env).unwrap(),&env,rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.clock.clone()).unwrap().with_figure_source(deltabadger::web::figure::loading::Source::Disabled).unwrap().with_settings_key_boundary(server.uri(),Arc::new(Logs::default())).unwrap();
    h.app.attach_jobs(scheduler.wakers()).unwrap();
    let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
    let (stop,rx)=tokio::sync::watch::channel(false);let clock=h.clock.clone();
    let control=async{
        let response=if route=="api"{
            h.browser.send_body(&h.app,"POST","/api/api_keys",Some(serde_json::json!({"api_key":{"exchange_id":1,"key_type":"trading","key":"account-b-key","secret":"account-b-secret","passphrase":"paper"}}).to_string()),Csrf::Header,&[("content-type","application/json"),("origin","http://localhost:3000")]).await
        }else{
            let path=if route=="bot"{format!("/bots/{bot}/add_api_key")}else{"/tracker/add_api_key".into()};
            h.submit("POST",&path,&[("exchange_id","1"),("key_type","trading"),("api_key[key]","account-b-key"),("api_key[secret]","account-b-secret"),("api_key[passphrase]","paper")],Csrf::Header).await
        };
        let deadline=tokio::time::Instant::now()+std::time::Duration::from_secs(20); /* exits at the condition; the bound only fails a hang */let mut synced=false;
        while tokio::time::Instant::now()<deadline{
            let version=model::credential_version_by_id(&h.c,h.seed.api_key_id).unwrap().unwrap();
            synced=old!=version&&sync::cache::ledger_produced_by(&h.c,h.seed.api_key_id,&version).unwrap()&&h.c.query_row("SELECT EXISTS(SELECT 1 FROM account_transactions WHERE tx_id='r3-b-interest')",[],|r|r.get::<_,bool>(0)).unwrap();
            if synced{break}tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        stop.send(true).unwrap();(response.status,synced)
    };
    let (run,(status,synced))=tokio::join!(scheduler.run(rx,clock.as_ref()),control);run.unwrap();
    assert_eq!(status,if route=="api"{201}else{200});
    assert!(synced,"R3 replacement through {route} must wake and complete B ledger sync without schedule, restart or a placed order");
}
#[tokio::test(flavor="current_thread")]
async fn r3_bot_replacement_wakes_ledger_after_commit(){r3_replacement_wake("bot").await;}
#[tokio::test(flavor="current_thread")]
async fn r3_settings_replacement_wakes_ledger_after_commit(){r3_replacement_wake("settings").await;}
#[tokio::test(flavor="current_thread")]
async fn r3_json_api_replacement_wakes_ledger_after_commit(){r3_replacement_wake("api").await;}
#[tokio::test(flavor="current_thread")]
async fn r3_html_settings_refuse_json_bodies_without_writes(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for (path,body) in [
        ("/settings/update_name",serde_json::json!({"user":{"name":"New Owner"}})),
        ("/settings/update_email",serde_json::json!({"user":{"email":"new@example.com","current_password":"Correct-horse-9"}})),
        ("/settings/update_password",serde_json::json!({"user":{"password":"New-horse-8","current_password":"Correct-horse-9"}})),
        ("/settings/update_time_zone",serde_json::json!({"user":{"time_zone":"UTC"}})),
        ("/settings/update_locale",serde_json::json!({"user":{"locale":"de"}})),
        ("/settings/update_wash_sale",serde_json::json!({"wash_sale":{"enabled":"1","jurisdiction":"US"}})),
        ("/settings/update_two_fa",serde_json::json!({"user":{"otp_attempt":"000000"}})),
        ("/settings/update_mcp_dry_run",serde_json::json!({"enabled":"1"})),
        ("/settings/update_mcp_tool_permissions",serde_json::json!({"tool":"list_bots","enabled":"1"})),
        ("/settings/update_rest_tool_permissions",serde_json::json!({"tool":"list_bots","enabled":"1"})),
    ]{
        let before=h.snapshot();let response=h.browser.send_body(&h.app,"PATCH",path,Some(body.to_string()),Csrf::Header,&[("content-type","application/json"),("origin","http://localhost:3000")]).await;
        assert_eq!(response.status,400,"R3 HTML settings are form-only: {path}");assert_eq!(h.snapshot(),before,"R3 JSON settings request must not write: {path}");
    }
    for path in ["/settings/revoke_mcp_client/999","/settings/destroy_api_key/1"]{
        let before=h.snapshot();let response=h.browser.send_body(&h.app,"DELETE",path,Some("{}".into()),Csrf::Header,&[("content-type","application/json"),("origin","http://localhost:3000")]).await;
        assert_eq!(response.status,400,"R3 HTML settings are form-only: {path}");assert_eq!(h.snapshot(),before);
    }
    let before=h.snapshot();let response=h.browser.send_body(&h.app,"PATCH","/de/settings/update_name",Some(serde_json::json!({"user":{"name":"New Owner"}}).to_string()),Csrf::Header,&[("content-type","Application/JSON; charset=utf-8"),("origin","http://localhost:3000")]).await;
    assert_eq!(response.status,400);assert_eq!(h.snapshot(),before);
    assert!(h.mail.0.lock().unwrap().is_empty());assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn r4_rotation_between_park_commit_and_cache_insertion_asks_b_clock() {
    use deltabadger::{engine::{run::{self,Engine},FixedClock},lease,store::Paths};
    use wiremock::matchers::header;
    let server=MockServer::start().await;
    let h=Harness::at_real_now(server.uri()).await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","buying_power":"10000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/clock")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"is_open":false,"next_open":(h.app.now()+chrono::Duration::hours(1)).to_rfc3339(),"next_close":(h.app.now()+chrono::Duration::hours(8)).to_rfc3339()}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/clock")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"is_open":true,"next_open":(h.app.now()+chrono::Duration::hours(1)).to_rfc3339(),"next_close":(h.app.now()+chrono::Duration::hours(8)).to_rfc3339()}))).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/stocks/AAPL/quotes/latest")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"quotes":{"BTC/USD":{"ap":200000}}}))).mount(&server).await;

    let (bot,_)=q_order(&h);h.c.execute("UPDATE assets SET category='Stock',instrument_type='stock' WHERE id=?1",[h.seed.btc]).unwrap();
    h.c.execute("UPDATE tickers SET minimum_quote_size=100000 WHERE id=?1",[h.seed.ticker_id]).unwrap();
    common::seed::fresh_stock_jobs(&h.c,h.app.now());
    let paths=Paths::from_env(&|_|None,h._dir.path());let lock=lease::lock(&paths,h.app.now()).unwrap();
    let mut engine=Engine::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),LocalAlpaca(server.uri()),h.app.cipher.clone(),lock);
    let ciphertext=h.app.cipher.encrypt("account-b-key");
    let secret=h.app.cipher.encrypt("account-b-secret");let now=h.app.now();let id=h.seed.api_key_id;
    engine.inject_cache_insert_step(move|c|{
        assert!(deltabadger::engine::model::load_bot(c,bot.id)?.transient["waiting_for_market_open"].as_bool().unwrap(),"R4 barrier runs after fenced park commits");
        c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(&ciphertext,&secret,id))?;
        common::seed::fresh_stock_jobs(c,now);Ok(())
    });
    run::step(&mut engine,&FixedClock(h.app.now())).await.unwrap();
    run::step(&mut engine,&FixedClock(h.app.now()+chrono::Duration::seconds(1))).await.unwrap();
    let requests=server.received_requests().await.unwrap();
    assert!(requests.iter().any(|r|r.url.path()=="/v2/clock"&&r.headers.get("APCA-API-KEY-ID").is_some_and(|h|h=="account-b-key")),"R4 B tick must read B's market clock instead of waiting on A's cached next_open");
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn r4_credential_job_success_without_producer_is_never_fresh(){
    use deltabadger::{jobs::{self,Job,JobFuture,Cx,Wake,Spec,state,Outcome},sync::jobs::LedgerSync};
    struct DropsProducer(LedgerSync<LocalAlpaca>);
    impl Job for DropsProducer {
        fn spec(&self)->Spec{self.0.spec()}
        fn run<'a>(&'a self,_cx:Cx<'a>,_wakes:Vec<Wake>)->JobFuture<'a>{Box::pin(async{Outcome::Done})}
    }
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    let scheduler=jobs::Scheduler::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone(),vec![Box::new(DropsProducer(LedgerSync::new(LocalAlpaca(server.uri()),h.seed.api_key_id)))],None);
    let (stop,stopped)=tokio::sync::watch::channel(false);
    let control=async{tokio::time::sleep(std::time::Duration::from_millis(50)).await;stop.send(true).unwrap();};
    let (result,())=tokio::join!(scheduler.run(stopped,&*h.clock),control);result.unwrap();
    assert_eq!(state::read(&h.c,"ledger_sync",Some("1")).unwrap().last_success_at,None,"R4 independent completion cannot label credential-derived success");
}

#[tokio::test(flavor="current_thread")]
async fn r4_snapshot_cannot_relabel_retained_a_balances_as_b(){
    use deltabadger::{engine::{model,FixedClock},sync::balances::{self,NoPrices},tracker::jobs,venue::{alpaca::{AlpacaVenue,Urls},http::{self,ReqwestTransport}}};
    use wiremock::matchers::header;
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let at=h.app.now();
    for (key,cash) in [("previous-key","10000"),("account-b-key","2000")]{
        Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID",key)).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":cash}))).expect(1).mount(&server).await;
    }
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).expect(2).mount(&server).await;
    let bot_id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,&deltabadger::codec::format_time(at)));
    let bot=model::load_bot(&h.c,bot_id).unwrap();let a=model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap();
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let venue=|credentials:&deltabadger::crypto::Credentials|AlpacaVenue::new(ReqwestTransport::new(http::client(),credentials.key.clone(),credentials.secret.clone()),Urls{trading:server.uri(),data:server.uri()});
    balances::sync(&db,&venue(&a),&NoPrices,h.seed.api_key_id,&a,&FixedClock(at)).await.unwrap().unwrap();
    let wall=jobs::system_wall();
    jobs::ledger_run::<ReqwestTransport>(&db,None,h.seed.user_id,&FixedClock(at),wall.clone(),&mut jobs::Allowance::run()).await.unwrap();
    let before=h.snapshot();
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();
    let retained=h.snapshot();
    let stale=jobs::ledger_run::<ReqwestTransport>(&db,None,h.seed.user_id,&FixedClock(at),wall.clone(),&mut jobs::Allowance::run()).await;
    assert!(stale.is_err(),"R4 retained A balances cannot receive B snapshot provenance");
    assert_eq!(h.snapshot(),retained,"R4 stale snapshot calculation must not commit any rows");
    assert_ne!(before,retained);
    let b=model::credentials_for(&h.c,&h.app.cipher,&bot).unwrap().unwrap();
    balances::sync(&db,&venue(&b),&NoPrices,h.seed.api_key_id,&b,&FixedClock(at)).await.unwrap().unwrap();
    jobs::ledger_run::<ReqwestTransport>(&db,None,h.seed.user_id,&FixedClock(at),wall.clone(),&mut jobs::Allowance::run()).await.unwrap();
    assert_eq!(h.c.query_row("SELECT value_usd FROM portfolio_snapshots WHERE user_id=?1",[h.seed.user_id],|r|r.get::<_,f64>(0)).unwrap(),2000.0,"R4 fresh B balance control publishes B's value");
    let origin_key=deltabadger::jobs::state::key("balance_origin",Some(&format!("{}:{}",h.seed.user_id,h.seed.exchange_id)));
    h.c.execute("DELETE FROM app_configs WHERE key=?1",[origin_key]).unwrap();
    let missing=h.snapshot();
    assert!(jobs::ledger_run::<ReqwestTransport>(&db,None,h.seed.user_id,&FixedClock(at),wall,&mut jobs::Allowance::run()).await.is_err(),"R4 missing balance producer is stale");
    assert_eq!(h.snapshot(),missing);
    server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn r4_submitted_validation_producer_matches_seven_stored_envelopes(){
    let server=MockServer::start().await;
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"status":"ACTIVE","cash":"0"}))).expect(1).mount(&server).await;
    let mut h=Harness::new(server.uri()).await;
    let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec{status:2,..common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00")});
    let observed=Arc::new(std::sync::Mutex::new(Vec::new()));let capture=observed.clone();
    h.app.set_validation_observer(move|origin|capture.lock().unwrap().push(origin)).unwrap();
    let response=h.submit("POST",&format!("/bots/{bot}/add_api_key"),&[("api_key[key]","prepared-key"),("api_key[secret]","prepared-secret"),("api_key[passphrase]","paper"),("api_key[access_token]","prepared-access"),("api_key[rsa_signature_key]","prepared-signing"),("api_key[rsa_encryption_key]","prepared-encryption"),("api_key[dh_param]","prepared-dh")],Csrf::Header).await;
    assert_eq!(response.status,200,"healthy submitted credential is accepted");
    {
        let origins=observed.lock().unwrap();assert_eq!(origins.len(),1,"observe the actual validator result exactly once");
        assert!(origins[0].is_some(),"actual validation result has a producer");
        let producer=origins[0].as_ref().unwrap();
        assert!(deltabadger::engine::model::credential_is_current(&h.c,producer).unwrap(),"R4 submitted validation producer must match all seven stored envelopes");
    }
    server.verify().await;
}


#[tokio::test(flavor="current_thread")]
async fn r5_snapshot_refuses_incomplete_origin_and_partial_balance_batches(){
    use deltabadger::{engine::{model,FixedClock},sync::balances::{self,NoPrices},tracker::jobs};
    use wiremock::matchers::header;
    use deltabadger::sync::jobs::Connect;
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let at=h.app.now();
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"cash":"10000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).and(header("APCA-API-KEY-ID","previous-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let bot=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00"));
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let a=model::credentials_for(&h.c,&h.app.cipher,&model::load_bot(&h.c,bot).unwrap()).unwrap().unwrap();
    let av=LocalAlpaca(server.uri()).connect(&a);
    balances::sync(&db,&av,&NoPrices,h.seed.api_key_id,&a,&FixedClock(at)).await.unwrap().unwrap();
    jobs::ledger_run::<deltabadger::venue::http::ReqwestTransport>(&db,None,h.seed.user_id,&FixedClock(at),jobs::system_wall(),&mut jobs::Allowance::run()).await.unwrap();
    let mut positions=vec![];
    for n in 0..101 {let symbol=format!("R5{n}");let (asset,_)=common::seed::add_alpaca_stock(&h.c,&h.seed,&symbol);h.c.execute("INSERT INTO exchange_assets(exchange_id,asset_id,created_at,updated_at) VALUES(?1,?2,'2026-09-01','2026-09-01')",[h.seed.exchange_id,asset]).unwrap();positions.push(serde_json::json!({"symbol":symbol,"asset_class":"us_equity","qty":"1"}));}
    Mock::given(method("GET")).and(path("/v2/account")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"cash":"2000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).and(header("APCA-API-KEY-ID","account-b-key")).respond_with(ResponseTemplate::new(200).set_body_json(positions)).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/stocks/snapshots")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({}))).mount(&server).await;
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();
    let b=model::credentials_for(&h.c,&h.app.cipher,&model::load_bot(&h.c,bot).unwrap()).unwrap().unwrap();let bv=LocalAlpaca(server.uri()).connect(&b);
    let gaps=std::cell::RefCell::new(vec![]);let owner=h.seed.user_id;let h_ref=&h;let db_ref=&db;let gaps_ref=&gaps;
    let step=move|gap|->std::pin::Pin<Box<dyn std::future::Future<Output=()>+'_>>{Box::pin(async move{
        let before=h_ref.snapshot();
        let result=jobs::ledger_run::<deltabadger::venue::http::ReqwestTransport>(db_ref,None,owner,&FixedClock(at),jobs::system_wall(),&mut jobs::Allowance::run()).await;
        assert!(result.is_err(),"R5 incomplete balance set must refuse snapshot at origin/batch gap {gap}");
        assert_eq!(h_ref.snapshot(),before,"R5 partial rows must not acquire a snapshot producer stamp");
        gaps_ref.borrow_mut().push(gap);
    })};
    balances::sync_with_steps(&db,&bv,&NoPrices,h.seed.api_key_id,&b,&FixedClock(at),&step).await.unwrap().unwrap();
    assert_eq!(*gaps.borrow(),vec![0,1,2],"cover origin-only and both partial batch gaps");
    jobs::ledger_run::<deltabadger::venue::http::ReqwestTransport>(&db,None,owner,&FixedClock(at),jobs::system_wall(),&mut jobs::Allowance::run()).await.unwrap();
    assert_eq!(h.c.query_row("SELECT value_usd FROM portfolio_snapshots WHERE user_id=?1",[owner],|r|r.get::<_,f64>(0)).unwrap(),2000.0,"R5 completed B balance set publishes B money");
}

async fn r5_retry_rotation(exhaust:bool,restart:bool){
    use deltabadger::{engine::{run::{self,Engine},model,FixedClock},venue::alpaca::LiveFactory};
    let server=MockServer::start().await;let h=Harness::at_real_now(server.uri()).await;let at=h.app.now();let (bot,_)=q_order(&h);
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"message":"temporary quote failure"}))).mount(&server).await;
    let paths=deltabadger::store::Paths::from_env(&|_|None,h._dir.path());let lock=deltabadger::lease::lock(&paths,at).unwrap();
    let mut engine=Engine::new(rusqlite::Connection::open(&paths.primary).unwrap(),LiveFactory::with_paper_boundary(server.uri()),h.app.cipher.clone(),lock);
    for pass in 0..if exhaust {4}else{3} {run::step(&mut engine,&FixedClock(at+chrono::Duration::seconds(pass*100))).await.unwrap();}
    assert_eq!(server.received_requests().await.unwrap().len(),if exhaust {4}else{3},"A generated the intended failure chain");
    let rotated_at=at+chrono::Duration::seconds(if exhaust {301}else{201});
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();
    common::seed::fresh_stock_jobs(&h.c,rotated_at);
    if restart {drop(engine);let lock=deltabadger::lease::lock(&paths,rotated_at).unwrap();engine=Engine::new(rusqlite::Connection::open(&paths.primary).unwrap(),LiveFactory::with_paper_boundary(server.uri()),h.app.cipher.clone(),lock);}
    run::step(&mut engine,&FixedClock(rotated_at)).await.unwrap();
    let requests=server.received_requests().await.unwrap();
    assert!(requests.iter().any(|r|r.headers.get("APCA-API-KEY-ID").is_some_and(|h|h=="account-b-key")),"R5 B tick must ignore A retry or persisted failure wait");
    let state=model::load_bot(&h.c,bot.id).unwrap();assert!(state.rust_defer().unwrap().is_none(),"R5 first B failure must be attempt one, without an exhausted deferral");
    let exhausted:i64=h.c.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id=?1 AND event='execution_retrying'",[bot.id],|r|r.get(0)).unwrap();assert_eq!(exhausted,if exhaust {1}else{0},"R5 B first failure must not exhaust the retry budget");
    let count=requests.len();run::step(&mut engine,&FixedClock(rotated_at+chrono::Duration::seconds(2))).await.unwrap();assert_eq!(server.received_requests().await.unwrap().len(),count,"B attempt-one wait is retained for the same digest");
    run::step(&mut engine,&FixedClock(rotated_at+chrono::Duration::seconds(3))).await.unwrap();assert_eq!(server.received_requests().await.unwrap().len(),count+1,"B attempt-one wait is three seconds");
}
#[tokio::test(flavor="current_thread")]
async fn r5_rotation_drops_a_wait_and_starts_b_at_attempt_one(){r5_retry_rotation(false,false).await;}
#[tokio::test(flavor="current_thread")]
async fn r5_rotation_drops_exhausted_failure_deferral(){r5_retry_rotation(true,false).await;}
#[tokio::test(flavor="current_thread")]
async fn r5_rotation_drops_persisted_failure_deferral_after_restart(){r5_retry_rotation(true,true).await;}


#[tokio::test(flavor="current_thread")]
async fn r5_completed_snapshot_capture_is_refused_when_a_new_batch_is_incomplete(){
    use deltabadger::{engine::FixedClock,sync::{self,balances::{self,NoPrices}},tracker::jobs};
    use deltabadger::sync::jobs::Connect;
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let at=h.app.now();
    Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"cash":"10000"}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    let db=deltabadger::jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());
    let a=sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();let av=LocalAlpaca(server.uri()).connect(&a);
    balances::sync(&db,&av,&NoPrices,h.seed.api_key_id,&a,&FixedClock(at)).await.unwrap().unwrap();
    let read=h.c.unchecked_transaction().unwrap();let origin=sync::cache::capture_read(&read,h.seed.user_id,None).unwrap().with_balance_producers(&read).unwrap();read.commit().unwrap();
    let h_ref=&h;let origin_ref=&origin;
    let step=move|gap|->std::pin::Pin<Box<dyn std::future::Future<Output=()>+'_>>{Box::pin(async move{
        let before=h_ref.snapshot();
        let out=jobs::written_for(&h_ref.c,origin_ref,&||at,|c,_|deltabadger::tracker::snapshot::write(c,h_ref.seed.user_id,&[],at.date_naive()));
        assert!(out.is_err(),"R5 write fence must recheck current balance completion after captured-complete input at gap {gap}");assert_eq!(h_ref.snapshot(),before);
    })};
    balances::sync_with_steps(&db,&av,&NoPrices,h.seed.api_key_id,&a,&FixedClock(at),&step).await.unwrap().unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn r5_tracker_ledger_cache_is_cold_after_rotation_even_after_b_sync(){
    use deltabadger::{engine::FixedClock,jobs::{self,Cx,Outcome},tracker::{self,cache::{self,State}}};
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).mount(&server).await;
    r3_sync_ledger(&h,&server.uri()).await;
    let db=jobs::Db::new(rusqlite::Connection::open(h._dir.path().join("production.sqlite3")).unwrap(),h.app.cipher.clone());let clock=FixedClock(h.app.now());
    let job=||tracker::jobs::resolve::<LocalAlpaca,deltabadger::venue::http::ReqwestTransport>(tracker::jobs::TRACKER_LEDGER,h.seed.user_id,&LocalAlpaca(server.uri()),std::rc::Rc::new(None),tracker::jobs::system_wall()).unwrap();
    assert_eq!(job().run(Cx{db:db.clone(),clock:&clock,wakers:jobs::Wakers::default()},vec![]).await,Outcome::Done,"R5 unchanged A cache publishes");
    assert!(matches!(cache::read(&h.c,h.seed.user_id,None,h.app.now()).unwrap(),State::Warm(_)));
    let before=deltabadger::app_config::get_plain(&h.c,&cache::key(h.seed.user_id)).unwrap();
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();
    assert!(matches!(cache::read(&h.c,h.seed.user_id,None,h.app.now()).unwrap(),State::Cold),"R5 cache cannot present A under B");
    r3_sync_ledger(&h,&server.uri()).await;
    assert!(matches!(cache::read(&h.c,h.seed.user_id,None,h.app.now()).unwrap(),State::Cold),"R5 B ledger freshness must not relabel A cached walk");
    assert_eq!(deltabadger::app_config::get_plain(&h.c,&cache::key(h.seed.user_id)).unwrap(),before,"retain cached history without relabelling");
    assert_eq!(job().run(Cx{db,clock:&clock,wakers:jobs::Wakers::default()},vec![]).await,Outcome::Done,"R5 B recomputation publishes");
    assert!(matches!(cache::read(&h.c,h.seed.user_id,None,h.app.now()).unwrap(),State::Warm(_)));
}


#[tokio::test(flavor="current_thread")]
async fn r5_rotation_discards_a_blocking_failure_and_keeps_b_own_failure(){
    use deltabadger::{engine::{model,tick::{self,Attempts,TickOutcome},FixedClock},sync::jobs::Connect};
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let at=h.app.now();
    Mock::given(method("GET")).and(path("/v2/clock")).respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({"code":40110000,"message":"unauthorized."}))).expect(3).mount(&server).await;
    let (asset,_)=common::seed::add_alpaca_stock(&h.c,&h.seed,"AAPL");
    let id=common::seed::insert_bot(&h.c,&h.seed,&common::seed::BotSpec::weekly(60.0,"2026-09-01 00:00:00").weights(&[(asset,1.0)]));common::seed::fresh_stock_jobs(&h.c,at);
    let a=deltabadger::sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();let av=LocalAlpaca(server.uri()).connect(&a);
    assert!(matches!(tick::tick(&h.c,&av,id,&FixedClock(at),&mut Attempts::default()).await.unwrap(),TickOutcome::Rescheduled));
    assert_eq!(model::load_bot(&h.c,id).unwrap().last_failure_kind().as_deref(),Some("invalid_key"));
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();common::seed::fresh_stock_jobs(&h.c,at);
    let b=deltabadger::sync::credentials(&h.c,&h.app.cipher,h.seed.api_key_id).unwrap();let bv=LocalAlpaca(server.uri()).connect(&b);
    let outcome=tick::tick(&h.c,&bv,id,&FixedClock(at+chrono::Duration::seconds(1)),&mut Attempts::default()).await.unwrap();
    assert!(matches!(outcome,TickOutcome::Rescheduled),"R5 first B blocking failure cannot inherit A failure decision: {outcome:?}");
    assert!(matches!(tick::tick(&h.c,&bv,id,&FixedClock(at+chrono::Duration::seconds(2)),&mut Attempts::default()).await.unwrap(),TickOutcome::Stopped),"R5 second unchanged B failure retains its own decision");server.verify().await;
}

#[tokio::test(flavor="current_thread")]
async fn r6_legacy_unclassified_failure_upgrade_rotation_ticks_b(){
    use deltabadger::{engine::{run::{self,Engine},model,FixedClock},venue::alpaca::LiveFactory};
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;h.clock.set(legacy::at());let at=h.app.now();let (bot,_)=q_order(&h);
    // This row was emitted by an actual unmodified pinned-base Engine::step.
    let legacy:serde_json::Value=serde_json::from_str(include_str!("fixtures/s1_legacy_failure_wait.json")).unwrap();
    assert!(legacy["origin"].is_null() && legacy["producer"].is_null());
    let until=at+chrono::Duration::days(7)-chrono::Duration::seconds(1);
    assert_eq!(legacy["until"],until.to_rfc3339_opts(chrono::SecondsFormat::AutoSi,true),"R7 fixture deadline is derived from the injected clock");
    assert_eq!(legacy["schedule"],format!("{}/Seconds(604800.0)",(at-chrono::Duration::seconds(1)).timestamp_micros()));
    h.c.execute("UPDATE bots SET status=5,transient_data=json_set(transient_data,'$.rust_defer_until',json(?1)) WHERE id=?2",(legacy.to_string(),bot.id)).unwrap();
    assert!(model::load_bot(&h.c,bot.id).unwrap().last_failure_kind().is_none());
    let paths=deltabadger::store::Paths::from_env(&|_|None,h._dir.path());let lock=deltabadger::lease::lock(&paths,at).unwrap();
    let mut engine=Engine::new(rusqlite::Connection::open(&paths.primary).unwrap(),LiveFactory::with_paper_boundary(server.uri()),h.app.cipher.clone(),lock);
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();
    let due=at+chrono::Duration::seconds(1);
    h.clock.set(due);
    assert!(model::load_bot(&h.c,bot.id).unwrap().rust_defer().unwrap().unwrap().0>due.timestamp_micros(),"R6 base A wait still lies in the future at chronological upgrade time");
    Mock::given(method("GET")).and(path("/v2/account/activities")).respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([]))).expect(1).mount(&server).await;
    r3_sync_ledger(&h,&server.uri()).await;
    assert!(deltabadger::engine::staleness::ledger_stale(&h.c,&model::load_bot(&h.c,bot.id).unwrap(),due).unwrap().is_none(),"R6 B completed its own real ledger sync");
    Mock::given(method("GET")).and(path("/v1beta3/crypto/us/latest/quotes")).respond_with(ResponseTemplate::new(503).set_body_json(serde_json::json!({"message":"B first transient"}))).expect(1).mount(&server).await;
    run::step(&mut engine,&FixedClock(due)).await.unwrap();
    let requests=server.received_requests().await.unwrap();
    assert!(requests.iter().any(|r|r.url.path()=="/v1beta3/crypto/us/latest/quotes" && r.headers.get("APCA-API-KEY-ID").is_some_and(|v|v=="account-b-key")),"R6 upgraded B must tick at its next due pass instead of inheriting A's legacy checkpoint");
    assert!(model::load_bot(&h.c,bot.id).unwrap().rust_defer().unwrap().is_none(),"R6 legacy unknown wait must be removed on read");
    assert_eq!(h.c.query_row("SELECT count(*) FROM transactions",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    server.verify().await;
}
#[tokio::test(flavor="current_thread")]
async fn r6_explicit_local_wait_survives_rotation_and_restart(){
    use deltabadger::{engine::{run::{self,Engine},placement,model,FixedClock},venue::alpaca::LiveFactory};
    let server=MockServer::start().await;let h=Harness::new(server.uri()).await;let at=h.app.now();let (bot,_)=q_order(&h);
    placement::defer_to_next_checkpoint(&h.c,&bot,at).unwrap();
    let wait_before=model::load_bot(&h.c,bot.id).unwrap().rust_defer().unwrap();
    h.c.execute("UPDATE api_keys SET key=?1,secret=?2 WHERE id=?3",(h.app.cipher.encrypt("account-b-key"),h.app.cipher.encrypt("account-b-secret"),h.seed.api_key_id)).unwrap();
    common::seed::fresh_stock_jobs(&h.c,at);
    let paths=deltabadger::store::Paths::from_env(&|_|None,h._dir.path());let lock=deltabadger::lease::lock(&paths,at).unwrap();
    let mut engine=Engine::new(rusqlite::Connection::open(&paths.primary).unwrap(),LiveFactory::with_paper_boundary(server.uri()),h.app.cipher.clone(),lock);
    run::step(&mut engine,&FixedClock(at+chrono::Duration::seconds(1))).await.unwrap();
    assert_eq!(model::load_bot(&h.c,bot.id).unwrap().rust_defer().unwrap(),wait_before,"R6 explicitly local waits survive credential rotation and restart");
    assert!(server.received_requests().await.unwrap().is_empty(),"R6 local wait must suppress the tick");
    assert_eq!(model::load_bot(&h.c,bot.id).unwrap().transient["rust_defer_until"]["origin"],"local","R6 local writer labels its origin");
}
#[tokio::test(flavor="current_thread")]
async fn r6_failure_wait_origin_is_venue_and_keeps_its_current_digest(){
    use deltabadger::engine::{run::{self,Engine},model,FixedClock};
    let h=Harness::new("http://localhost:3000".into()).await;h.clock.set(legacy::at());let at=h.app.now();let (bot,_)=q_order(&h);
    let transport=legacy::failed_order();
    let paths=deltabadger::store::Paths::from_env(&|_|None,h._dir.path());let lock=deltabadger::lease::lock(&paths,at).unwrap();
    let mut engine=Engine::new(rusqlite::Connection::open(&paths.primary).unwrap(),legacy::Factory(transport.clone()),h.app.cipher.clone(),lock);
    run::step(&mut engine,&FixedClock(at)).await.unwrap();let row=model::load_bot(&h.c,bot.id).unwrap();let wait=&row.transient["rust_defer_until"];
    assert_eq!(wait["origin"],"venue","R6 every failure deferral declares venue origin");
    assert!(!wait["producer"].is_null());let before=row.rust_defer().unwrap();
    assert_eq!(transport.requests().len(),2,"R7 actual engine consumed the quote and rejected order");
    run::step(&mut engine,&FixedClock(at+chrono::Duration::seconds(1))).await.unwrap();
    assert_eq!(model::load_bot(&h.c,bot.id).unwrap().rust_defer().unwrap(),before,"R6 same-producer venue wait is kept");
    assert_eq!(transport.requests().len(),2,"R7 current wait suppresses a second venue call");
}

#[tokio::test(flavor="current_thread")]
async fn r9_replacement_resets_only_credential_ledger_progress_and_keeps_history(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET key_type=0,last_synced_at='2026-09-09 00:00:00',last_sync_error='previous failure' WHERE id=?1",[h.seed.api_key_id]).unwrap();
    for key in ["rust_sync.ledger:1","rust_sync.ledger_splits:1","rust_sync.ledger:999"]{
        h.c.execute("INSERT INTO app_configs(key,value,created_at,updated_at)VALUES(?1,'retained-state','2026-01-01','2026-01-01')",[key]).unwrap();
    }
    h.link();
    let before=h.snapshot();
    let answer=h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","replacement-key"),("api_key[secret]","replacement-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(answer.status,201,"R9 valid replacement succeeds");
    let after=h.snapshot();assert_eq!(after["account_transactions"],before["account_transactions"],"R9 history remains linked");
    assert_eq!(after["api_keys"][0]["id"],before["api_keys"][0]["id"]);
    assert_eq!(after["api_keys"][0]["created_at"],before["api_keys"][0]["created_at"]);
    let watermark:Option<String>=h.c.query_row("SELECT last_synced_at FROM api_keys WHERE id=?1",[h.seed.api_key_id],|r|r.get(0)).unwrap();
    assert!(watermark.is_none(),"R9 replacement clears the previous credential watermark");
    assert_eq!(h.c.query_row("SELECT COUNT(*) FROM app_configs WHERE key IN ('rust_sync.ledger:1','rust_sync.ledger_splits:1')",[],|r|r.get::<_,i64>(0)).unwrap(),0,"R9 replacement clears both old import cursors");
    assert_eq!(h.c.query_row("SELECT value FROM app_configs WHERE key='rust_sync.ledger:999'",[],|r|r.get::<_,String>(0)).unwrap(),"retained-state","R9 another credential's progress survives");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn r9_same_key_revalidation_keeps_ciphertexts_realm_watermark_and_import(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    h.c.execute("UPDATE api_keys SET key_type=0,passphrase=?1,last_synced_at='2026-09-09 00:00:00',last_sync_error='prior failure' WHERE id=?2",(h.app.cipher.encrypt("live"),h.seed.api_key_id)).unwrap();
    h.c.execute("INSERT INTO app_configs(key,value,created_at,updated_at)VALUES('rust_sync.ledger:1','retained-state','2026-01-01','2026-01-01')",[]).unwrap();
    let envelopes=||h.c.query_row("SELECT key,secret,passphrase,last_synced_at,last_sync_error FROM api_keys WHERE id=1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?))).unwrap();
    let before=envelopes();
    let answer=h.submit("POST","/api/api_keys",&[("api_key[exchange_id]","1"),("api_key[key_type]","trading"),("api_key[key]","previous-key"),("api_key[secret]","previous-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
    assert_eq!(answer.status,201,"R9 identical key only queues revalidation");
    let after=h.c.query_row("SELECT key,secret,passphrase,last_synced_at,last_sync_error FROM api_keys WHERE id=1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?))).unwrap();
    assert_eq!(after,before,"R9 revalidation never rotates ciphertexts or adopts a submitted realm");
    assert_eq!(h.c.query_row("SELECT value FROM app_configs WHERE key='rust_sync.ledger:1'",[],|r|r.get::<_,String>(0)).unwrap(),"retained-state");
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn r9_confirmation_resend_is_limited_before_mail_and_keeps_normal_response(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for _ in 0..5{assert_eq!(h.submit("POST","/confirmation",&[("user[email]","unknown@example.com")],Csrf::Header).await.status,303);}
    let before=h.snapshot();let answer=h.submit("POST","/confirmation",&[("user[email]","unknown@example.com")],Csrf::Header).await;
    assert_eq!(answer.status,429,"R9 sixth resend is refused by the shared rate pipeline");
    assert_eq!(h.snapshot(),before);assert!(h.mail.0.lock().unwrap().is_empty());
}

#[test]
fn r9_password_reset_methods_share_one_rails_throttle(){
    use deltabadger::web::rate_limit::Limiter;
    let limiter=Limiter::default();let now=chrono::DateTime::parse_from_rfc3339("2026-09-10T12:00:30.123456Z").unwrap().with_timezone(&chrono::Utc);
    for method in ["POST","PATCH","PUT","POST","PATCH"]{assert!(limiter.hit(&method.parse().unwrap(),"/password","192.0.2.1",now).is_none());}
    assert!(limiter.hit(&"PUT".parse().unwrap(),"/password","192.0.2.1",now).is_some(),"R9 reset methods share the five-attempt budget");
}

#[test]
fn r9_recorded_oracle_sources_secrets_and_smtp_policy_are_current(){
    use sha2::{Digest,Sha256};
    let vector:serde_json::Value=serde_json::from_str(include_str!("fixtures/settings_r9_vectors.json")).unwrap();
    let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (name,hash) in vector["sources"].as_object().unwrap(){assert_eq!(format!("{:x}",Sha256::digest(std::fs::read(root.join(name)).unwrap())),hash.as_str().unwrap(),"R9 re-record the changed Rails oracle: {name}");}
    for name in vector["sensitive_query"].as_array().unwrap(){let name=name.as_str().unwrap();assert!(deltabadger::web::locale::sensitive_query(name));assert!(deltabadger::web::locale::sensitive_query(&format!("rows[][{}]",name.to_ascii_uppercase())));}
    assert!(!deltabadger::web::locale::sensitive_query("user[name]"));
    assert_eq!(vector["redacted"],"[redacted]");
    assert_eq!(vector["smtp_custom_requires_starttls"],"always");
}

#[tokio::test(flavor="current_thread")]
async fn r9_setup_token_reads_count_query_and_head_but_bare_reads_stay_free(){
    let server=MockServer::start().await;let mut h=Harness::new(server.uri()).await;
    for _ in 0..7{let answer=h.browser.send(&h.app,"GET","/setup",None,Csrf::None,&[]).await;assert_ne!(answer.status,429,"R9 bare setup read does not spend token budget");}
    for _ in 0..5{let answer=h.browser.send(&h.app,"HEAD","/setup?token[]=guess",None,Csrf::None,&[]).await;assert_ne!(answer.status,429);}
    let answer=h.browser.send(&h.app,"GET","/setup?token=guess",None,Csrf::None,&[]).await;
    assert_eq!(answer.status,429,"R9 setup token GET and HEAD share one guessing budget");
}

#[tokio::test(flavor="current_thread")]
async fn r9_read_only_validation_rejects_missing_cash_and_supported_unnamed_positions(){
    for (account,positions,accepted) in [
        (serde_json::json!({"status":"ACTIVE"}),serde_json::json!([]),false),
        (serde_json::json!({"status":"ACTIVE","cash":null}),serde_json::json!([]),false),
        (serde_json::json!({"status":"ACTIVE","cash":"0"}),serde_json::json!([{"asset_class":"us_equity","qty":"2"}]),false),
        (serde_json::json!({"status":"ACTIVE","cash":"0"}),serde_json::json!([{"asset_class":"option","qty":"2"}]),true),
    ]{
        let server=MockServer::start().await;
        Mock::given(method("GET")).and(path("/v2/account")).respond_with(ResponseTemplate::new(200).set_body_json(account)).expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path("/v2/positions")).respond_with(ResponseTemplate::new(200).set_body_json(positions)).expect(1).mount(&server).await;
        let mut h=Harness::new(server.uri()).await;let before=h.snapshot();
        let answer=h.submit("POST","/tracker/add_api_key",&[("exchange_id","1"),("key_type","read_only"),("api_key[key]","reading-key"),("api_key[secret]","reading-secret"),("api_key[passphrase]","paper")],Csrf::Header).await;
        assert_eq!(answer.status,if accepted{200}else{422},"R9 read-only validation matches the malformed-balance oracle");
        if !accepted{assert_eq!(h.snapshot(),before,"R9 malformed validation leaves stored rows unchanged");}
        server.verify().await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn csp_report_throttles_after_thirty_posts_without_session_or_csrf() {
    let (dir, opened, _) = fixture::install();
    let clock = web::TestClock::at("2026-09-10T12:00:30.123456Z");
    let env = web::env(web::SECRET);
    let app = App::new(Config::from_env(&env).unwrap(), &env, opened.primary, clock.clone()).unwrap();
    let mut browser = Browser::default();
    for number in 1..=31 {
        let answer = browser.send_body(
            &app, "POST", "/csp-report",
            Some(r#"{"csp-report":{"violated-directive":"script-src"}}"#.into()),
            Csrf::None,
            &[("content-type", "application/csp-report"), ("origin", "https://foreign.example")],
        ).await;
        assert_eq!(answer.status, if number <= 30 { 204 } else { 429 }, "CSP report POST {number}");
        assert_eq!(answer.header("set-cookie"), None, "CSP reports never open a session");
        if number == 31 {
            assert_eq!(answer.header("retry-after"), Some("30"));
            assert_eq!(answer.header("content-type"), Some("text/plain; charset=utf-8"));
        }
    }
    clock.set(web::at("2026-09-10T12:01:00.123456Z"));
    assert_eq!(browser.send(&app, "POST", "/csp-report", None, Csrf::None, &[]).await.status, 204);
    drop(dir);
}

async fn sign_in_email_matches_recorded_strip(case: &str) {
    let vector: serde_json::Value = serde_json::from_str(include_str!("fixtures/settings_r9_vectors.json")).unwrap();
    let recorded = &vector["sign_in_email"][case];
    let email = recorded["submitted"].as_str().unwrap();
    let expected = recorded["lookup_matches"].as_bool().unwrap();
    assert_eq!(expected, case == "nul", "Rails keeps Unicode spaces and strips NUL");
    let (dir, opened, seed) = fixture::install();
    let c = rusqlite::Connection::open(dir.path().join("production.sqlite3")).unwrap();
    c.execute(
        "UPDATE users SET email='strip@example.com', encrypted_password=?1, confirmed_at='2026-01-01 00:00:00', setup_completed=1 WHERE id=?2",
        (deltabadger::crypto::hash_password("Correct-horse-9").unwrap(), seed.user_id),
    ).unwrap();
    let env = web::env(web::SECRET);
    let app = App::new(Config::from_env(&env).unwrap(), &env, opened.primary,
        web::TestClock::at("2026-09-10T12:00:30.123456Z")).unwrap()
        .with_figure_source(deltabadger::web::figure::loading::Source::Disabled).unwrap();
    let mut browser = Browser::default();
    assert_eq!(browser.get(&app, "/login").await.status, 200);
    let answer = browser.post(&app, "/login", &[("user[email]", email), ("user[password]", "Correct-horse-9")]).await;
    assert_eq!(answer.status, if expected { 303 } else { 422 }, "Rails String#strip sign-in case {case}");
    let data = deltabadger::web::session::open(&app.keys.session,
        browser.cookie.as_ref().unwrap(), web::at("2026-09-10T12:00:30.123456Z")).unwrap();
    assert_eq!(data.user.is_some(), expected, "sign-in session for {case}");
    assert_eq!(c.query_row("SELECT failed_attempts FROM users WHERE id=?1", [seed.user_id], |r| r.get::<_, i64>(0)).unwrap(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn sign_in_email_nbsp_is_refused_as_rails_records() { sign_in_email_matches_recorded_strip("nbsp").await; }
#[tokio::test(flavor = "current_thread")]
async fn sign_in_email_em_space_is_refused_as_rails_records() { sign_in_email_matches_recorded_strip("em_space").await; }
#[tokio::test(flavor = "current_thread")]
async fn sign_in_email_nul_is_stripped_as_rails_records() { sign_in_email_matches_recorded_strip("nul").await; }
