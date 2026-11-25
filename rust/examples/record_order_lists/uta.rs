//! The `kucoin-uta` capture mode (PLAN-841 U5-0, r1.1–r5): one KuCoin account read before, right after and after
//! trading on its switch to a Unified Trading Account. Unlike the A28 order lists it records object answers (mode,
//! balance, order detail) and every page through the terminal empty one, and records a classic or HF route's refusal as
//! that route's answer. The three phases and the CSV share one scrub (salt, offset, id map, amount factor) through a
//! local state file (RECORDER_STATE, outside the repo). Read-only: only `READS` are ever sent.
//! rust/tests/order_list_recorder.rs includes this file beside recorder.rs, so CI runs its tests.
#![allow(dead_code)] // the test crate that includes this file uses only part of it

use crate::recorder::*;
use deltabadger::venue::http::{HttpRequest, Transport};
use deltabadger::v2::BigDec;
use deltabadger::venue::kucoin::{uta_evidence, HfOrder};
use std::collections::BTreeMap;
use chrono::{Duration, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::path::Path;

pub const MODE: &str = "/api/ua/v2/account/mode";
pub const OPENED: &str = "/api/v1/hf/accounts/opened";
pub const ACCOUNTS: &str = "/api/v1/accounts";
pub const FILLS: &str = "/api/v1/fills";
pub const HF_FILLS: &str = "/api/v1/hf/fills";
pub const ORDERS: &str = "/api/v1/orders";
pub const HF_ORDERS_DONE: &str = "/api/v1/hf/orders/done";
pub const BALANCE: &str = "/api/ua/v2/unified/account/balance";
pub const DETAIL: &str = "/api/ua/v2/unified/order/detail";
pub const EXECUTION: &str = "/api/ua/v2/unified/order/execution";
pub const HISTORY: &str = "/api/ua/v2/unified/order/history";
pub const API_KEY: &str = "/api/v1/user/api-key";
/// Every path this mode may send, all GET.
pub const READS: &[&str] = &[MODE, OPENED, ACCOUNTS, FILLS, HF_FILLS, ORDERS, HF_ORDERS_DONE, BALANCE, DETAIL, EXECUTION, HISTORY, API_KEY];
/// The importer's time header (tracker/import/kucoin.rs TIME_HEADER).
static TIME_HEADER: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"^Time\(UTC(?:([+-])([0-9]{1,2})(?::?([0-5][0-9]))?)?\)$").unwrap());
/// ponytail: a walk stops at 100 pages and aborts; a test account has a handful of fills.
const MAX_PAGES: usize = 100;

/// The UTA answers' own field rules (r2.2), names as `norm` spells them.
pub fn extra(factor: u32) -> Extra {
    Extra {
        factor,
        amounts: &["size", "filledsize", "cancelsize", "value", "fee", "balance", "available", "hold", "holds", "liability", "equity",
                   "funds", "dealsize", "dealfunds", "dealvalue", "potentialborrow", "tax", "amount", "remainsize", "remainfunds",
                   "cancelledsize", "cancelledfunds", "visiblesize", "filledvalue"],
        verbatim: &["price", "avgprice", "feerate", "taxrate", "code"],
        keep: &["tradetype", "selfaccountmode", "accounttype", "currency", "sizeunit", "liquidityrole", "filltype", "collateralstatus",
               "optype", "channel"],
        text: &["msg"],
        time: &["executiontime", "ordertime", "ts"],
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase { Before, Switched, Traded }
impl Phase {
    pub fn parse(s: &str) -> Option<Self> { Some(match s { "before" => Self::Before, "switched" => Self::Switched, "traded" => Self::Traded, _ => return None }) }
    pub fn dir(self) -> &'static str { match self { Self::Before => "before", Self::Switched => "switched", Self::Traded => "traded" } }
}

/// How a read goes on past its first answer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Walk { One, LastId, Pages }

/// One read: its file stem, path, query in the order sent, how it pages, and whether a refusal is its answer (a classic
/// or HF route on a UTA, and the order history the adapter never reads) rather than an abort.
pub struct Read { pub name: String, pub path: &'static str, pub query: Vec<(&'static str, String)>, pub walk: Walk, pub may_refuse: bool }

fn read(name: &str, path: &'static str, query: Vec<(&'static str, String)>, walk: Walk, may_refuse: bool) -> Read {
    Read { name: name.into(), path, query, walk, may_refuse }
}

/// The phase's reads, each shaped as the code that sends it in production (the file named beside it).
pub fn reads(phase: Phase, w: &Window, symbol: &str, orders: &[String]) -> Vec<Read> {
    let (a, b) = (w.since.timestamp_millis(), w.until.timestamp_millis());
    let s = || symbol.to_string();
    let mut r = vec![
        read("mode", MODE, vec![], Walk::One, false),                                           // venue/kucoin_account.rs detect
        read("hf_accounts_opened", OPENED, vec![], Walk::One, true),
        read("accounts", ACCOUNTS, vec![], Walk::One, true),
        // The continuity probe (r2.4): the known fill's symbol and window, on both classic routes.
        read("hf_fills", HF_FILLS, vec![("symbol", s()), ("startAt", a.to_string()), ("endAt", (b - 1).to_string()), ("limit", "100".into())], Walk::LastId, true), // sync/kucoin_hf_ledger.rs
        read("fills_symbol", FILLS, vec![("symbol", s()), ("startAt", a.to_string()), ("endAt", b.to_string())], Walk::Pages, true),
        // The ledger's own classic request (r3.2, sync/kucoin_ledger.rs Source::Fills): no symbol. It alone decides r2.5.
        read("fills_ledger", FILLS, vec![("startAt", a.to_string()), ("endAt", b.to_string())], Walk::Pages, true),
    ];
    if phase == Phase::Before { return r; }
    r.extend([
        read("orders_done", ORDERS, vec![("status", "done".into()), ("symbol", s()), ("startAt", a.to_string()), ("endAt", b.to_string())], Walk::Pages, true), // r1.4
        read("hf_orders_done", HF_ORDERS_DONE, vec![("symbol", s()), ("startAt", a.to_string()), ("endAt", b.to_string()), ("limit", "100".into())], Walk::LastId, true),
        read("unified_balance", BALANCE, vec![], Walk::One, false),                             // venue/kucoin_uta.rs uta_accounts
        // The ledger's account-wide walk (sync/kucoin_hf_ledger.rs, Route UTA).
        read("executions", EXECUTION, vec![("tradeType", "SPOT".into()), ("startAt", a.to_string()), ("endAt", (b - 1).to_string()), ("pageSize", "200".into())], Walk::LastId, false),
        read("order_history", HISTORY, vec![("tradeType", "SPOT".into()), ("symbol", s()), ("startAt", a.to_string()), ("endAt", b.to_string()), ("pageSize", "200".into())], Walk::LastId, true),
    ]);
    for (i, id) in orders.iter().enumerate() {
        // The adapter's own reads (venue/kucoin_uta.rs uta_order, with_totals).
        r.push(read(&format!("order_{}_detail", i + 1), DETAIL, vec![("tradeType", "SPOT".into()), ("symbol", s()), ("orderId", id.clone())], Walk::One, false));
        r.push(read(&format!("order_{}_executions", i + 1), EXECUTION, vec![("tradeType", "SPOT".into()), ("symbol", s()), ("orderId", id.clone()), ("pageSize", "200".into())], Walk::LastId, false));
    }
    r
}

/// What a non-success answer is: the route's recorded refusal, or an abort (throttled, a server error, or a route that
/// must answer).
/// A refusal names its code; a failed signature, key, passphrase, timestamp or IP (400001–400006, HTTP 401) or a
/// throttle is the probe failing, never the route's answer. 400007 (access denied) stays: it may be the very mode refusal sought.
fn refusal_ok(may_refuse: bool, status: u16, body: &Value) -> bool {
    let Some(code) = body["code"].as_str().filter(|c| c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit()) && *c != "200000") else { return false };
    // 110188: the UTA API's own throttle.
    // 110188, 166115, 230005, 148011: throttled or busy; so is any message that says so.
    let not_an_answer = ["400001", "400002", "400003", "400004", "400005", "400006", "110188", "166115", "230005", "148011"];
    let msg = body["msg"].as_str().unwrap_or_default().to_ascii_lowercase();
    let busy = ["busy", "try again", "too many", "timeout"].iter().any(|w| msg.contains(w));
    may_refuse && !busy && status != 401 && status != 429 && status < 500 && !code.starts_with("429") && !not_an_answer.contains(&code)
}

fn ok(status: u16, body: &Value) -> bool { (200..300).contains(&status) && body["code"] == "200000" }

struct Got { file: String, path: &'static str, params: Vec<(&'static str, String)>, status: u16, refused: bool, body: Value }

/// The state a session's runs share: the scrub and the amount factor (r2.1).
pub fn load_state(path: &Path, create: bool) -> Result<(Scrubber, u32), String> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let v: Value = serde_json::from_str(&text).map_err(|_| "the scrub state file is unreadable".to_string())?;
            let factor = v["factor"].as_u64().filter(|f| (2..=19).contains(f)).ok_or("the scrub state file has no factor")? as u32;
            Ok((Scrubber::from_state(&v["scrub"])?, factor))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            let sc = Scrubber::random();
            // ponytail: an integer factor of 2–19 scales exactly (no rounding to break a sum); it hides amounts only
            // from a reader who doesn't try 18 divisors. Enough for a ~5 USDT test order.
            let factor = 2 + (uuid::Uuid::new_v4().as_bytes()[0] % 18) as u32;
            Ok((sc, factor))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("no scrub state yet: run the before phase first (same RECORDER_STATE)".into()),
        Err(e) => Err(format!("the scrub state file: {e}")),
    }
}

/// Written to a fresh file (never through an existing path or link), owner-only, then renamed over the old state.
pub fn save_state(path: &Path, sc: &Scrubber, factor: u32) -> Result<(), String> {
    use std::io::Write;
    let text = serde_json::to_string(&json!({ "scrub": sc.state(), "factor": factor })).expect("a Value serializes");
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    { use std::os::unix::fs::OpenOptionsExt; o.mode(0o600); }
    let fail = |e: std::io::Error| format!("the scrub state file: {e}");
    let written = o.open(&tmp).and_then(|mut f| f.write_all(text.as_bytes()).and_then(|_| f.sync_all())).map_err(fail);
    if let Err(e) = written.and_then(|_| std::fs::rename(&tmp, path).map_err(fail)) { let _ = std::fs::remove_file(&tmp); return Err(e); }
    Ok(())
}

/// The state file must not sit in the repo: it maps every fake back to its original.
pub fn state_outside(path: &Path, repo: &Path) -> Result<(), String> {
    let abs = |p: &Path| p.parent().filter(|d| !d.as_os_str().is_empty()).map_or(std::env::current_dir().ok(), |d| d.canonicalize().ok());
    match (abs(path), repo.canonicalize().ok()) {
        (Some(d), Some(r)) if !d.starts_with(&r) => Ok(()),
        (Some(_), Some(_)) => Err("RECORDER_STATE is inside the repo; keep it beside the keys".into()),
        _ => Err("RECORDER_STATE's folder does not exist".into()),
    }
}

/// The order `id` on `symbol`, as the adapter's poll wants it, its side and type from the detail answer.
fn want(detail: &Value, id: &str, symbol: &str) -> Option<HfOrder> {
    let d = &detail["data"];
    Some(HfOrder { id: id.into(), symbol: symbol.into(), sell: d["side"].as_str()?.eq_ignore_ascii_case("sell"),
                   limit: d["orderType"].as_str()?.eq_ignore_ascii_case("limit") })
}

/// Order `n`'s evidence (its detail, then its execution pages through the empty one, in file order) read as the
/// adapter's poll reads it, for the order `id` asked for: filled size, quote and fees per currency.
fn readable<'a>(files: impl Iterator<Item = (&'a str, &'a Value)>, n: usize, id: &str, symbol: &str) -> Option<(BigDec, Option<BigDec>, BTreeMap<String, BigDec>)> {
    let (detail, pages) = (format!("order_{n}_detail.json"), format!("order_{n}_executions."));
    let answers: Vec<Value> = files.filter(|(f, _)| *f == detail || f.starts_with(&pages)).map(|(_, v)| v.clone()).collect();
    let want = want(answers.first()?, id, symbol)?;
    let (state, fees) = uta_evidence(&answers, &want).ok()?;
    Some((state.amount_exec, state.quote_amount_exec, fees))
}
/// The raw totals times the factor, as the scrubbed ones must be.
fn scaled_totals(raw: &(BigDec, Option<BigDec>, BTreeMap<String, BigDec>), k: u32) -> Option<(BigDec, Option<BigDec>, BTreeMap<String, BigDec>)> {
    let s = |d: &BigDec| BigDec::parse(&scale(&d.to_s_f(), k)?).ok();
    let quote = match &raw.1 { Some(q) => Some(s(q)?), None => None };
    let fees = raw.2.iter().map(|(c, f)| Some((c.clone(), s(f)?))).collect::<Option<_>>()?;
    Some((s(&raw.0)?, quote, fees))
}

/// One phase: the permission read (General only), every read through its last page, the mode and fill checks, the
/// scrub, the evidence check on each order, the leak check, and only then the write into `out/kucoin_uta/<phase>/`.
#[allow(clippy::too_many_arguments)]
/// The state (when given) is saved before the write, so a published set always has the mapping later phases need.
pub async fn record_uta<T: Transport>(t: &T, base: &str, phase: Phase, w: &Window, orders: &[String], keys: &Keys, sc: &mut Scrubber, factor: u32, out: &Path, state: Option<&Path>) -> Result<String, String> {
    let symbol = w.symbol.clone().ok_or("kucoin-uta needs --symbol (the known fill's, e.g. BTC-USDT)")?;
    if w.until - w.since > Duration::days(7) { return Err("the window is over 7 days, KuCoin's longest fills range".into()); }
    if !orders.is_empty() && phase != Phase::Traded { return Err("--order belongs to the traded phase".into()); }
    let get = |path: &'static str, query: Vec<(&'static str, String)>| HttpRequest { method: "GET", base: base.into(), path: path.into(), query, body: None, not_after: None };
    let send = |r: HttpRequest| async move {
        if r.method != "GET" || !READS.contains(&r.path.as_str()) { return Err(format!("{} {} is not on the read-only allow-list; refused", r.method, r.path)); }
        let resp = t.send(&r).await.map_err(|_| format!("{}: the request failed; nothing written", r.path))?;
        if long_integer(&resp.body) { return Err(format!("{}: an integer of 20+ digits would be lossy; nothing written", r.path)); }
        let body: Value = serde_json::from_str(&resp.body).map_err(|_| format!("{}: the answer is not JSON (HTTP {}); nothing written", r.path, resp.status))?;
        Ok((resp.status, body))
    };
    match t.send_observed(&get(API_KEY, vec![]), &|status, body| permission("kucoin", status, body)).await {
        Ok((_, Permission::ReadOnly)) => {}
        Ok((_, Permission::CanTrade)) => return Err("kucoin: this key can trade or withdraw; the recorder takes a General-only key".into()),
        _ => return Err("kucoin: this key's permissions could not be read; refused".into()),
    }
    let mut got: Vec<Got> = vec![];
    for rd in reads(phase, w, &symbol, orders) {
        let mut last: Option<String> = None;
        for page in 1..=MAX_PAGES {
            let mut query = rd.query.clone();
            match rd.walk {
                Walk::Pages => query.extend([("currentPage", page.to_string()), ("pageSize", "500".to_string())]),
                Walk::LastId => query.extend(last.clone().map(|id| ("lastId", id))),
                Walk::One => {}
            }
            let (status, body) = send(get(rd.path, query.clone())).await?;
            let file = if rd.walk == Walk::One { format!("{}.json", rd.name) } else { format!("{}.{page}.json", rd.name) };
            let refused = !ok(status, &body);
            if refused && !refusal_ok(rd.may_refuse, status, &body) {
                return Err(format!("{file}: HTTP {status}, code {}; nothing written", body["code"].as_str().unwrap_or("none")));
            }
            let data = body["data"].clone();
            got.push(Got { file, path: rd.path, params: query, status, refused, body });
            if refused { break; }
            let items = match (rd.walk, data["items"].as_array()) {
                (Walk::One, _) => 0,
                (_, Some(a)) => a.len(),
                (_, None) => return Err(format!("{}: page {page} has no item list; nothing written", rd.name)),
            };
            match rd.walk {
                Walk::One => break,
                // As the ledger: the page asked for against totalPage (a number or a numeric string).
                Walk::Pages => {
                    let total = match &data["totalPage"] { Value::Number(n) => n.as_u64(), Value::String(t) => t.parse().ok(), _ => None };
                    let total = total.ok_or_else(|| format!("{}: page {page} has no totalPage; nothing written", rd.name))?;
                    if page as u64 >= total { break; }
                }
                Walk::LastId if items == 0 => break,
                Walk::LastId => {
                    let next = match &data["lastId"] { Value::Number(n) => n.as_u64().map(|n| n.to_string()), Value::String(s) if s.parse::<u64>().is_ok() => Some(s.clone()), _ => None };
                    if next.is_none() || next == last { return Err(format!("{}: the lastId cursor did not move; nothing written", rd.name)); }
                    last = next;
                }
            }
            if page == MAX_PAGES { return Err(format!("{}: over {MAX_PAGES} pages; nothing written", rd.name)); }
        }
    }
    let mode = got.iter().find(|g| g.file == "mode.json").and_then(|g| g.body["data"]["selfAccountMode"].as_str()).unwrap_or_default().to_string();
    match (phase, mode.as_str()) {
        (Phase::Before, "CLASSIC") | (Phase::Switched | Phase::Traded, "UNIFIED") => {}
        _ => return Err(format!("the account's mode is {mode:?}, not the {} phase's; nothing written", phase.dir())),
    }
    // r2.4: a fill on --symbol, created inside the window, on a classic route.
    let (a, b) = (w.since.timestamp_millis(), w.until.timestamp_millis());
    let known = got.iter().filter(|g| (g.file.starts_with("hf_fills.") || g.file.starts_with("fills_symbol.")) && !g.refused)
        .flat_map(|g| g.body["data"]["items"].as_array().cloned().unwrap_or_default())
        .any(|f| f["symbol"] == symbol.as_str() && f["createdAt"].as_i64().is_some_and(|t| (a..b).contains(&t)));
    if phase == Phase::Before && !known {
        return Err("the before phase found no fill on --symbol in the window (r2.4: a known recent fill); nothing written".into());
    }
    let raw_totals: Vec<_> = orders.iter().enumerate().map(|(i, id)| readable(got.iter().map(|g| (g.file.as_str(), &g.body)), i + 1, id, &symbol)).collect();
    sc.extra = Some(extra(factor));
    for g in &got { sc.reserve(&g.body); }
    let mut files = vec![];
    let mut entries = vec![];
    for g in got {
        let body = sc.scrub("kucoin", &g.file, g.body)?;
        let mut params = Map::new();
        for (k, v) in g.params {
            let v = match k {
                "startAt" | "endAt" => sc.shift_str(&v).ok_or("a window param did not shift")?,
                "orderId" | "lastId" => sc.fake(&v, k == "lastId", &format!("params/{k}"))?,
                _ => v,
            };
            params.insert(k.into(), Value::String(v));
        }
        entries.push(json!({ "file": g.file, "method": "GET", "path": g.path, "params": params, "status": g.status, "refused": g.refused }));
        files.push((g.file, body));
    }
    let mut notes = vec![];
    for (i, raw) in raw_totals.iter().enumerate() {
        let n = i + 1;
        let Some(raw) = raw else { notes.push(format!("order {n}: the adapter cannot read this order's evidence (U5-1's first failing test)")); continue };
        let fake = sc.mapped(&orders[i]).ok_or_else(|| format!("order {n}: its id was not scrubbed; nothing written"))?;
        let scrubbed = readable(files.iter().map(|(f, v)| (f.as_str(), v)), n, &fake, &symbol);
        if scrubbed.is_none() || scrubbed != scaled_totals(raw, factor) {
            return Err(format!("order {n}: the scrubbed totals are not the recorded ones times the factor; nothing written"));
        }
    }
    let report = sc.report_text();
    let recorded_at = sc.shift_str(&Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)).expect("an RFC 3339 time shifts");
    let refused: Vec<&str> = entries.iter().filter(|e| e["refused"] == true).filter_map(|e| e["file"].as_str()).collect();
    let refused_text = refused.join(", ");
    let manifest = json!({ "exchange": "kucoin", "mode": "kucoin-uta", "phase": phase.dir(), "recorded_at": recorded_at, "files": entries,
                           "notes": notes, "scrubbed": sc.report.iter().map(|(f, k)| (f.clone(), json!(k))).collect::<Map<_, _>>() });
    let normalized = keys.secret.replace("\\n", "\n");
    let secrets = [keys.key.as_str(), keys.secret.as_str(), keys.passphrase.as_str(), normalized.as_str()];
    for (name, v) in &files { sc.leak_check(name, v, &secrets)?; }
    sc.leak_check("manifest.json", &manifest, &secrets)?;
    sc.leak_check("report", &Value::String(report.clone()), &secrets)?;
    let dir = out.join("kucoin_uta").join(phase.dir());
    files.push(("manifest.json".into(), manifest));
    if let Some(p) = state { save_state(p, sc, factor)?; }
    write_set(&dir, &files)?;
    let mut text = format!("wrote {}\nchanged fields:\n{report}", dir.display());
    if !refused.is_empty() { text += &format!("\nrefused (recorded as the answer): {refused_text}"); }
    for n in notes { text += &format!("\n{n}"); }
    Ok(text)
}

/// KuCoin's Account History export, scrubbed with the session's state (r2.1, r2.6): `UID` and free text replaced, the
/// time column shifted, `Amount` and `Fee` scaled, the enum columns kept. Every cell quoted on the way out.
pub fn scrub_csv(text: &str, sc: &mut Scrubber, factor: u32, secrets: &[&str]) -> Result<String, String> {
    use deltabadger::tracker::import::csv::Table;
    let table = Table::parse(text, |_| true).map_err(|e| format!("the export is not CSV: {}", e.0))?;
    let (header, _) = table.whole();
    let headers: Vec<String> = header.into_iter().map(|h| h.map(|h| h.into_owned()).unwrap_or_default()).collect();
    // The header row is copied as is, so it must be the export's: the importer's columns, each a short name.
    let named = |h: &str| headers.iter().any(|x| x == h);
    if !["UID", "Account Type", "Currency", "Side", "Amount", "Fee", "Remark", "Type"].iter().all(|h| named(h)) || !headers.iter().any(|h| TIME_HEADER.is_match(h))
        || headers.iter().any(|h| !TIME_HEADER.is_match(h) && (h.len() > 32 || !h.chars().all(|c| c.is_ascii_alphabetic() || c == ' '))) {
        return Err("this is not a KuCoin Account History export (its header row); nothing written".into());
    }
    for row in table.whole().1 { for c in row.iter().flatten() { sc.reserve(&Value::String(c.to_string())); } }
    // `row[name]`: the cell under the first header of that name.
    let get = |row: &[Option<std::borrow::Cow<'_, str>>], name: &str| headers.iter().position(|h| h == name).and_then(|i| row.get(i)?.as_deref().map(str::to_string));
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = vec![headers.iter().map(|h| quote(h)).collect::<Vec<_>>().join(",")];
    for row in table.whole().1.filter(|r| !r.is_empty()) {
        let mut cells = vec![];
        for (i, cell) in row.iter().enumerate() {
            let (h, c) = (headers.get(i).map_or("", String::as_str), cell.as_deref().unwrap_or(""));
            let v = if c.is_empty() { String::new() } else if TIME_HEADER.is_match(h) {
                sc.count("export.csv", h, "shifted");
                sc.shift_str(c).ok_or_else(|| format!("export.csv: a {h} cell is no time"))?
            } else if ["Amount", "Fee"].contains(&h) {
                sc.count("export.csv", h, "scaled");
                scale(c, factor).ok_or_else(|| format!("export.csv: a {h} cell is no decimal"))?
            } else if ["Account Type", "Currency", "Side", "Type"].contains(&h) && kept_shape(c) {
                c.to_string()
            } else if h == "Remark" && [get(&row, "Type"), get(&row, "Side")].contains(&Some(c.to_string())) {
                // A remark that only repeats the row's type or side ("Deposit"); any other may name a person or address.
                c.to_string()
            } else if c.chars().all(|x| x.is_ascii_alphanumeric() || "-_.:".contains(x)) {
                sc.count("export.csv", h, "replaced");
                // A value no fake can differ from ("-", "0") is replaced whole.
                sc.fake(c, false, h).unwrap_or_else(|_| { sc.dropped(c); "(scrubbed)".into() })
            } else {
                // Free text a fake would keep in part (a fake changes only ASCII letters and digits).
                sc.count("export.csv", h, "replaced");
                sc.dropped(c);
                "(scrubbed)".to_string()
            };
            cells.push(quote(&v));
        }
        out.push(cells.join(","));
    }
    let text = out.join("\n") + "\n";
    sc.leak_check("export.csv", &Value::String(text.clone()), secrets)?;
    Ok(text)
}
