//! W1's safety boundaries and V1 reads, exercised through authenticated web requests.
mod common;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use common::{seed, web::TestClock};
use deltabadger::web;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

const AT: &str = "2026-10-01T12:00:00Z";
const ADDRESS: &str = "bc1qwithdrawaltest";

fn picker(body: &str) -> (String, Vec<(String, String, bool)>) {
    let doc = scraper::Html::parse_document(body);
    let select = doc
        .select(&scraper::Selector::parse(".tag--address select").unwrap())
        .next()
        .expect("address picker");
    (
        select.value().attr("name").unwrap().to_string(),
        select
            .select(&scraper::Selector::parse("option").unwrap())
            .map(|e| {
                (
                    e.value().attr("value").unwrap().to_string(),
                    e.text().collect(),
                    e.value().attr("selected").is_some(),
                )
            })
            .collect(),
    )
}

async fn identity_listing(venue: &str, identities: &[&str]) -> (Install, MockServer) {
    use deltabadger::venue::withdrawal as w;
    let server = MockServer::start().await;
    let rows: Vec<Value> = identities
        .iter()
        .map(|identity| match venue {
            "Binance" => {
                json!({"coin":"BTC","address":ADDRESS,"network":"BTC","addressTag":identity})
            }
            "Mexc" => json!({"coin":"BTC","address":ADDRESS,"network":"BTC","memo":identity}),
            "Kraken" => json!({"address":ADDRESS,"key":identity,"verified":true}),
            _ => unreachable!(),
        })
        .collect();
    let (path, body) = match venue {
        "Binance" => (w::BINANCE_ADDRESSES, json!(rows)),
        "Mexc" => (w::MEXC_ADDRESSES, json!({"data":rows,"totalPageNum":1})),
        _ => (w::KRAKEN_ADDRESSES, json!({"error":[],"result":rows})),
    };
    Mock::given(matchers::path(path))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    if venue != "Kraken" {
        let network = if venue == "Mexc" { "Bitcoin" } else { "BTC" };
        Mock::given(matchers::path(if venue == "Binance" { w::BINANCE_COINS } else { w::MEXC_COINS }))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"coin":"BTC","networkList":[{"network":network,"netWork":"BTC","withdrawEnable":true,"withdrawTag":true,"withdrawFee":"0.0001"}]}])))
            .mount(&server).await;
    }
    let mut i = Install::new(venue);
    let class = match venue {
        "Binance" => "Exchanges::Binance",
        "Mexc" => "Exchanges::Mexc",
        "Kraken" => "Exchanges::Kraken",
        _ => unreachable!(),
    };
    i.app = i.app.with_venue_key_url(class, &server.uri()).unwrap();
    (i, server)
}

async fn select_each_identity(venue: &str) {
    // Quotes and form delimiters must round trip through HTML and form encoding.
    let identities = ["first & <memo>\"", "second + key"];
    for choice in 0..2 {
        let (i, _server) = identity_listing(venue, &identities).await;
        let (_, body) = i
            .send(
                "GET",
                "/rules/withdrawals/confirm_settings/new",
                "",
                Some(i.config()),
                true,
                false,
            )
            .await;
        let (name, options) = picker(&body);
        assert_eq!(options.len(), 2);
        assert_ne!(
            options[0].0, options[1].0,
            "D-RULE-LISTED-ENTRY: distinct submitted identities"
        );
        assert_ne!(
            options[0].1, options[1].1,
            "the user can distinguish destinations"
        );
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                (&name[..], &options[choice].0[..]),
                ("min_amount", "0.2"),
                ("threshold_type", "min_amount"),
            ])
            .finish();
        // Preview must retain the chosen identity before confirmation.
        let (status, preview) = i
            .send(
                "POST",
                "/rules/withdrawals/confirm_settings/preview",
                &form,
                Some(i.config()),
                true,
                true,
            )
            .await;
        assert_eq!(status, 200);
        let (_, selected) = picker(&preview);
        assert_eq!(selected.iter().filter(|o| o.2).count(), 1);
        assert!(selected[choice].2);
        let (status, body) = i
            .send(
                "POST",
                "/rules/withdrawals/confirm_settings",
                &form,
                Some(i.config()),
                true,
                true,
            )
            .await;
        assert_eq!(status, 200, "{venue}: {body}");
        let (address, settings): (String, String) =
            i.c.query_row("SELECT address,settings FROM rules", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(i.app.cipher.decrypt(&address).unwrap(), ADDRESS);
        let settings: Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(
            settings[if venue == "Kraken" {
                "address_name"
            } else {
                "address_tag"
            }],
            identities[choice]
        );
        if venue != "Kraken" {
            assert_eq!(
                settings["network"],
                if venue == "Mexc" { "Bitcoin" } else { "BTC" }
            );
        }
        // Changing the selection on a stopped tile uses the same identity contract.
        i.c.execute("UPDATE rules SET status=2", []).unwrap();
        let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
        let (name, options) = picker(&body);
        assert_eq!(options.iter().filter(|o| o.2).count(), 1);
        assert!(options[choice].2);
        let id: i64 =
            i.c.query_row("SELECT id FROM rules", [], |r| r.get(0))
                .unwrap();
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                (&name[..], &options[choice].0[..]),
                ("rules_withdrawal[min_amount]", "0.7"),
            ])
            .finish();
        assert_eq!(
            i.send(
                "PATCH",
                &format!("/rules/withdrawals/{id}"),
                &form,
                None,
                true,
                true
            )
            .await
            .0,
            200
        );
        let text: String =
            i.c.query_row("SELECT settings FROM rules", [], |r| r.get(0))
                .unwrap();
        let updated: Value = serde_json::from_str(&text).unwrap();
        for field in ["network", "address_tag", "address_name"] {
            assert_eq!(
                updated[field], settings[field],
                "{venue}: unrelated tile edit changed {field}"
            );
        }
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([(&name, &options[1 - choice].0)])
            .finish();
        let (status, body) = i
            .send(
                "PATCH",
                &format!("/rules/withdrawals/{id}"),
                &form,
                None,
                true,
                true,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let settings: String =
            i.c.query_row("SELECT settings FROM rules", [], |r| r.get(0))
                .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&settings).unwrap()[if venue == "Kraken" {
                "address_name"
            } else {
                "address_tag"
            }],
            identities[1 - choice]
        );
    }
}

#[tokio::test]
async fn picker_selects_each_memo_at_the_same_address_and_network() {
    for venue in ["Binance", "Mexc"] {
        select_each_identity(venue).await;
    }
}

#[tokio::test]
async fn picker_selects_each_kraken_key_at_the_same_address() {
    select_each_identity("Kraken").await;
}

#[tokio::test]
async fn tile_settings_keep_network_tag_and_key() {
    let (i, _server) = identity_listing("Kraken", &["first", "second"]).await;
    let id = i.rule(2);
    i.c.execute("UPDATE rules SET settings=json_set(settings,'$.network','legacy network','$.address_tag','legacy tag','$.address_name','second')", []).unwrap();
    for submit_picker in [false, true] {
        let mut fields = vec![
            (
                "rules_withdrawal[min_amount]".to_string(),
                "0.7".to_string(),
            ),
            ("rules_withdrawal[max_interval]".into(), "12".into()),
            (
                "rules_withdrawal[threshold_type]".into(),
                "min_amount".into(),
            ),
        ];
        if submit_picker {
            let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
            let (name, options) = picker(&body);
            let selected = options.iter().find(|o| o.2).unwrap();
            fields.push((name, selected.0.clone()));
        }
        let (status, body) = i
            .send(
                "PATCH",
                &format!("/rules/withdrawals/{id}"),
                &form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(fields)
                    .finish(),
                None,
                true,
                true,
            )
            .await;
        assert_eq!(status, 200, "{body}");
        let text: String =
            i.c.query_row("SELECT settings FROM rules", [], |r| r.get(0))
                .unwrap();
        let settings: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(settings["address_name"], "second");
        assert_eq!(settings["address_tag"], "legacy tag");
        assert_eq!(settings["network"], "legacy network");
        assert_eq!(settings["min_amount"], "0.7");
        assert_eq!(settings["max_interval"], "12");
    }
}

#[tokio::test]
async fn picker_refuses_identical_entries_and_forged_identities_without_writing() {
    for venue in ["Binance", "Mexc", "Kraken"] {
        let (i, _server) = identity_listing(venue, &["same", "same"]).await;
        let (_, body) = i
            .send(
                "GET",
                "/rules/withdrawals/confirm_settings/new",
                "",
                Some(i.config()),
                true,
                false,
            )
            .await;
        let (name, options) = picker(&body);
        assert_eq!(options[0].0, options[1].0);
        for value in [
            &options[0].0,
            "{invalid",
            r#"{"address":"forged","network":"BTC","tag":"same","key":"same"}"#,
        ] {
            let form = form_urlencoded::Serializer::new(String::new())
                .extend_pairs([(&name[..], value)])
                .finish();
            assert_eq!(
                i.send(
                    "POST",
                    "/rules/withdrawals/confirm_settings",
                    &form,
                    Some(i.config()),
                    true,
                    true
                )
                .await
                .0,
                302
            );
            assert_eq!(i.rows(), "[]");
        }
        let id = i.rule(2);
        let before = i.rows();
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([("rules_withdrawal[destination]", &options[0].0)])
            .finish();
        assert_eq!(
            i.send(
                "PATCH",
                &format!("/rules/withdrawals/{id}"),
                &form,
                None,
                true,
                true
            )
            .await
            .0,
            422
        );
        assert_eq!(i.rows(), before);
    }
}

#[tokio::test]
async fn picker_rechecks_every_posted_identity_field_against_the_listing() {
    for venue in ["Binance", "Mexc", "Kraken"] {
        let (i, _server) = identity_listing(venue, &["first", "second"]).await;
        let (_, body) = i
            .send(
                "GET",
                "/rules/withdrawals/confirm_settings/new",
                "",
                Some(i.config()),
                true,
                false,
            )
            .await;
        let (name, options) = picker(&body);
        let identity: Value = serde_json::from_str(&options[0].0).unwrap();
        let id = i.rule(2);
        let before = i.rows();
        for field in ["address", "network", "tag", "key"] {
            let mut forged = identity.clone();
            forged[field] = json!("unlisted");
            let value = forged.to_string();
            let form = form_urlencoded::Serializer::new(String::new())
                .extend_pairs([(&name[..], &value)])
                .finish();
            assert_eq!(
                i.send(
                    "POST",
                    "/rules/withdrawals/confirm_settings",
                    &form,
                    Some(i.config()),
                    true,
                    true
                )
                .await
                .0,
                302,
                "{venue} {field}"
            );
            let form = form_urlencoded::Serializer::new(String::new())
                .extend_pairs([("rules_withdrawal[destination]", &value)])
                .finish();
            assert_eq!(
                i.send(
                    "PATCH",
                    &format!("/rules/withdrawals/{id}"),
                    &form,
                    None,
                    true,
                    true
                )
                .await
                .0,
                422,
                "{venue} {field}"
            );
            assert_eq!(i.rows(), before);
        }
    }
}

#[tokio::test]
async fn gemini_picker_preserves_v1_account_and_group_deduplication() {
    for network in [json!("bitcoin"), Value::Null] {
        let server = MockServer::start().await;
        let entry = json!({"address":ADDRESS,"network":network,"status":"active"});
        Mock::given(matchers::path("/v1/approvedAddresses/account/bitcoin"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"approvedAddresses":[entry,entry]})),
            )
            .mount(&server)
            .await;
        let mut i = Install::new("Gemini");
        i.app = i
            .app
            .with_venue_key_url("Exchanges::Gemini", &server.uri())
            .unwrap();
        let (_, body) = i
            .send(
                "GET",
                "/rules/withdrawals/confirm_settings/new",
                "",
                Some(i.config()),
                true,
                false,
            )
            .await;
        let (name, options) = picker(&body);
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].0, options[1].0);
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([(&name, &options[0].0)])
            .finish();
        assert_eq!(
            i.send(
                "POST",
                "/rules/withdrawals/confirm_settings",
                &form,
                Some(i.config()),
                true,
                true
            )
            .await
            .0,
            200
        );
        let settings: String =
            i.c.query_row("SELECT settings FROM rules", [], |r| r.get(0))
                .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&settings).unwrap()["network"],
            "bitcoin"
        );
    }
}
struct Install {
    _dir: tempfile::TempDir,
    app: web::App,
    c: Connection,
    s: seed::Seeded,
}
impl Install {
    fn new(venue: &str) -> Self {
        let dir = common::rails_install();
        let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
        let s = seed::seed_kraken(&c, &seed::cipher());
        c.execute("UPDATE users SET confirmed_at=created_at", [])
            .unwrap();
        c.execute(
            "UPDATE exchanges SET type=?1,name=?2",
            (format!("Exchanges::{venue}"), venue),
        )
        .unwrap();
        c.execute("UPDATE tickers SET base='BTC'", []).unwrap();
        c.execute("UPDATE api_keys SET key_type=1", []).unwrap();
        c.execute("INSERT INTO exchange_assets(exchange_id,asset_id,withdrawal_chains,created_at,updated_at) VALUES (?1,?2,'[]','2026-01-01','2026-01-01')",(s.exchange_id,s.btc)).unwrap();
        let app = common::web::app(dir.path(), "engine-test-secret", TestClock::at(AT));
        Self {
            _dir: dir,
            app,
            c,
            s,
        }
    }
    fn rule(&self, status: i64) -> i64 {
        self.c.execute("INSERT INTO rules(user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES (?1,'Rules::Withdrawal',?2,?3,?4,?5,?6,'2026-01-01','2026-01-01')",
            params![self.s.user_id,status,self.s.exchange_id,self.s.btc,self.app.cipher.encrypt(ADDRESS),json!({"threshold_type":"min_amount","min_amount":"0.1","withdrawal_percentage":"100"}).to_string()]).unwrap();
        self.c.last_insert_rowid()
    }
    async fn send(
        &self,
        method: &str,
        path: &str,
        form: &str,
        config: Option<Value>,
        authenticated: bool,
        csrf: bool,
    ) -> (u16, String) {
        let token = web::csrf::new_token();
        let data = web::session::SessionData {
            user: authenticated.then_some((self.s.user_id, "x".into())),
            csrf: Some(token.clone()),
            withdrawal_rule_config: config,
            ..Default::default()
        };
        let cookie = format!(
            "{}={}",
            web::session::COOKIE,
            web::session::seal(&self.app.keys.session, &data, self.app.now())
        );
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost:3000")
            .header("cookie", cookie)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "text/vnd.turbo-stream.html, text/html");
        if csrf {
            req = req.header("x-csrf-token", web::csrf::masked(&token))
        }
        let response = web::router(self.app.clone())
            .oneshot(req.body(Body::from(form.to_string())).unwrap())
            .await
            .unwrap();
        (
            response.status().as_u16(),
            String::from_utf8(
                to_bytes(response.into_body(), 8_000_000)
                    .await
                    .unwrap()
                    .to_vec(),
            )
            .unwrap(),
        )
    }
    fn config(&self) -> Value {
        json!({"exchange_id":self.s.exchange_id.to_string(),"asset_id":self.s.btc.to_string(),"address":ADDRESS})
    }
    fn rows(&self) -> String {
        let mut st = self
            .c
            .prepare("SELECT id,status,address,settings,updated_at FROM rules ORDER BY id")
            .unwrap();
        format!(
            "{:?}",
            st.query_map([], |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?
            )))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
        )
    }
}
async fn gemini(server: &MockServer, network: &str) {
    Mock::given(matchers::path("/v1/approvedAddresses/account/bitcoin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"approvedAddresses":[{"address":ADDRESS,"network":network,"status":"active"}]}),
        ))
        .mount(server)
        .await;
}
#[tokio::test]
async fn auth_csrf_and_owner_scope_refuse_without_writing() {
    let i = Install::new("Gemini");
    let id = i.rule(2);
    let before = i.rows();
    for (method, path, authenticated, csrf, expected) in [
        ("GET", "/rules".to_string(), false, false, 302),
        (
            "PATCH",
            format!("/rules/withdrawals/{id}"),
            true,
            false,
            302,
        ),
        (
            "DELETE",
            format!("/rules/withdrawals/{}", id + 1),
            true,
            true,
            404,
        ),
    ] {
        assert_eq!(
            i.send(
                method,
                &path,
                "rules_withdrawal[status]=scheduled",
                None,
                authenticated,
                csrf
            )
            .await
            .0,
            expected
        );
        assert_eq!(i.rows(), before);
    }
    i.c.execute("INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES (?1,'other@example.com','x','2026-01-01 00:00:00','2026-01-01 00:00:00')",[i.s.user_id+100]).unwrap();
    i.c.execute("UPDATE rules SET user_id=user_id+100 WHERE id=?1", [id])
        .unwrap();
    let before = i.rows();
    for method in ["PATCH", "DELETE"] {
        assert_eq!(
            i.send(
                method,
                &format!("/rules/withdrawals/{id}"),
                "rules_withdrawal[status]=scheduled",
                None,
                true,
                true
            )
            .await
            .0,
            404
        );
        assert_eq!(i.rows(), before);
    }
}
#[tokio::test]
async fn wizard_persists_only_a_full_listed_destination_encrypted() {
    let server = MockServer::start().await;
    gemini(&server, "bitcoin").await;
    let mut i = Install::new("Gemini");
    i.app = i
        .app
        .with_venue_key_url("Exchanges::Gemini", &server.uri())
        .unwrap();
    let config = i.config();
    for form in ["address=not-listed", "network=ethereum"] {
        assert_eq!(
            i.send(
                "POST",
                "/rules/withdrawals/confirm_settings",
                form,
                Some(config.clone()),
                true,
                true
            )
            .await
            .0,
            302
        );
        assert_eq!(i.rows(), "[]");
    }
    assert_eq!(
        i.send(
            "POST",
            "/rules/withdrawals/confirm_settings",
            "network=bitcoin&min_amount=0.1&threshold_type=min_amount",
            Some(config),
            true,
            true
        )
        .await
        .0,
        200
    );
    let (status, address, settings): (i64, String, String) =
        i.c.query_row("SELECT status,address,settings FROM rules", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!(status, 0);
    assert!(!address.contains(ADDRESS));
    assert_eq!(i.app.cipher.decrypt(&address).unwrap(), ADDRESS);
    assert_eq!(
        serde_json::from_str::<Value>(&settings).unwrap()["network"],
        "bitcoin"
    );
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|r| r.url.path().contains("approvedAddresses")));
}
#[tokio::test]
async fn unresolved_and_dispatched_statuses_and_legacy_error_redaction() {
    let i = Install::new("Gemini");
    let id = i.rule(1);
    i.c.execute_batch("CREATE TABLE rust_withdrawal_intents(owner TEXT PRIMARY KEY,state TEXT)")
        .unwrap();
    i.c.execute(
        "INSERT INTO rust_withdrawal_intents VALUES (?1,'dispatched')",
        [format!("rule:{id}")],
    )
    .unwrap();
    i.c.execute("INSERT INTO rule_logs(rule_id,status,message,details,created_at) VALUES (?1,2,?2,'{}','2026-01-01 00:00:00')",
        (id,"Withdrawal failed: GET https://api.example/withdraw?address=SECRETADDRESS&signature=SECRETSIGNATURE")).unwrap();
    for state in ["dispatched", "unresolved"] {
        i.c.execute("UPDATE rust_withdrawal_intents SET state=?1", [state])
            .unwrap();
        let (status, body) = i.send("GET", "/rules", "", None, true, false).await;
        assert_eq!(status, 200);
        assert!(body.contains("Withdrawal status unconfirmed"));
        assert!(body.contains("not retried"));
        assert!(body.contains("resolve-withdrawal"));
        for secret in ["SECRETADDRESS", "SECRETSIGNATURE", "https://api.example"] {
            assert!(!body.contains(secret));
        }
    }
}
#[tokio::test]
async fn stale_preview_reads_v1_fee_estimate_and_stamps_only_its_asset() {
    let server = MockServer::start().await;
    gemini(&server, "bitcoin").await;
    Mock::given(matchers::path("/v2/withdraw/bitcoin/btc/feeEstimate"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"fee":"0.0002","currency":"BTC"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut i = Install::new("Gemini");
    i.app = i
        .app
        .with_venue_key_url("Exchanges::Gemini", &server.uri())
        .unwrap();
    let (status, body) = i
        .send(
            "GET",
            "/rules/withdrawals/confirm_settings/new",
            "",
            Some(i.config()),
            true,
            false,
        )
        .await;
    assert_eq!(status, 200);
    assert!(body.contains("0.0002"));
    let fee: String =
        i.c.query_row(
            "SELECT withdrawal_fee FROM exchange_assets WHERE asset_id=?1",
            [i.s.btc],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fee, "0.0002");
    assert_eq!(i.rows(), "[]");
    assert!(server.received_requests().await.unwrap().iter().all(|r| r
        .url
        .path()
        .contains("approvedAddresses")
        || r.url.path().ends_with("feeEstimate")));
}

#[tokio::test]
async fn every_venue_uses_v1_destination_selection_and_only_reads() {
    use deltabadger::venue::withdrawal as w;
    for (class, path, listing, coins, network, key) in [
        (
            "Exchanges::Binance",
            w::BINANCE_ADDRESSES,
            json!([{"coin":"BTC","address":ADDRESS,"network":"BTC","addressTag":""}]),
            Some((
                w::BINANCE_COINS,
                json!([{"coin":"BTC","networkList":[{"network":"BTC","withdrawEnable":true,"withdrawTag":false,"withdrawFee":"0.0001","isDefault":true}]}]),
            )),
            "BTC",
            None,
        ),
        (
            "Exchanges::Mexc",
            w::MEXC_ADDRESSES,
            json!({"data":[{"coin":"BTC","address":ADDRESS,"network":"Bitcoin","memo":""}],"totalPageNum":1}),
            Some((
                w::MEXC_COINS,
                json!([{"coin":"BTC","networkList":[{"network":"Bitcoin","netWork":"BTC","withdrawEnable":true,"withdrawFee":"0.0001","isDefault":true}]}]),
            )),
            "Bitcoin",
            None,
        ),
        (
            "Exchanges::Kraken",
            "/0/private/WithdrawAddresses",
            json!({"error":[],"result":[{"address":ADDRESS,"key":"cold","verified":true}]}),
            None,
            "",
            Some("cold"),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(matchers::path(path))
            .respond_with(ResponseTemplate::new(200).set_body_json(listing))
            .mount(&server)
            .await;
        if let Some((path, body)) = coins {
            Mock::given(matchers::path(path))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
        }
        let mut i = Install::new(class.strip_prefix("Exchanges::").unwrap());
        i.app = i.app.with_venue_key_url(class, &server.uri()).unwrap();
        let form = format!("network={network}&threshold_type=min_amount&min_amount=0.1");
        assert_eq!(
            i.send(
                "POST",
                "/rules/withdrawals/confirm_settings",
                &form,
                Some(i.config()),
                true,
                true
            )
            .await
            .0,
            200,
            "{class}"
        );
        let text: String =
            i.c.query_row("SELECT settings FROM rules", [], |r| r.get(0))
                .unwrap();
        let settings: Value = serde_json::from_str(&text).unwrap();
        if let Some(key) = key {
            assert_eq!(settings["address_name"], key)
        } else {
            assert_eq!(settings["network"], network)
        }
        assert!(server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| matches!(
                r.url.path(),
                w::BINANCE_ADDRESSES
                    | w::BINANCE_COINS
                    | w::MEXC_ADDRESSES
                    | w::MEXC_COINS
                    | "/0/private/WithdrawAddresses"
            )));
    }
}

#[tokio::test]
async fn deletion_accepts_only_created_or_stopped() {
    for status in 0..4 {
        let i = Install::new("Gemini");
        let id = i.rule(status);
        let before = i.rows();
        let response = i
            .send(
                "DELETE",
                &format!("/rules/withdrawals/{id}"),
                "",
                None,
                true,
                true,
            )
            .await;
        if matches!(status, 0 | 2) {
            assert_eq!(response.0, 200);
            assert_eq!(
                i.c.query_row("SELECT status FROM rules", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                3
            );
        } else {
            assert_eq!(response.0, 422);
            assert_eq!(i.rows(), before);
        }
    }
}

#[tokio::test]
async fn listing_from_a_replaced_key_cannot_save_a_destination() {
    let server = MockServer::start().await;
    let mut i = Install::new("Gemini");
    i.app = i
        .app
        .with_venue_key_url("Exchanges::Gemini", &server.uri())
        .unwrap();
    let path = i._dir.path().join("production.sqlite3");
    let secret = i.app.cipher.encrypt("replacement-secret");
    Mock::given(matchers::path("/v1/approvedAddresses/account/bitcoin"))
        .respond_with(move |_: &wiremock::Request| {
            Connection::open(&path).unwrap().execute("UPDATE api_keys SET secret=?1", [&secret]).unwrap();
            ResponseTemplate::new(200).set_body_json(json!({"approvedAddresses":[{"address":ADDRESS,"network":"bitcoin","status":"active"}]}))
        }).mount(&server).await;
    let response = i
        .send(
            "POST",
            "/rules/withdrawals/confirm_settings",
            "",
            Some(i.config()),
            true,
            true,
        )
        .await;
    assert_eq!(response.0, 302);
    assert_eq!(i.rows(), "[]");
}

#[tokio::test]
async fn an_unknown_fee_does_not_stamp_a_false_zero() {
    let server = MockServer::start().await;
    gemini(&server, "bitcoin").await;
    Mock::given(matchers::path("/v2/withdraw/bitcoin/btc/feeEstimate"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"currency":"BTC"})))
        .expect(1)
        .mount(&server)
        .await;
    let mut i = Install::new("Gemini");
    i.app = i
        .app
        .with_venue_key_url("Exchanges::Gemini", &server.uri())
        .unwrap();
    assert_eq!(
        i.send(
            "GET",
            "/rules/withdrawals/confirm_settings/new",
            "",
            Some(i.config()),
            true,
            false
        )
        .await
        .0,
        200
    );
    let row:(Option<String>,Option<String>)=i.c.query_row("SELECT withdrawal_fee,withdrawal_fee_updated_at FROM exchange_assets WHERE asset_id=?1",[i.s.btc],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(row, (None, None));
}

#[tokio::test]
async fn coin_fee_preview_prefers_fee_key_and_falls_back_to_withdrawal_key() {
    use deltabadger::venue::withdrawal as w;
    for configured in [false, true] {
        let server = MockServer::start().await;
        let mut i = Install::new("Binance");
        i.app = i
            .app
            .with_venue_key_url("Exchanges::Binance", &server.uri())
            .unwrap();
        if configured {
            i.c.execute(
                "INSERT INTO fee_api_keys(exchange_id,key,secret) VALUES (?1,?2,?3)",
                params![
                    i.s.exchange_id,
                    i.app.cipher.encrypt("fee-key"),
                    i.app.cipher.encrypt("fee-secret")
                ],
            )
            .unwrap();
        }
        Mock::given(matchers::path(w::BINANCE_ADDRESSES))
            .and(matchers::header("X-MBX-APIKEY", "test-key"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([{"coin":"BTC","address":ADDRESS,"network":"BTC"}])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(matchers::path(w::BINANCE_COINS)).and(matchers::header("X-MBX-APIKEY",if configured {"fee-key"} else {"test-key"}))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"coin":"BTC","networkList":[{"network":"BTC","withdrawEnable":true,"withdrawTag":false,"withdrawFee":"0.0003","isDefault":true}]}]))).expect(1).mount(&server).await;
        i.c.execute("INSERT INTO exchange_assets(exchange_id,asset_id,withdrawal_fee,created_at,updated_at) VALUES (?1,?2,'7','2026-01-01','2026-01-01')",(i.s.exchange_id,i.s.quote)).unwrap();
        assert_eq!(
            i.send(
                "GET",
                "/rules/withdrawals/confirm_settings/new",
                "",
                Some(i.config()),
                true,
                false
            )
            .await
            .0,
            200
        );
        let fee: String =
            i.c.query_row(
                "SELECT withdrawal_fee FROM exchange_assets WHERE asset_id=?1",
                [i.s.btc],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fee, "0.0003");
        let other: String =
            i.c.query_row(
                "SELECT withdrawal_fee FROM exchange_assets WHERE asset_id=?1",
                [i.s.quote],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(other, "7");
    }
}

#[tokio::test]
async fn refused_addresses_are_not_counted_as_available_wallets() {
    for (class, path, body) in [
        (
            "Exchanges::Gemini",
            "/v1/approvedAddresses/account/bitcoin",
            json!({"approvedAddresses":[{"address":ADDRESS,"network":"bitcoin","status":"pending"}]}),
        ),
        (
            "Exchanges::Kraken",
            "/0/private/WithdrawAddresses",
            json!({"error":[],"result":[{"address":ADDRESS,"key":"cold","verified":false}]}),
        ),
    ] {
        let server = MockServer::start().await;
        Mock::given(matchers::path(path))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let mut i = Install::new(class.strip_prefix("Exchanges::").unwrap());
        i.app = i.app.with_venue_key_url(class, &server.uri()).unwrap();
        i.rule(2);
        let (status, body) = i.send("GET", "/rules", "", None, true, false).await;
        assert_eq!(status, 200);
        assert!(!body.contains("name=\"rules_withdrawal[destination]\""));
        let expected = web::i18n::t(
            "en",
            "rules.tile.no_wallet_html",
            &[
                ("symbol", web::i18n::Arg::Text("BTC")),
                (
                    "exchange",
                    web::i18n::Arg::Text(class.strip_prefix("Exchanges::").unwrap()),
                ),
            ],
        );
        assert!(body.contains(&expected));
    }
}
