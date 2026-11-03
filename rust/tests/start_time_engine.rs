//! The engine running a bot whose starting time is on, against Rails' answers (fixtures/start_time_vectors.json): the
//! amount it owes around the delayed first run, Bot::Startable#disable_starting_time!'s writes after it, the run's timing,
//! and which start-time states it refuses.
mod common;
use chrono::{DateTime, Utc};
use common::seed::{self, BotSpec};
use deltabadger::engine::run::{self, Engine};
use deltabadger::engine::{amount, eligibility, model, FixedClock};
use deltabadger::lease;
use deltabadger::ruby::BigDec;
use deltabadger::store::{self, Paths};
use deltabadger::venue::fake::{FakeFactory, FakeVenue};
use rusqlite::Connection;
use serde_json::{json, Value};

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/start_time_vectors.json")).unwrap() }
fn time(s: &Value) -> DateTime<Utc> { s.as_str().unwrap().parse().unwrap() }
fn dec(s: &str) -> BigDec { BigDec::parse(s).unwrap() }

/// The bot exactly as Bot::Lifecycle#start left it in Rails.
fn spec(v: &Value) -> BotSpec {
    let started = &v["started"];
    let mut settings = json!({ "interval": v["interval"], "quote_amount": 60.0 });
    for (k, x) in v["settings"].as_object().unwrap().iter().chain(started["settings"].as_object().unwrap()) { settings[k] = x.clone(); }
    BotSpec { status: started["status"].as_i64().unwrap(), started_at: started["started_at"].as_str().map(str::to_owned),
              settings_changed_at: started["settings_changed_at"].as_str().map(str::to_owned), settings, transient: started["transient_data"].clone() }
}

/// The bot as Rails left it, its owner in the vector's zone.
fn insert(c: &Connection, s: &seed::Seeded, v: &Value) -> i64 {
    c.execute("UPDATE users SET time_zone = ?1", [v["zone"].as_str().unwrap()]).unwrap();
    seed::insert_bot(c, s, &spec(v))
}

fn stored(c: &Connection, id: i64) -> (Value, Value, String) {
    c.query_row("SELECT settings, transient_data, settings_changed_at FROM bots WHERE id = ?1", [id],
                |r| Ok((serde_json::from_str(&r.get::<_, String>(0)?).unwrap(), serde_json::from_str(&r.get::<_, String>(1)?).unwrap(), r.get(2)?))).unwrap()
}

#[test]
fn the_amount_and_the_disable_match_rails() {
    let (mut reads, mut disables) = (0, 0);
    for v in vectors()["schedule"].as_array().unwrap() {
        let (_d, o, s) = common::install();
        let c = &o.primary;
        let id = insert(c, &s, v);
        let pending = |at: DateTime<Utc>| amount::pending_quote_amount(c, &model::load_bot(c, id).unwrap(), at.timestamp_micros()).unwrap();
        for read in v["reads"].as_array().unwrap() {
            assert_eq!(pending(time(&read["now"])), dec(read["pending"].as_str().unwrap()), "{read}");
            reads += 1;
        }
        let Some(bought) = v["bought"].as_str() else { continue };
        let t0: DateTime<Utc> = deltabadger::codec::parse_time(v["started"]["started_at"].as_str().unwrap()).unwrap();
        // The first run's buy, as the recorder wrote it: closed, or (`open`) still open when the rule turns off.
        let open = v["open"] == json!(true);
        c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, quote_amount, quote_amount_exec, \
                   amount_exec, price, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
                   VALUES (?1, ?2, 'OSTART-1', 0, ?8, 0, 0, '60', ?3, ?9, '50000', 'BTC', 'EUR', ?4, ?5, ?6, 60, 'REGULAR', '[]', ?7, ?7)",
                  rusqlite::params![id, s.exchange_id, (!open).then_some(bought), s.btc, s.quote, v["interval"].as_str(),
                                    deltabadger::codec::format_time(t0 + chrono::Duration::milliseconds(500)),
                                    if open { 1 } else { 2 }, (!open).then_some("0.001")]).unwrap();
        amount::disable_starting_time(c, id, time(&v["disabled_at"])).unwrap();
        let (settings, transient, changed) = stored(c, id);
        let rails = &v["disabled"];
        assert_eq!(settings["start_time_enabled"], json!(false), "{v}");
        assert_eq!(transient, rails["transient_data"], "the carry and its marker, as Rails stores them: {v}");
        assert_eq!(changed, rails["settings_changed_at"].as_str().unwrap(), "the window stays where the start put it: {v}");
        if open {
            // Cancelled after the rule turned off, having filled `bought`: what it never filled is owed with the next run.
            c.execute("UPDATE transactions SET external_status = 3, quote_amount_exec = ?2 WHERE bot_id = ?1", rusqlite::params![id, bought]).unwrap();
        }
        assert_eq!(pending(time(&v["second"]["now"])), dec(v["second"]["pending"].as_str().unwrap()), "the next run's amount: {v}");
        // A second call is a no-op: the rule is off.
        amount::disable_starting_time(c, id, time(&v["second"]["now"])).unwrap();
        assert_eq!(stored(c, id).2, changed);
        disables += 1;
    }
    assert_eq!((reads, disables), (192, 36));
}

fn engine(v: &Value, venue: FakeVenue) -> (tempfile::TempDir, Engine<FakeFactory>, i64) {
    let dir = common::baseline_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-01-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = insert(&o.primary, &s, v);
    (dir, Engine::new(o.primary, FakeFactory(venue), seed::cipher(), lock), id)
}

/// Nothing before the start; at the first pass after it, one order for the amount Rails owes then; the rule then off, and
/// the next order at the next checkpoint, not before.
#[tokio::test(flavor = "current_thread")]
async fn the_first_run_waits_for_the_start_and_then_turns_the_rule_off() {
    let all = vectors();
    for v in all["schedule"].as_array().unwrap().iter().filter(|v| v["bought"].is_null()) {
        // Every stored start runs at its stored instant, Rails' own: one Rails moved off the chosen wall time by its fixed-day
        // step, a spring-gap time, a repeated wall time.
        let venue = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0");
        let (_d, mut e, id) = engine(v, venue.clone());
        let t0 = time(&v["wait_until"]);
        let us = |d: i64| FixedClock(t0 + chrono::Duration::microseconds(d));
        assert!(eligibility::check_install(&e.primary).unwrap().eligible.contains(&id), "{v}");
        let wake = run::step(&mut e, &us(-1)).await.unwrap();
        assert!(venue.sent().is_empty(), "never before the start: {v}");
        let (settings, transient, _) = stored(&e.primary, id);
        assert!(settings["start_time_enabled"] == json!(true) && transient.get("last_action_job_at").is_none_or(Value::is_null),
                "no run at all before the start (a run would also turn the rule off): {v}");
        assert!(wake >= t0.timestamp_micros() && wake <= t0.timestamp_micros() + 5_000_000, "it wakes at or just after the start: wake {wake} t0 {t0}: {v}");
        let first = &v["reads"][3]; // T0 + 0.5 s
        run::step(&mut e, &FixedClock(time(&first["now"]))).await.unwrap();
        let owed = dec(first["pending"].as_str().unwrap());
        if owed == BigDec::zero() {
            // Rails' month clamp: a start on the 31st owes nothing at its first run (see the vectors), and buys nothing.
            assert!(venue.sent().is_empty(), "{v}");
        } else {
            assert_eq!(venue.sent().len(), 1, "{v}");
            let order = &venue.sent()[0];
            let quote = match (&order.kind, order.quote_volume) {
                (_, true) => dec(&order.volume),
                (deltabadger::venue::OrderKind::Limit { price }, false) => &dec(&order.volume) * &dec(price),
                (_, false) => &dec(&order.volume) * &dec("50000.0"), // a market buy sized in base at the ask
            };
            assert!(&owed - &quote < dec("0.001") && quote <= owed, "the order is the amount Rails owes: {quote:?} for {owed:?}");
        }
        assert_eq!(stored(&e.primary, id).0["start_time_enabled"], json!(false), "the run turns the rule off: {v}");
        let next = time(&first["next"]);
        run::step(&mut e, &FixedClock(next - chrono::Duration::seconds(1))).await.unwrap();
        assert!(venue.sent().len() <= 1, "nothing more before the next checkpoint: {v}");
    }
}

// ---- A22: every start-time state Rails can leave runs (D-START-ANCHOR-STARTED, D-START-CONTINUE-WAITS) ----

fn edge(name: &str) -> Value {
    vectors()["edges"].as_array().unwrap().iter().find(|e| e["name"] == name).unwrap_or_else(|| panic!("no edge {name}")).clone()
}
fn step_at(e: &Value, label: &str) -> usize {
    e["steps"].as_array().unwrap().iter().position(|s| s["step"] == label).unwrap_or_else(|| panic!("{}: no step {label}", e["name"]))
}

/// The bot exactly as a Rails step left it.
fn edge_spec(e: &Value, step: &Value) -> BotSpec {
    let row = &step["row"];
    let mut settings = json!({ "interval": e["interval"], "quote_amount": 60.0 });
    for (k, x) in e["settings"].as_object().unwrap().iter().chain(row["settings"].as_object().unwrap()) { settings[k] = x.clone(); }
    BotSpec { status: row["status"].as_i64().unwrap(), started_at: row["started_at"].as_str().map(str::to_owned),
              settings_changed_at: row["settings_changed_at"].as_str().map(str::to_owned), settings, transient: row["transient_data"].clone() }
}

fn buy(c: &Connection, s: &seed::Seeded, id: i64, interval: &str, at: DateTime<Utc>, quote: &str, open: bool) {
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, external_status, side, order_type, quote_amount, quote_amount_exec, \
               amount_exec, price, base, quote, base_asset_id, quote_asset_id, bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
               VALUES (?1, ?2, ?3, 0, ?4, 0, 0, ?5, ?6, ?7, '50000', 'BTC', 'EUR', ?8, ?9, ?10, 60, 'REGULAR', '[]', ?11, ?11)",
              rusqlite::params![id, s.exchange_id, format!("OEDGE-{}", at.timestamp_micros()), if open { 1 } else { 2 }, quote, (!open).then_some(quote),
                                (!open).then_some("0.001"), s.btc, s.quote, interval, deltabadger::codec::format_time(at)]).unwrap();
}

/// The bot and the orders Rails' edge holds after `upto` (that step included): each run's buy 1/8 s after it, open or closed,
/// and a recorded cancellation at 15 filled.
fn edge_state(c: &Connection, s: &seed::Seeded, e: &Value, upto: usize) -> i64 {
    let steps = e["steps"].as_array().unwrap();
    c.execute("UPDATE users SET time_zone = ?1", [e["zone"].as_str().unwrap()]).unwrap();
    let id = seed::insert_bot(c, s, &edge_spec(e, &steps[upto]));
    for st in &steps[..=upto] {
        if st["step"] == "run" && dec(st["bought"].as_str().unwrap()) > BigDec::zero() {
            buy(c, s, id, e["interval"].as_str().unwrap(), time(&st["now"]) + chrono::Duration::microseconds(125_000), st["bought"].as_str().unwrap(), st["open"] == json!(true));
        }
        if st["step"] == "cancelled 15 of 60" {
            c.execute("UPDATE transactions SET external_status = 3, quote_amount_exec = '15' WHERE bot_id = ?1", [id]).unwrap();
        }
    }
    id
}

/// Rails' carry at every recorded step, wherever Rails' anchor is the bot's started_at (the rule off, or start_at equal to
/// started_at): there the two agree to the digit. Where Rails reads a start_at that differs, Rust reads started_at, and the
/// carry is the unmoved bot's (pinned by the replays below).
#[test]
fn the_edge_carry_matches_rails_wherever_rails_anchors_on_started_at() {
    let (mut same, mut moved) = (0, 0);
    for e in vectors()["edges"].as_array().unwrap() {
        for (i, st) in e["steps"].as_array().unwrap().iter().enumerate() {
            let row = &st["row"];
            let started = row["started_at"].as_str().map(|t| deltabadger::codec::parse_time(t).unwrap());
            let start_at = row["settings"]["start_at"].as_str().map(|t| deltabadger::codec::parse_time(t).unwrap());
            let (_d, o, s) = common::install();
            let id = edge_state(&o.primary, &s, e, i);
            let bot = model::load_bot(&o.primary, id).unwrap();
            assert!(eligibility::check_install(&o.primary).unwrap().problems.is_empty(), "{} {}: no start-time refusal", e["name"], st["step"]);
            let owed = amount::pending_quote_amount(&o.primary, &bot, time(&st["now"]).timestamp_micros()).unwrap();
            if row["settings"]["start_time_enabled"] == json!(true) && start_at.is_some() && start_at != started { moved += 1; continue; }
            assert_eq!(owed, dec(st["pending"].as_str().unwrap()), "{} {}", e["name"], st["step"]);
            same += 1;
        }
    }
    assert!(same > 100 && moved > 25, "{same} agreeing, {moved} moved");
}

fn venue() -> FakeVenue {
    FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("ZEUR", "100000", "0")
}

/// The engine on Rails' row after `label`, optionally continued through Rust (the web's `rust_continue_start`, status
/// scheduled) at `continued`.
fn edge_engine(e: &Value, label: &str, continued: Option<DateTime<Utc>>, venue: FakeVenue) -> (tempfile::TempDir, Engine<FakeFactory>, i64) {
    let dir = common::baseline_install();
    let p = Paths::from_env(&|_| None, dir.path());
    let lock = lease::lock(&p, "2026-01-01T00:00:00Z".parse().unwrap()).unwrap();
    let o = store::open(&p).unwrap();
    let s = seed::seed_kraken(&o.primary, &seed::cipher());
    let id = edge_state(&o.primary, &s, e, step_at(e, label));
    if let Some(at) = continued {
        o.primary.execute("UPDATE bots SET status = 1, transient_data = json_set(transient_data, '$.rust_continue_start', json(?2)) WHERE id = ?1",
                          rusqlite::params![id, json!({ "requested_at": deltabadger::codec::format_time(at), "was_stopped": true }).to_string()]).unwrap();
    }
    (dir, Engine::new(o.primary, FakeFactory(venue), seed::cipher(), lock), id)
}

fn quote_of(order: &deltabadger::venue::NewOrder) -> BigDec {
    if order.quote_volume { dec(&order.volume) } else { &dec(&order.volume) * &dec("50000.0") }
}

/// Steps the engine at each time and returns, per time, the quote of what it sent then (zero for nothing).
async fn replay(e: &mut Engine<FakeFactory>, venue: &FakeVenue, times: &[DateTime<Utc>]) -> Vec<BigDec> {
    let mut out = vec![];
    for t in times {
        let before = venue.sent().len();
        run::step(e, &FixedClock(*t)).await.unwrap();
        let sent = venue.sent();
        assert!(sent.len() <= before + 1, "at most one order per pass at {t}");
        out.push(sent.get(before).map_or(BigDec::zero(), quote_of));
    }
    out
}

fn amounts(v: &[&str]) -> Vec<BigDec> { v.iter().map(|a| dec(a)).collect() }
fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }

/// D-START-ANCHOR-STARTED on a continue: start_at moved three days later while stopped, continued inside the interval
/// through Rust. Rails bought 120 at once (intervals counted up to the future start_at); Rust waits for the next checkpoint
/// on started_at's grid and buys one amount there, then turns the rule off with the flag alone.
#[tokio::test(flavor = "current_thread")]
async fn a_continue_with_start_at_moved_later_waits_on_started_ats_grid() {
    let e = edge("continue after inside");
    let c = t("2026-09-10T12:30:00Z");
    let v = venue();
    let (_d, mut en, id) = edge_engine(&e, "start_at form", Some(c), v.clone());
    let got = replay(&mut en, &v, &[c, t("2026-09-11T07:29:59Z"), t("2026-09-11T07:30:00.250Z"), t("2026-09-12T07:29:59Z")]).await;
    assert_eq!(got, amounts(&["0", "0", "60", "0"]), "Rails: 120 at once");
    assert_eq!(stored(&en.primary, id).0["start_time_enabled"], json!(false));
}

/// start_at moved a day earlier (25 h) while stopped: Rails ran on the moved grid an hour before started_at's checkpoint and
/// again at it (60 twice an hour apart); Rust buys once, at started_at's checkpoint.
#[tokio::test(flavor = "current_thread")]
async fn a_continue_with_start_at_moved_earlier_buys_once_per_interval() {
    let e = edge("continue before inside");
    let c = t("2026-09-10T12:30:00Z");
    let v = venue();
    let (_d, mut en, _id) = edge_engine(&e, "start_at form", Some(c), v.clone());
    let got = replay(&mut en, &v, &[c, t("2026-09-11T06:30:00.250Z"), t("2026-09-11T07:30:00.250Z"), t("2026-09-11T08:30:00Z")]).await;
    assert_eq!(got, amounts(&["0", "0", "60", "0"]));
}

/// D-START-CONTINUE-WAITS: a delayed start stopped before its time and continued before it, start_at moved a day later.
/// Rails bought 60 at once and 60 again at the start; Rust waits for started_at and buys once. The rule stays on until then.
#[tokio::test(flavor = "current_thread")]
async fn a_continue_before_the_start_waits_for_it() {
    for (name, label) in [("never ran continue before shift 86400", "start_at form"), ("never ran continue before shift 0", "stop")] {
        let e = edge(name);
        let c = t("2026-09-10T07:00:00Z");
        let v = venue();
        let (_d, mut en, id) = edge_engine(&e, label, Some(c), v.clone());
        let got = replay(&mut en, &v, &[c, t("2026-09-10T07:29:59Z")]).await;
        assert_eq!(got, amounts(&["0", "0"]), "{name}: nothing before the start");
        assert_eq!(stored(&en.primary, id).0["start_time_enabled"], json!(true), "{name}: the rule waits with the bot");
        let got = replay(&mut en, &v, &[t("2026-09-10T07:30:00.250Z"), t("2026-09-11T07:29:00Z")]).await;
        assert_eq!(got, amounts(&["60", "0"]), "{name}");
        assert_eq!(stored(&en.primary, id).0["start_time_enabled"], json!(false), "{name}");
    }
}

/// D-START-CONTINUE-WAITS for a seller (a sale is sized from its sell amount, not the carry): continued half an hour before
/// its start, Rails sold at once and again at the start. Rust sells once, at the start; the frozen buy carry stays 30. The
/// same for a Rails row left retrying before the start (no continue marker, no Rust wait), across an engine restart too.
#[tokio::test(flavor = "current_thread")]
async fn a_seller_continued_or_retrying_before_its_start_sells_once_at_it() {
    use deltabadger::venue::Side;
    for denomination in ["base", "quote"] {
        let e = edge(&format!("selling never ran continue before {denomination}"));
        for how in ["continue", "retrying", "retrying after a restart"] {
            let c = t("2026-09-10T07:00:00Z");
            // One balance read is queued per sale: an early sale would take it, and the start's would find none.
            let v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0").balance_body("XBT", "1", "0");
            let (d, mut en, id) = edge_engine(&e, "stop", (how == "continue").then_some(c), v.clone());
            if how != "continue" { en.primary.execute("UPDATE bots SET status = 5 WHERE id = ?1", [id]).unwrap(); }
            for at in [c, t("2026-09-10T07:15:00Z"), t("2026-09-10T07:29:59Z")] { run::step(&mut en, &FixedClock(at)).await.unwrap(); }
            assert!(v.sent().is_empty(), "{denomination} {how}: no sale before the start");
            let r = eligibility::check_install(&en.primary).unwrap();
            assert!(r.eligible.contains(&id), "{denomination} {how}: {:?} {:?}", r.problems, r.unreadable);
            if how == "retrying after a restart" {
                let lock = en.lock; drop(en.primary);
                let p = Paths::from_env(&|_| None, d.path());
                en = Engine::new(store::open(&p).unwrap().primary, FakeFactory(v.clone()), seed::cipher(), lock);
            }
            for at in [t("2026-09-10T07:30:00.250Z"), t("2026-09-10T08:00:00Z"), t("2026-09-11T07:29:00Z")] { run::step(&mut en, &FixedClock(at)).await.unwrap(); }
            let sent = v.sent();
            assert_eq!(sent.len(), 1, "{denomination} {how}: one sale, at the start: {sent:?}");
            assert_eq!(sent[0].side, Side::Sell);
            assert_eq!(stored(&en.primary, id).1["missed_quote_amount"], json!("30.0"), "{denomination} {how}: the frozen buy carry");
        }
    }
}

/// start_at edited on a running bot before its first run: Rails' flip-off captured over the open first order and the 45 it
/// never filled were never owed again (60 next). Rust turns the rule off with the flag alone, so the next run owes them:
/// 105, the unmoved bot's answer (schedule[29]).
#[tokio::test(flavor = "current_thread")]
async fn a_start_at_moved_on_a_running_bot_keeps_the_first_orders_window() {
    for (name, first, second) in [("moved day 93600 open", "60", "105"), ("moved day -90000 filled", "60", "60"), ("moved month 93600 filled", "60", "0")] {
        let e = edge(name);
        let v = venue();
        let (_d, mut en, id) = edge_engine(&e, "before first run", None, v.clone());
        let changed = stored(&en.primary, id).2;
        let got = replay(&mut en, &v, &[t("2026-09-10T07:29:59Z"), t("2026-09-10T07:30:00.250Z")]).await;
        assert_eq!(got, amounts(&["0", first]), "{name}: the first run at started_at (Rails owed 0 there for an earlier start_at or a month)");
        let (settings, _, after) = stored(&en.primary, id);
        assert_eq!((settings["start_time_enabled"].clone(), after), (json!(false), changed), "{name}: flag only, the window stays");
        if name.ends_with("open") {
            en.primary.execute("UPDATE transactions SET external_status = 3, quote_amount_exec = '15', amount_exec = '0.0003' WHERE bot_id = ?1", [id]).unwrap();
        } else {
            en.primary.execute("UPDATE transactions SET external_status = 2, quote_amount_exec = quote_amount, amount_exec = '0.0012' WHERE bot_id = ?1", [id]).unwrap();
        }
        let next = if name.contains("month") { t("2026-10-10T07:30:01Z") } else { t("2026-09-11T07:30:01Z") };
        let owed = amount::pending_quote_amount(&en.primary, &model::load_bot(&en.primary, id).unwrap(), next.timestamp_micros()).unwrap();
        assert_eq!(owed, dec(second), "{name}: the next run");
    }
}

/// A run that left the rule on (Rails' ambiguous or suppressed tick) with start_at moved: Rails scheduled the next run on
/// the moved grid (2 h later); Rust runs on started_at's, once.
#[tokio::test(flavor = "current_thread")]
async fn a_kept_rule_with_a_moved_start_at_runs_on_started_ats_grid() {
    let e = edge("moved kept on");
    let v = venue();
    let (_d, mut en, _id) = edge_engine(&e, "after kept run", None, v.clone());
    let got = replay(&mut en, &v, &[t("2026-09-10T08:00:00Z"), t("2026-09-11T07:30:00.250Z"), t("2026-09-11T09:30:00.250Z")]).await;
    assert_eq!(got, amounts(&["0", "60", "0"]));
}

/// Carry Rails already stored from a moved start_at (a second settings edit captured the phantom 120, capped at 60) is
/// Rails' own record and is kept (Q3): continued through Rust inside the interval, the bot owes one amount and runs now;
/// taken over without a continue marker, it waits for the next checkpoint and pays it there with the interval's amount.
#[tokio::test(flavor = "current_thread")]
async fn a_carry_rails_stored_from_a_moved_start_at_is_spent_as_stored() {
    let e = edge("continue after inside second edit");
    let c = t("2026-09-10T12:30:00Z");
    let v = venue();
    let (_d, mut en, _id) = edge_engine(&e, "continue", Some(c), v.clone());
    assert_eq!(replay(&mut en, &v, &[c, t("2026-09-11T07:30:00.250Z")]).await, amounts(&["60", "60"]), "Rails: 180, then 60");
    let v = venue();
    let (_d, mut en, _id) = edge_engine(&e, "continue", None, v.clone());
    assert_eq!(replay(&mut en, &v, &[c, t("2026-09-11T07:30:00.250Z")]).await, amounts(&["0", "120"]));
}

/// A working bot whose start_time_mode is none of MODES (written past the form) runs as Rails does: the mode is read only
/// at a start.
#[tokio::test(flavor = "current_thread")]
async fn an_unknown_mode_on_a_working_bot_runs() {
    let e = edge("unknown mode");
    let v = venue();
    let (_d, mut en, id) = edge_engine(&e, "mode noon", None, v.clone());
    assert_eq!(replay(&mut en, &v, &[t("2026-09-10T07:29:59Z"), t("2026-09-10T07:30:00.250Z")]).await, amounts(&["0", "60"]));
    assert_eq!(stored(&en.primary, id).0["start_time_enabled"], json!(false));
}

/// Rows the old gate refused now run as stored, whatever the zone, the stored instant or the time of day say: a start Rails'
/// fixed-day step stored an hour off (Rails' own instant, never moved), a repeated wall time in Dublin, an unknown zone (no
/// app path writes one; a started bot reads no zone). An unreadable start_at stays refused as unreadable data.
#[test]
fn every_formerly_refused_row_is_eligible() {
    let v = &vectors()["schedule"][6]; // Warsaw, every day at 09:30
    for (zone, edit) in [
        ("Warsaw", "UPDATE bots SET started_at = '2026-10-25 07:30:00', settings = json_set(settings, '$.start_at', '2026-10-25T07:30:00Z') WHERE id = ?1"),
        ("Europe/Dublin", "UPDATE bots SET started_at = '2026-10-25 00:30:00', settings = json_set(settings, '$.start_at', '2026-10-25T00:30:00Z', '$.start_time_of_day', '01:30') WHERE id = ?1"),
        ("Mars/Olympus", "UPDATE bots SET settings = json_set(settings, '$.start_time_mode', 'date') WHERE id = ?1"),
        ("Warsaw", "UPDATE bots SET settings = json_set(settings, '$.start_at', '2026-09-12T07:30:00Z') WHERE id = ?1"),
        ("Warsaw", "UPDATE bots SET transient_data = json_set(transient_data, '$.rust_continue_start', json('{\"requested_at\":\"2026-09-10T05:00:00Z\"}')) WHERE id = ?1"),
    ] {
        let (_d, o, s) = common::install();
        let id = insert(&o.primary, &s, v);
        o.primary.execute("UPDATE users SET time_zone = ?1", [zone]).unwrap();
        o.primary.execute(edit, [id]).unwrap();
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(r.eligible.contains(&id), "{zone} {edit}: {:?} {:?}", r.problems, r.unreadable);
    }
    // An unreadable start_at is unreadable data, as every stored timestamp is (codec::validate_bot_times): refused as such,
    // never guessed. Rails parses it to nil and runs from started_at; no app path stores one (Rails writes nil for a bad value).
    for bad in ["garbage", ""] {
        let (_d, o, s) = common::install();
        let id = insert(&o.primary, &s, v);
        o.primary.execute("UPDATE bots SET settings = json_set(settings, '$.start_at', ?2) WHERE id = ?1", rusqlite::params![id, bad]).unwrap();
        let r = eligibility::check_install(&o.primary).unwrap();
        assert!(!r.eligible.contains(&id) && !r.unreadable.is_empty(), "{bad:?}: {:?}", r.unreadable);
    }
    // No anchor at all is still refused, as before.
    let (_d, o, s) = common::install();
    let id = insert(&o.primary, &s, v);
    o.primary.execute("UPDATE bots SET started_at = NULL, settings = json_remove(settings, '$.start_at') WHERE id = ?1", [id]).unwrap();
    assert!(!eligibility::check_install(&o.primary).unwrap().eligible.contains(&id));
}

/// D-START-ANCHOR-STARTED: turning the rule off writes the flag and updated_at only, whatever start_at holds (moved, or
/// unreadable): no capture, the carry window stays.
#[test]
fn turning_the_rule_off_is_the_flag_alone_whatever_start_at_holds() {
    let v = &vectors()["schedule"][6];
    for start_at in ["2026-09-12T07:30:00Z", "garbage"] {
        let (_d, o, s) = common::install();
        let c = &o.primary;
        let id = insert(c, &s, v);
        c.execute("UPDATE bots SET settings = json_set(settings, '$.start_at', ?2) WHERE id = ?1", rusqlite::params![id, start_at]).unwrap();
        let (before_settings, before_transient, before_changed) = stored(c, id);
        amount::disable_starting_time(c, id, "2026-09-12T08:00:00Z".parse().unwrap()).unwrap();
        let (settings, transient, changed) = stored(c, id);
        assert_eq!(settings["start_time_enabled"], json!(false), "{start_at}");
        let mut expected = before_settings; expected["start_time_enabled"] = json!(false);
        assert_eq!((settings, transient, changed), (expected, before_transient, before_changed), "{start_at}: the flag alone");
    }
}

/// Section 9's guarantee, sequentially over three intervals for a start_at moved on a running bot: each checkpoint's order
/// follows from the fills before it, one per interval, as for the unmoved bot. The first order fills 15 of 60 and is
/// cancelled; every later one fills. Daily: 60, 105, 60. Monthly (Rails' carry divides by the average month): 60, 45, 120.
#[tokio::test(flavor = "current_thread")]
async fn a_moved_start_at_settles_over_three_intervals_as_the_unmoved_bot() {
    for (name, times, expected) in [
        ("moved day 93600 open", ["2026-09-10T07:30:00.250Z", "2026-09-11T07:30:00.250Z", "2026-09-12T07:30:00.250Z"], ["60", "105", "60"]),
        ("moved month 93600 open", ["2026-09-10T07:30:00.250Z", "2026-10-10T07:30:00.250Z", "2026-11-10T07:30:00.250Z"], ["60", "45", "120"]),
    ] {
        let e = edge(name);
        let mut v = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.0", "49995.0");
        for _ in 0..6 { v = v.balance_body("ZEUR", "100000", "0"); }
        let (_d, mut en, id) = edge_engine(&e, "before first run", None, v.clone());
        let mut got = vec![];
        for (i, at) in times.iter().enumerate() {
            let at = t(at);
            // Nothing in the hour before the checkpoint, one order at it.
            got.extend(replay(&mut en, &v, &[at - chrono::Duration::hours(1), at]).await.into_iter().filter(|q| *q > BigDec::zero()));
            let last = "(SELECT max(id) FROM transactions WHERE bot_id = ?1)";
            let fill = if i == 0 { format!("UPDATE transactions SET external_status = 3, quote_amount_exec = '15', amount_exec = '0.0003' WHERE id = {last}") }
                       else { format!("UPDATE transactions SET external_status = 2, quote_amount_exec = quote_amount, amount_exec = '0.001' WHERE id = {last}") };
            en.primary.execute(&fill, [id]).unwrap();
        }
        assert_eq!(got, amounts(&expected), "{name}");
    }
}
