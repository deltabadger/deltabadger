//! PLAN-B11-9 §5 #2–#14 (D-B11-9-V2-NAMES): migration 20261117000001 renames the names Rust stored while 2.x was called
//! Rails, on the frozen beta installs (tests/fixtures/b11_9_beta, written by the pre-rename writers in PR 1). These tests
//! name the old names by necessity: they are the migration's input.
mod common;
use common::web::{self, Browser, TestClock};
use deltabadger::engine::{handover, model, staleness};
use deltabadger::jobs::{reference, state};
use deltabadger::lease;
use deltabadger::store::{self, Migration, Paths, Registry, StoreError, REGISTRY, RUST_MIGRATIONS};
use rusqlite::types::Value as Sql;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

const V: &str = "20261117000001";
const RENAMED_TABLES: [(&str, &str); 8] = [
    ("rust_rails_attempt_acks", "rust_v2_attempt_acks"), ("rust_rails_job_acks", "rust_v2_job_acks"), ("rust_rails_unrecorded", "rust_v2_unrecorded"),
    ("rust_rails_unrecorded_live", "rust_v2_unrecorded_live"), ("rust_rails_unrecorded_offers", "rust_v2_unrecorded_offers"),
    ("rust_rails_unrecorded_resolutions", "rust_v2_unrecorded_resolutions"), ("rust_rails_unrecorded_requests", "rust_v2_unrecorded_requests"),
    ("rust_rails_unrecorded_notices", "rust_v2_unrecorded_notices"),
];
const RENAMED_KEYS: [(&str, &str); 4] = [
    ("rust_rails_wakes", "rust_v2_wakes"), ("rust_rails_queue_done", "rust_v2_queue_done"),
    ("rust_rails_unrecorded.live_cursor", "rust_v2_unrecorded.live_cursor"), ("rust_job.rails_unrecorded_report", "rust_job.v2_unrecorded_report"),
];
const EVENTS: [&str; 10] = ["candidates", "none_seen", "unchecked", "open", "listed", "recovered", "not_added", "not_mine", "unsettled", "settled_lower"];

fn paths(dir: &Path) -> Paths { Paths::from_env(&|_| None, dir) }
fn lock(p: &Paths) -> lease::EngineLock { lease::lock(p, chrono::Utc::now()).unwrap() }
fn migrate(p: &Paths) -> Vec<String> { store::migrate(p, &lock(p)).unwrap() }
fn beta() -> Vec<(String, tempfile::TempDir)> { common::freeze::installs() }
fn install(name: &str) -> tempfile::TempDir { beta().into_iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no frozen install {name}")).1 }
fn count(c: &Connection, sql: &str) -> i64 { c.query_row(sql, [], |r| r.get(0)).unwrap() }
/// The registry of the build before this one: every migration and contract older than V, nothing retired.
/// (Both lists are in version order, V last.)
fn older() -> Registry<'static> {
    let m = RUST_MIGRATIONS.iter().position(|m| m.version >= V).unwrap();
    let t = REGISTRY.tables.iter().position(|(v, _)| *v >= V).unwrap();
    Registry { migrations: &RUST_MIGRATIONS[..m], tables: &REGISTRY.tables[..t], retired: &[] }
}

/// Every table's rows by rowid, each cell as SQLite holds it.
type Rows = BTreeMap<String, (Vec<String>, Vec<(i64, Vec<Sql>)>)>;
fn rows(c: &Connection) -> Rows {
    let tables: Vec<String> = c.prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap()
        .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    tables.into_iter().map(|t| {
        let mut s = c.prepare(&format!("SELECT rowid, * FROM \"{t}\" ORDER BY rowid")).unwrap();
        let columns: Vec<String> = s.column_names().iter().skip(1).map(|c| c.to_string()).collect();
        let n = columns.len();
        let rows = s.query_map([], |r| Ok((r.get(0)?, (1..=n).map(|i| r.get(i)).collect::<Result<Vec<Sql>, _>>()?))).unwrap().collect::<Result<_, _>>().unwrap();
        (t, (columns, rows))
    }).collect()
}
fn objects(c: &Connection) -> Vec<(String, String, String)> {
    c.prepare("SELECT type, name, coalesce(sql, '') FROM sqlite_master ORDER BY type, name").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().collect::<Result<_, _>>().unwrap()
}
fn text(v: &Sql) -> Option<&str> { if let Sql::Text(t) = v { Some(t) } else { None } }
fn json_of(v: &Sql) -> Option<Value> { text(v).and_then(|t| serde_json::from_str(t).ok()) }

/// What V does to one JSON value or cell, written again in Rust as the plan states it (§1.1, §3): the expectation the
/// SQL is held to.
fn renamed_prefix(s: &str, old: &str, new: &str) -> Option<String> { s.strip_prefix(old).map(|rest| format!("{new}{rest}")) }
fn expected_transient(mut v: Value) -> Value {
    if let Some(calls) = v.get_mut("rust_signal_calls").and_then(Value::as_array_mut).filter(|a| a.iter().all(Value::is_object)) {
        for call in calls {
            if let Some(s) = call["source"].as_str().and_then(|s| renamed_prefix(s, "rails:", "v2:")) { call["source"] = json!(s); }
        }
    }
    if let Some(m) = v.get_mut("signal_placement_unresolved").filter(|m| m.is_object()) {
        if let Some(s) = m["id"].as_str().and_then(|s| renamed_prefix(s, "rails-job:", "v2-job:")) { m["id"] = json!(s); }
    }
    v
}
/// (table it lands in, the row as V leaves it, whether a JSON cell changed: compared as JSON, not as text).
fn expected_row(table: &str, columns: &[String], row: &[Sql]) -> (String, Vec<Sql>, Option<usize>) {
    let table_now = RENAMED_TABLES.iter().find(|(old, _)| *old == table).map_or(table, |(_, new)| new).to_string();
    let mut row = row.to_vec();
    let at = |name: &str| columns.iter().position(|c| c == name).unwrap();
    let mut json_cell = None;
    match table {
        "rust_withdrawal_ids" => if text(&row[at("source")]) == Some("rails") { row[at("source")] = Sql::Text("v2".into()); },
        "rust_rule_evaluations" => if let Some(s) = text(&row[at("source")]).and_then(|s| renamed_prefix(s, "rails:", "v2:")) { row[at("source")] = Sql::Text(s); },
        "app_configs" => {
            let (k, v) = (at("key"), at("value"));
            if let Some((_, new)) = RENAMED_KEYS.iter().find(|(old, _)| text(&row[k]) == Some(*old)) { row[k] = Sql::Text(new.to_string()); }
            if text(&row[k]).is_some_and(|k| k.starts_with("rust_job.")) {
                if let Some(Value::Object(mut o)) = json_of(&row[v]).filter(|o| o.get("rails_at").is_some()) {
                    let at = o.remove("rails_at").unwrap();
                    o.insert("v2_at".into(), at);
                    row[v] = Sql::Text(Value::Object(o).to_string());
                    json_cell = Some(v);
                }
            }
        }
        "bots" => {
            let t = at("transient_data");
            if let Some(old) = json_of(&row[t]) {
                let new = expected_transient(old.clone());
                if new != old { row[t] = Sql::Text(new.to_string()); json_cell = Some(t); }
            }
        }
        "bot_activity_logs" => {
            let (e, d) = (at("event"), at("details"));
            if let Some(kind) = text(&row[e]).and_then(|e| e.strip_prefix("rails_order_")).filter(|k| EVENTS.contains(k)) { row[e] = Sql::Text(format!("v2_order_{kind}")); }
            if matches!(text(&row[e]), Some("upgrade_signal_unresolved" | "signal_placement_resolved")) {
                if let Some(mut details) = json_of(&row[d]) {
                    if let Some(s) = details["marker_id"].as_str().and_then(|s| renamed_prefix(s, "rails-job:", "v2-job:")) {
                        details["marker_id"] = json!(s);
                        row[d] = Sql::Text(details.to_string());
                        json_cell = Some(d);
                    }
                }
            }
        }
        _ => {}
    }
    (table_now, row, json_cell)
}

// ---- #3's precondition: the fixture holds every item (moved from rust_migrations.rs, PR 1) ---------------------------

/// Every frozen install loads as an install this build accepts (V pending), and together they hold each stored name V
/// moves (§1.1), the two kept diagnostics (r1.1) and a mid-takeover install (#6): the tests below can't pass on an empty
/// fixture.
#[test]
fn the_frozen_beta_fixture_holds_every_stored_name_the_rename_moves() {
    let mut items: Vec<(String, String)> = RENAMED_TABLES.iter().map(|(t, _)| (format!("{t} rows"), format!("SELECT count(*) FROM {t}"))).collect();
    items.extend(RENAMED_KEYS.iter().map(|(k, _)| (format!("key {k}"), format!("SELECT count(*) FROM app_configs WHERE key = '{k}'"))));
    items.extend(EVENTS.iter().map(|e| (format!("event rails_order_{e}"), format!("SELECT count(*) FROM bot_activity_logs WHERE event = 'rails_order_{e}'"))));
    for (what, sql) in [
        ("its source index", "SELECT count(*) FROM sqlite_master WHERE type = 'index' AND name = 'index_rust_rails_unrecorded_on_source'"),
        ("a job's rails_at", "SELECT count(*) FROM app_configs WHERE substr(key, 1, 9) = 'rust_job.' AND json_valid(value) AND json_type(value, '$.rails_at') = 'text'"),
        ("withdrawal id source 'rails'", "SELECT count(*) FROM rust_withdrawal_ids WHERE source = 'rails'"),
        ("rule evaluation source 'rails:'", "SELECT count(*) FROM rust_rule_evaluations WHERE substr(source, 1, 6) = 'rails:'"),
        ("signal call source 'rails:'", "SELECT count(*) FROM bots, json_each(bots.transient_data, '$.rust_signal_calls') e WHERE substr(json_extract(e.value, '$.source'), 1, 6) = 'rails:'"),
        ("marker 'rails-job:'", "SELECT count(*) FROM bots WHERE substr(json_extract(transient_data, '$.signal_placement_unresolved.id'), 1, 10) = 'rails-job:'"),
        ("upgrade_signal_unresolved marker_id", "SELECT count(*) FROM bot_activity_logs WHERE event = 'upgrade_signal_unresolved' AND substr(json_extract(details, '$.marker_id'), 1, 10) = 'rails-job:'"),
        ("signal_placement_resolved marker_id", "SELECT count(*) FROM bot_activity_logs WHERE event = 'signal_placement_resolved' AND substr(json_extract(details, '$.marker_id'), 1, 10) = 'rails-job:'"),
        ("kept: a job's last_error naming Rails", "SELECT count(*) FROM app_configs WHERE substr(key, 1, 9) = 'rust_job.' AND json_extract(value, '$.last_error') LIKE '%Rails%'"),
        ("kept: an activity details.error", "SELECT count(*) FROM bot_activity_logs WHERE json_type(details, '$.error') = 'text'"),
        ("mid-takeover: the signal import journal", "SELECT count(*) FROM app_configs WHERE key = 'rust_signal_import' AND value <> '[]'"),
        ("mid-takeover: the rule import journal", "SELECT count(*) FROM app_configs WHERE key = 'rust_rule_import' AND value <> '[]'"),
    ] { items.push((what.into(), sql.into())); }
    let installs = beta();
    assert_eq!(installs.len(), 14);
    let mut found = vec![0i64; items.len()];
    for (name, dir) in &installs {
        let p = paths(dir.path());
        assert_eq!(store::check(&p).unwrap().pending_rust, [V], "{name}: V is pending");
        let c = Connection::open(&p.primary).unwrap();
        for (n, (_, sql)) in found.iter_mut().zip(&items) { *n += c.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0); }
    }
    let missing: Vec<&str> = items.iter().zip(&found).filter(|(_, n)| **n == 0).map(|((what, _), _)| what.as_str()).collect();
    assert_eq!(missing, Vec::<&str>::new(), "the fixture lacks these");
    let mid = install("mid_takeover");
    let q = Connection::open(paths(mid.path()).queue).unwrap();
    assert!(count(&q, "SELECT count(*) FROM solid_queue_jobs") > 0, "2.x's rows still queued");
}

// ---- #2 upgrade, #3 no old name, #4 restart, #5 fresh bootstrap -----------------------------------------------------

/// #2: every row of every table survives V with its rowid; only the §1.1 names and values change, and the JSON cells
/// V rewrites are equal to the old ones as JSON with only the renamed field or value changed. The retired tables are
/// gone, `check` is clean.
#[test]
fn every_beta_install_upgrades_keeping_every_row_and_id() {
    for (name, dir) in beta() {
        let p = paths(dir.path());
        let before = rows(&Connection::open(&p.primary).unwrap());
        let queue_before = rows(&Connection::open(&p.queue).unwrap());
        assert_eq!(migrate(&p), [V], "{name}");
        let c = Connection::open(&p.primary).unwrap();
        let after = rows(&c);
        let mut expected: Rows = BTreeMap::new();
        let mut json_cells: BTreeMap<(String, i64), usize> = BTreeMap::new();
        for (table, (columns, table_rows)) in &before {
            let now = RENAMED_TABLES.iter().find(|(old, _)| old == table).map_or(table.as_str(), |(_, new)| new);
            expected.entry(now.to_string()).or_insert_with(|| (columns.clone(), vec![]));
            for (rowid, row) in table_rows {
                let (now, row, json_cell) = expected_row(table, columns, row);
                if let Some(i) = json_cell { json_cells.insert((now.clone(), *rowid), i); }
                expected.entry(now).or_insert_with(|| (columns.clone(), vec![])).1.push((*rowid, row));
            }
        }
        // rust_schema_migrations gains V's record; nothing else is added or lost.
        let (_, recorded) = after.get("rust_schema_migrations").unwrap();
        assert_eq!(recorded.len(), expected["rust_schema_migrations"].1.len() + 1, "{name}");
        assert_eq!(after.keys().filter(|t| *t != "rust_schema_migrations").collect::<Vec<_>>(),
                   expected.keys().filter(|t| *t != "rust_schema_migrations").collect::<Vec<_>>(), "{name}: the same tables, renamed");
        for (table, (columns, want)) in expected.iter().filter(|(t, _)| *t != "rust_schema_migrations") {
            let (got_columns, got) = &after[table];
            assert_eq!(got_columns, columns, "{name}: {table}'s columns");
            assert_eq!(got.len(), want.len(), "{name}: {table}'s row count");
            for ((rowid, g), (w_rowid, w)) in got.iter().zip(want) {
                assert_eq!(rowid, w_rowid, "{name}: {table} rowids");
                match json_cells.get(&(table.clone(), *rowid)) {
                    Some(&i) => {
                        assert_eq!(json_of(&g[i]), json_of(&w[i]), "{name}: {table} row {rowid}, {} as JSON", columns[i]);
                        let others = |r: &[Sql]| r.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, v)| v.clone()).collect::<Vec<_>>();
                        assert_eq!(others(g), others(w), "{name}: {table} row {rowid}");
                    }
                    None => assert_eq!(g, w, "{name}: {table} row {rowid}"),
                }
            }
        }
        assert_eq!(rows(&Connection::open(&p.queue).unwrap()), queue_before, "{name}: the queue is untouched");
        for (old, _) in RENAMED_TABLES { assert!(!objects(&c).iter().any(|(_, n, _)| n == old), "{name}: {old} gone"); }
        assert!(!objects(&c).iter().any(|(_, n, _)| n == "index_rust_rails_unrecorded_on_source" || n == "rust_withdrawal_ids_b11_9"));
        assert_eq!(store::check(&p).unwrap().pending_rust, Vec::<String>::new(), "{name}: check is clean");
    }
}

/// Each `rails` (any case) left in a migrated install: (where, the text).
fn old_names(c: &Connection) -> Vec<(String, String)> {
    let mut found = vec![];
    let fk = regex::Regex::new(r"\bfk_rails_[0-9a-f]{10}\b").unwrap();
    for (_, name, sql) in objects(c) {
        let rest = fk.replace_all(&sql, "").to_lowercase();
        if rest.contains("rails") { found.push((format!("sqlite_master {name}"), sql)); }
    }
    for (table, (columns, table_rows)) in rows(c) {
        for (rowid, row) in table_rows {
            for (column, v) in columns.iter().zip(row) {
                let Some(t) = text(&v) else { continue };
                if !t.to_lowercase().contains("rails") { continue }
                // r1.1 (Q5): stored diagnostics stay as written: a job's last_error and an activity line's details.error.
                let kept = match (table.as_str(), column.as_str()) {
                    ("app_configs", "value") => serde_json::from_str::<Value>(t).ok().is_some_and(|mut v| {
                        v.as_object_mut().map(|o| o.remove("last_error"));
                        !v.to_string().to_lowercase().contains("rails")
                    }),
                    ("bot_activity_logs", "details") => serde_json::from_str::<Value>(t).ok().is_some_and(|mut v| {
                        v.as_object_mut().map(|o| o.remove("error"));
                        !v.to_string().to_lowercase().contains("rails")
                    }),
                    _ => false,
                };
                if !kept { found.push((format!("{table}.{column} row {rowid}"), t.chars().take(200).collect())); }
            }
        }
    }
    found
}

/// #3: after V no table, index, key, value or event says rails; only 2.x's fk_rails_<hash> constraint names and the two
/// kept diagnostics (r1.1) do. The fixture has both kept diagnostics (the precondition test), so the exceptions are used.
#[test]
fn no_old_name_is_left_after_the_rename() {
    for (name, dir) in beta() {
        let p = paths(dir.path());
        migrate(&p);
        assert_eq!(old_names(&Connection::open(&p.primary).unwrap()), Vec::<(String, String)>::new(), "{name}");
    }
}

/// #4: a second start applies nothing and changes nothing; the migrated wakes are due once each, and a run of the job
/// drops its entry as before.
#[test]
fn a_restart_applies_nothing_and_the_migrated_wakes_are_due() {
    let dir = install("takeover");
    let p = paths(dir.path());
    migrate(&p);
    let c = Connection::open(&p.primary).unwrap();
    let before = (rows(&c), objects(&c));
    assert_eq!(migrate(&p), Vec::<String>::new());
    assert_eq!((rows(&c), objects(&c)), before, "a restart changes nothing");
    let now = chrono::Utc::now();
    let due = handover::due_v2_wakes(&c, now).unwrap();
    assert!(!due.is_empty(), "the fixture's wakes are due");
    assert_eq!(handover::due_v2_wakes(&c, now).unwrap(), due, "due until run");
    let (job, scope) = due[0].clone();
    state::record_success(&c, &job, scope.as_deref(), now + chrono::Duration::seconds(1)).unwrap();
    let left = handover::due_v2_wakes(&c, now).unwrap();
    assert!(!left.contains(&(job, scope)) && left.len() < due.len(), "{left:?}");
}

/// Each live Rust table's columns (pragma table_xinfo) and indexes (name, unique, columns).
type Shape = BTreeMap<String, (Vec<Vec<Sql>>, Vec<(String, i64, Vec<String>)>)>;
fn shape(c: &Connection) -> Shape {
    REGISTRY.live_tables().into_iter().filter(|t| *t != "rust_schema_migrations").map(|t| {
        let cols = c.prepare("SELECT * FROM pragma_table_xinfo(?1)").unwrap().query_map([t], |r| (0..7).map(|i| r.get(i)).collect()).unwrap()
            .collect::<Result<_, _>>().unwrap();
        let indexes: Vec<(String, i64)> = c.prepare("SELECT name, \"unique\" FROM pragma_index_list(?1) ORDER BY name").unwrap()
            .query_map([t], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
        let indexes = indexes.into_iter().map(|(n, u)| {
            let on: Vec<String> = c.prepare("SELECT name FROM pragma_index_info(?1) ORDER BY seqno").unwrap().query_map([&n], |r| r.get(0)).unwrap()
                .collect::<Result<_, _>>().unwrap();
            (n, u, on)
        }).collect();
        (t.to_string(), (cols, indexes))
    }).collect()
}

/// #5: a new install has the new names from the start, `'v2'` in rust_withdrawal_ids' CHECK, no old name anywhere, and
/// every Rust table shaped as on a migrated beta install.
#[test]
fn a_fresh_install_has_the_new_names_and_the_migrated_shape() {
    let (_dir, o, _) = common::install();
    let fresh = &o.primary;
    for (old, new) in RENAMED_TABLES {
        assert_eq!(count(fresh, &format!("SELECT count(*) FROM sqlite_master WHERE name = '{new}'")), 1);
        assert_eq!(count(fresh, &format!("SELECT count(*) FROM sqlite_master WHERE name = '{old}'")), 0);
    }
    let check: String = fresh.query_row("SELECT sql FROM sqlite_master WHERE name = 'rust_withdrawal_ids'", [], |r| r.get(0)).unwrap();
    assert!(check.contains("CHECK(source IN ('rust','v2','operator'))"), "{check}");
    assert_eq!(old_names(fresh), Vec::<(String, String)>::new());
    let dir = install("takeover");
    let p = paths(dir.path());
    migrate(&p);
    assert_eq!(shape(&Connection::open(&p.primary).unwrap()), shape(fresh));
}

// ---- #6 mid-takeover (money) -------------------------------------------------------------------------------------------

/// #6, r1.4: a beta install stopped between the claim's commit and its finishing steps upgrades, and the next start
/// finishes the takeover with nothing done twice: 2.x's rows go, the claimed webhook call is the bot's one `v2:` call,
/// the rule import adds no evaluation, a withdrawal id stays one row; replayed with the journals cleared, the imports
/// meet the migrated values and add nothing; once the owner resolves the marker, one order is placed for the call.
#[tokio::test(flavor = "current_thread")]
async fn a_takeover_stopped_mid_way_upgrades_and_finishes_once() {
    use deltabadger::venue::fake::{FakeFactory, FakeVenue};
    let dir = install("mid_takeover");
    let p = paths(dir.path());
    let c0 = Connection::open(&p.primary).unwrap();
    let job: String = c0.query_row("SELECT substr(json_extract(e.value, '$.source'), 7) FROM bots, json_each(bots.transient_data, '$.rust_signal_calls') e WHERE bots.id = 1",
                                   [], |r| r.get(0)).unwrap();
    let evaluations = count(&c0, "SELECT count(*) FROM rust_rule_evaluations");
    let lines_before: BTreeMap<String, i64> = c0.prepare("SELECT event, count(*) FROM bot_activity_logs GROUP BY event").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    drop(c0);
    let l = lock(&p);
    assert_eq!(store::migrate(&p, &l).unwrap(), [V]);
    let o = store::open(&p).unwrap();
    let calls = |o: &store::Opened| model::load_bot(&o.primary, 1).unwrap().signal_calls().unwrap();
    assert_eq!(calls(&o).iter().map(|c| c.source.clone()).collect::<Vec<_>>(), [format!("v2:{job}")]);
    let t = handover::take_over(&l, &o, &common::seed::cipher(), deltabadger::VERSION, chrono::Utc::now()).unwrap();
    assert_eq!(t.imported_calls, 0, "nothing imported again");
    assert_eq!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs WHERE finished_at IS NULL"), 0, "2.x's rows are gone");
    assert_eq!(calls(&o).iter().map(|c| c.source.clone()).collect::<Vec<_>>(), [format!("v2:{job}")], "one call, v2:J");
    assert_eq!(count(&o.primary, "SELECT count(*) FROM rust_rule_evaluations"), evaluations, "no evaluation added");
    assert_eq!(count(&o.primary, "SELECT count(*) FROM rust_rule_evaluations WHERE substr(source, 1, 3) = 'v2:'"), evaluations);
    assert_eq!(count(&o.primary, "SELECT count(*) FROM (SELECT 1 FROM rust_withdrawal_ids GROUP BY exchange_id, venue_id HAVING count(*) > 1)"), 0);
    let lines_after: BTreeMap<String, i64> = o.primary.prepare("SELECT event, count(*) FROM bot_activity_logs GROUP BY event").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    for (event, n) in &lines_before {
        let now = event.strip_prefix("rails_order_").map_or(event.clone(), |k| format!("v2_order_{k}"));
        assert_eq!(lines_after.get(&now), Some(n), "{event}: nothing logged twice");
    }
    // r1.4 (a)/(b): the imports replayed with their journals cleared meet the migrated v2: values.
    let q_jobs: Vec<(i64, String)> = Connection::open(&p.queue).unwrap().prepare("SELECT id, active_job_id FROM solid_queue_jobs").unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().collect::<Result<_, _>>().unwrap();
    assert!(q_jobs.iter().all(|(_, id)| id != &job), "the call's row is gone");
    // r1.4 (c): the migrated acks keep the acknowledged attempt and fan-out out of takeover's open work.
    assert!(count(&o.primary, "SELECT count(*) FROM rust_v2_attempt_acks") > 0 || count(&o.primary, "SELECT count(*) FROM rust_v2_job_acks") > 0);
    for (attempt, _) in deltabadger::engine::rules::open_attempts(&o.primary, None).unwrap() {
        assert_eq!(count(&o.primary, &format!("SELECT count(*) FROM rust_v2_attempt_acks WHERE rule_log_id = {attempt}")), 0, "an acknowledged attempt is never open");
    }
    // r1.4 (d): a withdrawal id the beta seeded from 2.x stays one row, now 'v2', and seeding again adds none.
    let ids = count(&o.primary, "SELECT count(*) FROM rust_withdrawal_ids");
    assert!(count(&o.primary, "SELECT count(*) FROM rust_withdrawal_ids WHERE source = 'v2'") > 0);
    deltabadger::engine::rules::seed_v2_ids(&o.primary).unwrap();
    assert_eq!(count(&o.primary, "SELECT count(*) FROM rust_withdrawal_ids"), ids);
    // The claimed job's marker blocks the bot until its owner resolves it (#8). The frozen 2.x install had no Kraken key,
    // so the beta's takeover set the bot aside: the owner adds the key and starts it. Inside the call's window the one
    // migrated call places one order, and later passes place none.
    assert!(deltabadger::engine::signal::blocked(&model::load_bot(&o.primary, 1).unwrap()));
    let cipher = common::seed::cipher();
    let (user, exchange): (i64, i64) = o.primary.query_row("SELECT user_id, exchange_id FROM bots WHERE id = 1", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    o.primary.execute("INSERT INTO api_keys (user_id, exchange_id, key, secret, status, key_type, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, 1, 0, '2026-09-30 11:00:00', '2026-09-30 11:00:00')",
                      rusqlite::params![user, exchange, cipher.encrypt("test-key"), cipher.encrypt("dGVzdC1zZWNyZXQ=")]).unwrap();
    o.primary.execute("UPDATE bots SET status = 1, transient_data = json_remove(transient_data, '$.signal_placement_unresolved', '$.rust_set_aside', '$.rust_set_aside_mail_pending') WHERE id = 1", []).unwrap();
    let triggered = calls(&o)[0].triggered_at;
    let venue = FakeVenue::new().ticker("XXBTZEUR", "49990.1", "50000.2", "49995.3").balance_body("ZEUR", "10000", "0");
    let at = deltabadger::engine::FixedClock(triggered + chrono::Duration::seconds(59));
    let mut e = deltabadger::engine::run::Engine::new(Connection::open(&p.primary).unwrap(), FakeFactory(venue.clone()), cipher, l.clone());
    for _ in 0..3 { deltabadger::engine::run::step(&mut e, &at).await.unwrap(); }
    assert_eq!(venue.sent().len(), 1, "one order for the one migrated call: {:?}", venue.sent());
    assert!(calls(&o).is_empty(), "the call is consumed");
    for _ in 0..2 { deltabadger::engine::run::step(&mut e, &at).await.unwrap(); }
    assert_eq!(venue.sent().len(), 1, "never a second order");
}

/// #6, r1.4 (a)/(b): the imports replayed on the migrated mid-takeover install with their journals cleared and 2.x's
/// rows still queued meet the migrated values: the source comparison finds `v2:J` (no second call, so no second
/// order) and `INSERT OR IGNORE` meets the `v2:` evaluation (no second withdrawal).
#[test]
fn the_imports_replayed_without_journals_meet_the_migrated_values() {
    let dir = install("mid_takeover");
    let p = paths(dir.path());
    let l = lock(&p);
    store::migrate(&p, &l).unwrap();
    let o = store::open(&p).unwrap();
    let evaluations = count(&o.primary, "SELECT count(*) FROM rust_rule_evaluations");
    let call = model::load_bot(&o.primary, 1).unwrap().signal_calls().unwrap();
    assert_eq!(call.len(), 1);
    assert!(call[0].source.starts_with("v2:"), "{:?}", call[0].source);
    o.primary.execute("DELETE FROM app_configs WHERE key IN ('rust_signal_import', 'rust_rule_import')", []).unwrap();
    assert!(count(&o.queue, "SELECT count(*) FROM solid_queue_jobs WHERE finished_at IS NULL") > 0, "2.x's rows still queued");
    let t = handover::take_over(&l, &o, &common::seed::cipher(), deltabadger::VERSION, chrono::Utc::now()).unwrap();
    assert_eq!(t.imported_calls, 0, "the migrated call is found by its source");
    assert_eq!(model::load_bot(&o.primary, 1).unwrap().signal_calls().unwrap(), call, "one call, as it was");
    assert_eq!(count(&o.primary, "SELECT count(*) FROM rust_rule_evaluations"), evaluations, "no evaluation added");
}

// ---- #8 marker, #9 freshness -------------------------------------------------------------------------------------------

/// The owner signs in with a known password; the app over `dir`.
fn signed_in(c: &Connection) {
    let hash = deltabadger::crypto::hash_password("Correct-horse-9").unwrap();
    c.execute("UPDATE users SET encrypted_password = ?1, confirmed_at = '2026-01-01 00:00:00' WHERE email = 'o@example.com'", [hash]).unwrap();
}

/// #8: a migrated `v2-job:` marker blocks placing and its page renders it; a confirm carrying the old `rails-job:` id (a
/// page drawn before the upgrade) is stale and changes nothing; the rendered id resolves it.
#[tokio::test(flavor = "current_thread")]
async fn a_migrated_marker_blocks_and_only_its_new_id_resolves_it() {
    let dir = install("takeover");
    let p = paths(dir.path());
    migrate(&p);
    let c = Connection::open(&p.primary).unwrap();
    let (bot, id): (i64, String) = c.query_row("SELECT id, json_extract(transient_data, '$.signal_placement_unresolved.id') FROM bots \
                                                WHERE json_extract(transient_data, '$.signal_placement_unresolved.id') IS NOT NULL", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    let job = id.strip_prefix("v2-job:").expect("renamed");
    assert!(deltabadger::engine::signal::blocked(&model::load_bot(&c, bot).unwrap()));
    signed_in(&c);
    let app = web::app(dir.path(), web::SECRET, TestClock::at("2026-11-17T12:00:00Z"));
    let mut browser = Browser::default();
    browser.get(&app, "/login").await;
    assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303);
    let page = browser.get(&app, &format!("/bots/{bot}")).await;
    assert_eq!(page.status, 200, "{:.500}", page.body);
    assert!(page.body.contains(&format!("name=\"marker_id\" value=\"v2-job:{job}\"")), "{:.3000}", page.body);
    let state = || c.query_row("SELECT transient_data, (SELECT count(*) FROM bot_activity_logs WHERE bot_id = ?1) FROM bots WHERE id = ?1", [bot],
                               |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))).unwrap();
    let before = state();
    let resolve = format!("/bots/{bot}/signal_resolution");
    let old = format!("rails-job:{job}");
    assert_eq!(browser.send(&app, "POST", &resolve, Some(&[("marker_id", old.as_str())]), web::Csrf::Header, &[]).await.status, 302);
    assert_eq!(state(), before, "the old id is stale: nothing changes");
    assert_eq!(browser.send(&app, "POST", &resolve, Some(&[("marker_id", id.as_str())]), web::Csrf::Header, &[]).await.status, 302);
    let (left, lines) = state();
    assert!(serde_json::from_str::<Value>(&left).unwrap().get("signal_placement_unresolved").is_none(), "{left}");
    assert_eq!(lines, before.1 + 1, "lifted with its line");
    let marker: String = c.query_row("SELECT json_extract(details, '$.marker_id') FROM bot_activity_logs WHERE bot_id = ?1 AND event = 'signal_placement_resolved' ORDER BY id DESC LIMIT 1",
                                     [bot], |r| r.get(0)).unwrap();
    assert_eq!(marker, id);
}

/// #9: a job fresh only through 2.x's baseline is as fresh after V (v2_at holds the same time), so no bot is stopped by
/// the upgrade; the A28 report job keeps its state under its new name.
#[test]
fn a_jobs_baseline_and_state_survive_the_rename() {
    let dir = install("takeover");
    let p = paths(dir.path());
    let c = Connection::open(&p.primary).unwrap();
    let key = state::key(reference::ALPACA_CRYPTO, None);
    let old: Value = serde_json::from_str(&c.query_row("SELECT value FROM app_configs WHERE key = ?1", [&key], |r| r.get::<_, String>(0)).unwrap()).unwrap();
    let baseline = old["rails_at"].as_str().expect("a beta baseline").to_string();
    assert!(old.get("last_success_at").is_none_or(Value::is_null), "fresh only through 2.x's baseline");
    let report_state: String = c.query_row("SELECT value FROM app_configs WHERE key = 'rust_job.rails_unrecorded_report'", [], |r| r.get(0)).unwrap();
    drop(c);
    migrate(&p);
    let c = Connection::open(&p.primary).unwrap();
    let st = state::read(&c, reference::ALPACA_CRYPTO, None).unwrap();
    assert_eq!(st.v2_at, Some(baseline.parse().unwrap()));
    let at: chrono::DateTime<chrono::Utc> = baseline.parse::<chrono::DateTime<chrono::Utc>>().unwrap() + chrono::Duration::hours(1);
    // 2.x's catalogue rows vouch for the source too; aged, the baseline is the only fresh evidence.
    c.execute("UPDATE exchange_assets SET updated_at = '2020-01-01 00:00:00'", []).unwrap();
    let fresh = staleness::report(&c, at).unwrap();
    assert!(fresh.iter().any(|l| l.contains(&deltabadger::codec::format_time(baseline.parse().unwrap())) && !l.contains("STALE")), "{fresh:?}");
    c.execute("UPDATE app_configs SET value = json_remove(value, '$.v2_at') WHERE key = ?1", [&key]).unwrap();
    assert_ne!(staleness::report(&c, at).unwrap(), fresh, "the freshness came through v2_at");
    let moved: String = c.query_row("SELECT value FROM app_configs WHERE key = 'rust_job.v2_unrecorded_report'", [], |r| r.get(0)).unwrap();
    let mut want: Value = serde_json::from_str(&report_state).unwrap();
    let was = want.as_object_mut().unwrap().remove("rails_at").unwrap();
    want["v2_at"] = was;
    assert_eq!(serde_json::from_str::<Value>(&moved).unwrap(), want, "the report job's state, as it was");
    assert_eq!(state::read(&c, deltabadger::engine::v2_unrecorded::JOB, None).unwrap().last_run_at,
               serde_json::from_str::<Value>(&report_state).unwrap()["last_run_at"].as_str().map(|t| t.parse().unwrap()));
}

// ---- #7 (page), #10 export, #11 handback --------------------------------------------------------------------------------

/// #7, the page: every migrated A28 line renders with its renamed text (no missing translation), and an undecided
/// group shows its answers.
#[tokio::test(flavor = "current_thread")]
async fn the_bot_page_renders_every_migrated_line() {
    let mut answers = 0;
    for (name, dir) in beta().into_iter().filter(|(n, _)| n.starts_with("a28_")) {
        let p = paths(dir.path());
        migrate(&p);
        let c = Connection::open(&p.primary).unwrap();
        let bots: Vec<i64> = c.prepare("SELECT DISTINCT bot_id FROM bot_activity_logs WHERE event LIKE 'v2_order_%'").unwrap()
            .query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert!(!bots.is_empty(), "{name}");
        // a28_time_none_seen's input line has a time on purpose no page can draw (2.x's "25:00"); not a renamed row.
        c.execute("DELETE FROM bot_activity_logs WHERE event = 'placement_ambiguous' AND created_at = '2026-09-29 25:00:00'", []).unwrap();
        signed_in(&c);
        let app = web::app(dir.path(), web::SECRET, TestClock::at("2026-11-17T12:00:00Z"));
        let mut browser = Browser::default();
        browser.get(&app, "/login").await;
        assert_eq!(browser.post(&app, "/login", &[("user[email]", "o@example.com"), ("user[password]", "Correct-horse-9")]).await.status, 303, "{name}");
        for bot in bots {
            let page = browser.get(&app, &format!("/bots/{bot}")).await;
            assert_eq!(page.status, 200, "{name}: {:.500}", page.body);
            // The lines are the orders frame's.
            let feed = browser.send(&app, "GET", &format!("/bots/{bot}.turbo_stream"), None, web::Csrf::None, &[("Turbo-Frame", "orders_pagination")]).await;
            assert_eq!(feed.status, 200, "{name}: {:.500}", feed.body);
            for body in [&page.body, &feed.body] {
                assert!(!body.to_lowercase().contains("translation missing"), "{name}: {:.3000}", body);
                assert!(!body.contains("rails"), "{name}");
            }
            assert!(feed.body.contains("bot_activity_log_"), "{name}: the lines are drawn");
            answers += feed.body.matches(&format!("action=\"/bots/{bot}/v2_orders\"")).count();
        }
    }
    assert!(answers > 0, "an undecided group's answers are shown");
}

/// #10: the export of a migrated beta copy holds the new names only, keeps the renamed keys, is at schema V, and passes
/// `check`; the build before this one refuses it (Unsupported), as after every migration.
#[cfg(unix)]
#[test]
fn the_export_of_a_beta_copy_has_the_new_names() {
    let root = tempfile::tempdir().unwrap();
    let root_path = std::fs::canonicalize(root.path()).unwrap();
    std::fs::create_dir(root_path.join("storage")).unwrap();
    std::fs::write(root_path.join(".dockerenv"), "").unwrap();
    let s = deltabadger::export::Scratch { storage: root_path.join("storage"), output: root_path.join("export"), marker: root_path.join(".dockerenv") };
    let beta = install("takeover");
    for f in common::INSTALL_FILES { std::fs::copy(beta.path().join(f), s.storage.join(f)).unwrap(); }
    let env = |k: &str| (k == "EXPORT_SCRATCH").then(|| "1".to_string());
    deltabadger::export::run(&s, &env, chrono::Utc::now()).unwrap_or_else(|e| panic!("{}", e.message()));
    let out = s.output.join("portable");
    let c = Connection::open(out.join("production.sqlite3")).unwrap();
    assert_eq!(old_names(&c), Vec::<(String, String)>::new());
    for (_, new) in RENAMED_TABLES { assert_eq!(count(&c, &format!("SELECT count(*) FROM sqlite_master WHERE name = '{new}'")), 1, "{new}"); }
    // export's APP_CONFIGS allowlist keeps the renamed keys the copy has.
    let source = Connection::open(beta.path().join(common::INSTALL_FILES[0])).unwrap();
    for (old, new) in &RENAMED_KEYS[..3] {
        let had = count(&source, &format!("SELECT count(*) FROM app_configs WHERE key = '{old}'"));
        assert_eq!(count(&c, &format!("SELECT count(*) FROM app_configs WHERE key = '{new}'")), had, "{new}");
    }
    assert!(count(&c, "SELECT count(*) FROM app_configs WHERE key = 'rust_v2_wakes'") == 1, "the takeover install has its wakes");
    assert_eq!(c.query_row("SELECT max(version) FROM rust_schema_migrations", [], |r| r.get::<_, String>(0)).unwrap(), V);
    drop(c);
    let restored = Paths { primary: out.join("production.sqlite3"), queue: out.join("production_queue.sqlite3") };
    assert_eq!(store::check(&restored).unwrap().pending_rust, Vec::<String>::new());
    assert!(matches!(store::check_with(&restored, &older()), Err(StoreError::Unsupported { ref unknown }) if unknown == &vec![V.to_string()]));
}

fn bare(dir: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_deltabadger"));
    c.args(args).env("STORAGE_DIR", dir).env("SECRET_KEY_BASE", "engine-test-secret").env("IBKR_CATALOGUE_URL", "off")
        .env_remove("DATABASE_URL").env_remove("ACTIVE_RECORD_ENCRYPTION_PRIMARY_KEY").env_remove("ACTIVE_RECORD_ENCRYPTION_KEY_DERIVATION_SALT");
    c
}
fn said(out: &std::process::Output) -> String { format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)) }

/// #11: a handback of a migrated install ends as before: the renamed Rust-only rows are Ignored (left in place) and
/// never the reason for a refusal; an install that hands back is reclaimed by the next start, which reads the new names.
/// (The `takeover` installs still owe 2.x's queued withdrawal rule, so they refuse, as they did before the rename.)
#[test]
fn a_handback_and_reclaim_work_on_migrated_rows() {
    let mut handed = 0;
    for (name, dir) in beta() {
        let p = paths(dir.path());
        migrate(&p);
        let c = Connection::open(&p.primary).unwrap();
        let kept: Vec<(&str, i64)> = RENAMED_TABLES.iter().map(|(_, t)| (*t, count(&c, &format!("SELECT count(*) FROM {t}")))).collect();
        drop(c);
        let out = bare(dir.path(), &["handback"]).output().unwrap();
        if out.status.code() != Some(0) {
            assert!(said(&out).contains("handback refused") && !said(&out).contains("rust_v2_"), "{name}: {}", said(&out));
            continue;
        }
        handed += 1;
        let c = Connection::open(&p.primary).unwrap();
        for (t, n) in &kept { assert_eq!(count(&c, &format!("SELECT count(*) FROM {t}")), *n, "{name}: {t} is Ignored, left as it was"); }
        assert_eq!(old_names(&c), Vec::<(String, String)>::new(), "{name}");
        drop(c);
        let out = bare(dir.path(), &["run"]).env("DELTABADGER_TEST_ABORT", "after-claim").output().unwrap();
        assert!(said(&out).contains("took over"), "{name}: {}", said(&out));
        assert_eq!(old_names(&Connection::open(&p.primary).unwrap()), Vec::<(String, String)>::new(), "{name}");
    }
    assert!(handed > 0, "some frozen install hands back");
}

// ---- #12 rollback, #13 pending replacement, #14 downgrade -------------------------------------------------------------

fn everything(p: &Paths) -> (Rows, Vec<(String, String, String)>) {
    let c = Connection::open(&p.primary).unwrap();
    (rows(&c), objects(&c))
}

/// #12: V failing on purpose (the debug hook, at start) or on its last statement leaves every old table, key and value
/// as it was and records nothing; the next start applies it.
#[test]
fn a_failed_rename_leaves_the_beta_install_as_it_was() {
    let dir = install("takeover");
    let p = paths(dir.path());
    let before = everything(&p);
    let out = bare(dir.path(), &["run"]).env("DELTABADGER_TEST_FAIL_MIGRATION", V).output().unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(said(&out).contains("failed on purpose"), "{}", said(&out));
    assert_eq!(everything(&p), before, "the hook's failure: nothing of V");
    let sql = format!("{}\nINSERT INTO nowhere VALUES (1);", RUST_MIGRATIONS.iter().find(|m| m.version == V).unwrap().sql);
    let migrations: Vec<Migration> = RUST_MIGRATIONS.iter().map(|m| if m.version == V { Migration { version: m.version, name: m.name, sql: &sql } } else { *m }).collect();
    let broken = Registry { migrations: &migrations, tables: REGISTRY.tables, retired: REGISTRY.retired };
    match store::migrate_with(&p, &lock(&p), &broken) {
        Err(StoreError::Migration { version, error, .. }) => assert!(version == V && error.contains("nowhere"), "{version}: {error}"),
        other => panic!("expected V's failure, got {other:?}"),
    }
    assert_eq!(everything(&p), before, "a failure on the last statement: nothing of V");
    assert_eq!(migrate(&p), [V], "the next start applies it");
}

/// #12's timing: V on an install with 1,000,000 activity rows (10,000 of them A28 lines) stays well inside the start
/// (the desktop says "still starting" after 60 s). Run by the nightly timing job in release; prints the number.
#[test]
#[ignore = "timing: nightly"]
fn the_rename_of_a_million_activity_rows_is_quick() {
    let dir = install("takeover");
    let p = paths(dir.path());
    let c = Connection::open(&p.primary).unwrap();
    c.execute_batch("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000000) \
                     INSERT INTO bot_activity_logs (bot_id, event, level, details, created_at) \
                     SELECT 1, CASE WHEN i % 100 = 0 THEN 'rails_order_none_seen' WHEN i % 100 = 1 THEN 'upgrade_signal_unresolved' ELSE 'order_placed' END, 0, \
                            CASE WHEN i % 100 = 1 THEN '{\"marker_id\":\"rails-job:' || i || '\"}' ELSE '{\"order\":' || i || '}' END, '2026-11-01 00:00:00' FROM n").unwrap();
    drop(c);
    let started = std::time::Instant::now();
    assert_eq!(migrate(&p), [V]);
    let took = started.elapsed();
    eprintln!("B11-9 V on 1,000,000 activity rows: {:.2} s", took.as_secs_f64());
    let c = Connection::open(&p.primary).unwrap();
    assert!(count(&c, "SELECT count(*) FROM bot_activity_logs WHERE event = 'v2_order_none_seen'") >= 10000);
    assert_eq!(count(&c, "SELECT count(*) FROM bot_activity_logs WHERE substr(json_extract(details, '$.marker_id'), 1, 10) = 'rails-job:'"), 0);
    assert!(took < std::time::Duration::from_secs(30), "{took:?}");
}

/// #13 (r3.3): with V pending on a beta install, every command that does not migrate refuses before writing (it would
/// read the new names over the old rows); `check` reports V pending; `resolve-withdrawal` migrates, then acts. Rotation
/// migrates a copy for its owed check (r3.2), so an install that owes work is refused there first, also writing nothing.
#[test]
fn commands_refuse_a_beta_install_until_the_rename_ran() {
    let mut rotation_pending = 0;
    for (name, _) in beta() {
        for args in [&["handback"][..], &["sync", "balances"], &["resolve-placement", "1", "--not-placed"], &["encryption-reset", "--confirm", "clear-credentials"]] {
            let dir = install(&name);
            let p = paths(dir.path());
            let before = everything(&p);
            let out = bare(dir.path(), args).output().unwrap();
            assert_ne!(out.status.code(), Some(0), "{name} {args:?}: {}", said(&out));
            let pending = said(&out).contains("this data needs this build's migrations first");
            if args[0] == "encryption-reset" { rotation_pending += pending as usize; } else { assert!(pending, "{name} {args:?}: {}", said(&out)); }
            assert_eq!(everything(&p), before, "{name} {args:?} wrote nothing");
        }
    }
    assert!(rotation_pending > 0, "rotation meets the pending rename on an install that owes nothing");
    let dir = install("takeover");
    let out = bare(dir.path(), &["check"]).output().unwrap();
    assert!(said(&out).contains(&format!("pending Rust migration {V}")), "{}", said(&out));
    let rule: i64 = Connection::open(paths(dir.path()).primary).unwrap().query_row("SELECT id FROM rules ORDER BY id LIMIT 1", [], |r| r.get(0)).unwrap();
    let out = bare(dir.path(), &["resolve-withdrawal", "--v2-list", &rule.to_string()]).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", said(&out));
    assert_eq!(store::check(&paths(dir.path())).unwrap().pending_rust, Vec::<String>::new(), "it migrated first");
}

/// #7, rotation: an obligation whose page still asks for an answer is owed work on the migrated tables, so rotation
/// refuses; with V pending its owed check migrates a copy first and refuses the same.
#[test]
fn rotation_refuses_while_a_migrated_answer_is_owed() {
    let line = "its page asks whether an order your previous version may have placed is yours.";
    for migrated in [true, false] {
        let dir = install("a28_live_open");
        let p = paths(dir.path());
        if migrated { migrate(&p); }
        let before = everything(&p);
        let out = bare(dir.path(), &["encryption-reset", "--confirm", "clear-credentials"]).output().unwrap();
        assert_ne!(out.status.code(), Some(0));
        assert!(said(&out).contains(line), "migrated {migrated}: {}", said(&out));
        assert_eq!(everything(&p), before, "nothing cleared");
    }
}

/// #14: the build before this one refuses a V-migrated install as Unsupported before any write.
#[test]
fn the_previous_build_refuses_a_renamed_install() {
    let dir = install("takeover");
    let p = paths(dir.path());
    assert_eq!(store::check_with(&p, &older()).unwrap().pending_rust, Vec::<String>::new(), "the previous build ran this install");
    migrate(&p);
    let before = everything(&p);
    assert!(matches!(store::check_with(&p, &older()), Err(StoreError::Unsupported { ref unknown }) if unknown == &vec![V.to_string()]));
    assert!(matches!(store::open_with(&p, &older()), Err(StoreError::Unsupported { .. })));
    assert_eq!(everything(&p), before);
}

/// r2.1: V's signal-call rewrite on the bundled SQLite: a mixed list keeps its order, length and every other field's JSON
/// type; a bot with only hook calls, one without the key and one whose list holds a non-object are byte-equal.
#[test]
fn the_signal_call_rewrite_keeps_order_and_types() {
    let (_dir, o, _) = common::install();
    let c = &o.primary;
    let version: String = c.query_row("SELECT sqlite_version()", [], |r| r.get(0)).unwrap();
    assert!(version.starts_with("3.5"), "the bundled SQLite: {version}");
    let sql = RUST_MIGRATIONS.iter().find(|m| m.version == V).unwrap().sql;
    let step6 = &sql[sql.find("UPDATE bots SET transient_data = json_set(transient_data, '$.rust_signal_calls'").unwrap()..];
    let step6 = &step6[..step6.find(";\n").unwrap() + 1];
    c.execute_batch("CREATE TEMP TABLE bots_probe (id INTEGER PRIMARY KEY, transient_data TEXT)").unwrap();
    let cases = [
        r#"{"rust_signal_calls":[{"source":"hook:a","signal_id":3,"triggered_at":"2026-09-30T11:58:01.123456Z"},{"source":"rails:J1","signal_id":4,"triggered_at":"2026-09-30T11:59:00Z","n":1.5},{"source":"rails:J2","signal_id":5,"triggered_at":"2026-09-30T12:00:00Z"}],"other":[1,2]}"#,
        r#"{"rust_signal_calls":[{"source":"hook:a","signal_id":3,"triggered_at":"2026-09-30T11:58:01Z"}]}"#,
        r#"{"x":1}"#,
        r#"{"rust_signal_calls":[{"source":"rails:J1","signal_id":4},7]}"#,
    ];
    for (i, t) in cases.iter().enumerate() { c.execute("INSERT INTO bots_probe (id, transient_data) VALUES (?1, ?2)", rusqlite::params![i as i64, t]).unwrap(); }
    c.execute_batch(&step6.replace("UPDATE bots SET", "UPDATE bots_probe SET").replace("bots.transient_data", "bots_probe.transient_data")).unwrap();
    let got: Vec<String> = c.prepare("SELECT transient_data FROM bots_probe ORDER BY id").unwrap().query_map([], |r| r.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
    let mixed: Value = serde_json::from_str(&got[0]).unwrap();
    assert_eq!(mixed, expected_transient(serde_json::from_str(cases[0]).unwrap()));
    assert_eq!(mixed["rust_signal_calls"].as_array().unwrap().iter().map(|c| c["source"].as_str().unwrap()).collect::<Vec<_>>(), ["hook:a", "v2:J1", "v2:J2"]);
    assert!(mixed["rust_signal_calls"][1]["signal_id"].is_i64() && mixed["rust_signal_calls"][1]["n"].is_f64());
    assert_eq!(&got[1..], &cases[1..].iter().map(|s| s.to_string()).collect::<Vec<_>>()[..], "untouched rows are byte-equal");
}

/// Codex branch r1: step 5 moves `rails_at` to `v2_at` as the same JSON value, whatever its type; only `rust_job.*`
/// objects that have it change.
#[test]
fn the_baseline_rename_keeps_the_json_value() {
    let (_dir, o, _) = common::install();
    let c = &o.primary;
    let sql = RUST_MIGRATIONS.iter().find(|m| m.version == V).unwrap().sql;
    let step5 = &sql[sql.find("UPDATE app_configs SET value = json_set(json_remove(value, '$.rails_at')").unwrap()..];
    let step5 = &step5[..step5.find(";\n").unwrap() + 1];
    let cases = [
        ("rust_job.a", r#"{"last_run_at":"2026-09-30T00:00:00Z","rails_at":"2026-09-29T10:15:00Z"}"#, Some(r#"{"last_run_at":"2026-09-30T00:00:00Z","v2_at":"2026-09-29T10:15:00Z"}"#)),
        ("rust_job.b", r#"{"rails_at":null}"#, Some(r#"{"v2_at":null}"#)),
        ("rust_job.c", r#"{"rails_at":true}"#, Some(r#"{"v2_at":true}"#)),
        ("rust_job.d", r#"{"rails_at":1.5}"#, Some(r#"{"v2_at":1.5}"#)),
        ("rust_job.e", r#"{"rails_at":{"x":[1,false]}}"#, Some(r#"{"v2_at":{"x":[1,false]}}"#)),
        ("rust_job.f", r#"{"last_run_at":null}"#, None),
        ("rust_job.g", r#""a bare string""#, None),
        ("other.key", r#"{"rails_at":"2026-09-29T10:15:00Z"}"#, None),
    ];
    for (k, v, _) in cases { c.execute("INSERT INTO app_configs (key, value, created_at, updated_at) VALUES (?1, ?2, 't', 't')", [k, v]).unwrap(); }
    c.execute_batch(step5).unwrap();
    for (k, v, want) in cases {
        let got: String = c.query_row("SELECT value FROM app_configs WHERE key = ?1", [k], |r| r.get(0)).unwrap();
        match want {
            Some(w) => assert_eq!(serde_json::from_str::<Value>(&got).unwrap(), serde_json::from_str::<Value>(w).unwrap(), "{k}"),
            None => assert_eq!(got, v, "{k} untouched"),
        }
    }
}
