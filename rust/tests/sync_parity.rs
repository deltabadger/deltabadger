#![cfg(unix)] // shells out to bin/rails
//! Row parity for the tracker's syncs: Rails' real jobs (AccountTransaction::SyncJob, AccountBalance::SyncJob) and the
//! Rust syncs run on copies of one install over the same scripted Alpaca and market-data bodies; the changed rows of
//! account_transactions, account_balances, api_keys, bots and bot_activity_logs, the requests sent and the key Rails
//! would read with must be identical, except in the listed divergences, where Rails' result and Rust's are each asserted.
use std::path::{Path, PathBuf};
use std::process::Command;
use serde_json::{json, Value};
use std::sync::Arc;

fn try_rails(args: &[&str]) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let run = |rails_args: &[&str]| -> Result<(), String> {
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(root).args(rails_args).env_remove("DATABASE_URL")
            .env("APP_ROOT_URL", "http://localhost:3000") // config/environments/development.rb requires it
            .env("SKIP_TEST_DATABASE", "true"); // schema:load in development also purges the repo's storage/test*.sqlite3
        for db in ["primary", "queue", "cache", "cable"] {
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
        }
        let out = cmd.output().expect("bin/rails runs");
        if out.status.success() { Ok(()) } else { Err(format!("bin/rails {rails_args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr))) }
    };
    run(&["db:schema:load"])?; // the oracle's own queue, cache and cable databases must exist
    let mut full = vec!["runner", "script/rust/sync.rb"];
    full.extend_from_slice(args);
    run(&full)
}
fn rails(args: &[&str]) { if let Err(e) = try_rails(args) { panic!("{e}"); } }

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

/// Exercise unreadable prices against the same Rails-built install that already has a stale AAPL price.
fn add_unreadable_price_scenarios(root: &Path) {
    let source = root.join("balances-no_trade_keeps_last_price");
    for value in ["NaN", "Infinity", "-Infinity", "garbage"] {
        for fallback in ["market", "stale"] {
            let dir = root.join(format!("balances-unreadable_price_{value}_{fallback}"));
            copy_dir(&source, &dir);
            let path = dir.join("scenario.json");
            let mut scenario: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let step = &mut scenario["steps"][0];
            step["alpaca"]["GET /v2/positions"][0]["body"] = json!([
                { "symbol": "AAPL", "qty": "3", "asset_class": "us_equity" },
                { "symbol": "KLAC", "qty": "2", "asset_class": "us_equity" }
            ]);
            step["alpaca"]["GET /v2/stocks/snapshots"][0]["body"] = json!({
                "AAPL": { "latestTrade": { "p": value } }, "KLAC": { "latestTrade": { "p": 200 } }
            });
            step["market"]["GET /api/v1/prices"][0]["body"] = if fallback == "market" {
                json!({ "data": { "AAPL.US": { "usd": 221 } } })
            } else { json!({ "data": {} }) };
            std::fs::write(path, serde_json::to_string_pretty(&scenario).unwrap()).unwrap();
        }
    }
}

fn cipher() -> Arc<deltabadger::crypto::Cipher> {
    Arc::new(deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "sync-parity").unwrap()))
}

/// Both halves are read without rounding an integer (`parity::exact`): raw_data must hold the number the venue sent.
fn read(path: &Path) -> Value { deltabadger::sync::parity::exact(&std::fs::read_to_string(path).unwrap()).unwrap() }
fn rows(out: &Value, table: &str) -> Vec<Value> { out["changes"][table].as_array().unwrap().clone() }
fn after_of(out: &Value, step: usize) -> Value { out["steps"][step]["requests"][0][1].as_array().unwrap().iter().find(|p| p[0] == "after").map(|p| p[1].clone()).unwrap_or(Value::Null) }
fn check(ok: bool, what: &str) -> Result<(), String> { if ok { Ok(()) } else { Err(what.to_string()) } }

// ---- a listed divergence: what may differ is named and asserted, and everything else must be identical ----

/// Takes the named columns out of the `after` of every changed row of `table`.
fn without_columns(out: &mut Value, table: &str, columns: &[&str]) {
    for row in out["changes"][table].as_array_mut().into_iter().flatten() {
        if let Some(after) = row["after"].as_object_mut() { for c in columns { after.shift_remove(*c); } }
    }
}
/// Takes every changed row of `table` out.
fn without_rows(out: &mut Value, table: &str) { out["changes"][table] = json!([]); }
/// With the permitted differences taken out of both, Rails' output and Rust's are the same: every other row, column,
/// request, raise and counter.
fn rest_is_identical(rails: &Value, rust: &Value, permitted: &dyn Fn(&mut Value)) -> Result<(), String> {
    let (mut a, mut b) = (rails.clone(), rust.clone());
    permitted(&mut a);
    permitted(&mut b);
    check(a == b, &format!("beyond the permitted differences the two outputs differ:\n  rails: {a}\n  rust:  {b}"))
}
fn key_error(out: &Value) -> Value { rows(out, "api_keys").first().map(|r| r["after"]["last_sync_error"].clone()).unwrap_or(Value::Null) }
/// Both sides fail the sync and write nothing but the key's error, whose text is each side's own.
fn only_the_error_text_differs(rails: &Value, rust: &Value, rails_text: &str, rust_text: &str) -> Result<(), String> {
    check(key_error(rails) == rails_text, &format!("Rails' error is no longer {rails_text:?}: {}", key_error(rails)))?;
    check(key_error(rust) == rust_text, &format!("Rust's error is not {rust_text:?}: {}", key_error(rust)))?;
    rest_is_identical(rails, rust, &|out| without_columns(out, "api_keys", &["last_sync_error"]))
}

/// A stored number: an INTEGER as it is, a REAL from its bits.
fn num(v: &Value) -> Option<f64> { v.as_f64().or_else(|| v["f"].as_str().map(|hex| f64::from_bits(u64::from_str_radix(hex, 16).unwrap()))) }

/// The contract the plan gives Plan 2d (*Which rows carry a split*): whether a factor may be taken from the split rows
/// of one symbol and one date, by the stored columns alone. One verdict per symbol and date, in stored order.
fn split_verdicts(out: &Value) -> Vec<String> {
    let mut groups: Vec<(String, Vec<Value>)> = vec![];
    for row in rows(out, "account_transactions").into_iter().map(|r| r["after"].clone()).filter(|a| a["entry_type"] == 15 && a["raw_data"]["corporate_action"] == "split") {
        let key = format!("{} {}", row["base_currency"].as_str().unwrap_or_default(), row["transacted_at"].as_str().and_then(|t| t.get(..10)).unwrap_or_default());
        match groups.iter_mut().find(|g| g.0 == key) { Some(g) => g.1.push(row), None => groups.push((key, vec![row])) }
    }
    groups.iter().map(|(key, rows)| format!("{key}: {}", split_verdict(rows))).collect()
}

fn split_verdict(rows: &[Value]) -> &'static str {
    let expected = oracle_split_verdict(rows);
    let normalized: Vec<Value> = rows.iter().map(|row| { let mut row=row.clone(); row["base_amount"]=json!(num(&row["base_amount"]).map(|n| n.to_string())); row }).collect();
    assert_eq!(deltabadger::engine::splits::row_verdict(&normalized), expected);
    expected
}

fn oracle_split_verdict(rows: &[Value]) -> &'static str {
    let [row] = rows else { return "several rows" };
    let raw = &row["raw_data"];
    if raw["merged_activity_ids"].as_array().is_none_or(|ids| ids.len() < 2) { return "a lone leg"; }
    let terms: Vec<f64> = raw["split_ratio"].as_str().unwrap_or_default().split(':').map(|t| t.parse::<u64>().map_or(0.0, |t| t as f64)).collect();
    let &[new, old] = terms.as_slice() else { return "no ratio" };
    if new < 1.0 || old < 1.0 || new == old { return "no ratio"; }
    // The ratio must be the one the row's own two numbers give, with the first leg (raw_data.qty) the one leg on its
    // side: within one per-mille of their quotient, which is the window Rails makes a ratio in.
    let first = raw["qty"].as_str().and_then(|q| q.parse::<f64>().ok()).or_else(|| raw["qty"].as_f64());
    let (Some(net), Some(first)) = (num(&row["base_amount"]), first) else { return "not one split" };
    let (before, after) = if first < 0.0 { (-first, net - first) } else { (first - net, first) };
    if before > 0.0 && after > 0.0 && (after / before - new / old).abs() <= 0.001 * (after / before) * (1.0 + 1e-9) { "trusted" } else { "not one split" }
}

/// Scenarios where Rust deliberately does not do what Rails does, each asserted on its own terms: Rails' result (so
/// the divergence is dropped the day Rails is fixed) beside Rust's, and then everything the ruling does not name.
fn listed(name: &str, rails: &Value, rust: &Value) -> Option<Result<(), String>> {
    let tx = |out: &Value| rows(out, "account_transactions");
    Some(match name {
        // A page token that did not move: Rails ends the ledger there and keeps the first page; Rust fails the run.
        "ledger-pages_stalled" => (|| {
            check(tx(rails).len() == 100 && rows(rails, "api_keys")[0]["after"]["last_synced_at"] == "2026-04-10 00:00:00" && rails["steps"][0]["raised"] == false,
                  "Rails no longer reads an unmoved page token as the end of the ledger: drop the listed divergence")?;
            let key = rows(rust, "api_keys")[0].clone();
            check(tx(rust).is_empty() && rust["steps"][0]["raised"] == true && key["after"]["last_sync_error"] == "the ledger's page tokens repeat: nothing was read"
                  && key["after"]["last_synced_at"] == key["before"]["last_synced_at"], &format!("Rust: nothing stored, the watermark unmoved, the reason on the key: {key}"))?;
            rest_is_identical(rails, rust, &|out| {
                without_rows(out, "account_transactions");
                without_columns(out, "api_keys", &["last_synced_at", "updated_at", "last_sync_error"]);
                out["steps"][0]["raised"] = Value::Null;
            })
        })(),
        // A split quantity no ratio can be made of: both fail the sync and store nothing.
        "ledger-split_hostile_quantity" => only_the_error_text_differs(rails, rust, "FloatDomainError: NaN", "unreadable qty: beyond 10^±40")
            .and_then(|()| check(tx(rust).is_empty() && rust["steps"][0]["raised"] == true, "nothing of the read is stored, and the job raised")),
        _ => return listed_balances(name, rails, rust),
    })
}

/// Keeps the first `keep` requests of the first step, and of those only the ones not sent to `but`.
fn only_requests(out: &mut Value, keep: usize, but: &str) {
    if let Some(requests) = out["steps"][0]["requests"].as_array_mut() { requests.truncate(keep); requests.retain(|r| r[0] != but); }
}
/// The balance sync's listed divergences (Task 5).
fn listed_balances(name: &str, rails: &Value, rust: &Value) -> Option<Result<(), String>> {
    // R3: a stock snapshot with no latest trade. Rails: price 0, value 0, freshly priced.
    let stock = if name.ends_with("unpriced") { 3 } else { 2 };
    let balance = |out: &Value| rows(out, "account_balances").into_iter().find(|r| r["after"]["asset_id"] == stock).map(|r| r["after"].clone()).unwrap_or(Value::Null);
    let figures = |r: &Value| (num(&r["usd_price"]), num(&r["usd_value"]), r["priced_at"].as_str().map(str::to_string));
    let fresh = Some("2026-09-20 02:30:00.750000".to_string());
    let (rails_row, rust_row) = (balance(rails), balance(rust));
    let prices_asked = |out: &Value| out["steps"][0]["requests"].as_array().unwrap().iter().filter(|r| r[0] == "GET /api/v1/prices").count();
    // Permitted: that stock's price, value and pricing time, and the question Rust puts to the market source for it.
    let no_trade = |rust_figures: (Option<f64>, Option<f64>, Option<String>), asked: (usize, usize)| -> Result<(), String> {
        check(figures(&rails_row) == (Some(0.0), Some(0.0), fresh.clone()), "Rails no longer values a stock with no latest trade at 0: drop the listed divergence")?;
        check(figures(&rust_row) == rust_figures, &format!("Rust's row: {rust_row}"))?;
        check((prices_asked(rails), prices_asked(rust)) == asked, &format!("the market source is asked {} and {} times", prices_asked(rails), prices_asked(rust)))?;
        rest_is_identical(rails, rust, &|out| {
            for row in out["changes"]["account_balances"].as_array_mut().into_iter().flatten().filter(|r| r["after"]["asset_id"] == stock) {
                if let Some(after) = row["after"].as_object_mut() { for c in ["usd_price", "usd_value", "priced_at"] { after.shift_remove(c); } }
            }
            only_requests(out, usize::MAX, "GET /api/v1/prices");
        })
    };
    // A malformed answer Rails reads as an empty holding: it removes balances and stamps the key; Rust fails the sync,
    // removes nothing and stamps nothing. Permitted: the balance rows, the key's two columns, and the requests after
    // the positions (Rails goes on to price what it kept).
    let refused = |rust_text: &str, rails_removed: &[i64]| -> Result<(), String> {
        let removed: Vec<i64> = rows(rails, "account_balances").iter().filter(|r| r["after"].is_null()).filter_map(|r| r["id"].as_i64()).collect();
        check(removed == rails_removed && rows(rails, "api_keys")[0]["after"]["balances_synced_at"] == fresh.clone().unwrap() && key_error(rails).is_null(),
              &format!("Rails no longer removes {rails_removed:?} and reports success (it removed {removed:?}): drop the listed divergence"))?;
        check(rows(rust, "account_balances").is_empty(), "Rust: no balance row is touched")?;
        let key = rows(rust, "api_keys")[0].clone();
        check(key["after"]["last_sync_error"] == rust_text && key["after"]["balances_synced_at"] == key["before"]["balances_synced_at"], &format!("Rust: the error, and the clock unmoved: {key}"))?;
        rest_is_identical(rails, rust, &|out| {
            without_rows(out, "account_balances");
            without_columns(out, "api_keys", &["last_sync_error", "balances_synced_at"]);
            only_requests(out, 2, "");
        })
    };
    Some(match name {
        "balances-no_trade_market_price" => no_trade((Some(226.4), Some(2377.2), fresh.clone()), (0, 1)),
        "balances-no_trade_keeps_last_price" => no_trade((Some(220.5), Some(2315.25), Some("2026-09-10 02:30:00".to_string())), (0, 1)),
        "balances-no_trade_unpriced" => no_trade((None, None, None), (0, 1)),
        "balances-account_null" => only_the_error_text_differs(rails, rust, "NoMethodError: undefined method '[]' for nil", "unreadable account"),
        "balances-positions_not_array" => only_the_error_text_differs(rails, rust, "TypeError: no implicit conversion of String into Integer", "unreadable positions"),
        "balances-account_no_cash" => refused("the account has no cash figure", &[1]),
        "balances-cash_only_null_cash" => refused("the account has no cash figure", &[1, 2, 3, 4]),

        "balances-position_no_quantity" => refused("a position without a quantity", &[2]),
        _ => return None,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_write_identical_rows_across_the_sync_grid() -> Result<(), Box<dyn std::error::Error>> {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    let handback = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    add_unreadable_price_scenarios(rails_root.path());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    let named = |prefix: &str| dirs.iter().filter(|d| d.file_name().unwrap().to_string_lossy().starts_with(prefix)).count();
    assert_eq!((named("ledger-"), named("balances-"), dirs.len()), (58, 41, 99), "the sync grid");
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    for scenario in ["ledger-split_future", "ledger-split_nested_duplicate_keys"] { copy_dir(&rails_root.path().join(scenario), &handback.path().join(scenario)); }
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let cipher = cipher();
    let ported = ["ledger-", "balances-"];
    let mut outputs: Vec<(String, Value, Value)> = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        if !ported.iter().any(|p| name.starts_with(p)) { continue; }
        let rust_out = deltabadger::sync::parity::run(&rust_root.path().join(&name), cipher.clone()).await.unwrap_or_else(|e| panic!("{name}: {e:?}"));
        outputs.push((name, read(&d.join("rails.json")), rust_out));
    }
    let (mut failures, mut divergences) = (vec![], vec![]);
    for (name, rails_out, rust_out) in &outputs {
        match listed(name, rails_out, rust_out) {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            Some(Ok(())) => { divergences.push(name.as_str()); assert_ne!(rails_out, rust_out, "{name}: a listed divergence that no longer differs"); }
            None if rails_out != rust_out => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            None => {}
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    assert_eq!(divergences, ["balances-account_no_cash", "balances-account_null", "balances-cash_only_null_cash", "balances-no_trade_keeps_last_price",
                             "balances-no_trade_market_price", "balances-no_trade_unpriced", "balances-position_no_quantity",
                             "balances-positions_not_array",
                             "ledger-pages_stalled", "ledger-split_hostile_quantity"], "the listed divergences");

    // What the grid must have exercised, read from Rails' own output: a scenario that silently stopped doing its thing
    // (a renamed column, a changed default) would otherwise still pass by agreeing on nothing.
    let rails_of = |name: &str| read(&rails_root.path().join(name).join("rails.json"));
    for value in ["NaN", "Infinity", "-Infinity", "garbage"] {
        for fallback in ["market", "stale"] {
            let out = rails_of(&format!("balances-unreadable_price_{value}_{fallback}"));
            let balances = rows(&out, "account_balances");
            let row = |id| balances.iter().find(|r| r["after"]["asset_id"] == id).unwrap()["after"].clone();
            assert_eq!((num(&row(1)["usd_value"]), num(&row(3)["usd_value"])), (Some(100.0), Some(400.0)), "cash and the healthy stock are freshly valued");
            let aapl = row(2);
            assert_eq!(num(&aapl["free"]), Some(3.0), "the unreadable price does not prevent a holding update");
            let (price, at) = if fallback == "market" { (221.0, "2026-09-20 02:30:00.750000") } else { (220.5, "2026-09-10 02:30:00") };
            assert_eq!((num(&aapl["usd_price"]), num(&aapl["usd_value"]), aapl["priced_at"].as_str()), (Some(price), Some(3.0 * price), Some(at)));
            assert_eq!(out["steps"][0]["raised"], false);
            assert!(key_error(&out).is_null());
        }
    }
    assert_eq!(rows(&rails_of("ledger-fills"), "account_transactions").len(), 9);
    assert_eq!(rows(&rails_of("ledger-pages_three"), "account_transactions").len(), 205);
    assert_eq!(rails_of("ledger-pages_three")["steps"][0]["requests"].as_array().unwrap().len(), 3);
    assert!(rows(&rails_of("ledger-page_two_fails"), "account_transactions").is_empty(), "a failed page stores nothing");
    assert!(rows(&rails_of("ledger-idle"), "api_keys").is_empty(), "an idle sync writes nothing to the key");
    assert_eq!(after_of(&rails_of("ledger-rerun"), 1), "2026-09-10T23:00:00Z", "the watermark less 25 h");
    assert_eq!(rows(&rails_of("ledger-skipped_rows_hold_watermark"), "api_keys")[0]["after"]["last_synced_at"], "2026-05-16 00:00:00");
    assert_eq!(rails_of("ledger-network_pre_send")["steps"][0]["raised"], true);
    let split = rows(&rails_of("ledger-split_forward"), "account_transactions");
    assert_eq!((split.len(), &split[0]["after"]["raw_data"]["split_ratio"]), (1, &json!("10:1")));
    assert_eq!(rows(&rails_of("ledger-split_reverse"), "account_transactions")[0]["after"]["raw_data"]["split_ratio"], "1:3");
    assert_eq!(rows(&rails_of("ledger-split_three_legs"), "account_transactions")[0]["after"]["raw_data"]["merged_activity_ids"].as_array().unwrap().len(), 3);
    // The counter moves only for bots that traded the split symbol on the venue: one of two here, none of one there.
    assert_eq!(rows(&rails_of("ledger-split_forward"), "bots").iter().map(|b| b["id"].clone()).collect::<Vec<_>>(), [json!(1)]);
    assert_eq!(rails_of("ledger-split_forward")["steps"][0]["generations"], json!([[1, 1], [2, 0]]), "legs that arrive together move the counter once");
    let untraded = rails_of("ledger-split_untraded_symbol");
    assert_eq!((rows(&untraded, "account_transactions").len(), rows(&untraded, "bots").len(), rows(&untraded, "bot_activity_logs").len()), (1, 0, 0));
    // Split rows are Rails' own, defects included (the comparison above found every one identical). What Rails does
    // with legs that do not arrive as one consecutive group, pinned here because the plan lists each with its amounts:
    // [tx_id, base_amount, ratio] of every split row, and the bot's counter after each sync.
    let splits = |name: &str| {
        let out = rails_of(name);
        let stored: Vec<Value> = rows(&out, "account_transactions").iter().filter(|r| r["after"]["entry_type"] == 15)
            .map(|r| json!([r["after"]["tx_id"], r["after"]["base_amount"], r["after"]["raw_data"]["split_ratio"]])).collect();
        let counters: Vec<Value> = out["steps"].as_array().unwrap().iter().map(|s| s["generations"][0][1].clone()).collect();
        (json!(stored), json!(counters))
    };
    assert_eq!(splits("ledger-split_remove_then_pair"), (json!([["klac-remove", -10, null]]), json!([1, 1])), "the removal first: the pair is skipped, the row stays one-legged");
    assert_eq!(splits("ledger-split_add_then_pair"), (json!([["klac-add", 100, null], ["klac-remove", 90, "10:1"]]), json!([1, 2])), "the addition first: the pair beside it, +190 where +90 is true");
    assert_eq!(splits("ledger-split_third_leg_later"), (json!([["ssp-remove", 5, "3:2"]]), json!([1, 1])), "a third leg later: skipped, +5 and 3:2 where +20 and 3:1 are true");
    assert_eq!(splits("ledger-split_overlap_new_leg_first"), (json!([["ssp-remove", 5, "3:2"], ["ssp-add-2", 30, null]]), json!([1, 2])), "a shared leg, the new one first: counted twice");
    assert_eq!(splits("ledger-split_overlap_new_leg_last"), (json!([["ssp-remove", 5, "3:2"]]), json!([1, 1])), "a shared leg, the new one last: the new leg lost");
    assert_eq!(splits("ledger-split_legs_apart"), (json!([["x-remove", -2, null], ["y-remove", -100, null], ["x-add", 4, null]]), json!([2])), "legs with activities between them: two rows, no ratio");
    assert_eq!(splits("ledger-split_two_on_one_date"), (json!([["one-remove", 50, "8:3"]]), json!([1])), "two splits on one date: one row, 8:3 where 2:1 and then 3:1 are true");
    assert_eq!(splits("ledger-split_leg_changed"), (json!([["klac-remove", 90, "10:1"]]), json!([1, 1])), "a corrected leg under the same id: skipped");
    assert_eq!(splits("ledger-split_two_near_one_on_one_date"), (json!([["one-remove", 5, "287:286"]]), json!([1])), "1000 to 1002 to 1005: one row, 287:286 where the position went 1.005");
    assert_eq!(splits("ledger-split_two_alike_on_one_date"), (json!([["one-remove", 5, "182:181"]]), json!([1])), "1000 to 100 to 1005: one row that reads as one split of 182:181");
    // The contract for Plan 2d, applied to what Rails stored (and Rust, identically): which groups of one symbol and
    // date a factor may be taken from, and which stored shapes say the split cannot be trusted.
    let verdicts = |name: &str| split_verdicts(&rails_of(name));
    for whole in ["forward", "reverse", "three_legs", "across_pages", "pair_then_leg", "future", "untraded_symbol", "old", "holders", "nested_duplicate_keys"] {
        assert_eq!(verdicts(&format!("ledger-split_{whole}")).iter().map(|v| v.rsplit(": ").next().unwrap_or_default()).collect::<Vec<_>>(), ["trusted"], "{whole}");
    }
    assert_eq!(verdicts("ledger-split_fractional"), ["QQQM 2026-09-15: trusted", "AAPL 2026-09-16: no ratio", "KLAC 2026-09-17: no ratio"]);
    assert_eq!(verdicts("ledger-split_unmerged"), ["KLAC 2026-09-10: a lone leg", "AAPL 2026-09-12: a lone leg", "AAPL 2026-09-13: a lone leg"]);
    assert_eq!(verdicts("ledger-split_remove_then_pair"), ["KLAC 2026-09-15: a lone leg"]);
    assert_eq!(verdicts("ledger-split_add_then_pair"), ["KLAC 2026-09-15: several rows"]);
    assert_eq!(verdicts("ledger-split_overlap_new_leg_first"), ["KLAC 2026-09-15: several rows"]);
    assert_eq!(verdicts("ledger-split_legs_apart"), ["QQQM 2026-09-14: several rows", "KLAC 2026-09-14: a lone leg"]);
    assert_eq!(verdicts("ledger-split_two_on_one_date"), ["KLAC 2026-09-15: not one split"]);
    assert_eq!(verdicts("ledger-split_two_near_one_on_one_date"), ["KLAC 2026-09-15: not one split"], "1.005 against 287:286 is a per-mille and a half apart");
    // Wrong, and nothing in the row says so: these four need the Rails fix, or the reader's check against the position
    // the venue reports. The last is two splits on one date whose merged ratio looks like one split's.
    for wrong in ["third_leg_later", "overlap_new_leg_last", "leg_changed", "two_alike_on_one_date"] {
        assert_eq!(verdicts(&format!("ledger-split_{wrong}")), ["KLAC 2026-09-15: trusted"], "{wrong}");
        use deltabadger::{engine::splits::position_agrees, ruby::BigDec};
        let out = rails_of(&format!("ledger-split_{wrong}"));
        let stored=rows(&out,"account_transactions")[0]["after"]["raw_data"]["split_ratio"].as_str().ok_or("missing stored ratio")?.to_string();
        let (new,old)=stored.split_once(':').ok_or("invalid stored ratio")?;
        let before=if wrong=="two_alike_on_one_date" {1000} else {10};
        let venue=match wrong { "leg_changed"=>200, "two_alike_on_one_date"=>1005, _=>30 };
        let expected=&BigDec::from_i64(before)*&BigDec::parse(new).map_err(|e| format!("{e:?}"))?.div(&BigDec::parse(old).map_err(|e| format!("{e:?}"))?).ok_or("invalid ratio division")?;
        assert!(!position_agrees(&expected,&BigDec::from_i64(venue)), "{wrong}: {stored} must stand down against {venue} shares");
    }
    // A key the venue sent twice is stored once, with its last value in its first place, at every level; and Rails
    // reads the row as a split (the comparison above found Rust's row identical).
    let twice = rails_of("ledger-split_nested_duplicate_keys");
    let stored = rows(&twice, "account_transactions")[0]["after"]["raw_data"].clone();
    assert_eq!((&stored["note"], &stored["extra"], twice["splits_read"].clone()), (&json!("last"), &json!({ "x": 3, "y": [{ "k": 2 }] }), json!(1)));
    assert_eq!(rows(&twice, "account_transactions").len(), 1, "the leg that comes again is a duplicate: its id is read out of merged_activity_ids");
    assert!(rows(&rails_of("ledger-nesting_over_limit"), "account_transactions").is_empty(), "a page nested past Ruby's limit is not read");
    // raw_data is compared as the text in the column, byte for byte (`raw_data_text`), in every scenario above. What
    // Rails writes there is its JSON encoder's rendering of what its parser read, not the venue's bytes:
    let floats = rows(&rails_of("ledger-raw_floats"), "account_transactions");
    let stored = floats[0]["after"]["raw_data_text"].as_str().unwrap();
    assert!(stored.contains(r#""a":0.3,"b":1.234567890123457,"c":1.0,"d":100.0,"e":0.0,"f":1e-07,"g":1e+20,"h":1000000000000000.0,"i":123456789012345680.0,"j":4.940656458412465e-324,"#), "{stored}");
    assert!(stored.contains(r#""l":1984.0207455399993,"m":0,"n":1.5,"o":[0.1,2.5e-05,{"p":1234567890123457}],"s":"a\u003cb\u003e\u0026c "#) && stored.contains(r#""t\u003c":null"#), "{stored}");
    assert!(floats[1]["after"]["raw_data_text"].as_str().unwrap().contains(r#""price":170.1,"qty":0.3,"#), "a fill's JSON numbers: {}", floats[1]["after"]["raw_data_text"]);
    assert!(rows(&rails_of("ledger-unsupported_and_canceled"), "account_transactions").iter().any(|r| r["after"]["raw_data_text"].as_str().is_some_and(|t| t.contains(r#""net_amount":1e-05,"#))));
    let refused = rails_of("ledger-malformed_overwritten");
    assert!(rows(&refused, "account_transactions").is_empty() && key_error(&refused).as_str().is_some_and(|e| e.starts_with(r#"[{"id":"m-1""#)), "a body the parser refuses fails the fetch with the body as its text");
    // An integer no double holds is in raw_data as the venue sent it, on both sides (the comparison above was exact).
    let big = rows(&rails_of("ledger-raw_large_integer"), "account_transactions")[0]["after"]["raw_data"].clone();
    assert_eq!((&big["reference"], &big["nested"]["ids"]), (&json!("<integer 18446744073709551617>"), &json!(["<integer -9223372036854775809>", "<integer 123456789012345678901234567890>"])));
    assert_eq!(rows(&rails_of("ledger-split_holders"), "bots").len(), 8);
    assert_eq!(rows(&rails_of("balances-first"), "account_balances").len(), 6);
    assert_eq!(rows(&rails_of("balances-account_unauthorized"), "api_keys")[0]["after"]["status"], 2);
    assert!(rows(&rails_of("balances-empty_account"), "account_balances").iter().all(|r| r["after"].is_null()), "gone positions are removed");

    // Handbacks: Rust runs a scenario's first night, Rails runs its second on Rust's copy.
    let hand_back = |scenario: &str| {
        let dir = handback.path().join(scenario);
        let whole = read(&dir.join("scenario.json"));
        let only = move |dir: &Path, step: usize| { let mut s = whole.clone(); s["steps"] = json!([whole["steps"][step]]); std::fs::write(dir.join("scenario.json"), s.to_string()).unwrap(); };
        (dir, only)
    };
    // Inside the window of a split dated ahead: Rails reads from Rust's capped watermark, so it asks from the first
    // sync's start less 25 h and stores what happened in between; its own watermark then stops at the second night's start.
    let (dir, only) = hand_back("ledger-split_future");
    only(&dir, 0);
    let rust_night = deltabadger::sync::parity::run(&dir, cipher.clone()).await.unwrap();
    assert_eq!(rows(&rust_night, "api_keys")[0]["after"]["last_synced_at"], "2026-09-20 02:00:00.250000", "Rust's watermark is the sync's start, not the split's date");
    only(&dir, 1);
    // A split Rust stored from legs with keys sent twice and nesting at Ruby's limit: Rails reads the row as it reads
    // its own (a split row, with its merged legs), so the leg that comes again is a duplicate and nothing moves.
    let (dup_dir, only) = hand_back("ledger-split_nested_duplicate_keys");
    only(&dup_dir, 0);
    let rust_night = deltabadger::sync::parity::run(&dup_dir, cipher.clone()).await.unwrap();
    assert_eq!((rows(&rust_night, "account_transactions").len(), rust_night["splits_read"].clone()), (1, json!(1)));
    only(&dup_dir, 1);

    rails(&["record", handback.path().to_str().unwrap()]);
    let rails_night = read(&dir.join("rails.json"));
    assert_eq!(after_of(&rails_night, 0), "2026-09-19T01:00:00Z");
    assert_eq!(rows(&rails_night, "account_transactions").iter().map(|r| r["after"]["tx_id"].clone()).collect::<Vec<_>>(), [json!("f-between"), json!("int-between")],
               "Rails stores the fill and the interest, and reads the split it finds stored as a duplicate");
    assert_eq!(rows(&rails_night, "api_keys")[0]["after"]["last_synced_at"], "2026-09-21 02:00:07.500000");
    let rails_night = read(&dup_dir.join("rails.json"));
    let own = rails_of("ledger-split_nested_duplicate_keys");
    assert_eq!((rows(&rails_night, "account_transactions").len(), rails_night["splits_read"].clone(), rails_night["steps"][0]["generations"].clone()),
               (0, json!(1), own["steps"][1]["generations"].clone()), "Rails on Rust's row: the same as Rails on its own");
    assert!(rows(&rails_night, "bots").is_empty() && rows(&rails_night, "bot_activity_logs").is_empty());
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn the_rust_half_refuses_anything_but_a_marked_scratch_copy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("scenario.json"), r#"{"api_key_id": 1, "steps": []}"#).unwrap();
    let err = deltabadger::sync::parity::run(dir.path(), cipher()).await.unwrap_err();
    assert!(err.0.contains("parity_scratch"), "{err:?}");
}

/// A step whose script lacks a call the job makes must fail the Rails run loudly (never reach the network).
#[test]
fn an_unscripted_call_fails_the_rails_run() {
    let built = tempfile::tempdir().unwrap();
    rails(&["grid", built.path().to_str().unwrap()]);
    for (scenario, emptied) in [("ledger-fills", "alpaca"), ("balances-first", "market")] {
        let root = tempfile::tempdir().unwrap();
        copy_dir(&built.path().join(scenario), &root.path().join("one"));
        let path = root.path().join("one/scenario.json");
        let mut sc = read(&path);
        sc["steps"][0][emptied] = serde_json::json!({});
        std::fs::write(&path, sc.to_string()).unwrap();
        let err = try_rails(&["record", root.path().to_str().unwrap()]).expect_err(&format!("{scenario}: an unscripted call must fail the run"));
        assert!(err.contains("unscripted"), "{scenario}: {err}");
    }
}
