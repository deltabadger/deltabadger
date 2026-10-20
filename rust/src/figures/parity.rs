//! The Rust half of the figures-parity harness (script/rust/figures.rb is the Rails half): every figure of one
//! scenario, computed from its database copy and its scripted market, in the shape Rails reports them.
use super::at::At;
use super::chart;
use super::db::{self, Subject};
use super::json::J;
use super::live;
use super::scripted::Scripted;
use super::totals::{self, Part, Rates};
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

struct Computed { subject: Subject, live: Answer<Metrics>, marked: Answer<Metrics> }

fn snapshot<T>(s: &totals::Snapshot<T>, write: impl Fn(&T) -> J) -> String {
    J::Obj(vec![("result".to_string(), J::opt(&s.result, write)), ("loading".to_string(), J::Bool(s.loading))]).write()
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
    if let Some(steps) = scenario["steps"].as_array() { return stepped(&c, &scenario, steps); }
    let market = Scripted::new(&scenario["script"], scenario["provider"].as_str());
    let mut out = compute(&c, &scenario, now, &market)?;
    if let Some(gap) = market.gaps.borrow().first() { return Err(FiguresError::Data(format!("unscripted market-data call {gap}"))); }
    let mut requests = market.requests.borrow().clone();
    requests.sort();
    requests.dedup();
    out.insert("requests".into(), json!(requests));
    Ok(Value::Object(out))
}

/// A scenario in steps: one market cache (`page_market::Cache`) across them, as the pages keep it, filled from each step's
/// script as a fill fills it, at that step's clock. Its figures are the last step's; each step's requests are listed.
fn stepped(c: &Connection, scenario: &Value, steps: &[Value]) -> Result<Value, FiguresError> {
    use super::page_market::{Cache, Reader};
    let unreadable = |e: &dyn std::fmt::Display| FiguresError::Data(e.to_string());
    let key: i64 = c.query_row("SELECT id FROM api_keys ORDER BY id LIMIT 1", [], |r| r.get(0))?;
    let producer = crate::engine::model::credential_version_by_id(c, key).map_err(|e| FiguresError::Data(format!("{e:?}")))?.ok_or_else(|| FiguresError::Data("no key to make figures under".into()))?;
    let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().map_err(|e| unreadable(&e))?;
    let mut cache = Cache::default();
    let (mut last, mut listed, mut every) = (None, vec![], vec![]);
    for step in steps {
        let now = step["at"].as_str().and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok()).and_then(|at| At::from_utc(at.to_utc()))
            .ok_or_else(|| FiguresError::Data("a step has no `at`".into()))?;
        let wire = StepWire { script: &step["script"], producer: producer.clone(), asked: Default::default() };
        let mut out = None;
        // A pass asks for what it does not hold; the fill fetches it, and the pass is computed again.
        for _ in 0..8 {
            let reader = Reader::new(&cache, now.utc().timestamp()).with_current(c);
            let pass = compute(c, scenario, now, &reader)?;
            let demands = reader.demands();
            if demands.is_empty() { out = Some(pass); break; }
            runtime.block_on(cache.fill(&wire, demands, now.utc().timestamp()));
        }
        let out = out.ok_or_else(|| FiguresError::Data("the figures kept asking for market data".into()))?;
        let mut asked = wire.asked.into_inner();
        if let Some(gap) = asked.iter().find(|line| step["script"].get(line.as_str()).is_none()) { return Err(FiguresError::Data(format!("unscripted market-data call {gap}"))); }
        asked.sort();
        asked.dedup();
        every.extend(asked.iter().cloned());
        listed.push(json!({ "requests": asked }));
        last = Some(out);
    }
    let mut out = last.ok_or_else(|| FiguresError::Data("a scenario with no steps".into()))?;
    every.sort();
    every.dedup();
    out.insert("requests".into(), json!(every));
    out.insert("steps".into(), json!(listed));
    Ok(Value::Object(out))
}

/// A step's market at the wire: every request answered from the step's script under the line the Rails harness logs
/// (method, host, path and the whole sorted query; script/rust/figures.rb `ScriptedMarket.lines`), and logged.
struct StepWire<'a> { script: &'a Value, producer: crate::engine::model::CredentialVersion, asked: std::cell::RefCell<Vec<String>> }
impl crate::venue::http::Transport for StepWire<'_> {
    fn producer(&self) -> Option<crate::engine::model::CredentialVersion> { Some(self.producer.clone()) }
    async fn send(&self, r: &crate::venue::http::HttpRequest) -> Result<crate::venue::http::HttpResponse, crate::venue::http::TransportError> {
        let host = r.base.split_once("://").map_or(r.base.as_str(), |(_, h)| h);
        let mut query: Vec<String> = r.query.iter().map(|(k, v)| format!("{k}={v}")).collect();
        query.sort();
        let line = if query.is_empty() { format!("{} {host}{}", r.method, r.path) } else { format!("{} {host}{}?{}", r.method, r.path, query.join("&")) };
        self.asked.borrow_mut().push(line.clone());
        let reply = self.script.get(&line).ok_or_else(|| crate::venue::http::TransportError::Permanent("Unscripted market request".into()))?;
        if reply["network"].is_string() { return Err(crate::venue::http::TransportError::NotSent("Market data unavailable".into())); }
        Ok(crate::venue::http::HttpResponse { status: reply["status"].as_u64().unwrap_or(200) as u16,
            body: reply["body"].as_str().map(str::to_string).unwrap_or_else(|| reply["body"].to_string()) })
    }
}

/// Every figure of the scenario's bots and account, read from `market`.
fn compute(c: &Connection, scenario: &Value, now: At, market: &dyn super::market::MarketData) -> Result<Map<String, Value>, FiguresError> {
    let user = db::user(c, scenario["user_id"].as_i64().unwrap_or(0))?;
    let zone = crate::web::timezone::strict(&user.time_zone).unwrap_or(chrono_tz::Tz::UTC);
    let written = |m: &Metrics| m.to_json(9).write();

    let mut bots = Map::new();
    let mut computed: Vec<(i64, Computed)> = vec![];
    let mut refused: Vec<(i64, String)> = vec![];
    for id in scenario["bot_ids"].as_array().into_iter().flatten().filter_map(Value::as_i64) {
        let subject = match Subject::load(c, id) {
            Ok(subject) => subject,
            Err(FiguresError::NotComputed(reason)) => { bots.insert(id.to_string(), json!({ "not_computed": reason })); refused.push((id, reason)); continue; }
            Err(other) => return Err(other),
        };
        let metrics = answer(walk::metrics(c, &subject, now))?;
        let live = after(&metrics, |metrics| live::live(c, &subject, metrics, market, now))?;
        let marked = after(&live, |live| chart::marked(c, &subject, live, market, now))?;
        // The page's chart: nothing where Rails has nothing (no point to plot, or a raise), and a reason where this
        // library has none to give.
        let page = match &marked {
            Ok(marked) => answer(chart::page(c, &subject, marked, user.hide_balances))?,
            Err(Absent::NotComputed(reason)) => Err(Absent::NotComputed(reason.clone())),
            Err(Absent::Raised(_)) => Ok(None),
        };
        let profit = after(&live, |live| totals::profit_in_usd(c, market, &mut Rates::default(), subject.quote.as_deref(), live))?;
        let mut out = Map::new();
        out.insert("metrics".into(), text(&metrics, written));
        out.insert("live".into(), text(&live, written));
        out.insert("marked".into(), text(&marked, written));
        out.insert("chart".into(), match &page {
            Ok(Some(page)) => Value::Object(page.attributes(&zone, 9).into_iter().map(|(name, value)| (name.to_string(), json!(value))).collect()),
            Ok(None) | Err(Absent::Raised(_)) => Value::Null,
            Err(Absent::NotComputed(reason)) => json!({ "not_computed": reason }),
        });
        out.insert("profit_in_usd".into(), text(&profit, |profit| J::opt(profit, J::num).write()));
        // Beside the figures, and no part of what is compared with Rails: the held assets the live pass left out.
        out.insert("unpriced".into(), json!(live.iter().flat_map(|live| &live.unpriced).map(|u| json!([u.key, u.reason.as_str()])).collect::<Vec<_>>()));
        // And the holdings the marked chart leaves out of its points.
        out.insert("chart_omitted".into(), json!(marked.iter().flat_map(|marked| &marked.chart_omitted).map(|u| json!([u.key, u.reason.as_str()])).collect::<Vec<_>>()));
        bots.insert(id.to_string(), Value::Object(out));
        computed.push((id, Computed { subject, live, marked }));
    }

    // The account's totals are over its bots that are not deleted, every one of them a part: the totals themselves
    // see to it that none is left out (`totals::complete`).
    let ids: Vec<i64> = db::account_bots(c, user.id)?.into_iter().map(|(id, _)| id).collect();
    let absent: Vec<(i64, Absent)> = refused.iter().map(|(id, reason)| (*id, Absent::NotComputed(reason.clone()))).collect();
    enum Which { Live, Cached, Marked }
    let parts = |which: Which| -> Vec<Part<'_>> {
        ids.iter().filter_map(|id| {
            if let Some((_, absent)) = absent.iter().find(|(bot, _)| bot == id) {
                return Some(Part { bot_id: *id, quote: None, traded: true, figures: Err(absent) });
            }
            let (_, bot) = computed.iter().find(|(bot, _)| bot == id)?;
            let figures = match which {
                Which::Live => bot.live.as_ref().map(Some),
                // What Rails would hold cached: figures that are stale, or have no chart point yet, are never written.
                Which::Cached => bot.live.as_ref().map(|m| Some(m).filter(|m| !m.prices_stale && !m.chart.labels.is_empty())),
                Which::Marked => bot.marked.as_ref().map(Some),
            };
            Some(Part { bot_id: *id, quote: bot.subject.quote.as_deref(), traded: !bot.subject.orders.is_empty(), figures })
        }).collect()
    };
    let mut out = Map::new();
    out.insert("bots".into(), Value::Object(bots));
    let named = |parts: &[Part<'_>], of: totals::In| json!(totals::left_out(parts).iter().filter(|left| left.of == of).map(|left| json!([left.bot_id, left.holding.key, left.holding.reason.as_str()])).collect::<Vec<_>>());
    out.insert("unpriced".into(), named(&parts(Which::Live), totals::In::Value));
    out.insert("chart_omitted".into(), named(&parts(Which::Marked), totals::In::Chart));
    let global = answer(totals::global_pnl(c, market, &mut Rates::default(), &parts(Which::Live)))?;
    out.insert("global_pnl".into(), text(&global, |pnl| J::opt(pnl, totals::GlobalPnl::to_json).write()));
    let ready = answer(totals::global_pnl_snapshot(c, market, &mut Rates::default(), &parts(Which::Cached)))?;
    out.insert("global_pnl_snapshot".into(), text(&ready, |s| snapshot(s, totals::GlobalPnl::to_json)));
    let history = answer(totals::pnl_history(c, market, &mut Rates::default(), &parts(Which::Marked)))?;
    out.insert("pnl_history".into(), text(&history, |s| snapshot(s, totals::History::to_json)));
    let denomination = answer(totals::denomination(c, market, &user.display_currency))?;
    out.insert("denomination".into(), text(&denomination, |d| J::Obj(vec![("currency".to_string(), J::Str(d.currency.clone())), ("rate".to_string(), J::Str(d.rate.to_s_f()))]).write()));

    Ok(out)
}
