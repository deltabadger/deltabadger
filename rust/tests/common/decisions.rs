//! The decision-parity grids of the recording harness: built and recorded by 2.x on scratch databases, played by
//! deltabadger::parity::decide on pristine copies (tests/parity.rs, tests/tick_basket.rs).
use std::path::Path;

/// The recording harness's answer to `<args>` (the last arg is the directory it wrote to), frozen (super::freeze::step).
pub fn v2(args: &[&str]) { super::freeze::step(args) }

/// `command`'s grid, as `v2(&[command, into])` builds it: frozen (super::frozen), and copied into `into` for every
/// test that asks.
pub fn built_grid(command: &str, into: &Path) {
    let template = super::frozen(&format!("decisions-{command}"), super::has_subdirs);
    super::copy_tree(&template, into);
}

pub fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for f in ["production.sqlite3", "production_queue.sqlite3", "scenario.json"] { std::fs::copy(from.join(f), to.join(f)).unwrap(); }
}

/// R5: failure bookkeeping preserves unrelated NULL evidence. The recorded grids expose
/// 2.x deleting this pre-existing NULL flag; require Rust to preserve it explicitly.
/// Return a comparison copy of 2.x; never rewrite either recorded output or any money field.
pub fn r5_cleanup_expected(v2: &serde_json::Value, rust: &serde_json::Value) -> serde_json::Value {
    const KEY: &str = "missed_quote_amount_was_set";
    let mut expected = v2.clone();
    for row in expected["changes"]["bots"].as_array_mut().unwrap() {
        if row["before"]["transient_data"].get(KEY) != Some(&serde_json::Value::Null)
            || !row["after"]["transient_data"].is_object()
            || row["after"]["transient_data"].get(KEY).is_some() { continue; }
        // Rust left the bot row untouched (a swap that started and ended within the run): the flag kept is all 2.x changed.
        if let Some(mine) = rust["changes"]["bots"].as_array().unwrap().iter().find(|b| b["id"] == row["id"]) {
            assert_eq!(mine["before"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 same initial NULL flag");
            assert_eq!(mine["after"]["transient_data"].get(KEY), Some(&serde_json::Value::Null), "R5 failure cleanup preserves unrelated NULL flag");
        }
        row["after"]["transient_data"][KEY] = serde_json::Value::Null;
    }
    // A row whose only change was that flag is no change at all once it is kept.
    expected["changes"]["bots"].as_array_mut().unwrap().retain(|row| row["before"] != row["after"]);
    sell_row_cadence_expected(&mut expected, rust);
    status_lines_expected(&mut expected);
    expected
}

/// D-LIQUIDATION-ONE-LINE: 2.x's status lines beside an order (`liquidation_placed`, `redeploy_placed`,
/// `rebalance_<side>_placed`) are the page's status rows in Rust, never written. 2.x's inserted rows of them go; later
/// lines' ids move down with them.
pub fn status_lines_expected(expected: &mut serde_json::Value) {
    const DROPPED: [&str; 4] = ["liquidation_placed", "redeploy_placed", "rebalance_sell_placed", "rebalance_buy_placed"];
    let Some(logs) = expected["changes"]["bot_activity_logs"].as_array_mut() else { return };
    let mut dropped = 0;
    logs.retain_mut(|row| {
        if row["before"].is_null() && row["after"]["event"].as_str().is_some_and(|e| DROPPED.contains(&e)) { dropped += 1; return false; }
        if dropped > 0 && row["before"].is_null() { let id = row["id"].as_i64().unwrap() - dropped; row["id"] = serde_json::json!(id); row["after"]["id"] = serde_json::json!(id); }
        true
    });
}

/// D-SELL-ROW-CADENCE: a selling bot's new REGULAR sell row records the sell cadence and the sell amount; 2.x writes the
/// buy interval and buy quote_amount. Only those two columns of such a row are taken from Rust (tests/tick.rs pins their
/// values); every other column and row is compared as 2.x recorded it.
fn sell_row_cadence_expected(expected: &mut serde_json::Value, rust: &serde_json::Value) {
    let Some(ours) = rust["changes"]["transactions"].as_array() else { return };
    let Some(theirs) = expected["changes"]["transactions"].as_array_mut() else { return };
    for row in theirs.iter_mut().filter(|t| t["before"].is_null() && t["after"]["side"] == 1 && t["after"]["transaction_type"] == "REGULAR") {
        let Some(mine) = ours.iter().find(|t| t["before"].is_null() && t["id"] == row["id"]) else { continue };
        for column in ["bot_interval", "bot_quote_amount"] { row["after"][column] = mine["after"][column].clone(); }
    }
}
