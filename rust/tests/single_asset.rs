//! PLAN-A23: legacy single-asset bots become one-asset baskets at every start (engine::single_asset), against Rails' own
//! Bot::SingleToComposition (fixtures/single_asset_vectors.json, script/rust/single_asset.rb).
mod common;
use chrono::{DateTime, Utc};
use common::seed;
use deltabadger::engine::single_asset::{self, Skip};
use deltabadger::engine::{handover, model, set_aside, EngineError};
use deltabadger::figures::{at::At, db, pair, walk};
use deltabadger::{lease, store};
use rusqlite::{types::Value as Sql, Connection};
use serde_json::{json, Value};

fn fixture() -> Value { serde_json::from_str(include_str!("fixtures/single_asset_vectors.json")).unwrap() }
fn now() -> DateTime<Utc> { fixture()["now"].as_str().unwrap().parse().unwrap() }
fn scenarios() -> Vec<Value> { fixture()["scenarios"].as_array().unwrap().clone() }
fn scenario(name: &str) -> Value { scenarios().into_iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("no scenario {name}")) }

/// The Rails sources the vectors were recorded from, unchanged since.
#[test]
fn ported_sources_are_unchanged() {
    use sha2::{Digest, Sha256};
    let f = fixture();
    let pinned = f["ported_sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 8);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the vectors were recorded");
    }
}

const TABLES: [&str; 9] = ["users", "exchanges", "assets", "exchange_assets", "tickers", "api_keys", "bots", "transactions", "bot_index_assets"];

/// A fresh install holding the scenario's `before` rows verbatim; the API keys re-encrypted with the test secret.
fn install(sc: &Value) -> (tempfile::TempDir, store::Opened) {
    let dir = common::baseline_install();
    let o = store::open(&store::Paths::from_env(&|_| None, dir.path())).unwrap();
    let c = &o.primary;
    c.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
    for table in TABLES {
        for row in sc["before"][table].as_array().into_iter().flatten() {
            let mut row = row.as_object().unwrap().clone();
            if table == "api_keys" {
                row.insert("key".into(), json!(seed::cipher().encrypt("test-key")));
                row.insert("secret".into(), json!(seed::cipher().encrypt("dGVzdC1zZWNyZXQ=")));
            }
            let columns: Vec<String> = row.keys().map(|k| format!("\"{k}\"")).collect();
            let values: Vec<Sql> = row.values().map(|v| match v {
                Value::Null => Sql::Null,
                Value::Number(n) if n.is_i64() => Sql::Integer(n.as_i64().unwrap()),
                Value::Number(n) => Sql::Real(n.as_f64().unwrap()),
                Value::String(s) => Sql::Text(s.clone()),
                other => Sql::Text(other.to_string()),
            }).collect();
            c.execute(&format!("INSERT INTO {table} ({}) VALUES ({})", columns.join(","), vec!["?"; columns.len()].join(",")), rusqlite::params_from_iter(values)).unwrap();
        }
    }
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    (dir, o)
}

/// `SELECT * FROM table ORDER BY id`, each value as SQLite holds it.
fn rows(c: &Connection, table: &str) -> Value {
    let mut s = c.prepare(&format!("SELECT * FROM {table} ORDER BY id")).unwrap();
    let names: Vec<String> = s.column_names().iter().map(|n| n.to_string()).collect();
    let out: Vec<Value> = s.query_map([], |r| {
        let mut m = serde_json::Map::new();
        for (i, n) in names.iter().enumerate() {
            m.insert(n.clone(), match r.get::<_, Sql>(i)? {
                Sql::Null => Value::Null, Sql::Integer(i) => json!(i), Sql::Real(f) => json!(f), Sql::Text(t) => json!(t), Sql::Blob(b) => json!(b),
            });
        }
        Ok(Value::Object(m))
    }).unwrap().collect::<Result<_, _>>().unwrap();
    Value::Array(out)
}

fn convert(c: &Connection) -> Vec<i64> {
    let tx = model::immediate(c).unwrap();
    let ids = single_asset::convert_all(&tx, now()).unwrap();
    tx.commit().unwrap();
    ids
}

/// Rails' skip, as Rust keeps the row (plan §4); None where Rust converts (the forced cases).
fn kept(reason: &str) -> Option<Skip> {
    Some(match reason.strip_prefix("not repaired: ").unwrap_or(reason) {
        "missing assets" | "missing exchange" | "missing asset rows" => Skip::Settings,
        "missing ticker" => Skip::Market,
        "orders of another asset" => Skip::History("orders of another asset"),
        "non-regular orders" => Skip::History("rebalancing, liquidation or redeploy orders"),
        "unpriced sell" => Skip::History("a sale whose proceeds weren't reported"),
        "unexpected composition rows" => Skip::Memberships,
        // D-SINGLE-QUIET-AT-CLAIM, D-SINGLE-UNTRADEABLE-CONVERTS, D-SINGLE-CONDITION-CONVERTS.
        "executing" | "job in flight" | "ticker not tradeable" | "condition watches another ticker" => return None,
        other => panic!("an unmapped Rails skip: {other}"),
    })
}

/// The ids Rust converts in a scenario: every bot whose row Rails' writes changed (the forced ones included).
fn changed(sc: &Value) -> Vec<i64> {
    let before = sc["before"]["bots"].as_array().unwrap();
    sc["after"]["bots"].as_array().unwrap().iter().filter(|a| !before.contains(a)).map(|a| a["id"].as_i64().unwrap()).collect()
}

/// §8 1 and 3: each recorded "before", converted at the same instant, equals Rails' "after" in every column; Rails' skips
/// are the kept rows with their reasons; a second pass changes nothing.
#[test]
fn recorded_conversions_are_byte_equal() {
    let all = scenarios();
    assert_eq!(all.len(), 55);
    for sc in &all {
        let name = sc["name"].as_str().unwrap();
        let (_d, o) = install(sc);
        let c = &o.primary;
        let mut ids = convert(c);
        if sc["mode"] == "run_twice" { ids = convert(c); }
        for table in ["bots", "bot_index_assets", "transactions"] { assert_eq!(rows(c, table), sc["after"][table], "{name}: {table}"); }
        let expected = if sc["mode"] == "run_twice" { vec![] } else { changed(sc) };
        assert_eq!(ids, expected, "{name}: converted");
        assert!(convert(c).is_empty(), "{name}: a second pass converts nothing");
        for table in ["bots", "bot_index_assets", "transactions"] { assert_eq!(rows(c, table), sc["after"][table], "{name}: {table} after a second pass"); }
        for skip in sc["rails_skips"].as_array().unwrap() {
            let (id, reason) = (skip[0].as_i64().unwrap(), skip[1].as_str().unwrap());
            let Some(want) = kept(reason) else { assert_eq!(sc["mode"], "forced", "{name}"); assert!(ids.contains(&id), "{name}: Rust converts it"); continue };
            assert_eq!(single_asset::skip(c, &model::load_bot(c, id).unwrap()).unwrap(), want, "{name}");
        }
    }
}

fn time(v: &Value) -> DateTime<Utc> { v.as_str().unwrap().parse().unwrap() }
fn dec(v: &Value) -> deltabadger::ruby::BigDec { deltabadger::ruby::BigDec::parse(v.as_str().unwrap()).unwrap() }

/// §8 2: Rust's next due time and carry for each converted bot equal Rails' for the basket it converted into.
#[test]
fn recorded_schedule_and_carry_hold() {
    let mut n = 0;
    for sc in scenarios() {
        let name = sc["name"].as_str().unwrap();
        let (_d, o) = install(&sc);
        let c = &o.primary;
        for id in convert(c) {
            let after = &sc["probes"][id.to_string()]["after"];
            assert_eq!(after["type"], "Bots::DcaMultiAsset", "{name}");
            let bot = model::load_bot(c, id).unwrap();
            let anchor = bot.started_at_us().unwrap().unwrap_or(now().timestamp_micros());
            let next = deltabadger::engine::schedule::checkpoints(anchor, now().timestamp_micros(), bot.effective().unwrap()).unwrap().next_us;
            // D-START-ANCHOR-STARTED (A22): Rails reads a start_at that differs from started_at while the rule is on; the
            // engine reads started_at, as for any basket. The conversion keeps both byte for byte.
            let rails = time(&after["next_interval_checkpoint_at"]).timestamp_micros();
            if bot.settings["start_time_enabled"] == json!(true) && bot.settings["start_at"].as_str().map(|s| time(&json!(s)).timestamp_micros()) == Some(rails) {
                assert_eq!(next, anchor, "{name}: the engine waits for started_at");
            } else {
                assert_eq!(next, rails, "{name}: next checkpoint");
            }
            let owed = deltabadger::engine::amount::pending_quote_amount(c, &bot, now().timestamp_micros()).unwrap();
            assert_eq!(owed, dec(&after["pending_quote_amount"]), "{name}: pending quote amount");
            n += 1;
        }
    }
    assert!(n >= 40, "{n} converted bots probed");
}

/// The walk's figures as Rails' `to_json` writes them.
fn figures(c: &Connection, id: i64) -> Value {
    let at = At::from_utc(now()).unwrap();
    let j = if model::load_bot(c, id).unwrap().bot_type == single_asset::SINGLE { pair::metrics(c, id, at).unwrap().to_json(3) }
            else { walk::metrics(c, &db::Subject::load(c, id).unwrap(), at).unwrap().to_json(3) };
    serde_json::from_str(&j.write()).unwrap()
}

/// §8 5 (the figures contract): Rust's pair figures before = Rails' before, the basket's after = Rails' after; before = after
/// only on consistent fills, and the inconsistent fill changes alike on both sides.
#[test]
fn figures_follow_rails_before_and_after() {
    let (mut compared, mut off) = (0, vec![]);
    for sc in scenarios() {
        let name = sc["name"].as_str().unwrap();
        let (_d, o) = install(&sc);
        let c = &o.primary;
        let id = 1;
        let (before, after) = (&sc["probes"]["1"]["before"]["metrics"], &sc["probes"]["1"]["after"]["metrics"]);
        // Rails can't load a JSON array as settings (every probe null); Rust reads it as {} and keeps it (S-SETTINGS). Rust's
        // pair figures need the bot's exchange, which Rails' walk doesn't read: outside this slice.
        // A clobbered basket with an unpriced sell is a basket before and after (its repair is skipped): Rust's basket walk
        // values that sale where Rails' reads nothing. A basket-walk question outside this slice (reported, not changed here).
        if before.is_null() || name == "missing exchange" || name == "clobbered basket, repair skipped: unpriced sell" { continue; }
        if &figures(c, id) != before { off.push(format!("{name}: before")); }
        convert(c);
        if !after.is_null() && &figures(c, id) != after { off.push(format!("{name}: after")); }
        compared += 1;
    }
    assert_eq!(off, Vec::<String>::new());
    assert!(compared >= 50, "{compared}");
    let odd = scenario("a buy whose price differs from its executed value");
    let (b, a) = (&odd["probes"]["1"]["before"]["metrics"], &odd["probes"]["1"]["after"]["metrics"]);
    assert_ne!(b["total_amount_value_in_quote"], a["total_amount_value_in_quote"], "Rails' own figures change with the conversion");
    let plain = scenario("stopped bot");
    let (b, a) = (&plain["probes"]["1"]["before"]["metrics"], &plain["probes"]["1"]["after"]["metrics"]);
    for key in ["total_quote_amount_invested", "total_amount_value_in_quote", "pnl"] { assert_eq!(b[key], a[key], "consistent fills: {key}"); }
}

fn take_over(dir: &std::path::Path, o: &store::Opened) -> Result<handover::Takeover, EngineError> {
    let lock = lease::lock(&store::Paths::from_env(&|_| None, dir), now()).unwrap();
    handover::take_over(&lock, o, &seed::cipher(), "test", now())
}

/// §8 4: a takeover of each case. Converted bots run (or are set aside by the basket check, never as legacy); kept working
/// ones are set aside with their sentence; kept stopped ones are untouched; a restart converts and sets aside nothing more.
#[test]
fn a_takeover_converts_what_it_can_and_names_what_it_keeps() {
    for sc in scenarios() {
        let name = sc["name"].as_str().unwrap();
        let (dir, o) = install(&sc);
        let c = &o.primary;
        let t = match take_over(dir.path(), &o) {
            Ok(t) => t,
            Err(e) => panic!("{name}: {e:?}"),
        };
        let before: Vec<Value> = sc["before"]["bots"].as_array().unwrap().clone();
        for b in &before {
            let id = b["id"].as_i64().unwrap();
            let bot = model::load_bot(c, id).unwrap();
            let aside = t.set_aside.iter().find(|a| a.id == id);
            if changed(&sc).contains(&id) {
                assert!(!single_asset::legacy(&bot), "{name}: converted");
                if let Some(a) = aside { assert!(!a.reasons.iter().any(|r| r.starts_with("This is an older kind of bot")), "{name}: {a:?}"); }
                continue;
            }
            let reason = sc["rails_skips"].as_array().unwrap().iter().find(|s| s[0] == id).map(|s| s[1].as_str().unwrap().to_string()).unwrap_or_else(|| panic!("{name}: kept without a Rails skip"));
            let sentence = set_aside::legacy_sentence(c, id, &kept(&reason).unwrap()).unwrap();
            let working = [1, 4, 5, 6].contains(&b["status"].as_i64().unwrap());
            if working {
                assert_eq!(aside.map(|a| a.reasons.clone()), Some(vec![sentence]), "{name}");
            } else {
                assert!(aside.is_none(), "{name}");
                assert_eq!(rows(c, "bots")[0]["status"], b["status"], "{name}: a stopped kept bot keeps its status");
                assert!(bot.transient.get(set_aside::KEY).is_none(), "{name}");
            }
        }
        let again = take_over(dir.path(), &o).unwrap_or_else(|e| panic!("{name}: restart {e:?}"));
        assert!(again.set_aside.is_empty(), "{name}: restart sets nothing more aside");
        assert!(convert(c).is_empty(), "{name}: nothing more converts");
    }
}

// ---- §8 6, 5b, 7: the engine after a takeover ----

use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::FixedClock;
use deltabadger::venue::fake::{FakeFactory, FakeVenue};

/// The Rails queue row a scenario recorded, as Solid Queue holds it.
fn queue(q: &Connection, sc: &Value) {
    for job in sc["queue"].as_array().unwrap() {
        q.execute("INSERT INTO solid_queue_jobs (queue_name, class_name, arguments, priority, created_at, updated_at) VALUES ('kraken', ?1, ?2, 0, '2026-08-31 09:00:00', '2026-08-31 09:00:00')",
                  [job["class_name"].as_str().unwrap(), job["arguments"].as_str().unwrap()]).unwrap();
        let id = q.last_insert_rowid();
        let args: Value = serde_json::from_str(job["arguments"].as_str().unwrap()).unwrap();
        match job["state"].as_str().unwrap() {
            "scheduled_future" | "scheduled_due" => {
                let at = deltabadger::codec::format_time(time(&args["scheduled_at"]));
                q.execute("INSERT INTO solid_queue_scheduled_executions (job_id, queue_name, priority, scheduled_at, created_at) VALUES (?1, 'kraken', 0, ?2, '2026-08-31 09:00:00')", rusqlite::params![id, at]).unwrap();
            }
            "claimed" => { q.execute("INSERT INTO solid_queue_claimed_executions (job_id, process_id, created_at) VALUES (?1, 1, '2026-08-31 09:00:00')", [id]).unwrap(); }
            other => panic!("{other}"),
        }
    }
}

fn kraken() -> FakeVenue {
    let v = FakeVenue::new().ticker("XXBTZEUR", "100.0", "100.0", "100.0");
    v.script("/0/private/BalanceEx", json!({ "error": [], "result": { "ZEUR": { "balance": "100000", "hold_trade": "0" }, "XXBT": { "balance": "10", "hold_trade": "0" } } }));
    v
}

/// The scenario's install, its Rails queue, taken over; then the engine on it.
fn engine(sc: &Value, venue: FakeVenue) -> (tempfile::TempDir, handover::Takeover, Engine<FakeFactory>) {
    let (dir, o) = install(sc);
    queue(&o.queue, sc);
    let lock = lease::lock(&store::Paths::from_env(&|_| None, dir.path()), now()).unwrap();
    let t = handover::take_over(&lock, &o, &seed::cipher(), "test", now()).unwrap();
    let e = Engine::new(o.primary, FakeFactory(venue), seed::cipher(), lock);
    (dir, t, e)
}
fn at(t: DateTime<Utc>) -> FixedClock { FixedClock(t) }

/// §8 6: a Rails ActionJob scheduled for later: no order before it, one at it, none extra (D-START-ANCHOR-STARTED: the
/// engine's own schedule lands where Rails' job would have run).
#[tokio::test(flavor = "current_thread")]
async fn a_future_rails_job_is_neither_doubled_nor_lost() {
    let sc = scenario("working bot with a future ActionJob");
    let job = time(&serde_json::from_str::<Value>(sc["queue"][0]["arguments"].as_str().unwrap()).unwrap()["scheduled_at"]);
    assert_eq!(job, time(&sc["probes"]["1"]["after"]["next_interval_checkpoint_at"]));
    // Rails' ActionJob stamps its run (`last_action_job_at: Time.current`, action_job.rb:108) before it schedules the next
    // one: the recorded row leaves it out, so it is put back as the run that scheduled this job wrote it. (Without it any
    // basket, converted or native, is due at the first pass: placement sizing below.)
    let mut sc = sc;
    let mut transient: Value = serde_json::from_str(sc["before"]["bots"][0]["transient_data"].as_str().unwrap()).unwrap();
    transient["last_action_job_at"] = json!("2026-09-10T09:00:00.300Z");
    sc["before"]["bots"][0]["transient_data"] = json!(transient.to_string());
    let venue = kraken();
    let (_d, t, mut e) = engine(&sc, venue.clone());
    assert_eq!(t.eligible, vec![1]);
    run::step(&mut e, &at(now())).await.unwrap();
    run::step(&mut e, &at(job - chrono::Duration::seconds(1))).await.unwrap();
    assert!(venue.sent().is_empty(), "nothing before Rails' job time");
    run::step(&mut e, &at(job + chrono::Duration::milliseconds(1))).await.unwrap();
    assert_eq!(venue.sent().len(), 1, "one order at it");
    run::step(&mut e, &at(job + chrono::Duration::seconds(2))).await.unwrap();
    assert_eq!(venue.sent().len(), 1, "none extra");
}

/// §8 6: a due Rails job: exactly one order at the start's first tick. A claimed one: the bot converts and carries C12.
#[tokio::test(flavor = "current_thread")]
async fn a_due_rails_job_ticks_once_and_a_claimed_one_carries_its_line() {
    let venue = kraken();
    let (_d, t, mut e) = engine(&scenario("due ActionJob"), venue.clone());
    assert_eq!(t.eligible, vec![1]);
    run::step(&mut e, &at(now())).await.unwrap();
    run::step(&mut e, &at(now() + chrono::Duration::seconds(1))).await.unwrap();
    assert_eq!(venue.sent().len(), 1);
    let (_d, t, e) = engine(&scenario("claimed ActionJob"), kraken());
    assert_eq!(t.eligible, vec![1]);
    assert_eq!(model::load_bot(&e.primary, 1).unwrap().bot_type, single_asset::MULTI);
    let lines: i64 = e.primary.query_row("SELECT count(*) FROM bot_activity_logs WHERE bot_id = 1 AND event = ?1", [handover::UPGRADE_ORDER_UNRECORDED], |r| r.get(0)).unwrap();
    assert_eq!(lines, 1, "C12 on the converted bot");
}

/// §8 7 (D-UPGRADE-RAILS-CHECKS-REDONE): a converted `retrying` bot ticks at the first pass after the takeover, as every
/// basket left retrying does; an `executing` one (its job claimed) converts and is unstuck.
#[tokio::test(flavor = "current_thread")]
async fn a_converted_retrying_bot_ticks_at_the_first_pass_and_an_executing_one_is_unstuck() {
    let venue = kraken();
    let (_d, t, mut e) = engine(&scenario("retrying with an exhausted ActionJob rescheduled"), venue.clone());
    assert_eq!(t.eligible, vec![1]);
    run::step(&mut e, &at(now())).await.unwrap();
    assert_eq!(venue.sent().len(), 1);
    let (_d, t, e) = engine(&scenario("executing"), kraken());
    assert_eq!((t.eligible, t.normalised), (vec![1], 1));
    let bot = model::load_bot(&e.primary, 1).unwrap();
    assert_eq!((bot.bot_type.as_str(), bot.status), (single_asset::MULTI, deltabadger::enums::BotStatus::Scheduled));
}

/// §8 7 (D-SINGLE-UNTRADEABLE-CONVERTS): a working bot on a ticker its venue no longer trades converts, and its tick fails
/// with the basket's own message; nothing is placed. (Rails' pair conditions filter tickers on `available` only, the basket's
/// on available && trading_enabled: identical in every recorded case.)
#[tokio::test(flavor = "current_thread")]
async fn an_untradeable_working_bot_converts_and_its_tick_fails_visibly() {
    let venue = kraken();
    let sc = scenario("working on an unavailable ticker");
    let (_d, t, mut e) = engine(&sc, venue.clone());
    assert_eq!(t.eligible, vec![1]);
    let due = time(&sc["probes"]["1"]["after"]["next_interval_checkpoint_at"]);
    let _ = run::step(&mut e, &at(due + chrono::Duration::milliseconds(1))).await;
    assert!(venue.sent().is_empty());
    let said: String = e.primary.query_row("SELECT transient_data || coalesce((SELECT group_concat(details) FROM bot_activity_logs WHERE bot_id = 1), '') FROM bots WHERE id = 1", [], |r| r.get(0)).unwrap();
    assert!(said.contains("None of the portfolio's weighted assets trade on Kraken"), "{said}");
    // With its price condition on (unlatched, pause): the condition finds no available ticker and the bot pauses there.
    let mut sc = sc;
    let mut settings: Value = serde_json::from_str(sc["before"]["bots"][0]["settings"].as_str().unwrap()).unwrap();
    settings["price_limited"] = json!(true);
    sc["before"]["bots"][0]["settings"] = json!(settings.to_string());
    let venue = kraken();
    let (_d, t, mut e) = engine(&sc, venue.clone());
    assert_eq!(t.eligible, vec![1]);
    let _ = run::step(&mut e, &at(due + chrono::Duration::milliseconds(1))).await;
    assert!(venue.sent().is_empty());
    let said: String = e.primary.query_row("SELECT transient_data || coalesce((SELECT group_concat(details) FROM bot_activity_logs WHERE bot_id = 1), '') FROM bots WHERE id = 1", [], |r| r.get(0)).unwrap();
    assert!(!said.contains("None of the portfolio's weighted assets"), "the gate stops it first: {said}");
}

/// §8 5b: the converted bot's orders are the amounts Rails' converted bot sizes: a buy at its carry, a sale of
/// `sell_amount` in base whether its sentence was blank (base, as the single bot sold) or quote.
#[tokio::test(flavor = "current_thread")]
async fn converted_orders_are_sized_as_rails_sizes_them() {
    for name in ["working bot with a future ActionJob", "smart interval bot", "selling, sell_denomination absent", "selling, sell_denomination empty string", "selling, sell_denomination quote"] {
        let sc = scenario(name);
        let probe = &sc["probes"]["1"]["after"];
        let venue = kraken();
        let (dir, o) = install(&sc);
        // A selling scenario is a stopped bot: started now, as a start would.
        o.primary.execute("UPDATE bots SET status = 1, started_at = coalesce(started_at, ?1) WHERE id = 1", [deltabadger::codec::format_time(now())]).unwrap();
        let lock = lease::lock(&store::Paths::from_env(&|_| None, dir.path()), now()).unwrap();
        let t = handover::take_over(&lock, &o, &seed::cipher(), "test", now()).unwrap();
        assert_eq!(t.eligible, vec![1], "{name}: {:?}", t.set_aside);
        let mut e = Engine::new(o.primary, FakeFactory(venue.clone()), seed::cipher(), lock);
        // The buy: never run since its start (no last_action_job_at), so due at the first pass, owing Rails' carry now.
        let due = if probe["sell_amount"].is_null() { now() } else { time(&probe["next_interval_checkpoint_at"]).max(now()) };
        run::step(&mut e, &at(due + chrono::Duration::milliseconds(1))).await.unwrap();
        let sent = venue.sent();
        let state: String = e.primary.query_row("SELECT status || ' ' || transient_data || coalesce((SELECT group_concat(event || ':' || details) FROM bot_activity_logs WHERE bot_id = 1), '') FROM bots WHERE id = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(sent.len(), 1, "{name}: {state}");
        let volume = deltabadger::ruby::BigDec::parse(&sent[0].volume).unwrap();
        if let Some(sell) = probe["sell_amount"].as_str() {
            assert!(!sent[0].quote_volume, "{name}: sold in base");
            assert_eq!(volume, deltabadger::ruby::BigDec::parse(sell).unwrap(), "{name}");
        } else {
            let quote = if sent[0].quote_volume { volume } else { &volume * &deltabadger::ruby::BigDec::parse("100.0").unwrap() };
            // What Rails' converted bot owes now (its smart-interval cadence included).
            assert_eq!(quote, dec(&probe["pending_quote_amount"]), "{name}");
        }
    }
}

// ---- §8 7: kept rows ----

/// A kept row whose ticker comes back while the engine runs: Skip::NextStart (stable, never admitted), a guarded write
/// commits, the row stays single; the restart converts it.
#[test]
fn a_kept_bot_whose_ticker_returns_converts_at_the_next_start() {
    let sc = scenario("missing ticker");
    let (dir, o) = install(&sc);
    let c = &o.primary;
    take_over(dir.path(), &o).unwrap();
    assert_eq!(single_asset::skip(c, &model::load_bot(c, 1).unwrap()).unwrap(), Skip::Market);
    let (base, quote): (i64, i64) = c.query_row("SELECT json_extract(settings, '$.base_asset_id'), json_extract(settings, '$.quote_asset_id') FROM bots WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    c.execute("INSERT INTO tickers (exchange_id, ticker, base, quote, base_asset_id, quote_asset_id, base_decimals, quote_decimals, price_decimals, minimum_base_size, minimum_quote_size, trading_enabled, available, created_at, updated_at) \
               VALUES (1, 'BACK', 'B', 'Q', ?1, ?2, 8, 2, 2, '0.0001', '1', 1, 1, '2026-09-10 12:00:00', '2026-09-10 12:00:00')", [base, quote]).unwrap();
    let bot = model::load_bot(c, 1).unwrap();
    assert_eq!(single_asset::skip(c, &bot).unwrap(), Skip::NextStart);
    let reasons = deltabadger::engine::eligibility::bot_reasons_typed(c, &bot).unwrap();
    assert_eq!(reasons.iter().map(|r| r.kind.clone()).collect::<Vec<_>>(), vec![deltabadger::engine::eligibility::Kind::Legacy(Skip::NextStart)]);
    let tx = model::immediate(c).unwrap();
    tx.execute("UPDATE bots SET label = 'renamed' WHERE id = 1", []).unwrap();
    deltabadger::engine::eligibility::guard(&tx, &seed::cipher(), Some(1)).unwrap();
    tx.commit().unwrap();
    assert_eq!(model::load_bot(c, 1).unwrap().bot_type, single_asset::SINGLE);
    take_over(dir.path(), &o).unwrap();
    assert_eq!(model::load_bot(c, 1).unwrap().bot_type, single_asset::MULTI, "converted at the restart");
}

/// A malformed transaction time on a kept row stays the unreadable hard stop, working or stopped; so does a single row
/// whose settings aren't JSON.
#[test]
fn damaged_legacy_rows_stay_unreadable() {
    for status in [1, 2] {
        let (dir, o) = install(&scenario("orders of another asset"));
        o.primary.execute("UPDATE bots SET status = ?1", [status]).unwrap();
        o.primary.execute("UPDATE transactions SET created_at = 'garbage'", []).unwrap();
        let report = deltabadger::engine::eligibility::check_install(&o.primary).unwrap();
        assert_eq!(report.unreadable.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![1], "status {status}");
        assert!(matches!(take_over(dir.path(), &o), Err(EngineError::HardStop(m)) if m.contains("This bot's data can't be read.")), "status {status}");
    }
    let (dir, o) = install(&scenario("working bot with a future ActionJob"));
    o.primary.execute("UPDATE bots SET settings = '{not json'", []).unwrap();
    assert!(matches!(take_over(dir.path(), &o), Err(EngineError::HardStop(m)) if m.contains("This bot's data can't be read.")));
    assert_eq!(model::immediate(&o.primary).map(|tx| single_asset::convert_all(&tx, now()).unwrap()).unwrap(), Vec::<i64>::new());
}

// ---- §8 8: check ----

/// `check` prints the would-convert line, and writes nothing.
#[test]
fn check_prints_what_the_next_start_converts_and_writes_nothing() {
    let (dir, o) = install(&scenario("stopped bot"));
    o.primary.execute("UPDATE bots SET label = NULL", []).unwrap();
    let before = rows(&o.primary, "bots");
    drop(o);
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_deltabadger")).arg("check").env("STORAGE_DIR", dir.path()).env("SECRET_KEY_BASE", "engine-test-secret")
        .env("IBKR_CATALOGUE_URL", "off").env_remove("DATABASE_URL").output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Bot 1 will be converted into a one-asset basket at the next start"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    let c = Connection::open(dir.path().join("production.sqlite3")).unwrap();
    assert_eq!(rows(&c, "bots"), before);
    assert_eq!(rows(&c, "bot_index_assets"), json!([]));
}

// ---- §8 7, Q4: the pages of a kept bot ----

mod web {
    use super::*;
    use common::web::{Browser, Csrf};
    use deltabadger::web::session::{self, SessionData};

    const HASH: &str = "$2a$04$abcdefghijklmnopqrstuuKq8n2RkM1bXh0Zc3TtYw5LpJv7dEoGi";

    fn browser(app: &deltabadger::web::App, user: i64, token: &str) -> Browser {
        let data = SessionData { user: Some((user, HASH[..29].into())), csrf: Some(token.into()), ..Default::default() };
        Browser { cookie: Some(session::seal(&app.keys.session, &data, app.now())), page: None }
    }

    /// PATCH /bots/1/start as Turbo submits it.
    async fn start_bot(b: &mut Browser, app: &deltabadger::web::App, token: &str) -> common::web::Answer {
        let token = deltabadger::web::csrf::masked(token);
        b.send_body(app, "PATCH", "/bots/1/start", Some(String::new()), Csrf::None, &[("accept", "text/vnd.turbo-stream.html"), ("x-csrf-token", &token), ("content-type", "application/x-www-form-urlencoded")]).await
    }

    /// A stopped kept single and a skipped clobber repair: the bot page and Start answer 501 with the sentence, a selling one
    /// before the wash-sale question; another user's bot is not found.
    #[tokio::test(flavor = "current_thread")]
    async fn a_kept_bots_page_and_start_say_why() {
        for (name, skip) in [("missing ticker", Skip::Market), ("clobbered basket, repair skipped: unpriced sell", Skip::History("a sale whose proceeds weren't reported"))] {
            let sc = scenario(name);
            let (dir, o) = install(&sc);
            let c = &o.primary;
            take_over(dir.path(), &o).unwrap();
            c.execute("UPDATE bots SET status = 2, transient_data = json_remove(transient_data, '$.rust_set_aside')", []).unwrap();
            // A seller whose owner owes the wash-sale answer: Start would ask it first.
            c.execute("UPDATE bots SET settings = json_set(settings, '$.direction', 'selling')", []).unwrap();
            c.execute("UPDATE users SET encrypted_password = ?1, admin = 1, confirmed_at = '2026-01-01 00:00:00', wash_sale_enabled = NULL", [HASH]).unwrap();
            let sentence = deltabadger::web::i18n::escape(&set_aside::legacy_sentence(c, 1, &skip).unwrap());
            let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::starting(now()));
            let token = deltabadger::web::csrf::new_token();
            let mut b = browser(&app, 1, &token);
            let page = b.get(&app, "/bots/1").await;
            assert_eq!(page.status, 501, "{name}: {:?} {:.600}", page.headers, page.body);
            assert!(page.body.contains(&sentence), "{name}: {:.2000}", page.body);
            let start = start_bot(&mut b, &app, &token).await;
            assert_eq!(start.status, 501, "{name}: {:.600}", start.body);
            assert!(start.body.contains(&sentence), "{name}: {:.2000}", start.body);
            assert_eq!(model::load_bot(c, 1).unwrap().status, deltabadger::enums::BotStatus::Stopped);
            // Another user's bot.
            c.execute("INSERT INTO users (email, encrypted_password, name, admin, confirmed_at, created_at, updated_at) VALUES ('x@example.com', ?1, 'X', 0, '2026-01-01', '2026-01-01', '2026-01-01')", [HASH]).unwrap();
            let mut other = browser(&app, c.last_insert_rowid(), &token);
            let page = other.get(&app, "/bots/1").await;
            assert!(!page.body.contains(&sentence) && page.status != 501, "{name}: {} {:.600}", page.status, page.body);
            let start = start_bot(&mut other, &app, &token).await;
            assert!(!start.body.contains(&sentence) && start.status != 501, "{name}: {} {:.600}", start.status, start.body);
        }
    }
}

/// §8 7 (D-SINGLE-CONDITION-CONVERTS): a condition watching another market reads after the conversion as Rails' single bot
/// read it before and its basket after: unlatched it finds no ticker among the bot's own (not met; a flip doesn't fire);
/// latched "after" it passes before any lookup. No price is read for any of them.
#[tokio::test(flavor = "current_thread")]
async fn a_condition_on_another_market_reads_as_rails_reads_it() {
    use deltabadger::engine::conditions::{self, Rule};
    let mut n = 0;
    for sc in scenarios().into_iter().filter(|s| s["rails_skips"].to_string().contains("condition watches another ticker")) {
        let name = sc["name"].as_str().unwrap();
        let (_d, o) = install(&sc);
        let c = &o.primary;
        assert_eq!(convert(c), vec![1], "{name}");
        let bot = model::load_bot(c, 1).unwrap();
        let rails = sc["probes"]["1"]["after"]["conditions"].as_object().unwrap().clone();
        assert_eq!(rails, sc["probes"]["1"]["before"]["conditions"].as_object().unwrap().clone(), "{name}: Rails reads it alike before and after");
        for (prefix, met) in rails {
            let selling = prefix.starts_with("sell_");
            let (kind, _) = Rule::of_flag(&format!("{prefix}ed")).unwrap();
            // evaluate_rule's reading before any venue call: a latched "after" rule is met; else its ticker among the bot's own.
            let rule = bot.condition_rule(kind, selling).unwrap().unwrap().common;
            let rust = (rule.after && rule.met_at_us.is_some()) || {
                assert!(conditions::rule_ticker(c, &bot, &rule).unwrap().is_none(), "{name}: {prefix} finds no ticker of its own");
                false
            };
            assert_eq!(json!(rust), met, "{name}: {prefix}");
            n += 1;
        }
    }
    assert_eq!(n, 10);
}

// ---- §8 5: the figures each surface serves, before and after ----

mod surfaces {
    use super::*;
    use axum::{body::{to_bytes, Body}, http::Request};
    use deltabadger::web::{figure, App};
    use tower::ServiceExt;

    const TOOLS: [&str; 2] = ["get_bot_details", "get_portfolio_summary"];

    async fn send(app: &App, method: &str, path: &str, token: &str, body: Option<Value>, session: Option<&str>) -> (u16, Option<String>, String) {
        let mut request = Request::builder().method(method).uri(path).header("host", "localhost:3000").header("authorization", format!("Bearer {token}"))
            .header("accept", "application/json, text/event-stream");
        if body.is_some() { request = request.header("content-type", "application/json"); }
        if let Some(session) = session { request = request.header("mcp-session-id", session).header("mcp-protocol-version", "2025-11-25"); }
        let response = deltabadger::web::router(app.clone()).oneshot(request.body(Body::from(body.map(|b| b.to_string()).unwrap_or_default())).unwrap()).await.unwrap();
        let status = response.status().as_u16();
        let session = response.headers().get("mcp-session-id").map(|v| v.to_str().unwrap().to_string());
        (status, session, String::from_utf8(to_bytes(response.into_body(), 64_000_000).await.unwrap().to_vec()).unwrap())
    }

    async fn mcp(app: &App, name: &str, args: Value) -> Value {
        let init = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"a23","version":"1"}}});
        let (_, session, _) = send(app, "POST", "/mcp", "a23-mcp", Some(init), None).await;
        let session = session.unwrap();
        assert_eq!(send(app, "POST", "/mcp", "a23-mcp", Some(json!({"jsonrpc":"2.0","method":"notifications/initialized"})), Some(&session)).await.0, 202);
        let (_, _, body) = send(app, "POST", "/mcp", "a23-mcp", Some(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}})), Some(&session)).await;
        serde_json::from_str::<Value>(&body).unwrap()["result"].clone()
    }

    async fn rest(app: &App, path: &str) -> Value {
        let (status, _, body) = send(app, "GET", path, "a23-api", None, None).await;
        assert_eq!(status, 200, "{path}: {body}");
        serde_json::from_str(&body).unwrap()
    }

    fn grant(c: &Connection) {
        c.execute("INSERT INTO oauth_applications(name,uid,redirect_uri,confidential,scopes,created_at,updated_at) VALUES ('a23','a23','http://localhost/cb',0,'mcp api','2026-09-01 12:00:00','2026-09-01 12:00:00')", []).unwrap();
        let application = c.last_insert_rowid();
        for (token, scope) in [("a23-mcp", "mcp"), ("a23-api", "api")] {
            c.execute("INSERT INTO oauth_access_tokens(application_id,resource_owner_id,token,scopes,created_at,expires_in) VALUES (?1,1,?2,?3,'2026-09-10 11:00:00',86400)", (application, token, scope)).unwrap();
        }
        c.execute("INSERT INTO connected_clients(user_id,oauth_application_id,mcp_tools,rest_tools,created_at,updated_at) VALUES (1,?1,NULL,NULL,'2026-09-01 12:00:00','2026-09-01 12:00:00')", [application]).unwrap();
        let on: serde_json::Map<String, Value> = TOOLS.iter().map(|t| (t.to_string(), json!(true))).collect();
        c.execute("UPDATE users SET mcp_settings=?1,rest_settings=?1", [json!({"tool_permissions": on}).to_string()]).unwrap();
    }

    /// Kraken's public market at 120 EUR a bitcoin: its ticker and one hourly candle.
    struct Kraken;
    impl deltabadger::venue::http::Transport for Kraken {
        async fn send(&self, r: &deltabadger::venue::http::HttpRequest) -> Result<deltabadger::venue::http::HttpResponse, deltabadger::venue::http::TransportError> {
            let pair = json!({ "a": ["120.0", "1", "1.000"], "b": ["120.0", "1", "1.000"], "c": ["120.0", "0.1"], "v": ["1", "1"], "p": ["120.0", "120.0"], "t": [1, 1], "l": ["120.0", "120.0"], "h": ["120.0", "120.0"], "o": "120.0" });
            let body = match r.path.as_str() {
                "/0/public/Ticker" => json!({ "error": [], "result": { "XXBTZEUR": pair, "XBTEUR": pair } }),
                "/0/public/OHLC" => {
                    let t = now().timestamp() / 3600 * 3600 - 3600;
                    json!({ "error": [], "result": { "XXBTZEUR": [[t, "120.0", "120.0", "120.0", "120.0", "120.0", "1", 1]], "last": t } })
                }
                other => panic!("unscripted {other}"),
            };
            Ok(deltabadger::venue::http::HttpResponse { status: 200, body: body.to_string() })
        }
    }

    /// What each surface serves for bot 1.
    async fn served(app: &App, c: &Connection) -> Value {
        use deltabadger::figures::page_market::{Cache, Reader};
        let at = At::from_utc(now()).unwrap();
        let mut cache = Cache::default();
        for _ in 0..4 {
            let reader = Reader::new(&cache, now().timestamp());
            let _ = figure::account(c, 1, &reader, at, "en", "", "");
            let demands = reader.demands();
            if demands.is_empty() { break; }
            cache.fill(&Kraken, demands, now().timestamp()).await;
        }
        let page = figure::account(c, 1, &Reader::new(&cache, now().timestamp()), at, "en", "", "").unwrap();
        json!({
            "figure": page,
            "mcp": mcp(app, "get_bot_details", json!({"bot_id": 1})).await,
            "rest": rest(app, "/api/v1/bots/1").await,
        })
    }

    /// The numbers in an HTML fragment, in order, tags removed.
    fn numbers(html: &str) -> Vec<f64> {
        let text = regex::Regex::new("<[^>]*>").unwrap().replace_all(html, " ");
        regex::Regex::new(r"[+-]?\d+(?:\.\d+)?").unwrap().find_iter(&text).map(|m| m.as_str().parse().unwrap()).collect()
    }
    /// The first number after `label` (case-insensitive) in the page's metrics panel.
    fn labelled(html: &str, label: &str) -> f64 {
        let at = html.to_lowercase().find(&label.to_lowercase()).unwrap_or_else(|| panic!("no {label}"));
        numbers(&html[at + label.len()..])[0]
    }
    /// The asset table's BTC row: (amount, average price, value, P/L %).
    fn row(html: &str) -> (f64, f64, f64, f64) {
        let row = html.split("<tr data-symbol=\"BTC\">").nth(1).unwrap().split("</tr>").next().unwrap();
        let n = numbers(row);
        (n[0], n[1], n[2], n[3])
    }

    /// §8 5: one converted bot with consistent fills (a buy of 1 BTC for 100, a sale of 0.5 for 55; the market at 120). The
    /// money each surface serves is the same before (Bots::DcaSingleAsset) and after (the basket): the web figures' tile,
    /// invested, portfolio value, and the BTC row's amount and average price; the five money fields of the MCP and REST bot
    /// reads (one code path: mcp::reads Details; walk figures at the last fill).
    /// Different by design, as in Rails after its own conversion: the basket's BTC row is the holding alone (value 0.5 × 120 =
    /// 60, P/L on its 50 cost) with the realised gain on its own line (+5), where the pair's row carries the whole bot (115,
    /// +15%); the reads' type and composition fields (type, allocations, allocation_assets, holdings, exited_holdings,
    /// redeploy_offer, the MCP text's Allocations/Members/Holdings lines). The tracker serves no per-bot figures: its REST
    /// reads are account transactions, which never read a bot's class, and the portfolio summary answers 501 off Alpaca.
    #[tokio::test(flavor = "current_thread")]
    async fn each_surface_serves_the_same_money_before_and_after() {
        let sc = scenario("stopped bot");
        let (dir, o) = install(&sc);
        let c = &o.primary;
        grant(c);
        let app = common::web::app(dir.path(), "engine-test-secret", common::web::TestClock::starting(now()))
            .with_figure_source(deltabadger::web::figure::loading::Source::Script(json!({}))).unwrap();
        let before = served(&app, c).await;
        assert_eq!(convert(c), vec![1]);
        let after = served(&app, c).await;

        // The web figures.
        let tile = |s: &Value| numbers(s["figure"]["bots"]["1"]["tile"].as_str().unwrap())[0];
        let panel = |s: &Value| s["figure"]["bots"]["1"]["metrics"].as_str().unwrap().to_string();
        let (b, a) = (panel(&before), panel(&after));
        assert_eq!((tile(&before), tile(&after)), (15.0, 15.0), "the tile's P/L");
        assert_eq!((labelled(&b, "Total invested"), labelled(&a, "Total invested")), (100.0, 100.0));
        assert_eq!((labelled(&b, "Portfolio Value"), labelled(&a, "Portfolio Value")), (115.0, 115.0));
        let (rb, ra) = (row(&b), row(&a));
        assert_eq!((rb.0, rb.1), (ra.0, ra.1), "the row's amount and average price");
        assert_eq!((rb.2, rb.3), (115.0, 15.0), "the pair's row: the whole bot");
        // The basket's row, as Rails' recorded basket walk after its conversion holds it.
        let rails = &sc["probes"]["1"]["after"]["metrics"];
        let held = &rails["asset_breakdown"]["BTC"];
        let num = |v: &Value| v.as_str().map_or_else(|| v.as_f64().unwrap(), |s| s.parse().unwrap());
        assert_eq!((num(&held["amount"]), num(&held["quote_invested"]), num(&rails["realised_pnl"])), (0.5, 50.0, 5.0));
        assert_eq!(ra.0, num(&held["amount"]));
        assert_eq!(ra.1, num(&held["quote_invested"]) / num(&held["amount"]), "average price = invested / amount");
        assert_eq!(ra.2, num(&held["amount"]) * 120.0, "the holding at the market");
        assert_eq!(ra.3, 20.0, "P/L on the holding's cost: 60 / 50 - 1");
        assert_eq!(labelled(&a, "Realised P/L"), num(&rails["realised_pnl"]));
        assert!(!b.contains("Realised P/L"));

        // The MCP and REST bot reads.
        for surface in ["rest", "mcp"] {
            let metrics = |s: &Value| if surface == "rest" { s["rest"]["data"]["metrics"].clone() } else {
                let text = s["mcp"]["content"][0]["text"].as_str().unwrap().to_string();
                let line = |l: &str| numbers(text.lines().find(|x| x.starts_with(l)).unwrap_or_else(|| panic!("{l}")))[0];
                json!({ "invested": line("Total invested"), "value": line("Current value"), "pnl": line("P/L"), "average_buy_price": line("Average buy price"), "total_base_amount": line("Total acquired") })
            };
            let (mb, ma) = (metrics(&before), metrics(&after));
            for key in ["invested", "value", "pnl", "average_buy_price", "total_base_amount"] {
                assert!(!mb[key].is_null(), "{surface} {key}");
                assert_eq!(num(&mb[key]), num(&ma[key]), "{surface} {key}");
            }
        }
        assert_eq!(before["rest"]["data"]["type"], "Bots::DcaSingleAsset");
        assert_eq!(after["rest"]["data"]["type"], "Bots::DcaMultiAsset");
    }
}
