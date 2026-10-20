#![cfg(unix)] // shells out to bin/rails and sets Unix file modes, so it only builds and runs on Unix
use std::path::{Path, PathBuf};
use std::process::Command;
use common::decisions::{built_grid, copy_dir, r5_cleanup_expected, rails, try_rails};

/// Grid variants where Rust deliberately decides otherwise than Rails, each asserted on its own terms below.
/// add_service_unavailable: AddOrder answered EService:Unavailable may still have been placed. Rails writes a failed
/// row; Rust keeps the intent and settles it by cl_ord_id recovery, as for a lost reply.
const DIVERGENCES: [&str; 1] = ["add_service_unavailable"];

fn intent_kept(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let failed_rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().iter().filter(|t| t["after"]["status"] == 1).count();
    if failed_rows(rails_out) == 0 { return Err(format!("Rails no longer writes a failed row: drop the listed divergence\n  rails: {rails_out}")); }
    rust_kept_the_intent(dir, rust_out)
}

/// One AddOrder sent, no order row, the bot retrying with its intent kept and one placement_ambiguous log.
fn rust_kept_the_intent(dir: &Path, rust_out: &serde_json::Value) -> Result<(), String> {
    if rust_out["sent"].as_array().map(Vec::len) != Some(1) { return Err(format!("Rust must send exactly one AddOrder: {rust_out}")); }
    if !rust_out["changes"]["transactions"].as_array().unwrap().is_empty() { return Err(format!("Rust wrote an order row: {rust_out}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let (status, intent): (i64, Option<String>) = c.query_row(
        "SELECT status, json_extract(transient_data, '$.rust_placement') FROM bots", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let logged: i64 = c.query_row("SELECT count(*) FROM bot_activity_logs WHERE event = 'placement_ambiguous'", [], |r| r.get(0)).unwrap();
    if (status, intent.is_some(), logged) != (5, true, 1) { return Err(format!("expected retrying with the intent kept and one placement_ambiguous log, got status {status}, intent {intent:?}, {logged} log(s)")); }
    Ok(())
}

/// Both grids' `-unreadable_{price,placed,poll}_{nan,infinity,garbage}` variants: a venue number that is "NaN", "Infinity" or
/// "garbage", in a price, in the placement's answer (Kraken: the placed order's first poll) and in a fill poll. Rust
/// refuses every one as an unreadable answer (ruby::json_to_d): a price fails the tick with no order (retried); the Alpaca
/// placement answer keeps the intent for recovery; a poll changes no row and records no fill. What Rails did, recorded
/// from the grid (Kraken parses with honeymaker's strict BigDecimal(), Alpaca with String#to_d):
/// - price, garbage: both venues read 0 and raise their own "Wrong ask/last price … 0.0": retried, no order (only the
///   message differs from Rust's).
/// - price, NaN or Infinity: both venues accept the non-finite price and sizing raises "comparison of BigDecimal with 0
///   failed": execution_failed, no order.
/// - placed, Kraken (the placed order's first poll): garbage raises in the follow-up job (the row stays waiting); NaN closes
///   the order with amount_exec written as NULL (NaN); Infinity closes it with amount_exec +Inf.
/// - placed, Alpaca (the POST answer): all three are ignored and the order is recorded as submitted.
/// - poll, Kraken (the sweep): garbage raises (execution_failed, no order); NaN closes the waiting order with a NULL
///   amount_exec and, where an amount is still owed, the same tick places another order; Infinity closes it with +Inf
///   (and some ticks then fail "comparison of BigDecimal with 0 failed").
/// - poll, Alpaca (the follow-up): garbage closes the order with a ZERO fill (amount_exec and quote_amount_exec 0, whose
///   amount the next tick buys again); NaN closes it with NULL fills; Infinity closes it with +Inf fills.
const UNREADABLE: &str = "-unreadable_";

fn unreadable_number_refused(name: &str, dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, alpaca: bool) -> Result<(), String> {
    if rails_out == rust_out { return Err("Rails now decides as Rust: drop the listed divergence".into()); }
    let sent = rust_out["sent"].as_array().map_or(0, Vec::len);
    let rows = rust_out["changes"]["transactions"].as_array().unwrap();
    let logged_unreadable = rust_out["changes"]["bot_activity_logs"].to_string().contains("unreadable");
    let ok = if name.contains("-unreadable_price_") {
        sent == 0 && rows.is_empty() && logged_unreadable
    } else if name.contains("-unreadable_placed_") && alpaca {
        return rust_kept_the_intent(dir, rust_out);
    } else if name.contains("-unreadable_placed_") {
        // The order was placed and recorded at placement; its unreadable first poll records nothing on it.
        sent == 1 && rows.len() == 1 && rows[0]["after"]["external_status"] == 0
            && rows[0]["after"]["amount_exec"].is_null() && rows[0]["after"]["quote_amount_exec"].is_null()
            && rust_out["poll_error"].as_str().is_some_and(|e| e.contains("unreadable"))
    } else if alpaca {
        sent == 0 && rows.is_empty() && rust_out["poll_error"].as_str().is_some_and(|e| e.contains("unreadable"))
    } else {
        sent == 0 && rows.is_empty() && logged_unreadable
    };
    if ok { Ok(()) } else { Err(format!("Rust must refuse the number as unreadable: {rust_out}")) }
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_scenario_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    built_grid("grid", rails_root.path());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 324, "the grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.unwrap();
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        if DIVERGENCES.iter().any(|v| name.ends_with(&format!("-{v}"))) {
            if let Err(e) = intent_kept(&rust_root.path().join(&name), &rails_out, &rust_out) { failures.push(format!("{name} (listed divergence): {e}")); }
            continue;
        }
        if name.contains(UNREADABLE) {
            if let Err(e) = unreadable_number_refused(&name, &rust_root.path().join(&name), &rails_out, &rust_out, false) { failures.push(format!("{name} (listed divergence): {e}")); }
            continue;
        }
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    }
    // The mails are part of what is compared; pin what Rails enqueues, so that two silent sides cannot pass. Checked
    // before the comparison's own verdict, so it holds even while other scenarios differ.
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let mailed: Vec<&str> = rails_out["mails"].as_array().unwrap().iter().map(|m| m["mail"].as_str().unwrap()).collect();
        let expected: Option<&[&str]> = [("-blocking", &["stopped_by_error"][..]), ("-rejected", &["end_of_funds"]), ("-low_funds", &["end_of_funds"]), ("-on_schedule", &[])]
            .into_iter().find(|(suffix, _)| name.ends_with(suffix)).map(|(_, mails)| mails);
        if let Some(expected) = expected { assert_eq!(mailed, expected, "{name}: the mails Rails enqueued"); }
        // A capped TradesHistory walk: Rails keeps the order waiting beyond 1,000 trades and abandons it at 1,000.
        for (suffix, abandoned) in [("-history_beyond_reach", false), ("-history_within_reach", true)] {
            if !name.ends_with(suffix) { continue; }
            let gone = rails_out["changes"]["transactions"].as_array().unwrap().iter().any(|t| t["after"]["external_id"] == "OFAR-1" && t["after"]["external_status"] == 4);
            assert_eq!(gone, abandoned, "{name}: Rails' verdict on OFAR-1");
        }
        // The starting-time scenarios must reach Bot::Startable#disable_starting_time!, or they compare nothing new.
        if name.ends_with("-start_time_first_tick") {
            let bot = &rails_out["changes"]["bots"][0]["after"];
            assert_eq!(bot["settings"]["start_time_enabled"], false, "{name}: the first run turns the rule off");
            assert!(rails_out["sent"].as_array().is_some_and(|s| !s.is_empty()), "{name}: the first run buys");
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}

#[tokio::test(flavor = "current_thread")]
async fn decide_refuses_anything_but_a_marked_scratch_copy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("scenario.json"), r#"{"bot_id": 1, "at": "2026-09-01T00:00:00Z", "script": {}}"#).unwrap();
    let err = deltabadger::parity::decide(dir.path()).await.unwrap_err();
    assert!(matches!(&err, deltabadger::engine::EngineError::Data(m) if m.contains("parity_scratch")), "{err:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn a_copy_is_planned_per_eligible_bot_at_its_next_checkpoint() {
    let src = tempfile::tempdir().unwrap();
    let rails_root = tempfile::tempdir().unwrap();
    built_grid("grid", rails_root.path());
    let one = rails_root.path().join("week-market-on_schedule");
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let out = tempfile::tempdir().unwrap();
    let body = serde_json::json!({ "error": [], "result": { "XXBTZEUR": { "a": ["2", "1", "1.000"], "b": ["1", "1", "1.000"], "c": ["1.5", "0.1"], "v": ["12.5", "30.1"], "p": ["1.5", "1.5"], "t": [100, 250], "l": ["1.5", "1.5"], "h": ["1.5", "1.5"], "o": "1.5" } } });
    let n = deltabadger::parity::plan_copy(src.path(), &serde_json::json!({ "XBTEUR": body }), out.path(), "2026-09-10T00:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(n, 1);
    let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.path().join("bot-1/scenario.json")).unwrap()).unwrap();
    assert_eq!(sc["parity_scratch"], true);
    assert_eq!(sc["at"], "2026-09-15T10:00:01.123456Z", "one second after the next weekly checkpoint");
    assert_eq!(sc["script"]["http"]["/0/public/Ticker"][0], body);
}

fn alive(pid: &str) -> bool {
    Command::new("kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().unwrap().success()
}

#[test]
fn a_killed_or_failed_parity_run_leaves_no_copies_and_no_children() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let src = tempfile::tempdir().unwrap();
    let grid = tempfile::tempdir().unwrap();
    built_grid("grid", grid.path());
    let one = grid.path().join("week-market-on_schedule");
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let body = serde_json::json!({ "error": [], "result": { "XXBTZEUR": { "a": ["2", "1", "1.000"], "b": ["1", "1", "1.000"], "c": ["1.5", "0.1"], "v": ["12.5", "30.1"], "p": ["1.5", "1.5"], "t": [100, 250], "l": ["1.5", "1.5"], "h": ["1.5", "1.5"], "o": "1.5" } } });
    let tickers = src.path().join("tickers.json");
    std::fs::write(&tickers, serde_json::json!({ "XBTEUR": body }).to_string()).unwrap();
    // A stand-in `sh` for the script's Rails step: it records its sleeping child's pid and then idles, so the run is
    // still mid-Rails (copies on disk, child alive) when the SIGTERM lands, however fast the real Rails would be.
    for kill_it in [true, false] {
    let fake = tempfile::tempdir().unwrap();
    let pid_file = fake.path().join("pid");
    let sh = fake.path().join("sh");
    std::fs::write(&sh, format!("#!/bin/bash\nsleep 600 &\necho $! > '{}'\n{}\n", pid_file.display(), if kill_it { "wait" } else { "kill $!; sleep 0.3; exit 1" })).unwrap();
    std::fs::set_permissions(&sh, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut child = Command::new(root.join("script/rust/parity_on_copy.sh")).arg(src.path()).arg(&tickers)
        .env("DELTABADGER_PARITY_BIN", env!("CARGO_BIN_EXE_deltabadger"))
        .env("TMPDIR", tmp.path()).env("PATH", format!("{}:{}", fake.path().display(), std::env::var("PATH").unwrap()))
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !pid_file.exists() {
        assert!(std::time::Instant::now() < deadline && child.try_wait().unwrap().is_none(), "the Rails step never started");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    std::thread::sleep(std::time::Duration::from_millis(200)); // the pid file is written before the line ends
    let pid = std::fs::read_to_string(&pid_file).unwrap().trim().to_string();
    if kill_it { assert!(Command::new("kill").args(["-TERM", &child.id().to_string()]).status().unwrap().success()); }
    child.wait().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while alive(&pid) && std::time::Instant::now() < deadline { std::thread::sleep(std::time::Duration::from_millis(50)); }
    let still = alive(&pid);
    if still { Command::new("kill").args(["-KILL", &pid]).status().ok(); } // do not leak from a red run
    let left: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().map(|e| e.unwrap().file_name()).collect();
    let how = if kill_it { "SIGTERM" } else { "a failing Rails step" };
    assert!(left.is_empty(), "scratch copies survived {how}: {left:?}");
    assert!(!still, "the Rails step's child survived {how}");
    }
}

/// Alpaca grid variants where Rust deliberately decides otherwise than Rails, each asserted on its own terms below.
/// add_server_error, add_unreadable: Alpaca may have placed the order. Rails writes a failed row; Rust keeps the intent and
/// settles it by client_order_id (the spec's 5xx ruling). untradable_clock_closed: Rails asks the stock market's clock for a
/// crypto bot whose ticker went untradable (an empty ticker list is not "all crypto") and parks it until the open; Rust
/// treats crypto as always open and fails the tick as the open-clock case does.
/// Binance grid variants where Rust deliberately decides otherwise than Rails (spec E §2.7, §2.10), each asserted on its own
/// terms: a placement that may have reached Binance (a 5xx, a lost reply, an unreadable answer, an answer without an
/// orderId) keeps the intent for recovery where Rails writes a failed row (or a "BTCUSDT-" row); a read answered HTTP 429
/// waits as a throttle where Rails fails the run.
const BINANCE_DIVERGENCES: [&str; 4] = ["add_server_error", "add_timeout_after_send", "add_unreadable", "add_no_order_id"];

fn unacknowledged_kept(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let rails_rows = rails_out["changes"]["transactions"].as_array().unwrap();
    if !rails_rows.iter().any(|t| t["after"]["external_id"] == "BTCUSDT-") { return Err(format!("Rails no longer stores \"BTCUSDT-\": drop the listed divergence\n  rails: {rails_out}")); }
    rust_kept_the_intent(dir, rust_out)
}

/// The scripted 429 answers every retry, so both sides end the run failed: Rails at once as a generic error, Rust only after
/// its throttle waits (the failure's kind is "throttle", its retries the rate-limit ones). Neither places or changes a row.
fn throttle_waits(rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let failed = |out: &serde_json::Value| -> Vec<serde_json::Value> { out["changes"]["bot_activity_logs"].as_array().unwrap().iter()
        .filter(|l| l["after"]["event"] == "execution_failed").map(|l| l["after"]["details"].clone()).collect() };
    if failed(rails_out).is_empty() || failed(rails_out).iter().any(|d| d["kind"] == "throttle") {
        return Err(format!("Rails no longer fails a 429 as a generic error: drop the listed divergence\n  rails: {rails_out}"));
    }
    let rust = failed(rust_out);
    if !rust_out["sent"].as_array().unwrap().is_empty() || !rust_out["changes"]["transactions"].as_array().unwrap().is_empty()
        || rust.len() != 1 || rust[0]["kind"] != "throttle" || rust[0]["rate_limited_exhausted"] != true {
        return Err(format!("Rust must wait as a throttle: no order, no row change, the run failed only after its throttle waits: {rust_out}"));
    }
    Ok(())
}

/// The harness scripts a refused connection as Client.network_failure names it ("Faraday::ConnectionFailed: …"); honeymaker's
/// #with_rescue reports the exception's own message. Both sides send once and write one failed row; only that wording differs.
fn refused_like_rails(rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let normalised = rust_out.to_string().replace("Faraday::ConnectionFailed: ", "");
    if serde_json::from_str::<serde_json::Value>(&normalised).unwrap() != *rails_out {
        return Err(format!("beyond the exception's class name the sides differ\n  rails: {rails_out}\n  rust:  {rust_out}"));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_binance_grid() {
    use common::venue_grid::{ending, run_venue_grid, Validator};
    let unreadable = |n: &str| n.contains(UNREADABLE);
    let outputs = run_venue_grid("grid-binance", 176, &[
        Validator { names: &unreadable, check: &|n, d, r, u| unreadable_number_refused(n, d, r, u, false) },
        Validator { names: &ending(&["add_no_order_id"]), check: &|_, d, r, u| unacknowledged_kept(d, r, u) },
        Validator { names: &ending(&BINANCE_DIVERGENCES), check: &|_, d, r, u| intent_kept(d, r, u) },
        Validator { names: &ending(&["sweep_rate_limited"]), check: &|_, _, r, u| throttle_waits(r, u) },
        Validator { names: &ending(&["add_network_pre_send"]), check: &|_, _, r, u| refused_like_rails(r, u) },
    ]).await;
    // Pin what Rails does where a silent pair of sides would otherwise pass.
    for (name, rails_out) in &outputs {
        let mailed: Vec<&str> = rails_out["mails"].as_array().unwrap().iter().map(|m| m["mail"].as_str().unwrap()).collect();
        let expected: Option<&[&str]> = [("-insufficient", &["end_of_funds"][..]), ("-invalid_key", &["stopped_by_error"]), ("-low_funds", &["end_of_funds"]),
                                         ("-on_schedule", &[]), ("-minus_1021", &[])].into_iter().find(|(suffix, _)| name.ends_with(suffix)).map(|(_, m)| m);
        if let Some(expected) = expected { assert_eq!(mailed, expected, "{name}: the mails Rails enqueued"); }
        let rows = rails_out["changes"]["transactions"].as_array().unwrap();
        if name.contains("-minus_1021") { assert!(rows.is_empty() && rails_out["sent"].as_array().unwrap().len() == 1, "{name}: -1021 writes no row"); }
        if name.ends_with("-market-first_tick") { assert!(rails_out["sent"][0]["quoteOrderQty"].is_string(), "{name}: a market buy by quote"); }
        if name.ends_with("-market-base_sized") { assert!(rails_out["sent"][0]["quantity"].is_string(), "{name}: a market buy by base"); }
        if name.ends_with("-poll_pending_cancel") { assert!(rails_out["poll_error"].as_str().is_some_and(|e| e.contains("status is unknown")), "{name}"); }
        if name.ends_with("-sweep_not_returned_waits") { assert!(!rows.iter().any(|t| t["after"]["external_id"] == "BTCUSDT-210"), "{name}: never abandoned"); }
        if name.ends_with("-sweep_rounding_two_decimals") { assert_eq!(rows[0]["after"]["price"], 1, "{name}: 1.00/1.01/1.00 rounds to 1.0"); }
    }
}

/// KuCoin grid variants where Rust deliberately decides otherwise than Rails (plan K r4/r5), each asserted on its own terms:
/// a placement that may have reached KuCoin (a 5xx, a lost reply, an unreadable answer, a 200000 without an orderId, and
/// the 500000 / 230005 envelopes) keeps the intent, which on KuCoin waits for the operator, where Rails writes a failed row.
const KUCOIN_DIVERGENCES: [&str; 6] = ["add_server_error", "add_timeout_after_send", "add_unreadable", "add_no_order_id", "add_internal_500000", "add_busy_230005"];

/// An order type Rails' #parse_order_data refuses raises out of #get_orders, so the run's error is that sentence alone;
/// Rust reports it inside the sweep's "Failed to fetch orders … Result: [...]". Same outcome (the run fails, no row
/// changes, one error mail); only the wording differs.
fn unknown_type_wrapped(rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let normalised = rust_out.to_string().replace(r#"Failed to fetch orders KC-208. Result: [\"Unknown KuCoin order type: stop\"]"#, "Unknown KuCoin order type: stop");
    if serde_json::from_str::<serde_json::Value>(&normalised).unwrap() != *rails_out {
        return Err(format!("beyond the error's wrapper the sides differ\n  rails: {rails_out}\n  rust:  {rust_out}"));
    }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_kucoin_grid() {
    use common::venue_grid::{ending, run_venue_grid, Validator};
    let outputs = run_venue_grid("grid-kucoin", 172, &[
        Validator { names: &ending(&KUCOIN_DIVERGENCES), check: &|_, d, r, u| intent_kept(d, r, u) },
        Validator { names: &ending(&["add_network_pre_send"]), check: &|_, _, r, u| refused_like_rails(r, u) },
        Validator { names: &ending(&["sweep_unknown_type"]), check: &|_, _, r, u| unknown_type_wrapped(r, u) },
    ]).await;
    // Pin what Rails does where a silent pair of sides would otherwise pass.
    for (name, rails_out) in &outputs {
        let mailed: Vec<&str> = rails_out["mails"].as_array().unwrap().iter().map(|m| m["mail"].as_str().unwrap()).collect();
        let expected: Option<&[&str]> = [("-insufficient", &["end_of_funds"][..]), ("-invalid_key", &["stopped_by_error"]), ("-low_funds", &["end_of_funds"]),
                                         ("-on_schedule", &[]), ("-timestamp_envelope", &[]), ("-timestamp_400", &[]), ("-sweep_forgotten_young", &["notify_about_error"])].into_iter().find(|(suffix, _)| name.ends_with(suffix)).map(|(_, m)| m);
        if let Some(expected) = expected { assert_eq!(mailed, expected, "{name}: the mails Rails enqueued"); }
        let rows = rails_out["changes"]["transactions"].as_array().unwrap();
        if name.contains("-timestamp_") { assert!(rows.is_empty() && rails_out["sent"].as_array().unwrap().len() == 1, "{name}: 400002 writes no row"); }
        if name.ends_with("-market-quote_min_decides") { assert!(rails_out["sent"][0]["funds"].is_string(), "{name}: a market buy by funds"); }
        if name.ends_with("-market-first_tick") { assert!(rails_out["sent"][0]["size"].is_string(), "{name}: a market buy by size"); }
        if name.ends_with("-limit-first_tick") { assert!(rails_out["sent"][0]["price"].is_string() && rails_out["sent"][0]["size"].is_string(), "{name}: a limit buy"); }
        if name.ends_with("-poll_done_no_deal") { assert!(rails_out["poll_error"].as_str().is_some_and(|e| e.contains("status is unknown")), "{name}"); }
        if name.ends_with("-sweep_forgotten_old") { assert!(rows.iter().any(|t| t["after"]["external_id"] == "KC-211" && t["after"]["external_status"] == 4), "{name}: abandoned"); }
        if name.ends_with("-sweep_forgotten_filled") { assert!(rows.iter().any(|t| t["after"]["external_id"] == "KC-209" && t["after"]["external_status"] == 2), "{name}: settled from its fills"); }
    }
}

/// Slice 6a: a one-asset basket selling on Kraken, Binance and Alpaca (script/rust/decisions.rb sell_scenarios), compared whole.
#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_sell_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    built_grid("grid-sell", rails_root.path());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 195, "the sell grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.unwrap();
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
        // Pin what Rails does, so that two silent sides cannot pass.
        let sent = rails_out["sent"].as_array().unwrap().len();
        let placing = ["-first_tick", "-on_schedule", "-free_below", "-carry_frozen", "-rejected_insufficient", "-resting_sell", "-reserve_settled", "-reserve_pending_cancel"]
            .iter().any(|v| name.ends_with(v));
        if sent != usize::from(placing) { failures.push(format!("{name}: Rails sent {sent} order(s)\n  rails: {rails_out}")); }
        let mailed: Vec<&str> = rails_out["mails"].as_array().unwrap().iter().map(|m| m["mail"].as_str().unwrap()).collect();
        if name.ends_with("-rejected_insufficient") { assert!(!mailed.contains(&"end_of_funds"), "{name}: a seller is never told to top up its quote"); }
        // Alpaca: free is qty_available (0.004), never qty (0.104, counting the shares on a resting sell).
        if name == "sell-alpaca-base_market-free_below" { assert_eq!(rails_out["sent"][0]["qty"], "0.004000000", "{name}"); }
        let td = rails_out["changes"]["bots"].get(0).map(|b| b["after"]["transient_data"].clone()).unwrap_or_default();
        // 60 − 19.2 filled returns once, capped at the effective amount (the Smart Intervals split, 20).
        let returned = if name.contains("-base_smart-") { serde_json::json!(20.0) } else { serde_json::json!("40.8") };
        if name.ends_with("-reserve_settled") { assert!(td["missed_quote_amount"] == returned && td.get("flip_reserve_pending").is_none(), "{name}: {td}"); }
        if name.ends_with("-reserve_pending_cancel") { assert_eq!(td["flip_reserve_pending"], serde_json::json!([1]), "{name}: nothing until the final state"); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}

/// Slice 6b: a wider basket's ranked DCA-out (script/rust/decisions.rb basket_sell_scenarios), compared whole: on Alpaca, and
/// (plan CB T3) on Kraken, Binance and KuCoin, valued by each venue's bulk price read.
/// Sanctioned divergence `unreadable_string_price` (plan CB Q6, ruby::json_to_d): a Binance or KuCoin price that is no decimal
/// ("abc", "12x") leaves its member unpriced, so nothing is valued and nothing sells, where Rails' String#to_d values it at 0
/// or 12 and trades on that.
#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_basket_sell_grid() {
    let (_copies, outputs) = grid_outputs("grid-basket-sell", 278).await;
    let mut failures = vec![];
    let diverges = |name: &str| name.contains("-unreadable_price-") && (name.contains("-binance-") || name.contains("-kucoin-"));
    for (name, _, rails_out, rust_out) in &outputs {
        if diverges(name) {
            if !rust_out["sent"].as_array().unwrap().is_empty() { failures.push(format!("{name}: Rust sold on an unreadable price\n  rust:  {rust_out}")); }
            if rails_out["sent"].as_array().unwrap().is_empty() { failures.push(format!("{name}: Rails no longer trades on its String#to_d price: drop the divergence\n  rails: {rails_out}")); }
            continue;
        }
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    // Pin what Rails does, so that two silent sides cannot pass: which members each cell sells, by base asset.
    let base = |o: &serde_json::Value| -> String {
        let code = o["symbol"].as_str().or(o["pair"].as_str()).unwrap();
        if code == "XBTEUR" { return "BTC".into(); }
        code.split(['/', '-']).next().unwrap().trim_end_matches("USDT").trim_end_matches("EUR").to_string()
    };
    let sold = |out: &serde_json::Value| -> Vec<String> { out["sent"].as_array().unwrap().iter().map(base).collect() };
    for (name, _, rails_out, _) in &outputs {
        if diverges(name) { continue; }
        let sold = sold(rails_out);
        let rows = rails_out["changes"]["transactions"].as_array().unwrap();
        let skipped = rows.iter().filter(|r| r["after"]["status"] == 2).count();
        let nothing = ["-unpriced_member-", "-stale_feed-", "-feed_unreachable-", "-no_holdings-", "-all_under_floor-", "-balance_error-",
                       "-unreadable_price-", "-raised_price-"];
        let crypto = ["-kraken-", "-binance-", "-kucoin-"].iter().any(|v| name.contains(v));
        let expect: Option<&[&str]> = if name.contains("-base-") || nothing.iter().any(|v| name.contains(v)) {
            Some(&[])
        // A crypto tie: 100 of each member. w50: BTC (composition order) sells all it holds, and what is left of ETH's share
        // is under the venue floor; tiers: SOL sits furthest over its target and its bid covers the whole 100.
        } else if crypto && name.contains("-w50-tie-") { Some(&["BTC"])
        } else if crypto && name.contains("-tiers-tie-") { Some(&["SOL"])
        // Kraken: net of the resting 0.009, BTC's 0.001 is worth 49.995 at its last price, just under ETH's 50.0002.
        } else if name.contains("-kraken-") && name.contains("-resting_top-") { Some(&["ETH", "BTC"])
        // tiers: SOL sits furthest over its smaller target once BTC cannot sell; "tie" is a tie only at w50's equal targets.
        } else if ["-tiers-floor_handover-", "-tiers-nothing_free_top-", "-tiers-tie-"].iter().any(|v| name.contains(v)) { Some(&["SOL", "ETH"])
        } else if name.contains("-tiers-foreign_crypto-") || name.contains("-tiers-free_below-") { Some(&["BTC", "SOL", "ETH"])
        // w50 tie: as much of each member, so BTC (composition order) sells out and ETH takes the rest of the 100.
        } else if name.contains("-w50-tie-") { Some(&["BTC", "ETH"])
        } else if name.contains("-over_top-") || name.contains("-rejected-") || name.contains("-cancelled_partial_sell-") { Some(&["BTC"])
        } else if name.contains("-w50-foreign_crypto-") { Some(&["BTC", "ETH"])
        } else if name.contains("-floor_handover-") || name.contains("-nothing_free_top-") { Some(&["ETH"])
        } else if name.contains("-free_below-") || name.contains("-resting_top-") { Some(&["BTC", "ETH"])
        } else if name.starts_with("basket-sell-stock-open-") { Some(&["AAPL"])
        } else if name.starts_with("basket-sell-stock-foreign-") { Some(&["MSFT"])
        } else if name.starts_with("basket-sell-stock-closed-") { Some(&[])
        } else { None };
        match expect { Some(e) => if sold != e { failures.push(format!("{name}: sold {sold:?}")); }, None => failures.push(format!("{name}: no expectation pinned")) }
        assert_eq!(skipped, usize::from(name.contains("-all_under_floor-")), "{name}: the first member's attempt only, once: {rails_out}");
        if name.contains("-foreign_crypto-") { assert_eq!(rails_out["sent"][0]["qty"], "0.010000000", "{name}: the local 0.01, not the merged 1.01"); }
    }
    assert!(failures.is_empty(), "pinned expectations:\n{}", failures.join("\n"));
}

/// Slice 6b: threshold rebalancing on a stopped basket (script/rust/decisions.rb rebalance_scenarios), fire by fire: on Alpaca,
/// and (plan CB T4) on Kraken, Binance and KuCoin, with each venue's fees, floors and placement answers.
/// Sanctioned divergence `unknown_outcome`: Rails halts a swap whose placement outcome is unknown (a lost reply, a 5xx, an
/// accepted order without an id, a venue's unknown-outcome text) at once; the engine keeps the intent with its phase and
/// resolves it by client order id (placement::recover_since), halting only if the venue still cannot answer a full absence
/// window later.
/// Sanctioned divergence `unreadable_string_price` (plan CB Q6): a Binance or KuCoin price that is no decimal leaves its
/// member unpriced, so nothing is due, where Rails' String#to_d values it at 0 or 12 and sells on the drift that makes.
#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_rebalance_identically() {
    let (_copies, outputs) = grid_outputs("grid-rebalance", 238).await;
    let unknown = ["-post_send", "-server_error", "-no_order_id", "-unknown_text"];
    let diverges = |name: &str| name.ends_with("-unreadable_price") && (name.contains("-binance-") || name.contains("-kucoin-"));
    let mut failures = vec![];
    let phase = |out: &serde_json::Value| -> serde_json::Value {
        out["changes"]["bots"].as_array().unwrap().last().map(|b| b["after"]["transient_data"]["rebalance_pending"]["phase"].clone()).unwrap_or_default()
    };
    for (name, _, rails_out, rust_out) in &outputs {
        if unknown.iter().any(|u| name.ends_with(u)) {
            // Both sent the one sell; Rails halted (or, on Binance's id-less answer, stored "BTCUSDT-" and waits on it), the
            // engine waits on its intent with the selling phase, no row on its side.
            assert_eq!(rails_out["sent"], rust_out["sent"], "{name}");
            assert_eq!(phase(rust_out), serde_json::json!("selling"), "{name}: {rust_out}");
            assert!(rust_out["changes"]["transactions"].as_array().unwrap().is_empty(), "{name}: {rust_out}");
            continue;
        }
        if diverges(name) {
            if !rust_out["sent"].as_array().unwrap().is_empty() { failures.push(format!("{name}: Rust traded on an unreadable price\n  rust:  {rust_out}")); }
            if rails_out["sent"].as_array().unwrap().is_empty() { failures.push(format!("{name}: Rails no longer trades on its String#to_d price: drop the divergence")); }
            continue;
        }
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    // Pin what Rails does, so that two silent sides cannot pass: what each cell sent (by base asset), and the phase it left.
    let base = |o: &serde_json::Value| -> String {
        let code = o["symbol"].as_str().or(o["pair"].as_str()).unwrap();
        if code == "XBTEUR" { return "BTC".into(); }
        code.split(['/', '-']).next().unwrap().trim_end_matches("USDT").trim_end_matches("EUR").to_string()
    };
    let side = |o: &serde_json::Value| -> String {
        o["side"].as_str().or(o["type"].as_str().filter(|t| *t == "buy" || *t == "sell")).unwrap_or("").to_lowercase()
    };
    let mut pins = vec![];
    for (name, _, rails_out, _) in &outputs {
        if diverges(name) { continue; }
        let sent: Vec<(String, String)> = rails_out["sent"].as_array().unwrap().iter().map(|o| (base(o), side(o))).collect();
        let state = name.split('-').skip(3).collect::<Vec<_>>().join("-");
        let state = if name.starts_with("rebalance-w50-") || name.starts_with("rebalance-tiers-") || name.starts_with("rebalance-index-") {
            name.split_once('-').unwrap().1.split_once('-').unwrap().1.to_string()
        } else { state };
        let expect: &[(&str, &str)] = match state.as_str() {
            "under_threshold" | "rule_off" | "selling" | "open_buy" | "sell_below_minimum" | "at_threshold" | "unbalanced" | "raised_price"
            | "unpriced_member" | "unreadable_price" | "takeover_sell_no_id" | "takeover_buy_attempted_no_id" | "takeover_proceeds_unknown" => &[],
            "owed_buy" | "unbalanced_owed_buy" | "takeover_buy_not_attempted" | "takeover_partial_buy" | "takeover_resolved_sell" => &[("ETH", "buy")],
            // A crypto Layer 1 bot: SOL left for ADA (unheld), so ETH sits furthest over its target.
            "exited_member" if !name.starts_with("rebalance-index-") => &[("ETH", "sell")],
            "exited_member" => &[("AAA", "sell")],
            "swap" | "buy_capped" | "buy_rejected" | "cancelled_partial_sell" => &[("BTC", "sell"), ("ETH", "buy")],
            s if s.starts_with("fee_") && s.ends_with("_swap") => &[("BTC", "sell"), ("ETH", "buy")],
            "partial_buy" => &[("BTC", "sell"), ("ETH", "buy"), ("ETH", "buy")],
            s if s.starts_with("fee_") && s.ends_with("_partial_buy") => &[("BTC", "sell"), ("ETH", "buy"), ("ETH", "buy")],
            _ => &[("BTC", "sell")],
        };
        let expect: Vec<(String, String)> = expect.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect();
        // tiers: the underweight member after BTC's sale is SOL or ETH by the figures; only the sides are pinned there.
        let same = if name.contains("-tiers-") { sent.iter().map(|s| &s.1).eq(expect.iter().map(|s| &s.1)) } else { sent == expect };
        if !same { pins.push(format!("{name}: sent {sent:?}, pinned {expect:?}")); }
        let left = phase(rails_out);
        let want = match state.as_str() {
            "swap" | "nothing_sold" | "buy_below_minimum" | "under_threshold" | "rule_off" | "selling" | "open_buy" | "sell_below_minimum" | "pre_send"
            | "at_threshold" | "unbalanced" | "safe_rejection" | "raised_price" | "unpriced_member" | "unreadable_price" | "takeover_proceeds_unknown" => serde_json::Value::Null,
            "rejected" | "rejected_400" | "buy_rejected" | "post_send" | "server_error" | "unknown_text" | "takeover_sell_no_id" | "takeover_buy_attempted_no_id" => serde_json::json!("ambiguous"),
            // Binance stores an id-less answer as "BTCUSDT-" and waits on it; elsewhere Rails halts.
            "no_order_id" if name.contains("-binance-") => serde_json::json!("selling"),
            "no_order_id" => serde_json::json!("ambiguous"),
            "resting_sell" | "open_buy_stale" | "below_drift" | "exited_member" => serde_json::json!("selling"),
            _ => serde_json::json!("buying"),
        };
        if left != want { pins.push(format!("{name}: phase {left}, pinned {want}")); }
        // What the swap owes after the fees (plan CB Q1, ported as Rails has it): a Binance quote commission comes off the
        // proceeds; a base or BNB commission does not; Kraken's `cost` and KuCoin's `dealFunds` are gross.
        let owed = match name.as_str() {
            "rebalance-binance-w50-fee_quote_swap" => Some("255.7304"), "rebalance-binance-w50-fee_base_swap" | "rebalance-binance-w50-fee_bnb_swap" => Some("256.0004"),
            "rebalance-kraken-w50-fee_quote_swap" => Some("199.96"), "rebalance-kucoin-w50-fee_quote_swap" => Some("256.0"),
            "rebalance-binance-w50-fee_quote_partial_buy" => Some("127.434288"), "rebalance-kraken-w50-fee_quote_partial_buy" => Some("99.98"),
            "rebalance-kucoin-w50-fee_quote_partial_buy" => Some("128.0"),
            // R0: a resolved halt that named a buy which filled 40 of the 80 owed buys the remainder only.
            "rebalance-kraken-w50-takeover_partial_buy" | "rebalance-kucoin-w50-takeover_partial_buy" => Some("40.0"),
            "rebalance-binance-w50-takeover_partial_buy" => Some("39.99184"), _ => None,
        };
        if let Some(owed) = owed {
            let left = &rails_out["changes"]["bots"].as_array().unwrap().last().unwrap()["after"]["transient_data"]["rebalance_pending"]["remaining_quote_amount"];
            if left != owed { pins.push(format!("{name}: owes {left}, pinned {owed}")); }
        }
    }
    assert!(pins.is_empty(), "pinned expectations:\n{}", pins.join("\n"));
}

const ALPACA_DIVERGENCES: [&str; 3] = ["add_server_error", "add_unreadable", "untradable_clock_closed"];

fn crypto_ignores_the_stock_clock(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let rails_events: Vec<&str> = rails_out["changes"]["bot_activity_logs"].as_array().unwrap().iter().filter_map(|l| l["after"]["event"].as_str()).collect();
    if !rails_events.contains(&"market_closed") { return Err(format!("Rails no longer parks it behind the clock: drop the listed divergence\n  rails: {rails_out}")); }
    if !rust_out["sent"].as_array().unwrap().is_empty() { return Err(format!("Rust sent an order: {rust_out}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let (status, error): (i64, String) = c.query_row(
        "SELECT b.status, json_extract(l.details, '$.error') FROM bots b JOIN bot_activity_logs l ON l.bot_id = b.id WHERE l.event = 'execution_failed'",
        [], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| format!("no execution_failed log: {e}"))?;
    if (status, error.as_str()) != (5, "None of the portfolio's weighted assets trade on Alpaca") { return Err(format!("got status {status}, error {error:?}")); }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_alpaca_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    built_grid("grid-alpaca", rails_root.path());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    // 369 (with #451's unreadable-number variants) and the six mail variants (54 scenarios).
    assert_eq!(dirs.len(), 423, "the Alpaca grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let mut rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        // The Rust engine's mail markers, seeded on the row before Rails ticked it: Rails must hand them back as they were.
        if let Some(left) = rails_out.as_object_mut().unwrap().remove("markers") {
            assert!(name.contains("-markers_survive_"), "{name}: only these scenarios seed markers");
            assert_eq!(left, seeded_markers(), "{name}: Rails changed or dropped a marker");
        } else {
            assert!(!name.contains("-markers_survive_"), "{name}: Rails reported no markers");
        }
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.unwrap();
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        let listed = if name.contains(UNREADABLE) {
            Some(unreadable_number_refused(&name, &rust_dir, &rails_out, &rust_out, true))
        } else if name.ends_with("-untradable_clock_closed") {
            Some(crypto_ignores_the_stock_clock(&rust_dir, &rails_out, &rust_out))
        } else if ALPACA_DIVERGENCES.iter().any(|v| name.ends_with(&format!("-{v}"))) {
            Some(intent_kept(&rust_dir, &rails_out, &rust_out))
        } else {
            None
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out != rust_out => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            None => {}
        }
    }
    // The comparison below sees the funds notification only if Rails really sends it one way and not the other: a Rust side
    // that suppressed it (or sent it always) then differs in `funds_notified` and fails.
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        if name.ends_with("-funds_low_buying_power") { assert_eq!(rails_out["funds_notified"], true, "{name}: Rails must notify"); }
        if name.ends_with("-funds_low_cash_only") { assert_eq!(rails_out["funds_notified"], false, "{name}: Rails must not notify"); }
        if name.ends_with("-poll_partially_filled") { assert_eq!(rails_out["poll_error"], serde_json::Value::Null, "{name}: mid-fill is open, asked again"); }
        if name.ends_with("-insufficient_buying_power") { assert_eq!(rails_out["funds_notified"], true, "{name}: Bot::Failable stamps the budget"); }
        // The mails are compared above, which proves nothing if Rails mails in neither scenario or in both: pin Rails' side.
        let mailed: Vec<&str> = rails_out["mails"].as_array().unwrap().iter().map(|m| m["mail"].as_str().unwrap()).collect();
        let expected: Option<&[&str]> = [("-funds_low_buying_power", &["end_of_funds"][..]), ("-funds_budget_spent", &[]), ("-funds_budget_reopened", &["end_of_funds"]),
                                         ("-error_budget_spent", &[]), ("-error_budget_reopened", &["notify_about_error"]), ("-insufficient_buying_power", &["end_of_funds"]),
                                         ("-unauthorized_twice", &["stopped_by_error"]), ("-markers_survive_a_tick", &[]), ("-markers_survive_a_failure", &["end_of_funds"]),
                                         ("-on_schedule", &[])].into_iter().find(|(suffix, _)| name.ends_with(suffix)).map(|(_, mails)| mails);
        if let Some(expected) = expected { assert_eq!(mailed, expected, "{name}: the mails Rails enqueued"); }
        if name.ends_with("-error_budget_reopened") { assert_eq!(rails_out["mails"][0]["errors"], serde_json::json!(["qty must be > 0 & <sane>"]), "{name}"); }
        if name.ends_with("-unauthorized_twice") { assert!(rails_out["mails"][0]["errors"][0].as_str().unwrap().starts_with("Alpaca rejected the API key."), "{name}"); }
        if name.ends_with("-retry_reuses_cached_price") {
            assert_eq!(rails_out["sent"].as_array().map(Vec::len), Some(2), "{name}: one failed and one accepted POST");
            assert_eq!(rails_out["sent"][0], rails_out["sent"][1], "{name}: Rails' retry reused its cached price");
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}

/// script/rust/decisions.rb SEEDED_MARKERS.
fn seeded_markers() -> serde_json::Value {
    serde_json::json!({ "rust_funds_mail_pending": { "quote_asset": 2, "stamped_at": "2026-08-31T09:00:00.000Z" },
                        "rust_error_mail_pending": { "unknown": { "error": "an <old> \"error\"", "stamped_at": "2026-08-31T09:00:00.000Z" } },
                        "rust_stopped_mail_pending": { "error": "unauthorized.", "stamped_at": "2026-08-31T09:00:00.000Z" },
                        "rust_limit_mail_pending": { "stamped_at": "2026-08-31T09:00:00.000Z" } })
}

/// The same four markers through Rails' web paths: the settings form on the running bot, a stop, the settings form on
/// the stopped bot. Rails writes `transient_data` back whole from the row it loaded, unknown keys included.
#[test]
fn a_rails_web_save_of_the_bot_keeps_the_mail_markers() {
    let dir = tempfile::tempdir().unwrap();
    rails(&["web-save", dir.path().to_str().unwrap()]);
    let out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("web_save.json")).unwrap()).unwrap();
    // Rails really saved, three times: or the markers' survival proves nothing.
    assert_eq!((out["saved_while_running"].clone(), out["stopped"].clone(), out["saved_while_stopped"].clone()), (serde_json::json!(true), serde_json::json!(true), serde_json::json!(true)), "{out}");
    assert_eq!(out["settings"], serde_json::json!({ "quote_amount": 80.0, "interval": "day", "limit_ordered": true }), "{out}");
    assert_eq!((out["status"].as_str(), out["label"].as_str()), (Some("stopped"), Some("Renamed again")), "{out}");
    assert_eq!(out["markers"], seeded_markers(), "Rails changed or dropped a marker");
}

#[test]
fn a_suppressed_funds_notification_is_a_difference() {
    // The grid compares whole outputs: flipping only `funds_notified` must make them unequal.
    let rails = serde_json::json!({ "sent": [], "changes": {}, "funds_notified": true, "poll_error": null });
    let mut rust = rails.clone();
    rust["funds_notified"] = serde_json::json!(false);
    assert_ne!(rails, rust);
}

#[tokio::test(flavor = "current_thread")]
async fn an_alpaca_copy_is_planned_with_alpaca_bodies() {
    let src = tempfile::tempdir().unwrap();
    let grid = tempfile::tempdir().unwrap();
    built_grid("grid-alpaca", grid.path());
    let one = grid.path().join("day-market-sweep_rejected_ignored"); // one waiting order, OMKT-1
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let quotes = serde_json::json!({ "quotes": { "BTC/USD": { "ap": 2.0, "bp": 1.0 } } });
    let trades = serde_json::json!({ "trades": { "BTC/USD": { "p": 1.5 } } });
    let out = tempfile::tempdir().unwrap();
    let n = deltabadger::parity::plan_copy(src.path(), &serde_json::json!({ "BTC/USD": { "quotes": quotes, "trades": trades } }), out.path(),
                                           "2026-09-10T00:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(n, 1);
    let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.path().join("bot-1/scenario.json")).unwrap()).unwrap();
    assert_eq!((sc["parity_scratch"].as_bool(), sc["venue"].as_str()), (Some(true), Some("alpaca")));
    assert_eq!(sc["script"]["alpaca"]["GET /v1beta3/crypto/us/latest/quotes"][0]["body"], quotes);
    assert_eq!(sc["script"]["alpaca"]["GET /v2/orders/OMKT-1"][0]["body"]["status"], "accepted", "a waiting order is answered as resting");
}

/// A scenario whose script lacks a path the tick needs must fail the run loudly, on either venue (never reach the network).
#[test]
fn an_unscripted_call_fails_the_run_on_both_venues() {
    for (grid, scenario) in [("grid", "script"), ("grid-alpaca", "alpaca"), ("grid-binance", "binance")] {
        let built = tempfile::tempdir().unwrap();
        built_grid(grid, built.path());
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(built.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        let root = tempfile::tempdir().unwrap();
        let from = dirs.iter().find(|d| {
            let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("scenario.json")).unwrap()).unwrap();
            sc.get("tick").and_then(|t| t.as_bool()).unwrap_or(true)
        }).unwrap();
        copy_dir(from, &root.path().join("one"));
        let path = root.path().join("one/scenario.json");
        let mut sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        if scenario == "script" { sc["script"]["http"] = serde_json::json!({}); } else { sc["script"][scenario] = serde_json::json!({}); }
        std::fs::write(&path, sc.to_string()).unwrap();
        let err = try_rails(&["record", root.path().to_str().unwrap()]).expect_err(&format!("{grid}: an unscripted call must fail the run"));
        assert!(err.contains("unscripted"), "{grid}: {err}");
    }
}


type Outputs = Vec<(String, PathBuf, serde_json::Value, serde_json::Value)>;

/// Builds `command`'s grid, records Rails over it, and decides every scenario in Rust on a pristine copy, asserting the grid's
/// size so a silently shrinking grid fails: (name, Rust's copy, Rails' output, Rust's output), in name order. The TempDir keeps
/// Rust's copies for the asserters that read them.
async fn grid_outputs(command: &str, expected: usize) -> (tempfile::TempDir, Outputs) {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    built_grid(command, rails_root.path());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), expected, "{command} has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); } // before Rails writes to its copies
    if command == "grid-basket" {
        let normalized = tempfile::tempdir().unwrap();
        let paired: Vec<_> = dirs.iter().filter(|d| d.file_name().unwrap().to_string_lossy().contains("-zero_quote_exec-")).collect();
        assert_eq!(paired.len(), 2);
        for d in &paired { copy_dir(d, &normalized.path().join(d.file_name().unwrap())); }
        rails(&["record-normalized", normalized.path().to_str().unwrap()]);
        for d in &paired {
            let name = d.file_name().unwrap();
            std::fs::copy(normalized.path().join(name).join("rails.json"), rust_root.path().join(name).join("rails-normalized.json")).unwrap();
        }
    }
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut out = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.unwrap();
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        out.push((name, rust_dir, rails_out, rust_out));
    }
    (rust_root, out)
}

/// `out` without the rows `drop` picks, and without row ids in transactions and bot_activity_logs: one side's extra row shifts
/// every later id on that side.
fn normalised(out: &serde_json::Value, drop: &dyn Fn(&str, &serde_json::Value) -> bool) -> serde_json::Value {
    let mut out = out.clone();
    for table in ["transactions", "bot_activity_logs"] {
        let rows: Vec<serde_json::Value> = out["changes"][table].as_array().unwrap().iter().filter(|r| !drop(table, r)).map(|r| {
            let mut r = r.clone();
            r.as_object_mut().unwrap().remove("id");
            if let Some(after) = r["after"].as_object_mut() { after.remove("id"); }
            r
        }).collect();
        out["changes"][table] = serde_json::Value::Array(rows);
    }
    out
}

fn notional_sum(sent: &[serde_json::Value]) -> f64 { sent.iter().map(|o| o["notional"].as_str().unwrap_or("0").parse::<f64>().unwrap()).sum() }

/// The 5xx leg's mail, listed with its failed row: Rails fails the run, mails notify_about_error for it and spends that mail's
/// day (failure_notifications.unknown); the engine keeps the intent, owes no mail and spends nothing. Each side must show
/// exactly that; the comparison then leaves the mails and that budget out.
fn five_xx_mails(rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(serde_json::Value, serde_json::Value), String> {
    let mailed = |out: &serde_json::Value| -> Vec<String> { out["mails"].as_array().into_iter().flatten().filter_map(|m| m["mail"].as_str().map(str::to_string)).collect() };
    if mailed(rails_out) != ["notify_about_error"] { return Err(format!("Rails mailed {:?} for the 5xx leg, not notify_about_error once", mailed(rails_out))); }
    if !mailed(rust_out).is_empty() { return Err(format!("Rust mailed {:?} for an ambiguous leg; it owes none", mailed(rust_out))); }
    let budget = |out: &serde_json::Value| out["changes"]["bots"].as_array().into_iter().flatten()
        .any(|b| !b["after"]["transient_data"]["failure_notifications"]["unknown"].is_null());
    if !budget(rails_out) { return Err("Rails spent no notify_about_error day for the 5xx leg".into()); }
    if budget(rust_out) { return Err("Rust spent a notify_about_error day for an ambiguous leg".into()); }
    let strip = |out: &serde_json::Value| {
        let mut out = out.clone();
        out.as_object_mut().unwrap().remove("mails");
        for b in out["changes"]["bots"].as_array_mut().into_iter().flatten() {
            for side in ["before", "after"] {
                if let Some(t) = b[side]["transient_data"].as_object_mut() { t.remove("failure_notifications"); }
            }
        }
        out
    };
    Ok((strip(rails_out), strip(rust_out)))
}

/// A 5xx on leg k: Rails writes legs 1..k-1 and a failed row for leg k; Rust writes the same k-1 rows, keeps leg k's intent on
/// leg k's own ticker, and logs placement_ambiguous once. Apart from that one row and that one log, everything else (the
/// POSTs, the bot's row with its carry and settings, the members, the logs, the funds notification) must equal Rails'.
fn leg_intent_kept(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, k: usize) -> Result<(), String> {
    let rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().clone();
    let rails_rows = rows(rails_out);
    let failed = rails_rows.iter().filter(|r| r["after"]["status"] == 1).count();
    if rails_rows.len() != k || rails_rows[k - 1]["after"]["status"] != 1 || failed != 1 {
        return Err(format!("Rails no longer writes legs 1..{} and one failed leg {k}: drop the listed divergence\n  rails: {rails_out}", k - 1));
    }
    let ambiguous = |r: &serde_json::Value| r["after"]["event"] == "placement_ambiguous";
    let logged = rust_out["changes"]["bot_activity_logs"].as_array().unwrap().iter().filter(|r| ambiguous(r)).count();
    if logged != 1 { return Err(format!("Rust logged placement_ambiguous {logged} time(s), not once\n  rust: {rust_out}")); }
    let (rails_cmp, rust_cmp) = five_xx_mails(rails_out, rust_out)?;
    let rails = normalised(&rails_cmp, &|t, r| t == "transactions" && r["after"]["status"] == 1);
    let rust = normalised(&rust_cmp, &|t, r| t == "bot_activity_logs" && ambiguous(r));
    if rails != rust { return Err(format!("beyond leg {k}'s failed row and Rust's ambiguity log\n  rails: {rails}\n  rust:  {rust}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let (status, ticker): (i64, Option<i64>) = c.query_row(
        "SELECT status, json_extract(transient_data, '$.rust_placement.ticker_id') FROM bots", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let leg_k: i64 = c.query_row("SELECT id FROM tickers WHERE ticker = ?1", [rust_out["sent"][k - 1]["symbol"].as_str().unwrap()], |r| r.get(0)).unwrap();
    if (status, ticker) != (5, Some(leg_k)) {
        return Err(format!("expected retrying with the intent on ticker {leg_k}; got status {status}, intent ticker {ticker:?}"));
    }
    Ok(())
}

/// A recover-* scenario: leg k was ambiguous, the engine's reconciliation tick settled it as not placed and sent nothing, and
/// at the next checkpoint both sides buy what was not bought. Listed differences, removed before comparing: Rust's
/// placement_ambiguous resolution log; in the 5xx variant also Rails' failed row for leg k and Rust's ambiguity log for it
/// (Rails logs none there). Each is removed only after it is counted exactly once, so a duplicate cannot hide.
fn settled_like_rails(rails_out: &serde_json::Value, rust_out: &serde_json::Value, five_xx: bool) -> Result<(), String> {
    if rust_out["recover_sent"] != 0 { return Err(format!("the reconciliation tick sent {} order(s); it must send none", rust_out["recover_sent"])); }
    let count = |out: &serde_json::Value, table: &str, f: &dyn Fn(&serde_json::Value) -> bool| out["changes"][table].as_array().unwrap().iter().filter(|r| f(&r["after"])).count();
    let resolved = count(rust_out, "bot_activity_logs", &|a| a["event"] == "placement_ambiguous" && a["details"]["resolution"] == "not_placed");
    if resolved != 1 { return Err(format!("Rust logged the not_placed resolution {resolved} time(s), not once\n  rust: {rust_out}")); }
    if five_xx {
        let failed = count(rails_out, "transactions", &|a| a["status"] == 1);
        if failed != 1 { return Err(format!("Rails wrote {failed} failed row(s) for the 5xx leg, not one: drop or revisit the listed difference\n  rails: {rails_out}")); }
        let ambiguity = count(rust_out, "bot_activity_logs", &|a| a["event"] == "placement_ambiguous" && a["details"]["resolution"].is_null());
        if ambiguity != 1 { return Err(format!("Rust logged the 5xx leg's ambiguity {ambiguity} time(s), not once\n  rust: {rust_out}")); }
    }
    let (rails_out, mut rust) = if five_xx { five_xx_mails(rails_out, rust_out)? } else { (rails_out.clone(), rust_out.clone()) };
    let rails_out = &rails_out;
    rust.as_object_mut().unwrap().remove("recover_sent");
    let rust = normalised(&rust, &|t, r| t == "bot_activity_logs" && r["after"]["event"] == "placement_ambiguous"
        && (five_xx || r["after"]["details"]["resolution"] == "not_placed"));
    let rails = normalised(rails_out, &|t, r| five_xx && t == "transactions" && r["after"]["status"] == 1);
    if rails != rust { return Err(format!("\n  rails: {rails}\n  rust:  {rust}")); }
    Ok(())
}

/// A landed ambiguous leg: what it was and what it must show. `k` counts the POSTs of the first tick through the ambiguous
/// one; `pair` is the landed leg's pair, `quote` and `base` the fill the lookup reported; `gap` is what Rails, with no row for
/// the landed order, buys more at the next checkpoint.
struct Landed { k: usize, pair: &'static str, quote: f64, base: f64, gap: f64 }

/// Rust's OTX-L row: status, external status, quote exec, base exec, base asset, the landed pair's asset, the bot's intent.
type LandedRow = (i64, i64, f64, f64, i64, i64, Option<String>);

/// A landed leg recovered by client order id, held to Rails' own decision. `reference` is Rails' output on the scenario
/// `<name>-reference`: the same bot at the next checkpoint, holding the rows the engine holds by then (the accepted legs,
/// swept to filled, and the landed leg as the engine records it). Rust must send exactly Rails' orders there, symbol by
/// symbol and amount by amount; its first `k` POSTs are Rails' own; its reconciliation tick sends nothing; its OTX-L row is
/// on the landed pair, closed, with the lookup's fill; no intent is left. Rails' own run still buys `gap` more: the listed
/// divergence. A wrong split (the whole total in one member, say) fails the comparison with the reference. `reference`
/// carries Rust's copy of the reference scenario too, whose decision equals Rails' there (its own grid cell), for the
/// members' final rows.
fn landed_matches_reference(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, reference: Option<(&Path, &serde_json::Value)>, l: &Landed) -> Result<(), String> {
    let (reference_dir, reference) = reference.ok_or("no -reference scenario for this landed recovery")?;
    if rust_out["recover_sent"] != 0 { return Err(format!("the reconciliation tick sent {} order(s); it must send none", rust_out["recover_sent"])); }
    let (rails_sent, rust_sent, reference_sent) = (rails_out["sent"].as_array().unwrap(), rust_out["sent"].as_array().unwrap(), reference["sent"].as_array().unwrap());
    if rails_sent.len() < l.k || rust_sent.len() < l.k || rails_sent[..l.k] != rust_sent[..l.k] {
        return Err(format!("the first {} POSTs differ\n  rails: {rails_sent:?}\n  rust:  {rust_sent:?}", l.k));
    }
    if reference_sent.is_empty() { return Err("the reference buys nothing, so it proves nothing".into()); }
    if rust_sent[l.k..] != reference_sent[..] {
        return Err(format!("the next checkpoint's orders are not Rails' on the recovered rows\n  reference: {reference_sent:?}\n  rust:      {:?}", &rust_sent[l.k..]));
    }
    // The ledger the next checkpoint writes and the bot it leaves, against the reference. The reference starts from the rows
    // the engine holds, so its new rows are exactly the next checkpoint's: Rust's rows created at or after the reference's
    // first new row must equal them, ids aside (Rust's copy holds more rows, so its ids run ahead). A row the reference
    // updated (an accepted leg, swept to filled) must end with the same fill on Rust's side. The bot must end with the same
    // status, stop key and time, settings and carry (the engine's own keys are outside both snapshots).
    let strip = |r: &serde_json::Value| { let mut a = r["after"].clone(); a.as_object_mut().unwrap().remove("id"); a };
    let rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().clone();
    let reference_new: Vec<serde_json::Value> = rows(reference).iter().filter(|r| r["before"].is_null()).map(strip).collect();
    let Some(since) = reference_new.iter().filter_map(|r| r["created_at"].as_str()).min().map(str::to_string) else {
        return Err("the reference writes no row at the next checkpoint, so it proves nothing".into());
    };
    let rust_new: Vec<serde_json::Value> = rows(rust_out).iter().map(strip).filter(|r| r["created_at"].as_str().is_some_and(|c| c >= since.as_str())).collect();
    if rust_new != reference_new {
        return Err(format!("the next checkpoint's ledger rows are not Rails'\n  reference: {reference_new:?}\n  rust:      {rust_new:?}"));
    }
    let fill = |r: &serde_json::Value| ["status", "external_status", "price", "amount_exec", "quote_amount_exec", "base_asset_id", "order_type"].map(|k| r[k].clone());
    for updated in rows(reference).iter().filter(|r| !r["before"].is_null()) {
        let ext = &updated["after"]["external_id"];
        let mine = rows(rust_out).iter().find(|r| r["after"]["external_id"] == *ext).map(strip);
        if mine.as_ref().map(fill) != Some(fill(&updated["after"])) {
            return Err(format!("order {ext} ends differently\n  reference: {}\n  rust:      {mine:?}", updated["after"]));
        }
    }
    // The logs: the first tick's (and the reconciliation's) are Rails' own run's; the next checkpoint's are the reference's.
    let logs = |out: &serde_json::Value, next: bool| -> Vec<serde_json::Value> {
        out["changes"]["bot_activity_logs"].as_array().unwrap().iter().map(strip)
            .filter(|r| r["created_at"].as_str().is_some_and(|c| (c >= since.as_str()) == next)).collect()
    };
    if logs(rust_out, false) != logs(rails_out, false) {
        return Err(format!("the logs before the next checkpoint are not Rails' own\n  rails: {:?}\n  rust:  {:?}", logs(rails_out, false), logs(rust_out, false)));
    }
    if logs(rust_out, true) != logs(reference, true) {
        return Err(format!("the next checkpoint's logs are not the reference's\n  reference: {:?}\n  rust:      {:?}", logs(reference, true), logs(rust_out, true)));
    }
    // The members as both copies end: weights, membership, exits, and whether the next checkpoint wrote them. Ids and the
    // build's wall-clock stamps aside (a row nothing rewrote keeps updated_at = created_at, read as unwritten).
    let members = |d: &Path| -> Vec<String> {
        let c = rusqlite::Connection::open(d.join("production.sqlite3")).unwrap();
        let mut s = c.prepare("SELECT asset_id, ticker_id, in_index, target_allocation, current_allocation, exited_at, \
                               CASE WHEN updated_at = created_at THEN NULL ELSE updated_at END FROM bot_index_assets ORDER BY asset_id").unwrap();
        s.query_map([], |r| Ok(format!("{:?}", (0..7).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect::<Result<Vec<_>, _>>()?)))
            .unwrap().collect::<Result<_, _>>().unwrap()
    };
    if members(dir) != members(reference_dir) {
        return Err(format!("the members end differently\n  reference: {:?}\n  rust:      {:?}", members(reference_dir), members(dir)));
    }
    let bot = |out: &serde_json::Value| out["changes"]["bots"].as_array().unwrap().first().map(|b| b["after"].clone()).unwrap_or_default();
    let (theirs, mine, rails_own) = (bot(reference), bot(rust_out), bot(rails_out));
    for k in ["status", "stop_message_key", "stopped_at", "settings"] {
        if theirs[k] != mine[k] { return Err(format!("the bot's {k} ends differently\n  reference: {}\n  rust:      {}", theirs[k], mine[k])); }
    }
    // R5's comparison copy requires the unrelated NULL flag to survive the first tick's failure.
    // The reference never failed: now both complete blobs must match, with no broad NULL filtering.
    if mine["transient_data"] != rails_own["transient_data"] || mine["transient_data"] != theirs["transient_data"] {
        return Err(format!("the bot's transient_data ends differently\n  rails:     {}\n  reference: {}\n  rust:      {}",
                           rails_own["transient_data"], theirs["transient_data"], mine["transient_data"]));
    }
    let landed = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().iter().any(|r| r["after"]["external_id"] == "OTX-L");
    if landed(rails_out) { return Err(format!("Rails now records the landed order OTX-L: drop the listed divergence\n  rails: {rails_out}")); }
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let row: Option<LandedRow> = c.query_row(
        "SELECT x.status, x.external_status, x.quote_amount_exec, x.amount_exec, x.base_asset_id, (SELECT base_asset_id FROM tickers WHERE ticker = ?1), \
                (SELECT json_extract(transient_data, '$.rust_placement') FROM bots) FROM transactions x WHERE x.external_id = 'OTX-L'",
        [l.pair], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))).ok();
    match row {
        Some((0, 2, q, b, asset, pair_asset, None)) if q == l.quote && b == l.base && asset == pair_asset => {}
        other => return Err(format!("Rust's OTX-L row (status, external status, quote exec, base exec, asset, {} asset, intent) is {other:?}; \
                                     expected submitted, closed, {} and {} on {}, no intent left", l.pair, l.quote, l.base, l.pair)),
    }
    let more = notional_sum(&rails_sent[l.k..]) - notional_sum(&rust_sent[l.k..]);
    // Each side floors each of up to three legs to the cent, so the gap may be off by up to 3 cents; compared in whole cents,
    // since 36.03 − 36.00 is 0.0300000000000011 in f64.
    if ((more - l.gap) * 100.0).round().abs() > 3.0 { return Err(format!("Rails buys {more:.2} more at the next checkpoint, not the landed {:.2}", l.gap)); }
    Ok(())
}

/// On Rust's copy after its ticks, the exited member (SOL) is still an `in_index = 0` row and eligibility admits the bot: an
/// exited member is a holding, never a refusal.
fn exited_member_admitted(dir: &Path) -> Result<(), String> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).unwrap();
    let exited: i64 = c.query_row("SELECT count(*) FROM bot_index_assets i JOIN tickers t ON t.base_asset_id = i.asset_id \
                                   WHERE t.ticker = 'SOL/USD' AND i.in_index = 0", [], |r| r.get(0)).unwrap();
    let r = deltabadger::engine::eligibility::check_install(&c).map_err(|e| format!("{e:?}"))?;
    if exited != 1 || !r.problems.is_empty() || r.eligible.len() != 1 {
        return Err(format!("expected SOL exited and the bot admitted; got {exited} exited row(s), problems {:?}, eligible {:?}", r.problems, r.eligible));
    }
    Ok(())
}

/// Rails' tie order is undefined: a 50/50 basket may place its two legs in either order. Equal once both sides' orders and new
/// rows are sorted by member.
fn equal_up_to_tie_order(rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> bool {
    let canon = |out: &serde_json::Value| {
        let mut o = normalised(out, &|_, _| false);
        let mut sent = o["sent"].as_array().unwrap().clone();
        sent.sort_by_key(|s| s["symbol"].to_string());
        o["sent"] = serde_json::Value::Array(sent);
        let mut rows = o["changes"]["transactions"].as_array().unwrap().clone();
        rows.sort_by_key(|r| r["after"]["base_asset_id"].to_string());
        o["changes"]["transactions"] = serde_json::Value::Array(rows);
        o
    };
    canon(rails_out) == canon(rust_out)
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_basket_grid() {
    let (_copies, outputs) = grid_outputs("grid-basket", 84).await;
    let reference = |name: &str| outputs.iter().find(|(n, ..)| *n == format!("{name}-reference")).map(|(_, d, r, _)| (d.as_path(), r));
    let mut failures = vec![];
    for (name, dir, rails_out, rust_out) in &outputs {
        let k = name.rsplit('-').next().and_then(|s| s.strip_prefix('k')).and_then(|s| s.parse().ok()).unwrap_or(0);
        if name.starts_with("basket-exited-") {
            if let Err(e) = exited_member_admitted(dir) { failures.push(format!("{name}: {e}")); }
        }
        let listed = if name.contains("-zero_quote_exec-") {
            let normalized: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("rails-normalized.json")).unwrap()).unwrap();
            assert_ne!(&normalized, rails_out, "{name}: the recorded normalization divergence disappeared");
            Some(if &normalized == rust_out { Ok(()) } else { Err(format!("normalized Rails: {normalized}\nRust: {rust_out}")) })
        } else if name.contains("-ambiguous_5xx-") {
            Some(leg_intent_kept(dir, rails_out, rust_out, k))
        } else if name == "basket-exited-intent-tiers-k3-landed" {
            // SOL's leg was ambiguous, then SOL was delisted and exited while its intent was unresolved. The reconciliation
            // still finds the order by client order id and records it against SOL.
            Some(landed_matches_reference(dir, rails_out, rust_out, reference(name), &Landed { k: 3, pair: "SOL/USD", quote: 24.0, base: 0.16, gap: 24.0 }))
        } else if name.ends_with("-landed") {
            Some(landed_matches_reference(dir, rails_out, rust_out, reference(name), &Landed { k: 2, pair: "ETH/USD", quote: 36.0, base: 0.0144, gap: 36.0 }))
        } else if name.starts_with("basket-recover-") && !name.ends_with("-reference") {
            Some(settled_like_rails(rails_out, rust_out, name.ends_with("-5xx")))
        } else {
            None
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out == rust_out => {}
            None if name.contains("-w50-") && equal_up_to_tie_order(rails_out, rust_out) => {
                eprintln!("{name}: the tied legs in the other order (listed divergence: Rails' tie order is undefined)");
            }
            None => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    // Rails' side must show what each cell exists for, or an equal comparison proves nothing.
    let events = |out: &serde_json::Value| -> Vec<String> {
        out["changes"]["bot_activity_logs"].as_array().unwrap().iter().filter_map(|l| l["after"]["event"].as_str().map(str::to_string)).collect()
    };
    for (name, _, rails_out, _) in &outputs {
        let rows = rails_out["changes"]["transactions"].as_array().unwrap();
        let sent = rails_out["sent"].as_array().unwrap().len();
        if name.starts_with("basket-minimum-one_below-") { assert!(events(rails_out).contains(&"orders_below_minimum".to_string()), "{name}: {rails_out}"); }
        if name.starts_with("basket-minimum-all_below-") { assert_eq!(rows.iter().filter(|r| r["after"]["status"] == 2).count(), 2, "{name}: two skipped rows"); }
        if name == "basket-untradable-one-market" {
            assert!(rails_out["changes"]["bot_index_assets"].as_array().unwrap().iter().any(|r| r["after"]["in_index"] == 0), "{name}: ETH exited");
        }
        if name == "basket-safety-tiers-pre_send-k2" { assert_eq!(sent, 2, "{name}: no retry after a placed leg"); }
        if name == "basket-safety-tiers-pre_send-k1" { assert!(sent > 1, "{name}: retry_on replays a run that placed nothing"); }
        if name == "basket-sizing-w70-resting_partial-market" { assert!(sent > 0, "{name}: Alpaca's partially_filled is open; the sweep goes on"); }
        if name == "basket-recover-tiers-k2-network" { assert_eq!(sent, 5, "{name}: two in the first tick, three at the next checkpoint"); }
        if name == "basket-exited-holds-tiers" { assert_eq!(sent, 2, "{name}: the exited SOL is never bought"); }
        if name == "basket-exited-intent-tiers-k3-landed" { assert_eq!(sent, 5, "{name}: three legs, then BTC and ETH only"); }
        // Plan CB T5: a parked member is bought at its cap weight only when every member has a cap.
        if matches!(name.as_str(), "basket-market_cap-parked" | "basket-market_cap-first_tick") { assert_eq!(sent, 3, "{name}: every member at its cap weight"); }
        if matches!(name.as_str(), "basket-market_cap-missing_cap" | "basket-market_cap-zero_cap") { assert_eq!(sent, 2, "{name}: the sliders stay"); }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_basket_copy_is_planned_with_every_members_prices() {
    let src = tempfile::tempdir().unwrap();
    let grid = tempfile::tempdir().unwrap();
    built_grid("grid-basket", grid.path());
    let one = grid.path().join("basket-sizing-w70-resting-market"); // a 70/30 basket with one resting ETH buy, OOPEN-1
    for f in ["production.sqlite3", "production_queue.sqlite3"] { std::fs::copy(one.join(f), src.path().join(f)).unwrap(); }
    let pair = |sym: &str, p: f64| (format!("{sym}/USD"), serde_json::json!({
        "quotes": { "quotes": { format!("{sym}/USD"): { "ap": p, "bp": p } } }, "trades": { "trades": { format!("{sym}/USD"): { "p": p } } } }));
    let tickers: serde_json::Map<String, serde_json::Value> = [pair("BTC", 64000.0), pair("ETH", 2500.0)].into_iter().collect();
    let out = tempfile::tempdir().unwrap();
    let n = deltabadger::parity::plan_copy(src.path(), &serde_json::Value::Object(tickers), out.path(), "2026-09-10T00:00:00Z".parse().unwrap()).unwrap();
    assert_eq!(n, 1);
    let sc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(out.path().join("bot-1/scenario.json")).unwrap()).unwrap();
    let quotes = &sc["script"]["alpaca"]["GET /v1beta3/crypto/us/latest/quotes"][0]["body"]["quotes"];
    assert!(quotes.get("BTC/USD").is_some() && quotes.get("ETH/USD").is_some(), "every leg reads its own pair: {quotes}");
    assert_eq!(sc["script"]["alpaca"]["GET /v2/orders/OOPEN-1"][0]["body"]["symbol"], "ETH/USD", "a waiting order is answered with its own pair");
    // Executed on both sides, each accepted leg is its own transaction. Both engines deduplicate an accepted order by its
    // external id, so the copy's script answers every placement with a distinct one.
    let bot_dir = out.path().join("bot-1");
    let rails_root = tempfile::tempdir().unwrap();
    copy_dir(&bot_dir, &rails_root.path().join("bot-1"));
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(rails_root.path().join("bot-1/rails.json")).unwrap()).unwrap();
    let rust_out = deltabadger::parity::decide(&bot_dir).await.unwrap();
    assert_eq!(rails_out, rust_out, "the copy decides identically");
    let ids: Vec<&str> = rust_out["changes"]["transactions"].as_array().unwrap().iter()
        .filter_map(|r| r["after"]["external_id"].as_str().filter(|e| e.starts_with("OPARITY-"))).collect();
    assert_eq!(rust_out["sent"].as_array().unwrap().len(), 2, "the 70/30 basket buys both members: {rust_out}");
    assert_eq!(ids.len(), 2, "one transaction per accepted leg: {ids:?}");
}

/// Amount-limit scenarios in which Rails mails stopped_by_amount_limit (Bot::Notifyable, deliver_later on every qualifying fill,
/// no dedupe), and so does the engine (notice::LIMIT, written with the fill). Archived: Rails mails although
/// Bot::Lifecycle#stop changes nothing.
const LIMIT_MAILS: [&str; 10] = ["limit-poll-reaches", "limit-poll-reaches-limit_order", "limit-poll-stopped_bot", "limit-poll-archived",
                                 "limit-sweep-reaches", "limit-sweep-reaches-under_floor", "limit-sweep-reaches-twice", "limit-poll-cancelled_partial_reaches",
                                 "limit-poll-cancelled_fractional_reaches", "limit-tick_then_poll"];

/// Plan CB T2's crypto cells (decisions.rb venue_limit_scenarios) in which Rails mails stopped_by_amount_limit: the fills that
/// spend the cap, a slipped base-sized fill past it, and Binance's fee-inclusive executed quote (Kraken's and KuCoin's are
/// gross, so the same fill leaves 0.06 and stops nothing).
fn venue_limit_mails(name: &str) -> bool {
    ["kraken", "binance", "kucoin"].iter().any(|v| ["poll-reaches", "sweep-reaches", "slippage-base"].iter().any(|c| name == format!("limit-{v}-{c}")))
        || name == "limit-binance-fee-quote"
}

/// The exact-cap guard's trim on a crypto venue (listed divergence, RULING-B2A-R2): Rails sizes 0.0002 BTC from its Float
/// remainder and spends past the exact cap by a few 1e-15; Rust sends one base increment less. Beyond that order's volume and
/// its row's amount and quote_amount, both sides are equal.
fn guard_trimmed(name: &str, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    use deltabadger::ruby::BigDec;
    let (field, step, ask, exact) = match name {
        "limit-kraken-guard-trim" => ("volume", "0.00000001", "15900.000000000015", "3.18"),
        "limit-kucoin-guard-trim" => ("size", "0.00000001", "15900.000000000015", "3.18"),
        _ => ("quantity", "0.00001", "35900.000000000015", "7.18"),
    };
    let bd = |s: &str| BigDec::parse(s).unwrap();
    let volume = |out: &serde_json::Value| -> Result<BigDec, String> {
        let sent = out["sent"].as_array().filter(|s| s.len() == 1).ok_or_else(|| format!("not one order: {out}"))?;
        sent[0][field].as_str().map(bd).ok_or_else(|| format!("no {field}: {out}"))
    };
    let (rails, rust) = (volume(rails_out)?, volume(rust_out)?);
    if &rails * &bd(ask) <= bd(exact) { return Err(format!("Rails no longer sizes past the exact cap ({} sent): drop the listed divergence", rails.to_s_f())); }
    if rust.checked_add(&bd(step)).unwrap() != rails || &rust * &bd(ask) > bd(exact) {
        return Err(format!("Rust sent {} where Rails sent {}: not one increment under, within the cap", rust.to_s_f(), rails.to_s_f()));
    }
    let mut patched = rust_out.clone();
    patched["sent"] = rails_out["sent"].clone();
    let rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().cloned().unwrap_or_default();
    if rows(rails_out).len() != 1 || rows(rust_out).len() != 1 { return Err(format!("not one order row each\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    for col in ["amount", "quote_amount"] { patched["changes"]["transactions"][0]["after"][col] = rails_out["changes"]["transactions"][0]["after"][col].clone(); }
    if patched != *rails_out { return Err(format!("beyond the trimmed volume\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    Ok(())
}

/// An unresolved intent counts as spent. Rails writes no row for the ambiguous 60 USD order that in fact landed, so its next
/// checkpoint spends the whole 100 USD cap again (160 sent against the cap); Rust recovers the order by its client order id,
/// counts it, and spends the 40 left (100 sent).
fn overspend_prevented(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, reference: Option<(&Path, &serde_json::Value)>) -> Result<(), String> {
    landed_matches_reference(dir, rails_out, rust_out, reference, &Landed { k: 1, pair: "BTC/USD", quote: 60.0, base: 0.0009375, gap: 60.0 })?;
    let total = |out: &serde_json::Value| notional_sum(out["sent"].as_array().unwrap());
    if total(rails_out) <= 100.0 { return Err(format!("Rails no longer overspends the cap ({:.2} sent): drop the listed divergence", total(rails_out))); }
    if total(rust_out) > 100.0 { return Err(format!("Rust sent {:.2} against a 100 cap", total(rust_out))); }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_amount_limit_grid() {
    let (_copies, outputs) = grid_outputs("grid-limit", 90).await;
    let reference = |name: &str| outputs.iter().find(|(n, ..)| *n == format!("{name}-reference")).map(|(_, d, r, _)| (d.as_path(), r));
    let mut failures = vec![];
    for (name, dir, rails_out, rust_out) in &outputs {
        let (mut rails_out, rust_out) = (rails_out.clone(), rust_out.clone());
        let mailed = |out: &serde_json::Value| -> Vec<String> {
            out["mails"].as_array().unwrap_or_else(|| panic!("{name}: no mails reported")).iter().map(|m| m["mail"].as_str().unwrap().to_string()).collect()
        };
        // Rails' side pinned, so that two silent sides cannot pass.
        let expected: &[&str] = if name == "limit-sweep-reaches-twice" {
            &["stopped_by_amount_limit", "stopped_by_amount_limit"] // one per callback
        } else if LIMIT_MAILS.contains(&name.as_str()) || venue_limit_mails(name) {
            &["stopped_by_amount_limit"]
        } else if name.ends_with("-5xx") {
            &["notify_about_error"] // Rails' failed row for the 5xx leg (Bot::Notifyable#notify_about_error)
        } else {
            &[]
        };
        if mailed(&rails_out) != expected { failures.push(format!("{name}: Rails mailed {:?}, expected {expected:?}", mailed(&rails_out))); }
        if name == "limit-sweep-reaches-twice" {
            // Listed divergence: the engine's mail marker is one key, so the two callbacks of one sweep owe one mail.
            if mailed(&rust_out) != ["stopped_by_amount_limit"] { failures.push(format!("{name}: Rust mailed {:?}, expected one", mailed(&rust_out))); }
            rails_out["mails"] = rust_out["mails"].clone();
        }
        let rust_out = &rust_out;
        let listed = if name.ends_with("-guard-trim") {
            Some(guard_trimmed(name, &rails_out, rust_out))
        } else if name == "limit-ambiguous_overspend" {
            Some(overspend_prevented(dir, &rails_out, rust_out, reference(name)))
        } else if name.ends_with("-landed") {
            Some(landed_matches_reference(dir, &rails_out, rust_out, reference(name), &Landed { k: 2, pair: "ETH/USD", quote: 36.0, base: 0.0144, gap: 36.0 }))
        } else if (name.starts_with("limit-recover-") && !name.ends_with("-reference")) || name == "limit-ambiguous_not_placed" {
            Some(settled_like_rails(&rails_out, rust_out, name.ends_with("-5xx")))
        } else {
            None
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out == *rust_out => {}
            None => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    // Rails' side must show what each cell exists for, or an equal comparison proves nothing.
    for (name, _, rails_out, _) in &outputs {
        let bot = rails_out["changes"]["bots"].as_array().unwrap().first().map(|b| b["after"].clone()).unwrap_or_default();
        let sent: Vec<&str> = rails_out["sent"].as_array().unwrap().iter().filter_map(|o| o["notional"].as_str()).collect();
        let skipped = rails_out["changes"]["transactions"].as_array().unwrap().iter().filter(|r| r["after"]["status"] == 2).count();
        if (LIMIT_MAILS.contains(&name.as_str()) && name != "limit-poll-archived") || venue_limit_mails(name) {
            assert_eq!((bot["status"].clone(), bot["stop_message_key"].clone()), (serde_json::json!(2), serde_json::json!("bot.settings.extra_amount_limit.amount_spent")), "{name}");
        }
        match name.as_str() {
            "limit-poll-short" => assert_ne!(bot["status"], 2, "{name}: 40 left, not reached"),
            "limit-state-exact" | "limit-state-overspent" => assert!(sent.is_empty() && rails_out["changes"]["bot_activity_logs"].as_array().unwrap().is_empty(), "{name}: no order, no log"),
            "limit-cut-market" | "limit-tally-closed" => assert_eq!(sent, vec!["40.00"], "{name}: cut to what is left"),
            "limit-kraken-fee-quote" | "limit-kucoin-fee-quote" => assert_ne!(bot["status"], 2, "{name}: the gross 60 leaves 0.06 of the cap"),
            // A cut to 40 of the 60 owed: 40 at each venue's ask, in base (Kraken, KuCoin) or in quote (Binance's NOTIONAL binds).
            "limit-kraken-tally-closed" => assert_eq!(rails_out["sent"][0]["volume"], "0.00079999", "{name}"),
            "limit-binance-tally-closed" => assert_eq!(rails_out["sent"][0]["quoteOrderQty"], "40.0", "{name}"),
            "limit-kucoin-tally-closed" => assert_eq!(rails_out["sent"][0]["size"], "0.00062499", "{name}"),
            "limit-tally-cancelled_partial" => assert_eq!(sent, vec!["80.00"], "{name}: the 20 filled counts in pending and in the cap"),
            "limit-sweep-reaches-under_floor" => assert_eq!((sent.len(), skipped), (0, 1), "{name}: the run sizes the 0.005 left before Bot::StopJob runs"),
            "limit-sweep-reaches-twice" => assert_eq!(rails_out["changes"]["bot_activity_logs"].as_array().unwrap().iter()
                .filter(|l| l["after"]["event"] == "stopped").count(), 2, "{name}: one StopJob, and one stopped line, per qualifying fill"),
            "limit-state-under_floor" | "limit-under_minimum-market" | "limit-under_minimum-limit" => {
                assert_eq!((sent.len(), skipped), (0, 1), "{name}: skipped, and the bot keeps running");
                assert_ne!(bot["status"], 2, "{name}");
            }
            _ => {}
        }
    }
}

/// Stock grid variants where Rust deliberately decides otherwise than Rails (spec Amendment 2026-10-02b), each asserted below.
const STOCK_DIVERGENCES: [&str; 7] = ["clock_5xx", "clock_unreadable", "clock_certificate", "clock_stale_body", "clock_stale_cache",
                                      "clock_past_next_open", "add_server_error"];

fn events_and_status(dir: &Path) -> Result<(Vec<String>, i64), String> {
    let c = rusqlite::Connection::open(dir.join("production.sqlite3")).map_err(|e| e.to_string())?;
    let mut s = c.prepare("SELECT event FROM bot_activity_logs ORDER BY id").map_err(|e| e.to_string())?;
    let events = s.query_map([], |r| r.get(0)).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    Ok((events, c.query_row("SELECT status FROM bots", [], |r| r.get(0)).map_err(|e| e.to_string())?))
}

/// Rails reads the clock as open and places; Rust places nothing. `parked`: Rust read the fresh clock as closed (no cache);
/// otherwise it retried as transient and ended `retrying` with execution_retrying.
fn clock_failed_closed(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value, parked: bool) -> Result<(), String> {
    if rails_out["sent"].as_array().map(Vec::len) != Some(1) { return Err(format!("Rails no longer places on this clock: drop the listed divergence\n  rails: {rails_out}")); }
    if !rust_out["sent"].as_array().ok_or("expected JSON array")?.is_empty() { return Err(format!("Rust sent an order: {rust_out}")); }
    let (events, status) = events_and_status(dir)?;
    let want: &[&str] = if parked { &["market_closed"] } else { &["execution_retrying"] };
    if events != want || (!parked && status != 5) { return Err(format!("Rust: events {events:?}, status {status}")); }
    Ok(())
}

/// Rails parks on a closed clock whose next_open is already past (and would spin on its cache); Rust retries and parks nothing.
fn past_next_open_retried(dir: &Path, rails_out: &serde_json::Value, rust_out: &serde_json::Value) -> Result<(), String> {
    let rails_events: Vec<&str> = rails_out["changes"]["bot_activity_logs"].as_array().ok_or("expected JSON array")?.iter().filter_map(|l| l["after"]["event"].as_str()).collect();
    if rails_events != ["market_closed"] { return Err(format!("Rails no longer parks on a past next_open: {rails_out}")); }
    if !rust_out["sent"].as_array().ok_or("expected JSON array")?.is_empty() { return Err(format!("Rust sent an order: {rust_out}")); }
    let (events, status) = events_and_status(dir)?;
    if events != ["execution_retrying"] || status != 5 { return Err(format!("Rust: events {events:?}, status {status}")); }
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_stock_grid() -> Result<(), Box<dyn std::error::Error>> {
    let rails_root = grid_dir("stock");
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid-stock", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 124, "the stock grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); }
    rails(&["record", rails_root.path().to_str().unwrap()]);

    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json"))?)?;
        let rust_dir = rust_root.path().join(&name);
        let rust_out = deltabadger::parity::decide(&rust_dir).await.map_err(|e| format!("{e:?}"))?;
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        let variant = STOCK_DIVERGENCES.iter().find(|v| name.ends_with(&format!("-{v}")));
        let listed = match variant.copied() {
            Some("add_server_error") => Some(intent_kept(&rust_dir, &rails_out, &rust_out)),
            Some("clock_past_next_open") => Some(past_next_open_retried(&rust_dir, &rails_out, &rust_out)),
            Some("clock_stale_cache") => Some(clock_failed_closed(&rust_dir, &rails_out, &rust_out, true)),
            Some(_) => Some(clock_failed_closed(&rust_dir, &rails_out, &rust_out, false)),
            None => None,
        };
        match listed {
            Some(Err(e)) => failures.push(format!("{name} (listed divergence): {e}")),
            Some(Ok(())) => {}
            None if rails_out != rust_out => failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")),
            None => {}
        }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    // The comparison proves parity only where Rails really does what the variant names: pin those facts on Rails' side.
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let r: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json"))?)?;
        let events: Vec<&str> = r["changes"]["bot_activity_logs"].as_array().ok_or("expected JSON array")?.iter().filter_map(|l| l["after"]["event"].as_str()).collect();
        if name.ends_with("-closed") { assert_eq!(events, ["market_closed"], "{name}"); }
        if name.ends_with("-split_recent") || name.ends_with("-split_unresolved") { assert_eq!(events, ["dca_skipped_restatement"], "{name}"); }
        if name.ends_with("-sweep_done_for_day") || name.ends_with("-sweep_held") { assert!(!events.contains(&"execution_failed"), "{name}: open, the sweep goes on"); }
        if name.ends_with("-funds_low_buying_power") { assert_eq!(r["funds_notified"], true, "{name}: a stock bot spends buying_power"); }
        if name.ends_with("-funds_low_cash_only") { assert_eq!(r["funds_notified"], false, "{name}"); }
        if name.ends_with("-first_tick") { assert_eq!(r["sent"][0]["time_in_force"], "day", "{name}"); }
    }
    Ok(())
}
#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_index_grid() -> Result<(), Box<dyn std::error::Error>> {
    let rails_root = grid_dir("index");
    let rust_root = tempfile::tempdir().unwrap();
    rails(&["grid-index", rails_root.path().to_str().unwrap()]);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 66, "the index grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); }
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        eprintln!("deciding {name}"); // nextest prints this on a TIMEOUT: the scenario that hung
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json"))?)?;
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.map_err(|e| format!("{e:?}"))?;
        let rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
        // Pinned on Rails' side, so a vacuous pass is impossible: what each composition names really happened there.
        let members: Vec<&serde_json::Value> = rails_out["changes"]["bot_index_assets"].as_array().ok_or("expected JSON array")?.iter().collect();
        if name.contains("-duplicate-") && name.ends_with("-all_flat") {
            let allocations = members.iter().map(|m| -> Result<_, Box<dyn std::error::Error>> { Ok(f64::from_bits(u64::from_str_radix(m["after"]["target_allocation"]["f"].as_str().ok_or("expected allocation bits")?, 16)?)) }).collect::<Result<Vec<_>, _>>()?;
            assert_eq!(allocations,vec![0.704545,0.295455],"Rails recorded [A,A,B]: {name}");
        }
        if name.contains("-leaver-") {
            assert!(members.iter().any(|m| m["after"]["in_index"] == 0 && m["after"]["exited_at"].is_string()), "{name}: CCC marked out");
            assert!(rails_out["sent"].as_array().ok_or("expected JSON array")?.iter().all(|o| o["side"] == "buy" && o["symbol"] != "CCC"), "{name}: never sold, not bought");
        }
        if name.contains("-index_missing-") { assert!(rails_out["sent"].as_array().ok_or("expected JSON array")?.is_empty(), "{name}"); }
        if name.contains("-incumbent_unpriced-") { assert!(rails_out["sent"].as_array().ok_or("expected JSON array")?.is_empty(), "{name}: the buy stalls"); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
    Ok(())
}


mod common;
#[tokio::test(flavor = "current_thread")]
async fn a_stock_and_an_index_copy_are_planned_with_stock_bodies_and_an_open_clock() -> Result<(), Box<dyn std::error::Error>> {
    use common::seed::{self, BotSpec};
    use deltabadger::store::{self, Paths};
    use serde_json::{json, Value};
    use chrono::{DateTime, Utc};
    let src = common::rails_install();
    {
        let o = store::open(&Paths::from_env(&|_| None, src.path())).map_err(|e| format!("{e:?}"))?;
        let s = seed::seed_alpaca(&o.primary, &seed::cipher());
        let (aapl, _) = seed::add_alpaca_stock(&o.primary, &s, "AAPL");
        seed::add_alpaca_stock(&o.primary, &s, "MSFT");
        seed::insert_bot(&o.primary, &s, &BotSpec::weekly(60.0, "2026-09-01 14:00:00").weights(&[(aapl, 1.0)]));
        seed::insert_index(&o.primary, "nasdaq-100", &["MSFT.US", "AAPL.US"], &json!({ "MSFT.US": 3.1e12, "AAPL.US": 3.4e12 }));
        seed::index_bot(&o.primary, &s, "nasdaq-100", 2, 0.0, false);
    }
    let tickers = json!({ "AAPL": { "quote": { "symbol": "AAPL", "quote": { "ap": 187.43 } }, "trade": { "symbol": "AAPL", "trade": { "p": 187.41 } } },
                          "MSFT": { "quote": { "symbol": "MSFT", "quote": { "ap": 401.25 } }, "trade": { "symbol": "MSFT", "trade": { "p": 401.2 } } } });
    let out = tempfile::tempdir().unwrap();
    assert_eq!(deltabadger::parity::plan_copy(src.path(), &tickers, out.path(), "2026-09-10T12:00:00Z".parse()?).map_err(|e| format!("{e:?}"))?, 2);
    let mut index_scripts = 0;
    for dir in std::fs::read_dir(out.path()).unwrap() {
        let sc: Value = serde_json::from_str(&std::fs::read_to_string(dir?.path().join("scenario.json"))?)?;
        let a = &sc["script"]["alpaca"];
        assert_eq!(a["GET /v2/stocks/AAPL/quotes/latest"][0]["body"]["quote"]["ap"], 187.43, "{sc}");
        assert_eq!(a["GET /v2/stocks/AAPL/trades/latest"][0]["body"]["trade"]["p"], 187.41, "{sc}");
        let clock = &a["GET /v2/clock"][0]["body"];
        let close: DateTime<Utc> = clock["next_close"].as_str().ok_or("expected JSON string")?.parse()?;
        let tick: DateTime<Utc> = sc["at"].as_str().ok_or("expected JSON string")?.parse()?;
        assert!(clock["is_open"] == true && close > tick, "both engines decide an open session: {sc}");
        if a.get("GET /v2/stocks/MSFT/quotes/latest").is_some() { index_scripts += 1; }
    }
    assert_eq!(index_scripts, 1, "only the index bot prices MSFT (a candidate it may probe)");
    Ok(())
}


struct GridDir { path: PathBuf, _temp: Option<tempfile::TempDir> }
impl GridDir { fn path(&self) -> &Path { &self.path } }
fn grid_dir(name: &str) -> GridDir {
    if let Ok(root) = std::env::var("RUST_GRID_CACHE") {
        // The source text keys the cache, so edited fixtures never reuse old results.
        use std::hash::{Hash, Hasher};
        let mut h=std::collections::hash_map::DefaultHasher::new();
        include_str!("../../script/rust/decisions.rb").hash(&mut h);
        let path=Path::new(&root).join(format!("{name}-{:x}",h.finish()));
        std::fs::create_dir_all(&path).unwrap(); GridDir {path,_temp:None}
    } else { let temp=tempfile::tempdir().unwrap(); GridDir {path:temp.path().into(),_temp:Some(temp)} }
}

/// Slice 7a: price_limit / sell_price_limit with `pause` on Kraken, Binance and Alpaca, buying and selling. Every tick, check
/// and check job queued (with its time) as Rails decides them.
#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_condition_grid() {
    let rails_root = tempfile::tempdir().unwrap();
    let rust_root = tempfile::tempdir().unwrap();
    built_grid("grid-conditions", rails_root.path());
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    assert_eq!(dirs.len(), 174, "the condition grid has {} scenarios", dirs.len());
    for d in &dirs { copy_dir(d, &rust_root.path().join(d.file_name().unwrap())); }
    rails(&["record", rails_root.path().to_str().unwrap()]);
    let mut failures = vec![];
    for d in &dirs {
        let name = d.file_name().unwrap().to_string_lossy().to_string();
        let rails_out: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(d.join("rails.json")).unwrap()).unwrap();
        let rust_out = deltabadger::parity::decide(&rust_root.path().join(&name)).await.unwrap();
        let mut rails_out = r5_cleanup_expected(&rails_out, &rust_out);
        // Named divergence (not the condition's): with its only pair unavailable, Rails' Kraken balance read (Bot::Fundable,
        // behind the pause) finds no EUR and mails end_of_funds; Rust reads the venue's EUR balance.
        if name.starts_with("cond-kraken-buy-missing_ticker") {
            assert_eq!(rails_out["mails"], serde_json::json!([{ "mail": "end_of_funds", "quote": "EUR" }]), "{name}: drop the listed divergence");
            rails_out["mails"] = serde_json::json!([]);
            rails_out["changes"]["bots"][0]["after"]["last_end_of_funds_notification"] = serde_json::Value::Null;
        }
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
        // Pin what Rails does, so that two silent sides cannot pass.
        let case = name.splitn(4, '-').nth(3).unwrap();
        let checks = rails_out["checks"].as_array().unwrap().len();
        let paused = ["unmet", "at_limit", "missing_ticker", "missing_ticker_unset", "server_error"].iter().any(|v| case.ends_with(v)) || case.starts_with("check_");
        let trades = !paused && !["zero_price", "rejected_key"].contains(&case)
            || (case.starts_with("check_met") && !(case == "check_met_no_carry" && name.contains("-buy-"))) || case == "check_lost_then_met";
        let sent = rails_out["sent"].as_array().unwrap().len();
        if sent != usize::from(trades) { failures.push(format!("{name}: Rails sent {sent} order(s)\n  rails: {rails_out}")); }
        let expected_checks = match case {
            "check_unmet" | "check_server_error" | "check_rejected_key" => 2,
            c if c.starts_with("check_") || paused => 1,
            _ => 0,
        };
        if checks != expected_checks { failures.push(format!("{name}: Rails queued {checks} check(s)\n  rails: {rails_out}")); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), dirs.len(), failures.join("\n"));
}

/// Plan CI T4: crypto index bots on Kraken (EUR), Binance (USDT and BTC), KuCoin (USDT and USDC) and Alpaca (USD crypto beside stocks), every venue
/// scripted at the Faraday adapter; the probe table (rejected keys, transport failures, a 5xx); rotation with a resting leaver
/// order and a carry; and each resync trigger followed by a tick (plan CI T3b), all decided by Rails and by Rust alike.
#[tokio::test(flavor = "current_thread")]
async fn rails_and_rust_decide_identically_across_the_venue_index_grid() -> Result<(), Box<dyn std::error::Error>> {
    let (_copies, outputs) = grid_outputs("grid-venue-index", 331).await;
    let mut failures = vec![];
    for (name, _dir, rails_out, rust_out) in &outputs {
        if rails_out != rust_out { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
        // Pinned on Rails' side, so that two silent engines cannot pass: what each case names really happened there.
        let sent: Vec<String> = rails_out["sent"].as_array().ok_or("sent")?.iter()
            .map(|o| o.get("pair").or_else(|| o.get("symbol")).and_then(|v| v.as_str()).unwrap_or_default().to_string()).collect();
        let members = rails_out["changes"]["bot_index_assets"].as_array().ok_or("members")?;
        let seated = members.iter().filter(|m| m["after"]["in_index"] == 1).count();
        let exited = members.iter().any(|m| m["after"]["in_index"] == 0 && m["after"]["exited_at"].is_string());
        let errors: Vec<String> = rails_out["changes"]["bot_activity_logs"].as_array().ok_or("logs")?.iter()
            .filter_map(|l| l["after"]["details"]["error"].as_str().map(str::to_string)).collect();
        let pinned = |ok: bool, what: &str| if ok { Ok(()) } else { Err(format!("{name}: Rails did not {what}: {rails_out}")) };
        let mut checks = vec![];
        if ["probe_401_", "probe_invalid_key"].iter().any(|c| name.contains(c)) {
            checks.push(pinned(sent.is_empty() && errors.iter().any(|e| e.contains(" rejected the API key: ")), "refuse the rejected key"));
        }
        if ["probe_not_sent", "probe_maybe_sent", "probe_5xx", "-entrant_unpriced-", "-entrant_no_ticker-", "-zero_market_cap-"].iter().any(|c| name.contains(c)) {
            let second = if name.starts_with("vi-binance_btc-") { "SOL" } else { "ETH" };
            checks.push(pinned(seated == 3 && !sent.iter().any(|p| p.starts_with(second)), "skip the second-ranked newcomer and seat the next"));
        }
        if name.contains("-leaver") || name.contains("-member_unavailable-") || name.contains("index_sync_then_tick") {
            checks.push(pinned(exited, "mark a member out of the index"));
        }
        if name.contains("-index_missing-") { checks.push(pinned(sent.is_empty() && errors == ["Index not found"], "fail with Index not found")); }
        if name.starts_with("vi-binance-top-") { checks.push(pinned(!sent.iter().any(|p| p == "USDTUSDT"), "skip the quote's own coin")); }
        if name == "vi-binance-top-fresh-market" { checks.push(pinned(seated == 3, "seat USDC beside BTC and ETH")); }
        if name.starts_with("vi-binance_btc-") { checks.push(pinned(!sent.iter().any(|p| p.starts_with("BTC")), "skip the quote's own coin")); }
        // KuCoin (plan K O6) at USDT and at USDC: each quote's own coin is skipped, the other stablecoin is a member.
        if name.starts_with("vi-kucoin-top-") { checks.push(pinned(!sent.iter().any(|p| p == "USDT-USDT"), "skip the quote's own coin")); }
        if name.starts_with("vi-kucoin_usdc-top-") { checks.push(pinned(!sent.iter().any(|p| p == "USDC-USDC"), "skip the quote's own coin")); }
        if name == "vi-kucoin_usdc-top-fresh-market" { checks.push(pinned(sent == ["BTC-USDC", "ETH-USDC", "USDT-USDC"], "seat USDT beside BTC and ETH at USDC")); }
        if name.contains("start_then_tick-unpriced_at_resync") { checks.push(pinned(exited, "seat the resync's choice and rotate it at the tick")); }
        // The owner's switch off Binance: SOL, a Binance-era member with no KuCoin pair, leaves; KuCoin's top three are bought.
        if name.contains("venue_switch_then_tick") {
            checks.push(pinned(exited && seated == 3 && sent == ["BTC-USDT", "ETH-USDT", "ADA-USDT"], "reseat the members on the new venue and buy them"));
        }
        if name.ends_with("closed_resync") { checks.push(pinned(sent.is_empty() && seated == 3, "resync with the market closed")); }
        // The resync's failure kinds: its newcomer probes are the tick's first reads, so the tick buys after each of them.
        if name.contains("_at_resync") && !name.contains("unpriced") { checks.push(pinned(!sent.is_empty() && seated >= 2, "buy at the tick after the failed resync")); }
        failures.extend(checks.into_iter().filter_map(Result::err));
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    Ok(())
}

/// Plan 8a: one webhook call of a signal bot (script/rust/decisions.rb signal_scenarios): Bot::SignalJob at `at` with its own
/// retries, against the engine's executor on a pristine copy, on Kraken, Binance, KuCoin and Alpaca (crypto and a stock).
/// Recorded per case: the rows, the lines, the mails and every order request the venue received (the assertion against a
/// double placement). The marker's `id` is the one value the two sides draw differently: Rails a fresh uuid, the engine its
/// intent's cl_ord_id. Listed, each asserted on its own terms:
/// - transport wording (Kraken, Binance, KuCoin): the harness scripts a transport failure as Client.network_failure names
///   it ("Faraday::ConnectionFailed: …", "Faraday::TimeoutError: …"); honeymaker's #with_rescue reports the exception's own
///   message. Same rows, lines and mails otherwise (as `refused_like_rails`).
/// - ambiguous wording (`unreadable`, `no_order_id`): both sides send once, write no row, raise the marker and log one
///   placement_ambiguous; only the error text in the two differs (the adapters name what they could not read).
/// - Alpaca `pre_send`: a refused connection provably sent nothing. Rails' Clients::Alpaca raises it and
///   #place_signal_order reads any raise as ambiguous (the marker, no row); the engine writes the failed row and mails, as
///   for any other definitive refusal, and the next call places.
#[tokio::test(flavor = "current_thread", start_paused = true)] // Exchange#with_transient_retry's backoff, never slept
async fn rails_and_rust_decide_identically_across_the_signal_grid() {
    let (_copies, outputs) = grid_outputs("grid-signal", 420).await;
    let marker = |out: &serde_json::Value| -> Option<serde_json::Value> {
        out["changes"]["bots"].as_array().unwrap().iter().find_map(|b| b["after"]["transient_data"].get("signal_placement_unresolved").cloned())
    };
    let normalised = |out: &serde_json::Value, wording: bool| -> serde_json::Value {
        let mut out = out.clone();
        for b in out["changes"]["bots"].as_array_mut().unwrap() {
            if let Some(m) = b["after"]["transient_data"].get_mut("signal_placement_unresolved").and_then(|m| m.as_object_mut()) {
                m.insert("id".into(), serde_json::json!("<id>"));
                if wording { m.insert("error".into(), serde_json::json!("<error>")); }
            }
        }
        if wording {
            for l in out["changes"]["bot_activity_logs"].as_array_mut().unwrap() {
                if l["after"]["event"] == "placement_ambiguous" { l["after"]["details"]["error"] = serde_json::json!("<error>"); }
            }
        }
        out
    };
    let mut failures = vec![];
    for (name, _, rails_out, rust_out) in &outputs {
        let honeymaker = ["signal-kraken-", "signal-binance-", "signal-kucoin-"].iter().any(|p| name.starts_with(p));
        let alpaca = name.starts_with("signal-alpaca");
        let variant = |v: &str| name.ends_with(&format!("-{v}"));
        let differs = if alpaca && variant("pre_send") {
            let rows = |out: &serde_json::Value| out["changes"]["transactions"].as_array().unwrap().iter().map(|t| t["after"]["status"].clone()).collect::<Vec<_>>();
            let one_sent = rails_out["sent"].as_array().unwrap().len() == 1 && rust_out["sent"] == rails_out["sent"];
            let rails_ambiguous = marker(rails_out).is_some() && rows(rails_out).is_empty() && rails_out["mails"].as_array().unwrap().is_empty();
            let rust_failed = marker(rust_out).is_none() && rows(rust_out) == [serde_json::json!(1)] && rust_out["mails"].as_array().unwrap().len() == 1;
            !(one_sent && rails_ambiguous && rust_failed)
        } else if variant("unreadable") || variant("no_order_id") {
            marker(rails_out).is_none() || normalised(rails_out, true) != normalised(rust_out, true)
        } else if honeymaker && (variant("pre_send") || variant("price_failed") || variant("timeout")) {
            let rust = rust_out.to_string().replace("Faraday::ConnectionFailed: ", "").replace("Faraday::TimeoutError: ", "");
            normalised(rails_out, false) != normalised(&serde_json::from_str(&rust).unwrap(), false)
        } else {
            normalised(rails_out, false) != normalised(rust_out, false)
        };
        if differs { failures.push(format!("{name}\n  rails: {rails_out}\n  rust:  {rust_out}")); }
    }
    assert!(failures.is_empty(), "{} of {} scenarios differ:\n{}", failures.len(), outputs.len(), failures.join("\n"));
    // Pinned on Rails' side, so that two silent sides cannot pass: what each case names really happened there.
    let mut pins = vec![];
    for (name, _, out, _) in &outputs {
        let variant = name.splitn(5, '-').nth(4).unwrap_or_default();
        let sent = out["sent"].as_array().unwrap().len();
        let rows: Vec<i64> = out["changes"]["transactions"].as_array().unwrap().iter().filter(|t| t["before"].is_null()).map(|t| t["after"]["status"].as_i64().unwrap()).collect();
        let lines: Vec<&str> = out["changes"]["bot_activity_logs"].as_array().unwrap().iter().map(|l| l["after"]["event"].as_str().unwrap()).collect();
        let mails = out["mails"].as_array().unwrap().len();
        let sell = name.contains("-sell-");
        let reads_balance = sell || name.contains("-percentage-");
        let expect: (usize, Vec<i64>, Vec<&str>, usize, bool) = match variant {
            "expired" | "unresolved_marker" => (0, vec![], vec!["signal_expired"], 0, false),
            "stopped" | "disabled" => (0, vec![], vec!["signal_ignored"], 0, false),
            "key_pending" => (0, vec![], vec!["signal_api_key_pending"], 0, false),
            "market_closed" => (0, vec![], vec!["signal_market_closed"], 0, false),
            "rule_removed" => (0, vec![], vec![], 0, false),
            "accepted" | "at_max_age" | "price_retried" | "free_below_fixed" => (1, vec![0], vec![], 0, false),
            "no_holding" if sell => (0, vec![2], vec!["order_skipped"], 0, false),
            "no_holding" => (1, vec![0], vec![], 0, false),
            "two_calls" => (2, vec![0, 0], vec![], 0, false),
            "price_failed" | "price_refused" => (0, vec![1], vec![], 1, false),
            "balance_failed" if reads_balance => (0, vec![1], vec![], 1, false),
            "balance_failed" => (1, vec![0], vec![], 0, false),
            "below_minimum" => (0, vec![2], vec!["order_skipped"], 0, false),
            "rejected" | "rejected_after_success" => (1, vec![1], vec![], 1, false),
            "rejected_after_failure" | "rejected_after_skip" => (1, vec![1], vec![], 0, false),
            "server_error" | "timeout" | "unreadable" | "no_order_id" => (1, vec![], vec!["placement_ambiguous"], 0, true),
            "pre_send" if name.starts_with("signal-alpaca") => (1, vec![], vec!["placement_ambiguous"], 0, true),
            "pre_send" => (1, vec![1], vec![], 1, false),
            "safe_then_accepted" => (2, vec![0], vec![], 0, false),
            "safe_then_rejected" | "safe_twice" => (2, vec![1], vec![], 1, false),
            other => { pins.push(format!("{name}: no expectation pinned for {other}")); continue; }
        };
        let got = (sent, rows, lines, mails, marker(out).is_some());
        if got != expect { pins.push(format!("{name}: Rails did {got:?}, pinned {expect:?}")); }
        if variant == "free_below_fixed" && name.contains("-sell-fixed-") && !out["sent"].to_string().contains(if name.contains("stock") { "\"0.100000000\"" } else { "0.001" }) {
            pins.push(format!("{name}: the fixed sell is not capped at the free base: {}", out["sent"]));
        }
    }
    assert!(pins.is_empty(), "pinned expectations:\n{}", pins.join("\n"));
}
