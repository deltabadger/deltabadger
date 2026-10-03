//! The Rust half of the figures-parity harness (script/rust/figures.rb is the Rails half): every figure of one
//! scenario, computed from its database copy and its scripted market, in the shape Rails reports them.
use super::at::At;
use super::chart;
use super::db::{self, Subject};
use super::live;
use super::scripted::Scripted;
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

fn after<T, U>(earlier: &Answer<T>, next: impl FnOnce(&T) -> Result<U, FiguresError>) -> Result<Answer<U>, FiguresError> {
    match earlier { Ok(value) => answer(next(value)), Err(error) => Ok(Err(error.clone())) }
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
    let market = Scripted::new(&scenario["script"], scenario["provider"].as_str());
    let user = db::user(&c, scenario["user_id"].as_i64().unwrap_or(0))?;
    let zone = crate::web::timezone::zone(&user.time_zone).unwrap_or(chrono_tz::Tz::UTC);
    let written = |m: &Metrics| m.to_json(9).write();

    let mut bots = Map::new();
    for id in scenario["bot_ids"].as_array().into_iter().flatten().filter_map(Value::as_i64) {
        let subject = match Subject::load(&c, id) {
            Ok(subject) => subject,
            Err(FiguresError::NotComputed(reason)) => { bots.insert(id.to_string(), json!({ "not_computed": reason })); continue; }
            Err(other) => return Err(other),
        };
        let metrics = answer(walk::metrics(&c, &subject, now))?;
        let live = after(&metrics, |metrics| live::live(&c, &subject, metrics, &market, now))?;
        let marked = after(&live, |live| chart::marked(&c, &subject, live, &market, now))?;
        // The page's chart: nothing where Rails has nothing (no point to plot, or a raise), and a reason where this
        // library has none to give.
        let page = match &marked {
            Ok(marked) => answer(chart::page(&c, &subject, marked, user.hide_balances))?,
            Err(Absent::NotComputed(reason)) => Err(Absent::NotComputed(reason.clone())),
            Err(Absent::Raised(_)) => Ok(None),
        };
        let mut out = Map::new();
        out.insert("metrics".into(), text(&metrics, written));
        out.insert("live".into(), text(&live, written));
        out.insert("marked".into(), text(&marked, written));
        out.insert("chart".into(), match &page {
            Ok(Some(page)) => Value::Object(page.attributes(&zone, 9).into_iter().map(|(name, value)| (name.to_string(), json!(value))).collect()),
            Ok(None) | Err(Absent::Raised(_)) => Value::Null,
            Err(Absent::NotComputed(reason)) => json!({ "not_computed": reason }),
        });
        // Beside the figures, and no part of what is compared with Rails: the held assets the live pass left out.
        out.insert("unpriced".into(), json!(live.iter().flat_map(|live| &live.unpriced).map(|u| json!([u.key, u.reason.as_str()])).collect::<Vec<_>>()));
        // And the holdings the marked chart leaves out of its points.
        out.insert("chart_omitted".into(), json!(marked.iter().flat_map(|marked| &marked.chart_omitted).map(|u| json!([u.key, u.reason.as_str()])).collect::<Vec<_>>()));
        bots.insert(id.to_string(), Value::Object(out));
    }

    let mut out = Map::new();
    out.insert("bots".into(), Value::Object(bots));
    if let Some(gap) = market.gaps.borrow().first() { return Err(FiguresError::Data(format!("unscripted market-data call {gap}"))); }
    Ok(Value::Object(out))
}
