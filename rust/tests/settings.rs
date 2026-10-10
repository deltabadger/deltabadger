//! Ordinary browser tests exercise the production handlers; only exchange HTTP and the application logger are captured.
mod common;
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
    async fn with_hook(url:String, hook:Option<deltabadger::web::PasswordHook>)->Self{
        let (dir, opened, seed) = fixture::install();
        let clock = web::TestClock::at("2026-09-10T12:00:30.123456Z");
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