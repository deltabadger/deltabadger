//! Real jobs publish the Rails completion once, after commit, without changing their outcomes.
mod common;
use deltabadger::{
    engine::FixedClock,
    jobs::{self, notifications::Notifications, Cx, Db, Job, Outcome},
    sync::jobs::{Connect, LedgerSync},
    tracker,
    venue::{
        alpaca::{AlpacaVenue, Urls},
        http::ScriptedTransport,
    },
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Venues(ScriptedTransport);
impl Connect for Venues {
    type T = ScriptedTransport;
    fn connect(&self, _: &deltabadger::crypto::Credentials) -> AlpacaVenue<Self::T> {
        AlpacaVenue::new(self.0.clone(), Urls::for_passphrase(Some("paper")))
    }
}
fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(include_str!("fixtures/tracker_completion.json")).unwrap()
        ["records"][name]
        .clone()
}
fn clock() -> FixedClock {
    FixedClock("2026-09-20T02:00:00Z".parse().unwrap())
}
fn cx<'a>(db: &Db, clock: &'a FixedClock) -> Cx<'a> {
    Cx {
        db: db.clone(),
        clock,
        wakers: jobs::Wakers::default(),
    }
}
type Messages = Arc<Mutex<Vec<Value>>>;
fn probe(
    path: std::path::PathBuf,
    messages: Messages,
    fail: bool,
    sql: &'static str,
) -> Notifications {
    Notifications::new(move |stream, payload| {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.busy_timeout(std::time::Duration::ZERO).unwrap();
        c.execute_batch("BEGIN IMMEDIATE")
            .expect("broadcast must run after commit");
        assert!(
            c.query_row(sql, [], |r| r.get::<_, bool>(0)).unwrap(),
            "committed result visible at broadcast"
        );
        c.execute_batch("ROLLBACK").unwrap();
        messages.lock().unwrap().push(json!([stream, payload]));
        if fail {
            Err(())
        } else {
            Ok(())
        }
    })
}

#[tokio::test(flavor = "current_thread")]
async fn completion_sync_success_and_failure_publish_once_after_commit() {
    for owner in [1, 42] {
        for (failure, delivery_failure) in
            [(false, false), (true, false), (false, true), (true, true)]
        {
            let (dir, opened, seed) = common::install_alpaca();
            if owner != seed.user_id {
                opened.primary.execute("INSERT INTO users(id,email,encrypted_password,created_at,updated_at) VALUES(42,'other@example.test','x','2026-01-01','2026-01-01')", []).unwrap();
                opened
                    .primary
                    .execute("UPDATE api_keys SET user_id=42", [])
                    .unwrap();
            }
            let heard = Messages::default();
            let sql = if failure {
                "SELECT last_sync_error IS NOT NULL FROM api_keys"
            } else {
                "SELECT count(*)=1 FROM account_transactions"
            };
            let notifications = probe(
                dir.path().join("production.sqlite3"),
                heard.clone(),
                delivery_failure,
                sql,
            );
            let db =
                Db::new(opened.primary, common::seed::cipher()).with_notifications(notifications);
            let answer = if failure {
                json!({"status":401,"body":{"message":"unauthorized."}})
            } else {
                json!({"status":200,"body":[{"id":"interest","activity_type":"INT","net_amount":"0.07","date":"2026-09-01"}]})
            };
            let venues = Venues(ScriptedTransport::from_script(
                &json!({"GET /v2/account/activities":[answer]}),
            ));
            let outcome = LedgerSync::new(venues, seed.api_key_id)
                .run(cx(&db, &clock()), vec![])
                .await;
            assert_eq!(
                outcome,
                if failure {
                    Outcome::Failed("unauthorized.".into())
                } else {
                    Outcome::Done
                }
            );
            assert_eq!(
                json!(*heard.lock().unwrap()),
                fixture(&format!(
                    "sync_{owner}_{}",
                    if failure { "failure" } else { "success" }
                ))
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn completion_early_sync_failure_still_removes_progress() {
    let (dir, opened, seed) = common::install_alpaca();
    let other = deltabadger::crypto::Cipher::new(
        &deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "another-instance").unwrap(),
    );
    opened
        .primary
        .execute(
            "UPDATE api_keys SET secret=?1",
            [other.encrypt("test-secret")],
        )
        .unwrap();
    let heard = Messages::default();
    let notifications = probe(
        dir.path().join("production.sqlite3"),
        heard.clone(),
        false,
        "SELECT count(*)=0 FROM account_transactions",
    );
    let db = Db::new(opened.primary, common::seed::cipher()).with_notifications(notifications);
    let job = LedgerSync::new(
        Venues(ScriptedTransport::from_script(&json!({}))),
        seed.api_key_id,
    );
    assert!(matches!(
        job.run(cx(&db, &clock()), vec![]).await,
        Outcome::Failed(_)
    ));
    assert_eq!(json!(*heard.lock().unwrap()), fixture("sync_1_failure"));
}

#[tokio::test(flavor = "current_thread")]
async fn completion_ledger_refreshes_once_after_commit_and_not_on_failure() {
    for delivery_failure in [false, true] {
        let (dir, opened, seed) = common::install_alpaca();
        opened.primary.execute("INSERT INTO account_transactions(user_id,exchange_id,entry_type,base_currency,base_amount,transacted_at,raw_data,manual_values,created_at,updated_at) VALUES(?1,?2,4,'USD',100,'2026-09-01 00:00:00','{}','{}','2026-09-01','2026-09-01')", [seed.user_id, seed.exchange_id]).unwrap();
        let heard = Messages::default();
        let notifications = probe(
            dir.path().join("production.sqlite3"),
            heard.clone(),
            delivery_failure,
            "SELECT count(*)=1 FROM portfolio_snapshots",
        );
        let db = Db::new(opened.primary, common::seed::cipher()).with_notifications(notifications);
        let api: std::rc::Rc<Option<jobs::data_api::DataApi<ScriptedTransport>>> =
            std::rc::Rc::new(None);
        let now = clock().0;
        let job = tracker::jobs::resolve(
            tracker::jobs::TRACKER_LEDGER,
            seed.user_id,
            &Venues(ScriptedTransport::from_script(&json!({}))),
            api,
            Arc::new(move || now),
        )
        .unwrap();
        assert_eq!(job.run(cx(&db, &clock()), vec![]).await, Outcome::Done);
        assert_eq!(json!(*heard.lock().unwrap()), fixture("ledger_1"));
        heard.lock().unwrap().clear();
        db.run(|c, _| c.execute_batch("CREATE TRIGGER refuse_snapshot BEFORE UPDATE ON portfolio_snapshots BEGIN SELECT RAISE(ABORT,'refuse'); END;").map_err(|e| e.to_string())).await.unwrap();
        assert!(matches!(
            job.run(cx(&db, &clock()), vec![]).await,
            Outcome::Failed(_)
        ));
        assert!(heard.lock().unwrap().is_empty());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn completion_empty_success_finishes_but_capped_import_keeps_progress() {
    for capped in [false, true] {
        let (dir, opened, seed) = common::install_alpaca();
        let heard = Messages::default();
        let notifications = probe(
            dir.path().join("production.sqlite3"),
            heard.clone(),
            false,
            "SELECT count(*)=0 FROM account_transactions",
        );
        let db = Db::new(opened.primary, common::seed::cipher()).with_notifications(notifications);
        let body = if capped {
            Value::Array((0..100).map(|i| json!({"id":format!("i{i:03}"),"activity_type":"INT","net_amount":"0.07","date":"2026-09-01"})).collect())
        } else {
            json!([])
        };
        let job = LedgerSync::new(
            Venues(ScriptedTransport::from_script(
                &json!({"GET /v2/account/activities":[{"status":200,"body":body}]}),
            )),
            seed.api_key_id,
        )
        .within(deltabadger::sync::ledger::Limits {
            pages: 1,
            runs: 100,
        });
        assert_eq!(
            job.run(cx(&db, &clock()), vec![]).await,
            if capped {
                Outcome::NothingNew
            } else {
                Outcome::Done
            }
        );
        assert_eq!(
            json!(*heard.lock().unwrap()),
            if capped {
                json!([])
            } else {
                fixture("sync_1_success")
            }
        );
    }
}
