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
async fn missing_key_refuses_recorded_rails_send() {
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

// E2: the production VenueAdapter and KrakenVenue, with offline HTTP answers.
use deltabadger::venue::{http::ScriptedTransport, kraken::KrakenVenue};
use w::Adapter;
const K_SEND: &str = "POST /0/private/Withdraw";
const K_STATUS: &str = "POST /0/private/WithdrawStatus";
fn kraken_db() -> Db {
    let d = Db::new();
    d.c.execute_batch("UPDATE exchanges SET type='Exchanges::Kraken',name='Kraken'; UPDATE tickers SET base='XBT'; UPDATE rules SET settings=json_set(settings,'$.address_name','cold');").unwrap();
    d
}
fn kraken_adapter(
    d: &Db,
    t: &ScriptedTransport,
) -> w::VenueAdapter<KrakenVenue<ScriptedTransport>> {
    let (cr, version) = deltabadger::engine::model::withdrawal_credentials(&d.c, &d.cipher, 1, 1)
        .unwrap()
        .unwrap();
    w::VenueAdapter::new(
        KrakenVenue::new(t.clone()).with_identity(&cr),
        version,
        None,
    )
}
fn kraken_reads() -> ScriptedTransport {
    let t = ScriptedTransport::default();
    t.reply(
        "POST /0/private/BalanceEx",
        200,
        json!({"error":[],"result":{"XBT":{"balance":"1","hold_trade":"0"}}}),
    );
    t.reply("POST /0/private/WithdrawAddresses", 200, json!({"error":[],"result":[{"address":"destination","key":"cold","method":"Bitcoin","verified":true}]}));
    t.reply(
        "POST /0/private/WithdrawInfo",
        200,
        json!({"error":[],"result":{"amount":"0.99","fee":"0.01"}}),
    );
    t.reply(
        "POST /0/private/WithdrawInfo",
        200,
        json!({"error":[],"result":{"amount":"0.9801","fee":"0.0099"}}),
    );
    t.reply(
        K_STATUS,
        200,
        json!({"error":[],"result":{"withdrawals":[],"cursor":null}}),
    );
    t
}
async fn kraken_prepared(d: &Db, t: &ScriptedTransport) -> w::Intent {
    w::evaluate(
        &d.c,
        &d.cipher,
        &kraken_adapter(d, t),
        d.eval(),
        &FixedClock(now()),
    )
    .await
    .unwrap()
    .unwrap()
}
fn kraken_sends(t: &ScriptedTransport) -> usize {
    t.requests()
        .iter()
        .filter(|r| r.path == "/0/private/Withdraw")
        .count()
}
#[tokio::test]
async fn kraken_g0_answers_restart_without_resends() {
    let answers = [
        (200, json!({"error":[],"result":{"refid":"K-1"}}), Some(1)),
        (200, json!({"error":["EFunding:Max fee exceeded"]}), Some(2)),
        (200, json!({"error":["EGeneral:Internal error"]}), None),
        (200, json!({"error":["EService:Unavailable"]}), None),
        (500, json!({"error":["EFunding:Max fee exceeded"]}), None),
        (200, json!("not JSON"), None),
        (200, json!({"error":[],"result":{}}), None),
        (200, json!({"error":[],"result":{"refid":" "}}), None),
        (200, json!({"result":{"refid":"K-1"}}), None),
        (
            200,
            json!({"error":["EFunding:Max fee exceeded","EService:Unavailable"]}),
            None,
        ),
        (200, json!({"error":[null]}), None),
        (200, json!({"error":["Unknown symbol for asset XBT"]}), None),
        (200, json!({"error":["EAPI:Unrecognized code"]}), None),
        (
            200,
            json!({"error":["EFunding:Max fee exceeded"],"result":{"refid":"K-1"}}),
            None,
        ),
        (200, json!({"error":["EGeneral:Invalid arguments"]}), None),
    ];
    for (http, body, log) in answers {
        let d = kraken_db();
        let t = kraken_reads();
        t.reply(K_SEND, http, body.clone());
        let i = kraken_prepared(&d, &t).await;
        let a = kraken_adapter(&d, &t);
        // A caller changing its in-memory copy must not change the signed request.
        let mut edited = i.clone();
        edited.request.amount = "99".into();
        edited.request.fee_cap = "50".into();
        w::dispatch(&d.c, &d.cipher, &a, &edited, &FixedClock(now()))
            .await
            .unwrap();
        assert_eq!(d.count("rule_logs"), i64::from(log.is_some()), "{body}");
        if let Some(expected) = log {
            assert_eq!(
                d.c.query_row("SELECT status FROM rule_logs", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                expected,
                "{body}"
            );
        }
        assert_eq!(
            d.count("rust_withdrawal_intents"),
            i64::from(log.is_none()),
            "{body}"
        );
        let wire = t
            .requests()
            .into_iter()
            .find(|r| r.path == "/0/private/Withdraw")
            .unwrap();
        assert_eq!(
            wire.body.unwrap(),
            json!({"asset":"XBT","key":"cold","address":"destination","amount":"0.99","max_fee":"0.0099"})
        );
        // Reopen the durable database and reconstruct the production adapter.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restart.sqlite");
        d.c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        let restarted = Db {
            c: Connection::open(path).unwrap(),
            q: Connection::open_in_memory().unwrap(),
            cipher: d.cipher,
        };
        let a = kraken_adapter(&restarted, &t);
        for _ in 0..2 {
            w::recover(
                &restarted.c,
                &restarted.cipher,
                &a,
                "rule:1",
                &FixedClock(now()),
            )
            .await
            .unwrap();
            w::dispatch(&restarted.c, &restarted.cipher, &a, &i, &FixedClock(now()))
                .await
                .unwrap();
        }
        assert_eq!(kraken_sends(&t), 1, "{body}");
        assert_eq!(
            rules::status(&restarted.c, 1).unwrap().is_some(),
            log.is_none()
        );
    }
}
#[tokio::test]
async fn kraken_timeout_is_unresolved_and_never_retried() {
    let d = kraken_db();
    let t = kraken_reads();
    t.network(K_SEND, "post_send", "timeout");
    let i = kraken_prepared(&d, &t).await;
    let a = kraken_adapter(&d, &t);
    w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.sqlite");
    d.c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let d = Db {
        c: Connection::open(path).unwrap(),
        q: Connection::open_in_memory().unwrap(),
        cipher: d.cipher,
    };
    let a = kraken_adapter(&d, &t);
    w::recover(
        &d.c,
        &d.cipher,
        &kraken_adapter(&d, &t),
        "rule:1",
        &FixedClock(now()),
    )
    .await
    .unwrap();
    w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(kraken_sends(&t), 1);
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(rules::status(&d.c, 1).unwrap().unwrap().state, "unresolved");
}
fn k_candidate(id: &str, offset: i64, key: &str, amount: &str, status: &str, prop: &str) -> Value {
    json!({"refid":id,"time":now().timestamp()+offset,"key":key,"amount":amount,"status":status,"status-prop":prop,"method":"Bitcoin","asset":"XXBT"})
}
#[tokio::test]
async fn kraken_paged_candidates_use_inclusive_window_and_never_settle() {
    let d = kraken_db();
    let reads = kraken_reads();
    let i = kraken_prepared(&d, &reads).await;
    let a = kraken_adapter(&d, &reads);
    assert!(
        w::mark_dispatched(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
            .await
            .unwrap()
    );
    let t = ScriptedTransport::default();
    t.reply(
        K_STATUS,
        200,
        json!({"error":[],"result":{"withdrawals":[
        k_candidate("early",-61,"cold","0.99","Success",""),
        k_candidate("start",-60,"cold","0.9900","Success",""),
        k_candidate("other",0,"other","0.99","Success",""),
        k_candidate("amount",0,"cold","0.98","Success","")],"cursor":"page-2"}}),
    );
    t.reply(
        K_STATUS,
        200,
        json!({"error":[],"result":{"withdrawals":[
        k_candidate("canceled",0,"cold","0.99","Pending","canceled"),
        k_candidate("return",1,"cold","0.99","Success","return"),
        k_candidate("review",2,"cold","0.99","Pending","onhold"),
        k_candidate("cancel-pending",3,"cold","0.99","Pending","cancel-pending"),
        k_candidate("initial",4,"cold","0.99","Initial",""),
        k_candidate("settled",5,"cold","0.99","Settled",""),
        k_candidate("end",3600,"cold","0.99","Failure",""),
        k_candidate("late",3601,"cold","0.99","Success","")],"cursor":null}}),
    );
    w::recover(
        &d.c,
        &d.cipher,
        &kraken_adapter(&d, &t),
        "rule:1",
        &FixedClock(now()),
    )
    .await
    .unwrap();
    let s = rules::status(&d.c, 1).unwrap().unwrap();
    assert_eq!(s.state, "unresolved");
    assert!(s.reason.unwrap().contains("resolve-withdrawal"));
    assert_eq!(
        s.candidates
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec![
            "start",
            "canceled",
            "return",
            "review",
            "cancel-pending",
            "initial",
            "settled",
            "end"
        ]
    );
    assert_eq!(
        s.candidates[1]["status"],
        "Pending (canceled; looks failed)"
    );
    assert_eq!(s.candidates[2]["status"], "Success (return; looks failed)");
    assert_eq!(s.candidates[3]["status"], "Pending (onhold; in review)");
    assert_eq!(
        s.candidates[4]["status"],
        "Pending (cancel-pending; in review)"
    );
    assert_eq!(s.candidates[5]["status"], "Initial");
    assert_eq!(s.candidates[6]["status"], "Settled");
    assert_eq!(s.candidates[7]["status"], "Failure (looks failed)");
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(d.count("rust_withdrawal_ids"), 0);
    let requests = t.requests();
    assert_eq!(requests.len(), 2);
    for (r, cursor) in requests.iter().zip(["true", "page-2"]) {
        assert_eq!(
            r.body.as_ref().unwrap(),
            &json!({"asset":"XBT","method":"Bitcoin","start":(now().timestamp()-60).to_string(),"end":(now().timestamp()+3600).to_string(),"cursor":cursor})
        );
    }
    assert_eq!(kraken_sends(&t), 0);
}
#[tokio::test]
async fn kraken_x0_classifier_cells_through_real_adapter() {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    let cells: Vec<_> = vectors["classifier"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["venue"] == "kraken" && c["replies"][K_SEND].is_array())
        .collect();
    assert_eq!(
        cells.len(),
        1 + 1 + 1 + 1 + 1,
        "Kraken X0: max fee + internal + service + HTML + timeout"
    );
    let d = kraken_db();
    let i = kraken_prepared(&d, &kraken_reads()).await;
    for c in cells {
        let reply = &c["replies"][K_SEND][0];
        let t = ScriptedTransport::default();
        if reply["network"].is_string() {
            t.network(K_SEND, "post_send", "timeout");
        } else {
            t.reply(
                K_SEND,
                reply["status"].as_u64().unwrap_or(200) as u16,
                reply["body"].clone(),
            );
        }
        let result = kraken_adapter(&d, &t).send(&i.request).await;
        let definitive = c["case"] == "kraken_max_fee_exceeded";
        let message = c["rule_logs"][0]["message"].as_str().unwrap();
        assert_eq!(rules::ambiguous("Kraken", message), !definitive);
        if let w::SendResult::Rejected { reason, .. } = &result {
            assert_eq!(reason, &reply["body"].to_string());
            assert!(rules::definitive("Kraken", reason));
            let body: Value = serde_json::from_str(reason).unwrap();
            assert_eq!(message, format!("Withdrawal failed: {}", body["error"][0].as_str().unwrap()));
        }
        assert_eq!(
            matches!(
                result,
                w::SendResult::Rejected {
                    definitive: true,
                    ..
                }
            ),
            definitive,
            "{}",
            c["case"]
        );
        assert_eq!(
            matches!(result, w::SendResult::Ambiguous { .. }),
            !definitive,
            "{}",
            c["case"]
        );
    }
}
#[tokio::test]
async fn kraken_quote_over_budget_or_threshold_refuses_before_send() {
    for threshold in [false, true] {
        let d = kraken_db();
        let t = kraken_reads();
        if threshold {
            d.c.execute("UPDATE rules SET settings=json_set(settings,'$.threshold_type','fee_percentage','$.max_fee_percentage','0.1','$.max_interval',NULL)",[]).unwrap();
        } else {
            // First quote consumes .01 of the budget; a .02 re-quote exceeds the remainder.
            // Replace the script to supply the two precise answers.
            let t = ScriptedTransport::from_script(&json!({
                "POST /0/private/BalanceEx":[{"status":200,"body":{"error":[],"result":{"XBT":{"balance":"1","hold_trade":"0"}}}}],
                "POST /0/private/WithdrawAddresses":[{"status":200,"body":{"error":[],"result":[{"address":"destination","key":"cold","method":"Bitcoin","verified":true}]}}],
                "POST /0/private/WithdrawInfo":[{"status":200,"body":{"error":[],"result":{"amount":"0.99","fee":"0.01"}}},{"status":200,"body":{"error":[],"result":{"amount":"0.97","fee":"0.02"}}}]
            }));
            assert!(w::evaluate(
                &d.c,
                &d.cipher,
                &kraken_adapter(&d, &t),
                d.eval(),
                &FixedClock(now())
            )
            .await
            .unwrap()
            .is_none());
            assert_eq!(kraken_sends(&t), 0);
            continue;
        }
        assert!(w::evaluate(
            &d.c,
            &d.cipher,
            &kraken_adapter(&d, &t),
            d.eval(),
            &FixedClock(now())
        )
        .await
        .unwrap()
        .is_none());
        assert_eq!(kraken_sends(&t), 0);
    }
}
/// D-RULE-{KRAKEN,BINANCE,MEXC,GEMINI}-NO-LIVE-FIXTURE: the four venues Rails withdraws on (less Binance.US,
/// D-RULE-BINANCE-US-UNSUPPORTED) send, each from its offline G0 evidence, with the fee semantics unknown.
#[test]
fn l_venues_capability_is_enabled_with_unknown_fee_semantics() {
    let on = ["Exchanges::Kraken", "Exchanges::Binance", "Exchanges::Mexc", "Exchanges::Gemini"];
    for row in &deltabadger::venue::registry::ROWS {
        let enabled = on.contains(&row.exchange_type);
        assert_eq!(row.withdraws, enabled, "{}", row.exchange_type);
        assert_eq!(row.withdrawals.is_some(), enabled, "{}", row.exchange_type);
        assert_eq!(row.fee_on_top, None, "{}", row.exchange_type);
        assert_eq!(w::G0_ADAPTERS.contains(&row.exchange_type), enabled, "{}", row.exchange_type);
    }
    assert!(deltabadger::venue::registry::row("Exchanges::BinanceUs").unwrap().withdrawals.is_none());
}

struct ScriptedKrakenFactory(ScriptedTransport);
impl deltabadger::venue::VenueFactory for ScriptedKrakenFactory {
    type V = KrakenVenue<ScriptedTransport>;
    fn for_bot(&self, exchange: &str, credentials: Option<deltabadger::crypto::Credentials>) -> Self::V {
        assert_eq!(exchange, "Exchanges::Kraken");
        KrakenVenue::new(self.0.clone()).with_identity(&credentials.unwrap())
    }
}

async fn kraken_engine_threshold(below: bool) {
    use deltabadger::{engine::run, lease, store::{self, Paths}};
    let dir = common::rails_install();
    let paths = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&paths, now()).unwrap();
    let opened = store::open(&paths).unwrap();
    let cipher = common::seed::cipher();
    let seed = common::seed::seed_kraken(&opened.primary, &cipher);
    opened.primary.execute("UPDATE api_keys SET key_type=1 WHERE id=?1", [seed.api_key_id]).unwrap();
    opened.primary.execute("INSERT INTO exchange_assets(exchange_id,asset_id,created_at,updated_at) VALUES(?1,?2,?3,?3)", params![seed.exchange_id,seed.btc,now().to_rfc3339()]).unwrap();
    let settings = json!({"address_name":"cold","withdrawal_percentage":"100","threshold_type":"min_amount","min_amount":if below {"0.1"} else {"0.01"}});
    opened.primary.execute("INSERT INTO rules(id,user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES(1,?1,'Rules::Withdrawal',1,?2,?3,?4,?5,?6,?6)", params![seed.user_id,seed.exchange_id,seed.btc,cipher.encrypt("destination"),settings.to_string(),now().to_rfc3339()]).unwrap();
    opened.primary.execute("INSERT INTO rust_rule_evaluations(rule_id,source,created_at) VALUES(1,'start:l-kraken',?1)", [now().to_rfc3339()]).unwrap();
    let t = if below {
        ScriptedTransport::from_script(&json!({
            "POST /0/private/BalanceEx":[{"status":200,"body":{"error":[],"result":{"XBT":{"balance":"0.05","hold_trade":"0"}}}}],
            "POST /0/private/WithdrawAddresses":[{"status":200,"body":{"error":[],"result":[{"address":"destination","key":"cold","method":"Bitcoin","verified":true}]}}],
            "POST /0/private/WithdrawInfo":[{"status":200,"body":{"error":[],"result":{"amount":"0.04","fee":"0.01"}}}]
        }))
    } else {
        kraken_reads()
    };
    t.set_now(now());
    t.reply(K_SEND, 200, json!({"error":[],"result":{"refid":"K-live-gate"}}));
    let mut engine = run::Engine::new(opened.primary, ScriptedKrakenFactory(t.clone()), cipher, lock);
    for _ in 0..2 {
        run::step(&mut engine, &FixedClock(now())).await.unwrap();
    }
    let c = &engine.primary;
    let count = |table: &str| c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_,i64>(0)).unwrap();
    assert_eq!(count("rule_logs"), 1);
    assert_eq!(count("rust_withdrawal_intents"), 0);
    assert_eq!(count("rust_rule_evaluations WHERE consumed_at IS NULL"), 0);
    assert!(rules::status(c, 1).unwrap().is_none());
    let (status, message, details): (i64, String, String) = c.query_row("SELECT status,message,details FROM rule_logs", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    let details: Value = serde_json::from_str(&details).unwrap();
    if below {
        assert_eq!(kraken_sends(&t), 0);
        assert_eq!(status, 0);
        let vectors: Value = serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
        let oracle = &vectors["execute"].as_array().unwrap().iter().find(|cell| cell["case"] == "below_minimum_min_mode").unwrap()["rule_logs"][0];
        assert_eq!(oracle["status"], "pending");
        assert_eq!(message, oracle["message"]);
        assert_eq!(details, oracle["details"]);
        assert_eq!(count("rust_withdrawal_ids"), 0);
    } else {
        assert_eq!(kraken_sends(&t), 1);
        assert_eq!(status, 1);
        assert_eq!(details["withdrawal_id"], "K-live-gate");
        assert_eq!(count("rust_withdrawal_ids"), 1);
        let requests = t.requests();
        let sent = requests.iter().find(|r| r.path == "/0/private/Withdraw").unwrap().body.as_ref().unwrap();
        // D-RULE-KRAKEN-NO-LIVE-FIXTURE: None reserves the first fee and re-quotes the reduced amount.
        assert_eq!(sent["amount"], "0.99");
        assert_eq!(sent["max_fee"], "0.0099");
        assert!(&n(sent["amount"].as_str().unwrap()) + &n(sent["max_fee"].as_str().unwrap()) <= n("1"));
        let quotes: Vec<_> = requests.iter().filter(|r| r.path == "/0/private/WithdrawInfo").map(|r| r.body.as_ref().unwrap()["amount"].clone()).collect();
        assert_eq!(quotes, vec![json!("1.0"), json!("0.99")]);
    }
}

#[tokio::test]
async fn l_kraken_engine_above_threshold_sends_once_and_reserves_fee() {
    kraken_engine_threshold(false).await;
}

#[tokio::test]
async fn l_kraken_engine_below_threshold_logs_without_sending() {
    kraken_engine_threshold(true).await;
}

/// The engine's factory for one venue in the L validators: the real adapter on ScriptedTransport.
struct ScriptedFactory<F>(&'static str, F);
impl<V: Venue, F: Fn(deltabadger::crypto::Credentials) -> V> deltabadger::venue::VenueFactory for ScriptedFactory<F> {
    type V = V;
    fn for_bot(&self, exchange: &str, credentials: Option<deltabadger::crypto::Credentials>) -> V {
        assert_eq!(exchange, self.0);
        (self.1)(credentials.unwrap())
    }
}
fn btc_pairs() -> deltabadger::venue::binance::Pairs {
    deltabadger::venue::binance::Pairs { assets: [("BTC".into(), "BTC".into())].into(), ..Default::default() }
}

/// L-Binance, L-MEXC, L-Gemini: an active rule runs through `run::step` with the real adapter. Above the threshold it
/// sends once, reserving the fee (fee_on_top None); below it, Rails' pending row and nothing sent.
async fn l_engine_threshold<V: Venue>(class: &'static str, below: bool, factory: impl Fn(&ScriptedTransport, deltabadger::crypto::Credentials) -> V) {
    use deltabadger::{engine::run, lease, store::{self, Paths}};
    let (name, network, send, accepted, venue_id) = match class {
        "Exchanges::Binance" => ("Binance", "BTC", B_SEND, json!({"id":"L-binance"}), "L-binance"),
        "Exchanges::Mexc" => ("MEXC", "Bitcoin", M_SEND, json!({"id":"L-mexc"}), "L-mexc"),
        _ => ("Gemini", "bitcoin", G_SEND, json!({"withdrawalId":"L-gemini"}), "L-gemini"),
    };
    let dir = common::rails_install();
    let paths = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&paths, now()).unwrap();
    let opened = store::open(&paths).unwrap();
    let cipher = common::seed::cipher();
    let seed = common::seed::seed_kraken(&opened.primary, &cipher);
    opened.primary.execute("UPDATE exchanges SET type=?1,name=?2 WHERE id=?3", params![class, name, seed.exchange_id]).unwrap();
    opened.primary.execute("UPDATE tickers SET ticker='BTCEUR',base='BTC' WHERE id=?1", [seed.ticker_id]).unwrap();
    opened.primary.execute("UPDATE api_keys SET key_type=1 WHERE id=?1", [seed.api_key_id]).unwrap();
    opened.primary.execute("INSERT INTO exchange_assets(exchange_id,asset_id,created_at,updated_at) VALUES(?1,?2,?3,?3)", params![seed.exchange_id,seed.btc,now().to_rfc3339()]).unwrap();
    let settings = json!({"network":network,"withdrawal_percentage":"100","threshold_type":"min_amount","min_amount":if below {"0.1"} else {"0.01"}});
    opened.primary.execute("INSERT INTO rules(id,user_id,type,status,exchange_id,asset_id,address,settings,created_at,updated_at) VALUES(1,?1,'Rules::Withdrawal',1,?2,?3,?4,?5,?6,?6)", params![seed.user_id,seed.exchange_id,seed.btc,cipher.encrypt("destination"),settings.to_string(),now().to_rfc3339()]).unwrap();
    opened.primary.execute("INSERT INTO rust_rule_evaluations(rule_id,source,created_at) VALUES(1,'start:l-venue',?1)", [now().to_rfc3339()]).unwrap();
    let free = if below { "0.05" } else { "1" };
    let t = ScriptedTransport::default();
    if class == "Exchanges::Gemini" {
        t.set_now(now() + chrono::Duration::seconds(3600));
        t.reply("POST /v1/balances", 200, json!([{"currency":"BTC","type":"exchange","amount":free,"available":free,"availableForWithdrawal":free}]));
        t.reply("POST /v1/approvedAddresses/account/bitcoin", 200, json!({"approvedAddresses":[{"address":"destination","network":"bitcoin","status":"active"}]}));
        t.reply("POST /v2/withdraw/bitcoin/btc/feeEstimate", 200, json!({"currency":"BTC","fee":"0.01"}));
        t.reply("POST /v2/withdraw/bitcoin/btc/feeEstimate", 200, json!({"currency":"BTC","fee":"0.0099"}));
        t.reply(G_STATUS, 200, json!([]));
    } else {
        let mexc = class == "Exchanges::Mexc";
        t.set_now(now());
        t.reply("GET /api/v3/account", 200, json!({"balances":[{"asset":"BTC","free":free,"locked":"0"}]}));
        t.reply(if mexc { "GET /api/v3/capital/config/getall" } else { "GET /sapi/v1/capital/config/getall" }, 200,
            json!([{"coin":"BTC","networkList":[{"network":network,"netWork":"BTC","withdrawEnable":true,"withdrawTag":false,"withdrawFee":"0.01","withdrawIntegerMultiple":"0.00000001","isDefault":true}]}]));
        t.reply(if mexc { "GET /api/v3/capital/withdraw/address" } else { "GET /sapi/v1/capital/withdraw/address/list" }, 200,
            if mexc { json!({"data":[{"coin":"BTC","network":"BTC","address":"destination"}],"page":1,"totalPageNum":1,"totalRecords":1}) }
            else { json!([{"coin":"BTC","network":"BTC","address":"destination"}]) });
        t.reply(e1_history(mexc), 200, json!([]));
    }
    t.reply(send, 200, accepted);
    let sends = |t: &ScriptedTransport| t.requests().iter().filter(|r| format!("{} {}", r.method, r.path) == send).count();
    let scripted = t.clone();
    let mut engine = run::Engine::new(opened.primary, ScriptedFactory(class, move |cr| factory(&scripted, cr)), cipher, lock);
    for _ in 0..2 {
        run::step(&mut engine, &FixedClock(now())).await.unwrap();
    }
    let c = &engine.primary;
    let count = |table: &str| c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_,i64>(0)).unwrap();
    assert_eq!(count("rule_logs"), 1, "{class}");
    assert_eq!(count("rust_withdrawal_intents"), 0, "{class}");
    assert_eq!(count("rust_rule_evaluations WHERE consumed_at IS NULL"), 0, "{class}");
    assert!(rules::status(c, 1).unwrap().is_none(), "{class}");
    let (status, message, details): (i64, String, String) = c.query_row("SELECT status,message,details FROM rule_logs", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
    let details: Value = serde_json::from_str(&details).unwrap();
    if below {
        assert_eq!(sends(&t), 0, "{class}");
        assert_eq!(status, 0, "{class}");
        let vectors: Value = serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
        let oracle = &vectors["execute"].as_array().unwrap().iter().find(|cell| cell["case"] == "below_minimum_min_mode").unwrap()["rule_logs"][0];
        assert_eq!(oracle["status"], "pending");
        assert_eq!(message, oracle["message"], "{class}");
        assert_eq!(details, oracle["details"], "{class}");
        assert_eq!(count("rust_withdrawal_ids"), 0, "{class}");
    } else {
        assert_eq!(sends(&t), 1, "{class}");
        assert_eq!(status, 1, "{class}: {message}");
        assert_eq!(details["withdrawal_id"], venue_id);
        assert_eq!(count("rust_withdrawal_ids"), 1, "{class}");
        let requests = t.requests();
        let sent = requests.iter().find(|r| format!("{} {}", r.method, r.path) == send).unwrap();
        let field = |k: &str| -> String {
            sent.body.as_ref().and_then(|b| b[k].as_str().map(str::to_string))
                .or_else(|| sent.query.iter().find(|(q, _)| *q == k).map(|(_, v)| v.clone()))
                .unwrap_or_else(|| panic!("{class}: no {k} in {sent:?}"))
        };
        // D-RULE-<VENUE>-NO-LIVE-FIXTURE: fee_on_top None reserves the fee, so amount + fee stays within the budget.
        let amount = n(&field("amount"));
        assert_eq!(amount, n("0.99"), "{class}");
        let fee = if class == "Exchanges::Gemini" { n("0.0099") } else { n("0.01") };
        assert!(&amount + &fee <= n("1"), "{class}");
    }
}

fn l_binance(t: &ScriptedTransport, cr: deltabadger::crypto::Credentials) -> BinanceVenue<ScriptedTransport> {
    BinanceVenue::new(t.clone(), deltabadger::venue::binance::URL, &cr.secret).with_identity(&cr).with_pairs(btc_pairs())
}
fn l_mexc(t: &ScriptedTransport, cr: deltabadger::crypto::Credentials) -> MexcVenue<ScriptedTransport> {
    MexcVenue::new(t.clone(), deltabadger::venue::mexc::URL, &cr.secret).with_identity(&cr).with_pairs(btc_pairs())
}
fn l_gemini(t: &ScriptedTransport, cr: deltabadger::crypto::Credentials) -> GeminiVenue<ScriptedTransport> {
    GeminiVenue::new(t.clone(), deltabadger::venue::gemini::URL).with_pairs(btc_pairs()).with_identity(&cr)
}

#[tokio::test]
async fn l_binance_engine_above_threshold_sends_once_and_reserves_fee() {
    l_engine_threshold("Exchanges::Binance", false, l_binance).await;
}
#[tokio::test]
async fn l_binance_engine_below_threshold_logs_without_sending() {
    l_engine_threshold("Exchanges::Binance", true, l_binance).await;
}
#[tokio::test]
async fn l_mexc_engine_above_threshold_sends_once_and_reserves_fee() {
    l_engine_threshold("Exchanges::Mexc", false, l_mexc).await;
}
#[tokio::test]
async fn l_mexc_engine_below_threshold_logs_without_sending() {
    l_engine_threshold("Exchanges::Mexc", true, l_mexc).await;
}
#[tokio::test]
async fn l_gemini_engine_above_threshold_sends_once_and_reserves_fee() {
    l_engine_threshold("Exchanges::Gemini", false, l_gemini).await;
}
#[tokio::test]
async fn l_gemini_engine_below_threshold_logs_without_sending() {
    l_engine_threshold("Exchanges::Gemini", true, l_gemini).await;
}

#[tokio::test]
async fn kraken_kill_worker() {
    let Ok(path) = std::env::var("E2_CRASH_DB") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let d = reopen(&path);
    let t = kraken_reads();
    t.reply(
        K_SEND,
        200,
        json!({"error":[],"result":{"refid":"K-crash"}}),
    );
    let i = kraken_prepared(&d, &t).await;
    let a = kraken_adapter(&d, &t);
    assert!(
        w::mark_dispatched(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
            .await
            .unwrap()
    );
    assert!(matches!(
        a.send(&i.request).await,
        w::SendResult::Accepted { .. }
    ));
    std::fs::write(path.with_extension("sends"), kraken_sends(&t).to_string()).unwrap();
    std::process::exit(74); // No settlement or destructors: the reply was lost.
}
#[tokio::test]
async fn kraken_kill_after_dispatch_real_adapter_sends_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.sqlite");
    let seed = kraken_db();
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
        .args(["--exact", "kraken_kill_worker", "--nocapture"])
        .env("E2_CRASH_DB", &path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(74),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let d = reopen(&path);
    let t = kraken_reads();
    let a = kraken_adapter(&d, &t);
    let i = w::load(&d.c, &d.cipher, "rule:1").unwrap().unwrap();
    assert_eq!(i.state, "dispatched");
    for _ in 0..2 {
        w::recover(&d.c, &d.cipher, &a, "rule:1", &FixedClock(now()))
            .await
            .unwrap();
        w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
            .await
            .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(path.with_extension("sends")).unwrap(),
        "1"
    );
    assert_eq!(kraken_sends(&t), 0);
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(rules::status(&d.c, 1).unwrap().unwrap().state, "unresolved");
}
#[tokio::test]
async fn kraken_lookup_malformed_permission_and_cursor_cycles_stay_unknown() {
    let d = kraken_db();
    let i = kraken_prepared(&d, &kraken_reads()).await;
    for body in [
        json!({"error":["EGeneral:Permission denied"]}),
        json!({"error":[],"result":{}}),
        json!({"error":[],"result":{"withdrawals":[{"key":"cold","amount":"bad"}],"cursor":null}}),
        json!({"error":[],"result":{"withdrawals":[],"cursor":42}}),
        json!({"error":[],"result":{"withdrawals":[],"cursor":"loop"}}),
    ] {
        let t = ScriptedTransport::default();
        t.reply(K_STATUS, 200, body);
        assert!(matches!(
            kraken_adapter(&d, &t).lookup(&i.request).await,
            w::LookupResult::Unknown { .. }
        ));
        assert!(t.requests().len() <= 2);
    }
}

#[tokio::test]
async fn kraken_method_is_immutable_and_an_entry_without_one_is_selected() {
    let d = kraken_db();
    let t = kraken_reads();
    let i = kraken_prepared(&d, &t).await;
    assert!(
        matches!(&w::load(&d.c,&d.cipher,"rule:1").unwrap().unwrap().request.destination,v::Destination::Kraken{method:Some(method),..} if method=="Bitcoin")
    );
    let changed = ScriptedTransport::default();
    changed.reply("POST /0/private/WithdrawAddresses",200,json!({"error":[],"result":[{"address":"destination","key":"cold","method":"Lightning","verified":true}]}));
    w::dispatch(
        &d.c,
        &d.cipher,
        &kraken_adapter(&d, &changed),
        &i,
        &FixedClock(now()),
    )
    .await
    .unwrap();
    assert_eq!(kraken_sends(&changed), 0);
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    let entries = v::kraken_entries(
        &json!({"error":[],"result":[{"address":"destination","key":"cold","verified":true}]}),
    )
    .unwrap();
    // Rails lists an entry without a method too (kraken.rb:477); it is selected with no method to filter the lookup by.
    assert!(matches!(v::select(
        v::Kind::Kraken,
        "XBT",
        &v::Stored::new("destination", None, None, Some("cold")),
        &entries,
        None
    ), Ok(v::Destination::Kraken { method: None, .. })));
}

// R0: a recorder owns one scenario, never a synthetic rule or rule evaluation.
fn recorder_spec() -> w::RecorderSpec {
    w::RecorderSpec {
        scenario: "external-1".into(),
        user_id: 1,
        exchange_id: 1,
        asset_id: 1,
        max_amount: "0.001".into(),
        destination: v::Stored::new("destination", Some("BTC"), None, None),
    }
}
fn recorder_lock() -> (tempfile::TempDir, deltabadger::lease::EngineLock) {
    let dir = tempfile::tempdir().unwrap();
    let paths = deltabadger::store::Paths::from_env(&|_| None, dir.path());
    let lock = deltabadger::lease::lock(&paths, now()).unwrap();
    (dir, lock)
}
#[tokio::test]
async fn recorder_refuses_ceiling_and_unlisted_destination_before_intent() {
    let d = Db::new();
    let f = Fake::new(&d);
    let (_dir, lock) = recorder_lock();
    for (amount, address) in [
        ("0.00100001", "destination"),
        ("0", "destination"),
        ("0.001", "elsewhere"),
    ] {
        let mut spec = recorder_spec();
        spec.max_amount = amount.into();
        spec.destination.address = address.into();
        assert!(
            w::begin_recorder(&lock, &d.c, &d.cipher, &f, &spec, &FixedClock(now()))
                .await
                .is_err()
        );
        assert_eq!(d.count("rust_withdrawal_intents"), 0);
        assert_eq!(f.sends.get(), 0);
    }
}
#[tokio::test]
async fn recorder_consumption_binding_and_no_rule_effects() {
    let d = Db::new();
    let f = Fake::new(&d);
    let (_dir, lock) = recorder_lock();
    d.c.execute("CREATE TABLE bots(id INTEGER PRIMARY KEY)", [])
        .unwrap();
    let eligibility_before = deltabadger::engine::eligibility::check_install(&d.c)
        .unwrap()
        .problems;
    let before = deltabadger::venue::registry::row("Exchanges::Binance")
        .unwrap()
        .withdraws;
    let spec = recorder_spec();
    let i = w::begin_recorder(&lock, &d.c, &d.cipher, &f, &spec, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(i.owner, "recorder:external-1");
    assert_eq!(i.request.amount, "0.0009");
    let mut other = recorder_spec();
    other.max_amount = "0.0008".into();
    assert!(format!(
        "{:?}",
        w::begin_recorder(&lock, &d.c, &d.cipher, &f, &other, &FixedClock(now())).await
    )
    .contains("different binding"));
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 1);
    assert_eq!(d.count("rules"), 1);
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(d.count("rust_rule_evaluations"), 0);
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert_eq!(
        d.c.query_row("SELECT outcome FROM rust_recorder_runs", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "sent"
    );
    assert!(format!(
        "{:?}",
        w::begin_recorder(&lock, &d.c, &d.cipher, &f, &spec, &FixedClock(now())).await
    )
    .contains("consumed"));
    assert_eq!(
        before,
        deltabadger::venue::registry::row("Exchanges::Binance")
            .unwrap()
            .withdraws
    );
    assert_eq!(
        eligibility_before,
        deltabadger::engine::eligibility::check_install(&d.c)
            .unwrap()
            .problems
    );
    // A venue that does not withdraw still never sends from the rule's evaluation after the recorder ran.
    d.c.execute("UPDATE exchanges SET type='Exchanges::BinanceUs'", []).unwrap();
    d.eval();
    w::phase(
        &d.c,
        &d.cipher,
        &deltabadger::venue::fake::FakeFactory::default(),
        &FixedClock(now()),
    )
    .await
    .unwrap();
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
}
#[tokio::test]
async fn recorder_dispatch_crash_recovery_and_operator_resolution() {
    for sent in [true, false] {
        let d = Db::new();
        let f = Fake::new(&d);
        let (_dir, lock) = recorder_lock();
        let i = w::begin_recorder(
            &lock,
            &d.c,
            &d.cipher,
            &f,
            &recorder_spec(),
            &FixedClock(now()),
        )
        .await
        .unwrap();
        assert!(
            w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                .await
                .unwrap()
        );
        // Restart at the committed dispatch boundary: the fresh adapter must never send.
        let restarted = Fake::new(&d);
        w::recover(&d.c, &d.cipher, &restarted, &i.owner, &FixedClock(now()))
            .await
            .unwrap();
        w::dispatch(&d.c, &d.cipher, &restarted, &i, &FixedClock(now()))
            .await
            .unwrap();
        assert_eq!(restarted.sends.get(), 0);
        assert_eq!(
            w::load(&d.c, &d.cipher, &i.owner).unwrap().unwrap().state,
            "unresolved"
        );
        assert_eq!(
            d.c.query_row("SELECT outcome FROM rust_recorder_runs", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "unresolved"
        );
        let args: Vec<String> = if sent {
            vec!["--recorder", "external-1", "--sent", "operator-id"]
        } else {
            vec!["--recorder", "external-1", "--not-sent"]
        }
        .into_iter()
        .map(str::to_owned)
        .collect();
        let (target, resolution) = w::parse_cli(&args).unwrap();
        w::resolve(&d.c, &d.q, &d.cipher, target, resolution, now()).unwrap();
        assert_eq!(d.count("rust_withdrawal_intents"), 0);
        assert_eq!(d.count("rule_logs"), 0);
        assert!(w::begin_recorder(
            &lock,
            &d.c,
            &d.cipher,
            &f,
            &recorder_spec(),
            &FixedClock(now())
        )
        .await
        .is_err());
    }
}

#[tokio::test]
async fn recorder_rotated_credentials_and_changed_listing_cannot_dispatch() {
    for changed in ["key", "listing", "expired"] {
        let d = Db::new();
        let mut f = Fake::new(&d);
        let (_dir, lock) = recorder_lock();
        let i = w::begin_recorder(
            &lock,
            &d.c,
            &d.cipher,
            &f,
            &recorder_spec(),
            &FixedClock(now()),
        )
        .await
        .unwrap();
        if changed == "key" {
            d.c.execute(
                "UPDATE api_keys SET secret=?1",
                [d.cipher.encrypt("different")],
            )
            .unwrap();
        }
        if changed == "listing" {
            f.network = "OTHER";
        }
        let at = if changed == "expired" {
            now() + chrono::Duration::seconds(w::SEND_WINDOW_SECONDS + 1)
        } else {
            now()
        };
        w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(at))
            .await
            .unwrap();
        assert_eq!(f.sends.get(), 0);
        assert_eq!(d.count("rule_logs"), 0);
        assert_eq!(d.count("rust_withdrawal_intents"), 0);
        assert_eq!(
            d.c.query_row("SELECT outcome FROM rust_recorder_runs", [], |r| r
                .get::<_, String>(0))
                .unwrap(),
            "not_sent"
        );
    }
}
#[tokio::test]
async fn recorder_fences_every_read_and_consumes_prepared_crash() {
    for read in 1..=5 {
        let d = Db::new();
        let f = Fake::new(&d);
        let (_dir, lock) = recorder_lock();
        f.rotate_at.set(read);
        assert!(w::begin_recorder(
            &lock,
            &d.c,
            &d.cipher,
            &f,
            &recorder_spec(),
            &FixedClock(now())
        )
        .await
        .is_err());
        assert_eq!(f.sends.get(), 0);
        assert_eq!(d.count("rust_withdrawal_intents"), 0);
    }
    let d = Db::new();
    let f = Fake::new(&d);
    let (_dir, lock) = recorder_lock();
    let i = w::begin_recorder(
        &lock,
        &d.c,
        &d.cipher,
        &f,
        &recorder_spec(),
        &FixedClock(now()),
    )
    .await
    .unwrap();
    w::recover(&d.c, &d.cipher, &f, &i.owner, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert!(w::begin_recorder(
        &lock,
        &d.c,
        &d.cipher,
        &f,
        &recorder_spec(),
        &FixedClock(now())
    )
    .await
    .is_err());
    assert_eq!(d.count("rule_logs"), 0);
}
#[tokio::test]
async fn recorder_ambiguous_reply_keeps_intent_and_resolver_seeds_rails_ids() {
    let d = Db::new();
    let f = Fake::new(&d);
    let (_dir, lock) = recorder_lock();
    *f.answer.borrow_mut() = w::SendResult::Accepted { id: "".into() };
    let i = w::begin_recorder(
        &lock,
        &d.c,
        &d.cipher,
        &f,
        &recorder_spec(),
        &FixedClock(now()),
    )
    .await
    .unwrap();
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(f.sends.get(), 1);
    assert_eq!(
        w::load(&d.c, &d.cipher, &i.owner).unwrap().unwrap().state,
        "unresolved"
    );
    d.log("Withdrew 1 BTC", Some("historical-id"));
    assert!(w::resolve(
        &d.c,
        &d.q,
        &d.cipher,
        w::Target::Recorder("external-1".into()),
        w::Resolution::Sent {
            id: "historical-id".into(),
            amount: None
        },
        now()
    )
    .is_err());
    assert_eq!(d.count("rust_withdrawal_ids"), 1);
    assert_eq!(d.count("rust_withdrawal_intents"), 1);
    assert_eq!(d.count("rule_logs"), 1);
}
#[tokio::test]
async fn recorder_different_account_or_destination_refused_even_before_consumption() {
    let d = Db::new();
    let f = Fake::new(&d);
    let (_dir, lock) = recorder_lock();
    let mut unlisted = recorder_spec();
    unlisted.destination.address = "unlisted".into();
    assert!(
        w::begin_recorder(&lock, &d.c, &d.cipher, &f, &unlisted, &FixedClock(now()))
            .await
            .is_err()
    );
    let reads = f.read.get();
    assert!(format!(
        "{:?}",
        w::begin_recorder(
            &lock,
            &d.c,
            &d.cipher,
            &f,
            &recorder_spec(),
            &FixedClock(now())
        )
        .await
    )
    .contains("different binding"));
    let mut changed_asset = unlisted;
    changed_asset.asset_id = 2;
    assert!(format!(
        "{:?}",
        w::begin_recorder(
            &lock,
            &d.c,
            &d.cipher,
            &f,
            &changed_asset,
            &FixedClock(now())
        )
        .await
    )
    .contains("different binding"));
    assert_eq!(f.read.get(), reads);
}

#[test]
fn recorder_process_child() {
    let Ok(path) = std::env::var("R0_CRASH_DB") else {
        return;
    };
    let dir = std::path::Path::new(&path).parent().unwrap();
    let paths = deltabadger::store::Paths::from_env(&|_| None, dir);
    let lock = deltabadger::lease::lock(&paths, now()).unwrap();
    let mut d = Db::new();
    d.c = Connection::open(&path).unwrap();
    let mut f = Fake::new(&d);
    f.counter = Some(dir.join("sends"));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let i = w::begin_recorder(
                &lock,
                &d.c,
                &d.cipher,
                &f,
                &recorder_spec(),
                &FixedClock(now()),
            )
            .await
            .unwrap();
            assert!(
                w::mark_dispatched(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
                    .await
                    .unwrap()
            );
            if std::env::var("R0_AFTER_SEND").as_deref() == Ok("1") {
                let _ = w::Adapter::send(&f, &i.request).await;
            }
            std::fs::write(dir.join("ready"), "committed").unwrap();
            loop {
                std::thread::park();
            }
        });
}
#[tokio::test]
async fn recorder_killed_process_reopens_durable_intent_without_resending() {
    for after_send in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("production.sqlite3");
        let mut d = Db::new();
        d.c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "recorder_process_child", "--nocapture"])
            .env("R0_CRASH_DB", &path)
            .env("R0_AFTER_SEND", if after_send { "1" } else { "0" })
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !dir.path().join("ready").exists() {
            if std::time::Instant::now() > deadline || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("recorder did not reach dispatch boundary");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        d.c = Connection::open(&path).unwrap();
        let paths = deltabadger::store::Paths::from_env(&|_| None, dir.path());
        let lock = deltabadger::lease::lock(&paths, now()).unwrap();
        let mut restarted = Fake::new(&d);
        restarted.counter = Some(dir.path().join("sends"));
        w::recover(
            &d.c,
            &d.cipher,
            &restarted,
            "recorder:external-1",
            &FixedClock(now()),
        )
        .await
        .unwrap();
        assert_eq!(
            w::load(&d.c, &d.cipher, "recorder:external-1")
                .unwrap()
                .unwrap()
                .state,
            "unresolved"
        );
        assert!(w::begin_recorder(
            &lock,
            &d.c,
            &d.cipher,
            &restarted,
            &recorder_spec(),
            &FixedClock(now())
        )
        .await
        .is_err());
        assert_eq!(
            std::fs::read(dir.path().join("sends"))
                .unwrap_or_default()
                .len(),
            usize::from(after_send)
        );
        assert_eq!(d.count("rule_logs"), 0);
    }
}

// E1: real Binance/MEXC adapters, signed requests and exact-id recovery; no network.
use deltabadger::venue::{binance::BinanceVenue, mexc::MexcVenue, Venue};
const B_SEND: &str = "POST /sapi/v1/capital/withdraw/apply";
const M_SEND: &str = "POST /api/v3/capital/withdraw";
const B_HISTORY: &str = "GET /sapi/v1/capital/withdraw/history";
const M_HISTORY: &str = "GET /api/v3/capital/withdraw/history";
const DAY_MS: i64 = 86_400_000;
fn e1_request(mexc: bool) -> w::Request {
    w::Request {
        rule_id: 1,
        user_id: 1,
        exchange_id: 1,
        asset_id: 1,
        exchange: if mexc {
            "Exchanges::Mexc"
        } else {
            "Exchanges::Binance"
        }
        .into(),
        symbol: "BTC".into(),
        coin: "BTC".into(),
        client_id: "our-client-id".into(),
        destination: if mexc {
            v::Destination::Mexc {
                coin: "BTC".into(),
                network: "Bitcoin".into(),
                net_work: "BTC".into(),
                address: "destination".into(),
                memo: Some("memo 1".into()),
            }
        } else {
            v::Destination::Binance {
                coin: "BTC".into(),
                network: "BTC".into(),
                address: "destination".into(),
                tag: Some("tag 1".into()),
            }
        },
        amount: "0.99".into(),
        fee_cap: "0.01".into(),
        free_balance: "1".into(),
        percentage: "100".into(),
        created_at: now().to_rfc3339(),
        wallet_type: (!mexc).then_some(0),
    }
}
async fn e1_send(mexc: bool, t: &ScriptedTransport, r: &w::Request) -> w::SendResult {
    if mexc {
        MexcVenue::new(t.clone(), deltabadger::venue::mexc::URL, "secret")
            .withdraw(r)
            .await
    } else {
        BinanceVenue::new(t.clone(), deltabadger::venue::binance::URL, "secret")
            .withdraw(r)
            .await
    }
}
async fn e1_lookup(mexc: bool, t: &ScriptedTransport, r: &w::Request) -> w::LookupResult {
    if mexc {
        MexcVenue::new(t.clone(), deltabadger::venue::mexc::URL, "secret")
            .withdrawal_lookup(r)
            .await
    } else {
        BinanceVenue::new(t.clone(), deltabadger::venue::binance::URL, "secret")
            .withdrawal_lookup(r)
            .await
    }
}
fn e1_history(mexc: bool) -> &'static str {
    if mexc {
        M_HISTORY
    } else {
        B_HISTORY
    }
}
fn e1_send_path(mexc: bool) -> &'static str {
    if mexc {
        M_SEND
    } else {
        B_SEND
    }
}
fn e1_row(mexc: bool, status: i64) -> Value {
    let mut row = json!({"id":"venue-id","withdrawOrderId":"our-client-id","coin":"BTC","address":"destination","amount":"0.99000000","network":if mexc {"Bitcoin"} else {"BTC"},"status":status,"info":"rejected by venue"});
    if mexc {
        row["memo"] = json!("memo 1");
    } else {
        row["walletType"] = json!(0);
        row["addressTag"] = json!("tag 1");
    }
    row
}
fn e1_query(
    r: &deltabadger::venue::http::HttpRequest,
) -> std::collections::BTreeMap<&'static str, String> {
    r.query.iter().map(|(k, v)| (*k, v.clone())).collect()
}
#[tokio::test]
async fn e1_signed_fields_and_absolute_send_bound() {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    for mexc in [false, true] {
        let t = ScriptedTransport::default();
        t.set_now(now());
        t.reply(e1_send_path(mexc), 200, json!({"id":"venue-id"}));
        let r = e1_request(mexc);
        assert!(
            matches!(e1_send(mexc, &t, &r).await, w::SendResult::Accepted { id } if id == "venue-id")
        );
        let requests = t.requests();
        let wire = &requests[0];
        let mut actual = e1_query(wire);
        let signature = actual.remove("signature").unwrap();
        let mut expected = std::collections::BTreeMap::from([
            ("coin", "BTC".into()),
            ("address", "destination".into()),
            ("amount", "0.99".into()),
            ("withdrawOrderId", "our-client-id".into()),
            ("recvWindow", "5000".into()),
            ("timestamp", now().timestamp_millis().to_string()),
        ]);
        if mexc {
            expected.insert("netWork", "BTC".into());
            expected.insert("memo", "memo 1".into());
        } else {
            expected.insert("network", "BTC".into());
            expected.insert("addressTag", "tag 1".into());
            expected.insert("walletType", "0".into());
        }
        assert_eq!(
            actual, expected,
            "D-RULE-CLIENT-ID, D-RULE-WALLET-SPOT, D-RULE-MEXC-CAPITAL, D-RULE-MEXC-ADDRESSTAG"
        );
        let encoded = actual
            .iter()
            .map(|(k, v)| format!("{k}={}", v.replace(' ', "+")))
            .collect::<Vec<_>>()
            .join("&");
        let mut mac = Hmac::<Sha256>::new_from_slice(b"secret").unwrap();
        mac.update(encoded.as_bytes());
        assert_eq!(signature, hex::encode(mac.finalize().into_bytes()));
        assert!(wire.body.is_none());
        assert_eq!(
            wire.not_after,
            Some(
                now()
                    + chrono::Duration::seconds(w::SEND_WINDOW_SECONDS)
                    + chrono::Duration::from_std(deltabadger::venue::http::TOTAL_TIMEOUT).unwrap()
            )
        );
        let t = ScriptedTransport::default();
        t.network(e1_send_path(mexc), "pre_send", "expired");
        assert!(matches!(
            e1_send(mexc, &t, &r).await,
            w::SendResult::NotSent { .. }
        ));
        let mut invalid = r;
        invalid.created_at = "bad".into();
        let t = ScriptedTransport::default();
        assert!(matches!(
            e1_send(mexc, &t, &invalid).await,
            w::SendResult::NotSent { .. }
        ));
        assert!(t.requests().is_empty());
    }
}
#[tokio::test]
async fn e1_only_clean_success_or_allowlisted_rejection_settles() {
    for mexc in [false, true] {
        let (code, message) = if mexc {
            (10101, "Insufficient balance")
        } else {
            (-4026, "You have insufficient balance.")
        };
        for (status, body, expected) in [
            (200, json!({"id":"ok"}), "accepted"),
            (400, json!({"code":code,"msg":message}), "rejected"),
            (200, json!({"code":code,"msg":message}), "rejected"),
            (500, json!({"code":code,"msg":message}), "ambiguous"),
            (400, json!({"code":999999,"msg":message}), "ambiguous"),
            (
                400,
                json!({"code":code,"msg":"unknown failure"}),
                "ambiguous",
            ),
            (400, json!({"msg":message}), "ambiguous"),
            (
                200,
                json!({"id":"ok","code":code,"msg":message}),
                "ambiguous",
            ),
            (200, json!({"id":"ok","success":false}), "ambiguous"),
            (
                400,
                json!({"code":code,"msg":message,"error":"unknown failure"}),
                "ambiguous",
            ),
            (400, json!({"id":"ok"}), "ambiguous"),
            (200, json!({}), "ambiguous"),
            (200, json!({"id":" "}), "ambiguous"),
            (200, json!({"id":123}), "ambiguous"),
            (502, json!("<html>gateway</html>"), "ambiguous"),
        ] {
            let t = ScriptedTransport::default();
            t.reply(e1_send_path(mexc), status, body.clone());
            let result = e1_send(mexc, &t, &e1_request(mexc)).await;
            let actual = match result {
                w::SendResult::Accepted { .. } => "accepted",
                w::SendResult::Rejected {
                    definitive: true, ..
                } => "rejected",
                _ => "ambiguous",
            };
            assert_eq!(actual, expected, "mexc={mexc}, {status}, {body}");
        }
        for kind in ["post_send", "permanent"] {
            let t = ScriptedTransport::default();
            t.network(e1_send_path(mexc), kind, "lost signed URL");
            assert!(matches!(
                e1_send(mexc, &t, &e1_request(mexc)).await,
                w::SendResult::Ambiguous { .. }
            ));
        }
    }
}
#[tokio::test]
async fn e1_exact_lookup_statuses_and_parameter_fences() {
    for mexc in [false, true] {
        for status in -1..=11 {
            let t = ScriptedTransport::default();
            t.set_now(now());
            t.reply(e1_history(mexc), 200, json!([e1_row(mexc, status)]));
            let result = e1_lookup(mexc, &t, &e1_request(mexc)).await;
            let sent = if mexc {
                matches!(status, 1..=7 | 10)
            } else {
                matches!(status, 0 | 2 | 4 | 6)
            };
            let failed = if mexc {
                matches!(status, 8 | 9)
            } else {
                matches!(status, 1 | 3 | 5)
            };
            assert_eq!(
                matches!(result, w::LookupResult::Found { ref id, status: w::FoundStatus::Sent } if id == "venue-id"),
                sent,
                "{mexc}, {status}"
            );
            assert_eq!(
                matches!(
                    result,
                    w::LookupResult::Found {
                        status: w::FoundStatus::Failed { .. },
                        ..
                    }
                ),
                failed,
                "{mexc}, {status}"
            );
            assert_eq!(
                matches!(result, w::LookupResult::Unknown { .. }),
                !sent && !failed
            );
        }
        for key in [
            "id",
            "coin",
            "address",
            "amount",
            "network",
            if mexc { "memo" } else { "walletType" },
        ] {
            let mut row = e1_row(mexc, 6);
            row[key] = if key == "id" {
                json!("")
            } else {
                json!("wrong")
            };
            let t = ScriptedTransport::default();
            t.set_now(now());
            t.reply(e1_history(mexc), 200, json!([row]));
            assert!(
                matches!(
                    e1_lookup(mexc, &t, &e1_request(mexc)).await,
                    w::LookupResult::Unknown { .. }
                ),
                "{mexc}, {key}"
            );
        }
        let mut unrelated = e1_row(mexc, 6);
        unrelated["withdrawOrderId"] = json!("someone-else");
        let t = ScriptedTransport::default();
        t.set_now(now());
        t.reply(e1_history(mexc), 200, json!([unrelated]));
        assert!(matches!(
            e1_lookup(mexc, &t, &e1_request(mexc)).await,
            w::LookupResult::NotFound
        ));
        for rows in [
            json!([]),
            json!([e1_row(mexc, 6), e1_row(mexc, 6)]),
            json!([e1_row(mexc, 6), {}]),
        ] {
            let empty = rows.as_array().unwrap().is_empty();
            let t = ScriptedTransport::default();
            t.set_now(now());
            t.reply(e1_history(mexc), 200, rows);
            let result = e1_lookup(mexc, &t, &e1_request(mexc)).await;
            assert_eq!(matches!(result, w::LookupResult::NotFound), empty);
            assert_eq!(matches!(result, w::LookupResult::Unknown { .. }), !empty);
        }
        for (status, body) in [
            (
                403,
                json!({"code":700007,"msg":"No permission to access the endpoint"}),
            ),
            (200, json!({"code":700007})),
            (200, json!("garbage")),
        ] {
            let t = ScriptedTransport::default();
            t.set_now(now());
            t.reply(e1_history(mexc), status, body);
            assert!(matches!(
                e1_lookup(mexc, &t, &e1_request(mexc)).await,
                w::LookupResult::Unknown { .. }
            ));
        }
        // §3 permits history to omit network. When present it must equal the canonical label, never MEXC netWork.
        let mut row = e1_row(mexc, 6);
        row.as_object_mut().unwrap().remove("network");
        let t = ScriptedTransport::default();
        t.set_now(now());
        t.reply(e1_history(mexc), 200, json!([row]));
        assert!(matches!(
            e1_lookup(mexc, &t, &e1_request(mexc)).await,
            w::LookupResult::Found { .. }
        ));
        if mexc {
            let mut row = e1_row(true, 6);
            row["network"] = json!("BTC");
            let t = ScriptedTransport::default();
            t.set_now(now());
            t.reply(M_HISTORY, 200, json!([row]));
            assert!(matches!(
                e1_lookup(true, &t, &e1_request(true)).await,
                w::LookupResult::Unknown { .. }
            ));
        }
    }
}
#[tokio::test]
async fn e1_binance_seven_day_boundary_and_exact_query() {
    let start = now() - chrono::Duration::seconds(60);
    for offset in [60_000, 7 * DAY_MS - 1, 7 * DAY_MS] {
        let t = ScriptedTransport::default();
        t.set_now(start + chrono::Duration::milliseconds(offset));
        t.reply(B_HISTORY, 200, json!([e1_row(false, 6)]));
        let result = e1_lookup(false, &t, &e1_request(false)).await;
        if offset == 7 * DAY_MS {
            assert!(matches!(result, w::LookupResult::Unknown { .. }));
            assert!(t.requests().is_empty());
        } else {
            assert!(matches!(result, w::LookupResult::Found { .. }));
            let mut q = e1_query(&t.requests()[0]);
            q.remove("signature");
            q.remove("timestamp");
            assert_eq!(
                q,
                std::collections::BTreeMap::from([
                    ("withdrawOrderId", "our-client-id".into()),
                    ("startTime", start.timestamp_millis().to_string()),
                    ("endTime", (start.timestamp_millis() + offset).to_string()),
                    ("recvWindow", "5000".into())
                ])
            );
        }
    }
}
#[tokio::test]
async fn e1_mexc_halves_full_windows_and_finishes_before_settling() {
    let start = now().timestamp_millis() - 60_000;
    let end = start + 7 * DAY_MS + 1;
    let t = ScriptedTransport::default();
    t.set_now(chrono::DateTime::from_timestamp_millis(end).unwrap());
    t.reply(M_HISTORY, 200, json!(vec![e1_row(true, 6); 1000]));
    t.reply(M_HISTORY, 200, json!([e1_row(true, 6)]));
    t.reply(M_HISTORY, 200, json!([]));
    t.reply(M_HISTORY, 200, json!([]));
    assert!(matches!(
        e1_lookup(true, &t, &e1_request(true)).await,
        w::LookupResult::Found { .. }
    ));
    let requests = t.requests();
    assert_eq!(
        requests.len(),
        1 + 2 + 1,
        "full window + two halves + remaining window"
    );
    let ranges: Vec<_> = requests
        .iter()
        .map(|r| {
            let q = e1_query(r);
            assert!(!q.contains_key("walletType"));
            assert_eq!(q["coin"], "BTC");
            assert_eq!(q["limit"], "1000");
            (
                q["startTime"].parse::<i64>().unwrap(),
                q["endTime"].parse::<i64>().unwrap(),
            )
        })
        .collect();
    assert_eq!(ranges[0], (start, start + 7 * DAY_MS - 1));
    assert_eq!(ranges[1].0, start);
    assert_eq!(ranges[1].1 + 1, ranges[2].0);
    assert_eq!(ranges[2].1, ranges[0].1);
    assert_eq!(ranges[3], (ranges[0].1 + 1, end));
    // A second matching record in a later window prevents settlement, even after an earlier exact match.
    let t = ScriptedTransport::default();
    t.set_now(chrono::DateTime::from_timestamp_millis(end).unwrap());
    t.reply(M_HISTORY, 200, json!([e1_row(true, 6)]));
    assert!(matches!(
        e1_lookup(true, &t, &e1_request(true)).await,
        w::LookupResult::Unknown { .. }
    ));
    let t = ScriptedTransport::default();
    t.set_now(now());
    t.reply(M_HISTORY, 200, json!(vec![e1_row(true, 6); 1000]));
    assert!(matches!(
        e1_lookup(true, &t, &e1_request(true)).await,
        w::LookupResult::Unknown { .. }
    ));
    assert!(
        t.requests().len() < 100,
        "a saturated millisecond cannot be halved"
    );
}
#[tokio::test]
async fn e1_mexc_ninety_day_limit_never_proves_absence() {
    let start = now() - chrono::Duration::seconds(60);
    for offset in [90 * DAY_MS - 1, 90 * DAY_MS, 91 * DAY_MS] {
        let t = ScriptedTransport::default();
        t.set_now(start + chrono::Duration::milliseconds(offset));
        t.reply(M_HISTORY, 200, json!([]));
        let result = e1_lookup(true, &t, &e1_request(true)).await;
        if offset < 90 * DAY_MS {
            assert!(matches!(result, w::LookupResult::NotFound));
            assert_eq!(t.requests().len(), 12 + 1, "twelve full weeks + remainder");
        } else {
            assert!(matches!(result, w::LookupResult::Unknown { .. }));
            assert!(t.requests().is_empty());
        }
    }
}
#[tokio::test]
async fn e1_x0_classifier_cells_through_real_adapters() {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    for mexc in [false, true] {
        let venue = if mexc { "mexc" } else { "binance" };
        let old_path = if mexc {
            "POST /api/v3/capital/withdraw/apply"
        } else {
            B_SEND
        };
        let cells: Vec<_> = vectors["classifier"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["venue"] == venue && c["replies"][old_path].is_array())
            .collect();
        assert_eq!(cells.len(), if mexc { 1 + 1 + 1 + 1 } else { 3 + 1 + 1 + 1 + 1 + 1 + 1 }, "X0: MEXC rejection + internal + raw + timeout; Binance three rejections + tranId + missing msg + empty + HTML + timeout + unreadable");
        for cell in cells {
            let reply = &cell["replies"][old_path][0];
            let t = ScriptedTransport::default();
            if reply["network"].is_string() {
                t.network(e1_send_path(mexc), "post_send", "timeout");
            } else {
                t.reply(
                    e1_send_path(mexc),
                    reply["status"].as_u64().unwrap_or(200) as u16,
                    reply["body"].clone(),
                );
            }
            let result = e1_send(mexc, &t, &e1_request(mexc)).await;
            let message = cell["rule_logs"][0]["message"].as_str().unwrap();
            let definitive = !rules::ambiguous(if mexc { "Mexc" } else { "Binance" }, message);
            assert_eq!(
                matches!(
                    result,
                    w::SendResult::Rejected {
                        definitive: true,
                        ..
                    }
                ),
                definitive,
                "{}",
                cell["case"]
            );
            assert_eq!(
                matches!(result, w::SendResult::Ambiguous { .. }),
                !definitive,
                "{}",
                cell["case"]
            );
            if let w::SendResult::Rejected { reason, .. } = result {
                assert_eq!(message, format!("Withdrawal failed: {reason}"));
            }
        }
    }
}
fn e1_db(mexc: bool) -> Db {
    let d = Db::new();
    if mexc {
        d.c.execute_batch("UPDATE exchanges SET type='Exchanges::Mexc',name='MEXC'; UPDATE rules SET settings=json_set(settings,'$.network','Bitcoin');").unwrap();
    }
    d
}
fn e1_reads(mexc: bool) -> ScriptedTransport {
    let t = ScriptedTransport::default();
    t.set_now(now());
    t.reply(
        "GET /api/v3/account",
        200,
        json!({"balances":[{"asset":"BTC","free":"1","locked":"0"}]}),
    );
    let network = if mexc { "Bitcoin" } else { "BTC" };
    t.reply(if mexc { "GET /api/v3/capital/config/getall" } else { "GET /sapi/v1/capital/config/getall" }, 200,
        json!([{"coin":"BTC","networkList":[{"network":network,"netWork":"BTC","withdrawEnable":true,"withdrawTag":false,"withdrawFee":"0.01","withdrawIntegerMultiple":"0.00000001","isDefault":true}]}]));
    t.reply(if mexc { "GET /api/v3/capital/withdraw/address" } else { "GET /sapi/v1/capital/withdraw/address/list" }, 200,
        if mexc { json!({"data":[{"coin":"BTC","network":"BTC","address":"destination"}],"page":1,"totalPageNum":1,"totalRecords":1}) }
        else { json!([{"coin":"BTC","network":"BTC","address":"destination"}]) });
    t.reply(e1_history(mexc), 200, json!([]));
    t
}
fn e1_binance_adapter(
    d: &Db,
    t: &ScriptedTransport,
) -> w::VenueAdapter<BinanceVenue<ScriptedTransport>> {
    let (cr, version) = deltabadger::engine::model::withdrawal_credentials(&d.c, &d.cipher, 1, 1)
        .unwrap()
        .unwrap();
    w::VenueAdapter::new(
        BinanceVenue::new(t.clone(), deltabadger::venue::binance::URL, "secret")
            .with_identity(&cr)
            .with_pairs(deltabadger::venue::binance::Pairs {
                assets: [("BTC".into(), "BTC".into())].into(),
                ..Default::default()
            }),
        version,
        None,
    )
}
fn e1_mexc_adapter(d: &Db, t: &ScriptedTransport) -> w::VenueAdapter<MexcVenue<ScriptedTransport>> {
    let (cr, version) = deltabadger::engine::model::withdrawal_credentials(&d.c, &d.cipher, 1, 1)
        .unwrap()
        .unwrap();
    w::VenueAdapter::new(
        MexcVenue::new(t.clone(), deltabadger::venue::mexc::URL, "secret")
            .with_identity(&cr)
            .with_pairs(deltabadger::venue::binance::Pairs {
                assets: [("BTC".into(), "BTC".into())].into(),
                ..Default::default()
            }),
        version,
        None,
    )
}
fn e1_sends(mexc: bool, t: &ScriptedTransport) -> usize {
    t.requests()
        .iter()
        .filter(|r| format!("{} {}", r.method, r.path) == e1_send_path(mexc))
        .count()
}
async fn e1_g0<V: Venue>(
    mexc: bool,
    factory: impl Fn(&Db, &ScriptedTransport) -> w::VenueAdapter<V>,
) {
    for outcome in ["accepted", "rejected", "ambiguous", "timeout"] {
        let d = e1_db(mexc);
        let t = e1_reads(mexc);
        let a = factory(&d, &t);
        let i = w::evaluate(&d.c, &d.cipher, &a, d.eval(), &FixedClock(now()))
            .await
            .unwrap()
            .unwrap();
        match outcome {
            "accepted" => {
                t.reply(e1_send_path(mexc), 200, json!({"id":"venue-id"}));
            }
            "rejected" => {
                t.reply(
                    e1_send_path(mexc),
                    400,
                    if mexc {
                        json!({"code":10101,"msg":"Insufficient balance"})
                    } else {
                        json!({"code":-4026,"msg":"You have insufficient balance."})
                    },
                );
            }
            "ambiguous" => {
                t.reply(e1_send_path(mexc), 200, json!({}));
            }
            _ => {
                t.network(e1_send_path(mexc), "post_send", "timeout");
            }
        }
        let mut edited = i.clone();
        edited.request.amount = "99".into();
        w::dispatch(&d.c, &d.cipher, &a, &edited, &FixedClock(now()))
            .await
            .unwrap();
        let wire = t
            .requests()
            .into_iter()
            .find(|r| r.method == "POST")
            .unwrap();
        assert_eq!(
            e1_query(&wire)["amount"],
            i.request.amount,
            "immutable persisted amount"
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restart.sqlite");
        d.c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        let d = Db {
            c: Connection::open(path).unwrap(),
            q: Connection::open_in_memory().unwrap(),
            cipher: d.cipher,
        };
        let a = factory(&d, &t);
        for _ in 0..2 {
            w::recover(&d.c, &d.cipher, &a, "rule:1", &FixedClock(now()))
                .await
                .unwrap();
            w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
                .await
                .unwrap();
        }
        let unresolved = matches!(outcome, "ambiguous" | "timeout");
        assert_eq!(e1_sends(mexc, &t), 1);
        assert_eq!(d.count("rule_logs"), i64::from(!unresolved));
        assert_eq!(d.count("rust_withdrawal_intents"), i64::from(unresolved));
        assert_eq!(rules::status(&d.c, 1).unwrap().is_some(), unresolved);
        if !unresolved {
            let status: i64 =
                d.c.query_row("SELECT status FROM rule_logs", [], |r| r.get(0))
                    .unwrap();
            assert_eq!(status, if outcome == "accepted" { 1 } else { 2 });
        }
    }
}
#[tokio::test]
async fn e1_g0_real_adapters_restart_without_resends() {
    e1_g0(false, e1_binance_adapter).await;
    e1_g0(true, e1_mexc_adapter).await;
}
async fn e1_kill_send(d: &Db, t: &ScriptedTransport, a: &impl w::Adapter) {
    let i = w::evaluate(&d.c, &d.cipher, a, d.eval(), &FixedClock(now()))
        .await
        .unwrap()
        .unwrap();
    assert!(
        w::mark_dispatched(&d.c, &d.cipher, a, &i, &FixedClock(now()))
            .await
            .unwrap()
    );
    assert!(matches!(
        a.send(&i.request).await,
        w::SendResult::Accepted { .. }
    ));
    assert_eq!(
        t.requests().iter().filter(|r| r.method == "POST").count(),
        1
    );
}
#[tokio::test]
async fn e1_kill_worker() {
    let Ok(path) = std::env::var("E1_CRASH_DB") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let d = reopen(&path);
    let mexc = std::env::var("E1_CRASH_MEXC").unwrap() == "true";
    let t = e1_reads(mexc);
    t.reply(e1_send_path(mexc), 200, json!({"id":"venue-id"}));
    if mexc {
        e1_kill_send(&d, &t, &e1_mexc_adapter(&d, &t)).await;
    } else {
        e1_kill_send(&d, &t, &e1_binance_adapter(&d, &t)).await;
    }
    std::fs::write(path.with_extension("sends"), e1_sends(mexc, &t).to_string()).unwrap();
    std::process::exit(74);
}
async fn e1_after_kill(d: &Db, a: &impl w::Adapter, i: &w::Intent) {
    for _ in 0..2 {
        w::recover(&d.c, &d.cipher, a, "rule:1", &FixedClock(now()))
            .await
            .unwrap();
        w::dispatch(&d.c, &d.cipher, a, i, &FixedClock(now()))
            .await
            .unwrap();
    }
}
#[tokio::test]
async fn e1_kill_after_dispatch_real_adapters_send_once_and_exact_lookup_settles() {
    for mexc in [false, true] {
        for status in [None, Some(6), Some(if mexc { 8 } else { 3 })] {
            let found = status.is_some();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("primary.sqlite");
            let d = e1_db(mexc);
            d.c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
                .unwrap();
            d.q.execute(
                "VACUUM INTO ?1",
                [path.with_extension("queue").to_str().unwrap()],
            )
            .unwrap();
            drop(d);
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "e1_kill_worker", "--nocapture"])
                .env("E1_CRASH_DB", &path)
                .env("E1_CRASH_MEXC", mexc.to_string())
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(74),
                "{} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let d = reopen(&path);
            let t = ScriptedTransport::default();
            t.set_now(now());
            let i = w::load(&d.c, &d.cipher, "rule:1").unwrap().unwrap();
            assert_eq!(i.state, "dispatched");
            let mut row = e1_row(mexc, status.unwrap_or(6));
            row["withdrawOrderId"] = json!(i.request.client_id);
            row["amount"] = json!(i.request.amount);
            row.as_object_mut()
                .unwrap()
                .remove(if mexc { "memo" } else { "addressTag" });
            t.reply(
                e1_history(mexc),
                200,
                if found { json!([row]) } else { json!([]) },
            );
            if mexc {
                e1_after_kill(&d, &e1_mexc_adapter(&d, &t), &i).await;
            } else {
                e1_after_kill(&d, &e1_binance_adapter(&d, &t), &i).await;
            }
            assert_eq!(
                std::fs::read_to_string(path.with_extension("sends")).unwrap(),
                "1"
            );
            assert_eq!(e1_sends(mexc, &t), 0);
            assert_eq!(d.count("rule_logs"), i64::from(found));
            assert_eq!(d.count("rust_withdrawal_intents"), i64::from(!found));
            assert_eq!(rules::status(&d.c, 1).unwrap().is_some(), !found);
            if let Some(status) = status {
                let actual: i64 =
                    d.c.query_row("SELECT status FROM rule_logs", [], |r| r.get(0))
                        .unwrap();
                assert_eq!(actual, if status == 6 { 1 } else { 2 });
            }
        }
    }
}

#[tokio::test]
async fn e1_missing_destination_network_and_wrong_wallet_are_never_sent() {
    for mexc in [false, true] {
        let mut r = e1_request(mexc);
        match &mut r.destination {
            v::Destination::Binance { network, .. } | v::Destination::Mexc { network, .. } => {
                network.clear()
            }
            _ => unreachable!(),
        }
        let t = ScriptedTransport::default();
        t.set_now(now());
        assert!(matches!(
            e1_send(mexc, &t, &r).await,
            w::SendResult::NotSent { .. }
        ));
        assert!(matches!(
            e1_lookup(mexc, &t, &r).await,
            w::LookupResult::Unknown { .. }
        ));
        assert!(t.requests().is_empty());
    }
    let t = ScriptedTransport::default();
    let mut r = e1_request(false);
    r.wallet_type = Some(1);
    assert!(matches!(
        e1_send(false, &t, &r).await,
        w::SendResult::NotSent { .. }
    ));
    assert!(t.requests().is_empty(), "D-RULE-WALLET-SPOT");
}

#[tokio::test]
async fn e1_partial_or_malformed_history_cannot_settle_an_earlier_match() {
    for mexc in [false, true] {
        for key in ["id", "amount", "status", "coin", "address"] {
            let mut malformed = e1_row(mexc, 6);
            malformed.as_object_mut().unwrap().remove(key);
            malformed["withdrawOrderId"] = json!("unrelated");
            let t = ScriptedTransport::default();
            t.set_now(now());
            t.reply(e1_history(mexc), 200, json!([e1_row(mexc, 6), malformed]));
            assert!(
                matches!(
                    e1_lookup(mexc, &t, &e1_request(mexc)).await,
                    w::LookupResult::Unknown { .. }
                ),
                "{mexc}, {key}"
            );
        }
    }
    let t = ScriptedTransport::default();
    t.set_now(now() + chrono::Duration::days(8));
    t.reply(M_HISTORY, 200, json!([e1_row(true, 6)]));
    t.reply(
        M_HISTORY,
        403,
        json!({"code":700007,"msg":"No permission to access the endpoint"}),
    );
    assert!(matches!(
        e1_lookup(true, &t, &e1_request(true)).await,
        w::LookupResult::Unknown { .. }
    ));
    assert_eq!(t.requests().len(), 1 + 1, "exact match then failed window");
}

// E3: the production VenueAdapter and GeminiVenue, with offline HTTP answers.
use deltabadger::venue::gemini::GeminiVenue;
const G_SEND: &str = "POST /v2/withdraw/bitcoin/btc";
const G_STATUS: &str = "POST /v2/transfers";
fn gemini_db() -> Db {
    let d = Db::new();
    d.c.execute_batch("UPDATE exchanges SET type='Exchanges::Gemini',name='Gemini'; UPDATE rules SET settings=json_set(settings,'$.network','bitcoin');").unwrap();
    d
}
fn gemini_adapter(
    d: &Db,
    t: &ScriptedTransport,
) -> w::VenueAdapter<GeminiVenue<ScriptedTransport>> {
    let (cr, version) = deltabadger::engine::model::withdrawal_credentials(&d.c, &d.cipher, 1, 1)
        .unwrap()
        .unwrap();
    w::VenueAdapter::new(
        GeminiVenue::new(t.clone(), deltabadger::venue::gemini::URL)
            .with_pairs(deltabadger::venue::binance::Pairs { assets: [("BTC".into(), "BTC".into())].into(), ..Default::default() })
            .with_identity(&cr),
        version,
        None,
    )
}
fn gemini_reads() -> ScriptedTransport {
    let t = ScriptedTransport::default();
    t.set_now(now() + chrono::Duration::seconds(3600));
    t.reply("POST /v1/balances", 200, json!([{"currency":"BTC","type":"exchange","amount":"1","available":"1","availableForWithdrawal":"1"}]));
    t.reply("POST /v1/approvedAddresses/account/bitcoin", 200, json!({"approvedAddresses":[{"address":"destination","network":"bitcoin","status":"active"}]}));
    t.reply("POST /v2/withdraw/bitcoin/btc/feeEstimate", 200, json!({"currency":"BTC","fee":"0.01"}));
    t.reply("POST /v2/withdraw/bitcoin/btc/feeEstimate", 200, json!({"currency":"BTC","fee":"0.0099"}));
    t.reply(G_STATUS, 200, json!([]));
    t
}
async fn gemini_prepared(d: &Db, t: &ScriptedTransport) -> w::Intent {
    w::evaluate(
        &d.c,
        &d.cipher,
        &gemini_adapter(d, t),
        d.eval(),
        &FixedClock(now()),
    )
    .await
    .unwrap()
    .unwrap()
}
fn gemini_sends(t: &ScriptedTransport) -> usize {
    t.requests()
        .iter()
        .filter(|r| r.path == "/v2/withdraw/bitcoin/btc")
        .count()
}
#[tokio::test(start_paused = true)]
async fn gemini_g0_answers_restart_without_resends() {
    let answers = [
        (200, json!({"withdrawalId":"G-1"}), Some(1)),
        (400, json!({"result":"error","reason":"InsufficientFunds"}), Some(2)),
        (500, json!({"result":"error","reason":"System"}), None),
        (500, json!({"result":"error","reason":"InsufficientFunds"}), None),
        (200, json!({"result":"error","reason":"Maintenance"}), None),
        (400, json!({"result":"error","reason":"RateLimit"}), None),
        (400, json!({"result":"error","reason":"NewReason"}), None),
        (200, json!("not JSON"), None),
        (200, json!({}), None),
        (200, json!({"withdrawalId":" "}), None),
        (400, json!({"withdrawalId":"G-1"}), None),
        (200, json!({"reason":"InsufficientFunds"}), None),
        (200, json!({"withdrawalId":"G-1","result":"error","reason":"InsufficientFunds"}), None),
        (200, json!({"withdrawalId":"G-1","reason":"System"}), None),
    ];
    for (http, body, log) in answers {
        let d = gemini_db();
        let t = gemini_reads();
        t.reply(G_SEND, http, body.clone());
        let i = gemini_prepared(&d, &t).await;
        let a = gemini_adapter(&d, &t);
        // A caller changing its in-memory copy must not change the signed request.
        let mut edited = i.clone();
        edited.request.amount = "99".into();
        edited.request.fee_cap = "50".into();
        w::dispatch(&d.c, &d.cipher, &a, &edited, &FixedClock(now()))
            .await
            .unwrap();
        assert_eq!(d.count("rule_logs"), i64::from(log.is_some()), "{body}");
        if let Some(expected) = log {
            assert_eq!(
                d.c.query_row("SELECT status FROM rule_logs", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                expected,
                "{body}"
            );
        }
        assert_eq!(
            d.count("rust_withdrawal_intents"),
            i64::from(log.is_none()),
            "{body}"
        );
        let wire = t
            .requests()
            .into_iter()
            .find(|r| r.path == "/v2/withdraw/bitcoin/btc")
            .unwrap();
        assert_eq!(
            wire.body.unwrap(),
            json!({"address":"destination","amount":"0.99","clientTransferId":i.request.client_id})
        );
        // Reopen the durable database and reconstruct the production adapter.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restart.sqlite");
        d.c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        let restarted = Db {
            c: Connection::open(path).unwrap(),
            q: Connection::open_in_memory().unwrap(),
            cipher: d.cipher,
        };
        let a = gemini_adapter(&restarted, &t);
        for _ in 0..2 {
            w::recover(
                &restarted.c,
                &restarted.cipher,
                &a,
                "rule:1",
                &FixedClock(now()),
            )
            .await
            .unwrap();
            w::dispatch(&restarted.c, &restarted.cipher, &a, &i, &FixedClock(now()))
                .await
                .unwrap();
        }
        assert_eq!(gemini_sends(&t), 1, "{body}");
        assert_eq!(
            rules::status(&restarted.c, 1).unwrap().is_some(),
            log.is_none()
        );
    }
}
#[tokio::test(start_paused = true)]
async fn gemini_timeout_is_unresolved_and_never_retried() {
    let d = gemini_db();
    let t = gemini_reads();
    t.network(G_SEND, "post_send", "timeout");
    let i = gemini_prepared(&d, &t).await;
    let a = gemini_adapter(&d, &t);
    w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.sqlite");
    d.c.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let d = Db {
        c: Connection::open(path).unwrap(),
        q: Connection::open_in_memory().unwrap(),
        cipher: d.cipher,
    };
    let a = gemini_adapter(&d, &t);
    w::recover(
        &d.c,
        &d.cipher,
        &gemini_adapter(&d, &t),
        "rule:1",
        &FixedClock(now()),
    )
    .await
    .unwrap();
    w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(gemini_sends(&t), 1);
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(rules::status(&d.c, 1).unwrap().unwrap().state, "unresolved");
}
fn g_candidate(id: &str, offset_ms: i64, address: &str, amount: &str, status: &str) -> Value {
    json!({"withdrawalId":id,"eid":id,"timestampms":now().timestamp_millis()+offset_ms,"destination":address,"amount":amount,"status":status,"currency":"BTC","type":"Withdrawal","network":"bitcoin"})
}
#[tokio::test(start_paused = true)]
async fn gemini_candidates_use_window_and_never_settle() {
    let d = gemini_db();
    let i = gemini_prepared(&d, &gemini_reads()).await;
    let t = ScriptedTransport::default();
    t.set_now(now() + chrono::Duration::seconds(3600));
    let mut deposit = g_candidate("deposit", 1, "destination", "0.99", "Complete");
    deposit["type"] = json!("Deposit");
    let mut other_coin = g_candidate("coin", 1, "destination", "0.99", "Complete");
    other_coin["currency"] = json!("ETH");
    t.reply(G_STATUS, 200, json!([
        g_candidate("early", -1, "destination", "0.99", "Complete"),
        g_candidate("start", 0, "destination", "0.9900", "Complete"),
        g_candidate("other", 1, "other", "0.99", "Complete"),
        g_candidate("amount", 1, "destination", "0.98", "Complete"),
        g_candidate("end", 3600000, "destination", "0.99", "Pending"),
        g_candidate("late", 3600001, "destination", "0.99", "Complete"), deposit, other_coin
    ]));
    let a = gemini_adapter(&d, &t);
    assert!(w::mark_dispatched(&d.c, &d.cipher, &gemini_adapter(&d, &gemini_reads()), &i, &FixedClock(now())).await.unwrap());
    w::recover(&d.c, &d.cipher, &a, "rule:1", &FixedClock(now())).await.unwrap();
    let s = rules::status(&d.c, 1).unwrap().unwrap();
    assert_eq!(s.state, "unresolved");
    assert!(s.reason.unwrap().contains("resolve-withdrawal"));
    assert_eq!(s.candidates.iter().map(|c| c["id"].as_str().unwrap()).collect::<Vec<_>>(), vec!["start", "end"]);
    assert_eq!(s.candidates[0]["status"], "Complete");
    assert_eq!(s.candidates[1]["status"], "Pending");
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(d.count("rust_withdrawal_ids"), 0);
    assert_eq!(t.requests()[0].body, Some(json!({"timestamp":now().timestamp_millis(),"limit_transfers":50})));
}
#[tokio::test(start_paused = true)]
async fn gemini_x0_classifier_cells_through_real_adapter() {
    let vectors: Value =
        serde_json::from_str(include_str!("fixtures/withdrawal_vectors.json")).unwrap();
    let cells: Vec<_> = vectors["classifier"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["venue"] == "gemini" && c["replies"]["POST /v1/withdraw/btc"].is_array())
        .collect();
    assert_eq!(
        cells.len(),
        2 + 2 + 1 + 1 + 1,
        "Gemini X0: definitive errors + server errors + rate limit + HTML + timeout"
    );
    let d = gemini_db();
    let i = gemini_prepared(&d, &gemini_reads()).await;
    for c in cells {
        let reply = &c["replies"]["POST /v1/withdraw/btc"][0];
        let t = ScriptedTransport::default();
        if reply["network"].is_string() {
            t.network(G_SEND, "post_send", "timeout");
        } else {
            t.reply(
                G_SEND,
                reply["status"].as_u64().unwrap_or(200) as u16,
                reply["body"].clone(),
            );
        }
        let result = gemini_adapter(&d, &t).send(&i.request).await;
        let definitive = matches!(c["case"].as_str(), Some("gemini_http_400_insufficient_funds" | "gemini_http_400_invalid_signature"));
        let message = c["rule_logs"][0]["message"].as_str().unwrap();
        assert_eq!(rules::ambiguous("Gemini", message), !definitive);
        if let w::SendResult::Rejected { reason, .. } = &result {
            assert!(rules::definitive("Gemini", reason));
        }
        assert_eq!(
            matches!(
                result,
                w::SendResult::Rejected {
                    definitive: true,
                    ..
                }
            ),
            definitive,
            "{}",
            c["case"]
        );
        assert_eq!(
            matches!(result, w::SendResult::Ambiguous { .. }),
            !definitive,
            "{}",
            c["case"]
        );
    }
}


#[tokio::test(start_paused = true)]
async fn gemini_kill_worker() {
    let Ok(path) = std::env::var("E3_CRASH_DB") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let d = reopen(&path);
    let t = gemini_reads();
    t.reply(
        G_SEND,
        200,
        json!({"withdrawalId":"G-crash"}),
    );
    let i = gemini_prepared(&d, &t).await;
    let a = gemini_adapter(&d, &t);
    assert!(
        w::mark_dispatched(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
            .await
            .unwrap()
    );
    assert!(matches!(
        a.send(&i.request).await,
        w::SendResult::Accepted { .. }
    ));
    std::fs::write(path.with_extension("sends"), gemini_sends(&t).to_string()).unwrap();
    std::process::exit(74); // No settlement or destructors: the reply was lost.
}
#[tokio::test(start_paused = true)]
async fn gemini_kill_after_dispatch_real_adapter_sends_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.sqlite");
    let seed = gemini_db();
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
        .args(["--exact", "gemini_kill_worker", "--nocapture"])
        .env("E3_CRASH_DB", &path)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(74),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let d = reopen(&path);
    let t = gemini_reads();
    let a = gemini_adapter(&d, &t);
    let i = w::load(&d.c, &d.cipher, "rule:1").unwrap().unwrap();
    assert_eq!(i.state, "dispatched");
    for _ in 0..2 {
        w::recover(&d.c, &d.cipher, &a, "rule:1", &FixedClock(now()))
            .await
            .unwrap();
        w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
            .await
            .unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(path.with_extension("sends")).unwrap(),
        "1"
    );
    assert_eq!(gemini_sends(&t), 0);
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(rules::status(&d.c, 1).unwrap().unwrap().state, "unresolved");
}

#[tokio::test]
async fn gemini_signed_fields_bound_and_pretransmission_refusal() {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let d = gemini_db();
    let t = gemini_reads();
    let i = gemini_prepared(&d, &t).await;
    t.network(G_SEND, "pre_send", "expired");
    assert!(matches!(gemini_adapter(&d, &t).send(&i.request).await, w::SendResult::NotSent { .. }));
    let r = t.requests().into_iter().find(|r| r.path == "/v2/withdraw/bitcoin/btc").unwrap();
    assert_eq!(r.not_after, Some(now() + chrono::Duration::seconds(w::SEND_WINDOW_SECONDS) + chrono::Duration::from_std(deltabadger::venue::http::TOTAL_TIMEOUT).unwrap()));
    assert_eq!(r.method, "POST");
    assert!(r.query.is_empty());
    let fields: Vec<_> = r.body.as_ref().unwrap().as_object().unwrap().iter().map(|(k,v)| (k.as_str(),v.clone())).collect();
    let headers = deltabadger::venue::signers::gemini::headers("key", "secret", &r.path, &fields, 123);
    let header = |key: &str| &headers.iter().find(|(k,_)| *k == key).unwrap().1;
    let payload: Value = serde_json::from_slice(&base64::engine::general_purpose::STANDARD.decode(header("X-GEMINI-PAYLOAD")).unwrap()).unwrap();
    assert_eq!(payload, json!({"address":"destination","amount":"0.99","clientTransferId":i.request.client_id,"request":"/v2/withdraw/bitcoin/btc","nonce":"123"}));
    let mut mac = Hmac::<sha2::Sha384>::new_from_slice(b"secret").unwrap();
    mac.update(header("X-GEMINI-PAYLOAD").as_bytes());
    assert_eq!(header("X-GEMINI-SIGNATURE"), &hex::encode(mac.finalize().into_bytes()));
    assert_eq!(header("X-GEMINI-APIKEY"), "key");
    // The actual HTTP transport checks the absolute bound before opening a socket.
    let live = deltabadger::venue::http::ReqwestTransport::gemini(deltabadger::venue::http::client(), "key".into(), "secret".into())
        .with_clock(std::sync::Arc::new(FixedClock(r.not_after.unwrap() + chrono::Duration::milliseconds(1))));
    use deltabadger::venue::Venue;
    assert!(matches!(GeminiVenue::new(live, deltabadger::venue::gemini::URL).withdraw(&i.request).await, w::SendResult::NotSent { .. }));
}

#[tokio::test]
async fn gemini_recorder_bootstrap_quotes_budget_then_amount() {
    let d = gemini_db();
    let (_dir, lock) = recorder_lock();
    let mut spec = recorder_spec();
    spec.destination.network = Some("bitcoin".into());
    let t = ScriptedTransport::default();
    t.reply("POST /v1/balances", 200, json!([{"currency":"BTC","type":"exchange","amount":"1","available":"1","availableForWithdrawal":"1"}]));
    t.reply("POST /v1/approvedAddresses/account/bitcoin", 200, json!({"approvedAddresses":[{"address":"destination","network":"bitcoin","status":"active"}]}));
    let fee = "POST /v2/withdraw/bitcoin/btc/feeEstimate";
    t.reply(fee, 200, json!({"currency":"BTC","fee":"0.0001"}));
    t.reply(fee, 200, json!({"currency":"BTC","fee":"0.00009"}));
    let a = gemini_adapter(&d, &t);
    let i = w::begin_recorder(&lock, &d.c, &d.cipher, &a, &spec, &FixedClock(now())).await.unwrap();
    assert_eq!(i.owner, "recorder:external-1");
    assert_eq!(i.request.amount, "0.0009");
    assert_eq!(i.request.fee_cap, "0.00009");
    let quotes: Vec<_> = t.requests().into_iter().filter(|r| r.path.ends_with("/feeEstimate")).map(|r| r.body.unwrap()).collect();
    assert_eq!(quotes, vec![json!({"address":"destination","amount":"0.001"}), json!({"address":"destination","amount":"0.0009"})]);
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(d.count("rust_rule_evaluations"), 0);
    assert_eq!(gemini_sends(&t), 0);
    assert!(w::begin_recorder(&lock, &d.c, &d.cipher, &a, &spec, &FixedClock(now())).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn gemini_lookup_pages_are_paced_deduplicated_and_never_prove_absence() {
    let d = gemini_db();
    let i = gemini_prepared(&d, &gemini_reads()).await;
    let t = ScriptedTransport::default();
    t.set_now(now() + chrono::Duration::seconds(3600));
    let page: Vec<_> = (0..50).map(|x| g_candidate(&x.to_string(), x, "destination", "0.99", "Complete")).collect();
    t.reply(G_STATUS, 200, json!(page));
    t.reply(G_STATUS, 200, json!([g_candidate("49", 49, "destination", "0.99", "Complete"),g_candidate("50", 50, "destination", "0.99", "Pending")]));
    let start = tokio::time::Instant::now();
    let w::LookupResult::Unknown { candidates, .. } = gemini_adapter(&d,&t).lookup(&i.request).await else { panic!("candidate lookup settled") };
    assert_eq!(candidates.len(), 50 + 1);
    assert!(start.elapsed() >= std::time::Duration::from_secs(5));
    assert_eq!(t.requests()[1].body.as_ref().unwrap()["timestamp"], now().timestamp_millis()+49);
    for body in [json!([]), json!({"result":"error","reason":"MissingRole"}), json!([{"type":"Withdrawal"}]), json!("bad JSON")] {
        let t = ScriptedTransport::default();
        t.reply(G_STATUS, 200, body);
        assert!(matches!(gemini_adapter(&d,&t).lookup(&i.request).await, w::LookupResult::Unknown { .. }));
    }
    let t = ScriptedTransport::default();
    t.reply(G_STATUS, 200, json!(page)); // repeating full page must terminate
    assert!(matches!(gemini_adapter(&d,&t).lookup(&i.request).await, w::LookupResult::Unknown { .. }));
    assert!(t.requests().len() <= 2);
}

#[tokio::test]
async fn gemini_send_refuses_non_memo_free_or_mismatched_network_before_transport() {
    let d = gemini_db();
    let i = gemini_prepared(&d, &gemini_reads()).await;
    for (ticker, network) in [("sol", "solana"), ("btc", "ethereum"), ("../btc", "bitcoin")] {
        let mut request = i.request.clone();
        request.destination = v::Destination::Gemini { ticker:ticker.into(), network:network.into(), address:"destination".into() };
        let t = ScriptedTransport::default();
        assert!(matches!(gemini_adapter(&d, &t).send(&request).await, w::SendResult::NotSent { .. }));
        assert!(t.requests().is_empty());
    }
}

// A withdrawal reaches the transport, but the transport refuses before transmitting it.
// Reads still use the production adapters and their usual scripted answers.
struct RefusingWithdrawalTransport {
    reads: ScriptedTransport,
    attempts: std::rc::Rc<Cell<usize>>,
    reason: &'static str,
}
impl deltabadger::venue::http::Transport for RefusingWithdrawalTransport {
    async fn send(
        &self,
        r: &deltabadger::venue::http::HttpRequest,
    ) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError>
    {
        if matches!(
            r.path.as_str(),
            "/0/private/Withdraw" | "/v2/withdraw/bitcoin/btc"
        ) {
            self.attempts.set(self.attempts.get() + 1);
            return Err(deltabadger::venue::http::TransportError::NotSent(
                self.reason.into(),
            ));
        }
        self.reads.send(r).await
    }
}
async fn assert_unsent_dispatch(d: &Db, i: &w::Intent, a: &impl w::Adapter) {
    w::dispatch(&d.c, &d.cipher, a, i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(d.count("rule_logs"), 1);
    let (status, message): (i64, String) =
        d.c.query_row("SELECT status,message FROM rule_logs", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(status, 2);
    assert!(message.starts_with("Withdrawal failed:"));
    assert_eq!(message, rules::safe_message(&message));
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert_eq!(d.count("rust_withdrawal_ids"), 0);
    assert!(
        rules::status(&d.c, 1).unwrap().is_none(),
        "no operator acknowledgement required"
    );
    w::recover(&d.c, &d.cipher, a, "rule:1", &FixedClock(now()))
        .await
        .unwrap();
    w::dispatch(&d.c, &d.cipher, a, i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(d.count("rule_logs"), 1);
}
#[tokio::test]
async fn kraken_not_sent_dispatch_settles_without_operator_ack() {
    for reason in ["connection refused", "expired send bound"] {
        let d = kraken_db();
        let t = kraken_reads();
        let i = kraken_prepared(&d, &t).await;
        let attempts = std::rc::Rc::new(Cell::new(0));
        let (cr, version) =
            deltabadger::engine::model::withdrawal_credentials(&d.c, &d.cipher, 1, 1)
                .unwrap()
                .unwrap();
        let a = w::VenueAdapter::new(
            KrakenVenue::new(RefusingWithdrawalTransport {
                reads: t.clone(),
                attempts: attempts.clone(),
                reason,
            })
            .with_identity(&cr),
            version,
            None,
        );
        assert_unsent_dispatch(&d, &i, &a).await;
        assert_eq!(attempts.get(), 1);
        assert_eq!(kraken_sends(&t), 0);
    }
}
#[tokio::test]
async fn gemini_not_sent_dispatch_settles_without_operator_ack() {
    for reason in ["connection refused", "expired send bound"] {
        let d = gemini_db();
        let t = gemini_reads();
        let i = gemini_prepared(&d, &t).await;
        let attempts = std::rc::Rc::new(Cell::new(0));
        let (cr, version) =
            deltabadger::engine::model::withdrawal_credentials(&d.c, &d.cipher, 1, 1)
                .unwrap()
                .unwrap();
        let a = w::VenueAdapter::new(
            GeminiVenue::new(
                RefusingWithdrawalTransport {
                    reads: t.clone(),
                    attempts: attempts.clone(),
                    reason,
                },
                deltabadger::venue::gemini::URL,
            )
            .with_identity(&cr),
            version,
            None,
        );
        assert_unsent_dispatch(&d, &i, &a).await;
        assert_eq!(attempts.get(), 1);
        assert_eq!(gemini_sends(&t), 0);
    }
}
#[tokio::test]
async fn gemini_long_rejection_dispatch_preserves_classification_and_scrubs_storage() {
    for reason in ["InsufficientFunds", "UnknownReason"] {
        let d = gemini_db();
        let t = gemini_reads();
        let body = format!("{{ \"result\":\"error\", \"reason\":\"{reason}\", \"message\":\"https://venue.invalid/private?signature=hidden token=hidden {}\" }}", "x".repeat(300));
        assert!(body.len() > 200);
        t.reply(G_SEND, 400, json!(body));
        let i = gemini_prepared(&d, &t).await;
        let a = gemini_adapter(&d, &t);
        w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
            .await
            .unwrap();
        if reason == "InsufficientFunds" {
            assert_eq!(d.count("rust_withdrawal_intents"), 0);
            let (status, message): (i64, String) =
                d.c.query_row("SELECT status,message FROM rule_logs", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .unwrap();
            assert_eq!(status, 2);
            assert_eq!(
                message,
                rules::safe_message(&format!("Withdrawal failed: {body}"))
            );
        } else {
            assert_eq!(d.count("rule_logs"), 0);
            let status = rules::status(&d.c, 1).unwrap().unwrap();
            assert_eq!(status.state, "unresolved");
            let message = status.reason.unwrap();
            assert_eq!(message, rules::safe_message(&message));
            assert!(!message.contains("hidden"));
        }
        assert_eq!(gemini_sends(&t), 1);
    }
}
#[tokio::test]
async fn kraken_rejection_keeps_unmodified_venue_body() {
    let d = kraken_db();
    let t = kraken_reads();
    let i = kraken_prepared(&d, &t).await;
    let body = "{ \"error\": [\"EFunding:Max fee exceeded\"], \"result\": null }";
    t.reply(K_SEND, 400, json!(body));
    let a = kraken_adapter(&d, &t);
    let w::SendResult::Rejected {
        definitive: true,
        reason,
    } = a.send(&i.request).await
    else {
        panic!("expected rejection")
    };
    assert_eq!(reason, body);
    w::dispatch(&d.c, &d.cipher, &a, &i, &FixedClock(now()))
        .await
        .unwrap();
    assert_eq!(d.count("rust_withdrawal_intents"), 0);
    assert_eq!(
        d.c.query_row("SELECT status FROM rule_logs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[tokio::test]
async fn unallowlisted_rejection_dispatch_stays_unresolved_with_safe_text() {
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    let body = format!(
        "https://venue.invalid/private?signature=hidden token=hidden {}",
        "x".repeat(300)
    );
    *f.answer.borrow_mut() = w::SendResult::Rejected {
        definitive: true,
        reason: body.clone(),
    };
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    let status = rules::status(&d.c, 1).unwrap().unwrap();
    assert_eq!(status.state, "unresolved");
    assert_eq!(
        status.reason.as_deref(),
        Some(rules::safe_message(&body).as_str())
    );
    assert_eq!(d.count("rule_logs"), 0);
    assert_eq!(f.sends.get(), 1);
}

#[tokio::test]
async fn kraken_settlement_double_checks_every_error_in_raw_body() {
    for body in [
        json!({"error":[]}),
        json!({"error":["EFunding:Max fee exceeded", "Unknown"]}),
        json!({"error":["EFunding:Max fee exceeded", null]}),
        json!({"error":["EGeneral:Invalid arguments"]}),
        json!({"error":["EService:Unavailable"]}),
        json!({"error":["EFunding:Max fee exceeded"], "result":{"refid":"K-1"}}),
    ] {
        let d = kraken_db();
        let t = kraken_reads();
        let i = kraken_prepared(&d, &t).await;
        assert!(w::mark_dispatched(
            &d.c,
            &d.cipher,
            &kraken_adapter(&d, &t),
            &i,
            &FixedClock(now())
        )
        .await
        .unwrap());
        w::settle(
            &d.c,
            &d.cipher,
            &i,
            "dispatched",
            w::SendResult::Rejected {
                definitive: true,
                reason: body.to_string(),
            },
            now(),
        )
        .unwrap();
        assert_eq!(d.count("rule_logs"), 0, "{body}");
        assert_eq!(
            rules::status(&d.c, 1).unwrap().unwrap().state,
            "unresolved",
            "{body}"
        );
    }
}

#[tokio::test]
async fn not_sent_and_candidate_text_obey_rule_log_secrets() {
    let text = format!(
        "https://venue.invalid/private?signature=hidden token=hidden {}",
        "x".repeat(300)
    );
    let d = Db::new();
    let f = Fake::new(&d);
    let i = d.prepared(&f).await;
    *f.answer.borrow_mut() = w::SendResult::NotSent {
        reason: text.clone(),
    };
    assert_unsent_dispatch(&d, &i, &f).await;
    let message: String =
        d.c.query_row("SELECT message FROM rule_logs", [], |r| r.get(0))
            .unwrap();
    assert!(!message.contains("hidden"));

    let i = d.prepared(&f).await;
    *f.answer.borrow_mut() = w::SendResult::Ambiguous {
        reason: text.clone(),
    };
    w::dispatch(&d.c, &d.cipher, &f, &i, &FixedClock(now()))
        .await
        .unwrap();
    *f.lookup.borrow_mut() = w::LookupResult::Unknown {
        reason: text.clone(),
        candidates: vec![json!({"id":text,"status":text,"amount":text,"at":text,"raw":text})],
    };
    w::recover(&d.c, &d.cipher, &f, "rule:1", &FixedClock(now()))
        .await
        .unwrap();
    let status = rules::status(&d.c, 1).unwrap().unwrap();
    assert_eq!(status.reason, Some(rules::safe_message(&text)));
    assert_eq!(
        status.candidates,
        vec![
            json!({"id":rules::safe_message(&text),"status":rules::safe_message(&text),"amount":rules::safe_message(&text),"at":rules::safe_message(&text)})
        ]
    );
}
