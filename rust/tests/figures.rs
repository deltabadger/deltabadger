#![cfg(unix)] // shells out to bin/rails
//! Figures parity: Rails' metrics, live figures, chart and account totals against this library's, on identical
//! databases built through the Rails models, with the market scripted on both sides and the clock frozen
//! (script/rust/figures.rb is the Rails half, deltabadger::figures::parity the Rust half).
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// The scenarios of script/rust/figures.rb: 72 written by hand, one per branch of the code they name, and 24 seeded
/// histories that mix every kind of order with splits.
const SCENARIOS: usize = 103;

fn rails(args: &[&str]) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(root.join("bin/rails"));
    cmd.current_dir(root).args(["runner", "script/rust/figures.rb"]).args(args).env_remove("DATABASE_URL")
        .env("APP_ROOT_URL", "http://localhost:3000") // config/environments/development.rb requires it
        .env("SKIP_TEST_DATABASE", "true");
    for db in ["primary", "queue", "cache", "cable"] { // boot Rails against scratch databases only
        cmd.env(format!("{}_DATABASE_URL", db.to_uppercase()), format!("sqlite3:{}/{db}.sqlite3", scratch.path().display()));
    }
    let out = cmd.output().expect("bin/rails runs");
    assert!(out.status.success(), "bin/rails runner script/rust/figures.rb {args:?} failed:\n{}", String::from_utf8_lossy(&out.stderr));
}

fn sha(path: &Path) -> String { hex::encode(Sha256::digest(std::fs::read(path).unwrap())) }

/// Every row of every table, in order: what "the database did not change" means, row for row.
fn rows(path: &Path) -> String {
    let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    let mut hash = Sha256::new();
    for table in tables {
        let mut statement = c.prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid")).unwrap();
        let columns = statement.column_count();
        let mut query = statement.query([]).unwrap();
        while let Some(row) = query.next().unwrap() {
            hash.update(table.as_bytes());
            for i in 0..columns { hash.update(format!("{:?}|", row.get_ref(i).unwrap()).as_bytes()); }
        }
    }
    hex::encode(hash.finalize())
}

struct Scenario {
    name: String,
    rails: Value,
    rust: Result<Value, String>,
    /// The database copy's bytes and its rows, before and after this library read it.
    before: (String, String),
    after: (String, String),
    /// Bytes in the copy's write-ahead log afterwards.
    logged: u64,
    /// How long this library took over the scenario.
    took: std::time::Duration,
}

struct Grid { scenarios: Vec<Scenario>, _roots: (tempfile::TempDir, tempfile::TempDir) }

/// Built once per test binary: Rails builds every scenario's install, the installs are copied before Rails reads
/// them, Rails records its figures, and this library computes its own from the copies.
fn grid() -> &'static Grid {
    static GRID: OnceLock<Grid> = OnceLock::new();
    GRID.get_or_init(|| {
        let (rails_root, rust_root) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        rails(&["grid", rails_root.path().to_str().unwrap()]);
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(rails_root.path()).unwrap().map(|e| e.unwrap().path()).filter(|p| p.join("scenario.json").exists()).collect();
        dirs.sort();
        for dir in &dirs {
            let copy = rust_root.path().join(dir.file_name().unwrap());
            std::fs::create_dir_all(&copy).unwrap();
            for file in ["production.sqlite3", "scenario.json"] { std::fs::copy(dir.join(file), copy.join(file)).unwrap(); }
        }
        rails(&["record", rails_root.path().to_str().unwrap()]);
        let scenarios = dirs.iter().map(|dir| {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            let copy = rust_root.path().join(&name);
            let database = copy.join("production.sqlite3");
            let state = || (sha(&database), rows(&database));
            let before = state();
            let started = std::time::Instant::now();
            let rust = deltabadger::figures::parity::figures(&copy).map_err(|e| format!("{e:?}"));
            let took = started.elapsed();
            // Reading a database in WAL mode creates its -wal and -shm files; a write would put bytes in the first.
            let logged = std::fs::metadata(copy.join("production.sqlite3-wal")).map_or(0, |log| log.len());
            Scenario { rails: serde_json::from_str(&std::fs::read_to_string(dir.join("rails.json")).unwrap()).unwrap(), rust, before, after: state(), logged, took, name }
        }).collect();
        Grid { scenarios, _roots: (rails_root, rust_root) }
    })
}

/// Where two figures part: both sides around the first character that differs.
/// A figure this library does not compute is an object (`{"not_computed": reason}`) where Rails has a text. It is
/// not a difference: `what_is_not_computed_is_named` holds the list of them, one by one.
fn difference(name: &str, path: &str, rails: &Value, rust: &Value, out: &mut Vec<String>) {
    match (rails, rust) {
        (_, Value::Object(b)) if b.contains_key("not_computed") => {}
        (Value::Object(a), Value::Object(b)) => {
            let mut keys: Vec<&String> = a.keys().chain(b.keys()).collect();
            keys.sort();
            keys.dedup();
            for key in keys { difference(name, &format!("{path}.{key}"), a.get(key).unwrap_or(&Value::Null), b.get(key).unwrap_or(&Value::Null), out); }
        }
        (Value::String(a), Value::String(b)) if a != b => {
            let at = a.bytes().zip(b.bytes()).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
            let cut = |s: &str| s.get(at.saturating_sub(120)..(at + 80).min(s.len())).unwrap_or("<not on a character boundary>").to_string();
            out.push(format!("{name} {path}, character {at}\n  rails: {}\n  rust:  {}", cut(a), cut(b)));
        }
        (a, b) if a != b => out.push(format!("{name} {path}\n  rails: {a}\n  rust:  {b}")),
        _ => {}
    }
}

/// Compares what `pick` takes from each side of every scenario, character for character.
fn compare(what: &str, pick: impl Fn(&Value) -> Value) {
    let grid = grid();
    assert_eq!(grid.scenarios.len(), SCENARIOS, "the grid has {} scenarios", grid.scenarios.len());
    let (mut out, mut differing) = (vec![], 0);
    for scenario in &grid.scenarios {
        let before = out.len();
        match &scenario.rust {
            Ok(rust) => difference(&scenario.name, "", &pick(&scenario.rails), &pick(rust), &mut out),
            Err(error) => out.push(format!("{}: {error}", scenario.name)),
        }
        if out.len() > before { differing += 1; }
    }
    assert!(out.is_empty(), "{what}: {differing} of {SCENARIOS} scenarios differ:\n{}", out[..out.len().min(8)].join("\n"));
}

/// One figure of every computed bot of a scenario.
fn of_bots(names: &'static [&'static str]) -> impl Fn(&Value) -> Value {
    move |out: &Value| {
        Value::Object(out["bots"].as_object().into_iter().flatten().filter(|(_, bot)| bot.get("metrics").is_some())
            .map(|(id, bot)| (id.clone(), Value::Object(names.iter().map(|name| (name.to_string(), bot[*name].clone())).collect()))).collect())
    }
}

#[test]
fn the_walk_over_the_orders_matches_rails_in_every_scenario() {
    compare("metrics", of_bots(&["metrics"]));
    // The grid means what it says only if the figures in it are figures: no scenario's walk ends in a raise.
    let walked = grid().scenarios.iter().flat_map(|s| s.rails["bots"].as_object().unwrap().values()).filter_map(|bot| bot["metrics"].as_str())
        .filter(|metrics| !metrics.contains("\"raised\"")).count();
    assert_eq!(walked, 132, "bots walked on the Rails side");
}

#[test]
fn the_database_is_only_read() {
    for scenario in &grid().scenarios {
        assert!(scenario.rust.is_ok(), "{}: {:?}", scenario.name, scenario.rust);
        assert_eq!(scenario.before.0, scenario.after.0, "{}: production.sqlite3 changed", scenario.name);
        assert_eq!(scenario.before.1, scenario.after.1, "{}: a row changed", scenario.name);
        assert_eq!(scenario.logged, 0, "{}: something was written to the write-ahead log", scenario.name);
    }
}

#[test]
fn figures_are_computed_only_from_a_marked_scratch_copy() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("scenario.json"), r#"{"at": "2026-09-01T00:00:00Z", "bot_ids": [1], "script": {}}"#).unwrap();
    let error = deltabadger::figures::parity::figures(dir.path()).unwrap_err();
    assert!(matches!(&error, deltabadger::figures::FiguresError::Data(m) if m.contains("parity_scratch")), "{error:?}");
}

/// 20,000 orders of one asset, a sale of all of them, a holding with no price and no candles: Rails' figures for a
/// long history, equal like any other, and within a bound no build should come near. What a quadratic pass
/// cannot meet is in rust/examples/figures_limits.rs: 100,000 orders against 6,250, with no Rails to wait for.
#[test]
fn a_long_history_is_computed_in_linear_time() {
    let scenario = grid().scenarios.iter().find(|s| s.name == "long_history").unwrap();
    assert!(scenario.took < std::time::Duration::from_secs(60), "long_history took {:?}", scenario.took);
    let slowest = grid().scenarios.iter().filter(|s| s.name != "long_history").map(|s| s.took).max().unwrap();
    println!("long_history took {:?}; the slowest other scenario {slowest:?}", scenario.took);
    // Beside them, for the record: what Rails took over the same figures (with its cache cold, as here).
    for scenario in grid().scenarios.iter().filter(|s| ["long_history", "priced_pairs", "unpriced_rebalances"].contains(&s.name.as_str())) {
        println!("{}: Rails {} s, this library {:?}", scenario.name, scenario.rails["seconds"], scenario.took);
    }
}

#[test]
fn the_live_figures_match_rails_in_every_scenario() {
    compare("live", of_bots(&["live"]));
    let lives: Vec<&str> = grid().scenarios.iter().flat_map(|s| s.rails["bots"].as_object().unwrap().values()).filter_map(|bot| bot["live"].as_str()).collect();
    let count = |needle: &str| lives.iter().filter(|live| live.contains(needle)).count();
    // Every way the pass can end is met: marked at the market, fallen back to the last fills, or raised to a retry.
    assert_eq!((count("\"live_prices\""), count("\"prices_stale\":true"), count("\"raised\"")), (114, 7, 1), "of {} bots", lives.len());
}

/// Rails leaves a held asset it cannot price out of the value without a word. The figures stay Rails'; beside them
/// the library names what was left out and why. Everywhere else the list is empty.
#[test]
fn holdings_left_out_of_the_value_are_named_beside_the_figures() {
    let mut named = std::collections::BTreeMap::new();
    for scenario in &grid().scenarios {
        for (id, bot) in scenario.rust.as_ref().unwrap()["bots"].as_object().unwrap() {
            if bot["unpriced"].as_array().is_some_and(|list| !list.is_empty()) { named.insert(format!("{} {id}", scenario.name), bot["unpriced"].to_string()); }
        }
    }
    let expected: std::collections::BTreeMap<String, String> = [
        // The venue lists it and does not trade it: a member, both members, a quitter of an index, a member never priced.
        ("delisted_member 1", r#"[["BBB","delisted"]]"#),
        ("all_delisted 1", r#"[["AAA","delisted"],["BBB","delisted"]]"#),
        ("index_rotation 1", r#"[["LLL","delisted"]]"#),
        ("unpriceable_first 1", r#"[["LLL","delisted"]]"#),
        // It has a ticker, and the venue's answer has no price for it.
        ("price_missing 1", r#"[["BBB","no_price"]]"#),
        ("price_nan 1", r#"[["BBB","no_price"]]"#),
        ("price_infinity 1", r#"[["BTC","no_price"]]"#),
        ("price_unreadable 1", r#"[["BBB","no_price"]]"#),
        ("candles_end_early 1", r#"[["BBB","no_price"]]"#),
        ("split_fill_past_the_candles 1", r#"[["AAA","no_price"]]"#),
        ("long_history 1", r#"[["BBB","no_price"]]"#),
        // Rows recorded under a string that no listing of the venue is spelled as.
        ("old_rows 1", r#"[["ZZZ","no_ticker"],["aaa","no_ticker"]]"#),
    ].into_iter().map(|(bot, list)| (bot.to_string(), list.to_string())).collect();
    assert_eq!(named, expected);
}

#[test]
fn the_chart_marked_at_market_matches_rails_in_every_scenario() {
    compare("marked", of_bots(&["marked"]));
    compare("chart", of_bots(&["chart"]));
    let charts: Vec<&Value> = grid().scenarios.iter().flat_map(|s| s.rails["bots"].as_object().unwrap().values()).map(|bot| &bot["chart"]).filter(|chart| chart.is_object()).collect();
    let with = |name: &str, needle: &str| charts.iter().filter(|chart| chart[name].as_str().is_some_and(|text| text.contains(needle))).count();
    // Points that kept their fill mark (a null in a holding's series) and prices outside a grid's reach are both met.
    assert_eq!((charts.len(), with("assets", "null"), with("prices", "null"), with("pnl-only", "true")), (121, 11, 23, 1));
}

/// The chart marked at market leaves out of every point a holding the bot has no ticker for today, though the
/// money that bought it stays in what went in. Named beside the chart, as the live pass names its own; everywhere
/// else the list is empty.
#[test]
fn holdings_left_out_of_the_chart_are_named_beside_it() {
    let mut named = std::collections::BTreeMap::new();
    for scenario in &grid().scenarios {
        let rust = scenario.rust.as_ref().unwrap();
        let mut by_bot = vec![];
        for (id, bot) in rust["bots"].as_object().unwrap() {
            let Some(list) = bot["chart_omitted"].as_array().filter(|list| !list.is_empty()) else { continue };
            named.insert(format!("{} {id}", scenario.name), Value::Array(list.clone()).to_string());
            by_bot.extend(list.iter().map(|u| serde_json::json!([id.parse::<i64>().unwrap(), u[0], u[1]])));
        }
        // The account's list is the same, by bot, wherever the totals are computed.
        if let Some(listed) = rust["chart_omitted"].as_array() { assert_eq!(listed, &by_bot, "{}", scenario.name); }
    }
    let expected: std::collections::BTreeMap<String, String> = [
        // Delisted while held: the live pass names them too.
        ("delisted_member 1", r#"[["BBB","delisted"]]"#),
        ("index_rotation 1", r#"[["LLL","delisted"]]"#),
        ("unpriceable_first 1", r#"[["LLL","delisted"]]"#),
        ("old_rows 1", r#"[["ZZZ","no_ticker"],["aaa","no_ticker"]]"#),
        // Sold out, then delisted: nothing of it is held, so only the chart leaves it out.
        ("sold_out_then_delisted 1", r#"[["AAA","delisted"]]"#),
        // all_delisted is not here: with no ticker at all there are no candles, and the chart is the walk's own.
    ].into_iter().map(|(bot, list)| (bot.to_string(), list.to_string())).collect();
    assert_eq!(named, expected);
}

/// The figures the bots list is made of, for the scenarios whose every bot is computed.
fn totals(out: &Value) -> Value {
    serde_json::json!({ "profit_in_usd": of_bots(&["profit_in_usd"])(out), "global_pnl": out["global_pnl"], "global_pnl_snapshot": out["global_pnl_snapshot"],
                        "pnl_history": out["pnl_history"], "denomination": out["denomination"] })
}

#[test]
fn the_accounts_totals_match_rails_in_every_scenario() {
    compare("totals", |out| if out["bots"].as_object().is_some_and(|bots| bots.values().all(|bot| bot.get("metrics").is_some())) { totals(out) } else { Value::Null });
}

#[test]
fn the_market_is_asked_for_exactly_what_rails_asks_it() {
    // Where a figure is not computed the library stops asking, so those scenarios are left out here.
    let whole = |s: &&Scenario| !s.rust.as_ref().is_ok_and(|out| out.to_string().contains("not_computed"));
    assert_eq!(grid().scenarios.iter().filter(whole).count(), SCENARIOS - 2);
    for scenario in grid().scenarios.iter().filter(whole) {
        assert_eq!(scenario.rails["requests"], scenario.rust.as_ref().unwrap()["requests"], "{}", scenario.name);
    }
    let asked: usize = grid().scenarios.iter().map(|s| s.rails["requests"].as_array().map_or(0, Vec::len)).sum();
    assert_eq!(asked, 631, "requests on the Rails side");
}

/// The one scenario with a bot this library does not compute (a pair bot beside a basket).
const NOT_COMPUTED: &str = "account_pair_bot";

#[test]
fn a_bot_of_another_type_is_not_computed_and_neither_is_a_total_that_needs_it() {
    let scenario = grid().scenarios.iter().find(|s| s.name == NOT_COMPUTED).unwrap();
    let rust = scenario.rust.as_ref().unwrap();
    assert_eq!(scenario.rails["bots"]["2"], serde_json::json!({ "type": "Bots::DcaSingleAsset" }));
    let reason = "bot 2 is a Bots::DcaSingleAsset: only baskets and index bots are computed";
    assert_eq!(rust["bots"]["2"], serde_json::json!({ "not_computed": reason }));
    for total in ["global_pnl", "global_pnl_snapshot", "pnl_history"] {
        assert_eq!(rust[total], serde_json::json!({ "not_computed": reason }), "{total}");
        assert!(scenario.rails[total].as_str().is_some_and(|text| text.contains("percent")), "Rails has a {total} here");
    }
    // The basket beside it is computed as in any other scenario (the three tests above compare it).
    assert!(rust["bots"]["1"]["metrics"].is_string());
}

/// Beside the account's totals: the same holdings, by bot. A deleted bot is in no total and in no list.
#[test]
fn the_accounts_totals_name_what_they_leave_out() {
    for scenario in &grid().scenarios {
        let rust = scenario.rust.as_ref().unwrap();
        let Some(listed) = rust["unpriced"].as_array() else { continue };
        let by_bot: Vec<Value> = rust["bots"].as_object().unwrap().iter()
            .flat_map(|(id, bot)| bot["unpriced"].as_array().into_iter().flatten().map(move |u| serde_json::json!([id.parse::<i64>().unwrap(), u[0], u[1]]))).collect();
        assert_eq!(listed, &by_bot, "{}", scenario.name);
    }
    let of = |name: &str| grid().scenarios.iter().find(|s| s.name == name).unwrap().rust.as_ref().unwrap()["unpriced"].to_string();
    assert_eq!(of("all_delisted"), r#"[[1,"AAA","delisted"],[1,"BBB","delisted"]]"#);
    assert_eq!(of("basket_buys"), "[]");
}

/// Everything this library returns as "not computed", figure by figure, with what Rails has in its place.
#[test]
fn what_is_not_computed_is_named() {
    fn collect(name: &str, path: &str, rails: &Value, rust: &Value, out: &mut Vec<(String, String, String)>) {
        match rust {
            Value::Object(b) if b.contains_key("not_computed") => out.push((format!("{name}{path}"), b["not_computed"].as_str().unwrap().to_string(), rails.to_string())),
            Value::Object(b) => for (key, value) in b { collect(name, &format!("{path}.{key}"), &rails[key], value, out); },
            _ => {}
        }
    }
    let mut found = vec![];
    for scenario in &grid().scenarios { collect(&scenario.name, "", &scenario.rails, scenario.rust.as_ref().unwrap(), &mut found); }
    let listed: Vec<String> = found.iter().map(|(place, reason, _)| format!("{place}: {reason}")).collect();
    let pair = "bot 2 is a Bots::DcaSingleAsset: only baskets and index bots are computed";
    let number = deltabadger::figures::NOT_A_NUMBER;
    let mut expected: Vec<String> = [".bots.2", ".global_pnl", ".global_pnl_snapshot", ".pnl_history"].iter().map(|place| format!("account_pair_bot{place}: {pair}")).collect();
    // A candle that is no number: the walk and the live figures stand, the chart and what is drawn from it do not.
    expected.extend([".bots.1.marked", ".bots.1.chart", ".pnl_history"].iter().map(|place| format!("candle_nan{place}: {number}")));
    assert_eq!(listed, expected);
    // Unreadable batch prices are omitted by Rails and named as no_price above; an unreadable candle
    // still prevents the chart from being computed.
    for (place, reason, rails) in &found {
        if reason == number { assert!(rails.contains("null"), "{place}: Rails has {rails}"); }
    }
}
