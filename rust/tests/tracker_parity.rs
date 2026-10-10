#![cfg(unix)] // shells out to bin/rails
//! Value parity for the tracker's walk, figures, snapshots and wash-sale locks: Rails' real jobs (Tracker::LedgerJob,
//! PortfolioSnapshot::BackfillJob) and the Rust jobs run on copies of one install over the same scripted data-api and
//! Alpaca bodies, at one instant. The scopes the walk stated, the figures of each of today's rows, the snapshot rows,
//! the lock rows, the stored prices, the history keys and the requests sent must be identical, except in the listed
//! divergences, where Rails' result and Rust's are each asserted.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn rails(args: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let run = |rails_args: &[&str]| {
        let mut cmd = Command::new(root.join("bin/rails"));
        cmd.current_dir(root).args(rails_args).env_remove("DATABASE_URL").env("TZ", "UTC")
            .env("APP_ROOT_URL", "http://localhost:3000").env("SKIP_TEST_DATABASE", "true");
        for db in ["primary", "queue", "cache", "cable"] {
            cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
        }
        let out = cmd.output().expect("bin/rails runs");
        assert!(out.status.success(), "bin/rails {rails_args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
    };
    run(&["db:schema:load"]);
    let mut full = vec!["runner", "script/rust/tracker.rb"];
    full.extend_from_slice(args);
    run(&full);
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

fn cipher() -> Arc<deltabadger::crypto::Cipher> {
    Arc::new(deltabadger::crypto::Cipher::new(&deltabadger::crypto::EncryptionKeys::resolve(&|_| None, "tracker-parity").unwrap()))
}

fn read(path: &Path) -> Value { serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap() }

/// A history Rust refuses: Rails states its figures, Rust writes no snapshot and says why (its reason starts so).
fn refused(rails: &Value, rust: &Value, why: &str) -> Result<(), String> {
    if rails["ledger_error"] != Value::Null || rails["tables"]["portfolio_snapshots"].as_array().is_none_or(Vec::is_empty) {
        return Err("Rails no longer computes this history: drop the listed divergence".into());
    }
    let says = |v: &Value| v.as_str().is_some_and(|m| m.starts_with(why));
    if !says(&rust["ledger_error"]) || !says(&rust["backfill_error"]) { return Err(format!("Rust: {} / {}", rust["ledger_error"], rust["backfill_error"])); }
    if rust["tables"]["portfolio_snapshots"] != json!([]) || rust["tables"]["portfolio_venue_snapshots"] != json!([]) { return Err("Rust wrote a snapshot".into()); }
    Ok(())
}

/// R: legacy fixture balances have no credential provenance. Retain every historical
/// amount; only today's two snapshots are partial until a producer sync succeeds.
const UNKNOWN_ORIGIN: &[&str] = &["a_failed_price_fetched_again", "bought_since_the_sync", "buys_priced", "coin_fee_stated_price", "coin_fee_then_withdrawal", "departed_and_cash_short", "dividends_fees_withdrawal", "linked_dollars", "old_loss_outside_the_horizon", "return_of_capital", "sale_gain_with_a_losing_lot", "sell_at_loss_wash_off", "sell_at_loss_wash_on", "sold_before_any_buy", "split_priced_by_a_second_fetch"];
fn unknown_origin(name:&str,rails:&Value,rust:&Value)->Option<Result<(),String>> {
    if !UNKNOWN_ORIGIN.contains(&name){return None;}
    Some((|| {
        let mut expected=rails.clone();
        for table in ["portfolio_snapshots","portfolio_venue_snapshots"] {
            let rows=expected["tables"][table].as_array_mut().ok_or("missing snapshot table")?;
            let today=rows.iter_mut().filter(|row|row["date"]=="2026-10-01").collect::<Vec<_>>();
            if today.len()!=1{return Err(format!("{table}: expected exactly one current fixture row"));}
            for row in today {
                if row["partial"]!=0{return Err(format!("{table}: Rails no longer calls the legacy figure complete"));}
                row["partial"]=json!(1);
            }
        }
        if expected==*rust{Ok(())}else{Err("a field besides today's reviewed partial flags differs".into())}
    })())
}

/// Scenarios where Rust deliberately does not do what Rails does, each asserted on its own terms.
fn listed(name: &str, rails: &Value, rust: &Value) -> Option<Result<(), String>> {
    let not_ported = |what: &str| format!("the tracker walk is not ported for {what}");
    Some(match name {
        // Rails values a fee nobody could price at zero (a partial day); Rust states no figure on a zero basis.
        "coin_fee_unpriced" => refused(rails, rust, "no price of BTC on 2026-09-10 after 2 fetches"),
        // Histories this build does not walk.
        "refused_non_cash_quote" => refused(rails, rust, &not_ported("a trade leg with no cash quote of its own")),
        "refused_other_venue_balance" => refused(rails, rust, &not_ported("balances on a venue other than Alpaca")),
        "refused_swap_legs" => refused(rails, rust, &not_ported("swap legs")),
        // Rails values a coin no catalogue names at zero without asking; Rust refuses.
        "refused_unnamed_coin" => refused(rails, rust, "no price of XYZ on 2026-09-10: no coin can be named"),
        _ => return None,
    })
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_state_identical_figures_snapshots_and_locks_across_the_tracker_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 24, "the tracker grid");
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let cipher = cipher();
    let (mut failures, mut divergences) = (vec![], vec![]);
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rust_out = deltabadger::tracker::parity::run(&rust_root.path().join(&name), cipher.clone()).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        let rails_out = read(&d.join("rails.json"));
        match unknown_origin(&name,&rails_out,&rust_out).or_else(||listed(&name, &rails_out, &rust_out)) {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            Some(Ok(())) => divergences.push(name),
            None if rails_out != rust_out => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            None => {}
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    let mut expected=UNKNOWN_ORIGIN.iter().map(|name|name.to_string()).collect::<Vec<_>>();
    expected.extend(["coin_fee_unpriced","refused_non_cash_quote","refused_other_venue_balance","refused_swap_legs","refused_unnamed_coin"].iter().map(|name|name.to_string()));
    expected.sort();
    assert_eq!(divergences,expected,"all reviewed legacy-origin and original divergences are exercised");

    // What the grid must have exercised, read from Rails' own output.
    let rails_of = |name: &str| read(&rails_root.path().join(name).join("rails.json"));
    let snapshots = |out: &Value| out["tables"]["portfolio_snapshots"].as_array().unwrap().clone();
    let today = |out: &Value| snapshots(out).into_iter().find(|r| r["date"] == "2026-10-01").unwrap();
    assert_eq!(snapshots(&rails_of("empty")), Vec::<Value>::new());
    assert_eq!(rails_of("empty")["tables"]["snapshot_history_version_1"], "1_0_");
    assert_eq!(snapshots(&rails_of("buys_priced")).len(), 31, "every day from the first transaction, and today");
    assert!(snapshots(&rails_of("buys_priced")).iter().any(|r| r["partial"] == 1 && r["date"].as_str() < Some("2026-10-01")), "a hole past the carry limit is partial");
    let lock = |name: &str| rails_of(name)["tables"]["wash_sale_locks"].clone();
    assert_eq!(lock("sell_at_loss_wash_on")[0]["buy_locked_until"], "2026-10-21 00:00:00");
    assert_eq!(lock("sell_at_loss_wash_on")[0]["source"], "ledger");
    assert_eq!(lock("sell_at_loss_wash_off"), json!([]));
    assert_eq!(lock("old_loss_outside_the_horizon"), json!([]));
    assert_eq!(rails_of("coin_fee_then_withdrawal")["ledger"]["whole"]["total_invested"], "80.0", "the withdrawal takes the $20 lot's basis");
    assert_eq!(lock("a_failed_price_fetched_again")[0]["buy_locked_until"], "2026-10-11 00:00:00", "the second fetch's price arms the loss");
    assert_eq!(rails_of("a_failed_price_fetched_again")["requests"]["market"].as_array().unwrap().len(), 2, "the empty fetch, then again");
    assert_eq!((lock("sale_gain_with_a_losing_lot")[0]["buy_locked_until"].clone(), lock("sale_gain_with_a_losing_lot")[0]["confirmed_locked_until"].clone()),
               (json!("2026-12-01 00:00:00"), json!("2026-10-24 00:00:00")), "a net gain with a losing lot still locks; a longer lock is kept");
    assert_eq!(rails_of("split_priced_by_a_second_fetch")["requests"]["market"].as_array().unwrap().len(), 2, "the prefetch, then the day's own window");
    assert_eq!(rails_of("coin_fee_unpriced")["requests"]["market"].as_array().unwrap().len(), 5);
    assert_eq!(today(&rails_of("coin_fee_unpriced"))["partial"], 1, "a price nobody had");
    assert_eq!(rails_of("coin_fee_stated_price")["requests"]["market"].as_array().unwrap().len(), 1, "the backfill's range only");
    for name in ["a_failed_sync_and_stale_prices", "stale_prices_alone", "cash_beyond_the_history_and_an_unpriced_holding"] {
        assert_eq!(today(&rails_of(name))["partial"], 1, "{name}");
    }
    assert_eq!(today(&rails_of("buys_priced"))["partial"], 0);
}
