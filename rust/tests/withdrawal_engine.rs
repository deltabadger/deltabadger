//! E0 G0: durable transaction boundaries, Rails evidence, and money fences. No sockets.
mod common;
use deltabadger::{
    crypto::{Cipher, EncryptionKeys},
    engine::{rules, withdrawal as w, FixedClock},
    ruby::BigDec,
    venue::withdrawal as v,
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
fn n(s: &str) -> BigDec {
    BigDec::parse(s).unwrap()
}
fn now() -> chrono::DateTime<chrono::Utc> {
    "2026-10-28T12:00:00Z".parse().unwrap()
}
struct Db {
    c: Connection,
    q: Connection,
    cipher: Cipher,
}
impl Db {
    fn new() -> Self {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE rules(id INTEGER PRIMARY KEY,user_id INTEGER,type TEXT,status INTEGER,exchange_id INTEGER,asset_id INTEGER,address TEXT,settings TEXT,updated_at TEXT); CREATE TABLE exchanges(id INTEGER PRIMARY KEY,type TEXT,name TEXT); CREATE TABLE assets(id INTEGER PRIMARY KEY,symbol TEXT); CREATE TABLE api_keys(id INTEGER PRIMARY KEY,user_id INTEGER,exchange_id INTEGER,key_type INTEGER,status INTEGER,key TEXT,secret TEXT,passphrase TEXT,access_token TEXT,rsa_signature_key TEXT,rsa_encryption_key TEXT,dh_param TEXT); CREATE TABLE tickers(id INTEGER PRIMARY KEY,exchange_id INTEGER,base_asset_id INTEGER,quote_asset_id INTEGER,base TEXT,quote TEXT,available INTEGER); CREATE TABLE rule_logs(id INTEGER PRIMARY KEY,rule_id INTEGER,status INTEGER,message TEXT,details TEXT,created_at TEXT); CREATE TABLE app_configs(id INTEGER PRIMARY KEY,key TEXT UNIQUE,value TEXT,created_at TEXT,updated_at TEXT); CREATE TABLE exchange_assets(exchange_id INTEGER,asset_id INTEGER,withdrawal_fee TEXT,withdrawal_fee_updated_at TEXT,withdrawal_chains TEXT,updated_at TEXT); INSERT INTO exchanges VALUES(1,'Exchanges::Binance','Binance'); INSERT INTO assets VALUES(1,'BTC'); INSERT INTO tickers VALUES(1,1,1,2,'BTC','USD',1); INSERT INTO exchange_assets(exchange_id,asset_id) VALUES(1,1);").unwrap();
        c.execute_batch(include_str!(
            "../migrations/20261026000001_withdrawal_rules.sql"
        ))
        .unwrap();
        c.execute_batch(include_str!(
            "../migrations/20261028000001_withdrawal_engine.sql"
        ))
        .unwrap();
        let cipher = Cipher::new(&EncryptionKeys::resolve(&|_| None, "withdrawal-test").unwrap());
        c.execute("INSERT INTO api_keys(id,user_id,exchange_id,key_type,status,key,secret) VALUES(1,1,1,1,1,?1,?2)",params![cipher.encrypt("key"),cipher.encrypt("secret")]).unwrap();
        let settings = json!({"network":"BTC","withdrawal_percentage":"100","threshold_type":"min_amount","min_amount":"0.01"});
        c.execute(
            "INSERT INTO rules VALUES(1,1,'Rules::Withdrawal',1,1,1,?1,?2,'revision')",
            params![cipher.encrypt("destination"), settings.to_string()],
        )
        .unwrap();
        let q = Connection::open_in_memory().unwrap();
        q.execute_batch("CREATE TABLE solid_queue_jobs(id INTEGER PRIMARY KEY,active_job_id TEXT,class_name TEXT,arguments TEXT,finished_at TEXT); CREATE TABLE solid_queue_claimed_executions(job_id INTEGER); CREATE TABLE solid_queue_failed_executions(job_id INTEGER); CREATE TABLE solid_queue_ready_executions(job_id INTEGER); CREATE TABLE solid_queue_scheduled_executions(job_id INTEGER); CREATE TABLE solid_queue_blocked_executions(job_id INTEGER);").unwrap();
        Self { c, q, cipher }
    }
    fn eval(&self) -> i64 {
        self.c
            .execute(
                "INSERT INTO rust_rule_evaluations(rule_id,source,created_at) VALUES(1,?1,?2)",
                params![
                    format!("start:{}", uuid::Uuid::new_v4()),
                    now().to_rfc3339()
                ],
            )
            .unwrap();
        self.c.last_insert_rowid()
    }
    fn count(&self, t: &str) -> i64 {
        self.c
            .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
            .unwrap()
    }
    fn log(&self, msg: &str, id: Option<&str>) -> i64 {
        self.c.execute("INSERT INTO rule_logs(rule_id,status,message,details,created_at) VALUES(1,?1,?2,?3,?4)",params![if id.is_some(){1}else{2},msg,json!({"amount":"0.9","withdrawal_id":id}).to_string(),now().to_rfc3339()]).unwrap();
        self.c.last_insert_rowid()
    }
    fn job(&self, id: &str, all: bool, claimed: bool) {
        self.q
            .execute(
                "INSERT INTO solid_queue_jobs(active_job_id,class_name,arguments) VALUES(?1,?2,?3)",
                params![
                    id,
                    if all {
                        "Rule::EvaluateAllJob"
                    } else {
                        "Rule::ExecuteJob"
                    },
                    json!({"arguments":[{"_aj_globalid":"gid://deltabadger/Rules::Withdrawal/1"}]})
                        .to_string()
                ],
            )
            .unwrap();
        let row = self.q.last_insert_rowid();
        self.q
            .execute(
                &format!(
                    "INSERT INTO solid_queue_{}_executions VALUES(?1)",
                    if claimed { "claimed" } else { "ready" }
                ),
                [row],
            )
            .unwrap();
    }
    async fn prepared(&self, f: &Fake<'_>) -> w::Intent {
        w::evaluate(&self.c, &self.cipher, f, self.eval(), &FixedClock(now()))
            .await
            .unwrap()
            .unwrap()
    }
}
struct Fake<'a> {
    counter: Option<std::path::PathBuf>,
    replacement_secret: String,
    network: &'static str,
    fee: &'static str,
    tag: Option<&'static str>,
    net_work: Option<&'static str>,
    change_at: Cell<usize>,
    change: String,
    version: deltabadger::engine::model::CredentialVersion,
    c: &'a Connection,
    sends: Cell<usize>,
    read: Cell<usize>,
    rotate_at: Cell<usize>,
    answer: RefCell<w::SendResult>,
    lookup: RefCell<w::LookupResult>,
}
impl<'a> Fake<'a> {
    fn new(d: &'a Db) -> Self {
        Self {
            counter: None,
            replacement_secret: d.cipher.encrypt("rotated-secret"),
            network: "BTC",
            fee: "0.0001",
            tag: None,
            net_work: None,
            change_at: Cell::new(0),
            change: String::new(),
            version: deltabadger::engine::model::credential_version_by_id(&d.c, 1)
                .unwrap()
                .unwrap(),
            c: &d.c,
            sends: Cell::new(0),
            read: Cell::new(0),
            rotate_at: Cell::new(0),
            answer: RefCell::new(w::SendResult::Accepted {
                id: "sent-1".into(),
            }),
            lookup: RefCell::new(w::LookupResult::NotFound),
        }
    }
    fn step(&self) {
        let i = self.read.get() + 1;
        self.read.set(i);
        if self.change_at.get() == i {
            self.c.execute(&self.change, []).unwrap();
        }
        if self.rotate_at.get() == i {
            self.c
                .execute("UPDATE api_keys SET secret=?1", [&self.replacement_secret])
                .unwrap();
        }
    }
}
impl w::Adapter for Fake<'_> {
    fn version(&self) -> deltabadger::engine::model::CredentialVersion {
        self.version.clone()
    }
    fn identity(&self) -> w::Identity {
        w::Identity {
            host: "https://api.binance.com".into(),
            account_scope: "spot".into(),
            key_identity: hex::encode(sha2::Sha256::digest(b"key")),
        }
    }
    async fn balance(&self, _: &str) -> Result<BigDec, deltabadger::venue::VenueError> {
        self.step();
        Ok(n("1"))
    }
    async fn networks(&self, _: &str) -> Result<Option<v::Coin>, deltabadger::venue::VenueError> {
        self.step();
        Ok(Some(v::coin_networks(&json!([{"coin":"BTC","networkList":[{"network":self.network,"withdrawEnable":true,"netWork":self.net_work,"withdrawTag":self.tag.is_some(),"withdrawFee":self.fee,"isDefault":true}]}]),"BTC","Binance").unwrap().unwrap()))
    }
    async fn addresses(&self, _: &str) -> Result<Vec<v::Entry>, deltabadger::venue::VenueError> {
        self.step();
        Ok(vec![v::Entry {
            address: "destination".into(),
            network: Some(self.network.into()),
            tag: self.tag.map(str::to_owned),
            key: None,
            label: None,
            refused: None,
        }])
    }
    async fn fee(
        &self,
        _: &v::Destination,
        _: &BigDec,
    ) -> Result<Option<BigDec>, deltabadger::venue::VenueError> {
        self.step();
        Ok(Some(n(self.fee)))
    }
    async fn send(&self, _: &w::Request) -> w::SendResult {
        self.sends.set(self.sends.get() + 1);
        if let Some(path) = &self.counter {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"x")
                .unwrap();
        }

        self.step();
        self.answer.borrow().clone()
    }
    async fn lookup(&self, _: &w::Request) -> w::LookupResult {
        self.step();
        self.lookup.borrow().clone()
    }
}
use sha2::Digest;
#[tokio::test]
async fn crash_after_begin_dispatch_settle_never_resends() {
    for boundary in ["begin", "dispatch", "settle"] {
        let d = Db::new();
        let f = Fake::new(&d);
        let i = d.prepared(&f).await;
        if boundary != "begin" {
            assert!(
                w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                    .await
                    .unwrap()
            );
        }
        if boundary == "settle" {
            w::settle(
                &d.c,
                &d.cipher,
                &i,
                "dispatched",
                w::SendResult::Accepted { id: "X".into() },
                now(),
            )
            .unwrap();
        }
        w::recover(&d.c, &d.cipher, &f, &i.owner, &FixedClock(now()))
            .await
            .unwrap();
        assert_eq!(f.sends.get(), 0, "restart {boundary}");
        if boundary == "dispatch" {
            assert_eq!(
                w::load(&d.c, &d.cipher, &i.owner).unwrap().unwrap().state,
                "unresolved"
            );
            assert!(rules::status(&d.c, 1).unwrap().is_some());
        } else {
            assert_eq!(d.count("rust_withdrawal_intents"), 0);
        }
    }
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    w::recover(&d.c, &d.cipher, &f, &i.owner, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 1);
    assert_eq!(d.count("rule_logs"), 1);
}
#[tokio::test]
async fn conditional_settlement_accept_reject_stale_and_duplicate() {
    for result in [
        w::SendResult::Accepted { id: "X".into() },
        w::SendResult::Rejected {
            definitive: true,
            reason: "You have insufficient balance.".into(),
        },
    ] {
        let d = Db::new();
        let f = Fake::new(&d);
        let i = d.prepared(&f).await;
        w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
            .await
            .unwrap();
        assert!(w::settle(&d.c, &d.cipher, &i, "dispatched", result.clone(), now()).unwrap());
        assert!(!w::settle(&d.c, &d.cipher, &i, "dispatched", result, now()).unwrap());
        assert_eq!(d.count("rule_logs"), 1);
    }
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    w::settle(
        &d.c,
        &d.cipher,
        &i,
        "dispatched",
        w::SendResult::Ambiguous {
            reason: "lost reply".into(),
        },
        now(),
    )
    .unwrap();
    assert!(!w::settle(
        &d.c,
        &d.cipher,
        &i,
        "dispatched",
        w::SendResult::Accepted { id: "stale".into() },
        now()
    )
    .unwrap());
    assert!(
        !w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
            .await
            .unwrap()
    );
    assert_eq!(d.count("rule_logs"), 0);
    d.log("Withdrew", Some("X"));
    w::settle(
        &d.c,
        &d.cipher,
        &i,
        "unresolved",
        w::SendResult::Accepted { id: "X".into() },
        now(),
    )
    .unwrap();
    assert_eq!(d.count("rule_logs"), 1);
    assert_eq!(
        w::load(&d.c, &d.cipher, &i.owner)
            .unwrap()
            .unwrap()
            .reason
            .as_deref(),
        Some("duplicate venue id")
    );
}
#[tokio::test]
async fn credential_fences_each_read_begin_dispatch_send_and_lookup() {
    for at in 1..=5 {
        let d = Db::new();
        let f = Fake::new(&d);
        f.rotate_at.set(at);
        assert!(
            w::evaluate(&d.c, &d.cipher, &f, d.eval(), &FixedClock(now()))
                .await
                .is_err()
        );
        assert_eq!(d.count("rust_withdrawal_intents"), 0);
        assert_eq!(
            d.count("rust_rule_evaluations WHERE consumed_at IS NULL"),
            1
        );
        assert_eq!(f.sends.get(), 0);
    }
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    d.c.execute("UPDATE api_keys SET key_type=0", []).unwrap();
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 0);
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert_eq!(d.count("rule_logs"), 1);
    for during in ["send", "lookup"] {
        let d = Db::new();
        let f = Fake::new(&d);
        let i = d.prepared(&f).await;
        if during == "send" {
            f.rotate_at.set(f.read.get() + 3);
            w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                .await
                .unwrap();
        } else {
            w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                .await
                .unwrap();
            f.rotate_at.set(f.read.get() + 1);
            *f.lookup.borrow_mut() = w::LookupResult::Found {
                id: "X".into(),
                status: w::FoundStatus::Sent,
            };
            w::recover(&d.c, &d.cipher, &f, &i.owner, &FixedClock(now()))
                .await
                .unwrap();
        }
        assert_eq!(d.count("rule_logs"), 0);
        assert_eq!(d.count("rust_withdrawal_intents"), 1);
    }
}
#[tokio::test]
async fn unreadable_and_unknown_rejections_remain_accounted_for() {
    for result in [
        w::SendResult::Accepted { id: String::new() },
        w::SendResult::Rejected {
            definitive: true,
            reason: "Failed to obtain tranId.".into(),
        },
        w::SendResult::Ambiguous {
            reason: "unreadable body".into(),
        },
    ] {
        let d = Db::new();
        let f = Fake::new(&d);
        *f.answer.borrow_mut() = result;
        let i = d.prepared(&f).await;
        w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
            .await
            .unwrap();
        for _ in 0..2 {
            w::recover(
                &d.c,
                &d.cipher,
                &f,
                &i.owner,
                &FixedClock(now() + chrono::Duration::days(100)),
            )
            .await
            .unwrap();
        }
        assert_eq!(f.sends.get(), 1);
        assert_eq!(d.count("rule_logs"), 0);
        assert!(rules::status(&d.c, 1).unwrap().is_some());
    }
}
#[tokio::test]
async fn stopped_evaluation_and_fire_restart() {
    let d = Db::new();
    let f = Fake::new(&d);
    d.c.execute("UPDATE rules SET status=2", []).unwrap();
    assert!(
        w::evaluate(&d.c, &d.cipher, &f, d.eval(), &FixedClock(now()))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(d.count("rule_logs"), 0);
    assert!(rules::handback_problems(&d.c).unwrap().is_empty());
    d.c.execute("UPDATE rules SET status=1", []).unwrap();
    rules::fire(&d.c, now()).unwrap();
    assert_eq!(d.count("rust_rule_evaluations"), 1);
    rules::fire(&d.c, now() + chrono::Duration::hours(4)).unwrap();
    rules::fire(&d.c, now() + chrono::Duration::hours(4)).unwrap();
    assert_eq!(d.count("rust_rule_evaluations"), 2);
}
#[test]
fn every_x0_classifier_vector_controls_takeover() {
    let v: Value = serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    let definitive = [
        "gemini_http_400_insufficient_funds",
        "gemini_http_400_invalid_signature",
        "binance_code_4026",
        "binance_code_2015",
        "binance_code_1021",
        "kraken_max_fee_exceeded",
        "mexc_code_10101",
        "mexc_no_coin_data",
    ];
    let mut count = 0;
    for cell in v["classifier"].as_array().unwrap() {
        let d = Db::new();
        let name = cell["case"].as_str().unwrap();
        let venue = cell["venue"].as_str().unwrap();
        d.c.execute(
            "UPDATE exchanges SET type=?1",
            [format!(
                "Exchanges::{}",
                match venue {
                    "binance" => "Binance",
                    "kraken" => "Kraken",
                    "gemini" => "Gemini",
                    _ => "Mexc",
                }
            )],
        )
        .unwrap();
        let message = cell["rule_logs"][0]["message"].as_str().unwrap();
        let id = d.log(message, None);
        let expected = !definitive.contains(&name) && !name.ends_with("unlisted_asset");
        let problems = rules::takeover_problems(&d.c, &d.q).unwrap();
        assert_eq!(
            !problems.is_empty(),
            expected,
            "D-RULE-AMBIGUITY-ALLOWLIST {name}"
        );
        if expected {
            assert!(problems.join(" ").contains(&id.to_string()));
        }
        count += 1;
    }
    assert_eq!(
        count,
        8 + 18 + 4,
        "8 definitive, 18 ambiguous, 4 pre-send rows"
    );
}
#[test]
fn two_attempts_require_independent_acks_in_either_order() {
    for reverse in [false, true] {
        let d = Db::new();
        let a = d.log("Withdrawal failed: Net::ReadTimeout", None);
        let b = d.log("Unexpected error: bad reply", None);
        d.log("Withdrew", Some("success"));
        let order = if reverse { [b, a] } else { [a, b] };
        assert_eq!(rules::takeover_problems(&d.c, &d.q).unwrap().len(), 2);
        for (i, id) in order.into_iter().enumerate() {
            w::resolve(
                &d.c,
                &d.q,
                &d.cipher,
                w::Target::Rails(id),
                w::Resolution::NotSent,
                now(),
            )
            .unwrap();
            assert_eq!(rules::takeover_problems(&d.c, &d.q).unwrap().len(), 1 - i);
        }
    }
}
#[tokio::test]
async fn all_resolvers_seed_before_takeover_and_refuse_historical_id() {
    for mode in 0..3 {
        let d = Db::new();
        let f = Fake::new(&d);
        let log = d.log("Withdrawal failed: Net::ReadTimeout", None);
        d.log("Withdrew", Some("X"));
        d.job("job", false, true);
        if mode == 2 {
            let i = d.prepared(&f).await;
            w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                .await
                .unwrap();
        }
        let target = match mode {
            0 => w::Target::Rails(log),
            1 => w::Target::RailsJob("job".into()),
            _ => w::Target::Rule(1),
        };
        assert!(w::resolve(
            &d.c,
            &d.q,
            &d.cipher,
            target,
            w::Resolution::Sent {
                id: "X".into(),
                amount: Some("0.9".into())
            },
            now()
        )
        .is_err());
        assert_eq!(d.count("rust_rails_attempt_acks"), 0);
        assert_eq!(d.count("rust_rails_job_acks"), 0);
        assert_eq!(d.count("rule_logs"), 2);
        assert_eq!(d.count("rust_withdrawal_ids"), 1);
    }
}
#[test]
fn import_and_cleanup_restart_boundaries() {
    for stage in 1..=3 {
        let d = Db::new();
        d.job("job", false, false);
        let jobs = rules::jobs(&d.c, &d.q).unwrap();
        {
            let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
            rules::import(&tx, &jobs, now()).unwrap();
            tx.commit().unwrap();
        }
        if stage >= 2 {
            rules::cleanup_queue(&d.c, &d.q).unwrap();
        }
        if stage >= 3 {
            rules::prune_journals(&d.c, &d.q, now()).unwrap();
        }
        let jobs = rules::jobs(&d.c, &d.q).unwrap();
        let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
        rules::import(&tx, &jobs, now()).unwrap();
        tx.commit().unwrap();
        rules::finish_cleanup(&d.c, &d.q, now()).unwrap();
        assert_eq!(d.count("rust_rule_evaluations"), 1);
        assert!(!rules::journals_pending(&d.c).unwrap());
    }
}
#[test]
fn evaluate_all_claimed_skip_and_recorded_child_do_not_refan() {
    let d = Db::new();
    d.job("all", true, false);
    let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
    rules::import(&tx, &rules::jobs(&tx, &d.q).unwrap(), now()).unwrap();
    tx.commit().unwrap();
    assert_eq!(d.count("rust_rule_evaluations"), 1);
    let d = Db::new();
    d.job("all", true, true);
    d.job("child", false, true);
    assert_eq!(rules::takeover_problems(&d.c, &d.q).unwrap().len(), 2);
    let log = d.log("Withdrew", Some("X"));
    d.c.execute(
        "UPDATE rule_logs SET details=json_set(details,'$.withdrawal_id',42) WHERE id=?1",
        [log],
    )
    .unwrap();
    w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::RailsJob("child".into()),
        w::Resolution::Recorded(log),
        now(),
    )
    .unwrap();
    w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::RailsJob("all".into()),
        w::Resolution::Skip,
        now(),
    )
    .unwrap();
    assert!(rules::takeover_problems(&d.c, &d.q).unwrap().is_empty());
    rules::finish_cleanup(&d.c, &d.q, now()).unwrap();
    let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
    rules::import(&tx, &rules::jobs(&tx, &d.q).unwrap(), now()).unwrap();
    tx.commit().unwrap();
    assert_eq!(d.count("rust_rule_evaluations"), 0);
    assert_eq!(d.count("rule_logs"), 1);
}
#[test]
fn recorded_job_and_ambiguous_log_evidence_are_same_rule() {
    let d = Db::new();
    d.job("child", false, true);
    let ambiguous = d.log("Withdrawal failed: Net::ReadTimeout", None);
    w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::Rails(ambiguous),
        w::Resolution::Sent {
            id: "X".into(),
            amount: None,
        },
        now(),
    )
    .unwrap();
    w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::RailsJob("child".into()),
        w::Resolution::Recorded(ambiguous),
        now(),
    )
    .unwrap();
    assert_eq!(d.count("rust_withdrawal_ids"), 1);
    assert_eq!(d.count("rule_logs"), 2);
}
#[tokio::test]
async fn rust_log_provenance_and_secret_scrubbing() {
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    w::mark_dispatched(
        &d.c,
        &d.cipher,
        &f,
        &i,
        &FixedClock(now() + chrono::Duration::hours(1)),
    )
    .await
    .unwrap();
    assert_eq!(d.count("rust_rule_log_origins"), 1);
    assert!(rules::takeover_problems(&d.c, &d.q).unwrap().is_empty());
    assert!(rules::handback_problems(&d.c).unwrap().is_empty());
    let v: Value = serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    let cell = v["classifier"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["case"] == "binance_empty_500")
        .unwrap();
    let safe = rules::safe_message(cell["rule_logs"][0]["message"].as_str().unwrap());
    assert!(!safe.contains("http"));
    assert!(!safe.contains("signature"));
    assert!(safe.chars().count() <= 200);
}
#[tokio::test]
async fn missing_key_refuses_recorded_rails_send_and_all_production_venues_stay_disabled() {
    let d = Db::new();
    let f = Fake::new(&d);
    d.c.execute("DELETE FROM api_keys", []).unwrap();
    w::evaluate(&d.c, &d.cipher, &f, d.eval(), &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 0);
    assert_eq!(d.count("rule_logs"), 1);
    assert_eq!(f.read.get(), 0, "no request with an empty signing key");
    let outcome: (i64, String) =
        d.c.query_row("SELECT status,message FROM rule_logs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(outcome,(2,"Failed to fetch balance: No valid withdrawal API key for Binance. Add one in Settings.".into()));
    let v: Value = serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    let cell = v["execute"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["case"] == "no_withdrawal_key")
        .unwrap();
    assert_eq!(
        cell["rule_logs"][0]["status"], "success",
        "D-RULE-NO-KEY-SENDS"
    );
    for row in deltabadger::venue::registry::ROWS.iter() {
        assert!(!row.withdraws);
    }
}
#[tokio::test]
async fn sizing_property_both_fee_semantics() {
    for free in ["0.00000001", "0.01", "1", "1000000"] {
        for pct in ["0.01", "25", "100"] {
            for fee in ["0", "0.0001", "0.1", "100"] {
                for on_top in [None, Some(true), Some(false)] {
                    let fee = n(fee);
                    let s = v::size_with_semantics(&n(free), &n(pct), None, on_top, |_| {
                        std::future::ready(Ok(Some(fee.clone())))
                    })
                    .await
                    .unwrap();
                    if let v::Sizing::Send {
                        budget,
                        amount,
                        fee_cap,
                        ..
                    } = s
                    {
                        let debit = if on_top == Some(false) {
                            amount
                        } else {
                            &amount + &fee_cap
                        };
                        assert!(debit <= budget && budget <= n(free));
                    }
                }
            }
        }
    }
}
#[test]
fn cli_all_modes_amount_and_list() {
    for args in [
        vec!["--rails-list", "1"],
        vec!["--rails", "1", "--sent", "X", "--amount", "0.1"],
        vec!["--rails-job", "job", "--sent", "X", "--amount", "0.1"],
        vec!["--rails-job", "job", "--recorded", "3"],
        vec!["--rails-job", "job", "--skip"],
        vec!["1", "--not-sent"],
    ] {
        assert!(w::parse_cli(&args.into_iter().map(str::to_string).collect::<Vec<_>>()).is_ok());
    }
}
#[tokio::test]
async fn chain_fee_uses_one_basis_against_recorded_cell() {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    let cell = vectors["execute"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["case"] == "chain_fee")
        .unwrap();
    assert_eq!(cell["rule_logs"][0]["details"]["amount"], "0.9998");
    assert_eq!(cell["rule_logs"][0]["details"]["fee"], "0.0002");
    let d = Db::new();
    d.c.execute(
        "UPDATE rules SET settings=json_set(settings,'$.network','LIGHTNING')",
        [],
    )
    .unwrap();
    let mut f = Fake::new(&d);
    f.network = "LIGHTNING";
    f.fee = "0.00001";
    let i = d.prepared(&f).await;
    assert_eq!(
        i.request.amount, "0.99999",
        "D-RULE-CHAIN-FEE: the selected network fee sizes the amount"
    );
    assert_eq!(i.request.fee_cap, "0.00001");
    let basis: String =
        d.c.query_row(
            "SELECT json_extract(fee_basis,'$.first_fee') FROM rust_withdrawal_intents",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(basis, i.request.fee_cap);
}
#[test]
fn unexpected_error_requires_operator_amount_and_recorded_rejects_unrelated_rule() {
    let d = Db::new();
    let log = d.log("Unexpected error: unreadable", None);
    d.c.execute("UPDATE rule_logs SET details='{}' WHERE id=?1", [log])
        .unwrap();
    assert!(w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::Rails(log),
        w::Resolution::Sent {
            id: "X".into(),
            amount: None
        },
        now()
    )
    .is_err());
    w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::Rails(log),
        w::Resolution::Sent {
            id: "X".into(),
            amount: Some("0.3".into()),
        },
        now(),
    )
    .unwrap();
    d.job("job", false, true);
    let unrelated = d.log("Withdrew", Some("other"));
    d.c.execute("UPDATE rule_logs SET rule_id=2 WHERE id=?1", [unrelated])
        .unwrap();
    assert!(w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::RailsJob("job".into()),
        w::Resolution::Recorded(unrelated),
        now()
    )
    .is_err());
}
#[tokio::test]
async fn candidates_are_visible_but_never_settle() {
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    *f.lookup.borrow_mut() = w::LookupResult::Unknown {
        reason: "operator evidence needed".into(),
        candidates: vec![json!({"id":"unrelated","status":"Pending"})],
    };
    w::recover(&d.c, &d.cipher, &f, &i.owner, &FixedClock(now()))
        .await
        .unwrap();
    let s = rules::status(&d.c, 1).unwrap().unwrap();
    assert_eq!(s.candidates.len(), 1);
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(f.sends.get(), 0);
}
#[test]
fn concurrent_starts_each_keep_their_fanout() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("starts.sqlite");
    let c = Connection::open(&path).unwrap();
    c.execute_batch("CREATE TABLE rules(id INTEGER PRIMARY KEY,type TEXT,status INTEGER); INSERT INTO rules VALUES(1,'Rules::Withdrawal',1),(2,'Rules::Withdrawal',1);").unwrap();
    c.execute_batch(include_str!(
        "../migrations/20261026000001_withdrawal_rules.sql"
    ))
    .unwrap();
    let handles = (0..2)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let c = Connection::open(path).unwrap();
                c.busy_timeout(std::time::Duration::from_secs(5)).unwrap();
                let tx = deltabadger::engine::model::immediate(&c).unwrap();
                rules::start_evaluations(&tx, now()).unwrap();
                tx.commit().unwrap();
            })
        })
        .collect::<Vec<_>>();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(
        c.query_row("SELECT count(*) FROM rust_rule_evaluations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
}
#[tokio::test]
async fn changed_owner_client_or_state_cannot_delete_or_settle() {
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    d.c.execute("UPDATE rust_withdrawal_intents SET client_id='newer'", [])
        .unwrap();
    assert!(!w::settle(
        &d.c,
        &d.cipher,
        &i,
        "dispatched",
        w::SendResult::Accepted { id: "stale".into() },
        now()
    )
    .unwrap());
    d.c.execute("UPDATE api_keys SET status=0", []).unwrap();
    assert!(
        !w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
            .await
            .unwrap()
    );
    assert_eq!(d.count("rust_withdrawal_intents"), 1);
    assert_eq!(d.count("rule_logs"), 0);
}
#[test]
fn claimed_job_ack_cleanup_crashes_are_idempotent() {
    for stage in 1..=3 {
        let d = Db::new();
        d.job("claimed", false, true);
        w::resolve(
            &d.c,
            &d.q,
            &d.cipher,
            w::Target::RailsJob("claimed".into()),
            w::Resolution::NotSent,
            now(),
        )
        .unwrap();
        if stage >= 2 {
            rules::cleanup_queue(&d.c, &d.q).unwrap();
        }
        if stage >= 3 {
            rules::prune_journals(&d.c, &d.q, now()).unwrap();
        }
        assert!(rules::takeover_problems(&d.c, &d.q).unwrap().is_empty());
        rules::finish_cleanup(&d.c, &d.q, now()).unwrap();
        assert_eq!(d.count("rust_rails_job_acks"), 1);
        assert_eq!(d.count("rust_rule_evaluations"), 0);
        assert!(!rules::journals_pending(&d.c).unwrap());
    }
}
#[tokio::test]
async fn settlement_uses_immutable_persisted_parameters() {
    let d = Db::new();
    let f = Fake::new(&d);
    let mut i = d.prepared(&f).await;
    w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    i.request.amount = "99999".into();
    w::settle(
        &d.c,
        &d.cipher,
        &i,
        "dispatched",
        w::SendResult::Accepted { id: "X".into() },
        now(),
    )
    .unwrap();
    let amount: String =
        d.c.query_row(
            "SELECT json_extract(details,'$.amount') FROM rule_logs",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(amount, "0.9999");
}
#[tokio::test]
async fn stopped_after_begin_drops_only_prepared_without_a_row() {
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    d.c.execute("UPDATE rules SET status=2", []).unwrap();
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert_eq!(f.sends.get(), 0);
}

#[tokio::test]
async fn exact_found_failed_status_is_proof_but_candidates_are_not() {
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    *f.lookup.borrow_mut() = w::LookupResult::Found {
        id: "failed-id".into(),
        status: w::FoundStatus::Failed {
            reason: "withdrawal canceled by venue".into(),
        },
    };
    w::recover(&d.c, &d.cipher, &f, &i.owner, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert_eq!(d.count("rule_logs WHERE status=2"), 1);
    assert_eq!(f.sends.get(), 0);
}

#[test]
fn unresolved_log_can_be_linked_to_job_without_discharging_the_log() {
    let d = Db::new();
    d.job("child", false, true);
    let log = d.log("Unexpected error: unreadable", None);
    w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::RailsJob("child".into()),
        w::Resolution::Recorded(log),
        now(),
    )
    .unwrap();
    rules::finish_cleanup(&d.c, &d.q, now()).unwrap();
    assert_eq!(rules::takeover_problems(&d.c, &d.q).unwrap().len(), 1);
    w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::Rails(log),
        w::Resolution::NotSent,
        now(),
    )
    .unwrap();
    assert!(rules::takeover_problems(&d.c, &d.q).unwrap().is_empty());
}

#[tokio::test]
async fn imported_execute_and_start_fanout_restarts_send_exactly_once() {
    for all in [false, true] {
        let d = Db::new();
        d.job("pending", all, false);
        let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
        rules::import(&tx, &rules::jobs(&tx, &d.q).unwrap(), now()).unwrap();
        tx.commit().unwrap();
        rules::finish_cleanup(&d.c, &d.q, now()).unwrap();
        let f = Fake::new(&d);
        let i = w::evaluate(&d.c, &d.cipher, &f, 1, &FixedClock(now()))
            .await
            .unwrap()
            .unwrap();
        w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
            .await
            .unwrap();
        assert!(w::evaluate(&d.c, &d.cipher, &f, 1, &FixedClock(now()))
            .await
            .unwrap()
            .is_none());
        w::recover(&d.c, &d.cipher, &f, &i.owner, &FixedClock(now()))
            .await
            .unwrap();
        assert_eq!(f.sends.get(), 1);
    }
}

fn reopen(path: &std::path::Path) -> Db {
    Db {
        c: Connection::open(path).unwrap(),
        q: Connection::open(path.with_extension("queue")).unwrap(),
        cipher: Cipher::new(&EncryptionKeys::resolve(&|_| None, "withdrawal-test").unwrap()),
    }
}

/// Invoked only by crash_after_every_committed_transaction: exit without Drop, at the named durable boundary.
#[tokio::test]
async fn crash_worker() {
    let Ok(boundary) = std::env::var("E0_CRASH_AT") else {
        return;
    };
    let path = std::path::PathBuf::from(std::env::var("E0_CRASH_DB").unwrap());
    let d = reopen(&path);
    let mut f = Fake::new(&d);
    f.counter = Some(path.with_extension("sends"));
    match boundary.as_str() {
        "fire" => {
            rules::fire(&d.c, now()).unwrap();
            rules::fire(&d.c, now() + chrono::Duration::hours(4)).unwrap();
        }
        "start" | "stop" => {
            let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
            rules::start_evaluations(&tx, now()).unwrap();
            tx.commit().unwrap();
            if boundary == "stop" {
                let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
                tx.execute("UPDATE rules SET status=2", []).unwrap();
                tx.execute("UPDATE rust_rule_evaluations SET consumed_at='2026-10-28',outcome='skipped' WHERE consumed_at IS NULL",[]).unwrap();
                tx.commit().unwrap();
            }
        }
        "import1" | "import2" | "import3" => {
            d.job("pending", false, false);
            let tx = deltabadger::engine::model::immediate(&d.c).unwrap();
            rules::import(&tx, &rules::jobs(&tx, &d.q).unwrap(), now()).unwrap();
            tx.commit().unwrap();
            if boundary != "import1" {
                rules::cleanup_queue(&d.c, &d.q).unwrap();
            }
            if boundary == "import3" {
                rules::prune_journals(&d.c, &d.q, now()).unwrap();
            }
        }
        "ack" | "cleanup" => {
            d.job("claimed", false, true);
            w::resolve(
                &d.c,
                &d.q,
                &d.cipher,
                w::Target::RailsJob("claimed".into()),
                w::Resolution::NotSent,
                now(),
            )
            .unwrap();
            if boundary == "cleanup" {
                rules::cleanup_queue(&d.c, &d.q).unwrap();
            }
        }
        "begin" | "dispatch" | "settle" => {
            let i = d.prepared(&f).await;
            if boundary == "dispatch" {
                w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                    .await
                    .unwrap();
            }
            if boundary == "settle" {
                w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                    .await
                    .unwrap();
            }
        }
        _ => panic!("unknown crash boundary"),
    }
    std::process::exit(74);
}

#[tokio::test]
async fn crash_after_every_committed_transaction() {
    for boundary in [
        "fire", "start", "stop", "import1", "import2", "import3", "begin", "dispatch", "settle",
        "ack", "cleanup",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("primary.sqlite");
        let seed = Db::new();
        seed.c
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        seed.q
            .execute(
                "VACUUM INTO ?1",
                [path.with_extension("queue").to_str().unwrap()],
            )
            .unwrap();
        drop(seed);
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "crash_worker", "--nocapture"])
            .env("E0_CRASH_AT", boundary)
            .env("E0_CRASH_DB", &path)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(74),
            "{boundary}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let d = reopen(&path);
        let mut f = Fake::new(&d);
        f.counter = Some(path.with_extension("sends"));
        rules::finish_cleanup(&d.c, &d.q, now()).unwrap();
        w::recover(&d.c, &d.cipher, &f, "rule:1", &FixedClock(now()))
            .await
            .unwrap();
        let pending = d
            .c
            .prepare("SELECT id FROM rust_rule_evaluations WHERE consumed_at IS NULL ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for id in pending {
            if let Some(i) = w::evaluate(&d.c, &d.cipher, &f, id, &FixedClock(now()))
                .await
                .unwrap()
            {
                w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                    .await
                    .unwrap();
            }
        }
        let sends = std::fs::read(path.with_extension("sends"))
            .unwrap_or_default()
            .len();
        let expected = usize::from(matches!(
            boundary,
            "fire" | "start" | "import1" | "import2" | "import3" | "settle"
        ));
        assert_eq!(
            sends, expected,
            "{boundary}: exact external send count across process death"
        );
        assert_eq!(
            rules::status(&d.c, 1).unwrap().is_some(),
            boundary == "dispatch",
            "{boundary}: recovery status line"
        );
        if boundary == "dispatch" {
            assert_eq!(
                w::load(&d.c, &d.cipher, "rule:1").unwrap().unwrap().state,
                "unresolved"
            );
        }
    }
}

#[tokio::test]
async fn generated_sizing_property_percentage_fees_and_venue_multiples() {
    use rand::{Rng, SeedableRng};
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xe0);
    for _ in 0..1000 {
        let free = n(&format!(
            "{}.{}",
            rng.gen_range(0..1000),
            rng.gen_range(10000000..99999999)
        ));
        let pct = n(&rng.gen_range(1..=100).to_string());
        let rate = n(&format!("0.{:04}", rng.gen_range(0..100)));
        let multiple = n(if rng.gen_bool(0.5) {
            "0.00000001"
        } else {
            "0.001"
        });
        let included = rng.gen_bool(0.5);
        let sized =
            v::size_with_semantics(&free, &pct, Some(&multiple), Some(!included), |amount| {
                std::future::ready(Ok(Some(&amount * &rate)))
            })
            .await
            .unwrap();
        if let v::Sizing::Send {
            budget,
            amount,
            fee_cap,
            ..
        } = sized
        {
            let debit = if included {
                amount.clone()
            } else {
                &amount + &fee_cap
            };
            assert!(debit <= budget && budget <= free);
            assert_eq!(v::floor_p(&amount, Some(&multiple)), amount);
        }
    }
}

#[tokio::test]
async fn status_role_and_ownership_are_fenced_even_when_the_digest_is_unchanged() {
    for update in ["status=0", "key_type=0", "user_id=2", "exchange_id=2"] {
        let d = Db::new();
        let f = Fake::new(&d);
        let i = d.prepared(&f).await;
        d.c.execute(&format!("UPDATE api_keys SET {update}"), [])
            .unwrap();
        assert!(
            !w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                .await
                .unwrap(),
            "{update}"
        );
        assert_eq!(d.count("rust_withdrawal_intents"), 0);
        assert_eq!(d.count("rule_logs"), 1);
        assert_eq!(f.sends.get(), 0);
    }
    let d = Db::new();
    let f = Fake::new(&d);
    d.c.execute(
        "UPDATE api_keys SET secret=?1",
        [d.cipher.encrypt("new-secret")],
    )
    .unwrap();
    assert!(
        w::evaluate(&d.c, &d.cipher, &f, d.eval(), &FixedClock(now()))
            .await
            .is_err()
    );
    assert_eq!(
        f.read.get(),
        0,
        "a handle captured before rotation cannot label new credentials"
    );
    assert_eq!(
        d.count("rust_rule_evaluations WHERE consumed_at IS NULL"),
        1
    );
}

#[tokio::test]
async fn destination_is_validated_at_begin_and_again_before_send() {
    let d = Db::new();
    let mut f = Fake::new(&d);
    f.network = "LIGHTNING";
    assert!(
        w::evaluate(&d.c, &d.cipher, &f, d.eval(), &FixedClock(now()))
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert_eq!(f.sends.get(), 0);
    let d = Db::new();
    let mut f = Fake::new(&d);
    let i = d.prepared(&f).await;
    f.network = "LIGHTNING";
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 0);
    assert_eq!(d.count("rule_logs"), 1);
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
}

fn old_address() -> String {
    Cipher::new(&EncryptionKeys::resolve(&|_| None, "previous-key").unwrap()).encrypt("destination")
}

#[tokio::test]
async fn inactive_unreadable_evaluation_skips_before_decryption() {
    for status in [0, 2, 3, 4, 5, 6, 7] {
        let d = Db::new();
        let f = Fake::new(&d);
        d.c.execute(
            "UPDATE rules SET status=?1,address=?2,settings='unreadable'",
            params![status, old_address()],
        )
        .unwrap();
        let evaluation = d.eval();
        assert!(
            w::evaluate(&d.c, &d.cipher, &f, evaluation, &FixedClock(now()))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            d.count("rust_rule_evaluations WHERE consumed_at IS NOT NULL AND outcome='skipped'"),
            1
        );
        assert_eq!(d.count("rule_logs"), 0);
        assert_eq!(f.read.get(), 0);
    }
}

#[tokio::test]
async fn active_unreadable_evaluation_logs_once_without_venue_reads() {
    for (column, value, message) in [
        (
            "address",
            old_address(),
            "withdrawal address cannot be decrypted",
        ),
        (
            "settings",
            "invalid-json".into(),
            "unreadable withdrawal settings",
        ),
    ] {
        let d = Db::new();
        let f = Fake::new(&d);
        d.c.execute(&format!("UPDATE rules SET {column}=?1"), [value])
            .unwrap();
        let evaluation = d.eval();
        for _ in 0..2 {
            assert!(
                w::evaluate(&d.c, &d.cipher, &f, evaluation, &FixedClock(now()))
                    .await
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(
            d.count("rust_rule_evaluations WHERE consumed_at IS NOT NULL AND outcome='logged'"),
            1
        );
        assert_eq!(d.count("rule_logs WHERE status=2"), 1);
        let logged: String =
            d.c.query_row("SELECT message FROM rule_logs", [], |r| r.get(0))
                .unwrap();
        assert_eq!(logged, format!("Unexpected error: {message}"));
        assert_eq!(d.count("rust_withdrawal_intents"), 0);
        assert_eq!(f.read.get(), 0);
    }
}

async fn engine_steps_with_unreadable_rule(status: i64) {
    use deltabadger::{
        engine::run,
        lease,
        store::{self, Paths},
        venue::fake::{FakeFactory, FakeVenue},
    };
    let dir = common::rails_install();
    let paths = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&paths, now()).unwrap();
    let opened = store::open(&paths).unwrap();
    let cipher = common::seed::cipher();
    let seed = common::seed::seed_kraken(&opened.primary, &cipher);
    opened.primary.execute("INSERT INTO rules(id,user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES(1,?1,'Rules::Withdrawal',?2,?3,?4,?5,'{}',?6,?6)", params![seed.user_id,status,seed.exchange_id,seed.btc,old_address(),now().to_rfc3339()]).unwrap();
    // A ready Rails ExecuteJob for the unreadable rule is imported before the first engine pass.
    let queue = Db::new();
    queue.job("old-key-job", false, false);
    let tx = deltabadger::engine::model::immediate(&opened.primary).unwrap();
    rules::import(&tx, &rules::jobs(&tx, &queue.q).unwrap(), now()).unwrap();
    tx.commit().unwrap();
    rules::finish_cleanup(&opened.primary, &queue.q, now()).unwrap();
    let mut engine = run::Engine::new(opened.primary, FakeFactory(FakeVenue::new()), cipher, lock);
    for pass in 0..2 {
        // A later evaluation proves that the same phase keeps processing other rules on every pass.
        engine.primary.execute("INSERT OR IGNORE INTO rules(id,user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES(?1,?2,'Rules::Withdrawal',2,?3,?4,?5,'{}',?6,?6)", params![2,seed.user_id,seed.exchange_id,seed.quote,engine.cipher.encrypt("other"),now().to_rfc3339()]).unwrap();
        engine
            .primary
            .execute(
                "INSERT INTO rust_rule_evaluations(rule_id,source,created_at) VALUES(?1,?2,?3)",
                params![2, format!("start:other:{pass}"), now().to_rfc3339()],
            )
            .unwrap();
        if status == 1 {
            // E0 intentionally refuses active rules at engine eligibility until an adapter passes G0.
            // Exercise the exact withdrawal phase called by step without enabling production sends.
            w::phase(
                &engine.primary,
                &engine.cipher,
                &engine.factory,
                &FixedClock(now()),
            )
            .await
            .unwrap();
        } else {
            run::step(&mut engine, &FixedClock(now())).await.unwrap();
        }
        let pending: i64 = engine
            .primary
            .query_row(
                "SELECT count(*) FROM rust_rule_evaluations WHERE consumed_at IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pending, 0);
        let logged: i64 = engine
            .primary
            .query_row("SELECT count(*) FROM rule_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(logged, i64::from(status == 1));
    }
    let outcome: String = engine
        .primary
        .query_row(
            "SELECT outcome FROM rust_rule_evaluations WHERE rule_id=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(outcome, if status == 1 { "logged" } else { "skipped" });
}

#[tokio::test]
async fn stopped_old_key_job_does_not_end_engine_steps() {
    engine_steps_with_unreadable_rule(2).await;
}

#[tokio::test]
async fn active_old_key_job_does_not_end_withdrawal_phase() {
    engine_steps_with_unreadable_rule(1).await;
}

#[tokio::test]
async fn legacy_no_tag_rule_dispatches_captured_tag() {
    let d = Db::new();
    let mut f = Fake::new(&d);
    f.tag = Some("123");
    let i = d.prepared(&f).await;
    assert!(
        matches!(&i.request.destination, v::Destination::Binance { tag: Some(tag), .. } if tag == "123")
    );
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 1);
    assert_eq!(d.count("rule_logs WHERE status=1"), 1);
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
}

#[tokio::test]
async fn mexc_network_alias_dispatches_normalized_destination() {
    let d = Db::new();
    d.c.execute(
        "UPDATE exchanges SET type='Exchanges::Mexc',name='MEXC'",
        [],
    )
    .unwrap();
    d.c.execute(
        "UPDATE rules SET settings=json_set(settings,'$.network','BTC-CODE')",
        [],
    )
    .unwrap();
    let mut f = Fake::new(&d);
    f.net_work = Some("BTC-CODE");
    let i = d.prepared(&f).await;
    assert!(
        matches!(&i.request.destination, v::Destination::Mexc { network, net_work, .. } if network == "BTC" && net_work == "BTC-CODE")
    );
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 1);
    assert_eq!(d.count("rule_logs WHERE status=1"), 1);
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
}

#[tokio::test]
async fn destination_changes_before_or_during_dispatch_delete_unsent() {
    for field in ["address", "address_tag", "network"] {
        for during_read in [false, true] {
            let d = Db::new();
            let mut f = Fake::new(&d);
            let i = d.prepared(&f).await;
            let change = if field == "address" {
                format!("UPDATE rules SET address='{}'", d.cipher.encrypt("changed"))
            } else {
                format!("UPDATE rules SET settings=json_set(settings,'$.{field}','changed')")
            };
            if during_read {
                f.change = change;
                f.change_at.set(f.read.get() + 2);
            } else {
                d.c.execute(&change, []).unwrap();
            }
            w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                .await
                .unwrap();
            assert_eq!(f.sends.get(), 0, "{field}, during read: {during_read}");
            assert_eq!(d.count("rust_withdrawal_intents"), 0);
            assert_eq!(d.count("rule_logs WHERE status=2"), 1);
        }
    }
}
