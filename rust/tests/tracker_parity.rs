#![cfg(unix)] // shells out to bin/rails
//! Value parity for the tracker's walk, figures, snapshots and wash-sale locks: Rails' real jobs (Tracker::LedgerJob,
//! PortfolioSnapshot::BackfillJob) and the Rust jobs run on copies of one install over the same scripted data-api and
//! Alpaca bodies, at one instant. The scopes the walk stated, the figures of each of today's rows, the snapshot rows,
//! the lock rows, the stored prices, the history keys and the requests sent must be identical, except in the listed
//! divergences, where Rails' result and Rust's are each asserted. A scenario that says "diverges: D-TRACKER-…" states the
//! Rust output checked by hand and committed in fixtures/tracker_divergences/<name>.json (TRACKER_DIVERGENCES_WRITE=1 writes
//! them afresh, for review), Rails' differs, and `reviewed_figures` asserts the figures it was reviewed for.
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
    run(&["runner", "script/rust/schema_template.rb"]);
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

/// A reviewed divergence's Rust output (TRACKER_DIVERGENCES_WRITE=1 writes it afresh, for review).
fn reviewed(name: &str) -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tracker_divergences").join(format!("{name}.json")) }

/// The figures each reviewed divergence was checked by hand to state (Rust, and Rails where it names Rails' error).
fn reviewed_figures(outputs: &std::collections::HashMap<String, (Value, Value)>) {
    let side = |name: &str, rails: bool| { let (a, b) = &outputs[name]; if rails { a.clone() } else { b.clone() } };
    // "SYMBOL quantity@cost" of a scope's positions, in symbol order; a scope is "whole" or an exchange id.
    let positions = |name: &str, scope: &str, rails: bool| -> String {
        let out = side(name, rails);
        let s = if scope == "whole" { out["ledger"]["whole"].clone() } else { out["ledger"]["venues"][scope].clone() };
        s["positions"].as_array().unwrap().iter().map(|p| format!("{} {}@{}", p["symbol"].as_str().unwrap(), p["quantity"].as_str().unwrap(), p["cost"].as_str().unwrap()))
            .collect::<Vec<_>>().join(", ")
    };
    let field = |name: &str, scope: &str, key: &str, rails: bool| -> String {
        let out = side(name, rails);
        let s = if scope == "whole" { out["ledger"]["whole"].clone() } else { out["ledger"]["venues"][scope].clone() };
        s[key].as_str().map_or_else(|| s[key].to_string(), str::to_string)
    };
    let figure = |name: &str, key: &str, rails: bool| side(name, rails)["ledger"]["figures"]["whole"][key].as_str().unwrap().to_string();
    let snapshot = |name: &str, date: &str, rails: bool| -> (String, String) {
        let out = side(name, rails);
        let row = out["tables"]["portfolio_snapshots"].as_array().unwrap().iter().find(|r| r["date"] == date).unwrap().clone();
        let num = |v: &Value| match v { Value::Object(m) => f64::from_be_bytes(u64::from_str_radix(m["f"].as_str().unwrap(), 16).unwrap().to_be_bytes()).to_string(), v => v.to_string() };
        (num(&row["value_usd"]), num(&row["held_value_usd"]))
    };
    // D-TRACKER-COIN-QUOTE. Each of the nine venues: 0.5 BTC sold for 10 ETH carries its 25,000 to the ETH; Rails realised 5,000.
    for venue in 2..=10 {
        let v = venue.to_string();
        assert_eq!(positions("cq_every_venue", &v, false), "BTC 0.5@25000.0, ETH 10.0@25000.0", "venue {v}");
        assert_eq!((field("cq_every_venue", &v, "realised", false), field("cq_every_venue", &v, "realised", true)), ("0.0".into(), "5000.0".into()));
        assert_eq!(positions("cq_every_venue", &v, true), "BTC 0.5@25000.0");
    }
    assert_eq!(figure("cq_every_venue", "unrealised", false), "90000.0");
    // A fee in the coin paid goes with it (taken at its basis: −50); in the coin received, it nets the units; a third coin, its value joins the cost.
    assert_eq!(positions("cq_buy_fee_in_quote", "whole", false), "BTC 0.499@24950.0, ETH 10.0@25000.0");
    assert_eq!((field("cq_buy_fee_in_quote", "whole", "realised", false), field("cq_buy_fee_in_quote", "whole", "fees", false)), ("-50.0".into(), "60.0".into()));
    assert_eq!(positions("cq_buy_fee_in_quote", "whole", true), "BTC 1.0@50000.0, ETH 10.0@30060.0");
    assert!(positions("cq_buy_fee_in_base", "whole", false).starts_with("BTC 0.5@25000.0, ETH 9.99@24999.99"));
    assert_eq!(positions("cq_sell_fee_third_coin", "whole", false), "BTC 0.5@25000.0, ETH 10.0@25010.0, KCS 99.0@990.0");
    assert_eq!(field("cq_sell_fee_third_coin", "whole", "fees", false), "10.0");
    assert!(positions("cq_sell_fee_in_quote", "whole", false).starts_with("BTC 0.5@25000.0, ETH 9.99@24999.99"));
    // D-TRACKER-SWAP-FEE: over two lots the KCS fee joins once (50,005 under Rails' allocation).
    assert_eq!(positions("cq_sell_fee_third_coin_two_lots", "whole", false), "ETH 20.0@50010.0, KCS 99.0@990.0");
    assert_eq!(figure("cq_sell_fee_third_coin_two_lots", "unrealised", false), "9990.0");
    assert_eq!(positions("cq_binance_two_lot_swap_fee", "whole", false), "BNB 0.9@270.0, BTC 0.05@2030.0");
    assert_eq!(positions("cq_binance_two_lot_swap_fee", "whole", true), "BNB 0.9@270.0, BTC 0.05@2015.0");
    // A quote coin never held opens what was spent (money in); Rails held ETH with nothing in.
    assert_eq!((field("cq_quote_never_held", "whole", "openings", false), field("cq_quote_never_held", "whole", "total_invested", false)), (r#"[["BTC","0.5"]]"#.into(), "30000.0".into()));
    assert_eq!(field("cq_quote_never_held", "whole", "total_invested", true), "0.0");
    // A cash base sold or bought for a coin reads as the cash trade booked the usual way (the control venue, id 3).
    assert_eq!(positions("cq_stable_base_sell", "2", false), positions("cq_stable_base_sell", "3", false));
    assert_eq!(positions("cq_stable_base_sell", "2", false), "BTC 1.0@50000.0");
    for key in ["realised", "cash", "total_invested"] { assert_eq!(field("cq_stable_base_buy", "2", key, false), field("cq_stable_base_buy", "3", key, false), "{key}"); }
    assert_eq!((positions("cq_stable_base_buy", "2", false), field("cq_stable_base_buy", "2", "realised", false)), ("BTC 0.5@25000.0".into(), "5000.0".into()));
    // Its fee leaves the cash, in the ledger and the history alike (D-TRACKER-HISTORY-SELL-FEE).
    assert_eq!((positions("cq_stable_base_sell_fee", "whole", false), field("cq_stable_base_sell_fee", "whole", "cash", false)), ("BTC 1.0@50000.0".into(), r#"[["USDT","0.0"]]"#.into()));
    assert_eq!(snapshot("cq_stable_base_sell_fee", "2026-09-16", false), ("60000".into(), "60000".into()), "no USDT left");
    assert_eq!(snapshot("cq_stable_base_sell_fee", "2026-09-16", true), ("60010".into(), "60000".into()));
    assert_eq!((positions("cq_fiat_base_sell_fee", "whole", false), field("cq_fiat_base_sell_fee", "whole", "fees", false)), ("BTC 0.02@1102.2".into(), "2.2".into()));
    assert_eq!(snapshot("cq_fiat_base_sell_fee", "2026-09-16", false), ("1200".into(), "1200".into()));
    assert_eq!((field("cq_stable_base_sell_fee_short", "whole", "total_invested", false), field("cq_stable_base_sell_fee_short", "whole", "cash", false)),
               ("50020.0".into(), r#"[["USDT","10.0"]]"#.into()));
    assert_eq!(snapshot("cq_stable_base_sell_fee_short", "2026-09-16", false), ("60010".into(), "60000".into()));
    // Since the sync: both coins move (ETH, unpriced at the venue, at its cost).
    let holdings = |name: &str, rails: bool| side(name, rails)["ledger"]["figures"]["whole"]["holdings"].as_array().unwrap().iter()
        .map(|h| format!("{} {}", h["symbol"].as_str().unwrap(), h["quantity"].as_str().unwrap())).collect::<Vec<_>>().join(", ");
    assert_eq!((holdings("cq_since_sync_buy", false), figure("cq_since_sync_buy", "value", false)), ("BTC 0.499, ETH 10.0".into(), "54940.0".into()));
    assert_eq!(holdings("cq_since_sync_buy", true), "BTC 0.999, ETH 10.0");
    assert_eq!((holdings("cq_since_sync_sell", false), figure("cq_since_sync_sell", "value", false)), ("BTC 0.5, ETH 10.0".into(), "55000.0".into()));
    assert_eq!(holdings("cq_since_sync_sell", true), "BTC 0.5");
    assert_eq!((positions("cq_same_instant_buy_then_split", "whole", false), field("cq_same_instant_buy_then_split", "whole", "realised", true)), ("ETH 20.0@50000.0".into(), "10000.0".into()));
    assert_eq!((positions("cq_quote_unpriced", "whole", false), field("cq_quote_unpriced", "whole", "incomplete", false)), ("BTC 0.5@25000.0, XYZ 10.0@25000.0".into(), "true".into()));
    // No loss, so no lock (Rails: BTC locked to 6 October); a lock already armed stays as it was.
    assert_eq!(side("cq_wash_coin_quoted_loss", false)["tables"]["wash_sale_locks"], json!([]));
    assert_ne!(side("cq_wash_coin_quoted_loss", true)["tables"]["wash_sale_locks"], json!([]));
    assert_eq!(side("cq_wash_existing_lock", false)["tables"]["wash_sale_locks"], side("cq_wash_existing_lock", true)["tables"]["wash_sale_locks"]);
    assert_eq!(positions("cq_group_collision", "whole", false), positions("cq_every_venue", "2", false));
    // D-TRACKER-HISTORY-SELL-FEE: the history holds 0.49 BTC as the ledger does (Rails 0.5).
    assert_eq!(snapshot("cq_history_sell_fee", "2026-09-16", false).1, "29400");
    assert_eq!(snapshot("cq_history_sell_fee", "2026-09-16", true).1, "30000");
    assert_eq!(snapshot("same_coin_sale_fee", "2026-09-20", false), ("1599.4".into(), "599.4".into()));
    assert_eq!(snapshot("fees_fifo_never_applied", "2026-09-09", false), ("85340".into(), "85140".into()), "0.005 ETH less at the history's close");
    // D-TRACKER-TRANSFER-ORDER (A Binance 2, B Kraken 3, C Bybit 4): no phantom opening, nothing invested twice.
    for (name, rust, rails) in [("to_same_second_transfer_past_deposit", "10000.0", "30000.0"), ("to_chain_funded_by_deposit", "60000.0", "120000.0"),
                                ("to_chain_ab_bc", "60000.0", "120000.0"), ("to_reciprocal_ab_ba", "60000.0", "120000.0"), ("to_cycle_unfunded", "60000.0", "120000.0"),
                                ("to_cycle_funded_forward", "60000.0", "180000.0"), ("to_opening_then_reciprocal", "60000.0", "120000.0"),
                                ("to_adjustments_then_reciprocal", "60000.0", "120000.0")] {
        assert_eq!((field(name, "whole", "total_invested", false), field(name, "whole", "total_invested", true)), (rust.into(), rails.into()), "{name}");
    }
    assert_eq!((field("to_same_second_transfer_past_deposit", "whole", "realised", false), field("to_same_second_transfer_past_deposit", "whole", "openings", false)), ("10000.0".into(), "[]".into()));
    assert_eq!(positions("to_chain_funded_by_deposit", "whole", false), "BTC 1.0@60000.0");
    assert_eq!(field("to_cycle_unfunded", "4", "openings", false), r#"[["BTC","1.0"]]"#, "one opening, on C");
    assert_eq!(field("to_opening_then_reciprocal", "2", "openings", false), r#"[["BTC","1.0"]]"#, "the opening the earlier withdrawal forced, on A alone");
    assert_eq!((positions("to_adjustments_then_reciprocal", "2", false), positions("to_adjustments_then_reciprocal", "3", false)), ("".into(), "BTC 1.0@60000.0".into()));
    // The parity cases of the same schedule (Rails' order is already funded).
    for name in ["to_chain_bc_ab", "to_reciprocal_ba_ab", "to_cycle_funded_reverse", "cq_stable_quoted_sale", "to_cash_linked_deposit_fee_fx"] {
        assert_eq!(side(name, false), side(name, true), "{name}");
    }
    assert_eq!(field("to_cash_linked_deposit_fee_fx", "4", "cash_basis", false), r#"[["EUR","101.0"]]"#, "C's euros carry the basis the deposit's fee left");
}

/// R: legacy fixture balances have no credential provenance. Retain every historical
/// amount; R4 refuses today's snapshots and associated wash-lock publication until a producer sync succeeds.
const UNKNOWN_ORIGIN: &[&str] = &["other_venue_balance", "a_failed_sync_and_stale_prices", "stale_prices_alone", "cash_beyond_the_history_and_an_unpriced_holding", "a_failed_price_fetched_again", "bought_since_the_sync", "buys_priced", "coin_fee_stated_price", "coin_fee_then_withdrawal", "departed_and_cash_short", "dividends_fees_withdrawal", "linked_dollars", "old_loss_outside_the_horizon", "return_of_capital", "sale_gain_with_a_losing_lot", "sell_at_loss_wash_off", "sell_at_loss_wash_on", "sold_before_any_buy", "split_priced_by_a_second_fetch"];
fn unknown_origin(name:&str,rails:&Value,rust:&Value,before_locks:&Value)->Option<Result<(),String>> {
    if !UNKNOWN_ORIGIN.contains(&name){return None;}
    Some((|| {
        if rails["ledger_error"]!=Value::Null || rails["ledger"]==Value::Null{return Err("Rails no longer publishes this legacy balance calculation".into());}
        let mut expected=rails.clone();
        expected["ledger"]=Value::Null;
        expected["ledger_error"]=json!(deltabadger::engine::model::CREDENTIALS_CHANGED);
        expected["tables"]["wash_sale_locks"]=before_locks.clone();
        for table in ["portfolio_snapshots","portfolio_venue_snapshots"] {
            let rows=expected["tables"][table].as_array_mut().ok_or("missing snapshot table")?;
            if !rows.iter().any(|row|row["date"]=="2026-10-01"){return Err(format!("{table}: expected a current fixture row"));}
            rows.retain(|row|row["date"]!="2026-10-01");
        }
        if expected==*rust{Ok(())}else{Err("a field besides the refused ledger/current snapshots/unchanged wash locks differs; retained history and amounts must compare exactly".into())}
    })())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_state_identical_figures_snapshots_and_locks_across_the_tracker_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 67 + 22 + 12, "the tracker grid");
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let cipher = cipher();
    let (mut failures, mut divergences) = (vec![], vec![]);
    let mut reviewed_names: Vec<String> = vec![];
    let mut outputs: std::collections::HashMap<String, (Value, Value)> = std::collections::HashMap::new();
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let scenario=read(&rust_root.path().join(&name).join("scenario.json"));
        let initial=rusqlite::Connection::open(rust_root.path().join(&name).join("production.sqlite3")).unwrap();
        let before_locks=deltabadger::tracker::parity::tables(&initial,&cipher,scenario["user_id"].as_i64().unwrap()).unwrap()["wash_sale_locks"].clone();
        drop(initial);
        let rust_out = deltabadger::tracker::parity::run(&rust_root.path().join(&name), cipher.clone()).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        let rails_out = read(&d.join("rails.json"));
        // A reviewed divergence (D-TRACKER-…): Rust's output is the file checked by hand, and Rails' is not.
        if let Some(id) = scenario["expect"].as_str().and_then(|e| e.strip_prefix("diverges: ")) {
            assert!(id.split(" + ").all(|d| d.starts_with("D-TRACKER-")), "{name}: {id}");
            let path = reviewed(&name);
            let text = serde_json::to_string_pretty(&rust_out).unwrap() + "\n";
            if std::env::var("TRACKER_DIVERGENCES_WRITE").is_ok() { std::fs::create_dir_all(path.parent().unwrap()).unwrap(); std::fs::write(&path, &text).unwrap(); }
            if rails_out == rust_out { failures.push(format!("{name}: {id} names a divergence, but Rust stated Rails' output")); }
            match std::fs::read_to_string(&path) {
                Ok(want) if want == text => {}
                Ok(_) => failures.push(format!("{name}: not the reviewed {id} output ({})\n  rails: {rails_out}\n  rust:  {rust_out}", path.display())),
                Err(e) => failures.push(format!("{name}: {}: {e}\n  rails: {rails_out}\n  rust:  {rust_out}", path.display())),
            }
            reviewed_names.push(name.clone());
            outputs.insert(name, (rails_out, rust_out));
            continue;
        }
        match unknown_origin(&name,&rails_out,&rust_out,&before_locks) {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            Some(Ok(())) => divergences.push(name),
            None if rails_out != rust_out => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            None => { outputs.insert(name, (rails_out, rust_out)); }
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    // Every reviewed file is a diverging scenario's.
    for f in std::fs::read_dir(reviewed("x").parent().unwrap()).unwrap() {
        let f = f.unwrap().file_name().into_string().unwrap();
        assert!(reviewed_names.iter().any(|n| format!("{n}.json") == f), "{f} has no diverging scenario");
    }
    reviewed_figures(&outputs);
    let mut expected=UNKNOWN_ORIGIN.iter().map(|name|name.to_string()).collect::<Vec<_>>();
    expected.sort();
    assert_eq!(divergences,expected,"all reviewed legacy-origin and original divergences are exercised");

    // What the grid must have exercised, read from Rails' own output.
    let rails_of = |name: &str| read(&rails_root.path().join(name).join("rails.json"));
    // The page's figures (Plan tracker holdings S0b): every one stated somewhere in the grid, not only zeros, and only
    // where Rust's are compared (a listed divergence nulls Rust's expected ledger).
    let compared: Vec<&PathBuf> = dirs.iter().filter(|d| !UNKNOWN_ORIGIN.contains(&d.file_name().unwrap().to_string_lossy().as_ref())).collect();
    let summaries: Vec<Value> = compared.iter().flat_map(|d| {
        let ledger = read(&d.join("rails.json"))["ledger"].clone();
        std::iter::once(ledger["whole"].clone()).chain(ledger["venues"].as_object().map(|v| v.values().cloned().collect::<Vec<_>>()).unwrap_or_default())
    }).filter(|s| !s.is_null()).collect();
    let nonzero = |field: &str| summaries.iter().filter(|s| s[field].as_str().is_some_and(|v| v != "0.0")).count();
    for field in ["received", "realised", "fees", "cash_usd", "unpriced_proceeds"] { assert!(nonzero(field) > 0, "no scenario states {field}"); }
    assert!(summaries.iter().any(|s| s["realised"].as_str().is_some_and(|v| v.starts_with('-'))), "a realised loss");
    assert!(rails_of("return_of_capital_past_basis")["ledger"]["whole"]["realised"].as_str().is_some_and(|v| v != "0.0"), "return of capital past the basis is realised");
    assert!(summaries.iter().any(|s| s["openings"].as_array().is_some_and(|o| !o.is_empty())), "an opening");
    let trips: Vec<&Value> = summaries.iter().flat_map(|s| s["round_trips"].as_array().into_iter().flatten()).collect();
    assert!(trips.iter().any(|t| t["incomplete"] == true) && trips.iter().any(|t| t["incomplete"] == false), "complete and incomplete round trips");
    assert!(trips.iter().any(|t| t["opened_at"].is_string()), "a round trip's opening date");
    assert!(summaries.iter().flat_map(|s| s["positions"].as_array().into_iter().flatten()).all(|p| p["opened_at"].is_string()), "every position's date");
    // Tracker::Figures::Result: every field stated, every note kind written, on a holding and in the list.
    let figures: Vec<Value> = compared.iter().flat_map(|d| read(&d.join("rails.json"))["ledger"]["figures"].as_object().map(|f| f.values().cloned().collect::<Vec<_>>()).unwrap_or_default()).collect();
    for field in ["fees", "realised", "unrealised", "total"] { assert!(figures.iter().any(|f| f[field].as_str().is_some_and(|v| v != "0.0")), "no scenario states figures {field}"); }
    let holdings: Vec<&Value> = figures.iter().flat_map(|f| f["holdings"].as_array().into_iter().flatten()).collect();
    assert!(holdings.iter().any(|h| h["percent"].is_string()) && holdings.iter().any(|h| h["unrealised"].is_string()), "a holding's gain");
    let kinds: std::collections::HashSet<&str> = figures.iter().flat_map(|f| f["notes"].as_array().into_iter().flatten()).filter_map(|n| n["kind"].as_str()).collect();
    for kind in ["left", "arrived", "since_sync", "unpriced_held", "estimated", "cash_out", "cash_in", "unpriced", "figures_disagree", "at_par"] { assert!(kinds.contains(kind), "no scenario writes a {kind} note"); }
    assert!(holdings.iter().any(|h| h["note"].is_object()), "a holding's own note");
    assert!(rails_of("departed_coin_names_its_venue")["ledger"]["figures"]["whole"]["notes"].as_array().unwrap().iter()
        .any(|n| n["kind"] == "left" && n["held"] == "0.0" && n["exchange"].is_string()), "a departed coin's note names its venue");
    assert!(figures.iter().flat_map(|f| f["notes"].as_array().into_iter().flatten()).any(|n| n["exchange"].is_string()), "a note naming its venue");
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
    // Every venue's history (Plan D2b-2a), as Rails states it.
    let requests = |name: &str, kind: &str| rails_of(name)["requests"][kind].as_array().unwrap().len();
    assert_eq!((requests("ecb_stale_table_loads", "ecb"), requests("ecb_falls_back_to_bundesbank", "ecb"), requests("ecb_fails_then_loads", "ecb")), (1, 12, 13),
               "the ECB; the Bundesbank behind it; a failed load, then the next pass's load");
    assert_eq!(requests("repeated_same_day_missing", "market"), 11, "a day nobody prices is asked for at every lookup");
    assert_ne!(lock("wash_global_missing_located_priced"), json!([]), "compute!'s second pass reads the price the first stored");
    assert_eq!(lock("wash_global_missing_located_empty"), json!([]));
    assert_eq!(lock("wash_priced_buy_unpriced_sale"), json!([]), "a sale nobody priced has no verdict");
    assert_ne!(lock("wash_mixed_priced_unpriced_lots_loss"), json!([]));
    assert_ne!(lock("wash_fee_only_missing"), json!([]), "a fee nobody priced leaves the proceeds' verdict");
    for name in ["binance_unpriced_coin", "fx_missing_btc_cross", "standalone_fiat_fee_missing_fx", "kraken_style_fiat_leg_fee_buy_no_rate"] {
        assert_eq!(today(&rails_of(name))["partial"], 1, "{name}");
    }
    for name in ["binance_usdt_buy_bnb_fee", "binance_convert_and_dust", "binance_eur_deposit_and_buy", "same_coin_sale_fee", "kraken_style_fiat_leg_fee_sell"] {
        assert_eq!(today(&rails_of(name))["partial"], 0, "{name}");
    }
    let asked = rails_of("btc_coin_beside_btc_security")["requests"]["market"].clone();
    assert!(asked.as_array().unwrap().iter().any(|r| r.to_string().contains("\"bitcoin\"")) && !asked.to_string().contains("BTC.US"),
            "the coin's fees at a broker listing a BTC security are priced as bitcoin: {asked}");
    assert!(rails_of("btc_coin_beside_btc_security")["tables"]["historical_prices"].as_array().unwrap().iter()
        .filter(|r| r["asset"] == "BTC").all(|r| r["price"].to_string().contains("60000")), "the bare key holds bitcoin's closes only");
    assert_eq!(rails_of("binance_to_alpaca_linked_transfer")["ledger"]["venues"].as_object().unwrap().len(), 2, "the transfer's two venues");
    // The backfill for every venue (Plan D2b-2b).
    let ecb_path_calls = |name: &str| rails_of(name)["requests"]["ecb"].as_array().unwrap().iter().filter(|r| r[0].as_str().is_some_and(|k| k.contains("/service/data/EXR/"))).count();
    assert_eq!(ecb_path_calls("ecb_fails_then_loads_backfill"), 4, "the ledger's load and the backfill's three, the last one loading");
    let eur = snapshots(&rails_of("backfill_old_eur_cash_without_current_fx"));
    assert!(eur.iter().any(|r| r["partial"] == 0 && r["date"].as_str() < Some("2026-09-10")) && eur.iter().any(|r| r["partial"] == 1 && r["date"].as_str() < Some("2026-10-01")),
            "EUR valued while a rate is within a week, partial after");
    assert!(snapshots(&rails_of("borrowed_only_history")).len() > 20, "a history of the borrowed wallets alone");
    let venue_days = |name: &str| rails_of(name)["tables"]["portfolio_venue_snapshots"].as_array().unwrap().iter().map(|r| r["exchange_id"].as_i64().unwrap()).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(venue_days("binance_to_alpaca_linked_transfer").len(), 2, "each venue's history under its own exchange");
    assert_eq!(requests("backfill_prices_read_before_the_walk", "market"), 2, "the backfill's empty range, then the walk's prefetch");
    assert!(snapshots(&rails_of("backfill_prices_read_before_the_walk")).iter().all(|r| r["partial"] == 1 || r["date"].as_str() < Some("2026-09-10")),
            "valued from the prices read before the walk: none");
    assert!(snapshots(&rails_of("ibkr_stock_from_stored_closes")).iter().any(|r| r["date"] == "2026-09-29" && r["partial"] == 0), "IBKR's AAPL from its stored closes");
    assert_eq!(requests("ibkr_stock_from_stored_closes", "alpaca") + requests("ibkr_stock_from_stored_closes", "market"), 0);
}
