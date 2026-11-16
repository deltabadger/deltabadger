//! The IBKR recorder's logic (PLAN-A27-1-ibkr-recorder.md): the material dir, the Guard (the per-mode allow-list and the
//! paper check, inside the transport), IBKR's scrub over the shared Scrubber, and the record flow for each mode through
//! the engine's own `venue::ibkr::Session`. rust/tests/ibkr_recorder.rs includes this file so CI runs its tests (CI never
//! runs an example's own tests); main.rs beside it only reads arguments and the material and builds the live transport.
#![allow(dead_code)] // the test crate that includes this file uses only part of it

#[path = "../common/scrub.rs"]
mod scrub;
pub use scrub::*;

use chrono::{SecondsFormat, Utc};
use deltabadger::engine::Clock;
use deltabadger::venue::http::{HttpRequest, HttpResponse, Transport, TransportError};
use deltabadger::venue::ibkr::{Error, Session, ACCOUNTS_PATH, HISTORY_PATH, SNAPSHOT_PATH};
use deltabadger::venue::ibkr_venue::{LIVE_ORDERS_PATH, MAX_REPLIES, PREFLIGHT_PAUSE, RULES_PATH, SNAPSHOT_FIELDS, TRADES_PATH};
use deltabadger::venue::signers::ibkr::Material;
use deltabadger::venue::{ibkr_account, VenueError};
use serde_json::{json, Map, Value};
use std::cell::RefCell;
use std::collections::HashSet;
use std::path::Path;

pub const PA_TRANSACTIONS_PATH: &str = "/v1/api/pa/transactions";
pub const PA_DAYS: u32 = 90;
/// How often, and how far apart, a market order's status is read until it is filled.
pub const POLLS: usize = 20;
pub const POLL_PAUSE: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode { Read, Paper }
impl Mode {
    pub fn dir(self) -> &'static str { match self { Mode::Read => "read", Mode::Paper => "paper" } }
}

// ---- the material and the command line

/// The material's files in `IBKR_RECORDER_DIR`, which must be outside the repository `repo`.
pub fn material(dir: &Path, repo: &Path) -> Result<Material, String> {
    let dir = dir.canonicalize().map_err(|_| "IBKR_RECORDER_DIR is not a readable directory")?;
    let repo = repo.canonicalize().map_err(|_| "the repository path does not resolve")?;
    if dir.starts_with(&repo) { return Err("IBKR_RECORDER_DIR must be outside the repository".into()); }
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).map(|s| s.trim().to_string()).map_err(|_| format!("IBKR_RECORDER_DIR has no readable {name}"));
    let realm = std::fs::read_to_string(dir.join("realm")).map(|s| s.trim().to_string()).unwrap_or_default();
    let m = Material {
        consumer_key: read("consumer_key")?, access_token: read("access_token")?, access_token_secret: read("access_token_secret")?,
        signature_key: read("private_signature.pem")?, encryption_key: read("private_encryption.pem")?, dh_param: read("dhparam.pem")?, realm,
    };
    if [&m.consumer_key, &m.access_token, &m.access_token_secret].iter().any(|v| v.is_empty()) { return Err("a credential file in IBKR_RECORDER_DIR is empty".into()); }
    Ok(m)
}

/// Every credential value the output must never hold: the five, and each long line of a PEM.
pub fn secrets(m: &Material) -> Vec<String> {
    let mut out = vec![m.consumer_key.clone(), m.access_token.clone(), m.access_token_secret.clone(), m.signature_key.clone(), m.encryption_key.clone()];
    for pem in [&m.signature_key, &m.encryption_key] {
        out.extend(pem.lines().map(str::trim).filter(|l| l.len() >= 16 && !l.starts_with("-----")).map(str::to_string));
    }
    out.retain(|s| !s.is_empty());
    out
}

/// Credentials are never an argument: refuse anything that looks like one.
pub fn refuse_key_args(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| ["key", "secret", "token", "pem", "material"].iter().any(|w| a.to_ascii_lowercase().contains(w))) {
        return Err("credentials are read from the files in IBKR_RECORDER_DIR only, never from arguments".into());
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stock { pub tag: &'static str, pub symbol: String, pub conid: i64, pub isin: String }

#[derive(Clone, Debug, PartialEq)]
pub struct Command { pub mode: Mode, pub stocks: Vec<Stock>, pub limit: Option<String>, pub untradable: Option<i64> }

pub const USAGE: &str = "usage: record_ibkr read --us SYMBOL,CONID,ISIN --eu SYMBOL,CONID,ISIN\n       record_ibkr paper --stock SYMBOL,CONID,ISIN --limit PRICE [--untradable CONID]";

fn stock(tag: &'static str, v: &str) -> Result<Stock, String> {
    let parts: Vec<&str> = v.split(',').map(str::trim).collect();
    let [symbol, conid, isin] = parts[..] else { return Err(format!("--{tag} takes SYMBOL,CONID,ISIN")) };
    let conid = conid.parse::<i64>().ok().filter(|c| *c > 0).ok_or(format!("--{tag}: the conid is not a number"))?;
    if !symbol_shape(symbol) || !isin_shape(isin) { return Err(format!("--{tag}: the symbol or ISIN is not IBKR's spelling")); }
    Ok(Stock { tag, symbol: symbol.into(), conid, isin: isin.into() })
}

pub fn parse_args(args: Vec<String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let mode = match args.next().as_deref() { Some("read") => Mode::Read, Some("paper") => Mode::Paper, _ => return Err(USAGE.into()) };
    let (mut us, mut eu, mut paper, mut limit, mut untradable) = (None, None, None, None, None);
    while let Some(flag) = args.next() {
        let value = args.next().filter(|v| !v.starts_with("--")).ok_or(USAGE)?;
        match (mode, flag.as_str()) {
            (Mode::Read, "--us") => us = Some(stock("us", &value)?),
            (Mode::Read, "--eu") => eu = Some(stock("eu", &value)?),
            (Mode::Paper, "--stock") => paper = Some(stock("stock", &value)?),
            (Mode::Paper, "--limit") => limit = Some(value.parse::<f64>().ok().filter(|p| p.is_finite() && *p > 0.0).map(|_| value).ok_or("--limit is not a positive price")?),
            (Mode::Paper, "--untradable") => untradable = Some(value.parse::<i64>().map_err(|_| "--untradable takes a conid")?),
            _ => return Err(USAGE.into()),
        }
    }
    match mode {
        Mode::Read => Ok(Command { mode, stocks: vec![us.ok_or(USAGE)?, eu.ok_or(USAGE)?], limit: None, untradable: None }),
        Mode::Paper => Ok(Command { mode, stocks: vec![paper.ok_or(USAGE)?], limit: Some(limit.ok_or(USAGE)?), untradable }),
    }
}

// ---- the Guard: what may be sent, per mode, checked inside the transport

/// The paths the tool may send ("{acct}": an account id, "{n}": digits, "{id}": a reply or order id), and whether only
/// paper mode may (an order, a reply, a cancel). Everything else is refused in both modes.
pub const ALLOWED: &[(&str, &str, bool)] = &[
    ("POST", "/v1/api/oauth/live_session_token", false),
    ("POST", "/v1/api/iserver/auth/ssodh/init", false),
    ("GET", "/v1/api/iserver/accounts", false),
    ("GET", "/v1/api/portfolio/{acct}/positions/{n}", false),
    ("GET", "/v1/api/portfolio/{acct}/ledger", false),
    ("GET", "/v1/api/iserver/marketdata/snapshot", false),
    ("GET", "/v1/api/iserver/marketdata/history", false),
    ("POST", "/v1/api/iserver/contract/rules", false),
    ("GET", "/v1/api/iserver/account/orders", false),
    ("GET", "/v1/api/iserver/account/trades", false),
    ("GET", "/v1/api/iserver/account/order/status/{n}", false),
    ("POST", "/v1/api/pa/transactions", false),
    ("GET", "/v1/api/iserver/secdef/search", false),
    ("GET", "/v1/api/iserver/secdef/info", false),
    ("GET", "/v1/api/trsrv/stocks", false),
    ("GET", "/v1/api/trsrv/secdef", false),
    ("GET", "/v1/api/iserver/contract/{n}/info", false),
    ("POST", "/v1/api/iserver/account/{acct}/orders", true),
    ("POST", "/v1/api/iserver/reply/{id}", true),
    ("DELETE", "/v1/api/iserver/account/{acct}/order/{id}", true),
];

/// The pattern `path` matches, with what its placeholders hold; None if no segment is safe or none matches.
fn route(method: &str, path: &str) -> Option<(&'static str, bool, Option<String>, Option<String>)> {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.first() != Some(&"") || segs[1..].iter().any(|s| s.is_empty() || *s == "." || *s == ".." || s.contains(['%', '?', '#', '\\'])) { return None; }
    'next: for (m, pattern, paper) in ALLOWED {
        if *m != method { continue; }
        let pat: Vec<&str> = pattern.split('/').collect();
        if pat.len() != segs.len() { continue; }
        let (mut acct, mut id) = (None, None);
        for (p, s) in pat.iter().zip(&segs) {
            match *p {
                "{acct}" if ibkr_account::parse(s).is_some() => acct = Some(s.to_string()),
                "{n}" if s.bytes().all(|b| b.is_ascii_digit()) => id = Some(s.to_string()),
                "{id}" if s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') => id = Some(s.to_string()),
                lit if lit == *s => {}
                _ => continue 'next,
            }
        }
        return Some((pattern, *paper, acct, id));
    }
    None
}

/// One request the Guard relayed and its answer as the inner transport gave it.
#[derive(Clone, Debug)]
pub struct Exchange { pub method: &'static str, pub path: String, pub query: Vec<(&'static str, String)>, pub body: Option<Value>, pub status: u16, pub text: String }

#[derive(Default)]
struct State {
    /// The paper account the last accounts answer named, alone; taken by the one paper-only request it allows.
    authorized: Option<String>,
    /// The prompt ids and order ids this run's own orders were answered with: the only ones a reply or cancel may name.
    prompts: HashSet<String>,
    orders: HashSet<String>,
    log: Vec<Exchange>,
    refused: Vec<String>,
    /// A credential (or the transport's redaction of one) was in an answer: the run stops, nothing is written.
    poisoned: bool,
}

/// The transport every request of a run goes through (PLAN Q3): the per-mode allow-list, the paper check of the account
/// list read just before, the binding of a paper-only request to that account and to this run's own ids, and the raw
/// credential check of every answer. A refusal is Permanent: nothing was sent.
pub struct Guard<T: Transport> { inner: T, mode: Mode, sensitive: Vec<String>, st: RefCell<State> }

impl<T: Transport> Guard<T> {
    pub fn new(inner: T, mode: Mode, sensitive: Vec<String>) -> Self { Self { inner, mode, sensitive, st: RefCell::default() } }
    pub fn log(&self) -> Vec<Exchange> { self.st.borrow().log.clone() }
    pub fn refused(&self) -> Vec<String> { self.st.borrow().refused.clone() }
    pub fn poisoned(&self) -> bool { self.st.borrow().poisoned }

    /// Whether `text` holds a credential, decoded or not, or is the transport's redaction of one.
    pub fn credential_in(&self, text: &str) -> bool {
        let values: Vec<&str> = self.sensitive.iter().map(String::as_str).collect();
        text.contains(deltabadger::crypto::VENUE_TEXT_REDACTED) || deltabadger::crypto::scrub_known(text, &values) != text
    }

    fn refuse(&self, r: &HttpRequest, why: &str) -> TransportError {
        self.st.borrow_mut().refused.push(format!("{} {}: {why}", r.method, r.path));
        TransportError::Permanent(format!("record_ibkr refused {} {}: {why}", r.method, r.path))
    }

    fn check(&self, r: &HttpRequest) -> Result<(), TransportError> {
        let Some((pattern, paper, acct, id)) = route(r.method, &r.path) else { return Err(self.refuse(r, "not on the allow-list")) };
        let mut st = self.st.borrow_mut();
        if pattern == "/v1/api/oauth/live_session_token" { return Ok(()); }
        // Every other request ends what the last account list allowed (an `ssodh/init` too); a paper-only one uses it up.
        let authorized = st.authorized.take();
        if !paper { return Ok(()); }
        drop(st);
        if self.mode != Mode::Paper { return Err(self.refuse(r, "read mode sends no order, reply or cancel")); }
        let Some(account) = authorized else { return Err(self.refuse(r, "no paper-only account list was read just before")) };
        if acct.is_some_and(|a| a != account) { return Err(self.refuse(r, "the path names another account than the one listed")); }
        let st = self.st.borrow();
        match pattern {
            "/v1/api/iserver/account/{acct}/orders" => {
                let orders = r.body.as_ref().and_then(|b| b.get("orders")).and_then(Value::as_array);
                let only_orders = r.body.as_ref().and_then(Value::as_object).is_some_and(|o| o.len() == 1);
                let mut named_account = false;
                if let Some(b) = &r.body { visit(b, "", &mut |text, path| { if path.ends_with("(a key)") && ["acctId", "account", "acct", "acctid"].contains(&text) { named_account = true; } }); }
                if !only_orders || orders.is_none_or(|o| o.is_empty()) || named_account { return Err({ drop(st); self.refuse(r, "an order body must be {\"orders\": [...]} naming no account") }); }
            }
            "/v1/api/iserver/reply/{id}" if !id.as_ref().is_some_and(|i| st.prompts.contains(i)) => return Err({ drop(st); self.refuse(r, "the reply names no prompt of this run") }),
            "/v1/api/iserver/account/{acct}/order/{id}" if !id.as_ref().is_some_and(|i| st.orders.contains(i)) => return Err({ drop(st); self.refuse(r, "the cancel names no order of this run") }),
            _ => {}
        }
        Ok(())
    }

    /// What an answer tells the Guard: an account list (the paper check), and a placement's prompt and order ids.
    fn observe(&self, r: &HttpRequest, resp: &HttpResponse) {
        let mut st = self.st.borrow_mut();
        if self.credential_in(&resp.body) { st.poisoned = true; }
        let parsed = serde_json::from_str::<Value>(&resp.body).ok();
        if r.method == "GET" && r.path == ACCOUNTS_PATH {
            let listed = parsed.as_ref().filter(|_| (200..300).contains(&resp.status)).map(ibkr_account::listed).unwrap_or_default();
            st.authorized = match &listed[..] {
                [Value::String(a)] if (a.starts_with("DU") || a.starts_with("DF")) && ibkr_account::parse(a).is_some() => Some(a.clone()),
                _ => None,
            };
        }
        let placement = r.method == "POST" && (r.path.ends_with("/orders") || r.path.starts_with("/v1/api/iserver/reply/"));
        if let (true, Some(v)) = (placement, &parsed) {
            for row in rows(v) {
                if let Some(id) = row.get("id").filter(|_| row.get("message").is_some()).map(text) { st.prompts.insert(id); }
                if let Some(id) = row.get("order_id").or_else(|| row.get("orderId")).filter(|v| !v.is_null()).map(text) { st.orders.insert(id); }
            }
        }
        st.log.push(Exchange { method: r.method, path: r.path.clone(), query: r.query.clone(), body: r.body.clone(), status: resp.status, text: resp.body.clone() });
    }
}

impl<T: Transport> Transport for Guard<T> {
    fn redact_diagnostic(&self, text: &str) -> String { self.inner.redact_diagnostic(text) }
    fn now(&self) -> chrono::DateTime<Utc> { self.inner.now() }
    async fn send(&self, r: &HttpRequest) -> Result<HttpResponse, TransportError> { self.send_typed(r).await.map(|(resp, _)| resp) }
    async fn send_typed(&self, r: &HttpRequest) -> Result<(HttpResponse, Option<String>), TransportError> {
        self.check(r)?;
        let (resp, content_type) = self.inner.send_typed(r).await?;
        self.observe(r, &resp);
        Ok((resp, content_type))
    }
}

fn text(v: &Value) -> String { v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()) }
fn rows(v: &Value) -> Vec<&Value> { match v { Value::Array(a) => a.iter().collect(), Value::Object(_) => vec![v], _ => vec![] } }

// ---- IBKR's scrub (PLAN Q4), over the shared Scrubber's primitives

/// IBKR field names (normalized: lowercase alphanumerics) whose key is kept; any other key is replaced.
pub const IBKR_KEYS: &[&str] = &[
    // envelopes, auth and errors
    "error", "errors", "statuscode", "code", "success", "message", "messages", "msg", "text", "warning", "warningmessage", "authenticated",
    "connected", "competing", "established", "fail", "id", "isSuppressed", "issuppressed", "messageids", "encryptmessage", "localorderid", "rc", "nd",
    // accounts
    "accounts", "acctprops", "aliases", "allowfeatures", "chartperiods", "groups", "profiles", "selectedaccount", "serverinfo", "servername",
    "serverversion", "sessionid", "isft", "ispaper", "haschildaccounts", "supportscashqty", "supportsfractions", "allowcustomertime", "liteunderpro",
    "nofxconv", "isfaclient", "isnfa", "isprop", "isibrokeraccount", "showgfis", "showeuconsolidateddata", "allowedtoperms", "debugpnl",
    "showtaxopt", "showimpactdashboard", "allowfxconv", "allowfinancialcalendar", "allowdynaccount", "allowtypeahead", "allowcrypto",
    "allowtaxoptimization", "allowapplicationcounter", "allowtradingschedule", "allowordermsgfilter", "restrictedprops", "isfreetrial",
    // positions and ledger
    "acctid", "conid", "conids", "conidex", "contractdesc", "position", "mktprice", "mktvalue", "currency", "avgcost", "avgprice", "realizedpnl",
    "unrealizedpnl", "exchs", "expiry", "putorcall", "multiplier", "strike", "exercisestyle", "conexchmap", "assetclass", "undconid", "model", "time",
    "chinesename", "allexchanges", "listingexchange", "countrycode", "name", "lasttradingday", "group", "sector", "sectorgroup", "ticker", "type",
    "fullname", "isus", "incrementrules", "displayrule", "iseventcontract", "pagesize", "hasoptions", "commoditymarketvalue", "futuremarketvalue",
    "settledcash", "exchangerate", "cashbalance", "corporatebondsmarketvalue", "warrantsmarketvalue", "netliquidationvalue", "interest",
    "stockmarketvalue", "moneyfunds", "realizedpnl", "funds", "acctcode", "issueroptionsmarketvalue", "key", "timestamp", "severity",
    "stockoptionmarketvalue", "futuresonlypnl", "tbondsmarketvalue", "futureoptionmarketvalue", "cashbalancefxsegment", "secondkey",
    "tbillsmarketvalue", "endofbundle", "dividends", "cryptocurrencyvalue", "displayrulestep", "magnification", "decimaldigits",
    // market data
    "updated", "serverid", "symbol", "priceFactor", "pricefactor", "starttime", "high", "low", "timeperiod", "barlength", "mdavailability",
    "mktdatadelay", "outsiderth", "volumefactor", "pricedisplayrule", "pricedisplayvalue", "negativecapable", "messageversion", "data", "points",
    "traveltime", "o", "c", "h", "l", "v", "t",
    // contract rules
    "rules", "algoeligible", "overnighteligible", "costreport", "cantradeacctids", "ordertypes", "ibalgotypes", "fraqtypes", "forceorderpreview",
    "cqttypes", "orderdefaults", "ordertypesoutside", "defaultsize", "cashsize", "sizeincrement", "tiftypes", "tifdefaults", "limitprice",
    "stopprice", "orderorigination", "preview", "displaysize", "fraqint", "cashccy", "cashqtyincr", "pricemagnifier", "incrementtype",
    "loweredge", "increment", "hassecondary", "modtypes", "incrementdigits", "isbuy", "info", "lmt", "mkt", "stp", "stplmt",
    // orders, status, trades
    "orders", "snapshot", "acct", "account", "orderid", "sizeandfills", "orderdesc", "description1", "sectype", "remainingquantity",
    "filledquantity", "totalsize", "companyname", "status", "orderccpstatus", "origordertype", "supportstaxopt", "lastexecutiontime",
    "ordertype", "bgcolor", "fgcolor", "orderref", "timeinforce", "lastexecutiontimer", "side", "price", "subtype", "requestid", "size",
    "optionacct", "cumfill", "orderstatus", "orderstatusdescription", "tif", "ordernoteditable", "editablefields", "cannotcancelorder",
    "deactivateorder", "availablechartperiods", "orderdescription", "orderdescriptionwithcontract", "alertactive", "childordertype",
    "orderclearingaccount", "exitstrategydisplayprice", "exitstrategychartdescription", "averageprice", "exitstrategytoolavailability",
    "allowedduplicateopposite", "ordertime", "executionid", "tradetime", "tradetimer", "submitter", "exchange", "commission", "netamount",
    "accountcode", "acountcode", "contractdescription1", "listingexchange", "clearingid", "clearingname", "liquidationtrade", "iseventtrading",
    "quantity", "coid", "confirmed", "parentid", "orderstatusrequestid",
    // portfolio analyst
    "acctids", "days", "transactions", "date", "cur", "fxrate", "pr", "qty", "amt", "desc", "rpnl", "from", "to", "includesrealtime", "amount",
    // contract lookups
    "companyheader", "description", "restricted", "fop", "opt", "war", "sections", "months", "legsectype", "issuers", "bondid", "issuerid",
    "cficode", "cusip", "expiryfull", "maturitydate", "industry", "instrumenttype", "tradingclass", "validexchanges", "allowselllong",
    "iszerocommissionsecurity", "localsymbol", "contractclarificationtype", "classifier", "underlyingconid", "rth", "underlyingissuer",
    "contractmonth", "isin", "secdef", "contracts", "category", "conidexchange", "showprips", "category", "isinstrumenttype",
];

/// Values kept anywhere when they pass their shape check (PLAN Q4 enums); a value off-shape is replaced.
fn enum_ok(name: &str, s: &str) -> bool {
    let set = |xs: &[&str]| xs.iter().any(|x| x.eq_ignore_ascii_case(s));
    match name {
        "status" | "orderstatus" => set(&["Filled", "Cancelled", "PendingCancel", "Inactive", "Submitted", "PreSubmitted", "PendingSubmit", "Rejected", "ApiPending", "ApiCancelled", "Unknown"]),
        "side" => set(&["BUY", "SELL", "B", "S"]),
        "ordertype" | "origordertype" => set(&["MKT", "LMT", "STP", "MARKET", "LIMIT", "STOP", "STOP_LIMIT"]),
        "tif" | "timeinforce" => set(&["DAY", "GTC", "IOC", "OPG", "GTD"]),
        "sectype" | "assetclass" | "instrumenttype" => set(&["STK", "OPT", "FUT", "FOP", "WAR", "CASH", "BOND", "FUND", "IND", "CFD", "CRYPTO", "BILL"]),
        "currency" | "cur" | "cashccy" => s == "BASE" || (s.len() == 3 && s.bytes().all(|b| b.is_ascii_uppercase())),
        "6509" | "mdavailability" => (1..=4).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic()),
        "messageids" => s.len() >= 2 && s.len() <= 6 && s.starts_with('o') && s[1..].bytes().all(|b| b.is_ascii_digit()),
        "ordertypes" | "ordertypesoutside" | "tiftypes" | "ibalgotypes" | "fraqtypes" | "cqttypes" | "modtypes" =>
            (1..=24).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'/'),
        "type" => set(&["Buy", "Sell", "Dividend", "Dividends", "Payment In Lieu", "Commission", "Withholding Tax", "Deposit", "Withdrawal", "Interest", "Other Fee", "STK"]),
        _ => false,
    }
}

/// Names whose small numbers (< 10 000) are kept: the request's own window and paging counters, not money.
const SMALL: &[&str] = &["days", "rc", "nd", "pagesize", "barlength", "points", "messageversion", "mktdatadelay", "traveltime", "pricefactor", "volumefactor"];
/// IBKR's time field names (normalized): shifted by the offset, else replaced.
const TIMES: &[&str] = &["updated", "tradetime", "tradetimer", "lastexecutiontime", "lastexecutiontimer", "t", "date", "starttime", "servertime", "timestamp", "time", "ordertime", "from", "to"];
const CONIDS: &[&str] = &["conid", "conids", "conidex", "conidexchange", "underlyingconid", "undconid"];

pub fn symbol_shape(s: &str) -> bool { (1..=12).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.' || b == b' ') }
pub fn isin_shape(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 12 && b[..2].iter().all(u8::is_ascii_uppercase) && b[2..11].iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && b[11].is_ascii_digit()
}
fn exchanges_shape(s: &str) -> bool { (1..=400).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.' || b == b',' || b == b';') }
fn conid_text(s: &str) -> bool {
    let (id, ex) = s.split_once('@').map_or((s, None), |(a, b)| (a, Some(b)));
    (1..=10).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()) && ex.is_none_or(|e| (1..=16).contains(&e.len()) && e.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'.'))
}

/// What a file's answer is, for the scrub's file-scoped rules.
#[derive(Clone, Copy)]
pub struct Ctx<'a> { pub file: &'a str, pub ok: bool, /// The run's stock symbols and ISINs (its arguments).
    /// with every value the set keeps as public (`collect_public`).
    pub symbols: &'a [String] }
impl Ctx<'_> {
    /// A 2xx answer of a contract-data file: its contract facts are kept (PLAN Q4).
    fn contract(&self) -> bool { self.ok && ["secdef_", "trsrv_", "contract_info.", "rules."].iter().any(|p| self.file.starts_with(p)) }
    fn snapshot(&self) -> bool { self.file.starts_with("snapshot") }
}

/// Seeds an account id's fake: its U/DU/DF prefix kept (A27-3's tests still parse it), its digits faked.
pub fn seed_account(sc: &mut Scrubber, account: &str) -> Result<(), String> {
    if ibkr_account::parse(account).is_none() { return Err("the account list names an id that is no IBKR account".into()); }
    let cut = account.find(|c: char| c.is_ascii_digit()).unwrap_or(account.len());
    let fake = format!("{}{}", &account[..cut], sc.fake(&account[cut..], true, "/accounts")?);
    sc.pin(account, &fake);
    Ok(())
}

/// The scrubbed answer (or request body) of one file.
pub fn scrub_ibkr(sc: &mut Scrubber, cx: Ctx<'_>, mut v: Value) -> Result<Value, String> {
    walk(sc, cx, String::new(), None, &mut v)?;
    Ok(v)
}

fn walk(sc: &mut Scrubber, cx: Ctx<'_>, path: String, name: Option<&str>, v: &mut Value) -> Result<(), String> {
    match v {
        Value::Null | Value::Bool(_) => {}
        Value::Array(a) => for x in a { walk(sc, cx, format!("{path}/[]"), name, x)?; },
        Value::Object(m) => {
            for (k, mut x) in std::mem::take(m) {
                let key = key(sc, cx, &path, &k)?;
                // The path names the scrubbed key (the report is written); the rules read the original name.
                walk(sc, cx, format!("{path}/{key}"), Some(&k), &mut x)?;
                m.insert(key, x);
            }
        }
        Value::String(s) => { if let Some(new) = string(sc, cx, &path, name, s)? { *s = new; } }
        Value::Number(n) => {
            let text = n.to_string();
            if let Some(new) = number(sc, cx, &path, name, &text)? {
                *v = serde_json::from_str(&new).map_err(|_| format!("{}: a replaced number at {path} is no number", cx.file))?;
            }
        }
    }
    Ok(())
}

fn key(sc: &mut Scrubber, cx: Ctx<'_>, path: &str, k: &str) -> Result<String, String> {
    if let Some(fake) = sc.mapped(k).filter(|_| ibkr_account::parse(k).is_some()) { sc.count(cx.file, path, "key-account"); return Ok(fake); }
    let n = norm(k);
    let digits = !k.is_empty() && k.len() <= 5 && k.bytes().all(|b| b.is_ascii_digit());
    let kept = (!digits && IBKR_KEYS.contains(&n.as_str()))
        || k == "BASE" || (k.len() == 3 && k.bytes().all(|b| b.is_ascii_uppercase()))
        || (digits && cx.snapshot() && path == "/[]")
        || (cx.contract() && cx.symbols.iter().any(|s| s == k));
    if kept { return Ok(k.to_string()); }
    sc.count(cx.file, path, "key-replaced");
    sc.fake(k, false, path)
}

/// Whether a string kept by name and shape alone (PLAN Q4): a conid, an enum, a contract fact in a 2xx contract file.
fn public_str(cx: Ctx<'_>, n: &str, s: &str) -> bool {
    if CONIDS.contains(&n) && conid_text(s) { return true; }
    if enum_ok(n, s) { return true; }
    cx.contract() && match n {
        "symbol" | "ticker" | "localsymbol" => symbol_shape(s),
        "isin" => isin_shape(s),
        "exchange" | "listingexchange" | "allexchanges" | "validexchanges" => exchanges_shape(s),
        "countrycode" => s.len() == 2 && s.bytes().all(|b| b.is_ascii_uppercase()),
        "sizeincrement" | "pricemagnifier" | "increment" | "loweredge" => decimal(s),
        _ => false,
    }
}

/// Every string a file keeps by `public_str`, so the same text elsewhere in the set is kept too: it is public by
/// construction, and replacing it there would leave its original beside the kept copy (the leak check refuses that).
pub fn collect_public(cx: Ctx<'_>, name: Option<&str>, v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Array(a) => for x in a { collect_public(cx, name, x, out); },
        Value::Object(m) => for (k, x) in m { collect_public(cx, Some(k), x, out); },
        // Only a word spreads (an exchange, a symbol, an enum): never a number or an id-like digit string, which elsewhere
        // could be an amount or an order id that merely equals a contract fact.
        Value::String(s) if s.bytes().any(|b| b.is_ascii_alphabetic()) && public_str(cx, &name.map(norm).unwrap_or_default(), s) => out.push(s.clone()),
        _ => {}
    }
}

fn string(sc: &mut Scrubber, cx: Ctx<'_>, path: &str, name: Option<&str>, s: &str) -> Result<Option<String>, String> {
    let n = name.map(norm).unwrap_or_default();
    if let Some(fake) = sc.mapped(s).filter(|_| ibkr_account::parse(s).is_some()) { sc.count(cx.file, path, "account"); return Ok(Some(fake)); }
    // The run's own stocks and every value the set keeps as public stay as they are wherever they appear.
    if public_str(cx, &n, s) || cx.symbols.iter().any(|x| x == s) { return Ok(None); }
    if TIMES.contains(&n.as_str()) {
        if let Some(t) = sc.shift_str(s).or_else(|| sc.shift_compact(s)) { sc.count(cx.file, path, "shifted"); return Ok(Some(t)); }
    }
    if !s.chars().any(|c| c.is_ascii_alphanumeric()) || zero(s) { return Ok(None); }
    if s.chars().any(char::is_whitespace) {
        sc.dropped(s);
        sc.count(cx.file, path, "text");
        return Ok(Some("[text]".into()));
    }
    sc.count(cx.file, path, "replaced");
    sc.fake(s, false, path).map(Some)
}

fn number(sc: &mut Scrubber, cx: Ctx<'_>, path: &str, name: Option<&str>, text: &str) -> Result<Option<String>, String> {
    let n = name.map(norm).unwrap_or_default();
    if CONIDS.contains(&n.as_str()) && text.parse::<i64>().is_ok() { return Ok(None); }
    if SMALL.contains(&n.as_str()) && text.parse::<u64>().is_ok_and(|x| x < 10_000) { return Ok(None); }
    if cx.contract() && ["sizeincrement", "pricemagnifier", "increment", "loweredge"].contains(&n.as_str()) { return Ok(None); }
    if TIMES.contains(&n.as_str()) {
        if let Some(t) = sc.shift_epoch(text).or_else(|| sc.shift_compact(text)) { sc.count(cx.file, path, "shifted"); return Ok(Some(t)); }
    }
    if zero(text) { return Ok(None); }
    sc.count(cx.file, path, "replaced");
    sc.fake(text, true, path).map(Some)
}

// ---- the record flow

/// One file of the set: the request as sent and the raw answer.
#[derive(Clone, Debug)]
pub struct Got { pub file: String, pub x: Exchange }

/// What a run hands main: the report it prints, and each prompt's ids and text (for Jan's whitelist call; never written).
#[derive(Debug)]
pub struct Outcome { pub report: String, pub prompts: Vec<(Vec<String>, String)> }

pub struct Recorder<'a, T: Transport, C: Clock + ?Sized> {
    s: &'a Session<'a, Guard<T>, C>,
    g: &'a Guard<T>,
    got: Vec<Got>,
    not_recorded: Vec<String>,
    message_ids: Vec<String>,
    prompts: Vec<(Vec<String>, String)>,
}

fn failed(file: &str, e: &Error) -> String {
    let kind = match e {
        Error::LockTimeout => "the login stayed busy",
        Error::Transport(TransportError::Permanent(_)) => "refused locally",
        Error::Transport(TransportError::NotSent(_)) => "not sent",
        Error::Transport(TransportError::MaybeSent(_)) => "no answer",
        Error::Auth(_) => "the session token was refused",
        Error::Key(_) => "the key material does not work",
        _ => "failed",
    };
    format!("{file}: {kind}; nothing written")
}

impl<'a, T: Transport, C: Clock + ?Sized> Recorder<'a, T, C> {
    pub fn new(s: &'a Session<'a, Guard<T>, C>, g: &'a Guard<T>) -> Self {
        Self { s, g, got: vec![], not_recorded: vec![], message_ids: vec![], prompts: vec![] }
    }

    fn poisoned(&self) -> Result<(), String> {
        if self.g.poisoned() { Err("an answer held a credential, or the transport's redaction of one; nothing written".into()) } else { Ok(()) }
    }

    /// The last exchange of `method path` relayed since `from`, as `file`.
    fn take(&mut self, from: usize, file: &str, method: &str, path: &str) -> Result<Value, String> {
        self.poisoned()?;
        let x = self.g.log().into_iter().skip(from).rev().find(|x| x.method == method && x.path == path).ok_or(format!("{file}: not sent; nothing written"))?;
        let body = serde_json::from_str(&x.text).unwrap_or_else(|_| Value::String(x.text.clone()));
        self.got.push(Got { file: file.into(), x });
        Ok(body)
    }

    /// One read under the login's lock, recorded whatever its HTTP status.
    pub async fn read(&mut self, file: &str, method: &'static str, path: &str, query: Vec<(&'static str, String)>, body: Option<Value>) -> Result<Value, String> {
        let from = self.g.log().len();
        let r = {
            let held = self.s.lock().await.map_err(|e| failed(file, &e))?;
            self.s.signed_request(&held, method, path, query, body).await
        };
        match r { Ok(_) | Err(Error::Http { .. }) | Err(Error::Unreadable) => {}, Err(e) => { self.poisoned()?; return Err(failed(file, &e)); } }
        self.take(from, file, method, path)
    }

    /// The account list, and the one account the run reads: paper mode stops here unless it is a lone paper account.
    pub async fn accounts(&mut self, mode: Mode) -> Result<String, String> {
        let answer = self.read("accounts", "GET", ACCOUNTS_PATH, vec![], None).await?;
        let status = self.got.last().map_or(0, |g| g.x.status);
        if !(200..300).contains(&status) { return Err("accounts: the account list was not answered; nothing written".into()); }
        let listed = ibkr_account::listed(&answer);
        let ids: Vec<&str> = listed.iter().filter_map(Value::as_str).filter(|a| ibkr_account::parse(a).is_some()).collect();
        if ids.len() != listed.len() || ids.is_empty() { return Err("accounts: the list names no account, or an id that is no IBKR account; nothing written".into()); }
        if mode == Mode::Paper && !ids.iter().all(|a| a.starts_with("DU") || a.starts_with("DF")) {
            return Err("paper mode: this login lists a live account; no order is sent on it. Use the paper login".into());
        }
        if ids.len() != 1 { return Err("this login lists more than one account; Deltabadger reads a login with one account".into()); }
        Ok(ids[0].to_string())
    }

    /// One order POST through `Session::submit` (sent once), then each prompt confirmed (paper mode only) and recorded:
    /// `<name>.order`, `<name>.reply.N`. Returns IBKR's order id, if it gave one.
    pub async fn place(&mut self, mode: Mode, name: &str, account: &str, order: Value) -> Result<Option<String>, String> {
        let path = format!("/v1/api/iserver/account/{account}/orders");
        let from = self.g.log().len();
        let held = self.s.lock().await.map_err(|e| failed(name, &e))?;
        let r = self.s.submit(&held, &path, json!({ "orders": [order] }), Some(account)).await;
        if let Err(VenueError::Rejected(_) | VenueError::Transient(_)) = &r { if !self.g.log().iter().skip(from).any(|x| x.path == path) { return Err(format!("{name}.order: not sent; nothing written")); } }
        let mut answer = self.take(from, &format!("{name}.order"), "POST", &path)?;
        for n in 1..=MAX_REPLIES {
            if let Some(id) = rows(&answer).iter().find_map(|r| r.get("order_id").or_else(|| r.get("orderId")).filter(|v| !v.is_null()).map(text)) { return Ok(Some(id)); }
            let Some(prompt) = rows(&answer).into_iter().find(|r| r.get("id").is_some_and(|i| !i.is_null()) && r.get("message").is_some()).cloned() else { return Ok(None) };
            let ids: Vec<String> = prompt.get("messageIds").and_then(Value::as_array).map(|a| a.iter().map(text).collect()).unwrap_or_default();
            let message = match &prompt["message"] { Value::Array(m) => m.iter().map(text).collect::<Vec<_>>().join(" / "), m => text(m) };
            self.message_ids.extend(ids.iter().filter(|i| !self.message_ids.contains(i)).cloned().collect::<Vec<_>>());
            self.prompts.push((ids, message));
            if mode != Mode::Paper { return Err(format!("{name}: a prompt outside paper mode; nothing written")); }
            let reply = format!("/v1/api/iserver/reply/{}", text(&prompt["id"]));
            let from = self.g.log().len();
            let _ = self.s.submit(&held, &reply, json!({ "confirmed": true }), Some(account)).await;
            answer = self.take(from, &format!("{name}.reply.{n}"), "POST", &reply)?;
        }
        Ok(None)
    }

    fn status_of(answer: &Value) -> String { answer.get("order_status").or_else(|| answer.get("status")).map(text).unwrap_or_default() }

    /// `id`'s status until it is filled: `<name>.status.first` and `<name>.status.final`.
    pub async fn until_filled(&mut self, name: &str, id: &str) -> Result<(), String> {
        let path = format!("/v1/api/iserver/account/order/status/{id}");
        let mut first = None;
        for _ in 0..POLLS {
            let answer = self.read(&format!("{name}.status.final"), "GET", &path, vec![], None).await?;
            let last = self.got.pop().expect("just recorded");
            first.get_or_insert_with(|| Got { file: format!("{name}.status.first"), x: last.x.clone() });
            match Self::status_of(&answer).as_str() {
                "Filled" => { self.got.extend([first.expect("set above"), last]); return Ok(()); }
                "Cancelled" | "Inactive" | "Rejected" => return Err(format!("{name}: the order ended unfilled; nothing written")),
                _ => tokio::time::sleep(POLL_PAUSE).await,
            }
        }
        Err(format!("{name}: not filled after {POLLS} reads; run in US market hours (the paper order may still be open); nothing written"))
    }

    /// The cancel of this run's order `id`, after its own account read under the same lock (the Guard's paper check).
    pub async fn cancel(&mut self, name: &str, account: &str, id: &str) -> Result<(), String> {
        let path = format!("/v1/api/iserver/account/{account}/order/{id}");
        let from = self.g.log().len();
        let r = {
            let held = self.s.lock().await.map_err(|e| failed(name, &e))?;
            let accounts = self.s.signed_request(&held, "GET", ACCOUNTS_PATH, vec![], None).await;
            match accounts {
                Ok(a) if ibkr_account::names_only(&a, account) => self.s.signed_request(&held, "DELETE", &path, vec![], None).await,
                _ => return Err(format!("{name}: the account list before the cancel did not name the paper account alone; nothing written")),
            }
        };
        match r { Ok(_) | Err(Error::Http { .. }) | Err(Error::Unreadable) => {}, Err(e) => { self.poisoned()?; return Err(failed(name, &e)); } }
        self.take(from, name, "DELETE", &path).map(|_| ())
    }

    /// The reads every mode records about the account: positions (page 0 and the boundary), ledger, live orders twice,
    /// trades.
    async fn account_reads(&mut self, account: &str) -> Result<Value, String> {
        self.read("positions.0", "GET", &format!("/v1/api/portfolio/{account}/positions/0"), vec![], None).await?;
        self.read("positions.1", "GET", &format!("/v1/api/portfolio/{account}/positions/1"), vec![], None).await?;
        self.read("ledger", "GET", &format!("/v1/api/portfolio/{account}/ledger"), vec![], None).await?;
        let live = self.read("live_orders.1", "GET", LIVE_ORDERS_PATH, vec![], None).await?;
        let live = match self.read("live_orders.2", "GET", LIVE_ORDERS_PATH, vec![], None).await? { Value::Object(o) if !o.is_empty() => Value::Object(o), _ => live };
        let trades = self.read("trades", "GET", TRADES_PATH, vec![], None).await?;
        Ok(json!({ "live": live, "trades": trades }))
    }

    /// Read mode: every read for the account and for each stock (PLAN Q5).
    pub async fn read_mode(&mut self, stocks: &[Stock]) -> Result<(), String> {
        let account = self.accounts(Mode::Read).await?;
        let seen = self.account_reads(&account).await?;
        let first_id = seen["live"]["orders"].as_array().and_then(|o| o.iter().find_map(|r| r.get("orderId").or_else(|| r.get("order_id")).map(text)))
            .or_else(|| seen["trades"].as_array().and_then(|t| t.iter().find_map(|r| r.get("order_id").or_else(|| r.get("orderId")).map(text))))
            .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
        match first_id {
            Some(id) => { self.read("order_status", "GET", &format!("/v1/api/iserver/account/order/status/{id}"), vec![], None).await?; }
            None => self.not_recorded.push("order_status.json (no live order or trade lists an id)".into()),
        }
        for s in stocks { self.stock_reads(&account, s).await?; }
        let both: Vec<i64> = stocks.iter().map(|s| s.conid).collect();
        self.read("pa_transactions.both", "POST", PA_TRANSACTIONS_PATH, vec![], Some(json!({ "acctIds": [account], "conids": both, "currency": "USD", "days": PA_DAYS }))).await?;
        Ok(())
    }

    async fn stock_reads(&mut self, account: &str, s: &Stock) -> Result<(), String> {
        let (t, conid) = (s.tag, s.conid.to_string());
        let snap = |fields: &str| vec![("conids", conid.clone()), ("fields", fields.to_string())];
        self.read(&format!("snapshot.{t}.1"), "GET", SNAPSHOT_PATH, snap(SNAPSHOT_FIELDS), None).await?;
        tokio::time::sleep(PREFLIGHT_PAUSE).await;
        self.read(&format!("snapshot.{t}.2"), "GET", SNAPSHOT_PATH, snap(SNAPSHOT_FIELDS), None).await?;
        self.read(&format!("snapshot_figures.{t}"), "GET", SNAPSHOT_PATH, snap("31,6509"), None).await?;
        self.read(&format!("history.{t}"), "GET", HISTORY_PATH, vec![("bar", "1d".into()), ("conid", conid.clone()), ("period", "30d".into())], None).await?;
        for (side, buy) in [("buy", true), ("sell", false)] {
            self.read(&format!("rules.{t}.{side}"), "POST", RULES_PATH, vec![], Some(json!({ "conid": s.conid, "isBuy": buy }))).await?;
        }
        let currency = self.currency_of(t);
        self.read(&format!("pa_transactions.{t}"), "POST", PA_TRANSACTIONS_PATH, vec![], Some(json!({ "acctIds": [account], "conids": [s.conid], "currency": currency, "days": PA_DAYS }))).await?;
        self.read(&format!("secdef_search.{t}.symbol"), "GET", "/v1/api/iserver/secdef/search", vec![("secType", "STK".into()), ("symbol", s.symbol.clone())], None).await?;
        self.read(&format!("secdef_search.{t}.isin"), "GET", "/v1/api/iserver/secdef/search", vec![("secType", "STK".into()), ("symbol", s.isin.clone())], None).await?;
        self.read(&format!("secdef_info.{t}"), "GET", "/v1/api/iserver/secdef/info", vec![("conid", conid.clone()), ("sectype", "STK".into())], None).await?;
        self.read(&format!("trsrv_stocks.{t}"), "GET", "/v1/api/trsrv/stocks", vec![("symbols", s.symbol.clone())], None).await?;
        self.read(&format!("trsrv_secdef.{t}"), "GET", "/v1/api/trsrv/secdef", vec![("conids", conid.clone())], None).await?;
        self.read(&format!("contract_info.{t}"), "GET", &format!("/v1/api/iserver/contract/{conid}/info"), vec![], None).await?;
        Ok(())
    }

    /// The currency the stock's snapshot or rules named (PA reads in it); USD when none.
    fn currency_of(&self, tag: &str) -> String {
        self.got.iter().filter(|g| g.file.starts_with(&format!("rules.{tag}.")) || g.file.starts_with(&format!("snapshot.{tag}.")))
            .filter_map(|g| serde_json::from_str::<Value>(&g.x.text).ok())
            .find_map(|v| { let mut c = None; visit(&v, "", &mut |text, path| if c.is_none() && (path.ends_with("/currency") || path.ends_with("/cashCcy")) && enum_ok("currency", text) && text != "BASE" { c = Some(text.to_string()); }); c })
            .unwrap_or_else(|| "USD".into())
    }

    /// Paper mode (PLAN Q5): a market buy to filled, a far limit buy and its cancel, a market sell, optionally an order on
    /// a non-tradable conid, then the account reads.
    pub async fn paper_mode(&mut self, stock: &Stock, limit: &str, untradable: Option<i64>) -> Result<(), String> {
        let account = self.accounts(Mode::Paper).await?;
        let order = |order_type: &str, side: &str, conid: i64| {
            let mut o = json!({ "conid": conid, "orderType": order_type, "side": side, "quantity": 1, "tif": "DAY", "cOID": format!("a27-{}", uuid::Uuid::new_v4().simple()) });
            if order_type == "LMT" { o["price"] = serde_json::from_str(limit).expect("--limit was checked as a number"); }
            o
        };
        let id = self.place(Mode::Paper, "buy_market", &account, order("MKT", "BUY", stock.conid)).await?.ok_or("buy_market: IBKR gave no order id; nothing written")?;
        self.until_filled("buy_market", &id).await?;
        let id = self.place(Mode::Paper, "buy_limit", &account, order("LMT", "BUY", stock.conid)).await?.ok_or("buy_limit: IBKR gave no order id; nothing written")?;
        self.read("buy_limit.status", "GET", &format!("/v1/api/iserver/account/order/status/{id}"), vec![], None).await?;
        self.cancel("buy_limit.cancel", &account, &id).await?;
        self.read("buy_limit.status_after_cancel", "GET", &format!("/v1/api/iserver/account/order/status/{id}"), vec![], None).await?;
        let id = self.place(Mode::Paper, "sell_market", &account, order("MKT", "SELL", stock.conid)).await?.ok_or("sell_market: IBKR gave no order id; nothing written")?;
        self.until_filled("sell_market", &id).await?;
        match untradable {
            Some(conid) => { self.place(Mode::Paper, "untradable", &account, order("MKT", "BUY", conid)).await?; }
            None => self.not_recorded.push("untradable.order.json (no --untradable conid given)".into()),
        }
        self.account_reads(&account).await?;
        Ok(())
    }

    /// The scrub, the leak check and only then the write of the whole set into `out/<mode>/` (PLAN Q4, Q5).
    pub fn finish(self, mode: Mode, stocks: &[Stock], unauthorized: Option<(u16, String)>, secrets: &[String], sc: &mut Scrubber, out: &Path) -> Result<Outcome, String> {
        self.poisoned()?;
        let mut got = self.got;
        if let Some((status, body)) = unauthorized {
            if self.g.credential_in(&body) { return Err("unauthorized: the answer held a credential; nothing written".into()); }
            got.push(Got { file: "unauthorized".into(), x: Exchange { method: "GET", path: ACCOUNTS_PATH.into(), query: vec![], body: None, status, text: body } });
        }
        let parsed: Vec<(Value, Option<Value>)> = got.iter().map(|g| (serde_json::from_str(&g.x.text).unwrap_or_else(|_| Value::String(g.x.text.clone())), g.x.body.clone())).collect();
        let mut symbols: Vec<String> = stocks.iter().flat_map(|s| [s.symbol.clone(), s.isin.clone()]).collect();
        for (g, (body, _)) in got.iter().zip(&parsed) {
            collect_public(Ctx { file: &g.file, ok: (200..300).contains(&g.x.status), symbols: &[] }, None, body, &mut symbols);
        }
        symbols.sort();
        symbols.dedup();
        for (body, request) in &parsed {
            if long_integer(&body.to_string()) { return Err("an answer holds an integer of 20+ digits, which would be lossy; nothing written".into()); }
            sc.reserve(body);
            if let Some(r) = request { sc.reserve(r); }
        }
        // The accounts are seeded first, so every later sight of them maps to the same fake.
        for (g, (body, _)) in got.iter().zip(&parsed) {
            if g.file == "accounts" && (200..300).contains(&g.x.status) {
                for a in ibkr_account::listed(body).iter().filter_map(Value::as_str) { seed_account(sc, a)?; }
            }
        }
        let conids: Vec<String> = stocks.iter().map(|s| s.conid.to_string()).collect();
        let mut files = Vec::new();
        let mut entries = Vec::new();
        for (g, (body, request)) in got.iter().zip(parsed) {
            let cx = Ctx { file: &g.file, ok: (200..300).contains(&g.x.status), symbols: &symbols };
            let body = scrub_ibkr(sc, cx, body)?;
            let req_file = format!("{} request", g.file);
            let request = request.map(|r| scrub_ibkr(sc, Ctx { file: &req_file, ok: false, symbols: &symbols }, r)).transpose()?;
            files.push((format!("{}.json", g.file), body));
            entries.push((g, request));
        }
        // Paths and queries last, once every id an answer named is mapped.
        let mut manifest_files = Vec::new();
        // The parts of a path or query not written by this tool (the allow-list's literals and its constant params can
        // name no one): only these, with the request bodies, go through the leak check.
        let mut dynamic: Vec<String> = Vec::new();
        for (g, request) in entries {
            let mut path = Vec::new();
            for seg in g.x.path.split('/') {
                path.push(match sc.mapped(seg) {
                    Some(fake) => { dynamic.push(fake.clone()); fake }
                    None if !seg.bytes().any(|b| b.is_ascii_digit()) || seg.len() <= 2 || conids.iter().any(|c| c == seg) => seg.to_string(),
                    None => return Err(format!("{}: a path segment of the request is neither public nor mapped; nothing written", g.file)),
                });
            }
            let mut query = Map::new();
            for (k, v) in &g.x.query {
                let v = match *k {
                    "conids" | "conid" | "fields" | "bar" | "period" | "secType" | "sectype" => v.clone(),
                    "symbol" | "symbols" if symbols.contains(v) || stocks.iter().any(|s| s.isin == *v) => v.clone(),
                    _ => { let f = sc.fake(v, false, "query")?; dynamic.push(f.clone()); f }
                };
                query.insert(k.to_string(), Value::String(v));
            }
            manifest_files.push(json!({ "file": format!("{}.json", g.file), "method": g.x.method, "path": path.join("/"), "query": query, "request": request, "status": g.x.status }));
        }
        let report = sc.report_text();
        let recorded_at = sc.shift_str(&Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)).expect("an RFC 3339 time shifts");
        let scrubbed: Map<String, Value> = sc.report.iter().map(|(f, kinds)| (f.clone(), json!(kinds))).collect();
        let manifest = json!({ "exchange": "ibkr", "mode": mode.dir(), "recorded_at": recorded_at, "files": manifest_files,
                               "not_recorded": self.not_recorded, "message_ids": self.message_ids, "scrubbed": scrubbed });
        let needles: Vec<&str> = secrets.iter().map(String::as_str).collect();
        // The whole of every file and of the manifest (paths and queries included) is checked for a credential.
        if files.iter().map(|(_, v)| v).chain([&manifest]).any(|v| self.g.credential_in(&v.to_string()) || needles.iter().any(|n| v.to_string().contains(n))) {
            return Err("a credential would be written; nothing written".into());
        }
        for (name, v) in &files { sc.leak_check(name, v, &needles)?; }
        let mut checked = manifest.clone();
        for f in checked["files"].as_array_mut().expect("files is a list") { f["path"] = Value::Null; f["query"] = Value::Null; }
        checked["dynamic"] = json!(dynamic);
        sc.leak_check("manifest.json", &checked, &needles)?;
        sc.leak_check("report", &Value::String(report.clone()), &needles)?;
        let statuses: Vec<String> = got.iter().filter(|g| !(200..300).contains(&g.x.status)).map(|g| format!("{} {}", g.file, g.x.status)).collect();
        files.push(("manifest.json".into(), manifest));
        let dir = out.join(mode.dir());
        write_set(&dir, &files)?;
        let mut text = format!("wrote {} ({} files)\nchanged fields:\n{report}", dir.display(), files.len());
        if !statuses.is_empty() { text += &format!("\nnot 2xx (recorded): {}", statuses.join(", ")); }
        if !self.message_ids.is_empty() { text += &format!("\nprompt message ids: {}", self.message_ids.join(", ")); }
        if !self.not_recorded.is_empty() { text += &format!("\nnot recorded: {}", self.not_recorded.join("; ")); }
        Ok(Outcome { report: text, prompts: self.prompts })
    }
}

/// One run: the mode's reads (and paper orders), then `finish`.
pub async fn record<T: Transport, C: Clock + ?Sized>(s: &Session<'_, Guard<T>, C>, g: &Guard<T>, cmd: &Command, unauthorized: Option<(u16, String)>,
                                                    secrets: &[String], sc: &mut Scrubber, out: &Path) -> Result<Outcome, String> {
    let mut r = Recorder::new(s, g);
    match cmd.mode {
        Mode::Read => r.read_mode(&cmd.stocks).await?,
        Mode::Paper => r.paper_mode(&cmd.stocks[0], cmd.limit.as_deref().unwrap_or_default(), cmd.untradable).await?,
    }
    r.finish(cmd.mode, &cmd.stocks, unauthorized, secrets, sc, out)
}
