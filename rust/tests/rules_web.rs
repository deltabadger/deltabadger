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

use web::rules::service::{self, Command, Patch};

#[tokio::test]
async fn service_stop_consumes_without_logging_and_preserves_intent() {
    let i = Install::new("Gemini");
    let id = i.rule(1);
    i.c.execute(
        "INSERT INTO rust_rule_evaluations(rule_id,source,created_at) VALUES (?1,'start:test',?2)",
        (id, AT),
    )
    .unwrap();
    i.c.execute("INSERT INTO rust_withdrawal_intents(owner,state,client_id,created_at,api_key_id,credential_digest,host,account_scope,key_identity,params_enc,fee_basis,budget,free_balance) VALUES (?1,'unresolved','test',?2,1,'digest','host','spot','key','encrypted','flat','1','1')", (format!("rule:{id}"), AT)).unwrap();
    service::call(&i.app, i.s.user_id, Command::Stop(id))
        .await
        .unwrap()
        .unwrap();
    let row: (Option<String>, String) =
        i.c.query_row(
            "SELECT consumed_at,outcome FROM rust_rule_evaluations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(row.0.is_some());
    assert_eq!(row.1, "skipped", "D-RULE-SKIP-CONSUMED");
    assert_eq!(
        i.c.query_row("SELECT count(*) FROM rule_logs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        i.c.query_row("SELECT count(*) FROM rust_withdrawal_intents", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    for cmd in [
        Command::Delete(id),
        Command::Start(id, Patch::default()),
        Command::Update(id, Patch::settings(json!({"min_amount":"0.2"}))),
    ] {
        assert_eq!(
            service::call(&i.app, i.s.user_id, cmd)
                .await
                .unwrap()
                .unwrap_err()
                .code,
            "withdrawal_unconfirmed"
        );
    }
    let rows = service::list(&i.c, &i.app, i.s.user_id).unwrap();
    assert_eq!(
        rows[0].unresolved_withdrawal.as_ref().unwrap().state,
        "unresolved"
    );
    assert_eq!(rows[0].address, "bc1qwi…test");
}

#[tokio::test]
async fn service_delete_revive_and_update_relist() {
    let server = MockServer::start().await;
    gemini(&server, "bitcoin").await;
    let mut i = Install::new("Gemini");
    i.app = i
        .app
        .with_venue_key_url("Exchanges::Gemini", &server.uri())
        .unwrap();
    let id = i.rule(2);
    i.c.execute(
        "INSERT INTO rust_rule_evaluations(rule_id,source,created_at) VALUES (?1,'start:test',?2)",
        (id, AT),
    )
    .unwrap();
    service::call(&i.app, i.s.user_id, Command::Delete(id))
        .await
        .unwrap()
        .unwrap();
    let patch = Patch {
        address: Some(ADDRESS.into()),
        settings: json!({"threshold_type":"min_amount","min_amount":"0.3","max_interval":"7"}),
        ..Default::default()
    };
    let revived = service::call(
        &i.app,
        i.s.user_id,
        Command::Create {
            exchange_id: i.s.exchange_id,
            asset_id: i.s.btc,
            patch,
        },
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(revived.id, id);
    assert_eq!(revived.status, "created");
    assert_eq!(
        i.c.query_row(
            "SELECT count(*) FROM rust_rule_evaluations WHERE consumed_at IS NULL",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    service::call(
        &i.app,
        i.s.user_id,
        Command::Update(id, Patch::settings(json!({"min_amount":"0.4"}))),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "every write re-lists"
    );
    server.reset().await;
    let before = i.rows();
    assert_eq!(
        service::call(
            &i.app,
            i.s.user_id,
            Command::Update(id, Patch::settings(json!({"min_amount":"0.5"})))
        )
        .await
        .unwrap()
        .unwrap_err()
        .code,
        "address_not_listed"
    );
    assert_eq!(i.rows(), before);
}

#[tokio::test]
async fn service_start_refuses_unsupported_disabled_missing_key_and_retired() {
    for (venue, key, code) in [
        ("BinanceUs", true, "withdrawal_unsupported"),
        ("Gemini", true, "withdrawal_disabled"),
        ("Gemini", false, "withdrawal_key_missing"),
        ("Bitmart", true, "exchange_retired"),
    ] {
        let i = Install::new(venue);
        let id = i.rule(0);
        if !key {
            i.c.execute("DELETE FROM api_keys", []).unwrap();
        }
        let before = i.rows();
        assert_eq!(
            service::call(&i.app, i.s.user_id, Command::Start(id, Patch::default()))
                .await
                .unwrap()
                .unwrap_err()
                .code,
            code,
            "{venue}"
        );
        assert_eq!(i.rows(), before);
        assert_eq!(
            i.c.query_row("SELECT count(*) FROM rust_rule_evaluations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}

#[test]
fn withdrawal_migration_is_registered_and_idempotent() {
    let i = Install::new("Gemini");
    assert!(deltabadger::store::test_support::migrate(&i.c)
        .unwrap()
        .is_empty());
    for name in [
        "rust_rule_evaluations",
        "rust_withdrawal_intents",
        "rust_withdrawal_ids",
        "rust_rails_attempt_acks",
        "rust_rails_job_acks",
        "rust_recorder_runs",
    ] {
        assert!(deltabadger::store::REGISTRY.owned_tables().contains(&name));
        assert_eq!(
            i.c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [name],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
    }
}

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
            .filter(|e| e.value().attr("disabled").is_none())
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

// Submit the tile's successful controls in document order, including Rails' hidden
// stopped status before the checkbox. Only the user's edited control is changed.
async fn submit_rendered_tile(
    i: &Install,
    id: i64,
    amount: Option<&str>,
    checked: Option<bool>,
) -> common::web::Answer {
    use common::web::{Browser, Csrf};
    let data = web::session::SessionData {
        user: Some((i.s.user_id, "x".into())),
        ..Default::default()
    };
    let mut browser = Browser {
        cookie: Some(web::session::seal(&i.app.keys.session, &data, i.app.now())),
        ..Default::default()
    };
    let page = browser.get(&i.app, "/rules").await;
    assert_eq!(page.status, 200);
    let doc = scraper::Html::parse_document(&page.body);
    let form = doc
        .select(&scraper::Selector::parse(&format!("form#rules_withdrawal_{id}")).unwrap())
        .next()
        .expect("rendered rule form");
    let mut fields = Vec::new();
    for control in form.select(&scraper::Selector::parse("input[name], select[name]").unwrap()) {
        let node = control.value();
        if node.attr("disabled").is_some() {
            continue;
        }
        let name = node.attr("name").unwrap();
        if node.attr("type") == Some("checkbox")
            && !checked.unwrap_or_else(|| node.attr("checked").is_some())
        {
            continue;
        }
        let value = if node.name() == "select" {
            let options: Vec<_> = control
                .select(&scraper::Selector::parse("option").unwrap())
                .collect();
            let selected = options
                .iter()
                .find(|o| o.value().attr("selected").is_some())
                .or_else(|| options.first())
                .expect("select option");
            assert!(selected.value().attr("disabled").is_none());
            selected.value().attr("value").unwrap_or_default()
        } else if name == "rules_withdrawal[min_amount]" {
            amount.unwrap_or_else(|| node.attr("value").unwrap_or_default())
        } else {
            node.attr("value").unwrap_or_default()
        };
        fields.push((name.to_string(), value.to_string()));
    }
    assert!(fields.iter().any(|(k, v)| k == "_method" && v == "patch"));
    assert!(fields
        .iter()
        .any(|(k, v)| k == "rules_withdrawal[status]" && v == "stopped"));
    browser
        .send_body(
            &i.app,
            &form.value().attr("method").unwrap().to_uppercase(),
            form.value().attr("action").unwrap(),
            Some(
                form_urlencoded::Serializer::new(String::new())
                    .extend_pairs(fields)
                    .finish(),
            ),
            Csrf::None,
            &[
                ("content-type", "application/x-www-form-urlencoded"),
                ("accept", "text/vnd.turbo-stream.html"),
            ],
        )
        .await
}

async fn edit_rendered_tile(status: i64) {
    let (i, _server) = identity_listing("Kraken", &["cold"]).await;
    let id = i.rule(status);
    let response = submit_rendered_tile(&i, id, Some("0.7"), None).await;
    assert_eq!(response.status, 200, "{}", response.body);
    assert!(response.body.contains(&format!(
        "<turbo-stream action=\"replace\" target=\"rules_withdrawal_{id}\">"
    )));
    let saved: (i64, String) = i
        .c
        .query_row(
            "SELECT status,json_extract(settings,'$.min_amount') FROM rules WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(saved, (status, "0.7".into()));
    assert_eq!(
        i.c.query_row("SELECT count(*) FROM rust_rule_evaluations", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn rendered_tile_edits_stopped_rule_amount_without_starting() {
    edit_rendered_tile(2).await;
}

#[tokio::test]
async fn rendered_tile_edits_created_rule_amount_without_stopping() {
    edit_rendered_tile(0).await;
}

#[tokio::test]
async fn rendered_tile_toggle_on_reaches_start_capability_guard() {
    let (i, _server) = identity_listing("Kraken", &["cold"]).await;
    let id = i.rule(2);
    let before = i.rows();
    let response = submit_rendered_tile(&i, id, None, Some(true)).await;
    // D-RULE-UNSUPPORTED-START: all venues currently disable withdrawals.
    assert_eq!(response.status, 422, "{}", response.body);
    assert!(response.body.contains("Withdrawals are not enabled for this exchange."));
    assert_eq!(i.rows(), before);
}

#[tokio::test]
async fn rendered_tile_toggle_off_stops_working_rule() {
    for status in [1, 4, 5, 6] {
        let (i, _server) = identity_listing("Kraken", &["cold"]).await;
        let id = i.rule(status);
        let response = submit_rendered_tile(&i, id, None, Some(false)).await;
        assert_eq!(response.status, 200, "status {status}: {}", response.body);
        assert!(response.body.contains("<turbo-stream action=\"refresh\""));
        assert_eq!(
            i.c.query_row("SELECT status FROM rules WHERE id=?1", [id], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}

#[tokio::test]
async fn picker_deduplicates_identical_entries_and_refuses_forged_identities() {
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
        assert_eq!(options.len(), 1);
        for value in [
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
            200
        );
        let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
        let (_, options) = picker(&body);
        assert_eq!(options.len(), 1);
        assert!(options[0].2);
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
            i.c.execute("UPDATE rules SET status=3", []).unwrap();
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
            i.c.execute("UPDATE rules SET status=2", []).unwrap();
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
    for networks in [
        [json!("bitcoin"), json!("bitcoin")],
        [Value::Null, Value::Null],
        [Value::Null, json!("bitcoin")],
        [json!("bitcoin"), Value::Null],
    ] {
        let server = MockServer::start().await;
        let entries =
            networks.map(|network| json!({"address":ADDRESS,"network":network,"status":"active"}));
        Mock::given(matchers::path("/v1/approvedAddresses/account/bitcoin"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"approvedAddresses":[{"address":"another-wallet","network":"bitcoin","status":"active"},entries[0],entries[1]]})),
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
        assert!(!options[0].2);
        assert!(options[1].2);
        let form = form_urlencoded::Serializer::new(String::new())
            .extend_pairs([
                (name.as_str(), options[1].0.as_str()),
                ("threshold_type", "min_amount"),
                ("min_amount", "0.1"),
            ])
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
        i.c.execute("UPDATE rules SET status=2", []).unwrap();
        let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
        let (_, options) = picker(&body);
        assert_eq!(options.len(), 2);
        assert!(!options[0].2);
        assert!(options[1].2);
        let settings: String =
            i.c.query_row("SELECT settings FROM rules", [], |r| r.get(0))
                .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&settings).unwrap()["network"],
            "bitcoin"
        );
        let id: i64 =
            i.c.query_row("SELECT id FROM rules", [], |r| r.get(0))
                .unwrap();
        for edit in [
            "rules_withdrawal[min_amount]=0.7",
            "rules_withdrawal[status]=scheduled",
        ] {
            assert_eq!(
                i.send(
                    "PATCH",
                    &format!("/rules/withdrawals/{id}"),
                    edit,
                    None,
                    true,
                    true
                )
                .await
                .0,
                if edit.contains("status") { 422 } else { 200 }
            );
            let address: String =
                i.c.query_row("SELECT address FROM rules", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(i.app.cipher.decrypt(&address).unwrap(), ADDRESS);
        }
        assert_eq!(
            i.c.query_row("SELECT status FROM rules", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}

#[tokio::test]
async fn mexc_picker_deduplicates_network_aliases_before_selecting_saving_and_starting() {
    use deltabadger::venue::withdrawal as w;
    for networks in [["BTC", "Bitcoin"], ["Bitcoin", "BTC"]] {
        for stored in ["BTC", "Bitcoin"] {
            let (i, server) = identity_listing("Mexc", &["memo"]).await;
            Mock::given(matchers::path(w::MEXC_ADDRESSES))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "data": networks.map(|network| json!({"coin":"BTC","address":ADDRESS,"network":network,"memo":"memo"})),
                    "totalPageNum":1
                })))
                .with_priority(1)
                .mount(&server).await;
            let id = i.rule(2);
            i.c.execute("UPDATE rules SET settings=json_set(settings,'$.network',?1,'$.address_tag','memo')", [stored]).unwrap();
            let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
            let (name, options) = picker(&body);
            assert_eq!(options.len(), 1, "{networks:?}, stored {stored}");
            assert!(options[0].2, "the equivalent destination must be selected");
            let form = form_urlencoded::Serializer::new(String::new())
                .extend_pairs([(name.as_str(), options[0].0.as_str())])
                .finish();
            let path = format!("/rules/withdrawals/{id}");
            let response = i.send("PATCH", &path, &form, None, true, true).await;
            assert_eq!(response.0, 200, "{}", response.1);
            let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
            let (_, options) = picker(&body);
            assert_eq!(options.len(), 1);
            assert!(options[0].2, "saving must retain the selection");
            assert_eq!(
                i.send(
                    "PATCH",
                    &path,
                    "rules_withdrawal[status]=scheduled",
                    None,
                    true,
                    true
                )
                .await
                .0,
                422 // D-RULE-UNSUPPORTED-START: live verification has not enabled sends.
            );
            let (status, settings): (i64, String) =
                i.c.query_row("SELECT status,settings FROM rules", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(status, 2);
            assert_eq!(
                serde_json::from_str::<Value>(&settings).unwrap()["network"],
                "Bitcoin"
            );
        }
    }
}

#[tokio::test]
async fn invalid_destination_toggle_guards_native_validation_before_stimulus_actions() {
    let (i, _server) = identity_listing("Kraken", &["first", "second"]).await;
    i.rule(2);
    let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
    let doc = scraper::Html::parse_document(&body);
    let form = doc
        .select(&scraper::Selector::parse("form.rule--inactive").unwrap())
        .next()
        .unwrap();
    assert!(form
        .select(
            &scraper::Selector::parse("select[required] option[value=''][selected][disabled]")
                .unwrap()
        )
        .next()
        .is_some());
    let toggle = form
        .select(&scraper::Selector::parse("input[data-rule-toggle-target=checkbox]").unwrap())
        .next()
        .unwrap();
    assert!(toggle.value().attr("checked").is_none());
    assert!(toggle.value().attr("disabled").is_none());
    // The target's inline handler runs before Stimulus's bubbling change actions:
    // restore the checkbox and stop submission, styling and disabling when invalid.
    assert_eq!(toggle.value().attr("onchange"), Some("if (!this.form.checkValidity()) { this.form.reportValidity(); this.checked = !this.checked; event.stopImmediatePropagation(); }"));
    assert_eq!(
        toggle.value().attr("data-action"),
        Some("change->form--submit#submit change->class-toggle#toggle change->rule-toggle#disable")
    );
}
#[tokio::test]
async fn unresolved_picker_has_no_successful_value_and_refuses_edits_and_start() {
    for venue in ["Binance", "Mexc", "Kraken"] {
        for identities in [&["first", "second"][..], &["first"][..]] {
            let (i, _server) = identity_listing(venue, identities).await;
            let id = i.rule(2);
            if identities.len() == 1 {
                i.c.execute(
                    "UPDATE rules SET address=?1",
                    [i.app.cipher.encrypt("removed-wallet")],
                )
                .unwrap();
            }
            let before = i.rows();
            let (_, body) = i.send("GET", "/rules", "", None, true, false).await;
            let doc = scraper::Html::parse_document(&body);
            let select = doc
                .select(&scraper::Selector::parse(".tag--address select").unwrap())
                .next()
                .unwrap();
            assert!(select.value().attr("required").is_some());
            let selected: Vec<_> = select
                .select(&scraper::Selector::parse("option[selected]").unwrap())
                .collect();
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].value().attr("value"), Some(""));
            assert!(selected[0].value().attr("disabled").is_some());
            // Disabled options are omitted from browser form data. Also refuse an explicit empty value.
            for destination in ["", "&rules_withdrawal[destination]="] {
                for edit in ["min_amount]=0.7", "status]=scheduled"] {
                    let form = format!("rules_withdrawal[{edit}{destination}");
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
        }
    }
}

#[tokio::test]
async fn preview_does_not_resolve_ambiguous_destinations_using_the_default_fee_network() {
    let server = MockServer::start().await;
    Mock::given(matchers::path(
        deltabadger::venue::withdrawal::BINANCE_ADDRESSES,
    ))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!([
        {"coin":"BTC","address":ADDRESS,"network":"BTC"},
        {"coin":"BTC","address":ADDRESS,"network":"ETH"}
    ])))
    .mount(&server)
    .await;
    Mock::given(matchers::path(
        deltabadger::venue::withdrawal::BINANCE_COINS,
    ))
    .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
        "coin":"BTC",
        "networkList":[
            {"network":"BTC","withdrawEnable":true,"withdrawTag":false},
            {"network":"ETH","withdrawEnable":true,"withdrawTag":false}
        ]
    }])))
    .mount(&server)
    .await;
    let mut i = Install::new("Binance");
    i.app = i
        .app
        .with_venue_key_url("Exchanges::Binance", &server.uri())
        .unwrap();
    i.c.execute("UPDATE exchange_assets SET withdrawal_fee='0.0001',withdrawal_fee_updated_at=?1,withdrawal_chains=?2", (AT, json!([
        {"name":"BTC","fee":"0.0001","is_default":true},
        {"name":"ETH","fee":"0.003","is_default":false}
    ]).to_string())).unwrap();
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
    let (_, options) = picker(&body);
    assert_eq!(options.len(), 2);
    assert!(options.iter().all(|o| !o.2));
    let doc = scraper::Html::parse_document(&body);
    assert_eq!(
        doc.select(
            &scraper::Selector::parse(
                "select[name=destination][required] option[value=''][selected][disabled]"
            )
            .unwrap()
        )
        .count(),
        1
    );
    // Submitting the disabled placeholder contributes no destination; the server must
    // reject the unresolved selection even when browser validation is bypassed.
    let (status, _) = i
        .send(
            "POST",
            "/rules/withdrawals/confirm_settings",
            "min_amount=0.7",
            Some(i.config()),
            true,
            true,
        )
        .await;
    assert_eq!(status, 302);
    assert_eq!(i.rows(), "[]");
}

#[tokio::test]
async fn mexc_preview_resolves_destination_network_before_calculating_fee() {
    let (i, _server) = identity_listing("Mexc", &["memo"]).await;
    i.c.execute("UPDATE exchange_assets SET withdrawal_fee='0.0001',withdrawal_fee_updated_at=?1,withdrawal_chains=?2", (AT, json!([
        {"name":"Other","fee":"0.0001","is_default":true},
        {"name":"Bitcoin","fee":"0.003","is_default":false}
    ]).to_string())).unwrap();
    let mut config = i.config();
    config["network"] = json!("Bitcoin");
    let (_, body) = i
        .send(
            "GET",
            "/rules/withdrawals/confirm_settings/new",
            "",
            Some(config.clone()),
            true,
            false,
        )
        .await;
    let (name, options) = picker(&body);
    let form = form_urlencoded::Serializer::new(String::new())
        .extend_pairs([
            (name.as_str(), options[0].0.as_str()),
            ("network", "Other"),
            ("threshold_type", "fee_percentage"),
            ("max_fee_percentage", "0.5"),
        ])
        .finish();
    let (status, preview) = i
        .send(
            "POST",
            "/rules/withdrawals/confirm_settings/preview",
            &form,
            Some(config.clone()),
            true,
            true,
        )
        .await;
    assert_eq!(status, 200);
    let doc = scraper::Html::parse_document(&preview);
    let info = doc
        .select(&scraper::Selector::parse(".small-info").unwrap())
        .find(|e| e.value().attr("style") == Some(""))
        .unwrap()
        .text()
        .collect::<String>();
    assert!(info.contains("0.003"), "{info}");
    assert!(info.contains("0.6"), "{info}");
    for html in [&body, &preview] {
        let doc = scraper::Html::parse_document(html);
        assert_eq!(
            doc.select(&scraper::Selector::parse("[name=network]").unwrap())
                .count(),
            0
        );
    }
    assert!(picker(&preview).1[0].2);
    assert_eq!(
        i.send(
            "POST",
            "/rules/withdrawals/confirm_settings",
            &form,
            Some(config),
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
        "Bitcoin"
    );
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
    i.c.execute(
        "INSERT INTO rust_withdrawal_intents(owner,state,client_id,created_at,api_key_id,credential_digest,host,account_scope,key_identity,params_enc,fee_basis,budget,free_balance) VALUES (?1,'dispatched','test','2026-01-01',1,'digest','host','spot','key','enc','flat','1','1')",
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
            assert_eq!(response.0, if status == 3 { 404 } else { 422 });
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

const X0_ADDRESS: &str = "bc1qx0destination0000000000000000000000";
fn status_number(s: &str) -> i64 {
    [
        "created",
        "scheduled",
        "stopped",
        "deleted",
        "executing",
        "retrying",
        "waiting",
        "archived",
    ]
    .iter()
    .position(|v| *v == s)
    .unwrap() as i64
}
fn x0_rows(i: &Install) -> Value {
    let mut st = i
        .c
        .prepare("SELECT id,status,exchange_id,asset_id,address,settings FROM rules ORDER BY id")
        .unwrap();
    json!(st.query_map([],|r| {
        let text:String=r.get(5)?;
        let mut settings:Value=serde_json::from_str(&text).unwrap();
        // JSON null and an absent store_accessor both read as nil in Rails.
        settings.as_object_mut().unwrap().retain(|_,v|!v.is_null());
        Ok(json!({"id":r.get::<_,i64>(0)?,"type":"Rules::Withdrawal",
            "status":(["created","scheduled","stopped","deleted","executing","retrying","waiting","archived"][r.get::<_,usize>(1)?]),
            "exchange_id":r.get::<_,i64>(2)?,"asset_id":r.get::<_,i64>(3)?,
            "address":i.app.cipher.decrypt(&r.get::<_,String>(4)?).unwrap(),"settings":settings}))
    }).unwrap().collect::<Result<Vec<_>,_>>().unwrap())
}
fn x0_seed(i: &Install, case: &str, vector: &Value) -> i64 {
    if case == "list_each_status" {
        for (n, venue) in ["Binance", "Kraken", "Gemini", "Mexc"].iter().enumerate() {
            let exchange = (n + 1) as i64;
            if n > 0 {
                i.c.execute("INSERT INTO exchanges(id,type,name,created_at,updated_at) VALUES (?1,?2,?3,?4,?4)",params![exchange,format!("Exchanges::{venue}"),venue,AT]).unwrap();
            }
            i.c.execute("INSERT INTO rules(id,user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES (?1,?2,'Rules::Withdrawal',?3,?1,?4,?5,?6,?7,?7)",
                params![exchange,i.s.user_id,n as i64,i.s.btc,i.app.cipher.encrypt(X0_ADDRESS),vector["rules"][n]["settings"].to_string(),AT]).unwrap();
        }
        return 1;
    }
    let state = if case == "create_exists" {
        Some(2)
    } else if case == "create_revives_deleted" {
        Some(3)
    } else if case.starts_with("create_") || case.starts_with("list_") || case == "start_missing" {
        None
    } else {
        Some(
            case.split('_')
                .nth(1)
                .filter(|s| ["created", "scheduled", "stopped", "deleted"].contains(s))
                .map(status_number)
                .unwrap_or(2),
        )
    };
    if let Some(state) = state {
        let id = i.rule(state);
        let mut settings = json!({"withdrawal_percentage":"100","threshold_type":"fee_percentage","max_fee_percentage":"1"});
        if case == "create_revives_deleted" {
            settings["max_interval"] = json!("7");
            settings["address_tag"] = json!("old");
        }
        i.c.execute(
            "UPDATE rules SET address=?1,settings=?2",
            (i.app.cipher.encrypt(X0_ADDRESS), settings.to_string()),
        )
        .unwrap();
        id
    } else {
        999
    }
}
async fn x0_install(case: &str, vector: &Value, server: &MockServer) -> (Install, i64) {
    let mut i = Install::new(if case == "create_unsupported_venue" {
        "Kucoin"
    } else {
        "Binance"
    });
    i.app = i
        .app
        .with_venue_key_url("Exchanges::Binance", &server.uri())
        .unwrap();
    let id = x0_seed(&i, case, vector);
    (i, id)
}
#[tokio::test]
async fn all_seventy_x0_api_cells_share_web_database_effects() {
    use deltabadger::venue::withdrawal as w;
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    let cells = vectors["api"].as_array().unwrap();
    assert_eq!(cells.len(), 70);
    for cell in cells {
        let case = cell["case"].as_str().unwrap();
        let oracle = cells
            .iter()
            .find(|v| v["case"] == case && v["surface"] == "rest")
            .unwrap();
        let server = MockServer::start().await;
        if case != "create_listing_failed" {
            Mock::given(matchers::path(w::BINANCE_ADDRESSES)).respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {"coin":"BTC","address":X0_ADDRESS,"name":"cold","network":"BTC","addressTag":""}
            ]))).mount(&server).await;
        }
        // V1 requires the selected network to be enabled; X0's Rails API never read it.
        Mock::given(matchers::path(w::BINANCE_COINS)).respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"coin":"BTC","networkList":[{"network":"BTC","withdrawEnable":true,"withdrawTag":false,"withdrawFee":"0.0001"}]}
        ]))).mount(&server).await;
        let (i, id) = x0_install(case, oracle, &server).await;
        let before = x0_rows(&i);
        let args = &cell["arguments"];
        let patch = Patch {
            settings: args.clone(),
            address: args["address"].as_str().map(str::to_owned),
            ..Default::default()
        };
        let exchange_id = if case == "create_unknown_exchange" {
            999
        } else {
            i.s.exchange_id
        };
        let asset_id = if case == "create_missing" {
            0
        } else if case == "create_unlisted_asset" {
            999
        } else {
            i.s.btc
        };
        let tool = cell["tool"].as_str().unwrap();
        let result = match tool {
            "create_rule" => Some(
                service::create_named(
                    &i.app,
                    i.s.user_id,
                    args["exchange_name"].as_str().unwrap_or_default(),
                    args["asset"].as_str().unwrap_or_default(),
                    patch,
                )
                .await
                .unwrap(),
            ),
            "start_rule" => Some(
                service::call(&i.app, i.s.user_id, Command::Start(id, patch))
                    .await
                    .unwrap(),
            ),
            "stop_rule" => Some(
                service::call(&i.app, i.s.user_id, Command::Stop(id))
                    .await
                    .unwrap(),
            ),
            "delete_rule" => Some(
                service::call(&i.app, i.s.user_id, Command::Delete(id))
                    .await
                    .unwrap(),
            ),
            "update_rule_settings" => Some(
                service::call(&i.app, i.s.user_id, Command::Update(id, patch))
                    .await
                    .unwrap(),
            ),
            "list_rules" => {
                let rows = service::list(&i.c, &i.app, i.s.user_id).unwrap();
                let expected = oracle["body"]["data"]["rules"].as_array().unwrap();
                assert_eq!(rows.len(), expected.len());
                for (r, e) in rows.iter().zip(expected) {
                    assert_eq!(r.id, e["id"].as_i64().unwrap());
                    assert_eq!(r.status, e["status"]);
                    assert_eq!(r.address, e["address"]);
                    assert_eq!(r.withdrawal_percentage, e["withdrawal_percentage"]);
                    assert_eq!(r.threshold_type, e["threshold_type"]);
                    assert!(r.unresolved_withdrawal.is_none());
                }
                None
            }
            _ => panic!("unmapped X0 cell"),
        };
        let expected_code = match case {
            "start_created" | "start_stopped" => Some("withdrawal_disabled"), // D-RULE-UNSUPPORTED-START
            "create_listed" | "create_invalid_network" => Some("address_not_listed"), // D-RULE-LISTED-ENTRY
            _ => oracle["body"]["error"]["code"].as_str(),
        };
        if let Some(result) = &result {
            assert_eq!(
                result.as_ref().err().map(|e| e.code),
                expected_code,
                "{} {case}",
                cell["surface"]
            );
            if result.is_err() {
                assert_eq!(x0_rows(&i), before, "{case}: refusal wrote");
            }
        }
        let mut expected = oracle["rules"].clone();
        if matches!(case, "start_created" | "start_stopped" | "create_listed") {
            expected = before.clone();
        }
        for r in expected.as_array_mut().unwrap() {
            r["settings"]
                .as_object_mut()
                .unwrap()
                .retain(|_, v| !v.is_null());
            if matches!(
                case,
                "create_min_amount"
                    | "create_revives_deleted"
                    | "update_created"
                    | "update_stopped"
                    | "update_all_four"
            ) {
                r["settings"]["network"] = json!("BTC"); // D-RULE-LISTED-ENTRY: exact listed network
            }
            if case == "create_revives_deleted" {
                assert_eq!(
                    r["settings"]["threshold_type"], "min_amount",
                    "X0 records fee-dependent API default"
                );
                r["settings"]["threshold_type"] = json!("fee_percentage"); // D-RULE-ONE-MATRIX: one explicit default
            }
        }
        assert_eq!(
            x0_rows(&i),
            expected,
            "{} {case}: Rails row and named deltas",
            cell["surface"]
        );

        let (web, id) = x0_install(case, oracle, &server).await;
        let (method, path, form, config) = match tool {
            "list_rules" => ("GET", "/rules".into(), String::new(), None),
            "create_rule" => {
                let conf = json!({"exchange_id":exchange_id.to_string(),"asset_id":asset_id.to_string(),"address":args["address"]});
                let fields = args
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), web::rules::model::string(v)))
                    .collect::<Vec<_>>();
                (
                    "POST",
                    "/rules/withdrawals/confirm_settings".into(),
                    form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields)
                        .finish(),
                    Some(conf),
                )
            }
            _ => {
                let fields = if tool == "start_rule" {
                    vec![("rules_withdrawal[status]".into(), "scheduled".into())]
                } else if tool == "stop_rule" {
                    vec![("rules_withdrawal[status]".into(), "stopped".into())]
                } else {
                    args.as_object()
                        .unwrap()
                        .iter()
                        .map(|(k, v)| {
                            (
                                format!("rules_withdrawal[{k}]"),
                                web::rules::model::string(v),
                            )
                        })
                        .collect::<Vec<_>>()
                };
                (
                    if tool == "delete_rule" {
                        "DELETE"
                    } else {
                        "PATCH"
                    },
                    format!("/rules/withdrawals/{id}"),
                    form_urlencoded::Serializer::new(String::new())
                        .extend_pairs(fields)
                        .finish(),
                    None,
                )
            }
        };
        let (status, body) = web.send(method, &path, &form, config, true, true).await;
        if result.as_ref().is_some_and(Result::is_ok) {
            assert_eq!(status, 200, "{case}: {body}");
        } else if result.as_ref().is_some_and(Result::is_err) {
            assert!(status >= 300, "{case}: {body}");
        }
        assert_eq!(
            x0_rows(&web),
            x0_rows(&i),
            "{case}: web and {} service",
            cell["surface"]
        );
        assert_eq!(
            i.c.query_row("SELECT count(*) FROM rule_logs", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(
            server.received_requests().await.unwrap().iter().all(|r| [
                w::BINANCE_ADDRESSES,
                w::BINANCE_COINS
            ]
            .contains(&r.url.path())),
            "W2 never sends"
        );
    }
}
