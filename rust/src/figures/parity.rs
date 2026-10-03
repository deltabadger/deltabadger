//! The Rust half of the figures-parity harness (script/rust/figures.rb is the Rails half): every figure of one
//! scenario, computed from its database copy and its scripted market, in the shape Rails reports them.
use super::at::At;
use super::db::Subject;
use super::walk::{self, Metrics};
use super::{Absent, FiguresError};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Map, Value};
use std::path::Path;

/// A figure, what Rails raises in its place (`"<class>: <message>"`), or why this library does not compute it.
type Answer<T> = Result<T, Absent>;

/// Any other error is this build's own and ends the run.
fn answer<T>(result: Result<T, FiguresError>) -> Result<Answer<T>, FiguresError> {
    match result {
        Ok(value) => Ok(Ok(value)),
        Err(FiguresError::Raised(error)) => Ok(Err(Absent::Raised(error))),
        Err(FiguresError::NotComputed(reason)) => Ok(Err(Absent::NotComputed(reason))),
        Err(other) => Err(other),
    }
}

/// The answer as JSON text, as the Rails harness writes it. A figure that is not computed is an object and no
/// text, so it can never be taken for one of Rails'.
fn text<T>(answer: &Answer<T>, write: impl Fn(&T) -> String) -> Value {
    match answer {
        Ok(value) => json!(write(value)),
        Err(Absent::Raised(error)) => json!(json!({ "raised": error }).to_string()),
        Err(Absent::NotComputed(reason)) => json!({ "not_computed": reason }),
    }
}

/// The scenario in `dir` (scenario.json beside production.sqlite3): Rails' rails.json, computed here.
pub fn figures(dir: &Path) -> Result<Value, FiguresError> {
    let unreadable = |e: &dyn std::fmt::Display| FiguresError::Data(e.to_string());
    let scenario = std::fs::read_to_string(dir.join("scenario.json")).map_err(|e| unreadable(&e))?;
    let scenario: Value = serde_json::from_str(&scenario).map_err(|e| unreadable(&e))?;
    if scenario["parity_scratch"] != true { return Err(FiguresError::Data("not a parity_scratch scenario".into())); }
    // Read-only at the connection: no statement of this library can change a row.
    let c = Connection::open_with_flags(dir.join("production.sqlite3"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let now = scenario["at"].as_str().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).and_then(|at| At::from_utc(at.to_utc()))
        .ok_or_else(|| FiguresError::Data("scenario.json has no `at`".into()))?;
    let written = |m: &Metrics| m.to_json(9).write();

    let mut bots = Map::new();
    for id in scenario["bot_ids"].as_array().into_iter().flatten().filter_map(Value::as_i64) {
        let subject = match Subject::load(&c, id) {
            Ok(subject) => subject,
            Err(FiguresError::NotComputed(reason)) => { bots.insert(id.to_string(), json!({ "not_computed": reason })); continue; }
            Err(other) => return Err(other),
        };
        let metrics = answer(walk::metrics(&c, &subject, now))?;
        let mut out = Map::new();
        out.insert("metrics".into(), text(&metrics, written));
        bots.insert(id.to_string(), Value::Object(out));
    }

    let mut out = Map::new();
    out.insert("bots".into(), Value::Object(bots));
    Ok(Value::Object(out))
}
