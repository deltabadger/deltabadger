//! A28-1 (PLAN-A28 r2-r5, D-RAILS-UNRECORDED-REPORTED): the orders Rails may have sent but never recorded are collected
//! at a takeover and reported with what the imported ledger shows. Rails-free: a primary from baseline.sql plus this
//! build's migrations, and a queue with the Solid Queue columns the collection reads.
mod common;
use chrono::{DateTime, Duration, Utc};
use common::seed::{self, BotSpec, Seeded};
use deltabadger::engine::{model, rails_unrecorded as ru};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

fn t(s: &str) -> DateTime<Utc> { s.parse().unwrap() }
/// The takeover.
fn now() -> DateTime<Utc> { t("2026-09-30T12:00:00Z") }
/// Rails' ambiguous tick.
const AT: &str = "2026-09-29 14:03:00";

fn primary() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    deltabadger::store::create_schema(&c).unwrap();
    deltabadger::store::test_support::migrate(&c).unwrap();
    c
}
fn queue() -> Connection {
    let q = Connection::open_in_memory().unwrap();
    q.execute_batch("CREATE TABLE solid_queue_jobs (id INTEGER PRIMARY KEY, class_name TEXT, arguments TEXT, scheduled_at TEXT, created_at TEXT, finished_at TEXT);
                     CREATE TABLE solid_queue_claimed_executions (id INTEGER PRIMARY KEY, job_id INTEGER, process_id INTEGER, created_at TEXT);
                     CREATE TABLE solid_queue_processes (id INTEGER PRIMARY KEY, last_heartbeat_at TEXT);
                     CREATE TABLE solid_queue_failed_executions (id INTEGER PRIMARY KEY, job_id INTEGER, created_at TEXT);").unwrap();
    q
}
/// A Binance install: the Kraken seed renamed (ticker BTCEUR), its key a reading key.
fn binance(c: &Connection) -> Seeded {
    let s = seed::seed_kraken(c, &seed::cipher());
    c.execute("UPDATE exchanges SET type = 'Exchanges::Binance', name = 'Binance' WHERE id = ?1", [s.exchange_id]).unwrap();
    c.execute("UPDATE tickers SET ticker = 'BTCEUR', base = 'BTC' WHERE id = ?1", [s.ticker_id]).unwrap();
    s
}
fn bot(c: &Connection, s: &Seeded) -> i64 { seed::insert_bot(c, s, &BotSpec::weekly(20.0, "2026-09-01 10:00:00")) }
fn ambiguous_line(c: &Connection, bot: i64, at: &str) -> i64 {
    c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'placement_ambiguous', 1, ?2, ?3)",
              params![bot, json!({ "error": "Net::ReadTimeout" }).to_string(), at]).unwrap();
    c.last_insert_rowid()
}
fn failed_row(c: &Connection, s: &Seeded, bot: i64, error: &str, at: &str) -> i64 {
    c.execute("INSERT INTO transactions (bot_id, exchange_id, status, side, order_type, quote_amount, base, quote, base_asset_id, quote_asset_id, \
               bot_interval, bot_quote_amount, transaction_type, error_messages, created_at, updated_at) \
               VALUES (?1, ?2, 1, 0, 0, '20', 'BTC', 'EUR', ?3, ?4, 'week', 20, 'REGULAR', ?5, ?6, ?6)",
              params![bot, s.exchange_id, s.btc, s.quote, json!([error]).to_string(), at]).unwrap();
    c.last_insert_rowid()
}
fn obligations(c: &Connection) -> Vec<(i64, String, String, String)> {
    let mut s = c.prepare("SELECT bot_id, source, span_from, span_to FROM rust_rails_unrecorded ORDER BY id").unwrap();
    s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).unwrap().map(Result::unwrap).collect()
}
fn lines(c: &Connection, bot: i64) -> Vec<(String, Value)> {
    let mut s = c.prepare("SELECT event, details FROM bot_activity_logs WHERE bot_id = ?1 AND event LIKE 'rails_order_%' ORDER BY id").unwrap();
    s.query_map([bot], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap().map(|r| { let (e, d) = r.unwrap(); (e, serde_json::from_str(&d).unwrap()) }).collect()
}
/// A finished ledger sync of the key, at `at`.
fn synced(c: &Connection, key: i64, at: DateTime<Utc>) { deltabadger::jobs::state::record_success(c, deltabadger::sync::jobs::LEDGER_SYNC, Some(&key.to_string()), at).unwrap(); }
/// Everything a report must leave alone: no order row, carry, figure or ledger link moves (the money guard, r2.6).
fn money(c: &Connection) -> Vec<String> {
    let dump = |sql: &str| -> String {
        let mut s = c.prepare(sql).unwrap();
        let n = s.column_count();
        s.query_map([], |r| Ok((0..n).map(|i| format!("{:?}", r.get_ref(i).unwrap())).collect::<Vec<_>>().join(","))).unwrap().map(Result::unwrap).collect::<Vec<_>>().join(";")
    };
    vec![dump("SELECT * FROM transactions ORDER BY id"), dump("SELECT id, settings, transient_data, status, started_at, settings_changed_at FROM bots ORDER BY id"),
         dump("SELECT id, transaction_id, linked_transaction_id FROM account_transactions ORDER BY id"), dump("SELECT count(*) FROM transactions")]
}

// ---- collection ------------------------------------------------------------------------------------------------------

#[test]
fn every_rails_ambiguous_line_on_a_dca_bot_is_collected_once_and_a_signal_bots_is_not() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, "2026-08-01 10:00:00");
    ambiguous_line(&c, b, AT);
    c.execute("INSERT INTO bots (type, status, exchange_id, user_id, settings, transient_data, created_at, updated_at) VALUES ('Bots::Signal', 1, ?1, ?2, '{}', '{}', ?3, ?3)",
              params![s.exchange_id, s.user_id, AT]).unwrap();
    ambiguous_line(&c, c.last_insert_rowid(), AT);
    assert_eq!(ru::collect(&c, &queue(), None, now()).unwrap(), 2, "every line, not only the newest (D-RAILS-UNRECORDED-ALL-LINES)");
    assert_eq!(ru::collect(&c, &queue(), None, now()).unwrap(), 0, "a second takeover collects nothing twice");
    assert_eq!(obligations(&c).iter().map(|o| o.2.clone()).collect::<Vec<_>>(), ["2026-08-01 10:00:00", AT]);
}

#[test]
fn only_what_rails_wrote_after_the_last_handback_is_collected() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, "2026-09-10 10:00:00"); // the engine's own, before its handback
    failed_row(&c, &s, b, "Net::ReadTimeout with #<TCPSocket>", "2026-09-10 10:00:01");
    let rails = ambiguous_line(&c, b, AT);
    ru::collect(&c, &queue(), Some(t("2026-09-20T00:00:00Z")), now()).unwrap();
    let got: Vec<i64> = c.prepare("SELECT source_ref FROM rust_rails_unrecorded").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(got, [rails]);
}

#[test]
fn a_failed_row_is_collected_only_when_its_saved_error_leaves_the_order_unknown() {
    for (error, collected) in [
        ("Faraday::TimeoutError: Net::ReadTimeout with #<TCPSocket:(closed)>", true),
        ("Faraday::ConnectionFailed: Connection reset by peer", true),
        ("HTTP 504", true),
        ("Unreadable response (HTTP 200)", true),
        ("Failed to set Kraken market order (order_id is nil)", true),
        ("Faraday::ConnectionFailed: Failed to open TCP connection to api.example:443 (execution expired)", false),
        ("Faraday::ConnectionFailed: Connection refused - connect(2) for 127.0.0.1:443", false),
        ("Net::OpenTimeout: execution expired", false),
        ("SocketError: getaddrinfo: nodename nor servname provided", false),
        ("Timestamp for this request is outside of the recvWindow", false),
        ("Account has insufficient balance for requested action.", false),
        ("HTTP 404", false),
        ("IBKR accepted the order but returned no order id for BTC", true),
        ("Order would immediately match and take.", true),
    ] {
        assert_eq!(ru::ambiguous_failure("Exchanges::Binance", &[error.to_string()]), collected, "{error}");
    }
}

#[test]
fn a_failed_row_keeps_the_pair_and_side_it_recorded() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    failed_row(&c, &s, b, "Faraday::TimeoutError: Net::ReadTimeout", AT);
    failed_row(&c, &s, b, "Account has insufficient balance for requested action.", AT);
    ru::collect(&c, &queue(), None, now()).unwrap();
    let scope: String = c.query_row("SELECT scope FROM rust_rails_unrecorded", [], |r| r.get(0)).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&scope).unwrap(), json!({ "exchange_id": s.exchange_id, "base_asset_id": s.btc, "quote_asset_id": s.quote, "side": 0 }));
    assert_eq!(obligations(&c).len(), 1, "the venue's own rejection is not collected");
}

#[test]
fn claimed_and_failed_action_jobs_are_each_their_own_obligation_with_their_span() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    let q = queue();
    let args = json!({ "job_class": "Bot::ActionJob", "arguments": [{ "_aj_globalid": format!("gid://deltabadger/Bots::DcaMultiAsset/{b}") }] }).to_string();
    q.execute("INSERT INTO solid_queue_jobs (id, class_name, arguments, scheduled_at, created_at) VALUES (1, 'Bot::ActionJob', ?1, '2026-09-29 14:00:00', '2026-09-22 14:00:00')", [&args]).unwrap();
    q.execute("INSERT INTO solid_queue_processes (id, last_heartbeat_at) VALUES (7, '2026-09-29 14:02:00')", []).unwrap();
    q.execute("INSERT INTO solid_queue_claimed_executions (job_id, process_id, created_at) VALUES (1, 7, '2026-09-29 14:00:01')", []).unwrap();
    // The tick it scheduled after an ambiguity, failed in an overlapping window: a second attempt, kept apart (r3.5).
    q.execute("INSERT INTO solid_queue_jobs (id, class_name, arguments, scheduled_at, created_at) VALUES (2, 'Bot::ActionJob', ?1, '2026-09-29 14:01:00', '2026-09-29 14:00:30')", [&args]).unwrap();
    q.execute("INSERT INTO solid_queue_failed_executions (job_id, created_at) VALUES (2, '2026-09-29 14:01:30')", []).unwrap();
    // One a process still had claimed with no process row: ten minutes from its claim.
    q.execute("INSERT INTO solid_queue_jobs (id, class_name, arguments, scheduled_at, created_at) VALUES (3, 'Bot::ActionJob', ?1, '2026-09-29 15:00:00', '2026-09-29 14:00:30')", [&args]).unwrap();
    q.execute("INSERT INTO solid_queue_claimed_executions (job_id, process_id, created_at) VALUES (3, 99, '2026-09-29 15:00:00')", []).unwrap();
    ru::collect(&c, &q, None, now()).unwrap();
    assert_eq!(obligations(&c), [
        (b, "queue_job".into(), "2026-09-29 14:00:01".into(), "2026-09-29 14:03:00".into()),
        (b, "queue_job".into(), "2026-09-29 14:01:00".into(), "2026-09-29 14:01:30".into()),
        (b, "queue_job".into(), "2026-09-29 15:00:00".into(), "2026-09-29 15:10:00".into()),
    ]);
}

// ---- the report ------------------------------------------------------------------------------------------------------

#[test]
fn a_line_waits_for_a_ledger_sync_after_the_window_then_is_written_once() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    ru::collect(&c, &queue(), None, now()).unwrap();
    let before = money(&c);
    assert_eq!(ru::report(&c, now()).unwrap(), 0, "no sync after the window yet: nothing said");
    synced(&c, s.api_key_id, now());
    assert_eq!(ru::report(&c, now()).unwrap(), 1);
    assert_eq!(ru::report(&c, now() + Duration::hours(1)).unwrap(), 0, "one line, ever");
    let l = lines(&c, b);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].0, ru::NONE_SEEN);
    assert_eq!(l[0].1, json!({ "when": "around 2026-09-29 14:03 UTC", "venue": "Binance" }));
    assert_eq!(money(&c), before, "nothing but the line is written");
}

#[test]
fn an_unfinished_sync_is_no_answer_and_seven_days_without_one_say_so() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    ru::collect(&c, &queue(), None, now()).unwrap();
    synced(&c, s.api_key_id, now());
    deltabadger::jobs::state::mark_incomplete(&c, deltabadger::sync::jobs::LEDGER_SYNC, Some(&s.api_key_id.to_string()), now()).unwrap();
    assert_eq!(ru::report(&c, now() + Duration::days(6)).unwrap(), 0);
    ru::report(&c, now() + Duration::days(7)).unwrap();
    assert_eq!(lines(&c, b)[0].1["reason"], "unread");
}

#[test]
fn kraken_has_no_history_to_check_against() {
    let c = primary();
    let s = seed::seed_kraken(&c, &seed::cipher());
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    ru::collect(&c, &queue(), None, now()).unwrap();
    ru::report(&c, now()).unwrap();
    let l = lines(&c, b);
    assert_eq!((l[0].0.as_str(), l[0].1["reason"].as_str(), l[0].1["venue"].as_str()), (ru::UNCHECKED, Some("no_history"), Some("Kraken")));
}

#[test]
fn a_settings_change_a_restart_or_a_later_order_elsewhere_leaves_the_scope_unknown() {
    for change in ["settings", "restart", "flip", "other_pair"] {
        let c = primary();
        let s = binance(&c);
        let b = bot(&c, &s);
        ambiguous_line(&c, b, AT);
        match change {
            "settings" => { c.execute("UPDATE bots SET settings_changed_at = '2026-09-30 09:00:00' WHERE id = ?1", [b]).unwrap(); }
            "restart" => { c.execute("UPDATE bots SET started_at = '2026-09-30 09:00:00' WHERE id = ?1", [b]).unwrap(); }
            "flip" => { model::log_activity(&c, b, "stopped", model::Level::Info, json!({}), t("2026-09-30T09:00:00Z")).unwrap(); }
            _ => {
                let (eth, _) = seed::add_eth_sol(&c, &s);
                c.execute("INSERT INTO transactions (bot_id, exchange_id, status, side, base_asset_id, quote_asset_id, transaction_type, error_messages, created_at, updated_at) \
                           VALUES (?1, ?2, 0, 0, ?3, ?4, 'REGULAR', '[]', '2026-09-30 09:00:00', '2026-09-30 09:00:00')", params![b, s.exchange_id, eth, s.quote]).unwrap();
            }
        }
        ru::collect(&c, &queue(), None, now()).unwrap();
        synced(&c, s.api_key_id, now());
        ru::report(&c, now()).unwrap();
        assert_eq!(lines(&c, b)[0].1["reason"], "settings_changed", "{change}");
    }
}

#[test]
fn an_index_bots_ambiguity_is_unchecked() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    c.execute("UPDATE bots SET type = 'Bots::DcaIndex' WHERE id = ?1", [b]).unwrap();
    ambiguous_line(&c, b, AT);
    ru::collect(&c, &queue(), None, now()).unwrap();
    ru::report(&c, now()).unwrap();
    assert_eq!(lines(&c, b)[0].1["reason"], "index");
}

#[test]
fn a_user_without_a_reading_key_on_the_venue_gets_unread_after_seven_days() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    c.execute("UPDATE api_keys SET status = 2 WHERE id = ?1", [s.api_key_id]).unwrap();
    ambiguous_line(&c, b, AT);
    ru::collect(&c, &queue(), None, now()).unwrap();
    assert_eq!(ru::report(&c, now()).unwrap(), 0);
    ru::report(&c, now() + Duration::days(8)).unwrap();
    assert_eq!(lines(&c, b)[0].1["reason"], "unread");
}

#[test]
fn every_line_renders_in_english_without_a_missing_key() {
    use deltabadger::web::i18n::{self, Arg};
    for key in ["rails_order_candidate", "rails_order_candidates", "rails_order_none_seen", "rails_order_unchecked_settings_changed", "rails_order_unchecked_index",
                "rails_order_unchecked_no_history", "rails_order_unchecked_unread", "rails_order_unchecked_unreadable", "rails_order_unchecked_reach", "rails_order_unchecked_gone", "rails_order_unchecked_time"] {
        let text = i18n::text("en", &format!("bot_activity.events.{key}"), &[("when", Arg::Text("around 2026-09-29 14:03 UTC")), ("venue", Arg::Text("Binance")), ("trades", Arg::Text("x"))]);
        assert!(text.starts_with("Before the update, an order from this bot may have gone through without being recorded (around 2026-09-29 14:03 UTC)."), "{key}: {text}");
        assert!(!text.contains("%{"), "{key}: {text}");
    }
}

// ---- the oracle: Rails' own verdict on real transport failures (script/rust/rails_unrecorded.rb) -----------------------

fn vectors() -> Value { serde_json::from_str(include_str!("fixtures/rails_unrecorded_vectors.json")).unwrap() }

#[test]
fn the_ruby_being_mirrored_has_not_changed() {
    use sha2::{Digest, Sha256};
    let v = vectors();
    let pinned = v["ported_sources"].as_object().unwrap();
    assert_eq!(pinned.len(), 5);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, sum) in pinned {
        let bytes = std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(hex::encode(Sha256::digest(&bytes)), sum.as_str().unwrap(), "{path} changed since the vectors were recorded");
    }
}

/// r3.3: the saved text alone gives Rails' verdict (Client.network_failure + the placement guard) on every recorded failure.
#[test]
fn the_saved_error_text_reads_as_rails_decided_the_failure() {
    let v = vectors();
    let cases = v["vectors"].as_array().unwrap();
    assert_eq!(cases.len(), 8);
    for case in cases {
        let errors: Vec<String> = case["errors"].as_array().unwrap().iter().map(|e| e.as_str().unwrap().to_string()).collect();
        assert_eq!(ru::ambiguous_failure("Exchanges::Binance", &errors), case["ambiguous"].as_bool().unwrap(), "{errors:?}");
    }
}

// ---- the trades a line lists (the imported ledger, read through sync::fills) ------------------------------------------

/// One Binance fill of `order` on BTCEUR, stored as the importer stores it.
#[allow(clippy::too_many_arguments)]
fn fill(c: &Connection, s: &Seeded, id: i64, order: i64, buyer: bool, qty: &str, quote: &str, at: &str) -> i64 {
    let ms = t(at).timestamp_millis();
    let raw = deltabadger::sync::activities::Raw::from_value(json!({ "symbol": "BTCEUR", "id": id, "orderId": order, "orderListId": -1, "price": "50000",
        "qty": qty, "quoteQty": quote, "commission": "0", "commissionAsset": "EUR", "time": ms, "isBuyer": buyer, "isMaker": false, "isBestMatch": true }));
    let legs = deltabadger::sync::binance_ledger::normalize_trade(&raw, "BTCEUR", "BTC", "EUR").unwrap();
    let e = &legs[0];
    c.execute("INSERT INTO account_transactions (user_id, exchange_id, api_key_id, tx_id, entry_type, base_currency, base_amount, quote_currency, quote_amount, \
               raw_data, transacted_at, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, 'BTC', ?6, 'EUR', ?7, ?8, ?9, ?9, ?9)",
              params![s.user_id, s.exchange_id, s.api_key_id, e.tx_id, e.entry_type, qty, quote, e.raw.value.to_string(),
                      deltabadger::codec::format_time(e.transacted_at.unwrap())]).unwrap();
    c.last_insert_rowid()
}
/// An ambiguous tick collected and its ledger synced, then the report run: the bot's one line.
fn reported(c: &Connection, s: &Seeded, b: i64) -> (String, Value) {
    ru::collect(c, &queue(), None, now()).unwrap();
    synced(c, s.api_key_id, now());
    let before = money(c);
    assert_eq!(ru::report(c, now()).unwrap(), 1);
    assert_eq!(money(c), before, "a report writes no order, carry or link");
    let mut l = lines(c, b);
    assert_eq!(l.len(), 1);
    l.remove(0)
}

#[test]
fn a_trade_no_bot_recorded_in_the_window_is_listed_with_its_fills_summed() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    fill(&c, &s, 11, 5, true, "0.0002", "10", "2026-09-29T14:03:05Z");
    fill(&c, &s, 12, 5, true, "0.0002", "10.01", "2026-09-29T14:03:06Z");
    let (event, details) = reported(&c, &s, b);
    assert_eq!(event, ru::CANDIDATES);
    assert_eq!(details, json!({ "when": "around 2026-09-29 14:03 UTC", "venue": "Binance", "count": 1,
                                "trades": "bought 0.0004 BTC for 20.01 EUR at 14:03:05 UTC (filled)" }));
}

#[test]
fn several_trades_are_listed_up_to_three_and_the_rest_counted() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    for n in 0..5 { fill(&c, &s, 20 + n, 30 + n, true, "0.0001", "5", &format!("2026-09-29T14:0{}:00Z", n + 1)); }
    let (_, details) = reported(&c, &s, b);
    assert_eq!(details["count"], 5);
    assert!(details["trades"].as_str().unwrap().ends_with("; and 2 more"), "{details}");
}

#[test]
fn an_order_a_bot_holds_a_linked_fill_another_side_and_a_trade_outside_the_window_are_not_listed() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    // Held: another bot's recorded order.
    let other = bot(&c, &s);
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
               VALUES (?1, ?2, 'BTCEUR-5', 0, 0, 'REGULAR', '[]', '2026-09-29 14:03:01', '2026-09-29 14:03:01')", params![other, s.exchange_id]).unwrap();
    let held = c.last_insert_rowid();
    fill(&c, &s, 11, 5, true, "0.0004", "20", "2026-09-29T14:03:05Z");
    // Linked: a fill the ledger already gives to a bot order.
    let linked = fill(&c, &s, 12, 6, true, "0.0004", "20", "2026-09-29T14:03:05Z");
    c.execute("UPDATE account_transactions SET transaction_id = ?2 WHERE id = ?1", params![linked, held]).unwrap();
    fill(&c, &s, 13, 7, false, "0.0004", "20", "2026-09-29T14:03:05Z"); // a sale: this bot buys
    fill(&c, &s, 14, 8, true, "0.0004", "20", "2026-09-29T14:14:00Z"); // 11 minutes after
    let (event, _) = reported(&c, &s, b);
    assert_eq!(event, ru::NONE_SEEN, "only recorded or unrelated trades in the history");
}

#[test]
fn a_ledger_row_that_cant_be_read_in_the_window_leaves_the_order_unchecked() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    let r = fill(&c, &s, 11, 5, true, "0.0004", "20", "2026-09-29T14:03:05Z");
    c.execute("UPDATE account_transactions SET raw_data = json_set(raw_data, '$.qty', 'abc') WHERE id = ?1", [r]).unwrap();
    let (_, details) = reported(&c, &s, b);
    assert_eq!(details["reason"], "unreadable");
}

#[test]
fn a_venue_whose_history_may_not_reach_back_never_says_none_seen() {
    let c = primary();
    let s = binance(&c);
    c.execute("UPDATE exchanges SET type = 'Exchanges::Mexc', name = 'MEXC' WHERE id = ?1", [s.exchange_id]).unwrap();
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    let (event, details) = reported(&c, &s, b);
    assert_eq!((event.as_str(), details["reason"].as_str()), (ru::UNCHECKED, Some("reach")));
}

#[test]
fn a_failed_rows_own_pair_is_searched_even_after_the_bots_settings_changed() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    failed_row(&c, &s, b, "Faraday::TimeoutError: Net::ReadTimeout", AT);
    c.execute("UPDATE bots SET settings_changed_at = '2026-09-30 09:00:00' WHERE id = ?1", [b]).unwrap();
    fill(&c, &s, 11, 5, true, "0.0004", "20", "2026-09-29T14:03:01Z");
    let (event, details) = reported(&c, &s, b);
    assert_eq!((event.as_str(), details["count"].as_i64()), (ru::CANDIDATES, Some(1)));
}

#[test]
fn a_delisted_pair_of_an_unchanged_bot_is_still_searched() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    c.execute("UPDATE tickers SET available = 0, trading_enabled = 0 WHERE id = ?1", [s.ticker_id]).unwrap();
    ambiguous_line(&c, b, AT);
    fill(&c, &s, 11, 5, true, "0.0004", "20", "2026-09-29T14:03:05Z");
    assert_eq!(reported(&c, &s, b).0, ru::CANDIDATES);
}

// ---- Codex branch r1 ---------------------------------------------------------------------------------------------------

#[test]
fn this_engines_own_ambiguity_lines_are_never_collected() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    for details in [json!({ "error": "the order never reached Binance", "resolution": "not_placed", "source": "rust", "cl_ord_id": "x" }),
                    json!({ "error": "resolved by the operator", "resolution": "filled", "source": "operator", "cl_ord_id": "y", "order_id": "1" })] {
        c.execute("INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) VALUES (?1, 'placement_ambiguous', 1, ?2, ?3)", params![b, details.to_string(), AT]).unwrap();
    }
    assert_eq!(ru::collect(&c, &queue(), None, now()).unwrap(), 0);
}

#[test]
fn an_unreadable_queue_time_never_fails_the_takeover_and_its_line_says_the_time_is_unknown() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    let q = queue();
    let args = json!({ "arguments": [{ "_aj_globalid": format!("gid://deltabadger/Bots::DcaMultiAsset/{b}") }] }).to_string();
    q.execute("INSERT INTO solid_queue_jobs (id, class_name, arguments, scheduled_at, created_at) VALUES (1, 'Bot::ActionJob', ?1, 'not a time', '2026-09-29 14:00:00')", [&args]).unwrap();
    q.execute("INSERT INTO solid_queue_failed_executions (job_id, created_at) VALUES (1, '2026-09-29 14:01:00')", []).unwrap();
    assert_eq!(ru::collect(&c, &q, None, now()).unwrap(), 1);
    ru::report(&c, now()).unwrap();
    let l = lines(&c, b);
    assert_eq!((l[0].1["reason"].as_str(), l[0].1["when"].as_str()), (Some("time"), Some("at a time that can't be read")));
}

#[test]
fn a_change_while_rails_was_placing_leaves_the_scope_unknown() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    let q = queue();
    let args = json!({ "arguments": [{ "_aj_globalid": format!("gid://deltabadger/Bots::DcaMultiAsset/{b}") }] }).to_string();
    q.execute("INSERT INTO solid_queue_jobs (id, class_name, arguments, scheduled_at, created_at) VALUES (1, 'Bot::ActionJob', ?1, '2026-09-29 12:00:00', '2026-09-29 12:00:00')", [&args]).unwrap();
    q.execute("INSERT INTO solid_queue_claimed_executions (job_id, process_id, created_at) VALUES (1, 99, '2026-09-29 12:00:00')", []).unwrap();
    c.execute("UPDATE bots SET settings_changed_at = '2026-09-29 12:05:00' WHERE id = ?1", [b]).unwrap();
    ru::collect(&c, &q, None, now()).unwrap();
    synced(&c, s.api_key_id, now());
    ru::report(&c, now()).unwrap();
    assert_eq!(lines(&c, b)[0].1["reason"], "settings_changed");
}

#[test]
fn a_fill_of_the_order_linked_outside_the_window_excludes_it() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    ambiguous_line(&c, b, AT);
    let other = bot(&c, &s);
    c.execute("INSERT INTO transactions (bot_id, exchange_id, external_id, status, side, transaction_type, error_messages, created_at, updated_at) \
               VALUES (?1, ?2, 'BTCEUR-OTHER', 0, 0, 'REGULAR', '[]', '2026-09-29 14:03:01', '2026-09-29 14:03:01')", params![other, s.exchange_id]).unwrap();
    let held = c.last_insert_rowid();
    fill(&c, &s, 11, 5, true, "0.0002", "10", "2026-09-29T14:03:05Z");
    let late = fill(&c, &s, 12, 5, true, "0.0002", "10", "2026-09-29T16:00:00Z");
    c.execute("UPDATE account_transactions SET transaction_id = ?2 WHERE id = ?1", params![late, held]).unwrap();
    assert_eq!(reported(&c, &s, b).0, ru::NONE_SEEN);
}

#[test]
fn a_bot_that_moved_exchange_is_told_to_check_the_exchange_it_traded_on_then() {
    let c = primary();
    let s = binance(&c);
    let b = bot(&c, &s);
    seed::insert_tx(&c, &s, b, &seed::TxSpec { status: 0, external_status: Some(2), external_id: Some("BTCEUR-1".into()), order_type: 0, amount: Some("0.001"),
        quote_amount: Some("20"), price: Some("20000"), quote_amount_exec: Some("20"), amount_exec: Some("0.001"), created_at: "2026-09-22 14:03:00".into() });
    ambiguous_line(&c, b, AT);
    c.execute("INSERT INTO exchanges (type, name, created_at, updated_at) VALUES ('Exchanges::Kraken', 'Kraken', ?1, ?1)", [AT]).unwrap();
    c.execute("UPDATE bots SET exchange_id = ?2, started_at = '2026-09-30 09:00:00' WHERE id = ?1", params![b, c.last_insert_rowid()]).unwrap();
    ru::collect(&c, &queue(), None, now()).unwrap();
    ru::report(&c, now()).unwrap();
    assert_eq!(lines(&c, b)[0].1["venue"], "Binance");
}
